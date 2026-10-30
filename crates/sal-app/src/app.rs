//! Main window: two panels, menu, function-key bar, dialogs
//! (replaces `mainwnd*.cpp`, `dialogs*.cpp`, `menu*.cpp`).

use std::collections::HashMap;
use std::path::{Path, PathBuf, MAIN_SEPARATOR};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, text::LayoutJob, Align, Color32, Event, Key, Layout, Modifiers, RichText, Sense, TextFormat};
use egui_extras::{Column, TableBuilder};
use sal_core::config::{Config, ContextMenuStyle, ContextMenuTrigger, DriveButtons, DriveHotkey, PanelConfig, ThemeMode};
use sal_core::drives::{self, Drive};
use sal_core::encoding::{display_name, display_path, format_size, format_thousands, visual_order};
use sal_core::fileops::{self, ConflictAction, ErrorAction, OpKind};
use sal_core::listing::Visibility;
use sal_core::location::{self, Target};
use sal_core::shell::TerminalKind;
use sal_core::{EntryKind, FileEntry, MaskGroup, SortKey};

use crate::ctxmenu::{self, MenuAction};
use crate::dialogwin::{self, DialogWin};
use crate::dirsize::DirSizes;
use crate::drive_icon;
use crate::i18n::{n_, tr, trf, trn};
use crate::icons::Icons;
use crate::keymap::{self, Action, Section};
use crate::panel::Panel;
use crate::plugins::ops::{FolderOp, OpenAs};
use crate::plugins::{self, FolderPlugin, Plugins};
use crate::shellmenu::{self, Picked};
use crate::shortcut::{key_letter, Shortcut};
use crate::viewer::Viewer;
use crate::worker::{format_duration, Job, JobKind, Jobs, Placement, PluginRef, Prompt, Reply};

const SELECTED_COLOR: Color32 = Color32::from_rgb(230, 40, 40);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cmd {
    Help,
    CheckUpdates,
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
    /// A newer release; `install` while (or after) it is being installed.
    Update {
        release: crate::update::Release,
        install: Option<crate::update::Install>,
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
    Plugins,
}

/// A plugin as Settings shows it.
struct PluginRow {
    id: String,
    name: String,
    description: String,
    file: String,
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
    viewers: Vec<Viewer>,
    viewer_serial: u64,
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
    ctx_menu: Option<CtxMenu>,
    rmb: Option<RightPress>,
    /// Row rectangles of the last frame (screen points), per panel.
    row_hits: [Vec<(egui::Rect, usize)>; 2],
    /// File list area of each panel.
    body_rects: [egui::Rect; 2],
    /// Width each panel's table was laid out for.
    table_widths: [f32; 2],
    /// Free space of the panels' drives, read in the background.
    free: crate::freespace::FreeSpace,
}

/// An open Windows 11 style context menu.
struct CtxMenu {
    panel: usize,
    /// Items it applies to; empty for the folder background.
    items: Vec<PathBuf>,
    dir: PathBuf,
    pos: egui::Pos2,
    can_paste: bool,
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

fn plural_dirs(n: usize) -> String {
    trn("{n} directory", "{n} directories", n as u64)
}

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

fn visibility(cfg: &Config) -> Visibility {
    Visibility { hidden: cfg.show_hidden, system: cfg.show_system }
}

fn fmt_time(t: Option<std::time::SystemTime>) -> String {
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

fn describe(sources: &[PathBuf]) -> String {
    match sources {
        [one] => format!("\"{}\"", one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        many => trn("{n} selected item", "{n} selected items", many.len() as u64),
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, args: Vec<String>) -> App {
        crate::debuglog::write("startup: window ready");
        let cfg = crate::debuglog::time("startup: configuration", Config::load);
        crate::debuglog::write(format!("startup: configuration file {}", sal_core::config::config_path().display()));
        crate::debuglog::time("startup: fonts", || crate::fonts::install(&cc.egui_ctx, &cfg.ui_font, &cfg.mono_font));
        App::with_config(&cc.egui_ctx, cfg, args)
    }

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
            apply_scrollbars(ctx, cfg.always_show_scrollbars);
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
        let update_check = (cfg.check_updates && !cfg!(test)).then(|| crate::update::Check::start(ctx, false));
        App {
            cfg,
            update_check,
            panels,
            active,
            dialog: Dialog::None,
            focus_dialog: false,
            last_mask: "*.*".into(),
            jobs: Jobs::default(),
            plugins,
            plugin_task: None,
            queue_place: Placement::Now,
            viewers: Vec::new(),
            viewer_serial: 0,
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
            ctx_menu: None,
            rmb: None,
            row_hits: [Vec::new(), Vec::new()],
            body_rects: [egui::Rect::NOTHING; 2],
            table_widths: [0.0; 2],
            free: Default::default(),
        }
    }

    fn vis(&self) -> Visibility {
        visibility(&self.cfg)
    }

    /// Plugins turned on or off (Settings): start them again; a panel inside
    /// an archive whose plugin is off now goes to the archive's folder.
    fn apply_plugins(&mut self) {
        self.plugins.start(&self.cfg.disabled_plugins);
        let hidden = self.vis();
        let active = self.plugins.active.clone();
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
        if self.plugin_task.is_some() {
            self.status = tr("A plugin command is still running.").into();
            return;
        }
        let here = self.act().path().to_path_buf();
        values.insert("panel".into(), sal_plugin_api::ui::path_value(&here));
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
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => self.open_dialog(Dialog::Message { title: name, text: e.to_string() }),
        }
    }

    /// Show a connection a plugin opened in the active panel.
    fn open_connection(&mut self, plugin: &Arc<dyn FolderPlugin>, m: &plugins::Mount) {
        let root = m.root();
        let hidden = self.vis();
        let start = if m.folder.is_empty() { root.clone() } else { root.join(&m.folder) };
        plugins::add_mount(&root, plugin.id(), &start);
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

    /// Close a connection: panels showing it go back to where they were
    /// before, and the plugin releases it in the background.
    fn close_connection(&mut self, root: &Path) {
        let Some((_, id)) = plugins::mount_root_of(root) else { return };
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

    fn open_from_archive(&mut self, ctx: &egui::Context, file: PathBuf, how: OpenAs) -> Result<(), String> {
        let plugin = self.in_archive(&file).map(|(p, _, _)| PluginRef(p));
        let dir = plugins::temp_dir("open").map_err(|e| e.to_string())?;
        self.jobs.add_with(ctx, JobKind::Archive(FolderOp::Open(how)), vec![file], dir, Placement::Now, plugin);
        Ok(())
    }

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
            Ok(Some(release)) => self.open_dialog(Dialog::Update { release, install: None }),
            Ok(None) if manual => self.open_dialog(Dialog::Message {
                title: tr("Check for updates").into(),
                text: trf("You have the newest version ({version}).", &[("version", &env!("CARGO_PKG_VERSION"))]),
            }),
            Err(e) if manual => self.error(trf("Could not look for updates: {error}", &[("error", &e)])),
            _ => {}
        }
    }

    fn open_dialog(&mut self, d: Dialog) {
        // a new dialog window opens centred on the main window again; the
        // windows of running operations stay as they are
        dialogwin::close(&mut self.dialog_places, self.hwnd, |k| !JOB_WINDOWS.contains(&k));
        self.dialog = d;
        self.focus_dialog = true;
    }

    /// Switch the UI language ("" = the Windows language) and remember it.
    fn set_language(&mut self, code: &str) {
        self.cfg.language = code.to_owned();
        crate::i18n::set_language(code);
    }

    fn error(&mut self, text: impl Into<String>) {
        self.open_dialog(Dialog::Message { title: tr("Error").into(), text: text.into() });
    }

    fn act(&mut self) -> &mut Panel {
        &mut self.panels[self.active]
    }

    fn reload_all(&mut self) {
        let h = self.vis();
        for p in &mut self.panels {
            p.reload(h);
        }
    }

    fn start_job(&mut self, ctx: &egui::Context, kind: JobKind, sources: Vec<PathBuf>, target: PathBuf) {
        if sources.is_empty() {
            return;
        }
        let place = std::mem::replace(&mut self.queue_place, Placement::Now);
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
        self.jobs.add_with(ctx, kind, sources, target, place, plugin);
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
        if self.panels[panel].archive.is_some() {
            self.status = tr("The context menu isn't available in plugin panels.").into();
            return;
        }
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
        let dir = p.path().to_path_buf();
        let menu = CtxMenu { panel, items, dir, pos, can_paste: shellmenu::clipboard_has_files() };
        match (self.cfg.context_menu_style, self.hwnd) {
            (ContextMenuStyle::Classic, _) => self.show_classic(ctx, menu.items, menu.dir, pos, from_mouse),
            (ContextMenuStyle::Windows11, Some(h)) => {
                let (top, rows) = self.menu_model(&menu);
                let mut actions = Vec::new();
                let mut native = Vec::new();
                let mut add = |e: &ctxmenu::Entry, native: &mut Vec<Option<(u32, String, String, bool)>>| {
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
                        ctxmenu::Row::Entry(e) => add(e, &mut native),
                        ctxmenu::Row::Separator => native.push(None),
                    }
                }
                let at = self.menu_point(ctx, h, pos, from_mouse);
                let id = shellmenu::show_native(h, &native, at);
                if id > 0 {
                    self.run_menu_action(ctx, menu, actions[id as usize - 1]);
                }
            }
            // no native window (tests): the menu drawn by the app
            (ContextMenuStyle::Windows11, None) => self.ctx_menu = Some(menu),
        }
    }

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
        match shellmenu::show_classic(h, &targets, &dir, at, extended) {
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
        let kinds: Vec<EntryKind> = menu.items.iter().map(|i| p.entries().iter().find(|e| &e.path == i).map_or(EntryKind::File, |e| e.kind)).collect();
        let any_dir = kinds.contains(&EntryKind::Dir);
        let single_file = n == 1 && kinds[0] == EntryKind::File;
        let single_dir = n == 1 && kinds[0] == EntryKind::Dir;
        if n > 0 {
            let mut top = vec![entry(MenuAction::Cut, tr("Cut"), "Ctrl+X".into(), true), entry(MenuAction::Copy, tr("Copy"), "Ctrl+C".into(), true)];
            if menu.can_paste && single_dir {
                top.push(entry(MenuAction::Paste, tr("Paste into folder"), "Ctrl+V".into(), true));
            }
            top.push(entry(MenuAction::Rename, tr("Rename"), key(Cmd::Rename), n == 1));
            top.push(entry(MenuAction::Delete, tr("Delete"), key(Cmd::Delete { permanent: false }), true));
            let mut rows = vec![
                ctxmenu::Row::Entry(entry(MenuAction::Open, tr("Open"), "Enter".into(), true)),
                ctxmenu::Row::Entry(entry(MenuAction::OpenWith, tr("Open with…"), String::new(), single_file)),
                ctxmenu::Row::Separator,
            ];
            if any_dir {
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
                top.push(entry(MenuAction::Paste, tr("Paste"), "Ctrl+V".into(), true));
            }
            let rows = vec![
                ctxmenu::Row::Entry(entry(MenuAction::NewFolder, tr("New folder"), key(Cmd::MkDir), true)),
                ctxmenu::Row::Entry(entry(MenuAction::Paste, tr("Paste"), "Ctrl+V".into(), menu.can_paste)),
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
            Browse::Editor => tr("Choose the editor (F4)"),
            Browse::Viewer => tr("Choose the viewer (F3)"),
            Browse::Terminal => tr("Choose the terminal"),
        };
        let Some(program) = shellmenu::pick_program(self.hwnd, title) else { return };
        match target {
            Browse::Editor => {
                self.cfg.editor = program;
                self.cfg.use_custom_editor = true;
            }
            Browse::Viewer => {
                self.cfg.viewer = program;
                self.cfg.use_custom_viewer = true;
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

    fn remember_dirs(&mut self) {
        for p in &self.panels {
            let key = drive_key(&drives::root_of(p.path()));
            if self.last_dir.get(&key).map(PathBuf::as_path) != Some(p.path()) {
                self.last_dir.insert(key, p.path().to_path_buf());
            }
        }
    }

    // ---------------------------------------------------------------- commands

    fn command(&mut self, ctx: &egui::Context, cmd: Cmd) {
        let hidden = self.vis();
        let other = 1 - self.active;
        let res: Result<(), String> = match cmd {
            Cmd::Help => {
                self.open_dialog(Dialog::Help);
                Ok(())
            }
            Cmd::CheckUpdates => {
                self.update_check = Some(crate::update::Check::start(ctx, true));
                Ok(())
            }
            Cmd::Enter => match self.act().enter(hidden) {
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
            Cmd::View => match self.act().current().filter(|e| e.kind == EntryKind::File).map(|e| e.path.clone()) {
                Some(p) if self.in_archive(&p).is_some() => self.open_from_archive(ctx, p, OpenAs::View),
                Some(p) if self.cfg.use_custom_viewer && !self.cfg.viewer.trim().is_empty() => {
                    sal_core::shell::open_with_program(&self.cfg.viewer, &p).map_err(|e| format!("{}: {e}", display_path(&p)))
                }
                Some(p) => {
                    self.viewer_serial += 1;
                    self.viewers.push(Viewer::open(p, self.viewer_serial));
                    Ok(())
                }
                None => Ok(()),
            },
            Cmd::Edit => match self.act().current().filter(|e| e.kind == EntryKind::File).map(|e| e.path.clone()) {
                Some(p) if self.in_archive(&p).is_some() => self.open_from_archive(ctx, p, OpenAs::Edit),
                Some(p) => {
                    let editor = self.cfg.use_custom_editor.then_some(self.cfg.editor.as_str());
                    sal_core::shell::edit(&p, editor).map_err(|e| format!("{}: {e}", display_path(&p)))
                }
                None => Ok(()),
            },
            Cmd::Copy | Cmd::Move => {
                let sources = self.act().targets();
                if !sources.is_empty() {
                    let mut target = display_path(self.panels[other].path());
                    // a connection is shown as a URL: its folders end with /
                    let sep = if location::virtual_parts(self.panels[other].path()).is_some() { '/' } else { MAIN_SEPARATOR };
                    if !target.ends_with(sep) {
                        target.push(sep);
                    }
                    let kind = if cmd == Cmd::Copy { OpKind::Copy } else { OpKind::Move };
                    self.open_dialog(Dialog::Transfer { kind, sources, target });
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
            Cmd::ContextMenu if self.act().archive.is_some() => Err(tr("The context menu isn't available in plugin panels.").into()),
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
            Cmd::Properties if self.act().archive.is_some() => Err(tr("Properties aren't available in plugin panels.").into()),
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
                apply_scrollbars(ctx, self.cfg.always_show_scrollbars);
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
                let conns: Vec<DriveConn> = plugins::connections()
                    .into_iter()
                    .map(|c| DriveConn { label: display_path(&c.root).trim_end_matches('/').to_owned(), detail: self.plugins.name_of(&c.plugin), root: c.root, start: c.start })
                    .collect();
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
                let paths: Vec<String> = self.act().targets().iter().map(|p| display_path(p)).collect();
                if !paths.is_empty() {
                    self.status = trf("Copied {paths} to the clipboard", &[("paths", &plural(paths.len(), "path"))]);
                    ctx.copy_text(paths.join("\r\n"));
                }
                Ok(())
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

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
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
        let digit = [Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9].iter().position(|&k| k == key);
        if let Some(n) = digit.filter(|_| m.ctrl) {
            return self.command(ctx, if m.shift { Cmd::HotPathSet(n) } else { Cmd::HotPathGo(n) });
        }
        // fixed navigation keys
        match (key, m.ctrl, m.shift, m.alt) {
            (Key::ArrowUp, true, _, _) => {
                let anywhere = self.cfg.quick_search_anywhere;
                self.act().quick_search_next(false, anywhere);
            }
            (Key::ArrowDown, true, _, _) => {
                let anywhere = self.cfg.quick_search_anywhere;
                self.act().quick_search_next(true, anywhere);
            }
            // the search acts as a filter: arrows skip to the previous/next match
            (Key::ArrowUp | Key::ArrowDown, false, false, false) if self.cfg.quick_search_arrows && !self.act().qs.is_empty() => {
                let anywhere = self.cfg.quick_search_anywhere;
                self.act().quick_search_next(key == Key::ArrowDown, anywhere);
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
            (Key::Home, ..) => {
                let p = self.act();
                p.qs.clear();
                p.set_cursor(0);
            }
            (Key::End, ..) => {
                let p = self.act();
                p.qs.clear();
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
        let anywhere = self.cfg.quick_search_anywhere;
        self.act().quick_search_push(c, anywhere);
    }

    /// Commands that open dialogs don't need the egui context.
    fn command_noctx(&mut self, cmd: Cmd) {
        if let Cmd::SelectMask(select) = cmd {
            let mask = self.last_mask.clone();
            self.open_dialog(Dialog::SelectMask { select, mask, error: None });
        }
    }

    // ---------------------------------------------------------------- panels

    fn panel_ui(&mut self, ui: &mut egui::Ui, idx: usize) {
        let active = self.active == idx;
        let row_h = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
        let mut clicked: Option<(usize, Modifiers, bool)> = None;
        let mut sort_click = None;
        let mut path_start_edit = false;
        let mut path_submit: Option<bool> = None;
        let mut crumb_clicked: Option<PathBuf> = None;
        let mut crumb_rects = Vec::new();
        let mut drive_clicked = false;
        let mut drive_picked: Option<PathBuf> = None;
        let drive_list = self.cached_drives().to_vec();
        let show_all = self.cfg.drive_buttons == DriveButtons::All;

        // path bar
        ui.horizontal(|ui| {
            let root = drives::root_of(self.panels[idx].path());
            let root_kind = drives::kind_of_root(&root, &drive_list);
            let on_listed_drive = drive_list.iter().any(|d| d.root == root);
            if show_all {
                ui.spacing_mut().item_spacing.x = 2.0;
                for d in &drive_list {
                    let letter = d.letter.trim_end_matches(':');
                    if drive_icon::button(ui, d.kind, letter, d.root == root).on_hover_text(describe_drive(d)).clicked() {
                        drive_picked = Some(d.root.clone());
                    }
                }
                ui.spacing_mut().item_spacing.x = 6.0;
            }
            // the current drive (or UNC share) when not shown among the buttons above
            if !show_all || !on_listed_drive {
                let label = display_path(&root).trim_end_matches(['\\', '/']).to_owned();
                let tip = match drive_list.iter().find(|d| d.root == root) {
                    Some(d) => describe_drive(d),
                    None => format!("{label} — {}", tr(root_kind.label())),
                };
                let menu_key = menu_key(&self.cfg.shortcuts, Cmd::DriveMenu(idx));
                let tip = if menu_key.is_empty() { tip } else { format!("{tip}\n{}", trf("Drive menu: {key}", &[("key", &menu_key)])) };
                if drive_icon::button(ui, root_kind, if label.is_empty() { "/" } else { &label }, false).on_hover_text(tip).clicked() {
                    drive_clicked = true;
                }
            }
            let (bg, fg) = if active { (ui.visuals().selection.bg_fill, ui.visuals().selection.stroke.color) } else { (ui.visuals().faint_bg_color, ui.visuals().text_color()) };
            let edit_key = menu_key(&self.cfg.shortcuts, Cmd::ChangeDir);
            egui::Frame::none().fill(bg).inner_margin(egui::Margin::symmetric(6.0, 3.0)).rounding(3.0f32).show(ui, |ui| {
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
                    let crumbs = location::breadcrumbs(self.panels[idx].path());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
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
        let banner = self.panels[idx].archive.as_ref().map(|a| plugin_banner(&self.plugins.name_of(a.plugin.id()), &a.file));
        let banner_h = if banner.is_some() { row_h } else { 0.0 };
        let body_h = (ui.available_height() - footer_h - banner_h).max(row_h * 3.0);
        if banner.is_some() {
            let body = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), body_h));
            ui.painter().rect_filled(body, 0.0, plugin_tint(ui.visuals()));
        }
        self.panels[idx].visible_rows = ((body_h - row_h) / row_h).floor().max(1.0) as usize;

        let mut sizes = self.dir_sizes.snapshot();
        // inside an archive every folder's size is known from the archive's index
        if let Some(a) = self.panels[idx].archive.as_ref().filter(|a| a.index.complete) {
            for e in self.panels[idx].entries().iter().filter(|e| e.kind == EntryKind::Dir) {
                let info = crate::dirsize::SizeInfo { size: plugins::stats(&a.index, &plugins::join_inner(&a.inner, &e.name)), done: true };
                sizes.insert(e.path.clone(), info);
            }
        }
        let use_icons = self.cfg.system_icons;
        let icons = &mut self.icons;
        let panel = &mut self.panels[idx];
        let scroll_to = if panel.scroll_to_cursor { Some(panel.cursor) } else { None };
        panel.scroll_to_cursor = false;
        let sort = panel.sort;
        let cursor = panel.cursor;
        let qs_mask = (!panel.qs.is_empty()).then(|| sal_core::quicksearch::prepare(&panel.qs));
        let qs_anywhere = self.cfg.quick_search_anywhere;
        let entries = &panel.listing.entries;
        let width = ui.available_width();
        // egui_extras keeps column widths between frames and won't shrink them
        // when the panel gets narrower: lay the columns out again on a resize
        let relayout = (width - self.table_widths[idx]).abs() > 0.5;
        self.table_widths[idx] = width;

        let mut row_hits = Vec::new();
        let body =
            ui.allocate_ui(egui::vec2(width, body_h), |ui| {
                // the table remembers column widths; a different scrollbar mode
                // reserves different space, so it gets its own layout state
                let solid_bars = !ui.style().spacing.scroll.floating;
                ui.push_id((idx, solid_bars), |ui| {
                    let bars = scroll_bar_visibility(ui);
                    // Ext, Size, Date, Attr at their usual widths when there is room,
                    // scaled down in a narrow panel so the table never overflows it
                    let name_min = (width * 0.3).min(120.0);
                    let gaps = ui.spacing().item_spacing.x * 5.0 + ui.spacing().scroll.allocated_width() + 4.0;
                    let k = ((width - gaps - name_min) / (55.0 + 95.0 + 120.0 + 42.0)).clamp(0.2, 1.0);
                    let fixed = |w: f32, min: f32| Column::initial(w * k).at_least(min.min(w * k)).clip(true);
                    let mut tb = TableBuilder::new(ui)
                        .scroll_bar_visibility(bars)
                        .striped(true)
                        .resizable(true)
                        .auto_shrink([false, false])
                        .sense(Sense::click())
                        .max_scroll_height(body_h)
                        .cell_layout(Layout::left_to_right(Align::Center))
                        .column(Column::remainder().at_least(name_min).clip(true))
                        .column(fixed(55.0, 30.0))
                        .column(fixed(95.0, 40.0))
                        .column(fixed(120.0, 40.0))
                        .column(fixed(42.0, 20.0));
                    if relayout {
                        tb.reset();
                    }
                    if let Some(r) = scroll_to {
                        tb = tb.scroll_to_row(r, None);
                    }
                    tb.header(row_h, |mut h| {
                        for (key, title) in
                            [(SortKey::Name, tr("Name")), (SortKey::Ext, tr("Ext")), (SortKey::Size, tr("Size")), (SortKey::Time, tr("Date")), (SortKey::Attr, tr("Attr"))]
                        {
                            h.col(|ui| {
                                let arrow = if sort.key == key {
                                    if sort.reverse {
                                        " ⏶"
                                    } else {
                                        " ⏷"
                                    }
                                } else {
                                    ""
                                };
                                if ui.add(egui::Label::new(RichText::new(format!("{title}{arrow}")).strong()).sense(Sense::click())).clicked() {
                                    sort_click = Some(key);
                                }
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(row_h, entries.len(), |mut row| {
                            let i = row.index();
                            let e = &entries[i];
                            row.set_selected(active && i == cursor);
                            let color = if e.selected { Some(SELECTED_COLOR) } else { None };
                            let text = |s: String| {
                                let mut t = RichText::new(s);
                                if let Some(c) = color {
                                    t = t.color(c);
                                }
                                t
                            };
                            row.col(|ui| {
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
                                // partial highlight only makes sense when visual == logical order
                                let hl = qs_mask
                                    .as_ref()
                                    .filter(|_| active && i == cursor)
                                    .and_then(|m| sal_core::quicksearch::find_match(&e.name, m, qs_anywhere))
                                    .map(|(a, b)| if shown == logical { (a, b) } else { (0, shown.chars().count()) });
                                let mut job = LayoutJob::default();
                                let style = ui.style();
                                let font = egui::TextStyle::Body.resolve(style);
                                let fg = color.unwrap_or_else(|| if active && i == cursor { ui.visuals().strong_text_color() } else { ui.visuals().text_color() });
                                let normal = TextFormat { font_id: font.clone(), color: fg, italics: e.name_lossy, ..Default::default() };
                                match hl {
                                    Some((a, b)) => {
                                        // char range -> byte offsets, clamped to the shown name (no extension)
                                        let at = |n: usize| shown.char_indices().nth(n).map_or(shown.len(), |(x, _)| x);
                                        let (sa, sb) = (at(a), at(b));
                                        let mut h = normal.clone();
                                        h.underline = egui::Stroke::new(1.5f32, fg);
                                        h.background = ui.visuals().warn_fg_color.gamma_multiply(0.35);
                                        job.append(&shown[..sa], 0.0, normal.clone());
                                        job.append(&shown[sa..sb], 0.0, h);
                                        job.append(&shown[sb..], 0.0, normal);
                                    }
                                    None => job.append(shown, 0.0, normal),
                                }
                                let r = ui.add(egui::Label::new(job).truncate().selectable(false));
                                if e.name_lossy {
                                    r.on_hover_text(tr(
                                        "This name is not valid Unicode (unpaired UTF-16 surrogate).\nIt is shown with U+FFFD, but all operations use the exact on-disk name.",
                                    ));
                                }
                            });
                            row.col(|ui| {
                                ui.label(text(if e.kind == EntryKind::File { display_name(e.ext()).into_owned() } else { String::new() }));
                            });
                            row.col(|ui| {
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
                                                EntryKind::File => format_thousands(e.size),
                                                EntryKind::Dir => tr("DIR").into(),
                                                EntryKind::UpDir => tr("UP-DIR").into(),
                                            }));
                                        }
                                    }
                                });
                            });
                            row.col(|ui| {
                                ui.label(text(fmt_time(e.modified)));
                            });
                            row.col(|ui| {
                                ui.label(text(if e.kind == EntryKind::UpDir { String::new() } else { e.attr_string() }));
                            });
                            let r = row.response();
                            row_hits.push((r.rect, i));
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
        if let Some(text) = banner {
            let fill = plugin_tint(ui.visuals()).gamma_multiply(2.5);
            egui::Frame::none().fill(fill).inner_margin(egui::Margin::symmetric(6.0, 1.0)).show(ui, |ui| {
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
        egui::Frame::none().fill(strip).inner_margin(egui::Margin::symmetric(6.0, 2.0)).rounding(2.0f32).show(ui, |ui| {
            ui.visuals_mut().override_text_color = Some(strong);
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if !qs.is_empty() {
                    ui.label(RichText::new(format!("🔍 {qs}")).color(ui.visuals().warn_fg_color)).on_hover_text(if qs_anywhere {
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

    // ---------------------------------------------------------------- dialogs

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
            dialogwin::close(&mut self.dialog_places, self.hwnd, |k| !JOB_WINDOWS.contains(&k));
        }
        let dw = DialogWin { owner: self.hwnd, theme: self.cfg.theme, modal: false, exclusive: false };
        // dialog windows are sized from the main window, not from themselves
        let main_h = ctx.screen_rect().height();
        // 0 = keep open, 1 = ok, 2 = cancel
        let mut outcome = 0u8;
        let text_field = |ui: &mut egui::Ui, s: &mut String| {
            let r = ui.add(egui::TextEdit::singleline(s).desired_width(f32::INFINITY));
            // a new window's first frame is an invisible sizing pass that drops
            // focus requests, so keep claiming focus while nothing holds it
            if focus || ui.memory(|m| m.focused().is_none()) {
                r.request_focus();
            }
        };
        let buttons = |ui: &mut egui::Ui, ok: &str| -> u8 {
            ui.add_space(6.0);
            match centered_buttons(ui, &[(ok, ""), (tr("Cancel"), "")]) {
                Some(0) => 1,
                Some(_) => 2,
                None => 0,
            }
        };

        let mut browse_field: Option<String> = None;
        let mut drive_disconnect: Option<PathBuf> = None;
        match &mut dialog {
            Dialog::None => return,
            Dialog::PluginForm { form, values, .. } => {
                dw.modal().show(ctx, &mut self.dialog_places, "plugin-form", &form.title, 500.0, &mut enter, &mut escape, &mut |ui| {
                    plugin_form(ui, form, values, focus, &mut browse_field);
                    outcome = buttons(ui, &form.ok);
                });
            }
            Dialog::Transfer { kind, sources, target } => {
                let verb = if *kind == OpKind::Copy { tr("Copy") } else { tr("Move") };
                let busy = self.jobs.running.len() + self.jobs.queue.len();
                dw.modal().show(ctx, &mut self.dialog_places, "transfer", verb, 560.0, &mut enter, &mut escape, &mut |ui| {
                    ui.label(if *kind == OpKind::Copy { trf("Copy {what} to:", &[("what", &describe(sources))]) } else { trf("Move {what} to:", &[("what", &describe(sources))]) });
                    text_field(ui, target);
                    ui.small(tr("Target can be a directory, a new name, or a mask such as *.bak"));
                    if busy > 0 {
                        ui.small(trn("{n} operation is running or waiting.", "{n} operations are running or waiting.", busy as u64));
                    }
                    ui.add_space(6.0);
                    let buttons = [
                        (verb, tr("Start now, alongside the running operations")),
                        (tr("Run first"), tr("Pause the running operations and run this one; they go on when it finishes")),
                        (tr("Queue last"), tr("Wait until the running and the waiting operations finish")),
                        (tr("Cancel"), ""),
                    ];
                    match centered_buttons(ui, &buttons) {
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
                let count = drives.len() + conns.len() + paths.len();
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
                let button_w = ctx.fonts(|f| f.layout_no_wrap(tr("Disconnect").to_owned(), small.clone(), Color32::WHITE).size().x) + 16.0;
                let needed = if conns.is_empty() { 0.0 } else { longest + 40.0 + button_w + 24.0 };
                let width = 420.0f32.min(panel_rect.width() - 8.0).max(280.0).max(needed).min(screen.right() - pos.x - 16.0);
                let bounds = egui::Rect::from_min_max(egui::pos2(screen.left(), pos.y), screen.max);
                let row_h = ctx.style().text_styles.get(&egui::TextStyle::Body).map_or(14.0, |f| f.size) + 9.0;
                // all of the room below the path bar, less the frame's margins
                let list_h = (bounds.height() - 24.0).max(row_h * 3.0);
                let moved = up || down;
                let shown = egui::Area::new(egui::Id::new("drive_selector")).order(egui::Order::Foreground).fixed_pos(pos).constrain_to(bounds).show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_width(width);
                        egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
                            ui.set_width(width);
                            ui.spacing_mut().item_spacing.y = 1.0;
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
                                    ui.add_space(3.0);
                                    ui.separator();
                                    ui.add_space(3.0);
                                }
                                let r = drive_icon::row(ui, d.kind, &d.letter, &d.label, &detail, k == *cursor).on_hover_text(describe_drive(d));
                                keep_visible(&r, k);
                                if r.clicked() {
                                    *cursor = k;
                                    outcome = 1;
                                }
                            }
                            // servers a plugin is connected to
                            if !conns.is_empty() {
                                ui.add_space(4.0);
                                ui.separator();
                                ui.label(RichText::new(tr("Connections")).small().weak());
                                for (k, c) in conns.iter().enumerate() {
                                    let i = drives.len() + k;
                                    let tip = format!("{} · {}\n{}\n{}", c.label, c.detail, display_path(&c.start), tr("Delete: disconnect"));
                                    let r = drive_icon::connection_row(ui, &c.label, i == *cursor, button_w + 8.0).on_hover_text(tip);
                                    keep_visible(&r, i);
                                    // a compact button inside the row's right end; it takes its clicks
                                    let at = egui::Rect::from_min_max(
                                        egui::pos2(r.rect.right() - 4.0 - button_w, r.rect.top() + 3.0),
                                        egui::pos2(r.rect.right() - 4.0, r.rect.bottom() - 3.0),
                                    );
                                    if row_button(ui, at, tr("Disconnect"), &small, ("disconnect", k)).on_hover_text(tr("Close this connection")).clicked() {
                                        drive_disconnect = Some(c.root.clone());
                                    } else if r.clicked() {
                                        *cursor = i;
                                        outcome = 1;
                                    }
                                }
                            }
                            if !paths.is_empty() {
                                ui.add_space(4.0);
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
                        });
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
                    ui.add_space(6.0);
                    if centered_buttons(ui, &[(tr("OK"), "")]).is_some() {
                        outcome = 2;
                    }
                });
                if enter {
                    outcome = 2;
                }
            }
            Dialog::Update { release, install } => {
                use crate::update::Progress;
                let state = install.as_ref().map(|i| i.lock().unwrap().clone());
                let running = matches!(state, Some(Progress::Downloading { .. } | Progress::Unpacking));
                let (mut open_page, mut start, mut run_new) = (false, false, None);
                dw.modal().show(ctx, &mut self.dialog_places, "update", tr("Update available"), 480.0, &mut enter, &mut escape, &mut |ui| {
                    ui.label(trf("RustySalamander {new} is available. You have {current}.", &[("new", &release.version), ("current", &env!("CARGO_PKG_VERSION"))]));
                    ui.add_space(2.0);
                    open_page = ui.link(release.page.as_str()).on_hover_text(tr("Open the release page in the browser")).clicked();
                    ui.add_space(6.0);
                    match &state {
                        None => {
                            let folder = crate::update::target_dir(release).map(|p| display_path(&p)).unwrap_or_default();
                            ui.label(trf(
                                "Update now downloads the new version, unpacks it to {folder} and copies your settings there. This version stays as it is.",
                                &[("folder", &folder)],
                            ));
                            ui.add_space(6.0);
                            match centered_buttons(ui, &[(tr("Update now"), ""), (tr("Close"), "")]) {
                                Some(0) => start = true,
                                Some(_) => outcome = 2,
                                None => {}
                            }
                        }
                        Some(Progress::Downloading { done, total }) => {
                            ui.label(trf("Downloading… {done} of {total}", &[("done", &format_size(*done)), ("total", &format_size(*total))]));
                            ui.add(egui::ProgressBar::new(if *total > 0 { *done as f32 / *total as f32 } else { 0.0 }));
                        }
                        Some(Progress::Unpacking) => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(tr("Unpacking…"));
                            });
                        }
                        Some(Progress::Done { folder, copied }) => {
                            ui.label(trf("RustySalamander {new} is installed in {folder}.", &[("new", &release.version), ("folder", &display_path(folder))]));
                            ui.label(if copied.is_empty() {
                                tr("There were no settings to copy.").to_owned()
                            } else {
                                trf("Settings copied: {files}", &[("files", &copied.join(", "))])
                            });
                            ui.add_space(6.0);
                            match centered_buttons(ui, &[(tr("Start the new version"), tr("Closes this version")), (tr("Close"), "")]) {
                                Some(0) => run_new = Some(folder.clone()),
                                Some(_) => outcome = 2,
                                None => {}
                            }
                        }
                        Some(Progress::Failed(e)) => {
                            ui.label(trf("The update failed: {error}", &[("error", e)]));
                            ui.add_space(6.0);
                            match centered_buttons(ui, &[(tr("Try again"), ""), (tr("Close"), "")]) {
                                Some(0) => start = true,
                                Some(_) => outcome = 2,
                                None => {}
                            }
                        }
                    }
                });
                // the window stays while the download runs
                if running {
                    enter = false;
                    escape = false;
                }
                if open_page {
                    if let Err(e) = shell_open(Path::new(&release.page)) {
                        self.error(format!("{}: {e}", release.page));
                    }
                }
                if start {
                    // the settings as they are now go to the new version
                    self.save_config();
                    *install = Some(crate::update::install(ctx, release.clone()));
                }
                if let Some(folder) = run_new {
                    let exe = folder.join("rustysalamander.exe");
                    match std::process::Command::new(&exe).current_dir(&folder).spawn() {
                        Ok(_) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => self.error(format!("{}: {e}", display_path(&exe))),
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
                        PluginRow { id: id.clone(), name: f.tr(&f.info.name), description: f.tr(&f.info.description), file: display_path(&f.file), state }
                    })
                    .collect();
                let broken_plugins: Vec<(String, String)> = self.plugins.broken.iter().map(|(f, e)| (display_path(f), e.clone())).collect();
                let plugins_dir = plugins::plugins_dir().map(|d| display_path(&d)).unwrap_or_default();
                let font_filter = &mut self.font_filter;
                let mut captured = false;
                // fixed width: the scroll areas inside take all the width they are given
                dw.show(ctx, &mut self.dialog_places, "settings", tr("Settings"), 860.0, &mut enter, &mut escape, &mut |ui| {
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
                                        ui.add_space(4.0);
                                        ui.separator();
                                        ui.add_space(2.0);
                                    }
                                    ui.label(RichText::new(tr(title)).strong());
                                    ui.add_space(2.0);
                                    egui::Grid::new(("settings", title)).num_columns(2).spacing([16.0, 8.0]).min_col_width(190.0).show(ui, |ui| body(ui));
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

                                    ui.label(tr("Font"));
                                    let (changed, rect) = font_picker(ui, "ui_font", &mut cfg.ui_font, crate::fonts::DEFAULT_UI_FONT, font_list, font_filter);
                                    fonts_changed |= changed;
                                    font_button_rect = Some(rect);
                                    ui.end_row();

                                    ui.label(tr("Viewer font"));
                                    fonts_changed |= font_picker(ui, "mono_font", &mut cfg.mono_font, crate::fonts::DEFAULT_MONO_FONT, font_list, font_filter).0;
                                    ui.end_row();

                                    ui.label(tr("File icons"));
                                    ui.checkbox(&mut cfg.system_icons, tr(LBL_ICONS));
                                    ui.end_row();

                                    ui.label(tr("Layout"));
                                    ui.vertical(|ui| {
                                        if ui.checkbox(&mut cfg.always_show_scrollbars, tr(LBL_SCROLLBARS)).changed() {
                                            apply_scrollbars(ctx, cfg.always_show_scrollbars);
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
                                });

                                section(ui, n_("Quick search"), false, &mut |ui| {
                                    ui.label(tr("Match"));
                                    ui.horizontal(|ui| {
                                        ui.radio_value(&mut cfg.quick_search_anywhere, false, tr(LBL_QS_PREFIX));
                                        ui.radio_value(&mut cfg.quick_search_anywhere, true, tr(LBL_QS_ANYWHERE));
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
                                        ui.radio_value(&mut cfg.context_menu_style, ContextMenuStyle::Classic, tr("Classic system menu (Windows 10 style, all shell extensions)"));
                                    });
                                    ui.end_row();
                                });

                                section(ui, n_("External programs"), false, &mut |ui| {
                                    let file_tip = tr("Program and optional arguments. {file} is replaced by the file;\nwithout it the file is added at the end.");

                                    ui.label(tr("Editor (F4)"));
                                    ui.vertical(|ui| {
                                        ui.radio_value(&mut cfg.use_custom_editor, false, tr("Notepad (default)"));
                                        ui.horizontal(|ui| {
                                            ui.radio_value(&mut cfg.use_custom_editor, true, tr("Custom:"));
                                            ui.add_enabled(
                                                cfg.use_custom_editor,
                                                egui::TextEdit::singleline(&mut cfg.editor).hint_text(r#""C:\Program Files\Notepad++\notepad++.exe""#).desired_width(250.0),
                                            )
                                            .on_hover_text(file_tip);
                                            if ui.button(tr("Browse…")).clicked() {
                                                browse = Some(Browse::Editor);
                                            }
                                        });
                                    });
                                    ui.end_row();

                                    ui.label(tr("Viewer (F3)"));
                                    ui.vertical(|ui| {
                                        ui.radio_value(&mut cfg.use_custom_viewer, false, tr("Built-in viewer (default)"));
                                        ui.horizontal(|ui| {
                                            ui.radio_value(&mut cfg.use_custom_viewer, true, tr("Custom:"));
                                            ui.add_enabled(
                                                cfg.use_custom_viewer,
                                                egui::TextEdit::singleline(&mut cfg.viewer).hint_text(r#""C:\Program Files\Viewer\viewer.exe""#).desired_width(250.0),
                                            )
                                            .on_hover_text(file_tip);
                                            if ui.button(tr("Browse…")).clicked() {
                                                browse = Some(Browse::Viewer);
                                            }
                                        });
                                    });
                                    ui.end_row();

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
                        SettingsTab::Plugins => {
                            ui.label(RichText::new(tr("Plugins add optional features. A plugin that is off changes nothing; turning one on or off applies at once.")).weak());
                            ui.label(RichText::new(trf("Plugins are found in {dir} when the program starts.", &[("dir", &plugins_dir)])).weak());
                            ui.add_space(6.0);
                            if plugin_rows.is_empty() {
                                ui.label(tr("No plugins found."));
                            }
                            for k in &plugin_rows {
                                egui::Frame::group(ui.style()).inner_margin(egui::Margin::same(10.0)).show(ui, |ui| {
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
                                ui.add_space(4.0);
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
                                    egui::Frame::group(ui.style()).inner_margin(egui::Margin::same(10.0)).show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.label(RichText::new(tr(section.label())).size(ui.text_style_height(&egui::TextStyle::Body) * 1.15).strong());
                                        ui.label(RichText::new(tr(section.description())).small().weak());
                                        ui.separator();
                                        egui::Grid::new(("keymap", section.label())).striped(true).num_columns(3).spacing([12.0, 6.0]).min_col_width(200.0).show(ui, |ui| {
                                            ui.label(RichText::new(tr("Command")).small().weak());
                                            ui.label(RichText::new(tr("Preset")).small().weak());
                                            ui.label(RichText::new(tr("Shortcuts (click to remove)")).small().weak());
                                            ui.end_row();
                                            for a in section.actions() {
                                                let info = a.info();
                                                let cur = keymap::bindings(&cfg.shortcuts, a);
                                                ui.label(tr(info.label));
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
                                            ui.add_space(4.0);
                                            ui.label(RichText::new(trf("Fixed: {keys}", &[("keys", &fixed.join(" · "))])).small().weak());
                                        }
                                    });
                                    ui.add_space(8.0);
                                }
                            });
                            ui.add_space(4.0);
                            if ui.button(tr("Reset all shortcuts to defaults")).clicked() {
                                edit = Some(KeyEdit::ResetAll);
                            }
                            ui.small(tr("Configured shortcuts take precedence over fixed keys."));
                        }
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.small(tr("Settings apply immediately and are saved on exit."));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button(tr("Close")).clicked() {
                                outcome = 2;
                            }
                        });
                    });
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
                    self.update_check = Some(crate::update::Check::start(ctx, true));
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
                            egui::Grid::new(("help", section.label())).striped(true).min_col_width(200.0).show(ui, |ui| {
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
                            ui.add_space(8.0);
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.small(tr("Shortcuts are configured in Options → Settings… → Keyboard shortcuts."));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button(tr("Close")).clicked() {
                                outcome = 2;
                            }
                        });
                    });
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
        if let Some(root) = drive_disconnect {
            if let Dialog::Drive { drives, conns, paths, cursor, .. } = &mut dialog {
                conns.retain(|c| c.root != root);
                *cursor = (*cursor).min((drives.len() + conns.len() + paths.len()).saturating_sub(1));
            }
            self.close_connection(&root);
            outcome = 0;
        }
        // a plugin form's Browse… button: the Windows "Open" dialog
        if let (Some(id), Dialog::PluginForm { values, .. }) = (browse_field, &mut dialog) {
            let current = values.get(&id).map(PathBuf::from);
            if let Some(path) = shellmenu::pick_file(self.hwnd, tr("Choose a file"), current.as_deref()) {
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

    fn dialog_ok(&mut self, ctx: &egui::Context, dialog: Dialog) {
        let hidden = self.vis();
        match dialog {
            Dialog::PluginForm { plugin, form, values } => match self.plugins.get(&plugin).cloned() {
                Some(p) => self.run_plugin_command(ctx, p, form.command, values),
                None => self.error(tr("The plugin for this file is not enabled.")),
            },
            Dialog::Transfer { kind, sources, target } => {
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
                self.start_job(ctx, JobKind::Transfer(kind), sources, target);
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
                let res = match (drives.get(cursor), paths.get(cursor.wrapping_sub(drives.len() + conns.len()))) {
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

    fn job_ui(&mut self, ctx: &egui::Context) {
        for job in self.jobs.poll(ctx) {
            self.job_finished(job);
        }
        if self.jobs.is_idle() {
            dialogwin::close(&mut self.dialog_places, self.hwnd, |k| k == "job" || k == "prompt");
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
        let dw = DialogWin { owner: self.hwnd, theme: self.cfg.theme, modal: false, exclusive: false };
        let (mut enter, mut escape) = (false, false);
        let mut act: Option<JobAct> = None;
        let jobs = &self.jobs;
        // a fixed title: the window is found by it to make it owned by the main one
        let title = tr("File operations");
        dw.show(ctx, &mut self.dialog_places, "job", title, 580.0, &mut enter, &mut escape, &mut |ui| {
            for (n, job) in jobs.running.iter().enumerate() {
                if n > 0 {
                    ui.add_space(4.0);
                    ui.separator();
                }
                job_progress_ui(ui, job, now, &mut act);
            }
            if !jobs.queue.is_empty() {
                ui.add_space(6.0);
                ui.separator();
                ui.label(RichText::new(trf("Queue ({n})", &[("n", &jobs.queue.len())])).strong());
                ui.label(RichText::new(tr("Waiting operations start one by one when the running ones finish.")).small().weak());
                ui.add_space(2.0);
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
                        if ui.add_enabled(i + 1 < jobs.queue.len(), egui::Button::new("⏷").small()).on_hover_text(tr("Move down")).clicked() {
                            act = Some(JobAct::Move(spec.id, false));
                        }
                        let up_tip = if i == 0 { tr("To the top: run it now; the running operations pause until it finishes") } else { tr("Move up") };
                        if ui.small_button("⏶").on_hover_text(up_tip).clicked() {
                            act = Some(JobAct::Move(spec.id, true));
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.add(egui::Label::new(format!("{}. {}", i + 1, spec.describe())).truncate());
                        });
                    });
                }
            }
            // controls for everything, on a strip like the panels' status bar
            ui.add_space(8.0);
            let all_paused = jobs.running.iter().filter(|j| j.spec.kind.can_pause()).all(|j| j.is_paused());
            egui::Frame::none().fill(strip_color(ui.visuals())).inner_margin(egui::Margin::symmetric(8.0, 6.0)).rounding(2.0f32).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let buttons = [
                    if all_paused { (tr("Resume all"), tr("Continue all paused operations")) } else { (tr("Pause all"), tr("Pause all running operations")) },
                    (tr("Cancel all"), tr("Stop the running operations and empty the queue")),
                    (tr("Run in background"), tr("Hide this window; the progress stays in the status bar")),
                ];
                match centered_buttons(ui, &buttons) {
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
            Some(JobAct::Hide) => self.jobs.hidden = true,
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

    /// The question of an operation that hit a conflict or an error, in its own window.
    fn prompt_ui(&mut self, ctx: &egui::Context) {
        let dw = DialogWin { owner: self.hwnd, theme: self.cfg.theme, modal: false, exclusive: true };
        let Some(job) = self.jobs.running.iter_mut().find(|j| j.prompt.is_some()) else {
            dialogwin::close(&mut self.dialog_places, self.hwnd, |k| k == "prompt");
            return;
        };
        let (mut enter, mut escape) = (false, false);
        let mut reply = None;
        let op = job.spec.describe();
        match job.prompt.as_ref().unwrap() {
            Prompt::Conflict { src, dst } => {
                dw.show(ctx, &mut self.dialog_places, "prompt", tr("Target already exists"), 660.0, &mut enter, &mut escape, &mut |ui| {
                    ui.label(RichText::new(op.as_str()).small().weak());
                    ui.label(RichText::new(tr("An item with the same name is already in the target folder.")).strong());
                    ui.add_space(6.0);
                    conflict_table(ui, src, dst);
                    ui.add_space(10.0);
                    let actions = [ConflictAction::Overwrite, ConflictAction::OverwriteAll, ConflictAction::Skip, ConflictAction::SkipAll, ConflictAction::Cancel];
                    let labels = [
                        (tr("Overwrite"), ""),
                        (tr("Overwrite All"), tr("Overwrite this and all further existing items")),
                        (tr("Skip"), ""),
                        (tr("Skip All"), tr("Skip this and all further existing items")),
                        (tr("Cancel"), tr("Stop the whole operation")),
                    ];
                    if let Some(i) = centered_buttons(ui, &labels) {
                        reply = Some(Reply::Conflict(actions[i]));
                    }
                });
                if escape && reply.is_none() {
                    reply = Some(Reply::Conflict(ConflictAction::Cancel));
                }
            }
            Prompt::Error { path, message } => {
                dw.show(ctx, &mut self.dialog_places, "prompt", tr("Error"), 560.0, &mut enter, &mut escape, &mut |ui| {
                    ui.label(RichText::new(op.as_str()).small().weak());
                    ui.add_space(4.0);
                    egui::Grid::new("error").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                        ui.label(RichText::new(tr("Item")).weak());
                        ui.add(egui::Label::new(display_path(path)).wrap());
                        ui.end_row();
                        ui.label(RichText::new(tr("Problem")).weak());
                        ui.colored_label(ui.visuals().error_fg_color, message.as_str());
                        ui.end_row();
                    });
                    ui.add_space(10.0);
                    let actions = [ErrorAction::Retry, ErrorAction::Skip, ErrorAction::SkipAll, ErrorAction::Cancel];
                    let labels =
                        [(tr("Retry"), ""), (tr("Skip"), ""), (tr("Skip All"), tr("Skip this and all further items that fail")), (tr("Cancel"), tr("Stop the whole operation"))];
                    if let Some(i) = centered_buttons(ui, &labels) {
                        reply = Some(Reply::Error(actions[i]));
                    }
                });
                if escape && reply.is_none() {
                    reply = Some(Reply::Error(ErrorAction::Cancel));
                }
            }
        }
        if let Some(r) = reply {
            job.answer(r);
        }
    }

    fn job_finished(&mut self, job: Job) {
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
        self.status = match job.result.unwrap() {
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
                self.error(trf("{job} failed: {error}", &[("job", &title), ("error", &e)]));
                String::new()
            }
        };
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
            OpenAs::View if self.cfg.use_custom_viewer && !self.cfg.viewer.trim().is_empty() => sal_core::shell::open_with_program(&self.cfg.viewer, &file).map_err(err)?,
            OpenAs::View => {
                self.viewer_serial += 1;
                self.viewers.push(Viewer::open(file.clone(), self.viewer_serial));
            }
            OpenAs::Edit => {
                let editor = self.cfg.use_custom_editor.then_some(self.cfg.editor.as_str());
                sal_core::shell::edit(&file, editor).map_err(err)?;
                self.status = tr("A temporary copy is open; changes to it are not saved back.").into();
            }
        }
        Ok(())
    }

    /// Progress of the running operations in the status bar; a click shows their window.
    fn jobs_indicator(&mut self, ui: &mut egui::Ui) {
        let Some(first) = self.jobs.running.first() else {
            ui.label(RichText::new(trn("{n} operation waiting", "{n} operations waiting", self.jobs.queue.len() as u64)).weak());
            return;
        };
        let (frac, left) = self.jobs.summary();
        let mut text = format!("{} {:.0} %", first.spec.kind.title(), frac * 100.0);
        if self.jobs.running.len() > 1 {
            text = format!("{} · {}", trn("{n} operation", "{n} operations", self.jobs.running.len() as u64), text);
        }
        if self.jobs.running.iter().all(|j| j.is_paused()) {
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
            dialogwin::bring_to_front(&self.dialog_places, "job");
        }
    }

    // ---------------------------------------------------------------- chrome

    fn menu_bar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let mut cmd = None;
        let mut toggle_hidden = false;
        let mut toggle_system = false;
        let ov = self.cfg.shortcuts.clone();
        let current_theme = self.cfg.theme;
        let mut theme_pick = None;
        let mut always_bars = self.cfg.always_show_scrollbars;
        let mut fkey_bar = self.cfg.show_fkey_bar;
        let mut sys_icons = self.cfg.system_icons;
        let mut layout_cmd = None;
        let mut lang_pick: Option<String> = None;
        // the Plugins menu: each plugin's commands under its name
        let plugin_menu: Vec<(String, Arc<dyn FolderPlugin>, Vec<plugins::Command>)> =
            self.plugins.active.iter().map(|p| (self.plugins.name_of(p.id()), p.clone(), p.commands())).filter(|(_, _, c)| !c.is_empty()).collect();
        let connections: Vec<PathBuf> = plugins::connections().into_iter().map(|c| c.root).collect();
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
                if !connections.is_empty() {
                    ui.label(RichText::new(tr("Connections")).small().weak());
                    for root in &connections {
                        let shown = display_path(root).trim_end_matches('/').to_owned();
                        if ui.button(trf("Disconnect from {where}", &[("where", &shown)])).clicked() {
                            disconnect = Some(root.clone());
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
                            tr("A Rust rewrite of Open Salamander with full Unicode (UTF-8) support.\n\nBased on Open Salamander © Open Salamander Authors, GPL-2.0-or-later.")
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

    fn fkey_bar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let mut cmd = None;
        ui.columns(9, |cols| {
            for (col, (k, label, c)) in cols.iter_mut().zip([
                ("F1", tr("Help"), Cmd::Help),
                ("F2", tr("Rename"), Cmd::Rename),
                ("F3", tr("View"), Cmd::View),
                ("F4", tr("Edit"), Cmd::Edit),
                ("F5", tr("Copy"), Cmd::Copy),
                ("F6", tr("Move"), Cmd::Move),
                ("F7", tr("MkDir"), Cmd::MkDir),
                ("F8", tr("Delete"), Cmd::Delete { permanent: false }),
                ("F10", tr("Exit"), Cmd::Exit),
            ]) {
                // one line: a label too long for a narrow window is cut, not wrapped
                let b = egui::Button::new(RichText::new(format!("{k} {label}"))).wrap_mode(egui::TextWrapMode::Truncate).min_size(egui::vec2(col.available_width(), 0.0));
                if col.add(b).clicked() {
                    cmd = Some(c);
                }
            }
        });
        if let Some(c) = cmd {
            self.command(ctx, c);
        }
    }
}

/// Dialog windows that belong to the running operations, not to `self.dialog`.
const JOB_WINDOWS: [&str; 2] = ["job", "prompt"];

/// The fields of a plugin's form: a label and an input per row. The first
/// text input takes the focus when the dialog opens; `browse` is set to the
/// field whose Browse… button was clicked.
fn plugin_form(ui: &mut egui::Ui, form: &plugins::Form, values: &mut plugins::Values, focus: bool, browse: &mut Option<String>) {
    use plugins::FieldKind;
    let mut focus_next = true;
    egui::Grid::new("plugin-form").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        for f in &form.fields {
            let enabled = f.enabled_if.as_ref().is_none_or(|(id, v)| values.get(id) == Some(v));
            match &f.kind {
                FieldKind::Hidden => continue,
                FieldKind::Note => {
                    ui.label("");
                    ui.add(egui::Label::new(f.label.as_str()).wrap());
                    ui.end_row();
                    continue;
                }
                _ => {
                    ui.add_enabled(enabled, egui::Label::new(f.label.as_str()));
                }
            }
            let value = values.entry(f.id.clone()).or_default();
            ui.add_enabled_ui(enabled, |ui| {
                let mut text = |ui: &mut egui::Ui, value: &mut String, password: bool| {
                    let r = ui.add(egui::TextEdit::singleline(value).password(password).desired_width(ui.available_width().min(320.0)).id_salt(("plugin-field", &f.id)));
                    if focus_next && enabled && (focus || ui.memory(|m| m.focused().is_none())) {
                        r.request_focus();
                    }
                    focus_next = false;
                };
                match &f.kind {
                    FieldKind::Text => text(ui, value, false),
                    FieldKind::Password => text(ui, value, true),
                    FieldKind::File => {
                        ui.horizontal(|ui| {
                            let w = (ui.available_width() - 90.0).clamp(120.0, 260.0);
                            ui.add(egui::TextEdit::singleline(value).desired_width(w).id_salt(("plugin-field", &f.id)));
                            if ui.button(tr("Browse…")).clicked() {
                                *browse = Some(f.id.clone());
                            }
                        });
                    }
                    FieldKind::Choice(options) => {
                        ui.horizontal(|ui| {
                            for (v, label) in options {
                                if ui.radio(value == v, label.as_str()).clicked() {
                                    *value = v.clone();
                                }
                            }
                        });
                    }
                    FieldKind::Check => {
                        let mut on = !value.is_empty();
                        if ui.checkbox(&mut on, "").changed() {
                            *value = if on { "1".into() } else { String::new() };
                        }
                    }
                    FieldKind::Note | FieldKind::Hidden => {}
                }
            });
            ui.end_row();
        }
    });
}

/// The background tint of a panel a plugin shows.
fn plugin_tint(v: &egui::Visuals) -> Color32 {
    v.selection.bg_fill.gamma_multiply(if v.dark_mode { 0.12 } else { 0.22 })
}

/// The banner below a plugin's panel: its name and what it shows (the
/// archive's name, or the connection).
fn plugin_banner(plugin: &str, file: &Path) -> String {
    let what = match location::virtual_parts(file) {
        Some((scheme, authority, _)) => format!("{scheme}://{authority}"),
        None => file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
    };
    format!("🧩 {plugin} · {what}")
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
fn job_progress_ui(ui: &mut egui::Ui, job: &Job, now: Instant, act: &mut Option<JobAct>) {
    let p = &job.progress;
    ui.add(egui::Label::new(RichText::new(job.spec.describe()).strong()).truncate());
    ui.add(egui::Label::new(RichText::new(display_path(&p.current)).small()).truncate());
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
            parts.push(match job.rate.remaining(p.bytes_done, p.bytes_total) {
                Some(d) => trf("about {time} left", &[("time", &format_duration(d))]),
                None => tr("estimating the time left…").to_owned(),
            });
        }
        ui.label(parts.join(" · "));
    } else {
        // no progress reported (Recycle Bin): egui animates only rounded bars
        ui.add(progress_bar(0.0).text(tr("working…")));
    }
    ui.add_space(4.0);
    let paused = job.is_paused();
    let pause = if paused { tr("Resume") } else { tr("Pause") };
    let labels: Vec<(&str, &str)> = if job.spec.kind.can_pause() { vec![(pause, ""), (tr("Cancel"), "")] } else { vec![(tr("Cancel"), "")] };
    match centered_buttons(ui, &labels) {
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
    let col_w = ((ui.available_width() - 90.0 - 2.0 * 14.0) / 2.0).max(120.0);
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
    egui::Grid::new("conflict").num_columns(3).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
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

/// A row of buttons centred in the available width, each with an optional
/// tooltip; returns the index of the clicked one.
fn centered_buttons(ui: &mut egui::Ui, buttons: &[(&str, &str)]) -> Option<usize> {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let pad = ui.spacing().button_padding.x * 2.0;
    let gap = ui.spacing().item_spacing.x;
    let total: f32 = buttons.iter().map(|(l, _)| ui.fonts(|f| f.layout_no_wrap(l.to_string(), font.clone(), Color32::WHITE).size().x) + pad).sum::<f32>()
        + gap * buttons.len().saturating_sub(1) as f32;
    let mut hit = None;
    ui.horizontal(|ui| {
        ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
        for (i, (label, tip)) in buttons.iter().enumerate() {
            let mut r = ui.button(*label);
            if !tip.is_empty() {
                r = r.on_hover_text(*tip);
            }
            if r.clicked() {
                hit = Some(i);
            }
        }
    });
    hit
}

/// Small vector icons, so they look the same regardless of installed fonts.
fn paint_icon(ui: &mut egui::Ui, kind: EntryKind, hidden: bool) {
    let h = ui.text_style_height(&egui::TextStyle::Body);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(h + 4.0, h), Sense::hover());
    let r = rect.shrink2(egui::vec2(3.0, 1.0));
    let alpha = if hidden { 0.45 } else { 1.0 };
    let p = ui.painter();
    match kind {
        EntryKind::Dir => {
            let fill = Color32::from_rgb(232, 182, 64).gamma_multiply(alpha);
            let tab = egui::Rect::from_min_max(r.min + egui::vec2(0.0, 1.0), egui::pos2(r.left() + r.width() * 0.45, r.top() + 4.0));
            p.rect_filled(tab, 1.0, fill);
            p.rect_filled(egui::Rect::from_min_max(egui::pos2(r.left(), r.top() + 3.0), r.max - egui::vec2(0.0, 1.0)), 1.5, fill);
        }
        EntryKind::UpDir => {
            let c = ui.visuals().text_color();
            p.arrow(egui::pos2(r.center().x, r.bottom() - 1.0), egui::vec2(0.0, -(r.height() - 3.0)), egui::Stroke::new(1.6f32, c));
        }
        EntryKind::File => {
            let c = ui.visuals().weak_text_color().gamma_multiply(alpha);
            let page = egui::Rect::from_center_size(r.center(), egui::vec2(r.height() * 0.7, r.height() - 1.0));
            p.rect_stroke(page, 1.0, egui::Stroke::new(1.0f32, c));
        }
    }
}

/// How many typed paths the drive selector remembers.
const MAX_TYPED_PATHS: usize = 12;

// Labels shared by the menus and Settings, so both always say the same.
const LBL_HIDDEN: &str = n_("Show hidden files");
const LBL_SYSTEM: &str = n_("Show system files");
const LBL_ICONS: &str = n_("Show Windows file icons");
const LBL_SCROLLBARS: &str = n_("Always show scrollbars");
const LBL_FKEYS: &str = n_("Show function key bar");
const LBL_RECYCLE: &str = n_("Delete to Recycle Bin");
const LBL_QS_PREFIX: &str = n_("From first letter");
const LBL_QS_ANYWHERE: &str = n_("Anywhere in name");
const LBL_QS_ARROWS: &str = n_("Arrows jump between matches");
const LBL_CONFIRM: &str = n_("Confirm delete");
const THEMES: [(ThemeMode, &str); 3] = [(ThemeMode::System, n_("System default")), (ThemeMode::Light, n_("Light")), (ThemeMode::Dark, n_("Dark"))];

/// Which program a Settings "Browse…" button chooses.
#[derive(Clone, Copy)]
enum Browse {
    Editor,
    Viewer,
    Terminal,
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
    let button = ui.add(egui::Button::new(shown).shortcut_text("⏷").min_size(egui::vec2(280.0, 0.0)).wrap_mode(egui::TextWrapMode::Truncate));
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

/// Solid always-visible scrollbars, or egui's thin ones that appear on hover.
fn apply_scrollbars(ctx: &egui::Context, always: bool) {
    let style = if always {
        // egui's "solid" bars are invisible until hovered; keep track and
        // handle drawn, a bit wider, like classic Windows scrollbars
        egui::style::ScrollStyle {
            bar_width: 10.0,
            dormant_background_opacity: 0.6,
            active_background_opacity: 0.8,
            dormant_handle_opacity: 0.75,
            active_handle_opacity: 0.9,
            ..egui::style::ScrollStyle::solid()
        }
    } else {
        egui::style::ScrollStyle::floating()
    };
    ctx.all_styles_mut(|s| s.spacing.scroll = style);
}

/// Scroll areas follow the scroll style chosen in [`apply_scrollbars`].
pub fn scroll_bar_visibility(ui: &egui::Ui) -> egui::scroll_area::ScrollBarVisibility {
    if ui.style().spacing.scroll.floating {
        egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded
    } else {
        egui::scroll_area::ScrollBarVisibility::AlwaysVisible
    }
}

/// Theme for the panels, the window title bar and the native menus.
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

/// Room around the text of every button (egui's default is 4 × 1).
const BUTTON_PADDING: egui::Vec2 = egui::vec2(12.0, 4.0);

/// Font size and button padding for both the dark and the light style.
fn apply_font_size(ctx: &egui::Context, size: f32) {
    ctx.all_styles_mut(|s| {
        s.spacing.button_padding = BUTTON_PADDING;
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
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.hwnd.is_none() {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(h) = frame.window_handle() {
                if let RawWindowHandle::Win32(w) = h.as_raw() {
                    self.hwnd = Some(w.hwnd.get());
                    apply_theme(ctx, self.cfg.theme);
                }
            }
        }
        self.frame(ctx);
        self.diag_menu(ctx);
        self.diag_dialog(ctx);
    }
}

impl App {
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
        timing.mark("background reads");
        let no_dialog = matches!(self.dialog, Dialog::None);
        let editing_path = self.path_edit.iter().any(Option::is_some);
        let menu_open = self.ctx_menu.is_some();
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
            self.handle_keys(ctx);
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
            plugins::cleanup_temp();
        }
        self.poll_plugin_task(ctx);

        timing.mark("before menu");
        egui::TopBottomPanel::top("menu").show(ctx, |ui| self.menu_bar(ctx, ui));
        timing.mark("menu");
        if self.cfg.show_fkey_bar || !self.status.is_empty() || !self.jobs.is_idle() {
            egui::TopBottomPanel::bottom("fkeys").show(ctx, |ui| {
                if !self.jobs.is_idle() {
                    self.jobs_indicator(ui);
                }
                if !self.status.is_empty() {
                    let strong = ui.visuals().strong_text_color();
                    ui.add(egui::Label::new(RichText::new(&self.status).color(strong)).truncate());
                }
                if self.cfg.show_fkey_bar {
                    self.fkey_bar(ctx, ui);
                }
            });
        }
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
        self.context_menu_ui(ctx);
        self.dialogs(ctx);
        timing.mark("dialogs");
        self.job_ui(ctx);
        timing.mark("operations");

        for v in &mut self.viewers {
            v.show(ctx);
        }
        self.viewers.retain(|v| v.open);
        timing.mark("viewers");
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

    struct Ui {
        ctx: egui::Context,
        app: App,
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
            let cfg = Config { use_recycle_bin: false, language: "en".into(), ..Config::default() };
            let app = App::with_config(&ctx, cfg, vec![left.to_string_lossy().into(), right.to_string_lossy().into()]);
            let mut ui = Ui { ctx, app, snap: Default::default(), last: None, screen: egui::vec2(1200.0, 800.0) };
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
        assert_eq!(ui.app.viewers.len(), 1, "extracted, then shown in the viewer");
        assert!(ui.app.viewers[0].title.contains("zpráva 🦎.txt"), "{}", ui.app.viewers[0].title);
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

    /// Wait for the plugin command running in the background.
    fn wait_plugin(ui: &mut Ui) {
        for _ in 0..500 {
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
        assert_eq!(plugin_banner(&ui.app.plugins.name_of(a.plugin.id()), &a.file), "🧩 memserver · mem://test-ui");
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
        assert_eq!(conns.iter().map(|c| (c.label.as_str(), c.detail.as_str())).collect::<Vec<_>>(), [("mem://test-ui", "memserver")]);
        *cursor = drives.len();
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
        assert_eq!(conns.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["mem://test-ui", "mem://test-ui-2"]);
        ui.key(Key::Delete);
        let Dialog::Drive { conns, .. } = &ui.app.dialog else { panic!("the selector stays open") };
        assert_eq!(conns.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["mem://test-ui"]);
        assert!(plugins::mount_root_of(&second).is_none());
        assert_eq!(ui.app.panels[1].path(), local.as_path(), "the panel left the closed session");
        assert_eq!(ui.app.panels[0].path(), root.as_path(), "the other session stays");
        ui.key(Key::Escape);

        // disconnecting returns the panel to where it was, and closes the connection
        ui.app.close_connection(&root);
        ui.app.command(&ctx, Cmd::DriveMenu(1));
        assert!(matches!(&ui.app.dialog, Dialog::Drive { conns, .. } if conns.is_empty()), "gone from the drive selector");
        ui.key(Key::Escape);
        assert_eq!(ui.app.panels[0].path(), local.as_path());
        assert!(plugins::mount_root_of(&root).is_none());
        // the plugin closes both sessions (in the background)
        for _ in 0..100 {
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
            assert_eq!(ui.app.viewers.len(), 1, "{case}");
            assert!(ui.app.viewers[0].title.contains("archiv ž.zip"), "{case}: {}", ui.app.viewers[0].title);

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

    #[test]
    fn viewer_opens_in_embedded_viewport() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("ж.txt"), "Příliš 日本 😀").unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.text("ж");
        ui.key(Key::F3);
        assert_eq!(ui.app.viewers.len(), 1);
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

    fn click(ui: &mut Ui, pos: egui::Pos2) {
        ui.run(vec![Event::PointerMoved(pos)], Modifiers::NONE);
        let button = |pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        ui.run(vec![button(true)], Modifiers::NONE);
        ui.run(vec![button(false)], Modifiers::NONE);
        ui.frames(2);
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
        let menu = CtxMenu { panel: 0, items: vec![dir.clone()], dir: t.path().to_path_buf(), pos: egui::pos2(100.0, 100.0), can_paste: false };
        ui.app.run_menu_action(&ui.ctx.clone(), menu, MenuAction::CalcSize);
        wait_sizes(&mut ui);
        assert_eq!(ui.app.dir_sizes.get(&dir).unwrap().size.bytes, 4096);
        let menu = CtxMenu { panel: 0, items: vec![dir.clone()], dir: t.path().to_path_buf(), pos: egui::pos2(100.0, 100.0), can_paste: false };
        ui.app.run_menu_action(&ui.ctx.clone(), menu, MenuAction::Open);
        assert_eq!(ui.app.panels[0].path(), dir.as_path());

        // Delete goes through the normal confirmation with exactly the menu's items
        let menu = CtxMenu { panel: 0, items: vec![dir.join("x.bin")], dir: dir.clone(), pos: egui::pos2(100.0, 100.0), can_paste: false };
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

    /// The update window in each state; Escape closes it except while the
    /// download runs. SNAPSHOT_DIR set: also saved as update_*.png.
    #[test]
    fn update_dialog_states() {
        use crate::update::{Progress, Release};
        use std::sync::{Arc, Mutex};
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        crate::fonts::install(&ui.ctx, "", "");
        let release = Release {
            version: "0.3.0-alpha".into(),
            page: "https://github.com/nocturneop15/RustySalamander/releases/tag/v0.3.0-alpha".into(),
            asset: "RustySalamander-0.3.0-alpha-win64.zip".into(),
            asset_url: String::new(),
            asset_size: 7_126_108,
        };
        let states = [
            ("offer", None),
            ("downloading", Some(Progress::Downloading { done: 3_100_000, total: 7_126_108 })),
            (
                "done",
                Some(Progress::Done {
                    folder: PathBuf::from(r"D:\Applications\RustySalamander\0.3.0-alpha"),
                    copied: vec!["config.toml".into(), r"plugins\scp\known_hosts".into()],
                }),
            ),
            ("failed", Some(Progress::Failed("https://github.com/…: the server answered 404".into()))),
        ];
        for (name, state) in states {
            let running = matches!(state, Some(Progress::Downloading { .. }));
            ui.app.dialog = Dialog::Update { release: release.clone(), install: state.map(|p| Arc::new(Mutex::new(p))) };
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

    /// A long drive list in a low window stays below the menu bar and
    /// scrolls, keeping the chosen row in view.
    #[test]
    fn drive_selector_stays_below_the_menu() {
        let t = tempfile::tempdir().unwrap();
        let mut ui = Ui::new(t.path(), t.path());
        ui.screen = egui::vec2(900.0, 320.0);
        ui.app.cfg.typed_paths = (0..12).map(|i| format!(r"C:\cesta {i}")).collect();
        ui.frames(2);
        ui.app.command(&ui.ctx.clone(), Cmd::DriveMenu(1));
        ui.frames(3);
        let popup = ui.app.drive_popup_rect.expect("drawn");
        let body = ui.app.body_rects[1];
        assert!(popup.top() >= body.top() - 0.5, "covers the menu or path bar: {popup:?} above {body:?}");
        assert!(popup.bottom() <= 320.5, "{popup:?}");
        // the last row, reached with the keyboard
        for _ in 0..30 {
            ui.key(Key::ArrowDown);
        }
        ui.frames(3);
        let popup = ui.app.drive_popup_rect.expect("drawn");
        assert!(popup.top() >= body.top() - 0.5 && popup.bottom() <= 320.5, "{popup:?}");
        let Dialog::Drive { drives, conns, paths, cursor, .. } = &ui.app.dialog else { panic!() };
        assert_eq!(*cursor, drives.len() + conns.len() + paths.len() - 1);
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
        // built-in by default
        ui.key(Key::F3);
        assert_eq!(ui.app.viewers.len(), 1);
        ui.app.viewers.clear();
        // a custom viewer is started instead (here one that doesn't exist: an error, no built-in window)
        ui.app.cfg.use_custom_viewer = true;
        ui.app.cfg.viewer = r#""C:\No Such Dir\viewer.exe""#.into();
        ui.key(Key::F3);
        assert!(ui.app.viewers.is_empty());
        assert!(matches!(&ui.app.dialog, Dialog::Message { text, .. } if text.contains("viewer.exe")), "reports the program that failed");
        // switching back to the built-in one keeps the custom command for later
        ui.key(Key::Escape);
        ui.app.cfg.use_custom_viewer = false;
        ui.key(Key::F3);
        assert_eq!(ui.app.viewers.len(), 1);
        assert!(!ui.app.cfg.viewer.is_empty());
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
        ui.app.cfg.use_custom_editor = true;
        ui.app.cfg.editor = r#""C:\Program Files\Notepad++\notepad++.exe""#.into();
        ui.app.dialog = Dialog::Settings { tab: SettingsTab::General, recording: None };
        ui.frames(25);
        let bg = ui.ctx.style().visuals.panel_fill;
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("external_programs.png"));
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

        // switched off: an arrow ends the search and moves one row
        ui.app.cfg.quick_search_arrows = false;
        ui.key(Key::Home);
        ui.text("rep");
        ui.key(Key::ArrowDown);
        assert!(ui.app.panels[0].qs.is_empty());
        assert_eq!(ui.cursor_name(), "annual.doc");
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
        click(&mut ui, egui::pos2(250.0, 10.0)); // Options menu
        ui.frames(3);
        let out = ui.last.take().unwrap();
        ui.snap.render(&ui.ctx, out, bg, &out_dir.join("options_menu.png"));
    }
}
