//! Text encoding detection/decoding for the viewer (replaces `codetbl.cpp`
//! and the `/convert` tables) and path display helpers.

use std::borrow::Cow;
use std::path::Path;

use encoding_rs::Encoding;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Legacy(&'static Encoding),
}

impl TextEncoding {
    /// The encoding's name in the viewer's list.
    pub fn name(self) -> &'static str {
        match self {
            TextEncoding::Utf8 => "UTF-8",
            TextEncoding::Utf16Le => "UTF-16 LE",
            TextEncoding::Utf16Be => "UTF-16 BE",
            TextEncoding::Legacy(e) => e.name(),
        }
    }
}

/// Encodings offered in the viewer's encoding menu.
pub fn encoding_choices() -> Vec<TextEncoding> {
    let mut v = vec![TextEncoding::Utf8, TextEncoding::Utf16Le, TextEncoding::Utf16Be];
    v.extend(
        [
            encoding_rs::WINDOWS_1250,
            encoding_rs::WINDOWS_1251,
            encoding_rs::WINDOWS_1252,
            encoding_rs::WINDOWS_1253,
            encoding_rs::WINDOWS_1254,
            encoding_rs::WINDOWS_1255,
            encoding_rs::WINDOWS_1256,
            encoding_rs::WINDOWS_1257,
            encoding_rs::ISO_8859_2,
            encoding_rs::KOI8_R,
            encoding_rs::IBM866,
            encoding_rs::SHIFT_JIS,
            encoding_rs::EUC_JP,
            encoding_rs::GBK,
            encoding_rs::BIG5,
            encoding_rs::EUC_KR,
        ]
        .map(TextEncoding::Legacy),
    );
    v
}

/// The system's ANSI code page, used for files that are not valid UTF-8.
pub fn system_legacy_encoding() -> &'static Encoding {
    #[cfg(windows)]
    {
        let acp = unsafe { windows_sys::Win32::Globalization::GetACP() };
        let label = match acp {
            932 => "shift_jis",
            936 => "gbk",
            949 => "euc-kr",
            950 => "big5",
            874 => "windows-874",
            1250..=1258 => return Encoding::for_label(format!("windows-{acp}").as_bytes()).unwrap_or(encoding_rs::WINDOWS_1252),
            _ => "windows-1252",
        };
        Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::WINDOWS_1252)
    }
    #[cfg(not(windows))]
    {
        encoding_rs::WINDOWS_1252
    }
}

/// Detects the encoding. Returns the encoding and the BOM length to skip.
pub fn detect(bytes: &[u8]) -> (TextEncoding, usize) {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return (TextEncoding::Utf8, 3);
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return (TextEncoding::Utf16Le, 2);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return (TextEncoding::Utf16Be, 2);
    }
    // BOM-less UTF-16: lots of zero bytes on one parity (ASCII-heavy text)
    let sample = &bytes[..bytes.len().min(4096)];
    if sample.len() >= 4 {
        let even_zeros = sample.iter().step_by(2).filter(|&&b| b == 0).count();
        let odd_zeros = sample.iter().skip(1).step_by(2).filter(|&&b| b == 0).count();
        let half = sample.len() / 2;
        if odd_zeros > half * 2 / 5 && even_zeros < half / 10 {
            return (TextEncoding::Utf16Le, 0);
        }
        if even_zeros > half * 2 / 5 && odd_zeros < half / 10 {
            return (TextEncoding::Utf16Be, 0);
        }
    }
    // a multi-byte char may be cut at the end of a partial read
    match std::str::from_utf8(bytes) {
        Ok(_) => (TextEncoding::Utf8, 0),
        Err(e) if e.error_len().is_none() => (TextEncoding::Utf8, 0),
        Err(_) => (TextEncoding::Legacy(system_legacy_encoding()), 0),
    }
}

/// `bytes` as text in `enc`; bytes that aren't valid become U+FFFD.
pub fn decode(bytes: &[u8], enc: TextEncoding) -> String {
    match enc {
        TextEncoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        TextEncoding::Utf16Le => encoding_rs::UTF_16LE.decode_without_bom_handling(bytes).0.into_owned(),
        TextEncoding::Utf16Be => encoding_rs::UTF_16BE.decode_without_bom_handling(bytes).0.into_owned(),
        TextEncoding::Legacy(e) => e.decode_without_bom_handling(bytes).0.into_owned(),
    }
}

/// Detect and decode in one step.
pub fn decode_auto(bytes: &[u8]) -> (String, TextEncoding) {
    let (enc, bom) = detect(bytes);
    (decode(&bytes[bom..], enc), enc)
}

/// Whether `c` is written right to left (Hebrew, Arabic, …).
fn is_rtl_char(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF)
}

/// Reorder one line into visual (left-to-right glyph) order using the Unicode
/// Bidirectional Algorithm; the GUI lays glyphs out strictly left to right, so
/// Hebrew/Arabic would otherwise appear reversed. `ltr_base` forces a
/// left-to-right paragraph (as Explorer does for file names); otherwise the
/// direction comes from the first strong character.
pub fn visual_order(s: &str, ltr_base: bool) -> Cow<'_, str> {
    if !s.chars().any(is_rtl_char) {
        return Cow::Borrowed(s);
    }
    let level = ltr_base.then(unicode_bidi::Level::ltr);
    let info = unicode_bidi::BidiInfo::new(s, level);
    let mut out = String::with_capacity(s.len());
    for para in &info.paragraphs {
        out.push_str(&info.reorder_line(para, para.range.clone()));
    }
    Cow::Owned(out)
}

/// Characters NTFS forbids (`*`, `:`, `?` …) that names from Macs, Samba
/// shares or WSL carry as private-use stand-ins, which no font can draw.
fn unmap_private(c: char) -> Option<char> {
    Some(match c as u32 {
        // Services for Macintosh / Samba "fruit:encoding = private"
        0xF020 => '"',
        0xF021 => '*',
        0xF022 => ':',
        0xF023 => '<',
        0xF024 => '>',
        0xF025 => '?',
        0xF026 => '\\',
        0xF027 => '|',
        0xF028 => ' ',
        0xF029 => '.',
        // WSL / Cygwin: U+F000 + the ASCII character
        0xF02A => '*',
        0xF03A => ':',
        0xF03C => '<',
        0xF03E => '>',
        0xF03F => '?',
        0xF05C => '\\',
        0xF07C => '|',
        _ => return None,
    })
}

/// File name as it should be shown: private-use stand-ins for characters
/// Windows can't store are displayed as the original character. Display
/// only; operations keep using the real name.
pub fn display_name(name: &str) -> Cow<'_, str> {
    if !name.chars().any(|c| unmap_private(c).is_some()) {
        return Cow::Borrowed(name);
    }
    Cow::Owned(name.chars().map(|c| unmap_private(c).unwrap_or(c)).collect())
}

/// UTF-8 path for display, without the `\\?\` verbatim prefix. A plugin's
/// connection shows as a URL: `\\?\scp\me@host\home` → `scp://me@host/home`.
pub fn display_path(p: &Path) -> String {
    if let Some((scheme, authority, inner)) = crate::location::virtual_parts(p) {
        return format!("{scheme}://{authority}/{}", inner.join("/"));
    }
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        s.into_owned()
    }
}

/// Human-readable size the way Salamander shows it in the status bar.
pub fn format_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.2} {}", UNITS[u])
}

/// Size with thousands separators, as shown in the panel's size column.
pub fn format_thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_boms_and_utf8() {
        let s = "Příliš žluťoučký kůň 日本 😀";
        let mut b = vec![0xEF, 0xBB, 0xBF];
        b.extend_from_slice(s.as_bytes());
        assert_eq!(decode_auto(&b), (s.to_string(), TextEncoding::Utf8));
        assert_eq!(decode_auto(s.as_bytes()).1, TextEncoding::Utf8);

        let mut le = vec![0xFF, 0xFE];
        le.extend(s.encode_utf16().flat_map(|u| u.to_le_bytes()));
        assert_eq!(decode_auto(&le), (s.to_string(), TextEncoding::Utf16Le));

        let mut be = vec![0xFE, 0xFF];
        be.extend(s.encode_utf16().flat_map(|u| u.to_be_bytes()));
        assert_eq!(decode_auto(&be), (s.to_string(), TextEncoding::Utf16Be));
    }

    #[test]
    fn bomless_utf16_and_legacy() {
        let le: Vec<u8> = "plain ascii text in utf16".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(detect(&le).0, TextEncoding::Utf16Le);
        // "Příliš" in windows-1250
        let cp1250 = [0x50, 0xF8, 0xED, 0x6C, 0x69, 0x9A];
        assert_eq!(decode(&cp1250, TextEncoding::Legacy(encoding_rs::WINDOWS_1250)), "Příliš");
        assert!(matches!(detect(&cp1250).0, TextEncoding::Legacy(_)));
    }

    #[test]
    fn private_use_stand_ins_display_as_originals() {
        assert!(matches!(display_name("plain name"), Cow::Borrowed(_)));
        // Mac / Samba (SFM) and WSL / Cygwin mappings
        assert_eq!(display_name("hv\u{F021}zdi\u{F022}ky"), "hv*zdi:ky");
        assert_eq!(display_name("a\u{F02A}b\u{F03F}c\u{F07C}d"), "a*b?c|d");
        assert_eq!(display_name("žluťoučký \u{F021}"), "žluťoučký *");
        // other private-use characters are left alone
        assert_eq!(display_name("\u{E000}x"), "\u{E000}x");
    }

    #[test]
    fn bidi_visual_order() {
        assert!(matches!(visual_order("plain.txt", true), Cow::Borrowed(_)));
        assert_eq!(visual_order("עברית", true), "תירבע");
        assert_eq!(visual_order("עברית.txt", true), "תירבע.txt");
        assert_eq!(visual_order("שלום 123", false), "123 םולש");
    }

    #[test]
    fn paths_and_sizes() {
        assert_eq!(display_path(Path::new(r"\\?\C:\x\ž")), r"C:\x\ž");
        assert_eq!(display_path(Path::new(r"\\?\UNC\srv\share")), r"\\srv\share");
        assert_eq!(format_thousands(1234567), "1 234 567");
        assert_eq!(format_size(1536), "1.50 KB");
    }
}
