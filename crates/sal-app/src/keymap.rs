//! User-configurable command shortcuts. Defaults follow Open Salamander; the
//! config stores only overrides (`[shortcuts]` table, action id → bindings).

use std::collections::{BTreeMap, HashMap};

use eframe::egui::{Key, Modifiers};

use crate::i18n::{n_, tr, trf};
use crate::shortcut::Shortcut;

pub type Overrides = BTreeMap<String, Vec<String>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Help,
    Rename,
    View,
    Edit,
    Copy,
    Move,
    MkDir,
    ChangeDir,
    Delete,
    DeletePermanent,
    Exit,
    Terminal,
    Settings,
    DriveMenuLeft,
    DriveMenuRight,
    Parent,
    EnterDir,
    Root,
    HistoryBack,
    HistoryForward,
    Refresh,
    ToggleHidden,
    SelectAll,
    UnselectAll,
    SelectCalcSize,
    CalcSelectionSize,
    ContextMenu,
    Properties,
    ToggleQuickSearchMode,
    ToggleSystem,
    ToggleTheme,
    SwapPanels,
    CopyPaths,
    SortName,
    SortExt,
    SortTime,
    SortSize,
    Find,
}

pub struct ActionInfo {
    /// Stable key in the config file.
    pub id: &'static str,
    pub label: &'static str,
    pub defaults: &'static [&'static str],
    /// Other sensible single bindings offered in the preset list.
    pub alternatives: &'static [&'static str],
}

/// An action's description (`const`, for the table below).
const fn info(id: &'static str, label: &'static str, defaults: &'static [&'static str], alternatives: &'static [&'static str]) -> ActionInfo {
    ActionInfo { id, label, defaults, alternatives }
}

/// Groups of the shortcut table (Settings and F1 help).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Files,
    Navigation,
    System,
}

impl Section {
    pub const ALL: [Section; 3] = [Section::Files, Section::Navigation, Section::System];

    /// The section's title (marked for translation).
    pub fn label(self) -> &'static str {
        match self {
            Section::Files => n_("File & folder operations"),
            Section::Navigation => n_("Navigation"),
            Section::System => n_("System & view"),
        }
    }

    /// One line about the section (marked for translation).
    pub fn description(self) -> &'static str {
        match self {
            Section::Files => n_("Viewing, editing, copying, moving, deleting and selecting items"),
            Section::Navigation => n_("Moving between folders, drives and history"),
            Section::System => n_("Sorting, display, panels, tools and the application itself"),
        }
    }

    /// The actions in the section, in table order.
    pub fn actions(self) -> impl Iterator<Item = Action> {
        Action::ALL.into_iter().filter(move |a| a.section() == self)
    }
}

impl Action {
    /// The section the action is listed in.
    pub fn section(self) -> Section {
        use Action::*;
        match self {
            View | Edit | Copy | Move | Rename | MkDir | Delete | DeletePermanent | CopyPaths | SelectAll | UnselectAll | SelectCalcSize | CalcSelectionSize | ContextMenu
            | Properties | Find => Section::Files,
            ChangeDir | Parent | EnterDir | Root | HistoryBack | HistoryForward | DriveMenuLeft | DriveMenuRight | ToggleQuickSearchMode => Section::Navigation,
            SortName | SortExt | SortTime | SortSize | Refresh | ToggleHidden | ToggleSystem | ToggleTheme | SwapPanels | Terminal | Settings | Help | Exit => Section::System,
        }
    }

    /// Grouped by section (files, navigation, system).
    pub const ALL: [Action; 38] = [
        Action::View,
        Action::Edit,
        Action::Copy,
        Action::Move,
        Action::Rename,
        Action::MkDir,
        Action::Delete,
        Action::DeletePermanent,
        Action::CopyPaths,
        Action::SelectAll,
        Action::UnselectAll,
        Action::SelectCalcSize,
        Action::CalcSelectionSize,
        Action::ContextMenu,
        Action::Properties,
        Action::Find,
        Action::ChangeDir,
        Action::Parent,
        Action::EnterDir,
        Action::Root,
        Action::HistoryBack,
        Action::HistoryForward,
        Action::DriveMenuLeft,
        Action::DriveMenuRight,
        Action::ToggleQuickSearchMode,
        Action::SortName,
        Action::SortExt,
        Action::SortTime,
        Action::SortSize,
        Action::Refresh,
        Action::ToggleHidden,
        Action::ToggleSystem,
        Action::ToggleTheme,
        Action::SwapPanels,
        Action::Terminal,
        Action::Settings,
        Action::Help,
        Action::Exit,
    ];

    /// The action's id, label and default shortcuts.
    pub fn info(self) -> ActionInfo {
        use Action::*;
        match self {
            View => info("view", n_("View file"), &["F3"], &[]),
            Edit => info("edit", n_("Edit file"), &["F4"], &[]),
            Copy => info("copy", n_("Copy"), &["F5"], &[]),
            Move => info("move", n_("Move"), &["F6"], &[]),
            Rename => info("rename", n_("Rename"), &["F2", "Shift+F6"], &[]),
            MkDir => info("mkdir", n_("Create directory"), &["F7"], &[]),
            Delete => info("delete", n_("Delete"), &["F8", "Delete"], &[]),
            DeletePermanent => info("delete_permanent", n_("Delete permanently"), &["Shift+F8", "Shift+Delete"], &[]),
            ChangeDir => info("change_dir", n_("Change directory"), &["Shift+F7"], &["Ctrl+L"]),
            Parent => info("parent", n_("Parent directory"), &["Backspace", "Ctrl+PageUp"], &["Alt+Up"]),
            EnterDir => info("enter_dir", n_("Enter directory"), &["Ctrl+PageDown"], &[]),
            Root => info("root", n_("Root of current drive"), &["Ctrl+Backspace", "Ctrl+Backslash"], &[]),
            HistoryBack => info("history_back", n_("History back"), &["Alt+Left"], &[]),
            HistoryForward => info("history_forward", n_("History forward"), &["Alt+Right"], &[]),
            DriveMenuLeft => info("drive_menu_left", n_("Drive menu (left panel)"), &["Alt+F1"], &[]),
            DriveMenuRight => info("drive_menu_right", n_("Drive menu (right panel)"), &["Alt+F2"], &[]),
            SelectAll => info("select_all", n_("Select all"), &["Ctrl+A"], &[]),
            UnselectAll => info("unselect_all", n_("Unselect all"), &["Ctrl+D"], &[]),
            ContextMenu => info("context_menu", n_("Context menu"), &["Shift+F10"], &[]),
            ToggleQuickSearchMode => info("quick_search_mode", n_("Switch quick search mode (first letter / anywhere)"), &[], &["Ctrl+Shift+Q"]),
            Properties => info("properties", n_("Properties"), &["Alt+Enter"], &[]),
            CalcSelectionSize => info("calc_size", n_("Calculate selection size"), &["Alt+F10"], &["Ctrl+Q"]),
            SelectCalcSize => info("select_size", n_("Select + calculate folder size"), &["Shift+Space"], &["Space", "Ctrl+Space"]),
            Find => info("find", n_("Find files"), &["Alt+F7"], &["Ctrl+F"]),
            CopyPaths => info("copy_paths", n_("Copy full paths"), &["Ctrl+Shift+C"], &[]),
            SortName => info("sort_name", n_("Sort by name"), &["Ctrl+F3"], &[]),
            SortExt => info("sort_ext", n_("Sort by extension"), &["Ctrl+F4"], &[]),
            SortTime => info("sort_time", n_("Sort by time"), &["Ctrl+F5"], &[]),
            SortSize => info("sort_size", n_("Sort by size"), &["Ctrl+F6"], &[]),
            Refresh => info("refresh", n_("Refresh"), &["Ctrl+R"], &[]),
            ToggleHidden => info("toggle_hidden", n_("Show hidden files"), &["Ctrl+H"], &[]),
            ToggleSystem => info("toggle_system", n_("Show system files"), &[], &["Ctrl+Shift+H"]),
            ToggleTheme => info("toggle_theme", n_("Switch dark/light mode"), &[], &["Ctrl+Shift+D"]),
            SwapPanels => info("swap_panels", n_("Swap panels"), &["Ctrl+U"], &[]),
            Terminal => info("terminal", n_("Open command prompt here"), &["Shift+F1"], &["Ctrl+Shift+T"]),
            Settings => info("settings", n_("Settings"), &[], &["Ctrl+Comma"]),
            Help => info("help", n_("Keyboard help"), &["F1"], &[]),
            Exit => info("exit", n_("Exit"), &["F10"], &["Alt+X"]),
        }
    }
}

/// Canonical text of a shortcut (`ctrl+\` → `Ctrl+Backslash`); invalid text is kept as typed.
pub fn normalize(s: &str) -> String {
    Shortcut::parse(s).map_or_else(|| s.trim().to_owned(), |sc| sc.to_string())
}

/// Shortcuts in their written form, without empty ones and repeats.
fn normalize_all<'a>(list: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in list {
        let n = normalize(s);
        if !n.is_empty() && !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

/// The action's default shortcuts.
pub fn defaults(a: Action) -> Vec<String> {
    normalize_all(a.info().defaults.iter().copied())
}

/// The action's shortcuts: the user's, or the defaults.
pub fn bindings(ov: &Overrides, a: Action) -> Vec<String> {
    match ov.get(a.info().id) {
        Some(list) => normalize_all(list.iter().map(String::as_str)),
        None => defaults(a),
    }
}

/// Store `list` for `a`; only differences from the defaults are kept.
pub fn set_bindings(ov: &mut Overrides, a: Action, list: Vec<String>) {
    let list = normalize_all(list.iter().map(String::as_str));
    if list == defaults(a) {
        ov.remove(a.info().id);
    } else {
        ov.insert(a.info().id.to_owned(), list);
    }
}

/// Give the action another shortcut.
pub fn add_binding(ov: &mut Overrides, a: Action, s: &str) {
    let mut list = bindings(ov, a);
    list.push(s.to_owned());
    set_bindings(ov, a, list);
}

/// Take a shortcut away from the action.
pub fn remove_binding(ov: &mut Overrides, a: Action, s: &str) {
    let list = bindings(ov, a).into_iter().filter(|b| b != s).collect();
    set_bindings(ov, a, list);
}

/// Preset choices for the dropdown: default set, its single bindings,
/// alternatives, and none. The first entry is the default.
pub fn presets(a: Action) -> Vec<(String, Vec<String>)> {
    let info = a.info();
    let def = defaults(a);
    let mut v = vec![(if def.is_empty() { tr("Default (none)").to_owned() } else { trf("Default: {keys}", &[("keys", &def.join(", "))]) }, def.clone())];
    if def.len() > 1 {
        v.extend(def.iter().map(|d| (trf("{key} only", &[("key", d)]), vec![d.clone()])));
    }
    for alt in info.alternatives {
        let n = normalize(alt);
        v.push((n.clone(), vec![n.clone()]));
        if !def.is_empty() {
            let mut both = def.clone();
            both.push(n.clone());
            v.push((format!("{} + {n}", def.join(", ")), both));
        }
    }
    if !def.is_empty() {
        v.push((tr("None (disabled)").to_owned(), Vec::new()));
    }
    v
}

/// Label of the preset matching the current bindings, or "Custom".
pub fn preset_label(ov: &Overrides, a: Action) -> String {
    let cur = bindings(ov, a);
    presets(a).into_iter().find(|(_, set)| *set == cur).map_or_else(|| tr("Custom").to_owned(), |(l, _)| l)
}

/// The action a key press runs, if any.
pub fn lookup(ov: &Overrides, key: Key, m: Modifiers) -> Option<Action> {
    Action::ALL.into_iter().find(|&a| bindings(ov, a).iter().any(|b| Shortcut::parse(b).is_some_and(|s| s.matches(key, m))))
}

/// Shortcuts bound to more than one action.
pub fn conflicts(ov: &Overrides) -> HashMap<String, Vec<Action>> {
    let mut by_key: HashMap<String, Vec<Action>> = HashMap::new();
    for a in Action::ALL {
        for b in bindings(ov, a) {
            by_key.entry(b).or_default().push(a);
        }
    }
    by_key.retain(|_, v| v.len() > 1);
    by_key
}

/// Bindings of `ov` that a drive-letter modifier would shadow, e.g. `Ctrl+Shift+C`.
pub fn shadowed_by_drive_letters(ov: &Overrides, matches: impl Fn(bool, bool, bool) -> bool) -> Vec<(Action, String)> {
    let mut out = Vec::new();
    for a in Action::ALL {
        for b in bindings(ov, a) {
            if let Some(s) = Shortcut::parse(&b) {
                if crate::shortcut::key_letter(s.key).is_some() && matches(s.ctrl, s.shift, s.alt) {
                    out.push((a, b));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_unique_and_conflict_free() {
        let ov = Overrides::new();
        let mut ids = std::collections::HashSet::new();
        for a in Action::ALL {
            assert!(ids.insert(a.info().id), "duplicate id {}", a.info().id);
            for b in a.info().defaults.iter().chain(a.info().alternatives) {
                assert!(Shortcut::parse(b).is_some(), "{b} does not parse");
            }
        }
        assert!(conflicts(&ov).is_empty(), "{:?}", conflicts(&ov));
        assert_eq!(lookup(&ov, Key::F5, Modifiers::NONE), Some(Action::Copy));
        assert_eq!(lookup(&ov, Key::Backslash, Modifiers::CTRL), Some(Action::Root));
        assert_eq!(lookup(&ov, Key::F5, Modifiers::SHIFT), None);
    }

    #[test]
    fn sections_group_every_action() {
        // ALL is ordered by section, so menus and help read top-down
        let order: Vec<Section> = Action::ALL.iter().map(|a| a.section()).collect();
        assert!(order.windows(2).all(|w| w[0] <= w[1]), "{order:?}");
        let total: usize = Section::ALL.iter().map(|s| s.actions().count()).sum();
        assert_eq!(total, Action::ALL.len());
        assert!(Section::ALL.iter().all(|s| s.actions().count() > 0));
        assert_eq!(Action::Copy.section(), Section::Files);
        assert_eq!(Action::Root.section(), Section::Navigation);
        assert_eq!(Action::Exit.section(), Section::System);
    }

    #[test]
    fn overrides_store_only_differences() {
        let mut ov = Overrides::new();
        add_binding(&mut ov, Action::Copy, "ctrl+k");
        assert_eq!(bindings(&ov, Action::Copy), ["F5", "Ctrl+K"]);
        assert_eq!(preset_label(&ov, Action::Copy), "Custom");
        remove_binding(&mut ov, Action::Copy, "Ctrl+K");
        assert!(ov.is_empty(), "back to default leaves no override");
        assert!(preset_label(&ov, Action::Copy).starts_with("Default"));

        set_bindings(&mut ov, Action::Delete, vec!["Delete".into()]);
        assert_eq!(preset_label(&ov, Action::Delete), "Delete only");
        set_bindings(&mut ov, Action::Delete, Vec::new());
        assert_eq!(preset_label(&ov, Action::Delete), "None (disabled)");
        assert_eq!(lookup(&ov, Key::F8, Modifiers::NONE), None);
        assert_eq!(ov["delete"], Vec::<String>::new(), "disabled is stored explicitly");
    }

    #[test]
    fn presets_and_conflicts() {
        let p = presets(Action::Parent);
        assert_eq!(p[0].1, ["Backspace", "Ctrl+PageUp"]);
        assert!(p.iter().any(|(_, s)| s == &["Backspace", "Ctrl+PageUp", "Alt+Up"]));
        assert_eq!(presets(Action::Settings)[0].0, "Default (none)");

        let mut ov = Overrides::new();
        add_binding(&mut ov, Action::Refresh, "F5");
        let c = conflicts(&ov);
        assert_eq!(c["F5"], [Action::Copy, Action::Refresh]);
        let shadowed = shadowed_by_drive_letters(&Overrides::new(), |c, s, a| c && s && !a);
        assert_eq!(shadowed, [(Action::CopyPaths, "Ctrl+Shift+C".to_owned())]);
    }
}
