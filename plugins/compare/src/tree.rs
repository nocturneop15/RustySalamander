//! Two folders compared: every item of either side, by its path inside,
//! with how the two sides compare. No window here, so it can be tested.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use sal_plugin_api::{join_inner, under, ItemMark};

/// What makes two files the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    /// The same size and modification time (FAT's two seconds allowed).
    SizeDate,
    /// The same bytes.
    Content,
}

/// One side of an item.
#[derive(Debug, Clone, PartialEq)]
pub struct Side {
    pub dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub attrs: u32,
}

/// How the two sides of an item compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Same,
    /// Different, neither newer (or a folder with differences inside).
    Different,
    LeftNewer,
    RightNewer,
    OnlyLeft,
    OnlyRight,
}

/// An item of either side.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Its name (the left side's spelling when both have it).
    pub name: String,
    pub left: Option<Side>,
    pub right: Option<Side>,
    pub state: State,
}

impl Node {
    /// Whether it is a folder (on the side that has it; the left one when
    /// both do).
    pub fn dir(&self) -> bool {
        self.left.as_ref().or(self.right.as_ref()).is_some_and(|s| s.dir)
    }

    /// The mark of the left (`left`) or the right side.
    pub fn mark(&self, left: bool) -> ItemMark {
        match (self.state, left) {
            (State::Same, _) => ItemMark::Same,
            (State::Different, _) => ItemMark::Different,
            (State::LeftNewer, true) | (State::RightNewer, false) => ItemMark::Newer,
            (State::LeftNewer, false) | (State::RightNewer, true) => ItemMark::Older,
            (State::OnlyLeft, true) | (State::OnlyRight, false) => ItemMark::OnlyHere,
            (State::OnlyLeft, false) | (State::OnlyRight, true) => ItemMark::Missing,
        }
    }
}

/// Two folders compared.
#[derive(Debug)]
pub struct Comparison {
    pub left: PathBuf,
    pub right: PathBuf,
    pub by: By,
    /// Every item below the two folders by its path inside (`a\b.txt`).
    pub nodes: BTreeMap<String, Node>,
}

/// Modification times this close are the same (FAT keeps two seconds).
const SAME_TIME_SECS: u64 = 2;

impl Comparison {
    /// Compare `left` with `right` and everything below; `stop` ends it early
    /// (an error, `Interrupted`).
    pub fn new(left: &Path, right: &Path, by: By, stop: &AtomicBool) -> io::Result<Comparison> {
        for dir in [left, right] {
            if !dir.is_dir() {
                return Err(io::Error::new(io::ErrorKind::NotFound, format!("{}: not a folder", dir.display())));
            }
        }
        let mut c = Comparison { left: left.to_path_buf(), right: right.to_path_buf(), by, nodes: BTreeMap::new() };
        c.compare_folder("", stop)?;
        Ok(c)
    }

    /// The items directly in `folder`.
    pub fn children(&self, folder: &str) -> impl Iterator<Item = (&String, &Node)> {
        let prefix = if folder.is_empty() { String::new() } else { format!("{folder}\\") };
        let len = prefix.len();
        self.nodes.range(prefix.clone()..).take_while(move |(k, _)| k.starts_with(&prefix)).filter(move |(k, _)| !k[len..].contains('\\'))
    }

    /// The path of `inner` on the left (`left`) or the right side.
    pub fn path(&self, left: bool, inner: &str) -> PathBuf {
        let base = if left { &self.left } else { &self.right };
        if inner.is_empty() {
            base.clone()
        } else {
            base.join(inner)
        }
    }

    /// Compare `inner` (an item that changed, or appeared, or went) again,
    /// with everything below it, and the folders above it.
    pub fn refresh(&mut self, inner: &str, stop: &AtomicBool) -> io::Result<()> {
        if inner.is_empty() {
            self.nodes.clear();
            return self.compare_folder("", stop).map(|_| ());
        }
        self.nodes.retain(|k, _| !under(k, inner));
        let (parent, name) = inner.rsplit_once('\\').map_or(("", inner), |(p, n)| (p, n));
        let (l, r) = (side_of(&self.path(true, inner)), side_of(&self.path(false, inner)));
        if l.is_some() || r.is_some() {
            let node = self.compare_item(inner, name.to_owned(), l, r, stop)?;
            self.nodes.insert(inner.to_owned(), node);
        }
        self.update_folders(parent);
        Ok(())
    }

    /// Recompute the states of `folder` and the folders above it from what
    /// is inside them.
    fn update_folders(&mut self, folder: &str) {
        let mut at = folder.to_owned();
        while !at.is_empty() {
            let differ = self.children(&at).any(|(_, n)| n.state != State::Same);
            if let Some(n) = self.nodes.get_mut(&at) {
                if n.left.is_some() && n.right.is_some() && n.left.as_ref().is_some_and(|s| s.dir) && n.right.as_ref().is_some_and(|s| s.dir) {
                    n.state = if differ { State::Different } else { State::Same };
                }
            }
            at = at.rsplit_once('\\').map_or(String::new(), |(p, _)| p.to_owned());
        }
    }

    /// Compare the folder `inner` (on both sides) and everything below; true
    /// when all of it is the same.
    fn compare_folder(&mut self, inner: &str, stop: &AtomicBool) -> io::Result<bool> {
        if stop.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        // names on Windows match in any case; the left side's spelling wins
        let mut names: BTreeMap<String, (String, Option<Side>, Option<Side>)> = BTreeMap::new();
        for (left, dir) in [(true, self.path(true, inner)), (false, self.path(false, inner))] {
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let Some(side) = side_of(&e.path()) else { continue };
                let slot = names.entry(name.to_lowercase()).or_insert_with(|| (name.clone(), None, None));
                if left {
                    slot.1 = Some(side);
                } else {
                    slot.2 = Some(side);
                }
            }
        }
        let mut all_same = true;
        for (_, (name, l, r)) in names {
            let path = join_inner(inner, &name);
            let node = self.compare_item(&path, name, l, r, stop)?;
            all_same &= node.state == State::Same;
            self.nodes.insert(path, node);
        }
        Ok(all_same)
    }

    /// Compare the item `inner`, whose sides are `l` and `r`.
    fn compare_item(&mut self, inner: &str, name: String, l: Option<Side>, r: Option<Side>, stop: &AtomicBool) -> io::Result<Node> {
        let state = match (&l, &r) {
            (Some(a), Some(b)) if a.dir && b.dir => {
                if self.compare_folder(inner, stop)? {
                    State::Same
                } else {
                    State::Different
                }
            }
            (Some(a), Some(b)) if a.dir != b.dir => {
                // a folder on one side, a file on the other: what is below
                // the folder is only there
                if a.dir {
                    self.one_side(inner, true, stop)?;
                } else {
                    self.one_side(inner, false, stop)?;
                }
                State::Different
            }
            (Some(a), Some(b)) => self.compare_files(inner, a, b)?,
            (Some(a), None) => {
                if a.dir {
                    self.one_side(inner, true, stop)?;
                }
                State::OnlyLeft
            }
            (None, Some(b)) => {
                if b.dir {
                    self.one_side(inner, false, stop)?;
                }
                State::OnlyRight
            }
            (None, None) => State::Same,
        };
        Ok(Node { name, left: l, right: r, state })
    }

    /// Everything below the folder `inner`, which only the left (`left`) or
    /// the right side has.
    fn one_side(&mut self, inner: &str, left: bool, stop: &AtomicBool) -> io::Result<()> {
        if stop.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let Ok(rd) = fs::read_dir(self.path(left, inner)) else { return Ok(()) };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(side) = side_of(&e.path()) else { continue };
            let path = join_inner(inner, &name);
            if side.dir {
                self.one_side(&path, left, stop)?;
            }
            let (l, r, state) = if left { (Some(side), None, State::OnlyLeft) } else { (None, Some(side), State::OnlyRight) };
            self.nodes.insert(path, Node { name, left: l, right: r, state });
        }
        Ok(())
    }

    /// How two files compare: by size and date, or by their bytes; the
    /// newer side when they differ and their times do.
    fn compare_files(&self, inner: &str, a: &Side, b: &Side) -> io::Result<State> {
        let same_time = match (a.modified, b.modified) {
            (Some(x), Some(y)) => x.duration_since(y).or_else(|_| y.duration_since(x)).map_or(true, |d| d.as_secs() <= SAME_TIME_SECS),
            _ => true,
        };
        let same = a.size == b.size
            && match self.by {
                By::SizeDate => same_time,
                By::Content => same_bytes(&self.path(true, inner), &self.path(false, inner))?,
            };
        Ok(match (same, same_time) {
            (true, _) => State::Same,
            (false, true) => State::Different,
            (false, false) if a.modified > b.modified => State::LeftNewer,
            (false, false) => State::RightNewer,
        })
    }
}

/// An item on disk, as a side; `None` for what can't be read and for links
/// and junctions (never followed, so a comparison can't loop).
fn side_of(p: &Path) -> Option<Side> {
    let md = fs::symlink_metadata(p).ok()?;
    if md.file_type().is_symlink() {
        return None;
    }
    Some(Side { dir: md.is_dir(), size: if md.is_dir() { 0 } else { md.len() }, modified: md.modified().ok(), attrs: attributes(&md) })
}

/// Whether two files hold the same bytes (read side by side, 1 MiB at a time).
pub fn same_bytes(a: &Path, b: &Path) -> io::Result<bool> {
    let (mut fa, mut fb) = (fs::File::open(a)?, fs::File::open(b)?);
    if fa.metadata()?.len() != fb.metadata()?.len() {
        return Ok(false);
    }
    let (mut ba, mut bb) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let n = read_full(&mut fa, &mut ba)?;
        let m = read_full(&mut fb, &mut bb)?;
        if n != m || ba[..n] != bb[..m] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

/// Fill `buf` as far as the file goes; the bytes read.
fn read_full(f: &mut fs::File, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

/// A file's Windows attributes.
#[cfg(windows)]
fn attributes(md: &fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    md.file_attributes()
}

/// Elsewhere: folder or not.
#[cfg(not(windows))]
fn attributes(md: &fs::Metadata) -> u32 {
    if md.is_dir() {
        sal_plugin_api::ATTR_DIRECTORY
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn set_time(p: &Path, secs: u64) {
        fs::File::options().write(true).open(p).unwrap().set_modified(UNIX_EPOCH + Duration::from_secs(secs)).unwrap();
    }

    /// Two trees: the same file, a newer one, files only on one side, a
    /// folder only on one side, a folder with a difference inside.
    #[test]
    fn two_trees() {
        let t = tempfile::tempdir().unwrap();
        let (l, r) = (t.path().join("levá"), t.path().join("pravá"));
        for d in [&l, &r] {
            fs::create_dir_all(d.join("sub 🦎")).unwrap();
            fs::write(d.join("same.txt"), "stejné").unwrap();
            set_time(&d.join("same.txt"), 1_000_000);
            fs::write(d.join("sub 🦎").join("deep.txt"), "a").unwrap();
        }
        fs::write(l.join("changed.txt"), "nové a delší").unwrap();
        fs::write(r.join("changed.txt"), "staré").unwrap();
        set_time(&l.join("changed.txt"), 2_000_000);
        set_time(&r.join("changed.txt"), 1_000_000);
        fs::write(l.join("only left.txt"), "x").unwrap();
        fs::create_dir_all(r.join("new dir").join("inner")).unwrap();
        fs::write(r.join("new dir").join("inner").join("f.txt"), "f").unwrap();
        // a difference deep inside a folder both have
        fs::write(r.join("sub 🦎").join("deep.txt"), "b").unwrap();
        set_time(&l.join("sub 🦎").join("deep.txt"), 1_000_000);
        set_time(&r.join("sub 🦎").join("deep.txt"), 1_000_000);
        // names differing in case are the same item
        fs::write(l.join("Case.TXT"), "c").unwrap();
        fs::write(r.join("case.txt"), "c").unwrap();
        set_time(&l.join("Case.TXT"), 5);
        set_time(&r.join("case.txt"), 5);

        let stop = AtomicBool::new(false);
        let c = Comparison::new(&l, &r, By::SizeDate, &stop).unwrap();
        let state = |p: &str| c.nodes[p].state;
        assert_eq!(state("same.txt"), State::Same);
        assert_eq!(state("changed.txt"), State::LeftNewer);
        assert_eq!(state("only left.txt"), State::OnlyLeft);
        assert_eq!(state("new dir"), State::OnlyRight);
        assert_eq!(state("new dir\\inner\\f.txt"), State::OnlyRight, "what is inside a one-sided folder");
        assert_eq!(state("Case.TXT"), State::Same, "the left side's spelling");
        // by size and date the deep file looks the same; by content it isn't
        assert_eq!(state("sub 🦎\\deep.txt"), State::Same);
        let c2 = Comparison::new(&l, &r, By::Content, &stop).unwrap();
        assert_eq!(c2.nodes["sub 🦎\\deep.txt"].state, State::Different);
        assert_eq!(c2.nodes["sub 🦎"].state, State::Different, "a folder with a difference inside");

        // the marks of each side
        assert_eq!((c.nodes["changed.txt"].mark(true), c.nodes["changed.txt"].mark(false)), (ItemMark::Newer, ItemMark::Older));
        assert_eq!((c.nodes["only left.txt"].mark(true), c.nodes["only left.txt"].mark(false)), (ItemMark::OnlyHere, ItemMark::Missing));
        let top: Vec<&str> = c.children("").map(|(k, _)| k.as_str()).collect();
        assert_eq!(top, ["Case.TXT", "changed.txt", "new dir", "only left.txt", "same.txt", "sub 🦎"]);
        assert_eq!(c.children("new dir").count(), 1);

        // after copying the file across, it is the same, and so is its folder
        let mut c2 = c2;
        fs::copy(l.join("sub 🦎").join("deep.txt"), r.join("sub 🦎").join("deep.txt")).unwrap();
        c2.refresh("sub 🦎\\deep.txt", &stop).unwrap();
        assert_eq!(c2.nodes["sub 🦎\\deep.txt"].state, State::Same);
        assert_eq!(c2.nodes["sub 🦎"].state, State::Same);
        // a deleted file goes from the comparison
        fs::remove_file(l.join("only left.txt")).unwrap();
        c2.refresh("only left.txt", &stop).unwrap();
        assert!(!c2.nodes.contains_key("only left.txt"));

        stop.store(true, Ordering::Relaxed);
        assert_eq!(Comparison::new(&l, &r, By::SizeDate, &stop).unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert!(Comparison::new(&l, &t.path().join("není"), By::SizeDate, &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn bytes() {
        let t = tempfile::tempdir().unwrap();
        let (a, b, c) = (t.path().join("a"), t.path().join("b"), t.path().join("c"));
        let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&a, &big).unwrap();
        fs::write(&b, &big).unwrap();
        let mut other = big.clone();
        other[2_500_000] ^= 1;
        fs::write(&c, &other).unwrap();
        assert!(same_bytes(&a, &b).unwrap());
        assert!(!same_bytes(&a, &c).unwrap(), "a byte past the first megabyte");
    }
}
