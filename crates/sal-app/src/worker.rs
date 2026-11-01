//! Background file operations with progress, conflict and error prompts
//! relayed to the UI thread over channels, plus the operation queue:
//! operations either start at once or wait until the running ones finish.

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;
use sal_core::encoding::display_path;
use sal_core::fileops::{self, ConflictAction, ErrorAction, OpKind, OpObserver, OpSummary, Progress};

use crate::i18n::{tr, trf, trn};
use crate::plugins::ops::{self, FolderOp};
use crate::plugins::FolderPlugin;

pub enum Prompt {
    Conflict { src: PathBuf, dst: PathBuf },
    Error { path: PathBuf, message: String },
}

pub enum Reply {
    Conflict(ConflictAction),
    Error(ErrorAction),
}

enum Msg {
    Progress(Progress),
    Prompt(Prompt),
    Done(Result<OpSummary, String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Transfer(OpKind),
    Delete,
    Recycle,
    /// Inside, into or out of a folder plugin's file (an archive).
    Archive(FolderOp),
}

impl JobKind {
    /// The operation's title in its window ("Copying").
    pub fn title(self) -> &'static str {
        match self {
            JobKind::Transfer(OpKind::Copy) => tr("Copying"),
            JobKind::Transfer(OpKind::Move) => tr("Moving"),
            JobKind::Transfer(OpKind::Delete) | JobKind::Delete => tr("Deleting"),
            JobKind::Recycle => tr("Moving to Recycle Bin"),
            JobKind::Archive(FolderOp::Transfer(OpKind::Copy)) => tr("Copying"),
            JobKind::Archive(FolderOp::Transfer(OpKind::Move)) => tr("Moving"),
            JobKind::Archive(FolderOp::Transfer(OpKind::Delete) | FolderOp::Delete) => tr("Deleting"),
            JobKind::Archive(FolderOp::MkDir) => tr("Creating folder"),
            JobKind::Archive(FolderOp::Rename) => tr("Renaming"),
            JobKind::Archive(FolderOp::Open(_)) => tr("Extracting"),
        }
    }

    /// Whether the operation reports progress.
    pub fn has_progress(self) -> bool {
        self != JobKind::Recycle
    }

    /// Whether it can be paused: not the Recycle Bin, which Windows does in
    /// one call.
    pub fn can_pause(self) -> bool {
        self != JobKind::Recycle
    }
}

/// When a new operation runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Start at once, alongside anything already running.
    Now,
    /// Top of the queue: pause the running operations and run this one; they
    /// go on when it finishes.
    First,
    /// Wait for the running operations and everything already queued.
    Last,
}

/// What an operation does, before and while it runs.
#[derive(Debug, Clone)]
pub struct Spec {
    pub id: u64,
    pub kind: JobKind,
    pub sources: Vec<PathBuf>,
    pub target: PathBuf,
    /// The plugin that does an archive operation.
    pub plugin: Option<PluginRef>,
}

/// A folder plugin, for operations inside its files.
#[derive(Clone)]
pub struct PluginRef(pub Arc<dyn FolderPlugin>);

impl std::fmt::Debug for PluginRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.id())
    }
}

impl Spec {
    /// "Copy 3 items to D:\backup", "Delete report.txt"
    pub fn describe(&self) -> String {
        let what = match self.sources.as_slice() {
            [one] => format!("\"{}\"", one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| display_path(one))),
            many => trn("{n} item", "{n} items", many.len() as u64),
        };
        let to = display_path(&self.target);
        match self.kind {
            JobKind::Transfer(OpKind::Copy) => trf("Copy {what} to {target}", &[("what", &what), ("target", &to)]),
            JobKind::Transfer(OpKind::Move) => trf("Move {what} to {target}", &[("what", &what), ("target", &to)]),
            JobKind::Transfer(OpKind::Delete) | JobKind::Delete => trf("Delete {what}", &[("what", &what)]),
            JobKind::Recycle => trf("Move {what} to the Recycle Bin", &[("what", &what)]),
            JobKind::Archive(FolderOp::Transfer(OpKind::Copy)) => trf("Copy {what} to {target}", &[("what", &what), ("target", &to)]),
            JobKind::Archive(FolderOp::Transfer(OpKind::Move)) => trf("Move {what} to {target}", &[("what", &what), ("target", &to)]),
            JobKind::Archive(FolderOp::Transfer(OpKind::Delete) | FolderOp::Delete) => trf("Delete {what}", &[("what", &what)]),
            JobKind::Archive(FolderOp::MkDir) => trf("Create folder {what}", &[("what", &what)]),
            JobKind::Archive(FolderOp::Rename) => {
                let new = self.target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                trf("Rename {what} to \"{name}\"", &[("what", &what), ("name", &new)])
            }
            JobKind::Archive(FolderOp::Open(_)) => trf("Extract {what}", &[("what", &what)]),
        }
    }
}

/// Transfer speed and time estimates from progress samples. Time spent
/// paused or waiting for an answer doesn't count.
pub struct Rate {
    started: Instant,
    paused_for: Duration,
    paused_at: Option<Instant>,
    /// (active time, bytes done), the last few seconds of them.
    samples: VecDeque<(Duration, u64)>,
    /// What is shown: (active time it was worked out at, speed, time left),
    /// worked out again every [`RATE_REFRESH`] so the numbers stay still.
    shown: Option<(Duration, f64, Duration)>,
}

/// Speed is measured over this much of the most recent active time.
const RATE_WINDOW: Duration = Duration::from_secs(8);
/// The speed and time left shown are worked out this often.
const RATE_REFRESH: Duration = Duration::from_secs(2);

impl Rate {
    /// A rate measured from `now`.
    pub fn new(now: Instant) -> Rate {
        Rate { started: now, paused_for: Duration::ZERO, paused_at: None, samples: VecDeque::new(), shown: None }
    }

    /// Time the operation has been running, without pauses.
    pub fn elapsed(&self, now: Instant) -> Duration {
        let paused = self.paused_for + self.paused_at.map_or(Duration::ZERO, |p| now.saturating_duration_since(p));
        now.saturating_duration_since(self.started).saturating_sub(paused)
    }

    /// Whether the clock is stopped.
    #[cfg(test)]
    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// Stop the clock (the operation is paused or asks a question).
    pub fn pause(&mut self, now: Instant) {
        self.paused_at.get_or_insert(now);
    }

    /// Start the clock again.
    pub fn resume(&mut self, now: Instant) {
        if let Some(p) = self.paused_at.take() {
            self.paused_for += now.saturating_duration_since(p);
        }
    }

    /// `bytes` of `total` done by `now`; old samples beyond the window are
    /// dropped. The speed and time left shown follow every [`RATE_REFRESH`].
    pub fn sample(&mut self, now: Instant, bytes: u64, total: u64) {
        let t = self.elapsed(now);
        self.samples.push_back((t, bytes));
        while self.samples.len() > 2 && self.samples.front().is_some_and(|&(s, _)| t.saturating_sub(s) > RATE_WINDOW) {
            self.samples.pop_front();
        }
        if self.shown.is_none_or(|(at, _, _)| t.saturating_sub(at) >= RATE_REFRESH) {
            if let Some(speed) = self.measured() {
                let left = Duration::from_secs_f64(total.saturating_sub(bytes) as f64 / speed);
                self.shown = Some((t, speed, left));
            }
        }
    }

    /// Bytes per second over the recent samples (needs a second of data).
    fn measured(&self) -> Option<f64> {
        let (&(t0, b0), &(t1, b1)) = (self.samples.front()?, self.samples.back()?);
        let span = (t1 - t0).as_secs_f64();
        (span >= 1.0 && b1 > b0).then(|| (b1 - b0) as f64 / span)
    }

    /// The speed shown, bytes per second.
    pub fn speed(&self) -> Option<f64> {
        self.shown.map(|(_, speed, _)| speed)
    }

    /// The estimated time to copy the rest, shown.
    pub fn remaining(&self) -> Option<Duration> {
        self.shown.map(|(_, _, left)| left)
    }
}

/// "0:42", "12:05", "1:02:03"
pub fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub struct Job {
    pub spec: Spec,
    /// The progress shown. A move's counts and bars follow the operation
    /// every [`RATE_REFRESH`] (the file name at once), so they don't jump
    /// about; other operations' at once.
    pub progress: Progress,
    /// When the shown counts were last brought up to date.
    shown_at: Option<Instant>,
    /// Progress newer than the shown, not shown yet.
    latest: Option<Progress>,
    pub prompt: Option<Prompt>,
    pub result: Option<Result<OpSummary, String>>,
    pub rate: Rate,
    /// Paused automatically while the operation with this id runs first.
    pub paused_by: Option<u64>,
    rx: Receiver<Msg>,
    reply: Sender<Reply>,
    cancel: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
}

struct ChannelObserver {
    tx: Sender<Msg>,
    rx: Receiver<Reply>,
    cancel: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    ctx: egui::Context,
}

impl ChannelObserver {
    /// Ask the user (on the UI thread) and wait for the answer; `None` when
    /// the window went away.
    fn ask(&mut self, p: Prompt) -> Option<Reply> {
        self.tx.send(Msg::Prompt(p)).ok()?;
        self.ctx.request_repaint();
        self.rx.recv().ok()
    }
}

impl OpObserver for ChannelObserver {
    fn progress(&mut self, p: &Progress) -> bool {
        let _ = self.tx.send(Msg::Progress(p.clone()));
        self.ctx.request_repaint();
        // paused: wait here, between two chunks of a file
        while self.paused.load(Ordering::Relaxed) && !self.cancel.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(50));
        }
        !self.cancel.load(Ordering::Relaxed)
    }

    fn conflict(&mut self, src: &Path, dst: &Path) -> ConflictAction {
        match self.ask(Prompt::Conflict { src: src.into(), dst: dst.into() }) {
            Some(Reply::Conflict(a)) => a,
            _ => ConflictAction::Cancel,
        }
    }

    fn error(&mut self, path: &Path, err: &io::Error) -> ErrorAction {
        match self.ask(Prompt::Error { path: path.into(), message: err.to_string() }) {
            Some(Reply::Error(a)) => a,
            _ => ErrorAction::Cancel,
        }
    }
}

impl Job {
    /// Start the operation `spec` on a thread of its own.
    pub fn start(ctx: &egui::Context, spec: Spec) -> Job {
        Job::spawn(ctx, spec, false)
    }

    /// Start `spec`, paused or not.
    fn spawn(ctx: &egui::Context, spec: Spec, paused: bool) -> Job {
        let (tx, rx) = channel();
        let (reply_tx, reply_rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(paused));
        let mut obs = ChannelObserver { tx: tx.clone(), rx: reply_rx, cancel: cancel.clone(), paused: paused.clone(), ctx: ctx.clone() };
        let ctx2 = ctx.clone();
        let (kind, sources, target, plugin) = (spec.kind, spec.sources.clone(), spec.target.clone(), spec.plugin.clone());
        thread::spawn(move || {
            let result = match kind {
                JobKind::Transfer(k) => Ok(fileops::transfer(k, &sources, &target, &mut obs)),
                JobKind::Delete => Ok(fileops::delete(&sources, &mut obs)),
                JobKind::Recycle => sal_core::shell::recycle(&sources).map(|_| OpSummary { files: sources.len() as u64, ..Default::default() }).map_err(|e| e.to_string()),
                JobKind::Archive(op) => match &plugin {
                    Some(p) => ops::run(&p.0, op, &sources, &target, &mut obs),
                    None => Err(ops::missing_plugin()),
                },
            };
            let _ = tx.send(Msg::Done(result));
            ctx2.request_repaint();
        });
        let mut rate = Rate::new(Instant::now());
        if paused.load(Ordering::Relaxed) {
            rate.pause(Instant::now());
        }
        Job { spec, progress: Progress::default(), shown_at: None, latest: None, prompt: None, result: None, rate, paused_by: None, rx, reply: reply_tx, cancel, paused }
    }

    /// Drain messages from the worker. Returns true once the job is finished.
    pub fn poll(&mut self) -> bool {
        let now = Instant::now();
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Progress(p) => {
                    self.rate.sample(now, p.bytes_done, p.bytes_total);
                    let moving = matches!(self.spec.kind, JobKind::Transfer(OpKind::Move) | JobKind::Archive(FolderOp::Transfer(OpKind::Move)));
                    if !moving || self.shown_at.is_none_or(|at| now.saturating_duration_since(at) >= RATE_REFRESH) {
                        self.progress = p;
                        self.shown_at = Some(now);
                        self.latest = None;
                    } else {
                        self.progress.current = p.current.clone();
                        self.latest = Some(p);
                    }
                }
                Msg::Prompt(p) => {
                    // waiting for an answer doesn't count as transfer time
                    self.rate.pause(now);
                    self.prompt = Some(p);
                }
                Msg::Done(r) => {
                    // the end shows where it got to
                    if let Some(p) = self.latest.take() {
                        self.progress = p;
                    }
                    self.result = Some(r);
                }
            }
        }
        self.result.is_some()
    }

    /// The user answered the question: the operation goes on.
    pub fn answer(&mut self, r: Reply) {
        self.prompt = None;
        if !self.is_paused() {
            self.rate.resume(Instant::now());
        }
        let _ = self.reply.send(r);
    }

    /// Whether the operation is paused.
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Pause or resume (by the user: it is no longer waiting for another operation).
    pub fn set_paused(&mut self, paused: bool) {
        self.paused_by = None;
        self.paused.store(paused, Ordering::Relaxed);
        let now = Instant::now();
        if paused {
            self.rate.pause(now);
        } else if self.prompt.is_none() {
            self.rate.resume(now);
        }
    }

    /// Cancel the operation (a question waiting is answered with Cancel).
    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        // unblock a worker waiting on a prompt
        if let Some(p) = self.prompt.take() {
            let _ = self.reply.send(match p {
                Prompt::Conflict { .. } => Reply::Conflict(ConflictAction::Cancel),
                Prompt::Error { .. } => Reply::Error(ErrorAction::Cancel),
            });
        }
    }

    /// Fraction done: by bytes, or by files when there are no bytes to copy.
    pub fn fraction(&self) -> f32 {
        let p = &self.progress;
        let (a, b) = if p.bytes_total > 0 { (p.bytes_done, p.bytes_total) } else { (p.files_done, p.files_total) };
        if b == 0 {
            0.0
        } else {
            (a as f64 / b as f64).min(1.0) as f32
        }
    }
}

/// Running operations and the queue of waiting ones.
#[derive(Default)]
pub struct Jobs {
    pub running: Vec<Job>,
    pub queue: VecDeque<Spec>,
    /// The operations window is hidden ("run in background").
    pub hidden: bool,
    next_id: u64,
}

impl Jobs {
    /// Whether nothing runs and nothing is queued.
    pub fn is_idle(&self) -> bool {
        self.running.is_empty() && self.queue.is_empty()
    }

    /// [`Self::add_with`] for an operation of the program's own (tests).
    #[cfg(test)]
    pub fn add(&mut self, ctx: &egui::Context, kind: JobKind, sources: Vec<PathBuf>, target: PathBuf, place: Placement) {
        self.add_with(ctx, kind, sources, target, place, None);
    }

    /// Run an operation now, or queue it (`place`); `plugin` does it when
    /// it happens inside a plugin's file or connection.
    pub fn add_with(&mut self, ctx: &egui::Context, kind: JobKind, sources: Vec<PathBuf>, target: PathBuf, place: Placement, plugin: Option<PluginRef>) {
        self.next_id += 1;
        let spec = Spec { id: self.next_id, kind, sources, target, plugin };
        match place {
            Placement::Now => self.running.push(Job::start(ctx, spec)),
            Placement::First => self.run_first(ctx, spec),
            Placement::Last => self.queue.push_back(spec),
        }
        self.pump(ctx);
    }

    /// Pause everything running and start `spec`; the paused operations go on
    /// when it finishes (those the user paused stay paused).
    fn run_first(&mut self, ctx: &egui::Context, spec: Spec) {
        for j in self.running.iter_mut().filter(|j| j.spec.kind.can_pause()) {
            if !j.is_paused() {
                j.set_paused(true);
                j.paused_by = Some(spec.id);
            }
        }
        self.running.push(Job::start(ctx, spec));
    }

    /// Start the next waiting operation once nothing is running.
    fn pump(&mut self, ctx: &egui::Context) {
        if self.running.is_empty() {
            if let Some(spec) = self.queue.pop_front() {
                self.running.push(Job::start(ctx, spec));
            }
        }
    }

    /// Collect finished operations and start the next queued one.
    pub fn poll(&mut self, ctx: &egui::Context) -> Vec<Job> {
        let mut done = Vec::new();
        let mut i = 0;
        while i < self.running.len() {
            if self.running[i].poll() {
                done.push(self.running.remove(i));
            } else {
                i += 1;
            }
        }
        for d in &done {
            for j in &mut self.running {
                if j.paused_by == Some(d.spec.id) {
                    j.set_paused(false);
                }
            }
        }
        if !done.is_empty() {
            self.pump(ctx);
        }
        if self.is_idle() {
            self.hidden = false;
        }
        done
    }

    /// Start an operation that is paused from the beginning (tests and snapshots).
    #[cfg(test)]
    pub fn add_paused(&mut self, ctx: &egui::Context, kind: JobKind, sources: Vec<PathBuf>, target: PathBuf) {
        self.next_id += 1;
        let spec = Spec { id: self.next_id, kind, sources, target, plugin: None };
        self.running.push(Job::spawn(ctx, spec, true));
    }

    /// Start a queued operation now, without waiting.
    pub fn start_now(&mut self, ctx: &egui::Context, id: u64) {
        if let Some(i) = self.queue.iter().position(|s| s.id == id) {
            let spec = self.queue.remove(i).unwrap();
            self.running.push(Job::start(ctx, spec));
        }
    }

    /// Pause or resume every running operation, as if the user did it one by one.
    pub fn pause_all(&mut self, paused: bool) {
        for j in self.running.iter_mut().filter(|j| j.spec.kind.can_pause()) {
            j.set_paused(paused);
        }
    }

    /// Cancel everything: the running operations stop, the waiting ones are dropped.
    pub fn cancel_all(&mut self) {
        self.queue.clear();
        for j in &mut self.running {
            j.cancel();
        }
    }

    /// Take a queued operation out of the queue.
    pub fn remove_queued(&mut self, id: u64) {
        self.queue.retain(|s| s.id != id);
    }

    /// Move a queued operation one place up (towards the start) or down.
    /// Up from the first place puts it on top: it runs at once and the
    /// running operations pause until it finishes.
    pub fn move_queued(&mut self, ctx: &egui::Context, id: u64, up: bool) {
        if let Some(i) = self.queue.iter().position(|s| s.id == id) {
            if up && i == 0 {
                let spec = self.queue.remove(0).unwrap();
                self.run_first(ctx, spec);
                return;
            }
            let j = if up { i.checked_sub(1) } else { (i + 1 < self.queue.len()).then_some(i + 1) };
            if let Some(j) = j {
                self.queue.swap(i, j);
            }
        }
    }

    /// Progress of everything running, for the status bar: (fraction, time left).
    pub fn summary(&self) -> (f32, Option<Duration>) {
        let (mut done, mut total) = (0u64, 0u64);
        let mut left: Option<Duration> = None;
        for j in &self.running {
            done += j.progress.bytes_done;
            total += j.progress.bytes_total;
            if let Some(r) = j.rate.remaining() {
                left = Some(left.map_or(r, |l| l.max(r)));
            }
        }
        let frac = if total > 0 { done as f32 / total as f32 } else { self.running.first().map_or(0.0, Job::fraction) };
        (frac.min(1.0), left)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_and_estimate() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let total = 100_000_000;
        let mut r = Rate::new(t0);
        r.sample(at(0), 0, total);
        r.sample(at(500), 5_000_000, total);
        assert_eq!(r.speed(), None, "too little data");
        r.sample(at(2000), 20_000_000, total);
        assert_eq!(r.speed(), Some(10_000_000.0));
        assert_eq!(r.remaining(), Some(Duration::from_secs(8)));
        // within two seconds the numbers shown stay as they were
        r.sample(at(3000), 40_000_000, total);
        assert_eq!((r.speed(), r.remaining()), (Some(10_000_000.0), Some(Duration::from_secs(8))));
        r.sample(at(4000), 40_000_000, total);
        assert_eq!(r.speed(), Some(10_000_000.0), "worked out again after two seconds");
        assert_eq!(r.remaining(), Some(Duration::from_secs(6)));
        r.sample(at(4000), 20_000_000, total);
        // a pause neither counts as running time nor lowers the speed
        r.pause(at(2000));
        assert!(r.is_paused());
        r.resume(at(62_000));
        assert_eq!(r.elapsed(at(63_000)), Duration::from_secs(3));
        r.sample(at(63_000), 30_000_000, total);
        assert_eq!(r.speed(), Some(10_000_000.0), "the pause doesn't count");
        // old samples drop out of the window: the speed follows the recent rate
        for s in 1..=20u64 {
            r.sample(at(63_000 + s * 1000), 30_000_000 + s * 1_000_000, total);
        }
        let v = r.speed().unwrap();
        assert!((v - 1_000_000.0).abs() < 1.0, "{v}");
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(Duration::from_secs(42)), "0:42");
        assert_eq!(format_duration(Duration::from_secs(725)), "12:05");
        assert_eq!(format_duration(Duration::from_secs(3723)), "1:02:03");
    }

    #[test]
    fn queue_waits_for_running_operations() {
        let ctx = egui::Context::default();
        let t = tempfile::tempdir().unwrap();
        let (a, b, c) = (t.path().join("a"), t.path().join("b"), t.path().join("c"));
        for d in [&a, &b, &c] {
            std::fs::create_dir(d).unwrap();
        }
        std::fs::write(a.join("f.txt"), "x").unwrap();
        let mut jobs = Jobs::default();
        // nothing is running: a queued operation starts at once
        jobs.add(&ctx, JobKind::Transfer(OpKind::Copy), vec![a.join("f.txt")], b.clone(), Placement::Last);
        assert_eq!((jobs.running.len(), jobs.queue.len()), (1, 0));
        jobs.add(&ctx, JobKind::Delete, vec![a.join("f.txt")], PathBuf::new(), Placement::Last);
        jobs.add(&ctx, JobKind::Transfer(OpKind::Copy), vec![a.join("f.txt")], c.clone(), Placement::Last);
        assert_eq!(jobs.queue.iter().map(|s| s.kind).collect::<Vec<_>>(), [JobKind::Delete, JobKind::Transfer(OpKind::Copy)], "in the order added");
        // the delete must wait: copy to c first
        let delete = jobs.queue[0].id;
        jobs.move_queued(&ctx, delete, false);
        assert_eq!(jobs.queue[1].id, delete);
        let mut finished = Vec::new();
        for _ in 0..500 {
            finished.extend(jobs.poll(&ctx).into_iter().map(|j| j.spec.kind));
            if jobs.is_idle() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(jobs.is_idle());
        assert_eq!(finished, [JobKind::Transfer(OpKind::Copy), JobKind::Transfer(OpKind::Copy), JobKind::Delete], "one after another");
        assert!(b.join("f.txt").exists() && c.join("f.txt").exists() && !a.join("f.txt").exists());
    }

    /// "Run first": the running operation pauses while the new one runs and
    /// goes on afterwards. The running one here waits for a conflict answer,
    /// so it can't finish on its own meanwhile.
    #[test]
    fn run_first_pauses_the_others() {
        let ctx = egui::Context::default();
        let t = tempfile::tempdir().unwrap();
        let (a, b, c) = (t.path().join("a"), t.path().join("b"), t.path().join("c"));
        for d in [&a, &b, &c] {
            std::fs::create_dir(d).unwrap();
        }
        std::fs::write(a.join("f.txt"), "new").unwrap();
        std::fs::write(b.join("f.txt"), "old").unwrap();
        let copy = JobKind::Transfer(OpKind::Copy);
        let mut jobs = Jobs::default();
        jobs.add(&ctx, copy, vec![a.join("f.txt")], b.clone(), Placement::Now);
        let poll_until = |jobs: &mut Jobs, what: &dyn Fn(&Jobs) -> bool| {
            for _ in 0..500 {
                jobs.poll(&ctx);
                if what(jobs) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("timed out");
        };
        poll_until(&mut jobs, &|j| j.running[0].prompt.is_some());
        // a user-paused operation stays paused; only the others wait for the new one
        jobs.add_paused(&ctx, copy, vec![a.join("f.txt")], c.join("x"));
        jobs.add(&ctx, copy, vec![a.join("f.txt")], c.clone(), Placement::First);
        assert_eq!(jobs.running.len(), 3, "runs at once");
        let first = jobs.running[2].spec.id;
        assert!(jobs.running[0].is_paused() && jobs.running[0].paused_by == Some(first));
        assert_eq!(jobs.running[1].paused_by, None);
        poll_until(&mut jobs, &|j| j.running.len() == 2);
        assert!(c.join("f.txt").exists());
        assert!(!jobs.running[0].is_paused(), "goes on after the first one finished");
        assert!(jobs.running[1].is_paused(), "the user paused this one");
        // moving a waiting operation up past the top runs it the same way
        jobs.add(&ctx, JobKind::Delete, vec![c.join("f.txt")], PathBuf::new(), Placement::Last);
        let waiting = jobs.queue[0].id;
        jobs.move_queued(&ctx, waiting, true);
        assert!(jobs.queue.is_empty() && jobs.running.iter().any(|j| j.spec.id == waiting));
        assert_eq!(jobs.running[0].paused_by, Some(waiting));
        poll_until(&mut jobs, &|j| j.running.len() == 2);
        assert!(!c.join("f.txt").exists());
        for j in &mut jobs.running {
            j.cancel();
        }
        poll_until(&mut jobs, &|j| j.is_idle());
    }

    #[test]
    fn pause_all_and_cancel_all() {
        let ctx = egui::Context::default();
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        std::fs::write(a.join("f.txt"), "x").unwrap();
        let copy = JobKind::Transfer(OpKind::Copy);
        let mut jobs = Jobs::default();
        jobs.add_paused(&ctx, copy, vec![a.join("f.txt")], b.join("1"));
        jobs.add_paused(&ctx, copy, vec![a.join("f.txt")], b.join("2"));
        jobs.running[0].set_paused(false);
        jobs.pause_all(true);
        assert!(jobs.running.iter().all(Job::is_paused));
        jobs.pause_all(false);
        assert!(jobs.running.iter().all(|j| !j.is_paused()));
        jobs.pause_all(true);
        jobs.add(&ctx, copy, vec![a.join("f.txt")], b.join("3"), Placement::Last);
        assert_eq!(jobs.queue.len(), 1);
        jobs.cancel_all();
        assert!(jobs.queue.is_empty());
        for _ in 0..300 {
            jobs.poll(&ctx);
            if jobs.is_idle() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(jobs.is_idle(), "everything stopped, even the paused ones");
        assert!(!b.join("3").exists(), "the waiting one never ran");
    }

    #[test]
    fn pause_holds_the_copy() {
        let ctx = egui::Context::default();
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        std::fs::write(a.join("f.txt"), "x").unwrap();
        let mut jobs = Jobs::default();
        // paused before the first progress report: the file isn't copied
        let spec = Spec { id: 1, kind: JobKind::Transfer(OpKind::Copy), sources: vec![a.join("f.txt")], target: b.clone(), plugin: None };
        jobs.running.push(Job::spawn(&ctx, spec, true));
        std::thread::sleep(Duration::from_millis(300));
        assert!(jobs.poll(&ctx).is_empty());
        assert!(!b.join("f.txt").exists(), "paused");
        assert!(jobs.running[0].rate.is_paused());
        jobs.running[0].set_paused(false);
        for _ in 0..300 {
            if !jobs.poll(&ctx).is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(b.join("f.txt").exists());
    }
}
