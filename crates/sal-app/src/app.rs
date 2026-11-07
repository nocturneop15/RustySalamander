//! Main window: two panels, menu, function-key bar, dialogs
//! (replaces `mainwnd*.cpp`, `dialogs*.cpp`, `menu*.cpp`).

use std::collections::HashMap;
use std::path::{Path, PathBuf, MAIN_SEPARATOR};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, text::LayoutJob, Align, Color32, Event, Key, Layout, Modifiers, RichText, Sense, TextFormat};
use egui_extras::{Column, TableBuilder};
use sal_core::config::{Config, ContextMenuStyle, ContextMenuTrigger, DriveButtons, DriveHotkey, OpenWith, PanelConfig, SuffixRule, ThemeMode, TransferDefaults};
use sal_core::drives::{self, Drive};
use sal_core::encoding::{display_name, display_path, format_size, format_thousands, visual_order};
use sal_core::fileops::{self, ConflictAction, ErrorAction, OpKind, OpSummary, Verify};
use sal_core::listing::Visibility;
use sal_core::location::{self, Target};
use sal_core::shell::TerminalKind;
use sal_core::{EntryKind, FileEntry, MaskGroup, SortKey};

use crate::ctxmenu::{self, MenuAction};
use crate::dialogwin::{self, DialogWin};
use crate::dirsize::DirSizes;
use crate::dnd;
use crate::drive_icon;
use crate::i18n::{n_, tr, trf, trn};
use crate::icons::Icons;
use crate::keymap::{self, Action, Section};
use crate::panel::Panel;
use crate::plugins::ops::{FolderOp, OpenAs};
use crate::plugins::{self, FolderPlugin, Plugins};
use crate::shellmenu::{self, Merged, Picked};
use crate::shortcut::{key_letter, Shortcut};
use crate::worker::{format_duration, Job, JobKind, JobOptions, Jobs, Placement, PluginRef, Prompt, Reply, Spec};
use sal_plugin_api::{OpenKind, Opener};
use sal_ui::layout;
use sal_ui::scroll_bar_visibility;

const SELECTED_COLOR: Color32 = Color32::from_rgb(230, 40, 40);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cmd {
    Help,
    CheckUpdates,
    Find,
    Rename,
    View,
    Edit,
    Copy,
    Move,
    MkDir,
    Delete {
        permanent: bool,
    },
    Exit,
    Refresh,
    ToggleHidden,
    ToggleSystem,
    SetTheme(ThemeMode),
    /// Dark <-> light (from "system", the opposite of what is shown now).
    ToggleTheme,
    ToggleScrollbars,
    ToggleAnimations,
    ToggleFkeyBar,
    ToggleQuickSearchMode,
    ToggleIcons,
    /// Calculate sizes of the selected folders (or the folder under the cursor).
    CalcSelectionSize,
    /// Context menu for the cursor item (keyboard).
    ContextMenu,
    Properties,
    SelectAll,
    /// Toggle selection of the cursor item and calculate its size if it is a folder.
    SelectCalcSize,
    UnselectAll,
    Invert,
    SelectMask(bool),
    Sort(SortKey),
    SwapPanels,
    ChangeDir,
    DriveMenu(usize),
    Terminal,
    CopyPaths,
    /// The selected items (or the cursor item) on the clipboard, to copy
    /// or (`true`) to move, as Explorer's Ctrl+C / Ctrl+X do.
    ClipboardPut(bool),
    /// Files on the clipboard pasted into the panel's folder (Ctrl+V).
    ClipboardPaste,
    History(bool),
    HotPathGo(usize),
    HotPathSet(usize),
    Parent,
    Root,
    /// Switch the active panel to a drive letter.
    Drive(char),
    Settings,
    Enter,
}

enum Dialog {
    None,
    /// A form a plugin asked for (`sal_plugin_api::Form`); OK runs the form's
    /// command with `values`.
    PluginForm {
        plugin: String,
        form: plugins::Form,
        values: plugins::Values,
    },
    Transfer {
        kind: OpKind,
        sources: Vec<PathBuf>,
        target: String,
        /// In the background, the result window, safe copy: from Settings,
        /// changed for this operation only.
        opts: TransferDefaults,
    },
    MkDir {
        name: String,
    },
    Rename {
        path: PathBuf,
        name: String,
    },
    Delete {
        sources: Vec<PathBuf>,
        recycle: bool,
    },
    SelectMask {
        select: bool,
        mask: String,
        error: Option<String>,
    },
    /// Drive selector; `cursor` runs over `drives`, then `conns`, then `paths`.
    Drive {
        panel: usize,
        drives: Vec<Drive>,
        /// Open plugin connections (servers), opened at their start folder.
        conns: Vec<DriveConn>,
        paths: Vec<String>,
        cursor: usize,
    },
    Message {
        title: String,
        text: String,
    },
    Help,
    /// Newer releases of the program and of plugins.
    Update {
        rows: Vec<UpdateRow>,
    },
    /// `recording`: the next key combination is added to that action.
    Settings {
        tab: SettingsTab,
        recording: Option<Action>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    General,
    Keyboard,
    Programs,
    Plugins,
}

/// An update in the update window: `install` while (or after) it is being
/// installed, its changelog once asked for.
struct UpdateRow {
    update: crate::update::Update,
    install: Option<crate::update::Install>,
    changelog: Option<crate::update::Changelog>,
    show_log: bool,
}

impl UpdateRow {
    /// An update not yet installed, its changelog not yet read.
    fn new(update: crate::update::Update) -> UpdateRow {
        UpdateRow { update, install: None, changelog: None, show_log: false }
    }
}

/// A plugin as Settings shows it.
struct PluginRow {
    id: String,
    name: String,
    description: String,
    file: String,
    /// Its own version and the interface it was built for.
    version: String,
    /// The viewers and editors it registers, one line each.
    openers: Vec<String>,
    /// Running (its status) or why it couldn't start.
    state: Result<String, String>,
}

/// The command a configurable shortcut runs.
fn action_cmd(a: Action) -> Cmd {
    match a {
        Action::Help => Cmd::Help,
        Action::Rename => Cmd::Rename,
        Action::View => Cmd::View,
        Action::Edit => Cmd::Edit,
        Action::Copy => Cmd::Copy,
        Action::Move => Cmd::Move,
        Action::MkDir => Cmd::MkDir,
        Action::ChangeDir => Cmd::ChangeDir,
        Action::Delete => Cmd::Delete { permanent: false },
        Action::DeletePermanent => Cmd::Delete { permanent: true },
        Action::Exit => Cmd::Exit,
        Action::Terminal => Cmd::Terminal,
        Action::Settings => Cmd::Settings,
        Action::DriveMenuLeft => Cmd::DriveMenu(0),
        Action::DriveMenuRight => Cmd::DriveMenu(1),
        Action::Parent => Cmd::Parent,
        Action::EnterDir => Cmd::Enter,
        Action::Root => Cmd::Root,
        Action::HistoryBack => Cmd::History(true),
        Action::HistoryForward => Cmd::History(false),
        Action::Refresh => Cmd::Refresh,
        Action::ToggleHidden => Cmd::ToggleHidden,
        Action::ToggleSystem => Cmd::ToggleSystem,
        Action::ToggleTheme => Cmd::ToggleTheme,
        Action::CalcSelectionSize => Cmd::CalcSelectionSize,
        Action::ContextMenu => Cmd::ContextMenu,
        Action::ToggleQuickSearchMode => Cmd::ToggleQuickSearchMode,
        Action::Properties => Cmd::Properties,
        Action::SelectAll => Cmd::SelectAll,
        Action::UnselectAll => Cmd::UnselectAll,
        Action::SelectCalcSize => Cmd::SelectCalcSize,
        Action::SwapPanels => Cmd::SwapPanels,
        Action::CopyPaths => Cmd::CopyPaths,
        Action::ClipboardCopy => Cmd::ClipboardPut(false),
        Action::ClipboardCut => Cmd::ClipboardPut(true),
        Action::ClipboardPaste => Cmd::ClipboardPaste,
        Action::Find => Cmd::Find,
        Action::SortName => Cmd::Sort(SortKey::Name),
        Action::SortExt => Cmd::Sort(SortKey::Ext),
        Action::SortTime => Cmd::Sort(SortKey::Time),
        Action::SortSize => Cmd::Sort(SortKey::Size),
    }
}

/// Shortcut text shown next to a menu item.
fn menu_key(ov: &keymap::Overrides, c: Cmd) -> String {
    Action::ALL.into_iter().find(|&a| action_cmd(a) == c).and_then(|a| keymap::bindings(ov, a).into_iter().next()).unwrap_or_default()
}

/// One pending edit from the shortcut table, applied after drawing it.
enum KeyEdit {
    Set(Action, Vec<String>),
    Remove(Action, String),
    Record(Action),
    ResetAll,
}

/// A button drawn exactly into `rect` (inside a list row), whatever the
/// style's minimum button height: text in `font`, centred.
fn row_button(ui: &mut egui::Ui, rect: egui::Rect, text: &str, font: &egui::FontId, id: impl std::hash::Hash) -> egui::Response {
    let r = ui.interact(rect, ui.id().with(id), Sense::click());
    let v = ui.style().interact(&r);
    let p = ui.painter();
    p.rect(rect, 3.0, v.weak_bg_fill, v.bg_stroke);
    let g = p.layout_no_wrap(text.to_owned(), font.clone(), v.text_color());
    p.galley(rect.center() - g.size() / 2.0, g, v.text_color());
    r
}

/// An open plugin connection as the drive selector lists it.
struct DriveConn {
    /// `scp://me@host`
    label: String,
    /// The plugin's name.
    detail: String,
    root: PathBuf,
    start: PathBuf,
    /// Results a plugin filled (a search): closed, not disconnected.
    results: bool,
    /// One side of two folders compared (listed under Comparisons).
    paired: bool,
    /// The plugin command that fills results again; empty for none.
    refresh: String,
    plugin: String,
}

impl DriveConn {
    /// Its group in the drive selector: connections, searches, comparisons.
    fn group(&self) -> u8 {
        conn_group(self.results, self.paired)
    }
}

/// What was clicked in a list of connections beside the drive buttons.
#[derive(Debug, Clone, PartialEq)]
enum ConnPick {
    /// Go there: (start, root).
    Open(PathBuf, PathBuf),
    /// Fill the results again: (plugin id, root, command).
    Refresh(String, PathBuf, String),
    /// Disconnect, or close the results: root.
    Close(PathBuf),
}

/// A drop-down list of connections beside the drive buttons: a button with
/// `icon` (selected when the panel is in one of them) opening the list;
/// nothing when there are none. Each row has the drive selector's buttons:
/// Disconnect for a server, Refresh (when it can) and Close for results.
fn connection_list(ui: &mut egui::Ui, salt: (&str, usize), icon: &str, color: Color32, tip: &str, list: &[plugins::Connection], here: &Path) -> Option<ConnPick> {
    if list.is_empty() {
        return None;
    }
    let inside = list.iter().any(|c| here.starts_with(&c.root));
    // a button like the drives' (the same size, in line with them), with a list below
    let button = drive_icon::glyph_button(ui, icon, color, sal_ui::mdi::MENU_DOWN, inside).on_hover_text(tip);
    let popup = ui.make_persistent_id(salt);
    if button.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup));
    }
    let mut picked = None;
    egui::popup_below_widget(ui, popup, &button, egui::PopupCloseBehavior::CloseOnClick, |ui| {
        ui.set_min_width(220.0);
        // name | Refresh | Close or Disconnect, the buttons in columns
        egui::Grid::new((salt, "rows")).num_columns(3).spacing(layout::MENU_GRID_SPACING).show(ui, |ui| {
            for c in list {
                let label = if c.name.is_empty() { display_path(&c.root).trim_end_matches('/').to_owned() } else { c.name.clone() };
                let row_tip = display_path(&c.start).trim_end_matches('/').to_owned();
                if ui.selectable_label(here.starts_with(&c.root), label).on_hover_text(row_tip).clicked() {
                    picked = Some(ConnPick::Open(c.start.clone(), c.root.clone()));
                }
                if c.refresh.is_empty() {
                    ui.label("");
                } else if ui.small_button(tr("Refresh")).on_hover_text(tr("Search again with the same settings")).clicked() {
                    picked = Some(ConnPick::Refresh(c.plugin.clone(), c.root.clone(), c.refresh.clone()));
                }
                let (close, close_tip) = if c.results { (tr("Close"), tr("Close these results")) } else { (tr("Disconnect"), tr("Close this connection")) };
                if ui.small_button(close).on_hover_text(close_tip).clicked() {
                    picked = Some(ConnPick::Close(c.root.clone()));
                }
                ui.end_row();
            }
        });
    });
    picked
}

/// The group a connection is listed in: 0 connections (servers), 1
/// searches, 2 comparisons.
fn conn_group(results: bool, paired: bool) -> u8 {
    match (results, paired) {
        (_, true) => 2,
        (true, false) => 1,
        _ => 0,
    }
}

/// The heading of a group of connections (see `conn_group`).
fn conn_group_name(group: u8) -> &'static str {
    match group {
        2 => tr("Comparisons"),
        1 => tr("Searches"),
        _ => tr("Connections"),
    }
}

/// A plugin command running in the background, and where its reply arrives.
struct PluginTask {
    plugin: Arc<dyn FolderPlugin>,
    reply: std::sync::mpsc::Receiver<std::io::Result<plugins::Reply>>,
}

pub struct App {
    cfg: Config,
    /// A look for a new release, until its answer is shown.
    update_check: Option<crate::update::Check>,
    /// The results window of a plugin's connection (search results).
    results: Option<crate::results::ResultsWin>,
    /// The results kept in this session, oldest first: each one's
    /// connections (folders, flat list). At most [`MAX_RESULTS`].
    kept_results: Vec<Vec<PathBuf>>,
    /// The suffix form open in Settings → Viewers and editors.
    suffix_edit: Option<SuffixEdit>,
    /// Connections shown side by side (left root, right root): the panels
    /// go into their folders together ([`App::follow_links`]).
    links: Vec<(PathBuf, PathBuf)>,
    /// The panels' folders when the links were last followed.
    linked_at: [PathBuf; 2],
    /// Results being filled again (Refresh): their plugin, connections and
    /// since when; panels showing them follow until the plugin is done.
    refreshing: Vec<(Arc<dyn FolderPlugin>, Vec<PathBuf>, Instant)>,
    refresh_polled: Option<Instant>,
    panels: [Panel; 2],
    active: usize,
    dialog: Dialog,
    /// Give keyboard focus to the dialog's text field on the next frame.
    focus_dialog: bool,
    last_mask: String,
    /// Running and queued file operations.
    jobs: Jobs,
    /// The plugins that are on (Settings → Plugins).
    plugins: Plugins,
    /// A plugin command running in the background (e.g. connecting).
    plugin_task: Option<PluginTask>,
    /// Where the next operation goes (set by the Copy/Move dialog's queue buttons).
    queue_place: Placement,
    /// How the next operation [`Self::start_job`] starts runs (taken by it).
    job_options: JobOptions,
    /// Operations that ended and asked for their result to be shown, the
    /// newest last; their window is open while there are any.
    reports: Vec<Report>,
    /// Where the protocol of the operations in the result window was saved.
    protocol_saved: Option<PathBuf>,
    status: String,
    drives: Vec<Drive>,
    drives_loaded: Option<Instant>,
    /// Last directory visited per drive root (upper-case), for drive changes.
    last_dir: HashMap<String, PathBuf>,
    /// A key just acted as a shortcut: drop the text it typed in this frame.
    skip_text: bool,
    /// Path bar being edited inline, per panel.
    path_edit: [Option<PathEdit>; 2],
    /// Clickable breadcrumb areas from the last frame, per panel.
    crumb_rects: [Vec<(egui::Rect, PathBuf)>; 2],
    dir_sizes: DirSizes,
    icons: Icons,
    /// Installed fonts (read when Settings first opens).
    font_list: Vec<crate::fonts::SystemFont>,
    /// Languages offered in Options → Language.
    languages: Vec<crate::i18n::LangInfo>,
    /// Search text of the font pickers.
    font_filter: String,
    /// Where the UI font picker's button was drawn (tests click it).
    font_button_rect: Option<egui::Rect>,
    /// Where the drive selector was drawn.
    drive_popup_rect: Option<egui::Rect>,
    /// Dialogs shown in their own windows.
    dialog_places: dialogwin::Places,
    /// Where the font size slider was drawn (tests drag it).
    font_slider_rect: Option<egui::Rect>,
    /// Items of an explicit size calculation, reported in the status line when done.
    size_report: Option<(usize, Vec<PathBuf>)>,
    /// Native window handle (for the shell menu and shell dialogs).
    hwnd: Option<isize>,
    /// Items dragged from a panel, while the button is held.
    panel_drag: Option<PanelDrag>,
    /// What Windows' drop handler and the window share: the drop zones, and
    /// files dragged in from other programs.
    drop_shared: Option<dnd::SharedRef>,
    /// Windows' drag (out of the window) took the button's release: egui
    /// gets it in the next frame.
    release_pointer: bool,
    /// What the last operation did, shown in the status bar for a moment
    /// ([`JOB_DONE_SHOWN`]) instead of "No operations running".
    job_done: Option<(String, Instant)>,
    ctx_menu: Option<CtxMenu>,
    /// When the SAL_DIAG_SEARCH diagnostics started their search.
    diag_since: Option<Instant>,
    /// Whether Windows animates the main window (minimizing, closing), as
    /// last set: it follows the Animations setting.
    transitions: Option<bool>,
    rmb: Option<RightPress>,
    /// Row rectangles of the last frame (screen points), per panel.
    row_hits: [Vec<(egui::Rect, usize)>; 2],
    /// File list area of each panel.
    body_rects: [egui::Rect; 2],
    /// Each panel's Ext, Size and Date widths (the user may drag them).
    col_widths: [Option<[f32; 3]>; 2],
    /// Each panel's column headers as last drawn (Name, Ext, Size, Date, Attr).
    header_rects: [Vec<egui::Rect>; 2],
    /// Free space of the panels' drives, read in the background.
    free: crate::freespace::FreeSpace,
    /// What the debug log last saw of the program's state (`-debug`).
    debug_watch: crate::debuglog::Watch,
    /// The plugins' windows, which the program draws.
    plugin_windows: crate::pluginwin::Windows,
    /// When the last frame began (pacing without vsync, and the debug log).
    last_frame: Option<Instant>,
}

/// An open Windows 11 style context menu.
struct CtxMenu {
    panel: usize,
    /// Items it applies to; empty for the folder background.
    items: Vec<PathBuf>,
    dir: PathBuf,
    pos: egui::Pos2,
    can_paste: bool,
    /// The items are the files on disk a plugin's items stand for: no
    /// rename, paste or size calculation, which are the panel's own.
    plugin: bool,
}

/// Right button pressed over a panel (for click vs. long press).
struct RightPress {
    panel: usize,
    row: Option<usize>,
    start: f64,
    pos: egui::Pos2,
    /// Held long enough: the menu opens on release.
    armed: bool,
}

struct PathEdit {
    text: String,
    /// Just opened: take focus and select the whole path.
    fresh: bool,
}

/// The key a drive's last folder is remembered by: its root, any case.
fn drive_key(root: &Path) -> String {
    display_path(root).to_uppercase()
}

#[derive(Debug, Default, PartialEq, Eq)]
struct SelectionTotals {
    files: usize,
    dirs: usize,
    /// File bytes plus the sizes of selected folders calculated so far.
    bytes: u64,
    /// Selected folders still being calculated / never calculated.
    calculating: usize,
    unknown: usize,
    /// Files and folders found inside the selected folders sized so far.
    inner_files: u64,
    inner_dirs: u64,
}

impl SelectionTotals {
    /// "1 file, 2 directories"
    fn items(&self) -> String {
        let mut parts = Vec::new();
        if self.files > 0 {
            parts.push(plural(self.files, "file"));
        }
        if self.dirs > 0 {
            parts.push(plural_dirs(self.dirs));
        }
        parts.join(", ")
    }

    /// Files including those inside sized folders, when any folder was sized.
    fn all_files(&self) -> Option<u64> {
        (self.dirs > self.unknown).then_some(self.files as u64 + self.inner_files)
    }

    /// "4 512 files, 87 subdirectories": everything counted, including the
    /// contents of sized folders (`None` until a folder has been sized).
    fn contents(&self) -> Option<String> {
        self.all_files().map(|files| format!("{}, {}", plural_u64(files, "file"), plural_subdirs(self.inner_dirs)))
    }
}

/// "1 subdirectory", "87 subdirectories"
fn plural_subdirs(n: u64) -> String {
    trn("{n} subdirectory", "{n} subdirectories", n)
}

/// "1 directory", "12 directories" (translated).
fn plural_dirs(n: usize) -> String {
    trn("{n} directory", "{n} directories", n as u64)
}

/// Size and counts of the selected items, with the folder sizes known so far.
fn selection_totals(entries: &[sal_core::FileEntry], sizes: &HashMap<PathBuf, crate::dirsize::SizeInfo>) -> SelectionTotals {
    let mut t = SelectionTotals::default();
    for e in entries.iter().filter(|e| e.selected) {
        match e.kind {
            EntryKind::File => {
                t.files += 1;
                t.bytes += e.size;
            }
            _ => {
                t.dirs += 1;
                match sizes.get(&e.path) {
                    Some(si) => {
                        t.bytes += si.size.bytes;
                        t.inner_files += si.size.files;
                        t.inner_dirs += si.size.dirs;
                        if !si.done {
                            t.calculating += 1;
                        }
                    }
                    None => t.unknown += 1,
                }
            }
        }
    }
    t
}

/// Open a file with the program Windows associates with it. Tests only
/// record it, so they never start anything on the desktop.
fn shell_open(p: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    {
        tests::OPENED.with(|o| o.borrow_mut().push(p.to_path_buf()));
        Ok(())
    }
    #[cfg(not(test))]
    sal_core::shell::open(p)
}

/// Which hidden and system items the panels show.
fn visibility(cfg: &Config) -> Visibility {
    Visibility { hidden: cfg.show_hidden, system: cfg.show_system }
}

/// A date and time as the panels show it (`2026-10-31 21:15`), local time.
pub(crate) fn fmt_time(t: Option<std::time::SystemTime>) -> String {
    t.map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
}

/// "1 file", "12 345 files" (translated)
fn plural_u64(n: u64, what: &str) -> String {
    match what {
        "file" => trn("{n} file", "{n} files", n),
        "folder" => trn("{n} folder", "{n} folders", n),
        "path" => trn("{n} path", "{n} paths", n),
        "item" => trn("{n} item", "{n} items", n),
        "directory" => trn("{n} directory", "{n} directories", n),
        _ => unreachable!("no plural forms for {what}"),
    }
}

/// "1 file", "12 files": `n` of `what`, translated.
fn plural(n: usize, what: &str) -> String {
    plural_u64(n as u64, what)
}

/// "C: System — Local disk (\\server\share)", translated.
fn describe_drive(d: &Drive) -> String {
    let mut s = d.letter.clone();
    if !d.label.is_empty() {
        s.push(' ');
        s.push_str(&d.label);
    }
    s.push_str(" — ");
    s.push_str(tr(d.kind.label()));
    if let Some(r) = &d.remote {
        s.push_str(&format!(" ({r})"));
    }
    s
}

/// The items of an operation for its question: the name of one, or how many.
/// A folder as the Copy and Move dialogs show their target: ending with
/// a separator (`/` for a connection's URL).
fn transfer_target(folder: &Path) -> String {
    let mut target = display_path(folder);
    let sep = if location::virtual_parts(folder).is_some() { '/' } else { MAIN_SEPARATOR };
    if !target.ends_with(sep) {
        target.push(sep);
    }
    target
}

/// Items dragged from a panel.
struct PanelDrag {
    sources: Vec<PathBuf>,
    /// Their volume, for the copy-or-move rule.
    volume: String,
    /// What a drag out of the window gives other programs.
    outside: DragOut,
}

/// What items dragged from a panel are out of the window.
enum DragOut {
    /// The files on disk they are, or a plugin's items stand for.
    Files(Vec<PathBuf>),
    /// A plugin's items (an archive's, a server's): files in the plugin's
    /// mount, read from the plugin only as other programs read them.
    Mount,
    /// Nothing (a comparison's placeholders): they stay in the window.
    No,
}

/// A drag over the window: the folder or panel it would go into outlined,
/// and at the pointer what would happen.
fn paint_drop(ctx: &egui::Context, pos: egui::Pos2, sources: &[PathBuf], hit: Option<&(dnd::Zone, OpKind)>) {
    let what = describe(sources);
    let text = match hit {
        Some((z, kind)) => {
            let folder = z.folder.file_name().map(|n| display_name(&n.to_string_lossy()).into_owned()).unwrap_or_else(|| display_path(&z.folder));
            let icon = if *kind == OpKind::Move { sal_ui::mdi::CONTENT_CUT } else { sal_ui::mdi::CONTENT_COPY };
            let line = if *kind == OpKind::Move {
                trf("Move {what} to {folder}", &[("what", &what), ("folder", &folder)])
            } else {
                trf("Copy {what} to {folder}", &[("what", &what), ("folder", &folder)])
            };
            format!("{icon} {line}")
        }
        None => what,
    };
    if let Some((z, _)) = hit {
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop_zone")));
        let accent = ctx.style().visuals.selection.bg_fill;
        painter.rect_stroke(z.rect.shrink(1.0), 2.0f32, egui::Stroke::new(2.0f32, accent));
    }
    egui::Area::new(egui::Id::new("drag_label")).order(egui::Order::Tooltip).fixed_pos(pos + egui::vec2(16.0, 18.0)).interactable(false).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            let mut t = RichText::new(text);
            if hit.is_none() {
                t = t.weak();
            }
            ui.add(egui::Label::new(t).extend());
        });
    });
}

fn describe(sources: &[PathBuf]) -> String {
    match sources {
        [one] => format!("\"{}\"", one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        many => trn("{n} selected item", "{n} selected items", many.len() as u64),
    }
}

impl App {
    /// The application as it starts: the saved settings, fonts, plugins and
    /// the folders given on the command line.
    pub fn new(cc: &eframe::CreationContext<'_>, args: Vec<String>) -> App {
        if let Some(rs) = &cc.wgpu_render_state {
            let a = rs.adapter.get_info();
            crate::debuglog::write(format!(
                "startup: graphics {:?} on {} ({:?}, driver {} {}, format {:?})",
                a.backend, a.name, a.device_type, a.driver, a.driver_info, rs.target_format
            ));
        }
        App::started(&cc.egui_ctx, args)
    }

    /// The application in its window, whatever draws it (eframe's wgpu, or
    /// the program's own windows drawn with GDI, `gdiwin`).
    pub fn started(ctx: &egui::Context, args: Vec<String>) -> App {
        crate::debuglog::write("startup: window ready");
        let cfg = crate::debuglog::time("startup: configuration", Config::load);
        crate::debuglog::write(format!("startup: configuration file {}", sal_core::config::config_path().display()));
        crate::debuglog::time("startup: fonts", || crate::fonts::install(ctx, &cfg.ui_font, &cfg.mono_font));
        let mut app = App::with_config(ctx, cfg, args);
        if let (true, Some(path)) = (crate::debuglog::full(), crate::debuglog::path()) {
            app.status = trf("Debug log: {file}", &[("file", &path.display().to_string())]);
        }
        app
    }

    /// The application with `cfg` (the tests' entry point): panels on the
    /// folders given or saved, plugins started, the update check begun.
    pub fn with_config(ctx: &egui::Context, mut cfg: Config, args: Vec<String>) -> App {
        if let Some(l) = args.first() {
            cfg.left.path = l.clone();
        }
        if let Some(r) = args.get(1) {
            cfg.right.path = r.clone();
        }
        // configs from before the shortcut table had a single root shortcut
        if let Some(old) = cfg.root_shortcut.take() {
            let old = keymap::normalize(&old);
            let mut list = keymap::defaults(Action::Root);
            if list.first() != Some(&old) {
                list[0] = old;
                keymap::set_bindings(&mut cfg.shortcuts, Action::Root, list);
            }
        }
        use crate::debuglog::time;
        time("startup: language", || crate::i18n::set_language(&cfg.language));
        time("startup: font size, theme, scrollbars", || {
            apply_font_size(ctx, cfg.font_size);
            apply_theme(ctx, cfg.theme);
            sal_ui::apply_scrollbars(ctx, cfg.always_show_scrollbars);
            apply_animations(ctx, cfg.animations);
            sal_ui::apply_accent(ctx, cfg.accent_light, cfg.accent_dark);
        });
        let plugins = time("startup: plugins", || Plugins::load(&cfg.disabled_plugins));
        if crate::debuglog::enabled() {
            let names: Vec<&str> = plugins.active.iter().map(|p| p.id()).collect();
            crate::debuglog::write(format!("startup: plugins running {names:?}, failed {:?}, unusable {:?}", plugins.errors, plugins.broken));
        }
        let panels = [
            time(&format!("startup: left panel {}", cfg.left.path), || Panel::new(&cfg.left.path, cfg.left.sort, visibility(&cfg), plugins.active.clone())),
            time(&format!("startup: right panel {}", cfg.right.path), || Panel::new(&cfg.right.path, cfg.right.sort, visibility(&cfg), plugins.active.clone())),
        ];
        let icons = time("startup: icon loader", || Icons::new(ctx));
        let languages = time("startup: language list", crate::i18n::available);
        let active = usize::from(cfg.right_active);
        // the headless tests never go online
        let update_check = (cfg.check_updates && !cfg!(test)).then(|| crate::update::Check::start(ctx, false, update_sources(&plugins), update_offer(&cfg, false)));
        App {
            cfg,
            update_check,
            results: None,
            kept_results: Vec::new(),
            suffix_edit: None,
            links: Vec::new(),
            linked_at: Default::default(),
            refreshing: Vec::new(),
            refresh_polled: None,
            panels,
            active,
            dialog: Dialog::None,
            focus_dialog: false,
            last_mask: "*.*".into(),
            jobs: Jobs::default(),
            plugins,
            plugin_task: None,
            queue_place: Placement::Now,
            job_options: JobOptions::default(),
            reports: Vec::new(),
            protocol_saved: None,
            status: String::new(),
            drives: Vec::new(),
            drives_loaded: None,
            last_dir: HashMap::new(),
            skip_text: false,
            path_edit: [None, None],
            crumb_rects: [Vec::new(), Vec::new()],
            dir_sizes: DirSizes::default(),
            icons,
            font_slider_rect: None,
            drive_popup_rect: None,
            dialog_places: Default::default(),
            font_list: Vec::new(),
            languages,
            font_button_rect: None,
            font_filter: String::new(),
            size_report: None,
            hwnd: None,
            panel_drag: None,
            debug_watch: Default::default(),
            plugin_windows: Default::default(),
            last_frame: None,
            drop_shared: None,
            release_pointer: false,
            job_done: None,
            ctx_menu: None,
            diag_since: None,
            transitions: None,
            rmb: None,
            row_hits: [Vec::new(), Vec::new()],
            body_rects: [egui::Rect::NOTHING; 2],
            col_widths: [None; 2],
            header_rects: [Vec::new(), Vec::new()],
            free: Default::default(),
        }
    }

    /// Which hidden and system items the panels show.
    fn vis(&self) -> Visibility {
        visibility(&self.cfg)
    }

    /// Plugins turned on or off (Settings): start them again; a panel inside
    /// an archive whose plugin is off now goes to the archive's folder.
    fn apply_plugins(&mut self) {
        self.plugins.start(&self.cfg.disabled_plugins);
        let hidden = self.vis();
        let active = self.plugins.active.clone();
        // a plugin turned off ended its connections and results
        let stopped: Vec<String> = self.plugins.found.iter().map(|f| f.info.id.clone()).filter(|id| !active.iter().any(|p| p.id() == id)).collect();
        plugins::forget_mounts_of(&stopped);
        if self.results.as_ref().is_some_and(|r| !active.iter().any(|p| p.id() == r.plugin.id())) {
            self.results = None;
        }
        let open = plugins::mounts();
        self.kept_results.retain(|set| set.iter().any(|r| open.iter().any(|(o, _)| o == r)));
        self.refreshing.retain(|(p, _, _)| active.iter().any(|a| a.id() == p.id()));
        for p in &mut self.panels {
            p.plugins = active.clone();
            let gone = p.archive.as_ref().is_some_and(|a| !active.iter().any(|x| x.id() == a.plugin.id()));
            if gone && location::virtual_parts(p.path()).is_some() {
                p.archive = None;
                p.leave_connection(hidden);
            } else if gone {
                if let Some(a) = p.archive.take() {
                    let name = a.file.file_name().map(|n| n.to_string_lossy().into_owned());
                    let dir = a.file.parent().map(Path::to_path_buf).unwrap_or_default();
                    let _ = p.load(&dir, hidden, name.as_deref());
                }
            }
        }
    }

    /// The plugin, archive and path inside it, when `p` is an item inside an
    /// archive a plugin shows (not the archive file itself).
    fn in_archive(&self, p: &Path) -> Option<(Arc<dyn FolderPlugin>, PathBuf, String)> {
        self.archive_folder(p).filter(|(_, _, inner)| !inner.is_empty())
    }

    /// Like [`Self::in_archive`], but the archive's root counts too (a target folder).
    fn archive_folder(&self, p: &Path) -> Option<(Arc<dyn FolderPlugin>, PathBuf, String)> {
        self.plugins.split(p)
    }

    /// Extract a file from an archive into a temporary folder, then open it.
    /// Run a plugin's command in the background; its reply is handled when
    /// it arrives ([`Self::poll_plugin_task`]). The plugin gets the active
    /// panel's path, and its connection shown there, with the values.
    fn run_plugin_command(&mut self, ctx: &egui::Context, plugin: Arc<dyn FolderPlugin>, command: String, mut values: plugins::Values) {
        if crate::debuglog::full() {
            // what was typed, but no passwords
            let shown: Vec<String> = values.iter().map(|(k, v)| if is_secret(k) { format!("{k}=(hidden)") } else { format!("{k}={v:?}") }).collect();
            crate::debuglog::did(format_args!("plugin {} command {command:?} {}", plugin.id(), shown.join(" ")));
        }
        if self.plugin_task.is_some() {
            self.status = tr("A plugin command is still running.").into();
            return;
        }
        let here = self.act().path().to_path_buf();
        values.insert("panel".into(), sal_plugin_api::ui::path_value(&here));
        // both panels' folders, cursors and selections
        let item = |p: &Panel| p.current().filter(|e| e.kind != EntryKind::UpDir).map(|e| sal_plugin_api::ui::path_value(&e.path)).unwrap_or_default();
        let selected = |p: &Panel| {
            let list: Vec<String> = p.entries().iter().filter(|e| e.selected).map(|e| sal_plugin_api::ui::path_value(&e.path)).collect();
            sal_plugin_api::ui::encode(&list)
        };
        let (act, other) = (&self.panels[self.active], &self.panels[1 - self.active]);
        values.insert("side".into(), (if self.active == 0 { "left" } else { "right" }).into());
        values.insert("cursor".into(), item(act));
        values.insert("selected".into(), selected(act));
        values.insert("other".into(), sal_plugin_api::ui::path_value(other.path()));
        values.insert("other_cursor".into(), item(other));
        values.insert("other_selected".into(), selected(other));
        // a window the command opens (a comparison) opens over the program's
        values.insert("owner".into(), self.owner_value());
        match plugins::mount_root_of(&here) {
            Some((root, id)) if id == plugin.id() => values.insert("mount".into(), sal_plugin_api::ui::path_value(&root)),
            _ => values.remove("mount"),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let (p, wake) = (plugin.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(p.run(&command, &values));
            wake.request_repaint();
        });
        self.status = trf("{plugin}: working…", &[("plugin", &self.plugins.name_of(plugin.id()))]);
        self.plugin_task = Some(PluginTask { plugin, reply: rx });
    }

    /// Handle the reply of the plugin command running in the background.
    fn poll_plugin_task(&mut self, ctx: &egui::Context) {
        let Some(task) = &self.plugin_task else { return };
        let reply = match task.reply.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100));
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(std::io::Error::other(tr("The plugin stopped."))),
        };
        let plugin = self.plugin_task.take().map(|t| t.plugin).expect("task");
        self.status.clear();
        let name = self.plugins.name_of(plugin.id());
        match reply {
            Ok(plugins::Reply::Done) => {}
            Ok(plugins::Reply::Message(text)) => self.open_dialog(Dialog::Message { title: name, text }),
            Ok(plugins::Reply::Form(form)) => {
                let values = form.fields.iter().map(|f| (f.id.clone(), f.value.clone())).collect();
                self.open_dialog(Dialog::PluginForm { plugin: plugin.id().to_owned(), form, values });
            }
            Ok(plugins::Reply::Mount(m)) => self.open_connection(&plugin, &m),
            Ok(plugins::Reply::Results(r)) => self.open_results(plugin, r),
            Ok(plugins::Reply::Panels(p)) => self.open_panels(&plugin, p),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => self.open_dialog(Dialog::Message { title: name, text: e.to_string() }),
        }
    }

    /// Show a connection a plugin opened in the active panel.
    fn open_connection(&mut self, plugin: &Arc<dyn FolderPlugin>, m: &plugins::Mount) {
        let root = m.root();
        let hidden = self.vis();
        let start = if m.folder.is_empty() { root.clone() } else { root.join(&m.folder) };
        plugins::add_mount(&root, plugin.id(), &start, None);
        let opened = self.act().load(&start, hidden, None).or_else(|_| self.act().load(&root, hidden, None));
        match opened {
            Ok(()) => self.status = trf("Connected to {where}.", &[("where", &display_path(&root))]),
            Err(e) => {
                plugins::remove_mount(&root);
                let p = plugin.clone();
                std::thread::spawn(move || p.close(&root));
                self.error(e);
            }
        }
    }

    /// Show two connections side by side (two folders compared): the left
    /// one in the left panel, the right one in the right; they are linked,
    /// so the panels go into the same folders together.
    fn open_panels(&mut self, plugin: &Arc<dyn FolderPlugin>, p: plugins::Panels) {
        // one comparison at a time: the one before closes (both its sides)
        for c in plugins::connections().into_iter().filter(|c| c.paired) {
            self.close_connection(&c.root);
        }
        let hidden = self.vis();
        let mut roots = Vec::new();
        for (i, (m, side)) in [(&p.left, tr("left")), (&p.right, tr("right"))].into_iter().enumerate() {
            let root = m.root();
            let start = if m.folder.is_empty() { root.clone() } else { root.join(&m.folder) };
            plugins::add_mount(&root, plugin.id(), &start, Some((&format!("{} ({side})", p.name), &p.refresh)));
            plugins::set_paired(&root);
            if let Err(e) = self.panels[i].load(&start, hidden, None).or_else(|_| self.panels[i].load(&root, hidden, None)) {
                self.error(e);
            }
            roots.push(root);
        }
        self.links.retain(|(l, r)| !roots.contains(l) && !roots.contains(r));
        self.links.push((roots[0].clone(), roots[1].clone()));
        self.linked_at = [self.panels[0].path().to_path_buf(), self.panels[1].path().to_path_buf()];
        self.status = p.name.clone();
    }

    /// The comparison's sides in their panels, where a panel doesn't show
    /// its side already.
    fn show_comparison(&mut self) {
        let Some((left, right)) = self.links.first().cloned() else { return };
        let hidden = self.vis();
        for (i, side) in [left, right].iter().enumerate() {
            if !self.panels[i].path().starts_with(side) {
                if let Err(e) = self.panels[i].load(side, hidden, None) {
                    self.error(e);
                }
            }
        }
        self.linked_at = [self.panels[0].path().to_path_buf(), self.panels[1].path().to_path_buf()];
    }

    /// Linked panels (two folders compared) go into the same folders
    /// together: when one panel moved inside its connection, the other
    /// follows to the same place in its own.
    fn follow_links(&mut self) {
        let now = [self.panels[0].path().to_path_buf(), self.panels[1].path().to_path_buf()];
        if now == self.linked_at {
            return;
        }
        let hidden = self.vis();
        let mut now = now;
        for (left, right) in self.links.clone() {
            let roots = [left, right];
            // a side shown in the other panel (typed, from history, panels
            // swapped): each panel keeps its own side, at the same place
            for i in 0..2 {
                if let Ok(inner) = now[i].strip_prefix(&roots[1 - i]) {
                    let own = roots[i].join(inner);
                    if self.panels[i].load(&own, hidden, None).is_err() {
                        let _ = self.panels[i].load(&roots[i], hidden, None);
                    }
                    now[i] = self.panels[i].path().to_path_buf();
                }
            }
            let inside: Vec<Option<PathBuf>> = (0..2).map(|i| now[i].strip_prefix(&roots[i]).ok().map(Path::to_path_buf)).collect();
            let (Some(a), Some(b)) = (&inside[0], &inside[1]) else { continue };
            if a == b {
                continue;
            }
            // the panel that moved leads; when both did, the active one
            let leader = if now[0] != self.linked_at[0] && now[1] == self.linked_at[1] {
                0
            } else if now[1] != self.linked_at[1] && now[0] == self.linked_at[0] {
                1
            } else {
                self.active
            };
            let target = roots[1 - leader].join(inside[leader].as_ref().unwrap());
            let focus = now[1 - leader].file_name().map(|n| n.to_string_lossy().into_owned());
            let _ = self.panels[1 - leader].load(&target, hidden, focus.as_deref());
        }
        self.linked_at = [self.panels[0].path().to_path_buf(), self.panels[1].path().to_path_buf()];
    }

    /// Show a plugin's results (a connection it fills, such as a search) in
    /// the results window. The results it showed before go as if the window
    /// was closed: they stay (in the drive selector's Searches) only where a
    /// panel shows them, until there are more than [`MAX_RESULTS`].
    fn open_results(&mut self, plugin: Arc<dyn FolderPlugin>, r: plugins::Results) {
        self.close_results();
        let root = r.mount.root();
        let start = if r.mount.folder.is_empty() { root.clone() } else { root.join(&r.mount.folder) };
        let name = if r.name.is_empty() { r.title.clone() } else { r.name.clone() };
        plugins::add_mount(&root, plugin.id(), &start, Some((&trf("{name} (filtered folders)", &[("name", &name)]), &r.refresh)));
        let flat = (!r.flat.is_empty()).then(|| plugins::Mount { authority: r.flat.clone(), ..r.mount.clone() }.root());
        if let Some(f) = &flat {
            plugins::add_mount(f, plugin.id(), f, Some((&trf("{name} (all in one list)", &[("name", &name)]), &r.refresh)));
        }
        // the oldest results go when there are too many
        let open = plugins::mounts();
        self.kept_results.retain(|set| set.iter().any(|r| open.iter().any(|(o, _)| o == r)));
        self.kept_results.push(std::iter::once(root.clone()).chain(flat.clone()).collect());
        while self.kept_results.len() > MAX_RESULTS {
            for r in self.kept_results.remove(0) {
                self.close_connection(&r);
            }
            self.status.clear();
        }
        self.results = Some(crate::results::ResultsWin::new(plugin, root, flat, r.title, r.again, r.refresh));
    }

    /// Close the results window; its connections too, unless a panel shows
    /// one (it then stays until closed in the drive selector).
    fn close_results(&mut self) {
        let Some(r) = self.results.take() else { return };
        for root in std::iter::once(r.root).chain(r.flat) {
            if !self.panels.iter().any(|p| p.path().starts_with(&root)) {
                self.close_connection(&root);
                self.status.clear();
            }
        }
    }

    /// The results window, and what its buttons do.
    fn results_ui(&mut self, ctx: &egui::Context) {
        let Some(win) = &mut self.results else {
            dialogwin::close(&mut self.dialog_places, self.hwnd, |k| k == "results");
            return;
        };
        // closed meanwhile (drive selector, plugin turned off)
        if !plugins::mounts().iter().any(|(r, _)| *r == win.root || Some(r) == win.flat.as_ref()) {
            self.results = None;
            return;
        }
        win.poll(ctx);
        let dw = DialogWin { owner: self.hwnd, theme: self.cfg.theme, modal: false, exclusive: false, icon: None, animations: animated(self.cfg.animations) };
        let (mut enter, mut escape) = (false, false);
        let height = (ctx.screen_rect().height() * 0.55).clamp(160.0, 520.0);
        let title = win.title.clone();
        let mut act = None;
        dw.show(ctx, &mut self.dialog_places, "results", &title, 820.0, &mut enter, &mut escape, &mut |ui| {
            act = win.ui(ui, height);
        });
        if enter && act.is_none() {
            act = win.selected().map(crate::results::Act::Open);
        }
        if escape {
            act = Some(crate::results::Act::Close);
        }
        // panels showing the results follow them
        if win.take_changed() {
            let (roots, hidden) = (std::iter::once(win.root.clone()).chain(win.flat.clone()).collect::<Vec<_>>(), self.vis());
            for p in self.panels.iter_mut().filter(|p| roots.iter().any(|r| p.path().starts_with(r))) {
                p.reload(hidden);
            }
        }
        if let Some(act) = act {
            self.results_act(ctx, act);
        }
    }

    /// Fill results again (Refresh, in the results window or the drive
    /// selector): the plugin runs `command` with them; the window and the
    /// panels showing them follow until it is done.
    fn refresh_results(&mut self, ctx: &egui::Context, plugin: Arc<dyn FolderPlugin>, root: &Path, command: String) {
        if command.is_empty() || self.plugin_task.is_some() {
            self.status = tr("A plugin command is still running.").into();
            return;
        }
        let values: plugins::Values = [("results".to_owned(), sal_plugin_api::ui::path_value(root))].into();
        self.run_plugin_command(ctx, plugin.clone(), command, values);
        let mut roots = self.kept_results.iter().find(|set| set.iter().any(|r| r == root)).cloned().unwrap_or_else(|| vec![root.to_path_buf()]);
        // two folders compared are filled again together
        for (l, r) in &self.links {
            if l == root || r == root {
                roots = vec![l.clone(), r.clone()];
            }
        }
        if let Some(win) = self.results.as_mut().filter(|w| roots.contains(&w.root)) {
            win.refreshing();
        }
        self.refreshing.retain(|(_, r, _)| *r != roots);
        self.refreshing.push((plugin, roots, Instant::now()));
    }

    /// Panels showing results being filled again follow them, a few times a
    /// second, until the plugin is done.
    fn poll_refreshing(&mut self, ctx: &egui::Context) {
        if self.refreshing.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(400));
        if self.refresh_polled.is_some_and(|t| t.elapsed() < Duration::from_millis(400)) {
            return;
        }
        self.refresh_polled = Some(Instant::now());
        let hidden = self.vis();
        let mut i = 0;
        while i < self.refreshing.len() {
            let (plugin, roots, since) = &self.refreshing[i];
            // a moment for the command to reach the plugin
            let running = since.elapsed() < Duration::from_secs(1) || plugin.activity(&roots[0]).running;
            for p in self.panels.iter_mut().filter(|p| roots.iter().any(|r| p.path().starts_with(r))) {
                p.reload(hidden);
            }
            if running {
                i += 1;
            } else {
                self.refreshing.remove(i);
            }
        }
    }

    /// Do what was asked in the results window.
    fn results_act(&mut self, ctx: &egui::Context, act: crate::results::Act) {
        use crate::results::Act;
        let Some(win) = &self.results else { return };
        let (plugin, root, flat, again, refresh) = (win.plugin.clone(), win.root.clone(), win.flat.clone(), win.again.clone(), win.refresh.clone());
        match act {
            Act::Open(ref inner) | Act::View(ref inner) | Act::Edit(ref inner) => match plugin.real_path(&root, inner) {
                None => self.status = tr("This result is not a file on disk.").into(),
                Some(real) if real.is_dir() => {
                    // a found folder opens in the active panel
                    let hidden = self.vis();
                    if let Err(e) = self.act().load(&real, hidden, None) {
                        self.error(e);
                    }
                }
                Some(real) => {
                    let r = match act {
                        Act::View(_) => self.view_file(&real, false),
                        Act::Edit(_) => self.edit_file(&real, false),
                        _ => shell_open(&real).map_err(|e| format!("{}: {e}", display_path(&real))),
                    };
                    if let Err(e) = r {
                        self.error(e);
                    }
                }
            },
            Act::GoTo(inner) => match plugin.real_path(&root, &inner) {
                Some(real) => {
                    let hidden = self.vis();
                    let dir = real.parent().map(Path::to_path_buf).unwrap_or_else(|| real.clone());
                    let name = real.file_name().map(|n| n.to_string_lossy().into_owned());
                    if let Err(e) = self.act().load(&dir, hidden, name.as_deref()) {
                        self.error(e);
                    }
                }
                None => self.status = tr("This result is not a file on disk.").into(),
            },
            Act::ShowFolder | Act::ShowList => {
                let hidden = self.vis();
                let at = if act == Act::ShowList { flat.unwrap_or(root) } else { root };
                if let Err(e) = self.act().load(&at, hidden, None) {
                    self.error(e);
                }
            }
            Act::Stop => plugin.stop(&root),
            Act::Again => self.run_plugin_command(ctx, plugin, again, plugins::Values::new()),
            Act::Refresh => self.refresh_results(ctx, plugin, &root, refresh),
            Act::Close => self.close_results(),
        }
    }

    /// The first viewer or editor a plugin registered for `file`'s name, in
    /// the order the plugins are listed.
    fn opener_for(&self, kind: OpenKind, file: &Path) -> Option<(Arc<dyn FolderPlugin>, Opener)> {
        let name = file.file_name()?.to_string_lossy().into_owned();
        self.plugins.active.iter().find_map(|p| p.openers().into_iter().find(|o| o.kind == kind && o.matches(&name)).map(|o| (p.clone(), o)))
    }

    /// What a plugin's viewer or editor is told about the program's look, so
    /// its window matches; `temporary` when the file is a temporary copy.
    fn opener_options(&self, temporary: bool) -> plugins::Values {
        let theme = match self.cfg.theme {
            ThemeMode::System => "system",
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
        };
        let scrollbars = if self.cfg.always_show_scrollbars { "solid" } else { "floating" };
        let (light, dark) = (sal_ui::accent_text(self.cfg.accent_light), sal_ui::accent_text(self.cfg.accent_dark));
        // plugins from before the themes had their own: the one of the theme chosen
        let one = if self.cfg.theme == ThemeMode::Dark { dark.clone() } else { light.clone() };
        [
            ("theme", theme),
            ("ui_font", &self.cfg.ui_font),
            ("mono_font", &self.cfg.mono_font),
            ("scrollbars", scrollbars),
            ("animations", if animated(self.cfg.animations) { "on" } else { "off" }),
            ("accent_light", &light),
            ("accent_dark", &dark),
            ("accent", &one),
            ("temporary", if temporary { "1" } else { "" }),
            ("owner", &self.owner_value()),
        ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect()
    }

    /// The program's window, as a number: a plugin's windows open over it,
    /// on the same monitor.
    fn owner_value(&self) -> String {
        self.hwnd.map(|h| h.to_string()).unwrap_or_default()
    }

    /// What F3 (`View`) or F4 (`Edit`) opens `file` with: what its suffix
    /// has of its own, or the default (Settings → Viewers and editors).
    fn open_with_for(&self, kind: OpenKind, file: &Path) -> OpenWith {
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let rule = sal_core::config::suffix_rule(&self.cfg.suffix_rules, &name);
        let own = rule.and_then(|r| if kind == OpenKind::View { r.view.clone() } else { r.edit.clone() });
        own.unwrap_or_else(|| if kind == OpenKind::View { self.cfg.view_with.clone() } else { self.cfg.edit_with.clone() })
    }

    /// Open a file on disk for F3 or F4. A chosen plugin that is off or
    /// missing (or a program left empty) gives way to the first plugin that
    /// registered for the file, then to Notepad for editing.
    fn open_for(&self, kind: OpenKind, p: &Path, temporary: bool) -> Result<(), String> {
        let err = |e: std::io::Error| format!("{}: {e}", display_path(p));
        match self.open_with_for(kind, p) {
            OpenWith::Program { command } if !command.trim().is_empty() => return sal_core::shell::open_with_program(&command, p).map_err(err),
            OpenWith::Notepad => return sal_core::shell::edit(p, None).map_err(err),
            OpenWith::Plugin { plugin, opener } => {
                let chosen = self.plugins.active.iter().find(|x| x.id() == plugin && x.openers().iter().any(|o| o.id == opener && o.kind == kind));
                if let Some(x) = chosen {
                    return x.open_file(&opener, p, &self.opener_options(temporary)).map_err(err);
                }
            }
            OpenWith::Program { .. } => {}
        }
        if let Some((plugin, opener)) = self.opener_for(kind, p) {
            return plugin.open_file(&opener.id, p, &self.opener_options(temporary)).map_err(err);
        }
        match kind {
            OpenKind::Edit => sal_core::shell::edit(p, None).map_err(err),
            OpenKind::View => Err(tr("No viewer is set for this file. Turn on the Viewer plugin in Settings → Plugins, or choose a viewer in Settings → Viewers and editors.").into()),
        }
    }

    /// F3 on a file on disk.
    fn view_file(&self, p: &Path, temporary: bool) -> Result<(), String> {
        self.open_for(OpenKind::View, p, temporary)
    }

    /// F4 on a file on disk.
    fn edit_file(&self, p: &Path, temporary: bool) -> Result<(), String> {
        self.open_for(OpenKind::Edit, p, temporary)
    }

    /// The file on disk a plugin's item stands for (a search result): the
    /// program opens, views and edits it instead of a temporary copy.
    fn real_file(&self, p: &Path) -> Option<PathBuf> {
        let (plugin, file, inner) = self.in_archive(p)?;
        plugin.real_path(&file, &inner)
    }

    /// The files on disk `items` stand for, for the clipboard, a drag out of
    /// the window and the shell's context menu: a plugin's items through
    /// [`FolderPlugin::real_path`] (those without one are left out), others
    /// as they are. An error when none of them is a file on disk.
    fn real_items(&self, items: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
        let real: Vec<PathBuf> = items
            .iter()
            .filter_map(|p| match self.in_archive(p) {
                Some((plugin, file, inner)) => plugin.real_path(&file, &inner),
                None => Some(p.clone()),
            })
            .collect();
        if real.is_empty() && !items.is_empty() {
            return Err(tr("These items aren't files on disk.").into());
        }
        Ok(real)
    }

    /// The folder the shell runs a verb in for `items`: the first one's.
    fn folder_of(items: &[PathBuf]) -> PathBuf {
        items.first().and_then(|p| p.parent()).map(Path::to_path_buf).unwrap_or_default()
    }

    /// Close a connection: panels showing it go back to where they were
    /// before, and the plugin releases it in the background.
    fn close_connection(&mut self, root: &Path) {
        let Some((_, id)) = plugins::mount_root_of(root) else { return };
        // two folders compared go together
        if let Some(at) = self.links.iter().position(|(l, r)| l == root || r == root) {
            let (l, r) = self.links.remove(at);
            let partner = if l == root { r } else { l };
            self.close_connection(&partner);
        }
        let results = plugins::is_results(root);
        plugins::remove_mount(root);
        let hidden = self.vis();
        for p in &mut self.panels {
            if p.path().starts_with(root) {
                p.archive = None;
                p.leave_connection(hidden);
            }
        }
        if let Some(plugin) = self.plugins.get(&id).cloned() {
            let root = root.to_path_buf();
            std::thread::spawn(move || plugin.close(&root));
        }
        self.status = trf("Disconnected from {where}.", &[("where", &display_path(root))]);
        if results {
            self.status = trf("Closed {where}.", &[("where", &display_path(root))]);
        }
    }

    /// The program is ending: let the plugins close their connections.
    fn close_all_connections(&mut self) {
        for (root, id) in plugins::mounts() {
            plugins::remove_mount(&root);
            if let Some(plugin) = self.plugins.get(&id) {
                plugin.close(&root);
            }
        }
    }

    /// Enter, F3 or F4 on an item inside a plugin's file: extract it in the
    /// background, then open it ([`App::open_extracted`]).
    fn open_from_archive(&mut self, ctx: &egui::Context, file: PathBuf, how: OpenAs) -> Result<(), String> {
        let plugin = self.in_archive(&file).map(|(p, _, _)| PluginRef(p));
        let dir = plugins::temp_dir("open").map_err(|e| e.to_string())?;
        self.jobs.add_with(ctx, JobKind::Archive(FolderOp::Open(how)), vec![file], dir, Placement::Now, plugin, JobOptions::default());
        Ok(())
    }

    /// Save the settings, with the panels' folders, sort orders and the
    /// active panel as they are now.
    fn save_config(&mut self) {
        // a connection is gone at the next start: keep the folder saved before it
        for (i, saved) in [(0, &mut self.cfg.left), (1, &mut self.cfg.right)] {
            let p = &self.panels[i];
            let path = if location::virtual_parts(p.path()).is_some() { saved.path.clone() } else { display_path(p.path()) };
            *saved = PanelConfig { path, sort: p.sort };
        }
        self.cfg.right_active = self.active == 1;
        if let Err(e) = self.cfg.save() {
            eprintln!("cannot save configuration: {e}");
        }
    }

    /// Show the answer of a look for a new release. One made at start-up
    /// waits until no dialog is open, and only speaks up about an update.
    fn poll_update_check(&mut self) {
        let Some(check) = &self.update_check else { return };
        if !check.manual && !matches!(self.dialog, Dialog::None) {
            return;
        }
        let Some(result) = check.take() else { return };
        let manual = check.manual;
        self.update_check = None;
        match result {
            Ok(updates) if !updates.is_empty() => self.open_dialog(Dialog::Update { rows: updates.into_iter().map(UpdateRow::new).collect() }),
            Ok(_) if manual => self.open_dialog(Dialog::Message {
                title: tr("Check for updates").into(),
                text: trf("You have the newest version ({version}).", &[("version", &env!("CARGO_PKG_VERSION"))]),
            }),
            Err(e) if manual => self.error(trf("Could not look for updates: {error}", &[("error", &e)])),
            _ => {}
        }
    }

    /// Show `d` as the dialog, in a window of its own.
    fn open_dialog(&mut self, d: Dialog) {
        // a new dialog window opens centred on the main window again; the
        // windows of running operations stay as they are
        dialogwin::close(&mut self.dialog_places, self.hwnd, |k| !OWN_WINDOWS.contains(&k));
        self.dialog = d;
        self.focus_dialog = true;
    }

    /// Switch the UI language ("" = the Windows language) and remember it.
    fn set_language(&mut self, code: &str) {
        self.cfg.language = code.to_owned();
        crate::i18n::set_language(code);
    }

    /// Show `text` in an error message.
    fn error(&mut self, text: impl Into<String>) {
        self.open_dialog(Dialog::Message { title: tr("Error").into(), text: text.into() });
    }

    /// The active panel.
    fn act(&mut self) -> &mut Panel {
        &mut self.panels[self.active]
    }

    /// Read both panels' folders again.
    fn reload_all(&mut self) {
        let h = self.vis();
        for p in &mut self.panels {
            p.reload(h);
        }
    }

    /// Start (or queue) a copy, move or delete of `sources` to `target`, or
    /// hand it to the plugin whose file is involved.
    fn start_job(&mut self, ctx: &egui::Context, kind: JobKind, sources: Vec<PathBuf>, target: PathBuf) {
        if sources.is_empty() {
            return;
        }
        let place = std::mem::replace(&mut self.queue_place, Placement::Now);
        let options = std::mem::take(&mut self.job_options);
        // inside, into or out of an archive: the plugin showing it does the work
        let plugin = self.in_archive(&sources[0]).or_else(|| if matches!(kind, JobKind::Transfer(_)) { self.archive_folder(&target) } else { None }).map(|(p, _, _)| PluginRef(p));
        let touches_archive = plugin.is_some();
        let kind = match kind {
            JobKind::Transfer(op) if touches_archive => JobKind::Archive(FolderOp::Transfer(op)),
            JobKind::Delete | JobKind::Recycle if touches_archive => JobKind::Archive(FolderOp::Delete),
            k => k,
        };
        // the items are handed over: their selection is cleared at once, even
        // when the operation only waits in the queue
        for p in &mut self.panels {
            p.unselect(&sources);
        }
        self.jobs.add_with(ctx, kind, sources, target, place, plugin, options);
    }

    /// Drive list, re-read at most every few seconds (it hits the OS per drive).
    fn cached_drives(&mut self) -> &[Drive] {
        if self.drives_loaded.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            self.drives = drives::list();
            self.drives_loaded = Some(Instant::now());
        }
        &self.drives
    }

    /// Switch `panel` to the drive at `root`, returning to the directory last
    /// visited there when that is enabled and it still exists.
    fn change_drive(&mut self, panel: usize, root: &Path) -> Result<(), String> {
        let hidden = self.vis();
        let remembered = if self.cfg.drive_remember_dir { self.last_dir.get(&drive_key(root)).filter(|p| p.is_dir()).cloned() } else { None };
        if let Some(dir) = remembered {
            if self.panels[panel].load(&dir, hidden, None).is_ok() {
                return Ok(());
            }
        }
        self.panels[panel].load(root, hidden, None)
    }

    /// "From the left/right panel": `panel` goes to the folder the other
    /// panel shows.
    fn go_to_other_panel(&mut self, panel: usize) -> Result<(), String> {
        let there = self.panels[1 - panel].path().to_path_buf();
        let hidden = self.vis();
        self.panels[panel].load(&there, hidden, None)
    }

    /// F8 / Shift+F8: delete `sources`, to the Recycle Bin when that is on
    /// and `permanent` is not asked for.
    fn delete_items(&mut self, ctx: &egui::Context, sources: Vec<PathBuf>, permanent: bool) {
        if sources.is_empty() {
            return;
        }
        let recycle = self.cfg.use_recycle_bin && !permanent && self.in_archive(&sources[0]).is_none();
        if self.cfg.confirm_delete || !recycle {
            self.open_dialog(Dialog::Delete { sources, recycle });
        } else {
            self.start_job(ctx, JobKind::Recycle, sources, PathBuf::new());
        }
    }

    /// Run a shell verb ("cut", "properties", …) on `items`, as Explorer would.
    fn shell_verb(&mut self, items: &[PathBuf], dir: &Path, verb: &str) -> Result<(), String> {
        let h = self.hwnd.ok_or_else(|| trf("\"{verb}\" needs the application window", &[("verb", &verb)]))?;
        shellmenu::invoke_verb(h, items, dir, verb)
    }

    /// Right button: a short click or a long press opens the context menu,
    /// depending on the configured trigger. With "long press" a short click
    /// selects (Norton/Salamander style); after the hold time a ring shows at
    /// the pointer and the menu opens when the button is released.
    fn handle_right_button(&mut self, ctx: &egui::Context) {
        let secondary = egui::PointerButton::Secondary;
        let (pressed, down, released, pos, time) =
            ctx.input(|i| (i.pointer.button_pressed(secondary), i.pointer.button_down(secondary), i.pointer.button_released(secondary), i.pointer.interact_pos(), i.time));
        if pressed {
            if let Some(pos) = pos {
                if let Some(panel) = (0..2).find(|&k| self.body_rects[k].contains(pos)) {
                    let row = self.row_hits[panel].iter().find(|(r, _)| r.contains(pos)).map(|(_, i)| *i);
                    self.rmb = Some(RightPress { panel, row, start: time, pos, armed: false });
                }
            }
        }
        let long = self.cfg.context_menu_trigger == ContextMenuTrigger::LongPress;
        let Some(r) = self.rmb.as_mut() else { return };
        if long && down && !r.armed {
            if pos.is_some_and(|p| p.distance(r.pos) > 8.0) {
                self.rmb = None; // moved: not a press-and-hold
                return;
            }
            if time - r.start >= self.cfg.long_press_ms as f64 / 1000.0 {
                r.armed = true;
            }
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        if let Some(r) = self.rmb.as_ref().filter(|r| r.armed && down) {
            // press-and-hold feedback, like Windows' touch ring
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("hold_ring")));
            painter.circle_stroke(r.pos, 16.0, egui::Stroke::new(2.5f32, ctx.style().visuals.selection.bg_fill));
        }
        if released {
            let Some(r) = self.rmb.take() else { return };
            if long && !r.armed {
                if let Some(i) = r.row {
                    self.active = r.panel;
                    let p = &mut self.panels[r.panel];
                    p.cursor = i;
                    p.toggle_select(false);
                }
            } else {
                self.open_context_menu(ctx, r.panel, r.row, r.pos, true);
            }
        }
    }

    /// Context menu for `row` (or the folder background) in `panel`. Like F5
    /// or F8 it applies to the selection when there is one, otherwise to the
    /// clicked item.
    fn open_context_menu(&mut self, ctx: &egui::Context, panel: usize, row: Option<usize>, pos: egui::Pos2, from_mouse: bool) {
        self.active = panel;
        let plugin = self.panels[panel].archive.is_some();
        let p = &mut self.panels[panel];
        p.qs.clear();
        let entry = row.and_then(|i| p.entries().get(i).cloned().map(|e| (i, e))).filter(|(_, e)| e.kind != EntryKind::UpDir);
        let selected: Vec<PathBuf> = p.entries().iter().filter(|x| x.selected).map(|x| x.path.clone()).collect();
        let items: Vec<PathBuf> = match &entry {
            Some((i, e)) => {
                p.set_cursor(*i);
                if selected.is_empty() {
                    vec![e.path.clone()]
                } else {
                    selected
                }
            }
            None => Vec::new(),
        };
        let mut dir = p.path().to_path_buf();
        // a plugin's items: the files on disk they stand for, if any
        let items = if plugin {
            if items.is_empty() {
                self.status = tr("The context menu isn't available in plugin panels.").into();
                return;
            }
            match self.real_items(&items) {
                Ok(real) => {
                    dir = Self::folder_of(&real);
                    real
                }
                Err(e) => {
                    self.status = e;
                    return;
                }
            }
        } else {
            items
        };
        let menu = CtxMenu { panel, items, dir, pos, can_paste: shellmenu::clipboard_has_files(), plugin };
        match (self.cfg.context_menu_style, self.hwnd) {
            (ContextMenuStyle::Classic, _) => self.show_classic(ctx, menu.items, menu.dir, pos, from_mouse),
            (ContextMenuStyle::Windows11, Some(h)) => {
                let (actions, native) = self.native_rows(&menu, false);
                let at = self.menu_point(ctx, h, pos, from_mouse);
                let id = shellmenu::show_native(h, &native, at);
                if id > 0 {
                    self.run_menu_action(ctx, menu, actions[id as usize - 1]);
                }
            }
            (ContextMenuStyle::Windows11Full, Some(h)) => {
                let (actions, native) = self.native_rows(&menu, true);
                let at = self.menu_point(ctx, h, pos, from_mouse);
                let targets = if menu.items.is_empty() { vec![menu.dir.clone()] } else { menu.items.clone() };
                // the shell's copies of what our part already has
                let skip: &[&str] = if menu.items.is_empty() {
                    &["paste", "properties"]
                } else {
                    &["open", "openas", "cut", "copy", "paste", "delete", "rename", "properties", "copyaspath"]
                };
                let extended = ctx.input(|i| i.modifiers.shift);
                match shellmenu::show_merged(h, &native, &targets, &menu.dir, at, extended, skip) {
                    Ok(Merged::Ours(id)) => self.run_menu_action(ctx, menu, actions[id as usize - 1]),
                    Ok(Merged::Shell(picked)) => self.shell_picked(ctx, Ok(picked), menu.items, targets),
                    Err(e) => self.error(e),
                }
            }
            // no native window (tests): the menu drawn by the app
            (ContextMenuStyle::Windows11 | ContextMenuStyle::Windows11Full, None) => self.ctx_menu = Some(menu),
        }
    }

    /// The Windows 11 style menu as native menu rows (ids from 1) and the
    /// action of each id; `full`: without "Show more options", for the menu
    /// that has the shell's items below.
    fn native_rows(&self, menu: &CtxMenu, full: bool) -> (Vec<MenuAction>, Vec<shellmenu::NativeRow>) {
        let (top, rows) = self.menu_model(menu);
        let mut actions = Vec::new();
        let mut native = Vec::new();
        let mut add = |e: &ctxmenu::Entry, native: &mut Vec<shellmenu::NativeRow>| {
            actions.push(e.action);
            native.push(Some((actions.len() as u32, e.label.clone(), e.shortcut.clone(), e.enabled)));
        };
        for e in &top {
            add(e, &mut native);
        }
        if !top.is_empty() {
            native.push(None);
        }
        for row in &rows {
            match row {
                ctxmenu::Row::Entry(e) if full && e.action == MenuAction::MoreOptions => {}
                ctxmenu::Row::Entry(e) => add(e, &mut native),
                ctxmenu::Row::Separator => native.push(None),
            }
        }
        while native.last().is_some_and(Option::is_none) {
            native.pop();
        }
        (actions, native)
    }

    /// Where a context menu opens, in screen pixels: at the mouse, or at the
    /// panel point `pos` when opened from the keyboard.
    fn menu_point(&self, ctx: &egui::Context, h: isize, pos: egui::Pos2, from_mouse: bool) -> (i32, i32) {
        if from_mouse {
            shellmenu::screen_point(h, None)
        } else {
            let ppp = ctx.pixels_per_point();
            shellmenu::screen_point(h, Some((pos.x * ppp, pos.y * ppp)))
        }
    }

    /// The full shell menu; for the background it is the folder's own menu.
    fn show_classic(&mut self, ctx: &egui::Context, items: Vec<PathBuf>, dir: PathBuf, pos: egui::Pos2, from_mouse: bool) {
        let Some(h) = self.hwnd else {
            self.error(tr("The system context menu needs the application window."));
            return;
        };
        let targets = if items.is_empty() { vec![dir.clone()] } else { items.clone() };
        let at = self.menu_point(ctx, h, pos, from_mouse);
        let extended = ctx.input(|i| i.modifiers.shift);
        let picked = shellmenu::show_classic(h, &targets, &dir, at, extended);
        self.shell_picked(ctx, picked, items, targets);
    }

    /// Do what was picked among the shell's items: rename and delete are
    /// the panel's own, whatever the shell ran may have changed sizes.
    fn shell_picked(&mut self, ctx: &egui::Context, picked: Result<Picked, String>, items: Vec<PathBuf>, targets: Vec<PathBuf>) {
        match picked {
            Ok(Picked::Rename) if items.len() == 1 => {
                let name = items[0].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.open_dialog(Dialog::Rename { path: items[0].clone(), name });
            }
            Ok(Picked::Delete) => self.delete_items(ctx, targets, ctx.input(|i| i.modifiers.shift)),
            Ok(Picked::Invoked(_)) => self.dir_sizes.clear(),
            Ok(_) => {}
            Err(e) => self.error(e),
        }
    }

    /// Items of the Windows 11 style menu: (icon-row commands, list).
    fn menu_model(&self, menu: &CtxMenu) -> (Vec<ctxmenu::Entry>, Vec<ctxmenu::Row>) {
        let ov = &self.cfg.shortcuts;
        let key = |c: Cmd| menu_key(ov, c);
        let entry = |action, label: &str, shortcut: String, enabled| ctxmenu::Entry { action, label: label.to_owned(), shortcut, enabled };
        let n = menu.items.len();
        let p = &self.panels[menu.panel];
        let on_disk = |i: &PathBuf| if i.is_dir() { EntryKind::Dir } else { EntryKind::File };
        let kinds: Vec<EntryKind> = menu.items.iter().map(|i| p.entries().iter().find(|e| &e.path == i).map_or_else(|| on_disk(i), |e| e.kind)).collect();
        let any_dir = kinds.contains(&EntryKind::Dir);
        let single_file = n == 1 && kinds[0] == EntryKind::File;
        let single_dir = n == 1 && kinds[0] == EntryKind::Dir;
        if n > 0 {
            let mut top = vec![entry(MenuAction::Cut, tr("Cut"), key(Cmd::ClipboardPut(true)), true), entry(MenuAction::Copy, tr("Copy"), key(Cmd::ClipboardPut(false)), true)];
            if menu.can_paste && single_dir && !menu.plugin {
                top.push(entry(MenuAction::Paste, tr("Paste into folder"), key(Cmd::ClipboardPaste), true));
            }
            if !menu.plugin {
                top.push(entry(MenuAction::Rename, tr("Rename"), key(Cmd::Rename), n == 1));
            }
            top.push(entry(MenuAction::Delete, tr("Delete"), key(Cmd::Delete { permanent: false }), true));
            let mut rows = vec![
                ctxmenu::Row::Entry(entry(MenuAction::Open, tr("Open"), "Enter".into(), true)),
                ctxmenu::Row::Entry(entry(MenuAction::OpenWith, tr("Open with…"), String::new(), single_file)),
                ctxmenu::Row::Separator,
            ];
            if any_dir && !menu.plugin {
                rows.push(ctxmenu::Row::Entry(entry(MenuAction::CalcSize, tr("Calculate size"), key(Cmd::CalcSelectionSize), true)));
            }
            rows.push(ctxmenu::Row::Entry(entry(MenuAction::CopyPath, tr("Copy as path"), key(Cmd::CopyPaths), true)));
            if single_dir {
                rows.push(ctxmenu::Row::Entry(entry(MenuAction::Terminal, tr("Open in Terminal"), String::new(), true)));
            }
            rows.push(ctxmenu::Row::Entry(entry(MenuAction::Properties, tr("Properties"), key(Cmd::Properties), true)));
            rows.push(ctxmenu::Row::Separator);
            rows.push(ctxmenu::Row::Entry(entry(MenuAction::MoreOptions, tr("Show more options"), String::new(), true)));
            (top, rows)
        } else {
            let mut top = Vec::new();
            if menu.can_paste {
                top.push(entry(MenuAction::Paste, tr("Paste"), key(Cmd::ClipboardPaste), true));
            }
            let rows = vec![
                ctxmenu::Row::Entry(entry(MenuAction::NewFolder, tr("New folder"), key(Cmd::MkDir), true)),
                ctxmenu::Row::Entry(entry(MenuAction::Paste, tr("Paste"), key(Cmd::ClipboardPaste), menu.can_paste)),
                ctxmenu::Row::Entry(entry(MenuAction::Refresh, tr("Refresh"), key(Cmd::Refresh), true)),
                ctxmenu::Row::Entry(entry(MenuAction::SelectAll, tr("Select all"), key(Cmd::SelectAll), true)),
                ctxmenu::Row::Separator,
                ctxmenu::Row::Entry(entry(MenuAction::Terminal, tr("Open in Terminal"), key(Cmd::Terminal), true)),
                ctxmenu::Row::Entry(entry(MenuAction::Properties, tr("Properties"), String::new(), true)),
                ctxmenu::Row::Separator,
                ctxmenu::Row::Entry(entry(MenuAction::MoreOptions, tr("Show more options"), String::new(), true)),
            ];
            (top, rows)
        }
    }

    /// When an explicit size calculation has finished, put the total in the
    /// status line: size, bytes, files and folders.
    fn report_sizes(&mut self) {
        let Some((panel, items)) = &self.size_report else { return };
        let entries = self.panels[*panel].entries();
        let (mut bytes, mut files, mut dirs, mut unreadable) = (0u64, 0u64, 0u64, 0u64);
        for item in items {
            match entries.iter().find(|e| &e.path == item) {
                Some(e) if e.kind == EntryKind::File => {
                    bytes += e.size;
                    files += 1;
                }
                _ => match self.dir_sizes.get(item) {
                    Some(si) if si.done => {
                        bytes += si.size.bytes;
                        files += si.size.files;
                        dirs += si.size.dirs;
                        unreadable += si.size.unreadable;
                    }
                    Some(_) => return, // still calculating
                    None => {}         // invalidated meanwhile: report what is known
                },
            }
        }
        let mut s = trf(
            "Size: {size} ({bytes} bytes) in {files}, {dirs}",
            &[("size", &format_size(bytes)), ("bytes", &format_thousands(bytes)), ("files", &plural_u64(files, "file")), ("dirs", &plural_subdirs(dirs))],
        );
        if unreadable > 0 {
            s.push_str(&trf(" — {folders} could not be read", &[("folders", &plural_u64(unreadable, "folder"))]));
        }
        self.status = s;
        self.size_report = None;
    }

    /// The program's own context menu, when one is open, and what was chosen in it.
    fn context_menu_ui(&mut self, ctx: &egui::Context) {
        let Some(menu) = &self.ctx_menu else { return };
        let (top, rows) = self.menu_model(menu);
        let icons = crate::fonts::icon_family(ctx);
        let (chosen, rect) = ctxmenu::show(ctx, menu.pos, &top, &rows, &icons);
        let (escape, clicked_outside) = ctx.input(|i| (i.key_pressed(Key::Escape), i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p))));
        if let Some(action) = chosen {
            let menu = self.ctx_menu.take().unwrap();
            self.run_menu_action(ctx, menu, action);
        } else if escape || clicked_outside {
            self.ctx_menu = None;
        }
    }

    /// Do what was chosen in the context menu `menu`.
    fn run_menu_action(&mut self, ctx: &egui::Context, menu: CtxMenu, action: MenuAction) {
        let hidden = self.vis();
        let items = menu.items.clone();
        let single = items.first().filter(|_| items.len() == 1).cloned();
        let is_dir = |p: &PathBuf| p.is_dir();
        let res: Result<(), String> = match action {
            MenuAction::Open => match &single {
                Some(p) if is_dir(p) => self.panels[menu.panel].load(p, hidden, None),
                Some(p) => shell_open(p).map_err(|e| e.to_string()),
                None => self.shell_verb(&items, &menu.dir, "open"),
            },
            MenuAction::OpenWith => self.shell_verb(&items, &menu.dir, "openas"),
            MenuAction::Cut => self.shell_verb(&items, &menu.dir, "cut"),
            MenuAction::Copy => self.shell_verb(&items, &menu.dir, "copy"),
            MenuAction::Paste => {
                // into the clicked folder, or the panel's folder for the background
                let target = single.clone().filter(is_dir).unwrap_or_else(|| menu.dir.clone());
                let r = self.shell_verb(std::slice::from_ref(&target), &target, "paste");
                self.dir_sizes.clear();
                r
            }
            MenuAction::Rename => {
                if let Some(p) = single {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.open_dialog(Dialog::Rename { path: p, name });
                }
                Ok(())
            }
            MenuAction::Delete => {
                self.delete_items(ctx, items, false);
                Ok(())
            }
            MenuAction::CopyPath => {
                let text: Vec<String> = items.iter().map(|p| display_path(p)).collect();
                self.status = trf("Copied {paths} to the clipboard", &[("paths", &plural(text.len(), "path"))]);
                ctx.copy_text(text.join("\r\n"));
                Ok(())
            }
            MenuAction::CalcSize => {
                for p in items.iter().filter(|p| is_dir(p)) {
                    self.dir_sizes.restart(p.clone(), ctx);
                }
                self.status = trf("Calculating the size of {what}…", &[("what", &plural(items.len(), "item"))]);
                self.size_report = Some((menu.panel, items.clone()));
                Ok(())
            }
            MenuAction::Terminal => {
                let dir = single.filter(is_dir).unwrap_or_else(|| menu.dir.clone());
                sal_core::shell::open_terminal(self.cfg.terminal, &self.cfg.terminal_custom, &dir).map_err(|e| e.to_string())
            }
            MenuAction::Properties => {
                let targets = if items.is_empty() { vec![menu.dir.clone()] } else { items };
                self.shell_verb(&targets, &menu.dir, "properties")
            }
            MenuAction::NewFolder => {
                self.command(ctx, Cmd::MkDir);
                Ok(())
            }
            MenuAction::Refresh => {
                self.command(ctx, Cmd::Refresh);
                Ok(())
            }
            MenuAction::SelectAll => {
                self.panels[menu.panel].select_all(true);
                Ok(())
            }
            MenuAction::MoreOptions => {
                self.show_classic(ctx, items, menu.dir, menu.pos, false);
                Ok(())
            }
        };
        if let Err(e) = res {
            self.error(e);
        }
    }

    /// Open what was typed into a panel's path bar: a directory, a file (its
    /// directory, focused), a bare drive letter, `\\server` or `\\server\share`.
    fn go_to_input(&mut self, idx: usize, input: &str) -> Result<(), String> {
        let hidden = self.vis();
        let cwd = self.panels[idx].path().to_path_buf();
        match location::resolve_input(input, &cwd).map_err(|e| e.to_string())? {
            Target::Drive(letter) => {
                let root = self.cached_drives().iter().find(|d| d.letter.starts_with(letter)).map(|d| d.root.clone());
                match root {
                    Some(root) => self.change_drive(idx, &root),
                    None => Err(trf("Drive {letter}: is not available", &[("letter", &letter)])),
                }
            }
            Target::Dir(dir) => {
                self.panels[idx].load(&dir, hidden, None)?;
                self.remember_path(&dir);
                Ok(())
            }
            Target::File { dir, name } => {
                self.panels[idx].load(&dir, hidden, Some(&name))?;
                self.remember_path(&dir);
                Ok(())
            }
        }
    }

    /// Pick a program with the Windows "Open" dialog and make it the custom choice.
    fn browse_program(&mut self, target: Browse) {
        let title = match target {
            Browse::Editor | Browse::RuleEditor => tr("Choose the editor (F4)"),
            Browse::Viewer | Browse::RuleViewer => tr("Choose the viewer (F3)"),
            Browse::Terminal => tr("Choose the terminal"),
        };
        let Some(program) = shellmenu::pick_program(self.hwnd, title) else { return };
        match target {
            Browse::Editor => {
                self.cfg.editor = program.clone();
                self.cfg.edit_with = OpenWith::Program { command: program };
            }
            Browse::Viewer => {
                self.cfg.viewer = program.clone();
                self.cfg.view_with = OpenWith::Program { command: program };
            }
            Browse::RuleViewer | Browse::RuleEditor => {
                if let Some(form) = &mut self.suffix_edit {
                    let (slot, typed) = if target == Browse::RuleViewer { (&mut form.rule.view, &mut form.view_program) } else { (&mut form.rule.edit, &mut form.edit_program) };
                    *typed = program.clone();
                    *slot = Some(OpenWith::Program { command: program });
                }
            }
            Browse::Terminal => {
                self.cfg.terminal_custom = program;
                self.cfg.terminal = TerminalKind::Custom;
            }
        }
    }

    /// Put a typed path at the top of the remembered list (most recent first).
    fn remember_path(&mut self, dir: &Path) {
        if !self.cfg.remember_typed_paths {
            return;
        }
        let s = display_path(dir);
        self.cfg.typed_paths.retain(|x| !x.eq_ignore_ascii_case(&s));
        self.cfg.typed_paths.insert(0, s);
        self.cfg.typed_paths.truncate(MAX_TYPED_PATHS);
    }

    /// Jump to an ancestor from the breadcrumbs, focusing the item we came from.
    fn go_to_crumb(&mut self, idx: usize, target: &Path) -> Result<(), String> {
        let hidden = self.vis();
        let cur = self.panels[idx].path().to_path_buf();
        let focus = if location::unc_server(target).is_some() {
            location::unc_share_root(&drives::root_of(&cur)).map(|(_, share)| share)
        } else {
            cur.strip_prefix(target).ok().and_then(|r| r.components().next()).map(|c| c.as_os_str().to_string_lossy().into_owned())
        };
        self.panels[idx].load(target, hidden, focus.as_deref())
    }

    /// Remember each panel's folder as its drive's last one (for changing
    /// drives back).
    fn remember_dirs(&mut self) {
        for p in &self.panels {
            let key = drive_key(&drives::root_of(p.path()));
            if self.last_dir.get(&key).map(PathBuf::as_path) != Some(p.path()) {
                self.last_dir.insert(key, p.path().to_path_buf());
            }
        }
    }

    // ---------------------------------------------------------------- commands

    /// Run a command (a key, a menu item, a function-key button); errors are
    /// shown in a message.
    fn command(&mut self, ctx: &egui::Context, cmd: Cmd) {
        crate::debuglog::did(format_args!("command {cmd:?}"));
        let hidden = self.vis();
        let other = 1 - self.active;
        let res: Result<(), String> = match cmd {
            Cmd::Help => {
                self.open_dialog(Dialog::Help);
                Ok(())
            }
            Cmd::CheckUpdates => {
                self.update_check = Some(crate::update::Check::start(ctx, true, update_sources(&self.plugins), update_offer(&self.cfg, true)));
                Ok(())
            }
            Cmd::Find => match self.plugins.get("search").cloned() {
                Some(p) => {
                    self.run_plugin_command(ctx, p, "find".into(), plugins::Values::new());
                    Ok(())
                }
                None => Err(tr("Finding files needs the Search plugin: turn it on in Settings → Plugins.").into()),
            },
            Cmd::Enter => match self.act().enter(hidden).map(|f| f.map(|f| self.real_file(&f).unwrap_or(f))) {
                Ok(Some(file)) if self.in_archive(&file).is_some() => self.open_from_archive(ctx, file, OpenAs::Default),
                // an archive opens as a folder (or with Windows when 7-Zip can't read it)
                Ok(Some(file)) if file.file_name().and_then(|n| n.to_str()).is_some_and(|n| self.plugins.active.iter().any(|p| p.handles(n))) => {
                    let name = file.file_name().map(|n| n.to_string_lossy().into_owned());
                    match self.act().load(&file, hidden, None) {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
                            let _ = self.act().load(&dir, hidden, name.as_deref());
                            Err(e)
                        }
                    }
                }
                Ok(Some(file)) => shell_open(&file).map_err(|e| format!("{}: {e}", display_path(&file))),
                Ok(None) => Ok(()),
                Err(e) => Err(e),
            },
            Cmd::Parent => self.act().go_parent(hidden),
            Cmd::Root => self.act().go_root(hidden),
            Cmd::Drive(letter) => {
                let root = self.cached_drives().iter().find(|d| d.letter.starts_with(letter)).map(|d| d.root.clone());
                match root {
                    Some(root) => self.change_drive(self.active, &root),
                    None => {
                        self.status = trf("Drive {letter}: is not available", &[("letter", &letter)]);
                        Ok(())
                    }
                }
            }
            Cmd::Settings => {
                self.open_dialog(Dialog::Settings { tab: SettingsTab::General, recording: None });
                Ok(())
            }
            Cmd::History(back) => self.act().history_step(back, hidden),
            Cmd::Rename => {
                if let Some(e) = self.act().current().filter(|e| e.kind != EntryKind::UpDir) {
                    let (path, name) = (e.path.clone(), e.name.clone());
                    self.open_dialog(Dialog::Rename { path, name });
                }
                Ok(())
            }
            Cmd::View => match self.act().current().filter(|e| e.kind == EntryKind::File).map(|e| e.path.clone()).map(|p| self.real_file(&p).unwrap_or(p)) {
                Some(p) if self.in_archive(&p).is_some() => self.open_from_archive(ctx, p, OpenAs::View),
                Some(p) => self.view_file(&p, false),
                None => Ok(()),
            },
            Cmd::Edit => match self.act().current().filter(|e| e.kind == EntryKind::File).map(|e| e.path.clone()).map(|p| self.real_file(&p).unwrap_or(p)) {
                Some(p) if self.in_archive(&p).is_some() => self.open_from_archive(ctx, p, OpenAs::Edit),
                Some(p) => self.edit_file(&p, false),
                None => Ok(()),
            },
            Cmd::Copy | Cmd::Move => {
                let sources = self.act().targets();
                if !sources.is_empty() {
                    let target = transfer_target(self.panels[other].path());
                    let kind = if cmd == Cmd::Copy { OpKind::Copy } else { OpKind::Move };
                    // what Settings sets; a change in the dialog is for this operation only
                    let d = if kind == OpKind::Copy { self.cfg.copy_defaults } else { self.cfg.move_defaults };
                    self.open_dialog(Dialog::Transfer { kind, sources, target, opts: d });
                }
                Ok(())
            }
            Cmd::MkDir => {
                self.open_dialog(Dialog::MkDir { name: String::new() });
                Ok(())
            }
            Cmd::Delete { permanent } => {
                let sources = self.act().targets();
                self.delete_items(ctx, sources, permanent);
                Ok(())
            }
            Cmd::ContextMenu => {
                let idx = self.active;
                let cursor = self.panels[idx].cursor;
                let pos = self.row_hits[idx]
                    .iter()
                    .find(|(_, i)| *i == cursor)
                    .map_or(self.body_rects[idx].left_top() + egui::vec2(24.0, 24.0), |(r, _)| r.left_bottom() + egui::vec2(32.0, 0.0));
                self.open_context_menu(ctx, idx, Some(cursor), pos, false);
                Ok(())
            }
            Cmd::Properties if self.act().archive.is_some() => {
                // a plugin's items: the files on disk they stand for
                let items = self.act().targets();
                if items.is_empty() {
                    Err(tr("Properties aren't available in plugin panels.").into())
                } else {
                    self.real_items(&items).and_then(|real| self.shell_verb(&real, &Self::folder_of(&real), "properties"))
                }
            }
            Cmd::Properties => {
                let mut items = self.act().targets();
                if items.is_empty() {
                    items.push(self.act().path().to_path_buf());
                }
                let dir = self.act().path().to_path_buf();
                self.shell_verb(&items, &dir, "properties")
            }
            Cmd::Exit => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                Ok(())
            }
            Cmd::Refresh => {
                self.dir_sizes.clear();
                self.icons.clear_files();
                self.reload_all();
                Ok(())
            }
            Cmd::SelectCalcSize => {
                let p = self.act();
                if let Some(e) = p.current().filter(|e| e.kind != EntryKind::UpDir).cloned() {
                    p.toggle_select(true);
                    let now_selected = p.entries().iter().any(|x| x.path == e.path && x.selected);
                    // folders inside archives have their sizes already
                    if e.kind == EntryKind::Dir && now_selected && p.archive.is_none() {
                        self.dir_sizes.start(e.path, ctx);
                    }
                }
                Ok(())
            }
            Cmd::ToggleHidden => {
                self.cfg.show_hidden = !self.cfg.show_hidden;
                self.reload_all();
                Ok(())
            }
            Cmd::SetTheme(mode) => {
                self.cfg.theme = mode;
                apply_theme(ctx, mode);
                Ok(())
            }
            Cmd::ToggleTheme => {
                let mode = if ctx.theme() == egui::Theme::Dark { ThemeMode::Light } else { ThemeMode::Dark };
                self.cfg.theme = mode;
                apply_theme(ctx, mode);
                Ok(())
            }
            Cmd::ToggleScrollbars => {
                self.cfg.always_show_scrollbars = !self.cfg.always_show_scrollbars;
                sal_ui::apply_scrollbars(ctx, self.cfg.always_show_scrollbars);
                Ok(())
            }
            Cmd::ToggleAnimations => {
                self.cfg.animations = !self.cfg.animations;
                apply_animations(ctx, self.cfg.animations);
                Ok(())
            }
            Cmd::ToggleQuickSearchMode => {
                self.cfg.quick_search_anywhere = !self.cfg.quick_search_anywhere;
                self.status = if self.cfg.quick_search_anywhere { tr("Quick search: anywhere in name") } else { tr("Quick search: from first letter") }.into();
                Ok(())
            }
            Cmd::ToggleFkeyBar => {
                self.cfg.show_fkey_bar = !self.cfg.show_fkey_bar;
                Ok(())
            }
            Cmd::ToggleIcons => {
                self.cfg.system_icons = !self.cfg.system_icons;
                Ok(())
            }
            Cmd::ToggleSystem => {
                self.cfg.show_system = !self.cfg.show_system;
                self.reload_all();
                Ok(())
            }
            Cmd::CalcSelectionSize => {
                let p = &self.panels[self.active];
                let selected: Vec<&FileEntry> = p.entries().iter().filter(|e| e.selected).collect();
                let items: Vec<&FileEntry> = if selected.is_empty() { p.current().into_iter().filter(|e| e.kind != EntryKind::UpDir).collect() } else { selected };
                let dirs: Vec<PathBuf> = items.iter().filter(|e| e.kind == EntryKind::Dir).map(|e| e.path.clone()).collect();
                let files = items.len() - dirs.len();
                // an explicit request recalculates even folders sized earlier
                if p.archive.is_none() {
                    for d in &dirs {
                        self.dir_sizes.restart(d.clone(), ctx);
                    }
                }
                let all: Vec<PathBuf> = items.iter().map(|e| e.path.clone()).collect();
                if !dirs.is_empty() {
                    self.size_report = Some((self.active, all));
                }
                self.status = match (dirs.len(), files) {
                    (0, 0) => tr("Nothing to calculate").into(),
                    (0, f) => trf("{files} — see the total below the panel", &[("files", &plural(f, "file"))]),
                    (d, _) => trf("Calculating the size of {what}…", &[("what", &plural(d, "folder"))]),
                };
                Ok(())
            }
            Cmd::SelectAll => {
                self.act().select_all(true);
                Ok(())
            }
            Cmd::UnselectAll => {
                self.act().select_all(false);
                Ok(())
            }
            Cmd::Invert => {
                self.act().invert_selection();
                Ok(())
            }
            Cmd::SelectMask(select) => {
                let mask = self.last_mask.clone();
                self.open_dialog(Dialog::SelectMask { select, mask, error: None });
                Ok(())
            }
            Cmd::Sort(key) => {
                let spec = self.act().sort.toggled(key);
                self.act().resort(spec);
                Ok(())
            }
            Cmd::SwapPanels => {
                self.panels.swap(0, 1);
                Ok(())
            }
            Cmd::ChangeDir => {
                let text = self.act().path_edit.clone();
                self.path_edit[self.active] = Some(PathEdit { text, fresh: true });
                Ok(())
            }
            Cmd::DriveMenu(panel) => {
                self.active = panel;
                self.drives_loaded = None;
                let mut drives = self.cached_drives().to_vec();
                drives.sort_by_key(|d| (!d.kind.is_local(), d.letter.clone()));
                let root = drives::root_of(self.panels[panel].path());
                // servers first, then searches
                let mut conns: Vec<DriveConn> = plugins::connections()
                    .into_iter()
                    .map(|c| DriveConn {
                        label: if c.name.is_empty() { display_path(&c.root).trim_end_matches('/').to_owned() } else { c.name.clone() },
                        detail: self.plugins.name_of(&c.plugin),
                        root: c.root,
                        start: c.start,
                        results: c.results,
                        paired: c.paired,
                        refresh: c.refresh,
                        plugin: c.plugin,
                    })
                    // a comparison's side belongs to its own panel only
                    .filter(|c| !c.paired || self.links.iter().any(|link| [&link.0, &link.1][panel] == &c.root))
                    .collect();
                conns.sort_by_key(DriveConn::group);
                let cursor = match conns.iter().position(|c| self.panels[panel].path().starts_with(&c.root)) {
                    Some(i) => drives.len() + i,
                    None => drives.iter().position(|d| d.root == root).unwrap_or(0),
                };
                let paths = if self.cfg.remember_typed_paths { self.cfg.typed_paths.clone() } else { Vec::new() };
                self.open_dialog(Dialog::Drive { panel, drives, conns, paths, cursor });
                Ok(())
            }
            Cmd::Terminal => {
                // inside an archive: in the archive's folder
                let p = match &self.act().archive {
                    Some(a) => a.file.parent().map(Path::to_path_buf).unwrap_or_default(),
                    None => self.act().path().to_path_buf(),
                };
                sal_core::shell::open_terminal(self.cfg.terminal, &self.cfg.terminal_custom, &p).map_err(|e| e.to_string())
            }
            Cmd::CopyPaths => {
                // a plugin's items: the file on disk each stands for, if any
                let paths: Vec<String> = self.act().targets().iter().map(|p| display_path(&self.real_file(p).unwrap_or_else(|| p.clone()))).collect();
                if !paths.is_empty() {
                    self.status = trf("Copied {paths} to the clipboard", &[("paths", &plural(paths.len(), "path"))]);
                    ctx.copy_text(paths.join("\r\n"));
                }
                Ok(())
            }
            Cmd::ClipboardPaste if self.act().archive.is_some() => Err(tr("The clipboard isn't available in plugin panels.").into()),
            Cmd::ClipboardPut(cut) => {
                let items = self.act().targets();
                if items.is_empty() {
                    return;
                }
                // a plugin's items: the files on disk they stand for
                let (items, dir) = if self.act().archive.is_some() {
                    match self.real_items(&items) {
                        Ok(real) => {
                            let dir = Self::folder_of(&real);
                            (real, dir)
                        }
                        Err(e) => return self.error(e),
                    }
                } else {
                    (items, self.act().path().to_path_buf())
                };
                self.shell_verb(&items, &dir, if cut { "cut" } else { "copy" }).map(|()| {
                    let what = plural(items.len(), "item");
                    self.status = if cut { trf("{items} cut to the clipboard", &[("items", &what)]) } else { trf("{items} copied to the clipboard", &[("items", &what)]) };
                })
            }
            Cmd::ClipboardPaste => {
                let dir = self.act().path().to_path_buf();
                let r = self.shell_verb(std::slice::from_ref(&dir), &dir, "paste");
                self.dir_sizes.clear();
                r
            }
            Cmd::HotPathSet(n) => {
                self.cfg.hot_paths[n] = display_path(self.panels[self.active].path());
                self.status = trf("Hot path {n} set to {path}", &[("n", &n), ("path", &self.cfg.hot_paths[n])]);
                Ok(())
            }
            Cmd::HotPathGo(n) => {
                let hp = self.cfg.hot_paths[n].clone();
                if hp.is_empty() {
                    self.status = trf("Hot path {n} is not defined (Ctrl+Shift+{n} sets it)", &[("n", &n)]);
                    Ok(())
                } else {
                    self.act().load(Path::new(&hp), hidden, None)
                }
            }
        };
        if let Err(e) = res {
            self.error(e);
        }
    }

    // ---------------------------------------------------------------- keyboard

    /// This frame's key presses and typed text, when no dialog has them.
    fn handle_keys(&mut self, ctx: &egui::Context, raw: Vec<(Key, Modifiers)>) {
        // the keys egui turns into clipboard events (see rawkeys), as keys
        let mut events: Vec<Event> = raw.into_iter().map(|(key, modifiers)| Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }).collect();
        events.extend(ctx.input(|i| i.events.clone()));
        for ev in events {
            match ev {
                Event::Key { key, pressed: true, modifiers, .. } => self.on_key(ctx, key, modifiers),
                Event::Text(t) => {
                    if std::mem::take(&mut self.skip_text) {
                        continue;
                    }
                    for c in t.chars() {
                        self.on_char(c);
                    }
                }
                _ => {}
            }
            if !matches!(self.dialog, Dialog::None) {
                // the key that opened the dialog (e.g. '+') must not be typed
                // into its text field, which is drawn later in this frame
                ctx.input_mut(|i| i.events.clear());
                break;
            }
        }
        self.skip_text = false;
    }

    /// Drive-letter shortcut; the modifier is configurable in Settings.
    fn drive_letter_shortcut(&mut self, key: Key, m: Modifiers) -> Option<Cmd> {
        let letter = key_letter(key)?;
        if !self.cfg.drive_hotkey.matches(m.ctrl, m.shift, m.alt) {
            return None;
        }
        // Shift+letter keeps working for quick search: while a search is in
        // progress, or when no such drive exists, the letter is typed instead
        if self.cfg.drive_hotkey == DriveHotkey::Shift && !self.act().qs.is_empty() {
            return None;
        }
        self.cached_drives().iter().any(|d| d.letter.starts_with(letter)).then_some(Cmd::Drive(letter))
    }

    /// A key press in the panels: quick search keys first, then the keyboard
    /// shortcuts, then the panel's own keys.
    fn on_key(&mut self, ctx: &egui::Context, key: Key, m: Modifiers) {
        // Backspace edits a quick search in progress before it means anything else
        if key == Key::Backspace && m.is_none() && !self.act().qs.is_empty() {
            return self.act().quick_search_pop();
        }
        // configurable shortcuts come first, so any fixed key can be overridden;
        // only a plain Space still types into a quick search in progress
        let typing_space = key == Key::Space && m.is_none() && !self.act().qs.is_empty();
        if let Some(a) = keymap::lookup(&self.cfg.shortcuts, key, m).filter(|_| !typing_space) {
            self.skip_text = true;
            return self.command(ctx, action_cmd(a));
        }
        if let Some(cmd) = self.drive_letter_shortcut(key, m) {
            self.skip_text = true;
            return self.command(ctx, cmd);
        }
        let page = self.act().visible_rows.max(2) as isize - 1;
        // with arrows jumping between matches, moving the cursor otherwise
        // ends the search; without, the search and its highlight stay while
        // the cursor moves freely (Esc ends it)
        let moving = matches!(key, Key::ArrowUp | Key::ArrowDown | Key::PageUp | Key::PageDown | Key::Home | Key::End);
        let jump = matches!(key, Key::ArrowUp | Key::ArrowDown) && (m.ctrl || m.is_none());
        if moving && !jump && self.cfg.quick_search_arrows {
            self.act().qs.clear();
        }
        let digit = [Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9].iter().position(|&k| k == key);
        if let Some(n) = digit.filter(|_| m.ctrl) {
            return self.command(ctx, if m.shift { Cmd::HotPathSet(n) } else { Cmd::HotPathGo(n) });
        }
        // fixed navigation keys
        match (key, m.ctrl, m.shift, m.alt) {
            (Key::ArrowUp, true, _, _) => {
                let (anywhere, loose) = (self.cfg.quick_search_anywhere, self.cfg.quick_search_loose);
                self.act().quick_search_next(false, anywhere, loose);
            }
            (Key::ArrowDown, true, _, _) => {
                let (anywhere, loose) = (self.cfg.quick_search_anywhere, self.cfg.quick_search_loose);
                self.act().quick_search_next(true, anywhere, loose);
            }
            // the search acts as a filter: arrows skip to the previous/next match
            (Key::ArrowUp | Key::ArrowDown, false, false, false) if self.cfg.quick_search_arrows && !self.act().qs.is_empty() => {
                let (anywhere, loose) = (self.cfg.quick_search_anywhere, self.cfg.quick_search_loose);
                self.act().quick_search_next(key == Key::ArrowDown, anywhere, loose);
            }
            (Key::ArrowUp | Key::ArrowDown, false, shift, false) => {
                let p = self.act();
                if shift {
                    p.toggle_select(false);
                }
                p.move_cursor(if key == Key::ArrowUp { -1 } else { 1 });
            }
            (Key::PageUp, ..) => self.act().move_cursor(-page),
            (Key::PageDown, ..) => self.act().move_cursor(page),
            (Key::Home, ..) => self.act().set_cursor(0),
            (Key::End, ..) => {
                let p = self.act();
                let last = p.entries().len().saturating_sub(1);
                p.set_cursor(last);
            }
            (Key::Enter, false, false, false) => self.command(ctx, Cmd::Enter),
            (Key::Tab, false, _, false) => {
                self.act().qs.clear();
                self.active = 1 - self.active;
            }
            (Key::Insert, false, false, false) => self.act().toggle_select(true),
            (Key::Escape, ..) => self.act().qs.clear(),
            _ => {}
        }
    }

    /// Typed text: it starts or continues a quick search in the active panel.
    fn on_char(&mut self, c: char) {
        if c.is_control() {
            return;
        }
        let p = self.act();
        if p.qs.is_empty() {
            match c {
                ' ' => return p.toggle_select(true),
                '+' => return self.command_noctx(Cmd::SelectMask(true)),
                '-' => return self.command_noctx(Cmd::SelectMask(false)),
                '*' => return p.invert_selection(),
                _ => {}
            }
        }
        let (anywhere, loose) = (self.cfg.quick_search_anywhere, self.cfg.quick_search_loose);
        self.act().quick_search_push(c, anywhere, loose);
    }

    /// Commands that open dialogs don't need the egui context.
    fn command_noctx(&mut self, cmd: Cmd) {
        if let Cmd::SelectMask(select) = cmd {
            let mask = self.last_mask.clone();
            self.open_dialog(Dialog::SelectMask { select, mask, error: None });
        }
    }

    // ---------------------------------------------------------------- panels

    /// Draw panel `idx`: its path bar, the file list, the banner of a plugin's
    /// panel and the footer; and handle the clicks in them.
    fn panel_ui(&mut self, ui: &mut egui::Ui, idx: usize) {
        let active = self.active == idx;
        let row_h = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
        let mut clicked: Option<(usize, Modifiers, bool)> = None;
        // a row dragged: its items (or the selection it is in) start a drag
        let mut drag_from: Option<usize> = None;
        let mut sort_click = None;
        let mut path_start_edit = false;
        let mut path_submit: Option<bool> = None;
        let mut crumb_clicked: Option<PathBuf> = None;
        let mut crumb_rects = Vec::new();
        let mut drive_clicked = false;
        let mut drive_picked: Option<PathBuf> = None;
        // a server session or search picked beside the drive buttons: (start, root)
        let mut conn_picked: Option<ConnPick> = None;
        let mut compare_picked = false;
        let mut from_other_picked = false;
        let drive_list = self.cached_drives().to_vec();
        let show_all = self.cfg.drive_buttons == DriveButtons::All;
        // with all drive buttons: the connections, by group, and this panel's side of the comparison
        let (servers, searches, compare_side) = if show_all {
            let all = plugins::connections();
            let group = |g: u8| all.iter().filter(|c| conn_group(c.results, c.paired) == g).cloned().collect::<Vec<_>>();
            (group(0), group(1), self.links.first().map(|l| if idx == 0 { l.0.clone() } else { l.1.clone() }))
        } else {
            (Vec::new(), Vec::new(), None)
        };
        // a panel a plugin shows is tinted from the path bar to its banner;
        // the tint goes under what is drawn, once the panel's height is known
        let plugin_panel = self.panels[idx].archive.is_some();
        let tint = plugin_tint(ui.visuals());
        let top = ui.cursor().min;
        let tint_slot = ui.painter().add(egui::Shape::Noop);

        // path bar
        ui.horizontal(|ui| {
            let root = drives::root_of(self.panels[idx].path());
            let root_kind = drives::kind_of_root(&root, &drive_list);
            let on_listed_drive = drive_list.iter().any(|d| d.root == root);
            if show_all {
                ui.spacing_mut().item_spacing.x = layout::CHIP_SPACING;
                for d in &drive_list {
                    let letter = d.letter.trim_end_matches(':');
                    if drive_icon::button(ui, d.kind, letter, d.root == root).on_hover_text(describe_drive(d)).clicked() {
                        drive_picked = Some(d.root.clone());
                    }
                }
                // server sessions and searches, each a list; the comparison, a button
                let here = self.panels[idx].path().to_path_buf();
                let network = drive_icon::color(sal_core::drives::DriveType::Network);
                for (list, icon, tip, salt) in [
                    (&servers, sal_ui::mdi::SERVER_NETWORK, tr("Server sessions"), "servers"),
                    (&searches, sal_ui::mdi::MAGNIFY, tr("Searches"), "searches"),
                ] {
                    if let Some(p) = connection_list(ui, (salt, idx), icon, network, tip, list, &here) {
                        conn_picked = Some(p);
                    }
                }
                if let Some(side) = &compare_side {
                    let tip = tr("The comparison: its sides in both panels");
                    if drive_icon::glyph_button(ui, sal_ui::mdi::COMPARE_HORIZONTAL, network, "", here.starts_with(side)).on_hover_text(tip).clicked() {
                        compare_picked = true;
                    }
                }
                let (icon, label) = from_other_label(idx);
                let tip = format!("{label}\n{}", display_path(self.panels[1 - idx].path()));
                if drive_icon::glyph_button(ui, icon, drive_icon::color(sal_core::drives::DriveType::Fixed), "", false).on_hover_text(tip).clicked() {
                    from_other_picked = true;
                }
                ui.spacing_mut().item_spacing.x = layout::CHROME_GAP;
            }
            // the current drive (or UNC share) when not shown among the buttons
            // above (a plugin's connection is, in its list or as the comparison)
            if !show_all || !on_listed_drive && plugins::mount_root_of(&root).is_none() {
                let named = plugins::mount_root_of(&root).and_then(|(r, id)| Some((plugins::results_name(&r)?, id, plugins::results_icon(&r))));
                let label = match &named {
                    // results: the plugin's name; the path bar says which
                    Some((_, id, _)) => self.plugins.name_of(id),
                    None => display_path(&root).trim_end_matches(['\\', '/']).to_owned(),
                };
                let tip = match drive_list.iter().find(|d| d.root == root) {
                    Some(d) => describe_drive(d),
                    None if named.is_some() => format!("{}\n{}", named.as_ref().unwrap().0, display_path(&root).trim_end_matches('/')),
                    None => format!("{label} — {}", tr(root_kind.label())),
                };
                let menu_key = menu_key(&self.cfg.shortcuts, Cmd::DriveMenu(idx));
                let tip = if menu_key.is_empty() { tip } else { format!("{tip}\n{}", trf("Drive menu: {key}", &[("key", &menu_key)])) };
                // a plugin's connection: a server, or a search's magnifier
                let (glyph, color) = match (&named, plugins::mount_root_of(&root)) {
                    (Some((_, _, icon)), _) => (*icon, drive_icon::color(sal_core::drives::DriveType::Network)),
                    (None, Some(_)) => (sal_ui::mdi::SERVER_NETWORK, drive_icon::color(sal_core::drives::DriveType::Network)),
                    (None, None) => (drive_icon::glyph(root_kind), drive_icon::color(root_kind)),
                };
                if drive_icon::glyph_button(ui, glyph, color, if label.is_empty() { "/" } else { &label }, false).on_hover_text(tip).clicked() {
                    drive_clicked = true;
                }
            }
            let (bg, fg) = match (active, plugin_panel) {
                (true, _) => (ui.visuals().selection.bg_fill, ui.visuals().selection.stroke.color),
                (false, true) => (tint.gamma_multiply(2.5), ui.visuals().text_color()),
                (false, false) => (ui.visuals().faint_bg_color, ui.visuals().text_color()),
            };
            let edit_key = menu_key(&self.cfg.shortcuts, Cmd::ChangeDir);
            egui::Frame::none().fill(bg).inner_margin(layout::PATH_MARGIN).rounding(3.0f32).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if let Some(edit) = &mut self.path_edit[idx] {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut edit.text)
                            .desired_width(f32::INFINITY)
                            .hint_text(r"C:\folder, \\server\share, D:, %USERPROFILE%, ..")
                            .id_salt(("path_edit", idx)),
                    );
                    if edit.fresh || !r.has_focus() && !r.lost_focus() {
                        r.request_focus();
                    }
                    if edit.fresh {
                        edit.fresh = false;
                        if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), r.id) {
                            let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(edit.text.chars().count()));
                            st.cursor.set_char_range(Some(all));
                            st.store(ui.ctx(), r.id);
                        }
                    }
                    if r.lost_focus() {
                        path_submit = Some(ui.input(|i| i.key_pressed(Key::Enter)));
                    }
                } else {
                    let crumbs = path_crumbs(self.panels[idx].path());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = layout::CRUMB_SPACING;
                        let font = egui::TextStyle::Body.resolve(ui.style());
                        let text_w = |ui: &egui::Ui, s: &str| ui.fonts(|f| f.layout_no_wrap(s.to_owned(), font.clone(), fg).size().x);
                        // keep the root and as many trailing segments as fit; the rest becomes "…"
                        let sep_w = text_w(ui, "›") + 6.0;
                        let widths: Vec<f32> = crumbs.iter().map(|(l, _)| text_w(ui, l) + 3.0 + sep_w).collect();
                        let room = ui.available_width() - 24.0;
                        let mut first = 1.min(crumbs.len());
                        while first + 1 < crumbs.len() && widths[0] + text_w(ui, "…") + sep_w + widths[first..].iter().sum::<f32>() > room {
                            first += 1;
                        }
                        let mut shown: Vec<(String, PathBuf)> = crumbs.iter().take(1).cloned().collect();
                        if first > 1 {
                            shown.push(("…".into(), crumbs[first - 1].1.clone()));
                        }
                        shown.extend(crumbs.iter().skip(first).cloned());
                        let last = shown.len().saturating_sub(1);
                        for (i, (label, target)) in shown.iter().enumerate() {
                            if i > 0 {
                                ui.label(RichText::new("›").color(fg.gamma_multiply(0.55)));
                            }
                            let r = ui.add(egui::Label::new(RichText::new(visual_order(&display_name(label), true)).color(fg).strong()).sense(Sense::click()).selectable(false));
                            if r.hovered() {
                                ui.painter().hline(r.rect.x_range(), r.rect.bottom(), egui::Stroke::new(1.0f32, fg));
                            }
                            let tip = if i == last { format!("{}\n{}", display_path(target), tr("Click to type a path")) } else { display_path(target) };
                            let r = r.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
                            crumb_rects.push((r.rect, target.clone()));
                            if r.clicked() {
                                if i == last {
                                    path_start_edit = true;
                                } else {
                                    crumb_clicked = Some(target.clone());
                                }
                            }
                        }
                        // the free space after the crumbs starts typing a path
                        let h = ui.text_style_height(&egui::TextStyle::Body);
                        let (_, r) = ui.allocate_exact_size(egui::vec2(ui.available_width().max(8.0), h), Sense::click());
                        let hint = if edit_key.is_empty() { tr("Click to type a path").to_owned() } else { trf("Click or {key} to type a path", &[("key", &edit_key)]) };
                        if r.on_hover_cursor(egui::CursorIcon::Text).on_hover_text(hint).clicked() {
                            path_start_edit = true;
                        }
                    });
                }
            });
        });
        self.crumb_rects[idx] = crumb_rects;

        let footer_h = row_h + 4.0;
        // a panel a plugin shows: tinted, with the plugin's name below the list
        let banner = self.panels[idx].archive.as_ref().map(|a| {
            let b = plugin_banner(&self.plugins.name_of(a.plugin.id()), &a.file);
            if a.marks.is_empty() {
                b
            } else {
                format!("{b}  ·  {}", tr("amber: different · green: only here · grey: only on the other side"))
            }
        });
        let banner_h = if banner.is_some() { row_h } else { 0.0 };
        let body_h = (ui.available_height() - footer_h - banner_h).max(row_h * 3.0);
        if plugin_panel {
            let bottom = ui.cursor().min.y + body_h;
            let rect = egui::Rect::from_min_max(top, egui::pos2(top.x + ui.available_width(), bottom));
            ui.painter().set(tint_slot, egui::Shape::rect_filled(rect, 0.0, tint));
        }
        self.panels[idx].visible_rows = ((body_h - row_h) / row_h).floor().max(1.0) as usize;

        let mut sizes = self.dir_sizes.snapshot_in(self.panels[idx].path());
        // inside an archive every folder's size is known from the archive's index
        if let Some(a) = self.panels[idx].archive.as_ref().filter(|a| a.index.complete) {
            for e in self.panels[idx].entries().iter().filter(|e| e.kind == EntryKind::Dir) {
                let info = crate::dirsize::SizeInfo { size: plugins::stats(&a.index, &plugins::join_inner(&a.inner, &e.name)), done: true };
                sizes.insert(e.path.clone(), info);
            }
        }
        // sorted by size: folders by their sizes once known (not while counting, so nothing jumps)
        if self.panels[idx].sort.key == SortKey::Size && !sizes.is_empty() {
            self.panels[idx].keep_sorted_by_size(&|e| sizes.get(&e.path).filter(|i| i.done).map(|i| i.size.bytes));
        }
        let use_icons = self.cfg.system_icons;
        let marks = self.panels[idx].archive.as_ref().map(|a| a.marks.clone()).filter(|m| !m.is_empty());
        let (weak, dark) = (ui.visuals().weak_text_color(), ui.visuals().dark_mode);
        // a quick search's marks, in shades of the accent color: rows it matches
        // (other than the cursor's) get a light wash, a bit stronger than the
        // tint of a plugin's panel; the matched letters a stronger one; on the
        // cursor's row (all accent) the letters a wash of its text color
        let accent = ui.visuals().selection;
        let qs_fill = accent.bg_fill.gamma_multiply(if dark { 0.22 } else { 0.3 });
        let qs_letters = accent.bg_fill.gamma_multiply(if dark { 0.55 } else { 0.5 });
        let qs_letters_cursor = accent.stroke.color.gamma_multiply(0.3);
        let icons = &mut self.icons;
        let panel = &mut self.panels[idx];
        let scroll_to = if panel.scroll_to_cursor { Some(panel.cursor) } else { None };
        panel.scroll_to_cursor = false;
        let sort = panel.sort;
        let cursor = panel.cursor;
        let qs_mask = (!panel.qs.is_empty()).then(|| sal_core::quicksearch::prepare(&panel.qs, self.cfg.quick_search_loose));
        let qs_anywhere = self.cfg.quick_search_anywhere;
        let entries = &panel.listing.entries;
        let width = ui.available_width();
        // the columns, packed against the right edge: Attr exactly as wide as all
        // its letters (or its title), Ext three letters to start with, Size and
        // Date at their usual widths (scaled down in a narrow panel). Ext, Size
        // and Date keep the widths the user drags them to; Name always takes the
        // rest, so there is never room left over after the columns
        let name_min = (width * 0.3).min(120.0);
        let gap = ui.spacing().item_spacing.x;
        let font = egui::TextStyle::Body.resolve(ui.style());
        let text_w = |s: &str| ui.fonts(|f| f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x).ceil() + 2.0;
        let titled = |title: &str| text_w(&format!("{title} {}", sal_ui::mdi::MENU_DOWN));
        let attr_w = text_w("RHSACE").max(titled(tr("Attr")));
        let ext_w = text_w("www").max(titled(tr("Ext")));
        let room = width - gap * 4.0 - ui.spacing().scroll.allocated_width() - 4.0 - name_min - attr_w;
        let mins = [(ext_w / 3.0).ceil(), 40.0, 40.0];
        let k = ((room - ext_w) / (95.0 + 120.0)).clamp(0.2, 1.0);
        let defaults = [ext_w, (95.0 * k).max(mins[1]), (120.0 * k).max(mins[2])];
        // the user's widths while they fit, else the usual ones
        let widths = self.col_widths[idx].filter(|w| w.iter().sum::<f32>() <= room).unwrap_or(defaults);
        self.col_widths[idx] = Some(widths);
        let mut header_rects = Vec::new();

        let mut row_hits = Vec::new();
        let body =
            ui.allocate_ui(egui::vec2(width, body_h), |ui| {
                // the table remembers column widths; a different scrollbar mode
                // reserves different space, so it gets its own layout state
                let solid_bars = !ui.style().spacing.scroll.floating;
                ui.push_id((idx, solid_bars), |ui| {
                    let bars = scroll_bar_visibility(ui);
                    // exact widths, none resizable by egui_extras: it can only move a
                    // column's right edge, and Name (the remainder, on the left)
                    // would push that edge back. The edges are dragged below instead
                    let mut tb = TableBuilder::new(ui)
                        .scroll_bar_visibility(bars)
                        .striped(true)
                        .resizable(false)
                        .auto_shrink([false, false])
                        .sense(Sense::click_and_drag())
                        .max_scroll_height(body_h)
                        .cell_layout(Layout::left_to_right(Align::Center))
                        .column(Column::remainder().at_least(name_min).clip(true))
                        .column(Column::exact(widths[0]).clip(true))
                        .column(Column::exact(widths[1]).clip(true))
                        .column(Column::exact(widths[2]).clip(true))
                        .column(Column::exact(attr_w));
                    if let Some(r) = scroll_to {
                        tb = tb.scroll_to_row(r, None);
                    }
                    tb.header(row_h, |mut h| {
                        for (key, title) in
                            [(SortKey::Name, tr("Name")), (SortKey::Ext, tr("Ext")), (SortKey::Size, tr("Size")), (SortKey::Time, tr("Date")), (SortKey::Attr, tr("Attr"))]
                        {
                            // the whole cell sorts, not only its title
                            let mut column = egui::Rect::NOTHING;
                            let (_, cell) = h.col(|ui| {
                                // the column's own room (the response's rect is wider)
                                column = ui.max_rect();
                                // up: ascending (A→Z, smallest, oldest first); down: descending
                                let arrow = if sort.key == key {
                                    if sort.reverse {
                                        sal_ui::mdi::MENU_DOWN
                                    } else {
                                        sal_ui::mdi::MENU_UP
                                    }
                                } else {
                                    ""
                                };
                                ui.add(egui::Label::new(RichText::new(format!("{title} {arrow}")).strong()).selectable(false));
                            });
                            header_rects.push(column);
                            if cell.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                sort_click = Some(key);
                            }
                        }
                    })
                    .body(|body| {
                        body.rows(row_h, entries.len(), |mut row| {
                            let i = row.index();
                            let e = &entries[i];
                            // the quick search's match in the whole name (chars), in every row that matches
                            let qs_hit = qs_mask.as_ref().filter(|_| active).and_then(|m| sal_core::quicksearch::find_match(&e.name, m, qs_anywhere));
                            let base_chars = if e.kind == EntryKind::File && !e.base().is_empty() { e.base().chars().count() } else { e.name.chars().count() };
                            row.set_selected(active && i == cursor);
                            let mark = marks.as_ref().and_then(|m| m.get(&e.name)).copied();
                            let missing = mark == Some(sal_plugin_api::ItemMark::Missing);
                            let color = if e.selected { Some(SELECTED_COLOR) } else if missing { Some(weak) } else { None };
                            // a quick search's match first (it lasts only while typing), else the mark
                            let qs_row = qs_hit.is_some() && i != cursor;
                            let mark_bg = if qs_row { Some(qs_fill) } else { mark.and_then(|m| mark_fill(m, dark)) };
                            // the color behind each cell of the row
                            let shade = |ui: &mut egui::Ui| {
                                if let Some(bg) = mark_bg {
                                    let r = ui.max_rect().expand2(egui::vec2(ui.spacing().item_spacing.x / 2.0, 0.0));
                                    ui.painter().rect_filled(r, 0.0, bg);
                                }
                            };
                            let text = |s: String| {
                                let mut t = RichText::new(s);
                                if let Some(c) = color {
                                    t = t.color(c);
                                }
                                t
                            };
                            row.col(|ui| {
                                shade(ui);
                                let tex = if use_icons && e.kind != EntryKind::UpDir { icons.get(e) } else { None };
                                match tex {
                                    Some(t) => {
                                        let h = ui.text_style_height(&egui::TextStyle::Body);
                                        // hidden items are ghosted, as in Explorer
                                        let tint = if e.is_hidden() { Color32::from_white_alpha(110) } else { Color32::WHITE };
                                        let img = egui::Image::from_texture(egui::load::SizedTexture::from_handle(&t)).fit_to_exact_size(egui::vec2(h, h)).tint(tint);
                                        ui.add_sized([h + 4.0, h], img);
                                    }
                                    None => paint_icon(ui, e.kind, e.is_hidden()),
                                }
                                let raw = if e.kind == EntryKind::File && !e.base().is_empty() { e.base() } else { e.name.as_str() };
                                let pretty = display_name(raw);
                                let logical: &str = &pretty;
                                let visual = visual_order(logical, true);
                                let shown: &str = &visual;
                                // the match's part in the name column (the base name); the whole
                                // name when its letters are reordered (right-to-left scripts)
                                let hl = qs_hit.map(|(a, b)| if shown == logical { (a.min(base_chars), b.min(base_chars)) } else { (0, shown.chars().count()) }).filter(|(a, b)| a < b);
                                let fg = color.unwrap_or_else(|| if active && i == cursor { ui.visuals().strong_text_color() } else { ui.visuals().text_color() });
                                let normal = TextFormat { font_id: egui::TextStyle::Body.resolve(ui.style()), color: fg, italics: e.name_lossy, ..Default::default() };
                                let job = qs_job(shown, hl, normal, if active && i == cursor { qs_letters_cursor } else { qs_letters });
                                let r = ui.add(egui::Label::new(job).truncate().selectable(false));
                                if e.name_lossy {
                                    r.on_hover_text(tr(
                                        "This name is not valid Unicode (unpaired UTF-16 surrogate).\nIt is shown with U+FFFD, but all operations use the exact on-disk name.",
                                    ));
                                }
                            });
                            row.col(|ui| {
                                shade(ui);
                                let ext = if e.kind == EntryKind::File { display_name(e.ext()).into_owned() } else { String::new() };
                                // the match's part in the extension (after the base name and its dot)
                                let skip = base_chars + 1;
                                let in_ext = qs_hit.filter(|_| e.kind == EntryKind::File && !e.base().is_empty()).map(|(a, b)| (a.saturating_sub(skip), b.saturating_sub(skip))).filter(|(a, b)| a < b);
                                match in_ext {
                                    Some(hl) => {
                                        let fg = color.unwrap_or_else(|| if active && i == cursor { ui.visuals().strong_text_color() } else { ui.visuals().text_color() });
                                        let normal = TextFormat { font_id: egui::TextStyle::Body.resolve(ui.style()), color: fg, ..Default::default() };
                                        let bg = if active && i == cursor { qs_letters_cursor } else { qs_letters };
                                        ui.add(egui::Label::new(qs_job(&ext, Some(hl), normal, bg)).truncate().selectable(false));
                                    }
                                    None => {
                                        ui.label(text(ext));
                                    }
                                }
                            });
                            row.col(|ui| {
                                shade(ui);
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    match (e.kind, sizes.get(&e.path)) {
                                        (EntryKind::Dir, Some(si)) => {
                                            // running total while calculating, final size when done
                                            let mut t = text(format_thousands(si.size.bytes));
                                            if !si.done {
                                                t = t.italics();
                                            }
                                            let mut tip = trf(
                                                "{size} in {files}, {dirs}",
                                                &[("size", &format_size(si.size.bytes)), ("files", &plural_u64(si.size.files, "file")), ("dirs", &plural_subdirs(si.size.dirs))],
                                            );
                                            if si.size.unreadable > 0 {
                                                tip.push_str(&format!("\n{}", trf("{folders} could not be read", &[("folders", &plural(si.size.unreadable as usize, "folder"))])));
                                            }
                                            if !si.done {
                                                ui.label(text("…".into()));
                                                tip.push_str(&format!("\n{}", tr("calculating…")));
                                            } else if si.size.unreadable > 0 {
                                                ui.label(text("*".into()));
                                            }
                                            ui.label(t).on_hover_text(tip);
                                        }
                                        (kind, _) => {
                                            ui.label(text(match kind {
                                                EntryKind::File if missing => String::new(),
                                                EntryKind::File => format_thousands(e.size),
                                                EntryKind::Dir => tr("DIR").into(),
                                                // the up-dir row needs no text: its name says it
                                                EntryKind::UpDir => String::new(),
                                            }));
                                        }
                                    }
                                });
                            });
                            row.col(|ui| {
                                shade(ui);
                                ui.label(text(fmt_time(e.modified)));
                            });
                            row.col(|ui| {
                                shade(ui);
                                ui.label(text(if e.kind == EntryKind::UpDir { String::new() } else { e.attr_string() }));
                            });
                            let r = row.response();
                            row_hits.push((r.rect, i));
                            if r.drag_started() {
                                drag_from = Some(i);
                            }
                            if r.double_clicked() {
                                clicked = Some((i, Modifiers::NONE, true));
                            } else if r.clicked() {
                                clicked = Some((i, r.ctx.input(|i| i.modifiers), false));
                            }
                        });
                    });
                });
            });

        self.body_rects[idx] = body.response.rect;
        if self.panels[idx].is_loading() {
            let r = body.response.rect;
            let weak = ui.visuals().weak_text_color();
            ui.painter().text(r.center_top() + egui::vec2(0.0, row_h * 2.0), egui::Align2::CENTER_CENTER, tr("Loading…"), egui::TextStyle::Body.resolve(ui.style()), weak);
        }
        self.row_hits[idx] = row_hits;
        // the left edges of Ext, Size and Date, dragged in the header: the column
        // on the right gets wider or narrower, Name the other way
        if header_rects.len() == 5 {
            // a divider after every column, from the header down through the list
            // (as egui_extras draws them for resizable columns)
            let full = header_rects[0].top()..=body.response.rect.bottom();
            for r in &header_rects {
                ui.painter().vline(r.right() + gap / 2.0, full.clone(), ui.visuals().widgets.noninteractive.bg_stroke);
            }
            let mut widths = widths;
            for c in 1..=3 {
                let x = header_rects[c].left() - gap / 2.0;
                let grab = ui.style().interaction.resize_grab_radius_side;
                let line = egui::Rect::from_min_max(egui::pos2(x - grab, header_rects[c].top()), egui::pos2(x + grab, header_rects[c].bottom()));
                let r = ui.interact(line, ui.id().with(("column_edge", idx, c)), Sense::drag());
                if r.hovered() || r.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
                    let stroke = if r.dragged() { ui.visuals().widgets.active.bg_stroke } else { ui.visuals().widgets.hovered.bg_stroke };
                    ui.painter().vline(x, full.clone(), stroke);
                }
                if r.dragged() {
                    let name_spare = (header_rects[0].width() - name_min).max(0.0);
                    let dx = r.drag_delta().x;
                    widths[c - 1] = (widths[c - 1] - dx).clamp(mins[c - 1], (widths[c - 1] + name_spare).max(mins[c - 1]));
                }
            }
            self.col_widths[idx] = Some(widths);
            self.header_rects[idx] = header_rects;
        }
        if let Some(text) = banner {
            let fill = plugin_tint(ui.visuals()).gamma_multiply(2.5);
            egui::Frame::none().fill(fill).inner_margin(layout::LINE_MARGIN).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(egui::Label::new(RichText::new(text).small().strong()).truncate());
            });
        }

        // footer: selection or cursor info + free space
        let panel = &self.panels[idx];
        let t = selection_totals(panel.entries(), &sizes);
        let info = if t.files + t.dirs > 0 {
            let mut s = trf("Selected: {size} in {items}", &[("size", &format_size(t.bytes)), ("items", &t.items())]);
            if let Some(contents) = t.contents() {
                s.push_str(&trf(" · {contents} in total", &[("contents", &contents)]));
            }
            if t.calculating > 0 {
                s.push_str(tr(" (calculating…)"));
            } else if t.unknown > 0 {
                s.push_str(&trf(" ({dirs} not sized)", &[("dirs", &plural(t.unknown, "directory"))]));
            }
            s
        } else if let Some(e) = panel.current() {
            match (e.kind, sizes.get(&e.path)) {
                (EntryKind::File, _) => {
                    trf("{name}: {bytes} bytes, {time}", &[("name", &display_name(&e.name)), ("bytes", &format_thousands(e.size)), ("time", &fmt_time(e.modified))])
                }
                (EntryKind::Dir, Some(si)) => {
                    let s = trf(
                        "{name}: {size} ({bytes} bytes) in {files}, {dirs}",
                        &[
                            ("name", &display_name(&e.name)),
                            ("size", &format_size(si.size.bytes)),
                            ("bytes", &format_thousands(si.size.bytes)),
                            ("files", &plural_u64(si.size.files, "file")),
                            ("dirs", &plural_subdirs(si.size.dirs)),
                        ],
                    );
                    if si.done {
                        s
                    } else {
                        s + tr(" (calculating…)")
                    }
                }
                _ => display_name(&e.name).into_owned(),
            }
        } else {
            String::new()
        };
        let qs = panel.qs.clone();
        let free = self.free.get(ui.ctx(), panel.path()).map(|(f, t)| trf("{free} free of {total}", &[("free", &format_size(f)), ("total", &format_size(t))]));
        // a strip that stands out from the panel in both themes
        let strip = strip_color(ui.visuals());
        let strong = ui.visuals().strong_text_color();
        egui::Frame::none().fill(strip).inner_margin(layout::LINE_MARGIN).rounding(2.0f32).show(ui, |ui| {
            ui.visuals_mut().override_text_color = Some(strong);
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if !qs.is_empty() {
                    ui.label(RichText::new(format!("{} {qs}", sal_ui::mdi::MAGNIFY)).color(ui.visuals().warn_fg_color)).on_hover_text(if qs_anywhere {
                        tr("Quick search: anywhere in name")
                    } else {
                        tr("Quick search: from first letter")
                    });
                    if qs_anywhere {
                        ui.label(RichText::new(tr("anywhere")).small().weak());
                    }
                    ui.separator();
                }
                // free space keeps its width; the info text is truncated to what is left
                let free_w = free.as_ref().map_or(0.0, |f| {
                    let font = egui::TextStyle::Body.resolve(ui.style());
                    ui.fonts(|fo| fo.layout_no_wrap(f.clone(), font, Color32::WHITE).size().x) + ui.spacing().item_spacing.x * 2.0
                });
                let h = ui.text_style_height(&egui::TextStyle::Body);
                ui.allocate_ui(egui::vec2((ui.available_width() - free_w).max(0.0), h), |ui| {
                    ui.add(egui::Label::new(info).truncate());
                });
                if let Some(f) = free {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(f));
                }
            });
        });

        // apply interactions after drawing
        // any press in the panel makes it the active one: also below the last
        // row, on the path bar or the footer, not only on an item
        let panel_rect = egui::Rect::from_min_max(top, egui::pos2(ui.max_rect().right(), ui.min_rect().bottom()));
        let press = ui.input(|i| i.pointer.any_pressed().then(|| i.pointer.interact_pos()).flatten());
        if press.is_some_and(|p| panel_rect.contains(p) && ui.ctx().layer_id_at(p) == Some(ui.layer_id())) {
            self.active = idx;
        }
        if let Some(key) = sort_click {
            self.active = idx;
            self.command(ui.ctx(), Cmd::Sort(key));
        }
        if path_start_edit {
            self.active = idx;
            self.command(ui.ctx(), Cmd::ChangeDir);
        }
        if let Some(target) = crumb_clicked {
            self.active = idx;
            if let Err(e) = self.go_to_crumb(idx, &target) {
                self.error(e);
            }
        }
        if let Some(submit) = path_submit {
            let text = self.path_edit[idx].take().map(|e| e.text).unwrap_or_default();
            self.active = idx;
            // this Enter/Esc must not also close an error message opened below
            ui.ctx().input_mut(|i| {
                i.consume_key(Modifiers::NONE, Key::Enter);
                i.consume_key(Modifiers::NONE, Key::Escape);
            });
            if submit {
                if let Err(e) = self.go_to_input(idx, &text) {
                    self.error(e);
                }
            }
        }
        if drive_clicked {
            self.command(ui.ctx(), Cmd::DriveMenu(idx));
        }
        if let Some(root) = drive_picked {
            self.active = idx;
            if let Err(e) = self.change_drive(idx, &root) {
                self.error(e);
            }
        }
        match conn_picked {
            Some(ConnPick::Open(start, root)) => {
                self.active = idx;
                let hidden = self.vis();
                if let Err(e) = self.panels[idx].load(&start, hidden, None).or_else(|_| self.panels[idx].load(&root, hidden, None)) {
                    self.error(e);
                }
            }
            Some(ConnPick::Refresh(id, root, command)) => {
                if let Some(plugin) = self.plugins.get(&id).cloned() {
                    let ctx = ui.ctx().clone();
                    self.refresh_results(&ctx, plugin, &root, command);
                }
            }
            Some(ConnPick::Close(root)) => self.close_connection(&root),
            None => {}
        }
        if compare_picked {
            self.active = idx;
            self.show_comparison();
        }
        if from_other_picked {
            self.active = idx;
            if let Err(e) = self.go_to_other_panel(idx) {
                self.error(e);
            }
        }
        if let Some(i) = drag_from {
            self.start_drag(idx, i);
        }
        if let Some((i, m, double)) = clicked {
            self.active = idx;
            let p = &mut self.panels[idx];
            p.qs.clear();
            if double {
                p.cursor = i;
                self.command(ui.ctx(), Cmd::Enter);
            } else if m.shift {
                let (a, b) = (p.cursor.min(i), p.cursor.max(i));
                for e in &mut p.listing.entries[a..=b] {
                    e.selected = e.kind != EntryKind::UpDir;
                }
                p.cursor = i;
            } else if m.ctrl {
                p.cursor = i;
                p.toggle_select(false);
            } else {
                p.cursor = i;
            }
        }
    }

    // ---------------------------------------------------------------- drag and drop

    /// The volume of `p` for the copy-or-move rule: an archive or a plugin's
    /// connection is one of its own, otherwise the drive or share.
    fn volume_of(&self, p: &Path) -> String {
        match self.archive_folder(p) {
            Some((_, file, _)) => file.to_string_lossy().to_lowercase(),
            None => dnd::volume(p),
        }
    }

    /// A plugin's items dragged out of the window: their paths in the
    /// plugin's mount (Windows' Projected File System), where other programs
    /// see them as files on disk, read from the plugin only as they are read.
    fn mount_items(&self, sources: &[PathBuf]) -> Result<Vec<PathBuf>, plugins::mount::MountError> {
        let Some((plugin, file, _)) = self.in_archive(&sources[0]) else { return Ok(Vec::new()) };
        if !plugins::mount::available() {
            return Err(plugins::mount::MountError::NotEnabled);
        }
        let inners: Vec<String> = sources.iter().filter_map(|s| self.in_archive(s).map(|(_, _, inner)| inner)).collect();
        let m = plugins::mount::mount(plugin, &file)?;
        m.prepare(inners.clone());
        Ok(inners.iter().map(|i| m.path_of(i)).collect())
    }

    /// Whether items can be dropped into panel `idx`: not into a plugin's
    /// panel that takes no files (search results, a read-only archive).
    fn takes_drops(&self, idx: usize) -> bool {
        match self.archive_folder(self.panels[idx].path()) {
            Some((plugin, file, _)) => !plugins::is_search_results(&file) && plugin.can_write(&file),
            None => true,
        }
    }

    /// The places items can be dropped on: the folders' rows (and ..), then
    /// each panel as a whole (its folder).
    fn drop_zones(&self) -> Vec<dnd::Zone> {
        let mut zones = Vec::new();
        let takes = [self.takes_drops(0), self.takes_drops(1)];
        for idx in (0..2).filter(|&i| takes[i]) {
            let body = self.body_rects[idx];
            let entries = &self.panels[idx].listing.entries;
            for (rect, i) in &self.row_hits[idx] {
                if let Some(e) = entries.get(*i).filter(|e| e.kind != EntryKind::File) {
                    let rect = rect.intersect(body);
                    if rect.height() > 0.0 {
                        zones.push(dnd::Zone { rect, folder: e.path.clone(), volume: self.volume_of(&e.path) });
                    }
                }
            }
        }
        for idx in (0..2).filter(|&i| takes[i]) {
            let folder = self.panels[idx].path().to_path_buf();
            zones.push(dnd::Zone { rect: self.body_rects[idx], volume: self.volume_of(&folder), folder });
        }
        zones
    }

    /// Row `i` of panel `idx` is dragged: the selection when the row is in it,
    /// otherwise its item (which gets the cursor).
    fn start_drag(&mut self, idx: usize, i: usize) {
        let p = &mut self.panels[idx];
        let Some(e) = p.listing.entries.get(i).filter(|e| e.kind != EntryKind::UpDir) else { return };
        let sources = if e.selected {
            p.targets()
        } else {
            let path = e.path.clone();
            p.cursor = i;
            vec![path]
        };
        // a plugin's items leave the window as the files on disk they stand
        // for, or as files in the plugin's mount (an archive's, a
        // server's); a comparison's placeholders can't
        let outside = match self.real_items(&sources) {
            Ok(real) if !real.is_empty() => DragOut::Files(real),
            _ if self.in_archive(&sources[0]).is_some_and(|(_, file, _)| !plugins::is_results(&file)) => DragOut::Mount,
            _ => DragOut::No,
        };
        let volume = self.volume_of(&sources[0]);
        self.active = idx;
        self.panel_drag = Some(PanelDrag { sources, volume, outside });
    }

    /// Drags, every frame: files from other programs over the window, items
    /// dragged from a panel (where they would go, the drop, Esc), and out of
    /// the window a Windows drag.
    fn drag_ui(&mut self, ctx: &egui::Context) {
        let zones = self.drop_zones();
        let outside = self.drop_shared.as_ref().map(|s| {
            let mut s = s.lock().unwrap();
            s.zones = zones.clone();
            (s.outside.hover.clone(), s.outside.dropped.take())
        });
        if let Some((hover, dropped)) = outside {
            if let Some((pos, files, shift, ctrl)) = hover {
                let volume = files.first().map(|f| dnd::volume(f)).unwrap_or_default();
                paint_drop(ctx, pos, &files, dnd::resolve(&zones, pos, &files, &volume, shift, ctrl).as_ref());
            }
            if let Some((files, folder, kind)) = dropped {
                self.drop_items(ctx, kind, files, &folder);
            }
        }
        let Some(drag) = &self.panel_drag else { return };
        let (pos, down, escape, m) = ctx.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down(), i.key_pressed(Key::Escape), i.modifiers));
        if escape {
            self.panel_drag = None;
            return;
        }
        // every frame while dragging: the window hears little of a pointer
        // outside it, so where the pointer is is asked of Windows
        ctx.request_repaint();
        let gone = self.hwnd.and_then(dnd::pointer_outside).unwrap_or(false) || pos.is_some_and(|p| !ctx.screen_rect().contains(p));
        // out of the window: a Windows drag, to Explorer or another program,
        // which copies or moves by the same rule
        if !matches!(drag.outside, DragOut::No) && down && gone {
            let drag = self.panel_drag.take().unwrap();
            if let Some(h) = self.hwnd {
                let r = match drag.outside {
                    DragOut::Files(files) => dnd::drag_out(h, &files, true),
                    // copied, never moved: the plugin's items stay where they are
                    _ => match self.mount_items(&drag.sources) {
                        Ok(paths) => dnd::drag_out(h, &paths, false),
                        Err(plugins::mount::MountError::NotEnabled) => {
                            // Windows' Projected File System, which makes them files on disk, is off
                            let text = tr("Dragging items out of archives and servers to other programs needs the Windows feature \"Windows Projected File System\", which is turned off.\n\nTo turn it on, open Turn Windows features on or off (optionalfeatures.exe), check Windows Projected File System and confirm. Or run this in PowerShell as administrator:\nEnable-WindowsOptionalFeature -Online -FeatureName Client-ProjFS\n\nUntil then, copy the items with F5, or drag them to the other panel.");
                            self.open_dialog(Dialog::Message { title: tr("Dragging out of the window").into(), text: text.into() });
                            Ok(())
                        }
                        Err(plugins::mount::MountError::Failed(e)) => Err(e),
                    },
                };
                if let Err(e) = r {
                    self.error(e);
                }
                self.release_pointer = true;
                self.reload_all();
            }
            return;
        }
        let Some(pos) = pos else { return };
        let hit = dnd::resolve(&zones, pos, &drag.sources, &drag.volume, m.shift, m.ctrl);
        if !down {
            let drag = self.panel_drag.take().unwrap();
            if let Some((z, kind)) = hit {
                self.drop_items(ctx, kind, drag.sources, &z.folder);
            }
            return;
        }
        // the arrow where they can go (Windows has no "grabbing" hand: it
        // would be the four-way move arrows), "no" where they can't
        ctx.set_cursor_icon(if hit.is_some() { egui::CursorIcon::Default } else { egui::CursorIcon::NoDrop });
        paint_drop(ctx, pos, &drag.sources, hit.as_ref());
    }

    /// `sources` dropped on `folder`: the Copy or Move dialog for them, or,
    /// with the confirmation off in Settings, the operation at once.
    fn drop_items(&mut self, ctx: &egui::Context, kind: OpKind, sources: Vec<PathBuf>, folder: &Path) {
        let opts = if kind == OpKind::Copy { self.cfg.copy_defaults } else { self.cfg.move_defaults };
        let d = Dialog::Transfer { kind, sources, target: transfer_target(folder), opts };
        if self.cfg.confirm_drop {
            self.open_dialog(d);
        } else {
            self.dialog_ok(ctx, d);
        }
    }

    // ---------------------------------------------------------------- dialogs

    /// The open dialog, in its window, and what its buttons do.
    fn dialogs(&mut self, ctx: &egui::Context) {
        let mut dialog = std::mem::replace(&mut self.dialog, Dialog::None);
        let (mut enter, mut escape) = ctx.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
        // keys typed into an open drop-down (font search) are not meant for the dialog
        if ctx.memory(|m| m.any_popup_open()) {
            enter = false;
            escape = false;
        }
        let focus = std::mem::take(&mut self.focus_dialog);
        if matches!(dialog, Dialog::None) {
            dialogwin::close(&mut self.dialog_places, self.hwnd, |k| !OWN_WINDOWS.contains(&k));
        }
        let dw = DialogWin { owner: self.hwnd, theme: self.cfg.theme, modal: false, exclusive: false, icon: None, animations: animated(self.cfg.animations) };
        // dialog windows are sized from the main window, not from themselves
        let main_h = ctx.screen_rect().height();
        // 0 = keep open, 1 = ok, 2 = cancel
        let mut outcome = 0u8;
        let text_field = |ui: &mut egui::Ui, s: &mut String| {
            let r = ui.add(layout::text_field(s).desired_width(f32::INFINITY));
            // a new window's first frame is an invisible sizing pass that drops
            // focus requests, so keep claiming focus while nothing holds it
            if focus || ui.memory(|m| m.focused().is_none()) {
                r.request_focus();
            }
        };
        let buttons = |ui: &mut egui::Ui, ok: &str| -> u8 {
            layout::before_buttons(ui);
            match layout::buttons(ui, &[(ok, ""), (tr("Cancel"), "")], Some(0)) {
                Some(0) => 1,
                Some(_) => 2,
                None => 0,
            }
        };

        let mut browse_field: Option<String> = None;
        let mut drive_disconnect: Option<PathBuf> = None;
        let mut drive_refresh: Option<(String, PathBuf, String)> = None;
        match &mut dialog {
            Dialog::None => return,
            Dialog::PluginForm { form, values, .. } => {
                dw.modal().show(ctx, &mut self.dialog_places, "plugin-form", &form.title, 640.0, &mut enter, &mut escape, &mut |ui| {
                    plugin_form(ui, form, values, focus, &mut browse_field);
                    outcome = buttons(ui, &form.ok);
                });
            }
            Dialog::Transfer { kind, sources, target, opts } => {
                let verb = if *kind == OpKind::Copy { tr("Copy") } else { tr("Move") };
                let busy = self.jobs.running.len() + self.jobs.queue.len();
                dw.modal().show(ctx, &mut self.dialog_places, "transfer", verb, 560.0, &mut enter, &mut escape, &mut |ui| {
                    ui.label(if *kind == OpKind::Copy { trf("Copy {what} to:", &[("what", &describe(sources))]) } else { trf("Move {what} to:", &[("what", &describe(sources))]) });
                    text_field(ui, target);
                    ui.small(tr("Target can be a directory, a new name, or a mask such as *.bak"));
                    ui.add_space(layout::GAP);
                    transfer_options(ui, *kind, opts);
                    if busy > 0 {
                        ui.small(trn("{n} operation is running or waiting.", "{n} operations are running or waiting.", busy as u64));
                    }
                    layout::before_buttons(ui);
                    let buttons = [
                        (verb, tr("Start now, alongside the running operations")),
                        (tr("Run first"), tr("Pause the running operations and run this one; they go on when it finishes")),
                        (tr("Queue last"), tr("Wait until the running and the waiting operations finish")),
                        (tr("Cancel"), ""),
                    ];
                    match layout::buttons(ui, &buttons, Some(0)) {
                        Some(0) => outcome = 1,
                        Some(1) => outcome = 3,
                        Some(2) => outcome = 4,
                        Some(_) => outcome = 2,
                        None => {}
                    }
                });
            }
            Dialog::MkDir { name } => {
                dw.modal().show(ctx, &mut self.dialog_places, "mkdir", tr("Create Directory"), 440.0, &mut enter, &mut escape, &mut |ui| {
                    ui.label(tr("Directory name (nested folders: /foo1/foo2/foo3):"));
                    text_field(ui, name);
                    outcome = buttons(ui, tr("Create"));
                });
            }
            Dialog::Rename { path, name } => {
                let old = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                dw.modal().show(ctx, &mut self.dialog_places, "rename", tr("Rename"), 440.0, &mut enter, &mut escape, &mut |ui| {
                    ui.label(trf("Rename \"{old}\" to:", &[("old", &old)]));
                    text_field(ui, name);
                    outcome = buttons(ui, tr("Rename"));
                });
            }
            Dialog::Delete { sources, recycle } => {
                let from_archive = sources.first().is_some_and(|s| self.in_archive(s).is_some());
                dw.modal().show(ctx, &mut self.dialog_places, "delete", tr("Delete"), 440.0, &mut enter, &mut escape, &mut |ui| {
                    ui.label(trf("Do you want to delete {what}?", &[("what", &describe(sources))]));
                    if from_archive {
                        *recycle = false;
                        ui.colored_label(ui.visuals().warn_fg_color, tr("Items will be deleted permanently, by the plugin."));
                    } else {
                        ui.checkbox(recycle, tr("Move to Recycle Bin"));
                    }
                    if !*recycle && !from_archive {
                        ui.colored_label(ui.visuals().warn_fg_color, tr("Items will be deleted permanently."));
                    }
                    outcome = buttons(ui, tr("Delete"));
                });
            }
            Dialog::SelectMask { select, mask, error } => {
                dw.modal().show(ctx, &mut self.dialog_places, "mask", if *select { tr("Select") } else { tr("Unselect") }, 440.0, &mut enter, &mut escape, &mut |ui| {
                    ui.label(tr("Mask (e.g. *.txt;*.doc or *.h*|*.html):"));
                    text_field(ui, mask);
                    if let Some(e) = error {
                        ui.colored_label(ui.visuals().error_fg_color, e.as_str());
                    }
                    outcome = buttons(ui, tr("OK"));
                });
            }
            Dialog::Drive { panel, drives, conns, paths, cursor } => {
                // the last row: the other panel's folder
                let other_row = drives.len() + conns.len() + paths.len();
                let other_path = display_path(self.panels[1 - *panel].path());
                let count = other_row + 1;
                let (up, down) = ctx.input(|i| (i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown)));
                if up {
                    *cursor = cursor.saturating_sub(1);
                }
                if down && *cursor + 1 < count {
                    *cursor += 1;
                }
                // typing a drive letter picks it
                let typed: Vec<char> = ctx.input(|i| i.events.iter().filter_map(|e| if let Event::Text(t) = e { t.chars().next() } else { None }).collect());
                for c in typed {
                    if let Some(k) = drives.iter().position(|d| d.letter.starts_with(c.to_ascii_uppercase())) {
                        *cursor = k;
                        outcome = 1;
                    }
                }
                // a dropdown right under the panel's path bar, as wide as needed;
                // it stays below the menu bar, and a long list scrolls
                let panel_rect = self.body_rects[*panel];
                let pos = panel_rect.left_top() + egui::vec2(4.0, 0.0);
                let screen = ctx.screen_rect();
                // wider for a long connection name (with its Disconnect button), up to the screen's edge
                let font = egui::TextStyle::Body.resolve(&ctx.style());
                let longest = conns.iter().map(|c| ctx.fonts(|f| f.layout_no_wrap(c.label.clone(), font.clone(), Color32::WHITE).size().x)).fold(0.0f32, f32::max);
                let small = egui::TextStyle::Small.resolve(&ctx.style());
                let button_w =
                    [tr("Disconnect"), tr("Close")].iter().map(|l| ctx.fonts(|f| f.layout_no_wrap(l.to_string(), small.clone(), Color32::WHITE).size().x)).fold(0.0f32, f32::max)
                        + 16.0;
                let refresh_w = ctx.fonts(|f| f.layout_no_wrap(tr("Refresh").to_owned(), small.clone(), Color32::WHITE).size().x) + 16.0;
                let has_refresh = conns.iter().any(|c| !c.refresh.is_empty());
                let needed = if conns.is_empty() { 0.0 } else { longest + 40.0 + button_w + 24.0 + if has_refresh { refresh_w + 4.0 } else { 0.0 } };
                let width = 420.0f32.min(panel_rect.width() - 8.0).max(280.0).max(needed).min(screen.right() - pos.x - 16.0);
                let row_h = ctx.style().text_styles.get(&egui::TextStyle::Body).map_or(14.0, |f| f.size) + 9.0;
                // as tall as its rows (measured last frame; estimated on the
                // first): below the path bar while that fits, otherwise moved
                // up over the window; it scrolls only when the rows are
                // taller than the whole window
                let content_id = egui::Id::new("drive_selector_rows");
                let rows = (count + 5) as f32;
                let content_h: f32 = ctx.data(|d| d.get_temp(content_id)).unwrap_or(rows * (row_h + 1.0));
                let (margin, frame) = (4.0, 16.0);
                // a few points of slack: with display scaling the rows round up a
                // little, which would otherwise show a scroll bar
                let list_h = (content_h.ceil() + 4.0).min(screen.height() - 2.0 * margin - frame).max(row_h * 3.0);
                let top = if pos.y + list_h + frame <= screen.bottom() - margin { pos.y } else { (screen.bottom() - margin - frame - list_h).max(screen.top() + margin) };
                let pos = egui::pos2(pos.x, top);
                let bounds = screen;
                let moved = up || down;
                // egui keeps an area's size from frame to frame (starting at
                // 600 × 400): one per height, sized for its rows, or it stays as
                // small as when it was first shown
                let shown = egui::Area::new(egui::Id::new(("drive_selector", list_h.round() as i32)))
                    .default_size(egui::vec2(width + frame, list_h + frame))
                    .order(egui::Order::Foreground)
                    .fixed_pos(pos)
                    .constrain_to(bounds)
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.set_width(width);
                            let list = egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
                                ui.set_width(width);
                                ui.spacing_mut().item_spacing.y = layout::MENU_ROW_SPACING;
                                let shown_cursor = *cursor;
                                let keep_visible = |r: &egui::Response, i: usize| {
                                    if moved && i == shown_cursor {
                                        r.scroll_to_me(None);
                                    }
                                };
                                for (k, d) in drives.iter().enumerate() {
                                    let detail = match &d.remote {
                                        Some(r) => format!("{} · {r}", tr(d.kind.label())),
                                        None => tr(d.kind.label()).to_owned(),
                                    };
                                    // local drives first, then network: a separator marks the change
                                    if k > 0 && drives[k - 1].kind.is_local() != d.kind.is_local() {
                                        ui.add_space(layout::MENU_GAP);
                                        ui.separator();
                                        ui.add_space(layout::MENU_GAP);
                                    }
                                    let r = drive_icon::row(ui, d.kind, &d.letter, &d.label, &detail, k == *cursor).on_hover_text(describe_drive(d));
                                    keep_visible(&r, k);
                                    if r.clicked() {
                                        *cursor = k;
                                        outcome = 1;
                                    }
                                }
                                // servers a plugin is connected to, then searches, then comparisons
                                if !conns.is_empty() {
                                    for (k, c) in conns.iter().enumerate() {
                                        if k == 0 || c.group() != conns[k - 1].group() {
                                            ui.add_space(layout::MENU_GAP);
                                            ui.separator();
                                            ui.label(RichText::new(conn_group_name(c.group())).small().weak());
                                        }
                                        let i = drives.len() + k;
                                        let tip = format!(
                                            "{} · {}\n{}\n{}",
                                            c.label,
                                            c.detail,
                                            display_path(&c.start).trim_end_matches('/'),
                                            if c.results { tr("Delete: close") } else { tr("Delete: disconnect") }
                                        );
                                        let refreshable = !c.refresh.is_empty();
                                        let reserve = button_w + 8.0 + if refreshable { refresh_w + 4.0 } else { 0.0 };
                                        let icon = if c.results { plugins::results_icon(&c.root) } else { sal_ui::mdi::SERVER_NETWORK };
                                        let r = drive_icon::connection_row(ui, icon, &c.label, i == *cursor, reserve).on_hover_text(tip);
                                        keep_visible(&r, i);
                                        // compact buttons inside the row's right end; they take their clicks
                                        let at = egui::Rect::from_min_max(
                                            egui::pos2(r.rect.right() - 4.0 - button_w, r.rect.top() + 3.0),
                                            egui::pos2(r.rect.right() - 4.0, r.rect.bottom() - 3.0),
                                        );
                                        let refresh_at = egui::Rect::from_min_max(egui::pos2(at.left() - 4.0 - refresh_w, at.top()), egui::pos2(at.left() - 4.0, at.bottom()));
                                        let (label, tip) = match c.group() {
                                            2 => (tr("Close"), tr("Close this comparison (both panels)")),
                                            1 => (tr("Close"), tr("Close these results")),
                                            _ => (tr("Disconnect"), tr("Close this connection")),
                                        };
                                        let refresh_tip = if c.paired { tr("Compare again") } else { tr("Search again with the same settings") };
                                        if row_button(ui, at, label, &small, ("disconnect", k)).on_hover_text(tip).clicked() {
                                            drive_disconnect = Some(c.root.clone());
                                        } else if refreshable
                                            && row_button(ui, refresh_at, tr("Refresh"), &small, ("refresh", k)).on_hover_text(refresh_tip).clicked()
                                        {
                                            drive_refresh = Some((c.plugin.clone(), c.root.clone(), c.refresh.clone()));
                                        } else if r.clicked() {
                                            *cursor = i;
                                            outcome = 1;
                                        }
                                    }
                                }
                                if !paths.is_empty() {
                                    ui.add_space(layout::MENU_GAP);
                                    ui.separator();
                                    ui.label(RichText::new(tr("Recent paths")).small().weak());
                                    for (k, p) in paths.iter().enumerate() {
                                        let i = drives.len() + conns.len() + k;
                                        let kind = drives::kind_of_root(&drives::root_of(Path::new(p)), drives.as_slice());
                                        let r = drive_icon::path_row(ui, kind, &display_name(p), i == *cursor).on_hover_text(p.as_str());
                                        keep_visible(&r, i);
                                        if r.clicked() {
                                            *cursor = i;
                                            outcome = 1;
                                        }
                                    }
                                }
                                ui.add_space(layout::MENU_GAP);
                                ui.separator();
                                ui.add_space(layout::MENU_GAP);
                                let (icon, label) = from_other_label(*panel);
                                let r = drive_icon::glyph_row(ui, icon, drive_icon::color(sal_core::drives::DriveType::Fixed), label, &other_path, other_row == *cursor)
                                    .on_hover_text(other_path.as_str());
                                keep_visible(&r, other_row);
                                if r.clicked() {
                                    *cursor = other_row;
                                    outcome = 1;
                                }
                            });
                            // the rows' height for the next frame; another frame when it changed
                            let h = list.content_size.y;
                            if (h - content_h).abs() > 0.5 {
                                ctx.data_mut(|d| d.insert_temp(content_id, h));
                                ctx.request_repaint();
                            }
                        });
                    });
                let rect = shown.response.rect;
                self.drive_popup_rect = Some(rect);
                // a click elsewhere closes it, like a menu
                if outcome == 0 && ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p))) {
                    outcome = 2;
                }
            }
            Dialog::Message { title, text } => {
                dw.modal().show(ctx, &mut self.dialog_places, "message", title.as_str(), 440.0, &mut enter, &mut escape, &mut |ui| {
                    ui.label(text.as_str());
                    layout::before_buttons(ui);
                    if layout::buttons(ui, &[(tr("OK"), "")], Some(0)).is_some() {
                        outcome = 2;
                    }
                });
                if enter {
                    outcome = 2;
                }
            }
            Dialog::Update { rows } => {
                use crate::update::{Progress, Target};
                let states: Vec<Option<Progress>> = rows.iter().map(|r| r.install.as_ref().map(|i| i.lock().unwrap().clone())).collect();
                let running = states.iter().any(|s| matches!(s, Some(Progress::Downloading { .. } | Progress::Unpacking)));
                let staged = states.iter().any(|s| matches!(s, Some(Progress::Staged)));
                let ready = startable(rows, &states);
                let (mut open_page, mut start, mut toggle_log, mut run_new, mut restart) = (None, None, None, None, false);
                let (mut update_all, mut decline) = (false, None);
                let title = match (rows.iter().all(|r| r.update.new), rows.len() == 1) {
                    (true, true) => tr("New plugin available"),
                    (true, false) => tr("New plugins available"),
                    (false, true) => tr("Update available"),
                    (false, false) => tr("Updates available"),
                };
                dw.modal().show(ctx, &mut self.dialog_places, "update", title, 560.0, &mut enter, &mut escape, &mut |ui| {
                    for (i, (row, state)) in rows.iter().zip(&states).enumerate() {
                        let u = &row.update;
                        layout::group_frame(ui.style()).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(u.name()).strong());
                                if u.new {
                                    ui.label(trf("new plugin, version {new}", &[("new", &u.release.version)]));
                                } else {
                                    ui.label(trf("{current} → {new}", &[("current", &u.current), ("new", &u.release.version)]));
                                }
                                if ui.link(tr("Release page")).on_hover_text(u.release.page.as_str()).clicked() {
                                    open_page = Some(u.release.page.clone());
                                }
                            });
                            if !u.about.is_empty() {
                                ui.label(u.about.as_str());
                            }
                            let plugin = matches!(u.target, Target::Plugin { .. });
                            // a plugin made for other program versions is shown, greyed
                            let fits = u.fits();
                            let mut offer = |ui: &mut egui::Ui, action: Option<&str>| {
                                ui.horizontal(|ui| {
                                    let log = if row.show_log { tr("Hide changelog") } else { tr("Show changelog") };
                                    if ui.button(log).clicked() {
                                        toggle_log = Some(i);
                                    }
                                    if let Some(action) = action {
                                        if ui.add_enabled(!running && fits.is_ok(), egui::Button::new(action)).clicked() {
                                            start = Some(i);
                                        }
                                        if u.new && ui.button(tr("Don't offer")).on_hover_text(tr("Not offered again when RustySalamander starts; Help → Check for updates still shows it")).clicked() {
                                            decline = Some(i);
                                        }
                                    }
                                });
                            };
                            match state {
                                None => {
                                    if let Err(why) = &fits {
                                        ui.colored_label(ui.visuals().warn_fg_color, trf("This version can't be installed here: it {why}.", &[("why", why)]));
                                    } else if u.new {
                                        ui.label(tr("Install downloads it; it is added when RustySalamander starts again."));
                                    } else if plugin {
                                        ui.label(tr("Update now downloads it; it is installed when RustySalamander starts again. What the plugin saved stays."));
                                    } else {
                                        let folder = crate::update::target_dir(&u.release).map(|p| display_path(&p)).unwrap_or_default();
                                        ui.label(trf(
                                            "Update now downloads the new version, unpacks it to {folder} and copies your settings there. This version stays as it is.",
                                            &[("folder", &folder)],
                                        ));
                                    }
                                    offer(ui, Some(if u.new { tr("Install") } else { tr("Update now") }));
                                }
                                Some(Progress::Downloading { done, total }) => {
                                    ui.label(trf("Downloading… {done} of {total}", &[("done", &format_size(*done)), ("total", &format_size(*total))]));
                                    ui.add(progress_bar(if *total > 0 { *done as f32 / *total as f32 } else { 0.0 }));
                                }
                                Some(Progress::Unpacking) => {
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label(tr("Unpacking…"));
                                    });
                                }
                                Some(Progress::Done { folder, copied }) => {
                                    ui.label(trf("RustySalamander {new} is installed in {folder}.", &[("new", &u.release.version), ("folder", &display_path(folder))]));
                                    ui.label(if copied.is_empty() {
                                        tr("There were no settings to copy.").to_owned()
                                    } else {
                                        trf("Settings copied: {files}", &[("files", &copied.join(", "))])
                                    });
                                    ui.horizontal(|ui| {
                                        if ui.button(tr("Start the new version")).on_hover_text(tr("Closes this version")).clicked() {
                                            run_new = Some(folder.clone());
                                        }
                                    });
                                }
                                Some(Progress::Staged) => {
                                    ui.label(tr("Downloaded. It is installed when RustySalamander starts again."));
                                    offer(ui, None);
                                }
                                Some(Progress::Failed(e)) => {
                                    ui.colored_label(ui.visuals().error_fg_color, trf("The update failed: {error}", &[("error", e)]));
                                    offer(ui, Some(tr("Try again")));
                                }
                            }
                            if row.show_log {
                                ui.add_space(layout::GAP_SMALL);
                                let log = row.changelog.as_ref().and_then(|c| c.lock().unwrap().clone());
                                match log {
                                    None => {
                                        ui.horizontal(|ui| {
                                            ui.spinner();
                                            ui.label(tr("Reading the changelog…"));
                                        });
                                    }
                                    Some(Ok(text)) if text.is_empty() => {
                                        ui.label(RichText::new(tr("The changelog says nothing about these versions.")).weak());
                                    }
                                    Some(Ok(text)) => {
                                        layout::well_frame(ui.style()).show(ui, |ui| {
                                            egui::ScrollArea::vertical().id_salt(("changelog", i)).max_height(240.0).auto_shrink([false, true]).show(ui, |ui| markdown(ui, &text));
                                        });
                                    }
                                    Some(Err(e)) => {
                                        ui.colored_label(ui.visuals().error_fg_color, trf("Could not read the changelog: {error}", &[("error", &e)]));
                                    }
                                }
                            }
                        });
                        ui.add_space(layout::GAP);
                    }
                    if staged {
                        ui.label(tr("Plugin updates are installed when RustySalamander starts again."));
                    }
                    layout::before_buttons(ui);
                    // Update all when more than one can start; Restart now once a
                    // plugin waits for it; Close
                    let all = ready.len() > 1 && !running;
                    let mut buttons = Vec::new();
                    if all {
                        buttons.push((tr("Update all"), tr("Downloads and installs every update that can be installed")));
                    }
                    if staged {
                        buttons.push((tr("Restart now"), tr("Closes and starts RustySalamander again")));
                    }
                    buttons.push((tr("Close"), ""));
                    // Enter closes, except while a download runs
                    let default = (!running).then(|| buttons.len() - 1);
                    if let Some(b) = layout::buttons(ui, &buttons, default) {
                        match (b, all, staged) {
                            (0, true, _) => update_all = true,
                            (0, false, true) | (1, true, true) => restart = true,
                            _ => outcome = 2,
                        }
                    }
                });
                // the window stays while a download runs
                if running {
                    enter = false;
                    escape = false;
                    if outcome == 2 {
                        outcome = 0;
                    }
                }
                if let Some(page) = open_page {
                    if let Err(e) = shell_open(Path::new(&page)) {
                        self.error(format!("{page}: {e}"));
                    }
                }
                if let Some(i) = decline {
                    if let Target::Plugin { id, .. } = &rows[i].update.target {
                        if !self.cfg.declined_plugins.contains(id) {
                            self.cfg.declined_plugins.push(id.clone());
                        }
                    }
                    rows.remove(i);
                    if rows.is_empty() {
                        outcome = 2;
                    }
                }
                if let Some(i) = toggle_log.filter(|&i| i < rows.len()) {
                    let row = &mut rows[i];
                    row.show_log = !row.show_log;
                    if row.changelog.is_none() {
                        row.changelog = Some(crate::update::changelog(ctx, &row.update));
                    }
                }
                let to_start: Vec<usize> = if update_all { ready } else { start.into_iter().collect() };
                // nothing starts in the frame a row was taken away (the indexes moved)
                let to_start = if decline.is_some() { Vec::new() } else { to_start };
                for i in to_start {
                    // installing one turned down before offers it again
                    if let Target::Plugin { id, .. } = &rows[i].update.target {
                        self.cfg.declined_plugins.retain(|d| d != id);
                    }
                    if rows[i].update.target == Target::Program {
                        // the settings as they are now go to the new version
                        self.save_config();
                    }
                    rows[i].install = Some(crate::update::install(ctx, rows[i].update.clone()));
                }
                if let Some(folder) = run_new {
                    let exe = folder.join("rustysalamander.exe");
                    match std::process::Command::new(&exe).current_dir(&folder).spawn() {
                        Ok(_) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => self.error(format!("{}: {e}", display_path(&exe))),
                    }
                }
                if restart {
                    self.save_config();
                    match std::env::current_exe().and_then(|exe| std::process::Command::new(&exe).current_dir(exe.parent().unwrap_or(Path::new("."))).spawn()) {
                        Ok(_) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => self.error(format!("RustySalamander: {e}")),
                    }
                }
            }
            Dialog::Settings { tab, recording } => {
                let cfg = &mut self.cfg;
                let mut edit: Option<KeyEdit> = None;
                let mut slider_rect = None;
                let mut reload_panels = false;
                let mut fonts_changed = false;
                let mut browse: Option<Browse> = None;
                let mut font_button_rect = None;
                if self.font_list.is_empty() {
                    self.font_list = crate::fonts::system_fonts();
                }
                let font_list = &self.font_list;
                let languages = &self.languages;
                let mut lang_changed = false;
                let mut plugins_changed = false;
                let view_choices = opener_choices(&self.plugins, OpenKind::View);
                let edit_choices = opener_choices(&self.plugins, OpenKind::Edit);
                let suffix_edit = &mut self.suffix_edit;
                let mut check_now = false;
                // each plugin found: id, name, description, its file and its
                // state: running (its status) or why it couldn't start
                let plugin_rows: Vec<PluginRow> = self
                    .plugins
                    .found
                    .iter()
                    .map(|f| {
                        let id = &f.info.id;
                        let state = match (self.plugins.get(id), self.plugins.errors.iter().find(|(e, _)| e == id)) {
                            (Some(p), _) => Ok(p.status()),
                            (None, Some((_, e))) => Err(e.clone()),
                            (None, None) => Ok(String::new()),
                        };
                        let openers = self.plugins.get(id).map(|p| p.openers()).unwrap_or_default();
                        let openers = openers
                            .iter()
                            .map(|o| {
                                let what = if o.kind == OpenKind::View { tr("Views (F3)") } else { tr("Edits (F4)") };
                                format!("{what}: {} · {}", o.label, o.masks)
                            })
                            .collect();
                        let version = trf("Version {version} · interface {interface}", &[("version", &f.info.version), ("interface", &f.info.interface)]);
                        PluginRow { id: id.clone(), name: f.tr(&f.info.name), description: f.tr(&f.info.description), file: display_path(&f.file), version, openers, state }
                    })
                    .collect();
                let broken_plugins: Vec<(String, String)> = self.plugins.broken.iter().map(|(f, e)| (display_path(f), e.clone())).collect();
                let plugins_dir = plugins::plugins_dir().map(|d| display_path(&d)).unwrap_or_default();
                let font_filter = &mut self.font_filter;
                let mut captured = false;
                // fixed width: the scroll areas inside take all the width they are given
                dw.with_icon(sal_ui::icon::settings).modal().show(ctx, &mut self.dialog_places, "settings", tr("Settings"), 860.0, &mut enter, &mut escape, &mut |ui| {
                    if let Some(action) = *recording {
                        // capture the next key combination; Esc cancels
                        let pressed = ui.input(|i| {
                            i.events.iter().find_map(|e| match e {
                                Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                                _ => None,
                            })
                        });
                        if let Some((key, m)) = pressed {
                            if !(key == Key::Escape && m.is_none()) {
                                keymap::add_binding(&mut cfg.shortcuts, action, &Shortcut::new(key, m).to_string());
                            }
                            *recording = None;
                            ui.input_mut(|i| i.events.clear());
                            captured = true;
                        }
                    }
                    ui.horizontal(|ui| {
                        ui.selectable_value(tab, SettingsTab::General, tr("General"));
                        ui.selectable_value(tab, SettingsTab::Keyboard, tr("Keyboard shortcuts"));
                        ui.selectable_value(tab, SettingsTab::Programs, tr("Viewers and editors"));
                        ui.selectable_value(tab, SettingsTab::Plugins, tr("Plugins"));
                    });
                    ui.separator();
                    match tab {
                        SettingsTab::General => {
                            let list_h = (main_h - 300.0).max(260.0);
                            egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
                                // one grid per section, same first-column width, a line between them
                                let section = |ui: &mut egui::Ui, title: &'static str, first: bool, body: &mut dyn FnMut(&mut egui::Ui)| {
                                    if !first {
                                        layout::divider(ui);
                                    }
                                    ui.label(RichText::new(tr(title)).strong());
                                    ui.add_space(layout::GAP_SMALL);
                                    layout::form_grid(("settings", title)).min_col_width(190.0).show(ui, |ui| body(ui));
                                };

                                section(ui, n_("Appearance"), true, &mut |ui| {
                                    ui.label(tr("Language"));
                                    let shown = languages.iter().find(|l| l.code == cfg.language).map_or(tr("System default"), |l| l.name.as_str());
                                    egui::ComboBox::from_id_salt("language").width(200.0).selected_text(shown).show_ui(ui, |ui| {
                                        lang_changed |= ui.selectable_value(&mut cfg.language, String::new(), tr("System default")).changed();
                                        for l in languages {
                                            lang_changed |= ui.selectable_value(&mut cfg.language, l.code.clone(), l.name.as_str()).changed();
                                        }
                                    });
                                    ui.end_row();

                                    ui.label(tr("Theme"));
                                    ui.horizontal(|ui| {
                                        for (mode, label) in THEMES {
                                            if ui.radio_value(&mut cfg.theme, mode, tr(label)).changed() {
                                                apply_theme(ctx, mode);
                                            }
                                        }
                                    });
                                    ui.end_row();

                                    ui.label(tr("Accent color")).on_hover_text(tr("The color of what is selected or active: the cursor, the active path bar, selected tabs and buttons; panels a plugin shows are tinted with it"));
                                    ui.vertical(|ui| {
                                        let before = (cfg.accent_light, cfg.accent_dark);
                                        for (theme, label, slot) in [
                                            (egui::Theme::Light, tr("Light theme:"), &mut cfg.accent_light),
                                            (egui::Theme::Dark, tr("Dark theme:"), &mut cfg.accent_dark),
                                        ] {
                                            ui.push_id(label, |ui| accent_choice(ui, label, theme, slot));
                                        }
                                        if (cfg.accent_light, cfg.accent_dark) != before {
                                            sal_ui::apply_accent(ctx, cfg.accent_light, cfg.accent_dark);
                                        }
                                    });
                                    ui.end_row();

                                    ui.label(tr("Font"));
                                    let (changed, rect) = font_picker(ui, "ui_font", &mut cfg.ui_font, crate::fonts::DEFAULT_UI_FONT, font_list, font_filter);
                                    fonts_changed |= changed;
                                    font_button_rect = Some(rect);
                                    ui.end_row();

                                    ui.label(tr("Font size"));
                                    // applying while dragging would resize the dialog and move the
                                    // slider under the pointer: apply when the drag ends (clicks and
                                    // arrow keys still apply at once)
                                    let r = ui.add(egui::Slider::new(&mut cfg.font_size, 10.0..=24.0).step_by(1.0).suffix(" pt"));
                                    slider_rect = Some(r.rect);
                                    if r.drag_stopped() || (r.changed() && !r.dragged()) {
                                        apply_font_size(ctx, cfg.font_size);
                                    }
                                    ui.end_row();

                                    ui.label(tr("Viewer font"));
                                    fonts_changed |= font_picker(ui, "mono_font", &mut cfg.mono_font, crate::fonts::DEFAULT_MONO_FONT, font_list, font_filter).0;
                                    ui.end_row();

                                    ui.label(tr("File icons"));
                                    ui.checkbox(&mut cfg.system_icons, tr(LBL_ICONS));
                                    ui.end_row();

                                    ui.label(tr("Misc"));
                                    ui.vertical(|ui| {
                                        if ui.checkbox(&mut cfg.always_show_scrollbars, tr(LBL_SCROLLBARS)).changed() {
                                            sal_ui::apply_scrollbars(ctx, cfg.always_show_scrollbars);
                                        }
                                        if crate::gdiwin::chosen() {
                                            animations_off(ui);
                                        } else if ui.checkbox(&mut cfg.animations, tr(LBL_ANIMATIONS)).on_hover_text(tr(TIP_ANIMATIONS)).changed() {
                                            apply_animations(ctx, cfg.animations);
                                        }
                                        ui.checkbox(&mut cfg.show_fkey_bar, tr(LBL_FKEYS));
                                    });
                                    ui.end_row();
                                });

                                section(ui, n_("Files"), false, &mut |ui| {
                                    ui.label(tr("Visibility"));
                                    ui.vertical(|ui| {
                                        reload_panels |= ui.checkbox(&mut cfg.show_hidden, tr(LBL_HIDDEN)).changed();
                                        reload_panels |= ui.checkbox(&mut cfg.show_system, tr(LBL_SYSTEM)).changed();
                                    });
                                    ui.end_row();

                                    ui.label(tr("Deleting"));
                                    ui.vertical(|ui| {
                                        ui.checkbox(&mut cfg.use_recycle_bin, tr(LBL_RECYCLE));
                                        ui.checkbox(&mut cfg.confirm_delete, tr(LBL_CONFIRM));
                                    });
                                    ui.end_row();

                                    ui.label(tr("Drag and drop"));
                                    ui.checkbox(&mut cfg.confirm_drop, tr(LBL_CONFIRM_DROP)).on_hover_text(tr(
                                        "On the same drive dropped items are moved, onto another drive copied; Shift moves and Ctrl copies. Ticked, the Copy or Move dialog opens first.",
                                    ));
                                    ui.end_row();

                                    // what the Copy and Move dialogs offer each time they open
                                    for (label, kind) in [(tr("Copying (F5)"), OpKind::Copy), (tr("Moving (F6)"), OpKind::Move)] {
                                        ui.label(label).on_hover_text(tr("What the dialog offers each time it opens; a change there is for that operation only"));
                                        let d = if kind == OpKind::Copy { &mut cfg.copy_defaults } else { &mut cfg.move_defaults };
                                        ui.push_id(("transfer_defaults", label), |ui| {
                                            ui.vertical(|ui| transfer_choices(ui, kind, d));
                                        });
                                        ui.end_row();
                                    }
                                });

                                section(ui, n_("Quick search"), false, &mut |ui| {
                                    ui.label(tr("Match"));
                                    ui.horizontal(|ui| {
                                        ui.radio_value(&mut cfg.quick_search_anywhere, false, tr(LBL_QS_PREFIX));
                                        ui.radio_value(&mut cfg.quick_search_anywhere, true, tr(LBL_QS_ANYWHERE));
                                    });
                                    ui.end_row();

                                    ui.label(tr("Letters"));
                                    ui.horizontal(|ui| {
                                        ui.radio_value(&mut cfg.quick_search_loose, true, tr(LBL_QS_LOOSE)).on_hover_text(tr(TIP_QS_LOOSE));
                                        ui.radio_value(&mut cfg.quick_search_loose, false, tr(LBL_QS_EXACT)).on_hover_text(tr(TIP_QS_EXACT));
                                    });
                                    ui.end_row();

                                    ui.label(tr("Arrow keys"));
                                    ui.checkbox(&mut cfg.quick_search_arrows, tr(LBL_QS_ARROWS))
                                        .on_hover_text(tr("While a quick search is active, ↑/↓ skip to the previous/next matching item."));
                                    ui.end_row();
                                });

                                section(ui, n_("Drives"), false, &mut |ui| {
                                    ui.label(tr("Drive buttons"));
                                    ui.horizontal(|ui| {
                                        ui.radio_value(&mut cfg.drive_buttons, DriveButtons::Current, tr("Drive selector"));
                                        ui.radio_value(&mut cfg.drive_buttons, DriveButtons::All, tr("All drives"));
                                    });
                                    ui.end_row();

                                    ui.label(tr("Changing drive"));
                                    ui.checkbox(&mut cfg.drive_remember_dir, tr("Return to the last directory on that drive"));
                                    ui.end_row();

                                    ui.label(tr("Typed paths"));
                                    ui.vertical(|ui| {
                                        let n = cfg.typed_paths.len();
                                        let label = if n == 0 {
                                            tr("Remember typed paths in the drive selector").to_owned()
                                        } else {
                                            trf("Remember typed paths in the drive selector ({n} remembered)", &[("n", &n)])
                                        };
                                        if ui.checkbox(&mut cfg.remember_typed_paths, label).on_hover_text(tr("Turning this off deletes the remembered paths.")).changed()
                                            && !cfg.remember_typed_paths
                                        {
                                            cfg.typed_paths.clear();
                                        }
                                    });
                                    ui.end_row();
                                });

                                section(ui, n_("Context menu"), false, &mut |ui| {
                                    ui.label(tr("Opens on"));
                                    ui.vertical(|ui| {
                                        ui.radio_value(&mut cfg.context_menu_trigger, ContextMenuTrigger::ShortClick, tr("Right-click"));
                                        ui.radio_value(
                                            &mut cfg.context_menu_trigger,
                                            ContextMenuTrigger::LongPress,
                                            tr("Long right-click: hold, then release (a short right-click selects)"),
                                        );
                                        ui.add_enabled_ui(cfg.context_menu_trigger == ContextMenuTrigger::LongPress, |ui| {
                                            ui.add(egui::Slider::new(&mut cfg.long_press_ms, 250..=1500).step_by(50.0).suffix(" ms").text(tr("hold time")));
                                        });
                                    });
                                    ui.end_row();

                                    ui.label(tr("Style"));
                                    ui.vertical(|ui| {
                                        ui.radio_value(
                                            &mut cfg.context_menu_style,
                                            ContextMenuStyle::Windows11,
                                            tr("Windows 11 (compact native menu, \"Show more options\" for the full one)"),
                                        );
                                        ui.radio_value(
                                            &mut cfg.context_menu_style,
                                            ContextMenuStyle::Windows11Full,
                                            tr("Windows 11, full (the compact menu's commands with all shell extensions below them)"),
                                        );
                                        ui.radio_value(&mut cfg.context_menu_style, ContextMenuStyle::Classic, tr("Classic system menu (Windows 10 style, all shell extensions)"));
                                    });
                                    ui.end_row();
                                });

                                section(ui, n_("External programs"), false, &mut |ui| {
                                    ui.label(tr("Command prompt"));
                                    ui.vertical(|ui| {
                                        for kind in TerminalKind::ALL.into_iter().filter(|k| *k != TerminalKind::Custom) {
                                            ui.radio_value(&mut cfg.terminal, kind, tr(kind.label()));
                                        }
                                        ui.horizontal(|ui| {
                                            ui.radio_value(&mut cfg.terminal, TerminalKind::Custom, tr("Custom:"));
                                            ui.add_enabled(
                                                cfg.terminal == TerminalKind::Custom,
                                                egui::TextEdit::singleline(&mut cfg.terminal_custom).hint_text(r#"alacritty.exe --working-directory "{dir}""#).desired_width(250.0),
                                            )
                                            .on_hover_text(tr("Program and arguments. {dir} is replaced by the panel's directory,\nwhich is also the working directory."));
                                            if ui.button(tr("Browse…")).clicked() {
                                                browse = Some(Browse::Terminal);
                                            }
                                        });
                                    });
                                    ui.end_row();
                                });

                                section(ui, n_("Updates"), false, &mut |ui| {
                                    ui.label(tr("New versions"));
                                    ui.horizontal(|ui| {
                                        ui.checkbox(&mut cfg.check_updates, tr("Look for a new version on GitHub at start-up"));
                                        check_now |= ui.button(tr("Check now")).clicked();
                                    });
                                    ui.end_row();
                                });
                            });
                        }
                        SettingsTab::Programs => {
                            let list_h = (main_h - 300.0).max(260.0);
                            egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
                                ui.label(RichText::new(tr("F3 views and F4 edits files with these. A file suffix below can have its own.")).weak());
                                ui.add_space(layout::GAP);
                                layout::form_grid("open_defaults").show(ui, |ui| {
                                    ui.label(tr("Viewer (F3)"));
                                    let mut v = Some(cfg.view_with.clone());
                                    if open_with_picker(ui, "default_view", &mut v, false, &view_choices, &mut cfg.viewer) {
                                        browse = Some(Browse::Viewer);
                                    }
                                    cfg.view_with = v.unwrap_or_else(OpenWith::viewer_plugin);
                                    ui.end_row();

                                    ui.label(tr("Editor (F4)"));
                                    let mut e = Some(cfg.edit_with.clone());
                                    if open_with_picker(ui, "default_edit", &mut e, false, &edit_choices, &mut cfg.editor) {
                                        browse = Some(Browse::Editor);
                                    }
                                    cfg.edit_with = e.unwrap_or(OpenWith::Notepad);
                                    ui.end_row();
                                });
                                if edit_choices.is_empty() {
                                    ui.label(RichText::new(tr("No plugin registers as an editor.")).small().weak());
                                }
                                layout::divider(ui);
                                ui.label(RichText::new(tr("By file suffix")).strong());
                                ui.label(RichText::new(tr("Files ending with a suffix open with its own viewer or editor; Default uses the ones above.")).weak());
                                ui.add_space(layout::GAP_SMALL);
                                let (mut edit_at, mut remove_at) = (None, None);
                                if cfg.suffix_rules.is_empty() {
                                    ui.label(tr("No suffix has its own viewer or editor."));
                                } else {
                                    layout::table_grid("suffix_rules").num_columns(4).striped(true).show(ui, |ui| {
                                        ui.label(RichText::new(tr("Suffix")).strong());
                                        ui.label(RichText::new(tr("Viewer (F3)")).strong());
                                        ui.label(RichText::new(tr("Editor (F4)")).strong());
                                        ui.label("");
                                        ui.end_row();
                                        for (i, r) in cfg.suffix_rules.iter().enumerate() {
                                            ui.label(format!(".{}", r.suffix));
                                            ui.label(open_with_label(r.view.as_ref(), &view_choices));
                                            ui.label(open_with_label(r.edit.as_ref(), &edit_choices));
                                            ui.horizontal(|ui| {
                                                if ui.button(tr("Edit…")).clicked() {
                                                    edit_at = Some(i);
                                                }
                                                if ui.button(tr("Remove")).on_hover_text(tr("The suffix goes back to the default viewer and editor")).clicked() {
                                                    remove_at = Some(i);
                                                }
                                            });
                                            ui.end_row();
                                        }
                                    });
                                }
                                if let Some(i) = edit_at {
                                    *suffix_edit = Some(SuffixEdit::of(Some(i), cfg.suffix_rules[i].clone(), &cfg.viewer, &cfg.editor));
                                }
                                if let Some(i) = remove_at {
                                    cfg.suffix_rules.remove(i);
                                    // the form edits by position
                                    if suffix_edit.as_ref().is_some_and(|s| s.index.is_some()) {
                                        *suffix_edit = None;
                                    }
                                }
                                ui.add_space(layout::GAP);
                                let mut done = false;
                                match suffix_edit {
                                    None => {
                                        if ui.button(tr("Add a suffix…")).clicked() {
                                            *suffix_edit = Some(SuffixEdit::of(None, SuffixRule::default(), &cfg.viewer, &cfg.editor));
                                        }
                                    }
                                    Some(form) => {
                                        layout::group_frame(ui.style()).show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.label(RichText::new(if form.index.is_some() { tr("Edit the suffix") } else { tr("A new suffix") }).strong());
                                            layout::form_grid("suffix_form").show(ui, |ui| {
                                                ui.label(tr("Suffix"));
                                                ui.add(egui::TextEdit::singleline(&mut form.rule.suffix).hint_text("txt, tar.gz").desired_width(160.0));
                                                ui.end_row();
                                                ui.label(tr("Viewer (F3)"));
                                                if open_with_picker(ui, "rule_view", &mut form.rule.view, true, &view_choices, &mut form.view_program) {
                                                    browse = Some(Browse::RuleViewer);
                                                }
                                                ui.end_row();
                                                ui.label(tr("Editor (F4)"));
                                                if open_with_picker(ui, "rule_edit", &mut form.rule.edit, true, &edit_choices, &mut form.edit_program) {
                                                    browse = Some(Browse::RuleEditor);
                                                }
                                                ui.end_row();
                                            });
                                            if let Some(e) = &form.error {
                                                ui.colored_label(ui.visuals().error_fg_color, e.as_str());
                                            }
                                            ui.horizontal(|ui| {
                                                if ui.button(tr("Save")).clicked() {
                                                    match form.check(&cfg.suffix_rules) {
                                                        Ok(rule) => {
                                                            match form.index {
                                                                Some(i) if i < cfg.suffix_rules.len() => cfg.suffix_rules[i] = rule,
                                                                _ => cfg.suffix_rules.push(rule),
                                                            }
                                                            done = true;
                                                        }
                                                        Err(e) => form.error = Some(e),
                                                    }
                                                }
                                                if ui.button(tr("Cancel")).clicked() {
                                                    done = true;
                                                }
                                            });
                                        });
                                    }
                                }
                                if done {
                                    *suffix_edit = None;
                                }
                            });
                        }
                        SettingsTab::Plugins => {
                            ui.label(RichText::new(tr("Plugins add optional features. A plugin that is off changes nothing; turning one on or off applies at once.")).weak());
                            ui.label(RichText::new(trf("Plugins are found in {dir} when the program starts.", &[("dir", &plugins_dir)])).weak());
                            ui.add_space(layout::GAP);
                            if plugin_rows.is_empty() {
                                ui.label(tr("No plugins found."));
                            }
                            for k in &plugin_rows {
                                layout::group_frame(ui.style()).show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    let mut on = !cfg.disabled_plugins.contains(&k.id);
                                    if ui.checkbox(&mut on, RichText::new(k.name.as_str()).strong()).on_hover_text(k.file.as_str()).changed() {
                                        if on {
                                            cfg.disabled_plugins.retain(|d| *d != k.id);
                                        } else {
                                            cfg.disabled_plugins.push(k.id.clone());
                                        }
                                        plugins_changed = true;
                                    }
                                    ui.label(k.description.as_str());
                                    ui.label(RichText::new(k.version.as_str()).small().weak());
                                    if on {
                                        for o in &k.openers {
                                            ui.label(RichText::new(o.as_str()).small().weak());
                                        }
                                    }
                                    match &k.state {
                                        _ if !on => {}
                                        Ok(status) if !status.is_empty() => {
                                            ui.label(RichText::new(status.as_str()).small().weak());
                                        }
                                        Err(e) => {
                                            ui.colored_label(ui.visuals().warn_fg_color, RichText::new(e.as_str()).small());
                                        }
                                        _ => {}
                                    }
                                });
                                ui.add_space(layout::GAP);
                            }
                            for (file, why) in &broken_plugins {
                                ui.colored_label(ui.visuals().warn_fg_color, RichText::new(trf("{file} can't be used: {why}", &[("file", file), ("why", why)])).small());
                            }
                        }
                        SettingsTab::Keyboard => {
                            let conflicts = keymap::conflicts(&cfg.shortcuts);
                            let warn = ui.visuals().error_fg_color;
                            let hk = cfg.drive_hotkey;
                            let shadowed = keymap::shadowed_by_drive_letters(&cfg.shortcuts, |c, s, al| hk.matches(c, s, al));
                            // quick links to the sections below
                            let mut jump = None;
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(tr("Jump to:")).weak());
                                for section in Section::ALL {
                                    if ui.link(tr(section.label())).clicked() {
                                        jump = Some(section);
                                    }
                                }
                            });
                            let list_h = (main_h - 330.0).max(260.0);
                            egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
                                for section in Section::ALL {
                                    if jump == Some(section) {
                                        ui.scroll_to_cursor(Some(Align::TOP));
                                    }
                                    layout::group_frame(ui.style()).show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.label(RichText::new(tr(section.label())).size(ui.text_style_height(&egui::TextStyle::Body) * 1.15).strong());
                                        ui.label(RichText::new(tr(section.description())).small().weak());
                                        ui.separator();
                                        let bar = keymap::bar_actions(&cfg.shortcuts, cfg.fkey_bar.as_deref());
                                        layout::table_grid(("keymap", section.label())).striped(true).num_columns(3).min_col_width(200.0).show(ui, |ui| {
                                            ui.label(RichText::new(tr("Command")).small().weak())
                                                .on_hover_text(tr("Tick a command to give it a button on the function key bar, labelled with its first shortcut"));
                                            ui.label(RichText::new(tr("Preset")).small().weak());
                                            ui.label(RichText::new(tr("Shortcuts (click to remove)")).small().weak());
                                            ui.end_row();
                                            for a in section.actions() {
                                                let info = a.info();
                                                let cur = keymap::bindings(&cfg.shortcuts, a);
                                                let mut on = bar.contains(&a);
                                                if ui.checkbox(&mut on, tr(info.label)).on_hover_text(tr("Show on the function key bar")).changed() {
                                                    keymap::set_in_bar(&mut cfg.fkey_bar, a, on);
                                                }
                                                let preset = keymap::preset_label(&cfg.shortcuts, a);
                                                egui::ComboBox::from_id_salt(("preset", info.id)).width(210.0).truncate().selected_text(preset.clone()).show_ui(ui, |ui| {
                                                    for (label, set) in keymap::presets(a) {
                                                        if ui.selectable_label(cur == set, label).clicked() {
                                                            edit = Some(KeyEdit::Set(a, set));
                                                        }
                                                    }
                                                });
                                                ui.horizontal(|ui| {
                                                    for b in &cur {
                                                        let others: Vec<&str> =
                                                            conflicts.get(b).map(|v| v.iter().filter(|&&o| o != a).map(|o| tr(o.info().label)).collect()).unwrap_or_default();
                                                        let mut text = RichText::new(format!("{b} ×")).monospace();
                                                        if !others.is_empty() {
                                                            text = text.color(warn);
                                                        }
                                                        let tip = if others.is_empty() {
                                                            tr("Click to remove").to_owned()
                                                        } else {
                                                            format!("{}\n{}", trf("Also bound to: {list}", &[("list", &others.join(", "))]), tr("Click to remove"))
                                                        };
                                                        if ui.add(egui::Button::new(text).small()).on_hover_text(tip).clicked() {
                                                            edit = Some(KeyEdit::Remove(a, b.clone()));
                                                        }
                                                    }
                                                    if cur.is_empty() {
                                                        ui.weak(tr("none"));
                                                    }
                                                    if *recording == Some(a) {
                                                        ui.label(RichText::new(tr("press keys… (Esc cancels)")).italics());
                                                    } else if ui.small_button(tr("+ Add…")).on_hover_text(tr("Record a custom shortcut")).clicked() {
                                                        edit = Some(KeyEdit::Record(a));
                                                    }
                                                });
                                                ui.end_row();
                                            }
                                            if section == Section::Navigation {
                                                ui.label(tr("Go to drive (A–Z)"));
                                                egui::ComboBox::from_id_salt("drive_hotkey").width(210.0).truncate().selected_text(tr(cfg.drive_hotkey.label())).show_ui(
                                                    ui,
                                                    |ui| {
                                                        for h in DriveHotkey::ALL {
                                                            ui.selectable_value(&mut cfg.drive_hotkey, h, tr(h.label()));
                                                        }
                                                    },
                                                );
                                                ui.vertical(|ui| {
                                                    for (a, b) in &shadowed {
                                                        ui.colored_label(warn, trf("{key} ({action}) wins over this letter", &[("key", b), ("action", &tr(a.info().label))]));
                                                    }
                                                    if hk == DriveHotkey::Shift {
                                                        ui.small(tr("Still types into a quick search in progress,\nand when no drive has that letter."));
                                                    }
                                                });
                                                ui.end_row();
                                            }
                                        });
                                        let fixed: Vec<String> = FIXED_KEYS.iter().filter(|(sec, ..)| *sec == section).map(|(_, k, d)| format!("{} — {}", tr(k), tr(d))).collect();
                                        if !fixed.is_empty() {
                                            ui.add_space(layout::GAP_SMALL);
                                            ui.label(RichText::new(trf("Fixed: {keys}", &[("keys", &fixed.join(" · "))])).small().weak());
                                        }
                                    });
                                    ui.add_space(layout::GAP);
                                }
                            });
                            ui.add_space(layout::GAP);
                            ui.horizontal(|ui| {
                                if ui.button(tr("Reset all shortcuts to defaults")).clicked() {
                                    edit = Some(KeyEdit::ResetAll);
                                }
                                if ui.add_enabled(cfg.fkey_bar.is_some(), egui::Button::new(tr("Reset function key bar"))).clicked() {
                                    cfg.fkey_bar = None;
                                }
                            });
                            ui.small(tr("Configured shortcuts take precedence over fixed keys. Ticked commands have a button on the function key bar."));
                        }
                    }
                    layout::before_buttons(ui);
                    ui.small(tr("Settings apply immediately and are saved on exit."));
                    if layout::buttons(ui, &[(tr("Close"), "")], Some(0)).is_some() {
                        outcome = 2;
                    }
                });
                if captured {
                    enter = false;
                    escape = false;
                }
                self.font_slider_rect = slider_rect;
                self.font_button_rect = font_button_rect;
                if reload_panels {
                    self.reload_all();
                }
                if let Some(target) = browse {
                    self.browse_program(target);
                }
                if lang_changed {
                    crate::i18n::set_language(&self.cfg.language);
                }
                if plugins_changed {
                    self.apply_plugins();
                }
                if check_now {
                    self.update_check = Some(crate::update::Check::start(ctx, true, update_sources(&self.plugins), update_offer(&self.cfg, true)));
                }
                if fonts_changed {
                    crate::fonts::install(ctx, &self.cfg.ui_font, &self.cfg.mono_font);
                    apply_font_size(ctx, self.cfg.font_size);
                }
                match edit {
                    Some(KeyEdit::Set(a, list)) => keymap::set_bindings(&mut self.cfg.shortcuts, a, list),
                    Some(KeyEdit::Remove(a, b)) => keymap::remove_binding(&mut self.cfg.shortcuts, a, &b),
                    Some(KeyEdit::Record(a)) => *recording = Some(a),
                    Some(KeyEdit::ResetAll) => {
                        self.cfg.shortcuts.clear();
                        *recording = None;
                    }
                    None => {}
                }
                // Enter/Esc while the shortcut table is active must not close it by accident
                if *tab == SettingsTab::Keyboard {
                    enter = false;
                }
            }
            Dialog::Help => {
                dw.show(ctx, &mut self.dialog_places, "help", tr("Keyboard"), 600.0, &mut enter, &mut escape, &mut |ui| {
                    egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
                        for section in Section::ALL {
                            ui.label(RichText::new(tr(section.label())).strong());
                            layout::table_grid(("help", section.label())).striped(true).num_columns(2).min_col_width(200.0).show(ui, |ui| {
                                let mut row = |k: &str, d: &str| {
                                    ui.label(RichText::new(k).monospace().strong());
                                    ui.label(d);
                                    ui.end_row();
                                };
                                for a in section.actions() {
                                    let b = keymap::bindings(&self.cfg.shortcuts, a);
                                    if !b.is_empty() {
                                        row(&b.join(", "), tr(a.info().label));
                                    }
                                }
                                if section == Section::Navigation && self.cfg.drive_hotkey != DriveHotkey::Disabled {
                                    row(&format!("{} (A..Z)", tr(self.cfg.drive_hotkey.label())), tr("Go to drive"));
                                }
                                for (_, k, d) in FIXED_KEYS.iter().filter(|(sec, ..)| *sec == section) {
                                    row(tr(k), tr(d));
                                }
                            });
                            ui.add_space(layout::GAP);
                        }
                    });
                    layout::before_buttons(ui);
                    ui.small(tr("Shortcuts are configured in Options → Settings… → Keyboard shortcuts."));
                    if layout::buttons(ui, &[(tr("Close"), "")], Some(0)).is_some() {
                        outcome = 2;
                    }
                });
                if enter {
                    outcome = 2;
                }
            }
        }
        if escape {
            outcome = 2;
        }
        // the drive selector's Disconnect (button, or Delete on a connection)
        if let Dialog::Drive { drives, conns, cursor, .. } = &dialog {
            let on_conn = cursor.checked_sub(drives.len()).and_then(|k| conns.get(k));
            if drive_disconnect.is_none() && ctx.input(|i| i.key_pressed(Key::Delete)) {
                drive_disconnect = on_conn.map(|c| c.root.clone());
            }
        }
        if let Some((id, root, command)) = drive_refresh {
            if let Some(plugin) = self.plugins.get(&id).cloned() {
                self.refresh_results(ctx, plugin, &root, command);
            }
            outcome = 0;
        }
        if let Some(root) = drive_disconnect {
            if let Dialog::Drive { drives, conns, paths, cursor, .. } = &mut dialog {
                conns.retain(|c| c.root != root);
                // the last row is the other panel's folder
                *cursor = (*cursor).min(drives.len() + conns.len() + paths.len());
            }
            self.close_connection(&root);
            outcome = 0;
        }
        // a plugin form's Browse… button: the Windows "Open" dialog
        if let (Some(id), Dialog::PluginForm { values, form, .. }) = (browse_field, &mut dialog) {
            let current = values.get(&id).map(PathBuf::from);
            let folder = form.fields.iter().any(|f| f.id == id && f.kind == plugins::FieldKind::Folder);
            let picked = if folder {
                shellmenu::pick_folder(self.hwnd, tr("Choose a folder"), current.as_deref())
            } else {
                shellmenu::pick_file(self.hwnd, tr("Choose a file"), current.as_deref())
            };
            if let Some(path) = picked {
                values.insert(id, path.to_string_lossy().into_owned());
            }
        }
        if enter && outcome == 0 {
            outcome = 1;
        }
        match outcome {
            0 => self.dialog = dialog,
            2 => {}
            3 | 4 => {
                self.queue_place = if outcome == 3 { Placement::First } else { Placement::Last };
                self.dialog_ok(ctx, dialog);
                self.queue_place = Placement::Now;
            }
            _ => self.dialog_ok(ctx, dialog),
        }
    }

    /// OK (Enter) in `dialog`: do what it asked for.
    fn dialog_ok(&mut self, ctx: &egui::Context, dialog: Dialog) {
        crate::debuglog::did(format_args!("confirmed: {}", dialog.summary()));
        let hidden = self.vis();
        match dialog {
            Dialog::PluginForm { plugin, form, values } => match self.plugins.get(&plugin).cloned() {
                Some(p) => self.run_plugin_command(ctx, p, form.command, values),
                None => self.error(tr("The plugin for this file is not enabled.")),
            },
            Dialog::Transfer { kind, sources, target, opts } => {
                let base = self.panels[self.active].path().to_path_buf();
                let t = location::parse_url(target.trim()).unwrap_or_else(|| PathBuf::from(target.trim()));
                let mut target = if t.is_absolute() { t } else { base.join(t) };
                if target.as_os_str().is_empty() {
                    target = base;
                }
                // a file as a folder ("a.zip\" or "a.zip\dir") without a plugin
                // that opens it: no folder can be made there
                if self.archive_folder(&target).is_none() {
                    let as_folder = target.as_os_str().to_string_lossy().ends_with(['\\', '/']);
                    let clean: PathBuf = target.components().collect();
                    if let Some(file) = clean.ancestors().skip(usize::from(!as_folder)).find(|a| a.is_file()) {
                        self.error(trf("{path} is a file, not a folder.", &[("path", &display_path(file))]));
                        return;
                    }
                }
                let checksum_name = if opts.verify == Verify::Checksum { chrono::Local::now().format("RustySalamander %Y-%m-%d %H-%M-%S.sha256").to_string() } else { String::new() };
                // a safe copy always tells how it went
                self.job_options = JobOptions { verify: opts.verify, checksum_name, report: opts.reports() };
                self.start_job(ctx, JobKind::Transfer(kind), sources, target);
                // its progress window goes away at once (or comes back for one that shows it)
                self.jobs.hidden = opts.background && !self.jobs.is_idle();
            }
            Dialog::MkDir { name } => {
                // "/a/b/c" or "a\\b\\c": nested folders inside the current one
                let name = name.trim().trim_matches(['\\', '/']).to_owned();
                let dir = self.act().path().to_path_buf();
                if self.act().archive.is_some() {
                    if let Err(e) = fileops::validate_name(&name, true) {
                        self.error(trf("Cannot create \"{name}\": {error}", &[("name", &name), ("error", &e)]));
                    } else {
                        let path = dir.join(name.replace('/', "\\"));
                        self.start_job(ctx, JobKind::Archive(FolderOp::MkDir), vec![path], PathBuf::new());
                    }
                    return;
                }
                match fileops::make_dir(&dir, &name) {
                    Ok(_) => {
                        let first = name.split(['\\', '/']).next().unwrap_or(&name).to_owned();
                        let _ = self.act().load(&dir, hidden, Some(&first));
                    }
                    Err(e) => {
                        self.error(trf("Cannot create \"{name}\": {error}", &[("name", &name), ("error", &e)]));
                    }
                }
            }
            Dialog::Rename { path, name } => {
                let name = name.trim().to_owned();
                if self.in_archive(&path).is_some() {
                    match fileops::validate_name(&name, false) {
                        Ok(()) => {
                            let new = path.with_file_name(&name);
                            self.start_job(ctx, JobKind::Archive(FolderOp::Rename), vec![path], new);
                        }
                        Err(e) => self.error(trf("Cannot rename: {error}", &[("error", &e)])),
                    }
                    return;
                }
                match fileops::rename(&path, &name) {
                    Ok(_) => {
                        let dir = self.act().path().to_path_buf();
                        let _ = self.act().load(&dir, hidden, Some(&name));
                    }
                    Err(e) => self.error(trf("Cannot rename: {error}", &[("error", &e)])),
                }
            }
            Dialog::Delete { sources, recycle } => {
                let kind = if recycle { JobKind::Recycle } else { JobKind::Delete };
                self.start_job(ctx, kind, sources, PathBuf::new());
            }
            Dialog::SelectMask { select, mask, .. } => match MaskGroup::new(&mask, false) {
                Ok(g) => {
                    self.last_mask = mask;
                    self.act().select_by_mask(&g, select);
                }
                Err(e) => {
                    self.open_dialog(Dialog::SelectMask { select, mask, error: Some(e.to_string()) });
                }
            },
            Dialog::Drive { panel, drives, conns, paths, cursor } => {
                let conn = conns.get(cursor.wrapping_sub(drives.len()));
                let other = cursor == drives.len() + conns.len() + paths.len();
                let res = match (drives.get(cursor), paths.get(cursor.wrapping_sub(drives.len() + conns.len()))) {
                    _ if other => self.go_to_other_panel(panel),
                    (Some(d), _) => self.change_drive(panel, &d.root),
                    // a connection: where it started, or its root
                    (None, _) if conn.is_some() => {
                        let c = conn.unwrap();
                        self.panels[panel].load(&c.start, hidden, None).or_else(|_| self.panels[panel].load(&c.root, hidden, None))
                    }
                    (None, Some(p)) => {
                        let p = PathBuf::from(p);
                        let r = self.panels[panel].load(&p, hidden, None);
                        if r.is_ok() {
                            self.remember_path(&p);
                        }
                        r
                    }
                    _ => Ok(()),
                };
                if let Err(e) = res {
                    self.error(e);
                }
            }
            Dialog::Message { .. } | Dialog::Help | Dialog::Update { .. } | Dialog::Settings { .. } | Dialog::None => {}
        }
    }

    // ---------------------------------------------------------------- job window

    /// The windows of running operations (progress, questions) and of the
    /// queue; finished operations are taken care of.
    fn job_ui(&mut self, ctx: &egui::Context) {
        for job in self.jobs.poll(ctx) {
            self.job_finished(job);
        }
        self.report_ui(ctx);
        if self.jobs.is_idle() {
            dialogwin::close(&mut self.dialog_places, self.hwnd, |k| k == "job" || k == "prompt");
            self.jobs.shown_for_prompt = false;
            return;
        }
        // keep the elapsed time and the estimate moving
        ctx.request_repaint_after(Duration::from_millis(500));
        self.prompt_ui(ctx);
        if self.jobs.hidden {
            dialogwin::close(&mut self.dialog_places, self.hwnd, |k| k == "job");
            return;
        }
        let now = Instant::now();
        let dw = DialogWin { owner: self.hwnd, theme: self.cfg.theme, modal: false, exclusive: false, icon: None, animations: animated(self.cfg.animations) };
        let (mut enter, mut escape) = (false, false);
        let mut act: Option<JobAct> = None;
        let jobs = &self.jobs;
        // a fixed title: the window is found by it to make it owned by the main one
        let title = tr("File operations");
        dw.show(ctx, &mut self.dialog_places, "job", title, 580.0, &mut enter, &mut escape, &mut |ui| {
            for (n, job) in jobs.running.iter().enumerate() {
                if n > 0 {
                    layout::divider(ui);
                }
                job_progress_ui(ui, job, now, &mut act);
            }
            if !jobs.queue.is_empty() {
                layout::divider(ui);
                ui.label(RichText::new(trf("Queue ({n})", &[("n", &jobs.queue.len())])).strong());
                ui.label(RichText::new(tr("Waiting operations start one by one when the running ones finish.")).small().weak());
                ui.add_space(layout::GAP_SMALL);
                let row_h = ui.spacing().interact_size.y;
                for (i, spec) in jobs.queue.iter().enumerate() {
                    // one line per operation: buttons on the right, the description cut to fit
                    ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), row_h), Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text(tr("Remove from the queue")).clicked() {
                            act = Some(JobAct::Remove(spec.id));
                        }
                        if ui.small_button(tr("Start now")).on_hover_text(tr("Run it now, alongside the running operations")).clicked() {
                            act = Some(JobAct::StartNow(spec.id));
                        }
                        if ui.add_enabled(i + 1 < jobs.queue.len(), egui::Button::new(sal_ui::mdi::CHEVRON_DOWN).small()).on_hover_text(tr("Move down")).clicked() {
                            act = Some(JobAct::Move(spec.id, false));
                        }
                        let up_tip = if i == 0 { tr("To the top: run it now; the running operations pause until it finishes") } else { tr("Move up") };
                        if ui.small_button(sal_ui::mdi::CHEVRON_UP).on_hover_text(up_tip).clicked() {
                            act = Some(JobAct::Move(spec.id, true));
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.add(egui::Label::new(format!("{}. {}", i + 1, spec.describe())).truncate());
                        });
                    });
                }
            }
            // controls for everything, on a strip like the panels' status bar
            layout::before_buttons(ui);
            let all_paused = jobs.running.iter().filter(|j| j.spec.kind.can_pause()).all(|j| j.is_paused());
            layout::strip_frame(strip_color(ui.visuals())).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let buttons = [
                    if all_paused { (tr("Resume all"), tr("Continue all paused operations")) } else { (tr("Pause all"), tr("Pause all running operations")) },
                    (tr("Cancel all"), tr("Stop the running operations and empty the queue")),
                    (tr("Run in background"), tr("Hide this window; the progress stays in the status bar")),
                ];
                // no default: the window asks nothing, and Enter isn't for it
                match layout::buttons(ui, &buttons, None) {
                    Some(0) => act = Some(JobAct::PauseAll(!all_paused)),
                    Some(1) => act = Some(JobAct::CancelAll),
                    Some(_) => act = Some(JobAct::Hide),
                    None => {}
                }
            });
        });
        // Esc or closing the window hides it; the operations go on
        if escape {
            act = Some(JobAct::Hide);
        }
        match act {
            Some(JobAct::Hide) => {
                self.jobs.hidden = true;
                self.jobs.shown_for_prompt = false;
            }
            Some(JobAct::PauseAll(paused)) => self.jobs.pause_all(paused),
            Some(JobAct::CancelAll) => self.jobs.cancel_all(),
            Some(JobAct::Pause(id, paused)) => {
                if let Some(j) = self.jobs.running.iter_mut().find(|j| j.spec.id == id) {
                    j.set_paused(paused);
                }
            }
            Some(JobAct::Cancel(id)) => {
                if let Some(j) = self.jobs.running.iter_mut().find(|j| j.spec.id == id) {
                    j.cancel();
                }
            }
            Some(JobAct::Move(id, up)) => self.jobs.move_queued(ctx, id, up),
            Some(JobAct::StartNow(id)) => self.jobs.start_now(ctx, id),
            Some(JobAct::Remove(id)) => self.jobs.remove_queued(id),
            None => {}
        }
    }

    /// The button Enter presses when the target exists: Skip.
const CONFLICT_DEFAULT: usize = 2;

/// The question of an operation that hit a conflict or an error, in its own window.
    fn prompt_ui(&mut self, ctx: &egui::Context) {
        let dw = DialogWin { owner: self.hwnd, theme: self.cfg.theme, modal: false, exclusive: true, icon: None, animations: animated(self.cfg.animations) };
        let Some(job) = self.jobs.running.iter_mut().find(|j| j.prompt.is_some()) else {
            dialogwin::close(&mut self.dialog_places, self.hwnd, |k| k == "prompt");
            // answered: back to the background it was in
            if std::mem::take(&mut self.jobs.shown_for_prompt) {
                self.jobs.hidden = !self.jobs.is_idle();
            }
            return;
        };
        // an operation in the background that needs an answer comes forward:
        // the question takes every window's input (the status bar's too), so
        // its progress is shown beside it, and goes back once answered
        if self.jobs.hidden {
            self.jobs.hidden = false;
            self.jobs.shown_for_prompt = true;
        }
        let (mut enter, mut escape) = (false, false);
        let mut reply = None;
        let op = job.spec.describe();
        match job.prompt.as_ref().unwrap() {
            Prompt::Conflict { src, dst } => {
                dw.show(ctx, &mut self.dialog_places, "prompt", tr("Target already exists"), 660.0, &mut enter, &mut escape, &mut |ui| {
                    ui.label(RichText::new(op.as_str()).small().weak());
                    ui.label(RichText::new(tr("An item with the same name is already in the target folder.")).strong());
                    ui.add_space(layout::GAP);
                    conflict_table(ui, src, dst);
                    layout::before_buttons(ui);
                    let actions = [ConflictAction::Overwrite, ConflictAction::OverwriteAll, ConflictAction::Skip, ConflictAction::SkipAll, ConflictAction::Cancel];
                    let labels = [
                        (tr("Overwrite"), ""),
                        (tr("Overwrite All"), tr("Overwrite this and all further existing items")),
                        (tr("Skip"), ""),
                        (tr("Skip All"), tr("Skip this and all further existing items")),
                        (tr("Cancel"), tr("Stop the whole operation")),
                    ];
                    // Enter skips: the safe answer, nothing is overwritten
                    if let Some(i) = layout::buttons(ui, &labels, Some(Self::CONFLICT_DEFAULT)) {
                        reply = Some(Reply::Conflict(actions[i]));
                    }
                });
                if enter && reply.is_none() {
                    reply = Some(Reply::Conflict(ConflictAction::Skip));
                }
                if escape && reply.is_none() {
                    reply = Some(Reply::Conflict(ConflictAction::Cancel));
                }
            }
            Prompt::Error { path, message } => {
                dw.show(ctx, &mut self.dialog_places, "prompt", tr("Error"), 560.0, &mut enter, &mut escape, &mut |ui| {
                    ui.label(RichText::new(op.as_str()).small().weak());
                    ui.add_space(layout::GAP);
                    layout::table_grid("error").num_columns(2).show(ui, |ui| {
                        ui.label(RichText::new(tr("Item")).weak());
                        ui.add(egui::Label::new(display_path(path)).wrap());
                        ui.end_row();
                        ui.label(RichText::new(tr("Problem")).weak());
                        ui.colored_label(ui.visuals().error_fg_color, message.as_str());
                        ui.end_row();
                    });
                    layout::before_buttons(ui);
                    let actions = [ErrorAction::Retry, ErrorAction::Skip, ErrorAction::SkipAll, ErrorAction::Cancel];
                    let labels =
                        [(tr("Retry"), ""), (tr("Skip"), ""), (tr("Skip All"), tr("Skip this and all further items that fail")), (tr("Cancel"), tr("Stop the whole operation"))];
                    // Enter tries again
                    if let Some(i) = layout::buttons(ui, &labels, Some(0)) {
                        reply = Some(Reply::Error(actions[i]));
                    }
                });
                if enter && reply.is_none() {
                    reply = Some(Reply::Error(ErrorAction::Retry));
                }
                if escape && reply.is_none() {
                    reply = Some(Reply::Error(ErrorAction::Cancel));
                }
            }
        }
        if let Some(r) = reply {
            job.answer(r);
        }
    }

    /// An operation ended: report how, open an extracted file, read the
    /// panels again.
    fn job_finished(&mut self, job: Job) {
        crate::debuglog::job_finished(job.spec.id, format_args!("{:?}", job.result));
        if let (JobKind::Archive(FolderOp::Open(how)), Some(Ok(s))) = (job.spec.kind, &job.result) {
            if !s.cancelled {
                if let Err(e) = self.open_extracted(&job.spec.sources[0], &job.spec.target, how) {
                    self.error(e);
                }
            }
            return;
        }
        self.dir_sizes.clear();
        self.free.forget();
        let title = job.spec.kind.title();
        let elapsed = job.rate.elapsed(Instant::now());
        let result = job.result.unwrap();
        let done = match &result {
            Ok(s) => {
                let mut m = format!("{title}: {} ({})", plural(s.files as usize, "file"), format_size(s.bytes));
                if s.skipped > 0 {
                    m.push_str(&trf(", {n} skipped", &[("n", &s.skipped)]));
                }
                if s.cancelled {
                    m.push_str(tr(" — cancelled"));
                } else {
                    // the panels may show other things by now: unselect just what was processed
                    for p in &mut self.panels {
                        p.unselect(&job.spec.sources);
                    }
                }
                m
            }
            Err(e) => {
                // a result window says so itself
                if !job.spec.options.report {
                    self.error(trf("{job} failed: {error}", &[("job", &title), ("error", &e)]));
                }
                String::new()
            }
        };
        self.job_done = (!done.is_empty()).then(|| (done, Instant::now()));
        if job.spec.options.report {
            self.reports.push(Report { spec: job.spec, result, elapsed, ended: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string() });
        }
        self.reload_all();
    }

    /// The window with the results of the operations that asked for one
    /// (in the background, or a safe copy): how each went, what it did,
    /// how its copies were checked and what went wrong.
    fn report_ui(&mut self, ctx: &egui::Context) {
        if self.reports.is_empty() {
            dialogwin::close(&mut self.dialog_places, self.hwnd, |k| k == "report");
            return;
        }
        // modal: the main window waits until the result has been read
        let dw = DialogWin { owner: self.hwnd, theme: self.cfg.theme, modal: true, exclusive: false, icon: None, animations: animated(self.cfg.animations) };
        let (mut enter, mut escape) = (false, false);
        let mut close = false;
        let mut remove = false;
        let mut save = false;
        let reports = &self.reports;
        let saved = &self.protocol_saved;
        let has_sums = reports.iter().any(|r| r.result.as_ref().is_ok_and(|s| !s.checksum_files.is_empty()));
        // the list's room from the main window: inside the dialog its own
        // window is the screen, which the dialog is sized to (it would shrink)
        let max_h = (ctx.screen_rect().height() * 0.7).max(240.0);
        // a fixed title: the window is found by it to make it owned by the main one
        dw.show(ctx, &mut self.dialog_places, "report", tr("Operation result"), 600.0, &mut enter, &mut escape, &mut |ui| {
            egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                // the newest first
                for (n, r) in reports.iter().rev().enumerate() {
                    if n > 0 {
                        layout::divider(ui);
                    }
                    report_body(ui, r);
                }
            });
            layout::before_buttons(ui);
            if let Some(p) = saved {
                ui.label(RichText::new(trf("Protocol saved to {path}", &[("path", &display_path(p))])).small().weak());
            }
            let mut buttons = vec![(tr("Close"), ""), (tr("Save protocol…"), tr("Save what was done, how it was checked, every problem and the checksum of every file to a text file"))];
            if has_sums {
                buttons.push((tr("Close and remove checksum files"), tr("Delete the .sha256 files listed above, in the target and in the source folders")));
            }
            match layout::buttons(ui, &buttons, Some(0)) {
                Some(1) => save = true,
                Some(2) => remove = true,
                Some(_) => close = true,
                None => {}
            }
        });
        if save {
            self.save_protocol();
        }
        if remove {
            self.remove_checksum_files();
        }
        if close || remove || enter || escape {
            self.reports.clear();
            self.protocol_saved = None;
        }
    }

    /// Ask where, and save the protocol of the operations in the result
    /// window there. It starts next to the newest one's target.
    fn save_protocol(&mut self) {
        let Some(newest) = self.reports.last() else { return };
        let name = chrono::Local::now().format("RustySalamander %Y-%m-%d %H-%M-%S protocol.txt").to_string();
        let owner = dialogwin::hwnd(&self.dialog_places, "report").or(self.hwnd);
        let Some(path) = shellmenu::pick_save(owner, tr("Save protocol"), Some(&newest.spec.target), &name) else { return };
        self.write_protocol(&path);
    }

    /// Write the protocol of the operations in the result window to `path`.
    fn write_protocol(&mut self, path: &Path) {
        // UTF-8 with a BOM: older editors read it as UTF-8 too
        let text = format!("\u{feff}{}", report_protocol(&self.reports));
        match std::fs::write(path, text) {
            Ok(()) => {
                self.protocol_saved = Some(path.to_path_buf());
                self.reload_all();
            }
            Err(e) => self.error(trf("Cannot save the protocol to {path}: {err}", &[("path", &display_path(path)), ("err", &e)])),
        }
    }

    /// Delete the checksum files the operations in the result window wrote.
    fn remove_checksum_files(&mut self) {
        let mut failed = Vec::new();
        for path in self.reports.iter().filter_map(|r| r.result.as_ref().ok()).flat_map(|s| &s.checksum_files) {
            if let Err(e) = std::fs::remove_file(path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    failed.push(format!("{}: {e}", display_path(path)));
                }
            }
        }
        if !failed.is_empty() {
            self.error(trf("Cannot remove the checksum files:\n{files}", &[("files", &failed.join("\n"))]));
        }
        self.reload_all();
    }

    /// Open a file extracted from an archive (`target` holds it under its
    /// path inside the archive). Changes to it don't go back into the archive.
    fn open_extracted(&mut self, virtual_path: &Path, target: &Path, how: OpenAs) -> Result<(), String> {
        let (_, _, inner) = self.in_archive(virtual_path).ok_or_else(|| display_path(virtual_path))?;
        let file = target.join(&inner);
        let err = |e: std::io::Error| format!("{}: {e}", display_path(&file));
        match how {
            OpenAs::Default => shell_open(&file).map_err(err)?,
            OpenAs::View => self.view_file(&file, true)?,
            OpenAs::Edit => {
                self.edit_file(&file, true)?;
                self.status = tr("A temporary copy is open; changes to it are not saved back.").into();
            }
        }
        Ok(())
    }

    /// The file operations in the status bar: "No operations running" (or,
    /// for a moment, what the last one did); else how many copy, move and
    /// delete, their progress and the time left. A click shows their window.
    fn jobs_indicator(&mut self, ui: &mut egui::Ui) {
        if self.jobs.running.is_empty() && self.jobs.queue.is_empty() {
            match self.job_done.as_ref().filter(|(_, at)| at.elapsed() < JOB_DONE_SHOWN) {
                Some((done, at)) => {
                    let strong = ui.visuals().strong_text_color();
                    ui.add(egui::Label::new(RichText::new(done).color(strong)).truncate());
                    ui.ctx().request_repaint_after(JOB_DONE_SHOWN.saturating_sub(at.elapsed()));
                }
                None => {
                    self.job_done = None;
                    ui.label(RichText::new(tr("No operations running")).weak());
                }
            }
            return;
        }
        // how many of each kind, running or waiting, in the order they come
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for kind in self.jobs.running.iter().map(|j| j.spec.kind).chain(self.jobs.queue.iter().map(|s| s.kind)) {
            match counts.iter_mut().find(|(g, _)| *g == kind.group()) {
                Some((_, n)) => *n += 1,
                None => counts.push((kind.group(), 1)),
            }
        }
        let mut text = counts.iter().map(|(g, n)| if *n > 1 { format!("{g} {n}") } else { (*g).to_owned() }).collect::<Vec<_>>().join(" · ");
        let (frac, left) = self.jobs.summary();
        if !self.jobs.running.is_empty() {
            text.push_str(&format!(" · {:.0} %", frac * 100.0));
        }
        if self.jobs.running.is_empty() {
            // waiting only: nothing to time
        } else if self.jobs.running.iter().all(|j| j.is_paused()) {
            text.push_str(&format!(" · {}", tr("Paused")));
        } else if let Some(l) = left {
            text.push_str(&format!(" · {}", trf("about {time} left", &[("time", &format_duration(l))])));
        }
        if !self.jobs.queue.is_empty() {
            text.push_str(&format!(" · {}", trn("{n} waiting", "{n} waiting", self.jobs.queue.len() as u64)));
        }
        let clicked = ui
            .horizontal(|ui| {
                let bar = ui.add(progress_bar(frac).desired_width(140.0).desired_height(ui.text_style_height(&egui::TextStyle::Small)));
                let label = ui.add(egui::Label::new(text).sense(Sense::click()));
                bar.union(label).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tr("Click to show the file operations")).clicked()
            })
            .inner;
        if clicked {
            self.jobs.hidden = false;
            self.jobs.shown_for_prompt = false;
            dialogwin::bring_to_front(&self.dialog_places, "job");
        }
    }

    // ---------------------------------------------------------------- chrome

    /// The menu bar.
    fn menu_bar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let mut cmd = None;
        let mut toggle_hidden = false;
        let mut toggle_system = false;
        let ov = self.cfg.shortcuts.clone();
        let current_theme = self.cfg.theme;
        let mut theme_pick = None;
        let mut always_bars = self.cfg.always_show_scrollbars;
        let mut animations = self.cfg.animations;
        let mut fkey_bar = self.cfg.show_fkey_bar;
        let mut sys_icons = self.cfg.system_icons;
        let mut layout_cmd = None;
        let mut lang_pick: Option<String> = None;
        // the Plugins menu: each plugin's commands under its name
        let plugin_menu: Vec<(String, Arc<dyn FolderPlugin>, Vec<plugins::Command>)> =
            self.plugins.active.iter().map(|p| (self.plugins.name_of(p.id()), p.clone(), p.commands())).filter(|(_, _, c)| !c.is_empty()).collect();
        let connections = plugins::connections();
        let busy = self.plugin_task.is_some();
        let mut plugin_pick: Option<(Arc<dyn FolderPlugin>, String)> = None;
        let mut disconnect: Option<PathBuf> = None;
        let mut plugin_settings = false;
        egui::menu::bar(ui, |ui| {
            // `key` is for fixed keys; configurable commands show their current binding
            let mut item = |ui: &mut egui::Ui, label: &str, key: &str, c: Cmd| {
                let shown = if key.is_empty() { menu_key(&ov, c) } else { key.to_owned() };
                if ui.add(egui::Button::new(label).shortcut_text(shown)).clicked() {
                    cmd = Some(c);
                    ui.close_menu();
                }
            };
            ui.menu_button(tr("Files"), |ui| {
                item(ui, tr("View"), "", Cmd::View);
                item(ui, tr("Edit"), "", Cmd::Edit);
                item(ui, tr("Copy…"), "", Cmd::Copy);
                item(ui, tr("Move…"), "", Cmd::Move);
                item(ui, tr("Rename…"), "", Cmd::Rename);
                item(ui, tr("Delete…"), "", Cmd::Delete { permanent: false });
                item(ui, tr("Copy full path"), "", Cmd::CopyPaths);
                ui.separator();
                item(ui, tr("Select…"), "+", Cmd::SelectMask(true));
                item(ui, tr("Unselect…"), "-", Cmd::SelectMask(false));
                item(ui, tr("Select all"), "", Cmd::SelectAll);
                item(ui, tr("Unselect all"), "", Cmd::UnselectAll);
                item(ui, tr("Invert selection"), "*", Cmd::Invert);
                item(ui, tr("Calculate selection size"), "", Cmd::CalcSelectionSize);
                ui.separator();
                item(ui, tr("Exit"), "", Cmd::Exit);
            });
            ui.menu_button(tr("Directory"), |ui| {
                item(ui, tr("Create directory…"), "", Cmd::MkDir);
                item(ui, tr("Change directory…"), "", Cmd::ChangeDir);
                item(ui, tr("Parent directory"), "", Cmd::Parent);
                item(ui, tr("Root directory"), "", Cmd::Root);
                item(ui, tr("Back"), "", Cmd::History(true));
                item(ui, tr("Forward"), "", Cmd::History(false));
                ui.separator();
                item(ui, tr("Left drive…"), "", Cmd::DriveMenu(0));
                item(ui, tr("Right drive…"), "", Cmd::DriveMenu(1));
            });
            ui.menu_button(tr("Commands"), |ui| {
                item(ui, tr("Open command prompt here"), "", Cmd::Terminal);
                item(ui, tr("Swap panels"), "", Cmd::SwapPanels);
                item(ui, tr("Refresh"), "", Cmd::Refresh);
            });
            ui.menu_button(tr("View"), |ui| {
                item(ui, tr("Sort by name"), "", Cmd::Sort(SortKey::Name));
                item(ui, tr("Sort by extension"), "", Cmd::Sort(SortKey::Ext));
                item(ui, tr("Sort by time"), "", Cmd::Sort(SortKey::Time));
                item(ui, tr("Sort by size"), "", Cmd::Sort(SortKey::Size));
                ui.separator();
                ui.label(egui::RichText::new(tr("Theme")).small().weak());
                for (mode, label) in THEMES {
                    if ui.radio(current_theme == mode, tr(label)).clicked() {
                        theme_pick = Some(mode);
                        ui.close_menu();
                    }
                }
                ui.separator();
                // checkbox label with the shortcut, like menu items
                let with_key = |label: &'static str, c: Cmd| match menu_key(&ov, c) {
                    k if k.is_empty() => tr(label).to_owned(),
                    k => format!("{}    ({k})", tr(label)),
                };
                let mut hidden = self.cfg.show_hidden;
                if ui.checkbox(&mut hidden, with_key(LBL_HIDDEN, Cmd::ToggleHidden)).changed() {
                    toggle_hidden = true;
                    ui.close_menu();
                }
                let mut system = self.cfg.show_system;
                if ui.checkbox(&mut system, with_key(LBL_SYSTEM, Cmd::ToggleSystem)).changed() {
                    toggle_system = true;
                    ui.close_menu();
                }
                if ui.checkbox(&mut sys_icons, tr(LBL_ICONS)).changed() {
                    layout_cmd = Some(Cmd::ToggleIcons);
                    ui.close_menu();
                }
                ui.separator();
                if ui.checkbox(&mut always_bars, tr(LBL_SCROLLBARS)).changed() {
                    layout_cmd = Some(Cmd::ToggleScrollbars);
                    ui.close_menu();
                }
                if crate::gdiwin::chosen() {
                    animations_off(ui);
                } else if ui.checkbox(&mut animations, tr(LBL_ANIMATIONS)).on_hover_text(tr(TIP_ANIMATIONS)).changed() {
                    layout_cmd = Some(Cmd::ToggleAnimations);
                    ui.close_menu();
                }
                if ui.checkbox(&mut fkey_bar, tr(LBL_FKEYS)).changed() {
                    layout_cmd = Some(Cmd::ToggleFkeyBar);
                    ui.close_menu();
                }
            });
            ui.menu_button(tr("Plugins"), |ui| {
                // as wide as its longest entry: every connection on one row
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                if plugin_menu.is_empty() {
                    ui.label(RichText::new(tr("No plugin that is on offers commands.")).weak());
                }
                for (name, plugin, commands) in &plugin_menu {
                    ui.label(RichText::new(name.as_str()).small().weak());
                    for c in commands {
                        if ui.add_enabled(!busy, egui::Button::new(c.label.as_str())).clicked() {
                            plugin_pick = Some((plugin.clone(), c.id.clone()));
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                }
                for group in 0..3 {
                    let list: Vec<&plugins::Connection> = connections.iter().filter(|c| conn_group(c.results, c.paired) == group).collect();
                    if list.is_empty() {
                        continue;
                    }
                    ui.label(RichText::new(conn_group_name(group)).small().weak());
                    for c in list {
                        let label = if group > 0 {
                            trf("Close {where}", &[("where", &c.name)])
                        } else {
                            trf("Disconnect from {where}", &[("where", &display_path(&c.root).trim_end_matches('/'))])
                        };
                        if ui.button(label).clicked() {
                            disconnect = Some(c.root.clone());
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                }
                if ui.button(tr("Plugin settings…")).clicked() {
                    plugin_settings = true;
                    ui.close_menu();
                }
            });
            ui.menu_button(tr("Options"), |ui| {
                ui.checkbox(&mut self.cfg.use_recycle_bin, tr(LBL_RECYCLE));
                ui.checkbox(&mut self.cfg.confirm_delete, tr(LBL_CONFIRM));
                ui.separator();
                ui.label(egui::RichText::new(tr("Quick search")).small().weak());
                ui.radio_value(&mut self.cfg.quick_search_anywhere, false, tr(LBL_QS_PREFIX));
                ui.radio_value(&mut self.cfg.quick_search_anywhere, true, tr(LBL_QS_ANYWHERE));
                ui.separator();
                ui.radio_value(&mut self.cfg.quick_search_loose, true, tr(LBL_QS_LOOSE)).on_hover_text(tr(TIP_QS_LOOSE));
                ui.radio_value(&mut self.cfg.quick_search_loose, false, tr(LBL_QS_EXACT)).on_hover_text(tr(TIP_QS_EXACT));
                ui.separator();
                ui.checkbox(&mut self.cfg.quick_search_arrows, tr(LBL_QS_ARROWS));
                ui.separator();
                ui.menu_button(tr("Language"), |ui| {
                    if ui.radio(self.cfg.language.is_empty(), tr("System default")).clicked() {
                        lang_pick = Some(String::new());
                        ui.close_menu();
                    }
                    ui.separator();
                    for l in &self.languages {
                        if ui.radio(self.cfg.language == l.code, l.name.as_str()).clicked() {
                            lang_pick = Some(l.code.clone());
                            ui.close_menu();
                        }
                    }
                });
                item(ui, tr("Settings…"), "", Cmd::Settings);
            });
            ui.menu_button(tr("Help"), |ui| {
                item(ui, tr("Keyboard"), "", Cmd::Help);
                item(ui, tr("Check for updates…"), "", Cmd::CheckUpdates);
                if ui.button(tr("About")).clicked() {
                    self.open_dialog(Dialog::Message {
                        title: tr("About").into(),
                        text: format!(
                            "RustySalamander {}\n{}",
                            env!("CARGO_PKG_VERSION"),
                            tr("A modern file manager for Windows, inspired by the long-proven efficiency and functionality of Altap Salamander. Full Unicode (UTF-8) support.\n\nParts are based on Open Salamander © Open Salamander Authors, GPL-2.0-or-later.")
                        ),
                    });
                    ui.close_menu();
                }
            });
        });
        if toggle_hidden {
            cmd = Some(Cmd::ToggleHidden);
        }
        if toggle_system {
            cmd = Some(Cmd::ToggleSystem);
        }
        if let Some(mode) = theme_pick {
            cmd = Some(Cmd::SetTheme(mode));
        }
        if layout_cmd.is_some() {
            cmd = layout_cmd;
        }
        if let Some(code) = lang_pick {
            self.set_language(&code);
        }
        if let Some((plugin, id)) = plugin_pick {
            self.run_plugin_command(ctx, plugin, id, plugins::Values::new());
        }
        if let Some(root) = disconnect {
            self.close_connection(&root);
        }
        if plugin_settings {
            self.open_dialog(Dialog::Settings { tab: SettingsTab::Plugins, recording: None });
        }
        if let Some(c) = cmd {
            self.command(ctx, c);
        }
    }

    /// The function-key buttons at the bottom: the commands ticked in
    /// Settings → Keyboard shortcuts (F1 … F10 by default), each with its
    /// first shortcut.
    fn fkey_bar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let actions = keymap::bar_actions(&self.cfg.shortcuts, self.cfg.fkey_bar.as_deref());
        if actions.is_empty() {
            return;
        }
        let buttons: Vec<(String, String, Action)> = actions
            .into_iter()
            .map(|a| {
                let key = keymap::bindings(&self.cfg.shortcuts, a).into_iter().next().unwrap_or_default();
                let text = if key.is_empty() { tr(keymap::bar_label(a)).to_owned() } else { format!("{key} {}", tr(keymap::bar_label(a))) };
                (text, tr(a.info().label).to_owned(), a)
            })
            .collect();
        let mut cmd = None;
        ui.columns(buttons.len(), |cols| {
            for (col, (text, tip, a)) in cols.iter_mut().zip(buttons) {
                // one line: a label too long for a narrow window is cut, not wrapped
                let b = egui::Button::new(RichText::new(text)).wrap_mode(egui::TextWrapMode::Truncate).min_size(egui::vec2(col.available_width(), 0.0));
                if col.add(b).on_hover_text(tip).clicked() {
                    cmd = Some(action_cmd(a));
                }
            }
        });
        if let Some(c) = cmd {
            self.command(ctx, c);
        }
    }
}

/// Dialog windows with a life of their own, not `self.dialog`'s: those of
/// the running operations, the results window and the operations' result
/// window. Closing or opening a
/// dialog leaves them alone (forgetting one every frame would make it hide,
/// show and take the focus again and again).
const OWN_WINDOWS: [&str; 4] = ["job", "prompt", "results", "report"];

/// The icon and label of "From the other panel" for `panel`: the left one
/// takes the right one's folder (double arrow left), and the other way.
fn from_other_label(panel: usize) -> (&'static str, &'static str) {
    if panel == 0 {
        (sal_ui::mdi::REWIND, tr("From right panel"))
    } else {
        (sal_ui::mdi::FAST_FORWARD, tr("From left panel"))
    }
}

/// The fields of a plugin's form: a label and an input per row. The first
/// text input takes the focus when the dialog opens; `browse` is set to the
/// field whose Browse… button was clicked.
fn plugin_form(ui: &mut egui::Ui, form: &plugins::Form, values: &mut plugins::Values, focus: bool, browse: &mut Option<String>) {
    use plugins::FieldKind;
    let mut focus_next = true;
    // the label column is as wide as the longest label of an input; tick
    // boxes and notes go in the input column, so they don't widen it
    let label_w = form
        .fields
        .iter()
        .filter(|f| !matches!(f.kind, FieldKind::Hidden | FieldKind::Note | FieldKind::Check))
        .map(|f| ui.fonts(|fonts| fonts.layout_no_wrap(f.label.clone(), egui::TextStyle::Body.resolve(ui.style()), Color32::WHITE).size().x))
        .fold(0.0f32, f32::max)
        .ceil();
    layout::controls(ui);
    let gap = layout::FORM_SPACING.x;
    let row_h = ui.spacing().interact_size.y;
    ui.spacing_mut().item_spacing.y = layout::FORM_SPACING.y;
    let enabled_fields = sal_plugin_api::ui::enabled_fields(&form.fields, values);
    for f in &form.fields {
        if f.kind == FieldKind::Hidden {
            continue;
        }
        let enabled = enabled_fields.get(&f.id).copied().unwrap_or(true);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let label = if matches!(f.kind, FieldKind::Note | FieldKind::Check) { "" } else { f.label.as_str() };
            ui.allocate_ui_with_layout(egui::vec2(label_w, row_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_min_size(egui::vec2(label_w, row_h));
                ui.add_enabled(enabled, egui::Label::new(label));
            });
            let w = ui.available_width();
            let value = values.entry(f.id.clone()).or_default();
            ui.add_enabled_ui(enabled, |ui| {
                ui.spacing_mut().item_spacing.x = layout::ITEM_SPACING.x;
                let mut text = |ui: &mut egui::Ui, value: &mut String, password: bool, width: f32| {
                    let r = ui.add(layout::text_field(value).password(password).desired_width(width).id_salt(("plugin-field", &f.id)));
                    if focus_next && enabled && (focus || ui.memory(|m| m.focused().is_none())) {
                        r.request_focus();
                    }
                    focus_next = false;
                };
                match &f.kind {
                    FieldKind::Text => text(ui, value, false, w),
                    FieldKind::Password => text(ui, value, true, w),
                    FieldKind::File | FieldKind::Folder => {
                        let browse_label = tr("Browse…");
                        let bw = ui.fonts(|fonts| fonts.layout_no_wrap(browse_label.to_owned(), egui::TextStyle::Button.resolve(ui.style()), Color32::WHITE).size().x)
                            + 2.0 * ui.spacing().button_padding.x;
                        // the text field's margin is outside its desired width
                        text(ui, value, false, (w - bw - layout::ITEM_SPACING.x - 2.0 * layout::FIELD_MARGIN.x).max(120.0));
                        if ui.button(browse_label).clicked() {
                            *browse = Some(f.id.clone());
                        }
                    }
                    FieldKind::Choice(options) => {
                        // a disabled choice shows its first option, as it counts as that
                        // (the pick stays, for when it is enabled again)
                        let shown = if enabled { value.as_str() } else { options.first().map_or("", |(v, _)| v.as_str()) }.to_owned();
                        for (v, label) in options {
                            if ui.radio(shown == *v, label.as_str()).clicked() {
                                *value = v.clone();
                            }
                        }
                    }
                    FieldKind::Check => {
                        let mut on = !value.is_empty();
                        if ui.checkbox(&mut on, f.label.as_str()).changed() {
                            *value = if on { "1".into() } else { String::new() };
                        }
                    }
                    FieldKind::Note => {
                        let note = if f.id == "error" { RichText::new(f.label.as_str()).color(ui.visuals().warn_fg_color) } else { RichText::new(f.label.as_str()).small().weak() };
                        ui.vertical(|ui| {
                            ui.set_width(w);
                            ui.add(egui::Label::new(note).wrap());
                        });
                    }
                    FieldKind::Hidden => {}
                }
            });
        });
    }
}

/// One theme's accent color in Settings: the theme's own, or a custom one
/// with a color button; and how a selected row looks with it.
fn accent_choice(ui: &mut egui::Ui, label: &str, theme: egui::Theme, slot: &mut Option<[u8; 3]>) {
    let own = theme.default_visuals().selection.bg_fill;
    ui.horizontal(|ui| {
        ui.add_sized([90.0, ui.spacing().interact_size.y], egui::Label::new(label));
        let mut custom = slot.is_some();
        if ui.radio_value(&mut custom, false, tr("From the theme (default)")).changed() {
            *slot = None;
        }
        if ui.radio_value(&mut custom, true, tr("Custom:")).changed() {
            *slot = Some([own.r(), own.g(), own.b()]);
        }
        let mut rgb = slot.unwrap_or([own.r(), own.g(), own.b()]);
        if ui.add_enabled_ui(custom, |ui| ui.color_edit_button_srgb(&mut rgb)).inner.changed() {
            *slot = Some(rgb);
        }
        // a selected row in that theme's colors
        let fill = slot.map_or(own, |[r, g, b]| Color32::from_rgb(r, g, b));
        let text = if slot.is_some() { sal_ui::readable_on(fill) } else { theme.default_visuals().selection.stroke.color };
        let h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
        let (r, _) = ui.allocate_exact_size(egui::vec2(110.0, h), egui::Sense::hover());
        ui.painter().rect_filled(r, 0.0, theme.default_visuals().panel_fill);
        ui.painter().rect_filled(r.shrink(2.0), 0.0, fill);
        ui.painter().text(r.left_center() + egui::vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, tr("Selected"), egui::TextStyle::Body.resolve(ui.style()), text);
    });
}

/// The color behind an item marked `m` (folders compared): amber for
/// different, green for only here; none for the same and for placeholders
/// (those are greyed instead).
fn mark_fill(m: sal_plugin_api::ItemMark, dark: bool) -> Option<Color32> {
    use sal_plugin_api::ItemMark;
    let a = if dark { 13 } else { 16 };
    match m {
        ItemMark::Different | ItemMark::Newer | ItemMark::Older => Some(Color32::from_rgba_unmultiplied(230, 160, 30, a)),
        ItemMark::OnlyHere => Some(Color32::from_rgba_unmultiplied(60, 180, 90, a)),
        ItemMark::Same | ItemMark::Missing => None,
    }
}

/// `text` with the characters `hl` (a range) marked as a quick search's
/// match: underlined, on `bg`.
fn qs_job(text: &str, hl: Option<(usize, usize)>, normal: TextFormat, bg: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    match hl {
        Some((a, b)) => {
            // char range -> byte offsets
            let at = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(x, _)| x);
            let (sa, sb) = (at(a), at(b));
            let mut h = normal.clone();
            h.underline = egui::Stroke::new(1.5f32, normal.color);
            h.background = bg;
            job.append(&text[..sa], 0.0, normal.clone());
            job.append(&text[sa..sb], 0.0, h);
            job.append(&text[sb..], 0.0, normal);
        }
        None => job.append(text, 0.0, normal),
    }
    job
}

/// The background tint of a panel a plugin shows: a faint wash of the
/// accent color (the selection color, see `sal_ui::apply_accent`).
fn plugin_tint(v: &egui::Visuals) -> Color32 {
    v.selection.bg_fill.gamma_multiply(if v.dark_mode { 0.12 } else { 0.22 })
}

/// The path bar's segments for `path`: results (a search) start with their
/// name instead of their address.
fn path_crumbs(path: &Path) -> Vec<(String, PathBuf)> {
    let mut crumbs = location::breadcrumbs(path);
    if let Some((root, _)) = plugins::mount_root_of(path) {
        if let (Some(name), Some(first)) = (plugins::results_name(&root), crumbs.first_mut()) {
            first.0 = format!("{} {name}", plugins::results_icon(&root));
        }
    }
    crumbs
}

/// The banner below a plugin's panel: its name and what it shows (the
/// archive's name, or the connection).
fn plugin_banner(plugin: &str, file: &Path) -> String {
    let what = match location::virtual_parts(file) {
        Some(_) if plugins::results_name(file).is_some() => plugins::results_name(file).unwrap_or_default(),
        Some((scheme, authority, _)) => format!("{scheme}://{authority}"),
        None => file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
    };
    format!("{} {plugin} · {what}", sal_ui::mdi::PUZZLE_OUTLINE)
}

/// Background of status strips: the panels' status bar, the operations
/// window's global controls.
fn strip_color(v: &egui::Visuals) -> Color32 {
    if v.dark_mode {
        v.extreme_bg_color
    } else {
        Color32::from_gray(222)
    }
}

/// How many search results a session keeps; the oldest are closed.
const MAX_RESULTS: usize = 10;

/// The updates that Update all installs now: not started (or failed), made
/// for this program, and of something installed (a new plugin only on its
/// own Install).
fn startable(rows: &[UpdateRow], states: &[Option<crate::update::Progress>]) -> Vec<usize> {
    use crate::update::Progress;
    rows.iter()
        .zip(states)
        .enumerate()
        .filter(|(_, (row, state))| !row.update.new && row.update.fits().is_ok() && matches!(state, None | Some(Progress::Failed(_))))
        .map(|(i, _)| i)
        .collect()
}

/// What the update check looks at for the plugins found: those that say
/// where they are published.
fn update_sources(plugins: &Plugins) -> Vec<crate::update::PluginSource> {
    plugins
        .found
        .iter()
        .filter_map(|f| {
            Some(crate::update::PluginSource {
                id: f.info.id.clone(),
                name: f.tr(&f.info.name),
                dir: f.dir().to_path_buf(),
                version: f.info.version.clone(),
                source: f.info.updates.clone()?,
            })
        })
        .collect()
}

/// The official plugins the check offers when they aren't installed: all of
/// them when asked for from the menu, else those not turned down.
fn update_offer(cfg: &Config, manual: bool) -> Option<crate::update::Offer> {
    let lang = if cfg.language.is_empty() { crate::i18n::system_language() } else { cfg.language.clone() };
    Some(crate::update::Offer { plugins: plugins::plugins_dir()?, lang, declined: if manual { Vec::new() } else { cfg.declined_plugins.clone() } })
}

/// A changelog's Markdown, simply: `##` and `###` headings, `-` bullets,
/// **bold** and `code`.
fn markdown(ui: &mut egui::Ui, text: &str) {
    let body = ui.text_style_height(&egui::TextStyle::Body);
    for line in text.lines().map(str::trim_end) {
        if let Some(h) = line.strip_prefix("## ") {
            ui.add_space(layout::GAP_SMALL);
            ui.label(RichText::new(h).strong().size(body * 1.15));
        } else if let Some(h) = line.strip_prefix("### ") {
            ui.add_space(layout::LINE_GAP);
            ui.label(RichText::new(h).strong());
        } else if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            ui.horizontal_top(|ui| {
                ui.add_space(layout::INDENT);
                ui.label("•");
                let job = inline_markdown(ui, item);
                ui.label(job);
            });
        } else if line.trim().is_empty() {
            ui.add_space(layout::LINE_GAP);
        } else if !line.starts_with("# ") {
            let job = inline_markdown(ui, line.trim_start());
            ui.label(job);
        }
    }
}

/// A line with **bold** and `code` as a wrapped layout.
fn inline_markdown(ui: &egui::Ui, text: &str) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let v = ui.visuals();
    let normal = TextFormat { font_id: egui::TextStyle::Body.resolve(ui.style()), color: v.text_color(), ..Default::default() };
    let strong = TextFormat { color: v.strong_text_color(), ..normal.clone() };
    let code = TextFormat { font_id: egui::TextStyle::Monospace.resolve(ui.style()), color: v.text_color(), background: v.code_bg_color, ..Default::default() };
    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    let mut bold = false;
    let mut rest = text;
    while !rest.is_empty() {
        let star = rest.find("**");
        let tick = rest.find('`');
        let fmt = if bold { strong.clone() } else { normal.clone() };
        match (star, tick) {
            (Some(s), t) if t.is_none_or(|t| s < t) => {
                job.append(&rest[..s], 0.0, fmt);
                bold = !bold;
                rest = &rest[s + 2..];
            }
            (_, Some(t)) => {
                job.append(&rest[..t], 0.0, fmt);
                let after = &rest[t + 1..];
                let end = after.find('`').unwrap_or(after.len());
                job.append(&after[..end], 0.0, code.clone());
                rest = after.get(end + 1..).unwrap_or("");
            }
            _ => {
                job.append(rest, 0.0, fmt);
                rest = "";
            }
        }
    }
    job
}

/// A progress bar with square corners.
fn progress_bar(frac: f32) -> egui::ProgressBar {
    egui::ProgressBar::new(frac).rounding(0.0)
}

/// What the user did in the operations window.
enum JobAct {
    Hide,
    /// Pause (true) or resume every running operation.
    PauseAll(bool),
    /// Cancel the running operations and empty the queue.
    CancelAll,
    Pause(u64, bool),
    Cancel(u64),
    /// Move a queued operation up (true) or down.
    Move(u64, bool),
    StartNow(u64),
    Remove(u64),
}

/// One running operation: what it does, progress bars, speed and time left,
/// Pause / Cancel.
/// An operation that ended, for the result window.
struct Report {
    spec: Spec,
    result: Result<OpSummary, String>,
    /// How long it ran, without pauses.
    elapsed: Duration,
    /// When it ended ("2026-11-03 14:25:01").
    ended: String,
}

/// Run in the background, the result at the end, and safe copy: in the Copy
/// and Move dialog, and as their defaults in Settings.
fn transfer_choices(ui: &mut egui::Ui, kind: OpKind, d: &mut TransferDefaults) {
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut d.background, tr("Run in background"))
            .on_hover_text(tr("The progress window doesn't open; the status bar shows the progress (click it to see the window)"));
        ui.add_space(layout::GAP_LARGE);
        // a safe copy always tells how it went: ticked and fixed
        let safe = d.verify != Verify::None;
        let mut shown = d.show_result || safe;
        let r = ui.add_enabled(!safe, egui::Checkbox::new(&mut shown, tr("Show the result when finished")));
        if safe {
            r.on_disabled_hover_text(tr("Always shown after a safe copy or move"));
        } else {
            r.on_hover_text(tr("A window with the result and a summary opens when the operation ends"));
            d.show_result = shown;
        }
    });
    ui.horizontal(|ui| {
        ui.label(if kind == OpKind::Move { tr("Safe move:") } else { tr("Safe copy:") });
        ui.radio_value(&mut d.verify, Verify::None, tr("Off"));
        ui.radio_value(&mut d.verify, Verify::Size, tr("Compare sizes")).on_hover_text(tr("Each copy must have as many bytes as its original"));
        ui.radio_value(&mut d.verify, Verify::Checksum, tr("SHA-256 checksums")).on_hover_text(tr(
            "Each copy is read back and its SHA-256 checksum compared with the original's; the checksums are written to a .sha256 file in the target folder and in the source folder",
        ));
    });
}

/// The options of the Copy and Move dialog, with what they mean.
fn transfer_options(ui: &mut egui::Ui, kind: OpKind, d: &mut TransferDefaults) {
    transfer_choices(ui, kind, d);
    if d.verify != Verify::None {
        let mut notes = Vec::new();
        if kind == OpKind::Move {
            notes.push(tr("An original is deleted only after its copy passed the check."));
        }
        if d.verify == Verify::Checksum {
            notes.push(tr("Reading every copy back takes about as long again."));
        }
        if !notes.is_empty() {
            ui.label(RichText::new(notes.join(" ")).small().weak());
        }
    }
}

/// How an operation in the result window went.
#[derive(Clone, Copy, PartialEq)]
enum Outcome {
    Good,
    Warn,
    Bad,
}

/// The status line of an operation in the result window and its protocol.
fn report_status(r: &Report) -> (&'static str, Outcome) {
    match &r.result {
        Err(_) => (tr("Failed"), Outcome::Bad),
        Ok(s) if s.cancelled => (tr("Cancelled"), Outcome::Warn),
        Ok(s) if s.verify_failed > 0 => (tr("Finished, some copies failed the check"), Outcome::Bad),
        Ok(s) if s.skipped > 0 || !s.problems.is_empty() => (tr("Finished with problems"), Outcome::Warn),
        Ok(_) => (tr("Finished"), Outcome::Good),
    }
}

/// The facts about an operation in the result window and its protocol:
/// heading and text (an error's text is shown in red).
fn report_rows(r: &Report) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    let archive = matches!(r.spec.kind, JobKind::Archive(_));
    match &r.result {
        Err(e) => rows.push((tr("Problem"), e.clone())),
        Ok(s) => {
            rows.push((tr("Done"), trf("{files} ({size})", &[("files", &plural(s.files as usize, "file")), ("size", &format_size(s.bytes))])));
            if s.skipped > 0 {
                rows.push((tr("Skipped"), plural(s.skipped as usize, "file")));
            }
            let secs = r.elapsed.as_secs_f64();
            let mut time = format_duration(r.elapsed);
            if secs >= 1.0 && s.bytes > 0 {
                time.push_str(&format!(" · {}/s", format_size((s.bytes as f64 / secs) as u64)));
            }
            rows.push((tr("Time"), time));
            let mut check = match (s.verify, archive) {
                (_, true) if r.spec.options.verify != Verify::None => tr("Not available inside archives and connections: nothing was checked").to_owned(),
                (Verify::None, _) => tr("Off").to_owned(),
                (Verify::Size, _) => trf("Sizes match: {files}", &[("files", &plural(s.verified as usize, "file"))]),
                (Verify::Checksum, _) => trf("SHA-256 checksums match: {files}", &[("files", &plural(s.verified as usize, "file"))]),
            };
            if s.verify_failed > 0 {
                check.push_str(&format!(" · {}", trf("{files} failed the check (not kept)", &[("files", &plural(s.verify_failed as usize, "file"))])));
            }
            if s.renamed > 0 && s.verify != Verify::None {
                check.push_str(&format!(" · {}", trf("{files} moved within the drive, no data copied", &[("files", &plural(s.renamed as usize, "file"))])));
            }
            rows.push((tr("Check"), check));
            if !s.checksum_files.is_empty() {
                rows.push((tr("Checksum files"), s.checksum_files.iter().map(|p| display_path(p)).collect::<Vec<_>>().join("\n")));
            }
        }
    }
    rows
}

/// One operation in the result window.
fn report_body(ui: &mut egui::Ui, r: &Report) {
    let visuals = ui.visuals().clone();
    let good = if visuals.dark_mode { Color32::from_rgb(110, 200, 120) } else { Color32::from_rgb(30, 130, 50) };
    let (status, outcome) = report_status(r);
    let color = match outcome {
        Outcome::Good => good,
        Outcome::Warn => visuals.warn_fg_color,
        Outcome::Bad => visuals.error_fg_color,
    };
    // the time only: the date is in the protocol
    let time = r.ended.rsplit(' ').next().unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(RichText::new(status).strong().color(color));
        ui.label(RichText::new(trf("at {time}", &[("time", &time)])).weak());
    });
    ui.add(egui::Label::new(r.spec.describe()).wrap());
    ui.add_space(layout::GAP_SMALL);
    layout::table_grid(("report", r.spec.id)).num_columns(2).show(ui, |ui| {
        for (head, text) in report_rows(r) {
            ui.label(RichText::new(head).weak());
            if r.result.is_err() {
                ui.colored_label(visuals.error_fg_color, text);
            } else {
                ui.add(egui::Label::new(text).wrap());
            }
            ui.end_row();
        }
    });
    if let Ok(s) = &r.result {
        if !s.problems.is_empty() {
            ui.add_space(layout::GAP_SMALL);
            let more = if s.problems.len() >= fileops::MAX_PROBLEMS { "+" } else { "" };
            egui::CollapsingHeader::new(RichText::new(format!("{} ({}{more})", tr("Problems"), s.problems.len())).color(visuals.warn_fg_color))
                .id_salt(("problems", r.spec.id))
                .default_open(s.problems.len() <= 10)
                .show(ui, |ui| {
                    for (path, message) in &s.problems {
                        ui.add(egui::Label::new(RichText::new(display_path(path)).small()).wrap());
                        ui.add(egui::Label::new(RichText::new(message).small().color(visuals.error_fg_color)).wrap());
                        ui.add_space(layout::GAP_SMALL);
                    }
                });
        }
    }
}

/// The protocol of the operations in the result window, as plain text to
/// keep: what was done, where, how it was checked, every problem, and the
/// checksum of every file (from the checksum files, while they exist).
fn report_protocol(reports: &[Report]) -> String {
    let mut out = String::new();
    let mut line = |text: &str| {
        out.push_str(text);
        out.push_str("\r\n");
    };
    line(&format!("RustySalamander {} – {}", env!("CARGO_PKG_VERSION"), tr("Operation protocol")));
    line(&trf("Saved {time}", &[("time", &chrono::Local::now().format("%Y-%m-%d %H:%M:%S"))]));
    for r in reports {
        line("");
        line(&"=".repeat(72));
        line(&r.spec.describe());
        line(&format!("{}: {}", tr("Status"), report_status(r).0));
        line(&format!("{}: {}", tr("Ended"), r.ended));
        line(&format!("{}:", tr("Sources")));
        for p in &r.spec.sources {
            line(&format!("    {}", display_path(p)));
        }
        line(&format!("{}: {}", tr("Target"), display_path(&r.spec.target)));
        for (head, text) in report_rows(r) {
            let mut lines = text.lines();
            line(&format!("{head}: {}", lines.next().unwrap_or_default()));
            for more in lines {
                line(&format!("    {more}"));
            }
        }
        if let Ok(s) = &r.result {
            if !s.problems.is_empty() {
                line(&format!("{} ({}):", tr("Problems"), s.problems.len()));
                for (path, message) in &s.problems {
                    line(&format!("    {}", display_path(path)));
                    line(&format!("        {message}"));
                }
            }
            // every file and its checksum: from the first checksum file still there
            if let Some(sums) = s.checksum_files.iter().find_map(|p| std::fs::read_to_string(p).ok()) {
                line(&format!("{}:", tr("Files and their SHA-256 checksums")));
                for l in sums.lines() {
                    line(&format!("    {l}"));
                }
            }
        }
    }
    out
}

fn job_progress_ui(ui: &mut egui::Ui, job: &Job, now: Instant, act: &mut Option<JobAct>) {
    let p = &job.progress;
    ui.add(egui::Label::new(RichText::new(job.spec.describe()).strong()).truncate());
    let current = if p.verifying { trf("Verifying {path}", &[("path", &display_path(&p.current))]) } else { display_path(&p.current) };
    ui.add(egui::Label::new(RichText::new(current).small()).truncate());
    let id = job.spec.id;
    if job.spec.kind.has_progress() {
        let frac = |a: u64, b: u64| if b == 0 { 0.0 } else { (a as f64 / b as f64).min(1.0) as f32 };
        ui.add(progress_bar(frac(p.bytes_done, p.bytes_total)).text(format!("{} / {}", format_size(p.bytes_done), format_size(p.bytes_total))));
        ui.add(progress_bar(frac(p.files_done, p.files_total)).text(trf("{done} / {total} files", &[("done", &p.files_done), ("total", &p.files_total)])));
        let mut parts = vec![trf("elapsed {time}", &[("time", &format_duration(job.rate.elapsed(now)))])];
        if job.prompt.is_some() {
            parts.push(tr("waiting for your answer").to_owned());
        } else if job.is_paused() {
            parts.push(tr("Paused").to_owned());
        } else {
            if let Some(speed) = job.rate.speed() {
                parts.push(format!("{}/s", format_size(speed as u64)));
            }
            parts.push(match job.rate.remaining() {
                Some(d) => trf("about {time} left", &[("time", &format_duration(d))]),
                None => tr("estimating the time left…").to_owned(),
            });
        }
        ui.label(parts.join(" · "));
    } else {
        // no progress reported (Recycle Bin): egui animates only rounded bars
        ui.add(progress_bar(0.0).text(tr("working…")));
    }
    layout::before_buttons(ui);
    let paused = job.is_paused();
    let pause = if paused { tr("Resume") } else { tr("Pause") };
    let labels: Vec<(&str, &str)> = if job.spec.kind.can_pause() { vec![(pause, ""), (tr("Cancel"), "")] } else { vec![(tr("Cancel"), "")] };
    // no default: progress asks nothing, and Enter must not cancel it
    match layout::buttons(ui, &labels, None) {
        Some(0) if labels.len() == 2 => *act = Some(JobAct::Pause(id, !paused)),
        Some(_) => *act = Some(JobAct::Cancel(id)),
        None => {}
    }
}

/// Both items of a copy/move conflict side by side: name, folder, size, date.
fn conflict_table(ui: &mut egui::Ui, src: &Path, dst: &Path) {
    let (s, d) = (std::fs::metadata(src).ok(), std::fs::metadata(dst).ok());
    let size = |m: &Option<std::fs::Metadata>| m.as_ref().filter(|m| m.is_file()).map(|m| m.len());
    let time = |m: &Option<std::fs::Metadata>| m.as_ref().and_then(|m| m.modified().ok());
    let (ss, ds) = (size(&s), size(&d));
    let (st, dt) = (time(&s), time(&d));
    let weak = ui.visuals().weak_text_color();
    let col_w = ((ui.available_width() - 90.0 - 2.0 * layout::TABLE_SPACING.x) / 2.0).max(120.0);
    let row_h = ui.spacing().interact_size.y;
    // every cell is one line of the same height, so the columns line up; long
    // texts are cut to the column, the whole text is in the tooltip
    let cell = |ui: &mut egui::Ui, text: RichText, width: f32, note: Option<&str>| {
        ui.allocate_ui_with_layout(egui::vec2(width, row_h), Layout::left_to_right(Align::Center), |ui| {
            ui.set_width(width);
            if let Some(n) = note {
                // the note first in the layout so it never gets cut off
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(n).small().strong().color(ui.visuals().warn_fg_color));
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(egui::Label::new(text.clone()).truncate()).on_hover_text(text.text());
                    });
                });
            } else {
                ui.add(egui::Label::new(text.clone()).truncate()).on_hover_text(text.text());
            }
        });
    };
    let head_w = 90.0;
    let name = |p: &Path| p.file_name().map(|n| display_name(&n.to_string_lossy()).into_owned()).unwrap_or_default();
    let folder = |p: &Path| p.parent().map(display_path).unwrap_or_default();
    let size_text = |n: Option<u64>| n.map_or_else(|| tr("folder").to_owned(), |n| trf("{size} ({bytes} bytes)", &[("size", &format_size(n)), ("bytes", &format_thousands(n))]));
    let newer = |a: Option<std::time::SystemTime>, b: Option<std::time::SystemTime>| matches!((a, b), (Some(a), Some(b)) if a > b);
    let larger = |a: Option<u64>, b: Option<u64>| matches!((a, b), (Some(a), Some(b)) if a > b);
    layout::table_grid("conflict").num_columns(3).striped(true).show(ui, |ui| {
        cell(ui, RichText::new(""), head_w, None);
        cell(ui, RichText::new(tr("Existing target")).strong(), col_w, None);
        cell(ui, RichText::new(tr("Source")).strong(), col_w, None);
        ui.end_row();
        cell(ui, RichText::new(tr("Name")).color(weak), head_w, None);
        cell(ui, RichText::new(name(dst)), col_w, None);
        cell(ui, RichText::new(name(src)), col_w, None);
        ui.end_row();
        cell(ui, RichText::new(tr("Folder")).color(weak), head_w, None);
        cell(ui, RichText::new(folder(dst)), col_w, None);
        cell(ui, RichText::new(folder(src)), col_w, None);
        ui.end_row();
        cell(ui, RichText::new(tr("Size")).color(weak), head_w, None);
        cell(ui, RichText::new(size_text(ds)), col_w, larger(ds, ss).then(|| tr("larger")));
        cell(ui, RichText::new(size_text(ss)), col_w, larger(ss, ds).then(|| tr("larger")));
        ui.end_row();
        cell(ui, RichText::new(tr("Modified")).color(weak), head_w, None);
        cell(ui, RichText::new(fmt_time(dt)), col_w, newer(dt, st).then(|| tr("newer")));
        cell(ui, RichText::new(fmt_time(st)), col_w, newer(st, dt).then(|| tr("newer")));
        ui.end_row();
    });
}

/// The icons of a panel's items without Windows' own (Material Design Icons).
fn paint_icon(ui: &mut egui::Ui, kind: EntryKind, hidden: bool) {
    let h = ui.text_style_height(&egui::TextStyle::Body);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(h + 4.0, h), Sense::hover());
    let alpha = if hidden { 0.45 } else { 1.0 };
    let (glyph, color) = match kind {
        EntryKind::Dir => (sal_ui::mdi::FOLDER, Color32::from_rgb(232, 182, 64)),
        EntryKind::UpDir => (sal_ui::mdi::ARROW_UP, ui.visuals().text_color()),
        EntryKind::File => (sal_ui::mdi::FILE_OUTLINE, ui.visuals().weak_text_color()),
    };
    drive_icon::paint_glyph(ui.painter(), rect.shrink2(egui::vec2(2.0, 0.0)), glyph, color.gamma_multiply(alpha));
}

/// How many typed paths the drive selector remembers.
const MAX_TYPED_PATHS: usize = 12;

// Labels shared by the menus and Settings, so both always say the same.
const LBL_HIDDEN: &str = n_("Show hidden files");
const LBL_SYSTEM: &str = n_("Show system files");
const LBL_ICONS: &str = n_("Show Windows file icons");
const LBL_SCROLLBARS: &str = n_("Always show scrollbars");
const LBL_ANIMATIONS: &str = n_("Animations");
/// How long the status bar shows what the last operation did.
const JOB_DONE_SHOWN: Duration = Duration::from_secs(5);
const TIP_ANIMATIONS: &str = n_("Windows and menus fading in, smooth scrolling to the cursor (quick search, cursor keys). Off: everything changes at once.");
const TIP_ANIMATIONS_GDI: &str = n_("Off while the windows are drawn without a graphics card (SAL_RENDER=gdi): every animation frame would have to be drawn and sent to the screen.");
const LBL_FKEYS: &str = n_("Show function key bar");
const LBL_RECYCLE: &str = n_("Delete to Recycle Bin");
const LBL_QS_PREFIX: &str = n_("From first letter");
const LBL_QS_ANYWHERE: &str = n_("Anywhere in name");
const LBL_QS_ARROWS: &str = n_("Arrows jump between matches");
const LBL_QS_LOOSE: &str = n_("Letters with or without diacritics");
const LBL_QS_EXACT: &str = n_("Exact letters");
const TIP_QS_LOOSE: &str = n_("vladce finds vládce and vládče; o finds ø, ss finds ß");
const TIP_QS_EXACT: &str = n_("vládce finds only vládce (upper or lower case)");
const LBL_CONFIRM: &str = n_("Confirm delete");
const LBL_CONFIRM_DROP: &str = n_("Confirm copying and moving by drag and drop");
const THEMES: [(ThemeMode, &str); 3] = [(ThemeMode::System, n_("System default")), (ThemeMode::Light, n_("Light")), (ThemeMode::Dark, n_("Dark"))];

/// Which program a Settings "Browse…" button chooses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Browse {
    Editor,
    Viewer,
    /// A program for the suffix form's viewer or editor.
    RuleViewer,
    RuleEditor,
    Terminal,
}

/// A plugin's viewer or editor offered in Settings: plugin id, opener id,
/// and how it reads ("Plugin Viewer: Text and hex viewer").
type OpenerChoice = (String, String, String);

/// The viewers (or editors) the running plugins register.
fn opener_choices(plugins: &Plugins, kind: OpenKind) -> Vec<OpenerChoice> {
    let mut out = Vec::new();
    for p in &plugins.active {
        let name = plugins.name_of(p.id());
        for o in p.openers().into_iter().filter(|o| o.kind == kind) {
            let masks = if o.masks.trim() == "*" { String::new() } else { format!(" · {}", o.masks) };
            let label = trf("Plugin {plugin}: {opener}", &[("plugin", &name), ("opener", &o.label)]);
            out.push((p.id().to_owned(), o.id.clone(), format!("{label}{masks}")));
        }
    }
    out
}

/// How a viewer or editor choice reads in Settings; `None` is a suffix's
/// "Default".
fn open_with_label(w: Option<&OpenWith>, choices: &[OpenerChoice]) -> String {
    match w {
        None => tr("Default").into(),
        Some(OpenWith::Notepad) => tr("Notepad").into(),
        Some(OpenWith::Program { command }) => trf("Program: {command}", &[("command", command)]),
        Some(OpenWith::Plugin { plugin, opener }) => choices
            .iter()
            .find(|(p, o, _)| p == plugin && o == opener)
            .map(|c| c.2.clone())
            .unwrap_or_else(|| trf("Plugin {plugin}: off or missing", &[("plugin", plugin)])),
    }
}

/// Radio buttons for a viewer or an editor: "Default" (`None`, when
/// `allow_default`), the plugins' openers, Notepad, and a program with its
/// command line (`program` keeps it while another choice is made). Returns
/// true when Browse… was clicked.
fn open_with_picker(ui: &mut egui::Ui, id: &str, value: &mut Option<OpenWith>, allow_default: bool, choices: &[OpenerChoice], program: &mut String) -> bool {
    let mut browse = false;
    ui.push_id(id, |ui| {
        ui.vertical(|ui| {
            if allow_default && ui.radio(value.is_none(), tr("Default")).clicked() {
                *value = None;
            }
            for (plugin, opener, label) in choices {
                let this = OpenWith::Plugin { plugin: plugin.clone(), opener: opener.clone() };
                if ui.radio(value.as_ref() == Some(&this), label.as_str()).clicked() {
                    *value = Some(this);
                }
            }
            // a plugin chosen earlier that is off now stays visible
            if let Some(w @ OpenWith::Plugin { plugin, opener }) = value.as_ref() {
                if !choices.iter().any(|(p, o, _)| p == plugin && o == opener) {
                    let _ = ui.radio(true, open_with_label(Some(w), choices));
                }
            }
            if ui.radio(*value == Some(OpenWith::Notepad), tr("Notepad")).clicked() {
                *value = Some(OpenWith::Notepad);
            }
            ui.horizontal(|ui| {
                let on = matches!(value, Some(OpenWith::Program { .. }));
                if ui.radio(on, tr("Program:")).clicked() && !on {
                    *value = Some(OpenWith::Program { command: program.clone() });
                }
                let tip = tr("Program and optional arguments. {file} is replaced by the file;\nwithout it the file is added at the end.");
                let field = egui::TextEdit::singleline(program).hint_text(r#""C:\Program Files\Notepad++\notepad++.exe""#).desired_width(250.0);
                if ui.add_enabled(on, field).on_hover_text(tip).changed() {
                    *value = Some(OpenWith::Program { command: program.clone() });
                }
                if ui.button(tr("Browse…")).clicked() {
                    browse = true;
                }
            });
        });
    });
    browse
}

/// The suffix form in Settings → Viewers and editors: a new rule, or the
/// one at `index` being changed.
struct SuffixEdit {
    index: Option<usize>,
    rule: SuffixRule,
    /// The command lines typed for "Program:", kept while another choice is made.
    view_program: String,
    edit_program: String,
    error: Option<String>,
}

impl SuffixEdit {
    /// A form for `rule`, at `index` in the list (`None`: a new one); the
    /// program command lines start from the ones typed last.
    fn of(index: Option<usize>, rule: SuffixRule, viewer: &str, editor: &str) -> SuffixEdit {
        let program = |w: &Option<OpenWith>, last: &str| match w {
            Some(OpenWith::Program { command }) => command.clone(),
            _ => last.to_owned(),
        };
        SuffixEdit { index, view_program: program(&rule.view, viewer), edit_program: program(&rule.edit, editor), rule, error: None }
    }

    /// The rule to save, or why it can't be.
    fn check(&self, rules: &[SuffixRule]) -> Result<SuffixRule, String> {
        let suffix = SuffixRule::normalize(&self.rule.suffix);
        if suffix.is_empty() {
            return Err(tr("Type a suffix, such as txt.").into());
        }
        if rules.iter().enumerate().any(|(i, r)| Some(i) != self.index && SuffixRule::normalize(&r.suffix) == suffix) {
            return Err(trf(".{suffix} has its own viewer or editor already.", &[("suffix", &suffix)]));
        }
        if self.rule.view.is_none() && self.rule.edit.is_none() {
            return Err(tr("Choose a viewer or an editor for it.").into());
        }
        Ok(SuffixRule { suffix, ..self.rule.clone() })
    }
}

/// Popup id of a font picker (tests open it directly).
fn font_popup_id(ui: &egui::Ui, id: &str) -> egui::Id {
    ui.make_persistent_id(("font_popup", id))
}

/// Drop-down of installed fonts with a search box. Returns true on a change.
/// (egui's ComboBox closes on any click, which made the search box unusable;
/// this popup only closes on a click outside it, Esc or a choice.)
fn font_picker(ui: &mut egui::Ui, id: &str, value: &mut String, default: &str, fonts: &[crate::fonts::SystemFont], filter: &mut String) -> (bool, egui::Rect) {
    let mut changed = false;
    let popup = font_popup_id(ui, id);
    let shown = if value.is_empty() { trf("{font} (default)", &[("font", &default)]) } else { value.clone() };
    let button = ui.add(egui::Button::new(shown).shortcut_text(sal_ui::mdi::MENU_DOWN).min_size(egui::vec2(280.0, 0.0)).wrap_mode(egui::TextWrapMode::Truncate));
    let opening = button.clicked() && !ui.memory(|m| m.is_popup_open(popup));
    if button.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup));
        if opening {
            filter.clear();
        }
    }
    egui::popup_below_widget(ui, popup, &button, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
        ui.set_min_width(300.0);
        let search = ui.add(egui::TextEdit::singleline(filter).hint_text(tr("Search fonts…")).desired_width(f32::INFINITY));
        if opening {
            search.request_focus();
        }
        ui.separator();
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            let pick = |ui: &mut egui::Ui, selected: bool, label: &str, tip: Option<String>| {
                let r = ui.add(egui::SelectableLabel::new(selected, label));
                let r = match tip {
                    Some(t) => r.on_hover_text(t),
                    None => r,
                };
                r.clicked()
            };
            if pick(ui, value.is_empty(), &trf("{font} (default)", &[("font", &default)]), None) {
                value.clear();
                changed = true;
            }
            let needle = filter.to_lowercase();
            for f in fonts.iter().filter(|f| needle.is_empty() || f.name.to_lowercase().contains(&needle)) {
                if pick(ui, value.eq_ignore_ascii_case(&f.name), &f.name, Some(f.path.display().to_string())) {
                    *value = f.name.clone();
                    changed = true;
                }
            }
        });
    });
    if changed {
        ui.memory_mut(|m| m.close_popup());
    } else if ui.memory(|m| m.is_popup_open(popup)) && ui.input(|i| i.key_pressed(Key::Escape)) {
        ui.memory_mut(|m| m.close_popup());
    }
    (changed, button.rect)
}

/// Gap between the two panels, with a line drawn in the middle.
const DIVIDER_WIDTH: f32 = 9.0;

/// The bar between the two panels, in `r`.
fn paint_divider(ui: &egui::Ui, r: egui::Rect) {
    let v = ui.visuals();
    let p = ui.painter();
    // a recessed strip with a clear line, readable in both themes
    // a strip in the separator color with a stronger line in the middle,
    // so the gap reads clearly in both themes
    p.rect_filled(r, 0.0, v.widgets.noninteractive.bg_stroke.color.gamma_multiply(0.6));
    let line = v.widgets.noninteractive.fg_stroke.color.gamma_multiply(0.55);
    p.vline(r.center().x, r.y_range(), egui::Stroke::new(2.0f32, line));
}

/// Theme for the panels, the window title bar and the native menus.
/// The Animations setting as it takes effect: always off when the windows
/// are drawn on the processor (`SAL_RENDER=gdi`, `crate::gdiwin`), where
/// every frame of an animation costs drawing and, over Remote Desktop,
/// sending. The setting itself is kept as it is.
fn animated(on: bool) -> bool {
    on && !crate::gdiwin::chosen()
}

/// Animations on or off ([`animated`]); drawn on the processor, the text
/// cursor doesn't blink either.
fn apply_animations(ctx: &egui::Context, on: bool) {
    sal_ui::apply_animations(ctx, animated(on));
    if crate::gdiwin::chosen() {
        ctx.all_styles_mut(|s| s.visuals.text_cursor.blink = false);
    }
}

/// The Animations checkbox when they're off for drawing on the processor:
/// unchecked, greyed, saying why.
fn animations_off(ui: &mut egui::Ui) {
    ui.add_enabled(false, egui::Checkbox::new(&mut false, tr(LBL_ANIMATIONS))).on_disabled_hover_text(tr(TIP_ANIMATIONS_GDI));
}

fn apply_theme(ctx: &egui::Context, mode: ThemeMode) {
    let (pref, menus) = match mode {
        ThemeMode::System => (egui::ThemePreference::System, None),
        ThemeMode::Light => (egui::ThemePreference::Light, Some(false)),
        ThemeMode::Dark => (egui::ThemePreference::Dark, Some(true)),
    };
    ctx.set_theme(pref);
    // to the main window, also when the change comes from a dialog window
    ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::SetTheme(dialogwin::system_theme(mode)));
    shellmenu::set_menu_theme(menus);
}

/// Font size and the common spacing ([`sal_ui::layout`]) for both the dark
/// and the light style.
fn apply_font_size(ctx: &egui::Context, size: f32) {
    ctx.all_styles_mut(|s| {
        s.spacing.button_padding = sal_ui::layout::BUTTON_PADDING;
        use egui::TextStyle::*;
        for (style, factor) in [(Body, 1.0), (Button, 1.0), (Monospace, 1.0), (Small, 0.8), (Heading, 1.4)] {
            if let Some(f) = s.text_styles.get_mut(&style) {
                f.size = size * factor;
            }
        }
    });
}

/// Keys that are not configurable, by section.
const FIXED_KEYS: &[(Section, &str, &str)] = &[
    (Section::Files, n_("Insert, Space, Ctrl+click"), n_("Select / unselect item")),
    (Section::Files, n_("Shift+↑/↓, Shift+click"), n_("Select range")),
    (Section::Files, "+ / - / *", n_("Select by mask / unselect / invert")),
    (Section::Navigation, "↑ ↓ PgUp PgDn Home End", n_("Move the cursor")),
    (Section::Navigation, n_("Enter / dbl-click"), n_("Open directory or file")),
    (Section::Navigation, "Tab", n_("Switch panel")),
    (Section::Navigation, n_("type text"), n_("Quick search ( / or \\ = any chars; from first letter or anywhere, see Options); Ctrl+↑/↓ next match")),
    (Section::Navigation, n_("click path segment"), n_("Go to that directory")),
    (Section::Navigation, n_("click end of path bar"), n_("Type a path: C:\\dir, \\\\server\\share, \\\\server, D:, %VAR%")),
    (Section::Navigation, "Ctrl+0..9 / Ctrl+Shift+0..9", n_("Go to / set hot path")),
    (Section::System, "Esc", n_("End quick search, cancel a dialog")),
];

impl eframe::App for App {
    /// Windows' drag (out of the window) ends with the button released, a
    /// release the window never saw: egui gets it here, or it would hold the
    /// button down.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.input_hook(ctx, raw);
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let hwnd = match frame.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(w)) => Some(w.hwnd.get()),
            _ => None,
        };
        self.update_frame(ctx, hwnd, frame.info().cpu_usage);
    }
}

impl App {
    /// Windows' drag (out of the window) ends with the button released, a
    /// release the window never saw: egui gets it here, or it would hold the
    /// button down. Before each frame of the main window.
    pub fn input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if std::mem::take(&mut self.release_pointer) {
            let pos = ctx.input(|i| i.pointer.latest_pos()).unwrap_or_default();
            raw.events.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: raw.modifiers });
        }
    }

    /// One frame of the main window `hwnd`; the last one took `drew`
    /// seconds to make and draw.
    pub fn update_frame(&mut self, ctx: &egui::Context, hwnd: Option<isize>, drew: Option<f32>) {
        if let (None, Some(h)) = (self.hwnd, hwnd) {
            self.hwnd = Some(h);
            crate::rawkeys::install(h);
            self.drop_shared = dnd::register(h, ctx.clone());
            crate::debuglog::write(format!("startup: drop target {}", if self.drop_shared.is_some() { "registered" } else { "not registered" }));
            apply_theme(ctx, self.cfg.theme);
            // the shell extensions load in the background, not on the first right-click
            shellmenu::warm_up(self.panels.iter().filter(|p| p.archive.is_none()).map(|p| p.path().to_path_buf()).collect());
        }
        if let Some(h) = self.hwnd.filter(|_| self.transitions != Some(animated(self.cfg.animations))) {
            sal_ui::native::set_transitions(h, animated(self.cfg.animations));
            self.transitions = Some(animated(self.cfg.animations));
        }
        self.pace(ctx, drew);
        if crate::debuglog::full() {
            let (events, closing) = ctx.input(|i| (i.events.clone(), i.viewport().close_requested()));
            crate::debuglog::input(&events, "", false, &|p| self.debug_target(p));
            if closing {
                crate::debuglog::user("close the main window");
            }
        }
        self.frame(ctx);
        self.plugin_windows.show(ctx, self.hwnd, animated(self.cfg.animations));
        self.diag_menu(ctx);
        self.diag_dialog(ctx);
        self.diag_view(ctx);
        if crate::debuglog::full() {
            let state = self.debug_state();
            self.debug_watch.look(state);
            crate::debuglog::end_frame();
        }
    }
}

/// The shortest time between frames without vsync: about 60 a second.
const FRAME_MIN: Duration = Duration::from_millis(16);

impl App {
    /// Frames without vsync (`crate::vsync`) are kept to about 60 a second:
    /// nothing else paces them, and an animation would draw as fast as the
    /// processor can. The debug log tells, for a frame with input, how long
    /// since the last one and how much of it was drawing (the rest is waiting
    /// for the screen, or for the input).
    fn pace(&mut self, ctx: &egui::Context, drew: Option<f32>) {
        let now = Instant::now();
        if let Some(last) = self.last_frame {
            let since = now - last;
            if !crate::vsync() && since < FRAME_MIN {
                std::thread::sleep(FRAME_MIN - since);
            }
            if crate::debuglog::full() && since < Duration::from_secs(1) && ctx.input(|i| !i.events.is_empty()) {
                let drew = drew.map(|s| crate::debuglog::ms(Duration::from_secs_f32(s))).unwrap_or_else(|| "?".into());
                crate::debuglog::write(format!("··· pace     {} since the last frame, which took {drew} to make and draw", crate::debuglog::ms(since)));
            }
        }
        self.last_frame = Some(Instant::now());
    }
}

/// A form value not to be written to the debug log.
fn is_secret(name: &str) -> bool {
    let n = name.to_lowercase();
    ["pass", "secret", "token", "pin"].iter().any(|s| n.contains(s))
}

impl Dialog {
    /// What the dialog is and asks, for the debug log (no passwords).
    fn summary(&self) -> String {
        match self {
            Dialog::None => "none".into(),
            Dialog::PluginForm { plugin, form, .. } => format!("plugin form \"{}\" of {plugin}", form.title),
            Dialog::Transfer { kind, sources, target, opts } => format!("{kind:?} {} items {:?} to {target:?} ({opts:?})", sources.len(), sources.iter().take(5).collect::<Vec<_>>()),
            Dialog::MkDir { name } => format!("new folder {name:?}"),
            Dialog::Rename { path, name } => format!("rename {} to {name:?}", path.display()),
            Dialog::Delete { sources, recycle } => format!("delete {} items {:?} (to the Recycle Bin: {recycle})", sources.len(), sources.iter().take(5).collect::<Vec<_>>()),
            Dialog::SelectMask { select, mask, error } => format!("{} by mask {mask:?}{}", if *select { "select" } else { "unselect" }, error.as_ref().map(|e| format!(", error: {e}")).unwrap_or_default()),
            Dialog::Drive { panel, .. } => format!("drive selector of the {} panel", if *panel == 0 { "left" } else { "right" }),
            Dialog::Message { title, text } => format!("message \"{title}\": {text}"),
            Dialog::Help => "help".into(),
            Dialog::Update { rows } => format!("updates ({} rows)", rows.len()),
            Dialog::Settings { recording, .. } => format!("settings{}", if recording.is_some() { ", recording a shortcut" } else { "" }),
        }
    }
}

impl App {
    /// What is under a click in the main window, for the debug log.
    fn debug_target(&self, pos: egui::Pos2) -> String {
        let side = |i: usize| if i == 0 { "left" } else { "right" };
        for i in 0..2 {
            if let Some((_, row)) = self.row_hits[i].iter().find(|(r, _)| r.contains(pos)) {
                let name = self.panels[i].entries().get(*row).map(|e| e.name.as_str()).unwrap_or("?");
                return format!("{} panel, item {name:?}", side(i));
            }
            if let Some(n) = self.header_rects[i].iter().position(|r| r.contains(pos)) {
                let col = ["Name", "Ext", "Size", "Date", "Attr"].get(n).copied().unwrap_or("?");
                return format!("{} panel, column header {col}", side(i));
            }
            if let Some((_, path)) = self.crumb_rects[i].iter().find(|(r, _)| r.contains(pos)) {
                return format!("{} path bar, {}", side(i), display_path(path));
            }
            if self.body_rects[i].contains(pos) {
                return format!("{} panel, empty space", side(i));
            }
        }
        String::new()
    }

    /// The program's state as the user sees it, for the debug log: what
    /// changes after an action is its result.
    fn debug_state(&self) -> Vec<(String, String)> {
        let mut s: Vec<(String, String)> = Vec::new();
        let mut put = |k: &str, v: String| s.push((k.to_owned(), v));
        put("active panel", if self.active == 0 { "left" } else { "right" }.into());
        for (i, side) in [(0, "left"), (1, "right")] {
            let p = &self.panels[i];
            let entries = p.entries();
            put(&format!("{side} folder"), display_path(p.path()));
            put(&format!("{side} items"), entries.len().to_string());
            let cursor = entries.get(p.cursor).map(|e| format!("{:?} ({} of {})", e.name, p.cursor + 1, entries.len())).unwrap_or_else(|| "none".into());
            put(&format!("{side} cursor"), cursor);
            let selected: Vec<&str> = entries.iter().filter(|e| e.selected).map(|e| e.name.as_str()).collect();
            put(&format!("{side} selected"), if selected.len() > 10 { format!("{} items", selected.len()) } else { format!("{selected:?}") });
            put(&format!("{side} quick search"), format!("{:?}", p.qs));
            put(&format!("{side} path bar"), self.path_edit[i].as_ref().map(|e| format!("editing {:?}", e.text)).unwrap_or_else(|| "shown".into()));
        }
        put("dialog", self.dialog.summary());
        put("status", self.status.clone());
        put("last operation", self.job_done.as_ref().map(|(t, _)| t.clone()).unwrap_or_default());
        let ops: Vec<String> = self
            .jobs
            .running
            .iter()
            .map(|j| {
                let q = match &j.prompt {
                    Some(crate::worker::Prompt::Conflict { src, dst }) => format!(", asks: {} exists ({})", dst.display(), src.display()),
                    Some(crate::worker::Prompt::Error { path, message }) => format!(", asks: error at {}: {message}", path.display()),
                    None => String::new(),
                };
                format!("{} {}{}{q}", j.spec.id, j.spec.describe(), if j.is_paused() { " (paused)" } else { "" })
            })
            .collect();
        put("operations running", if ops.is_empty() { "none".into() } else { ops.join("; ") });
        put("operations queued", self.jobs.queue.len().to_string());
        put("operations window", if self.jobs.hidden { "hidden" } else { "shown when running" }.into());
        put("result window", if self.reports.is_empty() { "closed".into() } else { report_protocol(&self.reports) });
        put("plugin command", self.plugin_task.as_ref().map(|t| format!("{} running", t.plugin.id())).unwrap_or_else(|| "none".into()));
        put("update check", if self.update_check.is_some() { "running" } else { "none" }.into());
        put("context menu", if self.ctx_menu.is_some() { "open" } else { "closed" }.into());
        put("dragging", self.panel_drag.as_ref().map(|_| "items from a panel").unwrap_or("no").into());
        // every setting, by its path in config.toml
        if let Ok(toml::Value::Table(t)) = toml::Value::try_from(&self.cfg) {
            fn flat(prefix: &str, v: &toml::Value, out: &mut Vec<(String, String)>) {
                match v {
                    toml::Value::Table(t) => t.iter().for_each(|(k, v)| flat(&format!("{prefix}.{k}"), v, out)),
                    toml::Value::Array(a) if a.iter().any(|v| v.is_table() || v.is_array()) => a.iter().enumerate().for_each(|(i, v)| flat(&format!("{prefix}[{i}]"), v, out)),
                    v => out.push((prefix.to_owned(), v.to_string())),
                }
            }
            for (k, v) in &t {
                flat(&format!("setting {k}"), v, &mut s);
            }
        }
        s
    }
}

impl App {
    /// Diagnostics (see docs/DEVELOPMENT.md). SAL_DIAG_VIEW=<files> (separated
    /// by `;`) opens them in the viewer after start-up, as F3 does, to look
    /// at the plugins' windows without sending any input.
    fn diag_view(&mut self, ctx: &egui::Context) {
        let Some(files) = std::env::var_os("SAL_DIAG_VIEW") else { return };
        match ctx.cumulative_pass_nr() {
            0..20 => ctx.request_repaint(),
            20 => {
                for f in std::env::split_paths(&files) {
                    if let Err(e) = self.view_file(&f, false) {
                        self.status = e;
                    }
                }
            }
            _ => {}
        }
    }

    /// Diagnostics (see docs/DEVELOPMENT.md). SAL_DIAG_DIALOG=
    /// mkdir|settings|help|copy|delete opens that dialog after start-up (any
    /// other value shows a message box), to look at the dialog windows
    /// without sending any input.
    fn diag_dialog(&mut self, ctx: &egui::Context) {
        let Some(which) = std::env::var_os("SAL_DIAG_DIALOG") else { return };
        if ctx.cumulative_pass_nr() < 20 {
            ctx.request_repaint();
        }
        // SAL_DIAG_DIALOG_CLOSE=cancel|ok: close the dialog again after 3 s
        if let Some(close) = std::env::var_os("SAL_DIAG_DIALOG_CLOSE").filter(|_| ctx.cumulative_pass_nr() > 20) {
            static CLOSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if ctx.input(|i| i.time) < 3.0 {
                ctx.request_repaint_after(Duration::from_millis(200));
            } else if !CLOSED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                let d = std::mem::replace(&mut self.dialog, Dialog::None);
                if close == "ok" {
                    self.dialog_ok(ctx, d);
                }
                ctx.request_repaint();
            }
        }
        // SAL_DIAG_ANSWER=1: answer a conflict / error question with Skip after 6 s
        if std::env::var_os("SAL_DIAG_ANSWER").is_some() && ctx.cumulative_pass_nr() > 20 {
            if ctx.input(|i| i.time) < 6.0 {
                ctx.request_repaint_after(Duration::from_millis(200));
            } else if let Some(j) = self.jobs.running.iter_mut().find(|j| j.prompt.is_some()) {
                let skip = match j.prompt {
                    Some(Prompt::Conflict { .. }) => Reply::Conflict(ConflictAction::Skip),
                    _ => Reply::Error(ErrorAction::Skip),
                };
                j.answer(skip);
                ctx.request_repaint();
            }
        }
        if ctx.cumulative_pass_nr() != 20 {
            return;
        }
        match which.to_str() {
            Some(w @ ("delete" | "copy")) => {
                // the first item in the left panel
                let p = &mut self.panels[0];
                if let Some(i) = p.entries().iter().position(|e| e.kind != EntryKind::UpDir) {
                    p.cursor = i;
                }
                self.active = 0;
                self.command(ctx, if w == "copy" { Cmd::Copy } else { Cmd::Delete { permanent: false } });
            }
            Some("mkdir") => self.command(ctx, Cmd::MkDir),
            Some("settings") => self.command(ctx, Cmd::Settings),
            Some("help") => self.command(ctx, Cmd::Help),
            _ => self.error("SAL_DIAG_DIALOG"),
        }
    }

    /// SAL_DIAG_MENU=1: after start-up, open the classic menu for the first
    /// file, let a timer close it, log what happened, then quit.
    fn diag_menu(&mut self, ctx: &egui::Context) {
        // SAL_DIAG_SEARCH=<folder>: search it for "*.dll" containing a text that
        // isn't there (a long search), watch the results window, then quit
        if let (Some(dir), Some(_)) = (std::env::var_os("SAL_DIAG_SEARCH"), self.hwnd) {
            let frame = ctx.cumulative_pass_nr();
            if frame == 30 {
                if let Some(p) = self.plugins.get("search").cloned() {
                    let values: plugins::Values = [
                        ("dir".to_owned(), dir.to_string_lossy().into_owned()),
                        ("names".to_owned(), "*.dll".to_owned()),
                        ("text".to_owned(), "zzqqxx-not-there".to_owned()),
                        ("subfolders".to_owned(), "1".to_owned()),
                    ]
                    .into();
                    self.run_plugin_command(ctx, p, "search".into(), values);
                }
                self.diag_since = Some(Instant::now());
            }
            if self.diag_since.is_some_and(|t| t.elapsed() > Duration::from_secs(6)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
        }
        #[cfg(windows)]
        if std::env::var_os("SAL_DIAG_MENU").is_some() && self.hwnd.is_some() {
            let frame = ctx.cumulative_pass_nr();
            if frame == 30 {
                let h = self.hwnd.unwrap();
                let first = self.panels[0].entries().iter().find(|e| e.kind == EntryKind::File).map(|e| e.path.clone());
                shellmenu::diag(&format!("hwnd={h:#x} item={first:?}"));
                if let Some(f) = first {
                    let dir = self.panels[0].path().to_path_buf();
                    let at = shellmenu::screen_point(h, Some((200.0, 200.0)));
                    // as after a right-button press: our window holds the mouse capture
                    unsafe {
                        windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(windows::Win32::Foundation::HWND(h as *mut _));
                    }
                    let hold = std::env::var("SAL_DIAG_MENU_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(1000);
                    shellmenu::arm_diag_timer(h, hold);
                    let r = shellmenu::show_classic(h, std::slice::from_ref(&f), &dir, at, false);
                    shellmenu::diag(&format!("classic result: {r:?}"));
                    shellmenu::arm_diag_timer(h, hold);
                    let i = self.panels[0].entries().iter().position(|e| e.path == f);
                    self.open_context_menu(ctx, 0, i, egui::pos2(200.0, 200.0), false);
                    shellmenu::diag("windows 11 style native menu done");
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
        }
    }
}

impl App {
    /// One UI frame; separate from `update` so tests can drive it headlessly.
    pub fn frame(&mut self, ctx: &egui::Context) {
        let mut timing = crate::debuglog::Frame::start(ctx.cumulative_pass_nr());
        self.remember_dirs();
        // network folders read in the background (at start-up)
        let hidden = self.vis();
        for p in &mut self.panels {
            p.poll_loading(hidden);
        }
        if self.panels.iter().any(Panel::is_loading) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.poll_update_check();
        self.poll_refreshing(ctx);
        timing.mark("background reads");
        let no_dialog = matches!(self.dialog, Dialog::None);
        let editing_path = self.path_edit.iter().any(Option::is_some);
        let menu_open = self.ctx_menu.is_some();
        // taken every frame: one pressed while a dialog was open is dropped
        let raw_keys = crate::rawkeys::take();
        if no_dialog && !editing_path && !menu_open {
            self.handle_right_button(ctx);
        }
        if no_dialog && !editing_path && !menu_open {
            // the panels own the keyboard: never let Tab/Space land on a widget
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
            self.handle_keys(ctx, raw_keys);
            timing.mark("keys");
            let hidden = self.vis();
            for p in &mut self.panels {
                if p.needs_refresh() {
                    p.reload(hidden);
                }
            }
            timing.mark("refresh check");
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            self.save_config();
            self.close_all_connections();
            plugins::mount::unmount_all();
            plugins::cleanup_temp();
        }
        self.poll_plugin_task(ctx);
        self.follow_links();

        timing.mark("before menu");
        egui::TopBottomPanel::top("menu").show(ctx, |ui| self.menu_bar(ctx, ui));
        timing.mark("menu");
        // the status bar always, the function keys when they are on; as much
        // room below as at the sides (egui's panels leave only 2 points)
        let frame = egui::Frame::side_top_panel(&ctx.style());
        let side = frame.inner_margin.left;
        let frame = frame.inner_margin(egui::Margin { left: side, right: side, top: layout::GAP_SMALL, bottom: side });
        egui::TopBottomPanel::bottom("fkeys").frame(frame).show(ctx, |ui| {
            self.jobs_indicator(ui);
            if !self.status.is_empty() {
                let strong = ui.visuals().strong_text_color();
                ui.add(egui::Label::new(RichText::new(&self.status).color(strong)).truncate());
            }
            if self.cfg.show_fkey_bar {
                self.fkey_bar(ctx, ui);
            }
        });
        timing.mark("status and function keys");
        egui::CentralPanel::default().show(ctx, |ui| {
            let full = ui.available_rect_before_wrap();
            let half = ((full.width() - DIVIDER_WIDTH) / 2.0).floor();
            let left = egui::Rect::from_min_size(full.min, egui::vec2(half, full.height()));
            let right = egui::Rect::from_min_max(egui::pos2(full.right() - half, full.top()), full.max);
            let divider = egui::Rect::from_min_max(egui::pos2(left.right(), full.top()), egui::pos2(right.left(), full.bottom()));
            for (idx, rect) in [(0, left), (1, right)] {
                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
                    ui.set_clip_rect(rect.intersect(ui.clip_rect()));
                    self.panel_ui(ui, idx);
                });
                timing.mark(if idx == 0 { "left panel" } else { "right panel" });
            }
            paint_divider(ui, divider);
            ui.allocate_rect(full, Sense::hover());
        });

        self.report_sizes();
        self.icons.poll(ctx);
        timing.mark("sizes and icons");
        self.drag_ui(ctx);
        self.context_menu_ui(ctx);
        self.dialogs(ctx);
        timing.mark("dialogs");
        self.job_ui(ctx);
        self.results_ui(ctx);
        timing.mark("operations");

        timing.finish();

        // poll directories for external changes
        ctx.request_repaint_after(Duration::from_millis(1500));
    }
}

#[cfg(test)]
mod tests {
    //! Headless UI tests: real frames, real keyboard events, no OS window.
    use super::*;
    use std::fs;

    thread_local! {
        /// Files handed to Windows to open (see `shell_open`).
        pub static OPENED: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Stands in for the Viewer plugin, whose windows tests never open: it
    /// views every file, edits `*.edit` files, and remembers each request
    /// (opener, file, options).
    #[derive(Default)]
    struct Recorder {
        opened: std::sync::Mutex<Vec<(String, PathBuf, plugins::Values)>>,
    }

    impl FolderPlugin for Recorder {
        fn id(&self) -> &str {
            "recorder"
        }
        fn handles(&self, _: &str) -> bool {
            false
        }
        fn can_write(&self, _: &Path) -> bool {
            false
        }
        fn status(&self) -> String {
            String::new()
        }
        fn list(&self, _: &Path, _: &str) -> std::io::Result<plugins::Index> {
            Err(std::io::ErrorKind::Unsupported.into())
        }
        fn extract(&self, _: &Path, _: &[String], _: &Path, _: &mut sal_plugin_api::Progress) -> std::io::Result<()> {
            Err(std::io::ErrorKind::Unsupported.into())
        }
        fn add(&self, _: &Path, _: &[(PathBuf, String)], _: &mut sal_plugin_api::Progress) -> std::io::Result<()> {
            Err(std::io::ErrorKind::Unsupported.into())
        }
        fn delete(&self, _: &Path, _: &[String], _: &mut sal_plugin_api::Progress) -> std::io::Result<()> {
            Err(std::io::ErrorKind::Unsupported.into())
        }
        fn rename(&self, _: &Path, _: &str, _: &str, _: &mut sal_plugin_api::Progress) -> std::io::Result<()> {
            Err(std::io::ErrorKind::Unsupported.into())
        }
        fn mkdir(&self, _: &Path, _: &str, _: &mut sal_plugin_api::Progress) -> std::io::Result<()> {
            Err(std::io::ErrorKind::Unsupported.into())
        }
        fn openers(&self) -> Vec<Opener> {
            vec![
                Opener { id: "view".into(), kind: OpenKind::View, label: "Recorder".into(), masks: "*".into() },
                Opener { id: "edit".into(), kind: OpenKind::Edit, label: "Recorder".into(), masks: "*.edit".into() },
            ]
        }
        fn open_file(&self, opener: &str, file: &Path, options: &plugins::Values) -> std::io::Result<()> {
            self.opened.lock().unwrap().push((opener.to_owned(), file.to_path_buf(), options.clone()));
            Ok(())
        }
    }

    struct Ui {
        ctx: egui::Context,
        app: App,
        /// What F3 and F4 asked the plugins to open.
        recorder: Arc<Recorder>,
        snap: crate::snapshot::Snapshot,
        last: Option<egui::FullOutput>,
        /// Screen size in points (taller for some snapshots).
        screen: egui::Vec2,
    }

    fn press(key: Key, modifiers: Modifiers) -> Vec<Event> {
        vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }, Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }]
    }

    impl Ui {
        fn new(left: &Path, right: &Path) -> Ui {
            let ctx = egui::Context::default();
            // the real Viewer plugin would open windows: the recorder views instead
            let cfg = Config { use_recycle_bin: false, language: "en".into(), disabled_plugins: vec!["viewer".into()], ..Config::default() };
            let mut app = App::with_config(&ctx, cfg, vec![left.to_string_lossy().into(), right.to_string_lossy().into()]);
            let recorder = Arc::new(Recorder::default());
            app.plugins.active.insert(0, recorder.clone());
            let mut ui = Ui { ctx, app, recorder, snap: Default::default(), last: None, screen: egui::vec2(1200.0, 800.0) };
            ui.frames(2);
            ui
        }

        fn run(&mut self, events: Vec<Event>, modifiers: Modifiers) {
            let raw = egui::RawInput { events, modifiers, screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.screen)), ..Default::default() };
            let app = &mut self.app;
            let out = self.ctx.run(raw, |ctx| app.frame(ctx));
            self.snap.apply(&out);
            self.last = Some(out);
        }

        fn frames(&mut self, n: usize) {
            for _ in 0..n {
                self.run(Vec::new(), Modifiers::NONE);
            }
        }

        fn key(&mut self, key: Key) {
            self.key_mod(key, Modifiers::NONE);
        }

        fn key_mod(&mut self, key: Key, m: Modifiers) {
            self.run(press(key, m), m);
            self.frames(2);
        }

        fn text(&mut self, t: &str) {
            self.run(vec![Event::Text(t.into())], Modifiers::NONE);
            self.frames(1);
        }

        fn wait_job(&mut self) {
            for _ in 0..500 {
                if self.app.jobs.is_idle() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
                self.frames(1);
            }
            panic!("job did not finish");
        }

        /// Put the recorder back first after a test changed the plugins.
        fn keep_recorder(&mut self) {
            self.app.plugins.active.retain(|p| p.id() != "recorder");
            self.app.plugins.active.insert(0, self.recorder.clone());
        }

        /// The files F3 showed, in order.
        fn viewed(&self) -> Vec<PathBuf> {
            self.recorder.opened.lock().unwrap().iter().filter(|(o, _, _)| o == "view").map(|(_, f, _)| f.clone()).collect()
        }

        fn cursor_name(&self) -> String {
            self.app.panels[self.app.active].current().map(|e| e.name.clone()).unwrap_or_default()
        }
    }

    #[test]
    fn mkdir_with_unicode_name() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.key(Key::F7);
        assert!(matches!(ui.app.dialog, Dialog::MkDir { .. }));
        ui.text("Новая папка 🦎 測試");
        ui.key(Key::Enter);
        assert!(t.path().join("Новая папка 🦎 測試").is_dir());
        assert_eq!(ui.cursor_name(), "Новая папка 🦎 測試");
    }

    /// The plugins folder the build made in target\<profile> (the 7-Zip
    /// plugin and its 7z.dll), for the archive tests.
    fn built_library() -> Option<PathBuf> {
        let dir = std::env::current_exe().ok()?.parent()?.parent()?.join("plugins");
        dir.join("7zip").join("7z.dll").is_file().then_some(dir)
    }

    /// The real Viewer plugin, as built: its own version, where it is
    /// published, and that it views every file. (Its windows are never
    /// opened in tests.)
    #[test]
    fn viewer_plugin_as_built() {
        let Some(dir) = built_library() else { return };
        let mut p = Plugins::scan(Some(&dir));
        let found = p.found.iter().find(|f| f.info.id == "viewer").expect("plugins\\viewer\\viewer.spl");
        assert_eq!(found.info.interface, sal_plugin_api::abi::VERSION);
        assert!(semver::Version::parse(&found.info.version).is_ok(), "{}", found.info.version);
        let src = found.info.updates.clone().expect("published");
        assert_eq!((src.tag.as_str(), src.changelog.as_str()), ("viewer-v", "plugins/viewer/CHANGELOG.md"));
        assert!(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(&src.changelog).is_file(), "the changelog it names exists");
        // every plugin of the program says its version and where it is published
        for f in p.found.iter().filter(|f| f.info.id != "filelist") {
            assert!(f.info.updates.is_some(), "{}", f.info.id);
        }
        p.start(&[]);
        let viewer = p.get("viewer").unwrap_or_else(|| panic!("{:?}", p.errors));
        let openers = viewer.openers();
        assert_eq!(openers.len(), 1);
        assert!(openers[0].kind == OpenKind::View && openers[0].matches("cokoli 🦎.bin"));
    }

    /// The Viewer plugin's real windows, which the program draws: a file
    /// opens a window, its frames draw the file (triangles and the font
    /// texture), Escape closes it, and turning the plugin off closes the
    /// windows it has open.
    #[test]
    fn viewer_windows() {
        use egui::{Event, Key, RawInput, ViewportId, ViewportInfo};
        let Some(dir) = built_library() else { panic!("plugins not built") };
        let mut p = Plugins::scan(Some(&dir));
        p.start(&[]);
        let viewer = p.get("viewer").unwrap_or_else(|| panic!("{:?}", p.errors)).clone();
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("první 🦎.json"), t.path().join("druhý.bin"));
        fs::write(&a, "{
  \"text\": \"Příliš žluťoučký kůň 日本語\",
  \"count\": 42
}
").unwrap();
        fs::write(&b, [0u8, 1, 2, 3, 255]).unwrap();
        let input = |events: Vec<Event>| {
            let mut raw = RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), events, ..Default::default() };
            raw.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(1.0), ..Default::default() });
            raw
        };
        // the window's content is made in the background, then handed to the program
        let opened = |n: usize| {
            let mut all = Vec::new();
            for _ in 0..300 {
                all.extend(crate::pluginwin::take_opened());
                if all.len() >= n {
                    return all;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!("{} windows of {n} opened", all.len());
        };
        let options: plugins::Values = [("theme".to_owned(), "dark".to_owned())].into();
        viewer.open_file("text", &a, &options).unwrap();
        let mut wins = opened(1);
        assert!(wins[0].0.contains("první 🦎.json"), "{}", wins[0].0);
        let first = (wins[0].1)(input(vec![])).expect("the window stays");
        assert!(first.title.contains("první 🦎.json"));
        assert!(!first.meshes.is_empty() && !first.textures.set.is_empty(), "the file drawn, with the font");
        assert!(first.commands.contains(&egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark)));
        let escape = Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };
        assert!((wins[0].1)(input(vec![escape])).is_none(), "Escape closes it");
        drop(wins);
        // turning the plugin off closes its windows
        viewer.open_file("text", &b, &options).unwrap();
        wins = opened(1);
        assert!((wins[0].1)(input(vec![])).is_some());
        drop(viewer);
        p.start(&["viewer".to_owned()]);
        assert!((wins[0].1)(input(vec![])).is_none(), "closed with the plugin");
    }

    fn arc_ui(left: &Path, right: &Path) -> Option<Ui> {
        let Some(dir) = built_library() else {
            eprintln!("plugins not built; skipping");
            return None;
        };
        // found when the app starts, so panels can open inside archives
        std::env::set_var("RUSTYSALAMANDER_PLUGINS", dir);
        let ui = Ui::new(left, right);
        assert!(ui.app.plugins.get("7zip").is_some(), "{:?} {:?}", ui.app.plugins.errors, ui.app.plugins.broken);
        Some(ui)
    }

    /// The 7-Zip plugin, started from the built plugins folder.
    fn seven_zip() -> Arc<dyn FolderPlugin> {
        let mut p = Plugins::scan(Some(&built_library().unwrap()));
        p.start(&[]);
        p.get("7zip").unwrap_or_else(|| panic!("{:?}", p.errors)).clone()
    }

    /// An archive with: docs\zpráva 🦎.txt, docs\sub\deep.txt, top.txt
    fn make_archive(dir: &Path, ext: &str) -> PathBuf {
        let stage = dir.join("stage");
        fs::create_dir_all(stage.join("docs").join("sub")).unwrap();
        fs::write(stage.join("docs").join("zpráva 🦎.txt"), "ahoj").unwrap();
        fs::write(stage.join("docs").join("sub").join("deep.txt"), "hluboko").unwrap();
        fs::write(stage.join("top.txt"), "nahoře").unwrap();
        let arc = dir.join(format!("archiv ž.{ext}"));
        let z = seven_zip();
        z.add(&arc, &[(stage.join("docs"), "docs".into()), (stage.join("top.txt"), "top.txt".into())], &mut |_, _| true).unwrap();
        fs::remove_dir_all(&stage).unwrap();
        arc
    }

    fn names(ui: &Ui, panel: usize) -> Vec<String> {
        ui.app.panels[panel].entries().iter().map(|e| e.name.clone()).collect()
    }

    fn cursor_to(ui: &mut Ui, name: &str) {
        let i = ui.app.panels[ui.app.active].entries().iter().position(|e| e.name == name).unwrap_or_else(|| panic!("{name} not listed in {:?}", names(ui, ui.app.active)));
        ui.app.panels[ui.app.active].set_cursor(i);
    }

    #[test]
    fn archive_opens_as_a_folder() {
        let t = tempfile::tempdir().unwrap();
        if built_library().is_none() {
            return;
        }
        let arc = make_archive(t.path(), "zip");
        let Some(mut ui) = arc_ui(t.path(), t.path()) else { return };
        cursor_to(&mut ui, "archiv ž.zip");
        ui.key(Key::Enter);
        if let Dialog::Message { text, .. } = &ui.app.dialog {
            panic!("error: {text}");
        }
        assert_eq!(ui.app.panels[0].path(), arc.as_path(), "inside the archive");
        assert_eq!(names(&ui, 0), ["..", "docs", "top.txt"]);
        cursor_to(&mut ui, "docs");
        ui.key(Key::Enter);
        assert_eq!(names(&ui, 0), ["..", "sub", "zpráva 🦎.txt"]);
        assert_eq!(ui.app.panels[0].path(), arc.join("docs").as_path());
        // .. goes up inside the archive, then out of it with the cursor on the archive
        ui.key(Key::Home);
        ui.key(Key::Enter);
        ui.key(Key::Home);
        ui.key(Key::Enter);
        assert_eq!(ui.app.panels[0].path(), t.path());
        assert_eq!(ui.cursor_name(), "archiv ž.zip");
        // switched off: the archive is an ordinary file again (Enter would hand it to Windows)
        ui.app.panels[0].load(&arc.join("docs"), ui.app.vis(), None).unwrap();
        ui.app.cfg.disabled_plugins = vec!["7zip".into()];
        ui.app.apply_plugins();
        assert_eq!(ui.app.panels[0].path(), t.path(), "left the archive");
        assert!(ui.app.panels[0].load(&arc, ui.app.vis(), None).is_err());
    }

    #[test]
    fn copy_out_of_and_into_an_archive() {
        if built_library().is_none() {
            return;
        }
        for ext in ["7z", "zip"] {
            let t = tempfile::tempdir().unwrap();
            let arc = make_archive(t.path(), ext);
            let out = t.path().join("out");
            fs::create_dir_all(&out).unwrap();
            fs::write(out.join("nový.txt"), "nový").unwrap();
            let Some(mut ui) = arc_ui(&arc, &out) else { return };
            assert_eq!(names(&ui, 0), ["..", "docs", "top.txt"]);
            let ctx = ui.ctx.clone();
            // out of the archive: a folder with its contents
            ui.app.start_job(&ctx, JobKind::Transfer(OpKind::Copy), vec![arc.join("docs")], out.clone());
            ui.wait_job();
            assert_eq!(fs::read_to_string(out.join("docs").join("sub").join("deep.txt")).unwrap(), "hluboko", "{ext}");
            assert_eq!(fs::read_to_string(out.join("docs").join("zpráva 🦎.txt")).unwrap(), "ahoj");
            // into a folder inside the archive
            ui.app.start_job(&ctx, JobKind::Transfer(OpKind::Move), vec![out.join("nový.txt")], arc.join("docs").join("sub"));
            ui.wait_job();
            assert!(!out.join("nový.txt").exists(), "moved");
            ui.app.panels[0].load(&arc.join("docs").join("sub"), ui.app.vis(), None).unwrap();
            assert_eq!(names(&ui, 0), ["..", "deep.txt", "nový.txt"]);
            // delete, new folder and rename inside the archive
            ui.app.start_job(&ctx, JobKind::Delete, vec![arc.join("docs").join("sub").join("deep.txt")], PathBuf::new());
            ui.wait_job();
            ui.app.start_job(&ctx, JobKind::Archive(FolderOp::MkDir), vec![arc.join("docs").join("sub").join("prázdná")], PathBuf::new());
            ui.wait_job();
            ui.app.start_job(&ctx, JobKind::Archive(FolderOp::Rename), vec![arc.join("top.txt")], arc.join("hlavní.txt"));
            ui.wait_job();
            if let Dialog::Message { text, .. } = &ui.app.dialog {
                panic!("{ext}: {text}");
            }
            ui.app.panels[0].reload(ui.app.vis());
            assert_eq!(names(&ui, 0), ["..", "prázdná", "nový.txt"], "{ext}");
            ui.app.panels[0].load(&arc, ui.app.vis(), None).unwrap();
            assert_eq!(names(&ui, 0), ["..", "docs", "hlavní.txt"], "{ext}");
        }
    }

    /// F5 into a panel showing a zip: the dialog's target ends with a
    /// separator ("…\\archiv ž.zip\\"), which once made the copy try to create
    /// a folder named like the archive (OS error 183).
    #[test]
    fn copy_into_an_archive_through_the_dialog() {
        let t = tempfile::tempdir().unwrap();
        if built_library().is_none() {
            return;
        }
        let arc = make_archive(t.path(), "zip");
        let src = t.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("přidaný.txt"), "nový").unwrap();
        let Some(mut ui) = arc_ui(&src, &arc) else { return };
        cursor_to(&mut ui, "přidaný.txt");
        ui.key(Key::F5);
        match &ui.app.dialog {
            Dialog::Transfer { target, .. } => assert!(target.ends_with('\\'), "{target}"),
            _ => panic!("no copy dialog"),
        }
        ui.key(Key::Enter);
        ui.wait_job();
        if let Dialog::Message { text, .. } = &ui.app.dialog {
            panic!("{text}");
        }
        assert!(arc.is_file(), "still the archive file, not a folder");
        ui.app.panels[1].reload(ui.app.vis());
        assert_eq!(names(&ui, 1), ["..", "docs", "přidaný.txt", "top.txt"]);
    }

    /// Drag and drop with a panel showing a zip: into it and out of it, in
    /// the window and from other programs.
    #[test]
    fn drag_and_drop_with_an_archive() {
        let t = tempfile::tempdir().unwrap();
        if built_library().is_none() {
            return;
        }
        let arc = make_archive(t.path(), "zip");
        let src = t.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("přidaný.txt"), "nový").unwrap();
        let Some(mut ui) = arc_ui(&src, &arc) else { return };
        ui.app.cfg.confirm_drop = false;
        ui.frames(2);
        // in the window: into the archive
        let (from, to) = (row_of(&ui, 0, "přidaný.txt"), ui.app.body_rects[1].center());
        drag(&mut ui, from, to, Modifiers::CTRL);
        ui.wait_job();
        if let Dialog::Message { text, .. } = &ui.app.dialog {
            panic!("into: {text}");
        }
        ui.app.panels[1].reload(ui.app.vis());
        ui.frames(2);
        assert_eq!(names(&ui, 1), ["..", "docs", "přidaný.txt", "top.txt"], "dropped into the archive");
        // out of the archive, onto the other panel
        let (from, to) = (row_of(&ui, 1, "top.txt"), ui.app.body_rects[0].center());
        drag(&mut ui, from, to, Modifiers::CTRL);
        ui.wait_job();
        if let Dialog::Message { text, .. } = &ui.app.dialog {
            panic!("out: {text}");
        }
        assert_eq!(fs::read_to_string(src.join("top.txt")).unwrap(), "nahoře", "dropped out of the archive");
        // from another program: the archive panel takes them
        let zones = ui.app.drop_zones();
        let outside = vec![src.join("top.txt")];
        let hit = dnd::resolve(&zones, ui.app.body_rects[1].center(), &outside, &dnd::volume(&outside[0]), false, false);
        assert_eq!(hit.map(|(z, k)| (z.folder, k)), Some((arc.clone(), OpKind::Copy)));
        // out of the window: files on disk in the plugin's mount (when
        // Windows' Projected File System is on), read from the archive as
        // they are read, folders with their contents
        assert!(ui.app.panel_drag.is_none());
        match ui.app.mount_items(&[arc.join("docs"), arc.join("top.txt")]) {
            Ok(paths) => {
                assert_eq!(paths[0].file_name().unwrap(), "docs");
                assert_eq!(fs::read_to_string(&paths[1]).unwrap(), "nahoře");
                assert_eq!(fs::read_to_string(paths[0].join("sub").join("deep.txt")).unwrap(), "hluboko");
                assert_eq!(fs::read_to_string(paths[0].join("zpráva 🦎.txt")).unwrap(), "ahoj");
            }
            Err(plugins::mount::MountError::NotEnabled) => assert!(!plugins::mount::available()),
            Err(plugins::mount::MountError::Failed(e)) => panic!("{e}"),
        }
        // search results take nothing; a comparison's side does
        let root = sal_plugin_api::mount_root("hledej", "1");
        plugins::add_mount(&root, "7zip", &root, Some(("hledání", "")));
        assert!(plugins::is_search_results(&root));
        plugins::set_paired(&root);
        assert!(!plugins::is_search_results(&root));
        plugins::remove_mount(&root);
    }

    #[test]
    fn copy_into_an_archive_asks_about_existing_items() {
        let t = tempfile::tempdir().unwrap();
        if built_library().is_none() {
            return;
        }
        let arc = make_archive(t.path(), "7z");
        let src = t.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("top.txt"), "nový obsah").unwrap();
        let Some(mut ui) = arc_ui(&src, &arc) else { return };
        let ctx = ui.ctx.clone();
        ui.app.start_job(&ctx, JobKind::Transfer(OpKind::Copy), vec![src.join("top.txt")], arc.clone());
        for _ in 0..500 {
            ui.frames(1);
            if ui.app.jobs.running.first().is_some_and(|j| j.prompt.is_some()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(ui.app.jobs.running[0].prompt, Some(Prompt::Conflict { .. })), "asks first");
        ui.app.jobs.running[0].answer(Reply::Conflict(ConflictAction::Overwrite));
        ui.wait_job();
        let out = t.path().join("check");
        seven_zip().extract(&arc, &["top.txt".into()], &out, &mut |_, _| true).unwrap();
        assert_eq!(fs::read_to_string(out.join("top.txt")).unwrap(), "nový obsah");
    }

    #[test]
    fn view_a_file_inside_an_archive() {
        let t = tempfile::tempdir().unwrap();
        if built_library().is_none() {
            return;
        }
        let arc = make_archive(t.path(), "zip");
        let Some(mut ui) = arc_ui(&arc.join("docs"), t.path()) else { return };
        cursor_to(&mut ui, "zpráva 🦎.txt");
        ui.key(Key::F3);
        ui.wait_job();
        let viewed = ui.viewed();
        assert_eq!(viewed.len(), 1, "extracted, then shown in the viewer");
        assert!(viewed[0].ends_with(Path::new("docs").join("zpráva 🦎.txt")), "{viewed:?}");
        let opened = ui.recorder.opened.lock().unwrap();
        assert_eq!(opened[0].2["temporary"], "1", "the viewer is told it is a temporary copy");
    }

    /// A plugin is loaded only by the program versions it says it works with.
    #[test]
    fn plugins_say_which_program_versions_they_need() {
        let this = env!("CARGO_PKG_VERSION");
        assert!(plugins::program_fits("", "").is_ok());
        assert!(plugins::program_fits(this, this).is_ok(), "both ends included");
        assert!(plugins::program_fits("0.1.0", "").is_ok() && plugins::program_fits("", "99.0.0").is_ok());
        assert!(plugins::program_fits("not a version", "").is_ok(), "a range that can't be read is no limit");
        assert_eq!(plugins::program_fits("99.0.0", "").unwrap_err(), "needs RustySalamander 99.0.0 or later");
        assert_eq!(plugins::program_fits("", "0.1.0").unwrap_err(), "works with RustySalamander up to 0.1.0 only");
        assert_eq!(plugins::program_fits("98.0.0", "99.0.0").unwrap_err(), "needs RustySalamander 98.0.0 to 99.0.0");
    }

    /// A plugins folder for one test, `target\<profile>\test-plugins\<name>`:
    /// with the built 7-Zip plugin and its language files when `plugin`, and
    /// `lib`: the real 7z.dll (`Some(true)`), a damaged one (`Some(false)`)
    /// or none. Not a temporary folder: loaded DLLs stay loaded, so it
    /// couldn't be removed; the next run replaces it.
    fn plugin_folder(name: &str, plugin: bool, lib: Option<bool>) -> Option<PathBuf> {
        let built = built_library()?.join("7zip");
        let dir = built.parent()?.parent()?.join("test-plugins").join(name);
        let _ = fs::remove_dir_all(&dir);
        let mine = dir.join("7zip");
        fs::create_dir_all(mine.join("lang")).unwrap();
        if plugin {
            fs::copy(built.join("7zip.spl"), mine.join("7zip.spl")).unwrap();
            for e in fs::read_dir(built.join("lang")).unwrap().flatten() {
                fs::copy(e.path(), mine.join("lang").join(e.file_name())).unwrap();
            }
        }
        match lib {
            Some(true) => drop(fs::copy(built.join("7z.dll"), mine.join("7z.dll")).unwrap()),
            Some(false) => fs::write(mine.join("7z.dll"), b"MZ but not really a library").unwrap(),
            None => {}
        }
        Some(dir)
    }

    /// A plugin that opens "connections" to a folder in memory: a command
    /// shows a form, the form's OK mounts it. Listed per folder, like a
    /// server.
    struct MemServer {
        files: std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
        closed: std::sync::Mutex<Vec<PathBuf>>,
    }

    impl FolderPlugin for MemServer {
        fn id(&self) -> &str {
            "memserver"
        }
        fn handles(&self, _: &str) -> bool {
            false
        }
        fn can_write(&self, _: &Path) -> bool {
            true
        }
        fn status(&self) -> String {
            String::new()
        }
        fn list(&self, _: &Path, folder: &str) -> std::io::Result<plugins::Index> {
            let files = self.files.lock().unwrap();
            let is_dir = |p: &str| p.is_empty() || files.keys().any(|k| k.starts_with(&format!("{p}\\")));
            if !is_dir(folder) {
                return Err(std::io::ErrorKind::NotFound.into());
            }
            let mut items = Vec::new();
            for (k, v) in files.iter().filter(|(k, _)| sal_plugin_api::under(k, folder) && k.as_str() != folder) {
                let rest = if folder.is_empty() { k.as_str() } else { &k[folder.len() + 1..] };
                let name = rest.split('\\').next().unwrap();
                let dir = rest.contains('\\') || k.ends_with("\\.keep");
                let path = plugins::join_inner(folder, name);
                if name != ".keep" && !items.iter().any(|i: &sal_plugin_api::Item| i.path == path) {
                    items.push(sal_plugin_api::Item { path, dir, size: if dir { 0 } else { v.len() as u64 }, modified: None, attrs: 0, index: None });
                }
            }
            Ok(plugins::Index { items, complete: false })
        }
        fn extract(&self, _: &Path, items: &[String], out: &Path, _: &mut sal_plugin_api::Progress) -> std::io::Result<()> {
            for (k, v) in self.files.lock().unwrap().iter().filter(|(k, _)| items.iter().any(|i| sal_plugin_api::under(k, i))) {
                fs::create_dir_all(out.join(k).parent().unwrap())?;
                fs::write(out.join(k), v)?;
            }
            Ok(())
        }
        fn add(&self, _: &Path, items: &[(PathBuf, String)], _: &mut sal_plugin_api::Progress) -> std::io::Result<()> {
            for (disk, inner) in items {
                self.files.lock().unwrap().insert(inner.clone(), fs::read(disk)?);
            }
            Ok(())
        }
        fn delete(&self, _: &Path, items: &[String], _: &mut sal_plugin_api::Progress) -> std::io::Result<()> {
            self.files.lock().unwrap().retain(|k, _| !items.iter().any(|i| sal_plugin_api::under(k, i)));
            Ok(())
        }
        fn rename(&self, _: &Path, _: &str, _: &str, _: &mut sal_plugin_api::Progress) -> std::io::Result<()> {
            Err(std::io::ErrorKind::Unsupported.into())
        }
        fn mkdir(&self, _: &Path, inner: &str, _: &mut sal_plugin_api::Progress) -> std::io::Result<()> {
            self.files.lock().unwrap().insert(format!("{inner}\\.keep"), Vec::new());
            Ok(())
        }
        fn commands(&self) -> Vec<plugins::Command> {
            vec![plugins::Command { id: "connect".into(), label: "Connect…".into() }]
        }
        fn run(&self, command: &str, input: &plugins::Values) -> std::io::Result<plugins::Reply> {
            match command {
                "connect" => Ok(plugins::Reply::Form(plugins::Form {
                    title: "Connect".into(),
                    ok: "Connect".into(),
                    command: "mount".into(),
                    fields: vec![
                        sal_plugin_api::Field { id: "host".into(), label: "Server:".into(), ..Default::default() },
                        sal_plugin_api::Field { id: "password".into(), label: "Password:".into(), kind: plugins::FieldKind::Password, ..Default::default() },
                    ],
                })),
                "mount" if input["password"] != "tajné" => Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Wrong password.")),
                "mount" => Ok(plugins::Reply::Mount(plugins::Mount { scheme: "mem".into(), authority: input["host"].clone(), folder: "home".into() })),
                _ => Err(std::io::ErrorKind::Unsupported.into()),
            }
        }
        fn close(&self, root: &Path) {
            self.closed.lock().unwrap().push(root.to_path_buf());
        }
    }

    /// The Compare plugin (the real DLL) compares the two panels' folders:
    /// both panels show the comparison, marked; going into a folder in one
    /// takes the other there too; copying across makes an item the same;
    /// closing one side closes both. SNAPSHOT_DIR set: also saved as
    /// compare_panels.png.
    #[test]
    fn compare_plugin_folders_in_both_panels() {
        use sal_plugin_api::ItemMark;
        let Some(built) = built_library().filter(|d| d.join("compare").join("compare.spl").is_file()) else {
            eprintln!("compare plugin not built; skipping");
            return;
        };
        let t = tempfile::tempdir().unwrap();
        let (l, r) = (t.path().join("levá"), t.path().join("pravá"));
        for d in [&l, &r] {
            fs::create_dir_all(d.join("docs 🦎")).unwrap();
            fs::write(d.join("same.txt"), "stejné").unwrap();
        }
        // the same times, so size and date match
        let when = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        for d in [&l, &r] {
            fs::File::options().write(true).open(d.join("same.txt")).unwrap().set_modified(when).unwrap();
        }
        fs::write(l.join("only left.txt"), "x").unwrap();
        fs::write(l.join("docs 🦎").join("inner.txt"), "nový obsah").unwrap();
        fs::write(r.join("docs 🦎").join("inner.txt"), "starý").unwrap();
        let dir = t.path().join("plugins");
        fs::create_dir_all(dir.join("compare").join("lang")).unwrap();
        fs::copy(built.join("compare").join("compare.spl"), dir.join("compare").join("compare.spl")).unwrap();
        let mut found = plugins::Plugins::scan(Some(&dir));
        found.start(&[]);
        let compare = found.get("compare").expect("the compare plugin starts").clone();

        let mut ui = Ui::new(&l, &r);
        ui.app.plugins = found;
        ui.keep_recorder();
        for p in &mut ui.app.panels {
            p.plugins = ui.app.plugins.active.clone();
        }
        crate::fonts::install(&ui.ctx, "", "");
        ui.screen = egui::vec2(1100.0, 500.0);
        let ctx = ui.ctx.clone();
        // nothing selected, the cursors on ..: the two panels' folders
        ui.app.active = 0;
        ui.app.run_plugin_command(&ctx, compare.clone(), "compare".into(), plugins::Values::new());
        wait_plugin(&mut ui);
        assert!(matches!(ui.app.dialog, Dialog::None), "{:?}", ui.app.status);
        let (lroot, rroot) = (sal_plugin_api::mount_root("compare", "1-left"), sal_plugin_api::mount_root("compare", "1-right"));
        assert_eq!((ui.app.panels[0].path(), ui.app.panels[1].path()), (lroot.as_path(), rroot.as_path()));
        // both list everything either side has, marked
        let names = |ui: &Ui, i: usize| ui.app.panels[i].entries().iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&ui, 0), names(&ui, 1), "the lists line up");
        let mark = |ui: &Ui, i: usize, name: &str| ui.app.panels[i].archive.as_ref().unwrap().marks.get(name).copied();
        assert_eq!(mark(&ui, 0, "only left.txt"), Some(ItemMark::OnlyHere));
        assert_eq!(mark(&ui, 1, "only left.txt"), Some(ItemMark::Missing));
        assert_eq!(mark(&ui, 0, "same.txt"), Some(ItemMark::Same));
        assert_eq!(mark(&ui, 0, "docs 🦎"), Some(ItemMark::Different), "a folder with a difference inside");
        ui.frames(3);
        if let Some(dir) = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from) {
            let bg = ui.ctx.style().visuals.panel_fill;
            let out = ui.last.take().unwrap();
            ui.snap.render(&ui.ctx, out, bg, &dir.join("compare_panels.png"));
        }

        // into the folder on the left: the right follows
        cursor_to(&mut ui, "docs 🦎");
        ui.key(Key::Enter);
        ui.frames(2);
        assert_eq!(ui.app.panels[0].path(), lroot.join("docs 🦎"));
        assert_eq!(ui.app.panels[1].path(), rroot.join("docs 🦎"), "the other panel follows");
        // and back up in the right one: the left follows
        ui.app.active = 1;
        ui.key(Key::Backspace);
        ui.frames(2);
        assert_eq!((ui.app.panels[0].path(), ui.app.panels[1].path()), (lroot.as_path(), rroot.as_path()));

        // the right side in the left panel: the left panel keeps its own side
        let hidden = ui.app.vis();
        ui.app.panels[0].load(&rroot.join("docs 🦎"), hidden, None).unwrap();
        ui.frames(2);
        assert_eq!(ui.app.panels[0].path(), lroot.join("docs 🦎"));
        assert_eq!(ui.app.panels[1].path(), rroot.join("docs 🦎"), "and the right one follows");
        ui.app.panels[0].load(&lroot, hidden, None).unwrap();
        ui.frames(2);
        assert_eq!(ui.app.panels[1].path(), rroot.as_path());
        // each panel's drive selector offers its own side only
        for (panel, own) in [(0, &lroot), (1, &rroot)] {
            ui.app.command(&ctx, Cmd::DriveMenu(panel));
            ui.frames(1);
            let Dialog::Drive { conns, .. } = &ui.app.dialog else { panic!("drive menu not open") };
            let sides: Vec<&PathBuf> = conns.iter().filter(|c| c.paired).map(|c| &c.root).collect();
            assert_eq!(sides, [own]);
            ui.key(Key::Escape);
        }

        // with all drive buttons: the comparison's button brings a panel that
        // left it back to its side
        ui.app.cfg.drive_buttons = DriveButtons::All;
        ui.app.panels[0].load(&l, hidden, None).unwrap();
        ui.frames(2);
        assert_eq!(ui.app.panels[1].path(), rroot.as_path(), "the other side stays");
        if let Some(dir) = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from) {
            let bg = ui.ctx.style().visuals.panel_fill;
            let out = ui.last.take().unwrap();
            ui.snap.render(&ui.ctx, out, bg, &dir.join("compare_all_drives.png"));
        }
        ui.app.show_comparison();
        ui.frames(2);
        assert_eq!((ui.app.panels[0].path(), ui.app.panels[1].path()), (lroot.as_path(), rroot.as_path()));
        ui.app.cfg.drive_buttons = DriveButtons::default();

        // copy the file only the left has into the right side (F5 from the left)
        ui.app.active = 0;
        cursor_to(&mut ui, "only left.txt");
        ui.key(Key::F5);
        ui.key(Key::Enter);
        ui.wait_job();
        ui.frames(2);
        assert_eq!(fs::read_to_string(r.join("only left.txt")).unwrap(), "x", "the real right folder");
        let hidden = ui.app.vis();
        ui.app.panels[1].reload(hidden);
        assert_eq!(mark(&ui, 1, "only left.txt"), Some(ItemMark::Same));

        // listed as a comparison, not a search
        assert!(plugins::connections().iter().filter(|c| c.root.starts_with(sal_plugin_api::mount_root("compare", ""))).all(|c| conn_group(c.results, c.paired) == 2));

        // one comparison at a time: a second one replaces the first
        let mut v = plugins::Values::new();
        v.insert("left".into(), l.join("docs 🦎").display().to_string());
        v.insert("right".into(), r.join("docs 🦎").display().to_string());
        ui.app.run_plugin_command(&ctx, compare.clone(), "folders".into(), v);
        wait_plugin(&mut ui);
        let (lroot, rroot) = (sal_plugin_api::mount_root("compare", "2-left"), sal_plugin_api::mount_root("compare", "2-right"));
        let open: Vec<PathBuf> = plugins::mounts().into_iter().filter(|(_, id)| id == "compare").map(|(m, _)| m).collect();
        assert_eq!(open, [lroot.clone(), rroot.clone()], "only the new comparison");
        assert_eq!(ui.app.links.len(), 1);
        assert_eq!((ui.app.panels[0].path(), ui.app.panels[1].path()), (lroot.as_path(), rroot.as_path()));

        // closing one side closes both: the panels go back
        ui.app.close_connection(&lroot);
        assert!(!plugins::mounts().iter().any(|(m, _)| *m == lroot || *m == rroot));
        assert!(ui.app.links.is_empty());
        assert_eq!((ui.app.panels[0].path(), ui.app.panels[1].path()), (l.as_path(), r.as_path()));
    }

    /// The lists beside all drive buttons: a row's name goes there, its
    /// right-hand button disconnects (a server) or closes (results), and
    /// results that can be filled again have Refresh. SNAPSHOT_DIR set: the
    /// open list is saved as connection_list.png.
    #[test]
    fn connection_list_buttons() {
        let conn = |name: &str, results: bool, refresh: &str| plugins::Connection {
            root: PathBuf::from(format!("scp://{name}/")),
            plugin: "scp".into(),
            start: PathBuf::from(format!("scp://{name}/home")),
            results,
            name: name.into(),
            refresh: refresh.into(),
            paired: false,
        };
        let list = vec![conn("me@server", false, ""), conn("*.txt \"hello\" · data", true, "refresh")];
        let mut ui = Ui::new(Path::new("."), Path::new("."));
        crate::fonts::install(&ui.ctx, "", "");
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 300.0));
        let popup = std::cell::Cell::new(egui::Id::NULL);
        let pass = |ui: &mut Ui, events: Vec<Event>| {
            let mut picked = None;
            let raw = egui::RawInput { events, screen_rect: Some(screen), ..Default::default() };
            let out = ui.ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        let id = ui.make_persistent_id(("test", 0));
                        popup.set(id);
                        if !ui.memory(|m| m.is_popup_open(id)) {
                            ui.memory_mut(|m| m.open_popup(id));
                        }
                        picked = connection_list(ui, ("test", 0), sal_ui::mdi::SERVER_NETWORK, Color32::GRAY, "tip", &list, Path::new("scp://me@server/home"));
                    });
                });
            });
            ui.snap.apply(&out);
            ui.last = Some(out);
            picked
        };
        for _ in 0..3 {
            pass(&mut ui, Vec::new());
        }
        if let Some(out) = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from) {
            let bg = ui.ctx.style().visuals.panel_fill;
            let frame = ui.last.take().unwrap();
            ui.snap.render(&ui.ctx, frame, bg, &out.join("connection_list.png"));
        }
        let area = ui.ctx.memory(|m| m.area_rect(popup.get())).expect("the list is open");
        let rows = area.shrink(ui.ctx.style().spacing.menu_margin.sum().y / 2.0);
        let row_h = rows.height() / 2.0;
        let click = |ui: &mut Ui, at: egui::Pos2| {
            pass(ui, vec![Event::PointerMoved(at)]);
            pass(ui, vec![Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
            pass(ui, vec![Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }])
        };
        // the last column starts where Disconnect, its widest button, does
        let right = |row: f32| egui::pos2(rows.right() - 75.0, rows.top() + row_h * (row + 0.5));
        let left = |row: f32| egui::pos2(rows.left() + 20.0, rows.top() + row_h * (row + 0.5));
        assert_eq!(click(&mut ui, right(0.0)), Some(ConnPick::Close(PathBuf::from("scp://me@server/"))), "Disconnect");
        assert_eq!(click(&mut ui, right(1.0)), Some(ConnPick::Close(PathBuf::from("scp://*.txt \"hello\" · data/"))), "Close");
        assert_eq!(click(&mut ui, left(0.0)), Some(ConnPick::Open(PathBuf::from("scp://me@server/home"), PathBuf::from("scp://me@server/"))));
    }

    /// The search plugin (the DLL the build made) end to end: the form, the
    /// results window filling, going to a result, the results as a folder,
    /// and opening a result opens the real file. SNAPSHOT_DIR set: the
    /// window is saved as search_results.png.
    #[test]
    fn search_plugin_results_window() {
        use crate::results::Act;
        let Some(built) = built_library().filter(|d| d.join("search").join("search.spl").is_file()) else {
            eprintln!("search plugin not built; skipping");
            return;
        };
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        fs::create_dir_all(data.join("docs").join("staré")).unwrap();
        fs::write(data.join("poznámky.txt"), "hello world").unwrap();
        fs::write(data.join("docs").join("prázdný.txt"), "nic").unwrap();
        fs::write(data.join("docs").join("staré").join("dopis 🦎.txt"), "Hello again").unwrap();
        let dir = t.path().join("plugins");
        fs::create_dir_all(dir.join("search").join("lang")).unwrap();
        fs::copy(built.join("search").join("search.spl"), dir.join("search").join("search.spl")).unwrap();
        let mut found = plugins::Plugins::scan(Some(&dir));
        found.start(&[]);
        let search = found.get("search").expect("the search plugin starts").clone();

        let mut ui = Ui::new(&data, &data);
        ui.app.plugins = found;
        ui.keep_recorder();
        for p in &mut ui.app.panels {
            p.plugins = ui.app.plugins.active.clone();
        }
        crate::fonts::install(&ui.ctx, "", "");
        let ctx = ui.ctx.clone();
        // Alt+F7's command: the form, in the active panel's folder
        ui.app.command(&ctx, Cmd::Find);
        wait_plugin(&mut ui);
        let Dialog::PluginForm { values, .. } = &mut ui.app.dialog else { panic!("no form") };
        assert_eq!(values["dir"], data.display().to_string());
        values.insert("names".into(), "*.txt".into());
        values.insert("text".into(), "hello".into());
        ui.key(Key::Enter);
        wait_plugin(&mut ui);
        assert!(matches!(ui.app.dialog, Dialog::None));
        let root = ui.app.results.as_ref().expect("the results window").root.clone();

        // the window fills until the search is done
        let mut rows = Vec::new();
        for _ in 0..400 {
            ui.frames(1);
            rows = ui.app.results.as_ref().unwrap().rows().iter().map(|r| r.inner.clone()).collect();
            if rows.len() == 2 && !search.activity(&root).running {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(rows, ["poznámky.txt", "docs\\staré\\dopis 🦎.txt"]);
        // the window stays the same window: frames without a dialog, and a
        // dialog opening and closing, leave it alone (forgotten every frame,
        // it hid, showed and took the focus again and again: it flickered)
        let serial = dialogwin::serial(&ui.app.dialog_places, "results");
        assert!(serial.is_some());
        ui.frames(3);
        ui.app.open_dialog(Dialog::Message { title: "x".into(), text: "y".into() });
        ui.frames(2);
        ui.app.dialog = Dialog::None;
        ui.frames(2);
        assert_eq!(dialogwin::serial(&ui.app.dialog_places, "results"), serial);
        ui.frames(3);
        if let Some(out) = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from) {
            let bg = ui.ctx.style().visuals.panel_fill;
            let frame = ui.last.take().unwrap();
            ui.snap.render(&ui.ctx, frame, bg, &out.join("search_results.png"));
        }

        // Refresh: the same search again, into the same results
        fs::write(data.join("nová.txt"), "hello").unwrap();
        ui.app.results_act(&ctx, Act::Refresh);
        wait_plugin(&mut ui);
        let mut rows = 0;
        for _ in 0..400 {
            ui.frames(1);
            rows = ui.app.results.as_ref().unwrap().rows().len();
            if rows == 3 && !search.activity(&root).running {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(rows, 3, "found again, with the new file");
        fs::remove_file(data.join("nová.txt")).unwrap();

        // the drive selector's name for them
        if let Some(dir) = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from) {
            // with all drive buttons: the searches in a list beside them
            ui.app.cfg.drive_buttons = DriveButtons::All;
            ui.frames(3);
            let bg = ui.ctx.style().visuals.panel_fill;
            let out = ui.last.take().unwrap();
            ui.snap.render(&ui.ctx, out, bg, &dir.join("search_all_drives.png"));
            ui.app.cfg.drive_buttons = DriveButtons::default();
        }
        let names_of: Vec<String> = plugins::connections().into_iter().filter(|c| c.results && c.plugin == "search").map(|c| c.name).collect();
        assert_eq!(names_of, ["*.txt \"hello\" · data (filtered folders)", "*.txt \"hello\" · data (all in one list)"]);
        // Open (Enter, double-click) opens the file itself; F3 views it
        ui.app.results_act(&ctx, Act::Open("poznámky.txt".into()));
        assert_eq!(tests::OPENED.with(|o| o.borrow().last().cloned()), Some(data.join("poznámky.txt")));
        ui.app.results_act(&ctx, Act::View("poznámky.txt".into()));
        assert_eq!(ui.viewed().last(), Some(&data.join("poznámky.txt")), "the file itself");

        // Go to file: its folder in the active panel, on the file
        ui.app.results_act(&ctx, Act::GoTo("docs\\staré\\dopis 🦎.txt".into()));
        assert_eq!(ui.app.act().path(), data.join("docs").join("staré"));
        assert_eq!(ui.app.act().current().unwrap().name, "dopis 🦎.txt");

        // as a folder: only what was found, below the folders leading to it
        ui.app.results_act(&ctx, Act::ShowFolder);
        assert_eq!(ui.app.act().path(), root);
        // the path bar says what it is, not search://1
        let crumbs: Vec<String> = path_crumbs(&root.join("docs")).into_iter().map(|(l, _)| l).collect();
        assert_eq!(crumbs, [format!("{} *.txt \"hello\" · data (filtered folders)", sal_ui::mdi::MAGNIFY), "docs".to_owned()]);
        let names = |ui: &mut Ui| ui.app.act().entries().iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&mut ui), ["docs", "poznámky.txt"], "a connection's root has no ..");
        // the flat list: every file found, without folders
        ui.app.results_act(&ctx, Act::ShowList);
        assert_eq!(names(&mut ui), ["dopis 🦎.txt", "poznámky.txt"]);
        assert!(plugins::is_results(&root), "closed, not disconnected");
        let hidden = ui.app.vis();
        ui.app.act().load(&root.join("docs").join("staré"), hidden, None).unwrap();
        // Enter on a result opens the real file, not a temporary copy
        let i = ui.app.act().entries().iter().position(|e| e.name == "dopis 🦎.txt").unwrap();
        ui.app.act().set_cursor(i);
        ui.app.command(&ctx, Cmd::Enter);
        let opened = tests::OPENED.with(|o| o.borrow().clone());
        assert_eq!(opened.last(), Some(&data.join("docs").join("staré").join("dopis 🦎.txt")));

        // closing the window keeps the results a panel shows; disconnecting
        // them in the drive selector ends them
        ui.app.results_act(&ctx, Act::Close);
        assert!(ui.app.results.is_none());
        assert!(plugins::mounts().iter().any(|(r, _)| *r == root));
        ui.app.close_connection(&root);
        assert!(!plugins::mounts().iter().any(|(r, _)| *r == root));
        // the plugin is told in the background
        let gone = (0..200).any(|_| {
            std::thread::sleep(Duration::from_millis(10));
            search.list(&root, "").is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        });
        assert!(gone, "the plugin forgot them");

        // turning the plugin off forgets its searches at once
        ui.app.command(&ctx, Cmd::Find);
        wait_plugin(&mut ui);
        ui.key(Key::Enter);
        wait_plugin(&mut ui);
        assert!(ui.app.results.is_some());
        assert!(plugins::connections().iter().any(|c| c.plugin == "search"));
        ui.app.cfg.disabled_plugins.push("search".into());
        ui.app.apply_plugins();
        assert!(!plugins::connections().iter().any(|c| c.plugin == "search"), "gone from the drive selector");
        assert!(ui.app.results.is_none() && ui.app.kept_results.is_empty());
    }

    /// A session keeps at most ten results; the oldest are closed, both
    /// their connections.
    #[test]
    fn at_most_ten_searches_are_kept() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        let plugin: Arc<dyn FolderPlugin> = Arc::new(MemServer { files: Default::default(), closed: Default::default() });
        let root = |n: usize, list: bool| sal_plugin_api::mount_root("limit", &if list { format!("{n}-list") } else { n.to_string() });
        for n in 1..=12 {
            let r = plugins::Results {
                title: format!("search {n}"),
                name: format!("search {n}"),
                mount: plugins::Mount { scheme: "limit".into(), authority: n.to_string(), folder: String::new() },
                flat: format!("{n}-list"),
                again: String::new(),
                refresh: String::new(),
            };
            ui.app.open_results(plugin.clone(), r);
            // as if a panel showed it, so the next search keeps it
            ui.app.results = None;
        }
        let open: Vec<PathBuf> = plugins::mounts().into_iter().map(|(r, _)| r).filter(|r| r.to_string_lossy().starts_with(r"\\?\limit\")).collect();
        for n in 1..=12 {
            let kept = n > 2;
            assert_eq!(open.contains(&root(n, false)), kept, "search {n}");
            assert_eq!(open.contains(&root(n, true)), kept, "search {n}, its list");
        }
        assert_eq!(ui.app.kept_results.len(), 10);
        for n in 3..=12 {
            ui.app.close_connection(&root(n, false));
            ui.app.close_connection(&root(n, true));
        }
    }

    /// A new search from the results window replaces the one shown there:
    /// its connections go unless a panel shows them.
    #[test]
    fn new_search_closes_the_previous_one() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        let plugin: Arc<dyn FolderPlugin> = Arc::new(MemServer { files: Default::default(), closed: Default::default() });
        ui.app.plugins.active.push(plugin.clone());
        for p in &mut ui.app.panels {
            p.plugins = ui.app.plugins.active.clone();
        }
        let root = |n: usize, list: bool| sal_plugin_api::mount_root("again", &if list { format!("{n}-list") } else { n.to_string() });
        let search = |n: usize| plugins::Results {
            title: format!("search {n}"),
            name: format!("search {n}"),
            mount: plugins::Mount { scheme: "again".into(), authority: n.to_string(), folder: String::new() },
            flat: format!("{n}-list"),
            again: String::new(),
            refresh: String::new(),
        };
        let open = || plugins::mounts().into_iter().map(|(r, _)| r).filter(|r| r.to_string_lossy().starts_with(r"\\?\again\")).collect::<Vec<_>>();
        ui.app.open_results(plugin.clone(), search(1));
        ui.app.open_results(plugin.clone(), search(2));
        assert_eq!(open(), [root(2, false), root(2, true)], "the first search is gone, both its views");
        // a panel shows the second search's list: only that one stays
        let hidden = ui.app.vis();
        ui.app.act().load(&root(2, true), hidden, None).unwrap();
        ui.app.open_results(plugin.clone(), search(3));
        let mut now = open();
        now.sort();
        let mut want = vec![root(2, true), root(3, false), root(3, true)];
        want.sort();
        assert_eq!(now, want);
        assert_eq!(ui.app.kept_results.len(), 2);
        let home = t.path().to_path_buf();
        ui.app.act().load(&home, hidden, None).unwrap();
        for r in [root(2, true), root(3, false), root(3, true)] {
            ui.app.close_connection(&r);
        }
    }

    /// Wait for the plugin command running in the background (generously:
    /// the tests run side by side and may keep the machine busy).
    fn wait_plugin(ui: &mut Ui) {
        let until = Instant::now() + Duration::from_secs(20);
        while Instant::now() < until {
            ui.frames(1);
            if ui.app.plugin_task.is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("plugin command did not finish");
    }

    #[test]
    fn a_plugin_connection_in_a_panel() {
        let t = tempfile::tempdir().unwrap();
        let local = t.path().join("local");
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join("místní 🦎.txt"), "z disku").unwrap();
        let server = Arc::new(MemServer {
            files: std::sync::Mutex::new([("home\\vzdálený.txt".to_owned(), b"ze serveru".to_vec()), ("home\\docs\\a.txt".to_owned(), b"a".to_vec())].into()),
            closed: std::sync::Mutex::new(Vec::new()),
        });
        let mut ui = Ui::new(&local, &local);
        let plugin: Arc<dyn FolderPlugin> = server.clone();
        ui.app.plugins.active.push(plugin.clone());
        for p in &mut ui.app.panels {
            p.plugins = ui.app.plugins.active.clone();
        }
        let ctx = ui.ctx.clone();

        // the command shows the plugin's form; a wrong password is reported
        ui.app.run_plugin_command(&ctx, plugin.clone(), "connect".into(), plugins::Values::new());
        wait_plugin(&mut ui);
        let Dialog::PluginForm { values, .. } = &mut ui.app.dialog else { panic!("no form") };
        values.insert("host".into(), "test-ui".into());
        values.insert("password".into(), "špatné".into());
        ui.key(Key::Enter);
        wait_plugin(&mut ui);
        assert!(matches!(&ui.app.dialog, Dialog::Message { text, .. } if text == "Wrong password."));
        ui.key(Key::Enter);

        // connected: the panel shows the plugin's folder, with its banner
        ui.app.run_plugin_command(&ctx, plugin.clone(), "connect".into(), plugins::Values::new());
        wait_plugin(&mut ui);
        let Dialog::PluginForm { values, .. } = &mut ui.app.dialog else { panic!("no form") };
        values.insert("host".into(), "test-ui".into());
        values.insert("password".into(), "tajné".into());
        ui.key(Key::Enter);
        wait_plugin(&mut ui);
        ui.app.active = 1;
        let root = sal_plugin_api::mount_root("mem", "test-ui");
        assert_eq!(ui.app.panels[0].path(), root.join("home"), "opened in the active panel");
        assert_eq!(display_path(ui.app.panels[0].path()), "mem://test-ui/home");
        assert_eq!(names(&ui, 0), ["..", "docs", "vzdálený.txt"]);
        let a = ui.app.panels[0].archive.as_ref().expect("a plugin view");
        assert_eq!(plugin_banner(&ui.app.plugins.name_of(a.plugin.id()), &a.file), format!("{} memserver · mem://test-ui", sal_ui::mdi::PUZZLE_OUTLINE));
        ui.frames(2);

        // copy in (F5 from the other panel), copy out, new folder, delete
        ui.app.panels[1].reload(ui.app.vis());
        cursor_to(&mut ui, "místní 🦎.txt");
        ui.key(Key::F5);
        let Dialog::Transfer { target, .. } = &ui.app.dialog else { panic!("no copy dialog") };
        assert_eq!(target, "mem://test-ui/home/");
        ui.key(Key::Enter);
        ui.wait_job();
        assert_eq!(server.files.lock().unwrap().get("home\\místní 🦎.txt").map(Vec::as_slice), Some(&b"z disku"[..]));
        ui.app.panels[0].reload(ui.app.vis());
        assert_eq!(names(&ui, 0), ["..", "docs", "místní 🦎.txt", "vzdálený.txt"]);
        ui.app.active = 0;
        cursor_to(&mut ui, "vzdálený.txt");
        ui.key(Key::F5);
        ui.key(Key::Enter);
        ui.wait_job();
        assert_eq!(fs::read_to_string(local.join("vzdálený.txt")).unwrap(), "ze serveru");
        ui.key(Key::F7);
        ui.text("nová");
        ui.key(Key::Enter);
        ui.wait_job();
        assert!(names(&ui, 0).contains(&"nová".to_owned()), "{:?}", names(&ui, 0));
        cursor_to(&mut ui, "docs");
        ui.key(Key::F8);
        ui.key(Key::Enter);
        ui.wait_job();
        assert!(!server.files.lock().unwrap().contains_key("home\\docs\\a.txt"));
        // .. goes up inside the connection; its root has no ..
        ui.key(Key::Home);
        ui.key(Key::Enter);
        assert_eq!(ui.app.panels[0].path(), root.as_path());
        assert_ne!(names(&ui, 0).first().map(String::as_str), Some(".."));

        // the drive selector lists the connection; choosing it opens where it started
        ui.app.command(&ctx, Cmd::DriveMenu(1));
        let Dialog::Drive { drives, conns, cursor, .. } = &mut ui.app.dialog else { panic!("drive menu not open") };
        // other tests' connections may be listed too (they share the list)
        let mine: Vec<(&str, &str)> = conns.iter().map(|c| (c.label.as_str(), c.detail.as_str())).filter(|(l, _)| l.starts_with("mem://")).collect();
        assert_eq!(mine, [("mem://test-ui", "memserver")]);
        *cursor = drives.len() + conns.iter().position(|c| c.label == "mem://test-ui").unwrap();
        ui.frames(2);
        ui.key(Key::Enter);
        assert_eq!(ui.app.panels[1].path(), root.join("home"), "the connection's start folder");
        // in that panel, the selector starts on the connection
        ui.app.command(&ctx, Cmd::DriveMenu(1));
        let Dialog::Drive { drives, cursor, .. } = &ui.app.dialog else { panic!("drive menu not open") };
        assert_eq!(*cursor, drives.len());
        ui.key(Key::Escape);
        ui.app.panels[1].load(&local, ui.app.vis(), None).unwrap();

        // a second session: both listed; one is disconnected from the drive selector
        ui.app.run_plugin_command(&ctx, plugin.clone(), "connect".into(), plugins::Values::new());
        wait_plugin(&mut ui);
        let Dialog::PluginForm { values, .. } = &mut ui.app.dialog else { panic!("no form") };
        values.insert("host".into(), "test-ui-2".into());
        values.insert("password".into(), "tajné".into());
        ui.key(Key::Enter);
        wait_plugin(&mut ui);
        let second = sal_plugin_api::mount_root("mem", "test-ui-2");
        assert_eq!(ui.app.panels[1].path(), second.join("home"));
        ui.app.command(&ctx, Cmd::DriveMenu(1));
        ui.frames(2);
        let Dialog::Drive { conns, .. } = &ui.app.dialog else { panic!("drive menu not open") };
        // (other tests running at the same time may have connections of their own)
        let mem = |conns: &[DriveConn]| conns.iter().map(|c| c.label.clone()).filter(|l| l.starts_with("mem://")).collect::<Vec<_>>();
        assert_eq!(mem(conns), ["mem://test-ui", "mem://test-ui-2"]);
        ui.key(Key::Delete);
        let Dialog::Drive { conns, .. } = &ui.app.dialog else { panic!("the selector stays open") };
        assert_eq!(mem(conns), ["mem://test-ui"]);
        assert!(plugins::mount_root_of(&second).is_none());
        assert_eq!(ui.app.panels[1].path(), local.as_path(), "the panel left the closed session");
        assert_eq!(ui.app.panels[0].path(), root.as_path(), "the other session stays");
        ui.key(Key::Escape);

        // disconnecting returns the panel to where it was, and closes the connection
        ui.app.close_connection(&root);
        ui.app.command(&ctx, Cmd::DriveMenu(1));
        assert!(matches!(&ui.app.dialog, Dialog::Drive { conns, .. } if mem(conns).is_empty()), "gone from the drive selector");
        ui.key(Key::Escape);
        assert_eq!(ui.app.panels[0].path(), local.as_path());
        assert!(plugins::mount_root_of(&root).is_none());
        // the plugin closes both sessions (in the background; slow when
        // the machine is busy)
        for _ in 0..1000 {
            if server.closed.lock().unwrap().len() == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut closed = server.closed.lock().unwrap().clone();
        closed.sort();
        assert_eq!(closed, [root.clone(), second.clone()]);
        // the connection's path is no longer open
        assert!(ui.app.panels[0].load(&root, ui.app.vis(), None).is_err());
    }

    /// Everything still works when the plugin or its 7z.dll is missing or
    /// damaged: archives are ordinary files then, and Settings says why.
    #[test]
    fn archives_without_the_plugin_or_its_library() {
        if built_library().is_none() {
            return;
        }
        for (case, plugin, lib) in [("no-plugin", false, None), ("no-7z-dll", true, None), ("bad-7z-dll", true, Some(false))] {
            let dir = plugin_folder(case, plugin, lib).unwrap();
            let t = tempfile::tempdir().unwrap();
            let arc = make_archive(t.path(), "zip");
            let before = fs::read(&arc).unwrap();
            let src = t.path().join("src");
            fs::create_dir_all(&src).unwrap();
            fs::write(src.join("přidaný.txt"), "nový").unwrap();
            let mut ui = Ui::new(&src, t.path());
            ui.app.set_language("cs");
            ui.app.plugins = Plugins::scan(Some(&dir));
            ui.app.apply_plugins();

            // what Settings → Plugins shows
            let p = &ui.app.plugins;
            assert!(p.active.is_empty() && p.broken.is_empty(), "{case}: {:?}", p.broken);
            let mine = display_path(&dir.join("7zip"));
            match case {
                "no-plugin" => assert!(p.found.is_empty() && p.errors.is_empty(), "{case}"),
                "no-7z-dll" => assert_eq!(p.errors, [("7zip".to_owned(), format!("V {mine} chybí 7z.dll."))], "the plugin's own Czech text"),
                _ => assert!(p.errors.len() == 1 && p.errors[0].1.starts_with(&format!("{mine}\\7z.dll: ")), "{case}: {:?}", p.errors),
            }
            if plugin {
                assert_eq!(p.found[0].tr(&p.found[0].info.name), "Archivy 7-Zip");
            }
            ui.app.open_dialog(Dialog::Settings { tab: SettingsTab::Plugins, recording: None });
            ui.frames(3);
            ui.key(Key::Escape);
            assert!(matches!(ui.app.dialog, Dialog::None), "{case}: settings closed");
            ui.keep_recorder();

            // a panel remembered inside the archive opens the archive's folder
            let panel = Panel::new(&arc.join("docs").to_string_lossy(), ui.app.panels[0].sort, ui.app.vis(), ui.app.plugins.active.clone());
            assert_eq!(panel.path(), t.path(), "{case}");
            assert_eq!(panel.entries()[panel.cursor].name, "archiv ž.zip", "{case}");
            // a typed path into it fails, the panel stays
            assert!(ui.app.panels[1].load(&arc.join("docs"), ui.app.vis(), None).is_err(), "{case}");
            assert_eq!(ui.app.panels[1].path(), t.path());

            // Enter hands the archive to Windows, like any file
            ui.app.active = 1;
            cursor_to(&mut ui, "archiv ž.zip");
            OPENED.with(|o| o.borrow_mut().clear());
            ui.key(Key::Enter);
            if let Dialog::Message { text, .. } = &ui.app.dialog {
                panic!("{case}: {text}");
            }
            assert_eq!(OPENED.with(|o| o.borrow().clone()), std::slice::from_ref(&arc), "{case}");
            assert_eq!(ui.app.panels[1].path(), t.path(), "{case}: not opened as a folder");
            // F3 shows the archive file itself
            ui.key(Key::F3);
            assert_eq!(ui.viewed(), std::slice::from_ref(&arc), "{case}");

            // copying "into" it is refused with a message; the archive stays as it was
            ui.app.active = 0;
            cursor_to(&mut ui, "přidaný.txt");
            ui.key(Key::F5);
            let Dialog::Transfer { target, .. } = &mut ui.app.dialog else { panic!("{case}: no copy dialog") };
            *target = format!("{}\\", arc.display());
            ui.key(Key::Enter);
            let Dialog::Message { text, .. } = &ui.app.dialog else { panic!("{case}: no message") };
            assert_eq!(*text, format!("{} je soubor, ne složka.", display_path(&arc)), "{case}");
            assert!(ui.app.jobs.is_idle(), "{case}: nothing started");
            ui.key(Key::Enter);
            // nor into a folder "inside" it
            ui.key(Key::F5);
            let Dialog::Transfer { target, .. } = &mut ui.app.dialog else { panic!("{case}: no copy dialog") };
            *target = arc.join("docs").display().to_string();
            ui.key(Key::Enter);
            assert!(matches!(&ui.app.dialog, Dialog::Message { text, .. } if text.contains("je soubor")), "{case}");
            ui.key(Key::Enter);
            assert_eq!(fs::read(&arc).unwrap(), before, "{case}: archive unchanged");
            assert!(src.join("přidaný.txt").is_file());
        }
    }

    #[test]
    fn queued_operation_clears_the_selection_at_once() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = (t.path().join("src"), t.path().join("dst"));
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        for n in ["a.txt", "b.txt", "c.txt"] {
            fs::write(src.join(n), n).unwrap();
        }
        let mut ui = Ui::new(&src, &dst);
        let ctx = ui.ctx.clone();
        // something already running, so the new operation has to wait
        ui.app.jobs.add_paused(&ctx, JobKind::Transfer(OpKind::Copy), vec![src.join("c.txt")], dst.clone());
        ui.app.panels[0].select_all(true);
        let selected = |ui: &Ui| ui.app.panels[0].entries().iter().filter(|e| e.selected).count();
        assert_eq!(selected(&ui), 3);
        ui.app.queue_place = Placement::Last;
        ui.app.start_job(&ctx, JobKind::Transfer(OpKind::Copy), vec![src.join("a.txt"), src.join("b.txt")], dst.clone());
        assert_eq!(ui.app.jobs.queue.len(), 1, "waits in the queue");
        assert_eq!(selected(&ui), 1, "a.txt and b.txt unselected at once, c.txt stays");
        ui.app.jobs.running[0].cancel();
        ui.wait_job();
    }

    /// A conflict while the operations window (with a queue) is open: both
    /// windows stay the same windows frame after frame. They used to be
    /// closed and reopened every frame, fighting over the focus.
    #[test]
    fn operation_windows_stay_open_during_a_conflict() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = (t.path().join("src"), t.path().join("dst"));
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("a.txt"), "new").unwrap();
        fs::write(dst.join("a.txt"), "old").unwrap();
        let mut ui = Ui::new(&src, &dst);
        let ctx = ui.ctx.clone();
        let copy = JobKind::Transfer(OpKind::Copy);
        ui.app.jobs.add(&ctx, copy, vec![src.join("a.txt")], dst.clone(), Placement::Now);
        ui.app.jobs.add(&ctx, copy, vec![src.join("a.txt")], t.path().join("later"), Placement::Last);
        for _ in 0..300 {
            ui.frames(1);
            if ui.app.jobs.running.first().is_some_and(|j| j.prompt.is_some()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        ui.frames(2);
        let serials = |ui: &Ui| ["job", "prompt"].map(|k| dialogwin::serial(&ui.app.dialog_places, k));
        let before = serials(&ui);
        assert!(before.iter().all(Option::is_some), "both windows are open: {before:?}");
        ui.frames(10);
        assert_eq!(serials(&ui), before, "the same windows, not reopened");
        // a regular dialog opening and closing leaves them alone too
        ui.app.open_dialog(Dialog::Message { title: "x".into(), text: "y".into() });
        ui.frames(2);
        ui.app.dialog = Dialog::None;
        ui.frames(2);
        assert_eq!(serials(&ui), before);
        ui.app.jobs.running[0].cancel();
        ui.wait_job();
    }

    #[test]
    fn mkdir_nested_path_and_narrow_window() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.key(Key::F7);
        // narrower than the main window, however long the text gets
        ui.text(&"x".repeat(150));
        let w = ui.ctx.memory(|m| m.area_rect(egui::Id::new(("dialog", "mkdir")))).expect("dialog window").width();
        assert!(w < 500.0, "dialog is {w} wide");
        ui.key(Key::Escape);
        // a leading slash means "inside the current folder", not the drive root
        ui.key(Key::F7);
        ui.text("/foo1/foo2/foo3");
        ui.key(Key::Enter);
        assert!(t.path().join("foo1").join("foo2").join("foo3").is_dir());
        assert_eq!(ui.cursor_name(), "foo1");
        // backslashes work too, and existing levels are fine
        ui.key(Key::F7);
        ui.text(r"foo1\bar\baz\");
        ui.key(Key::Enter);
        assert!(t.path().join(r"foo1\bar\baz").is_dir());
    }

    #[test]
    fn quick_search_select_and_copy() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = (t.path().join("src"), t.path().join("dst"));
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        for n in ["b.txt", "日本語.txt", "Ελληνικά.md"] {
            fs::write(src.join(n), n).unwrap();
        }
        let mut ui = Ui::new(&src, &dst);
        ui.text("日");
        assert_eq!(ui.cursor_name(), "日本語.txt");
        ui.key(Key::Insert);
        ui.text("ελ"); // case-insensitive Greek quick search
        assert_eq!(ui.cursor_name(), "Ελληνικά.md");
        ui.key(Key::Insert);
        ui.key(Key::F5);
        assert!(matches!(ui.app.dialog, Dialog::Transfer { .. }));
        ui.key(Key::Enter);
        ui.wait_job();
        assert_eq!(fs::read_to_string(dst.join("日本語.txt")).unwrap(), "日本語.txt");
        assert!(dst.join("Ελληνικά.md").exists());
        assert!(!dst.join("b.txt").exists());
        // the right panel picked up the new files
        assert!(ui.app.panels[1].entries().iter().any(|e| e.name == "日本語.txt"));
    }

    #[test]
    fn background_and_safe_copy_show_the_result() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = (t.path().join("src"), t.path().join("dst"));
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("a.txt"), "abc").unwrap();
        let mut ui = Ui::new(&src, &dst);
        ui.text("a");
        // a plain copy: no result window
        ui.key(Key::F5);
        ui.key(Key::Enter);
        ui.wait_job();
        assert!(ui.app.reports.is_empty());
        // in the background: the progress window is hidden at once, no result
        fs::remove_file(dst.join("a.txt")).unwrap();
        ui.key(Key::F5);
        let Dialog::Transfer { opts, .. } = &mut ui.app.dialog else { panic!("no copy dialog") };
        opts.background = true;
        ui.key(Key::Enter);
        assert!(ui.app.jobs.is_idle() || ui.app.jobs.hidden, "in the background");
        ui.wait_job();
        ui.frames(2);
        assert!(ui.app.reports.is_empty(), "the result only when asked");
        // the result asked for: shown at the end
        fs::remove_file(dst.join("a.txt")).unwrap();
        ui.key(Key::F5);
        let Dialog::Transfer { opts, .. } = &mut ui.app.dialog else { panic!("no copy dialog") };
        assert!(!opts.background, "not remembered: in the background only when asked each time");
        opts.show_result = true;
        ui.key(Key::Enter);
        ui.wait_job();
        ui.frames(2);
        assert_eq!(ui.app.reports.len(), 1);
        ui.app.reports.clear();
        // a safe copy always shows it, and writes the checksums to both sides
        fs::remove_file(dst.join("a.txt")).unwrap();
        ui.key(Key::F5);
        let Dialog::Transfer { opts, .. } = &mut ui.app.dialog else { panic!("no copy dialog") };
        assert!(!opts.show_result, "not remembered");
        opts.verify = Verify::Checksum;
        ui.key(Key::Enter);
        ui.wait_job();
        ui.frames(2);
        let [r] = ui.app.reports.as_slice() else { panic!("one result: {}", ui.app.reports.len()) };
        let s = r.result.as_ref().unwrap();
        assert_eq!((s.files, s.verified, s.verify), (1, 1, Verify::Checksum));
        assert_eq!(s.checksum_files.len(), 2);
        assert!(s.checksum_files[0].starts_with(&dst) && s.checksum_files[1].starts_with(&src), "{:?}", s.checksum_files);
        let listed = fs::read_to_string(&s.checksum_files[0]).unwrap();
        assert!(listed.ends_with(" *a.txt\n"), "{listed}");
        let sums = s.checksum_files.clone();
        // the protocol: what, where, the check, and every file's checksum
        let protocol = t.path().join("protocol.txt");
        ui.app.write_protocol(&protocol);
        assert_eq!(ui.app.protocol_saved.as_deref(), Some(protocol.as_path()));
        let text = fs::read_to_string(&protocol).unwrap();
        assert!(text.starts_with('\u{feff}'), "UTF-8 with a BOM");
        for want in ["Copy \"a.txt\" to", "Status: Finished", "SHA-256 checksums match: 1 file", listed.trim_end()] {
            assert!(text.contains(want), "{want:?} in:\n{text}");
        }
        // the window stays as it is from frame to frame, not opened again
        let serial = dialogwin::serial(&ui.app.dialog_places, "report").expect("shown");
        ui.frames(3);
        assert_eq!(dialogwin::serial(&ui.app.dialog_places, "report"), Some(serial), "kept open");
        // Esc closes the result window
        ui.key(Key::Escape);
        assert!(ui.app.reports.is_empty());
        ui.frames(2);
        assert!(dialogwin::serial(&ui.app.dialog_places, "report").is_none());
        assert!(sums.iter().all(|p| p.exists()), "closing keeps the checksum files");
        // closing with the other button removes them
        ui.app.reports.push(Report { spec: Spec { id: 99, kind: JobKind::Transfer(OpKind::Copy), sources: vec![], target: dst.clone(), plugin: None, options: JobOptions::default() }, result: Ok(OpSummary { checksum_files: sums.clone(), ..Default::default() }), elapsed: Duration::ZERO, ended: String::new() });
        ui.app.remove_checksum_files();
        assert!(sums.iter().all(|p| !p.exists()), "removed");
        assert!(dst.join("a.txt").exists() && src.join("a.txt").exists(), "only the checksum files");
        ui.app.reports.clear();

        // the dialogs' choices are for that operation only: they open with Settings' defaults
        assert_eq!(ui.app.cfg.copy_defaults, TransferDefaults::default(), "not remembered");
        let moving = TransferDefaults { background: true, show_result: false, verify: Verify::Size };
        ui.app.cfg.move_defaults = moving;
        for (key, kind, want) in [(Key::F5, OpKind::Copy, TransferDefaults::default()), (Key::F6, OpKind::Move, moving)] {
            ui.key(key);
            let Dialog::Transfer { kind: k, opts, .. } = &ui.app.dialog else { panic!("no dialog for {kind:?}") };
            assert_eq!((*k, *opts), (kind, want));
            assert_eq!(opts.reports(), kind == OpKind::Move, "a safe move always shows its result");
            ui.key(Key::Escape);
        }
    }

    /// A copy in the background that has to ask (the target exists) shows
    /// its progress window again beside the question.
    #[test]
    fn background_copy_that_asks_comes_forward() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = (t.path().join("src"), t.path().join("dst"));
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("a.txt"), "new").unwrap();
        fs::write(dst.join("a.txt"), "old").unwrap();
        let mut ui = Ui::new(&src, &dst);
        ui.text("a");
        ui.key(Key::F5);
        let Dialog::Transfer { opts, .. } = &mut ui.app.dialog else { panic!("no copy dialog") };
        opts.background = true;
        opts.show_result = true;
        ui.key(Key::Enter);
        assert!(ui.app.jobs.hidden, "in the background");
        for _ in 0..300 {
            ui.frames(1);
            if ui.app.jobs.running.first().is_some_and(|j| j.prompt.is_some()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        ui.frames(2);
        assert!(ui.app.jobs.running[0].prompt.is_some(), "asks");
        assert!(!ui.app.jobs.hidden, "its window is back with the question");
        ui.app.jobs.running[0].answer(Reply::Conflict(ConflictAction::Overwrite));
        ui.frames(2);
        assert!(ui.app.jobs.hidden || ui.app.jobs.is_idle(), "back in the background once answered");
        ui.wait_job();
        assert!(!ui.app.jobs.shown_for_prompt);
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "new");
        ui.frames(2);
        assert_eq!(ui.app.reports.len(), 1, "and the result at the end");
    }

    #[test]
    fn rename_move_and_delete() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("old.txt"), "x").unwrap();
        let mut ui = Ui::new(&a, &b);

        ui.text("old");
        ui.key(Key::F2);
        ui.key_mod(Key::A, Modifiers::COMMAND | Modifiers::CTRL); // select the prefilled name
        ui.text("přejmenováno ✓.txt");
        ui.key(Key::Enter);
        assert!(a.join("přejmenováno ✓.txt").exists());
        assert_eq!(ui.cursor_name(), "přejmenováno ✓.txt");

        ui.key(Key::F6);
        ui.key(Key::Enter);
        ui.wait_job();
        assert!(b.join("přejmenováno ✓.txt").exists());
        assert!(!a.join("přejmenováno ✓.txt").exists());

        ui.key(Key::Tab);
        assert_eq!(ui.app.active, 1);
        ui.text("př");
        ui.key_mod(Key::Delete, Modifiers::SHIFT);
        assert!(matches!(ui.app.dialog, Dialog::Delete { recycle: false, .. }));
        ui.key(Key::Enter);
        ui.wait_job();
        assert!(!b.join("přejmenováno ✓.txt").exists());
    }

    #[test]
    fn select_by_mask_and_navigation() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("složka");
        fs::create_dir_all(&d).unwrap();
        for n in ["a.txt", "b.TXT", "c.doc", "ž.txt"] {
            fs::write(d.join(n), "").unwrap();
        }
        let mut ui = Ui::new(&d, t.path());
        ui.text("+");
        assert!(matches!(ui.app.dialog, Dialog::SelectMask { select: true, .. }));
        ui.key_mod(Key::A, Modifiers::COMMAND | Modifiers::CTRL);
        ui.text("*.txt|ž*");
        ui.key(Key::Enter);
        let sel: Vec<_> = ui.app.panels[0].entries().iter().filter(|e| e.selected).map(|e| e.name.clone()).collect();
        assert_eq!(sel, ["a.txt", "b.TXT"]);

        // Backspace goes up and focuses the directory we came from
        ui.key(Key::Backspace);
        assert_eq!(ui.app.panels[0].path(), t.path());
        assert_eq!(ui.cursor_name(), "složka");
        ui.key(Key::Enter);
        assert_eq!(ui.app.panels[0].path(), d.as_path());
    }

    /// F3 and F4 go to the plugins registered as viewers and editors, with
    /// the program's look; without a viewer F3 says what to do.
    #[test]
    fn plugins_view_and_edit_files() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("ж.txt"), "Příliš 日本 😀").unwrap();
        fs::write(t.path().join("nastavení.edit"), "x").unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.app.cfg.theme = ThemeMode::Dark;
        ui.app.cfg.mono_font = "Cascadia Mono".into();
        ui.text("ж");
        ui.key(Key::F3);
        assert_eq!(ui.viewed(), [t.path().join("ж.txt")]);
        {
            let opened = ui.recorder.opened.lock().unwrap();
            let o = &opened[0].2;
            assert_eq!((o["theme"].as_str(), o["mono_font"].as_str(), o["temporary"].as_str()), ("dark", "Cascadia Mono", ""));
        }
        // F4: the plugin's editor, when chosen
        ui.app.cfg.edit_with = OpenWith::Plugin { plugin: "recorder".into(), opener: "edit".into() };
        ui.key(Key::Escape);
        ui.text("nas");
        ui.key(Key::F4);
        let last = ui.recorder.opened.lock().unwrap().last().cloned().unwrap();
        assert_eq!((last.0.as_str(), last.1), ("edit", t.path().join("nastavení.edit")));

        // no viewer: F3 explains
        ui.app.plugins.active.retain(|p| p.id() != "recorder");
        ui.key(Key::Escape);
        ui.text("ж");
        ui.key(Key::F3);
        assert!(matches!(&ui.app.dialog, Dialog::Message { text, .. } if text.contains("Viewer plugin")), "{:?}", ui.app.status);
    }

    /// Press `key` with modifiers, plus the text a real keyboard would type.
    fn chord(ui: &mut Ui, key: Key, m: Modifiers, text: Option<&str>) {
        let mut ev = vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: m }];
        if let Some(t) = text {
            ev.push(Event::Text(t.into()));
        }
        ev.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: m });
        ui.run(ev, m);
        ui.frames(2);
    }

    fn letter_key(c: char) -> Key {
        Key::from_name(&c.to_string()).unwrap()
    }

    /// Another accessible fixed drive than the one holding the temp dir.
    fn other_drive(here: &Path) -> Option<Drive> {
        let root = drives::root_of(here);
        drives::list().into_iter().find(|d| d.root != root && d.kind == drives::DriveType::Fixed && fs::read_dir(&d.root).is_ok())
    }

    #[test]
    fn root_shortcut_default_and_reconfigured() {
        let t = tempfile::tempdir().unwrap();
        let deep = t.path().join("a").join("b");
        fs::create_dir_all(&deep).unwrap();
        let mut ui = Ui::new(&deep, &deep);
        chord(&mut ui, Key::Backspace, Modifiers::CTRL, None);
        assert_eq!(ui.app.panels[0].path(), drives::root_of(&deep));

        keymap::set_bindings(&mut ui.app.cfg.shortcuts, Action::Root, vec!["Alt+R".into()]);
        ui.app.panels[0].load(&deep, Visibility::ALL, None).unwrap();
        chord(&mut ui, Key::Backspace, Modifiers::CTRL, None);
        assert_eq!(ui.app.panels[0].path(), deep.as_path(), "old shortcut no longer bound");
        chord(&mut ui, Key::R, Modifiers::ALT, None);
        assert_eq!(ui.app.panels[0].path(), drives::root_of(&deep));
    }

    #[test]
    fn drive_letter_shortcut_remembers_directory() {
        let t = tempfile::tempdir().unwrap();
        let Some(other) = other_drive(t.path()) else { return }; // needs a second drive
        let here = drives::root_of(t.path());
        let (other_letter, here_letter) = (other.letter.chars().next().unwrap(), here.to_string_lossy().chars().next().unwrap().to_ascii_uppercase());
        let mut ui = Ui::new(t.path(), t.path());

        chord(&mut ui, letter_key(other_letter), Modifiers::SHIFT, Some(&other_letter.to_string()));
        assert_eq!(drives::root_of(ui.app.panels[0].path()), other.root);
        assert!(ui.app.panels[0].qs.is_empty(), "the letter must not start a quick search");

        // back: returns to the directory we left, not the root
        chord(&mut ui, letter_key(here_letter), Modifiers::SHIFT, Some(&here_letter.to_string()));
        assert_eq!(ui.app.panels[0].path(), t.path());

        ui.app.cfg.drive_remember_dir = false;
        chord(&mut ui, letter_key(other_letter), Modifiers::SHIFT, Some(&other_letter.to_string()));
        chord(&mut ui, letter_key(here_letter), Modifiers::SHIFT, Some(&here_letter.to_string()));
        assert_eq!(ui.app.panels[0].path(), here.as_path());
    }

    #[test]
    fn shift_letter_falls_back_to_quick_search() {
        let t = tempfile::tempdir().unwrap();
        for n in ["Quarterly.xlsx", "Qx.txt"] {
            fs::write(t.path().join(n), "").unwrap();
        }
        let mut ui = Ui::new(t.path(), t.path());
        let missing = ('A'..='Z').rev().find(|c| !drives::list().iter().any(|d| d.letter.starts_with(*c))).unwrap();
        // no such drive: the letter types into quick search
        ui.app.panels[0].load(t.path(), Visibility::ALL, None).unwrap();
        chord(&mut ui, letter_key(missing), Modifiers::SHIFT, Some(&missing.to_string()));
        assert_eq!(ui.app.panels[0].path(), t.path());

        // during a quick search, Shift+letter extends it even if the drive exists
        ui.text("q");
        chord(&mut ui, Key::X, Modifiers::SHIFT, Some("X"));
        assert_eq!(ui.app.panels[0].qs, "qX");
        assert_eq!(ui.cursor_name(), "Qx.txt");

        // disabled: never switches
        ui.app.cfg.drive_hotkey = DriveHotkey::Disabled;
        ui.key(Key::Escape);
        let c = drives::root_of(t.path()).to_string_lossy().chars().next().unwrap().to_ascii_uppercase();
        chord(&mut ui, letter_key(c), Modifiers::SHIFT, Some(&c.to_string()));
        assert_eq!(ui.app.panels[0].path(), t.path());
    }

    #[test]
    fn settings_records_custom_shortcut() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.app.command(&ui.ctx.clone(), Cmd::Settings);
        ui.frames(2);
        let Dialog::Settings { tab, recording } = &mut ui.app.dialog else { panic!("settings not open") };
        *tab = SettingsTab::Keyboard;
        *recording = Some(Action::Root);
        ui.frames(1);
        chord(&mut ui, Key::F9, Modifiers::CTRL | Modifiers::SHIFT, None);
        assert_eq!(keymap::bindings(&ui.app.cfg.shortcuts, Action::Root), ["Ctrl+Backspace", "Ctrl+Backslash", "Ctrl+Shift+F9"]);
        assert!(matches!(ui.app.dialog, Dialog::Settings { recording: None, .. }), "dialog stays open");
        // Esc while recording cancels without closing or changing anything
        if let Dialog::Settings { recording, .. } = &mut ui.app.dialog {
            *recording = Some(Action::Copy);
        }
        ui.key(Key::Escape);
        assert_eq!(keymap::bindings(&ui.app.cfg.shortcuts, Action::Copy), ["F5"]);
        assert!(matches!(ui.app.dialog, Dialog::Settings { .. }));
        ui.key(Key::Escape);
        assert!(matches!(ui.app.dialog, Dialog::None));

        // the recorded shortcut works
        let deep = t.path().join("x");
        fs::create_dir_all(&deep).unwrap();
        ui.app.panels[0].load(&deep, Visibility::ALL, None).unwrap();
        chord(&mut ui, Key::F9, Modifiers::CTRL | Modifiers::SHIFT, None);
        assert_eq!(ui.app.panels[0].path(), drives::root_of(&deep));
    }

    #[test]
    fn rebinding_and_disabling_commands() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("a.txt"), "").unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.text("a");
        keymap::set_bindings(&mut ui.app.cfg.shortcuts, Action::Copy, vec!["Ctrl+K".into()]);
        ui.key(Key::F5);
        assert!(matches!(ui.app.dialog, Dialog::None), "F5 is no longer bound");
        chord(&mut ui, Key::K, Modifiers::CTRL, None);
        assert!(matches!(ui.app.dialog, Dialog::Transfer { kind: OpKind::Copy, .. }));
        ui.key(Key::Escape);

        // preset "None (disabled)" switches the shortcut off
        let none = keymap::presets(Action::MkDir).into_iter().find(|(l, _)| l.starts_with("None")).unwrap().1;
        keymap::set_bindings(&mut ui.app.cfg.shortcuts, Action::MkDir, none);
        ui.key(Key::F7);
        assert!(matches!(ui.app.dialog, Dialog::None));

        // a fixed key can be overridden: bind Tab to Refresh
        keymap::set_bindings(&mut ui.app.cfg.shortcuts, Action::Refresh, vec!["Tab".into()]);
        ui.key(Key::Tab);
        assert_eq!(ui.app.active, 0, "Tab now refreshes instead of switching panels");
    }

    #[test]
    fn legacy_root_shortcut_is_migrated() {
        let t = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let cfg = Config { root_shortcut: Some("alt+r".into()), ..Config::default() };
        let app = App::with_config(&ctx, cfg, vec![t.path().to_string_lossy().into()]);
        assert_eq!(keymap::bindings(&app.cfg.shortcuts, Action::Root), ["Alt+R", "Ctrl+Backslash"]);
        let cfg = Config { root_shortcut: Some("Ctrl+Backspace".into()), ..Config::default() };
        let app = App::with_config(&ctx, cfg, vec![t.path().to_string_lossy().into()]);
        assert!(app.cfg.shortcuts.is_empty(), "the old default needs no override");
    }

    #[test]
    fn all_drive_buttons_mode_renders() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.app.cfg.drive_buttons = DriveButtons::All;
        ui.frames(3);
        assert!(!ui.app.drives.is_empty());
    }

    #[test]
    fn drive_menu_lists_local_drives_first() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.key_mod(Key::F1, Modifiers::ALT);
        let Dialog::Drive { drives, cursor, .. } = &ui.app.dialog else { panic!("drive menu not open") };
        assert!(!drives.is_empty());
        let locality: Vec<bool> = drives.iter().map(|d| d.kind.is_local()).collect();
        assert!(locality.windows(2).all(|w| w[0] >= w[1]), "local before network: {locality:?}");
        // the cursor starts on the panel's own drive
        assert_eq!(drives[*cursor].root, drives::root_of(t.path()));
        ui.frames(2);
    }

    /// Drag `from` → `to` with the button held, `m` held at the end.
    fn drag(ui: &mut Ui, from: egui::Pos2, to: egui::Pos2, m: Modifiers) {
        ui.run(vec![Event::PointerMoved(from)], Modifiers::NONE);
        ui.run(vec![Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }], Modifiers::NONE);
        for i in 1..=6 {
            ui.run(vec![Event::PointerMoved(from + (to - from) * (i as f32 / 6.0))], m);
        }
        ui.run(vec![Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: m }], m);
        ui.frames(2);
    }

    /// The middle of the row of `name` in panel `idx`.
    fn row_of(ui: &Ui, idx: usize, name: &str) -> egui::Pos2 {
        let i = ui.app.panels[idx].listing.entries.iter().position(|e| e.name == name).unwrap_or_else(|| panic!("no {name}"));
        ui.app.row_hits[idx].iter().find(|(_, k)| *k == i).map(|(r, _)| r.center()).expect("row shown")
    }

    /// Items dragged from a panel: onto the other panel, onto a folder's row,
    /// the same drive moves, Ctrl copies, nothing where they are already; the
    /// selection goes as a whole.
    #[test]
    fn drag_and_drop_between_panels() {
        let t = tempfile::tempdir().unwrap();
        let (left, right) = (t.path().join("vlevo"), t.path().join("vpravo"));
        fs::create_dir_all(left.join("složka")).unwrap();
        fs::create_dir_all(right.join("cíl")).unwrap();
        for n in ["a.txt", "b.txt", "c.txt"] {
            fs::write(left.join(n), n).unwrap();
        }
        let mut ui = Ui::new(&left, &right);
        ui.frames(2);
        let target = |ui: &Ui| match &ui.app.dialog {
            Dialog::Transfer { kind, sources, target, .. } => Some((*kind, sources.iter().map(|s| s.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>(), target.clone())),
            _ => None,
        };
        // onto the other panel (same drive): Move, its folder
        let (from, to) = (row_of(&ui, 0, "a.txt"), ui.app.body_rects[1].center());
        drag(&mut ui, from, to, Modifiers::NONE);
        assert_eq!(target(&ui), Some((OpKind::Move, vec!["a.txt".to_owned()], transfer_target(&right))));
        ui.key(Key::Escape);
        // Ctrl copies
        drag(&mut ui, from, to, Modifiers::CTRL);
        assert_eq!(target(&ui).map(|t| t.0), Some(OpKind::Copy));
        ui.key(Key::Escape);
        // onto a folder's row: that folder
        let to = row_of(&ui, 1, "cíl");
        drag(&mut ui, from, to, Modifiers::NONE);
        assert_eq!(target(&ui).map(|t| t.2), Some(transfer_target(&right.join("cíl"))));
        ui.key(Key::Escape);
        // onto a folder in its own panel; onto its own panel: nothing
        let to = row_of(&ui, 0, "složka");
        drag(&mut ui, from, to, Modifiers::NONE);
        assert_eq!(target(&ui).map(|t| t.2), Some(transfer_target(&left.join("složka"))));
        ui.key(Key::Escape);
        let to = ui.app.body_rects[0].center_bottom() - egui::vec2(0.0, 10.0);
        drag(&mut ui, from, to, Modifiers::NONE);
        assert_eq!(target(&ui), None, "where it is already");
        // the selection, dragged by one of its rows
        for n in ["b.txt", "c.txt"] {
            let i = ui.app.panels[0].listing.entries.iter().position(|e| e.name == n).unwrap();
            ui.app.panels[0].listing.entries[i].selected = true;
        }
        ui.frames(1);
        let (from, to) = (row_of(&ui, 0, "c.txt"), ui.app.body_rects[1].center());
        drag(&mut ui, from, to, Modifiers::SHIFT);
        assert_eq!(target(&ui).map(|t| (t.0, t.1)), Some((OpKind::Move, vec!["b.txt".to_owned(), "c.txt".to_owned()])));
        ui.key(Key::Escape);
        // without the confirmation: done at once
        ui.app.cfg.confirm_drop = false;
        drag(&mut ui, from, to, Modifiers::CTRL);
        ui.wait_job();
        assert!(right.join("b.txt").is_file() && left.join("b.txt").is_file(), "copied");
        assert!(ui.app.panel_drag.is_none());
    }

    /// The drive selector's last row (Alt+F1 left, Alt+F2 right) takes the
    /// other panel's folder.
    #[test]
    fn drive_menu_goes_to_the_other_panels_folder() {
        let t = tempfile::tempdir().unwrap();
        let (left, right) = (t.path().join("levý"), t.path().join("pravý").join("hlubší"));
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        let mut ui = Ui::new(&left, &right);
        for (key, panel, from) in [(Key::F1, 0, &right), (Key::F2, 1, &left)] {
            // the other panel somewhere else first
            ui.app.panels[1 - panel].load(from, ui.app.vis(), None).unwrap();
            ui.key_mod(key, Modifiers::ALT);
            let Dialog::Drive { drives, conns, paths, .. } = &ui.app.dialog else { panic!("drive menu not open") };
            let rows = drives.len() + conns.len() + paths.len() + 1;
            ui.frames(2);
            for _ in 0..rows {
                ui.key(Key::ArrowDown);
            }
            let Dialog::Drive { cursor, .. } = &ui.app.dialog else { panic!("drive menu closed") };
            assert_eq!(*cursor, rows - 1, "the last row");
            ui.key(Key::Enter);
            assert!(matches!(ui.app.dialog, Dialog::None));
            assert_eq!(ui.app.panels[panel].path(), from.as_path(), "panel {panel}");
            // back, for the next round
            ui.app.panels[panel].load(t.path(), ui.app.vis(), None).unwrap();
        }
    }

    #[test]
    fn drive_icons_draw_for_every_type() {
        use sal_core::drives::DriveType;
        let kinds = [DriveType::Fixed, DriveType::Network, DriveType::Removable, DriveType::CdRom, DriveType::RamDisk, DriveType::Unknown];
        for dark in [true, false] {
            let ctx = egui::Context::default();
            ctx.set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
            let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), ..Default::default() };
            let out = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    for k in kinds {
                        drive_icon::button(ui, k, "X:", false);
                        drive_icon::button(ui, k, "", true);
                        drive_icon::row(ui, k, "X:", "label", k.label(), false);
                    }
                });
            });
            assert!(out.shapes.len() > kinds.len() * 3, "icons produced shapes");
        }
        // local and network are visually distinct
        assert_ne!(drive_icon::color(DriveType::Fixed), drive_icon::color(DriveType::Network));
    }

    /// Renders the drive UI to PNGs for visual review:
    /// `cargo test -p sal-app snapshot_drive_icons -- --ignored`
    #[test]
    #[ignore]
    fn snapshot_drive_icons() {
        use sal_core::drives::DriveType;
        let fake = |l: char, kind, label: &str, remote: Option<&str>| Drive {
            root: PathBuf::from(format!("{l}:\\")),
            letter: format!("{l}:"),
            label: label.into(),
            kind,
            remote: remote.map(Into::into),
        };
        let t = tempfile::tempdir().unwrap();
        let out_dir = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        for (dark, name) in [(true, "drives_dark.png"), (false, "drives_light.png")] {
            let mut ui = Ui::new(t.path(), t.path());
            crate::fonts::install(&ui.ctx, "", "");
            ui.ctx.set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
            ui.ctx.set_pixels_per_point(1.5);
            let real_c = drives::root_of(t.path());
            let mut list = vec![
                Drive { root: real_c.clone(), ..fake('C', DriveType::Fixed, "System", None) },
                fake('D', DriveType::Fixed, "Data", None),
                fake('E', DriveType::Removable, "USB STICK", None),
                fake('F', DriveType::CdRom, "", None),
                fake('R', DriveType::RamDisk, "RAMDISK", None),
                fake('X', DriveType::Network, "projects", Some(r"\\fileserver\projects")),
                fake('Z', DriveType::Network, "backup", Some(r"\\nas\backup")),
            ];
            ui.app.drives = list.clone();
            ui.app.drives_loaded = Some(Instant::now() + Duration::from_secs(60));
            ui.app.cfg.drive_buttons = DriveButtons::All;
            list.sort_by_key(|d| (!d.kind.is_local(), d.letter.clone()));
            ui.app.dialog =
                Dialog::Drive { panel: 0, drives: list, conns: Vec::new(), paths: vec![r"D:\Projekty\Účetnictví 2026".into(), r"\\nas\data\fotky".into()], cursor: 5 };
            ui.frames(20); // past the popup's fade-in
            let bg = ui.ctx.style().visuals.panel_fill;
            let out = ui.last.take().unwrap();
            ui.snap.render(&ui.ctx, out, bg, &out_dir.join(name));
        }
    }

    /// The path bars with all drive buttons, the real drives, at a few
    /// scales: `cargo test -p sal-app snapshot_all_drive_buttons -- --ignored`
    #[test]
    #[ignore]
    fn snapshot_all_drive_buttons() {
        let t = tempfile::tempdir().unwrap();
        let out_dir = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        for ppp in [1.0f32, 1.25, 1.5] {
            let mut ui = Ui::new(t.path(), t.path());
            crate::fonts::install(&ui.ctx, "", "");
            ui.ctx.set_pixels_per_point(ppp);
            ui.screen = egui::vec2(1000.0, 300.0);
            ui.app.cfg.drive_buttons = DriveButtons::All;
            ui.ctx.set_visuals(egui::Visuals::light());
            let real_c = drives::root_of(t.path());
            let fake = |l: char, kind| Drive { root: PathBuf::from(format!("{l}:\\")), letter: format!("{l}:"), label: String::new(), kind, remote: None };
            use sal_core::drives::DriveType;
            ui.app.drives = vec![Drive { root: real_c, ..fake('C', DriveType::Fixed) }, fake('D', DriveType::Fixed), fake('E', DriveType::Removable), fake('X', DriveType::Network), fake('Y', DriveType::Network), fake('Z', DriveType::Network)];
            ui.app.drives_loaded = Some(Instant::now() + Duration::from_secs(60));
            ui.frames(4);
            let bg = ui.ctx.style().visuals.panel_fill;
            let out = ui.last.take().unwrap();
            ui.snap.render(&ui.ctx, out, bg, &out_dir.join(format!("all_drives_{ppp}.png")));
        }
    }

    /// Columns: Attr just fits its letters and isn't resizable, Ext starts
    /// three letters wide; whatever column is dragged narrower or wider,
    /// Name takes the difference and the columns still end at the panel's
    /// right edge (no empty space after them).
    #[test]
    fn resized_columns_give_name_the_room() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("a.txt"), "x").unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        crate::fonts::install(&ui.ctx, "", "");
        ui.frames(3);
        let heads = |ui: &Ui| ui.app.header_rects[0].clone();
        let before = heads(&ui);
        assert_eq!(before.len(), 5);
        let font = egui::TextStyle::Body.resolve(&ui.ctx.style());
        let w = |s: &str| ui.ctx.fonts(|f| f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x);
        assert!(before[4].width() >= w("RHSACE") && before[4].width() < w("RHSACE") + w("Attr") , "Attr fits its letters: {}", before[4].width());
        assert!(before[1].width() >= w("www") && before[1].width() < w("wwwww"), "Ext about three letters: {}", before[1].width());
        let right_edge = before[4].right();

        // drag the edge between Size and Date (Date's left edge), left then right
        let gap = ui.ctx.style().spacing.item_spacing.x;
        let drag = |ui: &mut Ui, from: egui::Pos2, by: f32| {
            let to = from + egui::vec2(by, 0.0);
            ui.run(vec![Event::PointerMoved(from)], Modifiers::NONE);
            ui.run(vec![Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }], Modifiers::NONE);
            for i in 1..=5 {
                ui.run(vec![Event::PointerMoved(from + egui::vec2(by * i as f32 / 5.0, 0.0))], Modifiers::NONE);
            }
            ui.run(vec![Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }], Modifiers::NONE);
            ui.frames(3);
        };
        for by in [-30.0, 50.0] {
            let was = heads(&ui);
            let edge = egui::pos2(was[3].left() - gap / 2.0, was[3].center().y);
            drag(&mut ui, edge, by);
            let now = heads(&ui);
            assert!((now[3].width() - (was[3].width() - by)).abs() < 2.0, "Date resized by {by}: {} -> {}", was[3].width(), now[3].width());
            assert!((now[2].width() - was[2].width()).abs() < 0.5, "Size as it was");
            assert!((now[3].left() - gap / 2.0 - (edge.x + by)).abs() < 2.0, "the edge followed the pointer");
            assert!((now[0].width() - (was[0].width() + by)).abs() < 2.0, "Name takes the difference: {} -> {}", was[0].width(), now[0].width());
            assert!((now[4].right() - right_edge).abs() < 1.0, "no space after the columns: {} vs {right_edge}", now[4].right());
        }
    }

    /// Clicking a column header sorts the panel by that column, and again
    /// in reverse; anywhere in the header cell counts, not only its title.
    /// Sorted by size, folders go by their calculated sizes.
    #[test]
    fn header_clicks_sort() {
        let t = tempfile::tempdir().unwrap();
        let set_time = |p: &Path, secs: u64| fs::File::options().write(true).open(p).unwrap().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(secs)).unwrap();
        // files whose names, extensions, sizes and dates are in different orders
        for (name, size, secs) in [("b.txt", 300, 2_000_000), ("a.zip", 100, 3_000_000), ("c.doc", 200, 1_000_000)] {
            fs::write(t.path().join(name), vec![b'x'; size]).unwrap();
            set_time(&t.path().join(name), secs);
        }
        // folders: "aa" small, "zz" big
        fs::create_dir_all(t.path().join("aa")).unwrap();
        fs::write(t.path().join("aa").join("f"), vec![b'x'; 10]).unwrap();
        fs::create_dir_all(t.path().join("zz")).unwrap();
        fs::write(t.path().join("zz").join("f"), vec![b'x'; 10_000]).unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        crate::fonts::install(&ui.ctx, "", "");
        ui.frames(2);
        let names = |ui: &Ui| ui.app.panels[0].entries().iter().skip(1).map(|e| e.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&ui), ["aa", "zz", "a.zip", "b.txt", "c.doc"]);

        // the header row is the top row of the list: Name, Ext, Size, Date, Attr
        let heads = ui.app.header_rects[0].clone();
        assert_eq!(heads.len(), 5);
        let y = heads[0].center().y;
        let cell = |key: SortKey| {
            let r = match key {
                SortKey::Name => heads[0],
                SortKey::Ext => heads[1],
                SortKey::Size => heads[2],
                SortKey::Time => heads[3],
                SortKey::Attr => heads[4],
            };
            (r.left(), r.right())
        };
        let click = |ui: &mut Ui, x: f32| {
            let pos = egui::pos2(x, y);
            ui.run(vec![Event::PointerMoved(pos)], Modifiers::NONE);
            ui.run(vec![Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }], Modifiers::NONE);
            ui.run(vec![Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }], Modifiers::NONE);
            ui.frames(2);
        };
        for (key, order) in [
            (SortKey::Ext, ["aa", "zz", "c.doc", "b.txt", "a.zip"]),
            (SortKey::Size, ["aa", "zz", "a.zip", "c.doc", "b.txt"]),
            (SortKey::Time, ["aa", "zz", "c.doc", "b.txt", "a.zip"]),
            (SortKey::Name, ["aa", "zz", "a.zip", "b.txt", "c.doc"]),
        ] {
            let (l, r) = cell(key);
            // on the title
            click(&mut ui, l + 12.0);
            assert_eq!(ui.app.panels[0].sort, sal_core::SortSpec { key, reverse: false }, "{key:?} on its title");
            // in the empty part of the cell: the other way round
            click(&mut ui, r - 10.0);
            assert_eq!(ui.app.panels[0].sort, sal_core::SortSpec { key, reverse: true }, "{key:?} in the empty part");
            click(&mut ui, r - 10.0);
            assert_eq!(ui.app.panels[0].sort.key, key);
            if ui.app.panels[0].sort.reverse {
                click(&mut ui, r - 10.0);
            }
            assert_eq!(names(&ui), order, "{key:?}");
        }

        // sorted by size, folders go by their sizes once calculated (Alt+F10 on both)
        let (_, r) = cell(SortKey::Size);
        click(&mut ui, r - 10.0);
        assert_eq!(ui.app.panels[0].sort, sal_core::SortSpec { key: SortKey::Size, reverse: false });
        for d in ["aa", "zz"] {
            ui.app.dir_sizes.restart(t.path().join(d), &ui.ctx.clone());
        }
        for _ in 0..500 {
            ui.frames(1);
            if !ui.app.dir_sizes.busy() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        ui.frames(2);
        click(&mut ui, r - 10.0);
        assert_eq!(names(&ui), ["zz", "aa", "b.txt", "c.doc", "a.zip"], "largest first, folders by their sizes");
    }

    /// The status bar's operations: "No operations running" when nothing
    /// runs; what the last one did, for a moment (not left behind in the
    /// status line); kinds counted as copying, moving and deleting.
    #[test]
    fn operations_in_the_status_bar() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("pryč.txt"), "x").unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        assert!(ui.app.job_done.is_none());
        cursor_to(&mut ui, "pryč.txt");
        ui.key(Key::F8);
        ui.key(Key::Enter);
        ui.wait_job();
        ui.frames(2);
        assert!(!t.path().join("pryč.txt").exists());
        let (done, _) = ui.app.job_done.clone().expect("what it did, for a moment");
        assert!(done.starts_with("Deleting: 1 file"), "{done}");
        assert!(!ui.app.status.contains("Deleting"), "not left in the status line: {}", ui.app.status);
        // after a moment: no operations running
        ui.app.job_done.as_mut().unwrap().1 = Instant::now() - JOB_DONE_SHOWN - Duration::from_secs(1);
        ui.frames(2);
        assert!(ui.app.job_done.is_none());
        // the kinds the bar counts
        assert_eq!((JobKind::Recycle.group(), JobKind::Delete.group()), ("Deleting", "Deleting"));
        assert_eq!(JobKind::Transfer(OpKind::Move).group(), "Moving");
        assert_eq!(JobKind::Archive(FolderOp::Transfer(OpKind::Copy)).group(), "Copying");
        assert_eq!(JobKind::Archive(FolderOp::Rename).group(), "Renaming");
    }

    fn type_path(ui: &mut Ui, text: &str) {
        ui.app.command(&ui.ctx.clone(), Cmd::ChangeDir);
        ui.frames(2);
        ui.text(text);
        ui.key(Key::Enter);
    }

    #[test]
    fn path_bar_inline_edit() {
        let t = tempfile::tempdir().unwrap();
        let sub = t.path().join("Příliš žluťoučký").join("子目录");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("cíl.txt"), "").unwrap();
        let mut ui = Ui::new(t.path(), t.path());

        // the whole path is selected, so typing replaces it
        type_path(&mut ui, &sub.to_string_lossy());
        assert!(ui.app.path_edit[0].is_none());
        assert_eq!(ui.app.panels[0].path(), sub.as_path());
        assert!(ui.app.panels[1].path() == t.path(), "only the active panel changes");

        // relative path with forward slashes and ..
        type_path(&mut ui, "../子目录/cíl.txt");
        assert_eq!(ui.app.panels[0].path(), sub.as_path());
        assert_eq!(ui.cursor_name(), "cíl.txt");

        // Esc cancels
        ui.app.command(&ui.ctx.clone(), Cmd::ChangeDir);
        ui.frames(2);
        ui.text("C:\\Windows");
        ui.key(Key::Escape);
        assert!(ui.app.path_edit[0].is_none());
        assert_eq!(ui.app.panels[0].path(), sub.as_path());

        // a bad path reports an error and keeps the panel where it was
        type_path(&mut ui, "does-not-exist-🦎");
        assert!(matches!(ui.app.dialog, Dialog::Message { .. }));
        assert_eq!(ui.app.panels[0].path(), sub.as_path());
    }

    /// The middle of the text `text` as last drawn.
    fn text_pos(ui: &Ui, text: &str) -> Option<egui::Pos2> {
        fn find(shape: &egui::Shape, text: &str) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(t) if t.galley.text() == text => Some(t.pos + t.galley.rect.center().to_vec2()),
                egui::Shape::Vec(v) => v.iter().find_map(|s| find(s, text)),
                _ => None,
            }
        }
        ui.last.as_ref()?.shapes.iter().find_map(|c| find(&c.shape, text))
    }

    fn click(ui: &mut Ui, pos: egui::Pos2) {
        ui.run(vec![Event::PointerMoved(pos)], Modifiers::NONE);
        let button = |pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        ui.run(vec![button(true)], Modifiers::NONE);
        ui.run(vec![button(false)], Modifiers::NONE);
        ui.frames(2);
    }

    #[test]
    fn click_on_empty_space_activates_the_panel() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        assert_eq!(ui.app.active, 0);
        // below the last row of the right panel: no item there
        let body = ui.app.body_rects[1];
        let last = ui.app.row_hits[1].iter().map(|(r, _)| r.bottom()).fold(body.top(), f32::max);
        let empty = egui::pos2(body.center().x, (last + body.bottom()) / 2.0);
        assert!(empty.y > last + 4.0, "room below the rows: {last} vs {body:?}");
        click(&mut ui, empty);
        assert_eq!(ui.app.active, 1, "empty space");
        // and the footer of the left one
        let footer = egui::pos2(ui.app.body_rects[0].center().x, ui.app.body_rects[0].bottom() + 8.0);
        click(&mut ui, footer);
        assert_eq!(ui.app.active, 0, "footer");
    }

    #[test]
    fn breadcrumb_click_goes_up_and_focuses_origin() {
        let t = tempfile::tempdir().unwrap();
        let deep = t.path().join("ab").join("cd");
        fs::create_dir_all(&deep).unwrap();
        let mut ui = Ui::new(&deep, &deep);
        let target = t.path().to_path_buf();
        let rect = ui.app.crumb_rects[0].iter().find(|(_, p)| *p == target).map(|(r, _)| *r);
        let Some(rect) = rect else { panic!("crumb for {} not shown: {:?}", target.display(), ui.app.crumb_rects[0]) };
        click(&mut ui, rect.center());
        assert_eq!(ui.app.panels[0].path(), target.as_path());
        assert_eq!(ui.cursor_name(), "ab");

        // clicking the last segment (current directory) opens the editor
        let (last, _) = *ui.app.crumb_rects[0].last().unwrap();
        click(&mut ui, last.center());
        assert!(ui.app.path_edit[0].is_some());
    }

    #[test]
    fn unc_server_and_share_navigation() {
        // needs the local Server service and one readable disk share (C$ will do:
        // hidden shares are listed because hidden files are shown by default)
        let Ok(shares) = location::list_shares("localhost") else { return };
        let Some(share) = shares.iter().find(|s| fs::read_dir(format!(r"\\localhost\{}", s.name)).is_ok()) else { return };
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        assert!(ui.app.cfg.show_hidden);

        type_path(&mut ui, r"\\localhost");
        assert_eq!(ui.app.panels[0].path(), Path::new(r"\\localhost"));
        assert!(ui.app.panels[0].entries().iter().any(|e| e.name == share.name));

        type_path(&mut ui, &format!(r"\\localhost\{}", share.name));
        assert!(location::unc_share_root(ui.app.panels[0].path()).is_some());
        ui.key(Key::Backspace);
        assert_eq!(ui.app.panels[0].path(), Path::new(r"\\localhost"));
        assert_eq!(ui.cursor_name(), share.name);
        assert_eq!(drives::kind_of_root(&drives::root_of(ui.app.panels[0].path()), &[]), sal_core::drives::DriveType::Network);
    }

    /// `cargo test -p sal-app snapshot_path_bar -- --ignored` (set SNAPSHOT_DIR)
    #[test]
    #[ignore]
    fn snapshot_path_bar() {
        let out_dir = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let t = tempfile::tempdir().unwrap();
        let deep = t.path().join("Projekty").join("Účetnictví 2026").join("Faktury").join("Říjen").join("Přijaté");
        fs::create_dir_all(&deep).unwrap();
        let unc = PathBuf::from(r"\\localhost\C$\Windows\System32");
        assert!(unc.is_dir(), "{} not readable", unc.display());
        let left = unc;
        let mut ui = Ui::new(&left, &deep);
        ui.ctx.set_pixels_per_point(1.5);
        ui.frames(3);
        let bg = ui.ctx.style().visuals.panel_fill;
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("pathbar.png"));
        ui.app.active = 1;
        ui.app.command(&ui.ctx.clone(), Cmd::ChangeDir);
        ui.frames(3);
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("pathbar_edit.png"));
    }

    fn wait_sizes(ui: &mut Ui) {
        for _ in 0..500 {
            if !ui.app.dir_sizes.busy() {
                ui.frames(1);
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
            ui.frames(1);
        }
        panic!("size calculation did not finish");
    }

    fn sized_tree(root: &Path) -> u64 {
        fs::create_dir_all(root.join("velký adresář").join("vnořený 子")).unwrap();
        fs::write(root.join("velký adresář").join("a.bin"), vec![1u8; 70_000]).unwrap();
        fs::write(root.join("velký adresář").join("vnořený 子").join("b.bin"), vec![2u8; 5_000]).unwrap();
        fs::write(root.join("zz.txt"), vec![3u8; 100]).unwrap();
        75_000
    }

    #[test]
    fn shift_space_selects_and_sizes_folder_in_background() {
        let t = tempfile::tempdir().unwrap();
        let expected = sized_tree(t.path());
        let mut ui = Ui::new(t.path(), t.path());
        ui.text("vel");
        assert_eq!(ui.cursor_name(), "velký adresář");
        chord(&mut ui, Key::Space, Modifiers::SHIFT, Some(" "));
        assert!(ui.app.panels[0].qs.is_empty() && ui.app.panels[0].entries().iter().any(|e| e.name == "velký adresář" && e.selected));
        assert_eq!(ui.cursor_name(), "zz.txt", "cursor moves on like Space");
        wait_sizes(&mut ui);
        let si = ui.app.dir_sizes.get(&t.path().join("velký adresář")).expect("size calculated");
        assert!(si.done);
        assert_eq!((si.size.bytes, si.size.files, si.size.dirs), (expected, 2, 1));

        // the selection total includes the folder; then add the file too
        let totals = selection_totals(ui.app.panels[0].entries(), &ui.app.dir_sizes.snapshot());
        assert_eq!(totals, SelectionTotals { files: 0, dirs: 1, bytes: expected, calculating: 0, unknown: 0, inner_files: 2, inner_dirs: 1 });
        chord(&mut ui, Key::Space, Modifiers::SHIFT, Some(" "));
        let totals = selection_totals(ui.app.panels[0].entries(), &ui.app.dir_sizes.snapshot());
        assert_eq!((totals.files, totals.bytes), (1, expected + 100));

        // Ctrl+R forgets sizes (they may be stale)
        chord(&mut ui, Key::R, Modifiers::CTRL, None);
        assert!(ui.app.dir_sizes.get(&t.path().join("velký adresář")).is_none());
    }

    #[test]
    fn folder_size_shortcut_is_configurable() {
        let t = tempfile::tempdir().unwrap();
        sized_tree(t.path());
        let dir = t.path().join("velký adresář");
        let mut ui = Ui::new(t.path(), t.path());

        // plain Space only selects by default
        ui.text("vel");
        ui.key(Key::Escape); // end the quick search: Space would type into it
        chord(&mut ui, Key::Space, Modifiers::NONE, Some(" "));
        assert!(ui.app.panels[0].entries().iter().any(|e| e.path == dir && e.selected));
        assert!(ui.app.dir_sizes.get(&dir).is_none());

        // preset "Space": plain Space calculates sizes, Shift+Space no longer does
        let space = keymap::presets(Action::SelectCalcSize).into_iter().find(|(l, _)| l == "Space").unwrap().1;
        keymap::set_bindings(&mut ui.app.cfg.shortcuts, Action::SelectCalcSize, space);
        ui.key(Key::Home);
        ui.text("velký");
        chord(&mut ui, Key::Space, Modifiers::NONE, Some(" ")); // in a quick search Space types
        assert_eq!(ui.app.panels[0].qs, "velký ");
        ui.key(Key::Escape);
        chord(&mut ui, Key::Space, Modifiers::NONE, Some(" ")); // unselect: no calculation
        assert!(ui.app.dir_sizes.get(&dir).is_none());
        ui.key(Key::Home);
        ui.text("vel");
        ui.key(Key::Escape);
        chord(&mut ui, Key::Space, Modifiers::NONE, Some(" ")); // select: calculates
        wait_sizes(&mut ui);
        assert!(ui.app.dir_sizes.get(&dir).is_some_and(|s| s.done));
    }

    /// `cargo test -p sal-app snapshot_dir_sizes -- --ignored` (set SNAPSHOT_DIR)
    #[test]
    #[ignore]
    fn snapshot_dir_sizes() {
        let out_dir = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let t = tempfile::tempdir().unwrap();
        sized_tree(t.path());
        fs::create_dir_all(t.path().join("prázdný")).unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.ctx.set_pixels_per_point(1.5);
        ui.text("vel");
        chord(&mut ui, Key::Space, Modifiers::SHIFT, Some(" "));
        wait_sizes(&mut ui);
        ui.frames(2);
        let bg = ui.ctx.style().visuals.panel_fill;
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("dirsizes.png"));
        ui.app.command(&ui.ctx.clone(), Cmd::SetTheme(ThemeMode::Light));
        ui.frames(3);
        let bg = ui.ctx.style().visuals.panel_fill;
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("dirsizes_light.png"));
    }

    /// `cargo test -p sal-app snapshot_shortcut_sections -- --ignored` (set SNAPSHOT_DIR)
    #[test]
    #[ignore]
    fn snapshot_shortcut_sections() {
        let out_dir = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.screen = egui::vec2(1100.0, std::env::var("SNAPSHOT_HEIGHT").ok().and_then(|h| h.parse().ok()).unwrap_or(1700.0));
        ui.app.dialog = Dialog::Settings { tab: SettingsTab::Keyboard, recording: None };
        ui.frames(12);
        let bg = ui.ctx.style().visuals.panel_fill;
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("shortcut_sections.png"));
    }

    #[test]
    fn calculate_selection_size_command() {
        let t = tempfile::tempdir().unwrap();
        let expected = sized_tree(t.path());
        fs::create_dir_all(t.path().join("druhý")).unwrap();
        fs::write(t.path().join("druhý").join("c.bin"), vec![0u8; 1234]).unwrap();
        let mut ui = Ui::new(t.path(), t.path());

        // nothing selected: the folder under the cursor
        ui.text("vel");
        chord(&mut ui, Key::F10, Modifiers::ALT, None);
        wait_sizes(&mut ui);
        assert_eq!(ui.app.dir_sizes.get(&t.path().join("velký adresář")).unwrap().size.bytes, expected);
        assert!(!ui.app.panels[0].entries().iter().any(|e| e.selected), "selection is left alone");

        // with a selection: every selected folder, and the footer total adds up
        ui.key(Key::Escape);
        chord(&mut ui, Key::A, Modifiers::CTRL | Modifiers::COMMAND, None);
        chord(&mut ui, Key::F10, Modifiers::ALT, None);
        wait_sizes(&mut ui);
        let totals = selection_totals(ui.app.panels[0].entries(), &ui.app.dir_sizes.snapshot());
        assert_eq!(totals, SelectionTotals { files: 1, dirs: 2, bytes: expected + 1234 + 100, calculating: 0, unknown: 0, inner_files: 3, inner_dirs: 1 });
        ui.frames(2);
        assert!(ui.app.status.contains("in 4 files, 1 subdirectory"), "{}", ui.app.status);
    }

    #[cfg(windows)]
    #[test]
    fn hidden_and_system_toggles_are_separate() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = tempfile::tempdir().unwrap();
        for (name, attrs) in [("plain.txt", 0x80), ("skrytý.txt", 2), ("systémový.txt", 4)] {
            fs::OpenOptions::new().write(true).create(true).attributes(attrs).open(t.path().join(name)).unwrap();
        }
        let names = |ui: &Ui| ui.app.panels[0].entries().iter().skip(1).map(|e| e.name.clone()).collect::<Vec<_>>().join(",");
        let mut ui = Ui::new(t.path(), t.path());
        assert_eq!(names(&ui), "plain.txt,skrytý.txt,systémový.txt");
        chord(&mut ui, Key::H, Modifiers::CTRL | Modifiers::COMMAND, None); // hide hidden only
        assert_eq!(names(&ui), "plain.txt,systémový.txt");
        assert!(!ui.app.cfg.show_hidden && ui.app.cfg.show_system);
        // "Show system files" has no default key; use the offered preset
        let preset = keymap::presets(Action::ToggleSystem).into_iter().find(|(l, _)| l == "Ctrl+Shift+H").unwrap().1;
        keymap::set_bindings(&mut ui.app.cfg.shortcuts, Action::ToggleSystem, preset);
        chord(&mut ui, Key::H, Modifiers::CTRL | Modifiers::COMMAND | Modifiers::SHIFT, None);
        assert_eq!(names(&ui), "plain.txt");
        chord(&mut ui, Key::H, Modifiers::CTRL | Modifiers::COMMAND, None);
        assert_eq!(names(&ui), "plain.txt,skrytý.txt");
    }

    fn right_press(ui: &mut Ui, pos: egui::Pos2) {
        ui.run(vec![Event::PointerMoved(pos)], Modifiers::NONE);
        ui.run(vec![Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE }], Modifiers::NONE);
    }

    fn right_release(ui: &mut Ui, pos: egui::Pos2) {
        ui.run(vec![Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE }], Modifiers::NONE);
        ui.frames(2);
    }

    fn row_pos(ui: &Ui, panel: usize, name: &str) -> egui::Pos2 {
        let i = ui.app.panels[panel].entries().iter().position(|e| e.name == name).unwrap();
        ui.app.row_hits[panel].iter().find(|(_, r)| *r == i).map(|(rect, _)| rect.center()).unwrap()
    }

    fn menu_files(t: &Path) {
        for n in ["a.txt", "b.txt", "c.txt"] {
            fs::write(t.join(n), n).unwrap();
        }
        fs::create_dir_all(t.join("složka")).unwrap();
    }

    #[test]
    fn right_click_opens_menu_for_item_or_selection() {
        let t = tempfile::tempdir().unwrap();
        menu_files(t.path());
        let mut ui = Ui::new(t.path(), t.path());
        let at = row_pos(&ui, 0, "b.txt");
        right_press(&mut ui, at);
        right_release(&mut ui, at);
        let m = ui.app.ctx_menu.as_ref().expect("menu opened");
        assert_eq!(m.items, [t.path().join("b.txt")]);
        assert_eq!(ui.cursor_name(), "b.txt");
        assert!(!ui.app.panels[0].entries().iter().any(|e| e.selected), "a short right-click does not select in this mode");
        ui.key(Key::Escape);
        assert!(ui.app.ctx_menu.is_none());

        // with a selection, the menu covers the whole selection, even when
        // the right-clicked item is not part of it (like F5/F8)
        ui.app.panels[0].select_by_mask(&MaskGroup::new("a.txt;c.txt", false).unwrap(), true);
        for clicked in ["c.txt", "b.txt"] {
            let at = row_pos(&ui, 0, clicked);
            right_press(&mut ui, at);
            right_release(&mut ui, at);
            assert_eq!(ui.app.ctx_menu.as_ref().unwrap().items, [t.path().join("a.txt"), t.path().join("c.txt")], "right-clicked {clicked}");
            ui.key(Key::Escape);
        }

        // below the last row: the folder background
        let body = ui.app.body_rects[0];
        let empty = egui::pos2(body.center().x, body.bottom() - 10.0);
        right_press(&mut ui, empty);
        right_release(&mut ui, empty);
        let m = ui.app.ctx_menu.as_ref().unwrap();
        assert!(m.items.is_empty());
        assert_eq!(m.dir, t.path());
    }

    #[test]
    fn long_press_mode() {
        let t = tempfile::tempdir().unwrap();
        menu_files(t.path());
        let mut ui = Ui::new(t.path(), t.path());
        ui.app.cfg.context_menu_trigger = ContextMenuTrigger::LongPress;
        ui.app.cfg.long_press_ms = 400;

        // a short right-click selects, no menu
        let at = row_pos(&ui, 0, "a.txt");
        right_press(&mut ui, at);
        right_release(&mut ui, at);
        assert!(ui.app.ctx_menu.is_none());
        assert!(ui.app.panels[0].entries().iter().any(|e| e.name == "a.txt" && e.selected));

        // holding arms the menu (each frame advances egui's clock ~1/60 s);
        // it opens on release, for the selection (a.txt is selected)
        let at = row_pos(&ui, 0, "b.txt");
        right_press(&mut ui, at);
        ui.frames(10);
        assert!(!ui.app.rmb.as_ref().unwrap().armed, "not yet");
        ui.frames(30);
        assert!(ui.app.rmb.as_ref().unwrap().armed);
        assert!(ui.app.ctx_menu.is_none(), "the menu waits for the release");
        right_release(&mut ui, at);
        assert_eq!(ui.app.ctx_menu.as_ref().expect("opened after holding").items, [t.path().join("a.txt")]);
        assert!(!ui.app.panels[0].entries().iter().any(|e| e.name == "b.txt" && e.selected), "a long press does not select");
        assert_eq!(ui.cursor_name(), "b.txt");
    }

    #[test]
    fn menu_actions_and_keyboard() {
        let t = tempfile::tempdir().unwrap();
        menu_files(t.path());
        fs::write(t.path().join("složka").join("x.bin"), vec![0u8; 4096]).unwrap();
        let mut ui = Ui::new(t.path(), t.path());

        // Shift+F10 opens the menu for the cursor item
        ui.text("b.t");
        ui.key_mod(Key::F10, Modifiers::SHIFT);
        let m = ui.app.ctx_menu.take().expect("keyboard menu");
        assert_eq!(m.items, [t.path().join("b.txt")]);
        ui.app.run_menu_action(&ui.ctx.clone(), m, MenuAction::Rename);
        assert!(matches!(&ui.app.dialog, Dialog::Rename { path, .. } if path == &t.path().join("b.txt")));
        ui.key(Key::Escape);

        // folder actions
        let dir = t.path().join("složka");
        let menu = CtxMenu { panel: 0, items: vec![dir.clone()], dir: t.path().to_path_buf(), pos: egui::pos2(100.0, 100.0), can_paste: false, plugin: false };
        ui.app.run_menu_action(&ui.ctx.clone(), menu, MenuAction::CalcSize);
        wait_sizes(&mut ui);
        assert_eq!(ui.app.dir_sizes.get(&dir).unwrap().size.bytes, 4096);
        let menu = CtxMenu { panel: 0, items: vec![dir.clone()], dir: t.path().to_path_buf(), pos: egui::pos2(100.0, 100.0), can_paste: false, plugin: false };
        ui.app.run_menu_action(&ui.ctx.clone(), menu, MenuAction::Open);
        assert_eq!(ui.app.panels[0].path(), dir.as_path());

        // Delete goes through the normal confirmation with exactly the menu's items
        let menu = CtxMenu { panel: 0, items: vec![dir.join("x.bin")], dir: dir.clone(), pos: egui::pos2(100.0, 100.0), can_paste: false, plugin: false };
        ui.app.run_menu_action(&ui.ctx.clone(), menu, MenuAction::Delete);
        assert!(matches!(&ui.app.dialog, Dialog::Delete { sources, .. } if sources == &[dir.join("x.bin")]));
    }

    #[test]
    fn menu_draws_and_closes_on_outside_click() {
        let t = tempfile::tempdir().unwrap();
        menu_files(t.path());
        let mut ui = Ui::new(t.path(), t.path());
        let at = row_pos(&ui, 0, "složka");
        right_press(&mut ui, at);
        right_release(&mut ui, at);
        assert!(ui.app.ctx_menu.is_some());
        ui.frames(2);
        // a click far away closes it without doing anything
        click(&mut ui, egui::pos2(900.0, 400.0));
        assert!(ui.app.ctx_menu.is_none());
        assert!(matches!(ui.app.dialog, Dialog::None));
    }

    #[test]
    fn classic_style_without_window_reports_error() {
        let t = tempfile::tempdir().unwrap();
        menu_files(t.path());
        let mut ui = Ui::new(t.path(), t.path());
        ui.app.cfg.context_menu_style = ContextMenuStyle::Classic;
        let at = row_pos(&ui, 0, "a.txt");
        right_press(&mut ui, at);
        right_release(&mut ui, at);
        assert!(ui.app.ctx_menu.is_none());
        assert!(matches!(ui.app.dialog, Dialog::Message { .. }), "headless: no window for the native menu");
    }

    /// Ctrl+C, Ctrl+X and Ctrl+V in a panel run the shell's copy, cut and
    /// paste, as the context menu does (headless there is no window for
    /// them, which proves they got there without touching the clipboard).
    #[test]
    fn clipboard_keys_reach_the_shell() {
        let t = tempfile::tempdir().unwrap();
        menu_files(t.path());
        let mut ui = Ui::new(t.path(), t.path());
        let p = &mut ui.app.panels[0];
        let i = p.entries().iter().position(|e| e.name == "a.txt").unwrap();
        p.set_cursor(i);
        for (key, verb) in [(Key::C, "copy"), (Key::X, "cut"), (Key::V, "paste")] {
            ui.app.dialog = Dialog::None;
            ui.frames(1);
            ui.key_mod(key, Modifiers::CTRL);
            let Dialog::Message { text, .. } = &ui.app.dialog else { panic!("Ctrl+{key:?} did nothing") };
            assert!(text.contains(&format!("\"{verb}\"")), "{text}");
        }
    }

    /// `cargo test -p sal-app snapshot_context_menu -- --ignored` (set SNAPSHOT_DIR)
    #[test]
    #[ignore]
    fn snapshot_context_menu() {
        let out_dir = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let t = tempfile::tempdir().unwrap();
        menu_files(t.path());
        let mut ui = Ui::new(t.path(), t.path());
        crate::fonts::install(&ui.ctx, "", "");
        ui.ctx.set_pixels_per_point(1.5);
        ui.frames(2);
        let at = row_pos(&ui, 0, "složka");
        right_press(&mut ui, at);
        right_release(&mut ui, at);
        ui.frames(3);
        let bg = ui.ctx.style().visuals.panel_fill;
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("context_menu.png"));
        ui.key(Key::Escape);
        let body = ui.app.body_rects[0];
        let empty = egui::pos2(body.center().x, body.center().y + 100.0);
        right_press(&mut ui, empty);
        right_release(&mut ui, empty);
        ui.frames(3);
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("context_menu_bg.png"));
    }

    #[test]
    fn theme_switching() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.app.cfg.font_size = 19.0;
        apply_font_size(&ui.ctx, 19.0);
        let body = |ui: &Ui| ui.ctx.style().text_styles[&egui::TextStyle::Body].size;

        ui.app.command(&ui.ctx.clone(), Cmd::SetTheme(ThemeMode::Dark));
        ui.frames(2);
        assert_eq!(ui.ctx.theme(), egui::Theme::Dark);
        assert_eq!(body(&ui), 19.0, "font size survives the switch");

        ui.app.command(&ui.ctx.clone(), Cmd::SetTheme(ThemeMode::Light));
        ui.frames(2);
        assert_eq!(ui.ctx.theme(), egui::Theme::Light);
        assert_eq!(ui.app.cfg.theme, ThemeMode::Light);
        assert_eq!(body(&ui), 19.0);

        // the configurable toggle flips whatever is shown
        let preset = keymap::presets(Action::ToggleTheme).into_iter().find(|(l, _)| l == "Ctrl+Shift+D").unwrap().1;
        keymap::set_bindings(&mut ui.app.cfg.shortcuts, Action::ToggleTheme, preset);
        chord(&mut ui, Key::D, Modifiers::CTRL | Modifiers::COMMAND | Modifiers::SHIFT, None);
        assert_eq!(ui.ctx.theme(), egui::Theme::Dark);
        assert_eq!(ui.app.cfg.theme, ThemeMode::Dark);
    }

    /// `cargo test -p sal-app snapshot_theme_menu -- --ignored` (set SNAPSHOT_DIR)
    #[test]
    #[ignore]
    fn snapshot_theme_menu() {
        let out_dir = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let t = tempfile::tempdir().unwrap();
        menu_files(t.path());
        let mut ui = Ui::new(t.path(), t.path());
        ui.app.command(&ui.ctx.clone(), Cmd::SetTheme(ThemeMode::Light));
        ui.frames(2);
        click(&mut ui, egui::pos2(212.0, 10.0)); // "View" in the menu bar
        ui.frames(3);
        let bg = ui.ctx.style().visuals.panel_fill;
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("theme_menu.png"));
    }

    #[test]
    fn layout_options() {
        let t = tempfile::tempdir().unwrap();
        menu_files(t.path());
        let mut ui = Ui::new(t.path(), t.path());
        // the panels leave a gap for the divider and don't overlap
        let (l, r) = (ui.app.body_rects[0], ui.app.body_rects[1]);
        assert!(r.left() - l.right() >= DIVIDER_WIDTH - 1.0, "{l:?} {r:?}");

        assert!(ui.ctx.style().spacing.scroll.floating);
        ui.app.command(&ui.ctx.clone(), Cmd::ToggleScrollbars);
        ui.frames(2);
        assert!(ui.app.cfg.always_show_scrollbars);
        assert!(!ui.ctx.style().spacing.scroll.floating, "solid bars");
        ui.app.command(&ui.ctx.clone(), Cmd::SetTheme(ThemeMode::Light));
        ui.frames(1);
        assert!(!ui.ctx.style().spacing.scroll.floating, "kept across themes");

        // animations off: nothing fades in, scrolling to the cursor is instant
        assert!(ui.app.cfg.animations && ui.ctx.style().animation_time > 0.0, "on to start with");
        ui.app.command(&ui.ctx.clone(), Cmd::ToggleAnimations);
        ui.frames(1);
        assert!(!ui.app.cfg.animations);
        let style = ui.ctx.style();
        assert_eq!(style.animation_time, 0.0);
        assert_eq!(style.scroll_animation.duration.max, 0.0);
        ui.app.command(&ui.ctx.clone(), Cmd::SetTheme(ThemeMode::Dark));
        ui.frames(1);
        assert_eq!(ui.ctx.style().animation_time, 0.0, "kept across themes");
        assert_eq!(ui.app.opener_options(false)["animations"], "off", "plugin windows are told");
        ui.app.command(&ui.ctx.clone(), Cmd::ToggleAnimations);
        assert!(ui.ctx.style().animation_time > 0.0, "back on");

        // hiding the function key bar gives the panels the space
        let before = ui.app.body_rects[0].height();
        ui.app.command(&ui.ctx.clone(), Cmd::ToggleFkeyBar);
        ui.frames(3);
        assert!(!ui.app.cfg.show_fkey_bar);
        assert!(ui.app.body_rects[0].height() > before, "{} > {before}", ui.app.body_rects[0].height());
    }

    /// `cargo test -p sal-app snapshot_layout -- --ignored` (set SNAPSHOT_DIR)
    #[test]
    #[ignore]
    fn snapshot_layout() {
        let out_dir = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let t = tempfile::tempdir().unwrap();
        for i in 0..80 {
            fs::write(t.path().join(format!("soubor {i:02}.txt")), "x").unwrap();
        }
        for (name, theme) in [("layout_dark.png", ThemeMode::Dark), ("layout_light.png", ThemeMode::Light)] {
            let mut ui = Ui::new(t.path(), t.path());
            ui.app.command(&ui.ctx.clone(), Cmd::SetTheme(theme));
            ui.app.command(&ui.ctx.clone(), Cmd::ToggleScrollbars);
            ui.app.command(&ui.ctx.clone(), Cmd::ToggleFkeyBar);
            ui.frames(3);
            let bg = ui.ctx.style().visuals.panel_fill;
            let out = ui.last.take().unwrap();
            ui.snap.render(&ui.ctx, out, bg, &out_dir.join(name));
        }
    }

    #[test]
    fn size_results_include_file_counts() {
        let t = tempfile::tempdir().unwrap();
        let expected = sized_tree(t.path()); // "velký adresář": 2 files, 1 subfolder (counts are contents)
        let mut ui = Ui::new(t.path(), t.path());

        // the cursor folder: bytes, files and folders
        ui.text("vel");
        chord(&mut ui, Key::F10, Modifiers::ALT, None);
        wait_sizes(&mut ui);
        ui.frames(2);
        assert_eq!(ui.app.status, format!("Size: {} ({} bytes) in 2 files, 1 subdirectory", format_size(expected), format_thousands(expected)));

        // a selection with a file and a sized folder: totals include the folder's files
        ui.key(Key::Escape);
        chord(&mut ui, Key::A, Modifiers::CTRL | Modifiers::COMMAND, None);
        let t2 = selection_totals(ui.app.panels[0].entries(), &ui.app.dir_sizes.snapshot());
        assert_eq!((t2.items(), t2.all_files()), ("1 file, 1 directory".to_owned(), Some(3)));
        assert_eq!(t2.contents().as_deref(), Some("3 files, 1 subdirectory"), "footer: subdirectories inside sized folders");
        chord(&mut ui, Key::F10, Modifiers::ALT, None);
        wait_sizes(&mut ui);
        ui.frames(2);
        assert!(ui.app.status.contains("in 3 files, 1 subdirectory"), "{}", ui.app.status);
        assert!(ui.app.status.contains(&format_thousands(expected + 100)), "{}", ui.app.status);
    }

    #[test]
    fn private_use_names_display_with_original_characters() {
        let t = tempfile::tempdir().unwrap();
        // what a Mac or Samba share stores for "hvězdičky*" and "a:b"
        let star = "hvězdičky\u{F021}";
        fs::create_dir_all(t.path().join(star)).unwrap();
        fs::write(t.path().join("a\u{F022}b.txt"), "x").unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        assert!(ui.app.panels[0].entries().iter().any(|e| e.name == star), "the real name is kept");
        ui.text("hvě");
        ui.frames(2);
        // footer and breadcrumbs show the original character
        let crumbs = location::breadcrumbs(&t.path().join(star));
        assert_eq!(display_name(&crumbs.last().unwrap().0), "hvězdičky*");
        ui.key(Key::Enter);
        assert_eq!(ui.app.panels[0].path(), t.path().join(star).as_path(), "opening uses the real name");
    }

    #[test]
    fn font_size_slider_applies_when_drag_ends() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.app.cfg.font_size = 14.0;
        apply_font_size(&ui.ctx, 14.0);
        ui.app.dialog = Dialog::Settings { tab: SettingsTab::General, recording: None };
        ui.frames(3);
        // find the slider by its value text and drag it to the right
        let body = |ui: &Ui| ui.ctx.style().text_styles[&egui::TextStyle::Body].size;
        let rect = ui.app.font_slider_rect.expect("slider drawn");
        let from = egui::pos2(rect.left() + rect.width() * 0.25, rect.center().y);
        let to = egui::pos2(rect.right() - 4.0, rect.center().y);
        ui.run(vec![Event::PointerMoved(from)], Modifiers::NONE);
        ui.run(vec![Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }], Modifiers::NONE);
        for k in 1..=6 {
            let p = from + (to - from) * (k as f32 / 6.0);
            ui.run(vec![Event::PointerMoved(p)], Modifiers::NONE);
        }
        assert!(ui.app.cfg.font_size > 14.0, "the value follows the drag");
        assert_eq!(body(&ui), 14.0, "but the GUI keeps its size while dragging");
        ui.run(vec![Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }], Modifiers::NONE);
        ui.frames(2);
        assert_eq!(body(&ui), ui.app.cfg.font_size, "applied on release");
    }
    /// `cargo test -p sal-app snapshot_private_use_names -- --ignored` (set SNAPSHOT_DIR)
    #[test]
    #[ignore]
    fn snapshot_private_use_names() {
        let out_dir = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("hvězdičky\u{F021}")).unwrap();
        fs::write(t.path().join("otázka\u{F025} a\u{F022}b.txt"), "x").unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        crate::fonts::install(&ui.ctx, "", "");
        ui.ctx.set_pixels_per_point(1.5);
        ui.frames(3);
        let bg = ui.ctx.style().visuals.panel_fill;
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("private_use_names.png"));
    }

    #[test]
    fn menus_and_settings_share_labels() {
        // the keymap's command names match the checkbox labels
        assert_eq!(Action::ToggleHidden.info().label, LBL_HIDDEN);
        assert_eq!(Action::ToggleSystem.info().label, LBL_SYSTEM);
    }

    #[test]
    fn settings_general_draws_and_icons_toggle() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.app.dialog = Dialog::Settings { tab: SettingsTab::General, recording: None };
        ui.frames(3);
        assert!(ui.app.font_slider_rect.is_some(), "General tab drawn");
        ui.app.command(&ui.ctx.clone(), Cmd::ToggleIcons);
        assert!(!ui.app.cfg.system_icons);
    }

    /// The update window in each state, with the program and a plugin; a
    /// changelog shown. Escape closes it except while a download runs.
    /// SNAPSHOT_DIR set: also saved as update_*.png.
    #[test]
    fn update_dialog_states() {
        use crate::update::{Progress, Release, Target, Update};
        use std::sync::{Arc, Mutex};
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        crate::fonts::install(&ui.ctx, "", "");
        let release = |version: &str, tag: &str, asset: &str| Release {
            version: version.into(),
            tag: tag.into(),
            page: format!("https://github.com/nocturneop15/RustySalamander/releases/tag/{tag}"),
            asset: asset.into(),
            asset_url: String::new(),
            asset_size: 7_126_108,
            program_min: String::new(),
            program_max: String::new(),
        };
        let program = Update {
            target: Target::Program,
            current: "0.4.1-alpha".into(),
            new: false,
            about: String::new(),
            release: release("0.5.0-alpha", "v0.5.0-alpha", "RustySalamander-0.5.0-alpha-win64.zip"),
            repo: crate::update::REPO.into(),
            changelog: "CHANGELOG.md".into(),
        };
        let plugin = Update {
            target: Target::Plugin { id: "viewer".into(), name: "Viewer".into(), dir: t.path().join("plugins").join("viewer") },
            current: "0.5.0-alpha".into(),
            new: false,
            about: String::new(),
            release: release("0.6.0-alpha", "viewer-v0.6.0-alpha", "viewer-0.6.0-alpha-api5.zip"),
            repo: crate::update::REPO.into(),
            changelog: "plugins/viewer/CHANGELOG.md".into(),
        };
        let log = "## 0.6.0-alpha (2026-11-02)\n\n### Viewer\n\n- **Find** in the text (`Ctrl+F`), with the next match on F3.\n- Opens big files at once.";
        let states = [
            ("offer", None, None),
            ("downloading", Some(Progress::Downloading { done: 3_100_000, total: 7_126_108 }), None),
            (
                "done",
                Some(Progress::Done {
                    folder: PathBuf::from(r"D:\Applications\RustySalamander\0.5.0-alpha"),
                    copied: vec!["config.toml".into(), r"plugins\scp\known_hosts".into()],
                }),
                Some(Progress::Staged),
            ),
            ("failed", Some(Progress::Failed("https://github.com/…: the server answered 404".into())), Some(Progress::Failed("offline".into()))),
        ];
        for (name, state, plugin_state) in states {
            let running = matches!(state, Some(Progress::Downloading { .. }));
            let mut rows = vec![UpdateRow::new(program.clone()), UpdateRow::new(plugin.clone())];
            rows[0].install = state.map(|p| Arc::new(Mutex::new(p)));
            rows[1].install = plugin_state.map(|p| Arc::new(Mutex::new(p)));
            rows[1].show_log = true;
            rows[1].changelog = Some(Arc::new(Mutex::new(Some(Ok(log.to_owned())))));
            ui.app.dialog = Dialog::Update { rows };
            ui.frames(4);
            assert!(matches!(ui.app.dialog, Dialog::Update { .. }), "{name}: open");
            if let Some(dir) = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from) {
                let bg = ui.ctx.style().visuals.panel_fill;
                let out = ui.last.take().unwrap();
                ui.snap.render(&ui.ctx, out, bg, &dir.join(format!("update_{name}.png")));
            }
            ui.key(Key::Escape);
            ui.frames(1);
            assert_eq!(matches!(ui.app.dialog, Dialog::Update { .. }), running, "{name}: Escape");
        }
        ui.app.dialog = Dialog::None;

        // a plugin made for a newer program is greyed and left out of Update all
        let mut newer = plugin.clone();
        newer.target = Target::Plugin { id: "search".into(), name: "Search".into(), dir: t.path().join("plugins").join("search") };
        newer.release.program_min = "99.0.0".into();
        let mut scp = plugin.clone();
        scp.target = Target::Plugin { id: "scp".into(), name: "SCP".into(), dir: t.path().join("plugins").join("scp") };
        assert!(newer.fits().unwrap_err().contains("99.0.0"));
        // an official plugin not installed yet: offered, but only on its own Install
        let ftp = Update {
            target: Target::Plugin { id: "ftp".into(), name: "FTP / SFTP".into(), dir: t.path().join("plugins").join("ftp") },
            current: String::new(),
            new: true,
            about: "Connects to FTP, FTPS and SFTP servers and works with their files.".into(),
            release: release("0.1.0-alpha", "ftp-v0.1.0-alpha", "ftp-0.1.0-alpha-api8.zip"),
            repo: crate::update::REPO.into(),
            changelog: "plugins/ftp/CHANGELOG.md".into(),
        };
        let rows = vec![UpdateRow::new(program.clone()), UpdateRow::new(plugin.clone()), UpdateRow::new(newer), UpdateRow::new(scp), UpdateRow::new(ftp.clone())];
        let states = [None, Some(Progress::Staged), None, Some(Progress::Failed("offline".into())), None];
        assert_eq!(startable(&rows, &states), [0, 3], "not the staged one, not the one for a newer program, not a new plugin; a failed one again");
        ui.app.dialog = Dialog::Update { rows };
        ui.frames(4);
        if let Some(dir) = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from) {
            let bg = ui.ctx.style().visuals.panel_fill;
            let out = ui.last.take().unwrap();
            ui.snap.render(&ui.ctx, out, bg, &dir.join("update_all.png"));
        }
        ui.app.dialog = Dialog::None;
        // a new plugin alone; Don't offer remembers it and closes the window
        ui.app.dialog = Dialog::Update { rows: vec![UpdateRow::new(ftp)] };
        ui.frames(4);
        if let Some(dir) = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from) {
            let bg = ui.ctx.style().visuals.panel_fill;
            let out = ui.last.take().unwrap();
            ui.snap.render(&ui.ctx, out, bg, &dir.join("update_new_plugin.png"));
        }
        ui.frames(2);
        let button = text_pos(&ui, "Don't offer").expect("the Don't offer button");
        click(&mut ui, button);
        ui.frames(3);
        assert!(matches!(ui.app.dialog, Dialog::None), "closed: nothing left to offer");
        assert_eq!(ui.app.cfg.declined_plugins, ["ftp"]);
        assert!(update_offer(&ui.app.cfg, false).is_none_or(|o| o.declined == ["ftp"]));
        assert!(update_offer(&ui.app.cfg, true).is_none_or(|o| o.declined.is_empty()), "a check from the menu shows it");
        // the changelog's text, as laid out
        let job = inline_markdown(&egui::Ui::new(ui.ctx.clone(), egui::LayerId::background(), egui::Id::new("t"), egui::UiBuilder::new()), "a **b** `c` d");
        let parts: Vec<&str> = job.sections.iter().map(|s| &job.text[s.byte_range.clone()]).collect();
        assert_eq!(parts, ["a ", "b", " ", "c", " d"]);
    }

    #[cfg(windows)]
    #[test]
    fn file_icons_load_in_background() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("a.txt"), "x").unwrap();
        fs::create_dir_all(t.path().join("dir")).unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        let entries: Vec<FileEntry> = ui.app.panels[0].entries().to_vec();
        // generous: the whole suite loads icons in parallel
        for _ in 0..2000 {
            ui.frames(1);
            if entries.iter().filter(|e| e.kind != EntryKind::UpDir).all(|e| ui.app.icons.get(e).is_some()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("icons did not load: {} entries={:?}", ui.app.icons.debug_state(), entries.iter().map(|e| &e.name).collect::<Vec<_>>());
    }

    /// `cargo test -p sal-app snapshot_icons_and_settings -- --ignored` (set SNAPSHOT_DIR)
    #[test]
    #[ignore]
    fn snapshot_icons_and_settings() {
        let out_dir = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let t = tempfile::tempdir().unwrap();
        for n in ["zpráva.txt", "tabulka.xlsx", "dokument.docx", "foto.jpg", "archiv.zip", "skript.ps1", "stránka.html", "video.mp4", "hudba.mp3", "kniha.pdf", "README"] {
            fs::write(t.path().join(n), "x").unwrap();
        }
        fs::create_dir_all(t.path().join("složka")).unwrap();
        fs::copy(PathBuf::from(std::env::var("WINDIR").unwrap()).join("notepad.exe"), t.path().join("notepad.exe")).unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        crate::fonts::install(&ui.ctx, "", "");
        ui.ctx.set_pixels_per_point(1.5);
        for _ in 0..100 {
            ui.frames(1);
            std::thread::sleep(Duration::from_millis(10));
        }
        let bg = ui.ctx.style().visuals.panel_fill;
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("icons.png"));

        ui.screen = egui::vec2(1100.0, std::env::var("SNAPSHOT_HEIGHT").ok().and_then(|h| h.parse().ok()).unwrap_or(800.0));
        ui.app.dialog = Dialog::Settings { tab: SettingsTab::General, recording: None };
        ui.frames(8);
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("settings_general.png"));
        ui.app.dialog = Dialog::None;
        ui.frames(2);
        click(&mut ui, egui::pos2(212.0, 10.0)); // View menu
        ui.frames(3);
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("view_menu.png"));
    }

    #[test]
    fn panels_fit_after_the_window_shrinks() {
        let t = tempfile::tempdir().unwrap();
        menu_files(t.path());
        let mut ui = Ui::new(t.path(), t.path());
        for width in [1000.0, 800.0, 1300.0, 700.0] {
            ui.screen = egui::vec2(width, 800.0);
            ui.frames(4);
            let (l, r) = (ui.app.body_rects[0], ui.app.body_rects[1]);
            assert!(l.right() <= r.left() - DIVIDER_WIDTH + 1.0, "left panel overflows at {width}: {l:?} vs {r:?}");
            assert!(r.right() <= width + 0.5, "right panel overflows at {width}: {r:?}");
        }
    }

    #[test]
    fn drive_selector_is_a_compact_dropdown_under_the_panel() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        for panel in [0, 1] {
            ui.app.command(&ui.ctx.clone(), Cmd::DriveMenu(panel));
            ui.frames(3);
            let r = ui.app.drive_popup_rect.expect("drawn");
            let body = ui.app.body_rects[panel];
            assert!(r.width() <= 440.0, "not stretched across the window: {r:?}");
            assert!((r.top() - body.top()).abs() < 2.0 && r.left() >= body.left() - 1.0 && r.left() < body.left() + 20.0, "under panel {panel}: {r:?} vs {body:?}");
            ui.key(Key::Escape);
            assert!(matches!(ui.app.dialog, Dialog::None));
        }
    }

    #[test]
    fn typed_paths_are_remembered_and_offered() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("první"), t.path().join("druhá"));
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        type_path(&mut ui, &a.to_string_lossy());
        type_path(&mut ui, &b.to_string_lossy());
        type_path(&mut ui, &a.to_string_lossy()); // again: moves to the top, no duplicate
        assert_eq!(ui.app.cfg.typed_paths, [display_path(&a), display_path(&b)]);

        // offered after the drives; choosing one opens it
        ui.app.command(&ui.ctx.clone(), Cmd::DriveMenu(0));
        ui.frames(2);
        let Dialog::Drive { drives, conns, paths, cursor, .. } = &mut ui.app.dialog else { panic!() };
        assert_eq!(paths.len(), 2);
        *cursor = drives.len() + conns.len() + 1; // "druhá"
        ui.key(Key::Enter);
        assert_eq!(ui.app.panels[0].path(), b.as_path());
        assert_eq!(ui.app.cfg.typed_paths[0], display_path(&b), "used again: now the most recent");

        // switching the option off forgets them all, and nothing new is stored
        ui.app.cfg.remember_typed_paths = false;
        ui.app.cfg.typed_paths.clear();
        type_path(&mut ui, &a.to_string_lossy());
        assert!(ui.app.cfg.typed_paths.is_empty());
        ui.app.command(&ui.ctx.clone(), Cmd::DriveMenu(0));
        let Dialog::Drive { paths, .. } = &ui.app.dialog else { panic!() };
        assert!(paths.is_empty());
    }

    /// Rows added after the selector was last shown (searches), in a window
    /// that grew meanwhile: it opens as tall as all of them, not as it was.
    #[test]
    fn drive_selector_grows_when_rows_are_added() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.app.cfg.typed_paths = (1..=8).map(|i| format!(r"C:\Projekty\cesta {i}")).collect();
        ui.screen = egui::vec2(1100.0, 420.0);
        ui.frames(2);
        ui.app.command(&ui.ctx.clone(), Cmd::DriveMenu(1));
        ui.frames(5);
        ui.key(Key::Escape);
        ui.screen = egui::vec2(1100.0, 760.0);
        let roots: Vec<PathBuf> = (0..6).map(|n| sal_plugin_api::mount_root("grows", &n.to_string())).collect();
        for r in &roots {
            plugins::add_mount(r, "grows", r, Some(("hledání", "refresh")));
        }
        ui.frames(2);
        ui.app.command(&ui.ctx.clone(), Cmd::DriveMenu(1));
        ui.frames(4);
        let popup = ui.app.drive_popup_rect.unwrap();
        let rows: f32 = ui.ctx.data(|d| d.get_temp(egui::Id::new("drive_selector_rows"))).unwrap();
        assert!(popup.height() >= rows, "every row shown: {popup:?} for {rows}");
        assert!(rows > 400.0, "more than egui's default area height: {rows}");
        ui.key(Key::Escape);
        for r in &roots {
            plugins::remove_mount(r);
        }
    }

    /// A long drive list grows as tall as its rows, moving up over the
    /// window when the room below the path bar isn't enough; it scrolls only
    /// when it is taller than the whole window, keeping the chosen row in view.
    #[test]
    fn drive_selector_grows_before_it_scrolls() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        let open = |ui: &mut Ui, h: f32, paths: usize| {
            ui.screen = egui::vec2(900.0, h);
            ui.app.cfg.typed_paths = (0..paths).map(|i| format!(r"C:\cesta {i}")).collect();
            ui.frames(2);
            ui.app.command(&ui.ctx.clone(), Cmd::DriveMenu(1));
            ui.frames(4);
            ui.app.drive_popup_rect.expect("drawn")
        };

        // fits the window: every row shown, moved up as far as needed
        let popup = open(&mut ui, 700.0, 12);
        let body = ui.app.body_rects[1];
        let need = popup.height();
        assert!(popup.top() >= -0.5 && popup.bottom() <= 700.5, "{popup:?}");
        assert!(popup.bottom() > body.top() + 10.0 && need > 12.0 * 20.0, "all rows: {popup:?}");
        ui.key(Key::Escape);
        // a lower window: the list moves up instead of scrolling (other tests
        // may add connections meanwhile, so it is measured again)
        let popup = open(&mut ui, 420.0, 12);
        let rows: f32 = ui.ctx.data(|d| d.get_temp(egui::Id::new("drive_selector_rows"))).unwrap();
        assert!(need > 0.0 && popup.top() >= -0.5 && popup.bottom() <= 420.5, "{popup:?}");
        if rows + 24.0 < 420.0 {
            assert!(popup.height() >= rows, "every row, no scrolling: {popup:?} for {rows}");
            assert!(popup.top() < ui.app.body_rects[1].top() - 1.0, "moved up: {popup:?}");
        } else {
            assert!(popup.height() > 380.0, "the whole window: {popup:?}");
        }
        ui.key(Key::Escape);

        // taller than the window: the whole window, and it scrolls
        let popup = open(&mut ui, 320.0, 40);
        assert!(popup.top() >= -0.5 && popup.bottom() <= 320.5 && popup.height() > 280.0, "{popup:?}");
        for _ in 0..60 {
            ui.key(Key::ArrowDown);
        }
        ui.frames(3);
        let popup = ui.app.drive_popup_rect.expect("drawn");
        assert!(popup.top() >= -0.5 && popup.bottom() <= 320.5, "{popup:?}");
        let Dialog::Drive { drives, conns, paths, cursor, .. } = &ui.app.dialog else { panic!() };
        assert_eq!(*cursor, drives.len() + conns.len() + paths.len(), "on the last row, From left panel");
    }

    #[cfg(windows)]
    #[test]
    fn choosing_fonts_applies_and_is_saved() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.app.dialog = Dialog::Settings { tab: SettingsTab::General, recording: None };
        ui.frames(2);
        assert!(ui.app.font_list.iter().any(|f| f.name == "Arial"), "installed fonts listed");
        // what the picker does on a choice
        ui.app.cfg.ui_font = "Arial".into();
        ui.app.cfg.mono_font = "Courier New".into();
        crate::fonts::install(&ui.ctx, &ui.app.cfg.ui_font, &ui.app.cfg.mono_font);
        ui.frames(2);
        let fams = ui.ctx.fonts(|f| f.families());
        assert!(fams.contains(&egui::FontFamily::Proportional));
        assert_eq!((ui.app.cfg.ui_font.as_str(), ui.app.cfg.mono_font.as_str()), ("Arial", "Courier New"), "stored in the config");
    }

    /// `cargo test -p sal-app snapshot_fonts -- --ignored` (set SNAPSHOT_DIR)
    #[test]
    #[ignore]
    fn snapshot_fonts() {
        let out_dir = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let t = tempfile::tempdir().unwrap();
        menu_files(t.path());
        let mut ui = Ui::new(t.path(), t.path());
        ui.app.cfg.ui_font = "Georgia".into();
        ui.app.cfg.mono_font = "Cascadia Mono".into();
        crate::fonts::install(&ui.ctx, "Georgia", "Cascadia Mono");
        ui.ctx.set_pixels_per_point(1.5);
        ui.app.dialog = Dialog::Settings { tab: SettingsTab::General, recording: None };
        ui.frames(20);
        let bg = ui.ctx.style().visuals.panel_fill;
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("fonts.png"));
    }

    #[cfg(windows)]
    #[test]
    fn font_search_field_keeps_the_list_open() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.app.dialog = Dialog::Settings { tab: SettingsTab::General, recording: None };
        ui.frames(20);
        let button = ui.app.font_button_rect.expect("font picker drawn");
        click(&mut ui, button.center());
        assert!(ui.ctx.memory(|m| m.any_popup_open()), "list opens");
        // the search field sits right below the button: clicking it keeps the list open
        click(&mut ui, button.left_bottom() + egui::vec2(60.0, 16.0));
        assert!(ui.ctx.memory(|m| m.any_popup_open()), "clicking the search field must not close the list");
        ui.text("aria");
        assert_eq!(ui.app.font_filter, "aria");
        assert!(ui.ctx.memory(|m| m.any_popup_open()));
        // Enter in the search box doesn't close Settings; Esc closes only the list
        ui.key(Key::Enter);
        assert!(matches!(ui.app.dialog, Dialog::Settings { .. }));
        if !ui.ctx.memory(|m| m.any_popup_open()) {
            click(&mut ui, button.center());
        }
        ui.key(Key::Escape);
        assert!(!ui.ctx.memory(|m| m.any_popup_open()), "Esc closes the list");
        assert!(matches!(ui.app.dialog, Dialog::Settings { .. }), "…but not Settings");
        assert!(ui.app.cfg.ui_font.is_empty(), "nothing chosen");
    }

    #[test]
    fn custom_viewer_replaces_the_built_in_one() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("a.txt"), "x").unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.text("a.t");
        // the plugin's viewer by default
        ui.key(Key::F3);
        assert_eq!(ui.viewed().len(), 1);
        // a custom viewer is started instead (here one that doesn't exist: an error, no plugin window)
        ui.app.cfg.viewer = r#""C:\No Such Dir\viewer.exe""#.into();
        ui.app.cfg.view_with = OpenWith::Program { command: ui.app.cfg.viewer.clone() };
        ui.key(Key::F3);
        assert_eq!(ui.viewed().len(), 1);
        assert!(matches!(&ui.app.dialog, Dialog::Message { text, .. } if text.contains("viewer.exe")), "reports the program that failed");
        // switching back to the built-in one keeps the custom command for later
        ui.key(Key::Escape);
        ui.app.cfg.view_with = OpenWith::Plugin { plugin: "recorder".into(), opener: "view".into() };
        ui.key(Key::F3);
        assert_eq!(ui.viewed().len(), 2);
        assert!(!ui.app.cfg.viewer.is_empty());
    }

    /// A suffix with its own viewer and editor overrides the defaults; the
    /// Settings form adds, checks and changes such suffixes.
    #[test]
    fn suffixes_with_their_own_viewer_and_editor() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("a.txt"), "x").unwrap();
        fs::write(t.path().join("Poznámky.MD"), "x").unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        let recorder = |opener: &str| OpenWith::Plugin { plugin: "recorder".into(), opener: opener.into() };
        // by default nothing opens .md specially; the default viewer is a missing program here
        ui.app.cfg.view_with = OpenWith::Program { command: r#""C:\No Such Dir\view.exe""#.into() };
        ui.app.cfg.suffix_rules = vec![SuffixRule { suffix: "md".into(), view: Some(recorder("view")), edit: Some(recorder("edit")) }];
        assert_eq!(ui.app.open_with_for(OpenKind::View, &t.path().join("Poznámky.MD")), recorder("view"));
        assert_eq!(ui.app.open_with_for(OpenKind::Edit, &t.path().join("Poznámky.MD")), recorder("edit"));
        assert_eq!(ui.app.open_with_for(OpenKind::View, &t.path().join("a.txt")), ui.app.cfg.view_with);
        ui.text("Pozn");
        ui.key(Key::F3);
        ui.key(Key::F4);
        let opened: Vec<String> = ui.recorder.opened.lock().unwrap().iter().map(|(o, _, _)| o.clone()).collect();
        assert_eq!(opened, ["view", "edit"]);
        assert!(matches!(ui.app.dialog, Dialog::None));
        // a rule's "Default" uses the default
        ui.app.cfg.suffix_rules[0].view = None;
        assert_eq!(ui.app.open_with_for(OpenKind::View, &t.path().join("x.md")), ui.app.cfg.view_with);
        // a chosen plugin that is gone gives way to one that views the file
        ui.app.cfg.suffix_rules[0].view = Some(OpenWith::Plugin { plugin: "gone".into(), opener: "x".into() });
        ui.key(Key::F3);
        assert_eq!(ui.viewed().len(), 2);

        // the form: checks, then saves
        let rules = ui.app.cfg.suffix_rules.clone();
        let mut form = SuffixEdit::of(None, SuffixRule { suffix: " *.MD ".into(), view: None, edit: Some(OpenWith::Notepad) }, "", "");
        assert!(form.check(&rules).unwrap_err().contains(".md"), "taken");
        form.rule.suffix = "".into();
        assert!(form.check(&rules).is_err(), "empty");
        form.rule.suffix = ".Log".into();
        form.rule.edit = None;
        assert!(form.check(&rules).is_err(), "neither");
        form.rule.view = Some(OpenWith::Notepad);
        assert_eq!(form.check(&rules).unwrap().suffix, "log");
        // the same suffix may be saved over itself
        let again = SuffixEdit::of(Some(0), rules[0].clone(), "", "");
        assert!(again.check(&rules).is_ok());

        // the tab draws, with the table and the form open
        ui.key(Key::Escape);
        ui.app.suffix_edit = Some(form);
        ui.app.dialog = Dialog::Settings { tab: SettingsTab::Programs, recording: None };
        ui.frames(6);
        assert!(matches!(ui.app.dialog, Dialog::Settings { .. }));
        ui.app.dialog = Dialog::None;
        ui.app.suffix_edit = None;
    }

    /// The accent color is the selection color everywhere and tints the
    /// plugin panels; plugins' windows are told it. The setting draws with
    /// its color button and preview. SNAPSHOT_DIR set: also saved as
    /// accent_color.png.
    #[test]
    fn accent_color() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("a.txt"), "x").unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        crate::fonts::install(&ui.ctx, "", "");
        ui.screen = egui::vec2(1100.0, 1000.0);
        ui.app.cfg.accent_light = Some([40, 90, 200]);
        ui.app.cfg.accent_dark = Some([220, 120, 30]);
        sal_ui::apply_accent(&ui.ctx, ui.app.cfg.accent_light, ui.app.cfg.accent_dark);
        ui.ctx.set_theme(egui::Theme::Dark);
        ui.frames(2);
        let v = ui.ctx.style().visuals.clone();
        assert_eq!(v.selection.bg_fill, Color32::from_rgb(220, 120, 30), "the dark theme's");
        let tint = plugin_tint(&v);
        assert!(tint.r() > tint.g() && tint.g() > tint.b(), "{tint:?}");
        ui.text("a.t");
        ui.key(Key::F3);
        {
            let opened = ui.recorder.opened.lock().unwrap();
            let o = &opened[0].2;
            assert_eq!((o["accent_light"].as_str(), o["accent_dark"].as_str()), ("#285ac8", "#dc781e"));
        }
        ui.key(Key::Escape);
        ui.app.dialog = Dialog::Settings { tab: SettingsTab::General, recording: None };
        ui.frames(10);
        assert!(matches!(ui.app.dialog, Dialog::Settings { .. }));
        if let Some(dir) = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from) {
            let bg = ui.ctx.style().visuals.panel_fill;
            let out = ui.last.take().unwrap();
            ui.snap.render(&ui.ctx, out, bg, &dir.join("accent_color.png"));
        }
        ui.app.dialog = Dialog::None;
        // the light theme has its own
        ui.ctx.set_theme(egui::Theme::Light);
        ui.frames(1);
        assert_eq!(ui.ctx.style().visuals.selection.bg_fill, Color32::from_rgb(40, 90, 200), "the light theme's");
        sal_ui::apply_accent(&ui.ctx, None, None);
    }

    #[test]
    fn external_programs_section_draws() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.screen = egui::vec2(1100.0, 1400.0);
        ui.app.dialog = Dialog::Settings { tab: SettingsTab::General, recording: None };
        ui.frames(20);
        assert!(matches!(ui.app.dialog, Dialog::Settings { .. }));
    }

    /// `cargo test -p sal-app snapshot_external_programs -- --ignored` (set SNAPSHOT_DIR)
    #[test]
    #[ignore]
    fn snapshot_external_programs() {
        let out_dir = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        crate::fonts::install(&ui.ctx, "", "");
        ui.ctx.set_pixels_per_point(1.5);
        ui.screen = egui::vec2(1100.0, 1400.0);
        ui.app.cfg.editor = r#""C:\Program Files\Notepad++\notepad++.exe""#.into();
        ui.app.cfg.edit_with = OpenWith::Program { command: ui.app.cfg.editor.clone() };
        ui.app.cfg.view_with = OpenWith::Plugin { plugin: "recorder".into(), opener: "view".into() };
        ui.app.cfg.suffix_rules = vec![
            SuffixRule { suffix: "md".into(), view: Some(OpenWith::Program { command: "typora.exe".into() }), edit: None },
            SuffixRule { suffix: "ini".into(), view: None, edit: Some(OpenWith::Notepad) },
        ];
        ui.app.suffix_edit = Some(SuffixEdit::of(Some(0), ui.app.cfg.suffix_rules[0].clone(), "", ""));
        ui.app.dialog = Dialog::Settings { tab: SettingsTab::Programs, recording: None };
        ui.frames(25);
        let bg = ui.ctx.style().visuals.panel_fill;
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("viewers_and_editors.png"));
    }

    /// Main window, Settings and the keyboard table in Czech:
    /// `cargo test -p sal-app snapshot_czech -- --ignored` (set SNAPSHOT_DIR)
    #[test]
    #[ignore]
    fn snapshot_czech() {
        let out_dir = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let t = tempfile::tempdir().unwrap();
        sized_tree(t.path());
        if let Some(dir) = built_library() {
            std::env::set_var("RUSTYSALAMANDER_PLUGINS", dir);
        }
        let mut ui = Ui::new(t.path(), t.path());
        crate::fonts::install(&ui.ctx, "", "");
        ui.app.set_language("cs");
        ui.screen = egui::vec2(1100.0, 700.0);
        ui.frames(10);
        let bg = ui.ctx.style().visuals.panel_fill;
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("cs_main.png"));
        ui.screen = egui::vec2(1100.0, 1400.0);
        for (tab, name) in [(SettingsTab::General, "cs_settings.png"), (SettingsTab::Keyboard, "cs_keys.png"), (SettingsTab::Plugins, "cs_plugins.png")] {
            ui.app.dialog = Dialog::Settings { tab, recording: None };
            ui.frames(25);
            let out = ui.last.take().unwrap();
            ui.snap.render(&ui.ctx, out, bg, &out_dir.join(name));
        }
    }

    /// A connection in a plugin panel (tinted, with its banner) in both
    /// themes, and the SCP plugin's connect form:
    /// `cargo test -p sal-app snapshot_plugin_panel -- --ignored`
    #[test]
    #[ignore]
    fn snapshot_plugin_panel() {
        let out_dir = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let t = tempfile::tempdir().unwrap();
        sized_tree(t.path());
        let server = Arc::new(MemServer {
            files: std::sync::Mutex::new(
                [("home\\zpráva 🦎.txt".to_owned(), b"ahoj".to_vec()), ("home\\docs\\a.txt".to_owned(), b"a".to_vec()), ("home\\.bashrc".to_owned(), b"x".to_vec())].into(),
            ),
            closed: std::sync::Mutex::new(Vec::new()),
        });
        let mut ui = Ui::new(t.path(), t.path());
        crate::fonts::install(&ui.ctx, "", "");
        ui.screen = egui::vec2(1100.0, 600.0);
        let plugin: Arc<dyn FolderPlugin> = server;
        ui.app.plugins.active.push(plugin.clone());
        for p in &mut ui.app.panels {
            p.plugins = ui.app.plugins.active.clone();
        }
        ui.app.open_connection(&plugin, &plugins::Mount { scheme: "scp".into(), authority: "me@example.org".into(), folder: "home".into() });
        for (theme, name) in [(ThemeMode::Light, "plugin_panel_light.png"), (ThemeMode::Dark, "plugin_panel_dark.png")] {
            ui.app.cfg.theme = theme;
            apply_theme(&ui.ctx, theme);
            ui.frames(10);
            let bg = ui.ctx.style().visuals.panel_fill;
            let out = ui.last.take().unwrap();
            ui.snap.render(&ui.ctx, out, bg, &out_dir.join(name));
        }
        // a custom tint, on the inactive panel too
        ui.app.cfg.accent_light = Some([220, 120, 30]);
        ui.app.cfg.accent_dark = Some([220, 120, 30]);
        sal_ui::apply_accent(&ui.ctx, ui.app.cfg.accent_light, ui.app.cfg.accent_dark);
        ui.app.active = 1;
        ui.frames(10);
        let bg = ui.ctx.style().visuals.panel_fill;
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("plugin_panel_tint.png"));
        (ui.app.cfg.accent_light, ui.app.cfg.accent_dark) = (None, None);
        sal_ui::apply_accent(&ui.ctx, None, None);
        ui.app.active = 0;
        // the connection in the other panel's drive selector, in a low window
        ui.app.cfg.theme = ThemeMode::Light;
        apply_theme(&ui.ctx, ThemeMode::Light);
        ui.app.cfg.typed_paths = (1..=8).map(|i| format!(r"C:\Projekty\cesta {i}")).collect();
        ui.app.active = 1;
        ui.app.open_connection(
            &plugin,
            &plugins::Mount { scheme: "scp".into(), authority: "administrator@a-rather-long-server-name.example.org:2222".into(), folder: "home".into() },
        );
        ui.app.panels[1].load(t.path(), ui.app.vis(), None).unwrap();
        ui.screen = egui::vec2(1100.0, 420.0);
        ui.frames(3);
        ui.app.command(&ui.ctx.clone(), Cmd::DriveMenu(1));
        ui.frames(5);
        let bg = ui.ctx.style().visuals.panel_fill;
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("plugin_drive_menu.png"));
        ui.key(Key::Escape);
        // two searches: their own section, with Refresh and Close
        ui.screen = egui::vec2(1100.0, 760.0);
        let mut search_roots = Vec::new();
        for (n, what) in [(91, "*.txt \"faktura\" · Dokumenty"), (92, "*.jpg · Fotky")] {
            let r = plugins::Results {
                title: what.into(),
                name: what.into(),
                mount: plugins::Mount { scheme: "snapsearch".into(), authority: n.to_string(), folder: String::new() },
                flat: format!("{n}-list"),
                again: String::new(),
                refresh: "refresh".into(),
            };
            search_roots.push(r.mount.root());
            search_roots.push(plugins::Mount { authority: r.flat.clone(), ..r.mount.clone() }.root());
            ui.app.open_results(plugin.clone(), r);
            // as if a panel showed it, so the next search keeps it
            ui.app.results = None;
        }
        ui.frames(2);
        ui.app.command(&ui.ctx.clone(), Cmd::DriveMenu(1));
        ui.frames(6);
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("drive_menu_searches.png"));
        ui.key(Key::Escape);
        // a search in a panel: the path bar and the banner name it
        let hidden = ui.app.vis();
        ui.app.panels[1].load(&search_roots[0].join("home"), hidden, None).unwrap();
        ui.frames(4);
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("search_panel.png"));
        ui.app.panels[1].load(t.path(), hidden, None).unwrap();
        for r in &search_roots {
            ui.app.close_connection(r);
        }
        ui.screen = egui::vec2(1100.0, 420.0);
        // no recent paths, then one session disconnected from the selector
        ui.app.cfg.typed_paths.clear();
        ui.app.command(&ui.ctx.clone(), Cmd::DriveMenu(1));
        ui.frames(5);
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("plugin_drive_menu_short.png"));
        if let Dialog::Drive { drives, conns, cursor, .. } = &mut ui.app.dialog {
            *cursor = drives.len() + conns.len() - 1;
        }
        ui.frames(2);
        ui.key(Key::Delete);
        ui.frames(5);
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("plugin_drive_menu_after.png"));
        ui.key(Key::Escape);
        // the Plugins menu, opened with a click on its title
        click(&mut ui, egui::pos2(262.0, 12.0));
        ui.frames(5);
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("plugin_menu.png"));
        ui.key(Key::Escape);
        ui.frames(2);
        ui.screen = egui::vec2(1100.0, 600.0);
        // the SCP plugin's form, drawn by the program
        let Some(dir) = built_library() else { return };
        let mut found = Plugins::scan(Some(&dir));
        found.start(&[]);
        let Some(scp) = found.get("scp").cloned() else { return };
        ui.app.cfg.theme = ThemeMode::Light;
        apply_theme(&ui.ctx, ThemeMode::Light);
        let ctx = ui.ctx.clone();
        ui.app.run_plugin_command(&ctx, scp, "connect".into(), plugins::Values::new());
        wait_plugin(&mut ui);
        ui.frames(25);
        let bg = ui.ctx.style().visuals.panel_fill;
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("plugin_form.png"));
        // the Search plugin's form
        ui.app.dialog = Dialog::None;
        ui.frames(3);
        let Some(search) = found.get("search").cloned() else { return };
        ui.screen = egui::vec2(1100.0, 760.0);
        ui.app.run_plugin_command(&ctx, search, "find".into(), plugins::Values::new());
        wait_plugin(&mut ui);
        ui.frames(25);
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("search_form.png"));
        // the FTP / SFTP plugin's form
        ui.app.dialog = Dialog::None;
        ui.frames(3);
        let Some(ftp) = found.get("ftp").cloned() else { return };
        ui.screen = egui::vec2(1100.0, 760.0);
        ui.app.run_plugin_command(&ctx, ftp, "connect".into(), plugins::Values::new());
        wait_plugin(&mut ui);
        ui.frames(25);
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("ftp_form.png"));
        // FTP picked after a private key: what doesn't apply is greyed out
        if let Dialog::PluginForm { values, .. } = &mut ui.app.dialog {
            values.insert("protocol".into(), "ftp".into());
            values.insert("auth".into(), "key".into());
        }
        ui.frames(5);
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("ftp_form_ftp.png"));
    }

    /// The operations window (a paused copy and two queued operations) and a
    /// copy conflict: `cargo test -p sal-app snapshot_operations -- --ignored`
    #[test]
    #[ignore]
    fn snapshot_operations() {
        let out_dir = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = (t.path().join("zdroj"), t.path().join("cíl"));
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("video.mkv"), vec![0u8; 3_000_000]).unwrap();
        fs::write(src.join("zpráva.docx"), vec![1u8; 2_000]).unwrap();
        fs::write(dst.join("zpráva.docx"), vec![1u8; 1_000]).unwrap();
        let mut ui = Ui::new(&src, &dst);
        crate::fonts::install(&ui.ctx, "", "");
        ui.screen = egui::vec2(1100.0, 800.0);
        let ctx = ui.ctx.clone();
        let copy = JobKind::Transfer(OpKind::Copy);
        ui.app.jobs.add_paused(&ctx, copy, vec![src.join("video.mkv")], dst.clone());
        ui.app.jobs.add(&ctx, JobKind::Transfer(OpKind::Move), vec![src.join("video.mkv"), src.join("zpráva.docx")], t.path().join("archiv"), Placement::Last);
        ui.app.jobs.add(&ctx, JobKind::Delete, vec![src.join("zpráva.docx")], PathBuf::new(), Placement::Last);
        ui.frames(20);
        let bg = ui.ctx.style().visuals.panel_fill;
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("operations.png"));

        // a conflict: the target already has a smaller, older zpráva.docx
        ui.app.jobs = Jobs::default();
        let old = std::time::SystemTime::now() - Duration::from_secs(86_400 * 30);
        fs::File::options().write(true).open(dst.join("zpráva.docx")).unwrap().set_modified(old).unwrap();
        ui.app.jobs.add(&ctx, copy, vec![src.join("zpráva.docx")], dst.clone(), Placement::Now);
        for _ in 0..200 {
            ui.frames(1);
            if ui.app.jobs.running.first().is_some_and(|j| j.prompt.is_some()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        ui.frames(20);
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("conflict.png"));
        ui.app.jobs.running[0].cancel();
        ui.wait_job();

        // the Copy dialog with its options: in the background, safe copy with checksums
        ui.app.open_dialog(Dialog::Transfer { kind: OpKind::Move, sources: vec![src.join("video.mkv")], target: display_path(&dst), opts: TransferDefaults { background: true, show_result: false, verify: Verify::Checksum } });
        ui.frames(20);
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("transfer_options.png"));
        ui.app.dialog = Dialog::None;

        // the result window: a safe copy that went well, one with problems
        let spec = |id, kind| Spec { id, kind, sources: vec![src.join("video.mkv"), src.join("zpráva.docx")], target: dst.clone(), plugin: None, options: JobOptions::default() };
        let ok = OpSummary {
            files: 2,
            bytes: 3_002_000,
            verify: Verify::Checksum,
            verified: 2,
            checksum_files: vec![dst.join("RustySalamander 2026-11-03 14-25-01.sha256"), src.join("RustySalamander 2026-11-03 14-25-01.sha256")],
            ..Default::default()
        };
        let bad = OpSummary {
            files: 1,
            bytes: 3_000_000,
            skipped: 1,
            verify: Verify::Size,
            verified: 1,
            verify_failed: 1,
            problems: vec![(dst.join("zpráva.docx"), "verification failed: the copy has 1000 bytes, the original 2000".into())],
            ..Default::default()
        };
        ui.app.reports.push(Report { spec: spec(10, JobKind::Transfer(OpKind::Copy)), result: Ok(ok), elapsed: Duration::from_secs(12), ended: "14:25:01".into() });
        ui.app.reports.push(Report { spec: spec(11, JobKind::Transfer(OpKind::Move)), result: Ok(bad), elapsed: Duration::from_secs(3), ended: "14:31:40".into() });
        ui.frames(20);
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("operation_result.png"));
    }

    fn qs_files(t: &Path) {
        for n in ["alfa report.txt", "beta.txt", "Report 2026.xlsx", "gama.txt", "annual.doc", "zeta.txt"] {
            fs::write(t.join(n), "").unwrap();
        }
    }

    #[test]
    fn quick_search_anywhere_in_name() {
        let t = tempfile::tempdir().unwrap();
        qs_files(t.path());
        let mut ui = Ui::new(t.path(), t.path());
        // from the first letter (default): no name starts with "port"; keys that
        // match nothing are rejected one by one (only "r" of "Report" sticks)
        ui.text("port");
        assert_eq!(ui.app.panels[0].qs, "r");
        assert_eq!(ui.cursor_name(), "Report 2026.xlsx");
        ui.key(Key::Escape);
        // anywhere: finds "alfa report.txt" first, Ctrl+↓ the next one
        ui.app.cfg.quick_search_anywhere = true;
        ui.key(Key::Home);
        ui.text("port");
        assert_eq!(ui.app.panels[0].qs, "port");
        assert_eq!(ui.cursor_name(), "alfa report.txt");
        chord(&mut ui, Key::ArrowDown, Modifiers::CTRL, None);
        assert_eq!(ui.cursor_name(), "Report 2026.xlsx");
    }

    #[test]
    fn quick_search_without_diacritics() {
        let t = tempfile::tempdir().unwrap();
        for n in ["alfa.txt", "vládče.txt", "Grüße.txt"] {
            fs::write(t.path().join(n), "").unwrap();
        }
        let mut ui = Ui::new(t.path(), t.path());
        assert!(ui.app.cfg.quick_search_loose, "on by default");
        ui.text("vladce");
        assert_eq!(ui.app.panels[0].qs, "vladce");
        assert_eq!(ui.cursor_name(), "vládče.txt");
        ui.key(Key::Escape);
        ui.text("grusse");
        assert_eq!(ui.cursor_name(), "Grüße.txt");
        ui.key(Key::Escape);
        // exact letters: the plain "c" stops at "vládč"
        ui.app.cfg.quick_search_loose = false;
        ui.key(Key::Home);
        ui.text("vládce");
        assert_eq!(ui.app.panels[0].qs, "vlád");
    }

    #[test]
    fn switching_language_translates_the_ui() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let mut ui = Ui::new(dir.path(), dir.path());
        ui.app.command(&ui.ctx.clone(), Cmd::ToggleQuickSearchMode);
        assert_eq!(ui.app.status, "Quick search: anywhere in name");

        ui.app.set_language("cs");
        assert_eq!(ui.app.cfg.language, "cs", "remembered in the config");
        ui.app.command(&ui.ctx.clone(), Cmd::ToggleQuickSearchMode);
        assert_eq!(ui.app.status, "Rychlé hledání: od prvního písmena");
        assert_eq!(plural(3, "file"), "3 soubory");
        assert_eq!(plural(5, "file"), "5 souborů");

        ui.app.set_language("en");
        assert_eq!(plural(3, "file"), "3 files");
    }

    #[test]
    fn arrows_jump_between_matches_when_enabled() {
        let t = tempfile::tempdir().unwrap();
        qs_files(t.path());
        let mut ui = Ui::new(t.path(), t.path());
        // (sorted: alfa report, annual, beta, gama, Report 2026, zeta)
        assert!(ui.app.cfg.quick_search_arrows, "on by default");

        // from the first letter: arrows stay on names starting with the search
        ui.text("a");
        assert_eq!(ui.cursor_name(), "alfa report.txt");
        ui.key(Key::ArrowDown);
        assert_eq!(ui.cursor_name(), "annual.doc");
        ui.key(Key::ArrowDown);
        assert_eq!(ui.cursor_name(), "alfa report.txt", "beta… are skipped, wraps around");
        assert_eq!(ui.app.panels[0].qs, "a");
        ui.key(Key::Escape);

        // anywhere in the name: the same, across non-matching rows
        ui.app.cfg.quick_search_anywhere = true;
        ui.key(Key::Home);
        ui.text("rep");
        assert_eq!(ui.cursor_name(), "alfa report.txt");
        ui.key(Key::ArrowDown);
        assert_eq!(ui.cursor_name(), "Report 2026.xlsx", "annual, beta and gama are skipped");
        assert_eq!(ui.app.panels[0].qs, "rep");
        ui.key(Key::ArrowDown);
        assert_eq!(ui.cursor_name(), "alfa report.txt", "wraps around");
        ui.key(Key::ArrowUp);
        assert_eq!(ui.cursor_name(), "Report 2026.xlsx");
        // Esc ends the search; arrows move normally again
        ui.key(Key::Escape);
        ui.key(Key::ArrowDown);
        assert!(ui.app.panels[0].qs.is_empty());

        // with them on, Page Down / End end the search
        ui.key(Key::Home);
        ui.text("rep");
        ui.key(Key::End);
        assert!(ui.app.panels[0].qs.is_empty());

        // switched off: the cursor moves one row at a time, and the search
        // (its highlight) stays until Esc
        ui.app.cfg.quick_search_arrows = false;
        ui.key(Key::Home);
        ui.text("rep");
        ui.key(Key::ArrowDown);
        assert_eq!(ui.cursor_name(), "annual.doc");
        assert_eq!(ui.app.panels[0].qs, "rep");
        ui.key(Key::ArrowDown);
        assert_eq!(ui.cursor_name(), "beta.txt");
        ui.key(Key::End);
        ui.key(Key::Home);
        ui.key(Key::PageDown);
        assert_eq!(ui.app.panels[0].qs, "rep", "kept while the cursor moves");
        // Ctrl+arrows still jump to a match
        ui.key(Key::Home);
        ui.key(Key::ArrowDown);
        ui.key_mod(Key::ArrowDown, Modifiers::CTRL);
        assert_eq!(ui.cursor_name(), "Report 2026.xlsx");
        // typing goes on from where the cursor is
        ui.text("o");
        assert_eq!(ui.app.panels[0].qs, "repo");
        assert_eq!(ui.cursor_name(), "Report 2026.xlsx");
        ui.key(Key::Escape);
        assert!(ui.app.panels[0].qs.is_empty());
    }

    #[test]
    fn quick_search_mode_shortcut() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        assert!(keymap::bindings(&ui.app.cfg.shortcuts, Action::ToggleQuickSearchMode).is_empty(), "no key by default");
        let preset = keymap::presets(Action::ToggleQuickSearchMode).into_iter().find(|(l, _)| l == "Ctrl+Shift+Q").unwrap().1;
        keymap::set_bindings(&mut ui.app.cfg.shortcuts, Action::ToggleQuickSearchMode, preset);
        chord(&mut ui, Key::Q, Modifiers::CTRL | Modifiers::COMMAND | Modifiers::SHIFT, None);
        assert!(ui.app.cfg.quick_search_anywhere);
        assert_eq!(ui.app.status, "Quick search: anywhere in name");
        chord(&mut ui, Key::Q, Modifiers::CTRL | Modifiers::COMMAND | Modifiers::SHIFT, None);
        assert!(!ui.app.cfg.quick_search_anywhere);
    }

    /// `cargo test -p sal-app snapshot_quick_search -- --ignored` (set SNAPSHOT_DIR)
    #[test]
    #[ignore]
    fn snapshot_quick_search() {
        let out_dir = std::env::var_os("SNAPSHOT_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let t = tempfile::tempdir().unwrap();
        qs_files(t.path());
        let mut ui = Ui::new(t.path(), t.path());
        crate::fonts::install(&ui.ctx, "", "");
        ui.ctx.set_pixels_per_point(1.5);
        ui.app.cfg.quick_search_anywhere = true;
        ui.text("port");
        ui.frames(3);
        let bg = ui.ctx.style().visuals.panel_fill;
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("qs_anywhere.png"));
        ui.key(Key::Escape);
        // a match running into the extension, in every row that has it
        ui.text(".tx");
        ui.frames(3);
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("qs_ext.png"));
        ui.key(Key::Escape);
        click(&mut ui, egui::pos2(250.0, 10.0)); // Options menu
        ui.frames(3);
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("options_menu.png"));
    }
}
