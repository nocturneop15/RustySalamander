//! One file panel: listing, cursor, selection, quick search, history
//! (replaces `CFilesWindow`, `fileswn*.cpp`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use sal_core::encoding::display_path;
use sal_core::listing::{self, Listing, Visibility};
use sal_core::{drives, location};
use sal_core::{quicksearch, EntryKind, FileEntry, MaskGroup, SortSpec};

use crate::plugins::{self, FolderPlugin, Index};

/// What a plugin shows in the panel: the plugin, the archive file or the
/// connection's root, the folder inside, and the index. An archive's index
/// is complete (read once, again only when the file changes); a
/// connection's is the listing of the folder shown.
#[derive(Clone)]
pub struct ArchiveView {
    pub plugin: Arc<dyn FolderPlugin>,
    pub file: PathBuf,
    pub inner: String,
    pub index: Arc<Index>,
    stamp: (Option<SystemTime>, u64),
}

/// A file's modification time and size, to notice when it changed.
fn stamp_of(file: &Path) -> (Option<SystemTime>, u64) {
    std::fs::metadata(file).map(|m| (m.modified().ok(), m.len())).unwrap_or((None, 0))
}

pub struct Panel {
    pub listing: Listing,
    pub cursor: usize,
    pub sort: SortSpec,
    /// Text of the editable path bar.
    pub path_edit: String,
    /// Quick-search text typed so far (empty = inactive).
    pub qs: String,
    pub scroll_to_cursor: bool,
    pub visible_rows: usize,
    history: Vec<PathBuf>,
    history_pos: usize,
    dir_mtime: Option<SystemTime>,
    /// The folder's date is still to be read (for a network folder, by the
    /// first background check) before changes can be told.
    baseline_pending: bool,
    last_check: Instant,
    /// A change check running in the background: the path checked, its
    /// date, and whether it still exists.
    check: Option<Receiver<(PathBuf, Option<SystemTime>, bool)>>,
    /// A network folder being read in the background at start-up: its
    /// listing and the item to put the cursor on, or `None` if nothing
    /// could be read.
    loading: Option<Receiver<Loaded>>,
    /// Folder plugins that are on: files they handle open as folders.
    pub plugins: Vec<Arc<dyn FolderPlugin>>,
    /// Set while the panel shows a folder inside an archive.
    pub archive: Option<ArchiveView>,
}

/// The listing of a folder inside an archive. Paths are virtual: the archive
/// file's path continued by the path inside it.
fn archive_listing(view: &ArchiveView, vis: Visibility, sort: SortSpec) -> Listing {
    let base = if view.inner.is_empty() { view.file.clone() } else { view.file.join(&view.inner) };
    let mut entries = Vec::new();
    if let Some((parent, _)) = location::parent_of(&base) {
        entries.push(FileEntry::up_dir(parent));
    }
    for it in view.index.children(&view.inner) {
        let name = it.path.rsplit('\\').next().unwrap_or(&it.path).to_owned();
        let kind = if it.dir { EntryKind::Dir } else { EntryKind::File };
        let e = FileEntry::new(base.join(&name), kind, it.size, it.modified, it.attrs);
        if vis.shows(&e) {
            entries.push(e);
        }
    }
    sal_core::sort::sort_entries(&mut entries, sort);
    Listing { path: base, entries, unreadable: 0 }
}

/// What a background read sends: the listing and the item to put the
/// cursor on, or `None` when nothing could be read.
type Loaded = Option<(Listing, Option<String>)>;

/// The folder a panel shows when its own can't be read.
fn fallback_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" }))
}

impl Panel {
    /// A panel on `path` (or the nearest folder that can be read).
    pub fn new(path: &str, sort: SortSpec, vis: Visibility, plugins: Vec<Arc<dyn FolderPlugin>>) -> Self {
        let mut p = Panel {
            listing: Listing::default(),
            cursor: 0,
            sort,
            path_edit: String::new(),
            qs: String::new(),
            scroll_to_cursor: true,
            visible_rows: 20,
            history: Vec::new(),
            history_pos: 0,
            dir_mtime: None,
            baseline_pending: false,
            last_check: Instant::now(),
            check: None,
            loading: None,
            plugins,
            archive: None,
        };
        let start = if path.is_empty() { fallback_dir() } else { PathBuf::from(path) };
        // a network folder can take seconds to read (or to time out): read it
        // in the background, so the window is usable at once
        if drives::is_remote(&start) && plugins::split(&p.plugins, &start).is_none() {
            p.listing = Listing { path: start.clone(), entries: Vec::new(), unreadable: 0 };
            p.path_edit = display_path(&start);
            let sort = p.sort;
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let t = Instant::now();
                let found = listing::read_dir(&start, vis, sort).ok().map(|l| (l, None)).or_else(|| {
                    // the nearest folder above it that still opens
                    start.ancestors().skip(1).zip(start.ancestors()).find_map(|(dir, child)| {
                        let name = child.file_name().map(|n| n.to_string_lossy().into_owned());
                        listing::read_dir(dir, vis, sort).ok().map(|l| (l, name))
                    })
                });
                crate::debuglog::write(format!(
                    "background read of {}: {} ({})",
                    start.display(),
                    if found.is_some() { "done" } else { "failed" },
                    crate::debuglog::ms(t.elapsed())
                ));
                let _ = tx.send(found);
            });
            p.loading = Some(rx);
            return p;
        }
        if p.load(&start, vis, None).is_err() {
            // the nearest folder that still opens (e.g. the folder of an
            // archive whose plugin is gone), with the cursor on what led on
            let opened = start.ancestors().skip(1).zip(start.ancestors()).any(|(dir, child)| {
                let name = child.file_name().map(|n| n.to_string_lossy().into_owned());
                dir.is_dir() && p.load(dir, vis, name.as_deref()).is_ok()
            });
            if !opened {
                let _ = p.load(&fallback_dir(), vis, None);
            }
        }
        p
    }

    /// The folder shown.
    pub fn path(&self) -> &Path {
        &self.listing.path
    }

    /// The items shown, sorted.
    pub fn entries(&self) -> &[FileEntry] {
        &self.listing.entries
    }

    /// The item under the cursor.
    pub fn current(&self) -> Option<&FileEntry> {
        self.listing.entries.get(self.cursor)
    }

    /// Read a folder: a directory, or a folder inside an archive when the
    /// path goes through a file a folder plugin handles.
    fn read(&mut self, path: &Path, vis: Visibility) -> Result<(Listing, Option<ArchiveView>), String> {
        if let Some((plugin, file, inner)) = plugins::split(&self.plugins, path) {
            {
                let view = self.archive_view(&plugin, &file, inner)?;
                if view.index.complete && !view.index.is_dir(&view.inner) {
                    return Err(format!("{}: {}", display_path(path), crate::i18n::tr("not found in the archive")));
                }
                return Ok((archive_listing(&view, vis, self.sort), Some(view)));
            }
        }
        let listing = listing::read_dir(path, vis, self.sort).map_err(|e| format!("{}: {e}", display_path(path)))?;
        Ok((listing, None))
    }

    /// The plugin's index for `inner`: an archive's is read again only when
    /// the file has changed, a connection's folder every time.
    fn archive_view(&mut self, plugin: &Arc<dyn FolderPlugin>, file: &Path, inner: String) -> Result<ArchiveView, String> {
        let stamp = stamp_of(file);
        if let Some(v) = self.archive.as_ref().filter(|v| v.index.complete && v.file == file && v.stamp == stamp && v.plugin.id() == plugin.id()) {
            return Ok(ArchiveView { inner, ..v.clone() });
        }
        let index = plugin.list(file, &inner).map_err(|e| format!("{}: {e}", display_path(&file.join(&inner))))?;
        Ok(ArchiveView { plugin: plugin.clone(), file: file.to_path_buf(), inner, index: Arc::new(index), stamp })
    }

    /// What tells a change of the shown folder: the directory itself, or the archive file.
    fn stamp_path(&self) -> &Path {
        self.archive.as_ref().map_or(self.listing.path.as_path(), |a| a.file.as_path())
    }

    /// Whether a folder is still being read in the background.
    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    /// Take the background read's result when it has arrived; true when the
    /// panel changed.
    pub fn poll_loading(&mut self, vis: Visibility) -> bool {
        let Some(rx) = &self.loading else { return false };
        match rx.try_recv() {
            Err(TryRecvError::Empty) => false,
            Ok(Some((listing, focus))) => {
                self.loading = None;
                self.install(listing, None, focus.as_deref());
                true
            }
            Ok(None) | Err(TryRecvError::Disconnected) => {
                self.loading = None;
                let _ = self.load(&fallback_dir(), vis, None);
                true
            }
        }
    }

    /// Load a directory; on error the old listing is kept.
    pub fn load(&mut self, path: &Path, vis: Visibility, focus: Option<&str>) -> Result<(), String> {
        let (listing, view) = self.read(path, vis)?;
        // a background read still running is no longer wanted
        self.loading = None;
        self.install(listing, view, focus);
        Ok(())
    }

    /// Show a listing that was read: history, change detection, cursor.
    fn install(&mut self, listing: Listing, view: Option<ArchiveView>, focus: Option<&str>) {
        let same_dir = listing.path == self.listing.path && !self.history.is_empty();
        if !same_dir {
            self.history.truncate(self.history_pos + usize::from(!self.history.is_empty()));
            self.history.push(listing.path.clone());
            self.history_pos = self.history.len() - 1;
        }
        self.archive = view;
        self.listing = listing;
        self.set_baseline();
        self.path_edit = display_path(&self.listing.path);
        self.qs.clear();
        self.cursor = focus.and_then(|n| self.listing.entries.iter().position(|e| e.name == n)).unwrap_or(0);
        self.scroll_to_cursor = true;
    }

    /// Remember the shown folder's date, to tell when it changes. Reading it
    /// over the network can take a while, so for a network folder the first
    /// background check does it.
    fn set_baseline(&mut self) {
        self.check = None;
        if drives::is_remote(self.stamp_path()) {
            self.dir_mtime = None;
            self.baseline_pending = true;
        } else {
            self.dir_mtime = std::fs::metadata(self.stamp_path()).and_then(|m| m.modified()).ok();
            self.baseline_pending = false;
        }
        self.last_check = Instant::now();
    }

    /// Re-read the directory keeping cursor and selection.
    pub fn reload(&mut self, vis: Visibility) {
        let focus = self.current().map(|e| e.name.clone());
        let old_cursor = self.cursor;
        let selected: HashSet<String> = self.listing.entries.iter().filter(|e| e.selected).map(|e| e.name.clone()).collect();
        let path = self.listing.path.clone();
        let mut target = path.clone();
        // directory vanished: climb to the nearest existing ancestor (inside
        // an archive: the nearest folder the archive still has)
        let exists = |p: &Path, me: &mut Self| match plugins::split(&me.plugins, p) {
            Some((plugin, file, inner)) => me.archive_view(&plugin, &file, inner.clone()).is_ok_and(|v| !v.index.complete || v.index.is_dir(&inner)),
            None => p.is_dir(),
        };
        while !exists(&target, self) {
            match target.parent() {
                Some(p) => target = p.to_path_buf(),
                None => break,
            }
        }
        if self.load(&target, vis, focus.as_deref()).is_ok() {
            for e in &mut self.listing.entries {
                e.selected = selected.contains(&e.name);
            }
            if target == path && focus.as_deref() != self.current().map(|e| e.name.as_str()) {
                self.cursor = old_cursor.min(self.listing.entries.len().saturating_sub(1));
            }
        }
    }

    /// Cheap change detection: directory mtime changes when entries are
    /// added, removed or renamed (NTFS, ext4, …). The date is read on a
    /// background thread, so a slow network folder never holds up the
    /// window; its answer is looked at in a later frame.
    pub fn needs_refresh(&mut self) -> bool {
        if self.loading.is_some() {
            return false;
        }
        if let Some(rx) = &self.check {
            match rx.try_recv() {
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => self.check = None,
                Ok((path, mtime, exists)) => {
                    self.check = None;
                    if path != self.stamp_path() {
                        return false; // the panel has moved on since
                    }
                    if std::mem::take(&mut self.baseline_pending) {
                        self.dir_mtime = mtime;
                        return !exists;
                    }
                    let changed = mtime != self.dir_mtime || !exists;
                    if changed {
                        crate::debuglog::write(format!("refresh {}: date {:?} -> {mtime:?}, exists {exists}", path.display(), self.dir_mtime));
                    }
                    return changed;
                }
            }
        }
        // network folders less often: each check is a round trip to the server
        let every = if drives::is_remote(self.stamp_path()) { 5000 } else { 1500 };
        if !self.baseline_pending && self.last_check.elapsed().as_millis() < every {
            return false;
        }
        self.last_check = Instant::now();
        // a server's share list has no mtime; re-enumerating it would hit the
        // network, and so would a plugin's connection (refreshed with Ctrl+R)
        if location::unc_server(&self.listing.path).is_some() || self.archive.as_ref().is_some_and(|a| !a.index.complete) {
            self.baseline_pending = false;
            return false;
        }
        let (path, file) = (self.stamp_path().to_path_buf(), self.archive.is_some());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let md = std::fs::metadata(&path);
            let mtime = md.as_ref().ok().and_then(|m| m.modified().ok());
            let exists = md.is_ok_and(|m| if file { m.is_file() } else { m.is_dir() });
            let _ = tx.send((path, mtime, exists));
        });
        self.check = Some(rx);
        false
    }

    /// Sort by `sort`, keeping the cursor on its item.
    pub fn resort(&mut self, sort: SortSpec) {
        self.sort = sort;
        let focus = self.current().map(|e| e.name.clone());
        sal_core::sort::sort_entries(&mut self.listing.entries, sort);
        if let Some(n) = focus {
            self.cursor = self.listing.entries.iter().position(|e| e.name == n).unwrap_or(0);
        }
        self.scroll_to_cursor = true;
    }

    /// Leave a plugin's connection (closed, or its plugin turned off): back
    /// to the last folder in the history that is outside it.
    pub fn leave_connection(&mut self, vis: Visibility) {
        let back: Vec<PathBuf> = self.history[..self.history_pos.min(self.history.len())].iter().rev().filter(|p| location::virtual_parts(p).is_none()).cloned().collect();
        for p in back {
            if self.load(&p, vis, None).is_ok() {
                return;
            }
        }
        let _ = self.load(&fallback_dir(), vis, None);
    }

    /// Backspace: the parent folder, with the cursor on the folder left.
    pub fn go_parent(&mut self, vis: Visibility) -> Result<(), String> {
        // a UNC share root goes up to its server's share list
        match location::parent_of(&self.listing.path) {
            Some((parent, name)) => self.load(&parent, vis, Some(&name)),
            None => Ok(()),
        }
    }

    /// The root of the drive or share.
    pub fn go_root(&mut self, vis: Visibility) -> Result<(), String> {
        let root = sal_core::drives::root_of(&self.listing.path);
        self.load(&root, vis, None)
    }

    /// Back or forward in the panel's history.
    pub fn history_step(&mut self, back: bool, vis: Visibility) -> Result<(), String> {
        let pos = if back { self.history_pos.checked_sub(1) } else { Some(self.history_pos + 1).filter(|&p| p < self.history.len()) };
        let Some(pos) = pos else { return Ok(()) };
        let target = self.history[pos].clone();
        let (listing, view) = self.read(&target, vis)?;
        // navigate without touching the history list
        let focus = if back { self.listing.path.file_name().map(|n| n.to_string_lossy().into_owned()) } else { None };
        self.history_pos = pos;
        self.loading = None;
        self.archive = view;
        self.listing = listing;
        self.set_baseline();
        self.path_edit = display_path(&self.listing.path);
        self.qs.clear();
        self.cursor = focus.and_then(|n| self.listing.entries.iter().position(|e| e.name == n)).unwrap_or(0);
        self.scroll_to_cursor = true;
        Ok(())
    }

    /// Enter on the cursor item: navigates into directories, returns the
    /// path of a file that should be opened.
    pub fn enter(&mut self, vis: Visibility) -> Result<Option<PathBuf>, String> {
        let Some(e) = self.current().cloned() else { return Ok(None) };
        match e.kind {
            EntryKind::UpDir => self.go_parent(vis).map(|_| None),
            EntryKind::Dir => self.load(&e.path, vis, None).map(|_| None),
            EntryKind::File => Ok(Some(e.path)),
        }
    }

    /// Keyboard cursor movement; ends quick search like in Salamander.
    pub fn move_cursor(&mut self, delta: isize) {
        self.qs.clear();
        let n = self.listing.entries.len();
        if n == 0 {
            return;
        }
        self.cursor = (self.cursor as isize + delta).clamp(0, n as isize - 1) as usize;
        self.scroll_to_cursor = true;
    }

    /// Put the cursor on item `i` and scroll to it.
    pub fn set_cursor(&mut self, i: usize) {
        if i < self.listing.entries.len() {
            self.cursor = i;
            self.scroll_to_cursor = true;
        }
    }

    /// Insert / Space: select or unselect the item under the cursor;
    /// `advance` moves the cursor down.
    pub fn toggle_select(&mut self, advance: bool) {
        self.qs.clear();
        if let Some(e) = self.listing.entries.get_mut(self.cursor) {
            if e.kind != EntryKind::UpDir {
                e.selected = !e.selected;
            }
        }
        if advance {
            self.move_cursor(1);
        }
    }

    /// Select (or unselect) the items matching `mask`.
    pub fn select_by_mask(&mut self, mask: &MaskGroup, select: bool) {
        for e in &mut self.listing.entries {
            if e.kind != EntryKind::UpDir && mask.matches(&e.name) {
                e.selected = select;
            }
        }
    }

    /// Select or unselect everything (never `..`).
    pub fn select_all(&mut self, select: bool) {
        for e in &mut self.listing.entries {
            e.selected = select && e.kind != EntryKind::UpDir;
        }
    }

    /// Unselect these items (after an operation on them).
    pub fn unselect(&mut self, paths: &[PathBuf]) {
        for e in &mut self.listing.entries {
            if paths.contains(&e.path) {
                e.selected = false;
            }
        }
    }

    /// Invert the selection of the files.
    pub fn invert_selection(&mut self) {
        // like Salamander's Num*, inverts files only
        for e in &mut self.listing.entries {
            if e.kind == EntryKind::File {
                e.selected = !e.selected;
            }
        }
    }

    /// Selected items, or the cursor item when nothing is selected.
    pub fn targets(&self) -> Vec<PathBuf> {
        let sel: Vec<PathBuf> = self.listing.entries.iter().filter(|e| e.selected).map(|e| e.path.clone()).collect();
        if !sel.is_empty() {
            return sel;
        }
        match self.current() {
            Some(e) if e.kind != EntryKind::UpDir => vec![e.path.clone()],
            _ => Vec::new(),
        }
    }

    /// Add a character to quick search; returns false if nothing matches
    /// (the character is then rejected, as in Salamander).
    pub fn quick_search_push(&mut self, c: char, anywhere: bool) -> bool {
        let mut candidate = self.qs.clone();
        candidate.push(c);
        let mask = quicksearch::prepare(&candidate);
        let entries = &self.listing.entries;
        match quicksearch::find(|i| entries[i].name.as_str(), entries.len(), self.cursor, &mask, true, anywhere) {
            Some(i) => {
                self.qs = candidate;
                self.set_cursor(i);
                true
            }
            None => false,
        }
    }

    /// Backspace in a quick search: drop its last character.
    pub fn quick_search_pop(&mut self) {
        self.qs.pop();
    }

    /// Jump to the next/previous quick-search match.
    pub fn quick_search_next(&mut self, forward: bool, anywhere: bool) {
        if self.qs.is_empty() {
            return;
        }
        let mask = quicksearch::prepare(&self.qs);
        let entries = &self.listing.entries;
        let n = entries.len();
        if n == 0 {
            return;
        }
        let start = if forward { (self.cursor + 1) % n } else { (self.cursor + n - 1) % n };
        if let Some(i) = quicksearch::find(|i| entries[i].name.as_str(), n, start, &mask, forward, anywhere) {
            self.set_cursor(i);
        }
    }
}
