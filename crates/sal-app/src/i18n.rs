//! Translations, gettext style: the English text is the key.
//!
//! A language file (`lang/<code>.toml` next to the program) maps English
//! strings to translated ones; anything missing falls back to English, so
//! partial translations work. English is built in (the texts are the keys);
//! `en.toml` is the complete list of UI strings, the template for others.
//! The build copies `crates/sal-app/lang` next to the executable. Plugins
//! bring their own: `plugins\<name>\lang\<code>.toml` ([`tr_from`]).
//!
//! ```toml
//! [language]
//! code = "cs"
//! name = "Čeština"
//! plural = "cs"            # plural rule: "en" (1 / other) or "cs" (1 / 2–4 / other)
//!
//! [strings]
//! "Files" = "Soubory"
//! "Copy {what} to:" = "Kopírovat {what} do:"
//! "{n} file|{n} files" = "{n} soubor|{n} soubory|{n} souborů"
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
#[cfg(not(test))]
use std::sync::RwLock;

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluralRule {
    /// one / other
    English,
    /// one / few (2–4) / other — Czech, Slovak
    Czech,
}

impl PluralRule {
    fn index(self, n: u64) -> usize {
        match self {
            PluralRule::English => usize::from(n != 1),
            PluralRule::Czech => match n {
                1 => 0,
                2..=4 => 1,
                _ => 2,
            },
        }
    }
}

#[derive(Deserialize)]
struct File {
    language: Meta,
    #[serde(default)]
    strings: HashMap<String, String>,
}

#[derive(Deserialize)]
struct Meta {
    code: String,
    name: String,
    #[serde(default)]
    plural: String,
}

/// A loaded language.
pub struct Catalog {
    pub code: String,
    pub name: String,
    plural: PluralRule,
    /// Translations, leaked once so `tr` can hand out `&'static str`.
    strings: HashMap<&'static str, &'static str>,
}

impl Catalog {
    pub fn parse(text: &str) -> Result<Catalog, String> {
        let f: File = toml::from_str(text).map_err(|e| e.to_string())?;
        let plural = match f.language.plural.as_str() {
            "cs" | "sk" => PluralRule::Czech,
            _ => PluralRule::English,
        };
        let strings = f.strings.into_iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| (&*Box::leak(k.into_boxed_str()), &*Box::leak(v.into_boxed_str()))).collect();
        Ok(Catalog { code: f.language.code, name: f.language.name, plural, strings })
    }

    pub fn tr(&self, msg: &'static str) -> &'static str {
        self.strings.get(msg).copied().unwrap_or(msg)
    }

    /// Plural forms "one|other" (English) -> the translated form for `n`.
    pub fn trn(&self, one: &'static str, other: &'static str, n: u64) -> String {
        let key = format!("{one}|{other}");
        let form = match self.strings.get(key.as_str()) {
            Some(t) => {
                let forms: Vec<&str> = t.split('|').collect();
                forms[self.plural.index(n).min(forms.len() - 1)]
            }
            None => {
                if n == 1 {
                    one
                } else {
                    other
                }
            }
        };
        form.replace("{n}", &sal_core::encoding::format_thousands(n))
    }

    #[cfg(test)]
    pub fn keys(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.strings.keys().copied()
    }
}

/// A language that can be chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LangInfo {
    pub code: String,
    pub name: String,
}

// The language in use. Tests keep it per thread, so they can run in parallel.
#[cfg(not(test))]
static CURRENT: RwLock<Option<Catalog>> = RwLock::new(None);
#[cfg(test)]
thread_local! {
    static CURRENT: std::cell::RefCell<Option<Catalog>> = const { std::cell::RefCell::new(None) };
}

fn with_current<R>(f: impl FnOnce(Option<&Catalog>) -> R) -> R {
    #[cfg(not(test))]
    return f(CURRENT.read().unwrap().as_ref());
    #[cfg(test)]
    return CURRENT.with(|c| f(c.borrow().as_ref()));
}

fn set_current(cat: Option<Catalog>) {
    #[cfg(not(test))]
    {
        *CURRENT.write().unwrap() = cat;
    }
    #[cfg(test)]
    CURRENT.with(|c| *c.borrow_mut() = cat);
}

/// Marks a string for translation where it is defined; `tr` translates it
/// where it is shown.
pub const fn n_(s: &'static str) -> &'static str {
    s
}

/// Texts defined in sal-core (drive types, terminals, …), translated where
/// shown; listed here so the language files include them.
#[cfg(test)]
const CORE_STRINGS: &[&str] = &[
    "Disabled",
    "Shift + letter",
    "Alt + letter",
    "Ctrl+Shift + letter",
    "Local disk",
    "Removable drive",
    "Network drive",
    "CD/DVD drive",
    "RAM disk",
    "Drive",
    "Default terminal",
    "Command Prompt (cmd)",
    "PowerShell",
    "Custom command",
];

/// The `lang` folder next to the program (in tests: the source folder, as the
/// test executable lives elsewhere).
fn lang_dir() -> Option<PathBuf> {
    if cfg!(test) {
        return Some(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/lang")));
    }
    sal_core::config::app_dir().map(|d| d.join("lang"))
}

/// Texts of the language files in `dir`.
fn sources(dir: Option<&std::path::Path>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(Ok(rd)) = dir.map(std::fs::read_dir) {
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("toml"))).collect();
        files.sort();
        out.extend(files.iter().filter_map(|p| std::fs::read_to_string(p).ok()));
    }
    out
}

/// English without a file: the texts in the code are English already.
fn english() -> Catalog {
    Catalog { code: "en".into(), name: "English".into(), plural: PluralRule::English, strings: HashMap::new() }
}

fn available_in(dir: Option<&std::path::Path>) -> Vec<LangInfo> {
    let mut list = vec![LangInfo { code: "en".into(), name: "English".into() }];
    for text in sources(dir) {
        if let Ok(c) = Catalog::parse(&text) {
            list.retain(|l| l.code != c.code);
            list.push(LangInfo { code: c.code, name: c.name });
        }
    }
    list.sort_by(|a, b| (a.code != "en").cmp(&(b.code != "en")).then(a.name.cmp(&b.name)));
    list
}

fn load_in(dir: Option<&std::path::Path>, code: &str) -> Option<Catalog> {
    let found = sources(dir).iter().filter_map(|t| Catalog::parse(t).ok()).find(|c| c.code.eq_ignore_ascii_case(code));
    found.or_else(|| code.eq_ignore_ascii_case("en").then(english))
}

/// Languages available, English first, then by name.
pub fn available() -> Vec<LangInfo> {
    available_in(lang_dir().as_deref())
}

/// Load a language by code.
pub fn load(code: &str) -> Option<Catalog> {
    load_in(lang_dir().as_deref(), code)
}

/// Windows display language as a short code ("cs", "en", …).
pub fn system_language() -> String {
    #[cfg(windows)]
    {
        use windows::Win32::Globalization::GetUserDefaultLocaleName;
        let mut buf = [0u16; 85];
        let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
        if n > 1 {
            let tag = String::from_utf16_lossy(&buf[..n as usize - 1]);
            return tag.split('-').next().unwrap_or("en").to_lowercase();
        }
    }
    "en".into()
}

/// Switch the UI language: `""` = the Windows language if available, else English.
pub fn set_language(code: &str) {
    let wanted = if code.is_empty() { system_language() } else { code.to_owned() };
    let cat = load(&wanted).or_else(|| load("en"));
    set_current(cat);
}

/// Translate a fixed string.
pub fn tr(msg: &'static str) -> &'static str {
    with_current(|c| c.map_or(msg, |c| c.tr(msg)))
}

/// Translate a plugin's text from the language files in its `lang` folder
/// `dir`, into the language in use; English when there is no translation.
pub fn tr_from(dir: &Path, msg: &str) -> String {
    type Cache = std::collections::BTreeMap<(PathBuf, String), Option<std::sync::Arc<Catalog>>>;
    static CACHE: std::sync::Mutex<Cache> = std::sync::Mutex::new(Cache::new());
    let Some(code) = with_current(|c| c.map(|c| c.code.clone())) else { return msg.to_owned() };
    let cat = CACHE.lock().unwrap().entry((dir.to_path_buf(), code.clone())).or_insert_with(|| load_in(Some(dir), &code).map(std::sync::Arc::new)).clone();
    cat.and_then(|c| c.strings.get(msg).copied()).unwrap_or(msg).to_owned()
}

/// Translate a string with `{name}` placeholders and fill them in.
pub fn trf(msg: &'static str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut s = tr(msg).to_owned();
    for (name, value) in args {
        s = s.replace(&format!("{{{name}}}"), &value.to_string());
    }
    s
}

/// Translate a count: `trn("{n} file", "{n} files", 3)`.
pub fn trn(one: &'static str, other: &'static str, n: u64) -> String {
    with_current(|c| match c {
        Some(c) => c.trn(one, other, n),
        None => (if n == 1 { one } else { other }).replace("{n}", &sal_core::encoding::format_thousands(n)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plural_rules() {
        assert_eq!([0, 1, 2, 5].map(|n| PluralRule::English.index(n)), [1, 0, 1, 1]);
        assert_eq!([0, 1, 2, 4, 5, 22].map(|n| PluralRule::Czech.index(n)), [2, 0, 1, 1, 2, 2]);
    }

    fn source_lang_dir() -> PathBuf {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/lang"))
    }

    #[test]
    fn language_files() {
        let dir = source_lang_dir();
        let langs = available_in(Some(&dir));
        assert_eq!(langs[0].code, "en");
        assert!(langs.iter().any(|l| l.code == "cs" && l.name == "Čeština"));
        // without any files English is still there
        assert_eq!(available_in(None), [LangInfo { code: "en".into(), name: "English".into() }]);
        assert_eq!(load_in(None, "en").unwrap().tr("Files"), "Files");
        assert!(load_in(None, "cs").is_none());
        let cs = load_in(Some(&dir), "cs").expect("czech");
        assert_eq!(cs.tr("Files"), "Soubory");
        assert_eq!(cs.tr("No such string anywhere"), "No such string anywhere", "falls back to English");
        assert_eq!(cs.trn("{n} file", "{n} files", 1), "1 soubor");
        assert_eq!(cs.trn("{n} file", "{n} files", 3), "3 soubory");
        assert_eq!(cs.trn("{n} file", "{n} files", 12345), "12 345 souborů");
        let en = load_in(Some(&dir), "en").unwrap();
        assert_eq!(en.trn("{n} file", "{n} files", 1), "1 file");
        assert_eq!(en.trn("{n} file", "{n} files", 2), "2 files");
    }

    /// Labels that sal-core hands out are listed in CORE_STRINGS (and so in the language files).
    #[test]
    fn core_strings_cover_core_labels() {
        use sal_core::config::DriveHotkey;
        use sal_core::drives::DriveType;
        use sal_core::shell::TerminalKind;
        let mut labels: Vec<&str> = DriveHotkey::ALL.iter().map(|h| h.label()).collect();
        labels.extend(TerminalKind::ALL.iter().map(|t| t.label()));
        labels.extend([DriveType::Fixed, DriveType::Removable, DriveType::Network, DriveType::CdRom, DriveType::RamDisk, DriveType::Unknown].map(|d| d.label()));
        for l in labels {
            assert!(CORE_STRINGS.contains(&l), "{l:?} is missing from CORE_STRINGS");
        }
    }

    /// Sources and their language files: the program's (`src`, `lang`) and
    /// each plugin's (`plugins/<name>/src`, `plugins/<name>/lang`).
    fn source_sets() -> Vec<(PathBuf, PathBuf)> {
        let app = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut sets = vec![(app.join("src"), app.join("lang"))];
        let mut plugins: Vec<PathBuf> = std::fs::read_dir(app.join("../../plugins")).unwrap().flatten().map(|e| e.path()).filter(|p| p.join("src").is_dir()).collect();
        plugins.sort();
        sets.extend(plugins.into_iter().map(|p| (p.join("src"), p.join("lang"))));
        sets
    }

    /// Every string the code translates is in en.toml (the exported list)
    /// and in cs.toml next to it; neither file has strings the code no longer
    /// uses. Plugins' strings are in their own files, not the program's.
    #[test]
    fn language_files_match_the_code() {
        let sets = source_sets();
        assert!(sets.len() >= 2, "no plugins found: {sets:?}");
        for (i, (src, lang)) in sets.iter().enumerate() {
            let used = used_strings(src, i == 0);
            assert!(used.len() > if i == 0 { 150 } else { 1 }, "found only {} strings in {}", used.len(), src.display());
            for file in ["en.toml", "cs.toml"] {
                let path = lang.join(file);
                let cat = Catalog::parse(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap();
                let have: std::collections::BTreeSet<&str> = cat.keys().collect();
                let missing: Vec<&String> = used.iter().filter(|s| !have.contains(s.as_str())).collect();
                let unused: Vec<&&str> = have.iter().filter(|k| !used.contains(**k)).collect();
                assert!(missing.is_empty(), "{} lacks {} strings: {missing:#?}", path.display(), missing.len());
                assert!(unused.is_empty(), "{} has strings the code doesn't use: {unused:#?}", path.display());
            }
        }
    }

    /// A plugin's texts come from its own language files.
    #[test]
    fn plugin_texts_from_their_folder() {
        let (_, lang) = source_sets().into_iter().find(|(s, _)| s.ends_with("sevenzip/src") || s.ends_with("sevenzip\\src")).expect("the 7-Zip plugin");
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("lang");
        set_current(load_in(Some(&source), "cs"));
        assert_eq!(tr_from(&lang, "7-Zip archives"), "Archivy 7-Zip");
        assert_eq!(tr("7-Zip archives"), "7-Zip archives", "not in the program's files");
        assert_eq!(tr_from(&source, "Files"), "Soubory");
        assert_eq!(tr_from(Path::new("C:\\no such folder"), "7-Zip archives"), "7-Zip archives", "no files: English");
        set_current(None);
        assert_eq!(tr_from(&lang, "7-Zip archives"), "7-Zip archives");
    }

    /// Writes lang/en.toml from the strings in the code (keeps it complete),
    /// the program's and each plugin's:
    /// `cargo test -p sal-app export_english_strings -- --ignored`
    #[test]
    #[ignore]
    fn export_english_strings() {
        for (i, (src, lang)) in source_sets().iter().enumerate() {
            let used = used_strings(src, i == 0);
            let mut out = String::from(
                "# RustySalamander UI strings — the template for translations.\n\
                 # Generated from the source; copy to <code>.toml, set [language] and translate the values.\n\
                 # Keys with \"|\" are plural forms: give as many forms as the language's plural rule has.\n\n\
                 [language]\ncode = \"en\"\nname = \"English\"\nplural = \"en\"\n\n[strings]\n",
            );
            for s in &used {
                let q = toml_quote(s);
                out.push_str(&format!("{q} = {q}\n"));
            }
            std::fs::create_dir_all(lang).unwrap();
            std::fs::write(lang.join("en.toml"), out).unwrap();
        }
    }

    fn toml_quote(s: &str) -> String {
        let mut q = String::from("\"");
        for c in s.chars() {
            match c {
                '"' => q.push_str("\\\""),
                '\\' => q.push_str("\\\\"),
                '\n' => q.push_str("\\n"),
                c => q.push(c),
            }
        }
        q.push('"');
        q
    }

    /// Strings passed to tr / trf / trn / n_ in the sources under `dir`
    /// (with sal-core's labels for the program).
    fn used_strings(dir: &Path, core: bool) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        if core {
            out.extend(CORE_STRINGS.iter().map(|s| s.to_string()));
        }
        let re_one = regex_lite_find;
        // the sources and their subfolders
        let mut dirs = vec![dir.to_path_buf()];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                    continue;
                }
                let src = std::fs::read_to_string(&p).unwrap();
                let code = match src.find("#[cfg(test)]\nmod tests") {
                    Some(i) => &src[..i],
                    None => &src[..],
                };
                re_one(code, &mut out);
            }
        }
        out
    }

    /// Minimal scanner for `tr("…")`, `trf("…"`, `trn("…", "…"` and `n_("…")`.
    fn regex_lite_find(code: &str, out: &mut std::collections::BTreeSet<String>) {
        let bytes = code.as_bytes();
        for (call, args) in [("tr(", 1), ("trf(", 1), ("trn(", 2), ("n_(", 1)] {
            let mut from = 0;
            while let Some(pos) = code[from..].find(call) {
                let at = from + pos;
                from = at + call.len();
                // whole word only (not "str(" or "attr(")
                if at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_') {
                    continue;
                }
                let mut i = from;
                let mut parts = Vec::new();
                for _ in 0..args {
                    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\n' || bytes[i] == b',') {
                        i += 1;
                    }
                    if i >= bytes.len() || bytes[i] != b'"' {
                        break;
                    }
                    let (s, end) = read_str(code, i);
                    parts.push(s);
                    i = end;
                }
                if parts.len() == args {
                    out.insert(if args == 2 { format!("{}|{}", parts[0], parts[1]) } else { parts.remove(0) });
                }
            }
        }
    }

    /// Read a Rust string literal starting at `i` (the opening quote).
    fn read_str(code: &str, i: usize) -> (String, usize) {
        let mut s = String::new();
        let mut chars = code[i + 1..].char_indices();
        while let Some((k, c)) = chars.next() {
            match c {
                '"' => return (s, i + 1 + k + 1),
                '\\' => match chars.next().map(|(_, c)| c) {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some(c) => s.push(c),
                    None => {}
                },
                c => s.push(c),
            }
        }
        (s, code.len())
    }
}
