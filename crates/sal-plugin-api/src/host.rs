//! The program's side: a plugin found in a DLL, wrapped back into a
//! [`FolderPlugin`]. Plugin authors don't need this module; loading the DLL
//! itself (`LoadLibrary`, finding [`ENTRY`]) is the program's job.

use std::ffi::c_void;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::abi::*;
use crate::{FolderPlugin, Index, Item, Progress};

/// Who a plugin is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    /// Its id ([`Descriptor::id`]).
    pub id: String,
    /// Its name, in English as the plugin gives it.
    pub name: String,
    /// What it does, in English as the plugin gives it.
    pub description: String,
}

/// What `desc` says about its plugin; an error for another version.
///
/// # Safety
/// `desc` must be a plugin's descriptor (from its [`ENTRY`]).
pub unsafe fn info(desc: &'static Descriptor) -> Result<Info, String> {
    if desc.version != VERSION {
        return Err(format!("made for another version of the program (interface {}, this one has {VERSION})", desc.version));
    }
    let id = desc.id.get().to_owned();
    if id.is_empty() {
        return Err("the plugin has no id".into());
    }
    Ok(Info { id, name: desc.name.get().to_owned(), description: desc.description.get().to_owned() })
}

/// Start the plugin of `desc`; `dir` is its folder.
///
/// # Safety
/// `desc` must be a plugin's descriptor that [`info`] accepted.
pub unsafe fn start(desc: &'static Descriptor, host: &'static HostApi, dir: &Path) -> Result<Arc<dyn FolderPlugin>, String> {
    let id = info(desc)?.id;
    let dir = Wide::encode(dir);
    let mut error = String::new();
    let this = (desc.start)(host, Wide::of(&dir), &StrOut::new(&mut error));
    if this.is_null() {
        return Err(if error.is_empty() { "the plugin didn't start".into() } else { error });
    }
    Ok(Arc::new(Remote { id, this, vt: &*desc.vtable }))
}

/// A started plugin in a DLL.
struct Remote {
    id: String,
    this: *mut c_void,
    vt: &'static VTable,
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

    fn list(&self, file: &Path) -> io::Result<Index> {
        let file = Wide::encode(file);
        let mut items: Vec<Item> = Vec::new();
        let out = ItemOut { ctx: &mut items as *mut Vec<Item> as *mut c_void, put: put_item };
        call(|e| unsafe { (self.vt.list)(self.this, Wide::of(&file), &out, e) })?;
        Ok(Index { items })
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{trf, Plugin};
    use std::sync::Mutex;

    /// A plugin for `.mem` files kept in memory; adding "boom" panics.
    struct Mem {
        items: Mutex<Vec<Item>>,
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
        fn list(&self, _: &Path) -> io::Result<Index> {
            Ok(Index { items: self.items.lock().unwrap().clone() })
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
    }

    impl Plugin for Mem {
        const ID: &'static str = "mem";
        const NAME: &'static str = "Memory";
        const DESCRIPTION: &'static str = "Files in memory.";
        fn start(dir: &Path) -> Result<Self, String> {
            if dir.ends_with("bad") {
                return Err(format!("{} is bad", dir.display()));
            }
            Ok(Mem { items: Mutex::new(Vec::new()) })
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
        assert_eq!(info, Info { id: "mem".into(), name: "Memory".into(), description: "Files in memory.".into() });
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
        let ix = p.list(file).unwrap();
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
        assert_eq!(p.list(file).unwrap().items.len(), 1);
        drop(p);
        std::fs::remove_dir_all(&t).unwrap();
    }

    #[test]
    fn other_versions_are_refused() {
        static OLD: Descriptor = Descriptor { version: VERSION + 1, ..crate::export::descriptor::<Mem>(&VTABLE) };
        assert!(unsafe { info(&OLD) }.unwrap_err().contains("another version"));
    }
}
