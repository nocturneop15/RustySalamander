//! A timing log for finding out what makes the program slow, off unless
//! asked for: set `RUSTYSALAMANDER_LOG` to a file path, or to `1` for
//! `rustysalamander.log` in the temporary folder. Each line has the time
//! since the program started:
//!
//! ```text
//! [   312.4 ms]  startup: plugins (186.0 ms)
//! [  4021.7 ms]  slow frame 3: 3504.2 ms  (panel 1: 3490.1 ms, menu: 2.1 ms)
//! ```
//!
//! Startup steps are always logged; frames only when they take long, with
//! the time of their parts.

use std::fmt::Display;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

struct Log {
    start: Instant,
    file: Option<Mutex<std::fs::File>>,
}

static LOG: OnceLock<Log> = OnceLock::new();

fn log() -> &'static Log {
    LOG.get_or_init(|| {
        let file = std::env::var_os("RUSTYSALAMANDER_LOG").filter(|v| !v.is_empty()).and_then(|v| {
            let path = if v == "1" { std::env::temp_dir().join("rustysalamander.log") } else { PathBuf::from(v) };
            std::fs::OpenOptions::new().create(true).append(true).open(path).ok().map(Mutex::new)
        });
        Log { start: Instant::now(), file }
    })
}

/// Start the clock (call first thing in `main`) and note how the program
/// was started.
pub fn init() {
    if enabled() {
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
        let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
        write(format!("---- RustySalamander {} started: {exe} (working folder {cwd})", env!("CARGO_PKG_VERSION")));
    }
}

/// Whether the log is on.
pub fn enabled() -> bool {
    log().file.is_some()
}

/// Write a line.
pub fn write(msg: impl Display) {
    let l = log();
    if let Some(f) = &l.file {
        let ms = l.start.elapsed().as_secs_f64() * 1000.0;
        let mut f = f.lock().unwrap();
        let _ = writeln!(f, "[{ms:10.1} ms]  {msg}");
        let _ = f.flush();
    }
}

/// Run `f` and log how long it took under `label`.
pub fn time<R>(label: &str, f: impl FnOnce() -> R) -> R {
    if !enabled() {
        return f();
    }
    let t = Instant::now();
    let r = f();
    write(format!("{label} ({})", ms(t.elapsed())));
    r
}

pub fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

/// The parts of one frame and how long each took; logged when the frame was
/// slow, and for the first frames after the start.
pub struct Frame {
    n: u64,
    start: Instant,
    last: Instant,
    parts: Vec<(&'static str, Duration)>,
}

impl Frame {
    pub fn start(n: u64) -> Frame {
        let now = Instant::now();
        Frame { n, start: now, last: now, parts: Vec::new() }
    }

    /// The part since the previous mark took until now.
    pub fn mark(&mut self, part: &'static str) {
        if enabled() {
            let now = Instant::now();
            self.parts.push((part, now - self.last));
            self.last = now;
        }
    }

    pub fn finish(self) {
        if !enabled() {
            return;
        }
        let total = self.start.elapsed();
        if self.n < 5 || total > Duration::from_millis(150) {
            let parts: Vec<String> = self.parts.iter().filter(|(_, d)| *d > Duration::from_millis(1)).map(|(p, d)| format!("{p}: {}", ms(*d))).collect();
            let kind = if total > Duration::from_millis(150) { "slow frame" } else { "frame" };
            write(format!("{kind} {}: {}  ({})", self.n, ms(total), parts.join(", ")));
        }
    }
}
