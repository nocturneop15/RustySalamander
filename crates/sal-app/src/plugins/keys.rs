//! Keyboard shortcuts of plugins' commands: the commands each plugin
//! offers keys for (its [`FolderPlugin::hotkeys`], and every command of its
//! Plugins menu), and the keys the user gave them in Settings → Plugins,
//! kept apart from the program's configuration in the plugin's own folder
//! ([`FILE`] in [`sal_plugin_api::data_dir`]: the plugin's folder, or its
//! folder under `%APPDATA%` in multi-user mode), in a table named by the
//! plugin's id (plugins side by side in one folder share the file). Like
//! the program's own shortcuts, only the keys that differ from the
//! plugin's are saved.
//!
//! [`FolderPlugin::hotkeys`]: sal_plugin_api::FolderPlugin::hotkeys

use std::fs;
use std::io;
use std::path::PathBuf;

use eframe::egui::{Key, Modifiers};
use sal_plugin_api::{Command, Hotkey};

use crate::keymap::{self, Overrides};

/// The file in a plugin's folder with the keys the user gave its commands.
pub const FILE: &str = "shortcuts.toml";

/// A command of a plugin that can have keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginKey {
    /// The command's id ([`sal_plugin_api::FolderPlugin::run`]).
    pub command: String,
    /// Its name in Settings, translated.
    pub label: String,
    /// The keys the plugin gives it.
    pub defaults: Vec<String>,
}

/// One plugin's shortcuts.
#[derive(Debug, Clone, Default)]
pub struct PluginKeys {
    /// The plugin's id.
    pub plugin: String,
    /// Its commands that can have keys, in Settings' order.
    pub list: Vec<PluginKey>,
    /// The user's keys, by command id: only those that differ from the
    /// plugin's (an empty list: no key at all). Keys of commands the plugin
    /// no longer has stay, unused, in case they come back.
    pub overrides: Overrides,
    /// The user's keys as read or last saved here: a save puts in the file
    /// only the commands changed since (another copy of the program may
    /// have saved other commands' keys meanwhile).
    pub base: Overrides,
    /// Where they are saved; empty for nowhere (a plugin not from a DLL).
    pub file: PathBuf,
}

/// [`FILE`] as it is saved: the keys of each plugin by its id, in a table
/// of its own (plugins side by side in one folder share the file, each
/// changing only its own table).
type Saved = std::collections::BTreeMap<String, Overrides>;

/// The plugins' keys saved in `file`; none when it is missing or can't be read.
fn read(file: &std::path::Path) -> Saved {
    fs::read_to_string(file).ok().and_then(|t| toml::from_str::<Saved>(&t).ok()).unwrap_or_default()
}

/// The plugins' keys in `file` before changing it: none when there is no
/// file; an error when it can't be read (nothing is written over it then).
/// A damaged file (not understood) is kept beside as `<file>.damaged`, and
/// a new one started.
fn read_to_change(file: &std::path::Path) -> io::Result<Saved> {
    let text = match fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Saved::new()),
        Err(e) => return Err(e),
    };
    match toml::from_str::<Saved>(&text) {
        Ok(s) => Ok(s),
        Err(_) => {
            sal_plugin_api::settings::keep_damaged(file)?;
            Ok(Saved::new())
        }
    }
}

/// The commands that can have keys: the plugin's shortcuts first, in its
/// order (labelled like the menu command of the same id when they have no
/// label of their own), then the menu's other commands, without keys.
pub fn offered(commands: &[Command], hotkeys: &[Hotkey]) -> Vec<PluginKey> {
    let mut out: Vec<PluginKey> = Vec::new();
    for h in hotkeys {
        if out.iter().any(|k| k.command == h.command) {
            continue;
        }
        let label = match commands.iter().find(|c| c.id == h.command) {
            _ if !h.label.is_empty() => h.label.clone(),
            Some(c) => c.label.clone(),
            None => h.command.clone(),
        };
        out.push(PluginKey { command: h.command.clone(), label, defaults: h.keys.clone() });
    }
    for c in commands {
        if !out.iter().any(|k| k.command == c.id) {
            out.push(PluginKey { command: c.id.clone(), label: c.label.clone(), defaults: Vec::new() });
        }
    }
    out
}

impl PluginKeys {
    /// The shortcuts of `plugin` offering `list`, with the user's keys read
    /// from `file` (none when it is missing or can't be read).
    pub fn load(plugin: &str, file: PathBuf, list: Vec<PluginKey>) -> PluginKeys {
        let overrides = read(&file).remove(plugin).unwrap_or_default();
        PluginKeys { plugin: plugin.to_owned(), list, base: overrides.clone(), overrides, file }
    }

    /// Save the keys of the commands changed since the last save (or since
    /// they were read) into the plugin's table of the file as it is now:
    /// other commands' keys (another copy of the program may have saved
    /// them meanwhile) and other plugins' tables are kept. With no keys
    /// left in it, the file goes away. A file that can't be read isn't
    /// written over: an error.
    pub fn save(&mut self) -> io::Result<()> {
        if self.file.as_os_str().is_empty() {
            return Ok(());
        }
        // read, changed and written (or removed) with no other copy of the
        // program doing the same meanwhile
        let (base, mine) = (&self.base, &self.overrides);
        let result = sal_plugin_api::settings::locked(&self.file, || {
            let mut saved = read_to_change(&self.file)?;
            let table = saved.entry(self.plugin.clone()).or_default();
            let changed: std::collections::BTreeSet<&String> = base.keys().chain(mine.keys()).filter(|c| base.get(*c) != mine.get(*c)).collect();
            for c in changed {
                match mine.get(c) {
                    Some(keys) => table.insert(c.clone(), keys.clone()),
                    None => table.remove(c),
                };
            }
            if table.is_empty() {
                saved.remove(&self.plugin);
            }
            if saved.is_empty() {
                return match fs::remove_file(&self.file) {
                    Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                    _ => Ok(()),
                };
            }
            let text = toml::to_string_pretty(&saved).map_err(io::Error::other)?;
            sal_plugin_api::settings::replace(&self.file, &format!("# Keys of plugins' commands, set in RustySalamander's Settings → Plugins.\n\n{text}"))
        });
        if result.is_ok() {
            self.base = self.overrides.clone();
        }
        result
    }

    /// The keys the plugin gives `command`.
    fn defaults(&self, command: &str) -> &[String] {
        self.list.iter().find(|k| k.command == command).map_or(&[], |k| &k.defaults[..])
    }

    /// The keys of `command`: the user's, or the plugin's.
    pub fn bindings(&self, command: &str) -> Vec<String> {
        keymap::bindings_of(&self.overrides, command, self.defaults(command))
    }

    /// Give `command` exactly these keys.
    pub fn set(&mut self, command: &str, list: Vec<String>) {
        let defaults = self.defaults(command).to_vec();
        keymap::set_bindings_of(&mut self.overrides, command, &defaults, list);
    }

    /// Give `command` one more key.
    pub fn add(&mut self, command: &str, key: &str) {
        let mut list = self.bindings(command);
        list.push(key.to_owned());
        self.set(command, list);
    }

    /// Take a key away from `command`.
    pub fn remove(&mut self, command: &str, key: &str) {
        let list = self.bindings(command).into_iter().filter(|b| b != key).collect();
        self.set(command, list);
    }

    /// The plugin's own keys again, for every command.
    pub fn reset(&mut self) {
        self.overrides.clear();
    }

    /// Whether every command has the plugin's own keys.
    pub fn is_default(&self) -> bool {
        self.list.iter().all(|k| !self.overrides.contains_key(&k.command))
    }

    /// The command a key press runs, if any.
    pub fn lookup(&self, key: Key, m: Modifiers) -> Option<&str> {
        self.list.iter().find(|k| keymap::pressed(&self.bindings(&k.command), key, m)).map(|k| k.command.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(id: &str, label: &str) -> Command {
        Command { id: id.into(), label: label.into() }
    }

    #[test]
    fn offered_commands() {
        let commands = [command("compare", "Compare"), command("files_form", "Compare files…")];
        let hotkeys = [
            Hotkey { command: "files_form".into(), label: String::new(), keys: vec!["ctrl+shift+k".into()] },
            Hotkey { command: "hidden".into(), label: "Not in the menu".into(), keys: Vec::new() },
            Hotkey { command: "files_form".into(), label: "Twice".into(), keys: Vec::new() },
            Hotkey { command: "bare".into(), label: String::new(), keys: Vec::new() },
        ];
        let list = offered(&commands, &hotkeys);
        let labels: Vec<(&str, &str)> = list.iter().map(|k| (k.command.as_str(), k.label.as_str())).collect();
        assert_eq!(labels, [("files_form", "Compare files…"), ("hidden", "Not in the menu"), ("bare", "bare"), ("compare", "Compare")]);
        assert_eq!(list[0].defaults, ["ctrl+shift+k"]);
        assert!(list[3].defaults.is_empty(), "menu commands have no key of their own");
    }

    #[test]
    fn keys_are_changed_saved_and_read_again() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("plugin 🦎").join(FILE);
        let list = || offered(&[command("compare", "Compare"), command("a:b,", "Strange id")], &[Hotkey { command: "compare".into(), label: String::new(), keys: vec!["Ctrl+Shift+K".into()] }]);
        let mut k = PluginKeys::load("compare", file.clone(), list());
        assert!(k.is_default() && k.overrides.is_empty());
        assert_eq!(k.bindings("compare"), ["Ctrl+Shift+K"], "the plugin's key");
        assert_eq!(k.lookup(Key::K, Modifiers::CTRL | Modifiers::SHIFT), Some("compare"));
        assert_eq!(k.lookup(Key::K, Modifiers::CTRL), None);

        k.add("a:b,", "alt+f9");
        k.remove("compare", "Ctrl+Shift+K");
        assert_eq!(k.bindings("a:b,"), ["Alt+F9"], "written as Settings writes it");
        assert!(k.bindings("compare").is_empty(), "no key at all is kept too");
        assert_eq!(k.lookup(Key::F9, Modifiers::ALT), Some("a:b,"));
        assert_eq!(k.lookup(Key::K, Modifiers::CTRL | Modifiers::SHIFT), None);
        k.save().unwrap();

        let again = PluginKeys::load("compare", file.clone(), list());
        assert_eq!(again.overrides, k.overrides);
        assert!(!again.is_default());
        // back to the plugin's keys: nothing to keep, the file goes
        k.set("compare", vec!["Ctrl+Shift+K".into()]);
        k.set("a:b,", Vec::new());
        assert!(k.is_default());
        k.save().unwrap();
        assert!(!file.exists());
        k.save().unwrap();
        // damaged: no keys of the user's
        fs::write(&file, "not [ toml").unwrap();
        assert!(PluginKeys::load("compare", file, list()).overrides.is_empty());
        // nowhere to save: nothing saved, no error
        PluginKeys { plugin: "x".into(), overrides: [("a".to_owned(), vec![])].into(), ..Default::default() }.save().unwrap();
    }

    /// Two copies of the program changing keys of different commands of the
    /// same plugin: both kept, a reset in one takes back only what it knew.
    /// A damaged file is kept beside.
    #[test]
    fn copies_keep_each_others_commands() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join(FILE);
        let list = || offered(&[command("run", "Run"), command("stop", "Stop")], &[]);
        // both started before either saved
        let (mut one, mut two) = (PluginKeys::load("p", file.clone(), list()), PluginKeys::load("p", file.clone(), list()));
        one.add("run", "Ctrl+F7");
        one.save().unwrap();
        two.add("stop", "Ctrl+F8");
        two.save().unwrap();
        let again = PluginKeys::load("p", file.clone(), list());
        assert_eq!((again.bindings("run"), again.bindings("stop")), (vec!["Ctrl+F7".to_owned()], vec!["Ctrl+F8".to_owned()]), "{}", fs::read_to_string(&file).unwrap());
        // the first resets: its own change goes, the other's stays
        one.reset();
        one.save().unwrap();
        let again = PluginKeys::load("p", file.clone(), list());
        assert!(again.bindings("run").is_empty());
        assert_eq!(again.bindings("stop"), ["Ctrl+F8"]);
        // damaged: kept beside, the keys saved in a new file
        fs::write(&file, "not [ toml").unwrap();
        one.add("run", "Ctrl+F9");
        one.save().unwrap();
        assert_eq!(fs::read_to_string(t.path().join("shortcuts.toml.damaged")).unwrap(), "not [ toml");
        assert_eq!(PluginKeys::load("p", file.clone(), list()).bindings("run"), ["Ctrl+F9"]);
    }

    /// Two plugins in one folder share the file, each in a table of its
    /// own: keys of the same command id stay apart, a reset is of one.
    #[test]
    fn plugins_in_one_folder_keep_their_own_keys() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join(FILE);
        let list = || offered(&[command("run", "Run")], &[]);
        let (mut a, mut b) = (PluginKeys::load("alpha", file.clone(), list()), PluginKeys::load("beta", file.clone(), list()));
        a.add("run", "Ctrl+F7");
        a.save().unwrap();
        b.add("run", "Ctrl+F8");
        b.save().unwrap();
        // started again
        assert_eq!(PluginKeys::load("alpha", file.clone(), list()).bindings("run"), ["Ctrl+F7"]);
        assert_eq!(PluginKeys::load("beta", file.clone(), list()).bindings("run"), ["Ctrl+F8"]);
        a.reset();
        a.save().unwrap();
        assert!(PluginKeys::load("alpha", file.clone(), list()).bindings("run").is_empty(), "alpha reset");
        assert_eq!(PluginKeys::load("beta", file.clone(), list()).bindings("run"), ["Ctrl+F8"], "beta's kept");
        b.reset();
        b.save().unwrap();
        assert!(!file.exists(), "no keys left: the file goes");
        // two copies of the program saving the two plugins' keys at the same
        // time, over and over: both kept
        let threads: Vec<_> = [("alpha", "Ctrl+F7"), ("beta", "Ctrl+F8")]
            .into_iter()
            .map(|(id, key)| {
                let (file, list) = (file.clone(), list());
                std::thread::spawn(move || {
                    for _ in 0..20 {
                        let mut k = PluginKeys::load(id, file.clone(), list.clone());
                        k.set("run", vec![key.to_owned()]);
                        k.save().unwrap();
                    }
                })
            })
            .collect();
        threads.into_iter().for_each(|h| h.join().unwrap());
        assert_eq!(PluginKeys::load("alpha", file.clone(), list()).bindings("run"), ["Ctrl+F7"]);
        assert_eq!(PluginKeys::load("beta", file.clone(), list()).bindings("run"), ["Ctrl+F8"]);
    }
}
