//! Fonts: the user's chosen UI and viewer fonts (any installed TrueType/
//! OpenType family), backed by a Unicode fallback chain of Windows system
//! fonts for CJK, Indic, SE-Asian, symbols and emoji (egui's own fonts only
//! cover Latin).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use egui::{FontData, FontDefinitions, FontFamily};

/// (file in %WINDIR%\Fonts, collection index, use as default UI font)
const PROPORTIONAL: &[(&str, u32, bool)] = &[
    ("segoeui.ttf", 0, true),
    ("seguisym.ttf", 0, false),
    ("seguiemj.ttf", 0, false),
    ("msyh.ttc", 0, false),    // Chinese (Simplified)
    ("msjh.ttc", 0, false),    // Chinese (Traditional)
    ("YuGothM.ttc", 0, false), // Japanese
    ("meiryo.ttc", 0, false),  // Japanese (older systems)
    ("malgun.ttf", 0, false),  // Korean
    ("Nirmala.ttc", 0, false), // Indic scripts
    ("Nirmala.ttf", 0, false),
    ("LeelawUI.ttf", 0, false), // Thai, Lao, Khmer
    ("ebrima.ttf", 0, false),   // African scripts
    ("mmrtext.ttf", 0, false),  // Myanmar
    ("himalaya.ttf", 0, false), // Tibetan
    ("seguihis.ttf", 0, false), // historic scripts
    ("simsun.ttc", 0, false),   // CJK extension coverage
];

/// Default viewer fonts, first one found wins.
const MONOSPACE: &[&str] = &["consola.ttf", "cascadiamono.ttf"];

pub const DEFAULT_UI_FONT: &str = "Segoe UI";
pub const DEFAULT_MONO_FONT: &str = "Consolas";

fn fonts_dir() -> PathBuf {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    Path::new(&windir).join("Fonts")
}

/// Font files are read once and kept for the life of the app, so changing
/// the font doesn't re-read the large CJK fallbacks.
fn font_bytes(path: &Path) -> Option<&'static [u8]> {
    static CACHE: Mutex<Option<HashMap<PathBuf, &'static [u8]>>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some(b) = cache.get(path) {
        return Some(b);
    }
    let bytes: &'static [u8] = Box::leak(std::fs::read(path).ok()?.into_boxed_slice());
    cache.insert(path.to_path_buf(), bytes);
    Some(bytes)
}

/// An installed font family (its regular face).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemFont {
    pub name: String,
    pub path: PathBuf,
}

/// Words that mark a non-regular face in the registry's font names.
const STYLE_WORDS: &[&str] =
    &["bold", "italic", "oblique", "light", "semilight", "semibold", "demibold", "thin", "medium", "heavy", "extralight", "extrabold", "ultralight", "ultrabold"];

/// Installed TrueType/OpenType families, by name, from the Windows font
/// registry (machine-wide and per-user).
#[cfg(windows)]
pub fn system_fonts() -> Vec<SystemFont> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::System::Registry::{RegCloseKey, RegEnumValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};

    let key: Vec<u16> = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Fonts\0".encode_utf16().collect();
    let dir = fonts_dir();
    let mut out: Vec<SystemFont> = Vec::new();
    for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        let mut hk = HKEY::default();
        if unsafe { RegOpenKeyExW(root, PCWSTR(key.as_ptr()), 0, KEY_READ, &mut hk) }.is_err() {
            continue;
        }
        for i in 0.. {
            let mut name = [0u16; 512];
            let mut name_len = name.len() as u32;
            let mut data = [0u16; 1024];
            let mut data_len = std::mem::size_of_val(&data) as u32;
            let r = unsafe { RegEnumValueW(hk, i, PWSTR(name.as_mut_ptr()), &mut name_len, None, None, Some(data.as_mut_ptr().cast()), Some(&mut data_len)) };
            if r.is_err() {
                break; // ERROR_NO_MORE_ITEMS (or a value too long to matter)
            }
            let name = String::from_utf16_lossy(&name[..name_len as usize]);
            let chars = (data_len as usize / 2).min(data.len());
            let file = String::from_utf16_lossy(&data[..chars]).trim_end_matches('\0').to_owned();
            if let Some(f) = parse_entry(&name, &file, &dir) {
                out.push(f);
            }
        }
        unsafe {
            let _ = RegCloseKey(hk);
        }
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    out
}

#[cfg(not(windows))]
pub fn system_fonts() -> Vec<SystemFont> {
    Vec::new()
}

/// "Segoe UI Bold (TrueType)" + "segoeuib.ttf" -> None (not a regular face);
/// "Cascadia Mono (TrueType)" + "CascadiaMono.ttf" -> Some("Cascadia Mono").
fn parse_entry(reg_name: &str, file: &str, fonts_dir: &Path) -> Option<SystemFont> {
    let lower = file.to_lowercase();
    if !(lower.ends_with(".ttf") || lower.ends_with(".otf") || lower.ends_with(".ttc")) {
        return None; // bitmap/vector .fon fonts can't be used
    }
    let name = reg_name.split(" (").next().unwrap_or(reg_name).trim();
    if name.is_empty() || name.split([' ', '-']).any(|w| STYLE_WORDS.contains(&w.to_lowercase().as_str())) {
        return None;
    }
    // collections are listed as "Cambria & Cambria Math": the first face is used
    let name = name.split(" & ").next().unwrap_or(name).to_owned();
    let path = if Path::new(file).is_absolute() { PathBuf::from(file) } else { fonts_dir.join(file) };
    Some(SystemFont { name, path })
}

fn load(defs: &mut FontDefinitions, key: &str, path: &Path, index: u32) -> bool {
    match font_bytes(path) {
        Some(bytes) => {
            let mut fd = FontData::from_static(bytes);
            fd.index = index;
            defs.font_data.insert(key.to_owned(), fd);
            true
        }
        None => false,
    }
}

/// Font set for the chosen UI and viewer fonts (empty or unknown names mean
/// the defaults), with the Unicode fallback chain behind both.
pub fn definitions(ui_font: &str, mono_font: &str, installed: &[SystemFont]) -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    let dir = fonts_dir();
    let find = |name: &str| installed.iter().find(|f| !name.is_empty() && f.name.eq_ignore_ascii_case(name));

    let mut fallbacks = Vec::new();
    let mut default_ui = None;
    for &(file, index, is_default) in PROPORTIONAL {
        if load(&mut defs, file, &dir.join(file), index) {
            if is_default {
                default_ui = Some(file.to_owned());
            } else {
                fallbacks.push(file.to_owned());
            }
        }
    }
    let ui = match find(ui_font) {
        Some(f) if load(&mut defs, &format!("ui:{}", f.name), &f.path, 0) => Some(format!("ui:{}", f.name)),
        _ => default_ui.clone(),
    };
    let mono = match find(mono_font) {
        Some(f) if load(&mut defs, &format!("mono:{}", f.name), &f.path, 0) => Some(format!("mono:{}", f.name)),
        _ => MONOSPACE.iter().find(|&&file| load(&mut defs, file, &dir.join(file), 0)).map(|f| f.to_string()),
    };

    if let Some(u) = &ui {
        defs.families.entry(FontFamily::Proportional).or_default().insert(0, u.clone());
    }
    if let Some(m) = mono {
        defs.families.entry(FontFamily::Monospace).or_default().insert(0, m);
    }
    // fallbacks go after egui's own fonts in both families; Segoe UI covers
    // Latin, Greek, Cyrillic, Hebrew and Arabic a chosen font may lack
    for fam in [FontFamily::Proportional, FontFamily::Monospace] {
        let list = defs.families.entry(fam).or_default();
        for f in ui.iter().chain(default_ui.iter()) {
            if !list.contains(f) {
                list.push(f.clone());
            }
        }
        list.extend(fallbacks.iter().cloned());
    }
    // Windows icon font for the Windows 11 style context menu
    for file in ["SegoeIcons.ttf", "segmdl2.ttf"] {
        if load(&mut defs, ICON_FONT, &dir.join(file), 0) {
            defs.families.insert(FontFamily::Name(ICON_FONT.into()), vec![ICON_FONT.to_owned()]);
            break;
        }
    }
    defs
}

pub fn install(ctx: &egui::Context, ui_font: &str, mono_font: &str) {
    let installed = if ui_font.is_empty() && mono_font.is_empty() { Vec::new() } else { system_fonts() };
    ctx.set_fonts(definitions(ui_font, mono_font, &installed));
}

pub const ICON_FONT: &str = "icons";

/// The icon font family, if a Windows icon font was found.
pub fn icon_family(ctx: &egui::Context) -> Option<FontFamily> {
    let fam = FontFamily::Name(ICON_FONT.into());
    ctx.fonts(|f| f.families().contains(&fam)).then_some(fam)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_names_are_parsed() {
        let dir = Path::new(r"C:\Windows\Fonts");
        assert_eq!(parse_entry("Segoe UI (TrueType)", "segoeui.ttf", dir).unwrap().name, "Segoe UI");
        assert!(parse_entry("Segoe UI Bold (TrueType)", "segoeuib.ttf", dir).is_none());
        assert!(parse_entry("Arial Italic (TrueType)", "ariali.ttf", dir).is_none());
        assert!(parse_entry("Courier 10,12,15", "COURE.FON", dir).is_none(), "bitmap fonts are skipped");
        let c = parse_entry("Cambria & Cambria Math (TrueType)", "cambria.ttc", dir).unwrap();
        assert_eq!((c.name.as_str(), c.path.clone()), ("Cambria", dir.join("cambria.ttc")));
        let user = parse_entry("Fira Code (TrueType)", r"C:\Users\x\AppData\Local\Microsoft\Windows\Fonts\FiraCode.ttf", dir).unwrap();
        assert_eq!(user.path, PathBuf::from(r"C:\Users\x\AppData\Local\Microsoft\Windows\Fonts\FiraCode.ttf"));
    }

    #[cfg(windows)]
    #[test]
    fn installed_fonts_and_definitions() {
        let fonts = system_fonts();
        for want in ["Segoe UI", "Consolas", "Arial"] {
            let f = fonts.iter().find(|f| f.name == want).unwrap_or_else(|| panic!("{want} not listed"));
            assert!(f.path.is_file(), "{:?}", f.path);
        }
        assert!(fonts.windows(2).all(|w| w[0].name.to_lowercase() <= w[1].name.to_lowercase()), "sorted");
        assert!(!fonts.iter().any(|f| f.name.ends_with(" Bold")));

        // chosen fonts come first; the fallback chain stays behind them
        let d = definitions("Arial", "Courier New", &fonts);
        let prop = &d.families[&FontFamily::Proportional];
        let mono = &d.families[&FontFamily::Monospace];
        assert_eq!(prop[0], "ui:Arial");
        assert_eq!(mono[0], "mono:Courier New");
        assert!(prop.iter().any(|f| f == "msyh.ttc") && mono.iter().any(|f| f == "msyh.ttc"));

        // defaults, and unknown names fall back to them
        let d = definitions("", "", &fonts);
        assert_eq!(d.families[&FontFamily::Proportional][0], "segoeui.ttf");
        assert_eq!(d.families[&FontFamily::Monospace][0], "consola.ttf");
        let d = definitions("No Such Font", "Nope", &fonts);
        assert_eq!(d.families[&FontFamily::Proportional][0], "segoeui.ttf");
    }
}
