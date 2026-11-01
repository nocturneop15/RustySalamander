//! The Compare plugin: compares two files, side by side in a window of its
//! own, or two folders, in the two panels.
//!
//! **Folders** become two connections, `compare://<n>-left` in the left
//! panel and `compare://<n>-right` in the right ([`Reply::Panels`]). Each
//! lists everything either side has, so the two lists line up: what only the
//! other side has stands in as a placeholder. Items are marked
//! ([`FolderPlugin::marks`]): the same, different, newer, older, only here,
//! missing here. The items are the real files ([`FolderPlugin::real_path`]):
//! copying (F5) from one side into the other, deleting (F8, to the Recycle
//! Bin) and new folders (F7) change the real folders, and the comparison
//! follows.
//!
//! **What is compared** (the command Compare): two selected items in the
//! active panel; else the selected item of each panel; else the items under
//! the two cursors; else the two panels' folders.

mod diff;
mod tree;
mod window;

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use sal_plugin_api::ui::decode;
use sal_plugin_api::{
    export_plugin, mount_of, n_, tr, trf, Activity, Command, Field, FieldKind, FolderPlugin, Form, Index, Item, ItemMark, Mount, Panels, Plugin, Progress, Reply, Updates, Values,
};

use tree::{By, Comparison};

/// The scheme of the comparisons' connections.
const SCHEME: &str = "compare";

/// A comparison of two folders, behind its two connections.
struct Job {
    comparison: Mutex<Comparison>,
    /// Comparing (again) now.
    running: AtomicBool,
    stop: AtomicBool,
    /// Connections still open of the two (the job ends when both closed).
    views: AtomicU32,
}

pub struct Compare {
    next: AtomicU32,
    /// Comparisons by number ("1" of `1-left`, `1-right`).
    jobs: Mutex<HashMap<String, Arc<Job>>>,
    /// Windows comparing files.
    windows: sal_window::Windows,
    /// The values of the last form, offered again.
    last: Mutex<Values>,
}

export_plugin!(Compare);

impl Plugin for Compare {
    const ID: &'static str = "compare";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const UPDATES: Updates = Updates { repo: "nocturneop15/RustySalamander", tag: "compare-v", changelog: "plugins/compare/CHANGELOG.md" };
    const NAME: &'static str = n_("Compare");
    const DESCRIPTION: &'static str = n_("Compares two files side by side, or two folders in the two panels, with the differences marked. Start with Plugins → Compare.");

    fn start(_dir: &Path) -> Result<Self, String> {
        Ok(Compare {
            next: AtomicU32::new(1),
            jobs: Mutex::new(HashMap::new()),
            windows: sal_window::Windows::new(sal_ui::icon::program, [1100.0, 700.0]),
            last: Mutex::new(Values::new()),
        })
    }
}

/// The comparison's number and side (`true`: left) of a connection root or
/// a path inside it, and the path inside.
fn side_of(path: &Path) -> Option<(String, bool, String)> {
    let (scheme, authority, inner) = mount_of(path)?;
    if scheme != SCHEME {
        return None;
    }
    let (id, side) = authority.rsplit_once('-')?;
    Some((id.to_owned(), side == "left", inner))
}

/// A path from the values (`panel`, `cursor`, …); `None` when empty.
fn value_path(values: &Values, key: &str) -> Option<PathBuf> {
    values.get(key).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The paths of a list in the values (`selected`, `other_selected`).
fn value_list(values: &Values, key: &str) -> Vec<PathBuf> {
    values.get(key).and_then(|v| decode(v)).unwrap_or_default().into_iter().map(PathBuf::from).collect()
}

/// An error a person reads, translated.
fn refused(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, tr(msg))
}

impl Compare {
    /// The comparison of a connection, and the side.
    fn job(&self, path: &Path) -> io::Result<(Arc<Job>, bool, String)> {
        let (id, left, inner) = side_of(path).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        let job = self.jobs.lock().unwrap().get(&id).cloned().ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        Ok((job, left, inner))
    }

    /// What to compare, from the panels: two selected items in the active
    /// panel; else one selected in each; else the two cursors' items; else
    /// the two folders. Left first.
    fn pick(values: &Values) -> io::Result<(PathBuf, PathBuf)> {
        let left_active = values.get("side").is_none_or(|s| s != "right");
        let order = |active: PathBuf, other: PathBuf| if left_active { (active, other) } else { (other, active) };
        let (sel, other_sel) = (value_list(values, "selected"), value_list(values, "other_selected"));
        if sel.len() == 2 {
            return Ok((sel[0].clone(), sel[1].clone()));
        }
        if sel.len() == 1 && other_sel.len() == 1 {
            return Ok(order(sel[0].clone(), other_sel[0].clone()));
        }
        if let (Some(a), Some(b)) = (value_path(values, "cursor"), value_path(values, "other_cursor")) {
            if a.is_dir() == b.is_dir() {
                return Ok(order(a, b));
            }
        }
        match (value_path(values, "panel"), value_path(values, "other")) {
            (Some(a), Some(b)) => Ok(order(a, b)),
            _ => Err(refused(n_("Choose what to compare: two items, or the two panels' folders."))),
        }
    }

    /// Compare two files or two folders.
    fn compare(&self, left: PathBuf, right: PathBuf, by: By, options: &Values) -> io::Result<Reply> {
        for p in [&left, &right] {
            if mount_of(p).is_some() || !p.exists() {
                return Err(io::Error::new(io::ErrorKind::NotFound, trf("{path} is not a file or folder on disk; only those can be compared.", &[("path", &p.display())])));
            }
        }
        match (left.is_dir(), right.is_dir()) {
            (false, false) => {
                self.windows.open(Box::new(move || Box::new(window::DiffWin::new(left, right))), options.clone())?;
                Ok(Reply::Done)
            }
            (true, true) => self.compare_folders(left, right, by),
            _ => Err(refused(n_("A file and a folder can't be compared."))),
        }
    }

    /// Compare two folders into a new pair of connections.
    fn compare_folders(&self, left: PathBuf, right: PathBuf, by: By) -> io::Result<Reply> {
        let stop = AtomicBool::new(false);
        let comparison = Comparison::new(&left, &right, by, &stop)?;
        let id = self.next.fetch_add(1, Ordering::Relaxed).to_string();
        let job = Arc::new(Job { comparison: Mutex::new(comparison), running: AtomicBool::new(false), stop, views: AtomicU32::new(2) });
        self.jobs.lock().unwrap().insert(id.clone(), job);
        let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string());
        Ok(Reply::Panels(Panels {
            name: format!("{} ↔ {}", name(&left), name(&right)),
            left: Mount { scheme: SCHEME.into(), authority: format!("{id}-left"), folder: String::new() },
            right: Mount { scheme: SCHEME.into(), authority: format!("{id}-right"), folder: String::new() },
            refresh: "refresh".into(),
        }))
    }

    /// Compare the folders of a connection again, as the first time.
    fn refresh(&self, root: &Path) -> io::Result<()> {
        let (job, _, _) = self.job(root)?;
        job.running.store(true, Ordering::Relaxed);
        let r = job.comparison.lock().unwrap().refresh("", &job.stop);
        job.running.store(false, Ordering::Relaxed);
        r
    }

    /// The form comparing two files, or two folders (`folders`).
    fn form(&self, folders: bool, values: &Values) -> Form {
        let last = self.last.lock().unwrap().clone();
        let left_active = values.get("side").is_none_or(|s| s != "right");
        let (active, other) = if folders { ("panel", "other") } else { ("cursor", "other_cursor") };
        let start = |key: &str| values.get(key).cloned().unwrap_or_default();
        let (l, r) = if left_active { (start(active), start(other)) } else { (start(other), start(active)) };
        let kind = if folders { FieldKind::Folder } else { FieldKind::File };
        let mut fields = vec![
            Field { id: "left".into(), label: tr("Left:"), kind: kind.clone(), value: l, ..Default::default() },
            Field { id: "right".into(), label: tr("Right:"), kind, value: r, ..Default::default() },
        ];
        if folders {
            fields.push(Field {
                id: "by".into(),
                label: tr("Files are the same when:"),
                kind: FieldKind::Choice(vec![("date".into(), tr("Their size and date are")), ("content".into(), tr("Their contents are (slower)"))]),
                value: last.get("by").cloned().unwrap_or_else(|| "date".into()),
                enabled_if: None,
            });
        }
        Form {
            title: if folders { tr("Compare folders") } else { tr("Compare files") },
            fields,
            ok: tr("Compare"),
            command: if folders { "folders".into() } else { "files".into() },
        }
    }
}

impl FolderPlugin for Compare {
    fn id(&self) -> &str {
        Self::ID
    }
    fn handles(&self, _: &str) -> bool {
        false
    }
    fn can_write(&self, file: &Path) -> bool {
        side_of(file).is_some()
    }
    fn status(&self) -> String {
        String::new()
    }

    /// Everything either side has in `folder`: this side's items, and
    /// placeholders for what only the other side has.
    fn list(&self, file: &Path, folder: &str) -> io::Result<Index> {
        let (job, left, _) = self.job(file)?;
        let c = job.comparison.lock().unwrap();
        if !folder.is_empty() && !c.nodes.get(folder).is_some_and(|n| n.dir()) {
            return Err(io::ErrorKind::NotFound.into());
        }
        let items = c
            .children(folder)
            .map(|(path, n)| {
                let mine = if left { n.left.as_ref() } else { n.right.as_ref() };
                Item {
                    path: path.clone(),
                    dir: n.dir(),
                    size: mine.map_or(0, |s| s.size),
                    modified: mine.and_then(|s| s.modified),
                    attrs: mine.map_or(0, |s| s.attrs),
                    index: None,
                }
            })
            .collect();
        Ok(Index { items, complete: false })
    }

    fn marks(&self, file: &Path, folder: &str) -> Vec<(String, ItemMark)> {
        let Ok((job, left, _)) = self.job(file) else { return Vec::new() };
        let c = job.comparison.lock().unwrap();
        c.children(folder).map(|(_, n)| (n.name.clone(), n.mark(left))).collect()
    }

    fn real_path(&self, file: &Path, inner: &str) -> Option<PathBuf> {
        let (job, left, _) = self.job(file).ok()?;
        let c = job.comparison.lock().unwrap();
        let n = c.nodes.get(inner)?;
        let has = if left { n.left.is_some() } else { n.right.is_some() };
        has.then(|| c.path(left, inner))
    }

    /// Copy the real items of this side out.
    fn extract(&self, file: &Path, items: &[String], out: &Path, progress: &mut Progress) -> io::Result<()> {
        let mut done = 0u64;
        for inner in items {
            let real = self.real_path(file, inner).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, trf("{name} is only on the other side.", &[("name", inner)])))?;
            let to = out.join(inner);
            if let Some(dir) = to.parent() {
                fs::create_dir_all(dir)?;
            }
            copy_item(&real, &to, &mut |n| {
                done += n;
                progress(done, 0)
            })?;
        }
        Ok(())
    }

    /// Copy items from disk into this side's real folder, and compare them
    /// again.
    fn add(&self, file: &Path, items: &[(PathBuf, String)], progress: &mut Progress) -> io::Result<()> {
        let (job, left, _) = self.job(file)?;
        let mut done = 0u64;
        for (disk, inner) in items {
            let to = job.comparison.lock().unwrap().path(left, inner);
            if let Some(dir) = to.parent() {
                fs::create_dir_all(dir)?;
            }
            copy_item(disk, &to, &mut |n| {
                done += n;
                progress(done, 0)
            })?;
            job.comparison.lock().unwrap().refresh(inner, &job.stop)?;
        }
        Ok(())
    }

    /// Move this side's real items to the Recycle Bin, and compare again.
    fn delete(&self, file: &Path, items: &[String], _: &mut Progress) -> io::Result<()> {
        let (job, _, _) = self.job(file)?;
        let paths: Vec<PathBuf> = items.iter().filter_map(|i| self.real_path(file, i)).collect();
        recycle(&paths)?;
        let mut c = job.comparison.lock().unwrap();
        for inner in items {
            c.refresh(inner, &job.stop)?;
        }
        Ok(())
    }

    fn rename(&self, _: &Path, _: &str, _: &str, _: &mut Progress) -> io::Result<()> {
        Err(refused(n_("Items can't be renamed in a comparison; rename them in their folder.")))
    }

    /// A new folder in this side's real folder.
    fn mkdir(&self, file: &Path, inner: &str, _: &mut Progress) -> io::Result<()> {
        let (job, left, _) = self.job(file)?;
        let path = job.comparison.lock().unwrap().path(left, inner);
        fs::create_dir(&path)?;
        let mut c = job.comparison.lock().unwrap();
        c.refresh(inner, &job.stop)
    }

    fn commands(&self) -> Vec<Command> {
        vec![
            Command { id: "compare".into(), label: tr("Compare (selection, cursors or panels)") },
            Command { id: "files_form".into(), label: tr("Compare files…") },
            Command { id: "folders_form".into(), label: tr("Compare folders…") },
        ]
    }

    fn run(&self, command: &str, input: &Values) -> io::Result<Reply> {
        let options = input.clone();
        match command {
            "compare" => {
                let (l, r) = Compare::pick(input)?;
                let by = if self.last.lock().unwrap().get("by").is_some_and(|b| b == "content") { By::Content } else { By::SizeDate };
                self.compare(l, r, by, &options)
            }
            "files_form" => Ok(Reply::Form(self.form(false, input))),
            "folders_form" => Ok(Reply::Form(self.form(true, input))),
            "files" | "folders" => {
                let path = |k: &str| input.get(k).map(|v| PathBuf::from(v.trim().trim_matches('"'))).filter(|p| !p.as_os_str().is_empty());
                let (Some(l), Some(r)) = (path("left"), path("right")) else { return Err(refused(n_("Fill in both, the left and the right."))) };
                if command == "folders" {
                    self.last.lock().unwrap().insert("by".into(), input.get("by").cloned().unwrap_or_default());
                }
                let by = if input.get("by").is_some_and(|b| b == "content") { By::Content } else { By::SizeDate };
                self.compare(l, r, by, &options)
            }
            "refresh" => {
                let root = input.get("results").map(PathBuf::from).ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
                self.refresh(&root)?;
                Ok(Reply::Done)
            }
            _ => Err(io::Error::new(io::ErrorKind::Unsupported, format!("unknown command \"{command}\""))),
        }
    }

    /// One of a comparison's two connections closed; the comparison ends with
    /// the second.
    fn close(&self, root: &Path) {
        let Some((id, _, _)) = side_of(root) else { return };
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get(&id) {
            if job.views.fetch_sub(1, Ordering::Relaxed) <= 1 {
                job.stop.store(true, Ordering::Relaxed);
                jobs.remove(&id);
            }
        }
    }

    fn activity(&self, root: &Path) -> Activity {
        match self.job(root) {
            Ok((job, _, _)) if job.running.load(Ordering::Relaxed) => Activity { running: true, text: tr("Comparing…") },
            _ => Activity::default(),
        }
    }
}

/// Copy a file, or a folder with everything inside; `progress` gets the
/// bytes of each file and returns false to cancel.
fn copy_item(from: &Path, to: &Path, progress: &mut dyn FnMut(u64) -> bool) -> io::Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for e in fs::read_dir(from)? {
            let e = e?;
            copy_item(&e.path(), &to.join(e.file_name()), progress)?;
        }
        return Ok(());
    }
    let n = fs::copy(from, to)?;
    if !progress(n) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, tr("Cancelled.")));
    }
    Ok(())
}

/// Move items to the Recycle Bin.
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

/// Elsewhere there is no Recycle Bin: deleted for good.
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

/// The root of side `left` of comparison `id` (tests).
#[cfg(test)]
fn root(id: &str, left: bool) -> PathBuf {
    sal_plugin_api::mount_root(SCHEME, &format!("{id}-{}", if left { "left" } else { "right" }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> Values {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// What Compare compares, from the panels.
    #[test]
    fn what_is_compared() {
        let enc = |l: &[&str]| sal_plugin_api::ui::encode(l);
        // two selected in the active panel
        let v = values(&[("selected", &enc(&["C:\\a", "C:\\b"])), ("side", "right"), ("panel", "C:\\"), ("other", "D:\\")]);
        assert_eq!(Compare::pick(&v).unwrap(), (PathBuf::from("C:\\a"), PathBuf::from("C:\\b")));
        // one selected in each: left first, also when the right is active
        let v = values(&[("selected", &enc(&["D:\\x"])), ("other_selected", &enc(&["C:\\y"])), ("side", "right")]);
        assert_eq!(Compare::pick(&v).unwrap(), (PathBuf::from("C:\\y"), PathBuf::from("D:\\x")));
        // nothing selected, no cursors: the two folders
        let v = values(&[("panel", "C:\\l"), ("other", "D:\\r"), ("side", "left"), ("selected", "")]);
        assert_eq!(Compare::pick(&v).unwrap(), (PathBuf::from("C:\\l"), PathBuf::from("D:\\r")));
        assert!(Compare::pick(&Values::new()).is_err());
    }

    /// Two folders become two connections that list both sides, marked;
    /// copying an item across makes it the same.
    #[test]
    fn folders_in_two_panels() {
        let t = tempfile::tempdir().unwrap();
        let (l, r) = (t.path().join("L"), t.path().join("R"));
        fs::create_dir_all(&l).unwrap();
        fs::create_dir_all(&r).unwrap();
        fs::write(l.join("a.txt"), "a").unwrap();
        fs::write(r.join("b.txt"), "b").unwrap();
        let p = Compare::start(t.path()).unwrap();
        let v = values(&[("left", &l.display().to_string()), ("right", &r.display().to_string()), ("by", "content")]);
        let Reply::Panels(panels) = p.run("folders", &v).unwrap() else { panic!("two panels") };
        assert_eq!(panels.name, "L ↔ R");
        let (lr, rr) = (panels.left.root(), panels.right.root());
        assert_eq!((lr.clone(), rr.clone()), (root("1", true), root("1", false)));
        // both list a and b; b is a placeholder on the left
        let names = |root: &Path| p.list(root, "").unwrap().items.into_iter().map(|i| i.path).collect::<Vec<_>>();
        assert_eq!(names(&lr), ["a.txt", "b.txt"]);
        assert_eq!(names(&rr), ["a.txt", "b.txt"]);
        let marks: HashMap<String, ItemMark> = p.marks(&lr, "").into_iter().collect();
        assert_eq!((marks["a.txt"], marks["b.txt"]), (ItemMark::OnlyHere, ItemMark::Missing));
        assert_eq!(p.real_path(&lr, "a.txt"), Some(l.join("a.txt")));
        assert_eq!(p.real_path(&lr, "b.txt"), None, "a placeholder is no file");
        // copying a.txt from the left into the right side
        let tmp = t.path().join("tmp");
        p.extract(&lr, &["a.txt".into()], &tmp, &mut |_, _| true).unwrap();
        p.add(&rr, &[(tmp.join("a.txt"), "a.txt".into())], &mut |_, _| true).unwrap();
        assert_eq!(fs::read_to_string(r.join("a.txt")).unwrap(), "a");
        let marks: HashMap<String, ItemMark> = p.marks(&rr, "").into_iter().collect();
        assert_eq!(marks["a.txt"], ItemMark::Same);
        assert!(p.extract(&lr, &["b.txt".into()], &tmp, &mut |_, _| true).is_err(), "only on the other side");
        // refresh notices changes made elsewhere
        fs::write(l.join("c.txt"), "c").unwrap();
        p.run("refresh", &values(&[("results", &sal_plugin_api::ui::path_value(&lr))])).unwrap();
        assert_eq!(names(&rr), ["a.txt", "b.txt", "c.txt"]);
        // both connections closed: the comparison is gone
        p.close(&lr);
        assert!(p.list(&rr, "").is_ok());
        p.close(&rr);
        assert!(p.list(&rr, "").is_err());
        // a file and a folder, or something not on disk, are refused
        let v = values(&[("left", &l.join("a.txt").display().to_string()), ("right", &r.display().to_string())]);
        assert!(p.run("files", &v).is_err());
        assert!(p.run("files", &values(&[("left", "")])).is_err());
    }
}
