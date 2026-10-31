//! The program's side: a plugin found in a DLL, wrapped back into a
//! [`FolderPlugin`]. Plugin authors don't need this module; loading the DLL
//! itself (`LoadLibrary`, finding [`ENTRY`]) is the program's job.

use std::ffi::c_void;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::abi::*;
use crate::{Activity, Command, FolderPlugin, Index, Item, Opener, Progress, Reply, Values};

/// Who a plugin is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    /// Its id ([`Descriptor::id`]).
    pub id: String,
    /// Its name, in English as the plugin gives it.
    pub name: String,
    /// What it does, in English as the plugin gives it.
    pub description: String,
    /// Its own version ([`crate::Plugin::VERSION`]).
    pub version: String,
    /// The interface version it was built for ([`Descriptor::version`]).
    pub interface: u32,
    /// Where newer versions are published, if anywhere.
    pub updates: Option<Source>,
}

/// [`crate::Updates`] of a plugin, as the program keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// `owner/name` of the GitHub repository.
    pub repo: String,
    /// What the tags of the plugin's releases start with.
    pub tag: String,
    /// The changelog's path in the repository; empty for none.
    pub changelog: String,
}

/// What `desc` says about its plugin; an error for an interface version the
/// program doesn't have.
///
/// # Safety
/// `desc` must be a plugin's descriptor (from its [`ENTRY`]).
pub unsafe fn info(desc: &'static Descriptor) -> Result<Info, String> {
    if !(MIN_VERSION..=VERSION).contains(&desc.version) {
        let range = if MIN_VERSION == VERSION { VERSION.to_string() } else { format!("{MIN_VERSION} to {VERSION}") };
        return Err(format!("made for another version of the program (interface {}, this one has {range})", desc.version));
    }
    let id = desc.id.get().to_owned();
    if id.is_empty() {
        return Err("the plugin has no id".into());
    }
    let repo = desc.update_repo.get().to_owned();
    let updates = (!repo.is_empty()).then(|| Source { repo, tag: desc.update_tag.get().to_owned(), changelog: desc.changelog.get().to_owned() });
    Ok(Info {
        id,
        name: desc.name.get().to_owned(),
        description: desc.description.get().to_owned(),
        version: desc.plugin_version.get().to_owned(),
        interface: desc.version,
        updates,
    })
}

/// Start the plugin of `desc`; `dir` is its folder.
///
/// # Safety
/// `desc` must be a plugin's descriptor that [`info`] accepted.
pub unsafe fn start(desc: &'static Descriptor, host: &'static HostApi, dir: &Path) -> Result<Arc<dyn FolderPlugin>, String> {
    let Info { id, interface, .. } = info(desc)?;
    let dir = Wide::encode(dir);
    let mut error = String::new();
    let this = (desc.start)(host, Wide::of(&dir), &StrOut::new(&mut error));
    if this.is_null() {
        return Err(if error.is_empty() { "the plugin didn't start".into() } else { error });
    }
    Ok(Arc::new(Remote { id, interface, this, vt: &*desc.vtable }))
}

/// A started plugin in a DLL.
struct Remote {
    id: String,
    /// The interface version it was built for: functions added later aren't
    /// in its [`VTable`].
    interface: u32,
    this: *mut c_void,
    vt: &'static VTable,
}

impl Remote {
    /// Whether its [`VTable`] has the functions added in `version`.
    fn has(&self, version: u32) -> bool {
        self.interface >= version
    }
}

// The plugin is a FolderPlugin on its side, which is Send + Sync.
unsafe impl Send for Remote {}
unsafe impl Sync for Remote {}

impl Drop for Remote {
    fn drop(&mut self) {
        unsafe { (self.vt.free)(self.this) }
    }
}

/// Run a call that reports an error code and message.
fn call(f: impl FnOnce(*const StrOut) -> i32) -> io::Result<()> {
    let mut msg = String::new();
    let out = StrOut::new(&mut msg);
    match f(&out) {
        OK => Ok(()),
        code => Err(io::Error::new(kind_of(code), msg)),
    }
}

unsafe extern "C" fn call_progress(ctx: *mut c_void, done: u64, total: u64) -> bool {
    let f = &mut *(ctx as *mut &mut Progress);
    f(done, total)
}

fn with_progress<R>(progress: &mut Progress, f: impl FnOnce(*const ProgressFn) -> R) -> R {
    let mut p: &mut Progress = progress;
    let pf = ProgressFn { ctx: &mut p as *mut &mut Progress as *mut c_void, call: call_progress };
    f(&pf)
}

unsafe extern "C" fn put_item(ctx: *mut c_void, raw: *const RawItem) {
    let items = &mut *(ctx as *mut Vec<Item>);
    let r = &*raw;
    items.push(Item { path: r.path.get().to_owned(), dir: r.dir, size: r.size, modified: from_filetime(r.modified), attrs: r.attrs, index: r.has_index.then_some(r.index) });
}

impl FolderPlugin for Remote {
    fn id(&self) -> &str {
        &self.id
    }

    fn handles(&self, name: &str) -> bool {
        unsafe { (self.vt.handles)(self.this, Str::new(name)) }
    }

    fn can_write(&self, file: &Path) -> bool {
        let file = Wide::encode(file);
        unsafe { (self.vt.can_write)(self.this, Wide::of(&file)) }
    }

    fn status(&self) -> String {
        let mut s = String::new();
        unsafe { (self.vt.status)(self.this, &StrOut::new(&mut s)) };
        s
    }

    fn list(&self, file: &Path, folder: &str) -> io::Result<Index> {
        let file = Wide::encode(file);
        let mut items: Vec<Item> = Vec::new();
        let mut complete = false;
        let out = ItemOut { ctx: &mut items as *mut Vec<Item> as *mut c_void, put: put_item };
        call(|e| unsafe { (self.vt.list)(self.this, Wide::of(&file), Str::new(folder), &out, &mut complete, e) })?;
        Ok(Index { items, complete })
    }

    fn extract(&self, file: &Path, items: &[String], out: &Path, progress: &mut Progress) -> io::Result<()> {
        let (file, out) = (Wide::encode(file), Wide::encode(out));
        let strs: Vec<Str> = items.iter().map(|s| Str::new(s)).collect();
        with_progress(progress, |p| call(|e| unsafe { (self.vt.extract)(self.this, Wide::of(&file), strs.as_ptr(), strs.len(), Wide::of(&out), p, e) }))
    }

    fn add(&self, file: &Path, items: &[(PathBuf, String)], progress: &mut Progress) -> io::Result<()> {
        let file = Wide::encode(file);
        let disk_units: Vec<Vec<u16>> = items.iter().map(|(d, _)| Wide::encode(d)).collect();
        let disk: Vec<Wide> = disk_units.iter().map(|u| Wide::of(u)).collect();
        let inner: Vec<Str> = items.iter().map(|(_, i)| Str::new(i)).collect();
        with_progress(progress, |p| call(|e| unsafe { (self.vt.add)(self.this, Wide::of(&file), disk.as_ptr(), inner.as_ptr(), items.len(), p, e) }))
    }

    fn delete(&self, file: &Path, items: &[String], progress: &mut Progress) -> io::Result<()> {
        let file = Wide::encode(file);
        let strs: Vec<Str> = items.iter().map(|s| Str::new(s)).collect();
        with_progress(progress, |p| call(|e| unsafe { (self.vt.delete)(self.this, Wide::of(&file), strs.as_ptr(), strs.len(), p, e) }))
    }

    fn rename(&self, file: &Path, old: &str, new: &str, progress: &mut Progress) -> io::Result<()> {
        let file = Wide::encode(file);
        with_progress(progress, |p| call(|e| unsafe { (self.vt.rename)(self.this, Wide::of(&file), Str::new(old), Str::new(new), p, e) }))
    }

    fn mkdir(&self, file: &Path, inner: &str, progress: &mut Progress) -> io::Result<()> {
        let file = Wide::encode(file);
        with_progress(progress, |p| call(|e| unsafe { (self.vt.mkdir)(self.this, Wide::of(&file), Str::new(inner), p, e) }))
    }

    fn commands(&self) -> Vec<Command> {
        let mut s = String::new();
        unsafe { (self.vt.commands)(self.this, &StrOut::new(&mut s)) };
        crate::ui::decode_commands(&s)
    }

    fn run(&self, command: &str, input: &Values) -> io::Result<Reply> {
        let values = crate::ui::encode_values(input);
        let mut reply = String::new();
        let out = StrOut::new(&mut reply);
        call(|e| unsafe { (self.vt.run)(self.this, Str::new(command), Str::new(&values), &out, e) })?;
        crate::ui::decode_reply(&reply).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "the plugin sent a damaged reply"))
    }

    fn close(&self, root: &Path) {
        let root = Wide::encode(root);
        unsafe { (self.vt.close)(self.this, Wide::of(&root)) }
    }

    fn activity(&self, root: &Path) -> Activity {
        let root = Wide::encode(root);
        let mut text = String::new();
        let running = unsafe { (self.vt.activity)(self.this, Wide::of(&root), &StrOut::new(&mut text)) };
        Activity { running, text }
    }

    fn stop(&self, root: &Path) {
        let root = Wide::encode(root);
        unsafe { (self.vt.stop)(self.this, Wide::of(&root)) }
    }

    fn real_path(&self, file: &Path, inner: &str) -> Option<PathBuf> {
        let file = Wide::encode(file);
        let mut out = None;
        unsafe { (self.vt.real_path)(self.this, Wide::of(&file), Str::new(inner), &PathOut::new(&mut out)) };
        out
    }

    fn openers(&self) -> Vec<Opener> {
        if !self.has(5) {
            return Vec::new();
        }
        let mut s = String::new();
        unsafe { (self.vt.openers)(self.this, &StrOut::new(&mut s)) };
        crate::ui::decode_openers(&s)
    }

    fn open_file(&self, opener: &str, file: &Path, options: &Values) -> io::Result<()> {
        if !self.has(5) {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let file = Wide::encode(file);
        let options = crate::ui::encode_values(options);
        call(|e| unsafe { (self.vt.open_file)(self.this, Str::new(opener), Wide::of(&file), Str::new(&options), e) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{trf, Plugin};
    use std::sync::Mutex;

    /// A plugin for `.mem` files kept in memory; adding "boom" panics.
    struct Mem {
        items: Mutex<Vec<Item>>,
        closed: Mutex<Vec<PathBuf>>,
    }

    impl FolderPlugin for Mem {
        fn id(&self) -> &str {
            Self::ID
        }
        fn handles(&self, name: &str) -> bool {
            name.ends_with(".mem")
        }
        fn can_write(&self, file: &Path) -> bool {
            file.extension().is_some_and(|e| e == "mem")
        }
        fn status(&self) -> String {
            trf("{n} items", &[("n", &self.items.lock().unwrap().len())])
        }
        fn list(&self, _: &Path, folder: &str) -> io::Result<Index> {
            if folder == "missing" {
                return Err(io::ErrorKind::NotFound.into());
            }
            Ok(Index { items: self.items.lock().unwrap().clone(), complete: true })
        }
        fn extract(&self, _: &Path, items: &[String], out: &Path, progress: &mut Progress) -> io::Result<()> {
            if !progress(1, 2) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            for i in items {
                std::fs::write(out.join(i), i)?;
            }
            Ok(())
        }
        fn add(&self, _: &Path, items: &[(PathBuf, String)], _: &mut Progress) -> io::Result<()> {
            for (disk, inner) in items {
                assert_ne!(inner, "boom");
                let md = std::fs::metadata(disk)?;
                self.items.lock().unwrap().push(Item { path: inner.clone(), dir: false, size: md.len(), modified: md.modified().ok(), attrs: 0x20, index: Some(7) });
            }
            Ok(())
        }
        fn delete(&self, _: &Path, items: &[String], _: &mut Progress) -> io::Result<()> {
            self.items.lock().unwrap().retain(|it| !items.contains(&it.path));
            Ok(())
        }
        fn rename(&self, _: &Path, old: &str, new: &str, _: &mut Progress) -> io::Result<()> {
            let mut items = self.items.lock().unwrap();
            if items.iter().any(|it| it.path == new) {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("\"{new}\" exists")));
            }
            items.iter_mut().filter(|it| it.path == old).for_each(|it| it.path = new.to_owned());
            Ok(())
        }
        fn mkdir(&self, _: &Path, inner: &str, _: &mut Progress) -> io::Result<()> {
            self.items.lock().unwrap().push(Item { path: inner.to_owned(), dir: true, size: 0, modified: None, attrs: 0x10, index: None });
            Ok(())
        }
        fn commands(&self) -> Vec<Command> {
            vec![Command { id: "open".into(), label: "Open memory…".into() }]
        }
        fn run(&self, command: &str, input: &Values) -> io::Result<Reply> {
            match command {
                "open" => Ok(Reply::Form(crate::Form {
                    title: "Open".into(),
                    ok: "OK".into(),
                    command: "mount".into(),
                    fields: vec![crate::Field { id: "name".into(), label: "Name:".into(), value: input.get("panel").cloned().unwrap_or_default(), ..Default::default() }],
                })),
                "mount" => Ok(Reply::Mount(crate::Mount { scheme: "mem".into(), authority: input["name"].clone(), folder: "a".into() })),
                _ => Err(io::Error::new(io::ErrorKind::Unsupported, "no")),
            }
        }
        fn close(&self, root: &Path) {
            self.closed.lock().unwrap().push(root.to_path_buf());
        }
        fn activity(&self, root: &Path) -> Activity {
            let stopped = self.closed.lock().unwrap().iter().any(|r| r == root);
            Activity { running: !stopped, text: format!("hledám v {} 🦎", root.display()) }
        }
        fn stop(&self, root: &Path) {
            self.closed.lock().unwrap().push(root.to_path_buf());
        }
        fn real_path(&self, _: &Path, inner: &str) -> Option<PathBuf> {
            (!inner.is_empty()).then(|| PathBuf::from(r"D:\data").join(inner))
        }
        fn openers(&self) -> Vec<Opener> {
            vec![Opener { id: "mem".into(), kind: crate::OpenKind::View, label: "Paměť".into(), masks: "*.mem".into() }]
        }
        fn open_file(&self, opener: &str, file: &Path, options: &Values) -> io::Result<()> {
            if opener != "mem" {
                return Err(io::Error::new(io::ErrorKind::Unsupported, "no such opener"));
            }
            self.closed.lock().unwrap().push(file.join(&options["theme"]));
            Ok(())
        }
    }

    impl Plugin for Mem {
        const ID: &'static str = "mem";
        const NAME: &'static str = "Memory";
        const DESCRIPTION: &'static str = "Files in memory.";
        const VERSION: &'static str = "1.2.3-beta";
        const UPDATES: crate::Updates = crate::Updates { repo: "me/mem", tag: "mem-v", changelog: "CHANGELOG.md" };
        fn start(dir: &Path) -> Result<Self, String> {
            if dir.ends_with("bad") {
                return Err(format!("{} is bad", dir.display()));
            }
            Ok(Mem { items: Mutex::new(Vec::new()), closed: Mutex::new(Vec::new()) })
        }
    }

    static VTABLE: VTable = crate::export::vtable::<Mem>();
    static DESCRIPTOR: Descriptor = crate::export::descriptor::<Mem>(&VTABLE);

    unsafe extern "C" fn translate(ctx: *mut c_void, msg: Str, out: *const StrOut) {
        if msg.get() == "{n} items" && *(ctx as *const &str) == "mem" {
            (*out).send("{n} položek");
        }
    }
    static MEM: &str = "mem";
    static HOST: HostApi = HostApi { version: VERSION, ctx: &MEM as *const &str as *mut c_void, translate };

    #[test]
    fn through_the_c_interface() {
        let info = unsafe { info(&DESCRIPTOR) }.unwrap();
        assert_eq!(
            info,
            Info {
                id: "mem".into(),
                name: "Memory".into(),
                description: "Files in memory.".into(),
                version: "1.2.3-beta".into(),
                interface: VERSION,
                updates: Some(Source { repo: "me/mem".into(), tag: "mem-v".into(), changelog: "CHANGELOG.md".into() }),
            }
        );
        assert_eq!(unsafe { start(&DESCRIPTOR, &HOST, Path::new("C:\\x\\bad")) }.err().unwrap(), "C:\\x\\bad is bad");
        let p = unsafe { start(&DESCRIPTOR, &HOST, Path::new("C:\\plugins\\mem")) }.unwrap();
        let t = std::env::temp_dir().join(format!("sal-plugin-api-{}", std::process::id()));
        std::fs::create_dir_all(&t).unwrap();
        let src = t.join("zdroj 🦎.txt");
        std::fs::write(&src, "ahoj").unwrap();
        let file = Path::new("x.mem");

        assert!(p.handles("a.mem") && !p.handles("a.zip") && p.can_write(file) && p.id() == "mem");
        p.add(file, &[(src.clone(), "složka\\日本.txt".into())], &mut |_, _| true).unwrap();
        p.mkdir(file, "prázdná", &mut |_, _| true).unwrap();
        let ix = p.list(file, "").unwrap();
        assert!(ix.complete);
        assert_eq!(p.list(file, "missing").unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(ix.items.len(), 2);
        let it = &ix.items[0];
        assert_eq!((it.path.as_str(), it.size, it.index, it.attrs), ("složka\\日本.txt", 4, Some(7), 0x20));
        let on_disk = std::fs::metadata(&src).unwrap().modified().unwrap();
        let ms = |t: std::time::SystemTime| t.duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() / 10;
        assert_eq!(ms(it.modified.unwrap()), ms(on_disk), "dates survive (to 100 ns)");
        assert_eq!(ix.items[1].index, None);
        assert_eq!(p.status(), "2 položek", "the plugin translates through the program");

        let e = p.rename(file, "prázdná", "složka\\日本.txt", &mut |_, _| true).unwrap_err();
        assert_eq!((e.kind(), e.to_string()), (io::ErrorKind::AlreadyExists, "\"složka\\日本.txt\" exists".into()));
        p.extract(file, &["výstup.txt".into()], &t, &mut |_, _| true).unwrap();
        assert_eq!(std::fs::read_to_string(t.join("výstup.txt")).unwrap(), "výstup.txt");
        let mut asked = None;
        let e = p.extract(file, &["x".into()], &t, &mut |d, total| {
            asked = Some((d, total));
            false
        });
        assert_eq!((e.unwrap_err().kind(), asked), (io::ErrorKind::Interrupted, Some((1, 2))));
        let e = p.add(file, &[(src.clone(), "boom".into())], &mut |_, _| true).unwrap_err();
        assert!(e.to_string().starts_with("the plugin failed"), "a panic becomes an error: {e}");
        p.delete(file, &["prázdná".into()], &mut |_, _| true).unwrap();
        assert_eq!(p.list(file, "").unwrap().items.len(), 1);

        // commands, forms and connections cross the interface too
        assert_eq!(p.commands(), [Command { id: "open".into(), label: "Open memory…".into() }]);
        let input: Values = [("panel".to_owned(), r"C:\x".to_owned())].into();
        let Reply::Form(form) = p.run("open", &input).unwrap() else { panic!("no form") };
        assert_eq!((form.command.as_str(), form.fields[0].value.as_str()), ("mount", r"C:\x"));
        let input: Values = [("name".to_owned(), "ram 🦎".to_owned())].into();
        let Reply::Mount(m) = p.run(&form.command, &input).unwrap() else { panic!("no mount") };
        assert_eq!(m.root(), crate::mount_root("mem", "ram 🦎"));
        assert_eq!(p.run("nothing", &Values::new()).unwrap_err().to_string(), "no");
        // a job filling a connection, stopped; items standing for files on disk
        let root = m.root();
        assert_eq!(p.activity(&root), Activity { running: true, text: format!("hledám v {} 🦎", root.display()) });
        p.stop(&root);
        assert!(!p.activity(&root).running);
        assert_eq!(p.real_path(&root, "složka\\日本.txt"), Some(PathBuf::from(r"D:\data\složka\日本.txt")));
        assert_eq!(p.real_path(&root, ""), None);
        // viewers and editors
        assert_eq!(p.openers()[0].masks, "*.mem");
        p.open_file("mem", Path::new(r"C:\x\a 🦎.mem"), &[("theme".to_owned(), "dark".to_owned())].into()).unwrap();
        assert!(p.open_file("other", file, &Values::new()).is_err());
        p.close(&m.root());
        drop(p);
        std::fs::remove_dir_all(&t).unwrap();
    }

    #[test]
    fn other_versions_are_refused() {
        static NEWER: Descriptor = Descriptor { version: VERSION + 1, ..crate::export::descriptor::<Mem>(&VTABLE) };
        assert!(unsafe { info(&NEWER) }.unwrap_err().contains("another version"));
        static OLDER: Descriptor = Descriptor { version: MIN_VERSION - 1, ..crate::export::descriptor::<Mem>(&VTABLE) };
        assert!(unsafe { info(&OLDER) }.unwrap_err().contains("another version"));
    }
}
