//! Configuration stored as UTF-8 TOML in `config.toml` next to the program
//! (portable), or in `%APPDATA%\RustySalamander\config.toml` once the
//! program was switched to multi-user mode (see [`multi_user`]). The original
//! kept everything in the registry, see `regwork.cpp`.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::shell::TerminalKind;
use crate::sort::SortSpec;

/// What F3 or F4 opens a file with (Settings → Viewers and editors).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenWith {
    /// A plugin's viewer or editor: the plugin's id and its opener's id.
    Plugin { plugin: String, opener: String },
    /// Notepad.
    Notepad,
    /// A program's command line; `{file}` is replaced by the file, without
    /// it the file is added at the end. The file is an argument of its own;
    /// `shell` ("Run through a shell") passes the command line as typed,
    /// and only then may the program be a command interpreter
    /// ([`crate::shell::is_interpreter`]).
    Program {
        command: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        shell: bool,
    },
}

impl OpenWith {
    /// The default viewer: the Viewer plugin.
    pub fn viewer_plugin() -> OpenWith {
        OpenWith::Plugin { plugin: "viewer".into(), opener: "text".into() }
    }
}

/// Files whose names end with `.<suffix>` open with a viewer or editor of
/// their own instead of the default (`None`: the default).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SuffixRule {
    /// Without the dot, any case: `txt`, `tar.gz`.
    pub suffix: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<OpenWith>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<OpenWith>,
}

impl SuffixRule {
    /// A suffix as typed (`.TXT`, `*.txt`, ` txt `) as it is kept: `txt`.
    pub fn normalize(suffix: &str) -> String {
        suffix.trim().trim_start_matches(['*', '.']).trim().to_lowercase()
    }

    /// Whether a file with this name (no folder) has the suffix.
    pub fn matches(&self, name: &str) -> bool {
        let suffix = SuffixRule::normalize(&self.suffix);
        !suffix.is_empty() && name.to_lowercase().ends_with(&format!(".{suffix}"))
    }
}

/// The rule for a file name: the one with the longest matching suffix
/// (`tar.gz` before `gz`).
pub fn suffix_rule<'a>(rules: &'a [SuffixRule], name: &str) -> Option<&'a SuffixRule> {
    rules.iter().filter(|r| r.matches(name)).max_by_key(|r| SuffixRule::normalize(&r.suffix).len())
}

/// Drive buttons in each panel's path bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DriveButtons {
    /// One button showing the current drive (opens the drive menu).
    #[default]
    Current,
    /// A button for every drive.
    All,
}

/// Modifier that, combined with a letter key, switches the active panel to that drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DriveHotkey {
    Disabled,
    #[default]
    Shift,
    Alt,
    CtrlShift,
}

impl DriveHotkey {
    pub const ALL: [DriveHotkey; 4] = [DriveHotkey::Disabled, DriveHotkey::Shift, DriveHotkey::Alt, DriveHotkey::CtrlShift];

    /// The choice as Settings shows it (translated there).
    pub fn label(self) -> &'static str {
        match self {
            DriveHotkey::Disabled => "Disabled",
            DriveHotkey::Shift => "Shift + letter",
            DriveHotkey::Alt => "Alt + letter",
            DriveHotkey::CtrlShift => "Ctrl+Shift + letter",
        }
    }

    /// Exact modifier match; AltGr (reported as Ctrl+Alt) never matches.
    pub fn matches(self, ctrl: bool, shift: bool, alt: bool) -> bool {
        match self {
            DriveHotkey::Disabled => false,
            DriveHotkey::Shift => shift && !ctrl && !alt,
            DriveHotkey::Alt => alt && !ctrl && !shift,
            DriveHotkey::CtrlShift => ctrl && shift && !alt,
        }
    }
}

/// Color theme of the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemeMode {
    /// Follow the Windows app theme.
    #[default]
    System,
    Light,
    Dark,
}

/// How the right mouse button opens the context menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ContextMenuTrigger {
    /// A normal right-click opens the menu.
    #[default]
    ShortClick,
    /// Holding the right button opens it; a short right-click selects (Norton/Salamander style).
    LongPress,
}

/// Which context menu is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ContextMenuStyle {
    /// The program's commands, "Show more options" for the full menu.
    #[default]
    #[serde(alias = "Windows11")]
    Minimal,
    /// The program's commands and every shell extension in one menu.
    #[serde(alias = "Windows11Full", alias = "Classic")]
    Full,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct PanelConfig {
    pub path: String,
    pub sort: SortSpec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub left: PanelConfig,
    pub right: PanelConfig,
    pub right_active: bool,
    /// Show items with the hidden attribute.
    pub show_hidden: bool,
    /// Show items with the system attribute (protected OS files).
    pub show_system: bool,
    pub use_recycle_bin: bool,
    pub confirm_delete: bool,
    /// Items dropped on a folder or panel: the Copy or Move dialog first
    /// (off: the operation starts at once).
    pub confirm_drop: bool,
    /// What F3 opens files with, unless their suffix has its own.
    pub view_with: OpenWith,
    /// What F4 opens files with, unless their suffix has its own.
    pub edit_with: OpenWith,
    /// Suffixes with a viewer or editor of their own.
    pub suffix_rules: Vec<SuffixRule>,
    /// Older configs: F4 used `editor` instead of Notepad (read, never
    /// written; see `edit_with`).
    #[serde(skip_serializing)]
    pub use_custom_editor: bool,
    /// The last custom editor command line, offered again when "Program" is
    /// chosen ({file} = the file; otherwise appended).
    pub editor: String,
    /// Older configs: F3 used `viewer` (read, never written).
    #[serde(skip_serializing)]
    pub use_custom_viewer: bool,
    /// The last custom viewer command line, like `editor`.
    pub viewer: String,
    pub font_size: f32,
    /// UI font family (installed font name; empty = Segoe UI).
    pub ui_font: String,
    /// Font of the viewer's text and hex modes (empty = Consolas).
    pub mono_font: String,
    /// What "Open command prompt here" starts.
    pub terminal: TerminalKind,
    /// Command line for [`TerminalKind::Custom`]; `{dir}` = current directory.
    pub terminal_custom: String,
    /// The custom terminal command runs through a shell (as typed; it may be
    /// a command interpreter): see [`crate::shell::terminal_launch`].
    pub terminal_shell: bool,
    pub drive_buttons: DriveButtons,
    pub drive_hotkey: DriveHotkey,
    /// Changing drive returns to the last directory visited on it (as Salamander does).
    pub drive_remember_dir: bool,
    /// Remember paths typed into the path bar and offer them in the drive selector.
    pub remember_typed_paths: bool,
    /// Typed paths, most recent first.
    pub typed_paths: Vec<String>,
    pub theme: ThemeMode,
    /// UI language code ("en", "cs", …); empty = the Windows language.
    pub language: String,
    /// Plugins turned off in Settings → Plugins (by id, e.g. "7zip").
    pub disabled_plugins: Vec<String>,
    /// Quick search matches anywhere in the name (otherwise from the first letter).
    pub quick_search_anywhere: bool,
    /// Quick search ignores diacritics: `vladce` finds `vládče` (otherwise
    /// the letters must be the same).
    pub quick_search_loose: bool,
    /// While a quick search is active, Up/Down jump to the previous/next match.
    pub quick_search_arrows: bool,
    /// Solid scrollbars that are always shown (instead of thin ones on hover).
    pub always_show_scrollbars: bool,
    /// Animated effects: windows and menus fading in, smooth scrolling to
    /// the cursor (quick search, the cursor keys). Off: everything at once.
    pub animations: bool,
    /// The accent color of the light theme, RGB: what is selected or active
    /// (the cursor row, the active path bar, selected tabs), and the tint of
    /// panels a plugin shows. `None` for the theme's own.
    pub accent_light: Option<[u8; 3]>,
    /// The accent color of the dark theme, like `accent_light`.
    pub accent_dark: Option<[u8; 3]>,
    /// Older configs: one accent color for both themes (0.5.5–0.5.8; 0.5.3
    /// called it `plugin_tint`). Read, never written.
    #[serde(alias = "plugin_tint", skip_serializing)]
    pub accent_color: Option<[u8; 3]>,
    /// The function-key command bar at the bottom of the window.
    pub show_fkey_bar: bool,
    /// The commands on the function-key bar, by shortcut action id
    /// ("copy", "find", …); `None` for the default F1…F10 set.
    pub fkey_bar: Option<Vec<String>>,
    /// Windows file icons by type (otherwise simple drawn icons).
    pub system_icons: bool,
    pub context_menu_trigger: ContextMenuTrigger,
    pub context_menu_style: ContextMenuStyle,
    /// How long the right button must be held with [`ContextMenuTrigger::LongPress`].
    pub long_press_ms: u32,
    /// Keyboard shortcut overrides: action id -> bindings such as
    /// `["Ctrl+K"]`; an empty list disables the action's shortcut. Actions
    /// not listed use their defaults.
    pub shortcuts: BTreeMap<String, Vec<String>>,
    /// Pre-0.2 single "root of drive" shortcut; read for migration only.
    #[serde(skip_serializing)]
    pub root_shortcut: Option<String>,
    /// Hot paths, Ctrl+Shift+0..9 to set, Ctrl+0..9 to go (Salamander's "Hot Paths").
    pub hot_paths: Vec<String>,
    /// Look for a new release on GitHub when the program starts.
    pub check_updates: bool,
    /// Official plugins the user doesn't want offered when the program
    /// starts (by id); Help → Check for updates still shows them.
    pub declined_plugins: Vec<String>,
    /// How Copy (F5) starts: what its dialog offers each time.
    pub copy_defaults: TransferDefaults,
    /// How Move (F6) starts: what its dialog offers each time.
    pub move_defaults: TransferDefaults,
}

/// What the Copy or the Move dialog offers when it opens (Settings → Files);
/// a change in the dialog is for that operation only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TransferDefaults {
    /// Run in the background: no progress window.
    pub background: bool,
    /// Show the result window when it ends (always with a safe copy).
    pub show_result: bool,
    /// Safe copy: how each copy is checked.
    pub verify: crate::fileops::Verify,
}

impl TransferDefaults {
    /// Whether the result window opens: when asked, and always after a safe copy.
    pub fn reports(&self) -> bool {
        self.show_result || self.verify != crate::fileops::Verify::None
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            left: PanelConfig::default(),
            right: PanelConfig::default(),
            right_active: false,
            show_hidden: true,
            show_system: true,
            use_recycle_bin: true,
            confirm_delete: true,
            confirm_drop: true,
            view_with: OpenWith::viewer_plugin(),
            edit_with: OpenWith::Notepad,
            suffix_rules: Vec::new(),
            use_custom_editor: false,
            editor: String::new(),
            use_custom_viewer: false,
            viewer: String::new(),
            font_size: 14.0,
            ui_font: String::new(),
            mono_font: String::new(),
            terminal: TerminalKind::Default,
            terminal_custom: String::new(),
            terminal_shell: false,
            drive_buttons: DriveButtons::Current,
            drive_hotkey: DriveHotkey::Shift,
            drive_remember_dir: true,
            remember_typed_paths: true,
            typed_paths: Vec::new(),
            theme: ThemeMode::System,
            quick_search_anywhere: false,
            quick_search_loose: true,
            quick_search_arrows: true,
            language: String::new(),
            disabled_plugins: Vec::new(),
            always_show_scrollbars: false,
            animations: true,
            accent_light: None,
            accent_dark: None,
            accent_color: None,
            show_fkey_bar: true,
            fkey_bar: None,
            system_icons: true,
            context_menu_trigger: ContextMenuTrigger::ShortClick,
            context_menu_style: ContextMenuStyle::Minimal,
            long_press_ms: 500,
            shortcuts: BTreeMap::new(),
            root_shortcut: None,
            hot_paths: vec![String::new(); 10],
            check_updates: true,
            declined_plugins: Vec::new(),
            copy_defaults: TransferDefaults::default(),
            move_defaults: TransferDefaults::default(),
        }
    }
}

/// The folder of the program: configuration and languages live here
/// (portable mode).
pub fn app_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

/// The file next to the program that switches it to multi-user mode for
/// good: settings then live in each user's `%APPDATA%\RustySalamander`
/// instead of the program folder.
pub const MULTI_USER_FILE: &str = "multiuser.toml";

/// Whether the program keeps its settings per user (in `%APPDATA%`) rather
/// than next to itself (portable, the default).
pub fn multi_user() -> bool {
    app_dir().is_some_and(|d| d.join(MULTI_USER_FILE).is_file())
}

/// Switch the program folder `dir` to multi-user mode for good.
pub fn write_multi_user_marker(dir: &Path) -> io::Result<()> {
    fs::write(
        dir.join(MULTI_USER_FILE),
        "# RustySalamander keeps each user's settings in %APPDATA%\\RustySalamander\n\
         # instead of next to the program (multi-user mode, chosen in Settings).\n\
         mode = \"multi-user\"\n",
    )
}

/// Where everything is saved in multi-user mode: `%APPDATA%\RustySalamander`.
pub fn user_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("RustySalamander")
}

/// The folder settings are saved in: the program's own (portable), or
/// [`user_dir`] in multi-user mode.
pub fn settings_dir() -> Option<PathBuf> {
    if multi_user() {
        Some(user_dir())
    } else {
        app_dir()
    }
}

/// Where the configuration is saved: `config.toml` in [`settings_dir`].
/// `RUSTYSALAMANDER_CONFIG` names another file instead.
pub fn config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("RUSTYSALAMANDER_CONFIG") {
        return PathBuf::from(p);
    }
    settings_dir().unwrap_or_else(user_dir).join("config.toml")
}

/// The per-user configuration file of multi-user mode.
pub fn user_config_path() -> PathBuf {
    user_dir().join("config.toml")
}

/// Set by the program in multi-user mode for the plugins: the folder below
/// which each plugin keeps its files (`sal_plugin_api::data_dir`).
pub const PLUGIN_DATA_ENV: &str = "RUSTYSALAMANDER_PLUGIN_DATA";

/// Where plugins keep their files in multi-user mode:
/// `%APPDATA%\RustySalamander\plugins`.
pub fn user_plugin_data() -> PathBuf {
    user_dir().join("plugins")
}

/// Whether the file `rel` (relative to its plugin's folder) is something
/// the plugin saved there, rather than part of the plugin as installed
/// (programs, libraries, translations, licences, change logs).
pub fn is_plugin_data(rel: &Path) -> bool {
    let mut parts = rel.components().map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase()).collect::<Vec<_>>();
    let Some(name) = parts.pop() else { return false };
    if parts.iter().any(|d| d == "lang" || d.starts_with('.')) || name.starts_with('.') {
        return false;
    }
    let ext = Path::new(&name).extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    !matches!(ext.as_str(), "spl" | "dll" | "exe" | "pdb" | "md")
        && !["license", "licence", "notice", "readme", "changelog"].iter().any(|w| name.contains(w))
}

/// Copy what the plugins saved in their folders below `plugins` into
/// `to\<plugin folder>`. Returns the files copied (their originals).
pub fn copy_plugin_data(plugins: &Path, to: &Path) -> io::Result<Vec<PathBuf>> {
    fn walk(base: &Path, rel: &Path, to: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
        let Ok(dir) = fs::read_dir(base.join(rel)) else { return Ok(()) };
        for de in dir.flatten() {
            let rel = rel.join(de.file_name());
            if de.file_type()?.is_dir() {
                walk(base, &rel, to, out)?;
                continue;
            }
            if !is_plugin_data(&rel) {
                continue;
            }
            let dest = to.join(&rel);
            if let Some(d) = dest.parent() {
                fs::create_dir_all(d)?;
            }
            fs::copy(base.join(&rel), &dest)?;
            out.push(base.join(&rel));
        }
        Ok(())
    }
    let mut out = Vec::new();
    let Ok(dir) = fs::read_dir(plugins) else { return Ok(out) };
    // one folder per plugin; .updates and the like belong to the program
    for de in dir.flatten() {
        let name = de.file_name();
        if de.file_type()?.is_dir() && !name.to_string_lossy().starts_with('.') {
            let base = plugins.join(&name);
            let mut found = Vec::new();
            walk(&base, Path::new(""), &to.join(&name), &mut found)?;
            out.extend(found);
        }
    }
    Ok(out)
}

/// Whether settings can be saved where they belong: false for a portable
/// program in a folder the user can't write to (e.g. under Program Files).
pub fn settings_writable() -> bool {
    if std::env::var_os("RUSTYSALAMANDER_CONFIG").is_some() || multi_user() {
        return true;
    }
    let Some(dir) = app_dir() else { return false };
    let probe = dir.join(".write-test");
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

impl Config {
    /// The saved settings (see [`config_path`]), or the defaults. A
    /// portable program never reads settings from `%APPDATA%`.
    pub fn load() -> Config {
        let text = fs::read_to_string(config_path()).unwrap_or_default();
        let mut c = Config::parse(&text);
        c.hot_paths.resize(10, String::new());
        c
    }

    /// Settings from TOML text; what older versions saved is carried over.
    fn parse(text: &str) -> Config {
        let mut c: Config = toml::from_str(text).unwrap_or_default();
        // before the editor switch existed, a non-empty editor meant "use it"
        if !text.contains("use_custom_editor") {
            c.use_custom_editor = !c.editor.trim().is_empty();
        }
        // before the viewer and editor could be chosen from plugins
        if !text.contains("view_with") && c.use_custom_viewer && !c.viewer.trim().is_empty() {
            c.view_with = OpenWith::Program { command: c.viewer.clone(), shell: false };
        }
        if !text.contains("edit_with") && c.use_custom_editor && !c.editor.trim().is_empty() {
            c.edit_with = OpenWith::Program { command: c.editor.clone(), shell: false };
        }
        // before each theme had its own accent color
        if let Some(both) = c.accent_color.take() {
            if !text.contains("accent_light") && !text.contains("accent_dark") {
                (c.accent_light, c.accent_dark) = (Some(both), Some(both));
            }
        }
        c
    }

    /// Save to [`config_path`]. A portable program in a folder it can't
    /// write to doesn't save (there is no quiet fallback to `%APPDATA%`).
    pub fn save(&self) -> io::Result<()> {
        self.save_to(&config_path())
    }

    /// Save to the file `p`: whole or not at all (written beside it, then
    /// put in its place), so a stop half way never leaves it damaged.
    pub fn save_to(&self, p: &Path) -> io::Result<()> {
        let s = toml::to_string_pretty(self).map_err(io::Error::other)?;
        locked(p, || replace(p, &s))
    }

    /// Save to the file `p` the settings changed since `base` (the settings
    /// as this copy of the program last saved or read them), keeping what
    /// another copy saved there meanwhile: the file is read, the values that
    /// differ between `base` and these are put in it (others left as they
    /// are), and it is written whole, with no other copy doing the same
    /// meanwhile. A value both changed: these win (saved last).
    pub fn save_changes(&self, base: &Config, p: &Path) -> io::Result<()> {
        let value = |c: &Config| toml::Value::try_from(c).map_err(io::Error::other);
        let (mine, base) = (value(self)?, value(base)?);
        locked(p, || {
            // no file: these settings; one that can't be read isn't written over
            // (an error: tried again later); a damaged one (not TOML, or a
            // setting of the wrong kind, which would read as all defaults) is
            // kept beside, and these settings saved in its place
            let on_disk = match fs::read_to_string(p) {
                Ok(text) => match toml::from_str::<toml::Value>(&text) {
                    Ok(v) if v.is_table() && toml::from_str::<Config>(&text).is_ok() => Some(v),
                    _ => {
                        keep_damaged(p)?;
                        None
                    }
                },
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };
            let merged = match on_disk {
                Some(mut disk) => {
                    merge_changes(&mut disk, &base, &mine);
                    let text = toml::to_string(&disk).map_err(io::Error::other)?;
                    // read strictly: never all defaults in place of what was there,
                    // never these settings alone over the other copy's (an error:
                    // the file stays as it is); then through parse, which carries
                    // over what older versions saved
                    toml::from_str::<Config>(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("the settings merged with the file's aren't valid: {e}")))?;
                    Config::parse(&text)
                }
                None => self.clone(),
            };
            replace(p, &toml::to_string_pretty(&merged).map_err(io::Error::other)?)
        })
    }
}

/// The settings that are one of a choice of kinds ([`OpenWith`]: a
/// program, Notepad, or a plugin's viewer): merged whole, never mixed with
/// another kind.
const WHOLE_CHOICES: &[&str] = &["view_with", "edit_with"];

/// Put into `disk` (the whole settings, a table) the values that differ
/// between `base` and `mine`; the rest of `disk` stays. A setting that is
/// a table is merged key by key, and so deeper (a panel; the shortcuts, a
/// map: another copy's keys of other actions stay, also ones it added),
/// but for [`WHOLE_CHOICES`], put in whole.
fn merge_changes(disk: &mut toml::Value, base: &toml::Value, mine: &toml::Value) {
    merge_tables(disk, base, mine, WHOLE_CHOICES);
}

/// [`merge_changes`] of one table; `whole`: its keys put in whole.
fn merge_tables(disk: &mut toml::Value, base: &toml::Value, mine: &toml::Value, whole: &[&str]) {
    let (Some(d), Some(b), Some(m)) = (disk.as_table_mut(), base.as_table(), mine.as_table()) else {
        if base != mine {
            *disk = mine.clone();
        }
        return;
    };
    let keys: std::collections::BTreeSet<&String> = b.keys().chain(m.keys()).collect();
    for k in keys {
        match (b.get(k), m.get(k)) {
            (bv, mv) if bv == mv => {}
            (Some(bv), Some(mv)) if !whole.contains(&k.as_str()) && bv.is_table() && mv.is_table() && d.get(k).is_some_and(toml::Value::is_table) => {
                merge_tables(d.get_mut(k).unwrap(), bv, mv, &[]);
            }
            (_, Some(mv)) => {
                d.insert(k.clone(), mv.clone());
            }
            (_, None) => {
                d.remove(k);
            }
        }
    }
}

/// Run `f` (reading, changing and writing the file `p`) holding its lock:
/// the file `<p>.lock` beside it, locked for this program alone, so another
/// copy of the program saving the same file waits until `f` is done.
pub fn locked<R>(p: &Path, f: impl FnOnce() -> io::Result<R>) -> io::Result<R> {
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut name = p.file_name().unwrap_or_default().to_owned();
    name.push(".lock");
    let lock = fs::OpenOptions::new().create(true).truncate(false).write(true).open(p.with_file_name(name))?;
    lock.lock()?;
    let result = f();
    let _ = lock.unlock();
    result
}

/// Keep the damaged file `p` (one that can't be understood) beside it as
/// `<p>.damaged` (replacing an older one), before a new one is written in
/// its place: nothing in it is lost.
fn keep_damaged(p: &Path) -> io::Result<()> {
    let mut name = p.file_name().unwrap_or_default().to_owned();
    name.push(".damaged");
    let kept = p.with_file_name(name);
    let _ = fs::remove_file(&kept);
    fs::rename(p, kept)
}

/// Write `text` as the file `p`, whole or not at all: written beside it
/// under a name of its own (no other save, here or in another copy of the
/// program, writes the same one), then put in its place.
fn replace(p: &Path, text: &str) -> io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut name = p.file_name().unwrap_or_default().to_owned();
    name.push(format!(".{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let tmp = p.with_file_name(name);
    fs::write(&tmp, text)?;
    fs::rename(&tmp, p).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two copies of the program, each saving what it changed: what the
    /// other saved meanwhile is kept, also inside a table (a panel), and
    /// also when both save at the same time.
    #[test]
    fn copies_keep_each_others_changes() {
        let t = std::env::temp_dir().join(format!("sal-config-two-{}", std::process::id()));
        let p = t.join("config.toml");
        let start = Config::default();
        start.save_to(&p).unwrap();
        // both start from the same settings
        let (mut one, mut two) = (start.clone(), start.clone());
        one.show_hidden = !start.show_hidden;
        one.left.path = "C:\\one".into();
        one.save_changes(&start, &p).unwrap();
        two.language = "cs".into();
        two.right.path = "D:\\two".into();
        two.save_changes(&start, &p).unwrap();
        let saved = Config::parse(&fs::read_to_string(&p).unwrap());
        assert_eq!(saved.show_hidden, one.show_hidden, "the first copy's change kept");
        assert_eq!(saved.left.path, "C:\\one");
        assert_eq!((saved.language.as_str(), saved.right.path.as_str()), ("cs", "D:\\two"));
        // a value both changed: the later save wins
        let mut three = start.clone();
        three.language = "en".into();
        three.save_changes(&start, &p).unwrap();
        assert_eq!(Config::parse(&fs::read_to_string(&p).unwrap()).language, "en");
        // at the same time, over and over: every change kept
        let threads: Vec<_> = [0usize, 1]
            .into_iter()
            .map(|who| {
                let (p, start) = (p.clone(), start.clone());
                std::thread::spawn(move || {
                    let mut base = start.clone();
                    for i in 0..20 {
                        let mut mine = base.clone();
                        mine.hot_paths.resize(10, String::new());
                        mine.hot_paths[who] = format!("{who}-{i}");
                        if who == 0 {
                            mine.left.path = format!("L{i}");
                        } else {
                            mine.right.path = format!("R{i}");
                        }
                        mine.save_changes(&base, &p).unwrap();
                        base = mine;
                    }
                })
            })
            .collect();
        threads.into_iter().for_each(|h| h.join().unwrap());
        let saved = Config::parse(&fs::read_to_string(&p).unwrap());
        assert_eq!((saved.left.path.as_str(), saved.right.path.as_str()), ("L19", "R19"), "both copies' panels");
        // a choice of kinds changed in both: one copy edits the viewer program's
        // command, the other turns to a plugin's viewer; each a setting more.
        // Never mixed: the later save's whole choice, both other settings kept
        let mut base = start.clone();
        base.view_with = OpenWith::Program { command: "a.exe".into(), shell: false };
        base.save_to(&p).unwrap();
        let (mut edits, mut turns) = (base.clone(), base.clone());
        turns.view_with = OpenWith::Plugin { plugin: "viewer".into(), opener: "text".into() };
        turns.font_size = 21.0;
        turns.save_changes(&base, &p).unwrap();
        edits.view_with = OpenWith::Program { command: "b.exe".into(), shell: true };
        edits.language = "de".into();
        edits.save_changes(&base, &p).unwrap();
        let text = fs::read_to_string(&p).unwrap();
        let saved: Config = toml::from_str(&text).expect("valid");
        assert_eq!(saved.view_with, edits.view_with, "the later choice whole: {text}");
        assert_eq!((saved.font_size, saved.language.as_str()), (21.0, "de"), "both copies' other settings");
        // both start with no shortcuts of their own and each adds one for a
        // different action, then one changes and removes: every change kept
        let (mut first, mut second) = (saved.clone(), saved.clone());
        assert!(saved.shortcuts.is_empty());
        first.shortcuts.insert("copy".into(), vec!["Ctrl+F5".into()]);
        first.save_changes(&saved, &p).unwrap();
        second.shortcuts.insert("move".into(), vec!["Ctrl+F6".into()]);
        second.save_changes(&saved, &p).unwrap();
        let both = Config::parse(&fs::read_to_string(&p).unwrap());
        assert_eq!(both.shortcuts.get("copy").map(Vec::as_slice), Some(&["Ctrl+F5".to_owned()][..]), "{:?}", both.shortcuts);
        assert_eq!(both.shortcuts.get("move").map(Vec::as_slice), Some(&["Ctrl+F6".to_owned()][..]));
        let first_base = first.clone();
        first.shortcuts.remove("copy");
        first.shortcuts.insert("find".into(), vec![]);
        first.save_changes(&first_base, &p).unwrap();
        let after = Config::parse(&fs::read_to_string(&p).unwrap());
        assert_eq!(after.shortcuts.keys().collect::<Vec<_>>(), ["find", "move"], "the removal and the addition, the other's kept");
        // a damaged file: kept beside, these settings saved in its place
        fs::write(&p, "not [ toml").unwrap();
        one.save_changes(&start, &p).unwrap();
        assert_eq!(fs::read_to_string(t.join("config.toml.damaged")).unwrap(), "not [ toml");
        assert_eq!(Config::parse(&fs::read_to_string(&p).unwrap()).left.path, "C:\\one");
        // TOML, but a setting of the wrong kind beside settings of the user's:
        // it would read as all defaults; the file is kept beside too
        let wrong = "show_hidden = \"yes\"\nlanguage = \"cs\"\nfont_size = 17.0\n";
        fs::write(&p, wrong).unwrap();
        let mut four = start.clone();
        four.right.path = "E:\\four".into();
        four.save_changes(&start, &p).unwrap();
        assert_eq!(fs::read_to_string(t.join("config.toml.damaged")).unwrap(), wrong, "the user's settings kept");
        let saved = Config::parse(&fs::read_to_string(&p).unwrap());
        assert_eq!(saved.right.path, "E:\\four", "this copy's settings saved");
        // one that can't be read (a folder in its place): an error, not written over
        let dir = t.join("folder.toml");
        fs::create_dir(&dir).unwrap();
        assert!(one.save_changes(&start, &dir).is_err());
        assert!(dir.is_dir());
        fs::remove_dir_all(&t).unwrap();
    }

    #[test]
    fn portable_location() {
        // next to the program (here: the test executable)
        let exe_dir = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
        assert_eq!(app_dir().unwrap(), exe_dir);
        if std::env::var_os("RUSTYSALAMANDER_CONFIG").is_none() {
            assert_eq!(config_path(), exe_dir.join("config.toml"));
        }
        assert!(user_config_path().ends_with("RustySalamander/config.toml") || user_config_path().ends_with(r"RustySalamander\config.toml"));
        // RUSTYSALAMANDER_CONFIG names the file to use instead
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("sub").join("my config.toml");
        std::env::set_var("RUSTYSALAMANDER_CONFIG", &file);
        assert_eq!(config_path(), file);
        let c = Config { language: "cs".into(), font_size: 17.0, ..Config::default() };
        c.save().unwrap();
        let back = Config::load();
        std::env::remove_var("RUSTYSALAMANDER_CONFIG");
        assert_eq!((back.language.as_str(), back.font_size), ("cs", 17.0));
    }

    #[test]
    fn plugin_data_is_moved_programs_stay() {
        for data in ["known_hosts", "last-connection", "known_certificates", "sub/state.toml"] {
            assert!(is_plugin_data(Path::new(data)), "{data}");
        }
        for program in ["scp.spl", "7z.dll", "License.txt", "unRarLicense.txt", "CHANGELOG.md", "lang/cs.toml", ".write-test", ".updates/x"] {
            assert!(!is_plugin_data(Path::new(program)), "{program}");
        }
        let t = tempfile::tempdir().unwrap();
        let plugins = t.path().join("plugins");
        fs::create_dir_all(plugins.join("scp").join("lang")).unwrap();
        fs::create_dir_all(plugins.join(".updates").join("scp")).unwrap();
        fs::write(plugins.join("scp").join("scp.spl"), "x").unwrap();
        fs::write(plugins.join("scp").join("known_hosts"), "host key").unwrap();
        fs::write(plugins.join("scp").join("lang").join("cs.toml"), "x").unwrap();
        fs::write(plugins.join(".updates").join("scp").join("known_hosts"), "x").unwrap();
        let to = t.path().join("Uživatel 🦎").join("plugins");
        let moved = copy_plugin_data(&plugins, &to).unwrap();
        assert_eq!(moved, vec![plugins.join("scp").join("known_hosts")]);
        assert_eq!(fs::read_to_string(to.join("scp").join("known_hosts")).unwrap(), "host key");
        assert!(!to.join("scp").join("scp.spl").exists() && !to.join(".updates").exists());
    }

    #[test]
    fn defaults() {
        let c = Config::default();
        assert!(c.show_hidden && c.show_system, "hidden and system files are shown by default");
        let old: Config = toml::from_str("show_hidden = true").unwrap();
        assert!(old.show_system, "configs from before the split keep showing system files");
        assert_eq!(c.terminal, TerminalKind::Default);
        assert_eq!((c.drive_buttons, c.drive_hotkey), (DriveButtons::Current, DriveHotkey::Shift));
        assert!(c.shortcuts.is_empty());
        assert_eq!(c.theme, ThemeMode::System);
        assert_eq!((c.view_with.clone(), c.edit_with.clone()), (OpenWith::viewer_plugin(), OpenWith::Notepad));
        // older configs: a set editor or viewer keeps being used
        let old = Config::parse("editor = 'C:/Tools/ed.exe'");
        assert_eq!(old.edit_with, OpenWith::Program { command: "C:/Tools/ed.exe".into(), shell: false });
        let off = Config::parse("editor = 'C:/Tools/ed.exe'\nuse_custom_editor = false");
        assert_eq!(off.edit_with, OpenWith::Notepad);
        let viewer = Config::parse("viewer = 'v.exe {file}'\nuse_custom_viewer = true");
        assert_eq!(viewer.view_with, OpenWith::Program { command: "v.exe {file}".into(), shell: false }, "an older command: not through a shell");
        // and the new settings survive saving
        let c2 = Config {
            suffix_rules: vec![
                SuffixRule { suffix: "md".into(), view: Some(OpenWith::Program { command: "typora.exe".into(), shell: true }), edit: None },
                SuffixRule { suffix: "tar.gz".into(), view: None, edit: Some(OpenWith::Notepad) },
            ],
            edit_with: OpenWith::Plugin { plugin: "x".into(), opener: "ini".into() },
            ..Config::default()
        };
        let text = toml::to_string_pretty(&c2).unwrap();
        assert!(!text.contains("use_custom"), "{text}");
        let back = Config::parse(&text);
        assert_eq!((back.suffix_rules, back.edit_with, back.view_with), (c2.suffix_rules.clone(), c2.edit_with.clone(), c2.view_with.clone()));
        // the longest suffix wins, in any case
        assert_eq!(suffix_rule(&c2.suffix_rules, "Záloha.TAR.GZ").unwrap().suffix, "tar.gz");
        assert!(suffix_rule(&c2.suffix_rules, "a.gz").is_none() && suffix_rule(&c2.suffix_rules, "md").is_none());
        assert_eq!(SuffixRule::normalize(" *.TXT "), "txt");
        // the accent colors: each theme's own unless chosen
        assert_eq!((c.accent_light, c.accent_dark), (None, None));
        let tinted = Config::parse(&toml::to_string_pretty(&Config { accent_light: Some([200, 120, 0]), ..Config::default() }).unwrap());
        assert_eq!((tinted.accent_light, tinted.accent_dark), (Some([200, 120, 0]), None));
        // one color from before (0.5.3's plugin_tint, then accent_color) goes to both
        for old in ["plugin_tint = [1, 2, 3]", "accent_color = [1, 2, 3]"] {
            let c = Config::parse(old);
            assert_eq!((c.accent_light, c.accent_dark, c.accent_color), (Some([1, 2, 3]), Some([1, 2, 3]), None), "{old}");
        }
        assert!(!toml::to_string_pretty(&Config::parse("accent_color = [1, 2, 3]")).unwrap().contains("accent_color"));
        assert!(!c.always_show_scrollbars && c.show_fkey_bar && c.system_icons);
        let dark: Config = toml::from_str("theme = \"Dark\"").unwrap();
        assert_eq!(dark.theme, ThemeMode::Dark);
        assert_eq!((c.context_menu_trigger, c.context_menu_style, c.long_press_ms), (ContextMenuTrigger::ShortClick, ContextMenuStyle::Minimal, 500));
        let cm: Config = toml::from_str(
            "context_menu_trigger = \"LongPress\"
context_menu_style = \"Classic\"",
        )
        .unwrap();
        assert_eq!((cm.context_menu_trigger, cm.context_menu_style), (ContextMenuTrigger::LongPress, ContextMenuStyle::Full));
        // overrides round-trip; the legacy field is read but never written
        let mut c2 = Config::default();
        c2.shortcuts.insert("copy".into(), vec!["Ctrl+K".into()]);
        c2.shortcuts.insert("delete".into(), vec![]);
        c2.root_shortcut = Some("Alt+R".into());
        let text = toml::to_string_pretty(&c2).unwrap();
        assert!(!text.contains("root_shortcut"));
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.shortcuts, c2.shortcuts);
        let legacy: Config = toml::from_str("root_shortcut = \"Alt+R\"").unwrap();
        assert_eq!(legacy.root_shortcut.as_deref(), Some("Alt+R"));
        assert!(DriveHotkey::Shift.matches(false, true, false));
        assert!(!DriveHotkey::Alt.matches(true, false, true), "AltGr must not switch drives");
        // configs written by older versions get the new fields' defaults
        let old: Config = toml::from_str("show_hidden = false").unwrap();
        assert_eq!(old.terminal, TerminalKind::Default);
        let custom: Config = toml::from_str(
            "terminal = \"Custom\"
terminal_custom = 'nu.exe'",
        )
        .unwrap();
        assert_eq!((custom.terminal, custom.terminal_custom.as_str()), (TerminalKind::Custom, "nu.exe"));
    }

    #[test]
    fn roundtrip_unicode_paths() {
        let mut c = Config::default();
        c.left.path = r"C:\Uživatelé\测试\🦎".into();
        let s = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.left.path, c.left.path);
    }
}
