//! The search itself: which names match, what a file contains, and the walk
//! through the folders. Nothing here knows about the program.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

use regex::bytes::RegexBuilder as BytesRegexBuilder;
use regex::RegexBuilder;

const ATTR_HIDDEN: u32 = 0x2;
const ATTR_SYSTEM: u32 = 0x4;
const ATTR_DIRECTORY: u32 = 0x10;
/// Symbolic links and junctions: listed, never entered (they can loop).
const ATTR_REPARSE_POINT: u32 = 0x400;

// ---------------------------------------------------------------- names

/// A wildcard mask: `*` is any run of characters, `?` one character; case
/// doesn't matter, as in Windows. `*.*` is every name, with or without a dot.
#[derive(Debug, Clone)]
pub struct Mask(Vec<char>);

impl Mask {
    pub fn new(text: &str) -> Mask {
        Mask(text.chars().flat_map(char::to_lowercase).collect())
    }

    pub fn matches(&self, name: &str) -> bool {
        if self.0 == ['*', '.', '*'] || self.0 == ['*'] {
            return true;
        }
        let name: Vec<char> = name.chars().flat_map(char::to_lowercase).collect();
        // the classic greedy match with one backtrack point per `*`
        let (p, n) = (&self.0, &name);
        let (mut i, mut j) = (0, 0);
        let mut star: Option<(usize, usize)> = None;
        while j < n.len() {
            if i < p.len() && (p[i] == '?' || p[i] == n[j]) {
                i += 1;
                j += 1;
            } else if i < p.len() && p[i] == '*' {
                star = Some((i, j));
                i += 1;
            } else if let Some((si, sj)) = star {
                i = si + 1;
                j = sj + 1;
                star = Some((si, sj + 1));
            } else {
                return false;
            }
        }
        p[i..].iter().all(|&c| c == '*')
    }
}

/// Which names are found.
#[derive(Debug, Clone)]
pub enum Names {
    /// Masks separated by `;`; the ones after `|` leave names out
    /// (`*.txt;*.md|old*`), as in Salamander. No masks before `|` is `*.*`.
    Masks {
        include: Vec<Mask>,
        exclude: Vec<Mask>,
    },
    Regex(regex::Regex),
}

impl Names {
    pub fn masks(text: &str) -> Names {
        let (inc, exc) = text.split_once('|').unwrap_or((text, ""));
        let list = |s: &str| s.split(';').map(str::trim).filter(|m| !m.is_empty()).map(Mask::new).collect::<Vec<_>>();
        let mut include = list(inc);
        if include.is_empty() {
            include.push(Mask::new("*.*"));
        }
        Names::Masks { include, exclude: list(exc) }
    }

    /// A regular expression found anywhere in the name (`^…$` for whole names).
    pub fn regex(text: &str, case: bool) -> Result<Names, String> {
        RegexBuilder::new(text).case_insensitive(!case).build().map(Names::Regex).map_err(|e| e.to_string())
    }

    pub fn matches(&self, name: &str) -> bool {
        match self {
            Names::Masks { include, exclude } => include.iter().any(|m| m.matches(name)) && !exclude.iter().any(|m| m.matches(name)),
            Names::Regex(r) => r.is_match(name),
        }
    }
}

// ---------------------------------------------------------------- text

/// What a file must contain.
#[derive(Debug, Clone)]
pub struct Text(regex::bytes::Regex);

/// Files are read in pieces this big…
const CHUNK: usize = 1 << 20;
/// …and each piece keeps this much of the one before, so a match across the
/// boundary is found (texts longer than this aren't found across it).
const OVERLAP: usize = 64 << 10;

impl Text {
    /// Plain text, found in UTF-8 (and so ASCII and most ANSI text) and in
    /// UTF-16 files; without `case`, upper and lower case are the same.
    pub fn literal(text: &str, case: bool) -> Result<Text, String> {
        if text.is_empty() {
            return Err("empty text".into());
        }
        let utf8 = format!("(?{}:{})", if case { "-i" } else { "i" }, regex::escape(text));
        // UTF-16 LE: each character as its two bytes, any of its case
        // variants that are one UTF-16 unit too
        let mut utf16 = String::from("(?-u:");
        for c in text.chars() {
            let mut variants = vec![c];
            if !case {
                for v in c.to_lowercase().chain(c.to_uppercase()) {
                    if !variants.contains(&v) && c.to_lowercase().count() == 1 && c.to_uppercase().count() == 1 {
                        variants.push(v);
                    }
                }
            }
            let alts: Vec<String> = variants
                .iter()
                .map(|v| {
                    let mut buf = [0u16; 2];
                    v.encode_utf16(&mut buf).iter().flat_map(|u| u.to_le_bytes()).map(|b| format!("\\x{b:02X}")).collect()
                })
                .collect();
            utf16.push_str(&format!("(?:{})", alts.join("|")));
        }
        utf16.push(')');
        BytesRegexBuilder::new(&format!("{utf8}|{utf16}")).build().map(Text).map_err(|e| e.to_string())
    }

    /// A regular expression over the file's bytes as UTF-8 text; `^` and `$`
    /// match at line starts and ends.
    pub fn regex(text: &str, case: bool) -> Result<Text, String> {
        BytesRegexBuilder::new(text).case_insensitive(!case).multi_line(true).build().map(Text).map_err(|e| e.to_string())
    }

    /// Whether the file contains it; reading stops early when `stop` is set.
    pub fn found_in(&self, path: &Path, stop: &AtomicBool) -> io::Result<bool> {
        let mut f = fs::File::open(path)?;
        let mut buf = vec![0u8; OVERLAP + CHUNK];
        let mut kept = 0;
        loop {
            if stop.load(Ordering::Relaxed) {
                return Ok(false);
            }
            let mut filled = kept;
            // fill a whole piece (a read may return less)
            while filled < buf.len() {
                match f.read(&mut buf[filled..]) {
                    Ok(0) => break,
                    Ok(n) => filled += n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
            if filled == kept {
                return Ok(false);
            }
            if self.0.is_match(&buf[..filled]) {
                return Ok(true);
            }
            if filled < buf.len() {
                return Ok(false);
            }
            let keep = OVERLAP.min(filled);
            buf.copy_within(filled - keep..filled, 0);
            kept = keep;
        }
    }
}

// ---------------------------------------------------------------- walking

/// What to look for.
#[derive(Debug, Clone)]
pub struct Query {
    pub names: Names,
    /// Text the files must contain; folders are then never results.
    pub text: Option<Text>,
    pub subfolders: bool,
    /// Folders whose names match are results too.
    pub folders: bool,
    /// Look at hidden and system items (and into such folders).
    pub hidden: bool,
}

/// A result.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// The path below the searched folder, `\`-separated.
    pub inner: String,
    /// The file or folder on disk.
    pub real: PathBuf,
    pub dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub attrs: u32,
}

/// A search's progress and results, shared with whoever shows them.
#[derive(Debug, Default)]
pub struct State {
    pub stop: AtomicBool,
    pub files: AtomicU64,
    pub folders: AtomicU64,
    /// Folders and files that couldn't be read.
    pub errors: AtomicU64,
    /// The folder being searched.
    pub folder: Mutex<PathBuf>,
    pub found: Mutex<Vec<Found>>,
}

#[cfg(windows)]
pub fn attributes(md: &fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    md.file_attributes()
}

#[cfg(not(windows))]
pub fn attributes(md: &fs::Metadata) -> u32 {
    if md.is_dir() {
        ATTR_DIRECTORY
    } else {
        0
    }
}

/// Search `root` (a folder), adding results to `st.found` as they are
/// found, until done or `st.stop` is set. Folders are searched in name
/// order, each one's files before its subfolders.
pub fn search(root: &Path, q: &Query, st: &State) {
    let mut stack: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, inner)) = stack.pop() {
        if st.stop.load(Ordering::Relaxed) {
            return;
        }
        *st.folder.lock().unwrap() = dir.clone();
        st.folders.fetch_add(1, Ordering::Relaxed);
        let mut entries: Vec<(String, fs::DirEntry, fs::Metadata)> = match fs::read_dir(&dir) {
            Ok(rd) => rd
                .filter_map(|e| match e.and_then(|e| e.metadata().map(|m| (e, m))) {
                    Ok((e, m)) => Some((e.file_name().to_string_lossy().into_owned(), e, m)),
                    Err(_) => {
                        st.errors.fetch_add(1, Ordering::Relaxed);
                        None
                    }
                })
                .collect(),
            Err(_) => {
                st.errors.fetch_add(1, Ordering::Relaxed);
                continue;
            }
        };
        entries.sort_by_cached_key(|(name, _, _)| name.to_lowercase());
        let mut subfolders = Vec::new();
        for (name, e, md) in entries {
            if st.stop.load(Ordering::Relaxed) {
                return;
            }
            let attrs = attributes(&md);
            if !q.hidden && attrs & (ATTR_HIDDEN | ATTR_SYSTEM) != 0 {
                continue;
            }
            let child = if inner.is_empty() { name.clone() } else { format!("{inner}\\{name}") };
            let is_dir = attrs & ATTR_DIRECTORY != 0;
            let hit = |dir: bool| Found { inner: child.clone(), real: e.path(), dir, size: if dir { 0 } else { md.len() }, modified: md.modified().ok(), attrs };
            if is_dir {
                if q.folders && q.text.is_none() && q.names.matches(&name) {
                    st.found.lock().unwrap().push(hit(true));
                }
                if q.subfolders && attrs & ATTR_REPARSE_POINT == 0 {
                    subfolders.push((e.path(), child.clone()));
                }
                continue;
            }
            st.files.fetch_add(1, Ordering::Relaxed);
            if !q.names.matches(&name) {
                continue;
            }
            let contains = match &q.text {
                None => true,
                Some(t) => t.found_in(&e.path(), &st.stop).unwrap_or_else(|_| {
                    st.errors.fetch_add(1, Ordering::Relaxed);
                    false
                }),
            };
            if contains && !st.stop.load(Ordering::Relaxed) {
                st.found.lock().unwrap().push(hit(false));
            }
        }
        // popped in name order
        stack.extend(subfolders.into_iter().rev());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks() {
        let m = |mask: &str, name: &str| Names::masks(mask).matches(name);
        assert!(m("*.*", "README") && m("*.*", "a.b.c") && m("", "cokoli"));
        assert!(m("*.TXT", "zpráva.txt") && !m("*.txt", "zpráva.txt.bak"));
        assert!(m("?.rs", "a.rs") && !m("?.rs", "ab.rs"));
        assert!(m("*.txt;*.md", "a.md") && !m("*.txt; *.md", "a.rs"));
        assert!(m("ŽLUŤ*", "žluťoučký kůň") && m("*kůň*", "Žluťoučký KŮŇ.txt"));
        assert!(m("a*b*c", "aXXbYYc") && !m("a*b*c", "aXXbYY") && m("*a*a", "banana"));
        // exclusions after |
        assert!(m("*.txt|old*", "new.txt") && !m("*.txt|old*", "old.txt"));
        assert!(m("|*.bak", "a.txt") && !m("|*.bak;*.tmp", "a.tmp"));
    }

    #[test]
    fn regex_names() {
        let r = Names::regex(r"^\d{4}-.*\.jpe?g$", false).unwrap();
        assert!(r.matches("2024-dovolená.JPG") && !r.matches("x2024-a.jpg"));
        assert!(!Names::regex("^A", true).unwrap().matches("abc"));
        assert!(Names::regex("(", false).is_err());
    }

    fn file(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, data).unwrap();
        p
    }

    #[test]
    fn text_in_files() {
        let t = tempfile::tempdir().unwrap();
        let stop = AtomicBool::new(false);
        let utf8 = file(t.path(), "a.txt", "Příliš žluťoučký kůň úpěl".as_bytes());
        let utf16: Vec<u8> = "\u{feff}Hello Žluťoučký World".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let utf16 = file(t.path(), "b.txt", &utf16);
        let lit = |s: &str, case: bool, f: &Path| Text::literal(s, case).unwrap().found_in(f, &stop).unwrap();
        assert!(lit("ŽLUŤOUČKÝ", false, &utf8) && !lit("ŽLUŤOUČKÝ", true, &utf8) && lit("žluťoučký", true, &utf8));
        assert!(lit("world", false, &utf16) && lit("žluťoučký", false, &utf16), "UTF-16 text, any case");
        assert!(!lit("world", true, &utf16) && lit("World", true, &utf16));
        assert!(!lit("a.b", false, &utf8), "plain text is not a pattern");
        assert!(Text::regex(r"^Příliš\s+\w+", false).unwrap().found_in(&utf8, &stop).unwrap());
        assert!(!Text::regex(r"^kůň", false).unwrap().found_in(&utf8, &stop).unwrap());
        assert!(Text::literal("", false).is_err() && Text::regex("[", false).is_err());

        // a match across the pieces a big file is read in, and at its very end
        let mut big = vec![b'.'; CHUNK + OVERLAP - 3];
        big.extend_from_slice(b"needle");
        big.extend(vec![b'.'; CHUNK]);
        big.extend_from_slice(b"KONEC");
        let big = file(t.path(), "big.bin", &big);
        assert!(lit("needle", true, &big) && lit("konec", false, &big) && !lit("nothing", false, &big));
        assert!(Text::literal("x", false).unwrap().found_in(&t.path().join("missing"), &stop).is_err());
    }

    #[test]
    fn walking() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        fs::create_dir_all(root.join("docs").join("staré")).unwrap();
        fs::create_dir_all(root.join("txt-folder")).unwrap();
        file(root, "b.txt", b"hello");
        file(root, "a.TXT", b"nothing");
        file(&root.join("docs"), "c.txt", b"say hello");
        file(&root.join("docs"), "d.md", b"hello");
        file(&root.join("docs").join("staré"), "e.txt", b"HELLO");
        let run = |q: Query| {
            let st = State::default();
            search(root, &q, &st);
            st.found.into_inner().unwrap().into_iter().map(|f| f.inner).collect::<Vec<_>>()
        };
        let q = Query { names: Names::masks("*.txt"), text: None, subfolders: true, folders: false, hidden: true };
        assert_eq!(run(q.clone()), ["a.TXT", "b.txt", "docs\\c.txt", "docs\\staré\\e.txt"], "files before subfolders, in name order");
        assert_eq!(run(Query { subfolders: false, ..q.clone() }), ["a.TXT", "b.txt"]);
        assert_eq!(run(Query { names: Names::masks("*txt*"), folders: true, ..q.clone() }), ["a.TXT", "b.txt", "txt-folder", "docs\\c.txt", "docs\\staré\\e.txt"]);
        let text = Query { names: Names::masks("*.*"), text: Some(Text::literal("hello", false).unwrap()), folders: true, ..q.clone() };
        assert_eq!(run(text), ["b.txt", "docs\\c.txt", "docs\\d.md", "docs\\staré\\e.txt"], "folders never match text");
        let case = Query { names: Names::masks("*.*"), text: Some(Text::literal("hello", true).unwrap()), ..q.clone() };
        assert_eq!(run(case), ["b.txt", "docs\\c.txt", "docs\\d.md"]);

        let st = State::default();
        search(root, &q, &st);
        let f = st.found.lock().unwrap()[3].clone();
        assert_eq!((f.real, f.size, f.dir), (root.join("docs").join("staré").join("e.txt"), 5, false));
        assert_eq!((st.files.load(Ordering::Relaxed), st.folders.load(Ordering::Relaxed), st.errors.load(Ordering::Relaxed)), (5, 4, 0));

        // stopped before it starts: nothing
        let st = State::default();
        st.stop.store(true, Ordering::Relaxed);
        search(root, &q, &st);
        assert!(st.found.lock().unwrap().is_empty());
        // a folder that can't be read is counted, not fatal
        let st = State::default();
        search(&root.join("missing"), &q, &st);
        assert_eq!(st.errors.load(Ordering::Relaxed), 1);
    }

    #[cfg(windows)]
    #[test]
    fn hidden_items() {
        use std::os::windows::process::CommandExt;
        let t = tempfile::tempdir().unwrap();
        file(t.path(), "seen.txt", b"");
        fs::create_dir(t.path().join("skrytá")).unwrap();
        file(&t.path().join("skrytá"), "inside.txt", b"");
        let ok = std::process::Command::new("attrib").raw_arg("+h").arg(t.path().join("skrytá")).status().unwrap().success();
        assert!(ok);
        let q = Query { names: Names::masks("*.txt"), text: None, subfolders: true, folders: false, hidden: false };
        let st = State::default();
        search(t.path(), &q, &st);
        assert_eq!(st.found.lock().unwrap().iter().map(|f| f.inner.as_str()).collect::<Vec<_>>(), ["seen.txt"], "hidden folders aren't entered");
        let st = State::default();
        search(t.path(), &Query { hidden: true, ..q }, &st);
        assert_eq!(st.found.lock().unwrap().len(), 2);
    }
}
