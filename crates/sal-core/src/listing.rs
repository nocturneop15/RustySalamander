//! Reading a directory into a panel listing.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::entry::{self, EntryKind, FileEntry};
use crate::sort::{sort_entries, SortSpec};

/// Which items with the hidden/system attributes a panel shows. Like
/// Explorer, an item that is both hidden and system needs both enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Visibility {
    pub hidden: bool,
    pub system: bool,
}

impl Visibility {
    pub const ALL: Visibility = Visibility { hidden: true, system: true };

    pub fn shows(self, e: &FileEntry) -> bool {
        (self.hidden || e.attrs & entry::ATTR_HIDDEN == 0) && (self.system || e.attrs & entry::ATTR_SYSTEM == 0)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Listing {
    pub path: PathBuf,
    pub entries: Vec<FileEntry>,
    /// Number of entries that could not be stat'ed (still listed, with zero size).
    pub unreadable: usize,
}

#[cfg(windows)]
fn attrs_of(md: &fs::Metadata, _name: &std::ffi::OsStr) -> u32 {
    use std::os::windows::fs::MetadataExt;
    md.file_attributes()
}

#[cfg(not(windows))]
fn attrs_of(md: &fs::Metadata, name: &std::ffi::OsStr) -> u32 {
    let mut a = 0;
    if md.is_dir() {
        a |= entry::ATTR_DIRECTORY;
    }
    if md.permissions().readonly() {
        a |= entry::ATTR_READONLY;
    }
    if md.file_type().is_symlink() {
        a |= entry::ATTR_REPARSE_POINT;
    }
    if name.as_encoded_bytes().first() == Some(&b'.') {
        a |= entry::ATTR_HIDDEN;
    }
    a
}

/// Build a [`FileEntry`] for an existing path (follows nothing; symlinks to
/// directories are reported as directories, like Salamander does).
pub fn stat_entry(path: PathBuf) -> io::Result<FileEntry> {
    let md = fs::symlink_metadata(&path)?;
    Ok(from_metadata(path, &md))
}

fn from_metadata(path: PathBuf, md: &fs::Metadata) -> FileEntry {
    let attrs = attrs_of(md, path.file_name().unwrap_or_default());
    let is_dir = attrs & entry::ATTR_DIRECTORY != 0 || (md.file_type().is_symlink() && path.is_dir());
    let kind = if is_dir { EntryKind::Dir } else { EntryKind::File };
    let size = if is_dir { 0 } else { md.len() };
    FileEntry::new(path, kind, size, md.modified().ok(), attrs)
}

/// Make `p` absolute without resolving symlinks or adding a `\\?\` prefix.
pub fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

pub fn read_dir(path: &Path, vis: Visibility, sort: SortSpec) -> io::Result<Listing> {
    // `\\server` is not a directory: list its shares instead
    if let Some(server) = crate::location::unc_server(path) {
        let mut entries = crate::location::share_entries(&server, vis.hidden)?;
        sort_entries(&mut entries, sort);
        return Ok(Listing { path: crate::location::server_path(&server), entries, unreadable: 0 });
    }
    let path = absolute(path);
    let mut entries = Vec::new();
    let mut unreadable = 0;
    if let Some((parent, _)) = crate::location::parent_of(&path) {
        entries.push(FileEntry::up_dir(parent));
    }
    for de in fs::read_dir(&path)? {
        let de = match de {
            Ok(de) => de,
            Err(_) => {
                unreadable += 1;
                continue;
            }
        };
        let e = match de.metadata() {
            Ok(md) => from_metadata(de.path(), &md),
            Err(_) => {
                unreadable += 1;
                FileEntry::new(de.path(), EntryKind::File, 0, None, 0)
            }
        };
        if e.kind != EntryKind::UpDir && !vis.shows(&e) {
            continue;
        }
        entries.push(e);
    }
    sort_entries(&mut entries, sort);
    Ok(Listing { path, entries, unreadable })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_unicode_and_long_names() {
        let dir = tempfile::tempdir().unwrap();
        let names = ["日本語.txt", "Ελληνικά.md", "emoji 🦎.rs", "čeština-ěščřžýáíé.doc", "عربي.txt"];
        for n in names {
            fs::write(dir.path().join(n), n.as_bytes()).unwrap();
        }
        // a path well beyond the old MAX_PATH (260) limit
        let mut deep = dir.path().to_path_buf();
        for i in 0..12 {
            deep.push(format!("hluboký-adresář-číslo-{i:02}-ñandú-🦎"));
        }
        fs::create_dir_all(&deep).unwrap();
        assert!(deep.as_os_str().len() > 300);
        fs::write(deep.join("konec.txt"), b"ok").unwrap();

        let l = read_dir(dir.path(), Visibility::ALL, SortSpec::default()).unwrap();
        for n in names {
            let e = l.entries.iter().find(|e| e.name == n).expect(n);
            assert_eq!(fs::read(&e.path).unwrap(), n.as_bytes());
            assert!(!e.name_lossy);
        }
        let l = read_dir(&deep, Visibility::ALL, SortSpec::default()).unwrap();
        assert_eq!(l.entries[0].kind, EntryKind::UpDir);
        assert_eq!(l.entries[1].name, "konec.txt");
        assert_eq!(l.entries[1].size, 2);
    }

    #[cfg(windows)]
    #[test]
    fn hidden_and_system_are_separate() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        // FILE_ATTRIBUTE_HIDDEN = 2, FILE_ATTRIBUTE_SYSTEM = 4
        for (name, attrs) in [("plain.txt", 0x80), ("hidden.txt", 2), ("system.txt", 4), ("both.txt", 6)] {
            fs::OpenOptions::new().write(true).create(true).attributes(attrs).open(dir.path().join(name)).unwrap();
        }
        let names = |vis| {
            let mut v: Vec<String> = read_dir(dir.path(), vis, SortSpec::default()).unwrap().entries.into_iter().skip(1).map(|e| e.name).collect();
            v.sort();
            v
        };
        assert_eq!(names(Visibility::ALL), ["both.txt", "hidden.txt", "plain.txt", "system.txt"]);
        assert_eq!(names(Visibility { hidden: true, system: false }), ["hidden.txt", "plain.txt"]);
        assert_eq!(names(Visibility { hidden: false, system: true }), ["plain.txt", "system.txt"]);
        assert_eq!(names(Visibility { hidden: false, system: false }), ["plain.txt"]);
    }
}
