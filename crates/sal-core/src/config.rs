//! Configuration stored as UTF-8 TOML in `%APPDATA%\RustySalamander\config.toml`
//! (the original kept everything in the registry, see `regwork.cpp`).

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
    /// it the file is added at the end.
    Program { command: String },
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
    /// Compact Windows 11 style menu with "Show more options".
    #[default]
    Windows11,
    /// The full classic shell menu (Windows 10 style, all shell extensions).
    Classic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct PanelConfig {
    pub path: String,
    pub sort: SortSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// While a quick search is active, Up/Down jump to the previous/next match.
    pub quick_search_arrows: bool,
    /// Solid scrollbars that are always shown (instead of thin ones on hover).
    pub always_show_scrollbars: bool,
    /// The function-key command bar at the bottom of the window.
    pub show_fkey_bar: bool,
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
            drive_buttons: DriveButtons::Current,
            drive_hotkey: DriveHotkey::Shift,
            drive_remember_dir: true,
            remember_typed_paths: true,
            typed_paths: Vec::new(),
            theme: ThemeMode::System,
            quick_search_anywhere: false,
            quick_search_arrows: true,
            language: String::new(),
            disabled_plugins: Vec::new(),
            always_show_scrollbars: false,
            show_fkey_bar: true,
            system_icons: true,
            context_menu_trigger: ContextMenuTrigger::ShortClick,
            context_menu_style: ContextMenuStyle::Windows11,
            long_press_ms: 500,
            shortcuts: BTreeMap::new(),
            root_shortcut: None,
            hot_paths: vec![String::new(); 10],
            check_updates: true,
        }
    }
}

/// The folder of the program: configuration and languages live here
/// (portable mode).
pub fn app_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

/// Where the configuration is saved: `config.toml` next to the program.
/// `RUSTYSALAMANDER_CONFIG` names another file instead.
pub fn config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("RUSTYSALAMANDER_CONFIG") {
        return PathBuf::from(p);
    }
    app_dir().map_or_else(user_config_path, |d| d.join("config.toml"))
}

/// The per-user location used before portable mode (`%APPDATA%`): read when
/// the program folder has no configuration yet, and written when the program
/// folder isn't writable.
pub fn user_config_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("RustySalamander").join("config.toml")
}

/// The configuration file to read: the portable one when it exists,
/// otherwise the per-user one (taken over on the next save).
pub fn config_source() -> PathBuf {
    let portable = config_path();
    if portable.exists() || std::env::var_os("RUSTYSALAMANDER_CONFIG").is_some() {
        return portable;
    }
    let user = user_config_path();
    if user.exists() {
        user
    } else {
        portable
    }
}

impl Config {
    pub fn load() -> Config {
        let text = fs::read_to_string(config_source()).unwrap_or_default();
        let mut c = Config::parse(&text);
        c.hot_paths.resize(10, String::new());
        c
    }

    fn parse(text: &str) -> Config {
        let mut c: Config = toml::from_str(text).unwrap_or_default();
        // before the editor switch existed, a non-empty editor meant "use it"
        if !text.contains("use_custom_editor") {
            c.use_custom_editor = !c.editor.trim().is_empty();
        }
        // before the viewer and editor could be chosen from plugins
        if !text.contains("view_with") && c.use_custom_viewer && !c.viewer.trim().is_empty() {
            c.view_with = OpenWith::Program { command: c.viewer.clone() };
        }
        if !text.contains("edit_with") && c.use_custom_editor && !c.editor.trim().is_empty() {
            c.edit_with = OpenWith::Program { command: c.editor.clone() };
        }
        c
    }

    /// Save next to the program; where that is not allowed (e.g. under
    /// Program Files), to the per-user location instead.
    pub fn save(&self) -> io::Result<()> {
        let s = toml::to_string_pretty(self).map_err(io::Error::other)?;
        let write = |p: &Path| -> io::Result<()> {
            if let Some(dir) = p.parent() {
                fs::create_dir_all(dir)?;
            }
            fs::write(p, &s)
        };
        let portable = config_path();
        match write(&portable) {
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied && std::env::var_os("RUSTYSALAMANDER_CONFIG").is_none() => write(&user_config_path()),
            r => r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(config_source(), file);
        let c = Config { language: "cs".into(), font_size: 17.0, ..Config::default() };
        c.save().unwrap();
        let back = Config::load();
        std::env::remove_var("RUSTYSALAMANDER_CONFIG");
        assert_eq!((back.language.as_str(), back.font_size), ("cs", 17.0));
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
        assert_eq!(old.edit_with, OpenWith::Program { command: "C:/Tools/ed.exe".into() });
        let off = Config::parse("editor = 'C:/Tools/ed.exe'\nuse_custom_editor = false");
        assert_eq!(off.edit_with, OpenWith::Notepad);
        let viewer = Config::parse("viewer = 'v.exe {file}'\nuse_custom_viewer = true");
        assert_eq!(viewer.view_with, OpenWith::Program { command: "v.exe {file}".into() });
        // and the new settings survive saving
        let mut c2 = Config::default();
        c2.suffix_rules = vec![
            SuffixRule { suffix: "md".into(), view: Some(OpenWith::Program { command: "typora.exe".into() }), edit: None },
            SuffixRule { suffix: "tar.gz".into(), view: None, edit: Some(OpenWith::Notepad) },
        ];
        c2.edit_with = OpenWith::Plugin { plugin: "x".into(), opener: "ini".into() };
        let text = toml::to_string_pretty(&c2).unwrap();
        assert!(!text.contains("use_custom"), "{text}");
        let back = Config::parse(&text);
        assert_eq!((back.suffix_rules, back.edit_with, back.view_with), (c2.suffix_rules.clone(), c2.edit_with.clone(), c2.view_with.clone()));
        // the longest suffix wins, in any case
        assert_eq!(suffix_rule(&c2.suffix_rules, "Záloha.TAR.GZ").unwrap().suffix, "tar.gz");
        assert!(suffix_rule(&c2.suffix_rules, "a.gz").is_none() && suffix_rule(&c2.suffix_rules, "md").is_none());
        assert_eq!(SuffixRule::normalize(" *.TXT "), "txt");
        assert!(!c.always_show_scrollbars && c.show_fkey_bar && c.system_icons);
        let dark: Config = toml::from_str("theme = \"Dark\"").unwrap();
        assert_eq!(dark.theme, ThemeMode::Dark);
        assert_eq!((c.context_menu_trigger, c.context_menu_style, c.long_press_ms), (ContextMenuTrigger::ShortClick, ContextMenuStyle::Windows11, 500));
        let cm: Config = toml::from_str(
            "context_menu_trigger = \"LongPress\"
context_menu_style = \"Classic\"",
        )
        .unwrap();
        assert_eq!((cm.context_menu_trigger, cm.context_menu_style), (ContextMenuTrigger::LongPress, ContextMenuStyle::Classic));
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
