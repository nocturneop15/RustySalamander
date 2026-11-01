//! A file as the viewer shows it: its text in an encoding (detected, or
//! chosen), colored by its syntax, or its bytes as hex. No window here, so
//! it can be tested.

use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::highlight::{self, Span};

use sal_core::encoding::{self, display_path, TextEncoding};
use sal_plugin_api::tr;

/// Files larger than this are shown truncated.
pub const MAX_BYTES: u64 = 32 << 20;
/// Word wrap lays out all text at once, so it's limited to smaller files.
pub const MAX_WRAP_BYTES: usize = 2 << 20;

pub struct Doc {
    pub title: String,
    bytes: Vec<u8>,
    /// The whole file's length; more than `bytes` when it was too large.
    pub file_len: u64,
    pub encoding: TextEncoding,
    bom: usize,
    pub lines: Vec<String>,
    pub hex: bool,
    pub wrap: bool,
    /// Why the file couldn't be read.
    pub error: Option<String>,
    /// The syntax its text is colored by (detected, or chosen); `None` for
    /// plain text.
    pub syntax: Option<String>,
    /// The colors of the lines: (dark theme, `version` they were made for, spans).
    spans: Option<(bool, u64, LineSpans)>,
    /// Changes with the lines and the syntax, so old colors aren't used.
    version: u64,
    /// Coloring running in the background.
    job: Option<Job>,
}

/// The colors of every line, one list of spans per line.
type LineSpans = Vec<Vec<Span>>;

/// Coloring the lines on a thread of its own.
struct Job {
    dark: bool,
    version: u64,
    stop: Arc<AtomicBool>,
    /// Filled when the thread ends: the colors, or `None` when they could
    /// not be made.
    result: Arc<Mutex<Option<Option<LineSpans>>>>,
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
    Ready,
}

impl Doc {
    /// Read `path` (at most [`MAX_BYTES`]): its text in the encoding detected,
    /// its syntax by name or first line; binary files open as hex.
    pub fn open(path: &Path) -> Doc {
        let title = format!("{} - {}", encoding::display_name(&display_path(path)), tr("Viewer"));
        let mut d = Doc {
            title,
            bytes: Vec::new(),
            file_len: 0,
            encoding: TextEncoding::Utf8,
            bom: 0,
            lines: Vec::new(),
            hex: false,
            wrap: false,
            error: None,
            syntax: None,
            spans: None,
            version: 0,
            job: None,
        };
        match std::fs::File::open(path) {
            Ok(f) => {
                d.file_len = f.metadata().map(|m| m.len()).unwrap_or(0);
                let mut buf = Vec::new();
                if let Err(e) = f.take(MAX_BYTES).read_to_end(&mut buf) {
                    d.error = Some(e.to_string());
                }
                d.bytes = buf;
                let (enc, bom) = encoding::detect(&d.bytes);
                d.bom = bom;
                d.set_encoding(enc);
                // binary files open in hex mode
                d.hex = d.bytes.iter().take(8192).any(|&b| b == 0) && !matches!(enc, TextEncoding::Utf16Le | TextEncoding::Utf16Be);
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                d.syntax = highlight::detect(&name, d.lines.first().map(String::as_str).unwrap_or(""));
            }
            Err(e) => d.error = Some(e.to_string()),
        }
        d
    }

    /// Show the text in `enc`.
    pub fn set_encoding(&mut self, enc: TextEncoding) {
        self.encoding = enc;
        let bom = if matches!((enc, self.bom), (TextEncoding::Utf8, 3) | (TextEncoding::Utf16Le | TextEncoding::Utf16Be, 2)) { self.bom } else { 0 };
        let text = encoding::decode(&self.bytes[bom..], enc);
        self.lines = text.lines().map(|l| encoding::visual_order(&l.replace('\t', "    "), false).into_owned()).collect();
        self.version += 1;
    }

    /// Color the text by `syntax` (`None`: plain text).
    pub fn set_syntax(&mut self, syntax: Option<String>) {
        self.syntax = syntax;
        self.version += 1;
    }

    /// The colors of line `row`, once they are made.
    pub fn line_spans(&self, row: usize) -> Option<&[Span]> {
        match &self.spans {
            Some((_, v, spans)) if *v == self.version => spans.get(row).map(Vec::as_slice),
            _ => None,
        }
    }

    /// Keep the colors up to date for the dark or the light theme: start
    /// coloring in the background when needed (`wake` is called when it is
    /// done), take the result when it is there.
    pub fn colors(&mut self, dark: bool, wake: impl Fn() + Send + 'static) -> Colors {
        let Some(syntax) = self.syntax.clone().filter(|_| !self.hex && self.error.is_none()) else {
            self.cancel();
            return Colors::Off;
        };
        if self.bytes.len() > highlight::MAX_BYTES {
            return Colors::TooLarge;
        }
        if matches!(&self.spans, Some((d, v, _)) if *d == dark && *v == self.version) {
            return Colors::Ready;
        }
        if let Some(job) = &self.job {
            if job.dark == dark && job.version == self.version {
                let done = job.result.lock().unwrap().take();
                return match done {
                    None => Colors::Working,
                    Some(spans) => {
                        self.job = None;
                        match spans {
                            Some(spans) => {
                                self.spans = Some((dark, self.version, spans));
                                Colors::Ready
                            }
                            None => Colors::Off,
                        }
                    }
                };
            }
            self.cancel();
        }
        let job = Job { dark, version: self.version, stop: Arc::new(AtomicBool::new(false)), result: Arc::new(Mutex::new(None)) };
        let (lines, stop, result) = (self.lines.clone(), job.stop.clone(), job.result.clone());
        let spawned = std::thread::Builder::new().name("viewer colors".into()).spawn(move || {
            let spans = highlight::highlight(&lines, &syntax, dark, &stop);
            if !stop.load(Ordering::Relaxed) {
                *result.lock().unwrap() = Some(spans);
                wake();
            }
        });
        if spawned.is_err() {
            return Colors::Off;
        }
        self.job = Some(job);
        Colors::Working
    }

    /// Stop the coloring that runs.
    fn cancel(&mut self) {
        if let Some(job) = self.job.take() {
            job.stop.store(true, Ordering::Relaxed);
        }
    }

    /// The bytes read (at most [`MAX_BYTES`]).
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether word wrap can be turned on.
    pub fn can_wrap(&self) -> bool {
        self.bytes.len() <= MAX_WRAP_BYTES
    }

    /// The number of hex rows (16 bytes each).
    pub fn hex_rows(&self) -> usize {
        self.bytes.len().div_ceil(16)
    }

    /// Hex row `row`: offset, 16 bytes in hex, the same as text.
    pub fn hex_line(&self, row: usize) -> String {
        let start = row * 16;
        let chunk = &self.bytes[start..(start + 16).min(self.bytes.len())];
        let mut s = format!("{start:08X}  ");
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
        assert_eq!(d.lines, ["Dobrý den", "    žluťoučký kůň"], "the BOM is left out, tabs are spaces");
        assert!(d.title.ends_with("zpráva 🦎.txt - Viewer"), "{}", d.title);

        // UTF-16 has zero bytes but is text
        let utf16 = t.path().join("u16.txt");
        let mut b = vec![0xFF, 0xFE];
        b.extend("ahoj".encode_utf16().flat_map(u16::to_le_bytes));
        std::fs::write(&utf16, b).unwrap();
        let d = Doc::open(&utf16);
        assert_eq!((d.encoding, d.hex, d.lines.clone()), (TextEncoding::Utf16Le, false, vec!["ahoj".to_owned()]));

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

    /// A JSON file is colored in the background; another syntax or the
    /// other theme colors it again; plain text and hex aren't colored.
    #[test]
    fn colors_in_the_background() {
        let t = tempfile::tempdir().unwrap();
        let json = t.path().join("data.json");
        std::fs::write(&json, "{\n  \"a\": [1, true, \"x\"]\n}\n").unwrap();
        let mut d = Doc::open(&json);
        assert_eq!(d.syntax.as_deref(), Some("JSON"));
        let wait = |d: &mut Doc, dark: bool| {
            for _ in 0..500 {
                match d.colors(dark, || {}) {
                    Colors::Working => std::thread::sleep(std::time::Duration::from_millis(5)),
                    other => return other,
                }
            }
            panic!("never colored");
        };
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
}
