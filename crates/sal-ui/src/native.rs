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
