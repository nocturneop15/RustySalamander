//! One file panel: listing, cursor, selection, quick search, history
//! (replaces `CFilesWindow`, `fileswn*.cpp`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use sal_core::encoding::display_path;
use sal_core::listing::{self, Listing, Visibility};
use sal_core::location;
use sal_core::{quicksearch, EntryKind, FileEntry, MaskGroup, SortSpec};

use crate::plugins::{self, FolderPlugin, Index};

/// The archive being browsed: the plugin showing it, its file, the folder
/// inside it, and its index (read once, again only when the file changes).
#[derive(Clone)]
pub struct ArchiveView {
    pub plugin: Arc<dyn FolderPlugin>,
    pub file: PathBuf,
    pub inner: String,
    pub index: Arc<Index>,
    stamp: (Option<SystemTime>, u64),
}

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
    last_check: Instant,
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

fn fallback_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" }))
}

impl Panel {
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
            last_check: Instant::now(),
            plugins,
            archive: None,
        };
        let start = if path.is_empty() { fallback_dir() } else { PathBuf::from(path) };
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

    pub fn path(&self) -> &Path {
        &self.listing.path
    }

    pub fn entries(&self) -> &[FileEntry] {
        &self.listing.entries
    }

    pub fn current(&self) -> Option<&FileEntry> {
        self.listing.entries.get(self.cursor)
    }

    /// Read a folder: a directory, or a folder inside an archive when the
    /// path goes through a file a folder plugin handles.
    fn read(&mut self, path: &Path, vis: Visibility) -> Result<(Listing, Option<ArchiveView>), String> {
        if let Some((plugin, file, inner)) = plugins::split(&self.plugins, path) {
            {
                let view = self.archive_view(&plugin, &file, inner)?;
                if !view.index.is_dir(&view.inner) {
                    return Err(format!("{}: {}", display_path(path), crate::i18n::tr("not found in the archive")));
                }
                return Ok((archive_listing(&view, vis, self.sort), Some(view)));
            }
        }
        let listing = listing::read_dir(path, vis, self.sort).map_err(|e| format!("{}: {e}", display_path(path)))?;
        Ok((listing, None))
    }

    /// The archive's index, read again only when the file has changed.
    fn archive_view(&mut self, plugin: &Arc<dyn FolderPlugin>, file: &Path, inner: String) -> Result<ArchiveView, String> {
        let stamp = stamp_of(file);
        if let Some(v) = self.archive.as_ref().filter(|v| v.file == file && v.stamp == stamp && v.plugin.id() == plugin.id()) {
            return Ok(ArchiveView { inner, ..v.clone() });
        }
        let index = plugin.list(file).map_err(|e| format!("{}: {e}", display_path(file)))?;
        Ok(ArchiveView { plugin: plugin.clone(), file: file.to_path_buf(), inner, index: Arc::new(index), stamp })
    }

    /// What tells a change of the shown folder: the directory itself, or the archive file.
    fn stamp_path(&self) -> &Path {
        self.archive.as_ref().map_or(self.listing.path.as_path(), |a| a.file.as_path())
    }

    /// Load a directory; on error the old listing is kept.
    pub fn load(&mut self, path: &Path, vis: Visibility, focus: Option<&str>) -> Result<(), String> {
        let (listing, view) = self.read(path, vis)?;
        let same_dir = listing.path == self.listing.path;
        if !same_dir {
            self.history.truncate(self.history_pos + usize::from(!self.history.is_empty()));
            self.history.push(listing.path.clone());
            self.history_pos = self.history.len() - 1;
        }
        self.archive = view;
        self.dir_mtime = std::fs::metadata(self.stamp_path()).and_then(|m| m.modified()).ok();
        self.listing = listing;
        self.path_edit = display_path(&self.listing.path);
        self.qs.clear();
        self.cursor = focus.and_then(|n| self.listing.entries.iter().position(|e| e.name == n)).unwrap_or(0);
        self.scroll_to_cursor = true;
        Ok(())
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
            Some((plugin, file, inner)) => me.archive_view(&plugin, &file, inner.clone()).is_ok_and(|v| v.index.is_dir(&inner)),
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
    /// added, removed or renamed (NTFS, ext4, …).
    pub fn needs_refresh(&mut self) -> bool {
        if self.last_check.elapsed().as_millis() < 1500 {
            return false;
        }
        self.last_check = Instant::now();
        // a server's share list has no mtime; re-enumerating it would hit the network
        if location::unc_server(&self.listing.path).is_some() {
            return false;
        }
        let now = std::fs::metadata(self.stamp_path()).and_then(|m| m.modified()).ok();
        let gone = match &self.archive {
            Some(a) => !a.file.is_file(),
            None => !self.listing.path.is_dir(),
        };
        now != self.dir_mtime || gone
    }

    pub fn resort(&mut self, sort: SortSpec) {
        self.sort = sort;
        let focus = self.current().map(|e| e.name.clone());
        sal_core::sort::sort_entries(&mut self.listing.entries, sort);
        if let Some(n) = focus {
            self.cursor = self.listing.entries.iter().position(|e| e.name == n).unwrap_or(0);
        }
        self.scroll_to_cursor = true;
    }

    pub fn go_parent(&mut self, vis: Visibility) -> Result<(), String> {
        // a UNC share root goes up to its server's share list
        match location::parent_of(&self.listing.path) {
            Some((parent, name)) => self.load(&parent, vis, Some(&name)),
            None => Ok(()),
        }
    }

    pub fn go_root(&mut self, vis: Visibility) -> Result<(), String> {
        let root = sal_core::drives::root_of(&self.listing.path);
        self.load(&root, vis, None)
    }

    pub fn history_step(&mut self, back: bool, vis: Visibility) -> Result<(), String> {
        let pos = if back { self.history_pos.checked_sub(1) } else { Some(self.history_pos + 1).filter(|&p| p < self.history.len()) };
        let Some(pos) = pos else { return Ok(()) };
        let target = self.history[pos].clone();
        let (listing, view) = self.read(&target, vis)?;
        // navigate without touching the history list
        let focus = if back { self.listing.path.file_name().map(|n| n.to_string_lossy().into_owned()) } else { None };
        self.history_pos = pos;
        self.archive = view;
        self.dir_mtime = std::fs::metadata(self.stamp_path()).and_then(|m| m.modified()).ok();
        self.listing = listing;
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

    pub fn set_cursor(&mut self, i: usize) {
        if i < self.listing.entries.len() {
            self.cursor = i;
            self.scroll_to_cursor = true;
        }
    }

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

    pub fn select_by_mask(&mut self, mask: &MaskGroup, select: bool) {
        for e in &mut self.listing.entries {
            if e.kind != EntryKind::UpDir && mask.matches(&e.name) {
                e.selected = select;
            }
        }
    }

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
