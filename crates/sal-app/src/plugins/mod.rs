//! Plugins: optional parts of the program, each a DLL of its own (`*.spl`)
//! in a folder under `plugins` next to the program, found when the program
//! starts and turned on and off in Settings → Plugins. The program contains
//! none of them; what it and the plugins share is `sal-plugin-api`.
//!
//! A *folder plugin* shows things as folders: the panel browses them, and
//! the file operations work inside them through the plugin. Paths there are
//! virtual:
//!
//! - through a file the plugin handles (an archive): `C:\x\a.zip\dir\f.txt`;
//! - inside a connection the plugin opened from the Plugins menu (a
//!   *mount*): `\\?\scp\me@host\home\f.txt`, shown as
//!   `scp://me@host/home/f.txt`. The open mounts are listed here, with the
//!   plugin each belongs to.

pub mod keys;
pub mod mount;
pub mod ops;

use std::ffi::c_void;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use sal_core::location;
use sal_plugin_api::abi::{self, Descriptor, HostApi, Str, StrOut};
use sal_plugin_api::host::{self, Info};
pub use sal_plugin_api::{join_inner, Command, FieldKind, FolderPlugin, Form, Index, Mount, Panels, Reply, Results, Values};

use crate::i18n::{tr, trf};

/// An open connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub root: PathBuf,
    /// The id of the plugin it belongs to.
    pub plugin: String,
    /// The folder it opened in (the user's home on a server); the drive
    /// selector goes there.
    pub start: PathBuf,
    /// Results a plugin filled (a search): closed, not disconnected.
    pub results: bool,
    /// What the drive selector and the Plugins menu call it (results have
    /// a name of their own); empty for its path.
    pub name: String,
    /// The plugin command that fills results again (`Results::refresh`).
    pub refresh: String,
    /// One of two connections shown side by side (folders compared).
    pub paired: bool,
}

/// Open connections, in the order they were opened.
static MOUNTS: Mutex<Vec<Connection>> = Mutex::new(Vec::new());

/// Remember a connection the plugin `id` opened at `start`; `results` are
/// the name and the refresh command of results it fills (a search).
pub fn add_mount(root: &Path, id: &str, start: &Path, results: Option<(&str, &str)>) {
    let mut m = MOUNTS.lock().unwrap();
    m.retain(|c| c.root != root);
    let (name, refresh) = results.unwrap_or_default();
    m.push(Connection {
        root: root.to_path_buf(),
        plugin: id.to_owned(),
        start: start.to_path_buf(),
        results: results.is_some(),
        name: name.to_owned(),
        refresh: refresh.to_owned(),
        paired: false,
    });
}

/// Mark the connection `root` as one of two shown side by side.
pub fn set_paired(root: &Path) {
    for c in MOUNTS.lock().unwrap().iter_mut().filter(|c| c.root == root) {
        c.paired = true;
    }
}

/// The icon of results at `root`: a comparison's, or a search's.
pub fn results_icon(root: &Path) -> &'static str {
    if MOUNTS.lock().unwrap().iter().any(|c| c.root == root && c.paired) {
        sal_ui::mdi::COMPARE_HORIZONTAL
    } else {
        sal_ui::mdi::MAGNIFY
    }
}

/// Forget the connections of these plugins (turned off: stopping a plugin
/// ended them).
pub fn forget_mounts_of(stopped: &[String]) {
    MOUNTS.lock().unwrap().retain(|c| !stopped.contains(&c.plugin));
}

/// The name of the results at `root` (a search), if it has one: the path
/// bar, the drive button and the banner show it instead of the address.
pub fn results_name(root: &Path) -> Option<String> {
    MOUNTS.lock().unwrap().iter().find(|c| c.root == root && c.results && !c.name.is_empty()).map(|c| c.name.clone())
}

/// Whether `root` holds results a plugin filled (closed, not disconnected).
pub fn is_results(root: &Path) -> bool {
    MOUNTS.lock().unwrap().iter().any(|c| c.root == root && c.results)
}

/// Whether `root` holds a search's results (not a side of a comparison):
/// nothing can be dropped into them.
pub fn is_search_results(root: &Path) -> bool {
    MOUNTS.lock().unwrap().iter().any(|c| c.root == root && c.results && !c.paired)
}

/// Forget a connection (after the plugin closed it).
pub fn remove_mount(root: &Path) {
    MOUNTS.lock().unwrap().retain(|c| c.root != root);
}

/// The open connections.
pub fn connections() -> Vec<Connection> {
    MOUNTS.lock().unwrap().clone()
}

/// The open connections: (root, plugin id).
pub fn mounts() -> Vec<(PathBuf, String)> {
    connections().into_iter().map(|c| (c.root, c.plugin)).collect()
}

/// The root of the connection `path` is in, if it is open.
pub fn mount_root_of(path: &Path) -> Option<(PathBuf, String)> {
    let (scheme, authority, _) = location::virtual_parts(path)?;
    let root = location::virtual_path(&scheme, &authority, &[]);
    mounts().into_iter().find(|(r, _)| *r == root)
}

/// Translates a plugin's text; `ctx` is its `lang` folder (a `PathBuf`).
unsafe extern "C" fn translate(ctx: *mut c_void, msg: Str, out: *const StrOut) {
    (*out).send(&crate::i18n::tr_from(&*(ctx as *const PathBuf), msg.get()));
}

/// Whether this program version is within `min` ..= `max` (empty: no
/// limit; a version that can't be read is no limit either); if not, why,
/// translated ("needs RustySalamander 0.6.0 or later").
pub fn program_fits(min: &str, max: &str) -> Result<(), String> {
    let parse = |s: &str| semver::Version::parse(s.trim()).ok();
    let this = parse(env!("CARGO_PKG_VERSION")).expect("the program's version");
    let (lo, hi) = (parse(min), parse(max));
    if lo.as_ref().is_none_or(|lo| this >= *lo) && hi.as_ref().is_none_or(|hi| this <= *hi) {
        return Ok(());
    }
    Err(match (lo.is_some(), hi.is_some()) {
        (true, true) => trf("needs RustySalamander {min} to {max}", &[("min", &min.trim()), ("max", &max.trim())]),
        (true, false) => trf("needs RustySalamander {min} or later", &[("min", &min.trim())]),
        _ => trf("works with RustySalamander up to {max} only", &[("max", &max.trim())]),
    })
}

/// Where plugins are looked for: `plugins` next to the program
/// (`RUSTYSALAMANDER_PLUGINS` names another folder).
pub fn plugins_dir() -> Option<PathBuf> {
    match std::env::var_os("RUSTYSALAMANDER_PLUGINS") {
        Some(p) => Some(PathBuf::from(p)),
        None => sal_core::config::app_dir().map(|d| d.join("plugins")),
    }
}

/// A plugin found in a DLL; started only when it is on.
pub struct Found {
    pub info: Info,
    pub file: PathBuf,
    /// Its descriptor: a pointer, never a reference (an older plugin's is
    /// shorter than [`Descriptor`]; see `host::info`). The DLL stays loaded.
    desc: *const Descriptor,
    /// What the program offers this plugin: translations from its `lang`
    /// folder. Kept for the session, like the DLL.
    host: &'static HostApi,
}

impl Found {
    /// A plugin found in `file`, with its translations ready.
    fn new(info: Info, file: PathBuf, desc: *const Descriptor) -> Found {
        let lang: &'static PathBuf = Box::leak(Box::new(file.parent().unwrap_or(Path::new(".")).join("lang")));
        let host = Box::leak(Box::new(HostApi {
            version: abi::VERSION,
            ctx: lang as *const PathBuf as *mut c_void,
            translate,
            open_window: crate::pluginwin::open_window,
            repaint: crate::pluginwin::repaint,
        }));
        Found { info, file, desc, host }
    }

    /// The plugin's folder.
    pub fn dir(&self) -> &Path {
        self.file.parent().unwrap_or(Path::new("."))
    }

    /// A text of the plugin (its name, description) in the language in use.
    pub fn tr(&self, msg: &str) -> String {
        crate::i18n::tr_from(&self.dir().join("lang"), msg)
    }

    /// Start the plugin.
    fn start(&self) -> Result<Arc<dyn FolderPlugin>, String> {
        unsafe { host::start(self.desc, self.host, self.dir()) }
    }
}

/// The plugins found, the ones that are on and started, and what went wrong.
#[derive(Default, Clone)]
pub struct Plugins {
    /// Found when the program started, by name.
    pub found: Arc<Vec<Found>>,
    /// Plugin files that couldn't be used, and why.
    pub broken: Vec<(PathBuf, String)>,
    pub active: Vec<Arc<dyn FolderPlugin>>,
    /// Plugins that are on but couldn't start: (id, why).
    pub errors: Vec<(String, String)>,
    /// The folders of plugins updated at this start, and the updates that
    /// couldn't be put in place yet (they are tried again next time).
    pub updated: Vec<String>,
    pub update_failed: Vec<String>,
    /// The shortcuts of the running plugins' commands, in `active`'s order.
    pub keys: Vec<keys::PluginKeys>,
}

impl Plugins {
    /// Find the plugins in [`plugins_dir`] and start all but the disabled
    /// ones; plugin updates downloaded last time are put in place first.
    pub fn load(disabled: &[String]) -> Plugins {
        let dir = plugins_dir();
        let app = sal_core::config::app_dir();
        Plugins::load_with(disabled, || crate::update::at_start(app.as_deref(), dir.as_deref()), |skip| Plugins::scan_skipping(dir.as_deref(), skip))
    }

    /// [`Plugins::load`]: `at_start` puts the updates in place (and returns
    /// the update lock, held until the plugins are loaded: no other running
    /// copy of the program changes them meanwhile), `scan` finds the
    /// plugins but those it is given. When the updates say plugins are to
    /// wait, none is looked for nor started.
    fn load_with(disabled: &[String], at_start: impl FnOnce() -> (crate::update::Started, Option<crate::update::UpdateLock>), scan: impl FnOnce(&[String]) -> Plugins) -> Plugins {
        let (s, update_lock) = at_start();
        if s != crate::update::Started::default() {
            crate::debuglog::write(format!("startup: plugins updated {:?}, not updated {:?}, not loaded {:?}, all waiting {}", s.updated, s.failed, s.skip, s.defer_plugins));
        }
        let mut p = if s.defer_plugins {
            // another running copy is changing files: not even looked for
            Plugins::default()
        } else {
            let mut p = scan(&s.skip);
            p.start(disabled);
            p
        };
        drop(update_lock);
        p.updated = s.updated;
        p.update_failed = s.failed;
        p
    }

    /// Find the plugins in `dir`: `dir\*.spl` and `dir\<folder>\*.spl`.
    #[cfg(test)]
    pub fn scan(dir: Option<&Path>) -> Plugins {
        Plugins::scan_skipping(dir, &[])
    }

    /// [`Plugins::scan`], leaving out the plugin folders `skip` (an update
    /// of them is half done): listed as broken, never loaded.
    pub fn scan_skipping(dir: Option<&Path>, skip: &[String]) -> Plugins {
        let mut files = Vec::new();
        let mut broken = Vec::new();
        let is_plugin = |p: &Path| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case(abi::EXTENSION));
        if let Some(Ok(rd)) = dir.map(fs::read_dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| skip.iter().any(|s| s == n)) {
                    broken.push((p, tr("its update is half done; it is finished or undone when RustySalamander starts next time").to_owned()));
                } else if p.is_dir() {
                    if let Ok(sub) = fs::read_dir(&p) {
                        files.extend(sub.flatten().map(|e| e.path()).filter(|p| is_plugin(p)));
                    }
                } else if is_plugin(&p) && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| skip.iter().any(|s| s == n)) {
                    broken.push((p, tr("its update is half done; it is finished or undone when RustySalamander starts next time").to_owned()));
                } else if is_plugin(&p) {
                    files.push(p);
                }
            }
        }
        files.sort();
        let mut found: Vec<Found> = Vec::new();
        for file in files {
            match load_library(&file) {
                Ok(desc) => match unsafe { host::info(desc) } {
                    Ok(info) if found.iter().any(|f| f.info.id == info.id) => broken.push((file, format!("another plugin has the id \"{}\"", info.id))),
                    Ok(info) if program_fits(&info.program_min, &info.program_max).is_err() => {
                        let why = program_fits(&info.program_min, &info.program_max).unwrap_err();
                        broken.push((file, trf("{why} (this is {version})", &[("why", &why.as_str()), ("version", &env!("CARGO_PKG_VERSION"))])))
                    }
                    Ok(info) => found.push(Found::new(info, file, desc)),
                    Err(e) => broken.push((file, e)),
                },
                Err(e) => broken.push((file, e)),
            }
        }
        found.sort_by_key(|a| a.info.name.to_lowercase());
        Plugins { found: Arc::new(found), broken, active: Vec::new(), errors: Vec::new(), updated: Vec::new(), update_failed: Vec::new(), keys: Vec::new() }
    }

    /// Start the plugins that are on and stop the others; running ones keep
    /// running.
    pub fn start(&mut self, disabled: &[String]) {
        let running = std::mem::take(&mut self.active);
        self.errors.clear();
        for f in self.found.iter().filter(|f| !disabled.contains(&f.info.id)) {
            if let Some(p) = running.iter().find(|p| p.id() == f.info.id) {
                self.active.push(p.clone());
                continue;
            }
            match f.start() {
                Ok(p) => self.active.push(p),
                Err(e) => self.errors.push((f.info.id.clone(), e)),
            }
        }
        self.refresh_keys();
    }

    /// Ask the running plugins which of their commands can have keys again
    /// (their labels change with the language); the user's keys of a
    /// plugin are read from its folder when it starts.
    pub fn refresh_keys(&mut self) {
        let old = std::mem::take(&mut self.keys);
        for p in &self.active {
            let list = keys::offered(&p.commands(), &p.hotkeys());
            let k = match old.iter().find(|k| k.plugin == p.id()) {
                Some(k) => keys::PluginKeys { list, ..k.clone() },
                None => {
                    let file = self.found.iter().find(|f| f.info.id == p.id()).map(|f| sal_plugin_api::data_dir(f.dir()).join(keys::FILE)).unwrap_or_default();
                    keys::PluginKeys::load(p.id(), file, list)
                }
            };
            self.keys.push(k);
        }
    }

    /// The shortcuts of the running plugin `id`.
    pub fn keys_of_mut(&mut self, id: &str) -> Option<&mut keys::PluginKeys> {
        self.keys.iter_mut().find(|k| k.plugin == id)
    }

    /// The plugin and its command that a key press runs, if any.
    pub fn key_command(&self, key: eframe::egui::Key, m: eframe::egui::Modifiers) -> Option<(Arc<dyn FolderPlugin>, String)> {
        self.keys.iter().find_map(|k| Some((self.get(&k.plugin)?.clone(), k.lookup(key, m)?.to_owned())))
    }

    /// The first key of the plugin `id`'s command, for its menu item.
    pub fn key_text(&self, id: &str, command: &str) -> String {
        self.keys.iter().find(|k| k.plugin == id).and_then(|k| k.bindings(command).into_iter().next()).unwrap_or_default()
    }

    /// The running plugin with this id.
    pub fn get(&self, id: &str) -> Option<&Arc<dyn FolderPlugin>> {
        self.active.iter().find(|p| p.id() == id)
    }

    /// A plugin's name in the language in use (its id when it wasn't found
    /// in a DLL, as in tests).
    pub fn name_of(&self, id: &str) -> String {
        self.found.iter().find(|f| f.info.id == id).map(|f| f.tr(&f.info.name)).unwrap_or_else(|| id.to_owned())
    }

    /// The plugin, file and path inside it, when `path` goes through a file
    /// that an active plugin shows as a folder.
    pub fn split(&self, path: &Path) -> Option<(Arc<dyn FolderPlugin>, PathBuf, String)> {
        split(&self.active, path)
    }
}

/// The descriptor of the plugin DLL `file`. The DLL stays loaded for the
/// rest of the session (plugins started from it may be running).
fn load_library(file: &Path) -> Result<*const Descriptor, String> {
    use windows::core::{PCSTR, PCWSTR};
    use windows::Win32::Foundation::FreeLibrary;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR};

    #[link(name = "kernel32")]
    extern "system" {
        fn SetThreadErrorMode(new: u32, old: *mut u32) -> i32;
    }
    // SEM_FAILCRITICALERRORS: a damaged DLL fails with an error instead of a
    // Windows "Bad Image" box that blocks the start until it is dismissed
    const SEM_FAILCRITICALERRORS: u32 = 1;

    let file = std::path::absolute(file).map_err(|e| e.to_string())?;
    let wide: Vec<u16> = abi::Wide::encode(&file).into_iter().chain([0]).collect();
    // the plugin's own folder is searched for what it needs
    let module = unsafe {
        let mut old = 0;
        SetThreadErrorMode(SEM_FAILCRITICALERRORS, &mut old);
        let m = LoadLibraryExW(PCWSTR(wide.as_ptr()), None, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
        SetThreadErrorMode(old, std::ptr::null_mut());
        m
    }
    .map_err(|e| e.message().to_string())?;
    let entry = format!("{}\0", abi::ENTRY);
    let Some(f) = (unsafe { GetProcAddress(module, PCSTR(entry.as_ptr())) }) else {
        unsafe {
            let _ = FreeLibrary(module);
        }
        return Err("not a RustySalamander plugin".into());
    };
    let entry: extern "C" fn() -> *const Descriptor = unsafe { std::mem::transmute(f) };
    let desc = entry();
    if desc.is_null() {
        return Err("not a RustySalamander plugin".into());
    }
    Ok(desc)
}

/// The plugin, file and path inside it (`\`-separated, empty at the file's
/// root) for a path that goes through a file one of `plugins` handles.
pub fn split(plugins: &[Arc<dyn FolderPlugin>], path: &Path) -> Option<(Arc<dyn FolderPlugin>, PathBuf, String)> {
    if plugins.is_empty() {
        return None;
    }
    // inside an open connection
    if let Some((_, _, names)) = location::virtual_parts(path) {
        let (root, id) = mount_root_of(path)?;
        let plugin = plugins.iter().find(|p| p.id() == id)?;
        return Some((plugin.clone(), root, names.join("\\")));
    }
    // without a trailing separator: "a.zip\" names the file a.zip
    let path: PathBuf = path.components().collect();
    for anc in path.ancestors() {
        let Some(name) = anc.file_name().and_then(|n| n.to_str()) else { continue };
        let Some(plugin) = plugins.iter().find(|p| p.handles(name)) else { continue };
        if anc.is_file() {
            let rest = path.strip_prefix(anc).ok()?;
            let inner: Vec<String> = rest.components().filter_map(|c| if let Component::Normal(s) = c { Some(s.to_string_lossy().into_owned()) } else { None }).collect();
            return Some((plugin.clone(), anc.to_path_buf(), inner.join("\\")));
        }
    }
    None
}

/// Size, files and folders inside the folder `inner` of `index`.
/// What [`stats`] gives for each folder directly in `inner`, by its name,
/// in one pass over the index (not one pass per folder).
pub fn child_stats(index: &Index, inner: &str) -> std::collections::HashMap<String, sal_core::fileops::DirSize> {
    use std::collections::{HashMap, HashSet};
    let mut found: HashMap<&str, (sal_core::fileops::DirSize, HashSet<String>)> = HashMap::new();
    for it in index.items.iter().filter(|it| it.path.len() > inner.len() && sal_plugin_api::under(&it.path, inner)) {
        let rest = if inner.is_empty() { &it.path[..] } else { &it.path[inner.len() + 1..] };
        let Some((child, below)) = rest.split_once('\\') else {
            // directly in `inner`: a folder is there (empty, maybe); a file isn't one
            if it.dir {
                found.entry(rest).or_default();
            }
            continue;
        };
        let (size, dirs) = found.entry(child).or_default();
        if it.dir {
            dirs.insert(below.to_owned());
        } else {
            size.files += 1;
            size.bytes += it.size;
        }
        // folders known only from the paths of what is inside them
        let mut parts: Vec<&str> = below.split('\\').collect();
        parts.pop();
        for n in 1..=parts.len() {
            dirs.insert(parts[..n].join("\\"));
        }
    }
    found.into_iter().map(|(name, (mut size, dirs))| {
        size.dirs = dirs.len() as u64;
        (name.to_owned(), size)
    }).collect()
}

#[cfg(test)]
pub fn stats(index: &Index, inner: &str) -> sal_core::fileops::DirSize {
    let s = index.stats(inner);
    sal_core::fileops::DirSize { bytes: s.bytes, files: s.files, dirs: s.dirs, ..Default::default() }
}

/// A unique new folder for temporary files of this session, under
/// `%TEMP%\RustySalamander`. [`cleanup_temp`] removes them all.
pub fn temp_dir(tag: &str) -> io::Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join("RustySalamander").join(format!("{}-{n}-{tag}", std::process::id()));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Remove this session's temporary folders.
pub fn cleanup_temp() {
    let base = std::env::temp_dir().join("RustySalamander");
    let mine = format!("{}-", std::process::id());
    if let Ok(rd) = fs::read_dir(&base) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&mine) {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }
}

#[cfg(test)]
mod load_tests {
    use super::*;

    /// The sizes of an archive's folders in one pass are those of each
    /// folder on its own: files, bytes, folders (those only in paths too).
    #[test]
    fn child_stats_in_one_pass() {
        let item = |path: &str, dir: bool, size: u64| sal_plugin_api::Item { path: path.into(), dir, size, modified: None, attrs: 0, index: None };
        let index = Index {
            items: vec![
                item("a", true, 0),
                item("a\\x.txt", false, 10),
                item("a\\deep\\er\\y.bin", false, 5),
                item("a\\empty", true, 0),
                item("b\\only-in-paths\\z", false, 7),
                item("c", true, 0),
                item("top.txt", false, 100),
                item("ž\\kůň.txt", false, 1),
            ],
            complete: true,
        };
        for inner in ["", "a", "a\\deep", "b"] {
            let all = child_stats(&index, inner);
            let names: std::collections::HashSet<String> = index
                .items
                .iter()
                .filter(|it| it.path.len() > inner.len() && sal_plugin_api::under(&it.path, inner))
                .filter_map(|it| {
                    let rest = if inner.is_empty() { &it.path[..] } else { &it.path[inner.len() + 1..] };
                    rest.split_once('\\').map(|(c, _)| c.to_owned()).or_else(|| it.dir.then(|| rest.to_owned()))
                })
                .collect();
            assert_eq!(all.len(), names.len(), "{inner:?}: {all:?}");
            for name in names {
                assert_eq!(all[&name], stats(&index, &join_inner(inner, &name)), "{inner:?} {name}");
            }
        }
        assert_eq!(child_stats(&index, "")["a"], sal_core::fileops::DirSize { bytes: 15, files: 2, dirs: 3, ..Default::default() });
    }

    /// While another running copy changes files, the start looks for no
    /// plugin at all: one it puts in place after the start learnt that would
    /// otherwise be found (and loaded half there).
    #[test]
    fn plugins_wait_for_another_copys_update() {
        let t = tempfile::tempdir().unwrap();
        let plugins = t.path().join("plugins");
        fs::create_dir_all(&plugins).unwrap();
        let scanned = std::cell::Cell::new(false);
        let p = Plugins::load_with(
            &[],
            || {
                let s = crate::update::Started { failed: vec!["another running RustySalamander is updating".into()], defer_plugins: true, ..Default::default() };
                // the other copy puts a plugin in place right after
                fs::create_dir_all(plugins.join("new")).unwrap();
                fs::write(plugins.join("new").join("new.spl"), "half").unwrap();
                (s, None)
            },
            |skip| {
                scanned.set(true);
                Plugins::scan_skipping(Some(&plugins), skip)
            },
        );
        assert!(!scanned.get(), "not looked for");
        assert!(p.found.is_empty() && p.active.is_empty() && p.broken.is_empty());
        assert!(p.update_failed[0].contains("another running RustySalamander"), "why");
        // otherwise looked for
        let p = Plugins::load_with(
            &[],
            || (crate::update::Started::default(), None),
            |skip| {
                scanned.set(true);
                Plugins::scan_skipping(Some(&plugins), skip)
            },
        );
        assert!(scanned.get());
        assert_eq!(p.broken.len(), 1, "the half plugin found (and refused: not a DLL)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sal_plugin_api::Progress;

    /// A plugin for `.fake` files, for testing paths without a library.
    struct Fake;
    impl FolderPlugin for Fake {
        fn id(&self) -> &str {
            "fake"
        }
        fn handles(&self, name: &str) -> bool {
            name.ends_with(".fake")
        }
        fn can_write(&self, _: &Path) -> bool {
            false
        }
        fn status(&self) -> String {
            String::new()
        }
        fn list(&self, _: &Path, _: &str) -> io::Result<Index> {
            Ok(Index::default())
        }
        fn extract(&self, _: &Path, _: &[String], _: &Path, _: &mut Progress) -> io::Result<()> {
            Ok(())
        }
        fn add(&self, _: &Path, _: &[(PathBuf, String)], _: &mut Progress) -> io::Result<()> {
            Ok(())
        }
        fn delete(&self, _: &Path, _: &[String], _: &mut Progress) -> io::Result<()> {
            Ok(())
        }
        fn rename(&self, _: &Path, _: &str, _: &str, _: &mut Progress) -> io::Result<()> {
            Ok(())
        }
        fn mkdir(&self, _: &Path, _: &str, _: &mut Progress) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn paths_through_files() {
        let plugins: Vec<Arc<dyn FolderPlugin>> = vec![Arc::new(Fake)];
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("a b.fake");
        fs::write(&file, b"x").unwrap();
        let s = |p: &Path| split(&plugins, p).map(|(pl, f, i)| (pl.id().to_owned(), f, i));
        let fake = || "fake".to_owned();
        assert_eq!(s(&file), Some((fake(), file.clone(), String::new())));
        assert_eq!(s(&file.join("dir").join("f.txt")), Some((fake(), file.clone(), "dir\\f.txt".into())));
        // the Copy dialog's targets end with a separator
        let mut slash = file.clone().into_os_string();
        slash.push("\\");
        assert_eq!(s(Path::new(&slash)), Some((fake(), file.clone(), String::new())));
        assert_eq!(s(t.path()), None);
        // a folder named like such a file is an ordinary folder
        fs::create_dir(t.path().join("x.fake")).unwrap();
        assert_eq!(s(&t.path().join("x.fake").join("f")), None);
        assert!(split(&[], &file).is_none(), "no plugins, no virtual folders");
    }

    /// The plugins folder the build made in target\<profile>.
    pub fn built_plugins() -> Option<PathBuf> {
        let dir = std::env::current_exe().ok()?.parent()?.parent()?.join("plugins");
        dir.join("7zip").join("7zip.spl").is_file().then_some(dir)
    }

    #[test]
    fn finds_plugins_in_their_folders() {
        let t = tempfile::tempdir().unwrap();
        assert!(Plugins::scan(Some(t.path())).found.is_empty() && Plugins::scan(None).found.is_empty());
        // files that are not plugins
        fs::create_dir(t.path().join("bad")).unwrap();
        fs::write(t.path().join("bad").join("bad.spl"), "MZ, but not a DLL").unwrap();
        fs::write(t.path().join("ignored.txt"), "x").unwrap();
        let Some(built) = built_plugins() else {
            eprintln!("plugins not built; skipping");
            return;
        };
        // one copy in a folder of its own, one loose: the same plugin twice
        let mine = t.path().join("7zip");
        fs::create_dir(&mine).unwrap();
        for f in ["7zip.spl", "7z.dll"] {
            fs::copy(built.join("7zip").join(f), mine.join(f)).unwrap();
        }
        fs::copy(built.join("7zip").join("7zip.spl"), t.path().join("dup.spl")).unwrap();
        let mut p = Plugins::scan(Some(t.path()));
        assert_eq!(p.found.iter().map(|f| f.info.id.as_str()).collect::<Vec<_>>(), ["7zip"]);
        assert_eq!(p.found[0].info.name, "7-Zip archives");
        let why: Vec<String> = p.broken.iter().map(|(f, e)| format!("{}: {e}", f.file_name().unwrap().to_string_lossy())).collect();
        assert_eq!(why.len(), 2, "{why:?}");
        assert!(why.iter().any(|w| w.starts_with("bad.spl:")) && why.iter().any(|w| w.starts_with("dup.spl: another plugin")), "{why:?}");

        p.start(&[]);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        let z = p.get("7zip").unwrap().clone();
        assert!(z.status().contains("7z.dll") && z.handles("a.zip"));
        // turned off and on: a running plugin is kept, a stopped one starts again
        p.start(&["7zip".into()]);
        assert!(p.active.is_empty());
        p.start(&[]);
        assert!(!Arc::ptr_eq(p.get("7zip").unwrap(), &z));
        let again = p.get("7zip").unwrap().clone();
        p.start(&[]);
        assert!(Arc::ptr_eq(p.get("7zip").unwrap(), &again));
        // without 7z.dll it says why
        let alone = t.path().join("alone");
        fs::create_dir_all(alone.join("7zip")).unwrap();
        fs::copy(built.join("7zip").join("7zip.spl"), alone.join("7zip").join("7zip.spl")).unwrap();
        let mut p = Plugins::scan(Some(&alone));
        p.start(&[]);
        assert!(p.active.is_empty());
        assert!(p.errors[0].1.starts_with("7z.dll is missing in"), "{:?}", p.errors);
    }
}
