//! The program's windows as Windows sees them: finding one, and switching off
//! the animation Windows plays when it shows or closes one.

/// The window of this thread titled `title`, shown or not yet.
#[cfg(windows)]
pub fn thread_window(title: &str) -> Option<isize> {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{EnumThreadWindows, GetWindowTextW};

    struct Search {
        title: Vec<u16>,
        found: Option<isize>,
    }
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
        let s = &mut *(l.0 as *mut Search);
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(h, &mut buf).max(0) as usize;
        if buf[..n] == s.title[..] {
            s.found = Some(h.0 as isize);
            return BOOL(0);
        }
        BOOL(1)
    }
    let mut s = Search { title: title.encode_utf16().collect(), found: None };
    unsafe {
        let _ = EnumThreadWindows(GetCurrentThreadId(), Some(each), LPARAM(&mut s as *mut Search as isize));
    }
    s.found
}

/// Elsewhere there is nothing to find.
#[cfg(not(windows))]
pub fn thread_window(_: &str) -> Option<isize> {
    None
}

/// Whether Windows animates the window `hwnd` when it shows, closes or
/// minimizes it. Set it before the window is first shown.
#[cfg(windows)]
pub fn set_transitions(hwnd: isize, on: bool) {
    use windows::Win32::Foundation::{BOOL, HWND};
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED};
    let off = BOOL::from(!on);
    unsafe {
        let _ = DwmSetWindowAttribute(HWND(hwnd as *mut _), DWMWA_TRANSITIONS_FORCEDISABLED, &off as *const BOOL as *const _, std::mem::size_of::<BOOL>() as u32);
    }
}

/// Elsewhere windows aren't animated by the system here.
#[cfg(not(windows))]
pub fn set_transitions(_: isize, _: bool) {}

/// Put the window `hwnd` (not shown yet) over the window `owner`: centred on
/// it, moved down and right by `step` cascade steps, and kept inside the work
/// area of the monitor `owner` is on. So a plugin's window opens on the
/// program's monitor, not where Windows would put it.
#[cfg(windows)]
pub fn place_over(hwnd: isize, owner: isize, step: usize) {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsWindow, SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER};
    let (h, o) = (HWND(hwnd as *mut _), HWND(owner as *mut _));
    unsafe {
        if !IsWindow(o).as_bool() {
            return;
        }
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(MonitorFromWindow(o, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() {
            return;
        }
        let work = info.rcWork;
        // twice: the first move may take the window to a monitor of another
        // scale, which resizes it; the second centres it at its new size
        for _ in 0..2 {
            let (mut own, mut win) = (RECT::default(), RECT::default());
            if GetWindowRect(o, &mut own).is_err() || GetWindowRect(h, &mut win).is_err() {
                return;
            }
            let (w, ht) = (win.right - win.left, win.bottom - win.top);
            let shift = (step % 8) as i32 * 30 * dpi_of(o) as i32 / 96;
            let x = (own.left + own.right - w) / 2 + shift;
            let y = (own.top + own.bottom - ht) / 2 + shift;
            // inside the work area; a window larger than it starts at its corner
            let x = x.min(work.right - w).max(work.left);
            let y = y.min(work.bottom - ht).max(work.top);
            let _ = SetWindowPos(h, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }
}

/// The DPI of the window `hwnd`: `GetDpiForWindow` where Windows has it
/// (10 and later), looked up when needed, so the program still starts on
/// Windows 8.1 / Server 2012 R2; there, the screen's.
#[cfg(windows)]
fn dpi_of(hwnd: windows::Win32::Foundation::HWND) -> u32 {
    use std::sync::OnceLock;
    use windows::core::s;
    use windows::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, ReleaseDC, LOGPIXELSX};
    use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
    type Fn = unsafe extern "system" fn(windows::Win32::Foundation::HWND) -> u32;
    static FOR_WINDOW: OnceLock<Option<Fn>> = OnceLock::new();
    let f = FOR_WINDOW.get_or_init(|| unsafe {
        let user32 = GetModuleHandleA(s!("user32.dll")).ok()?;
        GetProcAddress(user32, s!("GetDpiForWindow")).map(|p| std::mem::transmute::<_, Fn>(p))
    });
    let dpi = unsafe {
        match f {
            Some(f) => f(hwnd),
            None => {
                let dc = GetDC(None);
                let d = GetDeviceCaps(dc, LOGPIXELSX);
                ReleaseDC(None, dc);
                d.max(0) as u32
            }
        }
    };
    dpi.max(96)
}

/// Elsewhere the system places windows.
#[cfg(not(windows))]
pub fn place_over(_: isize, _: isize, _: usize) {}
