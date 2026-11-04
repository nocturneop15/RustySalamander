//! Drag and drop: the rule that decides between moving and copying, the
//! places in the window where items can be dropped, and Windows' drag and
//! drop with other programs.
//!
//! - Inside the program a drag is egui's own (`app.rs`): items dragged from
//!   a panel onto a folder or onto the other panel.
//! - Dragged out of the window, the items become a Windows drag
//!   ([`drag_out`]): Explorer, the desktop or any program that takes files
//!   copies or moves them by the same rule.
//! - Files dragged onto the window from another program arrive through
//!   [`register`]: the program shows where they would go, as for its own
//!   drags, and takes them on the drop.
//!
//! The rule, as in Explorer: on the same drive the items are moved, onto
//! another drive copied; Shift moves and Ctrl copies whatever the drives.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eframe::egui;
use sal_core::fileops::OpKind;

/// A place in the window where items can be dropped: a folder's row, or a
/// panel (its folder).
#[derive(Clone, Debug)]
pub struct Zone {
    /// Where, in the window's points.
    pub rect: egui::Rect,
    /// The folder the items go into.
    pub folder: PathBuf,
    /// Its volume ([`volume`]): moving within one is the default.
    pub volume: String,
}

/// Files from another program, over the window or dropped on it.
#[derive(Clone, Debug, Default)]
pub struct Outside {
    /// Over the window now: where (in points), and what.
    pub hover: Option<(egui::Pos2, Vec<PathBuf>, bool, bool)>,
    /// Dropped: the files, where, and what to do.
    pub dropped: Option<(Vec<PathBuf>, PathBuf, OpKind)>,
}

/// What the window and Windows' drop handler share: the drop zones (the
/// program updates them every frame) and the files from outside.
#[derive(Default)]
pub struct Shared {
    pub zones: Vec<Zone>,
    pub outside: Outside,
}

pub type SharedRef = Arc<Mutex<Shared>>;

/// The volume of `path`: its drive or share, in lower case. Items inside an
/// archive or on a plugin's connection are on a volume of their own, given
/// by the caller.
pub fn volume(path: &Path) -> String {
    sal_core::drives::root_of(path).to_string_lossy().to_lowercase()
}

/// Move or copy: Shift moves and Ctrl copies; otherwise items move within
/// their volume and are copied to another.
pub fn kind(same_volume: bool, shift: bool, ctrl: bool) -> OpKind {
    match (shift, ctrl) {
        (true, false) => OpKind::Move,
        (false, true) => OpKind::Copy,
        _ if same_volume => OpKind::Move,
        _ => OpKind::Copy,
    }
}

/// Whether `sources` can go into `folder`: not into one of them or what it
/// holds, and not where they are already.
pub fn can_drop(sources: &[PathBuf], folder: &Path) -> bool {
    !sources.is_empty() && sources.iter().all(|s| !folder.starts_with(s)) && !sources.iter().all(|s| s.parent() == Some(folder))
}

/// The zone at `pos`: the first that holds it (folder rows come before the
/// panels).
pub fn zone_at(zones: &[Zone], pos: egui::Pos2) -> Option<&Zone> {
    zones.iter().find(|z| z.rect.contains(pos))
}

/// Where `sources` dropped at `pos` go, and how; `None` where they can't.
pub fn resolve(zones: &[Zone], pos: egui::Pos2, sources: &[PathBuf], source_volume: &str, shift: bool, ctrl: bool) -> Option<(Zone, OpKind)> {
    let z = zone_at(zones, pos)?;
    can_drop(sources, &z.folder).then(|| (z.clone(), kind(z.volume == source_volume, shift, ctrl)))
}

#[cfg(windows)]
pub use win::{drag_out, pointer_outside, register};

#[cfg(not(windows))]
pub fn pointer_outside(_: isize) -> Option<bool> {
    None
}

#[cfg(not(windows))]
pub fn drag_out(_: isize, _: &[PathBuf], _: bool) -> Result<(), String> {
    Err("not supported".into())
}

#[cfg(not(windows))]
pub fn register(_: isize, _: egui::Context) -> Option<SharedRef> {
    None
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::cell::RefCell;

    use windows::core::{implement, Result as WinResult, PCWSTR};
    use windows::Win32::Foundation::{COLORREF, HWND, POINT, POINTL, SIZE};
    use windows::Win32::Graphics::Gdi::{CreateDIBSection, DeleteObject, ScreenToClient, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS};
    use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
    use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetClientRect, GetCursorPos};
    use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, IDataObject, CLSCTX_INPROC_SERVER, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
    use windows::Win32::System::Ole::{
        IDropTarget, IDropTarget_Impl, OleInitialize, RegisterDragDrop, ReleaseStgMedium, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE, DROPEFFECT_NONE,
    };
    use windows::Win32::System::SystemServices::{MK_CONTROL, MK_SHIFT, MODIFIERKEYS_FLAGS};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        BHID_DataObject, CLSID_DragDropHelper, DragQueryFileW, IDragSourceHelper, IDropTargetHelper, IShellItemArray, SHCreateShellItemArrayFromIDLists, SHDoDragDrop, SHGetFileInfoW,
        SHGetImageList, SHParseDisplayName, HDROP, SHDRAGIMAGE, SHFILEINFOW, SHGFI_SYSICONINDEX, SHIL_EXTRALARGE,
    };

    fn hwnd(h: isize) -> HWND {
        HWND(h as *mut core::ffi::c_void)
    }

    /// Drag `paths` (real files and folders) out of the window as Windows
    /// does: Explorer, the desktop or another program copies them, or moves
    /// them if `can_move`. Returns when the drag ends.
    pub fn drag_out(owner: isize, paths: &[PathBuf], can_move: bool) -> Result<(), String> {
        unsafe {
            let mut pidls: Vec<*mut ITEMIDLIST> = Vec::new();
            for p in paths {
                let wide: Vec<u16> = p.as_os_str().encode_wide_nul();
                let mut pidl = std::ptr::null_mut();
                if SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None).is_ok() && !pidl.is_null() {
                    pidls.push(pidl);
                }
            }
            let result = (|| -> WinResult<()> {
                let list: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const ITEMIDLIST).collect();
                let array: IShellItemArray = SHCreateShellItemArrayFromIDLists(&list)?;
                let data: IDataObject = array.BindToHandler(None, &BHID_DataObject)?;
                // our own drag image at once: without it Windows shows a
                // placeholder until it has made a folder's thumbnail
                if let Some(first) = paths.first() {
                    set_drag_image(&data, first);
                }
                // a default drop source
                SHDoDragDrop(hwnd(owner), &data, None, if can_move { DROPEFFECT_COPY | DROPEFFECT_MOVE } else { DROPEFFECT_COPY })?;
                Ok(())
            })();
            for p in pidls {
                CoTaskMemFree(Some(p as *const _));
            }
            result.map_err(|e| e.message())
        }
    }

    /// Whether the mouse pointer is outside the client area of `owner`
    /// (asked of Windows; `None` when it can't say).
    pub fn pointer_outside(owner: isize) -> Option<bool> {
        unsafe {
            let mut p = POINT::default();
            GetCursorPos(&mut p).ok()?;
            ScreenToClient(hwnd(owner), &mut p).ok().ok()?;
            let mut r = RECT::default();
            GetClientRect(hwnd(owner), &mut r).ok()?;
            Some(p.x < r.left || p.y < r.top || p.x >= r.right || p.y >= r.bottom)
        }
    }

    /// The drag image of `data`: the large shell icon of `path`, with the
    /// pointer at its middle. Nothing when the icon can't be had (Windows
    /// then makes an image itself).
    fn set_drag_image(data: &IDataObject, path: &Path) {
        unsafe {
            let Some(icon) = large_icon(path) else { return };
            let (w, h) = (icon.width as i32, icon.height as i32);
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let Ok(bmp) = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0) else { return };
            if bits.is_null() {
                let _ = DeleteObject(bmp);
                return;
            }
            // premultiplied BGRA, as the drag image wants
            let out = std::slice::from_raw_parts_mut(bits.cast::<u8>(), (w * h * 4) as usize);
            for (o, p) in out.as_chunks_mut::<4>().0.iter_mut().zip(icon.pixels.as_chunks::<4>().0) {
                let a = p[3] as u32;
                o[0] = (p[2] as u32 * a / 255) as u8;
                o[1] = (p[1] as u32 * a / 255) as u8;
                o[2] = (p[0] as u32 * a / 255) as u8;
                o[3] = p[3];
            }
            let image = SHDRAGIMAGE { sizeDragImage: SIZE { cx: w, cy: h }, ptOffset: POINT { x: w / 2, y: h / 2 }, hbmpDragImage: bmp, crColorKey: COLORREF(0xFFFF_FFFF) };
            let helper: WinResult<IDragSourceHelper> = CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER);
            // the helper owns the bitmap once it took it
            if helper.and_then(|h| h.InitializeFromBitmap(&image, data)).is_err() {
                let _ = DeleteObject(bmp);
            }
        }
    }

    /// The extra-large (48 px) shell icon of `path`, RGBA.
    fn large_icon(path: &Path) -> Option<crate::icons::Rgba> {
        unsafe {
            let wide = path.as_os_str().encode_wide_nul();
            let mut info = SHFILEINFOW::default();
            if SHGetFileInfoW(PCWSTR(wide.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(0), Some(&mut info), std::mem::size_of::<SHFILEINFOW>() as u32, SHGFI_SYSICONINDEX) == 0 {
                return None;
            }
            let list: IImageList = SHGetImageList(SHIL_EXTRALARGE as i32).ok()?;
            let icon = list.GetIcon(info.iIcon, ILD_TRANSPARENT.0).ok()?;
            let rgba = crate::icons::icon_to_rgba(icon);
            let _ = DestroyIcon(icon);
            rgba
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

        /// `SAL_DRAG_TARGET=<folder> cargo test -p sal-app time_drag_out_setup -- --ignored --nocapture`
        #[test]
        #[ignore]
        fn time_drag_out_setup() {
            let dir = std::env::var_os("SAL_DRAG_TARGET").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                let _ = OleInitialize(None);
                let t = std::time::Instant::now();
                let wide = dir.as_os_str().encode_wide_nul();
                let mut pidl = std::ptr::null_mut();
                SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None).unwrap();
                eprintln!("parse {:?}", t.elapsed());
                let array: IShellItemArray = SHCreateShellItemArrayFromIDLists(&[pidl as *const ITEMIDLIST]).unwrap();
                eprintln!("array {:?}", t.elapsed());
                let data: IDataObject = array.BindToHandler(None, &BHID_DataObject).unwrap();
                eprintln!("data object {:?}", t.elapsed());
                let _ = large_icon(&dir);
                eprintln!("icon {:?}", t.elapsed());
                set_drag_image(&data, &dir);
                eprintln!("drag image {:?}", t.elapsed());
                let fmt = FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
                let mut m = data.GetData(&fmt).unwrap();
                ReleaseStgMedium(&mut m);
                eprintln!("CF_HDROP {:?}", t.elapsed());
                CoTaskMemFree(Some(pidl as *const _));
            }
        }

        /// A folder's drag image is ready at once: its 48 px shell icon,
        /// which Windows takes for the data object of a drag.
        #[test]
        fn folder_drag_image_at_once() {
            let t = tempfile::tempdir().unwrap();
            let dir = t.path().join("složka");
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(dir.join("a.txt"), "a").unwrap();
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            let icon = large_icon(&dir).expect("folder icon");
            assert_eq!((icon.width, icon.height), (48, 48));
            assert!(icon.pixels.as_chunks::<4>().0.iter().any(|p| p[3] > 0), "not empty");
            let wide = dir.as_os_str().encode_wide_nul();
            unsafe {
                let mut pidl = std::ptr::null_mut();
                SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None).unwrap();
                let array: IShellItemArray = SHCreateShellItemArrayFromIDLists(&[pidl as *const ITEMIDLIST]).unwrap();
                let data: IDataObject = array.BindToHandler(None, &BHID_DataObject).unwrap();
                set_drag_image(&data, &dir);
                // the image is stored in the data object
                let fmt = windows::Win32::System::DataExchange::RegisterClipboardFormatW(windows::core::w!("DragImageBits"));
                let f = FORMATETC { cfFormat: fmt as u16, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
                assert!(data.QueryGetData(&f).is_ok(), "drag image set");
                CoTaskMemFree(Some(pidl as *const _));
            }
        }
    }

    trait WideNul {
        fn encode_wide_nul(&self) -> Vec<u16>;
    }

    impl WideNul for std::ffi::OsStr {
        fn encode_wide_nul(&self) -> Vec<u16> {
            use std::os::windows::ffi::OsStrExt;
            self.encode_wide().chain(std::iter::once(0)).collect()
        }
    }

    /// Take files dropped on the window `owner` from other programs (and the
    /// program's own items dragged out and back). Instead of the window
    /// library's own handler, which only says "copy" and not where.
    pub fn register(owner: isize, ctx: egui::Context) -> Option<SharedRef> {
        let shared: SharedRef = Arc::new(Mutex::new(Shared::default()));
        unsafe {
            // OLE on the window's thread (COM is already there: S_FALSE is fine)
            let _ = OleInitialize(None);
            let helper: Option<IDropTargetHelper> = CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).ok();
            let target: IDropTarget = Target { owner, shared: shared.clone(), ctx, files: RefCell::new(Vec::new()), helper }.into();
            RegisterDragDrop(hwnd(owner), &target).ok()?;
        }
        Some(shared)
    }

    #[implement(IDropTarget)]
    struct Target {
        owner: isize,
        shared: SharedRef,
        ctx: egui::Context,
        /// What is being dragged (real files), from DragEnter on.
        files: RefCell<Vec<PathBuf>>,
        /// Draws the drag image over the window.
        helper: Option<IDropTargetHelper>,
    }

    impl Target {
        /// `pt` (screen pixels) in the window's points.
        fn pos(&self, pt: &POINTL) -> egui::Pos2 {
            let mut p = POINT { x: pt.x, y: pt.y };
            unsafe {
                let _ = ScreenToClient(hwnd(self.owner), &mut p);
            }
            let ppp = self.ctx.pixels_per_point();
            egui::pos2(p.x as f32 / ppp, p.y as f32 / ppp)
        }

        /// Where the files would go: the effect Windows shows, the hover the
        /// window draws; on the drop, what to do.
        fn over(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, dropped: bool) -> DROPEFFECT {
            let files = self.files.borrow().clone();
            let (shift, ctrl) = ((keys.0 & MK_SHIFT.0) != 0, (keys.0 & MK_CONTROL.0) != 0);
            let pos = self.pos(pt);
            let mut s = self.shared.lock().unwrap();
            let src_volume = files.first().map(|f| volume(f)).unwrap_or_default();
            let hit = resolve(&s.zones, pos, &files, &src_volume, shift, ctrl);
            if dropped {
                s.outside.hover = None;
                if let Some((z, k)) = &hit {
                    s.outside.dropped = Some((files, z.folder.clone(), *k));
                }
            } else {
                s.outside.hover = (!files.is_empty()).then_some((pos, files, shift, ctrl));
            }
            drop(s);
            self.ctx.request_repaint();
            match hit {
                // the program moves the files itself: the program they come
                // from is told "copy", so it doesn't delete them as well
                Some(_) if dropped => DROPEFFECT_COPY,
                Some((_, OpKind::Move)) => DROPEFFECT_MOVE,
                Some(_) => DROPEFFECT_COPY,
                None => DROPEFFECT_NONE,
            }
        }
    }

    /// The paths of the files in `data` (CF_HDROP): none for other contents.
    fn files_of(data: Option<&IDataObject>) -> Vec<PathBuf> {
        let Some(data) = data else { return Vec::new() };
        let fmt = FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
        let mut out = Vec::new();
        unsafe {
            let Ok(mut medium) = data.GetData(&fmt) else { return out };
            let drop = HDROP(medium.u.hGlobal.0);
            let n = DragQueryFileW(drop, u32::MAX, None);
            for i in 0..n {
                let len = DragQueryFileW(drop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                let got = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
                use std::os::windows::ffi::OsStringExt;
                out.push(PathBuf::from(std::ffi::OsString::from_wide(&buf[..got])));
            }
            ReleaseStgMedium(&mut medium);
        }
        out
    }

    impl IDropTarget_Impl for Target_Impl {
        fn DragEnter(&self, data: Option<&IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> WinResult<()> {
            *self.files.borrow_mut() = files_of(data);
            let e = self.over(keys, pt, false);
            unsafe {
                *effect = e;
                if let (Some(h), Some(d)) = (&self.helper, data) {
                    let p = POINT { x: pt.x, y: pt.y };
                    let _ = h.DragEnter(hwnd(self.owner), d, &p, e);
                }
            }
            Ok(())
        }

        fn DragOver(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> WinResult<()> {
            let e = self.over(keys, pt, false);
            unsafe {
                *effect = e;
                if let Some(h) = &self.helper {
                    let p = POINT { x: pt.x, y: pt.y };
                    let _ = h.DragOver(&p, e);
                }
            }
            Ok(())
        }

        fn DragLeave(&self) -> WinResult<()> {
            self.files.borrow_mut().clear();
            self.shared.lock().unwrap().outside.hover = None;
            self.ctx.request_repaint();
            if let Some(h) = &self.helper {
                unsafe {
                    let _ = h.DragLeave();
                }
            }
            Ok(())
        }

        fn Drop(&self, data: Option<&IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> WinResult<()> {
            if self.files.borrow().is_empty() {
                *self.files.borrow_mut() = files_of(data);
            }
            let e = self.over(keys, pt, true);
            unsafe {
                *effect = e;
                if let (Some(h), Some(d)) = (&self.helper, data) {
                    let p = POINT { x: pt.x, y: pt.y };
                    let _ = h.Drop(d, &p, e);
                }
            }
            self.files.borrow_mut().clear();
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_drive_moves_another_copies_keys_decide() {
        assert_eq!(kind(true, false, false), OpKind::Move);
        assert_eq!(kind(false, false, false), OpKind::Copy);
        assert_eq!(kind(false, true, false), OpKind::Move, "Shift moves");
        assert_eq!(kind(true, false, true), OpKind::Copy, "Ctrl copies");
        assert_eq!(volume(Path::new(r"C:\a\b")), volume(Path::new(r"c:\x")));
        assert_ne!(volume(Path::new(r"C:\a")), volume(Path::new(r"D:\a")));
        assert_ne!(volume(Path::new(r"\\server\share\a")), volume(Path::new(r"C:\a")));
    }

    #[test]
    fn not_into_itself_nor_where_it_is() {
        let src = vec![PathBuf::from(r"C:\data\photos"), PathBuf::from(r"C:\data\a.txt")];
        assert!(can_drop(&src, Path::new(r"D:\backup")));
        assert!(can_drop(&src, Path::new(r"C:\data\other")));
        assert!(!can_drop(&src, Path::new(r"C:\data")), "where they are");
        assert!(!can_drop(&src, Path::new(r"C:\data\photos")), "into itself");
        assert!(!can_drop(&src, Path::new(r"C:\data\photos\2026")), "into what it holds");
        assert!(!can_drop(&[], Path::new(r"D:\")));
    }

    #[test]
    fn the_first_zone_wins() {
        let r = |x: f32, w: f32| egui::Rect::from_min_size(egui::pos2(x, 0.0), egui::vec2(w, 100.0));
        let zones = vec![
            Zone { rect: r(10.0, 20.0), folder: PathBuf::from(r"C:\a\sub"), volume: "c:\\".into() },
            Zone { rect: r(0.0, 100.0), folder: PathBuf::from(r"C:\a"), volume: "c:\\".into() },
        ];
        let src = vec![PathBuf::from(r"D:\x.txt")];
        let (z, k) = resolve(&zones, egui::pos2(15.0, 5.0), &src, "d:\\", false, false).unwrap();
        assert_eq!((z.folder.as_path(), k), (Path::new(r"C:\a\sub"), OpKind::Copy));
        let (z, k) = resolve(&zones, egui::pos2(60.0, 5.0), &src, "d:\\", true, false).unwrap();
        assert_eq!((z.folder.as_path(), k), (Path::new(r"C:\a"), OpKind::Move));
        assert!(resolve(&zones, egui::pos2(160.0, 5.0), &src, "d:\\", false, false).is_none());
    }
}
