//! Panel item record (replaces `CFileData` from `spl_com.h`).

use std::path::PathBuf;
use std::time::SystemTime;

pub const ATTR_READONLY: u32 = 0x1;
pub const ATTR_HIDDEN: u32 = 0x2;
pub const ATTR_SYSTEM: u32 = 0x4;
pub const ATTR_DIRECTORY: u32 = 0x10;
pub const ATTR_ARCHIVE: u32 = 0x20;
pub const ATTR_REPARSE_POINT: u32 = 0x400;
pub const ATTR_COMPRESSED: u32 = 0x800;
pub const ATTR_OFFLINE: u32 = 0x1000;
pub const ATTR_ENCRYPTED: u32 = 0x4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// The `..` item at the top of a panel.
    UpDir,
    Dir,
    File,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Lossless identity of the item; always use this for file operations.
    pub path: PathBuf,
    /// UTF-8 display name. Equal to the real name unless `name_lossy` is set.
    pub name: String,
    /// True if the on-disk name is not valid Unicode (unpaired surrogate) and
    /// `name` contains U+FFFD replacement characters.
    pub name_lossy: bool,
    /// Byte offset in `name` where the extension starts (after the last dot);
    /// `name.len()` when there is no extension. Directories have no extension.
    pub ext_start: usize,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Win32 `FILE_ATTRIBUTE_*` bits (synthesised on other platforms).
    pub attrs: u32,
    pub selected: bool,
}

impl FileEntry {
    pub fn new(path: PathBuf, kind: EntryKind, size: u64, modified: Option<SystemTime>, attrs: u32) -> Self {
        let (name, name_lossy) = match path.file_name() {
            Some(os) => match os.to_str() {
                Some(s) => (s.to_owned(), false),
                None => (os.to_string_lossy().into_owned(), true),
            },
            None => (path.to_string_lossy().into_owned(), false),
        };
        let ext_start = if kind == EntryKind::File { ext_start(&name) } else { name.len() };
        FileEntry { path, name, name_lossy, ext_start, kind, size, modified, attrs, selected: false }
    }

    pub fn up_dir(parent: PathBuf) -> Self {
        FileEntry { path: parent, name: "..".into(), name_lossy: false, ext_start: 2, kind: EntryKind::UpDir, size: 0, modified: None, attrs: ATTR_DIRECTORY, selected: false }
    }

    /// Name without the extension (and without the separating dot).
    pub fn base(&self) -> &str {
        if self.ext_start < self.name.len() {
            &self.name[..self.ext_start - 1]
        } else {
            &self.name
        }
    }

    pub fn ext(&self) -> &str {
        &self.name[self.ext_start..]
    }

    pub fn is_dir(&self) -> bool {
        self.kind != EntryKind::File
    }

    pub fn is_hidden(&self) -> bool {
        self.attrs & (ATTR_HIDDEN | ATTR_SYSTEM) != 0
    }

    pub fn is_link(&self) -> bool {
        self.attrs & ATTR_REPARSE_POINT != 0
    }

    /// Salamander-style attribute string, e.g. `RHSA`.
    pub fn attr_string(&self) -> String {
        let mut s = String::with_capacity(4);
        for (bit, ch) in [(ATTR_READONLY, 'R'), (ATTR_HIDDEN, 'H'), (ATTR_SYSTEM, 'S'), (ATTR_ARCHIVE, 'A'), (ATTR_COMPRESSED, 'C'), (ATTR_ENCRYPTED, 'E')] {
            if self.attrs & bit != 0 {
                s.push(ch);
            }
        }
        s
    }
}

/// Byte offset of the extension: text after the last dot. A leading dot is an
/// extension too (`.cvspass`), matching Windows/Salamander semantics.
pub fn ext_start(name: &str) -> usize {
    match name.rfind('.') {
        Some(i) => i + 1,
        None => name.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> FileEntry {
        FileEntry::new(PathBuf::from(name), EntryKind::File, 0, None, 0)
    }

    #[test]
    fn extension_split() {
        let e = file("archive.tar.gz");
        assert_eq!(e.base(), "archive.tar");
        assert_eq!(e.ext(), "gz");
        let e = file(".cvspass");
        assert_eq!(e.base(), "");
        assert_eq!(e.ext(), "cvspass");
        let e = file("README");
        assert_eq!(e.base(), "README");
        assert_eq!(e.ext(), "");
    }

    #[test]
    fn unicode_names() {
        let e = file("日本語ファイル.テキスト");
        assert_eq!(e.base(), "日本語ファイル");
        assert_eq!(e.ext(), "テキスト");
        let e = file("😀 emoji.txt");
        assert_eq!(e.ext(), "txt");
        assert!(!e.name_lossy);
    }

    #[test]
    fn dirs_have_no_extension() {
        let e = FileEntry::new(PathBuf::from("dir.d"), EntryKind::Dir, 0, None, ATTR_DIRECTORY);
        assert_eq!(e.ext(), "");
        assert_eq!(e.base(), "dir.d");
    }
}
