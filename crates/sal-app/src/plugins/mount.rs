//! A plugin's items as files on disk, for dragging them out of the window.
//!
//! Windows' Projected File System (ProjFS, an optional Windows feature)
//! lets a program fill an ordinary folder on demand: other programs see
//! normal files and folders there, and Windows asks the program for each
//! folder's contents when one is listed and for each file's bytes when one
//! is first read. A [`Mount`] does that for one plugin file or connection
//! (an archive, a server), in a folder under `%TEMP%\RustySalamander`:
//!
//! - folders are listed from the plugin's [`FolderPlugin::list`] (a
//!   [`Tree`]), only when something looks into them;
//! - files are read through the plugin's [`FolderPlugin::stream`] (a
//!   [`Feed`]), only when a program opens them, and stay there afterwards.
//!
//! A drag out of the window then carries the files' paths in that folder,
//! like any files on disk: Explorer, the desktop and programs that take only
//! paths copy them as usual, and only what they read is read from the
//! plugin.
//!
//! The plugin sends files in its own order (an archive's own order is the
//! quick one: a solid archive is decoded once), while the program they are
//! dropped on reads them in its order. The items of the drag are sent in one
//! run: a file read while the plugin sends it goes straight to the reader,
//! one the plugin sends before anyone asks for it is kept in a spool folder
//! until it is read. A file outside the drag is sent on its own. A plugin
//! that can't stream extracts the items into the spool folder instead.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use sal_plugin_api::{under, FolderPlugin, Item, Sink, ATTR_DIRECTORY};

/// `inner`'s folder: `a\b\c` → `a\b`, `c` → ``.
fn parent_of(inner: &str) -> &str {
    inner.rsplit_once('\\').map_or("", |(p, _)| p)
}

/// `inner`'s name: `a\b\c` → `c`.
fn leaf_of(inner: &str) -> &str {
    inner.rsplit_once('\\').map_or(inner, |(_, n)| n)
}

/// Windows compares names without case: the key of a path.
fn key(inner: &str) -> String {
    inner.to_lowercase()
}

/// Whether Windows can have a file or folder called `name`.
fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.ends_with([' ', '.']) && !name.chars().any(|c| c < ' ' || "<>:\"/|?*".contains(c))
}

/// One folder's items, without names Windows can't have and without
/// duplicates (the last one of a name counts).
fn clean(items: impl IntoIterator<Item = Item>) -> Vec<Item> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Item> = Vec::new();
    for it in items.into_iter().filter(|it| valid_name(leaf_of(&it.path))) {
        match seen.get(&key(leaf_of(&it.path))) {
            Some(&k) => out[k] = it,
            None => {
                seen.insert(key(leaf_of(&it.path)), out.len());
                out.push(it);
            }
        }
    }
    out
}

/// A complete index by folder (key of its path): what each one holds,
/// folders known only from the paths inside them too.
fn by_folder(items: &[Item]) -> HashMap<String, Arc<Vec<Item>>> {
    let mut dirs: HashMap<String, Item> = HashMap::new();
    let mut files: Vec<Item> = Vec::new();
    let folder = |p: &str| Item { path: p.to_owned(), dir: true, size: 0, modified: None, attrs: ATTR_DIRECTORY, index: None };
    for it in items {
        if it.dir {
            let d = dirs.entry(key(&it.path)).or_insert_with(|| folder(&it.path));
            d.modified = it.modified;
            d.attrs = it.attrs | ATTR_DIRECTORY;
        } else {
            files.push(it.clone());
        }
        let mut p = parent_of(&it.path);
        while !p.is_empty() {
            dirs.entry(key(p)).or_insert_with(|| folder(p));
            p = parent_of(p);
        }
    }
    let mut out: HashMap<String, Vec<Item>> = HashMap::new();
    out.insert(String::new(), Vec::new());
    for it in dirs.into_values().chain(files) {
        out.entry(key(parent_of(&it.path))).or_default().push(it);
    }
    out.into_iter().map(|(k, v)| (k, Arc::new(clean(v)))).collect()
}

/// The folders of a plugin's file or connection, listed as they are looked
/// into and remembered.
pub(crate) struct Tree {
    plugin: Arc<dyn FolderPlugin>,
    file: PathBuf,
    state: Mutex<TreeState>,
}

#[derive(Default)]
struct TreeState {
    /// A complete index (an archive), by folder.
    all: Option<Arc<HashMap<String, Arc<Vec<Item>>>>>,
    /// Folders listed one at a time (a server), by key.
    dirs: HashMap<String, Arc<Vec<Item>>>,
}

impl Tree {
    pub fn new(plugin: Arc<dyn FolderPlugin>, file: PathBuf) -> Tree {
        Tree { plugin, file, state: Mutex::new(TreeState::default()) }
    }

    /// Forget what was listed: it is listed again when looked into.
    pub fn forget(&self) {
        *self.state.lock().unwrap() = TreeState::default();
    }

    /// The items directly in the folder `dir` (its path as the plugin has
    /// it).
    pub fn children(&self, dir: &str) -> io::Result<Arc<Vec<Item>>> {
        let k = key(dir);
        {
            let s = self.state.lock().unwrap();
            if let Some(all) = &s.all {
                return Ok(all.get(&k).cloned().unwrap_or_default());
            }
            if let Some(d) = s.dirs.get(&k) {
                return Ok(d.clone());
            }
        }
        let index = self.plugin.list(&self.file, dir)?;
        let mut s = self.state.lock().unwrap();
        if index.complete {
            let all = Arc::new(by_folder(&index.items));
            s.all = Some(all.clone());
            return Ok(all.get(&k).cloned().unwrap_or_default());
        }
        let items = Arc::new(clean(index.children(dir)));
        s.dirs.insert(k, items.clone());
        Ok(items)
    }

    /// The item at `rel` (in any case), with the path the plugin has it
    /// under; `None` when there is none. The root is a folder with an empty
    /// path.
    pub fn find(&self, rel: &str) -> io::Result<Option<Item>> {
        let mut found = Item { path: String::new(), dir: true, size: 0, modified: None, attrs: ATTR_DIRECTORY, index: None };
        for part in rel.split('\\').filter(|p| !p.is_empty()) {
            if !found.dir {
                return Ok(None);
            }
            let want = key(part);
            match self.children(&found.path)?.iter().find(|it| key(leaf_of(&it.path)) == want) {
                Some(it) => found = it.clone(),
                None => return Ok(None),
            }
        }
        Ok(Some(found))
    }

    /// The files at or below `items` (paths as the plugin has them).
    pub fn files_under(&self, items: &[String]) -> io::Result<Vec<Item>> {
        let mut out = Vec::new();
        let mut folders: VecDeque<String> = VecDeque::new();
        for inner in items {
            match self.find(inner)? {
                Some(it) if it.dir => folders.push_back(it.path),
                Some(it) => out.push(it),
                None => {}
            }
        }
        while let Some(f) = folders.pop_front() {
            for it in self.children(&f)?.iter() {
                if it.dir {
                    folders.push_back(it.path.clone());
                } else {
                    out.push(it.clone());
                }
            }
        }
        Ok(out)
    }
}

/// How many chunks the plugin may be ahead of a reader.
const AHEAD: usize = 16;

/// How much the spool may hold before the plugin waits for readers (unless
/// a reader waits for a file the plugin hasn't sent yet).
const SPOOL_LIMIT: u64 = 512 << 20;

/// What readers and the plugin's runs share.
#[derive(Default)]
struct State {
    /// The items of the last drag, not yet sent: the first file of them
    /// read sends them all.
    prepared: Vec<String>,
    /// The files (keys) each run will still send.
    runs: HashMap<u64, HashSet<String>>,
    next_run: u64,
    /// Files sent before they were read, by key.
    spooled: HashMap<String, Spooled>,
    /// Their size together.
    spool_bytes: u64,
    next_spool: u64,
    /// Readers waiting for a run to send their file: key → slot.
    waiting: HashMap<String, u64>,
    /// A file on its way straight to a reader, by slot.
    slots: HashMap<u64, Slot>,
    next_slot: u64,
    /// Unmounted: everything stops.
    closed: bool,
}

/// A file sent before it was read.
struct Spooled {
    path: PathBuf,
    size: u64,
    /// Complete (else the plugin is still sending it).
    done: bool,
}

/// A file going straight from the plugin to a reader.
#[derive(Default)]
struct Slot {
    /// The plugin has started sending it.
    begun: bool,
    chunks: VecDeque<Vec<u8>>,
    /// It is complete.
    ended: bool,
    /// Why it didn't come, or stopped.
    failed: Option<String>,
    /// The run ended without sending it.
    missed: bool,
}

pub(crate) struct Shared {
    state: Mutex<State>,
    cv: Condvar,
}

/// The bytes of a plugin's files for readers, sent by the plugin in its own
/// order.
pub(crate) struct Feed {
    plugin: Arc<dyn FolderPlugin>,
    file: PathBuf,
    tree: Arc<Tree>,
    /// Where files sent before they are read wait.
    spool: PathBuf,
    /// Whether a file is already there for good (read before): not sent
    /// again.
    present: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    shared: Arc<Shared>,
}

impl Feed {
    pub fn new(plugin: Arc<dyn FolderPlugin>, file: PathBuf, tree: Arc<Tree>, spool: PathBuf, present: Arc<dyn Fn(&str) -> bool + Send + Sync>) -> Feed {
        Feed { plugin, file, tree, spool, present, shared: Arc::new(Shared { state: Mutex::new(State::default()), cv: Condvar::new() }) }
    }

    /// The items of a drag (paths as the plugin has them): when the first
    /// of their files is read, the plugin sends them all, in its order.
    /// What an earlier drag's run left unread is dropped.
    pub fn prepare(&self, items: Vec<String>) {
        let mut s = self.shared.state.lock().unwrap();
        let done: Vec<String> = s.spooled.iter().filter(|(_, sp)| sp.done).map(|(k, _)| k.clone()).collect();
        for k in done {
            let sp = s.spooled.remove(&k).expect("listed");
            s.spool_bytes -= sp.size;
            let _ = fs::remove_file(sp.path);
        }
        s.prepared = items;
        self.shared.cv.notify_all();
    }

    /// Stop: runs end, readers get an error.
    pub fn close(&self) {
        self.shared.state.lock().unwrap().closed = true;
        self.shared.cv.notify_all();
    }

    /// Open the file `inner` (its path as the plugin has it) for reading.
    /// Blocks until its first bytes are on their way.
    pub fn open(&self, inner: &str) -> io::Result<Source> {
        let k = key(inner);
        let mut s = self.shared.state.lock().unwrap();
        // waiting for a run to send it; `single`: a run of its own
        let mut slot: Option<(u64, bool)> = None;
        loop {
            if s.closed {
                return Err(io::ErrorKind::Interrupted.into());
            }
            if let Some((id, single)) = slot {
                let st = s.slots.get(&id).expect("ours");
                if st.begun {
                    return Ok(Source::Direct { shared: self.shared.clone(), id });
                }
                if st.missed {
                    let why = st.failed.clone();
                    s.slots.remove(&id);
                    if single {
                        return Err(io::Error::other(why.unwrap_or_else(|| format!("{inner}: the plugin didn't send it"))));
                    }
                    // the drag's run didn't send it after all: on its own
                    slot = None;
                    continue;
                }
                s = self.shared.cv.wait(s).unwrap();
                continue;
            }
            if let Some(sp) = s.spooled.get(&k) {
                if sp.done {
                    let sp = s.spooled.remove(&k).expect("just seen");
                    s.spool_bytes -= sp.size;
                    self.shared.cv.notify_all();
                    drop(s);
                    return Ok(Source::Spooled { file: fs::File::open(&sp.path)?, path: sp.path });
                }
                s = self.shared.cv.wait(s).unwrap();
                continue;
            }
            let in_run = s.runs.values().any(|r| r.contains(&k));
            if !in_run && s.prepared.iter().any(|w| under(&k, &key(w))) {
                // the drag's items, all in one run
                let items = std::mem::take(&mut s.prepared);
                drop(s);
                let files = self.tree.files_under(&items);
                s = self.shared.state.lock().unwrap();
                if let Ok(files) = files {
                    let keys = files.iter().map(|f| key(&f.path)).collect();
                    self.start(&mut s, items, keys);
                }
                continue;
            }
            let id = s.next_slot;
            s.next_slot += 1;
            s.slots.insert(id, Slot::default());
            if let Some(old) = s.waiting.insert(k.clone(), id) {
                // someone else waited for it: they try again
                if let Some(o) = s.slots.get_mut(&old) {
                    o.missed = true;
                }
            }
            slot = Some((id, !in_run));
            if !in_run {
                self.start(&mut s, vec![inner.to_owned()], HashSet::from([k.clone()]));
            }
            self.shared.cv.notify_all();
        }
    }

    /// Run the plugin on a thread of its own, sending `items`, whose files
    /// are `keys`.
    fn start(&self, s: &mut State, items: Vec<String>, keys: HashSet<String>) {
        let run = s.next_run;
        s.next_run += 1;
        s.runs.insert(run, keys);
        let (plugin, file, spool) = (self.plugin.clone(), self.file.clone(), self.spool.clone());
        let mut sink = Feeding { shared: self.shared.clone(), run, spool: spool.clone(), present: self.present.clone(), target: Target::None };
        std::thread::spawn(move || {
            let r = match plugin.stream(&file, &items, &mut sink) {
                Err(e) if e.kind() == io::ErrorKind::Unsupported => from_disk(&*plugin, &file, &items, &spool, run, &mut sink),
                r => r,
            };
            let why = match r {
                Ok(()) => None,
                Err(e) => Some(format!("{}: {e}", sal_core::encoding::display_path(&file))),
            };
            sink.finish(why);
        });
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.close();
    }
}

/// Where the bytes the plugin sends now go.
enum Target {
    None,
    /// Straight to a reader.
    Slot(u64),
    /// Into the spool, under this key.
    Spool(String, fs::File),
    /// Nowhere: not wanted.
    Skip,
}

/// A run's side of the feed.
struct Feeding {
    shared: Arc<Shared>,
    run: u64,
    spool: PathBuf,
    present: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    target: Target,
}

impl Feeding {
    /// The file being sent is complete (`why`: or failed).
    fn end_current(&mut self, why: Option<String>) {
        let target = std::mem::replace(&mut self.target, Target::None);
        let mut s = self.shared.state.lock().unwrap();
        match target {
            Target::Slot(id) => {
                if let Some(st) = s.slots.get_mut(&id) {
                    st.ended = true;
                    st.failed = why;
                }
            }
            Target::Spool(k, f) => {
                drop(f);
                if why.is_some() {
                    if let Some(sp) = s.spooled.remove(&k) {
                        s.spool_bytes -= sp.size;
                        let _ = fs::remove_file(sp.path);
                    }
                } else if let Some(sp) = s.spooled.get_mut(&k) {
                    sp.done = true;
                }
            }
            Target::None | Target::Skip => {}
        }
        self.shared.cv.notify_all();
    }

    /// The run is over (`why`: it failed): readers waiting for a file it
    /// didn't send are told.
    fn finish(&mut self, why: Option<String>) {
        self.end_current(why.clone());
        let mut s = self.shared.state.lock().unwrap();
        s.runs.remove(&self.run);
        let missed: Vec<String> = s.waiting.keys().filter(|k| !s.runs.values().any(|r| r.contains(*k))).cloned().collect();
        for k in missed {
            let id = s.waiting.remove(&k).expect("listed");
            if let Some(st) = s.slots.get_mut(&id) {
                st.missed = true;
                st.failed.clone_from(&why);
            }
        }
        self.shared.cv.notify_all();
    }
}

impl Sink for Feeding {
    fn begin(&mut self, inner: &str, size: u64) -> bool {
        self.end_current(None);
        let k = key(inner);
        let present = (self.present)(inner);
        let mut s = self.shared.state.lock().unwrap();
        if let Some(r) = s.runs.get_mut(&self.run) {
            r.remove(&k);
        }
        loop {
            if s.closed {
                // taken, so the next write says stop
                self.target = Target::Skip;
                return true;
            }
            // someone reads it now: straight to them
            if let Some(id) = s.waiting.remove(&k) {
                if let Some(st) = s.slots.get_mut(&id) {
                    st.begun = true;
                    self.target = Target::Slot(id);
                    self.shared.cv.notify_all();
                    return true;
                }
            }
            if present || s.spooled.contains_key(&k) {
                self.target = Target::Skip;
                return false;
            }
            // room in the spool, or someone waits for a file still to come
            let run = s.runs.get(&self.run);
            let awaited = s.waiting.keys().any(|w| run.is_some_and(|r| r.contains(w)));
            if s.spool_bytes == 0 || s.spool_bytes + size <= SPOOL_LIMIT || awaited {
                break;
            }
            s = self.shared.cv.wait(s).unwrap();
        }
        let path = self.spool.join(format!("{}.part", s.next_spool));
        s.next_spool += 1;
        match fs::File::create(&path) {
            Ok(f) => {
                s.spooled.insert(k.clone(), Spooled { path, size, done: false });
                s.spool_bytes += size;
                self.target = Target::Spool(k, f);
                true
            }
            Err(_) => {
                self.target = Target::Skip;
                false
            }
        }
    }

    fn write(&mut self, data: &[u8]) -> bool {
        match &mut self.target {
            Target::Slot(id) => {
                let id = *id;
                let mut s = self.shared.state.lock().unwrap();
                loop {
                    if s.closed {
                        return false;
                    }
                    // the reader is gone: the rest isn't wanted
                    let Some(st) = s.slots.get_mut(&id) else { return true };
                    if st.chunks.len() < AHEAD {
                        st.chunks.push_back(data.to_vec());
                        self.shared.cv.notify_all();
                        return true;
                    }
                    s = self.shared.cv.wait(s).unwrap();
                }
            }
            Target::Spool(k, f) => {
                if f.write_all(data).is_err() {
                    // no room: read from the plugin when asked for, then
                    let k = k.clone();
                    self.target = Target::Skip;
                    let mut s = self.shared.state.lock().unwrap();
                    if let Some(sp) = s.spooled.remove(&k) {
                        s.spool_bytes -= sp.size;
                        let _ = fs::remove_file(sp.path);
                    }
                    self.shared.cv.notify_all();
                }
                !self.shared.state.lock().unwrap().closed
            }
            Target::None | Target::Skip => !self.shared.state.lock().unwrap().closed,
        }
    }
}

/// For a plugin that can't stream: `items` extracted into the spool folder,
/// then sent from there.
fn from_disk(plugin: &dyn FolderPlugin, file: &Path, items: &[String], spool: &Path, run: u64, sink: &mut Feeding) -> io::Result<()> {
    let dir = spool.join(format!("extract-{run}"));
    fs::create_dir_all(&dir)?;
    let shared = sink.shared.clone();
    let r = (|| {
        plugin.extract(file, items, &dir, &mut |_, _| !shared.state.lock().unwrap().closed)?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut folders = vec![dir.clone()];
        while let Some(f) = folders.pop() {
            for e in fs::read_dir(&f)?.flatten() {
                let p = e.path();
                if e.file_type()?.is_dir() {
                    folders.push(p);
                    continue;
                }
                let inner = p.strip_prefix(&dir).unwrap_or(&p).to_string_lossy().into_owned();
                if !sink.begin(&inner, e.metadata()?.len()) {
                    continue;
                }
                let mut src = fs::File::open(&p)?;
                loop {
                    let n = src.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    if !sink.write(&buf[..n]) {
                        return Err(io::ErrorKind::Interrupted.into());
                    }
                }
            }
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&dir);
    r
}

/// A file opened for reading through a [`Feed`].
pub(crate) enum Source {
    /// Coming straight from the plugin.
    Direct { shared: Arc<Shared>, id: u64 },
    /// Sent before it was read: from the spool (removed when done).
    Spooled { file: fs::File, path: PathBuf },
}

impl Read for Source {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Source::Spooled { file, .. } => file.read(buf),
            Source::Direct { shared, id } => {
                let mut s = shared.state.lock().unwrap();
                loop {
                    // not Interrupted: readers try that again
                    if s.closed {
                        return Err(io::Error::other("the mount was closed"));
                    }
                    let st = s.slots.get_mut(id).ok_or_else(|| io::Error::other("the mount was closed"))?;
                    if let Some(mut chunk) = st.chunks.pop_front() {
                        let n = chunk.len().min(buf.len());
                        buf[..n].copy_from_slice(&chunk[..n]);
                        if n < chunk.len() {
                            chunk.drain(..n);
                            st.chunks.push_front(chunk);
                        }
                        shared.cv.notify_all();
                        return Ok(n);
                    }
                    if st.ended || st.missed {
                        return match &st.failed {
                            Some(why) => Err(io::Error::other(why.clone())),
                            None => Ok(0),
                        };
                    }
                    s = shared.cv.wait(s).unwrap();
                }
            }
        }
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        match self {
            Source::Direct { shared, id } => {
                shared.state.lock().unwrap().slots.remove(id);
                shared.cv.notify_all();
            }
            Source::Spooled { path, .. } => {
                let _ = fs::remove_file(path);
            }
        }
    }
}

/// Why a plugin's items can't be mounted.
#[derive(Debug)]
pub enum MountError {
    /// Windows' Projected File System isn't turned on.
    NotEnabled,
    /// Something else went wrong.
    Failed(String),
}

#[cfg(windows)]
pub use win::{available, mount, unmount_all};

/// Elsewhere there is no Projected File System.
#[cfg(not(windows))]
pub fn available() -> bool {
    false
}

#[cfg(not(windows))]
pub fn unmount_all() {}

#[cfg(not(windows))]
pub struct Mount;

#[cfg(not(windows))]
impl Mount {
    pub fn prepare(&self, _: Vec<String>) {}
    pub fn path_of(&self, _: &str) -> PathBuf {
        PathBuf::new()
    }
}

#[cfg(not(windows))]
pub fn mount(_: Arc<dyn FolderPlugin>, _: &Path) -> Result<Arc<Mount>, MountError> {
    Err(MountError::NotEnabled)
}

/// The Windows side: ProjFS, loaded only when used (`ProjectedFSLib.dll`
/// exists only with the feature turned on), and its callbacks.
#[cfg(windows)]
mod win {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::sync::OnceLock;
    use std::time::SystemTime;

    use windows::core::{s, w, GUID, HRESULT, PCWSTR};
    use windows::Win32::Foundation::{BOOLEAN, HANDLE, E_FAIL, E_INVALIDARG, E_OUTOFMEMORY, S_OK};
    use windows::Win32::Storage::ProjectedFileSystem::{
        PRJ_CALLBACKS, PRJ_CALLBACK_DATA, PRJ_CB_DATA_FLAG_ENUM_RESTART_SCAN, PRJ_CB_DATA_FLAG_ENUM_RETURN_SINGLE_ENTRY, PRJ_DIR_ENTRY_BUFFER_HANDLE, PRJ_FILE_BASIC_INFO, PRJ_FILE_STATE,
        PRJ_FILE_STATE_DIRTY_PLACEHOLDER, PRJ_FILE_STATE_FULL, PRJ_FILE_STATE_HYDRATED_PLACEHOLDER, PRJ_NAMESPACE_VIRTUALIZATION_CONTEXT, PRJ_PLACEHOLDER_INFO, PRJ_PLACEHOLDER_VERSION_INFO,
        PRJ_STARTVIRTUALIZING_OPTIONS, PRJ_VIRTUALIZATION_INSTANCE_INFO,
    };
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

    type Ctx = PRJ_NAMESPACE_VIRTUALIZATION_CONTEXT;

    /// The ProjFS functions used.
    struct Lib {
        start: unsafe extern "system" fn(PCWSTR, *const PRJ_CALLBACKS, *const c_void, *const PRJ_STARTVIRTUALIZING_OPTIONS, *mut Ctx) -> HRESULT,
        stop: unsafe extern "system" fn(Ctx),
        mark: unsafe extern "system" fn(PCWSTR, PCWSTR, *const PRJ_PLACEHOLDER_VERSION_INFO, *const GUID) -> HRESULT,
        placeholder: unsafe extern "system" fn(Ctx, PCWSTR, *const PRJ_PLACEHOLDER_INFO, u32) -> HRESULT,
        fill: unsafe extern "system" fn(PCWSTR, *const PRJ_FILE_BASIC_INFO, PRJ_DIR_ENTRY_BUFFER_HANDLE) -> HRESULT,
        matches: unsafe extern "system" fn(PCWSTR, PCWSTR) -> BOOLEAN,
        compare: unsafe extern "system" fn(PCWSTR, PCWSTR) -> i32,
        write: unsafe extern "system" fn(Ctx, *const GUID, *const c_void, u64, u32) -> HRESULT,
        alloc: unsafe extern "system" fn(Ctx, usize) -> *mut c_void,
        free: unsafe extern "system" fn(*const c_void),
        info: unsafe extern "system" fn(Ctx, *mut PRJ_VIRTUALIZATION_INSTANCE_INFO) -> HRESULT,
        state: unsafe extern "system" fn(PCWSTR, *mut PRJ_FILE_STATE) -> HRESULT,
    }

    /// A function found in a DLL, as the type it has.
    unsafe fn proc_as<T: Copy>(f: unsafe extern "system" fn() -> isize) -> T {
        debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of_val(&f));
        std::mem::transmute_copy(&f)
    }

    /// ProjFS, if Windows has it turned on.
    fn lib() -> Option<&'static Lib> {
        static LIB: OnceLock<Option<Lib>> = OnceLock::new();
        LIB.get_or_init(|| unsafe {
            let h = LoadLibraryExW(w!("ProjectedFSLib.dll"), HANDLE::default(), LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
            macro_rules! f {
                ($name:literal) => {
                    proc_as(GetProcAddress(h, s!($name))?)
                };
            }
            Some(Lib {
                start: f!("PrjStartVirtualizing"),
                stop: f!("PrjStopVirtualizing"),
                mark: f!("PrjMarkDirectoryAsPlaceholder"),
                placeholder: f!("PrjWritePlaceholderInfo"),
                fill: f!("PrjFillDirEntryBuffer"),
                matches: f!("PrjFileNameMatch"),
                compare: f!("PrjFileNameCompare"),
                write: f!("PrjWriteFileData"),
                alloc: f!("PrjAllocateAlignedBuffer"),
                free: f!("PrjFreeAlignedBuffer"),
                info: f!("PrjGetVirtualizationInstanceInfo"),
                state: f!("PrjGetOnDiskFileState"),
            })
        })
        .as_ref()
    }

    /// Whether Windows' Projected File System is turned on.
    pub fn available() -> bool {
        lib().is_some()
    }

    fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
        s.as_ref().encode_wide().chain([0]).collect()
    }

    /// `HRESULT_FROM_WIN32`.
    fn from_win32(code: u32) -> HRESULT {
        HRESULT((code & 0xFFFF | 0x8007_0000) as i32)
    }

    const ERROR_FILE_NOT_FOUND: u32 = 2;
    const ERROR_HANDLE_EOF: u32 = 38;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

    fn hresult_of(e: &io::Error) -> HRESULT {
        match (e.raw_os_error(), e.kind()) {
            (Some(code), _) => from_win32(code as u32),
            (None, io::ErrorKind::NotFound) => from_win32(ERROR_FILE_NOT_FOUND),
            _ => E_FAIL,
        }
    }

    /// A plugin's file or connection, filled in a folder on disk as other
    /// programs look into it.
    pub struct Mount {
        /// The folder.
        pub root: PathBuf,
        file: PathBuf,
        plugin: Arc<dyn FolderPlugin>,
        /// The file's size and date when mounted (an archive): a changed
        /// file gets a new mount.
        stamp: Option<(u64, Option<SystemTime>)>,
        ctx: Ctx,
        provider: Box<Provider>,
    }

    // the context is ProjFS's handle, used from any thread
    unsafe impl Send for Mount {}
    unsafe impl Sync for Mount {}

    impl Mount {
        /// Where the item `inner` is on disk.
        pub fn path_of(&self, inner: &str) -> PathBuf {
            self.root.join(inner)
        }

        /// The items of a drag: listed afresh, and when the first of their
        /// files is read, the plugin sends them all in its own order.
        pub fn prepare(&self, items: Vec<String>) {
            self.provider.tree.forget();
            self.provider.feed.prepare(items);
        }
    }

    impl Drop for Mount {
        fn drop(&mut self) {
            self.provider.feed.close();
            unsafe { (self.provider.lib.stop)(self.ctx) };
        }
    }

    static MOUNTS: Mutex<Vec<Arc<Mount>>> = Mutex::new(Vec::new());

    fn stamp_of(file: &Path) -> Option<(u64, Option<SystemTime>)> {
        fs::metadata(file).ok().map(|m| (m.len(), m.modified().ok()))
    }

    /// The mount of the plugin's `file` (an archive, a connection's root),
    /// made the first time. It stays until [`unmount_all`].
    pub fn mount(plugin: Arc<dyn FolderPlugin>, file: &Path) -> Result<Arc<Mount>, MountError> {
        let lib = lib().ok_or(MountError::NotEnabled)?;
        let stamp = stamp_of(file);
        let mut mounts = MOUNTS.lock().unwrap();
        if let Some(m) = mounts.iter().find(|m| m.file == file && m.stamp == stamp && m.plugin.id() == plugin.id()) {
            return Ok(m.clone());
        }
        if mounts.is_empty() {
            remove_stale();
        }
        let failed = |e: io::Error| MountError::Failed(format!("{}: {e}", sal_core::encoding::display_path(file)));
        let base = super::super::temp_dir("mount").map_err(failed)?;
        // named after the archive or server, as copy dialogs show it
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| valid_name(n)).unwrap_or_else(|| "mount".into());
        let (root, spool) = (base.join(&name), base.join("spool"));
        fs::create_dir_all(&root).map_err(failed)?;
        fs::create_dir_all(&spool).map_err(failed)?;
        let tree = Arc::new(Tree::new(plugin.clone(), file.to_path_buf()));
        let present_root = root.clone();
        let present: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(move |inner: &str| {
            let mut st = PRJ_FILE_STATE::default();
            let p = wide(present_root.join(inner));
            unsafe { (lib.state)(PCWSTR(p.as_ptr()), &mut st) }.is_ok() && (st.0 & (PRJ_FILE_STATE_HYDRATED_PLACEHOLDER.0 | PRJ_FILE_STATE_DIRTY_PLACEHOLDER.0 | PRJ_FILE_STATE_FULL.0)) != 0
        });
        let feed = Feed::new(plugin.clone(), file.to_path_buf(), tree.clone(), spool, present);
        let provider = Box::new(Provider { tree, feed, enums: Mutex::new(HashMap::new()), lib, block: 0.into() });
        let root_w = wide(&root);
        let callbacks = PRJ_CALLBACKS {
            StartDirectoryEnumerationCallback: Some(start_enum),
            EndDirectoryEnumerationCallback: Some(end_enum),
            GetDirectoryEnumerationCallback: Some(get_enum),
            GetPlaceholderInfoCallback: Some(placeholder_info),
            GetFileDataCallback: Some(file_data),
            ..Default::default()
        };
        let ctx = unsafe {
            let id = GUID::new().map_err(|e| MountError::Failed(e.message()))?;
            // the root itself: no target
            let hr = (lib.mark)(PCWSTR(root_w.as_ptr()), PCWSTR::null(), std::ptr::null(), &id);
            if hr.is_err() {
                let _ = fs::remove_dir_all(&base);
                return Err(start_error(hr));
            }
            let mut ctx = Ctx::default();
            let hr = (lib.start)(PCWSTR(root_w.as_ptr()), &callbacks, &*provider as *const Provider as *const c_void, std::ptr::null(), &mut ctx);
            if hr.is_err() {
                let _ = fs::remove_dir_all(&base);
                return Err(start_error(hr));
            }
            ctx
        };
        let mut info = PRJ_VIRTUALIZATION_INSTANCE_INFO::default();
        let align = if unsafe { (lib.info)(ctx, &mut info) }.is_ok() { info.WriteAlignment.max(1) as usize } else { 4096 };
        // whole multiples of the alignment, about a megabyte
        provider.block.store((1 << 20) / align * align, std::sync::atomic::Ordering::Relaxed);
        let m = Arc::new(Mount { root, file: file.to_path_buf(), plugin, stamp, ctx, provider });
        mounts.push(m.clone());
        Ok(m)
    }

    /// The ProjFS driver not running is the feature turned off too.
    fn start_error(hr: HRESULT) -> MountError {
        // ERROR_FILE_SYSTEM_VIRTUALIZATION_UNAVAILABLE, ERROR_NOT_SUPPORTED,
        // ERROR_SERVICE_DOES_NOT_EXIST, ERROR_SERVICE_DISABLED
        if [369u32, 50, 1060, 1058].iter().any(|&c| from_win32(c) == hr) {
            MountError::NotEnabled
        } else {
            MountError::Failed(hr.message())
        }
    }

    /// Stop every mount (the program ends); their folders go with the
    /// session's temporary files.
    pub fn unmount_all() {
        let all = std::mem::take(&mut *MOUNTS.lock().unwrap());
        drop(all);
    }

    /// Mount folders of sessions that ended without removing them.
    fn remove_stale() {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        let base = std::env::temp_dir().join("RustySalamander");
        let Ok(rd) = fs::read_dir(&base) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(pid) = name.strip_suffix("-mount").and_then(|n| n.split('-').next()).and_then(|p| p.parse::<u32>().ok()) else { continue };
            if pid == std::process::id() {
                continue;
            }
            let alive = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.map(|h| unsafe { CloseHandle(h) }).is_ok();
            if !alive {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }

    /// What the callbacks work with.
    struct Provider {
        tree: Arc<Tree>,
        feed: Feed,
        /// Directory listings in progress, by ProjFS's id.
        enums: Mutex<HashMap<u128, Listing>>,
        lib: &'static Lib,
        /// How much is written at a time.
        block: std::sync::atomic::AtomicUsize,
    }

    /// A folder listing in progress.
    struct Listing {
        /// Names (with the closing 0) and items, in ProjFS's order.
        entries: Vec<(Vec<u16>, Item)>,
        next: usize,
        filter: Option<Vec<u16>>,
    }

    unsafe fn provider<'a>(d: *const PRJ_CALLBACK_DATA) -> &'a Provider {
        &*((*d).InstanceContext as *const Provider)
    }

    /// The path the callback is about, below the mount's folder.
    unsafe fn rel_path(d: *const PRJ_CALLBACK_DATA) -> String {
        let p = (*d).FilePathName;
        if p.is_null() {
            String::new()
        } else {
            String::from_utf16_lossy(p.as_wide())
        }
    }

    fn basic(it: &Item) -> PRJ_FILE_BASIC_INFO {
        let t = match sal_plugin_api::abi::to_filetime(it.modified) {
            0 => sal_plugin_api::abi::to_filetime(Some(SystemTime::now())),
            t => t,
        } as i64;
        // read-only, hidden, system and archive are kept
        let attrs = if it.dir { 0x10 } else { (it.attrs & 0x27).max(0x80) };
        PRJ_FILE_BASIC_INFO {
            IsDirectory: BOOLEAN(it.dir as u8),
            FileSize: if it.dir { 0 } else { it.size as i64 },
            CreationTime: t,
            LastAccessTime: t,
            LastWriteTime: t,
            ChangeTime: t,
            FileAttributes: attrs,
        }
    }

    fn guid_key(id: *const GUID) -> u128 {
        unsafe { (*id).to_u128() }
    }

    unsafe extern "system" fn start_enum(d: *const PRJ_CALLBACK_DATA, id: *const GUID) -> HRESULT {
        let p = provider(d);
        let rel = rel_path(d);
        let r = (|| {
            let dir = match p.tree.find(&rel)? {
                Some(it) if it.dir => it.path,
                _ => return Err(io::Error::from(io::ErrorKind::NotFound)),
            };
            let mut entries: Vec<(Vec<u16>, Item)> = p.tree.children(&dir)?.iter().map(|it| (wide(leaf_of(&it.path)), it.clone())).collect();
            entries.sort_by(|a, b| (p.lib.compare)(PCWSTR(a.0.as_ptr()), PCWSTR(b.0.as_ptr())).cmp(&0));
            Ok(entries)
        })();
        match r {
            Ok(entries) => {
                p.enums.lock().unwrap().insert(guid_key(id), Listing { entries, next: 0, filter: None });
                S_OK
            }
            Err(e) => hresult_of(&e),
        }
    }

    unsafe extern "system" fn end_enum(d: *const PRJ_CALLBACK_DATA, id: *const GUID) -> HRESULT {
        provider(d).enums.lock().unwrap().remove(&guid_key(id));
        S_OK
    }

    unsafe extern "system" fn get_enum(d: *const PRJ_CALLBACK_DATA, id: *const GUID, search: PCWSTR, handle: PRJ_DIR_ENTRY_BUFFER_HANDLE) -> HRESULT {
        let p = provider(d);
        let mut enums = p.enums.lock().unwrap();
        let Some(l) = enums.get_mut(&guid_key(id)) else { return E_INVALIDARG };
        let flags = (*d).Flags.0;
        let restart = flags & PRJ_CB_DATA_FLAG_ENUM_RESTART_SCAN.0 != 0;
        if restart || l.filter.is_none() {
            l.filter = Some(if search.is_null() { wide("*") } else { search.as_wide().iter().copied().chain([0]).collect() });
            if restart {
                l.next = 0;
            }
        }
        let filter = l.filter.as_ref().expect("just set");
        let mut added = 0;
        while l.next < l.entries.len() {
            let (name, it) = &l.entries[l.next];
            if (p.lib.matches)(PCWSTR(name.as_ptr()), PCWSTR(filter.as_ptr())).as_bool() {
                let info = basic(it);
                let hr = (p.lib.fill)(PCWSTR(name.as_ptr()), &info, handle);
                if hr == from_win32(ERROR_INSUFFICIENT_BUFFER) {
                    // full: the rest next time
                    return if added == 0 { hr } else { S_OK };
                }
                if hr.is_err() {
                    return hr;
                }
                added += 1;
            }
            l.next += 1;
            if added > 0 && flags & PRJ_CB_DATA_FLAG_ENUM_RETURN_SINGLE_ENTRY.0 != 0 {
                break;
            }
        }
        S_OK
    }

    unsafe extern "system" fn placeholder_info(d: *const PRJ_CALLBACK_DATA) -> HRESULT {
        let p = provider(d);
        match p.tree.find(&rel_path(d)) {
            Ok(Some(it)) => {
                let info = PRJ_PLACEHOLDER_INFO { FileBasicInfo: basic(&it), ..Default::default() };
                // the name as the plugin has it (it may differ in case)
                let name = wide(&it.path);
                (p.lib.placeholder)((*d).NamespaceVirtualizationContext, PCWSTR(name.as_ptr()), &info, std::mem::size_of::<PRJ_PLACEHOLDER_INFO>() as u32)
            }
            Ok(None) => from_win32(ERROR_FILE_NOT_FOUND),
            Err(e) => hresult_of(&e),
        }
    }

    unsafe extern "system" fn file_data(d: *const PRJ_CALLBACK_DATA, offset: u64, length: u32) -> HRESULT {
        let p = provider(d);
        let it = match p.tree.find(&rel_path(d)) {
            Ok(Some(it)) if !it.dir => it,
            Ok(_) => return from_win32(ERROR_FILE_NOT_FOUND),
            Err(e) => return hresult_of(&e),
        };
        let mut src = match p.feed.open(&it.path) {
            Ok(s) => s,
            Err(e) => return hresult_of(&e),
        };
        let ctx = (*d).NamespaceVirtualizationContext;
        let block = p.block.load(std::sync::atomic::Ordering::Relaxed).max(4096);
        let buf = (p.lib.alloc)(ctx, block);
        if buf.is_null() {
            return E_OUTOFMEMORY;
        }
        let out = std::slice::from_raw_parts_mut(buf as *mut u8, block);
        // the whole file, a block at a time from the start: only the last
        // write may be shorter than a block
        let mut pos: u64 = 0;
        let mut hr = S_OK;
        'file: loop {
            let mut filled = 0;
            while filled < block {
                match src.read(&mut out[filled..]) {
                    Ok(0) => break,
                    Ok(n) => filled += n,
                    Err(e) => {
                        hr = hresult_of(&e);
                        break 'file;
                    }
                }
            }
            if filled == 0 {
                break;
            }
            hr = (p.lib.write)(ctx, &(*d).DataStreamId, buf, pos, filled as u32);
            if hr.is_err() {
                break;
            }
            pos += filled as u64;
            if filled < block {
                break;
            }
        }
        (p.lib.free)(buf);
        if hr.is_ok() && pos < offset + length as u64 {
            hr = from_win32(ERROR_HANDLE_EOF);
        }
        hr
    }

    #[cfg(test)]
    mod tests {
        use super::super::tests::{expected, fake};
        use super::*;

        /// A real mount, if ProjFS is on: the items are files on disk, read
        /// from the plugin as they are read.
        #[test]
        fn mounted_items_are_files() {
            if !available() {
                assert!(matches!(mount(fake(true, true), Path::new("x.fake")), Err(MountError::NotEnabled)));
                eprintln!("ProjFS isn't turned on: skipped");
                return;
            }
            for (streams, complete) in [(true, true), (false, false)] {
                let p = fake(streams, complete);
                let m = mount(p.clone(), Path::new(&format!("x-{streams}-{complete}.fake"))).unwrap();
                m.prepare(vec!["data".into(), "other.txt".into()]);
                let mut names: Vec<String> = fs::read_dir(m.path_of("data")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
                names.sort();
                assert_eq!(names, ["a.txt", "b.txt", "sub"]);
                for inner in ["data\\b.txt", "data\\sub\\c 🦎.txt", "data\\a.txt", "other.txt"] {
                    assert_eq!(fs::read(m.path_of(inner)).unwrap(), expected(&p, inner), "{inner}");
                }
                // in any case
                assert_eq!(fs::read(m.path_of("DATA\\B.TXT")).unwrap(), expected(&p, "data\\b.txt"));
                assert!(!m.path_of("nothing.txt").exists());
            }
            unmount_all();
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use sal_plugin_api::{Index, Progress};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A plugin whose files hold their own name repeated `size` times;
    /// `streams` false: it can only extract. It sends in its own order.
    pub struct Fake {
        items: Vec<Item>,
        streams: bool,
        complete: bool,
        pub runs: AtomicUsize,
    }

    fn item(path: &str, dir: bool, size: u64) -> Item {
        Item { path: path.into(), dir, size, modified: None, attrs: 0, index: None }
    }

    fn content(f: &Item) -> Vec<u8> {
        f.path.bytes().cycle().take(f.size as usize).collect()
    }

    impl FolderPlugin for Fake {
        fn id(&self) -> &str {
            "fake"
        }
        fn handles(&self, _: &str) -> bool {
            false
        }
        fn can_write(&self, _: &Path) -> bool {
            false
        }
        fn status(&self) -> String {
            String::new()
        }
        fn list(&self, _: &Path, folder: &str) -> io::Result<Index> {
            if self.complete {
                return Ok(Index { items: self.items.clone(), complete: true });
            }
            Ok(Index { items: self.items.iter().filter(|i| parent_of(&i.path) == folder).cloned().collect(), complete: false })
        }
        fn extract(&self, _: &Path, items: &[String], out: &Path, _: &mut Progress) -> io::Result<()> {
            for f in self.items.iter().filter(|f| !f.dir && items.iter().any(|w| under(&f.path, w))) {
                let p = out.join(&f.path);
                fs::create_dir_all(p.parent().unwrap())?;
                fs::write(p, content(f))?;
            }
            Ok(())
        }
        fn stream(&self, _: &Path, items: &[String], sink: &mut dyn Sink) -> io::Result<()> {
            if !self.streams {
                return Err(io::ErrorKind::Unsupported.into());
            }
            self.runs.fetch_add(1, Ordering::SeqCst);
            for f in self.items.iter().filter(|f| !f.dir && items.iter().any(|w| under(&f.path, w))) {
                if sink.begin(&f.path, f.size) {
                    for c in content(f).chunks(3) {
                        if !sink.write(c) {
                            return Err(io::ErrorKind::Interrupted.into());
                        }
                    }
                }
            }
            Ok(())
        }
        fn add(&self, _: &Path, _: &[(PathBuf, String)], _: &mut Progress) -> io::Result<()> {
            unimplemented!()
        }
        fn delete(&self, _: &Path, _: &[String], _: &mut Progress) -> io::Result<()> {
            unimplemented!()
        }
        fn rename(&self, _: &Path, _: &str, _: &str, _: &mut Progress) -> io::Result<()> {
            unimplemented!()
        }
        fn mkdir(&self, _: &Path, _: &str, _: &mut Progress) -> io::Result<()> {
            unimplemented!()
        }
    }

    pub fn fake(streams: bool, complete: bool) -> Arc<Fake> {
        Arc::new(Fake {
            items: vec![
                item("data", true, 0),
                item("data\\b.txt", false, 50),
                item("data\\sub\\c 🦎.txt", false, 7),
                item("data\\a.txt", false, 0),
                item("data\\sub", true, 0),
                item("other.txt", false, 5),
                item("bad:name.txt", false, 1),
            ],
            streams,
            complete,
            runs: AtomicUsize::new(0),
        })
    }

    pub fn expected(p: &Fake, inner: &str) -> Vec<u8> {
        content(p.items.iter().find(|i| i.path == inner).unwrap())
    }

    fn read_all(mut s: Source) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        s.read_to_end(&mut out)?;
        Ok(out)
    }

    fn feed(p: &Arc<Fake>, spool: &Path, present: &'static [&'static str]) -> Feed {
        let tree = Arc::new(Tree::new(p.clone(), "x".into()));
        Feed::new(p.clone(), "x".into(), tree, spool.to_path_buf(), Arc::new(move |inner: &str| present.contains(&inner)))
    }

    /// Folders as they are looked into, in any case; folders known only
    /// from the paths inside them; names Windows can't have left out.
    #[test]
    fn folders_listed() {
        for complete in [true, false] {
            let p = fake(true, complete);
            let t = Tree::new(p.clone(), "x".into());
            let names = |dir: &str| {
                let mut n: Vec<String> = t.children(dir).unwrap().iter().map(|i| i.path.clone()).collect();
                n.sort();
                n
            };
            assert_eq!(names(""), ["data", "other.txt"], "complete: {complete}");
            assert_eq!(names("data"), ["data\\a.txt", "data\\b.txt", "data\\sub"]);
            assert_eq!(t.find("DATA\\Sub\\C 🦎.TXT").unwrap().map(|i| i.path), Some("data\\sub\\c 🦎.txt".into()));
            assert_eq!(t.find("data\\nic").unwrap(), None);
            assert_eq!(t.find("other.txt\\x").unwrap(), None, "a file has nothing inside");
            assert!(t.find("").unwrap().unwrap().dir, "the root");
            let mut files: Vec<String> = t.files_under(&["data".into()]).unwrap().into_iter().map(|i| i.path).collect();
            files.sort();
            assert_eq!(files, ["data\\a.txt", "data\\b.txt", "data\\sub\\c 🦎.txt"]);
        }
        // a folder only in the paths
        let p = Arc::new(Fake { items: vec![item("a\\b\\c.txt", false, 1)], streams: true, complete: true, runs: AtomicUsize::new(0) });
        let t = Tree::new(p, "x".into());
        assert_eq!(t.find("a\\b").unwrap().map(|i| (i.path, i.dir)), Some(("a\\b".into(), true)));
    }

    /// A drag's files read in any order: one run of the plugin; what it
    /// sent before it was read waits in the spool, and the spool is empty
    /// at the end.
    #[test]
    fn drag_read_in_any_order() {
        for streams in [true, false] {
            let t = tempfile::tempdir().unwrap();
            let p = fake(streams, true);
            let f = feed(&p, t.path(), &[]);
            f.prepare(vec!["data".into(), "other.txt".into()]);
            // the plugin's order is b, c, a, other: read backwards
            for inner in ["other.txt", "data\\a.txt", "data\\sub\\c 🦎.txt", "data\\b.txt"] {
                assert_eq!(read_all(f.open(inner).unwrap()).unwrap(), expected(&p, inner), "{inner} (streams: {streams})");
            }
            assert_eq!(p.runs.load(Ordering::SeqCst), usize::from(streams), "one run");
            // the run's end may still be cleaning up
            let empty = || fs::read_dir(t.path()).unwrap().next().is_none();
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !empty() && std::time::Instant::now() < until {
                std::thread::yield_now();
            }
            assert!(empty(), "spool empty");
        }
    }

    /// A file outside the drag is sent on its own; one already on disk
    /// isn't sent again; a file the plugin doesn't have is an error.
    #[test]
    fn outside_the_drag() {
        let t = tempfile::tempdir().unwrap();
        let p = fake(true, true);
        let f = feed(&p, t.path(), &["data\\b.txt"]);
        assert_eq!(read_all(f.open("other.txt").unwrap()).unwrap(), expected(&p, "other.txt"));
        assert_eq!(p.runs.load(Ordering::SeqCst), 1);
        // the drag: b is already on disk, so it isn't sent
        f.prepare(vec!["data".into()]);
        assert_eq!(read_all(f.open("data\\a.txt").unwrap()).unwrap(), b"");
        assert_eq!(read_all(f.open("data\\sub\\c 🦎.txt").unwrap()).unwrap(), expected(&p, "data\\sub\\c 🦎.txt"));
        assert_eq!(p.runs.load(Ordering::SeqCst), 2);
        assert!(fs::read_dir(t.path()).unwrap().next().is_none(), "nothing spooled");
        assert!(f.open("nic.txt").is_err());
    }

    /// Closing stops a run waiting for a reader, and readers.
    #[test]
    fn closing_stops() {
        let t = tempfile::tempdir().unwrap();
        let p = fake(true, true);
        let f = feed(&p, t.path(), &[]);
        f.prepare(vec!["data".into()]);
        let mut s = f.open("data\\b.txt").unwrap();
        let mut buf = [0u8; 3];
        assert_eq!(s.read(&mut buf).unwrap(), 3);
        f.close();
        assert!(s.read_to_end(&mut Vec::new()).is_err());
        assert!(f.open("data\\a.txt").is_err());
    }
}
