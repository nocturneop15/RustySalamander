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
//! A *folder plugin* shows things as folders in a panel, and the file
//! operations work inside them through the plugin:
//!
//! - **files** it handles, such as archives: a path through such a file is
//!   virtual, `C:\x\a.zip\dir\f.txt`;
//! - **connections** it opens from a command in the Plugins menu, such as a
//!   server: a [`Mount`] with a virtual root, shown as `scp://user@host/dir`.
//!
//! Any plugin can also be a **viewer or editor** of files ([`Opener`]): F3
//! and F4 then open matching files with it, in a window of its own.
//!
//! Each plugin has its own **version** ([`Plugin::VERSION`]) and may say
//! where newer ones are published ([`Plugin::UPDATES`]); the program's
//! update check looks there too.
//!
//! The guide for plugin authors, with complete examples, is
//! `docs/PLUGINS.md` in the repository.

#![warn(missing_docs)]

pub mod abi;
#[doc(hidden)]
pub mod export;
pub mod host;
pub mod ui;

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf, Prefix};
use std::time::SystemTime;

pub use export::{n_, tr, trf};
pub use ui::{Command, Field, FieldKind, Form, Mount, OpenKind, Opener, Reply, Results, Values};

/// The attribute bit of folders (`FILE_ATTRIBUTE_DIRECTORY`).
pub const ATTR_DIRECTORY: u32 = 0x10;
/// The attribute bit of hidden items (`FILE_ATTRIBUTE_HIDDEN`).
pub const ATTR_HIDDEN: u32 = 0x2;

/// Reports (bytes done, bytes total); returning false cancels. It may block
/// for as long as the operation is paused.
pub type Progress<'a> = dyn FnMut(u64, u64) -> bool + 'a;

/// A plugin that shows files it handles, or connections it opens, as
/// folders.
///
/// `file` in the methods below is the file on disk (an archive) or the root
/// of a connection the plugin opened ([`Mount::root`]). Paths inside it
/// ("inner paths") are `\`-separated without a leading separator; the root
/// is `""`.
pub trait FolderPlugin: Send + Sync {
    /// Its id in the configuration ("7zip").
    fn id(&self) -> &str;
    /// Whether a file with this name opens as a folder. Plugins that only
    /// open connections return false.
    fn handles(&self, name: &str) -> bool;
    /// Whether the contents can be changed.
    fn can_write(&self, file: &Path) -> bool;
    /// One line about its state, for Settings (e.g. its library and formats).
    fn status(&self) -> String;

    /// The items in the folder `folder`. A plugin that reads a whole file at
    /// once (an archive) returns everything and sets [`Index::complete`];
    /// the program then asks only again when the file changes. Otherwise
    /// return at least the folder's children; the program asks for each
    /// folder it shows. Fail with `NotFound` when `folder` isn't a folder.
    fn list(&self, file: &Path, folder: &str) -> io::Result<Index>;
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

    /// Commands for the Plugins menu, labels translated.
    fn commands(&self) -> Vec<Command> {
        Vec::new()
    }
    /// Run `command` (from [`FolderPlugin::commands`], or the
    /// [`Form::command`] of a form the plugin showed) with `input`: the
    /// form's values, plus `panel` (the active panel's path) and `mount`
    /// (the root of the plugin's connection shown in it, if any). It runs
    /// in the background, so it may take a while (connecting to a server).
    fn run(&self, command: &str, input: &Values) -> io::Result<Reply> {
        let _ = input;
        Err(io::Error::new(io::ErrorKind::Unsupported, format!("unknown command \"{command}\"")))
    }
    /// The connection with this root ([`Mount::root`]) is closed by the user
    /// or the program is ending: release it.
    fn close(&self, root: &Path) {
        let _ = root;
    }

    /// What the plugin is doing for the connection `root` on its own, such
    /// as a search filling it with results; the program shows it in the
    /// connection's results window ([`Reply::Results`]) and asks again while
    /// [`Activity::running`]. The default is nothing.
    fn activity(&self, root: &Path) -> Activity {
        let _ = root;
        Activity::default()
    }
    /// Stop what [`FolderPlugin::activity`] reports as running; what was done
    /// so far stays.
    fn stop(&self, root: &Path) {
        let _ = root;
    }
    /// The file or folder on disk the item `inner` stands for, when it is
    /// one (search results, a list of files). The program then opens, views
    /// and edits that file itself instead of a temporary copy, and can show
    /// it in its folder. The default is `None`: items exist only inside.
    fn real_path(&self, file: &Path, inner: &str) -> Option<PathBuf> {
        let _ = (file, inner);
        None
    }

    /// The plugin's viewers and editors of files; the default is none.
    fn openers(&self) -> Vec<Opener> {
        Vec::new()
    }
    /// Show `file` (a file on disk) with the opener `opener`, in a window of
    /// the plugin's own; return as soon as it is on its way, without waiting
    /// for the window to close. `options` says how the program looks, so
    /// the window can match it: `theme` (`system`, `light` or `dark`),
    /// `ui_font` and `mono_font` (font names, empty for the default),
    /// `scrollbars` (`solid` or `floating`), `accent_light` and
    /// `accent_dark` (the accent colors of the two themes as `#rrggbb`,
    /// empty for the theme's own; `accent` is the one of the theme chosen,
    /// for plugins from before); and `temporary` (`1` when the
    /// file is a temporary copy, from an archive or a server: changes to it
    /// are not saved back).
    fn open_file(&self, opener: &str, file: &Path, options: &Values) -> io::Result<()> {
        let _ = (file, options);
        Err(io::Error::new(io::ErrorKind::Unsupported, format!("unknown opener \"{opener}\"")))
    }
}

/// Where newer versions of a plugin are published ([`Plugin::UPDATES`]):
/// releases of a GitHub repository. A release is one of the plugin's when
/// its tag is `tag` followed by a version (`viewer-v0.6.0`); its zip, named
/// `<id>-<version>-api<interface>.zip`, holds the plugin's folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Updates {
    /// `owner/name` of the repository; empty for no updates.
    pub repo: &'static str,
    /// What the tags of the plugin's releases start with (`viewer-v`).
    pub tag: &'static str,
    /// The changelog's path in the repository (`plugins/viewer/CHANGELOG.md`),
    /// with a `## <version>` heading per version; empty for none.
    pub changelog: &'static str,
}

impl Updates {
    /// No updates are published.
    pub const NONE: Updates = Updates { repo: "", tag: "", changelog: "" };
}

/// What a plugin is doing for one of its connections
/// ([`FolderPlugin::activity`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    /// Still working: the program asks again in a moment, and offers Stop.
    pub running: bool,
    /// One line about it, translated ("Searching C:\Users… 12 found").
    pub text: String,
}

/// What a plugin DLL says about itself and how it starts.
pub trait Plugin: FolderPlugin + Sized + 'static {
    /// Its id in the configuration; the same as [`FolderPlugin::id`].
    const ID: &'static str;
    /// The name shown in Settings → Plugins and on its panels, in English,
    /// marked with [`n_`] (the program translates it from the plugin's
    /// language files).
    const NAME: &'static str;
    /// One or two sentences about what the plugin does, in English, marked
    /// with [`n_`].
    const DESCRIPTION: &'static str;
    /// The plugin's own version, a semantic version
    /// (`env!("CARGO_PKG_VERSION")`). It is independent of the program's.
    const VERSION: &'static str;
    /// Where newer versions are published; the default is nowhere.
    const UPDATES: Updates = Updates::NONE;
    /// The oldest RustySalamander version the plugin works with (`"0.6.0"`),
    /// when it needs something only newer ones have; empty (the default)
    /// for any. An older program doesn't load it, and its update check
    /// doesn't offer it.
    const PROGRAM_MIN: &'static str = "";
    /// The newest RustySalamander version the plugin works with, itself
    /// included; empty (the default) for any. A plugin that is being phased
    /// out sets it: newer programs don't load it.
    const PROGRAM_MAX: &'static str = "";
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
    folder.is_empty() || path == folder || (path.len() > folder.len() && path.starts_with(folder) && path.as_bytes()[folder.len()] == b'\\')
}

/// The root of a connection: `\\?\<scheme>\<authority>`, a path no file
/// system has, shown as `<scheme>://<authority>`.
pub fn mount_root(scheme: &str, authority: &str) -> PathBuf {
    PathBuf::from(format!(r"\\?\{scheme}\{authority}"))
}

/// The scheme and authority of a connection root or a path inside one, and
/// the inner path: `\\?\scp\me@host\home\me` → `("scp", "me@host", "home\me")`.
pub fn mount_of(path: &Path) -> Option<(String, String, String)> {
    let mut parts = path.components();
    let Some(Component::Prefix(p)) = parts.next() else { return None };
    let Prefix::Verbatim(scheme) = p.kind() else { return None };
    let scheme = scheme.to_str()?.to_owned();
    // a scheme is lowercase ASCII, which tells it from \\?\Volume{…} and the like
    if scheme.is_empty() || !scheme.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        return None;
    }
    let mut rest = parts.filter_map(|c| if let Component::Normal(s) = c { Some(s.to_string_lossy().into_owned()) } else { None });
    let authority = rest.next()?;
    let inner: Vec<String> = rest.collect();
    Some((scheme, authority, inner.join("\\")))
}

/// One item inside a plugin's file or connection.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Path inside, `\`-separated, without a leading separator.
    pub path: String,
    /// Whether it is a folder.
    pub dir: bool,
    /// Size in bytes (0 for folders; the program adds up their files).
    pub size: u64,
    /// Last change, if known.
    pub modified: Option<SystemTime>,
    /// Windows file attributes (`FILE_ATTRIBUTE_*`); [`ATTR_DIRECTORY`] is
    /// added for folders, [`ATTR_HIDDEN`] hides an item unless hidden files
    /// are shown.
    pub attrs: u32,
    /// The plugin's own number for it; `None` for folders known only from
    /// the paths inside them.
    pub index: Option<u32>,
}

/// Items inside a plugin's file or connection, for browsing it folder by
/// folder.
#[derive(Debug, Clone, Default)]
pub struct Index {
    /// The items, in any order. Folders don't need entries of their own: a
    /// folder that appears only in the paths of other items exists too.
    pub items: Vec<Item>,
    /// Whether `items` is everything (a whole archive). When false, they are
    /// the listed folder's contents, and folder sizes are unknown.
    pub complete: bool,
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
    /// calculated on disk). Meaningful only for a complete index.
    pub fn stats(&self, inner: &str) -> Stats {
        let mut s = Stats::default();
        let mut dirs = HashSet::new();
        for it in self.items.iter().filter(|it| it.path.len() > inner.len() && under(&it.path, inner)) {
            let rest = if inner.is_empty() { &it.path[..] } else { &it.path[inner.len() + 1..] };
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

/// A path's text as given, for the names in [`Values`].
pub(crate) fn path_text(p: &Path) -> String {
    p.as_os_str().to_string_lossy().into_owned()
}

/// A path from its text in [`Values`].
pub(crate) fn text_path(s: &str) -> PathBuf {
    PathBuf::from(OsString::from(s))
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
            items: vec![item("složka 日本", true, 0), item("a.txt", false, 3), item("složka 日本\\soubor 🦎.txt", false, 6), item("jen\\v cestě\\x.bin", false, 10)],
            complete: true,
        };
        let root: Vec<(String, bool, u64)> = ix.children("").into_iter().map(|i| (i.path, i.dir, i.size)).collect();
        assert_eq!(root, [("a.txt".into(), false, 3), ("jen".into(), true, 10), ("složka 日本".into(), true, 6)], "folders only known from deeper paths are listed too");
        assert_eq!(ix.children("jen").into_iter().map(|i| i.path).collect::<Vec<_>>(), ["jen\\v cestě"]);
        assert!(ix.is_dir("jen\\v cestě") && ix.is_dir("") && !ix.is_dir("a.txt") && !ix.is_dir("je"));
        assert!(ix.exists("a.txt") && !ix.exists("b.txt"));
        assert_eq!(ix.size_of("složka 日本"), 6);
        assert_eq!(ix.stats("jen"), Stats { bytes: 10, files: 1, dirs: 1 });
        assert_eq!(ix.stats(""), Stats { bytes: 19, files: 3, dirs: 3 });
    }

    #[test]
    fn connection_roots() {
        let root = mount_root("scp", "me@host:2222");
        assert_eq!(mount_of(&root), Some(("scp".into(), "me@host:2222".into(), String::new())));
        assert_eq!(mount_of(&root.join("home").join("me")), Some(("scp".into(), "me@host:2222".into(), "home\\me".into())));
        assert!(root.is_absolute() && root.join("a").parent() == Some(root.as_path()));
        assert_eq!(mount_of(Path::new(r"C:\x")), None);
        assert_eq!(mount_of(Path::new(r"\\?\C:\x")), None);
        assert_eq!(mount_of(Path::new(r"\\?\Volume{1}\x")), None);
        assert_eq!(mount_of(Path::new(r"\\server\share")), None);
    }
}
