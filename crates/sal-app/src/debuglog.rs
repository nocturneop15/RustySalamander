//! The program's log, off unless asked for.
//!
//! - **`-debug`** on the command line: the full debug log, for finding out
//!   what went wrong. It goes to `rustysalamander-debug-<date>-<time>.log`
//!   next to the program (in the temporary folder when that can't be
//!   written); `-debug=<file>` names the file. It has the computer (Windows,
//!   graphics adapters, Remote Desktop, OpenGL), every user action and what
//!   came of it, the messages of the libraries (window, OpenGL), panics, and
//!   the timing below.
//! - **`RUSTYSALAMANDER_LOG`** set to a file path, or to `1` for
//!   `rustysalamander.log` in the temporary folder: only the timing (startup
//!   steps, slow frames).
//!
//! Each line has the time since the start. In the debug log, what the user
//! did and what came of it are marked and numbered, so each result can be
//! told by the action it belongs to:
//!
//! ```text
//! [  5120.3 ms] >>> #12 USER    key Ctrl+C
//! [  5120.4 ms]     #12 DO      command ClipboardPut(false)
//! [  5121.0 ms] <<< #12 RESULT  status: "2 items put on the clipboard"
//! [  6311.9 ms] ··· after #12   left folder: C:\Windows
//! [  7002.2 ms]     LIB         winit DEBUG: …
//! ```
//!
//! `RESULT` is what changed in the frame of the action; `after #N` is what
//! changed later by itself (a folder read in the background); `at start`
//! is how things were before the first action. A finished operation names
//! the action that started it.

use std::collections::HashMap;
use std::fmt::Display;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

struct Log {
    start: Instant,
    file: Option<Mutex<std::fs::File>>,
    /// The full debug log (`-debug`), not only the timing.
    full: bool,
    path: Option<PathBuf>,
}

static LOG: OnceLock<Log> = OnceLock::new();
/// The number of the last user action.
static ACTION: AtomicU64 = AtomicU64::new(0);
/// A user action came in the current frame.
static ACTED: AtomicBool = AtomicBool::new(false);
/// Operations (by id) and the action that started them.
static JOBS: Mutex<Option<HashMap<u64, u64>>> = Mutex::new(None);

/// What the command line asks for: `-debug` (also `--debug`, any case),
/// `-debug=<file>`; the other arguments are given back.
pub fn parse_args(args: Vec<String>) -> (Option<Option<PathBuf>>, Vec<String>) {
    let mut debug = None;
    let mut rest = Vec::new();
    for a in args {
        let lower = a.to_lowercase();
        let flag = lower.strip_prefix("--").or_else(|| lower.strip_prefix('-')).unwrap_or("");
        if flag == "debug" {
            debug = Some(None);
        } else if flag.starts_with("debug=") || flag.starts_with("debug:") {
            let at = a.find(['=', ':']).unwrap() + 1;
            debug = Some(Some(PathBuf::from(&a[at..])));
        } else {
            rest.push(a);
        }
    }
    (debug, rest)
}

fn log() -> &'static Log {
    LOG.get_or_init(|| open(None))
}

/// The log file: the debug log when asked for (`debug`), else the timing
/// log of `RUSTYSALAMANDER_LOG`, else none.
fn open(debug: Option<Option<PathBuf>>) -> Log {
    let start = Instant::now();
    let create = |p: &Path| std::fs::OpenOptions::new().create(true).append(true).open(p).ok();
    if let Some(named) = debug {
        let name = format!("rustysalamander-debug-{}.log", chrono::Local::now().format("%Y%m%d-%H%M%S"));
        let beside = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(&name)));
        let tries = match named {
            Some(p) => vec![p, std::env::temp_dir().join(&name)],
            None => beside.into_iter().chain([std::env::temp_dir().join(&name)]).collect(),
        };
        for p in tries {
            if let Some(f) = create(&p) {
                return Log { start, file: Some(Mutex::new(f)), full: true, path: Some(p) };
            }
        }
        return Log { start, file: None, full: false, path: None };
    }
    let file = std::env::var_os("RUSTYSALAMANDER_LOG").filter(|v| !v.is_empty()).and_then(|v| {
        let path = if v == "1" { std::env::temp_dir().join("rustysalamander.log") } else { PathBuf::from(v) };
        create(&path).map(|f| (f, path))
    });
    match file {
        Some((f, p)) => Log { start, file: Some(Mutex::new(f)), full: false, path: Some(p) },
        None => Log { start, file: None, full: false, path: None },
    }
}

/// Start the clock and open the log (call first thing in `main`, with what
/// [`parse_args`] found), and note how the program was started.
pub fn init(debug: Option<Option<PathBuf>>, args: &[String]) {
    let _ = LOG.set(open(debug));
    if !enabled() {
        return;
    }
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    write(format!("---- RustySalamander {} started {}: {exe} (working folder {cwd})", env!("CARGO_PKG_VERSION"), chrono::Local::now().format("%Y-%m-%d %H:%M:%S")));
    if !full() {
        return;
    }
    write(format!("     debug log: {}", path().map(|p| p.display().to_string()).unwrap_or_default()));
    write(format!("     arguments: {args:?}"));
    for line in system::describe() {
        write(format!("     {line}"));
    }
    let _ = log::set_logger(&Bridge);
    log::set_max_level(log::LevelFilter::Debug);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current().name().unwrap_or("?").to_owned();
        line("!!!", None, "PANIC", format!("in thread {thread}: {info}\n{}", std::backtrace::Backtrace::force_capture()));
        previous(info);
    }));
}

/// Whether the log is on.
pub fn enabled() -> bool {
    log().file.is_some()
}

/// Whether the full debug log is on (`-debug`).
pub fn full() -> bool {
    log().full
}

/// The log file, when there is one.
pub fn path() -> Option<&'static Path> {
    log().path.as_deref()
}

/// Write a line.
pub fn write(msg: impl Display) {
    let l = log();
    if let Some(f) = &l.file {
        let ms = l.start.elapsed().as_secs_f64() * 1000.0;
        let mut f = f.lock().unwrap_or_else(|e| e.into_inner());
        let msg = msg.to_string();
        let _ = if msg.is_empty() { writeln!(f) } else { writeln!(f, "[{ms:10.1} ms]  {msg}") };
        let _ = f.flush();
    }
}

/// A marked line of the debug log: `mark` in front, the action number,
/// the kind (`USER`, `DO`, …) in a column of its own.
fn line(mark: &str, action: Option<u64>, kind: &str, msg: impl Display) {
    let n = action.map(|n| format!("#{n}")).unwrap_or_default();
    write(format!("{mark:3} {n:>4} {kind:<7} {msg}"));
}

/// The user did something (a key, typed text, a click): a new action.
pub fn user(what: impl Display) {
    if full() {
        let n = ACTION.fetch_add(1, Ordering::Relaxed) + 1;
        ACTED.store(true, Ordering::Relaxed);
        write("");
        line(">>>", Some(n), "USER", what);
    }
}

/// More of the same user action (the text a key typed).
fn user_more(what: impl Display) {
    line(">>>", Some(ACTION.load(Ordering::Relaxed)), "USER", what);
}

/// What the program does for the current action (a command, an
/// operation started, a dialog confirmed).
pub fn did(what: impl Display) {
    if full() {
        line("", Some(ACTION.load(Ordering::Relaxed)), "DO", what);
    }
}

/// Something that changed: a result of an action in this frame, or a change
/// by itself after it.
pub fn changed(what: impl Display) {
    if !full() {
        return;
    }
    let n = ACTION.load(Ordering::Relaxed);
    if ACTED.load(Ordering::Relaxed) {
        line("<<<", Some(n), "RESULT", what);
    } else if n == 0 {
        write(format!("··· at start   {what}"));
    } else {
        write(format!("··· after #{n:<3}  {what}"));
    }
}

/// The frame ended: what changes from now on is no result of its actions.
pub fn end_frame() {
    ACTED.store(false, Ordering::Relaxed);
}

/// An error worth seeing in the log.
pub fn error(what: impl Display) {
    if enabled() {
        line("!!!", None, "ERROR", what);
    }
}

/// Operation `id` was started by the current action.
pub fn job_started(id: u64, what: impl Display) {
    if full() {
        let n = ACTION.load(Ordering::Relaxed);
        JOBS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(id, n);
        line("", Some(n), "DO", format!("operation {id} started: {what}"));
    }
}

/// Operation `id` ended: its result, under the action that started it.
pub fn job_finished(id: u64, what: impl Display) {
    if full() {
        let n = JOBS.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|m| m.remove(&id));
        line("<<<", n, "RESULT", format!("operation {id} finished: {what}"));
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

/// A duration in milliseconds, for the log.
pub fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

/// The messages of the libraries (eframe, winit, glutin, egui_glow) into the
/// debug log.
struct Bridge;

impl log::Log for Bridge {
    fn enabled(&self, m: &log::Metadata) -> bool {
        // the graphics libraries tell a lot at debug level
        let quiet = ["wgpu", "naga"].iter().any(|t| m.target().starts_with(t));
        m.level() <= if quiet { log::Level::Info } else { log::Level::Debug }
    }

    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            line("", None, "LIB", format!("{} {}: {}", r.target(), r.level(), r.args()));
        }
    }

    fn flush(&self) {}
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
    /// Frame `n` begins now.
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

    /// The frame ended: log its parts when it took long.
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

/// Input events as user actions: keys, typed text, clicks, the window
/// closed. `place` says where (`""` for the main window); `secret` logs
/// only how much was typed, not what (a dialog may ask for a password);
/// `at` names what is under a click.
pub fn input(events: &[eframe::egui::Event], place: &str, secret: bool, at: &dyn Fn(eframe::egui::Pos2) -> String) {
    use eframe::egui::Event;
    if !full() {
        return;
    }
    let place = if place.is_empty() { String::new() } else { format!("  [{place}]") };
    // the text a key typed belongs to that key's action
    let mut after_key = false;
    for e in events {
        let key = matches!(e, Event::Key { pressed: true, .. });
        let text = matches!(e, Event::Text(_));
        let what = match e {
            Event::Key { key, pressed: true, repeat, modifiers, .. } => format!("key {}{}{}", mods(modifiers), key.name(), if *repeat { " (held)" } else { "" }),
            Event::Text(t) if secret => format!("typed {} characters", t.chars().count()),
            Event::Text(t) => format!("typed {t:?}"),
            Event::Copy => "copy (Ctrl+C)".into(),
            Event::Cut => "cut (Ctrl+X)".into(),
            Event::Paste(t) => format!("paste ({} characters)", t.chars().count()),
            Event::PointerButton { pos, button, pressed: true, modifiers } => {
                let on = at(*pos);
                format!("click {}{button:?} at ({:.0}, {:.0}){}", mods(modifiers), pos.x, pos.y, if on.is_empty() { String::new() } else { format!(" on {on}") })
            }
            _ => continue,
        };
        if text && after_key {
            user_more(format!("{what}{place}"));
        } else {
            user(format!("{what}{place}"));
        }
        after_key = key;
    }
}

/// "Ctrl+Shift+" for the modifiers held.
fn mods(m: &eframe::egui::Modifiers) -> String {
    let mut s = String::new();
    for (on, name) in [(m.ctrl, "Ctrl+"), (m.shift, "Shift+"), (m.alt, "Alt+")] {
        if on {
            s.push_str(name);
        }
    }
    s
}

/// What changed between two looks at the program's state (name, value),
/// logged as results.
#[derive(Default)]
pub struct Watch {
    last: HashMap<String, String>,
}

impl Watch {
    /// Log the values that differ from the last look, and those gone.
    pub fn look(&mut self, now: Vec<(String, String)>) {
        let mut next = HashMap::with_capacity(now.len());
        for (name, value) in now {
            if self.last.remove(&name).as_ref() != Some(&value) {
                if value.contains('\n') {
                    changed(format!("{name}:\n        {}", value.replace('\n', "\n        ")));
                } else {
                    changed(format!("{name}: {value}"));
                }
            }
            next.insert(name, value);
        }
        let mut gone: Vec<&String> = self.last.keys().collect();
        gone.sort();
        for name in gone {
            changed(format!("{name}: (none)"));
        }
        self.last = next;
    }
}

/// The computer, for the head of the debug log.
mod system {
    /// Windows, Remote Desktop, the graphics adapters.
    #[cfg(windows)]
    pub fn describe() -> Vec<String> {
        use windows::core::PCWSTR;
        use windows::Win32::Graphics::Gdi::{EnumDisplayDevicesW, DISPLAY_DEVICEW};
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS, SM_REMOTESESSION};
        let mut out = vec![format!("Windows: {}", windows_version())];
        let remote = unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0;
        out.push(format!("session: {}, monitors: {}", if remote { "Remote Desktop" } else { "local" }, unsafe { GetSystemMetrics(SM_CMONITORS) }));
        let mut seen = Vec::new();
        for i in 0.. {
            let mut d = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
            if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), i, &mut d, 0) }.as_bool() {
                break;
            }
            let name = String::from_utf16_lossy(&d.DeviceString).trim_end_matches('\0').to_owned();
            let flags = d.StateFlags;
            // DISPLAY_DEVICE_ATTACHED_TO_DESKTOP = 1, PRIMARY = 4
            let state = match (flags & 1 != 0, flags & 4 != 0) {
                (true, true) => "in use, primary",
                (true, false) => "in use",
                _ => "not in use",
            };
            let line = format!("display adapter: {name} ({state})");
            if !seen.contains(&line) {
                seen.push(line.clone());
                out.push(line);
            }
        }
        out
    }

    #[cfg(not(windows))]
    pub fn describe() -> Vec<String> {
        vec![format!("system: {}", std::env::consts::OS)]
    }

    /// "Windows 11 Pro 24H2 (build 26100.1150)" from the registry.
    #[cfg(windows)]
    fn windows_version() -> String {
        use windows::core::HSTRING;
        use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};
        let key = HSTRING::from(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
        let text = |name: &str| {
            let mut buf = [0u16; 256];
            let mut size = (buf.len() * 2) as u32;
            let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, &key, &HSTRING::from(name), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut size)) };
            r.is_ok().then(|| String::from_utf16_lossy(&buf[..(size as usize / 2).saturating_sub(1)]))
        };
        let number = |name: &str| {
            let mut v = 0u32;
            let mut size = 4u32;
            let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, &key, &HSTRING::from(name), RRF_RT_REG_DWORD, None, Some((&mut v as *mut u32).cast()), Some(&mut size)) };
            r.is_ok().then_some(v)
        };
        let build = text("CurrentBuild").unwrap_or_default();
        // Windows 11 still says "Windows 10" in ProductName
        let product = text("ProductName").unwrap_or_default();
        let product = if build.parse::<u32>().is_ok_and(|b| b >= 22000) { product.replacen("Windows 10", "Windows 11", 1) } else { product };
        let ubr = number("UBR").map(|u| format!(".{u}")).unwrap_or_default();
        format!("{product} {} (build {build}{ubr})", text("DisplayVersion").unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_on_the_command_line() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_args(args(&["C:\\", "-debug", "D:\\"])), (Some(None), args(&["C:\\", "D:\\"])));
        assert_eq!(parse_args(args(&["--DEBUG"])), (Some(None), vec![]));
        assert_eq!(parse_args(args(&["-debug=C:\\Logs\\a.log"])), (Some(Some(PathBuf::from("C:\\Logs\\a.log"))), vec![]));
        assert_eq!(parse_args(args(&["C:\\debug"])), (None, args(&["C:\\debug"])));
    }
}
