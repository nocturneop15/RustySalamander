//! The search plugin: finds files and folders by name (wildcard masks or a
//! regular expression) and, optionally, by text inside the files.
//!
//! **Plugins → Find files…** shows a form; OK starts the search in the
//! background and the program opens a results window that fills while it
//! runs ([`Reply::Results`]). The results are two connections a panel can
//! browse like an archive:
//!
//! - `search://<n>`: the searched folder as a filter, with its folders but
//!   only what was found in them;
//! - `search://<n>-list`: everything found, files and folders, in one list
//!   under its own name (numbered when names repeat: `a (2).txt`); a found
//!   folder opens to its real contents.
//!
//! The search ends when both are closed.
//!
//! Every result is a real file, so the program opens, views and edits the
//! file itself ([`FolderPlugin::real_path`]). In the virtual folder:
//!
//! - **copy out** (F5) copies the found files (a found folder with all it
//!   contains), keeping the folders between;
//! - **delete** (F8) moves the found files and folders to the Recycle Bin;
//!   on a folder that only leads to results, the results inside it;
//! - **rename** (F2) renames a found file or folder on disk;
//! - files can't be added and folders can't be made.

mod find;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sal_plugin_api::{
    export_plugin, join_inner, mount_of, n_, tr, trf, under, Activity, Command, Field, FieldKind, FolderPlugin, Form, Index, Item, Mount, Plugin, Progress, Reply, Results, Values,
};

use find::{attributes, search, Found, Names, Query, State, Text};

/// The scheme of the results' connections: `search://1`.
const SCHEME: &str = "search";
/// The flat list of search `<n>` is the connection `<n>-list`.
const LIST: &str = "-list";

/// One search: what was asked, and its progress and results.
struct Job {
    /// The folder searched, and what for (to search again on Refresh).
    dir: PathBuf,
    query: Query,
    state: State,
    started: Instant,
    /// How long it took, once it is over.
    took: Mutex<Option<Duration>>,
    /// Its connections not closed yet (the folders and the list).
    views: AtomicU32,
}

pub struct Search {
    next: AtomicU32,
    /// Searches by their connection's authority ("1", "2", …).
    jobs: Mutex<HashMap<String, Arc<Job>>>,
    /// The values of the last search, offered again.
    last: Mutex<Values>,
}

export_plugin!(Search);

impl Plugin for Search {
    const ID: &'static str = "search";
    const NAME: &'static str = n_("Search");
    const DESCRIPTION: &'static str = n_("Finds files and folders by name (wildcards or regular expressions) and by the text inside files. Start with Plugins → Find files…");

    fn start(_dir: &Path) -> Result<Self, String> {
        Ok(Search { next: AtomicU32::new(1), jobs: Mutex::new(HashMap::new()), last: Mutex::new(Values::new()) })
    }
}

fn unsupported(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, tr(msg))
}

fn on(values: &Values, id: &str) -> bool {
    values.get(id).is_some_and(|v| !v.is_empty())
}

/// The search form, filled with `values`; `error` on top when the last
/// attempt was wrong.
fn form(values: &Values, error: Option<String>) -> Form {
    let v = |id: &str, default: &str| values.get(id).cloned().unwrap_or_else(|| default.to_owned());
    let mut fields = Vec::new();
    if let Some(e) = error {
        fields.push(Field { id: "error".into(), label: format!("⚠ {e}"), kind: FieldKind::Note, ..Default::default() });
    }
    let check = |id: &str, label: &str, default: &str, enabled_if: Option<(String, String)>| Field {
        id: id.into(),
        label: label.into(),
        kind: FieldKind::Check,
        value: v(id, default),
        enabled_if,
    };
    fields.extend([
        Field { id: "names".into(), label: tr("Names:"), value: v("names", "*.*"), ..Default::default() },
        Field {
            id: "syntax".into(),
            label: tr("Names are:"),
            kind: FieldKind::Choice(vec![("mask".into(), tr("Wildcards")), ("regex".into(), tr("A regular expression"))]),
            value: v("syntax", "mask"),
            enabled_if: None,
        },
        Field {
            id: "help".into(),
            label: tr("Wildcards: * is any text, ? one character; separate masks with ; and put the ones to leave out after | (*.txt;*.md|old*)."),
            kind: FieldKind::Note,
            ..Default::default()
        },
        Field { id: "dir".into(), label: tr("Look in:"), kind: FieldKind::Folder, value: v("dir", ""), ..Default::default() },
        check("subdirs", &tr("Include subfolders"), "1", None),
        check("folders", &tr("Find folders too"), "", None),
        check("hidden", &tr("Include hidden and system items"), "1", None),
        Field { id: "text".into(), label: tr("Containing text:"), value: v("text", ""), ..Default::default() },
        check("text_regex", &tr("The text is a regular expression"), "", None),
        check("case", &tr("Match case"), "", None),
        Field {
            id: "help2".into(),
            label: tr("Leave the text empty to find by name only. Plain text is found in UTF-8, ANSI and UTF-16 files; Match case also applies to a regular expression for names."),
            kind: FieldKind::Note,
            ..Default::default()
        },
    ]);
    Form { title: tr("Find files"), fields, ok: tr("Find"), command: "search".into() }
}

/// The query the form's values describe, and the folder to search.
fn query(values: &Values) -> Result<(PathBuf, Query), String> {
    let dir = values.get("dir").map(|d| d.trim().trim_matches('"')).unwrap_or_default();
    if dir.is_empty() {
        return Err(tr("Choose the folder to look in."));
    }
    let dir = PathBuf::from(dir);
    if !dir.is_dir() {
        return Err(trf("{dir} is not a folder.", &[("dir", &dir.display())]));
    }
    let case = on(values, "case");
    let names_text = values.get("names").map(String::as_str).unwrap_or_default();
    let names = if values.get("syntax").map(String::as_str) == Some("regex") {
        Names::regex(names_text, case).map_err(|e| trf("The regular expression for names is wrong: {error}", &[("error", &e)]))?
    } else {
        Names::masks(names_text)
    };
    let text = match values.get("text").filter(|t| !t.is_empty()) {
        None => None,
        Some(t) if on(values, "text_regex") => Some(Text::regex(t, case).map_err(|e| trf("The regular expression for the text is wrong: {error}", &[("error", &e)]))?),
        Some(t) => Some(Text::literal(t, case).map_err(|e| e.to_string())?),
    };
    Ok((dir, Query { names, text, subfolders: on(values, "subdirs"), folders: on(values, "folders"), hidden: on(values, "hidden") }))
}

/// The window title for a search.
fn title(values: &Values, dir: &Path) -> String {
    let names = values.get("names").map(|n| n.trim()).filter(|n| !n.is_empty()).unwrap_or("*.*");
    match values.get("text").filter(|t| !t.is_empty()) {
        Some(t) => trf("Find \"{text}\" in {names} in {dir}", &[("text", t), ("names", &names), ("dir", &dir.display())]),
        None => trf("Find {names} in {dir}", &[("names", &names), ("dir", &dir.display())]),
    }
}

/// A short name for a search, for the drive selector: what was looked for
/// and the name of the folder (`*.txt "hello" · Documents`).
fn short_name(values: &Values, dir: &Path) -> String {
    let names = values.get("names").map(|n| n.trim()).filter(|n| !n.is_empty()).unwrap_or("*.*");
    let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.display().to_string());
    match values.get("text").filter(|t| !t.is_empty()) {
        Some(t) => format!("{names} \"{t}\" · {folder}"),
        None => format!("{names} · {folder}"),
    }
}

impl Search {
    /// The search a connection shows, and whether it is its flat list.
    fn job(&self, root: &Path) -> io::Result<(Arc<Job>, bool)> {
        let found = mount_of(root).filter(|(scheme, _, _)| scheme == SCHEME).and_then(|(_, authority, _)| {
            let (id, flat) = authority.strip_suffix(LIST).map_or((authority.as_str(), false), |id| (id, true));
            Some((self.jobs.lock().unwrap().get(id).cloned()?, flat))
        });
        found.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, tr("These search results are closed.")))
    }

    /// Start a search in the background; its connection's authority.
    fn start_search(&self, dir: PathBuf, q: Query) -> String {
        let id = self.next.fetch_add(1, Ordering::Relaxed).to_string();
        self.run_job(&id, dir, q, 2);
        id
    }

    /// Run a search as the job `id` (a new one, or again in place of the
    /// one there: its connections stay, `views` of them open).
    fn run_job(&self, id: &str, dir: PathBuf, query: Query, views: u32) {
        let job = Arc::new(Job { dir, query, state: State::default(), started: Instant::now(), took: Mutex::new(None), views: AtomicU32::new(views) });
        if let Some(old) = self.jobs.lock().unwrap().insert(id.to_owned(), job.clone()) {
            old.state.stop.store(true, Ordering::Relaxed);
        }
        std::thread::Builder::new()
            .name("search".into())
            .spawn(move || {
                search(&job.dir, &job.query, &job.state);
                *job.took.lock().unwrap() = Some(job.started.elapsed());
            })
            .expect("start the search thread");
    }

    /// Search again, as the first time, into the same connections.
    fn refresh(&self, results: &Path) -> io::Result<()> {
        let (job, _) = self.job(results)?;
        let (_, authority, _) = mount_of(results).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        let id = authority.strip_suffix(LIST).unwrap_or(&authority).to_owned();
        self.run_job(&id, job.dir.clone(), job.query.clone(), job.views.load(Ordering::Relaxed));
        Ok(())
    }

    /// The results as one of the search's connections shows them, by their
    /// path there: below the folders leading to them, or (the flat list)
    /// each under its own name, numbered when names repeat.
    /// Results whose files are gone are dropped (deleted or moved meanwhile).
    fn view(&self, job: &Job, flat: bool) -> Vec<(String, Found)> {
        let mut found = job.state.found.lock().unwrap();
        if job.took.lock().unwrap().is_some() {
            found.retain(|f| f.real.symlink_metadata().is_ok());
        }
        if !flat {
            return found.iter().map(|f| (f.inner.clone(), f.clone())).collect();
        }
        let mut taken = HashSet::new();
        found
            .iter()
            .map(|f| {
                let name = f.real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let (stem, ext) = match name.rsplit_once('.') {
                    Some((stem, ext)) if !stem.is_empty() => (stem.to_owned(), format!(".{ext}")),
                    _ => (name.clone(), String::new()),
                };
                let (mut shown, mut n) = (name.clone(), 2);
                while !taken.insert(shown.to_lowercase()) {
                    shown = format!("{stem} ({n}){ext}");
                    n += 1;
                }
                (shown, f.clone())
            })
            .collect()
    }

    /// What `inner` of a connection is on disk: a result, or in the flat list
    /// also something inside a found folder (opened there, a found folder
    /// shows what it really contains).
    fn resolve(&self, job: &Job, flat: bool, inner: &str) -> Option<Found> {
        let view = self.view(job, flat);
        if let Some((_, f)) = view.iter().find(|(p, _)| p == inner) {
            return Some(f.clone());
        }
        if !flat {
            return None;
        }
        let (p, f) = view.iter().find(|(p, f)| f.dir && inner.len() > p.len() && under(inner, p))?;
        let real = inner[p.len() + 1..].split('\\').fold(f.real.clone(), |r, part| r.join(part));
        let md = real.symlink_metadata().ok()?;
        let attrs = attributes(&md);
        let dir = attrs & sal_plugin_api::ATTR_DIRECTORY != 0;
        Some(Found { inner: tree_inner(&job.dir, &real), real, dir, size: if dir { 0 } else { md.len() }, modified: md.modified().ok(), attrs })
    }

    /// What the chosen items of a connection stand for: the results at or
    /// below them (in the flat list, what they are on disk); a found folder
    /// covers what is inside it.
    fn chosen(&self, job: &Job, flat: bool, items: &[String]) -> Vec<(String, Found)> {
        let mut chosen: Vec<(String, Found)> = if flat {
            items.iter().filter_map(|i| self.resolve(job, true, i).map(|f| (i.clone(), f))).collect()
        } else {
            self.view(job, flat).into_iter().filter(|(p, _)| items.iter().any(|i| under(p, i))).collect()
        };
        let dirs: Vec<String> = chosen.iter().filter(|(_, f)| f.dir).map(|(p, _)| p.clone()).collect();
        chosen.retain(|(p, _)| !dirs.iter().any(|d| d != p && under(p, d)));
        chosen
    }
}

/// The path of `real` below the searched folder `dir`, as the folders view
/// shows it.
fn tree_inner(dir: &Path, real: &Path) -> String {
    let rel = real.strip_prefix(dir).unwrap_or(real);
    rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("\\")
}

fn copy_dir(from: &Path, to: &Path, progress: &mut dyn FnMut(u64) -> bool) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if e.file_type()?.is_dir() {
            copy_dir(&src, &dst, progress)?;
        } else {
            let n = fs::copy(&src, &dst)?;
            if !progress(n) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, tr("Cancelled.")));
            }
        }
    }
    Ok(())
}

/// Move to the Recycle Bin.
#[cfg(windows)]
fn recycle(paths: &[PathBuf]) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW};
    if paths.is_empty() {
        return Ok(());
    }
    // a list of NUL-terminated paths, ending with another NUL
    let mut from: Vec<u16> = Vec::new();
    for p in paths {
        from.extend(p.as_os_str().encode_wide());
        from.push(0);
    }
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT).0 as u16,
        ..Default::default()
    };
    match unsafe { SHFileOperationW(&mut op) } {
        0 if !op.fAnyOperationsAborted.as_bool() => Ok(()),
        0 => Err(io::Error::new(io::ErrorKind::Interrupted, tr("Cancelled."))),
        code => Err(io::Error::other(trf("Windows could not move the items to the Recycle Bin (error {code}).", &[("code", &code)]))),
    }
}

#[cfg(not(windows))]
fn recycle(paths: &[PathBuf]) -> io::Result<()> {
    for p in paths {
        if p.is_dir() {
            fs::remove_dir_all(p)?;
        } else {
            fs::remove_file(p)?;
        }
    }
    Ok(())
}

impl FolderPlugin for Search {
    fn id(&self) -> &str {
        Self::ID
    }

    fn handles(&self, _name: &str) -> bool {
        false
    }

    fn can_write(&self, _file: &Path) -> bool {
        true
    }

    fn status(&self) -> String {
        String::new()
    }

    fn list(&self, file: &Path, folder: &str) -> io::Result<Index> {
        let (job, flat) = self.job(file)?;
        if flat && !folder.is_empty() {
            // a found folder opened in the flat list: what it really contains
            let f = self.resolve(&job, true, folder).filter(|f| f.dir).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            let mut items = Vec::new();
            for e in fs::read_dir(&f.real)? {
                let e = e?;
                let md = e.metadata()?;
                let attrs = attributes(&md);
                let dir = attrs & sal_plugin_api::ATTR_DIRECTORY != 0;
                let path = join_inner(folder, &e.file_name().to_string_lossy());
                items.push(Item { path, dir, size: if dir { 0 } else { md.len() }, modified: md.modified().ok(), attrs, index: None });
            }
            return Ok(Index { items, complete: false });
        }
        let items = self
            .view(&job, flat)
            .into_iter()
            .enumerate()
            .map(|(i, (path, f))| Item { path, dir: f.dir, size: f.size, modified: f.modified, attrs: f.attrs, index: Some(i as u32) })
            .collect();
        // the folders view is everything: the program browses its folders
        // itself; the flat list's found folders are asked for one by one
        Ok(Index { items, complete: !flat })
    }

    fn extract(&self, file: &Path, items: &[String], out: &Path, progress: &mut Progress) -> io::Result<()> {
        let (job, flat) = self.job(file)?;
        // a found folder is copied whole: results inside it come with it
        let chosen = self.chosen(&job, flat, items);
        let total: u64 = chosen.iter().map(|(_, f)| f.size).sum();
        let mut done = 0;
        let cancelled = || io::Error::new(io::ErrorKind::Interrupted, tr("Cancelled."));
        for (path, f) in chosen {
            if !progress(done, total) {
                return Err(cancelled());
            }
            let to = out.join(&path);
            if f.dir {
                copy_dir(&f.real, &to, &mut |n| {
                    done += n;
                    progress(done, total.max(done))
                })?;
            } else {
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent)?;
                }
                done += fs::copy(&f.real, &to)?;
            }
        }
        progress(done, total.max(done));
        Ok(())
    }

    fn add(&self, _file: &Path, _items: &[(PathBuf, String)], _progress: &mut Progress) -> io::Result<()> {
        Err(unsupported(n_("Files can't be added to search results.")))
    }

    fn delete(&self, file: &Path, items: &[String], progress: &mut Progress) -> io::Result<()> {
        let (job, flat) = self.job(file)?;
        let chosen = self.chosen(&job, flat, items);
        if !progress(0, chosen.len() as u64) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, tr("Cancelled.")));
        }
        let paths: Vec<PathBuf> = chosen.iter().map(|(_, f)| f.real.clone()).collect();
        let r = recycle(&paths);
        // whatever is gone now leaves the results, even after an error
        job.state.found.lock().unwrap().retain(|f| f.real.symlink_metadata().is_ok());
        progress(chosen.len() as u64, chosen.len() as u64);
        r
    }

    fn rename(&self, file: &Path, old: &str, new: &str, _progress: &mut Progress) -> io::Result<()> {
        let (job, flat) = self.job(file)?;
        let (old_parent, new_parent) = (old.rsplit_once('\\').map_or("", |(p, _)| p), new.rsplit_once('\\').map_or("", |(p, _)| p));
        if old_parent != new_parent {
            return Err(unsupported(n_("Search results can only be renamed in their own folder.")));
        }
        let new_name = new.rsplit_once('\\').map_or(new, |(_, n)| n);
        let Some(target) = self.resolve(&job, flat, old) else {
            return Err(unsupported(n_("Only what was found can be renamed, not the folders leading to it.")));
        };
        let to = target.real.with_file_name(new_name);
        if to.symlink_metadata().is_ok() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, trf("{name} already exists.", &[("name", &to.display())])));
        }
        fs::rename(&target.real, &to)?;
        // the result, and results inside a renamed folder, move with it
        for g in job.state.found.lock().unwrap().iter_mut() {
            if let Ok(rest) = g.real.strip_prefix(&target.real) {
                g.real = if rest.as_os_str().is_empty() { to.clone() } else { to.join(rest) };
                g.inner = tree_inner(&job.dir, &g.real);
            }
        }
        Ok(())
    }

    fn mkdir(&self, _file: &Path, _inner: &str, _progress: &mut Progress) -> io::Result<()> {
        Err(unsupported(n_("Folders can't be made in search results.")))
    }

    fn commands(&self) -> Vec<Command> {
        vec![Command { id: "find".into(), label: tr("Find files…") }]
    }

    fn run(&self, command: &str, input: &Values) -> io::Result<Reply> {
        match command {
            "find" => {
                let mut values = self.last.lock().unwrap().clone();
                // the active panel's folder, unless it shows a plugin's place
                if let Some(panel) = input.get("panel").map(PathBuf::from).filter(|p| p.is_dir()) {
                    values.insert("dir".into(), panel.to_string_lossy().into_owned());
                }
                Ok(Reply::Form(form(&values, None)))
            }
            "search" => {
                let values: Values = input.iter().filter(|(k, _)| !matches!(k.as_str(), "panel" | "mount" | "error")).map(|(k, v)| (k.clone(), v.clone())).collect();
                match query(&values) {
                    Err(e) => Ok(Reply::Form(form(&values, Some(e)))),
                    Ok((dir, q)) => {
                        *self.last.lock().unwrap() = values.clone();
                        let (title, name) = (title(&values, &dir), short_name(&values, &dir));
                        let authority = self.start_search(dir, q);
                        let flat = format!("{authority}{LIST}");
                        Ok(Reply::Results(Results {
                            title,
                            name,
                            mount: Mount { scheme: SCHEME.into(), authority, folder: String::new() },
                            flat,
                            again: "find".into(),
                            refresh: "refresh".into(),
                        }))
                    }
                }
            }
            "refresh" => {
                let results = input.get("results").map(PathBuf::from).ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
                self.refresh(&results)?;
                Ok(Reply::Done)
            }
            _ => Err(io::Error::new(io::ErrorKind::Unsupported, format!("unknown command \"{command}\""))),
        }
    }

    fn close(&self, root: &Path) {
        let Some((_, authority, _)) = mount_of(root) else { return };
        let id = authority.strip_suffix(LIST).unwrap_or(&authority);
        let mut jobs = self.jobs.lock().unwrap();
        // the search ends with its last connection (the folders or the list)
        if jobs.get(id).is_some_and(|job| job.views.fetch_sub(1, Ordering::Relaxed) <= 1) {
            if let Some(job) = jobs.remove(id) {
                job.state.stop.store(true, Ordering::Relaxed);
            }
        }
    }

    fn activity(&self, root: &Path) -> Activity {
        let Ok((job, _)) = self.job(root) else { return Activity::default() };
        let st = &job.state;
        let found = st.found.lock().unwrap().len() as u64;
        let (files, errors) = (st.files.load(Ordering::Relaxed), st.errors.load(Ordering::Relaxed));
        let took = *job.took.lock().unwrap();
        // counts as "label: n", which reads right in every language
        let mut text = match took {
            None => trf("Searching {folder} · found: {found} · files checked: {files}", &[("folder", &st.folder.lock().unwrap().display()), ("found", &found), ("files", &files)]),
            Some(took) => {
                let what = if st.stop.load(Ordering::Relaxed) { tr("Stopped") } else { tr("Done") };
                trf(
                    "{what} · found: {found} · files checked: {files} · folders: {folders} · {seconds} s",
                    &[("what", &what), ("found", &found), ("files", &files), ("folders", &st.folders.load(Ordering::Relaxed)), ("seconds", &format!("{:.1}", took.as_secs_f64()))],
                )
            }
        };
        if errors > 0 {
            text.push_str(&trf(" · could not be read: {n}", &[("n", &errors)]));
        }
        Activity { running: took.is_none(), text }
    }

    fn stop(&self, root: &Path) {
        if let Ok((job, _)) = self.job(root) {
            job.state.stop.store(true, Ordering::Relaxed);
        }
    }

    fn real_path(&self, file: &Path, inner: &str) -> Option<PathBuf> {
        let (job, flat) = self.job(file).ok()?;
        self.resolve(&job, flat, inner).map(|f| f.real)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(p: &Search, root: &Path) -> Activity {
        let t = Instant::now();
        loop {
            let a = p.activity(root);
            if !a.running {
                return a;
            }
            assert!(t.elapsed() < Duration::from_secs(20), "the search doesn't end");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn start(p: &Search, values: &[(&str, &str)]) -> io::Result<Reply> {
        let v: Values = values.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        p.run("search", &v)
    }

    #[test]
    fn search_through_the_plugin() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("data");
        fs::create_dir_all(root.join("docs").join("old")).unwrap();
        fs::write(root.join("a.txt"), "hello world").unwrap();
        fs::write(root.join("docs").join("b.txt"), "nothing here").unwrap();
        fs::write(root.join("docs").join("old").join("c.txt"), "HELLO again").unwrap();
        fs::write(root.join("docs").join("d.md"), "hello").unwrap();
        let p = Search::start(Path::new(".")).unwrap();
        assert_eq!(p.commands()[0].id, "find");

        // the form starts in the active panel's folder
        let Reply::Form(f) = p.run("find", &[("panel".to_owned(), root.display().to_string())].into()).unwrap() else { panic!("no form") };
        assert_eq!(f.fields.iter().find(|f| f.id == "dir").unwrap().value, root.display().to_string());
        assert_eq!(f.command, "search");

        // mistakes come back as the form with the reason on top
        let dir = root.display().to_string();
        let Reply::Form(f) = start(&p, &[("dir", "Z:\\nowhere\\at all"), ("names", "*.*")]).unwrap() else { panic!("no form") };
        assert!(f.fields[0].label.contains("is not a folder"), "{:?}", f.fields[0]);
        let Reply::Form(f) = start(&p, &[("dir", &dir), ("names", "(x"), ("syntax", "regex")]).unwrap() else { panic!("no form") };
        assert!(f.fields[0].label.contains("regular expression"));
        assert_eq!(f.fields.iter().find(|f| f.id == "names").unwrap().value, "(x", "what was typed stays");

        let Reply::Results(r) = start(&p, &[("dir", &dir), ("names", "*.txt"), ("subdirs", "1"), ("text", "hello"), ("hidden", "1")]).unwrap() else { panic!("no results") };
        assert_eq!((r.mount.scheme.as_str(), r.again.as_str()), ("search", "find"));
        assert!(r.title.contains("hello") && r.title.contains("*.txt"), "{}", r.title);
        assert_eq!(r.name, "*.txt \"hello\" · data");
        let mount = r.mount.root();
        let a = wait(&p, &mount);
        assert!(a.text.starts_with("Done · found: 2 · files checked: 4 · folders: 3"), "{}", a.text);
        let ix = p.list(&mount, "").unwrap();
        assert!(ix.complete);
        let paths: Vec<&str> = ix.items.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, ["a.txt", "docs\\old\\c.txt"]);
        assert_eq!(ix.children("docs").iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["docs\\old"], "folders lead to the results");
        assert_eq!(p.real_path(&mount, "docs\\old\\c.txt"), Some(root.join("docs").join("old").join("c.txt")));
        assert_eq!(p.real_path(&mount, "docs"), None, "a folder only leading to results is no file");

        // Refresh: the same search again, into the same connections
        fs::write(root.join("docs").join("new.txt"), "hello, new").unwrap();
        assert_eq!(p.run("refresh", &[("results".to_owned(), mount.display().to_string())].into()).unwrap(), Reply::Done);
        wait(&p, &mount);
        assert_eq!(p.list(&mount, "").unwrap().items.len(), 3);
        fs::remove_file(root.join("docs").join("new.txt")).unwrap();
        p.run("refresh", &[("results".to_owned(), mount.display().to_string())].into()).unwrap();
        wait(&p, &mount);

        // the last values are offered again
        let Reply::Form(f) = p.run("find", &Values::new()).unwrap() else { panic!("no form") };
        assert_eq!(f.fields.iter().find(|f| f.id == "text").unwrap().value, "hello");

        // copy out keeps the folders between
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        p.extract(&mount, &["docs".into()], &out, &mut |_, _| true).unwrap();
        assert_eq!(fs::read_to_string(out.join("docs").join("old").join("c.txt")).unwrap(), "HELLO again");
        assert!(!out.join("docs").join("b.txt").exists(), "only what was found");

        // rename on disk, in its own folder only
        p.rename(&mount, "a.txt", "a2.txt", &mut |_, _| true).unwrap();
        assert!(root.join("a2.txt").is_file() && !root.join("a.txt").exists());
        assert_eq!(p.real_path(&mount, "a2.txt"), Some(root.join("a2.txt")));
        assert!(p.rename(&mount, "a2.txt", "docs\\a2.txt", &mut |_, _| true).is_err());
        assert!(p.rename(&mount, "docs", "dokumenty", &mut |_, _| true).is_err());
        assert!(p.add(&mount, &[], &mut |_, _| true).is_err() && p.mkdir(&mount, "x", &mut |_, _| true).is_err());

        // a file deleted elsewhere leaves the results
        fs::remove_file(root.join("a2.txt")).unwrap();
        assert_eq!(p.list(&mount, "").unwrap().items.len(), 1);

        // the flat list: files only, under their own names
        let list = Mount { authority: r.flat.clone(), ..r.mount.clone() }.root();
        let ix = p.list(&list, "").unwrap();
        assert_eq!(ix.items.iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["c.txt"]);
        assert_eq!(p.real_path(&list, "c.txt"), Some(root.join("docs").join("old").join("c.txt")));
        assert_eq!(p.activity(&list), p.activity(&mount), "one search behind both");

        // the search ends when both connections are closed
        p.close(&mount);
        assert!(p.list(&list, "").is_ok());
        p.close(&list);
        assert_eq!(p.list(&mount, "").unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(p.activity(&mount), Activity::default());
    }

    #[test]
    fn flat_list_numbers_repeated_names() {
        let t = tempfile::tempdir().unwrap();
        for d in ["a", "b", "c"] {
            fs::create_dir(t.path().join(d)).unwrap();
            fs::write(t.path().join(d).join("zpráva.txt"), d).unwrap();
        }
        fs::write(t.path().join("a").join("README"), "x").unwrap();
        fs::write(t.path().join("b").join("readme"), "y").unwrap();
        let p = Search::start(Path::new(".")).unwrap();
        let dir = t.path().display().to_string();
        let Reply::Results(r) = start(&p, &[("dir", &dir), ("names", "*"), ("subdirs", "1"), ("folders", "1")]).unwrap() else { panic!() };
        wait(&p, &r.mount.root());
        let list = Mount { authority: r.flat.clone(), ..r.mount.clone() }.root();
        let names: Vec<String> = p.list(&list, "").unwrap().items.into_iter().map(|i| i.path).collect();
        assert_eq!(names, ["a", "b", "c", "README", "zpráva.txt", "readme (2)", "zpráva (2).txt", "zpráva (3).txt"], "files and folders; repeated names numbered");
        // a found folder opens to what it really contains
        let inside: Vec<String> = p.list(&list, "b").unwrap().items.into_iter().map(|i| i.path).collect();
        assert_eq!(inside.len(), 2);
        assert!(inside.contains(&"b\\readme".to_owned()) && inside.contains(&"b\\zpráva.txt".to_owned()));
        assert_eq!(p.real_path(&list, "b\\readme"), Some(t.path().join("b").join("readme")));
        assert!(p.list(&list, "README").is_err(), "a file is no folder");
        assert_eq!(p.real_path(&list, "zpráva (3).txt"), Some(t.path().join("c").join("zpráva.txt")));
        // copied out and renamed under the names shown
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        p.extract(&list, &["zpráva (2).txt".into()], &out, &mut |_, _| true).unwrap();
        assert_eq!(fs::read_to_string(out.join("zpráva (2).txt")).unwrap(), "b");
        p.rename(&list, "zpráva (3).txt", "dopis.txt", &mut |_, _| true).unwrap();
        assert!(t.path().join("c").join("dopis.txt").is_file());
        assert_eq!(p.real_path(&r.mount.root(), "c\\dopis.txt"), Some(t.path().join("c").join("dopis.txt")), "the folders view follows");
    }

    #[test]
    fn folders_and_stopping() {
        let t = tempfile::tempdir().unwrap();
        for i in 0..30 {
            let d = t.path().join(format!("složka {i:02}"));
            fs::create_dir(&d).unwrap();
            fs::write(d.join("x.log"), "x").unwrap();
        }
        let p = Search::start(Path::new(".")).unwrap();
        let dir = t.path().display().to_string();
        let Reply::Results(r) = start(&p, &[("dir", &dir), ("names", "složka 0*"), ("subdirs", "1"), ("folders", "1")]).unwrap() else { panic!() };
        let mount = r.mount.root();
        wait(&p, &mount);
        let ix = p.list(&mount, "").unwrap();
        assert_eq!(ix.items.len(), 10);
        assert!(ix.items.iter().all(|i| i.dir));
        // a found folder is copied whole
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        p.extract(&mount, &["složka 03".into()], &out, &mut |_, _| true).unwrap();
        assert!(out.join("složka 03").join("x.log").is_file());

        // stopped at once: the search ends and says so
        let Reply::Results(r) = start(&p, &[("dir", &dir), ("names", "*"), ("subdirs", "1"), ("text", "zzz")]).unwrap() else { panic!() };
        let mount = r.mount.root();
        p.stop(&mount);
        let a = wait(&p, &mount);
        assert!(a.text.starts_with("Stopped") || a.text.starts_with("Done"), "{}", a.text);
    }

    #[cfg(windows)]
    #[test]
    fn delete_moves_to_the_recycle_bin() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join("sub")).unwrap();
        fs::write(t.path().join("sub").join("smazat 🦎.tmp"), "x").unwrap();
        fs::write(t.path().join("sub").join("nechat.txt"), "x").unwrap();
        let p = Search::start(Path::new(".")).unwrap();
        let dir = t.path().display().to_string();
        let Reply::Results(r) = start(&p, &[("dir", &dir), ("names", "*.tmp"), ("subdirs", "1")]).unwrap() else { panic!() };
        let mount = r.mount.root();
        wait(&p, &mount);
        // a folder leading to results: the results inside it go, nothing else
        p.delete(&mount, &["sub".into()], &mut |_, _| true).unwrap();
        assert!(!t.path().join("sub").join("smazat 🦎.tmp").exists());
        assert!(t.path().join("sub").join("nechat.txt").exists());
        assert!(p.list(&mount, "").unwrap().items.is_empty());
    }
}
