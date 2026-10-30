//! Plugins: optional parts of the program, each a DLL of its own (`*.spl`)
//! in a folder under `plugins` next to the program, found when the program
//! starts and turned on and off in Settings → Plugins. The program contains
//! none of them; what it and the plugins share is `sal-plugin-api`.
//!
//! A *folder plugin* shows certain files as folders (archives): the panel
//! browses them, and the file operations work inside them through the
//! plugin. A path that goes through such a file is virtual:
//! `C:\x\a.zip\dir\f.txt`.

pub mod ops;

use std::ffi::c_void;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use sal_plugin_api::abi::{self, Descriptor, HostApi, Str, StrOut};
use sal_plugin_api::host::{self, Info};
pub use sal_plugin_api::{join_inner, FolderPlugin, Index};

/// Translates a plugin's text; `ctx` is its `lang` folder (a `PathBuf`).
unsafe extern "C" fn translate(ctx: *mut c_void, msg: Str, out: *const StrOut) {
    (*out).send(&crate::i18n::tr_from(&*(ctx as *const PathBuf), msg.get()));
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
    desc: &'static Descriptor,
    /// What the program offers this plugin: translations from its `lang`
    /// folder. Kept for the session, like the DLL.
    host: &'static HostApi,
}

impl Found {
    fn new(info: Info, file: PathBuf, desc: &'static Descriptor) -> Found {
        let lang: &'static PathBuf = Box::leak(Box::new(file.parent().unwrap_or(Path::new(".")).join("lang")));
        let host = Box::leak(Box::new(HostApi { version: abi::VERSION, ctx: lang as *const PathBuf as *mut c_void, translate }));
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
}

impl Plugins {
    /// Find the plugins in [`plugins_dir`] and start all but the disabled ones.
    pub fn load(disabled: &[String]) -> Plugins {
        let mut p = Plugins::scan(plugins_dir().as_deref());
        p.start(disabled);
        p
    }

    /// Find the plugins in `dir`: `dir\*.spl` and `dir\<folder>\*.spl`.
    pub fn scan(dir: Option<&Path>) -> Plugins {
        let mut files = Vec::new();
        let is_plugin = |p: &Path| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case(abi::EXTENSION));
        if let Some(Ok(rd)) = dir.map(fs::read_dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if let Ok(sub) = fs::read_dir(&p) {
                        files.extend(sub.flatten().map(|e| e.path()).filter(|p| is_plugin(p)));
                    }
                } else if is_plugin(&p) {
                    files.push(p);
                }
            }
        }
        files.sort();
        let mut found: Vec<Found> = Vec::new();
        let mut broken = Vec::new();
        for file in files {
            match load_library(&file) {
                Ok(desc) => match unsafe { host::info(desc) } {
                    Ok(info) if found.iter().any(|f| f.info.id == info.id) => broken.push((file, format!("another plugin has the id \"{}\"", info.id))),
                    Ok(info) => found.push(Found::new(info, file, desc)),
                    Err(e) => broken.push((file, e)),
                },
                Err(e) => broken.push((file, e)),
            }
        }
        found.sort_by_key(|a| a.info.name.to_lowercase());
        Plugins { found: Arc::new(found), broken, active: Vec::new(), errors: Vec::new() }
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
    }

    pub fn get(&self, id: &str) -> Option<&Arc<dyn FolderPlugin>> {
        self.active.iter().find(|p| p.id() == id)
    }

    /// The plugin, file and path inside it, when `path` goes through a file
    /// that an active plugin shows as a folder.
    pub fn split(&self, path: &Path) -> Option<(Arc<dyn FolderPlugin>, PathBuf, String)> {
        split(&self.active, path)
    }
}

/// The descriptor of the plugin DLL `file`. The DLL stays loaded for the
/// rest of the session (plugins started from it may be running).
fn load_library(file: &Path) -> Result<&'static Descriptor, String> {
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
    Ok(unsafe { &*desc })
}

/// The plugin, file and path inside it (`\`-separated, empty at the file's
/// root) for a path that goes through a file one of `plugins` handles.
pub fn split(plugins: &[Arc<dyn FolderPlugin>], path: &Path) -> Option<(Arc<dyn FolderPlugin>, PathBuf, String)> {
    if plugins.is_empty() {
        return None;
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
        fn list(&self, _: &Path) -> io::Result<Index> {
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
