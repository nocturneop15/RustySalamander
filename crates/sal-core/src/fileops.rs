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

    /// Decide what to do when `dst` exists. Ok(true) = proceed (overwrite).
    fn resolve_conflict(&mut self, src: &Path, dst: &Path) -> Step {
        if fs::symlink_metadata(dst).is_err() {
            return Ok(true);
        }
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

    /// Copy one file, after asking about an existing target.
    fn copy_file(&mut self, src: &Path, dst: &Path) -> Step {
        if !self.resolve_conflict(src, dst)? {
            self.summary.skipped += 1;
            if let Ok(md) = fs::metadata(src) {
                self.progress.bytes_done += md.len() * self.passes();
            }
            self.progress.files_done += 1;
            return Ok(false);
        }
        self.progress.current = src.to_path_buf();
        self.report(true)?;
        let start_bytes = self.progress.bytes_done;
        let done = self.attempt(src, |ctx| {
            ctx.progress.bytes_done = start_bytes;
            ctx.copy_file_data(src, dst)
        })?;
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
    fn copy_file_data(&mut self, src: &Path, dst: &Path) -> io::Result<(u64, Option<[u8; 32]>)> {
        let md = fs::metadata(src)?;
        self.progress.verifying = false;
        let mut hasher = (self.verify == Verify::Checksum).then(Sha256::new);
        let mut input = fs::File::open(src)?;
        // clear read-only on an existing target so it can be overwritten
        if let Ok(dmd) = fs::metadata(dst) {
            let mut perms = dmd.permissions();
            if perms.readonly() {
                #[allow(clippy::permissions_set_readonly_false)]
                perms.set_readonly(false);
                fs::set_permissions(dst, perms)?;
            }
        }
        let mut output = fs::File::create(dst)?;
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
    /// delete.
    fn move_item(&mut self, src: &Path, dst: &Path) -> Step {
        if src == dst {
            return Ok(true);
        }
        if fs::symlink_metadata(dst).is_ok() {
            let is_dir = src.is_dir();
            if is_dir && dst.is_dir() {
                // merge directories: move children one by one
                return self.move_by_copy(src, dst);
            }
            if !self.resolve_conflict(src, dst)? {
                self.summary.skipped += 1;
                return Ok(false);
            }
            if !is_dir {
                let _ = fs::remove_file(dst);
            }
        }
        // fast path: rename within a volume
        self.progress.current = src.to_path_buf();
        self.report(true)?;
        match fs::rename(src, dst) {
            Ok(()) => {
                let (f, b) = measure(&[dst.to_path_buf()]);
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
            Err(_) => self.move_by_copy(src, dst),
        }
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
    /// was copied.
    fn move_by_copy(&mut self, src: &Path, dst: &Path) -> Step {
        // copy_tree resolves conflicts itself; deleting the source only when
        // everything below it was copied keeps skipped files safe
        // a single file whose conflict move_item already resolved: overwrite
        // it without asking again, then forget that (it was this file only).
        // A merged directory keeps what the user chose inside it, so an
        // Overwrite All there holds for the rest of the operation.
        let forced = !self.overwrite_all && fs::symlink_metadata(dst).is_ok() && !src.is_dir();
        if forced {
            self.overwrite_all = true;
        }
        let ok = self.copy_tree(src, dst);
        if forced {
            self.overwrite_all = false;
        }
        if ok? {
            self.delete_tree(src, false)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Delete `p` with everything below it; `count` adds it to the progress.
    fn delete_tree(&mut self, p: &Path, count: bool) -> Step {
        let md = match self.attempt(p, |_| fs::symlink_metadata(p))? {
            Some(md) => md,
            None => return Ok(false),
        };
        self.progress.current = p.to_path_buf();
        self.report(false)?;
        let clear_ro = |p: &Path| {
            if let Ok(md) = fs::symlink_metadata(p) {
                let mut perms = md.permissions();
                if perms.readonly() {
                    #[allow(clippy::permissions_set_readonly_false)]
                    perms.set_readonly(false);
                    let _ = fs::set_permissions(p, perms);
                }
            }
        };
        if md.is_dir() {
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
            clear_ro(p);
            Ok(self.attempt(p, |_| fs::remove_dir(p))?.is_some())
        } else {
            let is_link_dir = md.file_type().is_symlink() && p.is_dir();
            clear_ro(p);
            let r = self.attempt(p, |_| if is_link_dir { fs::remove_dir(p) } else { fs::remove_file(p) })?;
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
    let (files_total, bytes_total) = measure(sources);
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
    }
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
/// (F2-style `*.bak`, or a new name for a single item).
fn target_for(src: &Path, dest_dir: &Path, mask: &Option<String>) -> PathBuf {
    let name = src.file_name().unwrap_or_default();
    match mask {
        // a new name for a single item
        Some(m) if m.is_empty() => dest_dir.to_path_buf(),
        Some(m) => match name.to_str() {
            Some(n) => dest_dir.join(masks::mask_name(n, m)),
            None => dest_dir.join(name),
        },
        None => dest_dir.join(name),
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
        let dst = target_for(src, &dest, &mask);
        let r = match kind {
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
pub fn rename(src: &Path, new_name: &str) -> io::Result<PathBuf> {
    validate_name(new_name, false)?;
    let dst = src.with_file_name(new_name);
    let same_ignoring_case = src.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.to_lowercase() == new_name.to_lowercase());
    if dst.exists() && !same_ignoring_case {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "a file or directory with this name already exists"));
    }
    fs::rename(src, &dst)?;
    Ok(dst)
}

/// Reject names Windows cannot store. Any Unicode character is fine.
pub fn validate_name(name: &str, allow_separators: bool) -> io::Result<()> {
    let bad = |c: char| (c as u32) < 32 || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || (!allow_separators && matches!(c, '\\' | '/'));
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid name"));
    }
    if let Some(c) = name.chars().find(|&c| bad(c)) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("name must not contain {c:?}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // merging into a folder that exists copies, checks, then deletes
        fs::create_dir_all(src.join("dir")).unwrap();
        fs::write(src.join("dir").join("b.txt"), "abc").unwrap();
        let s = transfer_with(OpKind::Move, &[src.join("dir")], &dst, &options, &mut Auto(ConflictAction::Cancel));
        assert_eq!((s.files, s.verified, s.renamed), (1, 1, 0));
        assert!(!src.join("dir").exists() && dst.join("dir/b.txt").exists());
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
}
