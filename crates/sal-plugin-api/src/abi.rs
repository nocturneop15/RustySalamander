//! The C interface between the program and a plugin DLL.
//!
//! Plugin authors normally don't touch this module: implement the traits in
//! the crate root and use [`crate::export_plugin!`], which generates the
//! functions below. It is public so the layout is documented and so plugins
//! could be written in another language that can export a C function.
//!
//! # How a plugin is found and called
//!
//! 1. The program loads every `plugins\*.spl` and `plugins\<folder>\*.spl`
//!    and calls the exported function [`ENTRY`] (no arguments), which
//!    returns a pointer to the plugin's static [`Descriptor`].
//! 2. If [`Descriptor::version`] isn't [`VERSION`], the plugin is skipped.
//! 3. When the plugin is turned on, the program calls [`Descriptor::start`]
//!    once, with a [`HostApi`] of its own for this plugin and the plugin's
//!    folder. The returned pointer (the plugin's state, opaque to the
//!    program) is passed as `this` to every function of the [`VTable`] until
//!    [`VTable::free`] is called.
//!
//! # Rules both sides keep
//!
//! - **Strings** cross as UTF-8 ([`Str`]), **paths** as UTF-16 ([`Wide`]),
//!   so every Windows file name survives, including unpaired surrogates.
//!   Both are borrowed for the duration of the call only.
//! - **Memory** is always freed by the side that allocated it. A function
//!   that returns a string or a list calls back into a receiver
//!   ([`StrOut`], [`ItemOut`]) owned by the caller, which copies the data.
//! - **Errors** come back as a code ([`OK`], [`ERR_OTHER`], …) plus a
//!   message sent to the `error` receiver.
//! - **Threads:** VTable functions may be called from several threads at
//!   once (the program runs file operations in the background), so the
//!   plugin's state must be thread-safe.
//! - **Panics** must not cross the boundary: the generated functions catch
//!   them and turn them into errors.

use std::ffi::{c_void, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

/// The interface version. It changes whenever anything in this module
/// changes; the program skips plugins built for another version.
pub const VERSION: u32 = 3;

/// The name of the function every plugin exports:
/// `extern "C" fn rustysalamander_plugin() -> *const Descriptor`.
pub const ENTRY: &str = "rustysalamander_plugin";

/// The extension of plugin files: `*.spl` (as Salamander's plugins).
pub const EXTENSION: &str = "spl";

/// Borrowed UTF-8 text: a pointer and a length in bytes, not terminated.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Str {
    /// The first byte; may be dangling when `len` is 0.
    pub ptr: *const u8,
    /// Length in bytes.
    pub len: usize,
}

impl Str {
    /// Borrows `s`; the result is valid as long as `s` is.
    pub const fn new(s: &str) -> Str {
        Str { ptr: s.as_ptr(), len: s.len() }
    }

    /// The text; invalid UTF-8 reads as U+FFFD.
    ///
    /// # Safety
    /// `ptr` and `len` must describe valid memory for the lifetime used.
    pub unsafe fn get<'a>(self) -> &'a str {
        if self.len == 0 {
            return "";
        }
        let bytes = std::slice::from_raw_parts(self.ptr, self.len);
        std::str::from_utf8(bytes).unwrap_or("\u{FFFD}")
    }
}

/// A borrowed UTF-16 path: a pointer and a length in code units, not
/// terminated.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Wide {
    /// The first code unit; may be dangling when `len` is 0.
    pub ptr: *const u16,
    /// Length in UTF-16 code units.
    pub len: usize,
}

impl Wide {
    /// The UTF-16 form of `p`, to borrow with [`Wide::of`].
    pub fn encode(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().collect()
    }

    /// Borrows `units`; the result is valid as long as `units` is.
    pub fn of(units: &[u16]) -> Wide {
        Wide { ptr: units.as_ptr(), len: units.len() }
    }

    /// The path, losslessly.
    ///
    /// # Safety
    /// `ptr` and `len` must describe valid memory.
    pub unsafe fn path(self) -> PathBuf {
        if self.len == 0 {
            return PathBuf::new();
        }
        PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(self.ptr, self.len)))
    }
}

/// Receives a string from the other side, which `put` copies into memory
/// of the receiver's own. Calling `put` again replaces the earlier string.
#[repr(C)]
pub struct StrOut {
    /// The receiver's data, passed back to `put`.
    pub ctx: *mut c_void,
    /// Takes the string; it is valid only during this call.
    pub put: unsafe extern "C" fn(ctx: *mut c_void, s: Str),
}

unsafe extern "C" fn put_string(ctx: *mut c_void, s: Str) {
    *(ctx as *mut String) = s.get().to_owned();
}

impl StrOut {
    /// A receiver that writes into `s`.
    pub fn new(s: &mut String) -> StrOut {
        StrOut { ctx: s as *mut String as *mut c_void, put: put_string }
    }

    /// Sends `s` to the receiver.
    ///
    /// # Safety
    /// `self` must come from the other side and be valid for this call.
    pub unsafe fn send(&self, s: &str) {
        (self.put)(self.ctx, Str::new(s));
    }
}

/// Reports progress of an operation and asks whether to go on.
#[repr(C)]
pub struct ProgressFn {
    /// The program's data, passed back to `call`.
    pub ctx: *mut c_void,
    /// (bytes done, bytes total); returns false when the user cancelled.
    /// It may block for as long as the user keeps the operation paused.
    pub call: unsafe extern "C" fn(ctx: *mut c_void, done: u64, total: u64) -> bool,
}

/// One item inside a plugin's file, the C form of [`crate::Item`].
#[repr(C)]
pub struct RawItem {
    /// Path inside the file, `\`-separated, without a leading separator.
    pub path: Str,
    /// Whether it is a folder.
    pub dir: bool,
    /// Size in bytes (0 for folders).
    pub size: u64,
    /// FILETIME (100 ns since 1601); 0 when unknown.
    pub modified: u64,
    /// Windows file attributes (`FILE_ATTRIBUTE_*`).
    pub attrs: u32,
    /// Whether `index` is set.
    pub has_index: bool,
    /// The plugin's own number for the item, see [`crate::Item::index`].
    pub index: u32,
}

/// Receives the items of a listing one by one; `put` copies each.
#[repr(C)]
pub struct ItemOut {
    /// The receiver's data, passed back to `put`.
    pub ctx: *mut c_void,
    /// Takes one item; it is valid only during this call.
    pub put: unsafe extern "C" fn(ctx: *mut c_void, item: *const RawItem),
}

/// Success.
pub const OK: i32 = 0;
/// Any other failure; the message says what happened.
pub const ERR_OTHER: i32 = 1;
/// Cancelled through the progress ([`std::io::ErrorKind::Interrupted`]).
pub const ERR_INTERRUPTED: i32 = 2;
/// Something is already there ([`std::io::ErrorKind::AlreadyExists`]).
pub const ERR_EXISTS: i32 = 3;
/// Something isn't there ([`std::io::ErrorKind::NotFound`]).
pub const ERR_NOT_FOUND: i32 = 4;
/// Access denied ([`std::io::ErrorKind::PermissionDenied`]).
pub const ERR_PERMISSION: i32 = 5;
/// Bad input or a damaged file ([`std::io::ErrorKind::InvalidData`]).
pub const ERR_INVALID: i32 = 6;

/// What the program offers a plugin. Each plugin gets its own, valid for
/// the rest of the session.
#[repr(C)]
pub struct HostApi {
    /// [`VERSION`] of the program.
    pub version: u32,
    /// The program's own data for this plugin, passed back with each call.
    pub ctx: *mut c_void,
    /// Translates an English text of the plugin into the program's language,
    /// from the language files in the plugin's `lang` folder; sends the text
    /// unchanged when there is no translation.
    pub translate: unsafe extern "C" fn(ctx: *mut c_void, msg: Str, out: *const StrOut),
}

/// Who a plugin is and how to start it. Static in the DLL: the program
/// reads it without starting the plugin (for Settings → Plugins).
#[repr(C)]
pub struct Descriptor {
    /// [`VERSION`] the plugin was built for.
    pub version: u32,
    /// Unique id, saved in the configuration when the plugin is turned off.
    pub id: Str,
    /// The name shown in Settings, in English (the program translates it).
    pub name: Str,
    /// One or two sentences about what the plugin does, in English.
    pub description: Str,
    /// Starts the plugin; `dir` is its folder. Returns the plugin's state,
    /// or null on failure with the reason sent to `error`.
    pub start: unsafe extern "C" fn(host: *const HostApi, dir: Wide, error: *const StrOut) -> *mut c_void,
    /// The functions of a started plugin; each takes what `start` returned.
    pub vtable: *const VTable,
}

/// The functions of a started plugin; see [`crate::FolderPlugin`] for what
/// each does. Functions returning `i32` return [`OK`] or an error code with
/// a message sent to `error`.
#[repr(C)]
pub struct VTable {
    /// Stops the plugin; `this` is not used again.
    pub free: unsafe extern "C" fn(this: *mut c_void),
    /// [`crate::FolderPlugin::handles`]: `name` is a file name without folder.
    pub handles: unsafe extern "C" fn(this: *mut c_void, name: Str) -> bool,
    /// [`crate::FolderPlugin::can_write`].
    pub can_write: unsafe extern "C" fn(this: *mut c_void, file: Wide) -> bool,
    /// [`crate::FolderPlugin::status`], sent to `out`.
    pub status: unsafe extern "C" fn(this: *mut c_void, out: *const StrOut),
    /// [`crate::FolderPlugin::list`] of `folder`: every item is sent to
    /// `items`, and `complete` is set to [`crate::Index::complete`].
    pub list: unsafe extern "C" fn(this: *mut c_void, file: Wide, folder: Str, items: *const ItemOut, complete: *mut bool, error: *const StrOut) -> i32,
    /// [`crate::FolderPlugin::extract`]: `items` is an array of `n` paths inside.
    pub extract: unsafe extern "C" fn(this: *mut c_void, file: Wide, items: *const Str, n: usize, out: Wide, progress: *const ProgressFn, error: *const StrOut) -> i32,
    /// [`crate::FolderPlugin::add`]: `disk[i]` is added as `inner[i]`, `n` pairs.
    pub add: unsafe extern "C" fn(this: *mut c_void, file: Wide, disk: *const Wide, inner: *const Str, n: usize, progress: *const ProgressFn, error: *const StrOut) -> i32,
    /// [`crate::FolderPlugin::delete`]: `items` is an array of `n` paths inside.
    pub delete: unsafe extern "C" fn(this: *mut c_void, file: Wide, items: *const Str, n: usize, progress: *const ProgressFn, error: *const StrOut) -> i32,
    /// [`crate::FolderPlugin::rename`].
    pub rename: unsafe extern "C" fn(this: *mut c_void, file: Wide, old: Str, new: Str, progress: *const ProgressFn, error: *const StrOut) -> i32,
    /// [`crate::FolderPlugin::mkdir`].
    pub mkdir: unsafe extern "C" fn(this: *mut c_void, file: Wide, inner: Str, progress: *const ProgressFn, error: *const StrOut) -> i32,
    /// [`crate::FolderPlugin::commands`], sent to `out` encoded by
    /// [`crate::ui`] (pairs of id and label).
    pub commands: unsafe extern "C" fn(this: *mut c_void, out: *const StrOut),
    /// [`crate::FolderPlugin::run`]: `values` and the reply sent to `out`
    /// are encoded by [`crate::ui`].
    pub run: unsafe extern "C" fn(this: *mut c_void, command: Str, values: Str, out: *const StrOut, error: *const StrOut) -> i32,
    /// [`crate::FolderPlugin::close`].
    pub close: unsafe extern "C" fn(this: *mut c_void, root: Wide),
}

// Only pointers to static data and functions, or to data the program keeps
// for the whole session.
unsafe impl Sync for Descriptor {}
unsafe impl Sync for VTable {}
unsafe impl Sync for HostApi {}
unsafe impl Send for HostApi {}

/// The code for an error kind.
pub fn code_of(kind: std::io::ErrorKind) -> i32 {
    use std::io::ErrorKind as K;
    match kind {
        K::Interrupted => ERR_INTERRUPTED,
        K::AlreadyExists => ERR_EXISTS,
        K::NotFound => ERR_NOT_FOUND,
        K::PermissionDenied => ERR_PERMISSION,
        K::InvalidInput | K::InvalidData => ERR_INVALID,
        _ => ERR_OTHER,
    }
}

/// The error kind for a code.
pub fn kind_of(code: i32) -> std::io::ErrorKind {
    use std::io::ErrorKind as K;
    match code {
        ERR_INTERRUPTED => K::Interrupted,
        ERR_EXISTS => K::AlreadyExists,
        ERR_NOT_FOUND => K::NotFound,
        ERR_PERMISSION => K::PermissionDenied,
        ERR_INVALID => K::InvalidData,
        _ => K::Other,
    }
}

/// 100 ns intervals between 1601-01-01 (FILETIME) and 1970-01-01 (Unix).
const FILETIME_UNIX_DIFF: u64 = 116_444_736_000_000_000;

/// A time as FILETIME (100 ns since 1601); `None` and times before 1601
/// become 0 or 1, as [`RawItem::modified`] expects.
pub fn to_filetime(t: Option<std::time::SystemTime>) -> u64 {
    match t {
        Some(t) => match t.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => FILETIME_UNIX_DIFF + d.as_nanos() as u64 / 100,
            Err(e) => FILETIME_UNIX_DIFF.saturating_sub(e.duration().as_nanos() as u64 / 100).max(1),
        },
        None => 0,
    }
}

/// A FILETIME as a time; 0 means unknown.
pub fn from_filetime(ft: u64) -> Option<std::time::SystemTime> {
    use std::time::{Duration, UNIX_EPOCH};
    match ft {
        0 => None,
        ft if ft >= FILETIME_UNIX_DIFF => Some(UNIX_EPOCH + Duration::from_nanos((ft - FILETIME_UNIX_DIFF) * 100)),
        ft => Some(UNIX_EPOCH - Duration::from_nanos((FILETIME_UNIX_DIFF - ft) * 100)),
    }
}
