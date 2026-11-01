//! Folder sizes calculated on background threads (Salamander's "calculate
//! occupied space" when selecting a directory). Results are cached per path
//! until a refresh or a file operation invalidates them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;
use sal_core::fileops::{self, DirSize};

#[derive(Clone, Copy, Debug)]
pub struct SizeInfo {
    pub size: DirSize,
    pub done: bool,
}

struct Job {
    info: SizeInfo,
    cancel: Arc<AtomicBool>,
}

#[derive(Default, Clone)]
pub struct DirSizes {
    jobs: Arc<Mutex<HashMap<PathBuf, Job>>>,
}

impl DirSizes {
    /// Calculate `path` again even if a result is cached.
    pub fn restart(&self, path: PathBuf, ctx: &egui::Context) {
        if let Some(old) = self.jobs.lock().unwrap().remove(&path) {
            old.cancel.store(true, Ordering::Relaxed);
        }
        self.start(path, ctx);
    }

    /// Start calculating `path` unless it is already known or in progress.
    pub fn start(&self, path: PathBuf, ctx: &egui::Context) {
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut jobs = self.jobs.lock().unwrap();
            if jobs.contains_key(&path) {
                return;
            }
            jobs.insert(path.clone(), Job { info: SizeInfo { size: DirSize::default(), done: false }, cancel: cancel.clone() });
        }
        let jobs = self.jobs.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let update = |size: &DirSize, done: bool| {
                let mut jobs = jobs.lock().unwrap();
                // a cleared or restarted entry belongs to someone else now
                if let Some(j) = jobs.get_mut(&path).filter(|j| Arc::ptr_eq(&j.cancel, &cancel)) {
                    j.info = SizeInfo { size: *size, done };
                }
            };
            let total = fileops::dir_size(&path, &cancel, &mut |s| {
                update(s, false);
                ctx.request_repaint();
            });
            if !cancel.load(Ordering::Relaxed) {
                update(&total, true);
                ctx.request_repaint();
            }
        });
    }

    /// The size of `path` known so far, if it was asked for.
    pub fn get(&self, path: &std::path::Path) -> Option<SizeInfo> {
        self.jobs.lock().unwrap().get(path).map(|j| j.info)
    }

    /// Copy of all known sizes.
    #[cfg(test)]
    pub fn snapshot(&self) -> HashMap<PathBuf, SizeInfo> {
        self.jobs.lock().unwrap().iter().map(|(p, j)| (p.clone(), j.info)).collect()
    }

    /// The known sizes of the folders directly in `dir`, for drawing a panel
    /// showing it (each frame, so not a copy of every size known).
    pub fn snapshot_in(&self, dir: &Path) -> HashMap<PathBuf, SizeInfo> {
        self.jobs.lock().unwrap().iter().filter(|(p, _)| p.parent() == Some(dir)).map(|(p, j)| (p.clone(), j.info)).collect()
    }

    /// Whether a size is still being counted.
    #[cfg(test)]
    pub fn busy(&self) -> bool {
        self.jobs.lock().unwrap().values().any(|j| !j.info.done)
    }

    /// Forget everything (sizes may be stale after a refresh or file operation).
    pub fn clear(&self) {
        for (_, j) in self.jobs.lock().unwrap().drain() {
            j.cancel.store(true, Ordering::Relaxed);
        }
    }
}
