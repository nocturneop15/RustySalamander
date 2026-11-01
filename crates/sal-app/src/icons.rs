//! Windows file icons (SHGetFileInfoW), loaded on a background thread and
//! cached per extension; programs, shortcuts and icon files get their own.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui;
use sal_core::{EntryKind, FileEntry};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IconKey {
    Folder,
    /// Lower-case extension ("" for none).
    Ext(String),
    /// Files whose icon is inside the file itself.
    File(PathBuf),
}

/// Extensions whose files carry their own icon.
const PER_FILE: &[&str] = &["exe", "ico", "lnk", "url", "msc", "cpl", "scr", "appref-ms", "cur", "ani"];

/// An icon as straight (non-premultiplied) RGBA.
pub struct Rgba {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

pub struct Icons {
    textures: HashMap<IconKey, Option<egui::TextureHandle>>,
    pending: HashSet<IconKey>,
    tx: Sender<(IconKey, PathBuf)>,
    rx: Receiver<(IconKey, Option<Rgba>)>,
}

impl Icons {
    /// The cache, with a thread that loads icons from Windows.
    pub fn new(ctx: &egui::Context) -> Icons {
        let (tx, worker_rx) = channel::<(IconKey, PathBuf)>();
        let (worker_tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            use std::sync::mpsc::RecvTimeoutError;
            init_com();
            loop {
                // an STA thread must keep handling window messages: the shell may
                // create hidden windows on it and other threads then wait on them
                pump_messages();
                match worker_rx.recv_timeout(std::time::Duration::from_millis(30)) {
                    Ok((key, path)) => {
                        // the shell's icon cache can fail while several threads
                        // initialise it at once: retry briefly before giving up
                        let mut icon = load(&key, &path);
                        for _ in 0..4 {
                            if icon.is_some() {
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(40));
                            pump_messages();
                            icon = load(&key, &path);
                        }
                        if worker_tx.send((key, icon)).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        Icons { textures: HashMap::new(), pending: HashSet::new(), tx, rx }
    }

    /// What an item's icon is cached by: its type, or the file itself for
    /// programs, shortcuts and icons.
    fn key_for(e: &FileEntry) -> IconKey {
        match e.kind {
            EntryKind::File => {
                let ext = e.ext().to_lowercase();
                // network paths keep the per-type icon: reading each file would be slow
                let local = !e.path.to_string_lossy().starts_with(r"\\");
                if local && PER_FILE.contains(&ext.as_str()) {
                    IconKey::File(e.path.clone())
                } else {
                    IconKey::Ext(ext)
                }
            }
            _ => IconKey::Folder,
        }
    }

    /// Texture for the entry, or `None` while it loads (or if Windows has none).
    pub fn get(&mut self, e: &FileEntry) -> Option<egui::TextureHandle> {
        let key = Self::key_for(e);
        if let Some(t) = self.textures.get(&key) {
            return t.clone();
        }
        if self.pending.insert(key.clone()) {
            let _ = self.tx.send((key.clone(), e.path.clone()));
        }
        // a program's own icon is still loading: show its type icon meanwhile
        if let IconKey::File(_) = key {
            let ext = IconKey::Ext(e.ext().to_lowercase());
            if let Some(t) = self.textures.get(&ext) {
                return t.clone();
            }
            if self.pending.insert(ext.clone()) {
                let _ = self.tx.send((ext, e.path.clone()));
            }
        }
        None
    }

    /// Turn loaded icons into textures; call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((key, icon)) = self.rx.try_recv() {
            self.pending.remove(&key);
            let tex = icon.map(|i| {
                let img = egui::ColorImage::from_rgba_unmultiplied([i.width, i.height], &i.pixels);
                ctx.load_texture(format!("icon:{key:?}"), img, egui::TextureOptions::LINEAR)
            });
            self.textures.insert(key, tex);
        }
    }

    /// What is loaded and what is pending, for test failures.
    #[cfg(test)]
    pub fn debug_state(&self) -> String {
        format!("textures={:?} pending={:?}", self.textures.iter().map(|(k, v)| (k, v.is_some())).collect::<Vec<_>>(), self.pending)
    }

    /// Forget per-file icons (e.g. after a refresh: a program may have changed).
    pub fn clear_files(&mut self) {
        self.textures.retain(|k, _| !matches!(k, IconKey::File(_)));
    }
}

/// The shell needs COM on the loading thread.
#[cfg(windows)]
fn init_com() {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
}

/// Elsewhere there is no COM.
#[cfg(not(windows))]
fn init_com() {}

/// Handle the thread's window messages (some icon handlers need them).
#[cfg(windows)]
fn pump_messages() {
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};
    let mut msg = MSG::default();
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Elsewhere there are no window messages.
#[cfg(not(windows))]
fn pump_messages() {}

/// Load the small shell icon for `key`.
#[cfg(windows)]
pub fn load(key: &IconKey, path: &Path) -> Option<Rgba> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES};
    use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON, SHGFI_USEFILEATTRIBUTES};
    use windows::Win32::UI::WindowsAndMessaging::DestroyIcon;

    // by type: a made-up name plus attributes, no disk access
    let (name, attrs, flags) = match key {
        IconKey::Folder => ("folder".to_owned(), FILE_ATTRIBUTE_DIRECTORY, SHGFI_ICON | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES),
        IconKey::Ext(ext) if ext.is_empty() => ("file".to_owned(), FILE_ATTRIBUTE_NORMAL, SHGFI_ICON | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES),
        IconKey::Ext(ext) => (format!("file.{ext}"), FILE_ATTRIBUTE_NORMAL, SHGFI_ICON | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES),
        IconKey::File(_) => (sal_core::encoding::display_path(path), FILE_FLAGS_AND_ATTRIBUTES(0), SHGFI_ICON | SHGFI_SMALLICON),
    };
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut info = SHFILEINFOW::default();
    let ok = unsafe { SHGetFileInfoW(PCWSTR(wide.as_ptr()), attrs, Some(&mut info), std::mem::size_of::<SHFILEINFOW>() as u32, flags) };
    if ok == 0 || info.hIcon.is_invalid() {
        return None;
    }
    let rgba = icon_to_rgba(info.hIcon);
    unsafe {
        let _ = DestroyIcon(info.hIcon);
    }
    rgba
}

/// Elsewhere there are no shell icons.
#[cfg(not(windows))]
pub fn load(_: &IconKey, _: &Path) -> Option<Rgba> {
    None
}

/// An icon's pixels, RGBA.
#[cfg(windows)]
fn icon_to_rgba(icon: windows::Win32::UI::WindowsAndMessaging::HICON) -> Option<Rgba> {
    use windows::Win32::Graphics::Gdi::{CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP};
    use windows::Win32::UI::WindowsAndMessaging::{GetIconInfo, ICONINFO};

    unsafe {
        let mut ii = ICONINFO::default();
        GetIconInfo(icon, &mut ii).ok()?;
        let read = |bmp: HBITMAP, w: i32, h: i32| -> Option<Vec<u8>> {
            let dc = CreateCompatibleDC(None);
            let mut bi = BITMAPINFO {
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
            let mut buf = vec![0u8; (w * h * 4) as usize];
            let lines = GetDIBits(dc, bmp, 0, h as u32, Some(buf.as_mut_ptr().cast()), &mut bi, DIB_RGB_COLORS);
            let _ = DeleteDC(dc);
            (lines == h).then_some(buf)
        };
        let result = (|| {
            if ii.hbmColor.is_invalid() {
                return None; // monochrome icons: not worth supporting
            }
            let mut bm = BITMAP::default();
            if GetObjectW(ii.hbmColor, std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut BITMAP as *mut _)) == 0 {
                return None;
            }
            let (w, h) = (bm.bmWidth, bm.bmHeight);
            let mut px = read(ii.hbmColor, w, h)?;
            // BGRA -> RGBA
            for p in px.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
            }
            // old-style icons have no alpha channel: take transparency from the mask
            if px.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
                let mask = read(ii.hbmMask, w, h)?;
                for (p, m) in px.as_chunks_mut::<4>().0.iter_mut().zip(mask.as_chunks::<4>().0) {
                    p[3] = if m[0] == 0 { 255 } else { 0 };
                }
            }
            Some(Rgba { width: w as usize, height: h as usize, pixels: px })
        })();
        let _ = DeleteObject(ii.hbmColor);
        let _ = DeleteObject(ii.hbmMask);
        result
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn visible(i: &Rgba) -> usize {
        i.pixels.as_chunks::<4>().0.iter().filter(|p| p[3] > 0).count()
    }

    #[test]
    fn loads_type_folder_and_program_icons() {
        init_com();
        for key in [IconKey::Folder, IconKey::Ext("txt".into()), IconKey::Ext("".into()), IconKey::Ext("zip".into())] {
            let i = load(&key, Path::new("")).unwrap_or_else(|| panic!("no icon for {key:?}"));
            assert!(i.width >= 16 && i.height >= 16, "{key:?}: {}x{}", i.width, i.height);
            assert_eq!(i.pixels.len(), i.width * i.height * 4);
            assert!(visible(&i) > 20, "{key:?} is blank");
            assert!(visible(&i) < i.width * i.height, "{key:?} has no transparency");
        }
        // a program's own icon, read from the file
        let notepad = PathBuf::from(std::env::var("WINDIR").unwrap()).join("notepad.exe");
        let own = load(&IconKey::File(notepad.clone()), &notepad).expect("notepad icon");
        let generic = load(&IconKey::Ext("exe".into()), Path::new("")).expect("exe icon");
        assert_ne!(own.pixels, generic.pixels, "notepad has its own icon");
    }
}
