//! What a plugin remembers between sessions (the choices in its windows'
//! toolbars, say): a small file of `name = value` lines in the plugin's
//! data folder ([`crate::data_dir`]), saved as soon as something changes.
//!
//! ```no_run
//! # use sal_plugin_api::settings::Settings;
//! # let dir = std::path::Path::new("plugins/viewer");
//! let settings = Settings::load(dir); // in Plugin::start, with its folder
//! let wrap = settings.bool("wrap", false);
//! settings.set_bool("wrap", true); // saved at once
//! ```
//!
//! The file is TOML (`wrap = true`, `name = "text"`), so people can read
//! and edit it; lines it doesn't understand are left out. A copy of
//! [`Settings`] shares the values with the one it was copied from (windows
//! on other threads see what one of them changed).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The file's name in the plugin's data folder.
pub const FILE: &str = "settings.toml";

/// A plugin's remembered values, saved in [`FILE`] when one changes.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// Where they are saved; empty for nowhere (in tests).
    file: PathBuf,
    values: Arc<Mutex<BTreeMap<String, String>>>,
    /// The names of values whose save failed: saved with the next one
    /// (also when it is the same value again). Locked only while `values` is.
    unsaved: Arc<Mutex<std::collections::BTreeSet<String>>>,
}

impl Settings {
    /// The settings of the plugin in the folder `plugin_dir` (what
    /// [`crate::Plugin::start`] gets), read from its data folder; none
    /// saved yet (or a file that can't be read): every value its default.
    pub fn load(plugin_dir: &Path) -> Settings {
        Settings::at(crate::data_dir(plugin_dir).join(FILE))
    }

    /// The settings saved in `file`.
    pub fn at(file: PathBuf) -> Settings {
        let values = fs::read_to_string(&file).map(|t| parse(&t)).unwrap_or_default();
        Settings { file, values: Arc::new(Mutex::new(values)), unsaved: Default::default() }
    }

    /// Settings that are never saved (for tests, or when there is no folder).
    pub fn in_memory() -> Settings {
        Settings::default()
    }

    /// The text saved as `name`, or `default`.
    pub fn get(&self, name: &str, default: &str) -> String {
        self.values.lock().unwrap().get(name).cloned().unwrap_or_else(|| default.to_owned())
    }

    /// The yes/no saved as `name`, or `default`.
    pub fn bool(&self, name: &str, default: bool) -> bool {
        match self.values.lock().unwrap().get(name).map(String::as_str) {
            Some("true") => true,
            Some("false") => false,
            _ => default,
        }
    }

    /// Remember `value` as `name` and save, when it is something new. An
    /// error (a folder that can't be written to) says why; the value is
    /// remembered for this session anyway.
    ///
    /// Only this value is changed in the file (with any whose save failed
    /// before: those are tried again now, also when `value` is what it was):
    /// what another copy of the program saved there meanwhile (other values)
    /// is kept, and taken in. Saved while the values are held, so two
    /// windows saving at once save one after the other, the later last.
    pub fn set(&self, name: &str, value: &str) -> io::Result<()> {
        let mut v = self.values.lock().unwrap();
        let mut unsaved = self.unsaved.lock().unwrap();
        if v.get(name).map(String::as_str) == Some(value) && !unsaved.contains(name) {
            return Ok(());
        }
        v.insert(name.to_owned(), value.to_owned());
        if self.file.as_os_str().is_empty() {
            return Ok(());
        }
        // saved below, or still to be
        unsaved.insert(name.to_owned());
        let to_save: Vec<(String, String)> = unsaved.iter().filter_map(|n| v.get(n).map(|x| (n.clone(), x.clone()))).collect();
        // the file as it is now (another copy may have saved since), this value
        // changed; no other copy reads or writes it meanwhile. A file that
        // can't be read isn't written over: an error (the value is kept here)
        let saved = locked(&self.file, || {
            let mut saved = match fs::read_to_string(&self.file) {
                Ok(text) => {
                    let (values, whole) = parse_checked(&text);
                    // lines not understood (written by hand?) would be lost: the
                    // file is kept beside, what was understood goes on
                    if !whole {
                        keep_damaged(&self.file)?;
                    }
                    values
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => v.clone(),
                Err(e) => return Err(e),
            };
            saved.extend(to_save);
            replace(&self.file, &format(&saved))?;
            Ok(saved)
        })?;
        unsaved.clear();
        *v = saved;
        Ok(())
    }

    /// Remember a yes/no as `name` and save, when it is something new.
    pub fn set_bool(&self, name: &str, value: bool) -> io::Result<()> {
        self.set(name, if value { "true" } else { "false" })
    }
}

/// Run `f` (reading, changing and writing `file`) holding the lock of
/// `file`: the file `<file>.lock` beside it, locked for this program alone,
/// so another copy of the program changing the same file waits until `f`
/// is done (and reads what it wrote).
pub fn locked<R>(file: &Path, f: impl FnOnce() -> io::Result<R>) -> io::Result<R> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut name = file.file_name().unwrap_or_default().to_owned();
    name.push(".lock");
    let lock = fs::OpenOptions::new().create(true).truncate(false).write(true).open(file.with_file_name(name))?;
    lock.lock()?;
    let result = f();
    let _ = lock.unlock();
    result
}

/// Keep the damaged `file` (one that can't be understood) beside it as
/// `<file>.damaged` (replacing an older one), before a new one is written
/// in its place: nothing in it is lost.
pub fn keep_damaged(file: &Path) -> io::Result<()> {
    let mut name = file.file_name().unwrap_or_default().to_owned();
    name.push(".damaged");
    let kept = file.with_file_name(name);
    let _ = fs::remove_file(&kept);
    fs::rename(file, kept)
}

/// Write `text` as `file`, whole or not at all: written beside it under a
/// name of its own (no other save, here or in another copy of the program,
/// writes the same one), then put in its place.
pub fn replace(file: &Path, text: &str) -> io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut name = file.file_name().unwrap_or_default().to_owned();
    name.push(format!(".{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let tmp = file.with_file_name(name);
    fs::write(&tmp, text)?;
    fs::rename(&tmp, file).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// The values of the file's `name = value` lines; a value is `true`,
/// `false`, or text in double quotes.
fn parse(text: &str) -> BTreeMap<String, String> {
    parse_checked(text).0
}

/// [`parse`], and whether every line was understood (else a save would
/// leave some out).
fn parse_checked(text: &str) -> (BTreeMap<String, String>, bool) {
    let mut out = BTreeMap::new();
    let mut whole = true;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let value = line.split_once('=').and_then(|(name, value)| {
            let (name, value) = (name.trim(), value.trim());
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                return None;
            }
            let value = match value {
                "true" | "false" => value.to_owned(),
                _ => unquote(value)?,
            };
            Some((name.to_owned(), value))
        });
        match value {
            Some((name, value)) => {
                out.insert(name, value);
            }
            None => whole = false,
        }
    }
    (out, whole)
}

/// The text of a TOML basic string (`"a \"b\""`), if `s` is one.
fn unquote(s: &str) -> Option<String> {
    let inner = s.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next()? {
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                _ => return None,
            },
            '"' => return None,
            c => out.push(c),
        }
    }
    Some(out)
}

/// The file's text for `values`.
fn format(values: &BTreeMap<String, String>) -> String {
    let mut s = String::from("# Remembered by the plugin between sessions.\n");
    for (name, value) in values {
        if value == "true" || value == "false" {
            s.push_str(&format!("{name} = {value}\n"));
        } else {
            let quoted = value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t");
            s.push_str(&format!("{name} = \"{quoted}\"\n"));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_at_once_and_read_again() {
        let t = std::env::temp_dir().join(format!("sal-settings-{}", std::process::id()));
        let file = t.join("plugin 🦎").join(FILE);
        let s = Settings::at(file.clone());
        assert!(s.bool("numbers", true) && !s.bool("wrap", false), "defaults before anything is saved");
        s.set_bool("numbers", false).unwrap();
        s.set("name", "a \"b\" \\ c\nřádek 🦎").unwrap();
        let shared = s.clone();
        shared.set_bool("wrap", true).unwrap();
        assert!(s.bool("wrap", false), "copies share the values");
        let again = Settings::at(file.clone());
        assert!(!again.bool("numbers", true) && again.bool("wrap", false));
        assert_eq!(again.get("name", ""), "a \"b\" \\ c\nřádek 🦎");
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("numbers = false\n") && text.contains("wrap = true\n"), "{text}");
        let left: Vec<_> = fs::read_dir(file.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert!(!left.iter().any(|n| n.to_string_lossy().ends_with(".tmp")), "no temporary file left: {left:?}");
        // the same again writes nothing
        fs::remove_file(&file).unwrap();
        s.set_bool("numbers", false).unwrap();
        assert!(!file.exists());
        // nowhere to save: remembered all the same
        let m = Settings::in_memory();
        m.set_bool("x", true).unwrap();
        assert!(m.bool("x", false));
        fs::remove_dir_all(&t).unwrap();
    }

    /// Two copies of the program (each its own values) changing different
    /// values: both kept. Windows of one saving at once: every value kept.
    #[test]
    fn saves_keep_each_others_values() {
        let t = std::env::temp_dir().join(format!("sal-settings-two-{}", std::process::id()));
        let file = t.join(FILE);
        let (one, two) = (Settings::at(file.clone()), Settings::at(file.clone()));
        one.set_bool("numbers", false).unwrap();
        two.set_bool("wrap", true).unwrap();
        let again = Settings::at(file.clone());
        assert!(!again.bool("numbers", true) && again.bool("wrap", false), "{}", fs::read_to_string(&file).unwrap());
        assert!(!two.bool("numbers", true), "the other's value taken in");
        // eight threads at once, a value each
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let s = one.clone();
                std::thread::spawn(move || s.set_bool(&format!("t{i}"), true).unwrap())
            })
            .collect();
        threads.into_iter().for_each(|h| h.join().unwrap());
        let again = Settings::at(file.clone());
        assert!((0..8).all(|i| again.bool(&format!("t{i}"), false)), "{}", fs::read_to_string(&file).unwrap());
        // two copies of the program (values of their own each) saving at the
        // same time, over and over: every value of both kept
        let copies: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|who| {
                let s = Settings::at(file.clone());
                std::thread::spawn(move || (0..20).for_each(|i| s.set_bool(&format!("{who}{i}"), true).unwrap()))
            })
            .collect();
        copies.into_iter().for_each(|h| h.join().unwrap());
        let again = Settings::at(file.clone());
        let missing: Vec<String> = ["a", "b"].iter().flat_map(|w| (0..20).map(move |i| format!("{w}{i}"))).filter(|k| !again.bool(k, false)).collect();
        assert!(missing.is_empty(), "lost: {missing:?}");
        fs::remove_dir_all(&t).unwrap();
    }

    /// A file that can't be read isn't written over; one with lines not
    /// understood is kept beside before the new one is written.
    #[test]
    fn unreadable_or_damaged_files_are_kept() {
        let t = std::env::temp_dir().join(format!("sal-settings-bad-{}", std::process::id()));
        // a folder where the file should be: can't be read, an error
        let file = t.join("x").join(FILE);
        fs::create_dir_all(&file).unwrap();
        let s = Settings::at(file.clone());
        assert!(s.set_bool("wrap", true).is_err());
        assert!(file.is_dir(), "not written over");
        assert!(s.bool("wrap", false), "remembered for the session anyway");
        // not saved: set again (the same value), tried again, an error again;
        // once it can be, saved, with every value not saved before
        assert!(s.set_bool("wrap", true).is_err(), "not taken as saved");
        assert!(s.set_bool("numbers", false).is_err());
        fs::remove_dir(&file).unwrap();
        s.set_bool("wrap", true).unwrap();
        let again = Settings::at(file.clone());
        assert!(again.bool("wrap", false) && !again.bool("numbers", true), "{}", fs::read_to_string(&file).unwrap());
        s.set_bool("wrap", true).unwrap();
        // lines not understood: the file kept as .damaged, the rest carried on
        let file = t.join("y").join(FILE);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "numbers = false\nsize = 12\n").unwrap();
        let s = Settings::at(file.clone());
        s.set_bool("wrap", true).unwrap();
        let damaged = fs::read_to_string(file.with_file_name("settings.toml.damaged")).unwrap();
        assert_eq!(damaged, "numbers = false\nsize = 12\n");
        let again = Settings::at(file.clone());
        assert!(!again.bool("numbers", true) && again.bool("wrap", false));
        fs::remove_dir_all(&t).unwrap();
    }

    #[test]
    fn what_the_file_may_hold() {
        let v = parse("# comment\n\na = true\nb=false\n c = \"text = here\" \nbad line\n\"q\" = true\nd = 12\ne = \"open\n[table]\n");
        let pairs: Vec<(&str, &str)> = v.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(pairs, [("a", "true"), ("b", "false"), ("c", "text = here")], "what isn't understood is left out");
        assert_eq!(Settings::at(PathBuf::from(r"Z:\nowhere\settings.toml")).get("a", "x"), "x");
    }
}
