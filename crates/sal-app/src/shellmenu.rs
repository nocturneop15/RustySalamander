//! The Windows shell context menu (IContextMenu): the classic, full menu
//! with all verbs and third-party extensions, plus invoking single verbs
//! ("properties", "openas", "cut", "copy", "paste") without showing it.

use std::path::{Path, PathBuf};

/// What the user picked in the classic menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    Nothing,
    /// The shell ran the command itself.
    Invoked(String),
    /// Verbs the panel performs itself (the shell can't rename in our view).
    Rename,
    Delete,
}

/// A row of a native menu: `Some((id, label, shortcut, enabled))`, `None` for a separator.
pub type NativeRow = Option<(u32, String, String, bool)>;

/// What was picked in the merged menu ([`show_merged`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merged {
    /// One of our own rows, by id.
    Ours(u32),
    /// A shell item, or nothing.
    Shell(Picked),
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::cell::RefCell;

    use windows::core::{Interface, PCSTR, PCWSTR, PSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::DataExchange::IsClipboardFormatAvailable;
    use windows::Win32::System::Ole::CF_HDROP;
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        DefSubclassProc, IContextMenu, IContextMenu2, IContextMenu3, IShellFolder, RemoveWindowSubclass, SHBindToParent, SHParseDisplayName, SetWindowSubclass, CMF_CANRENAME,
        CMF_EXTENDEDVERBS, CMF_NORMAL, CMIC_MASK_PTINVOKE, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, GCS_VERBW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreatePopupMenu, DestroyMenu, GetCursorPos, SetForegroundWindow, TrackPopupMenuEx, HMENU, SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_DRAWITEM, WM_INITMENUPOPUP,
        WM_MEASUREITEM, WM_MENUCHAR,
    };

    const CMIC_MASK_UNICODE: u32 = 0x4000;
    const FIRST_ID: u32 = 1;
    const LAST_ID: u32 = 0x7FFF;

    /// A window handle as Windows takes it.
    fn hwnd(h: isize) -> HWND {
        HWND(h as *mut core::ffi::c_void)
    }

    /// A NUL-terminated UTF-16 string.
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Absolute PIDLs, freed on drop.
    struct Pidls(Vec<*mut ITEMIDLIST>);

    impl Drop for Pidls {
        fn drop(&mut self) {
            for p in &self.0 {
                unsafe { CoTaskMemFree(Some(*p as *const _)) };
            }
        }
    }

    /// Context menu for items that share one parent folder.
    fn context_menu(owner: HWND, items: &[PathBuf]) -> Result<(IContextMenu, Pidls), String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        if items.is_empty() {
            return Err("nothing selected".into());
        }
        let mut pidls = Pidls(Vec::new());
        for p in items {
            let name = wide(&sal_core::encoding::display_path(p));
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            unsafe { SHParseDisplayName(PCWSTR(name.as_ptr()), None, &mut pidl, 0, None) }.map_err(|e| format!("{}: {}", p.display(), e.message()))?;
            pidls.0.push(pidl);
        }
        let mut children: Vec<*const ITEMIDLIST> = Vec::new();
        let mut parent: Option<IShellFolder> = None;
        for &pidl in &pidls.0 {
            let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
            let folder: IShellFolder = unsafe { SHBindToParent(pidl, Some(&mut child)) }.map_err(|e| e.message().to_string())?;
            parent.get_or_insert(folder);
            children.push(child);
        }
        let menu: IContextMenu = unsafe { parent.unwrap().GetUIObjectOf(owner, &children, None) }.map_err(|e| e.message().to_string())?;
        Ok((menu, pidls))
    }

    /// The verb ("open", "properties") of the menu item `id`, if it has one.
    #[cfg(test)]
    fn verb_of(menu: &IContextMenu, id: u32) -> Option<String> {
        verb_at(menu, id, FIRST_ID)
    }

    /// Like [`verb_of`] for a menu whose shell items start at `first`.
    fn verb_at(menu: &IContextMenu, id: u32, first: u32) -> Option<String> {
        if id < first {
            return None;
        }
        let mut buf = [0u16; 256];
        unsafe { menu.GetCommandString((id - first) as usize, GCS_VERBW, None, PSTR(buf.as_mut_ptr().cast()), buf.len() as u32) }.ok()?;
        let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
        (n > 0).then(|| String::from_utf16_lossy(&buf[..n]))
    }

    /// Run a menu item (by id) or a verb on the menu's items.
    fn invoke(menu: &IContextMenu, owner: HWND, verb: Result<u32, &str>, dir: &Path, pt: POINT) -> Result<(), String> {
        invoke_at(menu, owner, verb, dir, pt, FIRST_ID)
    }

    /// Like [`invoke`] for a menu whose shell items start at `first`.
    fn invoke_at(menu: &IContextMenu, owner: HWND, verb: Result<u32, &str>, dir: &Path, pt: POINT, first: u32) -> Result<(), String> {
        let dir_w = wide(&sal_core::encoding::display_path(dir));
        let verb_a: Vec<u8>;
        let verb_w: Vec<u16>;
        let (lp_verb, lp_verb_w) = match verb {
            // MAKEINTRESOURCE: the command offset in place of a pointer
            Ok(id) => (PCSTR((id - first) as usize as *const u8), PCWSTR((id - first) as usize as *const u16)),
            Err(name) => {
                verb_a = name.bytes().chain(std::iter::once(0)).collect();
                verb_w = wide(name);
                (PCSTR(verb_a.as_ptr()), PCWSTR(verb_w.as_ptr()))
            }
        };
        let info = CMINVOKECOMMANDINFOEX {
            cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
            fMask: CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE,
            hwnd: owner,
            lpVerb: lp_verb,
            lpVerbW: lp_verb_w,
            lpDirectoryW: PCWSTR(dir_w.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ptInvoke: pt,
            ..Default::default()
        };
        unsafe { menu.InvokeCommand(&info as *const CMINVOKECOMMANDINFOEX as *const CMINVOKECOMMANDINFO) }.map_err(|e| e.message().to_string())
    }

    thread_local! {
        static ACTIVE: RefCell<Option<IContextMenu>> = const { RefCell::new(None) };
    }

    /// Forwards owner-draw and lazy-submenu messages ("Send to", "Open with")
    /// to the menu handlers while the menu is open.
    unsafe extern "system" fn subclass(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
        if matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
            let handled = ACTIVE.with(|a| {
                let a = a.borrow();
                let menu = a.as_ref()?;
                if let Ok(m3) = menu.cast::<IContextMenu3>() {
                    let mut res = LRESULT(0);
                    if m3.HandleMenuMsg2(msg, wp, lp, Some(&mut res)).is_ok() {
                        return Some(res);
                    }
                } else if let Ok(m2) = menu.cast::<IContextMenu2>() {
                    if m2.HandleMenuMsg(msg, wp, lp).is_ok() {
                        return Some(LRESULT(0));
                    }
                }
                None
            });
            if let Some(res) = handled {
                return res;
            }
        }
        DefSubclassProc(h, msg, wp, lp)
    }

    /// Screen point of a client-area position given in physical pixels, or
    /// the mouse position.
    pub fn screen_point(owner: isize, client_px: Option<(f32, f32)>) -> (i32, i32) {
        let mut pt = POINT::default();
        unsafe {
            match client_px {
                Some((x, y)) => {
                    pt = POINT { x: x as i32, y: y as i32 };
                    let _ = ClientToScreen(hwnd(owner), &mut pt);
                }
                None => {
                    let _ = GetCursorPos(&mut pt);
                }
            }
        }
        (pt.x, pt.y)
    }

    /// Show `hmenu`, whose shell items belong to `menu`, at `at`; the chosen id (0 = none).
    fn track(owner: HWND, hmenu: HMENU, menu: &IContextMenu, at: (i32, i32)) -> u32 {
        ACTIVE.with(|a| *a.borrow_mut() = Some(menu.clone()));
        let id = unsafe {
            let _ = SetWindowSubclass(owner, Some(subclass), 0x5A1A, 0);
            release_capture(owner);
            let fg = SetForegroundWindow(owner);
            let n = windows::Win32::UI::WindowsAndMessaging::GetMenuItemCount(hmenu);
            diag(&format!("track: hwnd={:?} items={n} at={at:?} foreground={}", owner.0, fg.as_bool()));
            let t0 = std::time::Instant::now();
            let id = TrackPopupMenuEx(hmenu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, at.0, at.1, owner, None).0 as u32;
            let err = windows::Win32::Foundation::GetLastError();
            diag(&format!("track returned id={id} after {:?}, last error={:?}", t0.elapsed(), err));
            let _ = RemoveWindowSubclass(owner, Some(subclass), 0x5A1A);
            id
        };
        ACTIVE.with(|a| *a.borrow_mut() = None);
        id
    }

    /// What picking the shell item `id` (shell items start at `first`) means:
    /// rename and delete are the panel's, anything else the shell runs.
    fn pick(menu: &IContextMenu, owner: HWND, id: u32, first: u32, dir: &Path, at: (i32, i32)) -> Result<Picked, String> {
        let verb = verb_at(menu, id, first).unwrap_or_default();
        match verb.to_ascii_lowercase().as_str() {
            "rename" => Ok(Picked::Rename),
            "delete" => Ok(Picked::Delete),
            _ => {
                invoke_at(menu, owner, Ok(id), dir, POINT { x: at.0, y: at.1 }, first)?;
                Ok(Picked::Invoked(verb))
            }
        }
    }

    /// Show the shell's own context menu for `items` at `at`; what was picked.
    pub fn show_classic(owner: isize, items: &[PathBuf], dir: &Path, at: (i32, i32), extended: bool) -> Result<Picked, String> {
        let owner = hwnd(owner);
        let t0 = std::time::Instant::now();
        let (menu, _pidls) = context_menu(owner, items)?;
        let hmenu: HMENU = unsafe { CreatePopupMenu() }.map_err(|e| e.message().to_string())?;
        let flags = CMF_NORMAL | CMF_CANRENAME | if extended { CMF_EXTENDEDVERBS } else { 0 };
        let result = (|| {
            unsafe { menu.QueryContextMenu(hmenu, 0, FIRST_ID, LAST_ID, flags) }.map_err(|e| e.message().to_string())?;
            diag(&format!("classic menu built in {:?}", t0.elapsed()));
            match track(owner, hmenu, &menu, at) {
                0 => Ok(Picked::Nothing),
                id => pick(&menu, owner, id, FIRST_ID, dir, at),
            }
        })();
        unsafe {
            let _ = DestroyMenu(hmenu);
        }
        result
    }

    /// Where the shell's items start in the merged menu (ours are below).
    const MERGED_FIRST: u32 = 0x100;

    /// One menu: our `rows` (as in [`show_native`]) and below them the
    /// shell's items for `items`, without those whose verb is in `skip`
    /// (lowercase; the commands we already have).
    pub fn show_merged(
        owner: isize,
        rows: &[Option<(u32, String, String, bool)>],
        items: &[PathBuf],
        dir: &Path,
        at: (i32, i32),
        extended: bool,
        skip: &[&str],
    ) -> Result<Merged, String> {
        use windows::Win32::UI::WindowsAndMessaging::{DeleteMenu, GetMenuItemCount, GetMenuItemID, GetMenuState, MF_BYPOSITION, MF_SEPARATOR};
        let owner = hwnd(owner);
        let t0 = std::time::Instant::now();
        let (menu, _pidls) = context_menu(owner, items)?;
        let hmenu: HMENU = unsafe { CreatePopupMenu() }.map_err(|e| e.message().to_string())?;
        let flags = CMF_NORMAL | CMF_CANRENAME | if extended { CMF_EXTENDEDVERBS } else { 0 };
        let result = (|| {
            append_rows(hmenu, rows);
            let ours = unsafe { GetMenuItemCount(hmenu) }.max(0) as u32;
            unsafe { windows::Win32::UI::WindowsAndMessaging::AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null()) }.map_err(|e| e.message().to_string())?;
            unsafe { menu.QueryContextMenu(hmenu, ours + 1, MERGED_FIRST, LAST_ID, flags) }.map_err(|e| e.message().to_string())?;
            // the shell's copies of our commands go
            let mut i = unsafe { GetMenuItemCount(hmenu) } - 1;
            while i > ours as i32 {
                let id = unsafe { GetMenuItemID(hmenu, i) };
                if id != u32::MAX && id >= MERGED_FIRST {
                    if let Some(v) = verb_at(&menu, id, MERGED_FIRST) {
                        if skip.contains(&v.to_ascii_lowercase().as_str()) {
                            unsafe {
                                let _ = DeleteMenu(hmenu, i as u32, MF_BYPOSITION);
                            }
                        }
                    }
                }
                i -= 1;
            }
            // no separators doubled or at the end
            let is_sep = |i: i32| unsafe { GetMenuState(hmenu, i as u32, MF_BYPOSITION) } & MF_SEPARATOR.0 != 0;
            let mut i = unsafe { GetMenuItemCount(hmenu) } - 1;
            while i >= 0 {
                let last = i == unsafe { GetMenuItemCount(hmenu) } - 1;
                if is_sep(i) && (i == 0 || last || is_sep(i - 1)) {
                    unsafe {
                        let _ = DeleteMenu(hmenu, i as u32, MF_BYPOSITION);
                    }
                }
                i -= 1;
            }
            diag(&format!("merged menu built in {:?}", t0.elapsed()));
            match track(owner, hmenu, &menu, at) {
                0 => Ok(Merged::Shell(Picked::Nothing)),
                id if id < MERGED_FIRST => Ok(Merged::Ours(id)),
                id => pick(&menu, owner, id, MERGED_FIRST, dir, at).map(Merged::Shell),
            }
        })();
        unsafe {
            let _ = DestroyMenu(hmenu);
        }
        result
    }

    /// Build the shell menus of the folders `dirs` and of a file in each once
    /// on a background thread, so the shell extensions are loaded before the
    /// first context menu opens.
    pub fn warm_up(dirs: Vec<PathBuf>) {
        if cfg!(test) {
            return;
        }
        let _ = std::thread::Builder::new().name("shell-menu-warm-up".into()).spawn(move || {
            let t0 = std::time::Instant::now();
            let mut paths = Vec::new();
            for d in dirs {
                let file = std::fs::read_dir(&d).ok().and_then(|it| it.flatten().find(|e| e.file_type().is_ok_and(|t| t.is_file())).map(|e| e.path()));
                paths.push(d);
                paths.extend(file);
            }
            paths.dedup();
            for p in &paths {
                let Ok((menu, _pidls)) = context_menu(HWND::default(), std::slice::from_ref(p)) else { continue };
                if let Ok(hmenu) = unsafe { CreatePopupMenu() } {
                    unsafe {
                        let _ = menu.QueryContextMenu(hmenu, 0, FIRST_ID, LAST_ID, CMF_NORMAL | CMF_CANRENAME);
                        let _ = DestroyMenu(hmenu);
                    }
                }
            }
            diag(&format!("shell menus warmed up in {:?}", t0.elapsed()));
        });
    }

    /// Our rows (see [`show_native`]) appended to `hmenu`.
    fn append_rows(hmenu: HMENU, rows: &[Option<(u32, String, String, bool)>]) {
        use windows::Win32::UI::WindowsAndMessaging::{AppendMenuW, MF_GRAYED, MF_SEPARATOR, MF_STRING};
        for row in rows {
            unsafe {
                let _ = match row {
                    None => AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null()),
                    Some((id, label, shortcut, enabled)) => {
                        // a tab right-aligns the shortcut like in system menus
                        let text = if shortcut.is_empty() { label.clone() } else { format!("{label}\t{shortcut}") };
                        let w = wide(&text);
                        AppendMenuW(hmenu, if *enabled { MF_STRING } else { MF_STRING | MF_GRAYED }, *id as usize, PCWSTR(w.as_ptr()))
                    }
                };
            }
        }
    }

    /// A menu can't track the mouse while our window still holds the capture
    /// from a pressed button.
    fn release_capture(owner: HWND) {
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture};
        unsafe {
            if GetCapture() == owner {
                let _ = ReleaseCapture();
            }
        }
    }

    /// Theme of native menus: `None` follows the Windows app theme, otherwise
    /// forced dark or light. Uses uxtheme's undocumented SetPreferredAppMode
    /// and FlushMenuThemes (ordinals 135/136, as Notepad++ and others do);
    /// silently skipped where unavailable.
    pub fn set_menu_theme(dark: Option<bool>) {
        use windows::core::{s, PCSTR};
        use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
        // PreferredAppMode: 1 = AllowDark (follow system), 2 = ForceDark, 3 = ForceLight
        let mode = match dark {
            None => 1,
            Some(true) => 2,
            Some(false) => 3,
        };
        unsafe {
            let Ok(ux) = LoadLibraryA(s!("uxtheme.dll")) else { return };
            if let Some(f) = GetProcAddress(ux, PCSTR(135usize as *const u8)) {
                let set_mode: extern "system" fn(i32) -> i32 = std::mem::transmute(f);
                set_mode(mode);
            }
            if let Some(f) = GetProcAddress(ux, PCSTR(136usize as *const u8)) {
                let flush: extern "system" fn() = std::mem::transmute(f);
                flush();
            }
        }
    }

    /// A plain native popup menu. `rows`: `Some((id, label, shortcut, enabled))`
    /// or `None` for a separator. Returns the chosen id (0 = none).
    pub fn show_native(owner: isize, rows: &[Option<(u32, String, String, bool)>], at: (i32, i32)) -> u32 {
        let owner = hwnd(owner);
        let Ok(hmenu) = (unsafe { CreatePopupMenu() }) else { return 0 };
        append_rows(hmenu, rows);
        let id = unsafe {
            release_capture(owner);
            let _ = SetForegroundWindow(owner);
            let id = TrackPopupMenuEx(hmenu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, at.0, at.1, owner, None).0 as u32;
            diag(&format!("native menu returned id={id}"));
            let _ = DestroyMenu(hmenu);
            id
        };
        id
    }

    /// The standard "Open" dialog for choosing a program. Returns the path,
    /// quoted when it contains spaces (ready to use as a command line).
    pub fn pick_program(owner: Option<isize>, title: &str) -> Option<String> {
        use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_DONTADDTORECENT, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW};
        let filter: Vec<u16> = "Programs (*.exe;*.bat;*.cmd;*.com)\0*.exe;*.bat;*.cmd;*.com\0All files (*.*)\0*.*\0\0".encode_utf16().collect();
        let title = wide(title);
        let mut file = vec![0u16; 4096];
        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: owner.map(hwnd).unwrap_or_default(),
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: windows::core::PWSTR(file.as_mut_ptr()),
            nMaxFile: file.len() as u32,
            lpstrTitle: PCWSTR(title.as_ptr()),
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_DONTADDTORECENT,
            ..Default::default()
        };
        if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
            return None;
        }
        let n = file.iter().position(|&c| c == 0).unwrap_or(0);
        let path = String::from_utf16_lossy(&file[..n]);
        Some(if path.contains(' ') { format!("\"{path}\"") } else { path })
    }

    /// The standard "Open" dialog for any file (a plugin form's Browse…),
    /// starting in the folder of `current` when given. Never in tests: they
    /// must not open windows.
    pub fn pick_file(owner: Option<isize>, title: &str, current: Option<&Path>) -> Option<PathBuf> {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_DONTADDTORECENT, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW};
        if cfg!(test) {
            return None;
        }
        let filter: Vec<u16> = "All files (*.*)\0*.*\0\0".encode_utf16().collect();
        let title = wide(title);
        let dir: Option<Vec<u16>> = current.and_then(Path::parent).filter(|d| d.is_dir()).map(|d| d.as_os_str().encode_wide().chain([0]).collect());
        let mut file = vec![0u16; 32768];
        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: owner.map(hwnd).unwrap_or_default(),
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: windows::core::PWSTR(file.as_mut_ptr()),
            nMaxFile: file.len() as u32,
            lpstrTitle: PCWSTR(title.as_ptr()),
            lpstrInitialDir: dir.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_DONTADDTORECENT,
            ..Default::default()
        };
        if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
            return None;
        }
        let n = file.iter().position(|&c| c == 0).unwrap_or(0);
        Some(PathBuf::from(std::ffi::OsString::from_wide(&file[..n])))
    }

    /// The standard "Save as" dialog for a text file, starting in `dir` with
    /// `name`; asks before replacing a file. Never in tests.
    pub fn pick_save(owner: Option<isize>, title: &str, dir: Option<&Path>, name: &str) -> Option<PathBuf> {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use windows::Win32::UI::Controls::Dialogs::{GetSaveFileNameW, OFN_DONTADDTORECENT, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW};
        if cfg!(test) {
            return None;
        }
        let filter: Vec<u16> = "Text files (*.txt)\0*.txt\0All files (*.*)\0*.*\0\0".encode_utf16().collect();
        let title = wide(title);
        let ext = wide("txt");
        let dir: Option<Vec<u16>> = dir.filter(|d| d.is_dir()).map(|d| d.as_os_str().encode_wide().chain([0]).collect());
        let mut file = vec![0u16; 32768];
        for (i, c) in name.encode_utf16().take(file.len() - 1).enumerate() {
            file[i] = c;
        }
        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: owner.map(hwnd).unwrap_or_default(),
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: windows::core::PWSTR(file.as_mut_ptr()),
            nMaxFile: file.len() as u32,
            lpstrTitle: PCWSTR(title.as_ptr()),
            lpstrDefExt: PCWSTR(ext.as_ptr()),
            lpstrInitialDir: dir.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
            Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_DONTADDTORECENT,
            ..Default::default()
        };
        if !unsafe { GetSaveFileNameW(&mut ofn) }.as_bool() {
            return None;
        }
        let n = file.iter().position(|&c| c == 0).unwrap_or(0);
        Some(PathBuf::from(std::ffi::OsString::from_wide(&file[..n])))
    }

    /// The standard folder picker (a plugin form's Browse… for a folder),
    /// starting in `current` when it exists. Never in tests.
    pub fn pick_folder(owner: Option<isize>, title: &str, current: Option<&Path>) -> Option<PathBuf> {
        use std::os::windows::ffi::OsStringExt;
        use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
        use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH};
        if cfg!(test) {
            return None;
        }
        unsafe {
            let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            let options = dialog.GetOptions().ok()?;
            dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
            let title = wide(title);
            let _ = dialog.SetTitle(PCWSTR(title.as_ptr()));
            if let Some(dir) = current.filter(|d| d.is_dir()) {
                let dir = wide(&dir.to_string_lossy());
                if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(dir.as_ptr()), None) {
                    let _ = dialog.SetFolder(&item);
                }
            }
            dialog.Show(owner.map(hwnd).unwrap_or_default()).ok()?;
            let name = dialog.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
            let path = PathBuf::from(std::ffi::OsString::from_wide(name.as_wide()));
            CoTaskMemFree(Some(name.0 as *const _));
            Some(path)
        }
    }

    /// Diagnostics (SAL_DIAG_MENU set): log to stderr.
    pub fn diag(msg: &str) {
        if std::env::var_os("SAL_DIAG_MENU").is_some() {
            eprintln!("[menu] {msg}");
        }
    }

    unsafe extern "system" fn diag_timer(h: HWND, _msg: u32, id: usize, _time: u32) {
        use windows::Win32::UI::WindowsAndMessaging::{EndMenu, FindWindowW, IsWindowVisible, KillTimer};
        let class: Vec<u16> = "#32768\0".encode_utf16().collect();
        let menu = FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()).unwrap_or_default();
        diag(&format!("timer: menu window {:?} visible={}", menu.0, !menu.0.is_null() && IsWindowVisible(menu).as_bool()));
        let _ = KillTimer(h, id);
        let _ = EndMenu();
    }

    /// Closes an open menu after `ms` (diagnostics only; no input involved).
    pub fn arm_diag_timer(owner: isize, ms: u32) {
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetTimer(hwnd(owner), 0x5A1B, ms, Some(diag_timer));
        }
    }

    /// Run the shell verb `verb` ("cut", "properties", …) on `items`.
    pub fn invoke_verb(owner: isize, items: &[PathBuf], dir: &Path, verb: &str) -> Result<(), String> {
        let owner = hwnd(owner);
        let (menu, _pidls) = context_menu(owner, items)?;
        let hmenu = unsafe { CreatePopupMenu() }.map_err(|e| e.message().to_string())?;
        // some handlers only accept verbs after building their menu
        let r = unsafe { menu.QueryContextMenu(hmenu, 0, FIRST_ID, LAST_ID, CMF_NORMAL) }
            .map_err(|e| e.message().to_string())
            .and_then(|_| invoke(&menu, owner, Err(verb), dir, POINT::default()));
        unsafe {
            let _ = DestroyMenu(hmenu);
        }
        r
    }

    #[cfg(test)]
    pub fn context_menu_for_test(items: &[PathBuf]) -> Result<(IContextMenu, impl Drop), String> {
        context_menu(HWND::default(), items)
    }

    /// Verbs the shell offers for `items` (menu is built, never shown).
    #[cfg(test)]
    pub fn verbs(items: &[PathBuf]) -> Result<Vec<String>, String> {
        let (menu, _pidls) = context_menu(HWND::default(), items)?;
        let hmenu = unsafe { CreatePopupMenu() }.map_err(|e| e.message().to_string())?;
        use windows::Win32::UI::WindowsAndMessaging::{GetMenuItemCount, GetMenuItemID};
        let mut out = Vec::new();
        if unsafe { menu.QueryContextMenu(hmenu, 0, FIRST_ID, LAST_ID, CMF_NORMAL) }.is_ok() {
            let n = unsafe { GetMenuItemCount(hmenu) };
            for i in 0..n {
                let id = unsafe { GetMenuItemID(hmenu, i) };
                if id != u32::MAX {
                    if let Some(v) = verb_of(&menu, id) {
                        out.push(v);
                    }
                }
            }
        }
        unsafe {
            let _ = DestroyMenu(hmenu);
        }
        Ok(out)
    }

    /// Whether the clipboard holds files to paste.
    pub fn clipboard_has_files() -> bool {
        unsafe { IsClipboardFormatAvailable(CF_HDROP.0 as u32) }.is_ok()
    }
}

#[cfg(all(windows, test))]
use imp::verbs;
#[cfg(windows)]
pub use imp::{
    arm_diag_timer, clipboard_has_files, diag, invoke_verb, pick_file, pick_folder, pick_program, pick_save, screen_point, set_menu_theme, show_classic, show_merged, show_native, warm_up,
};

#[cfg(not(windows))]
mod fallback {
    use super::*;
    /// Elsewhere there is no shell menu.
    pub fn show_classic(_: isize, _: &[PathBuf], _: &Path, _: (i32, i32), _: bool) -> Result<Picked, String> {
        Err("the system context menu is only available on Windows".into())
    }
    /// Elsewhere there is no shell menu to merge.
    pub fn show_merged(_: isize, _: &[Option<(u32, String, String, bool)>], _: &[PathBuf], _: &Path, _: (i32, i32), _: bool, _: &[&str]) -> Result<Merged, String> {
        Err("the system context menu is only available on Windows".into())
    }
    /// Elsewhere there are no shell extensions.
    pub fn warm_up(_: Vec<PathBuf>) {}
    /// Elsewhere there are no shell verbs.
    pub fn invoke_verb(_: isize, _: &[PathBuf], _: &Path, verb: &str) -> Result<(), String> {
        Err(format!("\"{verb}\" is only available on Windows"))
    }
    /// Elsewhere window points are screen points.
    pub fn screen_point(_: isize, p: Option<(f32, f32)>) -> (i32, i32) {
        p.map_or((0, 0), |(x, y)| (x as i32, y as i32))
    }
    /// Elsewhere there is nothing to paste.
    pub fn clipboard_has_files() -> bool {
        false
    }
    /// Elsewhere there are no native menus.
    pub fn show_native(_: isize, _: &[Option<(u32, String, String, bool)>], _: (i32, i32)) -> u32 {
        0
    }
    /// Elsewhere menus have no theme.
    pub fn set_menu_theme(_: Option<bool>) {}
    /// Elsewhere there is no Open dialog.
    pub fn pick_file(_: Option<isize>, _: &str, _: Option<&Path>) -> Option<PathBuf> {
        None
    }
    /// Elsewhere there is no Save dialog.
    pub fn pick_save(_: Option<isize>, _: &str, _: Option<&Path>, _: &str) -> Option<PathBuf> {
        None
    }
    /// Elsewhere there is no folder picker.
    pub fn pick_folder(_: Option<isize>, _: &str, _: Option<&Path>) -> Option<PathBuf> {
        None
    }
    /// Elsewhere there is no Open dialog.
    pub fn pick_program(_: Option<isize>, _: &str) -> Option<String> {
        None
    }
}

#[cfg(not(windows))]
pub use fallback::{clipboard_has_files, invoke_verb, pick_file, pick_folder, pick_program, pick_save, screen_point, set_menu_theme, show_classic, show_merged, show_native, warm_up};

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// `cargo test -p sal-app time_classic_menu -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn time_classic_menu() {
        let target = std::env::var_os("SAL_MENU_TARGET").map(PathBuf::from).unwrap_or_else(|| std::env::current_exe().unwrap());
        use windows::Win32::UI::Shell::{CMF_CANRENAME, CMF_NORMAL};
        use windows::Win32::UI::WindowsAndMessaging::{CreatePopupMenu, DestroyMenu};
        if std::env::var_os("SAL_MENU_WARM").is_some() {
            let t = std::time::Instant::now();
            let tg = target.clone();
            std::thread::spawn(move || {
                for _ in 0..2 {
                    let _ = verbs(std::slice::from_ref(&tg));
                }
            })
            .join()
            .unwrap();
            eprintln!("round warm-up thread: {:?}", t.elapsed());
        }
        for round in 0..3 {
            let t = std::time::Instant::now();
            let (menu, _p) = imp::context_menu_for_test(std::slice::from_ref(&target)).unwrap();
            let t1 = t.elapsed();
            let h = unsafe { CreatePopupMenu() }.unwrap();
            unsafe { menu.QueryContextMenu(h, 0, 1, 0x7FFF, CMF_NORMAL | CMF_CANRENAME) }.unwrap();
            eprintln!("round {round}: object {t1:?}, query {:?}", t.elapsed() - t1);
            unsafe { DestroyMenu(h) }.unwrap();
        }
    }

    #[test]
    fn shell_offers_standard_verbs() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("dokument ž.txt");
        std::fs::write(&f, "x").unwrap();
        let v = verbs(std::slice::from_ref(&f)).unwrap();
        for want in ["open", "properties", "copy", "delete"] {
            assert!(v.iter().any(|x| x.eq_ignore_ascii_case(want)), "{want} missing from {v:?}");
        }
        // several items from one folder share a menu
        let g = t.path().join("druhý.txt");
        std::fs::write(&g, "y").unwrap();
        let v = verbs(&[f, g]).unwrap();
        assert!(v.iter().any(|x| x.eq_ignore_ascii_case("properties")), "{v:?}");
        assert!(verbs(&[t.path().join("does-not-exist")]).is_err());
    }
}
