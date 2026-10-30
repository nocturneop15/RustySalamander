//! The plugin's side: [`export_plugin!`](crate::export_plugin) turns a [`Plugin`] into the C
//! interface, and [`tr`] translates through the program.

use std::ffi::c_void;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use crate::abi::*;
use crate::Plugin;

/// Exports a [`Plugin`] from a DLL: `sal_plugin_api::export_plugin!(MyPlugin);`
#[macro_export]
macro_rules! export_plugin {
    ($t:ty) => {
        #[no_mangle]
        pub extern "C" fn rustysalamander_plugin() -> *const $crate::abi::Descriptor {
            static VTABLE: $crate::abi::VTable = $crate::export::vtable::<$t>();
            static DESCRIPTOR: $crate::abi::Descriptor = $crate::export::descriptor::<$t>(&VTABLE);
            &DESCRIPTOR
        }
    };
}

/// The program, once the plugin has been started.
static HOST: AtomicPtr<HostApi> = AtomicPtr::new(ptr::null_mut());

/// Marks a string for translation where it is defined; [`tr`] translates it
/// where it is shown.
pub const fn n_(s: &'static str) -> &'static str {
    s
}

/// Translate a text into the program's language, from the language files in
/// the plugin's `lang` folder (English when there is no translation, or in
/// tests).
pub fn tr(msg: &str) -> String {
    let host = HOST.load(Ordering::Acquire);
    let mut s = msg.to_owned();
    if !host.is_null() {
        unsafe { ((*host).translate)((*host).ctx, Str::new(msg), &StrOut::new(&mut s)) };
    }
    s
}

/// Translate a text with `{name}` placeholders and fill them in.
pub fn trf(msg: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut s = tr(msg);
    for (name, value) in args {
        s = s.replace(&format!("{{{name}}}"), &value.to_string());
    }
    s
}

pub const fn vtable<T: Plugin>() -> VTable {
    VTable {
        free: free::<T>,
        handles: handles::<T>,
        can_write: can_write::<T>,
        status: status::<T>,
        list: list::<T>,
        extract: extract::<T>,
        add: add::<T>,
        delete: delete::<T>,
        rename: rename::<T>,
        mkdir: mkdir::<T>,
        commands: commands::<T>,
        run: run_command::<T>,
        close: close::<T>,
    }
}

pub const fn descriptor<T: Plugin>(vtable: &'static VTable) -> Descriptor {
    Descriptor { version: VERSION, id: Str::new(T::ID), name: Str::new(T::NAME), description: Str::new(T::DESCRIPTION), start: start::<T>, vtable }
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    let what = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
    format!("the plugin failed: {what}")
}

/// Run an operation: its error, or a panic, goes to `error`.
unsafe fn run(error: *const StrOut, f: impl FnOnce() -> io::Result<()>) -> i32 {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => OK,
        Ok(Err(e)) => {
            (*error).send(&e.to_string());
            code_of(e.kind())
        }
        Err(p) => {
            (*error).send(&panic_text(&*p));
            ERR_OTHER
        }
    }
}

unsafe fn me<'a, T>(this: *mut c_void) -> &'a T {
    &*(this as *const T)
}

unsafe fn strings(items: *const Str, n: usize) -> Vec<String> {
    if n == 0 {
        return Vec::new();
    }
    std::slice::from_raw_parts(items, n).iter().map(|s| s.get().to_owned()).collect()
}

unsafe fn with_progress<R>(p: *const ProgressFn, f: impl FnOnce(&mut dyn FnMut(u64, u64) -> bool) -> R) -> R {
    let p = &*p;
    f(&mut |done, total| (p.call)(p.ctx, done, total))
}

unsafe extern "C" fn start<T: Plugin>(host: *const HostApi, dir: Wide, error: *const StrOut) -> *mut c_void {
    if !host.is_null() {
        HOST.store(host as *mut HostApi, Ordering::Release);
    }
    let dir = dir.path();
    match catch_unwind(|| T::start(&dir)) {
        Ok(Ok(p)) => Box::into_raw(Box::new(p)) as *mut c_void,
        Ok(Err(e)) => {
            (*error).send(&e);
            ptr::null_mut()
        }
        Err(p) => {
            (*error).send(&panic_text(&*p));
            ptr::null_mut()
        }
    }
}

unsafe extern "C" fn free<T: Plugin>(this: *mut c_void) {
    let _ = catch_unwind(|| drop(Box::from_raw(this as *mut T)));
}

unsafe extern "C" fn handles<T: Plugin>(this: *mut c_void, name: Str) -> bool {
    catch_unwind(|| me::<T>(this).handles(name.get())).unwrap_or(false)
}

unsafe extern "C" fn can_write<T: Plugin>(this: *mut c_void, file: Wide) -> bool {
    catch_unwind(|| me::<T>(this).can_write(&file.path())).unwrap_or(false)
}

unsafe extern "C" fn status<T: Plugin>(this: *mut c_void, out: *const StrOut) {
    let s = catch_unwind(|| me::<T>(this).status()).unwrap_or_else(|p| panic_text(&*p));
    (*out).send(&s);
}

unsafe extern "C" fn list<T: Plugin>(this: *mut c_void, file: Wide, folder: Str, items: *const ItemOut, complete: *mut bool, error: *const StrOut) -> i32 {
    run(error, || {
        let index = me::<T>(this).list(&file.path(), folder.get())?;
        *complete = index.complete;
        let out = &*items;
        for it in &index.items {
            let raw = RawItem {
                path: Str::new(&it.path),
                dir: it.dir,
                size: it.size,
                modified: to_filetime(it.modified),
                attrs: it.attrs,
                has_index: it.index.is_some(),
                index: it.index.unwrap_or(0),
            };
            (out.put)(out.ctx, &raw);
        }
        Ok(())
    })
}

unsafe extern "C" fn extract<T: Plugin>(this: *mut c_void, file: Wide, items: *const Str, n: usize, out: Wide, progress: *const ProgressFn, error: *const StrOut) -> i32 {
    run(error, || with_progress(progress, |p| me::<T>(this).extract(&file.path(), &strings(items, n), &out.path(), p)))
}

unsafe extern "C" fn add<T: Plugin>(this: *mut c_void, file: Wide, disk: *const Wide, inner: *const Str, n: usize, progress: *const ProgressFn, error: *const StrOut) -> i32 {
    run(error, || {
        let disk: Vec<PathBuf> = if n == 0 { Vec::new() } else { std::slice::from_raw_parts(disk, n).iter().map(|w| w.path()).collect() };
        let pairs: Vec<(PathBuf, String)> = disk.into_iter().zip(strings(inner, n)).collect();
        with_progress(progress, |p| me::<T>(this).add(&file.path(), &pairs, p))
    })
}

unsafe extern "C" fn delete<T: Plugin>(this: *mut c_void, file: Wide, items: *const Str, n: usize, progress: *const ProgressFn, error: *const StrOut) -> i32 {
    run(error, || with_progress(progress, |p| me::<T>(this).delete(&file.path(), &strings(items, n), p)))
}

unsafe extern "C" fn rename<T: Plugin>(this: *mut c_void, file: Wide, old: Str, new: Str, progress: *const ProgressFn, error: *const StrOut) -> i32 {
    run(error, || with_progress(progress, |p| me::<T>(this).rename(&file.path(), old.get(), new.get(), p)))
}

unsafe extern "C" fn mkdir<T: Plugin>(this: *mut c_void, file: Wide, inner: Str, progress: *const ProgressFn, error: *const StrOut) -> i32 {
    run(error, || with_progress(progress, |p| me::<T>(this).mkdir(&file.path(), inner.get(), p)))
}

unsafe extern "C" fn commands<T: Plugin>(this: *mut c_void, out: *const StrOut) {
    let list = catch_unwind(|| me::<T>(this).commands()).unwrap_or_default();
    (*out).send(&crate::ui::encode_commands(&list));
}

unsafe extern "C" fn run_command<T: Plugin>(this: *mut c_void, command: Str, values: Str, out: *const StrOut, error: *const StrOut) -> i32 {
    run(error, || {
        let reply = me::<T>(this).run(command.get(), &crate::ui::decode_values(values.get()))?;
        (*out).send(&crate::ui::encode_reply(&reply));
        Ok(())
    })
}

unsafe extern "C" fn close<T: Plugin>(this: *mut c_void, root: Wide) {
    let _ = catch_unwind(|| me::<T>(this).close(&root.path()));
}
