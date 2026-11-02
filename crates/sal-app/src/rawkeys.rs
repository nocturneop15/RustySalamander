//! Key presses egui never sees as keys. egui-winit turns Ctrl+C, Ctrl+X,
//! Ctrl+V, Ctrl+Insert, Shift+Insert and Shift+Delete into its clipboard
//! events for text fields (`Copy`, `Cut`, `Paste` with text only) and drops
//! the key itself; with files on the clipboard Ctrl+V gives nothing at all.
//! The panels need them as keys (copy and paste files, Shift+Delete), so the
//! main window's messages are watched for them here.

use eframe::egui::{Key, Modifiers};

/// The keys egui-winit takes for its clipboard events (on Windows).
pub fn swallowed(key: Key, m: Modifiers) -> bool {
    (m.ctrl && matches!(key, Key::C | Key::X | Key::V | Key::Insert)) || (m.shift && matches!(key, Key::Insert | Key::Delete))
}

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;

    use eframe::egui::{Key, Modifiers};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_DELETE, VK_INSERT, VK_MENU, VK_SHIFT};
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN;

    thread_local! {
        static PRESSED: RefCell<Vec<(Key, Modifiers)>> = const { RefCell::new(Vec::new()) };
    }

    /// The key of a virtual-key code, for the keys egui-winit takes.
    fn key_of(vk: u16) -> Option<Key> {
        match vk {
            0x43 => Some(Key::C),
            0x58 => Some(Key::X),
            0x56 => Some(Key::V),
            v if v == VK_INSERT.0 => Some(Key::Insert),
            v if v == VK_DELETE.0 => Some(Key::Delete),
            _ => None,
        }
    }

    unsafe extern "system" fn subclass(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
        if msg == WM_KEYDOWN {
            if let Some(key) = key_of(wp.0 as u16) {
                let down = |vk: u16| GetKeyState(vk as i32) < 0;
                let m = Modifiers { ctrl: down(VK_CONTROL.0), shift: down(VK_SHIFT.0), alt: down(VK_MENU.0), command: down(VK_CONTROL.0), mac_cmd: false };
                if super::swallowed(key, m) {
                    PRESSED.with(|p| p.borrow_mut().push((key, m)));
                }
            }
        }
        // winit still gets every message
        DefSubclassProc(h, msg, wp, lp)
    }

    /// Watch the main window `h` for those keys.
    pub fn install(h: isize) {
        unsafe {
            let _ = SetWindowSubclass(HWND(h as *mut _), Some(subclass), 0x5A1C, 0);
        }
    }

    /// The keys pressed since the last call.
    pub fn take() -> Vec<(Key, Modifiers)> {
        PRESSED.with(|p| std::mem::take(&mut *p.borrow_mut()))
    }
}

#[cfg(windows)]
pub use imp::{install, take};

/// Elsewhere egui gets the keys itself.
#[cfg(not(windows))]
pub fn install(_: isize) {}

/// Elsewhere nothing is recorded.
#[cfg(not(windows))]
pub fn take() -> Vec<(Key, Modifiers)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keys_egui_takes() {
        let (ctrl, shift) = (Modifiers::CTRL, Modifiers::SHIFT);
        for (key, m) in [(Key::C, ctrl), (Key::X, ctrl), (Key::V, ctrl), (Key::C, ctrl | shift), (Key::Insert, ctrl), (Key::Insert, shift), (Key::Delete, shift)] {
            assert!(swallowed(key, m), "{key:?} {m:?}");
        }
        for (key, m) in [(Key::C, Modifiers::NONE), (Key::Delete, Modifiers::NONE), (Key::V, Modifiers::ALT), (Key::F5, ctrl)] {
            assert!(!swallowed(key, m), "{key:?} {m:?}");
        }
    }
}
