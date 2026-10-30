//! Dialogs in their own windows, which can be moved outside the main one.
//!
//! Each dialog is an egui immediate viewport: a real top-level window, owned
//! by the main window (it stays above it and minimizes with it), sized to its
//! content and themed like the main window. Backends with a single window
//! (the headless tests) show the same content as an `egui::Window` instead.

use std::collections::HashMap;

use eframe::egui::{self, Align2, Key, ViewportBuilder, ViewportClass, ViewportCommand, ViewportId};
use sal_core::config::ThemeMode;

/// Inner margin of a dialog window.
const MARGIN: f32 = 12.0;

/// State of one open dialog window.
pub struct Place {
    /// Tells apart a window kept open from one closed and opened again (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    serial: u64,
    /// Screen position chosen when it opened (later the user may move it).
    pos: Option<egui::Pos2>,
    /// Whether it opened as the active window; fixed for the window's life
    /// (changing it would make egui recreate the window).
    active: Option<bool>,
    theme: Option<ThemeMode>,
    /// The dialog's own window, once it is owned by the main window.
    hwnd: Option<isize>,
    flashed: bool,
    /// The main window is disabled while this (modal) dialog is open.
    disabled_owner: bool,
    /// Our other windows disabled while this (exclusive) dialog is open.
    disabled_others: Option<Vec<isize>>,
}

impl Place {
    fn new() -> Place {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Place {
            serial: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            pos: None,
            active: None,
            theme: None,
            hwnd: None,
            flashed: false,
            disabled_owner: false,
            disabled_others: None,
        }
    }
}

/// Open dialog windows by key ("mkdir", "settings", "job", …).
pub type Places = HashMap<&'static str, Place>;

/// What every dialog window needs from the app.
#[derive(Clone, Copy)]
pub struct DialogWin {
    pub owner: Option<isize>,
    pub theme: ThemeMode,
    /// The main window takes no input while the dialog is open.
    pub modal: bool,
    /// No other window of the program takes input while the dialog is open
    /// (a question the running operation waits for).
    pub exclusive: bool,
}

impl DialogWin {
    pub fn modal(self) -> DialogWin {
        DialogWin { modal: true, ..self }
    }

    /// Show `body` in the dialog window `key`, `width` points wide (narrower
    /// than the main window). Enter, Esc and closing the window are OR-ed into
    /// `enter` / `escape`.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        self,
        ctx: &egui::Context,
        places: &mut Places,
        key: &'static str,
        title: &str,
        width: f32,
        enter: &mut bool,
        escape: &mut bool,
        body: &mut dyn FnMut(&mut egui::Ui),
    ) {
        let main = ctx.input(|i| i.viewport().outer_rect).unwrap_or_else(|| ctx.screen_rect());
        let width = width.min(ctx.screen_rect().width() - 60.0).max(260.0);
        let place = places.entry(key).or_insert_with(Place::new);
        let pos = *place.pos.get_or_insert_with(|| egui::pos2(main.center().x - width / 2.0, main.top() + 120.0));
        // a dialog takes the focus only from our own windows; while the user
        // works in another program it opens behind and the taskbar flashes
        let active = *place.active.get_or_insert_with(|| self.owner.is_none_or(|_| our_app_is_foreground()));
        let builder = ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([width, 160.0])
            .with_position(pos)
            .with_resizable(false)
            .with_minimize_button(false)
            .with_maximize_button(false)
            .with_taskbar(false)
            .with_active(active);
        let id = ViewportId::from_hash_of(("dialog", key));
        ctx.show_viewport_immediate(id, builder, |vctx, class| {
            // keys typed into an open drop-down (font search) are not meant for the dialog
            let popup = vctx.memory(|m| m.any_popup_open());
            let (e, x, closed) = vctx.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape), i.viewport().close_requested()));
            if !popup {
                *enter |= e;
                *escape |= x;
            }
            *escape |= closed;
            if class == ViewportClass::Embedded {
                // auto_sized: without it a non-resizable window keeps the size it had
                // in its first frames and clips content that grows afterwards
                egui::Window::new(title)
                    .id(egui::Id::new(("dialog", key)))
                    .collapsible(false)
                    .auto_sized()
                    .anchor(Align2::CENTER_TOP, [0.0, 140.0])
                    .min_width(width)
                    .max_width(width)
                    .show(vctx, |ui| body(ui));
                return;
            }
            let frame = egui::Frame::central_panel(&vctx.style()).inner_margin(MARGIN);
            let height = egui::CentralPanel::default().frame(frame).show(vctx, |ui| {
                // the panel itself always fills the window: measure a scope inside it
                ui.scope(|ui| body(ui)).response.rect.height()
            });
            // fit the window to the content
            let want = egui::vec2(width, (height.inner + 2.0 * MARGIN).ceil());
            let have = vctx.input(|i| i.viewport().inner_rect).map(|r| r.size());
            let fits = have.is_some_and(|h| (h.y - want.y).abs() <= 2.0 && (h.x - want.x).abs() <= 2.0);
            if !fits {
                vctx.send_viewport_cmd(ViewportCommand::InnerSize(want));
            }
            if place.theme != Some(self.theme) {
                place.theme = Some(self.theme);
                vctx.send_viewport_cmd(ViewportCommand::SetTheme(system_theme(self.theme)));
            }
            if let Some(owner) = self.owner {
                if place.hwnd.is_none() {
                    place.hwnd = set_owner(owner, title);
                }
                if place.hwnd.is_some() && !active && !place.flashed {
                    place.flashed = true;
                    flash(owner);
                }
                // only once the dialog exists: a disabled main window without
                // its dialog would be stuck
                if self.modal && place.hwnd.is_some() && !place.disabled_owner {
                    place.disabled_owner = true;
                    enable(owner, false);
                }
                // every frame: windows opened meanwhile (the operations window
                // shown in the same frame) are disabled as well
                if let (true, Some(own)) = (self.exclusive, place.hwnd) {
                    place.disabled_others.get_or_insert_with(Vec::new).extend(disable_others(own));
                }
            }
        });
    }
}

/// Disable this thread's other enabled top-level windows (the main window,
/// the operations window, other dialogs, viewers); returns them for enabling
/// again. Windows that were disabled already stay as they are.
#[cfg(windows)]
fn disable_others(keep: isize) -> Vec<isize> {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled};
    use windows::Win32::UI::WindowsAndMessaging::{EnumThreadWindows, IsWindowVisible};

    struct Search {
        keep: isize,
        found: Vec<isize>,
    }
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
        let s = &mut *(l.0 as *mut Search);
        if h.0 as isize != s.keep && IsWindowVisible(h).as_bool() && IsWindowEnabled(h).as_bool() {
            let _ = EnableWindow(h, false);
            s.found.push(h.0 as isize);
        }
        BOOL(1)
    }
    let mut s = Search { keep, found: Vec::new() };
    unsafe {
        let _ = EnumThreadWindows(GetCurrentThreadId(), Some(each), LPARAM(&mut s as *mut Search as isize));
    }
    s.found
}

#[cfg(not(windows))]
fn disable_others(_: isize) -> Vec<isize> {
    Vec::new()
}

/// Bring an open dialog window to the front, when we have the foreground
/// (e.g. after a click in the main window).
pub fn bring_to_front(places: &Places, key: &str) {
    if let Some(h) = places.get(key).and_then(|p| p.hwnd) {
        if our_app_is_foreground() {
            set_foreground(h);
        }
    }
}

/// Identity of the open dialog window `key` (tests: was it kept or reopened?).
#[cfg(test)]
pub fn serial(places: &Places, key: &str) -> Option<u64> {
    places.get(key).map(|p| p.serial)
}

/// Forget the dialog windows `which` selects (they close as they are no longer
/// shown) and give the focus back to the main window. Windows would otherwise
/// activate whatever window comes next, often another application.
pub fn close(places: &mut Places, owner: Option<isize>, which: impl Fn(&str) -> bool) {
    let keys: Vec<&'static str> = places.keys().copied().filter(|k| which(k)).collect();
    if keys.is_empty() {
        return;
    }
    let mut enable_owner = false;
    for k in keys {
        if let Some(p) = places.remove(k) {
            enable_owner |= p.disabled_owner;
            for h in p.disabled_others.unwrap_or_default() {
                enable(h, true);
            }
        }
    }
    if let Some(owner) = owner {
        // re-enabled before the dialog goes away, or Windows activates another app
        if enable_owner {
            enable(owner, true);
        }
        activate(owner);
    }
}

#[cfg(windows)]
fn enable(owner: isize, on: bool) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
    unsafe {
        let _ = EnableWindow(HWND(owner as *mut _), on);
    }
}

#[cfg(not(windows))]
fn enable(_: isize, _: bool) {}

/// Bring the main window to the front, but only while one of our windows is
/// in the foreground: a job finishing in the background must not steal it.
fn activate(owner: isize) {
    if our_app_is_foreground() {
        set_foreground(owner);
    }
}

/// Whether the foreground window belongs to this process. Focus changes use
/// plain SetForegroundWindow, never egui's Focus command: winit implements
/// that by injecting an Alt key press, which takes the focus from other programs.
#[cfg(windows)]
fn our_app_is_foreground() -> bool {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
        pid == GetCurrentProcessId()
    }
}

#[cfg(windows)]
fn set_foreground(h: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
    unsafe {
        let _ = SetForegroundWindow(HWND(h as *mut _));
    }
}

/// Flash the main window's taskbar button until the user switches to us.
#[cfg(windows)]
fn flash(owner: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{FlashWindowEx, FLASHWINFO, FLASHW_TIMERNOFG, FLASHW_TRAY};
    let info = FLASHWINFO { cbSize: std::mem::size_of::<FLASHWINFO>() as u32, hwnd: HWND(owner as *mut _), dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG, uCount: 0, dwTimeout: 0 };
    unsafe {
        let _ = FlashWindowEx(&info);
    }
}

#[cfg(not(windows))]
fn our_app_is_foreground() -> bool {
    true
}

#[cfg(not(windows))]
fn set_foreground(_: isize) {}

#[cfg(not(windows))]
fn flash(_: isize) {}

pub fn system_theme(mode: ThemeMode) -> egui::SystemTheme {
    match mode {
        ThemeMode::System => egui::SystemTheme::SystemDefault,
        ThemeMode::Light => egui::SystemTheme::Light,
        ThemeMode::Dark => egui::SystemTheme::Dark,
    }
}

/// Make this thread's top-level window titled `title` owned by `owner`: it
/// stays above the main window and is minimized with it.
#[cfg(windows)]
fn set_owner(owner: isize, title: &str) -> Option<isize> {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{EnumThreadWindows, GetWindowTextW, SetWindowLongPtrW, GWLP_HWNDPARENT};

    struct Search {
        title: Vec<u16>,
        owner: isize,
        found: Option<isize>,
    }
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
        let s = &mut *(l.0 as *mut Search);
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(h, &mut buf).max(0) as usize;
        if h.0 as isize != s.owner && buf[..n] == s.title[..] {
            SetWindowLongPtrW(h, GWLP_HWNDPARENT, s.owner);
            s.found = Some(h.0 as isize);
            return BOOL(0);
        }
        BOOL(1)
    }
    let mut s = Search { title: title.encode_utf16().collect(), owner, found: None };
    unsafe {
        let _ = EnumThreadWindows(GetCurrentThreadId(), Some(each), LPARAM(&mut s as *mut Search as isize));
    }
    s.found
}

#[cfg(not(windows))]
fn set_owner(_: isize, _: &str) -> Option<isize> {
    Some(0)
}
