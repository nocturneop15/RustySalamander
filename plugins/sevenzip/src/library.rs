//! 7-Zip's archive library, `7z.dll`, used in-process through its COM-style
//! interfaces (`IInArchive`, `IOutArchive` and the callbacks it calls).
//!
//! The DLL is built from the 7-Zip source in `third_party/7zip-*` (GNU LGPL,
//! the RAR code also under the unRAR restriction) and ships next to the
//! program. Interfaces and IDs follow `CPP/7zip/Archive/IArchive.h`,
//! `IStream.h`, `IProgress.h` and `PropID.h` there.

#![allow(clippy::missing_safety_doc)]

use std::ffi::c_void;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A COM result code (named as in the Windows headers).
#[allow(clippy::upper_case_acronyms)]
pub type HRESULT = i32;
pub const S_OK: HRESULT = 0;
pub const E_ABORT: HRESULT = 0x8000_4004_u32 as i32;
pub const E_NOINTERFACE: HRESULT = 0x8000_4002_u32 as i32;
pub const E_FAIL: HRESULT = 0x8000_4005_u32 as i32;
pub const E_INVALIDARG: HRESULT = 0x8007_0057_u32 as i32;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

const IID_IUNKNOWN: Guid = Guid { data1: 0, data2: 0, data3: 0, data4: [0xC0, 0, 0, 0, 0, 0, 0, 0x46] };

/// `23170F69-40C1-278A-0000-00gg00ss0000`: a 7-Zip interface of group `g`, number `s`.
const fn iid(group: u8, sub: u8) -> Guid {
    Guid { data1: 0x2317_0F69, data2: 0x40C1, data3: 0x278A, data4: [0, 0, 0, group, 0, sub, 0, 0] }
}

pub const IID_IPROGRESS: Guid = iid(0, 0x05);
pub const IID_ISEQUENTIAL_IN_STREAM: Guid = iid(3, 0x01);
pub const IID_ISEQUENTIAL_OUT_STREAM: Guid = iid(3, 0x02);
pub const IID_IIN_STREAM: Guid = iid(3, 0x03);
pub const IID_IOUT_STREAM: Guid = iid(3, 0x04);
pub const IID_IARCHIVE_OPEN_CALLBACK: Guid = iid(6, 0x10);
pub const IID_IARCHIVE_EXTRACT_CALLBACK: Guid = iid(6, 0x20);
pub const IID_IIN_ARCHIVE: Guid = iid(6, 0x60);
pub const IID_IARCHIVE_UPDATE_CALLBACK: Guid = iid(6, 0x80);
pub const IID_IOUT_ARCHIVE: Guid = iid(6, 0xA0);

// property IDs (PropID.h)
pub const KPID_PATH: u32 = 3;
pub const KPID_IS_DIR: u32 = 6;
pub const KPID_SIZE: u32 = 7;
pub const KPID_ATTRIB: u32 = 9;
pub const KPID_CTIME: u32 = 10;
pub const KPID_ATIME: u32 = 11;
pub const KPID_MTIME: u32 = 12;
pub const KPID_IS_ANTI: u32 = 21;

// handler properties (NArchive::NHandlerPropID)
const HANDLER_NAME: u32 = 0;
const HANDLER_CLASS_ID: u32 = 1;
const HANDLER_EXTENSION: u32 = 2;
const HANDLER_UPDATE: u32 = 4;
const HANDLER_SIGNATURE: u32 = 6;
const HANDLER_MULTI_SIGNATURE: u32 = 7;

// NExtract::NAskMode and NOperationResult
const ASK_EXTRACT: i32 = 0;
pub const OP_OK: i32 = 0;
const OP_WRONG_PASSWORD: i32 = 9;

// VARTYPE
const VT_EMPTY: u16 = 0;
const VT_I4: u16 = 3;
const VT_BSTR: u16 = 8;
const VT_BOOL: u16 = 11;
const VT_UI1: u16 = 17;
const VT_UI2: u16 = 18;
const VT_UI4: u16 = 19;
const VT_I8: u16 = 20;
const VT_UI8: u16 = 21;
const VT_FILETIME: u16 = 64;

/// PROPVARIANT (24 bytes on 64-bit Windows).
#[repr(C)]
pub struct PropVariant {
    vt: u16,
    r1: u16,
    r2: u16,
    r3: u16,
    data: [u64; 2],
}

impl Default for PropVariant {
    fn default() -> Self {
        PropVariant { vt: VT_EMPTY, r1: 0, r2: 0, r3: 0, data: [0; 2] }
    }
}

#[cfg(windows)]
mod ole {
    #[link(name = "oleaut32")]
    extern "system" {
        pub fn SysAllocStringLen(s: *const u16, len: u32) -> *mut u16;
        pub fn SysFreeString(s: *mut u16);
        pub fn SysStringLen(s: *const u16) -> u32;
        pub fn SysStringByteLen(s: *const u16) -> u32;
    }
}

impl PropVariant {
    fn bstr_ptr(&self) -> *mut u16 {
        self.data[0] as usize as *mut u16
    }

    pub fn as_string(&self) -> Option<String> {
        if self.vt != VT_BSTR || self.bstr_ptr().is_null() {
            return None;
        }
        unsafe {
            let n = ole::SysStringLen(self.bstr_ptr()) as usize;
            Some(String::from_utf16_lossy(std::slice::from_raw_parts(self.bstr_ptr(), n)))
        }
    }

    /// The raw bytes of a binary value stored in a BSTR (class IDs).
    fn as_bytes(&self) -> Option<Vec<u8>> {
        if self.vt != VT_BSTR || self.bstr_ptr().is_null() {
            return None;
        }
        unsafe {
            let n = ole::SysStringByteLen(self.bstr_ptr()) as usize;
            Some(std::slice::from_raw_parts(self.bstr_ptr() as *const u8, n).to_vec())
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self.vt {
            VT_UI1 => Some(self.data[0] & 0xFF),
            VT_UI2 => Some(self.data[0] & 0xFFFF),
            VT_UI4 | VT_I4 => Some(self.data[0] & 0xFFFF_FFFF),
            VT_UI8 | VT_I8 => Some(self.data[0]),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        (self.vt == VT_BOOL).then_some(self.data[0] & 0xFFFF != 0)
    }

    pub fn as_time(&self) -> Option<SystemTime> {
        (self.vt == VT_FILETIME).then(|| filetime_to_system(self.data[0]))
    }

    fn set_string(&mut self, s: &str) {
        let w: Vec<u16> = s.encode_utf16().collect();
        self.vt = VT_BSTR;
        self.data[0] = unsafe { ole::SysAllocStringLen(w.as_ptr(), w.len() as u32) } as usize as u64;
    }

    fn set_bool(&mut self, b: bool) {
        self.vt = VT_BOOL;
        self.data[0] = if b { 0xFFFF } else { 0 };
    }

    fn set_u32(&mut self, v: u32) {
        self.vt = VT_UI4;
        self.data[0] = v as u64;
    }

    fn set_u64(&mut self, v: u64) {
        self.vt = VT_UI8;
        self.data[0] = v;
    }

    fn set_time(&mut self, t: SystemTime) {
        self.vt = VT_FILETIME;
        self.data[0] = system_to_filetime(t);
    }

    fn clear(&mut self) {
        if self.vt == VT_BSTR && !self.bstr_ptr().is_null() {
            unsafe { ole::SysFreeString(self.bstr_ptr()) };
        }
        // overwrite without dropping the old value (its string is freed above)
        unsafe { ptr::write(self, PropVariant::default()) };
    }
}

impl Drop for PropVariant {
    fn drop(&mut self) {
        self.clear();
    }
}

const FILETIME_UNIX_DIFF: u64 = 116_444_736_000_000_000;

fn filetime_to_system(ft: u64) -> SystemTime {
    if ft >= FILETIME_UNIX_DIFF {
        UNIX_EPOCH + Duration::from_nanos((ft - FILETIME_UNIX_DIFF) * 100)
    } else {
        UNIX_EPOCH - Duration::from_nanos((FILETIME_UNIX_DIFF - ft) * 100)
    }
}

fn system_to_filetime(t: SystemTime) -> u64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => FILETIME_UNIX_DIFF + (d.as_nanos() / 100) as u64,
        Err(e) => FILETIME_UNIX_DIFF.saturating_sub((e.duration().as_nanos() / 100) as u64),
    }
}

// ------------------------------------------------------------ objects of 7z.dll

type QueryInterface = unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT;
type AddRef = unsafe extern "system" fn(*mut c_void) -> u32;
type Release = unsafe extern "system" fn(*mut c_void) -> u32;

#[repr(C)]
struct IUnknownVtbl {
    query_interface: QueryInterface,
    add_ref: AddRef,
    release: Release,
}

#[repr(C)]
struct IInArchiveVtbl {
    base: IUnknownVtbl,
    open: unsafe extern "system" fn(*mut c_void, *mut c_void, *const u64, *mut c_void) -> HRESULT,
    close: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    get_number_of_items: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
    get_property: unsafe extern "system" fn(*mut c_void, u32, u32, *mut PropVariant) -> HRESULT,
    extract: unsafe extern "system" fn(*mut c_void, *const u32, u32, i32, *mut c_void) -> HRESULT,
    // the rest of IInArchive is not used
}

#[repr(C)]
struct IOutArchiveVtbl {
    base: IUnknownVtbl,
    update_items: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, *mut c_void) -> HRESULT,
    get_file_time_type: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
}

/// A reference to an object of 7z.dll; released when dropped.
struct ComPtr<V> {
    p: *mut *const V,
}

impl<V> ComPtr<V> {
    fn vtbl(&self) -> &V {
        unsafe { &**self.p }
    }

    fn raw(&self) -> *mut c_void {
        self.p as *mut c_void
    }

    fn unknown(&self) -> &IUnknownVtbl {
        unsafe { &*(*self.p as *const IUnknownVtbl) }
    }

    fn query<W>(&self, iid: &Guid) -> Option<ComPtr<W>> {
        let mut out = ptr::null_mut();
        let hr = unsafe { (self.unknown().query_interface)(self.raw(), iid, &mut out) };
        (hr == S_OK && !out.is_null()).then_some(ComPtr { p: out as *mut *const W })
    }
}

impl<V> Drop for ComPtr<V> {
    fn drop(&mut self) {
        unsafe { (self.unknown().release)(self.raw()) };
    }
}

// ------------------------------------------------------------ our objects

/// A COM object implemented here: vtable pointer first, then a reference
/// count, then the data. All our interfaces are single-inheritance chains,
/// so one pointer serves the interface and its bases.
#[repr(C)]
struct Obj<V: 'static, T> {
    vtbl: &'static V,
    refs: AtomicU32,
    iids: &'static [Guid],
    data: T,
}

impl<V: 'static, T> Obj<V, T> {
    fn create(vtbl: &'static V, iids: &'static [Guid], data: T) -> *mut c_void {
        Box::into_raw(Box::new(Obj { vtbl, refs: AtomicU32::new(1), iids, data })) as *mut c_void
    }

    unsafe fn data<'a>(this: *mut c_void) -> &'a mut T {
        &mut (*(this as *mut Obj<V, T>)).data
    }

    unsafe extern "system" fn query_interface(this: *mut c_void, iid: *const Guid, out: *mut *mut c_void) -> HRESULT {
        let o = &*(this as *mut Obj<V, T>);
        if *iid == IID_IUNKNOWN || o.iids.contains(&*iid) {
            o.refs.fetch_add(1, Ordering::AcqRel);
            *out = this;
            S_OK
        } else {
            *out = ptr::null_mut();
            E_NOINTERFACE
        }
    }

    unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
        (*(this as *mut Obj<V, T>)).refs.fetch_add(1, Ordering::AcqRel) + 1
    }

    unsafe extern "system" fn release(this: *mut c_void) -> u32 {
        let n = (*(this as *mut Obj<V, T>)).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if n == 0 {
            drop(Box::from_raw(this as *mut Obj<V, T>));
        }
        n
    }

    const UNKNOWN: IUnknownVtbl = IUnknownVtbl { query_interface: Self::query_interface, add_ref: Self::add_ref, release: Self::release };
}

/// Our reference to one of our objects, for passing it to 7z.dll.
struct Own(*mut c_void);

impl Own {
    fn raw(&self) -> *mut c_void {
        self.0
    }
}

impl Drop for Own {
    fn drop(&mut self) {
        unsafe {
            let vt = &**(self.0 as *mut *const IUnknownVtbl);
            (vt.release)(self.0);
        }
    }
}

// --- IInStream over a file

#[repr(C)]
struct InStreamVtbl {
    base: IUnknownVtbl,
    read: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, *mut u32) -> HRESULT,
    seek: unsafe extern "system" fn(*mut c_void, i64, u32, *mut u64) -> HRESULT,
}

type InStreamObj = Obj<InStreamVtbl, File>;

unsafe extern "system" fn in_read(this: *mut c_void, data: *mut c_void, size: u32, processed: *mut u32) -> HRESULT {
    let f = InStreamObj::data(this);
    let buf = std::slice::from_raw_parts_mut(data as *mut u8, size as usize);
    let n = loop {
        match f.read(buf) {
            Ok(n) => break n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return E_FAIL,
        }
    };
    if !processed.is_null() {
        *processed = n as u32;
    }
    S_OK
}

fn seek_from(offset: i64, origin: u32) -> Option<SeekFrom> {
    match origin {
        0 => Some(SeekFrom::Start(offset as u64)),
        1 => Some(SeekFrom::Current(offset)),
        2 => Some(SeekFrom::End(offset)),
        _ => None,
    }
}

unsafe extern "system" fn in_seek(this: *mut c_void, offset: i64, origin: u32, new_pos: *mut u64) -> HRESULT {
    let f = InStreamObj::data(this);
    let Some(from) = seek_from(offset, origin) else { return E_INVALIDARG };
    match f.seek(from) {
        Ok(p) => {
            if !new_pos.is_null() {
                *new_pos = p;
            }
            S_OK
        }
        Err(_) => E_FAIL,
    }
}

static IN_STREAM_VTBL: InStreamVtbl = InStreamVtbl { base: InStreamObj::UNKNOWN, read: in_read, seek: in_seek };
static IN_STREAM_IIDS: [Guid; 2] = [IID_ISEQUENTIAL_IN_STREAM, IID_IIN_STREAM];

fn in_stream(f: File) -> Own {
    Own(InStreamObj::create(&IN_STREAM_VTBL, &IN_STREAM_IIDS, f))
}

// --- IOutStream over a file

#[repr(C)]
struct OutStreamVtbl {
    base: IUnknownVtbl,
    write: unsafe extern "system" fn(*mut c_void, *const c_void, u32, *mut u32) -> HRESULT,
    seek: unsafe extern "system" fn(*mut c_void, i64, u32, *mut u64) -> HRESULT,
    set_size: unsafe extern "system" fn(*mut c_void, u64) -> HRESULT,
}

type OutStreamObj = Obj<OutStreamVtbl, File>;

unsafe extern "system" fn out_write(this: *mut c_void, data: *const c_void, size: u32, processed: *mut u32) -> HRESULT {
    let f = OutStreamObj::data(this);
    let buf = std::slice::from_raw_parts(data as *const u8, size as usize);
    if f.write_all(buf).is_err() {
        return E_FAIL;
    }
    if !processed.is_null() {
        *processed = size;
    }
    S_OK
}

unsafe extern "system" fn out_seek(this: *mut c_void, offset: i64, origin: u32, new_pos: *mut u64) -> HRESULT {
    let f = OutStreamObj::data(this);
    let Some(from) = seek_from(offset, origin) else { return E_INVALIDARG };
    match f.seek(from) {
        Ok(p) => {
            if !new_pos.is_null() {
                *new_pos = p;
            }
            S_OK
        }
        Err(_) => E_FAIL,
    }
}

unsafe extern "system" fn out_set_size(this: *mut c_void, size: u64) -> HRESULT {
    if OutStreamObj::data(this).set_len(size).is_ok() {
        S_OK
    } else {
        E_FAIL
    }
}

static OUT_STREAM_VTBL: OutStreamVtbl = OutStreamVtbl { base: OutStreamObj::UNKNOWN, write: out_write, seek: out_seek, set_size: out_set_size };
static OUT_STREAM_IIDS: [Guid; 2] = [IID_ISEQUENTIAL_OUT_STREAM, IID_IOUT_STREAM];

fn out_stream(f: File) -> Own {
    Own(OutStreamObj::create(&OUT_STREAM_VTBL, &OUT_STREAM_IIDS, f))
}

// --- IArchiveOpenCallback (no password support: encrypted headers fail to open)

#[repr(C)]
struct OpenCallbackVtbl {
    base: IUnknownVtbl,
    set_total: unsafe extern "system" fn(*mut c_void, *const u64, *const u64) -> HRESULT,
    set_completed: unsafe extern "system" fn(*mut c_void, *const u64, *const u64) -> HRESULT,
}

type OpenCallbackObj = Obj<OpenCallbackVtbl, ()>;

unsafe extern "system" fn open_progress(_: *mut c_void, _: *const u64, _: *const u64) -> HRESULT {
    S_OK
}

static OPEN_CALLBACK_VTBL: OpenCallbackVtbl = OpenCallbackVtbl { base: OpenCallbackObj::UNKNOWN, set_total: open_progress, set_completed: open_progress };
static OPEN_CALLBACK_IIDS: [Guid; 1] = [IID_IARCHIVE_OPEN_CALLBACK];

// --- progress shared by extraction and updates

/// Reports (done, total) bytes; returning false cancels. It may block (pause).
pub type ProgressFn<'a> = dyn FnMut(u64, u64) -> bool + 'a;

struct Progress<'a> {
    total: u64,
    report: &'a mut ProgressFn<'a>,
    cancelled: bool,
}

impl Progress<'_> {
    fn completed(&mut self, done: u64) -> HRESULT {
        if (self.report)(done, self.total) {
            S_OK
        } else {
            self.cancelled = true;
            E_ABORT
        }
    }
}

// --- IArchiveExtractCallback

#[repr(C)]
struct ExtractCallbackVtbl {
    base: IUnknownVtbl,
    set_total: unsafe extern "system" fn(*mut c_void, u64) -> HRESULT,
    set_completed: unsafe extern "system" fn(*mut c_void, *const u64) -> HRESULT,
    get_stream: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void, i32) -> HRESULT,
    prepare_operation: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT,
    set_operation_result: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT,
}

/// Where each extracted item goes, and what happened.
struct Extracting<'a> {
    progress: Progress<'a>,
    /// Destination for an item index; `None` skips it.
    target: &'a dyn Fn(u32) -> Option<(PathBuf, bool)>,
    /// Item modification times, applied after writing.
    times: &'a dyn Fn(u32) -> Option<SystemTime>,
    current: Option<(PathBuf, u32)>,
    errors: Vec<(PathBuf, i32)>,
    io_error: Option<io::Error>,
}

type ExtractObj<'a> = Obj<ExtractCallbackVtbl, Extracting<'a>>;

unsafe extern "system" fn ex_set_total(this: *mut c_void, total: u64) -> HRESULT {
    ExtractObj::data(this).progress.total = total;
    S_OK
}

unsafe extern "system" fn ex_set_completed(this: *mut c_void, done: *const u64) -> HRESULT {
    let d = ExtractObj::data(this);
    let v = if done.is_null() { 0 } else { *done };
    d.progress.completed(v)
}

unsafe extern "system" fn ex_get_stream(this: *mut c_void, index: u32, out: *mut *mut c_void, mode: i32) -> HRESULT {
    let d = ExtractObj::data(this);
    *out = ptr::null_mut();
    d.current = None;
    if mode != ASK_EXTRACT {
        return S_OK;
    }
    let Some((path, is_dir)) = (d.target)(index) else { return S_OK };
    let r = if is_dir { std::fs::create_dir_all(&path).map(|_| None) } else { path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| File::create(&path)).map(Some) };
    match r {
        Ok(Some(f)) => {
            d.current = Some((path, index));
            *out = out_stream(f).into_raw();
            S_OK
        }
        Ok(None) => {
            d.current = Some((path, index));
            S_OK
        }
        Err(e) => {
            d.io_error = Some(io::Error::new(e.kind(), format!("{}: {e}", path.display())));
            E_ABORT
        }
    }
}

unsafe extern "system" fn ex_prepare(_: *mut c_void, _: i32) -> HRESULT {
    S_OK
}

unsafe extern "system" fn ex_set_result(this: *mut c_void, result: i32) -> HRESULT {
    let d = ExtractObj::data(this);
    if let Some((path, index)) = d.current.take() {
        if result != OP_OK {
            d.errors.push((path, result));
        } else if let Some(t) = (d.times)(index) {
            // the stream is released by 7z.dll before this: the file is closed
            let _ = File::options().write(true).open(&path).and_then(|f| f.set_modified(t));
        }
    }
    S_OK
}

impl Own {
    fn into_raw(self) -> *mut c_void {
        let p = self.0;
        std::mem::forget(self);
        p
    }
}

// --- IArchiveUpdateCallback

#[repr(C)]
struct UpdateCallbackVtbl {
    base: IUnknownVtbl,
    set_total: unsafe extern "system" fn(*mut c_void, u64) -> HRESULT,
    set_completed: unsafe extern "system" fn(*mut c_void, *const u64) -> HRESULT,
    get_update_item_info: unsafe extern "system" fn(*mut c_void, u32, *mut i32, *mut i32, *mut u32) -> HRESULT,
    get_property: unsafe extern "system" fn(*mut c_void, u32, u32, *mut PropVariant) -> HRESULT,
    get_stream: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT,
    set_operation_result: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT,
}

/// One item of the archive being written.
#[derive(Debug, Clone)]
pub enum UpdateItem {
    /// An existing item, unchanged.
    Keep(u32),
    /// An existing item under a new path.
    Rename(u32, String),
    /// A new item: its path in the archive and, for a file, where its data is.
    New { path: String, source: Option<PathBuf>, dir: bool, size: u64, modified: Option<SystemTime>, attrib: u32 },
}

struct Updating<'a> {
    progress: Progress<'a>,
    items: &'a [UpdateItem],
    io_error: Option<io::Error>,
}

type UpdateObj<'a> = Obj<UpdateCallbackVtbl, Updating<'a>>;

unsafe extern "system" fn up_set_total(this: *mut c_void, total: u64) -> HRESULT {
    UpdateObj::data(this).progress.total = total;
    S_OK
}

unsafe extern "system" fn up_set_completed(this: *mut c_void, done: *const u64) -> HRESULT {
    let d = UpdateObj::data(this);
    let v = if done.is_null() { 0 } else { *done };
    d.progress.completed(v)
}

unsafe extern "system" fn up_item_info(this: *mut c_void, index: u32, new_data: *mut i32, new_props: *mut i32, index_in_archive: *mut u32) -> HRESULT {
    let d = UpdateObj::data(this);
    let Some(it) = d.items.get(index as usize) else { return E_INVALIDARG };
    let (data, props, old) = match it {
        UpdateItem::Keep(i) => (0, 0, *i),
        UpdateItem::Rename(i, _) => (0, 1, *i),
        UpdateItem::New { .. } => (1, 1, u32::MAX),
    };
    if !new_data.is_null() {
        *new_data = data;
    }
    if !new_props.is_null() {
        *new_props = props;
    }
    if !index_in_archive.is_null() {
        *index_in_archive = old;
    }
    S_OK
}

unsafe extern "system" fn up_property(this: *mut c_void, index: u32, prop: u32, value: *mut PropVariant) -> HRESULT {
    let d = UpdateObj::data(this);
    let v = &mut *value;
    std::ptr::write(v, PropVariant::default());
    let Some(it) = d.items.get(index as usize) else { return E_INVALIDARG };
    match (it, prop) {
        (_, KPID_IS_ANTI) => v.set_bool(false),
        (UpdateItem::Rename(_, path), KPID_PATH) => v.set_string(path),
        (UpdateItem::New { path, .. }, KPID_PATH) => v.set_string(path),
        (UpdateItem::New { dir, .. }, KPID_IS_DIR) => v.set_bool(*dir),
        (UpdateItem::New { size, .. }, KPID_SIZE) => v.set_u64(*size),
        (UpdateItem::New { attrib, .. }, KPID_ATTRIB) => v.set_u32(*attrib),
        (UpdateItem::New { modified: Some(t), .. }, KPID_MTIME | KPID_CTIME | KPID_ATIME) => v.set_time(*t),
        _ => {}
    }
    S_OK
}

unsafe extern "system" fn up_get_stream(this: *mut c_void, index: u32, out: *mut *mut c_void) -> HRESULT {
    let d = UpdateObj::data(this);
    *out = ptr::null_mut();
    if let Some(UpdateItem::New { source: Some(p), .. }) = d.items.get(index as usize) {
        match File::open(p) {
            Ok(f) => *out = in_stream(f).into_raw(),
            Err(e) => {
                d.io_error = Some(io::Error::new(e.kind(), format!("{}: {e}", p.display())));
                return E_ABORT;
            }
        }
    }
    S_OK
}

unsafe extern "system" fn up_set_result(_: *mut c_void, _: i32) -> HRESULT {
    S_OK
}

// ------------------------------------------------------------ the library

type CreateObjectFn = unsafe extern "system" fn(*const Guid, *const Guid, *mut *mut c_void) -> HRESULT;
type GetNumberOfFormatsFn = unsafe extern "system" fn(*mut u32) -> HRESULT;
type GetHandlerProperty2Fn = unsafe extern "system" fn(u32, u32, *mut PropVariant) -> HRESULT;

/// An archive format of the library.
#[derive(Debug, Clone)]
pub struct Format {
    pub name: String,
    class_id: Guid,
    /// Lower-case extensions, e.g. ["zip", "jar", …].
    pub extensions: Vec<String>,
    pub can_update: bool,
    /// The format recognizes its files by a signature (formats without one
    /// would take any file).
    has_signature: bool,
}

/// The loaded `7z.dll`.
pub struct Library {
    _module: isize,
    create_object: CreateObjectFn,
    pub formats: Vec<Format>,
    pub path: PathBuf,
}

// the DLL's entry points and format list are usable from any thread
unsafe impl Send for Library {}
unsafe impl Sync for Library {}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> isize;
    fn GetProcAddress(module: isize, name: *const u8) -> *mut c_void;
    fn SetThreadErrorMode(new: u32, old: *mut u32) -> i32;
}

/// SEM_FAILCRITICALERRORS: a damaged library fails to load with an error
/// instead of a Windows "Bad Image" box that blocks until it is dismissed.
const SEM_FAILCRITICALERRORS: u32 = 1;

fn error(msg: impl Into<String>) -> io::Error {
    io::Error::other(msg.into())
}

/// Item path for an operation result (NOperationResult).
pub fn result_text(code: i32) -> &'static str {
    match code {
        1 => "unsupported compression method",
        2 => "data error",
        3 => "CRC error",
        4 => "unavailable data",
        5 => "unexpected end of data",
        6 => "data after the end",
        7 => "not an archive",
        8 => "headers error",
        OP_WRONG_PASSWORD => "encrypted (passwords are not supported)",
        _ => "error",
    }
}

impl Library {
    pub fn load(path: &Path) -> io::Result<Library> {
        let wide: Vec<u16> = path.as_os_str().encode_wide_z();
        let module = unsafe {
            let mut old = 0;
            SetThreadErrorMode(SEM_FAILCRITICALERRORS, &mut old);
            let m = LoadLibraryW(wide.as_ptr());
            let err = io::Error::last_os_error();
            SetThreadErrorMode(old, ptr::null_mut());
            if m == 0 {
                return Err(err);
            }
            m
        };
        let sym = |name: &[u8]| unsafe { GetProcAddress(module, name.as_ptr()) };
        let (c, n, h) = (sym(b"CreateObject\0"), sym(b"GetNumberOfFormats\0"), sym(b"GetHandlerProperty2\0"));
        if c.is_null() || n.is_null() || h.is_null() {
            return Err(error(format!("{} is not 7-Zip's archive library", path.display())));
        }
        let create_object: CreateObjectFn = unsafe { std::mem::transmute(c) };
        let count: GetNumberOfFormatsFn = unsafe { std::mem::transmute(n) };
        let prop: GetHandlerProperty2Fn = unsafe { std::mem::transmute(h) };
        let mut num = 0u32;
        unsafe { count(&mut num) };
        let mut formats = Vec::new();
        for i in 0..num {
            let get = |id: u32| {
                let mut v = PropVariant::default();
                unsafe { prop(i, id, &mut v) };
                v
            };
            let name = get(HANDLER_NAME).as_string().unwrap_or_default();
            let Some(cls) = get(HANDLER_CLASS_ID).as_bytes().filter(|b| b.len() == 16) else { continue };
            let class_id = Guid {
                data1: u32::from_le_bytes(cls[0..4].try_into().unwrap()),
                data2: u16::from_le_bytes(cls[4..6].try_into().unwrap()),
                data3: u16::from_le_bytes(cls[6..8].try_into().unwrap()),
                data4: cls[8..16].try_into().unwrap(),
            };
            let extensions = get(HANDLER_EXTENSION).as_string().unwrap_or_default().split_whitespace().map(str::to_lowercase).collect();
            let can_update = get(HANDLER_UPDATE).as_bool().unwrap_or(false);
            let has_signature = [HANDLER_SIGNATURE, HANDLER_MULTI_SIGNATURE].iter().any(|&id| get(id).as_bytes().is_some_and(|b| !b.is_empty()));
            formats.push(Format { name, class_id, extensions, can_update, has_signature });
        }
        Ok(Library { _module: module, create_object, formats, path: path.to_path_buf() })
    }

    fn create<V>(&self, format: usize, iid: &Guid) -> Option<ComPtr<V>> {
        let mut out = ptr::null_mut();
        let hr = unsafe { (self.create_object)(&self.formats[format].class_id, iid, &mut out) };
        (hr == S_OK && !out.is_null()).then_some(ComPtr { p: out as *mut *const V })
    }

    /// Formats to try for a file: those claiming its extension first, then
    /// the others that recognize their files by a signature (a .zip that is
    /// really a 7z archive still opens).
    fn candidates(&self, path: &Path) -> Vec<usize> {
        let ext = path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default();
        let (mut first, rest): (Vec<usize>, Vec<usize>) = (0..self.formats.len()).partition(|&i| self.formats[i].extensions.contains(&ext));
        first.extend(rest.into_iter().filter(|&i| self.formats[i].has_signature));
        first
    }

    /// Open an archive file.
    pub fn open(&self, path: &Path) -> io::Result<OpenArchive> {
        let file = File::open(path)?;
        let stream = in_stream(file);
        for format in self.candidates(path) {
            let Some(arc) = self.create::<IInArchiveVtbl>(format, &IID_IIN_ARCHIVE) else { continue };
            let cb = Own(OpenCallbackObj::create(&OPEN_CALLBACK_VTBL, &OPEN_CALLBACK_IIDS, ()));
            let max_check: u64 = 1 << 22;
            // rewind: a failed attempt may have moved the stream
            unsafe { in_seek(stream.raw(), 0, 0, ptr::null_mut()) };
            let hr = unsafe { (arc.vtbl().open)(arc.raw(), stream.raw(), &max_check, cb.raw()) };
            if hr == S_OK {
                return Ok(OpenArchive { arc, format, _stream: stream, path: path.to_path_buf() });
            }
        }
        Err(error(format!("{}: not an archive 7-Zip can open (or encrypted)", path.display())))
    }

    /// Write `items` as a new archive of `format` into `out`: kept and
    /// renamed items come from `base` (the archive open before).
    pub fn write(&self, base: Option<&OpenArchive>, format: usize, out: &Path, items: &[UpdateItem], progress: &mut ProgressFn) -> io::Result<()> {
        let out_arc: ComPtr<IOutArchiveVtbl> = match base {
            Some(b) => b.arc.query(&IID_IOUT_ARCHIVE),
            None => self.create(format, &IID_IOUT_ARCHIVE),
        }
        .ok_or_else(|| error(format!("7-Zip can't write {} archives", self.formats[format].name)))?;
        let file = File::create(out)?;
        let stream = out_stream(file);
        // 'static lifetimes for the vtable; the borrowed data lives through the call
        let progress: &mut ProgressFn<'static> = unsafe { std::mem::transmute(progress) };
        let items: &'static [UpdateItem] = unsafe { std::mem::transmute(items) };
        let data = Updating { progress: Progress { total: 0, report: progress, cancelled: false }, items, io_error: None };
        let cb = UpdateObj::create(&UPDATE_CALLBACK_VTBL, &UPDATE_CALLBACK_IIDS, data);
        let hr = unsafe { (out_arc.vtbl().update_items)(out_arc.raw(), stream.raw(), items.len() as u32, cb) };
        let (cancelled, io_error) = unsafe {
            let d = UpdateObj::data(cb);
            (d.progress.cancelled, d.io_error.take())
        };
        unsafe { UpdateObj::release(cb) };
        drop(stream);
        if hr == S_OK {
            return Ok(());
        }
        let _ = std::fs::remove_file(out);
        if cancelled {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        Err(io_error.unwrap_or_else(|| error(format!("7-Zip could not write the archive (0x{hr:08X})"))))
    }
}

static UPDATE_CALLBACK_VTBL: UpdateCallbackVtbl = UpdateCallbackVtbl {
    base: UpdateObj::UNKNOWN,
    set_total: up_set_total,
    set_completed: up_set_completed,
    get_update_item_info: up_item_info,
    get_property: up_property,
    get_stream: up_get_stream,
    set_operation_result: up_set_result,
};
static UPDATE_CALLBACK_IIDS: [Guid; 2] = [IID_IPROGRESS, IID_IARCHIVE_UPDATE_CALLBACK];

static EXTRACT_CALLBACK_VTBL: ExtractCallbackVtbl = ExtractCallbackVtbl {
    base: ExtractObj::UNKNOWN,
    set_total: ex_set_total,
    set_completed: ex_set_completed,
    get_stream: ex_get_stream,
    prepare_operation: ex_prepare,
    set_operation_result: ex_set_result,
};
static EXTRACT_CALLBACK_IIDS: [Guid; 2] = [IID_IPROGRESS, IID_IARCHIVE_EXTRACT_CALLBACK];

/// One item as stored in an archive.
#[derive(Debug, Clone, PartialEq)]
pub struct RawItem {
    pub index: u32,
    pub path: String,
    pub dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub attrib: u32,
}

/// An archive open for reading (and as the base of an update).
pub struct OpenArchive {
    arc: ComPtr<IInArchiveVtbl>,
    pub format: usize,
    _stream: Own,
    pub path: PathBuf,
}

impl OpenArchive {
    fn prop(&self, index: u32, id: u32) -> PropVariant {
        let mut v = PropVariant::default();
        unsafe { (self.arc.vtbl().get_property)(self.arc.raw(), index, id, &mut v) };
        v
    }

    /// Everything stored in the archive.
    pub fn items(&self) -> Vec<RawItem> {
        let mut n = 0u32;
        unsafe { (self.arc.vtbl().get_number_of_items)(self.arc.raw(), &mut n) };
        // single-file formats (gz, xz) store no name: the archive's name without its extension
        let stem = self.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        (0..n)
            .map(|i| {
                let path = self.prop(i, KPID_PATH).as_string().unwrap_or_default().replace('/', "\\");
                let path = path.trim_matches('\\').to_owned();
                RawItem {
                    index: i,
                    path: if path.is_empty() { stem.clone() } else { path },
                    dir: self.prop(i, KPID_IS_DIR).as_bool().unwrap_or(false),
                    size: self.prop(i, KPID_SIZE).as_u64().unwrap_or(0),
                    modified: self.prop(i, KPID_MTIME).as_time(),
                    attrib: self.prop(i, KPID_ATTRIB).as_u64().unwrap_or(0) as u32,
                }
            })
            .collect()
    }

    /// Extract the items `indices`; `target` says where each goes (path,
    /// is it a folder). Items that fail are listed in the error.
    pub fn extract(
        &self,
        indices: &[u32],
        target: &dyn Fn(u32) -> Option<(PathBuf, bool)>,
        times: &dyn Fn(u32) -> Option<SystemTime>,
        progress: &mut ProgressFn,
    ) -> io::Result<()> {
        let mut sorted = indices.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        let progress: &mut ProgressFn<'static> = unsafe { std::mem::transmute(progress) };
        let target: &'static dyn Fn(u32) -> Option<(PathBuf, bool)> = unsafe { std::mem::transmute(target) };
        let times: &'static dyn Fn(u32) -> Option<SystemTime> = unsafe { std::mem::transmute(times) };
        let data = Extracting { progress: Progress { total: 0, report: progress, cancelled: false }, target, times, current: None, errors: Vec::new(), io_error: None };
        let cb = ExtractObj::create(&EXTRACT_CALLBACK_VTBL, &EXTRACT_CALLBACK_IIDS, data);
        let hr = unsafe { (self.arc.vtbl().extract)(self.arc.raw(), sorted.as_ptr(), sorted.len() as u32, 0, cb) };
        let (cancelled, io_error, errors) = unsafe {
            let d = ExtractObj::data(cb);
            (d.progress.cancelled, d.io_error.take(), std::mem::take(&mut d.errors))
        };
        unsafe { ExtractObj::release(cb) };
        if cancelled {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        if let Some(e) = io_error {
            return Err(e);
        }
        if let Some((p, code)) = errors.first() {
            let more = if errors.len() > 1 { format!(" (+{} more)", errors.len() - 1) } else { String::new() };
            return Err(error(format!("{}: {}{more}", p.display(), result_text(*code))));
        }
        if hr != S_OK {
            return Err(error(format!("7-Zip could not extract (0x{hr:08X})")));
        }
        Ok(())
    }
}

impl Drop for OpenArchive {
    fn drop(&mut self) {
        unsafe { (self.arc.vtbl().close)(self.arc.raw()) };
    }
}

trait EncodeWideZ {
    fn encode_wide_z(&self) -> Vec<u16>;
}

impl EncodeWideZ for std::ffi::OsStr {
    fn encode_wide_z(&self) -> Vec<u16> {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            self.encode_wide().chain(Some(0)).collect()
        }
        #[cfg(not(windows))]
        {
            self.to_string_lossy().encode_utf16().chain(Some(0)).collect()
        }
    }
}
