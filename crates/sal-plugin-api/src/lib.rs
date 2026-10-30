//! What RustySalamander and its plugins share.
//!
//! A plugin is a DLL with the extension `.spl`, in its own folder under
//! `plugins` next to the program (`plugins\7zip\7zip.spl`), together with
//! anything it needs (`plugins\7zip\7z.dll`). The program looks for plugins
//! when it starts; it never contains them.
//!
//! The program and a plugin talk only through the C interface in [`abi`], so
//! each can be built separately. A plugin implements [`FolderPlugin`] and
//! [`Plugin`] and exports itself with [`export_plugin!`](crate::export_plugin); the program wraps
//! what it finds back into a [`FolderPlugin`].
//!
//! A *folder plugin* shows certain files as folders (archives): the panel
//! browses them, and the file operations work inside them through the
//! plugin. A path that goes through such a file is virtual:
//! `C:\x\a.zip\dir\f.txt`.
//!
//! The guide for plugin authors, with a complete example, is
//! `docs/PLUGINS.md` in the repository.

#![warn(missing_docs)]

pub mod abi;
#[doc(hidden)]
pub mod export;
pub mod host;

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub use export::{n_, tr, trf};

/// The attribute bit of folders (`FILE_ATTRIBUTE_DIRECTORY`).
pub const ATTR_DIRECTORY: u32 = 0x10;

/// Reports (bytes done, bytes total); returning false cancels. It may block
/// for as long as the operation is paused.
pub type Progress<'a> = dyn FnMut(u64, u64) -> bool + 'a;

/// A plugin that shows certain files as folders.
pub trait FolderPlugin: Send + Sync {
    /// Its id in the configuration ("7zip").
    fn id(&self) -> &str;
    /// Whether a file with this name opens as a folder.
    fn handles(&self, name: &str) -> bool;
    /// Whether the file's contents can be changed.
    fn can_write(&self, file: &Path) -> bool;
    /// One line about its state, for Settings (e.g. its library and formats).
    fn status(&self) -> String;

    /// Everything in the file.
    fn list(&self, file: &Path) -> io::Result<Index>;
    /// Copy `items` (folders with their contents) to `out`, keeping their
    /// paths: `out\inner\item`.
    fn extract(&self, file: &Path, items: &[String], out: &Path, progress: &mut Progress) -> io::Result<()>;
    /// Add files and folders from disk: `(path on disk, path inside)`;
    /// items already at those paths are replaced. Creates the file if needed.
    fn add(&self, file: &Path, items: &[(PathBuf, String)], progress: &mut Progress) -> io::Result<()>;
    /// Delete `items` (folders with their contents).
    fn delete(&self, file: &Path, items: &[String], progress: &mut Progress) -> io::Result<()>;
    /// Rename `old` to `new` (a folder's contents move with it).
    fn rename(&self, file: &Path, old: &str, new: &str, progress: &mut Progress) -> io::Result<()>;
    /// Create the folder `inner`.
    fn mkdir(&self, file: &Path, inner: &str, progress: &mut Progress) -> io::Result<()>;
}

/// What a plugin DLL says about itself and how it starts.
pub trait Plugin: FolderPlugin + Sized + 'static {
    /// Its id in the configuration; the same as [`FolderPlugin::id`].
    const ID: &'static str;
    /// The name shown in Settings → Plugins, in English, marked with [`n_`]
    /// (the program translates it from the plugin's language files).
    const NAME: &'static str;
    /// One or two sentences about what the plugin does, in English, marked
    /// with [`n_`].
    const DESCRIPTION: &'static str;
    /// Start it; `dir` is the plugin's folder. The error says why it can't run.
    fn start(dir: &Path) -> Result<Self, String>;
}

/// `name` inside the folder `base`.
pub fn join_inner(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_owned()
    } else {
        format!("{base}\\{name}")
    }
}

/// Whether `path` is `folder` or inside it.
pub fn under(path: &str, folder: &str) -> bool {
    path == folder || (path.len() > folder.len() && path.starts_with(folder) && path.as_bytes()[folder.len()] == b'\\')
}

/// One item inside a plugin's file.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Path inside, `\`-separated, without a leading separator.
    pub path: String,
    /// Whether it is a folder.
    pub dir: bool,
    /// Size in bytes (0 for folders; the program adds up their files).
    pub size: u64,
    /// Last change, if the file records it.
    pub modified: Option<SystemTime>,
    /// Windows file attributes (`FILE_ATTRIBUTE_*`); [`ATTR_DIRECTORY`] is
    /// added for folders.
    pub attrs: u32,
    /// The plugin's own number for it; `None` for folders known only from
    /// the paths inside them.
    pub index: Option<u32>,
}

/// Everything inside a plugin's file, for browsing it folder by folder.
#[derive(Debug, Clone, Default)]
pub struct Index {
    /// Every item, in any order. Folders don't need entries of their own:
    /// a folder that appears only in the paths of other items exists too.
    pub items: Vec<Item>,
}

/// Size, files and folders inside a folder.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Total size of the files.
    pub bytes: u64,
    /// Number of files.
    pub files: u64,
    /// Number of folders, nested ones included.
    pub dirs: u64,
}

impl Index {
    /// The items directly inside `inner`; folders that exist only as part of
    /// deeper paths are included. Folder sizes are the total of their files.
    pub fn children(&self, inner: &str) -> Vec<Item> {
        let prefix = if inner.is_empty() { String::new() } else { format!("{inner}\\") };
        let mut out: BTreeMap<String, Item> = BTreeMap::new();
        for it in &self.items {
            let Some(rest) = it.path.strip_prefix(&prefix) else { continue };
            if rest.is_empty() {
                continue;
            }
            match rest.split_once('\\') {
                None if it.dir => {
                    // a folder's own entry: its date and attributes; the size
                    // comes from its files, which may be listed before or after
                    let e = out.entry(rest.to_owned()).or_insert_with(|| Item { size: 0, ..it.clone() });
                    e.modified = it.modified;
                    e.attrs = it.attrs | ATTR_DIRECTORY;
                    e.index = it.index;
                }
                None => {
                    out.insert(rest.to_owned(), it.clone());
                }
                Some((first, _)) => {
                    let e = out.entry(first.to_owned()).or_insert_with(|| Item {
                        path: join_inner(inner, first),
                        dir: true,
                        size: 0,
                        modified: None,
                        attrs: ATTR_DIRECTORY,
                        index: None,
                    });
                    if !it.dir {
                        e.size += it.size;
                    }
                }
            }
        }
        out.into_values().collect()
    }

    /// Whether `inner` is a folder (the root is one).
    pub fn is_dir(&self, inner: &str) -> bool {
        inner.is_empty() || self.items.iter().any(|it| (it.dir && it.path == inner) || (it.path.len() > inner.len() && under(&it.path, inner)))
    }

    /// Whether anything is stored at `inner`.
    pub fn exists(&self, inner: &str) -> bool {
        self.items.iter().any(|it| it.path == inner) || self.is_dir(inner)
    }

    /// Size, files and folders inside the folder `inner` (like a folder size
    /// calculated on disk).
    pub fn stats(&self, inner: &str) -> Stats {
        let mut s = Stats::default();
        let mut dirs = HashSet::new();
        for it in self.items.iter().filter(|it| it.path.len() > inner.len() && under(&it.path, inner)) {
            let rest = &it.path[inner.len() + 1..];
            if it.dir {
                dirs.insert(rest.to_owned());
            } else {
                s.files += 1;
                s.bytes += it.size;
            }
            // folders known only from the paths of what is inside them
            let mut parts: Vec<&str> = rest.split('\\').collect();
            parts.pop();
            for n in 1..=parts.len() {
                dirs.insert(parts[..n].join("\\"));
            }
        }
        s.dirs = dirs.len() as u64;
        s
    }

    /// Total size of the files at or below `inner`.
    pub fn size_of(&self, inner: &str) -> u64 {
        self.items.iter().filter(|it| !it.dir && under(&it.path, inner)).map(|it| it.size).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str, dir: bool, size: u64) -> Item {
        Item { path: path.into(), dir, size, modified: None, attrs: if dir { ATTR_DIRECTORY } else { 0 }, index: Some(0) }
    }

    #[test]
    fn folders_of_an_index() {
        let ix = Index {
            items: vec![item("složka 日本", true, 0), item("a.txt", false, 3), item("složka 日本\\soubor 🦎.txt", false, 6), item("jen\\v cestě\\x.bin", false, 10)]
        };
        let root: Vec<(String, bool, u64)> = ix.children("").into_iter().map(|i| (i.path, i.dir, i.size)).collect();
        assert_eq!(root, [("a.txt".into(), false, 3), ("jen".into(), true, 10), ("složka 日本".into(), true, 6)], "folders only known from deeper paths are listed too");
        assert_eq!(ix.children("jen").into_iter().map(|i| i.path).collect::<Vec<_>>(), ["jen\\v cestě"]);
        assert!(ix.is_dir("jen\\v cestě") && ix.is_dir("") && !ix.is_dir("a.txt") && !ix.is_dir("je"));
        assert!(ix.exists("a.txt") && !ix.exists("b.txt"));
        assert_eq!(ix.size_of("složka 日本"), 6);
        assert_eq!(ix.stats("jen"), Stats { bytes: 10, files: 1, dirs: 1 });
    }
}
