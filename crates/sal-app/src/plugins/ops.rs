//! File operations inside a folder plugin's files, run by the worker thread:
//! the plugin does its side, the normal copy engine (with its conflict and
//! error questions) the files on disk.
//!
//! Copying out extracts into a temporary folder and moves the items from
//! there; copying in adds the items under the target folder's path. The
//! same works for archives and for connections a plugin opened.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sal_core::encoding::display_path;
use sal_core::fileops::{self, ConflictAction, OpKind, OpObserver, OpSummary, Progress};

use super::{FolderPlugin, Index};
use crate::i18n::{tr, trf};

/// What to do with a file extracted for opening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAs {
    /// The program Windows associates with it (Enter).
    Default,
    /// The viewer (F3).
    View,
    /// The editor (F4).
    Edit,
}

/// An operation inside a plugin's files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderOp {
    /// Copy or move out of, into, or between such files.
    Transfer(OpKind),
    Delete,
    /// Create the folder `sources[0]`.
    MkDir,
    /// Rename `sources[0]` to `target`.
    Rename,
    /// Extract `sources[0]` into the folder `target`, then open it.
    Open(OpenAs),
}

/// Progress of one plugin call, in the bytes the plugin reports.
struct Phase<'a> {
    obs: &'a mut dyn OpObserver,
    p: Progress,
}

impl Phase<'_> {
    /// A phase of `files` items, starting at `current`.
    fn new<'a>(obs: &'a mut dyn OpObserver, current: &Path, files: u64) -> Phase<'a> {
        let p = Progress { current: current.to_path_buf(), files_done: 0, files_total: files, bytes_done: 0, bytes_total: 0 };
        Phase { obs, p }
    }

    /// Report; blocks while the operation is paused, false when cancelled.
    fn tick(&mut self, done: u64, total: u64) -> bool {
        self.p.bytes_done = done;
        self.p.bytes_total = total;
        if total > 0 && done >= total {
            self.p.files_done = self.p.files_total;
        }
        self.obs.progress(&self.p)
    }
}

/// Ok(false) when the user cancelled.
fn done(r: io::Result<()>) -> Result<bool, String> {
    match r {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

struct Op<'a> {
    plugin: &'a Arc<dyn FolderPlugin>,
}

impl Op<'_> {
    /// The plugin's file and the path inside it for `p`.
    fn split(&self, p: &Path) -> Result<(PathBuf, String), String> {
        super::split(std::slice::from_ref(self.plugin), p).map(|(_, f, i)| (f, i)).ok_or_else(|| trf("{path} is not inside an archive.", &[("path", &display_path(p))]))
    }

    /// An error unless the plugin can change `file`.
    fn writable(&self, file: &Path) -> Result<(), String> {
        if self.plugin.can_write(file) {
            Ok(())
        } else {
            Err(trf("{archive} can only be read.", &[("archive", &display_path(file))]))
        }
    }

    /// What is in `folder` (and, for an archive, everywhere else).
    fn index(&self, file: &Path, folder: &str) -> Result<Index, String> {
        self.plugin.list(file, folder).map_err(|e| format!("{}: {e}", display_path(file)))
    }
}

/// Run an operation inside plugins' files (extract, add, delete, rename,
/// new folder), reporting to `obs`.
pub fn run(plugin: &Arc<dyn FolderPlugin>, op: FolderOp, sources: &[PathBuf], target: &Path, obs: &mut dyn OpObserver) -> Result<OpSummary, String> {
    let o = Op { plugin };
    let mut summary = OpSummary::default();
    let first = sources.first().ok_or("nothing to do")?;
    match op {
        FolderOp::Open(_) => {
            let (file, inner) = o.split(first)?;
            let mut ph = Phase::new(obs, first, 1);
            if done(plugin.extract(&file, &[inner], target, &mut |d, t| ph.tick(d, t)))? {
                summary.files = 1;
                summary.bytes = ph.p.bytes_total;
            } else {
                summary.cancelled = true;
            }
        }
        FolderOp::Delete => {
            let (file, _) = o.split(first)?;
            o.writable(&file)?;
            let inners = sources.iter().map(|s| o.split(s).map(|x| x.1)).collect::<Result<Vec<_>, _>>()?;
            let mut ph = Phase::new(obs, &file, inners.len() as u64);
            if done(plugin.delete(&file, &inners, &mut |d, t| ph.tick(d, t)))? {
                summary.files = inners.len() as u64;
            } else {
                summary.cancelled = true;
            }
        }
        FolderOp::MkDir => {
            let (file, inner) = o.split(first)?;
            o.writable(&file)?;
            let mut ph = Phase::new(obs, first, 1);
            summary.cancelled = !done(plugin.mkdir(&file, &inner, &mut |d, t| ph.tick(d, t)))?;
        }
        FolderOp::Rename => {
            let (file, old) = o.split(first)?;
            let (_, new) = o.split(target)?;
            o.writable(&file)?;
            let mut ph = Phase::new(obs, first, 1);
            summary.cancelled = !done(plugin.rename(&file, &old, &new, &mut |d, t| ph.tick(d, t)))?;
        }
        FolderOp::Transfer(kind) => transfer(&o, kind, sources, target, obs, &mut summary)?,
    }
    Ok(summary)
}

/// Copy or move: out of a plugin's file, into one, or between two.
fn transfer(o: &Op, kind: OpKind, sources: &[PathBuf], target: &Path, obs: &mut dyn OpObserver, summary: &mut OpSummary) -> Result<(), String> {
    let dst = o.split(target).ok();
    let Ok((src_file, _)) = o.split(&sources[0]) else {
        // files into a plugin's file
        let (file, inner) = dst.ok_or("not a plugin operation")?;
        let packed = pack(o, obs, &file, &inner, sources, summary)?;
        if kind == OpKind::Move && !summary.cancelled && !packed.is_empty() {
            let s = fileops::delete(&packed, obs);
            summary.cancelled |= s.cancelled;
        }
        return Ok(());
    };
    if kind == OpKind::Move {
        o.writable(&src_file)?;
    }
    let inners = sources.iter().map(|s| o.split(s).map(|x| x.1)).collect::<Result<Vec<_>, _>>()?;
    let tmp = super::temp_dir("extract").map_err(|e| e.to_string())?;
    let r = (|| -> Result<(), String> {
        let mut ph = Phase::new(obs, &src_file, inners.len() as u64);
        if !done(o.plugin.extract(&src_file, &inners, &tmp, &mut |d, t| ph.tick(d, t)))? {
            summary.cancelled = true;
            return Ok(());
        }
        let staged: Vec<PathBuf> = inners.iter().map(|i| tmp.join(i)).collect();
        match &dst {
            // out to disk: the copy engine asks about conflicts
            None => {
                let s = fileops::transfer(OpKind::Move, &staged, target, obs);
                summary.files += s.files;
                summary.bytes += s.bytes;
                summary.skipped += s.skipped;
                summary.cancelled |= s.cancelled;
            }
            Some((file, inner)) => {
                pack(o, obs, file, inner, &staged, summary)?;
            }
        }
        // a move removes the items, unless some were left out
        if kind == OpKind::Move && !summary.cancelled && summary.skipped == 0 {
            let mut ph = Phase::new(obs, &src_file, inners.len() as u64);
            summary.cancelled = !done(o.plugin.delete(&src_file, &inners, &mut |d, t| ph.tick(d, t)))?;
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    r
}

/// Add `sources` to the folder `inner` of `file`, asking about items that
/// are there already. Returns the items added.
fn pack(o: &Op, obs: &mut dyn OpObserver, file: &Path, inner: &str, sources: &[PathBuf], summary: &mut OpSummary) -> Result<Vec<PathBuf>, String> {
    o.writable(file)?;
    // a connection always exists; an archive may be created by this copy
    let ix = if file.exists() || sal_core::location::virtual_parts(file).is_some() { o.index(file, inner)? } else { Index::default() };
    // placeholders for what only another side has (folders compared) are not there
    let placeholders: Vec<String> = o.plugin.marks(file, inner).into_iter().filter(|(_, m)| *m == sal_plugin_api::ItemMark::Missing).map(|(n, _)| n).collect();
    let (mut overwrite_all, mut skip_all) = (false, false);
    let mut chosen = Vec::new();
    for s in sources {
        let name = s.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let at = super::join_inner(inner, &name);
        if !overwrite_all && ix.exists(&at) && !placeholders.contains(&name) {
            let action = if skip_all { ConflictAction::Skip } else { obs.conflict(s, &file.join(&at)) };
            match action {
                ConflictAction::Overwrite => {}
                ConflictAction::OverwriteAll => overwrite_all = true,
                ConflictAction::Skip | ConflictAction::SkipAll => {
                    skip_all |= action == ConflictAction::SkipAll;
                    summary.skipped += 1;
                    continue;
                }
                ConflictAction::Cancel => {
                    summary.cancelled = true;
                    return Ok(Vec::new());
                }
            }
        }
        chosen.push((s.clone(), at));
    }
    if chosen.is_empty() {
        return Ok(Vec::new());
    }
    let mut ph = Phase::new(obs, file, chosen.len() as u64);
    if !done(o.plugin.add(file, &chosen, &mut |d, t| ph.tick(d, t)))? {
        summary.cancelled = true;
        return Ok(Vec::new());
    }
    summary.files += chosen.len() as u64;
    summary.bytes += ph.p.bytes_total;
    Ok(chosen.into_iter().map(|(p, _)| p).collect())
}

/// For an archive operation whose plugin was turned off meanwhile.
pub fn missing_plugin() -> String {
    tr("The plugin for this file is not enabled.").to_owned()
}
