//! Free space of the drives the panels show, read on background threads.
//!
//! The footer shows it on every frame, but asking Windows can take a round
//! trip to a server (100 ms and more for a network drive, or until a timeout
//! when the server is gone). So it is read in the background, kept for a few
//! seconds, and the footer shows the last answer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use sal_core::drives;

/// How long an answer is shown before it is asked for again.
const FRESH: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Entry {
    value: Option<(u64, u64)>,
    at: Option<Instant>,
    asking: bool,
}

#[derive(Default, Clone)]
pub struct FreeSpace {
    roots: Arc<Mutex<HashMap<PathBuf, Entry>>>,
}

impl FreeSpace {
    /// (free, total) bytes of the drive `path` is on, as last read; a new
    /// read starts in the background when the answer is old. `None` until
    /// the first answer, and for places without drives (plugin connections).
    pub fn get(&self, ctx: &egui::Context, path: &Path) -> Option<(u64, u64)> {
        if sal_core::location::virtual_parts(path).is_some() {
            return None;
        }
        let root = drives::root_of(path);
        let mut roots = self.roots.lock().unwrap();
        let e = roots.entry(root.clone()).or_default();
        if !e.asking && e.at.is_none_or(|t| t.elapsed() > FRESH) {
            e.asking = true;
            let (roots, ctx) = (self.roots.clone(), ctx.clone());
            std::thread::spawn(move || {
                let value = drives::free_space(&root);
                if let Some(e) = roots.lock().unwrap().get_mut(&root) {
                    *e = Entry { value, at: Some(Instant::now()), asking: false };
                }
                ctx.request_repaint();
            });
        }
        e.value
    }

    /// Forget all answers (after a copy or delete changed the free space).
    pub fn forget(&self) {
        for e in self.roots.lock().unwrap().values_mut() {
            e.at = None;
        }
    }
}
