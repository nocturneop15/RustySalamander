//! A file as the viewer shows it: its text in an encoding (detected, or
//! chosen), colored by its syntax, or its bytes as hex. It can follow the
//! file as it grows (a log being written), like `tail -f`. No window here,
//! so it can be tested.

use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::highlight::{self, Span};

use sal_core::encoding::{self, display_path, TextEncoding};
use sal_plugin_api::tr;

/// Files larger than this are shown truncated.
pub const MAX_BYTES: u64 = 32 << 20;
/// Word wrap lays out all text at once, so it's limited to smaller files.
pub const MAX_WRAP_BYTES: usize = 2 << 20;
/// How much of the end of what was read is read again when the file grew,
/// to tell a file that grew from one that was replaced by another.
const CHECK_BYTES: usize = 64;

pub struct Doc {
    pub title: String,
    path: PathBuf,
    bytes: Vec<u8>,
    /// Where in the file `bytes` start: past 0 when only its end is shown.
    start: u64,
    /// The whole file's length; more than `bytes` when it was too large.
    pub file_len: u64,
    pub encoding: TextEncoding,
    bom: usize,
    /// Shared with a search running beside the window (copied only when
    /// the file grows while one runs).
    pub lines: std::sync::Arc<Vec<String>>,
    /// How many of `lines` end with a line break: all of them, or all but
    /// the last. They stay as they are when the file grows.
    complete: usize,
    pub hex: bool,
    pub wrap: bool,
    /// Why the file couldn't be read.
    pub error: Option<String>,
    /// The syntax its text is colored by (detected, or chosen); `None` for
    /// plain text.
    pub syntax: Option<String>,
    /// The colors of the lines, as far as they are made.
    spans: Option<Spans>,
    /// Changes with the lines and the syntax (search results made for
    /// another one are out of date).
    version: u64,
    /// Changes when the lines change other than by more being added, so
    /// that no colors made before can be kept.
    epoch: u64,
    /// Coloring running in the background.
    job: Option<Job>,
}

/// The colors of every line, one list of spans per line.
type LineSpans = Vec<Vec<Span>>;

/// What a coloring thread made: the colors and the state to carry on
/// from, or `None` when they could not be made.
type Colored = Option<(LineSpans, highlight::State)>;

/// The lines' colors, as far as they are made.
struct Spans {
    dark: bool,
    epoch: u64,
    /// The colors of the first lines.
    rows: LineSpans,
    /// The state after the first `.0` lines, to color the lines after them
    /// from; `None` when no colors could be made.
    resume: Option<(usize, highlight::State)>,
}

/// Coloring the lines from `from` on, on a thread of its own.
struct Job {
    dark: bool,
    epoch: u64,
    from: usize,
    /// The line it keeps the state at (the lines up to it end with a line
    /// break, so they stay as they are).
    resume: usize,
    /// How many lines from the start are still as they were colored: the
    /// last line may have grown since.
    keep: usize,
    stop: Arc<AtomicBool>,
    /// Filled when the thread ends: the colors, or `None` when they could
    /// not be made.
    result: Arc<Mutex<Option<Colored>>>,
}

/// Where coloring a document stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colors {
    /// Plain text, or hex.
    Off,
    /// Too large to color ([`highlight::MAX_BYTES`]).
    TooLarge,
    /// Being made; plain until then.
    Working,
    /// Made (the lines added since, being made, are plain for a while).
    Ready,
}

/// What was read of a file.
struct Loaded {
    bytes: Vec<u8>,
    start: u64,
    len: u64,
}

/// Read `path`: its first [`MAX_BYTES`], or with `tail` its last.
fn load(path: &Path, tail: bool) -> io::Result<Loaded> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let mut start = if tail { len.saturating_sub(MAX_BYTES) } else { 0 };
    // UTF-16 text stays in its pairs of bytes
    start += start % 2;
    f.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    f.take(MAX_BYTES).read_to_end(&mut bytes)?;
    Ok(Loaded { len: len.max(start + bytes.len() as u64), bytes, start })
}

/// Decoded text as the viewer's lines: tabs as spaces, right-to-left text
/// in the order it is seen.
fn to_lines(text: &str) -> impl Iterator<Item = String> + '_ {
    text.lines().map(|l| encoding::visual_order(&l.replace('\t', "    "), false).into_owned())
}

impl Doc {
    /// Read `path` (at most [`MAX_BYTES`]): its text in the encoding detected,
    /// its syntax by name or first line; binary files open as hex.
    pub fn open(path: &Path) -> Doc {
        let title = format!("{} - {}", encoding::display_name(&display_path(path)), tr("Viewer"));
        let mut d = Doc {
            title,
            path: path.to_path_buf(),
            bytes: Vec::new(),
            start: 0,
            file_len: 0,
            encoding: TextEncoding::Utf8,
            bom: 0,
            lines: Default::default(),
            complete: 0,
            hex: false,
            wrap: false,
            error: None,
            syntax: None,
            spans: None,
            version: 0,
            epoch: 0,
            job: None,
        };
        match load(path, false) {
            Ok(l) => {
                (d.bytes, d.start, d.file_len) = (l.bytes, l.start, l.len);
                let (enc, bom) = encoding::detect(&d.bytes);
                d.bom = bom;
                d.set_encoding(enc);
                // binary files open in hex mode
                d.hex = d.bytes.iter().take(8192).any(|&b| b == 0) && !d.utf16();
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                d.syntax = highlight::detect(&name, d.lines.first().map(String::as_str).unwrap_or(""));
            }
            Err(e) => d.error = Some(e.to_string()),
        }
        d
    }

    fn utf16(&self) -> bool {
        matches!(self.encoding, TextEncoding::Utf16Le | TextEncoding::Utf16Be)
    }

    /// Show the text in `enc`.
    pub fn set_encoding(&mut self, enc: TextEncoding) {
        self.encoding = enc;
        self.decode();
    }

    /// Make the lines from all the bytes again.
    fn decode(&mut self) {
        let enc = self.encoding;
        let bom = if matches!((enc, self.bom), (TextEncoding::Utf8, 3) | (TextEncoding::Utf16Le | TextEncoding::Utf16Be, 2)) { self.bom } else { 0 };
        let text = encoding::decode(&self.bytes[bom.min(self.bytes.len())..], enc);
        self.complete = text.matches('\n').count();
        self.lines = std::sync::Arc::new(to_lines(&text).collect());
        self.version += 1;
        self.epoch += 1;
    }

    /// Color the text by `syntax` (`None`: plain text).
    pub fn set_syntax(&mut self, syntax: Option<String>) {
        self.syntax = syntax;
        self.version += 1;
        self.epoch += 1;
    }

    /// Start following the file: show its end when only its start was read
    /// (it was too large), and what was added since. Whether anything changed.
    pub fn follow(&mut self) -> bool {
        if self.start == 0 && self.file_len > self.bytes.len() as u64 {
            return self.reload();
        }
        self.poll()
    }

    /// Show what was added to the file since it was read; read it again
    /// when it got shorter or another file took its place (a log started
    /// anew). Whether anything changed. A file that can't be read just now
    /// (deleted, locked) leaves the text as it is.
    pub fn poll(&mut self) -> bool {
        if self.error.is_some() {
            return self.reload();
        }
        let Ok(len) = std::fs::metadata(&self.path).map(|m| m.len()) else { return false };
        let end = self.start + self.bytes.len() as u64;
        if len == end {
            return false;
        }
        if len < end || len - end > MAX_BYTES {
            return self.reload();
        }
        // what grew, and the end of what was read before, to see it's the same file
        let k = self.bytes.len().min(CHECK_BYTES);
        let read = || -> io::Result<Vec<u8>> {
            let mut f = std::fs::File::open(&self.path)?;
            f.seek(SeekFrom::Start(end - k as u64))?;
            let mut buf = Vec::new();
            f.take(len - end + k as u64).read_to_end(&mut buf)?;
            Ok(buf)
        };
        let Ok(buf) = read() else { return false };
        if buf.len() < k || buf[..k] != self.bytes[self.bytes.len() - k..] {
            return self.reload();
        }
        if buf.len() == k {
            return false;
        }
        self.append(&buf[k..]);
        self.file_len = self.start + self.bytes.len() as u64;
        true
    }

    /// Read the file again, its end when it is too large. Whether it could be.
    fn reload(&mut self) -> bool {
        let Ok(l) = load(&self.path, true) else { return false };
        (self.bytes, self.start, self.file_len, self.error) = (l.bytes, l.start, l.len, None);
        self.bom = if self.start == 0 { encoding::detect(&self.bytes).1 } else { 0 };
        // from the middle of the file: from its first whole line
        if self.start > 0 && !self.utf16() {
            if let Some(p) = self.bytes.iter().position(|&b| b == b'\n') {
                self.bytes.drain(..=p);
                self.start += p as u64 + 1;
            }
        }
        self.decode();
        true
    }

    /// The file grew by `new`: only its last line (when it had no line
    /// break yet) and the lines after are made anew, and their colors.
    fn append(&mut self, new: &[u8]) {
        let old = self.bytes.len();
        self.bytes.extend_from_slice(new);
        if self.bytes.len() as u64 > MAX_BYTES {
            return self.drop_front();
        }
        // UTF-16 line breaks aren't single bytes: all the text again
        let cut = if self.utf16() { None } else { self.bytes[..old].iter().rposition(|&b| b == b'\n').map(|p| p + 1) };
        let Some(cut) = cut else { return self.decode() };
        let kept = self.complete;
        std::sync::Arc::make_mut(&mut self.lines).truncate(kept);
        let text = encoding::decode(&self.bytes[cut..], self.encoding);
        self.complete += text.matches('\n').count();
        std::sync::Arc::make_mut(&mut self.lines).extend(to_lines(&text));
        // the colors of the lines kept stay
        if let Some(s) = self.spans.as_mut().filter(|s| s.epoch == self.epoch) {
            s.rows.truncate(kept);
        }
        if let Some(j) = &mut self.job {
            j.keep = j.keep.min(kept);
        }
        self.version += 1;
    }

    /// Too much text: the oldest quarter goes, from a line's start.
    fn drop_front(&mut self) {
        let mut cut = self.bytes.len() - MAX_BYTES as usize * 3 / 4;
        cut += cut % 2;
        if !self.utf16() {
            if let Some(p) = self.bytes[cut..].iter().position(|&b| b == b'\n') {
                cut += p + 1;
            }
        }
        self.bytes.drain(..cut);
        self.start += cut as u64;
        self.bom = 0;
        self.decode();
    }

    /// The colors of line `row`, once they are made.
    pub fn line_spans(&self, row: usize) -> Option<&[Span]> {
        match &self.spans {
            Some(s) if s.epoch == self.epoch => s.rows.get(row).map(Vec::as_slice),
            _ => None,
        }
    }

    /// Whether some lines have their colors (others may be made still).
    fn has_spans(&self) -> bool {
        self.spans.as_ref().is_some_and(|s| s.epoch == self.epoch && !s.rows.is_empty())
    }

    /// Keep the colors up to date for the dark or the light theme: start
    /// coloring in the background when needed (`wake` is called when it is
    /// done), take the result when it is there. Lines added to the file are
    /// colored from where the coloring of the lines before them ended.
    pub fn colors(&mut self, dark: bool, wake: impl Fn() + Send + 'static) -> Colors {
        let Some(syntax) = self.syntax.clone().filter(|_| !self.hex && self.error.is_none()) else {
            self.cancel();
            return Colors::Off;
        };
        if self.bytes.len() > highlight::MAX_BYTES {
            self.cancel();
            return Colors::TooLarge;
        }
        if self.job.as_ref().is_some_and(|j| j.dark != dark || j.epoch != self.epoch) {
            self.cancel();
        }
        if let Some(job) = &self.job {
            let done = job.result.lock().unwrap().take();
            let Some(result) = done else {
                return if self.has_spans() { Colors::Ready } else { Colors::Working };
            };
            let job = self.job.take().unwrap();
            self.finish(job, result);
        }
        let fresh = self.spans.as_ref().filter(|s| s.dark == dark && s.epoch == self.epoch);
        let (from, state) = match fresh {
            Some(Spans { resume: None, .. }) => return Colors::Off,
            Some(s) if s.rows.len() >= self.lines.len() => return Colors::Ready,
            Some(Spans { resume: Some((n, st)), .. }) => (*n, Some(st.clone())),
            None => (0, None),
        };
        let resume = self.complete.max(from);
        let job = Job {
            dark,
            epoch: self.epoch,
            from,
            resume,
            keep: usize::MAX,
            stop: Arc::new(AtomicBool::new(false)),
            result: Arc::new(Mutex::new(None)),
        };
        let (lines, stop, result) = (self.lines[from..].to_vec(), job.stop.clone(), job.result.clone());
        let spawned = std::thread::Builder::new().name("viewer colors".into()).spawn(move || {
            let spans = highlight::highlight_from(&lines, &syntax, dark, state, resume - from, &stop);
            if !stop.load(Ordering::Relaxed) {
                *result.lock().unwrap() = Some(spans);
                wake();
            }
        });
        if spawned.is_err() {
            return Colors::Off;
        }
        self.job = Some(job);
        if self.has_spans() {
            Colors::Ready
        } else {
            Colors::Working
        }
    }

    /// Take the colors `job` made.
    fn finish(&mut self, job: Job, result: Colored) {
        let mut s = match self.spans.take() {
            Some(s) if s.dark == job.dark && s.epoch == job.epoch => s,
            _ => Spans { dark: job.dark, epoch: job.epoch, rows: Vec::new(), resume: None },
        };
        match result {
            Some((rows, state)) => {
                s.rows.truncate(job.from);
                s.rows.extend(rows);
                s.rows.truncate(job.keep);
                s.resume = Some((job.resume, state));
            }
            None => (s.rows, s.resume) = (Vec::new(), None),
        }
        self.spans = Some(s);
    }

    /// Stop the coloring that runs.
    fn cancel(&mut self) {
        if let Some(job) = self.job.take() {
            job.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Changes whenever the lines or their colors do (search results made for
    /// another one are out of date).
    pub fn version(&self) -> u64 {
        self.version
    }

    /// The bytes read (at most [`MAX_BYTES`]).
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Where in the file the bytes shown start: past 0 when only its end is.
    pub fn start(&self) -> u64 {
        self.start
    }

    /// Whether word wrap can be turned on.
    pub fn can_wrap(&self) -> bool {
        self.bytes.len() <= MAX_WRAP_BYTES
    }

    /// The number of hex rows (16 bytes each).
    pub fn hex_rows(&self) -> usize {
        self.bytes.len().div_ceil(16)
    }

    /// Hex row `row`: offset in the file, 16 bytes in hex, the same as text.
    pub fn hex_line(&self, row: usize) -> String {
        let start = row * 16;
        let chunk = &self.bytes[start..(start + 16).min(self.bytes.len())];
        let mut s = format!("{:08X}  ", self.start + start as u64);
        for i in 0..16 {
            match chunk.get(i) {
                Some(b) => s.push_str(&format!("{b:02X} ")),
                None => s.push_str("   "),
            }
            if i == 7 {
                s.push(' ');
            }
        }
        s.push(' ');
        s.extend(chunk.iter().map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' }));
        s
    }
}

impl Drop for Doc {
    /// The window closed: coloring that still runs stops.
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_hex() {
        let t = tempfile::tempdir().unwrap();
        let utf8 = t.path().join("zpráva 🦎.txt");
        std::fs::write(&utf8, "\u{FEFF}Dobrý den\n\tžluťoučký kůň\n").unwrap();
        let d = Doc::open(&utf8);
        assert_eq!((d.encoding, d.hex, d.error.as_deref()), (TextEncoding::Utf8, false, None));
        assert_eq!(*d.lines, ["Dobrý den", "    žluťoučký kůň"], "the BOM is left out, tabs are spaces");
        assert!(d.title.ends_with("zpráva 🦎.txt - Viewer"), "{}", d.title);

        // UTF-16 has zero bytes but is text
        let utf16 = t.path().join("u16.txt");
        let mut b = vec![0xFF, 0xFE];
        b.extend("ahoj".encode_utf16().flat_map(u16::to_le_bytes));
        std::fs::write(&utf16, b).unwrap();
        let d = Doc::open(&utf16);
        assert_eq!((d.encoding, d.hex, d.lines.to_vec()), (TextEncoding::Utf16Le, false, vec!["ahoj".to_owned()]));

        // binary files open as hex
        let bin = t.path().join("data.bin");
        std::fs::write(&bin, (0u8..20).collect::<Vec<_>>()).unwrap();
        let mut d = Doc::open(&bin);
        assert!(d.hex);
        assert_eq!((d.hex_rows(), d.len(), d.file_len), (2, 20, 20));
        assert_eq!(d.hex_line(0), "00000000  00 01 02 03 04 05 06 07  08 09 0A 0B 0C 0D 0E 0F  ................");
        assert_eq!(d.hex_line(1), format!("00000010  10 11 12 13{}  ....", " ".repeat(3 * 12 + 1)));
        // another encoding on demand
        d.set_encoding(TextEncoding::Utf16Le);
        assert_eq!(d.encoding, TextEncoding::Utf16Le);

        let missing = Doc::open(&t.path().join("není.txt"));
        assert!(missing.error.is_some());
    }

    /// Wait for the colors to be made.
    fn wait(d: &mut Doc, dark: bool) -> Colors {
        for _ in 0..1000 {
            match d.colors(dark, || {}) {
                Colors::Working => std::thread::sleep(std::time::Duration::from_millis(5)),
                Colors::Ready if d.job.is_some() => std::thread::sleep(std::time::Duration::from_millis(5)),
                other => return other,
            }
        }
        panic!("never colored");
    }

    /// A JSON file is colored in the background; another syntax or the
    /// other theme colors it again; plain text and hex aren't colored.
    #[test]
    fn colors_in_the_background() {
        let t = tempfile::tempdir().unwrap();
        let json = t.path().join("data.json");
        std::fs::write(&json, "{\n  \"a\": [1, true, \"x\"]\n}\n").unwrap();
        let mut d = Doc::open(&json);
        assert_eq!(d.syntax.as_deref(), Some("JSON"));
        assert!(d.line_spans(1).is_none(), "not before they are made");
        assert_eq!(wait(&mut d, true), Colors::Ready);
        let dark = d.line_spans(1).unwrap().to_vec();
        assert!(dark.len() > 3);
        assert_eq!(wait(&mut d, false), Colors::Ready);
        assert_ne!(d.line_spans(1).unwrap(), dark.as_slice(), "the light theme");
        d.set_syntax(Some("YAML".into()));
        assert!(d.line_spans(1).is_none(), "old colors go with the syntax");
        assert_eq!(wait(&mut d, false), Colors::Ready);
        d.set_syntax(None);
        assert_eq!(d.colors(false, || {}), Colors::Off);
        d.set_syntax(Some("JSON".into()));
        d.hex = true;
        assert_eq!(d.colors(false, || {}), Colors::Off);

        let txt = t.path().join("a.txt");
        std::fs::write(&txt, "text").unwrap();
        assert_eq!(Doc::open(&txt).syntax, None);
    }

    fn add(path: &Path, text: &str) {
        use std::io::Write;
        std::fs::OpenOptions::new().append(true).open(path).unwrap().write_all(text.as_bytes()).unwrap();
    }

    /// Following a log: lines written to it are added, a line written in
    /// pieces is made whole, a log started anew is read anew.
    #[test]
    fn follows_a_growing_file() {
        let t = tempfile::tempdir().unwrap();
        let log = t.path().join("app.log");
        std::fs::write(&log, "\u{FEFF}první\ndruhý\n").unwrap();
        let mut d = Doc::open(&log);
        assert!(!d.follow(), "nothing new");
        add(&log, "tře");
        assert!(d.poll());
        assert_eq!(*d.lines, ["první", "druhý", "tře"]);
        add(&log, "tí\nčtvrtý\n");
        assert!(d.poll());
        assert_eq!(*d.lines, ["první", "druhý", "třetí", "čtvrtý"]);
        assert_eq!((d.file_len, d.len() as u64), (std::fs::metadata(&log).unwrap().len(), d.file_len));
        let v = d.version();
        assert!(!d.poll());
        assert_eq!(d.version(), v, "no change, no new version");

        // a character split between two writes
        let kun = "kůň\n".as_bytes();
        std::fs::OpenOptions::new().append(true).open(&log).map(|mut f| io::Write::write_all(&mut f, &kun[..2])).unwrap().unwrap();
        assert!(d.poll());
        std::fs::OpenOptions::new().append(true).open(&log).map(|mut f| io::Write::write_all(&mut f, &kun[2..])).unwrap().unwrap();
        assert!(d.poll());
        assert_eq!(d.lines.last().unwrap(), "kůň");

        // shorter: read anew
        std::fs::write(&log, "nový\n").unwrap();
        assert!(d.poll());
        assert_eq!(*d.lines, ["nový"]);
        // replaced by a longer one: read anew too
        std::fs::write(&log, "jiný soubor, delší než ten předtím\n").unwrap();
        assert!(d.poll());
        assert_eq!(*d.lines, ["jiný soubor, delší než ten předtím"]);

        // deleted: what was shown stays
        std::fs::remove_file(&log).unwrap();
        assert!(!d.poll());
        assert_eq!(d.lines.len(), 1);

        // UTF-16 grows too
        let u16 = t.path().join("u16.log");
        let enc = |s: &str| s.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>();
        std::fs::write(&u16, [vec![0xFF, 0xFE], enc("a\n")].concat()).unwrap();
        let mut d = Doc::open(&u16);
        std::fs::OpenOptions::new().append(true).open(&u16).map(|mut f| io::Write::write_all(&mut f, &enc("b\n"))).unwrap().unwrap();
        assert!(d.poll());
        assert_eq!(*d.lines, ["a", "b"]);
    }

    /// Lines added to a colored file keep the colors of the lines before
    /// them, and are colored as if the whole file had been.
    #[test]
    fn colors_carry_on_as_the_file_grows() {
        let t = tempfile::tempdir().unwrap();
        let py = t.path().join("run.py");
        std::fs::write(&py, "a = 1\ns = \"\"\"long\n").unwrap();
        let mut d = Doc::open(&py);
        assert_eq!(wait(&mut d, true), Colors::Ready);
        add(&py, "still a string\n\"\"\"\nb = 2");
        assert!(d.poll());
        assert!(d.line_spans(1).is_some(), "the old lines keep their colors");
        assert!(d.line_spans(2).is_none(), "the new ones get theirs");
        assert_eq!(wait(&mut d, true), Colors::Ready);
        add(&py, "0\n");
        assert!(d.poll());
        assert_eq!(wait(&mut d, true), Colors::Ready);
        let mut whole = Doc::open(&py);
        assert_eq!(wait(&mut whole, true), Colors::Ready);
        for i in 0..d.lines.len() {
            assert_eq!(d.line_spans(i), whole.line_spans(i), "line {i}: {}", d.lines[i]);
        }
    }
}
