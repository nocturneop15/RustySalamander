//! RustySalamander: a dual-pane file manager for Windows, a Unicode-native
//! rewrite of Open Salamander in Rust.
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
mod drive_icon;
#[cfg(test)]
mod exe_icon;
mod freespace;
mod i18n;
mod icons;
mod keymap;
mod panel;
mod plugins;
mod results;
mod shellmenu;
mod shortcut;
#[cfg(test)]
mod snapshot;
mod update;
mod worker;

use eframe::egui;
use sal_ui::fonts;

/// Start: the log, then the main window with the panels on the folders
/// given on the command line.
fn main() -> eframe::Result<()> {
    debuglog::init();
    // optional start directories: rustysalamander [left] [right]
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RustySalamander")
            .with_icon(sal_ui::icon::program())
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    debuglog::write("startup: creating the window");
    eframe::run_native("RustySalamander", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, args)))))
}
