//! RustySalamander: a modern, Unicode-native dual-pane file manager for
//! Windows, inspired by Altap Salamander.
//!
//! This crate is the GUI (egui/eframe). The file system logic that needs no
//! window lives in `sal-core`; plugins are separate DLLs that talk to the
//! program through `sal-plugin-api`.
//!
//! - `app`: the main window, keyboard model, dialogs and the headless UI tests
//! - `panel`: one file panel (listing, cursor, selection, history)
//! - `worker`: background file operations and their queue
//! - `plugins`: finding and loading plugin DLLs, paths through their files
//! - `dialogwin`: dialogs in their own native windows
//! - `i18n`: translations
//! - `update`: new releases on GitHub, and installing them
//! - `results`: the results window of a plugin's connection (search results)
//! - the rest: icons, context menus, shortcuts, folder sizes
//!
//! Viewing (F3) and editing (F4) go to the plugins registered for it (the
//! viewer is the Viewer plugin); fonts and scroll bars, which the plugins'
//! windows share, are in `sal-ui`.

// a release build has no console window; debug builds keep one for logs
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod ctxmenu;
mod debuglog;
mod dialogwin;
mod dirsize;
mod dnd;
mod gdiwin;
mod drive_icon;
#[cfg(test)]
mod exe_icon;
mod freespace;
mod i18n;
mod icons;
mod keymap;
mod panel;
mod pluginwin;
mod plugins;
mod rawkeys;
mod results;
mod shellmenu;
mod shortcut;
#[cfg(test)]
mod snapshot;
mod update;
mod worker;

use eframe::egui;
use sal_ui::fonts;

/// Whether frames wait for the screen's refresh (vsync), chosen at start.
static VSYNC: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// Whether frames wait for the screen's refresh: on a local screen yes, in
/// a Remote Desktop session no (there the refresh is only simulated, and
/// waiting for it held each frame about 94 ms: a held key moved the cursor
/// some 10 times a second). `SAL_VSYNC=on|off` chooses.
pub fn vsync() -> bool {
    *VSYNC.get_or_init(|| match std::env::var("SAL_VSYNC").as_deref() {
        Ok("on") => true,
        Ok("off") => false,
        _ => !remote_session(),
    })
}

/// Whether the program runs in a Remote Desktop session.
pub fn remote_session() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION};
    unsafe { GetSystemMetrics(SM_REMOTESESSION) != 0 }
}

/// Drawing through Direct3D 12 (wgpu): it works on every Windows 10/11,
/// also without a graphics driver (a virtual machine, Remote Desktop:
/// Windows' software renderer WARP). `WGPU_BACKEND` (`dx12`, `vulkan`)
/// chooses another. Without vsync ([`vsync`]) a frame is shown as soon as
/// it is drawn, with no frame queued ahead.
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::wgpu::PresentMode;
    let backends = eframe::wgpu::util::backend_bits_from_env().unwrap_or(eframe::wgpu::Backends::DX12);
    let vsync = vsync();
    debuglog::write(format!("startup: vsync {}{}", if vsync { "on" } else { "off" }, if std::env::var_os("SAL_VSYNC").is_some() { " (SAL_VSYNC)" } else { "" }));
    eframe::egui_wgpu::WgpuConfiguration {
        supported_backends: backends,
        present_mode: if vsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync },
        desired_maximum_frame_latency: if vsync { None } else { Some(1) },
        ..Default::default()
    }
}

/// Start: the log, then the main window with the panels on the folders
/// given on the command line.
fn main() -> eframe::Result<()> {
    // optional start directories: rustysalamander [left] [right]; -debug
    // writes the debug log (debuglog.rs)
    let (debug, args) = debuglog::parse_args(std::env::args().skip(1).collect());
    debuglog::init(debug, &args);
    // the version in the title bar ("RustySalamander 0.7.1-alpha"), and
    // that the debug log is being written
    let mut title = concat!("RustySalamander ", env!("CARGO_PKG_VERSION")).to_owned();
    if debuglog::full() {
        title.push_str(" [debug log]");
    }
    let viewport = egui::ViewportBuilder::default()
        .with_title(title)
        .with_icon(sal_ui::icon::program())
        .with_inner_size([1200.0, 760.0])
        .with_min_inner_size([640.0, 400.0])
        // the program takes drops itself (dnd.rs): where, and copy or move
        .with_drag_and_drop(false);
    if gdiwin::chosen() {
        // frames are paced by the program (`App::pace`), no screen refresh to wait for
        let _ = VSYNC.set(false);
        debuglog::write(format!("startup: creating the window (GDI: {})", gdiwin::choice().1));
        let r = gdiwin::run(viewport, args);
        match &r {
            Ok(()) => debuglog::write("---- RustySalamander closed"),
            Err(e) => debuglog::error(format!("the program could not run: {e}")),
        }
        return r.map_err(|e| eframe::Error::AppCreation(e.into()));
    }
    let options = eframe::NativeOptions { viewport, wgpu_options: wgpu_options(), ..Default::default() };
    debuglog::write("startup: creating the window");
    let r = eframe::run_native("RustySalamander", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, args)))));
    match &r {
        Ok(()) => debuglog::write("---- RustySalamander closed"),
        Err(e) => debuglog::error(format!("the program could not run: {e} ({e:?})")),
    }
    r
}
