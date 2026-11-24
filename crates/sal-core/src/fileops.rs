//! File operations: copy, move, delete, mkdir, rename (replaces `worker.cpp`).
//!
//! Operations run synchronously and report to an [`OpObserver`], which the GUI
//! implements on top of a worker thread + channels. Paths go through std's
//! wide-char APIs, so Unicode names and paths beyond `MAX_PATH` just work.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::masks;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Copy,
    Move,
    Delete,
}

/// How a copy (or a move that copies) checks each file it wrote: safe copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Verify {
    /// Not checked.
    #[default]
    None,
    /// The copy has as many bytes as the original.
    Size,
    /// The copy is read back and its SHA-256 compared with the original's;
    /// the checksums are written to a `.sha256` file on both sides.
    Checksum,
}

/// How [`transfer_with`] copies and moves.
#[derive(Debug, Clone, Default)]
pub struct TransferOptions {
    pub verify: Verify,
    /// The name of the checksum files [`Verify::Checksum`] writes
    /// (`checksums.sha256` when empty).
    pub checksum_name: String,
}

#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub current: PathBuf,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// `current` is being read back to check its copy.
    pub verifying: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictAction {
    Overwrite,
    OverwriteAll,
    Skip,
    SkipAll,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorAction {
    Retry,
    Skip,
    SkipAll,
    Cancel,
}

pub trait OpObserver {
    /// Return false to cancel the operation.
    fn progress(&mut self, p: &Progress) -> bool;
    /// What to do with `src` when `dst` exists already.
    fn conflict(&mut self, src: &Path, dst: &Path) -> ConflictAction;
    /// What to do after `err` on `path`.
    fn error(&mut self, path: &Path, err: &io::Error) -> ErrorAction;
}

#[derive(Debug, Default)]
pub struct OpSummary {
    pub files: u64,
    pub bytes: u64,
    pub skipped: u64,
    pub cancelled: bool,
    /// How the copies were checked.
    pub verify: Verify,
    /// Files whose copy was checked and matches the original.
    pub verified: u64,
    /// Files whose copy did not match and were skipped (the bad copy is
    /// removed; a move keeps the original).
    pub verify_failed: u64,
    /// Files moved by renaming them on the same drive: no data was copied,
    /// so there is nothing to compare.
    pub renamed: u64,
    /// The checksum files written.
    pub checksum_files: Vec<PathBuf>,
    /// What went wrong, item by item (at most [`MAX_PROBLEMS`]).
    pub problems: Vec<(PathBuf, String)>,
}

/// [`OpSummary::problems`] lists at most this many.
pub const MAX_PROBLEMS: usize = 500;

impl OpSummary {
    fn problem(&mut self, path: &Path, message: String) {
        if self.problems.len() < MAX_PROBLEMS {
            self.problems.push((path.to_path_buf(), message));
        }
    }
}

/// A copy that failed its check: the kind of error that says so.
fn verify_error(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Result (or running total) of a folder size calculation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirSize {
    pub bytes: u64,
    pub files: u64,
    pub dirs: u64,
    /// Subfolders that could not be read (e.g. access denied).
    pub unreadable: u64,
}

/// Total size of everything below `root`. Junctions and symbolic links are
/// not followed (no loops, no double counting). `report` gets running totals
/// about every 100 ms; setting `cancel` stops early with a partial result.
pub fn dir_size(root: &Path, cancel: &std::sync::atomic::AtomicBool, report: &mut dyn FnMut(&DirSize)) -> DirSize {
    use std::sync::atomic::Ordering;
    let mut total = DirSize::default();
    let mut stack = vec![root.to_path_buf()];
    let mut last = Instant::now();
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(rd) = fs::read_dir(&dir) else {
            total.unreadable += 1;
            continue;
        };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue; // junction or symlink: its target is counted where it lives
            }
            if ft.is_dir() {
                total.dirs += 1;
                stack.push(e.path());
            } else {
                total.files += 1;
                total.bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
        if last.elapsed().as_millis() >= 100 {
            last = Instant::now();
            report(&total);
        }
    }
    report(&total);
    total
}

/// Count files and bytes below `sources` (for progress totals).
pub fn measure(sources: &[PathBuf]) -> (u64, u64) {
    /// Count the files and bytes below `p` (symbolic links are not followed).
    fn walk(p: &Path, files: &mut u64, bytes: &mut u64) {
        match fs::symlink_metadata(p) {
            Ok(md) if md.is_dir() => {
                if let Ok(rd) = fs::read_dir(p) {
                    for e in rd.flatten() {
                        walk(&e.path(), files, bytes);
                    }
                }
            }
            Ok(md) => {
                *files += 1;
                *bytes += md.len();
            }
            Err(_) => {}
        }
    }
    let (mut f, mut b) = (0, 0);
    for s in sources {
        walk(s, &mut f, &mut b);
    }
    (f, b)
}

/// Resolve the target typed into the Copy/Move dialog. A trailing component
/// with wildcards (`D:\dst\*.bak`) is a rename mask; an existing directory or
/// a path ending in a separator is a destination directory; otherwise it is a
/// new name (only valid for a single source).
pub fn resolve_target(target: &Path) -> (PathBuf, Option<String>) {
    let s = target.to_string_lossy();
    if s.ends_with(['\\', '/']) || target.is_dir() {
        return (target.to_path_buf(), None);
    }
    if let Some(name) = target.file_name().and_then(|n| n.to_str()) {
        if masks::has_wildcards(name) {
            let dir = target.parent().map(Path::to_path_buf).unwrap_or_default();
            return (dir, Some(name.to_owned()));
        }
    }
    (target.to_path_buf(), Some(String::new()))
}

struct Ctx<'a> {
    obs: &'a mut dyn OpObserver,
    progress: Progress,
    summary: OpSummary,
    overwrite_all: bool,
    skip_conflicts: bool,
    skip_errors: bool,
    last_report: Instant,
    buf: Vec<u8>,
    verify: Verify,
    /// With [`Verify::Checksum`]: each file checked, (original, copy, SHA-256).
    sums: Vec<(PathBuf, PathBuf, [u8; 32])>,
    /// A target a move already asked about, and the file that was there
    /// then: its copy across volumes overwrites that file without asking
    /// again (another one there by then is asked about).
    preapproved: Option<(PathBuf, Option<FileId>)>,
    /// While a move copies across volumes: each original copied, as it was
    /// when its copy began ([`Copied`]). Only those are deleted after, and
    /// only while they are still those, unchanged.
    copied_ids: Option<std::collections::HashMap<PathBuf, Copied>>,
    /// Reads a file's [`FileStamp`]: [`file_stamp`] (in tests, one that
    /// fails on cue).
    stamp_of: fn(&Path) -> Option<FileStamp>,
    /// The files and bytes of each source, counted when the operation began
    /// (a source moved by a rename is counted by these, not walked again).
    source_totals: std::collections::HashMap<PathBuf, (u64, u64)>,
}

/// An original a move across volumes copied, as it was then: what its
/// deletion after is checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Copied {
    /// A folder, by its identity only (its times change as what is in it
    /// goes; what is in it is checked one by one).
    Folder(FileId),
    /// A file, by its identity and its size and times (only one whose
    /// stamp could be read is recorded: one that couldn't isn't deleted).
    File(FileId, FileStamp),
}

/// What [`file_id`] tells a file by.
type FileId = (u64, u128);

/// What may be where a file is put: nothing (`None`), or the file the user
/// agreed to overwrite, as it was when asked (`Some`; its identity, `None`
/// inside when that couldn't be read).
type Approved = Option<Option<FileId>>;

/// The error of a target that another operation put there meanwhile: asked
/// about, then tried again.
const APPEARED: &str = "another file was put there meanwhile";

fn appeared() -> io::Error {
    io::Error::new(io::ErrorKind::AlreadyExists, APPEARED)
}

fn is_appeared(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::AlreadyExists && e.to_string() == APPEARED
}

/// `e`, or [`appeared`] when it says the target is there.
fn appeared_if_exists(e: io::Error) -> io::Error {
    if e.kind() == io::ErrorKind::AlreadyExists {
        appeared()
    } else {
        e
    }
}

/// Internal outcome of a step: `Err(())` means cancelled.
type Step = Result<bool, ()>;

impl Ctx<'_> {
    /// Tell the observer the progress (at most every 50 ms unless `force`);
    /// `Err` when it cancelled.
    fn report(&mut self, force: bool) -> Result<(), ()> {
        if force || self.last_report.elapsed().as_millis() >= 50 {
            self.last_report = Instant::now();
            if !self.obs.progress(&self.progress) {
                return Err(());
            }
        }
        Ok(())
    }

    /// Run `f`, asking the observer on failure. Ok(true)=done, Ok(false)=skipped.
    fn attempt<T>(&mut self, path: &Path, mut f: impl FnMut(&mut Self) -> io::Result<T>) -> Result<Option<T>, ()> {
        loop {
            match f(self) {
                Ok(v) => return Ok(Some(v)),
                Err(e) if e.kind() == io::ErrorKind::Interrupted && e.to_string() == "cancelled" => return Err(()),
                Err(e) => {
                    let action = if self.skip_errors { ErrorAction::Skip } else { self.obs.error(path, &e) };
                    match action {
                        ErrorAction::Retry => continue,
                        ErrorAction::Cancel => return Err(()),
                        ErrorAction::Skip | ErrorAction::SkipAll => {
                            self.skip_errors |= action == ErrorAction::SkipAll;
                            self.summary.skipped += 1;
                            if e.kind() == io::ErrorKind::InvalidData {
                                self.summary.verify_failed += 1;
                            }
                            self.summary.problem(path, e.to_string());
                            return Ok(None);
                        }
                    }
                }
            }
        }
    }

    /// Ask whether `dst`, which is there, may be overwritten (unless Overwrite
    /// All or Skip All answered it). Ok(true) = overwrite. Whatever happens
    /// to `dst` meanwhile, the answer is for the file that was there: the
    /// caller tells it by its identity ([`Approved`]).
    fn ask_overwrite(&mut self, src: &Path, dst: &Path) -> Step {
        if self.overwrite_all {
            return Ok(true);
        }
        if self.skip_conflicts {
            return Ok(false);
        }
        match self.obs.conflict(src, dst) {
            ConflictAction::Overwrite => Ok(true),
            ConflictAction::OverwriteAll => {
                self.overwrite_all = true;
                Ok(true)
            }
            ConflictAction::Skip => Ok(false),
            ConflictAction::SkipAll => {
                self.skip_conflicts = true;
                Ok(false)
            }
            ConflictAction::Cancel => Err(()),
        }
    }

    /// Skip `src` after an error the user can't retry away (a copy onto
    /// itself): it is reported, counted and listed. `Err` when cancelled.
    fn refuse(&mut self, src: &Path, e: io::Error) -> Step {
        self.summary.skipped += 1;
        self.summary.problem(src, e.to_string());
        let (f, b) = measure(&[src.to_path_buf()]);
        self.progress.files_done += f;
        self.progress.bytes_done += b * self.passes();
        let action = if self.skip_errors { ErrorAction::Skip } else { self.obs.error(src, &e) };
        match action {
            ErrorAction::Cancel => Err(()),
            a => {
                self.skip_errors |= a == ErrorAction::SkipAll;
                Ok(false)
            }
        }
    }

    /// Copy one file, after asking about an existing target. A target that
    /// appears meanwhile (another operation made it) is asked about too,
    /// never overwritten unasked.
    fn copy_file(&mut self, src: &Path, dst: &Path) -> Step {
        let done = loop {
            let exists = fs::symlink_metadata(dst).is_ok();
            // the same file by another name (a case alias, a short name, a
            // hard link, another drive letter): overwriting it would destroy it
            if exists && same_file(src, dst, true) {
                return self.refuse(src, io::Error::new(io::ErrorKind::InvalidInput, "cannot copy a file onto itself"));
            }
            let approved: Approved = if exists {
                // the file as it is now is what the answer is about
                let id = file_id(dst, false);
                let pre = self.preapproved.take().is_some_and(|(p, i)| p == dst && i == id);
                if !pre && !self.ask_overwrite(src, dst)? {
                    self.summary.skipped += 1;
                    if let Ok(md) = fs::metadata(src) {
                        self.progress.bytes_done += md.len() * self.passes();
                    }
                    self.progress.files_done += 1;
                    return Ok(false);
                }
                Some(id)
            } else {
                None
            };
            self.progress.current = src.to_path_buf();
            self.report(true)?;
            let start_bytes = self.progress.bytes_done;
            // the original as it is when its copy begins (a move deletes it
            // only while it is still this file, unchanged; one whose stamp
            // can't be read is copied, never deleted)
            // (only then: an ordinary copy keeps the original anyway, and
            // each lookup opens it again, slow for many small files on a server)
            let src_id = if self.copied_ids.is_some() { file_id(src, false).zip((self.stamp_of)(src)) } else { None };
            let done = self.attempt(src, |ctx| {
                ctx.progress.bytes_done = start_bytes;
                match ctx.copy_file_data(src, dst, approved) {
                    // another file there now, nobody said it may be overwritten: ask
                    Err(e) if is_appeared(&e) => Ok(None),
                    r => r.map(Some),
                }
            })?;
            match done {
                Some(None) => self.progress.bytes_done = start_bytes,
                Some(Some(done)) => {
                    if let (Some(ids), Some((id, stamp))) = (&mut self.copied_ids, src_id) {
                        ids.insert(src.to_path_buf(), Copied::File(id, stamp));
                    }
                    break Some(done);
                }
                None => break None,
            }
        };
        self.progress.files_done += 1;
        match done {
            Some((n, sum)) => {
                self.summary.files += 1;
                self.summary.bytes += n;
                if self.verify != Verify::None {
                    self.summary.verified += 1;
                }
                if let Some(sum) = sum {
                    self.sums.push((src.to_path_buf(), dst.to_path_buf(), sum));
                }
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// The SHA-256 of `path`, read with progress (it counts as verifying).
    fn hash_file(&mut self, path: &Path) -> io::Result<[u8; 32]> {
        let mut input = fs::File::open(path)?;
        let mut hasher = Sha256::new();
        self.progress.verifying = true;
        if self.report(true).is_err() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        loop {
            let n = input.read(&mut self.buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&self.buf[..n]);
            self.progress.bytes_done += n as u64;
            if self.report(false).is_err() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
        }
        self.progress.verifying = false;
        let mut sum = [0u8; 32];
        sum.copy_from_slice(&hasher.finalize());
        Ok(sum)
    }

    /// Check the copy `dst` of `src` (`len` bytes, `copied` of them read):
    /// its size, and with [`Verify::Checksum`] its SHA-256 against `source_sum`.
    fn check_copy(&mut self, src: &Path, dst: &Path, len: u64, copied: u64, source_sum: Option<[u8; 32]>) -> io::Result<Option<[u8; 32]>> {
        if copied != len {
            return Err(verify_error(format!("verification failed: {} changed while it was copied ({len} bytes, {copied} read)", src.display())));
        }
        let written = fs::metadata(dst)?.len();
        if written != len {
            return Err(verify_error(format!("verification failed: the copy has {written} bytes, the original {len}")));
        }
        let Some(expected) = source_sum else { return Ok(None) };
        let sum = self.hash_file(dst)?;
        if sum != expected {
            return Err(verify_error(format!("verification failed: the SHA-256 of the copy ({}) differs from the original's ({})", hex(&sum), hex(&expected))));
        }
        Ok(Some(sum))
    }

    /// Copy the contents, times and attributes of `src` to `dst`, reporting
    /// progress, and check the copy as asked: the bytes copied and, with
    /// [`Verify::Checksum`], the SHA-256 both have.
    ///
    /// Over a file the user agreed to overwrite (`approved`), that file
    /// stays until its replacement is complete and checked: the copy goes
    /// into a file beside it first ([`partial_name`]), renamed over it at the
    /// end ([`commit_rename`]: only over that file). A cancelled, failed or
    /// bad copy leaves it as it was. Otherwise `dst` is made new. Either
    /// way, a file someone else put there meanwhile is an [`appeared`] error.
    fn copy_file_data(&mut self, src: &Path, dst: &Path, approved: Approved) -> io::Result<(u64, Option<[u8; 32]>)> {
        let md = fs::metadata(src)?;
        self.progress.verifying = false;
        if approved.is_none() {
            return self.copy_data_to(src, dst, &md).map_err(appeared_if_exists);
        }
        if fs::symlink_metadata(dst).is_ok_and(|m| m.is_dir()) {
            // found before copying it all for nothing
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "a folder of this name is there already"));
        }
        if approved == Some(None) {
            return Err(unknown_identity(dst));
        }
        let partial = partial_name(dst);
        let done = self.copy_data_to(src, &partial, &md)?;
        if let Err(e) = commit_rename(&partial, dst, approved) {
            remove_file_forced(&partial);
            return Err(e);
        }
        Ok(done)
    }

    /// [`Self::copy_file_data`] into `dst`, a file that isn't there yet
    /// (removed again when the copy fails).
    fn copy_data_to(&mut self, src: &Path, dst: &Path, md: &fs::Metadata) -> io::Result<(u64, Option<[u8; 32]>)> {
        // Windows copies it: on a network drive the server copies the data
        // itself (SMB server-side copy) instead of sending it here and back.
        // A checksum needs the bytes here anyway, so that one reads them.
        #[cfg(windows)]
        if self.verify != Verify::Checksum {
            return self.system_copy(src, dst, md);
        }
        let mut hasher = (self.verify == Verify::Checksum).then(Sha256::new);
        let mut input = fs::File::open(src)?;
        // never over a file that is there: that one isn't this copy's to clear
        let mut output = fs::File::options().write(true).create_new(true).open(dst)?;
        let mut total = 0u64;
        let result = (|| -> io::Result<()> {
            loop {
                let n = input.read(&mut self.buf)?;
                if n == 0 {
                    break;
                }
                output.write_all(&self.buf[..n])?;
                if let Some(h) = &mut hasher {
                    h.update(&self.buf[..n]);
                }
                total += n as u64;
                self.progress.bytes_done += n as u64;
                if self.report(false).is_err() {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                }
            }
            output.set_modified(md.modified()?)?;
            if self.verify != Verify::None {
                // on the disk before it is checked
                output.sync_all()?;
            }
            Ok(())
        })();
        drop(output);
        let source_sum = hasher.map(|h| {
            let mut sum = [0u8; 32];
            sum.copy_from_slice(&h.finalize());
            sum
        });
        let result = result.and_then(|()| if self.verify == Verify::None { Ok(None) } else { self.check_copy(src, dst, md.len(), total, source_sum) });
        self.progress.verifying = false;
        let sum = match result {
            Ok(sum) => sum,
            Err(e) => {
                // don't leave a partial or a bad copy behind
                let _ = fs::remove_file(dst);
                return Err(e);
            }
        };
        fs::set_permissions(dst, md.permissions())?;
        Ok((total, sum))
    }

    /// [`Self::copy_file_data`] by `CopyFileExW`, which lets an SMB server
    /// (or an ODX-capable SAN) copy the data without it passing through this
    /// computer. Checks the sizes with [`Verify::Size`].
    #[cfg(windows)]
    fn system_copy(&mut self, src: &Path, dst: &Path, md: &fs::Metadata) -> io::Result<(u64, Option<[u8; 32]>)> {
        use std::ffi::c_void;
        use windows_sys::Win32::Foundation::{ERROR_REQUEST_ABORTED, HANDLE};
        use windows_sys::Win32::Storage::FileSystem::{CopyFileExW, LPPROGRESS_ROUTINE_CALLBACK_REASON};
        // winbase.h (in windows-sys behind WindowsProgramming)
        const PROGRESS_CONTINUE: u32 = 0;
        const PROGRESS_CANCEL: u32 = 1;
        const COPY_FILE_FAIL_IF_EXISTS: u32 = 0x1;
        const COPY_FILE_ALLOW_DECRYPTED_DESTINATION: u32 = 0x8;

        struct Data<'c, 'a> {
            ctx: &'c mut Ctx<'a>,
            start: u64,
            len: u64,
            cancelled: bool,
        }
        unsafe extern "system" fn routine(
            _total: i64,
            done: i64,
            _stream_size: i64,
            _stream_done: i64,
            _stream: u32,
            _reason: LPPROGRESS_ROUTINE_CALLBACK_REASON,
            _from: HANDLE,
            _to: HANDLE,
            data: *const c_void,
        ) -> u32 {
            // SAFETY: `data` is the `Data` below, alive for the whole call,
            // and the routine runs on this thread
            let d = unsafe { &mut *(data as *mut Data) };
            // streams beyond the main one count too: not past the file's size
            d.ctx.progress.bytes_done = d.start + (done.max(0) as u64).min(d.len);
            if d.ctx.report(false).is_err() {
                d.cancelled = true;
                return PROGRESS_CANCEL;
            }
            PROGRESS_CONTINUE
        }

        let (from, to) = (long_wide(src)?, long_wide(dst)?);
        let len = md.len();
        let mut data = Data { start: self.progress.bytes_done, len, cancelled: false, ctx: self };
        // SAFETY: both names end in a NUL; `data` outlives the call
        let ok = unsafe {
            CopyFileExW(
                from.as_ptr(),
                to.as_ptr(),
                Some(routine),
                &mut data as *mut Data as *const c_void,
                std::ptr::null_mut(),
                COPY_FILE_FAIL_IF_EXISTS | COPY_FILE_ALLOW_DECRYPTED_DESTINATION,
            )
        };
        let cancelled = data.cancelled;
        let start = data.start;
        if ok == 0 {
            let e = io::Error::last_os_error();
            // a file that was there already isn't this copy's to remove
            if !matches!(e.raw_os_error(), Some(80 | 183)) {
                // ERROR_FILE_EXISTS, ERROR_ALREADY_EXISTS
                let _ = fs::remove_file(dst);
            }
            if cancelled || e.raw_os_error() == Some(ERROR_REQUEST_ABORTED as i32) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            return Err(e);
        }
        self.progress.bytes_done = start + len;
        let result = (|| -> io::Result<()> {
            // times and attributes came with the copy
            if self.verify != Verify::None {
                // on the disk before it is checked (writable for that: a
                // read-only original gives a read-only copy, set back below)
                if md.permissions().readonly() {
                    let mut perms = md.permissions();
                    #[allow(clippy::permissions_set_readonly_false)]
                    perms.set_readonly(false);
                    fs::set_permissions(dst, perms)?;
                }
                fs::OpenOptions::new().write(true).open(dst)?.sync_all()?;
                // what the original has now: it changed while copied if not `len`
                let now = fs::metadata(src)?.len();
                self.check_copy(src, dst, len, now, None)?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            let _ = fs::remove_file(dst);
            return Err(e);
        }
        fs::set_permissions(dst, md.permissions())?;
        Ok((len, None))
    }

    /// Copy `src` with everything below it.
    fn copy_tree(&mut self, src: &Path, dst: &Path) -> Step {
        let md = match self.attempt(src, |_| fs::symlink_metadata(src))? {
            Some(md) => md,
            None => return Ok(false),
        };
        if !md.is_dir() {
            return self.copy_file(src, dst);
        }
        if dst.starts_with(src) {
            let e = io::Error::new(io::ErrorKind::InvalidInput, "cannot copy a directory into itself");
            return match self.obs.error(src, &e) {
                ErrorAction::Cancel => Err(()),
                _ => Ok(false),
            };
        }
        if self.attempt(dst, |_| fs::create_dir_all(dst))?.is_none() {
            return Ok(false);
        }
        // a move deletes this folder after only while it is still this one
        // (what turns up in it after its list was read stays, and so it)
        // (by identity only: its times change as what is in it goes)
        if let (Some(ids), Some(id)) = (&mut self.copied_ids, file_id(src, false)) {
            ids.insert(src.to_path_buf(), Copied::Folder(id));
        }
        let children = match self.attempt(src, |_| fs::read_dir(src)?.map(|e| e.map(|e| e.path())).collect::<io::Result<Vec<_>>>())? {
            Some(c) => c,
            None => return Ok(false),
        };
        let mut all_ok = true;
        for child in children {
            let name = child.file_name().unwrap_or_default();
            all_ok &= self.copy_tree(&child, &dst.join(name))?;
        }
        if let Ok(m) = md.modified() {
            if let Ok(f) = fs::File::open(dst) {
                let _ = f.set_modified(m);
            }
        }
        Ok(all_ok)
    }

    /// Move `src` to `dst`: a rename on the same volume, else a copy and a
    /// delete. A target that appears meanwhile (another operation made it)
    /// is asked about too, never replaced unasked.
    fn move_item(&mut self, src: &Path, dst: &Path) -> Step {
        if src == dst {
            return Ok(true);
        }
        let (renamed, approved) = loop {
            // `dst` the item itself by another name (a case alias, a short
            // name, another drive letter): nothing to overwrite. Only a change
            // of the name's case (or a hard link's name) is left to do.
            let exists = fs::symlink_metadata(dst).is_ok();
            let itself = exists && same_file(src, dst, false);
            if itself && src.file_name() == dst.file_name() {
                return Ok(true);
            }
            let approved: Approved = if exists && !itself {
                if src.is_dir() && dst.is_dir() {
                    // merge directories: move children one by one (each renamed
                    // when it can be, so nothing is copied on the same volume)
                    return self.merge_dirs(src, dst);
                }
                // the file as it is now is what the answer is about
                let id = file_id(dst, false);
                if !self.ask_overwrite(src, dst)? {
                    self.summary.skipped += 1;
                    return Ok(false);
                }
                Some(id)
            } else {
                None
            };
            // fast path: rename within a volume; over a file only when that
            // was asked about, and only over that file
            self.progress.current = src.to_path_buf();
            let renamed = self.report(true).and_then(|()| {
                self.attempt(src, |_| match if itself { rename_itself(src, dst) } else { commit_rename(src, dst, approved) } {
                    Ok(()) => Ok(Some(true)),
                    Err(e) if rename_needs_copy(&e) => Ok(Some(false)),
                    Err(e) if is_appeared(&e) => Ok(None),
                    Err(e) => Err(e),
                })
            });
            match renamed? {
                // another file there now: ask about it
                Some(None) => continue,
                Some(Some(r)) => break (Some(r), approved),
                None => break (None, None),
            }
        };
        match renamed {
            Some(true) => {
                // a source counted when the operation began isn't walked again
                let (f, b) = self.source_totals.get(src).copied().unwrap_or_else(|| measure(&[dst.to_path_buf()]));
                self.progress.files_done += f;
                self.progress.bytes_done += b;
                self.summary.files += f;
                self.summary.bytes += b;
                self.summary.renamed += f;
                if self.verify == Verify::Checksum {
                    // no data was copied, but the checksum files list these too
                    self.hash_moved(src, dst)?;
                }
                Ok(true)
            }
            Some(false) => self.move_by_copy(src, dst, approved),
            None => {
                // skipped after the error: done with, for the progress
                let (f, b) = self.source_totals.get(src).copied().unwrap_or_else(|| measure(&[src.to_path_buf()]));
                self.progress.files_done += f;
                self.progress.bytes_done += b * self.passes();
                Ok(false)
            }
        }
    }

    /// Move the contents of the directory `src` into the existing `dst`,
    /// item by item, then remove `src` when all of it went.
    fn merge_dirs(&mut self, src: &Path, dst: &Path) -> Step {
        if dst.starts_with(src) {
            let e = io::Error::new(io::ErrorKind::InvalidInput, "cannot move a directory into itself");
            return match self.obs.error(src, &e) {
                ErrorAction::Cancel => Err(()),
                _ => Ok(false),
            };
        }
        let children = match self.attempt(src, |_| fs::read_dir(src)?.map(|e| e.map(|e| e.path())).collect::<io::Result<Vec<_>>>())? {
            Some(c) => c,
            None => return Ok(false),
        };
        let mut all_ok = true;
        for child in children {
            let name = child.file_name().unwrap_or_default();
            all_ok &= self.move_item(&child, &dst.join(name))?;
        }
        if !all_ok {
            // what was skipped stays where it was, in its folder
            return Ok(false);
        }
        // empty now: deleting it counts nothing
        self.delete_tree(src, false)
    }

    /// How many times each byte is read: twice when the copies are read back.
    fn passes(&self) -> u64 {
        if self.verify == Verify::Checksum {
            2
        } else {
            1
        }
    }

    /// The SHA-256 of every file below `dst`, which was moved there from
    /// `src` by renaming it, for the checksum files.
    fn hash_moved(&mut self, src: &Path, dst: &Path) -> Result<(), ()> {
        let Ok(md) = fs::symlink_metadata(dst) else { return Ok(()) };
        if md.is_dir() {
            let children = fs::read_dir(dst).map(|rd| rd.flatten().map(|e| e.file_name()).collect::<Vec<_>>()).unwrap_or_default();
            for name in children {
                self.hash_moved(&src.join(&name), &dst.join(&name))?;
            }
        } else if !md.file_type().is_symlink() {
            self.progress.current = dst.to_path_buf();
            let start = self.progress.bytes_done;
            if let Some(sum) = self.attempt(dst, |ctx| {
                ctx.progress.bytes_done = start;
                ctx.hash_file(dst)
            })? {
                self.sums.push((src.to_path_buf(), dst.to_path_buf(), sum));
            }
            self.progress.verifying = false;
        }
        Ok(())
    }

    /// Move across volumes: copy, then delete the source only when all of it
    /// was copied. `approved`: the file at `dst` the user agreed to overwrite.
    fn move_by_copy(&mut self, src: &Path, dst: &Path, approved: Approved) -> Step {
        // copy_tree resolves conflicts itself; deleting the source only when
        // everything below it was copied keeps skipped files safe.
        // A single file whose conflict move_item already resolved: that file
        // is overwritten without asking again (another one there by then is
        // asked about)
        if let (Some(id), false) = (approved, src.is_dir()) {
            self.preapproved = Some((dst.to_path_buf(), id));
        }
        self.copied_ids = Some(Default::default());
        let ok = self.copy_tree(src, dst);
        self.preapproved = None;
        let r = match ok {
            // only what was copied goes, and only while it is what was copied:
            // a file another operation put in its place meanwhile stays (and
            // so does its folder): a move only in part
            Ok(true) => self.delete_tree(src, false),
            other => other,
        };
        self.copied_ids = None;
        r
    }

    /// Whether `p` may be deleted as the original of a move: always, but in
    /// a move across volumes only while it is still the file or folder that
    /// was copied ([`Ctx::copied_ids`]).
    fn was_copied(&self, p: &Path) -> bool {
        let Some(ids) = &self.copied_ids else { return true };
        let Some(now) = file_id(p, false) else { return false };
        match ids.get(p) {
            Some(Copied::Folder(id)) => *id == now,
            // a file also unchanged: written to since (appended, rewritten),
            // it has more than its copy; its stamp unreadable now, that
            // can't be told
            Some(Copied::File(id, stamp)) => *id == now && (self.stamp_of)(p) == Some(*stamp),
            None => false,
        }
    }

    /// Keep `p`, the original of a move that another file or folder took
    /// the place of while it was copied (or that turned up in a folder
    /// copied): listed, not deleted.
    fn keep_uncopied(&mut self, p: &Path) {
        self.summary.skipped += 1;
        self.summary.problem(p, "not moved: it changed or turned up while it was copied (or that couldn't be checked), so it was kept".into());
    }

    /// Delete `p` with everything below it; `count` adds it to the progress.
    fn delete_tree(&mut self, p: &Path, count: bool) -> Step {
        let md = match self.attempt(p, |_| fs::symlink_metadata(p))? {
            Some(md) => md,
            None => return Ok(false),
        };
        self.progress.current = p.to_path_buf();
        self.report(false)?;
        // made writable only to be deleted at once (as it was again when
        // that fails)
        let remove = |p: &Path, dir: bool| {
            let was_readonly = clear_readonly(p);
            let r = if dir { fs::remove_dir(p) } else { fs::remove_file(p) };
            if r.is_err() && was_readonly {
                set_readonly(p);
            }
            r
        };
        if md.is_dir() {
            // the original of a move: only the folder that was copied
            if !self.was_copied(p) {
                self.keep_uncopied(p);
                return Ok(false);
            }
            let children = match self.attempt(p, |_| fs::read_dir(p)?.map(|e| e.map(|e| e.path())).collect::<io::Result<Vec<_>>>())? {
                Some(c) => c,
                None => return Ok(false),
            };
            let mut all_ok = true;
            for c in children {
                all_ok &= self.delete_tree(&c, count)?;
            }
            if !all_ok {
                return Ok(false);
            }
            let r = self.attempt(p, |ctx| {
                let _lock = entry_lock(&[p])?;
                if !ctx.was_copied(p) {
                    return Ok(false);
                }
                remove(p, true).map(|()| true)
            })?;
            Ok(match r {
                Some(false) => {
                    self.keep_uncopied(p);
                    false
                }
                r => r.is_some(),
            })
        } else {
            let is_link_dir = md.file_type().is_symlink() && p.is_dir();
            let r = self.attempt(p, |ctx| {
                // not while another operation relies on it being there
                let _lock = entry_lock(&[p])?;
                // the original of a move: only the file that was copied
                if !ctx.was_copied(p) {
                    return Ok(false);
                }
                remove(p, is_link_dir).map(|()| true)
            })?;
            let r = match r {
                Some(false) => {
                    self.keep_uncopied(p);
                    None
                }
                r => r,
            };
            if count {
                self.progress.files_done += 1;
                self.progress.bytes_done += md.len();
                if r.is_some() {
                    self.summary.files += 1;
                    self.summary.bytes += md.len();
                }
            }
            Ok(r.is_some())
        }
    }
}

/// An operation's state, with the totals of `sources` counted first.
fn new_ctx<'a>(obs: &'a mut dyn OpObserver, sources: &[PathBuf]) -> Ctx<'a> {
    // each source counted once, kept for when it is moved by a rename
    let source_totals: std::collections::HashMap<PathBuf, (u64, u64)> = sources.iter().map(|s| (s.clone(), measure(std::slice::from_ref(s)))).collect();
    let (files_total, bytes_total) = sources.iter().filter_map(|s| source_totals.get(s)).fold((0, 0), |(f, b), (sf, sb)| (f + sf, b + sb));
    Ctx {
        obs,
        progress: Progress { files_total, bytes_total, ..Default::default() },
        summary: OpSummary::default(),
        overwrite_all: false,
        skip_conflicts: false,
        skip_errors: false,
        last_report: Instant::now(),
        buf: vec![0; 1 << 20],
        verify: Verify::None,
        sums: Vec::new(),
        preapproved: None,
        copied_ids: None,
        stamp_of: file_stamp,
        source_totals,
    }
}

/// `p` for a wide Windows API, NUL-terminated: absolute and with the `\\?\`
/// (or `\\?\UNC\`) prefix, so paths beyond `MAX_PATH` work as they do in std.
#[cfg(windows)]
fn long_wide(p: &Path) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    let abs = std::path::absolute(p)?;
    let wide: Vec<u16> = abs.as_os_str().encode_wide().collect();
    let text = abs.to_string_lossy();
    let mut out: Vec<u16> = if text.starts_with(r"\\?\") || text.starts_with(r"\\.\") {
        wide
    } else if let Some(rest) = wide.strip_prefix(&[b'\\' as u16, b'\\' as u16][..]) {
        r"\\?\UNC\".encode_utf16().chain(rest.iter().copied()).collect()
    } else {
        r"\\?\".encode_utf16().chain(wide).collect()
    };
    out.push(0);
    Ok(out)
}

/// Rename `from` to `to`, failing (`AlreadyExists`) when something is
/// there, in one step: nothing appearing meanwhile is replaced.
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
        let (f, t) = (long_wide(from)?, long_wide(to)?);
        // no MOVEFILE_REPLACE_EXISTING, no MOVEFILE_COPY_ALLOWED: another
        // volume fails as a rename does (`rename_needs_copy`)
        // SAFETY: both names end in a NUL
        if unsafe { MoveFileExW(f.as_ptr(), t.as_ptr(), 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        if fs::symlink_metadata(to).is_ok() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "the target is there"));
        }
        fs::rename(from, to)
    }
}

/// Whether a failed rename means the target is on another volume (or a file
/// system that cannot rename), so a move has to copy and delete. Other
/// errors (in use, access denied) would fail the copy's delete too.
fn rename_needs_copy(e: &io::Error) -> bool {
    #[cfg(windows)]
    {
        // ERROR_INVALID_FUNCTION, ERROR_NOT_SAME_DEVICE, ERROR_NOT_SUPPORTED
        matches!(e.raw_os_error(), Some(1 | 17 | 50))
    }
    #[cfg(not(windows))]
    {
        // EXDEV
        e.raw_os_error() == Some(18)
    }
}

/// What tells a file apart from every other one, whatever path names it:
/// (volume serial number, file ID). `follow` opens what a link points to,
/// else the link itself. `None` when it can't be opened.
#[cfg(windows)]
fn file_id(p: &Path, follow: bool) -> Option<(u64, u128)> {
    use std::ffi::c_void;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandle, GetFileInformationByHandleEx, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let flags = FILE_FLAG_BACKUP_SEMANTICS | if follow { 0 } else { FILE_FLAG_OPEN_REPARSE_POINT };
    // no access asked: works on files others hold open without sharing
    let f = fs::OpenOptions::new().access_mode(0).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).custom_flags(flags).open(p).ok()?;
    let h = f.as_raw_handle();
    // the 128-bit ID (ReFS needs it), else the 64-bit one (FAT has only that)
    // SAFETY: plain-data structs filled by the calls, sized as passed
    let mut info: FILE_ID_INFO = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandleEx(h, FileIdInfo, &mut info as *mut FILE_ID_INFO as *mut c_void, std::mem::size_of::<FILE_ID_INFO>() as u32) } != 0 {
        return Some((info.VolumeSerialNumber, u128::from_le_bytes(info.FileId.Identifier)));
    }
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(h, &mut info) } != 0 {
        return Some((info.dwVolumeSerialNumber as u64, (info.nFileIndexHigh as u128) << 32 | info.nFileIndexLow as u128));
    }
    None
}

#[cfg(not(windows))]
fn file_id(p: &Path, follow: bool) -> Option<(u64, u128)> {
    use std::os::unix::fs::MetadataExt;
    let md = if follow { fs::metadata(p) } else { fs::symlink_metadata(p) }.ok()?;
    Some((md.dev(), md.ino() as u128))
}

/// What tells whether a file was written to: its size, and when it was last
/// written and last changed (Windows sets that one itself, on any change).
type FileStamp = (u64, i64, i64);

/// `p`'s [`FileStamp`] (not following a link), `None` when it can't be read.
#[cfg(windows)]
fn file_stamp(p: &Path) -> Option<FileStamp> {
    use std::ffi::c_void;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileBasicInfo, FileStandardInfo, GetFileInformationByHandleEx, FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO,
    };
    // only its attributes read: works on files others hold open without sharing
    let f = fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(p)
        .ok()?;
    let h = f.as_raw_handle();
    // SAFETY: plain-data structs filled by the calls, sized as passed
    let mut basic: FILE_BASIC_INFO = unsafe { std::mem::zeroed() };
    let mut standard: FILE_STANDARD_INFO = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        GetFileInformationByHandleEx(h, FileBasicInfo, &mut basic as *mut FILE_BASIC_INFO as *mut c_void, std::mem::size_of::<FILE_BASIC_INFO>() as u32) != 0
            && GetFileInformationByHandleEx(h, FileStandardInfo, &mut standard as *mut FILE_STANDARD_INFO as *mut c_void, std::mem::size_of::<FILE_STANDARD_INFO>() as u32) != 0
    };
    ok.then_some((standard.EndOfFile as u64, basic.LastWriteTime, basic.ChangeTime))
}

#[cfg(not(windows))]
fn file_stamp(p: &Path) -> Option<FileStamp> {
    use std::os::unix::fs::MetadataExt;
    let md = fs::symlink_metadata(p).ok()?;
    Some((md.len(), md.mtime() * 1_000_000_000 + md.mtime_nsec(), md.ctime() * 1_000_000_000 + md.ctime_nsec()))
}

/// Whether `a` and `b` are one and the same file (or folder, or link):
/// another name of it, or a hard link ([`same_file`], links not followed).
pub fn is_same_file(a: &Path, b: &Path) -> bool {
    same_file(a, b, false)
}

/// Whether `a` and `b` are one and the same file or folder: the same path
/// spelled otherwise (letter case, short name, another drive letter or
/// share for it), or a hard link. Without IDs, by the paths alone.
fn same_file(a: &Path, b: &Path, follow: bool) -> bool {
    match (file_id(a, follow), file_id(b, follow)) {
        (Some(x), Some(y)) => x == y,
        _ => {
            let abs = |p: &Path| std::path::absolute(p).map(|p| p.to_string_lossy().to_lowercase()).ok();
            a == b || abs(a).is_some_and(|x| Some(x) == abs(b))
        }
    }
}

/// Whether `dst` is the directory `src` or inside it, by whatever path.
fn within(dst: &Path, src: &Path) -> bool {
    let Some(id) = file_id(src, true) else { return dst.starts_with(src) };
    dst.ancestors().filter(|a| !a.as_os_str().is_empty()).any(|a| file_id(a, true) == Some(id))
}

/// Make `p` writable if it is read-only; whether it was.
fn clear_readonly(p: &Path) -> bool {
    let Ok(md) = fs::symlink_metadata(p) else { return false };
    let mut perms = md.permissions();
    if !perms.readonly() {
        return false;
    }
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(p, perms).is_ok()
}

/// Make `p` read-only again (after [`clear_readonly`]).
fn set_readonly(p: &Path) {
    if let Ok(md) = fs::symlink_metadata(p) {
        let mut perms = md.permissions();
        perms.set_readonly(true);
        let _ = fs::set_permissions(p, perms);
    }
}

/// Delete the file `p`, read-only or not, ignoring failure.
fn remove_file_forced(p: &Path) {
    clear_readonly(p);
    let _ = fs::remove_file(p);
}

/// What a copy that will replace `dst` is written as until it is complete:
/// `<name>.rsal-partial` beside it (a number added when that is taken).
fn partial_name(dst: &Path) -> PathBuf {
    const SUFFIX: &str = ".rsal-partial";
    let name = dst.file_name().unwrap_or_default();
    let units = name.to_string_lossy().encode_utf16().count();
    let base = if units + SUFFIX.len() + 4 <= MAX_NAME {
        let mut n = name.to_os_string();
        n.push(SUFFIX);
        n
    } else {
        // a name that long has no room for it
        SUFFIX.into()
    };
    let mut path = dst.with_file_name(&base);
    let mut n = 2;
    while fs::symlink_metadata(&path).is_ok() {
        let mut numbered = base.clone();
        numbered.push(n.to_string());
        path = dst.with_file_name(numbered);
        n += 1;
    }
    path
}

/// The error of overwriting a file whose identity can't be read: then it
/// can't be told whether it is still the one the user agreed to overwrite.
fn unknown_identity(to: &Path) -> io::Error {
    io::Error::other(format!("{}: which file this is can't be read, so it isn't overwritten", to.display()))
}

/// Rename `from` to `to` in one step: over the file there only when it is
/// the one the user agreed to overwrite (`approved`, told by its identity),
/// else only where nothing is. Another file there is an [`appeared`] error,
/// and stays as it is. The check and the rename happen under
/// [`entry_lock`], so no other operation of this program (nor of another
/// running copy) removes the file there in between, and so puts another one
/// there; another program could.
fn commit_rename(from: &Path, to: &Path, approved: Approved) -> io::Result<()> {
    match approved {
        Some(None) => Err(unknown_identity(to)),
        Some(Some(id)) => {
            let _lock = entry_lock(&[from, to])?;
            if fs::symlink_metadata(to).is_err() {
                // what was there is gone: only where nothing is
                return rename_no_replace(from, to).map_err(appeared_if_exists);
            }
            if file_id(to, false) != Some(id) {
                return Err(appeared());
            }
            replace_file(from, to)
        }
        None => {
            // `from` leaves its place: not while another operation relies on it
            let _lock = entry_lock(&[from, to])?;
            rename_no_replace(from, to).map_err(appeared_if_exists)
        }
    }
}

/// Rename `from` to `to` when they are one file by two names (a change of
/// the name's letter case, or a hard link's other name): checked again
/// right before, under [`entry_lock`]; another file there now is an
/// [`appeared`] error. A change of case renames without replacing.
fn rename_itself(from: &Path, to: &Path) -> io::Result<()> {
    let _lock = entry_lock(&[from, to])?;
    if !same_file(from, to, false) {
        return Err(appeared());
    }
    let case_only = match (from.file_name().and_then(|n| n.to_str()), to.file_name().and_then(|n| n.to_str())) {
        (Some(a), Some(b)) => a.to_lowercase() == b.to_lowercase(),
        _ => false,
    };
    if case_only {
        rename_no_replace(from, to).map_err(appeared_if_exists)
    } else {
        fs::rename(from, to)
    }
}

/// Held while an operation removes the entry of a file or folder (deletes
/// it, renames it away, or puts another file in its place): one operation
/// at a time per place, of this program and its other running copies (a
/// named mutex per place, in this Windows session). A file is only put
/// where one is by replacing it under this lock; making a new one fails
/// where one is. So a file another operation checked under the lock stays
/// there until it lets go.
pub struct EntryLock {
    #[cfg(windows)]
    handles: Vec<windows_sys::Win32::Foundation::HANDLE>,
    #[cfg(not(windows))]
    _guard: std::sync::MutexGuard<'static, ()>,
}

/// How long [`entry_lock`] waits for another operation.
const ENTRY_WAIT_MS: u32 = 60_000;

/// The name of the place of `p` for [`entry_lock`]: its folder by identity
/// and its name in any case, so every path to it (letter case, short names,
/// drive letters) is one.
#[cfg(windows)]
fn entry_key(p: &Path) -> String {
    let parent = p.parent().unwrap_or(Path::new(""));
    let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let key = match file_id(parent, true) {
        Some((volume, id)) => format!("{volume:x}:{id:x}\\{name}"),
        None => std::path::absolute(p).map(|p| p.to_string_lossy().to_lowercase()).unwrap_or(name),
    };
    format!("Local\\RustySalamander-entry-{}", hex(&Sha256::digest(key.as_bytes())[..16]))
}

/// Take the locks of the places `paths` ([`EntryLock`]), always in the same
/// order (so two operations never wait for each other).
pub fn entry_lock(paths: &[&Path]) -> io::Result<EntryLock> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_ABANDONED, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
        let mut keys: Vec<String> = paths.iter().map(|p| entry_key(p)).collect();
        keys.sort();
        keys.dedup();
        // taken so far: let go again (by the drop) when one isn't to be had
        let mut lock = EntryLock { handles: Vec::with_capacity(keys.len()) };
        for key in &keys {
            let wide: Vec<u16> = key.encode_utf16().chain([0]).collect();
            // SAFETY: a NUL-terminated name; the handle is closed by the lock
            let handle = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: a mutex handle of ours
            match unsafe { WaitForSingleObject(handle, ENTRY_WAIT_MS) } {
                // abandoned: its holder ended without letting go, ours now
                WAIT_OBJECT_0 | WAIT_ABANDONED => lock.handles.push(handle),
                _ => {
                    // SAFETY: as above
                    unsafe { CloseHandle(handle) };
                    let what = paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ");
                    return Err(io::Error::new(io::ErrorKind::TimedOut, format!("{what}: another operation is still changing it")));
                }
            }
        }
        Ok(lock)
    }
    #[cfg(not(windows))]
    {
        // one lock for every place (no named mutexes), in this program
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _ = (paths, ENTRY_WAIT_MS);
        Ok(EntryLock { _guard: LOCK.lock().unwrap_or_else(|e| e.into_inner()) })
    }
}

#[cfg(windows)]
impl Drop for EntryLock {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::ReleaseMutex;
        for &handle in self.handles.iter().rev() {
            // SAFETY: the mutex is ours (waited for above) until here
            unsafe {
                ReleaseMutex(handle);
                CloseHandle(handle);
            }
        }
    }
}

/// Put the complete copy `from` in place of the file `to`, in one rename:
/// `to` stays as it was when that fails.
fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    let was_readonly = clear_readonly(to);
    fs::rename(from, to).inspect_err(|_| {
        if was_readonly {
            set_readonly(to);
        }
    })
}

/// "ba7816bf…": `bytes` as lowercase hex digits.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `path` as a checksum file in `dir` lists it: relative to `dir` with `/`
/// (as `sha256sum -c` reads it), or the whole path when it is elsewhere.
fn listed_name(path: &Path, dir: &Path) -> String {
    match path.strip_prefix(dir) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// Write the checksum file `name` into `dir` (with " (2)" … before the
/// extension when one of that name is there already): one line per file,
/// `<sha256> *<name>`. The path written.
fn write_checksums(dir: &Path, name: &str, lines: &[String]) -> io::Result<PathBuf> {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
        _ => (name.to_owned(), String::new()),
    };
    let mut path = dir.join(name);
    let mut n = 2;
    while fs::symlink_metadata(&path).is_ok() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    let mut text = lines.join("\n");
    text.push('\n');
    fs::write(&path, text)?;
    Ok(path)
}

/// Where `src` goes in `dest_dir`: its own name, or the one `mask` makes
/// (F2-style `*.bak`, or a new name for a single item). A name made is
/// checked before anything is done with it: a mask can make an empty name
/// (`*` of `.txt` cut at the dot), a device's (`nul.txt` by `*`), one with a
/// trailing dot (`*.` of `a`)…
fn target_for(src: &Path, dest_dir: &Path, mask: &Option<String>) -> io::Result<PathBuf> {
    let name = src.file_name().unwrap_or_default();
    match mask {
        // a new name for a single item
        Some(m) if m.is_empty() => {
            let new = dest_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            check_component(&new, false).map_err(|e| invalid(format!("\"{new}\" can't be the new name: {e}")))?;
            Ok(dest_dir.to_path_buf())
        }
        Some(m) => match name.to_str() {
            Some(n) => {
                let new = masks::mask_name(n, m);
                check_component(&new, false).map_err(|e| invalid(format!("the mask {m} makes \"{new}\" of \"{n}\": {e}")))?;
                Ok(dest_dir.join(new))
            }
            None => Ok(dest_dir.join(name)),
        },
        None => Ok(dest_dir.join(name)),
    }
}

/// Copy or move `sources` to `target` (directory, `dir\mask`, or new name).
pub fn transfer(kind: OpKind, sources: &[PathBuf], target: &Path, obs: &mut dyn OpObserver) -> OpSummary {
    transfer_with(kind, sources, target, &TransferOptions::default(), obs)
}

/// [`transfer`], checking each copy as `options` asks (safe copy). A move
/// deletes an original only after its copy passed the check. With
/// [`Verify::Checksum`] a checksum file goes into the target folder and one
/// into the sources' folder (kept after a move), listing every file checked.
pub fn transfer_with(kind: OpKind, sources: &[PathBuf], target: &Path, options: &TransferOptions, obs: &mut dyn OpObserver) -> OpSummary {
    let (dest, mut mask) = resolve_target(target);
    // several sources into a non-existent target: it becomes a new directory
    if mask.as_deref() == Some("") && sources.len() > 1 {
        mask = None;
    }
    // one folder moved to its own name in other letter case: the target is
    // there (it is the folder), but it is the new name, not where it goes
    if let [src] = sources {
        if kind == OpKind::Move && mask.is_none() && src.file_name() != dest.file_name() && same_file(src, &dest, false) {
            mask = Some(String::new());
        }
    }
    let mut ctx = new_ctx(obs, sources);
    ctx.verify = options.verify;
    ctx.summary.verify = options.verify;
    ctx.progress.bytes_total *= ctx.passes();
    let mut cancelled = false;
    if mask.as_deref() != Some("") {
        if let Err(e) = fs::create_dir_all(&dest) {
            let _ = ctx.obs.error(&dest, &e);
            ctx.summary.cancelled = true;
            return ctx.summary;
        }
    }
    for src in sources {
        let dst = match target_for(src, &dest, &mask) {
            Ok(dst) => dst,
            // a name no item may get: this one is skipped, the others go
            Err(e) => {
                if ctx.refuse(src, e).is_err() {
                    cancelled = true;
                    break;
                }
                continue;
            }
        };
        // a folder into itself by another path (letter case, short name,
        // drive letter, junction): it would copy itself without end. A move
        // onto itself is only a rename (move_item).
        let inside = src.is_dir()
            && match kind {
                OpKind::Copy => within(&dst, src),
                _ => dst.parent().is_some_and(|p| within(p, src)),
            };
        let r = match kind {
            _ if inside => {
                let what = if kind == OpKind::Copy { "cannot copy a directory into itself" } else { "cannot move a directory into itself" };
                ctx.refuse(src, io::Error::new(io::ErrorKind::InvalidInput, what))
            }
            OpKind::Copy => ctx.copy_tree(src, &dst),
            OpKind::Move => ctx.move_item(src, &dst),
            OpKind::Delete => unreachable!("use delete()"),
        };
        if r.is_err() {
            cancelled = true;
            break;
        }
    }
    let _ = ctx.report(true);
    ctx.summary.cancelled = cancelled;
    // the files checked, also when cancelled part way
    if !ctx.sums.is_empty() {
        let name = if options.checksum_name.is_empty() { "checksums.sha256" } else { options.checksum_name.as_str() };
        // a single file to a new name: the target is that file
        let target_dir = if mask.as_deref() == Some("") { dest.parent().map(Path::to_path_buf).unwrap_or_default() } else { dest.clone() };
        let source_dir = sources[0].parent().map(Path::to_path_buf).unwrap_or_default();
        let lines = |copy: bool, dir: &Path| ctx.sums.iter().map(|(s, d, sum)| format!("{} *{}", hex(sum), listed_name(if copy { d } else { s }, dir))).collect::<Vec<_>>();
        let mut sides = vec![(target_dir.clone(), lines(true, &target_dir))];
        if source_dir == target_dir {
            // copies beside their originals: one file lists both
            sides[0].1.extend(lines(false, &source_dir));
        } else {
            sides.push((source_dir.clone(), lines(false, &source_dir)));
        }
        for (dir, lines) in sides {
            match write_checksums(&dir, name, &lines) {
                Ok(p) => ctx.summary.checksum_files.push(p),
                Err(e) => ctx.summary.problem(&dir.join(name), format!("cannot write the checksum file: {e}")),
            }
        }
    }
    ctx.summary
}

/// Permanently delete `sources` (recursively).
pub fn delete(sources: &[PathBuf], obs: &mut dyn OpObserver) -> OpSummary {
    let mut ctx = new_ctx(obs, sources);
    for src in sources {
        if ctx.delete_tree(src, true).is_err() {
            ctx.summary.cancelled = true;
            break;
        }
    }
    let _ = ctx.report(true);
    ctx.summary
}

/// The stack of a thread that copies, moves or deletes: copying and
/// deleting go down a tree one call per folder, and NTFS allows trees about
/// 16 000 folders deep. Only what is used of it is ever allocated.
pub const JOB_STACK: usize = 256 * 1024 * 1024;

#[cfg(test)]
const DEEP_TEST_LEVELS: usize = 3_000;

/// Create a directory (F7); intermediate directories are created as needed.
pub fn make_dir(parent: &Path, name: &str) -> io::Result<PathBuf> {
    validate_name(name, true)?;
    let p = parent.join(name);
    if p.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "a file or directory with this name already exists"));
    }
    fs::create_dir_all(&p)?;
    Ok(p)
}

/// Rename in place (F2 / Shift+F6). Allows changing only the case of a name.
/// Never over another file, not even one put there meanwhile.
pub fn rename(src: &Path, new_name: &str) -> io::Result<PathBuf> {
    validate_name(new_name, false)?;
    let dst = src.with_file_name(new_name);
    let _lock = entry_lock(&[src, &dst])?;
    // a change of case only: the name is the file's own (renames itself);
    // anything else there is refused in the same step as the rename
    rename_no_replace(src, &dst).map_err(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            io::Error::new(io::ErrorKind::AlreadyExists, "a file or directory with this name already exists")
        } else {
            e
        }
    })?;
    Ok(dst)
}

/// The longest name of one file or folder, in UTF-16 units (NTFS, and most
/// other file systems, in bytes).
pub const MAX_NAME: usize = 255;

/// Whether Windows keeps `name` for a device (`CON`, `nul.txt`, `COM1`…):
/// a file of that name would be the device, not a file.
pub fn is_device_name(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or("").trim_end_matches(' ').to_ascii_uppercase();
    match base.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        b if b.starts_with("COM") || b.starts_with("LPT") => {
            let n = &b[3..];
            n.len() == 1 && n.as_bytes()[0].is_ascii_digit() || matches!(n, "¹" | "²" | "³")
        }
        _ => false,
    }
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

/// One name (no separators) as Windows can store it.
fn check_component(name: &str, wildcards: bool) -> io::Result<()> {
    if name.is_empty() {
        return Err(invalid("a folder name in the path is empty".into()));
    }
    if name == "." || name == ".." {
        return Err(invalid("\".\" and \"..\" can't be used as a name".into()));
    }
    let bad = |c: char| (c as u32) < 32 || c == '\u{7f}' || matches!(c, '<' | '>' | ':' | '"' | '|') || (!wildcards && matches!(c, '?' | '*'));
    if let Some(c) = name.chars().find(|&c| bad(c)) {
        return Err(invalid(if (c as u32) < 32 || c == '\u{7f}' { "a name must not contain control characters".into() } else { format!("a name must not contain {c}") }));
    }
    // Windows drops a trailing dot or space: the item would get another name
    if !wildcards && (name.ends_with('.') || name.ends_with(' ')) {
        return Err(invalid(format!("\"{name}\" ends with a dot or a space, which Windows drops")));
    }
    if is_device_name(name) {
        return Err(invalid(format!("\"{name}\" is a name Windows keeps for a device")));
    }
    if name.encode_utf16().count() > MAX_NAME {
        return Err(invalid(format!("a name can have at most {MAX_NAME} characters")));
    }
    Ok(())
}

/// Reject names Windows cannot store, or would store as something else:
/// characters it forbids, control characters, device names (`CON`,
/// `NUL`…), a trailing dot or space, more than [`MAX_NAME`] characters.
/// Any other Unicode character is fine. With `allow_separators`, a relative
/// path of folders (`a\b\c`), without `.` or `..`.
pub fn validate_name(name: &str, allow_separators: bool) -> io::Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(invalid("no name was entered".into()));
    }
    if !allow_separators {
        if let Some(c) = name.chars().find(|c| matches!(c, '\\' | '/')) {
            return Err(invalid(format!("a name must not contain {c}")));
        }
    }
    for part in name.split(['\\', '/']) {
        check_component(part, false)?;
    }
    Ok(())
}

/// Check where a copy or move goes (as typed, made absolute): no device
/// (`\\.\…`, `NUL`, `CON`…), no characters Windows forbids, no alternate
/// data stream (`file:stream`), no control characters. `*` and `?` are
/// allowed in the last name, a mask that renames (`*.bak`). `in_plugin`: the
/// target is inside a plugin's archive or connection (the caller asked the
/// plugins), whose names the plugin checks; only control characters are
/// refused there.
pub fn validate_target(target: &Path, in_plugin: bool) -> io::Result<()> {
    let s = target.to_string_lossy();
    if s.chars().any(|c| (c as u32) < 32 || c == '\u{7f}') {
        return Err(invalid("the target must not contain control characters".into()));
    }
    validate_location(target)?;
    if in_plugin {
        return Ok(());
    }
    // \\?\C:\… and \\?\UNC\… are ordinary paths; anything else there is a device
    let device = |rest: &[u8]| !(rest.len() >= 3 && rest[0].is_ascii_alphabetic() && rest[1..3] == *b":\\" || rest.len() >= 4 && rest[..4].eq_ignore_ascii_case(b"UNC\\"));
    let b = s.as_bytes();
    if b.starts_with(br"\\.\") || b.starts_with(br"\\?\") && device(&b[4..]) || b.starts_with(br"\??\") {
        return Err(invalid(format!("{s} is a device, not a folder")));
    }
    // "a:b" is drive A:'s current folder, then b: never what is meant
    if matches!(target.components().next(), Some(std::path::Component::Prefix(_))) && !target.has_root() {
        let drive = s.chars().take(2).collect::<String>();
        return Err(invalid(format!("{s} has no folder after the drive; type {drive}\\ and the folder")));
    }
    let names: Vec<&std::ffi::OsStr> = target.components().filter_map(|c| if let std::path::Component::Normal(n) = c { Some(n) } else { None }).collect();
    for (i, n) in names.iter().enumerate() {
        let n = n.to_string_lossy();
        let last = i + 1 == names.len();
        check_component(&n, last && crate::masks::has_wildcards(&n))?;
    }
    Ok(())
}

/// What kind of place a path is ([`validate_location`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceKind {
    /// On a drive of this computer (`C:\…`, `\\?\C:\…`).
    Local,
    /// On a server: `\\server`, `\\server\share\…` (`\\?\UNC\…`).
    Unc,
    /// Inside a plugin's connection, `\\?\<scheme>\<authority>\…`.
    Mounted,
}

/// Whether `p` may be looked at at all: checked before anything asks the
/// file system or a plugin about it (whether it is a file, a folder, an
/// archive), which for some paths already opens something. Refused:
/// control characters; device paths (`\\.\…`, `\\?\` but for `\\?\C:\` and
/// `\\?\UNC\`, `\??\`); a drive without its root (`C:x`, its current folder
/// there); a device name (`CON`, `NUL`, `COM1`, `nul.txt`…) as any name in
/// it, since looking at a folder above it looks at the device; a stream
/// (`name:stream`); a server whose name can't be one. In a plugin's
/// connection, the plugin checks names, but `.` and `..` aren't names.
/// Says which kind of place it is: the rules for names differ.
pub fn validate_location(p: &Path) -> io::Result<PlaceKind> {
    use std::path::{Component, Prefix};
    let s = p.to_string_lossy();
    if s.chars().any(|c| (c as u32) < 32 || c == '\u{7f}') {
        return Err(invalid("a path must not contain control characters (line breaks, tabs…)".into()));
    }
    if let Some((_, _, names)) = crate::location::virtual_parts(p) {
        // as typed: the path's own parsing leaves "." and ".." out of its names
        if names.iter().any(|n| n.is_empty()) || s.split(['\\', '/']).skip(3).any(|n| n == "." || n == "..") {
            return Err(invalid(format!("{s}: \".\" and \"..\" aren't names of folders on a server")));
        }
        return Ok(PlaceKind::Mounted);
    }
    // \\?\C:\… and \\?\UNC\… are ordinary paths; anything else there is a device
    let device = |rest: &[u8]| !(rest.len() >= 3 && rest[0].is_ascii_alphabetic() && rest[1..3] == *b":\\" || rest.len() >= 4 && rest[..4].eq_ignore_ascii_case(b"UNC\\"));
    let b = s.as_bytes();
    if b.starts_with(br"\\.\") || b.starts_with(br"//./") || b.starts_with(br"\\?\") && device(&b[4..]) || b.starts_with(br"//?/") || b.starts_with(br"\??\") {
        return Err(invalid(format!("{s} is a device, not a folder")));
    }
    // a server: its name, and only that
    let server_ok = |name: &str| !name.is_empty() && !name.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '/' | '\\'));
    let mut kind = PlaceKind::Local;
    match p.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::UNC(server, _) | Prefix::VerbatimUNC(server, _) => {
                if !server_ok(&server.to_string_lossy()) {
                    return Err(invalid(format!("{s}: not a name of a server")));
                }
                kind = PlaceKind::Unc;
            }
            // "a:b" is drive A:'s current folder, then b: never what is meant
            Prefix::Disk(_) if !p.has_root() => {
                let drive = s.chars().take(2).collect::<String>();
                return Err(invalid(format!("{s} has no folder after the drive; type {drive}\\ and the folder")));
            }
            _ => {}
        },
        _ => {
            if let Some(server) = crate::location::unc_server(p) {
                if !server_ok(&server) {
                    return Err(invalid(format!("{s}: not a name of a server")));
                }
                return Ok(PlaceKind::Unc);
            }
        }
    }
    for c in p.components() {
        if let Component::Normal(n) = c {
            let n = n.to_string_lossy();
            if is_device_name(&n) {
                return Err(invalid(format!("\"{n}\" is a name Windows keeps for a device")));
            }
            if n.contains(':') {
                return Err(invalid(format!("\"{n}\": a name must not contain : (that names a stream of a file)")));
            }
        }
    }
    Ok(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What may be looked at, and what is refused before anything is.
    #[test]
    fn places_safe_to_look_at() {
        for (p, kind) in [
            (r"C:\Users\me", PlaceKind::Local),
            (r"\\?\C:\very\long", PlaceKind::Local),
            (r"\\server", PlaceKind::Unc),
            (r"\\server\share\dir", PlaceKind::Unc),
            (r"\\?\UNC\server\share", PlaceKind::Unc),
            (r"\\?\scp\me@host\home", PlaceKind::Mounted),
            (r"C:\nul_\console\com10", PlaceKind::Local),
        ] {
            assert_eq!(validate_location(Path::new(p)).unwrap(), kind, "{p}");
        }
        for p in [
            r"\\.\PhysicalDrive0",
            r"\\.\pipe\x",
            "//./C:",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1",
            r"\??\C:\x",
            "C:x",
            r"C:\a\NUL\b",
            r"C:\a\con.txt",
            r"C:\a\COM1\x.zip\in",
            r"\\server\share\AUX",
            r"C:\a\b:stream",
            r"\\?\scp\me@host\..\etc",
            "C:\\a\nb",
            r"\\se<r\share",
            r"\\a*b",
        ] {
            assert!(validate_location(Path::new(p)).is_err(), "{p:?} wasn't refused");
        }
    }

    struct Auto(ConflictAction);
    impl OpObserver for Auto {
        fn progress(&mut self, _: &Progress) -> bool {
            true
        }
        fn conflict(&mut self, _: &Path, _: &Path) -> ConflictAction {
            self.0
        }
        fn error(&mut self, p: &Path, e: &io::Error) -> ErrorAction {
            panic!("unexpected error on {}: {e}", p.display())
        }
    }

    fn tree(root: &Path) {
        fs::create_dir_all(root.join("složka/子目录")).unwrap();
        fs::write(root.join("složka/žluťoučký.txt"), "kůň").unwrap();
        fs::write(root.join("složka/子目录/文件.txt"), "数据").unwrap();
        fs::write(root.join("😀.bin"), vec![7u8; 3 << 20]).unwrap();
    }

    #[test]
    fn copy_move_delete_unicode() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        let dst = t.path().join("dst");
        tree(&src);
        let sources = vec![src.join("složka"), src.join("😀.bin")];

        let s = transfer(OpKind::Copy, &sources, &dst, &mut Auto(ConflictAction::Cancel));
        assert!(!s.cancelled);
        assert_eq!(s.files, 3);
        assert_eq!(fs::read_to_string(dst.join("složka/子目录/文件.txt")).unwrap(), "数据");
        assert_eq!(fs::metadata(dst.join("😀.bin")).unwrap().len(), 3 << 20);

        // copying again: conflicts skipped
        let s = transfer(OpKind::Copy, &sources, &dst, &mut Auto(ConflictAction::SkipAll));
        assert_eq!(s.files, 0);
        assert_eq!(s.skipped, 3);

        let moved = t.path().join("moved");
        let s = transfer(OpKind::Move, &[dst.join("složka")], &moved, &mut Auto(ConflictAction::Cancel));
        assert_eq!(s.files, 2);
        assert!(!dst.join("složka").exists());
        assert!(moved.join("žluťoučký.txt").exists()); // single dir to a new name = rename

        let s = delete(std::slice::from_ref(&moved), &mut Auto(ConflictAction::Cancel));
        assert!(!s.cancelled);
        assert!(!moved.exists());
    }

    /// SHA-256 of "abc".
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn safe_copy_checks_sizes() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = (t.path().join("src"), t.path().join("dst"));
        tree(&src);
        let options = TransferOptions { verify: Verify::Size, ..Default::default() };
        let s = transfer_with(OpKind::Copy, &[src.join("složka"), src.join("😀.bin")], &dst, &options, &mut Auto(ConflictAction::Cancel));
        assert_eq!((s.files, s.verified, s.verify_failed), (3, 3, 0));
        assert!(s.checksum_files.is_empty() && s.problems.is_empty());
        assert_eq!(fs::read_dir(&dst).unwrap().count(), 2, "no checksum file with sizes only");
    }

    #[test]
    fn safe_copy_writes_checksums_to_both_sides() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = (t.path().join("src"), t.path().join("dst"));
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), "abc").unwrap();
        fs::write(src.join("sub").join("b.txt"), "abc").unwrap();
        let options = TransferOptions { verify: Verify::Checksum, checksum_name: "check.sha256".into() };
        let mut progress = Vec::new();
        struct Watch<'a>(&'a mut Vec<Progress>);
        impl OpObserver for Watch<'_> {
            fn progress(&mut self, p: &Progress) -> bool {
                self.0.push(p.clone());
                true
            }
            fn conflict(&mut self, _: &Path, _: &Path) -> ConflictAction {
                ConflictAction::Cancel
            }
            fn error(&mut self, p: &Path, e: &io::Error) -> ErrorAction {
                panic!("unexpected error on {}: {e}", p.display())
            }
        }
        let s = transfer_with(OpKind::Copy, &[src.join("a.txt"), src.join("sub")], &dst, &options, &mut Watch(&mut progress));
        assert_eq!((s.files, s.verified), (2, 2));
        assert_eq!(s.checksum_files, [dst.join("check.sha256"), src.join("check.sha256")]);
        let want = format!("{ABC} *a.txt\n{ABC} *sub/b.txt\n");
        assert_eq!(fs::read_to_string(dst.join("check.sha256")).unwrap(), want);
        assert_eq!(fs::read_to_string(src.join("check.sha256")).unwrap(), want);
        // the copies are read back: twice the bytes, and it says so
        let last = progress.last().unwrap();
        assert_eq!((last.bytes_done, last.bytes_total), (12, 12));
        assert!(progress.iter().any(|p| p.verifying));
        // again: the checksum files don't overwrite the earlier ones
        let s = transfer_with(OpKind::Copy, &[src.join("a.txt")], &dst, &options, &mut Auto(ConflictAction::OverwriteAll));
        assert_eq!(s.checksum_files, [dst.join("check (2).sha256"), src.join("check (2).sha256")]);
    }

    #[test]
    fn safe_move_keeps_the_sources_checksum_file() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = (t.path().join("src"), t.path().join("dst"));
        fs::create_dir_all(src.join("dir")).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("dir").join("a.txt"), "abc").unwrap();
        let options = TransferOptions { verify: Verify::Checksum, checksum_name: "m.sha256".into() };
        // on the same drive a move renames: nothing copied, but listed
        let s = transfer_with(OpKind::Move, &[src.join("dir")], &dst, &options, &mut Auto(ConflictAction::Cancel));
        assert_eq!((s.files, s.renamed, s.verified), (1, 1, 0));
        assert!(!src.join("dir").exists() && dst.join("dir/a.txt").exists());
        assert_eq!(fs::read_to_string(src.join("m.sha256")).unwrap(), format!("{ABC} *dir/a.txt\n"), "kept where the originals were");
        assert_eq!(fs::read_to_string(dst.join("m.sha256")).unwrap(), format!("{ABC} *dir/a.txt\n"));
        // merging into a folder that exists renames what is inside it
        fs::create_dir_all(src.join("dir")).unwrap();
        fs::write(src.join("dir").join("b.txt"), "abc").unwrap();
        let s = transfer_with(OpKind::Move, &[src.join("dir")], &dst, &options, &mut Auto(ConflictAction::Cancel));
        assert_eq!((s.files, s.verified, s.renamed), (1, 0, 1));
        assert!(!src.join("dir").exists() && dst.join("dir/b.txt").exists());
    }

    /// A move into a folder of the same name renames each item inside it
    /// (nothing is copied on one volume), merging deeper where both have a
    /// folder, and asks about files both have.
    #[test]
    fn merging_move_renames_children() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = (t.path().join("src"), t.path().join("dst"));
        fs::create_dir_all(src.join("Photos/2024")).unwrap();
        fs::create_dir_all(src.join("Photos/2025")).unwrap();
        fs::create_dir_all(dst.join("Photos/2024")).unwrap();
        fs::write(src.join("Photos/2024/a.jpg"), "new").unwrap();
        fs::write(src.join("Photos/2024/b.jpg"), "b").unwrap();
        fs::write(src.join("Photos/2025/c.jpg"), "c").unwrap();
        fs::write(dst.join("Photos/2024/a.jpg"), "old").unwrap();
        fs::write(dst.join("Photos/2024/keep.jpg"), "k").unwrap();
        let s = transfer(OpKind::Move, &[src.join("Photos")], &dst, &mut Auto(ConflictAction::Overwrite));
        assert!(!s.cancelled);
        assert_eq!((s.files, s.renamed, s.skipped), (3, 3, 0));
        assert!(!src.join("Photos").exists());
        assert_eq!(fs::read_to_string(dst.join("Photos/2024/a.jpg")).unwrap(), "new");
        assert!(dst.join("Photos/2024/keep.jpg").exists() && dst.join("Photos/2025/c.jpg").exists());

        // a skipped file keeps its folder (and what is left in it) at the source
        fs::create_dir_all(src.join("Photos")).unwrap();
        fs::write(src.join("Photos/x.jpg"), "1").unwrap();
        fs::write(dst.join("Photos/x.jpg"), "2").unwrap();
        fs::write(src.join("Photos/y.jpg"), "3").unwrap();
        let s = transfer(OpKind::Move, &[src.join("Photos")], &dst, &mut Auto(ConflictAction::Skip));
        assert_eq!((s.files, s.skipped), (1, 1));
        assert!(src.join("Photos/x.jpg").exists() && !src.join("Photos/y.jpg").exists());
        assert_eq!(fs::read_to_string(dst.join("Photos/x.jpg")).unwrap(), "2");
    }

    /// The system copy keeps times and the read-only attribute, checks sizes,
    /// and copies paths beyond MAX_PATH.
    #[test]
    fn system_copy_keeps_attributes_and_long_paths() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("ro.txt");
        fs::write(&src, "read only").unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000);
        fs::File::options().write(true).open(&src).unwrap().set_modified(old).unwrap();
        let mut perms = fs::metadata(&src).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&src, perms).unwrap();
        let mut deep = t.path().join("long");
        while deep.as_os_str().len() < 300 {
            deep.push("a_rather_long_folder_name_here");
        }
        for out in [t.path().join("out"), deep] {
            fs::create_dir_all(&out).unwrap();
            let options = TransferOptions { verify: Verify::Size, ..Default::default() };
            let s = transfer_with(OpKind::Copy, std::slice::from_ref(&src), &out, &options, &mut Auto(ConflictAction::Cancel));
            assert_eq!((s.files, s.verified), (1, 1), "{}", out.display());
            let md = fs::metadata(out.join("ro.txt")).unwrap();
            assert!(md.permissions().readonly());
            assert_eq!(md.modified().unwrap(), old);
            assert_eq!(fs::read_to_string(out.join("ro.txt")).unwrap(), "read only");
            // again over the read-only copy
            let s = transfer_with(OpKind::Copy, std::slice::from_ref(&src), &out, &options, &mut Auto(ConflictAction::Overwrite));
            assert_eq!(s.files, 1);
        }
        let mut perms = fs::metadata(&src).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&src, perms).unwrap();
    }

    /// Overwrite All answered inside one merged folder holds for the next
    /// folders too (it was forgotten after each folder of a move).
    #[test]
    fn overwrite_all_holds_across_merged_folders() {
        struct Count(u32);
        impl OpObserver for Count {
            fn progress(&mut self, _: &Progress) -> bool {
                true
            }
            fn conflict(&mut self, _: &Path, _: &Path) -> ConflictAction {
                self.0 += 1;
                ConflictAction::OverwriteAll
            }
            fn error(&mut self, p: &Path, e: &io::Error) -> ErrorAction {
                panic!("unexpected error on {}: {e}", p.display())
            }
        }
        for kind in [OpKind::Copy, OpKind::Move] {
            let t = tempfile::tempdir().unwrap();
            let (src, dst) = (t.path().join("src"), t.path().join("dst"));
            for side in [&src, &dst] {
                for d in ["a", "b", "c"] {
                    fs::create_dir_all(side.join(d)).unwrap();
                    fs::write(side.join(d).join("x.txt"), side.to_string_lossy().as_bytes()).unwrap();
                    fs::write(side.join(d).join("y.txt"), side.to_string_lossy().as_bytes()).unwrap();
                }
            }
            let sources: Vec<PathBuf> = ["a", "b", "c"].iter().map(|d| src.join(d)).collect();
            let mut obs = Count(0);
            let s = transfer(kind, &sources, &dst, &mut obs);
            assert_eq!(obs.0, 1, "{kind:?}: asked once");
            assert_eq!((s.files, s.skipped), (6, 0), "{kind:?}");
            assert_eq!(fs::read_to_string(dst.join("c/y.txt")).unwrap(), src.to_string_lossy(), "{kind:?}");
        }
    }

    #[test]
    fn a_copy_that_fails_its_check_is_removed() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("a.txt"), "abc").unwrap();
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        let mut auto = Auto(ConflictAction::Cancel);
        let mut ctx = new_ctx(&mut auto, &[]);
        ctx.verify = Verify::Checksum;
        let e = ctx.check_copy(&t.path().join("a.txt"), &t.path().join("a.txt"), 3, 3, Some([0; 32])).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        assert!(e.to_string().contains("differs"), "{e}");
        let e = ctx.check_copy(&t.path().join("a.txt"), &t.path().join("a.txt"), 4, 4, None).unwrap_err();
        assert!(e.to_string().contains("has 3 bytes"), "{e}");
        // skipped after the question: counted, listed
        struct SkipAll;
        impl OpObserver for SkipAll {
            fn progress(&mut self, _: &Progress) -> bool {
                true
            }
            fn conflict(&mut self, _: &Path, _: &Path) -> ConflictAction {
                ConflictAction::Cancel
            }
            fn error(&mut self, _: &Path, _: &io::Error) -> ErrorAction {
                ErrorAction::SkipAll
            }
        }
        let mut skip = SkipAll;
        let mut ctx = new_ctx(&mut skip, &[]);
        let r = ctx.attempt(&out, |_| Err::<(), _>(verify_error("verification failed: test".into()))).unwrap();
        assert!(r.is_none());
        assert_eq!((ctx.summary.skipped, ctx.summary.verify_failed), (1, 1));
        assert_eq!(ctx.summary.problems, [(out.clone(), "verification failed: test".to_owned())]);
    }

    #[test]
    fn copy_with_rename_mask() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("zpráva.txt"), "x").unwrap();
        fs::write(t.path().join("日記.txt"), "y").unwrap();
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        let s = transfer(OpKind::Copy, &[t.path().join("zpráva.txt"), t.path().join("日記.txt")], &out.join("*.bak"), &mut Auto(ConflictAction::Cancel));
        assert_eq!(s.files, 2);
        assert!(out.join("zpráva.bak").exists());
        assert!(out.join("日記.bak").exists());
    }

    /// A name a mask makes is checked before anything is done with it: one
    /// that can't be (empty, a device's, a trailing dot) skips that item with
    /// the reason; the others are copied or moved.
    #[test]
    fn names_masks_make_are_checked() {
        let t = tempfile::tempdir().unwrap();
        for f in ["nul.txt", "a.txt", "b.log"] {
            fs::write(t.path().join(f), f).unwrap();
        }
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        let sources: Vec<PathBuf> = ["nul.txt", "a.txt", "b.log"].iter().map(|f| t.path().join(f)).collect();
        // "*" of nul.txt is NUL
        for (kind, mask) in [(OpKind::Copy, "*"), (OpKind::Move, "*")] {
            let mut obs = Noted::new(ConflictAction::Cancel);
            let s = transfer(kind, &sources, &out.join(mask), &mut obs);
            assert_eq!(s.skipped, 1, "{kind:?}");
            assert!(obs.errors[0].contains("nul") && obs.errors[0].contains("device"), "{:?}", obs.errors);
            assert!(t.path().join("nul.txt").exists(), "the original stays");
            let _ = fs::remove_dir_all(&out);
            fs::create_dir(&out).unwrap();
            if kind == OpKind::Move {
                assert!(!t.path().join("a.txt").exists());
            }
        }
        // "* " of b.log: "b.log " (Windows would drop the space)
        fs::write(t.path().join("b.log"), "b").unwrap();
        let mut obs = Noted::new(ConflictAction::Cancel);
        let s = transfer(OpKind::Copy, &[t.path().join("b.log")], &out.join("* "), &mut obs);
        assert_eq!((s.files, s.skipped), (0, 1));
        assert!(names(&out).is_empty(), "nothing made: {:?}", names(&out));
        // a single item to a new name that can't be one
        let mut obs = Noted::new(ConflictAction::Cancel);
        let s = transfer(OpKind::Copy, &[t.path().join("b.log")], &out.join("con.txt"), &mut obs);
        assert_eq!((s.files, s.skipped), (0, 1));
        assert!(names(&out).is_empty());
    }

    #[test]
    fn copy_single_to_new_name() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("a.txt"), "x").unwrap();
        transfer(OpKind::Copy, &[t.path().join("a.txt")], &t.path().join("ä-copy.txt"), &mut Auto(ConflictAction::Cancel));
        assert_eq!(fs::read_to_string(t.path().join("ä-copy.txt")).unwrap(), "x");
    }

    #[test]
    fn mkdir_rename_validate() {
        let t = tempfile::tempdir().unwrap();
        let d = make_dir(t.path(), "Нова папка 🦎").unwrap();
        assert!(d.is_dir());
        let r = rename(&d, "Νέος φάκελος").unwrap();
        assert!(r.is_dir());
        assert!(rename(&r, "bad|name").is_err());
        assert!(make_dir(t.path(), "").is_err());
    }

    #[test]
    fn dir_size_counts_tree_and_cancels() {
        use std::sync::atomic::AtomicBool;
        let t = tempfile::tempdir().unwrap();
        tree(t.path()); // 3 files: 3 B + 6 B + 3 MiB, 2 dirs (+ the root's own entries)
        let expected = fs::metadata(t.path().join("😀.bin")).unwrap().len()
            + fs::metadata(t.path().join("složka/žluťoučký.txt")).unwrap().len()
            + fs::metadata(t.path().join("složka/子目录/文件.txt")).unwrap().len();
        let mut reports = 0;
        let s = dir_size(t.path(), &AtomicBool::new(false), &mut |_| reports += 1);
        assert_eq!(s, DirSize { bytes: expected, files: 3, dirs: 2, unreadable: 0 });
        assert!(reports >= 1, "final total is always reported");

        let cancelled = dir_size(t.path(), &AtomicBool::new(true), &mut |_| {});
        assert_eq!(cancelled.files, 0);
        let missing = dir_size(&t.path().join("nope"), &AtomicBool::new(false), &mut |_| {});
        assert_eq!(missing.unreadable, 1);
    }

    #[cfg(windows)]
    #[test]
    fn dir_size_does_not_follow_junctions() {
        use std::sync::atomic::AtomicBool;
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("root");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub").join("data.bin"), vec![0u8; 1000]).unwrap();
        // a junction back to the root would loop forever if followed
        let link = root.join("sub").join("loop");
        let ok = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(&root).output().map(|o| o.status.success()).unwrap_or(false);
        if !ok {
            return;
        }
        let s = dir_size(&root, &AtomicBool::new(false), &mut |_| {});
        assert_eq!((s.bytes, s.files, s.dirs), (1000, 1, 1));
    }

    #[cfg(windows)]
    #[test]
    fn invalid_utf16_name_roundtrips() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        let t = tempfile::tempdir().unwrap();
        // "a" + unpaired high surrogate + ".txt": not representable in UTF-8
        let name = OsString::from_wide(&[0x61, 0xD800, 0x2E, 0x74, 0x78, 0x74]);
        let src = t.path().join(&name);
        fs::write(&src, "lossless").unwrap();
        let l = crate::listing::read_dir(t.path(), crate::listing::Visibility::ALL, Default::default()).unwrap();
        let e = l.entries.iter().find(|e| e.name_lossy).expect("lossy entry");
        assert_eq!(e.name, "a\u{FFFD}.txt");
        // the lossless path still works for operations
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        transfer(OpKind::Copy, std::slice::from_ref(&e.path), &out, &mut Auto(ConflictAction::Cancel));
        assert_eq!(fs::read_to_string(out.join(&name)).unwrap(), "lossless");
    }

    /// Every hostile input: no panic; names that would be devices, leave
    /// the folder, or that Windows forbids or changes are refused with a
    /// message; nothing is made outside the folder.
    #[test]
    fn hostile_names_are_refused_and_nothing_leaves_the_folder() {
        let t = tempfile::tempdir().unwrap();
        let parent = t.path().join("p");
        fs::create_dir(&parent).unwrap();
        for bad in crate::hostile::never_a_name() {
            assert!(validate_name(&bad, true).is_err(), "accepted as a folder path: {bad:?}");
            assert!(validate_name(&bad, false).is_err(), "accepted as a name: {bad:?}");
            let e = make_dir(&parent, &bad).unwrap_err();
            assert!(!e.to_string().is_empty());
        }
        for s in crate::hostile::inputs() {
            let _ = make_dir(&parent, s.trim());
            let _ = validate_target(&parent.join(&s), false);
        }
        let outside: Vec<String> = fs::read_dir(t.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(outside, vec!["p".to_string()], "nothing outside the folder");
        // what was made has names Windows keeps as typed
        fn walk(d: &Path) {
            for e in fs::read_dir(d).unwrap() {
                let e = e.unwrap();
                let n = e.file_name().to_string_lossy().into_owned();
                assert!(validate_name(&n, false).is_ok(), "made {n:?}");
                if e.file_type().unwrap().is_dir() {
                    walk(&e.path());
                }
            }
        }
        walk(&parent);
        // odd but valid names are kept
        for good in ["Příliš žluťoučký kůň", "日本語のファイル", "& calc.exe", "%PATH%", "{error}", "👨‍👩‍👧‍👦", "zero\u{200b}width", "\u{202e}gpj.exe", "CONSOLE", "com10", "nul_", "x.con"] {
            assert!(validate_name(good, false).is_ok(), "{good:?}");
        }
        assert!(validate_name(&"a".repeat(MAX_NAME), false).is_ok());
        assert!(validate_name(&"😀".repeat(MAX_NAME / 2), false).is_ok());
        assert!(validate_name(&"😀".repeat(MAX_NAME / 2 + 1), false).is_err(), "counted in UTF-16 units");
    }

    /// Renaming to a hostile name: refused, or done in the same folder.
    #[test]
    fn hostile_renames_stay_in_the_folder() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("d");
        fs::create_dir(&dir).unwrap();
        let mut f = dir.join("victim.txt");
        fs::write(&f, "x").unwrap();
        for s in crate::hostile::inputs() {
            if let Ok(new) = rename(&f, s.trim()) {
                assert_eq!(new.parent(), Some(dir.as_path()), "{s:?}");
                f = rename(&new, "victim.txt").unwrap();
            }
        }
        for bad in crate::hostile::never_a_name() {
            assert!(rename(&f, &bad).is_err(), "{bad:?}");
        }
        assert!(f.exists());
        assert_eq!(fs::read_dir(t.path()).unwrap().count(), 1);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    }

    /// Where a copy or move may go.
    #[test]
    fn copy_targets() {
        for bad in [
            r"\\.\PhysicalDrive0",
            r"\\.\C:",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\x",
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\x",
            r"\??\C:\x",
            r"C:\x\NUL",
            r"C:\x\nul.txt",
            r"C:\x\con\y",
            r"C:\x\COM1.log",
            r"C:\x\a:b",
            r"C:\x\file.txt:stream",
            r"C:\x\a|b",
            r"C:\x\a<b",
            r"C:\x\*\y",
            "C:\\x\\line\nbreak",
            "C:\\x\\a\0b",
            r"C:\x\trailing.\y",
            "a:b",
            "C:relative",
        ] {
            assert!(validate_target(Path::new(bad), false).is_err(), "{bad}");
        }
        for good in [r"C:\x\y", r"\\?\C:\x", r"\\?\UNC\server\share\d", r"\\server\share\d", r"C:\x\*.bak", r"C:\x\a?c.*", r"D:\Příliš\日本 & co", r"C:\x\..\y"] {
            assert!(validate_target(Path::new(good), false).is_ok(), "{good}");
        }
        // inside a plugin: its names are the plugin's business, line breaks are not
        assert!(validate_target(Path::new(r"\\?\scp\me@host\a:b"), true).is_ok());
        assert!(validate_target(Path::new("\\\\?\\ftp\\host\\a\r\nDELE x"), true).is_err());
        // no panic on anything
        for s in crate::hostile::inputs() {
            let _ = validate_target(Path::new(&s), false);
            let _ = validate_target(Path::new(&format!(r"\\?\{s}")), false);
        }
    }

    /// A tree thousands of folders deep (as an archive or another program
    /// can leave): counted, copied and deleted on a job's thread without
    /// running out of stack (with 2 MB it overflowed at 3 000 folders).
    /// Slow, minutes: `cargo test --release -- --ignored very_deep`.
    #[test]
    #[ignore]
    fn very_deep_trees() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("deep");
        let mut p = src.clone();
        fs::create_dir(&p).unwrap();
        for _ in 0..DEEP_TEST_LEVELS {
            p.push("a");
            fs::create_dir(&p).unwrap();
        }
        fs::write(p.join("bottom.txt"), "deep").unwrap();
        let dst = t.path().join("copy");
        fs::create_dir(&dst).unwrap();
        let (src2, dst2) = (src.clone(), dst.clone());
        let summary = std::thread::Builder::new()
            .stack_size(JOB_STACK)
            .spawn(move || {
                assert_eq!(measure(std::slice::from_ref(&src2)), (1, 4));
                let s = transfer(OpKind::Copy, std::slice::from_ref(&src2), &dst2, &mut Auto(ConflictAction::Cancel));
                delete(&[src2.clone(), dst2.join("deep")], &mut Auto(ConflictAction::Cancel));
                s
            })
            .unwrap()
            .join()
            .expect("the job's thread ran out of stack");
        assert_eq!(summary.files, 1);
        assert!(!src.exists() && !dst.join("deep").exists());
    }

    /// Answers conflicts with `answer`, skips errors and notes them.
    struct Noted {
        answer: ConflictAction,
        errors: Vec<String>,
        asked: u32,
    }
    impl Noted {
        fn new(answer: ConflictAction) -> Self {
            Noted { answer, errors: Vec::new(), asked: 0 }
        }
    }
    impl OpObserver for Noted {
        fn progress(&mut self, _: &Progress) -> bool {
            true
        }
        fn conflict(&mut self, _: &Path, _: &Path) -> ConflictAction {
            self.asked += 1;
            self.answer
        }
        fn error(&mut self, _: &Path, e: &io::Error) -> ErrorAction {
            self.errors.push(e.to_string());
            ErrorAction::Skip
        }
    }

    /// The names in `dir`.
    fn names(dir: &Path) -> Vec<String> {
        let mut n: Vec<String> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        n.sort();
        n
    }

    /// A copy onto the very file it copies, by the same path or another
    /// (letter case, a hard link), is refused: overwriting it destroyed it.
    #[test]
    fn copy_onto_itself_is_refused() {
        for verify in [Verify::None, Verify::Size, Verify::Checksum] {
            let t = tempfile::tempdir().unwrap();
            let dir = t.path().join("dir");
            fs::create_dir(&dir).unwrap();
            let file = dir.join("x.txt");
            fs::write(&file, "data").unwrap();
            let links = t.path().join("links");
            fs::create_dir(&links).unwrap();
            fs::hard_link(&file, links.join("x.txt")).unwrap();
            let options = TransferOptions { verify, ..Default::default() };
            let mut targets = vec![dir.clone(), links.clone()];
            if cfg!(windows) {
                // case-insensitive names: the same folder, the same file
                targets.extend([t.path().join("DIR"), t.path().join("DIR").join("X.TXT")]);
            }
            for target in targets {
                let mut obs = Noted::new(ConflictAction::Overwrite);
                let s = transfer_with(OpKind::Copy, std::slice::from_ref(&file), &target, &options, &mut obs);
                assert_eq!((s.files, s.skipped, obs.asked), (0, 1, 0), "{verify:?} to {}", target.display());
                assert!(obs.errors.len() == 1 && obs.errors[0].contains("onto itself"), "{:?}", obs.errors);
                assert_eq!(fs::read_to_string(&file).unwrap(), "data", "{verify:?} to {}", target.display());
                assert_eq!(names(&dir), ["x.txt"]);
            }
        }
    }

    /// A folder copied into itself by another path stops at once.
    #[cfg(windows)]
    #[test]
    fn copy_into_itself_by_alias_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("dir");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a.txt"), "a").unwrap();
        for (kind, target) in [(OpKind::Copy, t.path().join("DIR")), (OpKind::Copy, t.path().join("DIR").join("SUB")), (OpKind::Move, t.path().join("DIR").join("SUB"))] {
            let mut obs = Noted::new(ConflictAction::Overwrite);
            let s = transfer(kind, std::slice::from_ref(&dir), &target, &mut obs);
            assert_eq!((s.files, s.skipped), (0, 1), "{kind:?} to {}", target.display());
            assert!(obs.errors[0].contains("into itself"), "{:?}", obs.errors);
            assert_eq!(names(&dir), ["a.txt", "sub"]);
            assert_eq!(names(&dir.join("sub")).len(), 0);
        }
    }

    /// A move onto itself by another path changes nothing, asks nothing;
    /// one that changes only the case of the name renames it.
    #[cfg(windows)]
    #[test]
    fn move_onto_itself_keeps_it_or_changes_the_case() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("dir");
        fs::create_dir(&dir).unwrap();
        let file = dir.join("x.txt");
        fs::write(&file, "data").unwrap();
        let mut obs = Noted::new(ConflictAction::Overwrite);
        let s = transfer(OpKind::Move, std::slice::from_ref(&file), &t.path().join("DIR"), &mut obs);
        assert!(!s.cancelled && obs.errors.is_empty() && obs.asked == 0);
        assert_eq!(fs::read_to_string(&file).unwrap(), "data");
        let s = transfer(OpKind::Move, std::slice::from_ref(&file), &dir.join("X.TXT"), &mut obs);
        assert!(!s.cancelled && obs.errors.is_empty() && obs.asked == 0, "{:?}", obs.errors);
        assert_eq!(s.renamed, 1);
        assert_eq!(names(&dir), ["X.TXT"]);
        assert_eq!(fs::read_to_string(dir.join("X.TXT")).unwrap(), "data");
        // a folder likewise
        let s = transfer(OpKind::Move, std::slice::from_ref(&dir), &t.path().join("Dir"), &mut obs);
        assert!(!s.cancelled && obs.errors.is_empty() && obs.asked == 0, "{:?}", obs.errors);
        assert_eq!(names(t.path()), ["Dir"]);
    }

    /// No file beside `dst` left from a copy that didn't finish.
    fn no_partial(dir: &Path) {
        assert!(names(dir).iter().all(|n| !n.contains("rsal-partial")), "{:?}", names(dir));
    }

    /// A copy over an existing file that is cancelled, or fails its check,
    /// leaves that file as it was.
    #[test]
    fn a_failed_overwrite_keeps_the_old_file() {
        struct Cancel;
        impl OpObserver for Cancel {
            fn progress(&mut self, _: &Progress) -> bool {
                false
            }
            fn conflict(&mut self, _: &Path, _: &Path) -> ConflictAction {
                ConflictAction::Overwrite
            }
            fn error(&mut self, _: &Path, _: &io::Error) -> ErrorAction {
                ErrorAction::Cancel
            }
        }
        /// Grows the original while it is copied.
        struct Grow(PathBuf);
        impl OpObserver for Grow {
            fn progress(&mut self, _: &Progress) -> bool {
                fs::OpenOptions::new().append(true).open(&self.0).unwrap().write_all(b"more").unwrap();
                true
            }
            fn conflict(&mut self, _: &Path, _: &Path) -> ConflictAction {
                ConflictAction::Overwrite
            }
            fn error(&mut self, _: &Path, _: &io::Error) -> ErrorAction {
                ErrorAction::Cancel
            }
        }
        let long_ago = || Instant::now().checked_sub(std::time::Duration::from_secs(1)).unwrap();
        for verify in [Verify::None, Verify::Size, Verify::Checksum] {
            let t = tempfile::tempdir().unwrap();
            let src = t.path().join("big.bin");
            fs::write(&src, vec![1u8; 3 << 20]).unwrap();
            let out = t.path().join("out");
            fs::create_dir(&out).unwrap();
            let dst = out.join("big.bin");
            fs::write(&dst, "old").unwrap();

            let mut obs = Cancel;
            let mut ctx = new_ctx(&mut obs, &[]);
            ctx.verify = verify;
            ctx.last_report = long_ago();
            let e = ctx.copy_file_data(&src, &dst, Some(file_id(&dst, false))).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::Interrupted, "{verify:?}: {e}");
            assert_eq!(fs::read_to_string(&dst).unwrap(), "old", "{verify:?}");
            no_partial(&out);

            if verify == Verify::Checksum {
                let mut obs = Grow(src.clone());
                let mut ctx = new_ctx(&mut obs, &[]);
                ctx.verify = verify;
                ctx.last_report = long_ago();
                let e = ctx.copy_file_data(&src, &dst, Some(file_id(&dst, false))).unwrap_err();
                assert_eq!(e.kind(), io::ErrorKind::InvalidData, "{e}");
                assert_eq!(fs::read_to_string(&dst).unwrap(), "old");
                no_partial(&out);
            }

            // and one that succeeds replaces it, read-only or not
            set_readonly(&dst);
            let options = TransferOptions { verify, ..Default::default() };
            let s = transfer_with(OpKind::Copy, std::slice::from_ref(&src), &out, &options, &mut Noted::new(ConflictAction::Overwrite));
            assert_eq!(s.files, 1);
            assert_eq!(fs::metadata(&dst).unwrap().len(), fs::metadata(&src).unwrap().len());
            assert!(!fs::metadata(&dst).unwrap().permissions().readonly(), "the original's attributes");
            no_partial(&out);
        }
    }

    /// A move over an existing file whose rename fails leaves that file
    /// (it was deleted before the rename was tried).
    #[cfg(windows)]
    #[test]
    fn a_failed_move_keeps_the_old_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("a.txt");
        fs::write(&src, "new").unwrap();
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        let dst = out.join("a.txt");
        fs::write(&dst, "old").unwrap();
        set_readonly(&dst);
        let held = fs::OpenOptions::new().read(true).share_mode(0).open(&src).unwrap();
        let mut obs = Noted::new(ConflictAction::Overwrite);
        let s = transfer(OpKind::Move, std::slice::from_ref(&src), &out, &mut obs);
        drop(held);
        assert_eq!((s.files, obs.errors.len()), (0, 1), "{:?}", obs.errors);
        assert_eq!(fs::read_to_string(&dst).unwrap(), "old");
        assert!(fs::metadata(&dst).unwrap().permissions().readonly(), "read-only as it was");
        // not held: replaces it
        let s = transfer(OpKind::Move, std::slice::from_ref(&src), &out, &mut Noted::new(ConflictAction::Overwrite));
        assert_eq!((s.files, s.renamed), (1, 1));
        assert_eq!(fs::read_to_string(&dst).unwrap(), "new");
        assert!(!src.exists());
    }

    #[test]
    fn partial_names() {
        let t = tempfile::tempdir().unwrap();
        let dst = t.path().join("a.txt");
        assert_eq!(partial_name(&dst), t.path().join("a.txt.rsal-partial"));
        fs::write(t.path().join("a.txt.rsal-partial"), "").unwrap();
        assert_eq!(partial_name(&dst), t.path().join("a.txt.rsal-partial2"));
        let long = t.path().join("n".repeat(250));
        assert_eq!(partial_name(&long), t.path().join(".rsal-partial"));
    }

    /// A file whose identity couldn't be read when it was asked about isn't
    /// overwritten (it couldn't be told from another one put there since).
    #[test]
    fn an_unknown_file_isnt_overwritten() {
        let t = tempfile::tempdir().unwrap();
        let (from, to) = (t.path().join("new.txt"), t.path().join("a.txt"));
        fs::write(&from, "new").unwrap();
        fs::write(&to, "old").unwrap();
        assert!(commit_rename(&from, &to, Some(None)).unwrap_err().to_string().contains("can't be read"));
        assert_eq!(fs::read_to_string(&to).unwrap(), "old");
        // its identity known: replaced
        commit_rename(&from, &to, Some(file_id(&to, false))).unwrap();
        assert_eq!(fs::read_to_string(&to).unwrap(), "new");
    }

    /// Putting a file over another waits while another operation (here a
    /// thread) is putting one at the same place, by whatever path.
    #[cfg(windows)]
    #[test]
    fn one_rename_at_a_time_per_target() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("dir");
        fs::create_dir(&dir).unwrap();
        let (from, to) = (dir.join("new.txt"), dir.join("a.txt"));
        fs::write(&from, "new").unwrap();
        fs::write(&to, "old").unwrap();
        let id = file_id(&to, false);
        // held under another spelling of the same target
        let held = entry_lock(&[&t.path().join("DIR").join("A.TXT")]).unwrap();
        let worker = std::thread::spawn(move || commit_rename(&from, &to, Some(id)));
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "old", "waits");
        drop(held);
        worker.join().unwrap().unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "new");
        // another target isn't held up by it
        let _held = entry_lock(&[&dir.join("a.txt")]).unwrap();
        fs::write(dir.join("b.new"), "b").unwrap();
        fs::write(dir.join("b.txt"), "old b").unwrap();
        let (bn, bt) = (dir.join("b.new"), dir.join("b.txt"));
        let id = file_id(&bt, false);
        std::thread::spawn(move || commit_rename(&bn, &bt, Some(id))).join().unwrap().unwrap();
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "b");
    }

    /// A move across volumes deletes an original only while it is the file
    /// it copied: one another operation put in its place meanwhile stays,
    /// and the move is reported as done in part.
    #[test]
    fn a_move_deletes_only_what_it_copied() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::create_dir_all(src.join("empty")).unwrap();
        fs::write(src.join("a.txt"), "a").unwrap();
        fs::write(src.join("sub").join("b.txt"), "b").unwrap();
        let dst = t.path().join("dst");
        // as the copy across volumes does it, here on one
        let mut obs = Noted::new(ConflictAction::Cancel);
        let mut ctx = new_ctx(&mut obs, &[]);
        assert_eq!(ctx.move_by_copy(&src, &dst, None), Ok(true));
        assert!(!src.exists());
        assert_eq!(fs::read_to_string(dst.join("sub").join("b.txt")).unwrap(), "b");
        // the same, but b.txt replaced after it was copied
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), "a").unwrap();
        fs::write(src.join("sub").join("b.txt"), "b").unwrap();
        let dst = t.path().join("dst2");
        let mut obs = Noted::new(ConflictAction::Cancel);
        let mut ctx = new_ctx(&mut obs, &[]);
        ctx.copied_ids = Some(Default::default());
        assert_eq!(ctx.copy_tree(&src, &dst), Ok(true));
        let other = src.join("sub").join("other");
        fs::write(&other, "someone else's").unwrap();
        fs::rename(&other, src.join("sub").join("b.txt")).unwrap();
        assert_eq!(ctx.delete_tree(&src, false), Ok(false), "in part");
        assert_eq!(fs::read_to_string(src.join("sub").join("b.txt")).unwrap(), "someone else's", "kept");
        assert!(!src.join("a.txt").exists(), "what was copied went");
        assert!(ctx.summary.problems[0].1.contains("changed or turned up while it was copied"), "{:?}", ctx.summary.problems);
    }

    /// An original written to after it was copied (appended to, rewritten:
    /// still the same file) has what its copy hasn't: a move keeps it.
    #[test]
    fn a_move_keeps_an_original_changed_after_it_was_copied() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        fs::create_dir_all(&src).unwrap();
        for f in ["appended.txt", "rewritten.txt", "untouched.txt"] {
            fs::write(src.join(f), "copied").unwrap();
        }
        let mut obs = Noted::new(ConflictAction::Cancel);
        let mut ctx = new_ctx(&mut obs, &[]);
        ctx.copied_ids = Some(Default::default());
        assert_eq!(ctx.copy_tree(&src, &t.path().join("dst")), Ok(true));
        let id = file_id(&src.join("rewritten.txt"), false);
        fs::OpenOptions::new().append(true).open(src.join("appended.txt")).unwrap().write_all(b" and more").unwrap();
        // the same size, in place
        fs::OpenOptions::new().write(true).open(src.join("rewritten.txt")).unwrap().write_all(b"edited").unwrap();
        assert_eq!(file_id(&src.join("rewritten.txt"), false), id, "still the same file");
        assert_eq!(ctx.delete_tree(&src, false), Ok(false), "in part");
        assert_eq!(fs::read_to_string(src.join("appended.txt")).unwrap(), "copied and more");
        assert_eq!(fs::read_to_string(src.join("rewritten.txt")).unwrap(), "edited");
        assert!(!src.join("untouched.txt").exists(), "what is as copied went");
        assert_eq!(ctx.summary.problems.len(), 2, "{:?}", ctx.summary.problems);
    }

    /// A file whose size and times can't be read, when its copy begins or
    /// when the move would delete it, can't be told unchanged: copied, and
    /// kept (read-only still, if it was); the move is done in part.
    #[test]
    fn a_move_keeps_an_original_it_cant_check() {
        fn unreadable(_: &Path) -> Option<FileStamp> {
            None
        }
        for at_start in [true, false] {
            let t = tempfile::tempdir().unwrap();
            let src = t.path().join("src");
            fs::create_dir_all(&src).unwrap();
            let file = src.join("a.txt");
            fs::write(&file, "a").unwrap();
            set_readonly(&file);
            let dst = t.path().join("dst");
            let mut obs = Noted::new(ConflictAction::Cancel);
            let mut ctx = new_ctx(&mut obs, &[]);
            ctx.copied_ids = Some(Default::default());
            if at_start {
                ctx.stamp_of = unreadable;
            }
            assert_eq!(ctx.copy_tree(&src, &dst), Ok(true));
            assert!(file_id(&file, false).is_some(), "its identity is known");
            ctx.stamp_of = if at_start { file_stamp } else { unreadable };
            assert_eq!(ctx.delete_tree(&src, false), Ok(false), "in part");
            assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "a", "copied");
            assert_eq!(fs::read_to_string(&file).unwrap(), "a", "kept");
            assert!(fs::metadata(&file).unwrap().permissions().readonly(), "still read-only");
            assert_eq!(ctx.summary.problems.len(), 1, "{:?}", ctx.summary.problems);
            let mut perms = fs::metadata(&file).unwrap().permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            fs::set_permissions(&file, perms).unwrap();
        }
    }

    /// Folders likewise: one that turns up in a folder after it was copied,
    /// or takes the place of one copied, stays, and so do the folders it is
    /// in; what was copied goes.
    #[test]
    fn a_move_deletes_only_the_folders_it_copied() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        for d in ["kept/empty", "gone/empty"] {
            fs::create_dir_all(src.join(d)).unwrap();
        }
        fs::write(src.join("gone").join("a.txt"), "a").unwrap();
        let mut obs = Noted::new(ConflictAction::Cancel);
        let mut ctx = new_ctx(&mut obs, &[]);
        ctx.copied_ids = Some(Default::default());
        assert_eq!(ctx.copy_tree(&src, &t.path().join("dst")), Ok(true));
        // after the copy: a new empty folder, and one copied replaced by another
        fs::create_dir(src.join("kept").join("new")).unwrap();
        fs::remove_dir(src.join("kept").join("empty")).unwrap();
        fs::create_dir(src.join("kept").join("empty")).unwrap();
        assert_eq!(ctx.delete_tree(&src, false), Ok(false), "in part");
        assert!(src.join("kept").join("new").is_dir() && src.join("kept").join("empty").is_dir(), "kept");
        assert!(!src.join("gone").exists(), "what was copied went");
        assert_eq!(ctx.summary.problems.len(), 2, "{:?}", ctx.summary.problems);
    }

    /// While an operation holds a place (it checked the file there and is
    /// about to replace it), another one deletes it, renames it or moves it
    /// away only after it let go.
    #[cfg(windows)]
    #[test]
    fn removing_waits_for_the_place() {
        let t = tempfile::tempdir().unwrap();
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        let held = out.join("a.txt");
        type Job = Box<dyn FnOnce(&Path, &Path) + Send>;
        let jobs: Vec<(&str, Job)> = vec![
            (
                "delete",
                Box::new(|f, _| {
                    delete(&[f.to_path_buf()], &mut Auto(ConflictAction::Cancel));
                }),
            ),
            (
                "F2",
                Box::new(|f, _| {
                    rename(f, "b.txt").unwrap();
                }),
            ),
            (
                "move",
                Box::new(|f, other| {
                    transfer(OpKind::Move, &[f.to_path_buf()], other, &mut Auto(ConflictAction::Cancel));
                }),
            ),
        ];
        for (what, job) in jobs {
            fs::write(&held, "checked").unwrap();
            let other = t.path().join(format!("other-{what}"));
            fs::create_dir(&other).unwrap();
            let lock = entry_lock(&[&t.path().join("OUT").join("A.TXT")]).unwrap();
            let (f, o) = (held.clone(), other.clone());
            let worker = std::thread::spawn(move || job(&f, &o));
            std::thread::sleep(std::time::Duration::from_millis(300));
            assert_eq!(fs::read_to_string(&held).unwrap(), "checked", "{what} waits");
            drop(lock);
            worker.join().unwrap();
            assert!(!held.exists(), "{what} done after");
            let _ = fs::remove_file(out.join("b.txt"));
        }
    }

    /// F2 never renames over another file; a change of case renames.
    #[test]
    fn f2_never_replaces() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("a.txt"), "a").unwrap();
        fs::write(t.path().join("b.txt"), "b").unwrap();
        assert_eq!(rename(&t.path().join("a.txt"), "b.txt").unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(t.path().join("b.txt")).unwrap(), "b");
        rename(&t.path().join("a.txt"), "A.TXT").unwrap();
        assert_eq!(names(t.path()), ["A.TXT", "b.txt"]);
    }

    /// A target replaced by another file while the user is asked about it
    /// (or right after): the answer was about the file that was there, so
    /// the new one is asked about again, not overwritten.
    #[test]
    fn an_answer_is_for_the_file_asked_about() {
        /// Swaps the target for another file while answering Overwrite;
        /// then answers Skip.
        struct Swap {
            target: PathBuf,
            asked: u32,
        }
        impl OpObserver for Swap {
            fn progress(&mut self, _: &Progress) -> bool {
                true
            }
            fn conflict(&mut self, _: &Path, _: &Path) -> ConflictAction {
                self.asked += 1;
                if self.asked > 1 {
                    return ConflictAction::Skip;
                }
                let other = self.target.with_extension("other");
                fs::write(&other, "other").unwrap();
                fs::rename(&other, &self.target).unwrap();
                ConflictAction::Overwrite
            }
            fn error(&mut self, p: &Path, e: &io::Error) -> ErrorAction {
                panic!("unexpected error on {}: {e}", p.display())
            }
        }
        for kind in [OpKind::Copy, OpKind::Move] {
            for verify in [Verify::None, Verify::Checksum] {
                let t = tempfile::tempdir().unwrap();
                let src = t.path().join("a.txt");
                fs::write(&src, "mine").unwrap();
                let out = t.path().join("out");
                fs::create_dir(&out).unwrap();
                let target = out.join("a.txt");
                fs::write(&target, "old").unwrap();
                let mut obs = Swap { target: target.clone(), asked: 0 };
                let options = TransferOptions { verify, ..Default::default() };
                let s = transfer_with(kind, std::slice::from_ref(&src), &out, &options, &mut obs);
                let case = format!("{kind:?} {verify:?}");
                assert_eq!(obs.asked, 2, "{case}: asked again about the other file");
                assert_eq!((s.files, s.skipped), (0, 1), "{case}");
                assert_eq!(fs::read_to_string(&target).unwrap(), "other", "{case}");
                assert!(src.exists(), "{case}");
                assert!(names(&out).iter().all(|n| !n.contains("rsal-partial")), "{case}: {:?}", names(&out));
            }
        }
    }

    /// A target that another operation makes while this one is under way
    /// (after the check whether it is there) is asked about, not replaced.
    #[test]
    fn a_target_made_meanwhile_is_asked_about() {
        /// Makes the target when the copy reports its first progress.
        struct Meanwhile {
            target: PathBuf,
            answer: ConflictAction,
            asked: u32,
        }
        impl OpObserver for Meanwhile {
            fn progress(&mut self, _: &Progress) -> bool {
                if self.asked == 0 && !self.target.exists() {
                    fs::write(&self.target, "theirs").unwrap();
                }
                true
            }
            fn conflict(&mut self, _: &Path, _: &Path) -> ConflictAction {
                self.asked += 1;
                self.answer
            }
            fn error(&mut self, p: &Path, e: &io::Error) -> ErrorAction {
                panic!("unexpected error on {}: {e}", p.display())
            }
        }
        for kind in [OpKind::Copy, OpKind::Move] {
            for verify in [Verify::None, Verify::Checksum] {
                for answer in [ConflictAction::Skip, ConflictAction::Overwrite] {
                    let t = tempfile::tempdir().unwrap();
                    let src = t.path().join("a.txt");
                    fs::write(&src, "mine").unwrap();
                    let out = t.path().join("out");
                    fs::create_dir(&out).unwrap();
                    let target = out.join("a.txt");
                    let mut obs = Meanwhile { target: target.clone(), answer, asked: 0 };
                    let options = TransferOptions { verify, ..Default::default() };
                    let s = transfer_with(kind, std::slice::from_ref(&src), &out, &options, &mut obs);
                    let case = format!("{kind:?} {verify:?} {answer:?}");
                    assert_eq!(obs.asked, 1, "{case}");
                    if answer == ConflictAction::Skip {
                        assert_eq!((s.files, s.skipped), (0, 1), "{case}");
                        assert_eq!(fs::read_to_string(&target).unwrap(), "theirs", "{case}");
                        assert!(src.exists(), "{case}");
                    } else {
                        assert_eq!(s.files, 1, "{case}");
                        assert_eq!(fs::read_to_string(&target).unwrap(), "mine", "{case}");
                        assert_eq!(src.exists(), kind == OpKind::Copy, "{case}");
                    }
                    assert!(names(&out).iter().all(|n| !n.contains("rsal-partial")), "{case}: {:?}", names(&out));
                }
            }
        }
    }
}
