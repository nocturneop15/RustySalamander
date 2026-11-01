//! Syntax highlighting: the syntax definitions and color themes of `bat`
//! (through the `two-face` crate) run by `syntect`. Over 200 formats: JSON,
//! XML, HTML, PowerShell, Bash, Batch, INI, TOML, YAML, Markdown, CSS,
//! JavaScript, Python, C, Rust, SQL and many more.
//!
//! No window here, so it can be tested: a file's lines go in, colored spans
//! per line come out.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme};
use syntect::parsing::SyntaxSet;
use two_face::theme::{EmbeddedLazyThemeSet, EmbeddedThemeName};

/// Larger files are shown without colors.
pub const MAX_BYTES: usize = 8 << 20;
/// Longer lines (minified files) are left without colors: highlighting them
/// would take long, and they read no better.
const MAX_LINE: usize = 20_000;

/// A run of text in one style, `len` bytes long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub len: usize,
    pub rgb: [u8; 3],
    pub italic: bool,
    pub underline: bool,
}

/// The syntax definitions, loaded once.
fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

/// The colors for the dark or the light theme.
fn theme(dark: bool) -> &'static Theme {
    static THEMES: OnceLock<EmbeddedLazyThemeSet> = OnceLock::new();
    THEMES.get_or_init(two_face::theme::extra).get(if dark { EmbeddedThemeName::OneHalfDark } else { EmbeddedThemeName::OneHalfLight })
}

/// The syntax of a file, by its name (`Makefile`, `.bashrc`, its extension)
/// or else its first line (`#!/bin/bash`, `<?xml`); `None` for plain text.
pub fn detect(file_name: &str, first_line: &str) -> Option<String> {
    let ss = syntaxes();
    let lower = file_name.to_lowercase();
    let ext = Path::new(&lower).extension().map(|e| e.to_string_lossy().into_owned());
    ss.find_syntax_by_extension(file_name)
        .or_else(|| ss.find_syntax_by_extension(&lower))
        .or_else(|| ext.as_deref().and_then(|e| ss.find_syntax_by_extension(e)))
        .or_else(|| ss.find_syntax_by_first_line(first_line))
        .filter(|s| s.name != "Plain Text")
        .map(|s| s.name.clone())
}

/// Every syntax there is, by name, for choosing one by hand.
pub fn names() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut v: Vec<String> = syntaxes().syntaxes().iter().filter(|s| !s.hidden && s.name != "Plain Text").map(|s| s.name.clone()).collect();
        v.sort_by_key(|n| n.to_lowercase());
        v.dedup();
        v
    })
}

/// The colored spans of each line in the syntax `syntax`, with the dark or
/// the light theme. `None` when the syntax is unknown, or `stop` was set.
pub fn highlight(lines: &[String], syntax: &str, dark: bool, stop: &AtomicBool) -> Option<Vec<Vec<Span>>> {
    let ss = syntaxes();
    let syntax = ss.find_syntax_by_name(syntax)?;
    let mut h = HighlightLines::new(syntax, theme(dark));
    let mut out = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        if i % 256 == 0 && stop.load(Ordering::Relaxed) {
            return None;
        }
        if line.len() > MAX_LINE {
            out.push(Vec::new());
            continue;
        }
        // the syntaxes expect each line with its line break
        let with_break = format!("{line}\n");
        let Ok(ranges) = h.highlight_line(&with_break, ss) else {
            out.push(Vec::new());
            continue;
        };
        let mut spans = Vec::with_capacity(ranges.len());
        let mut left = line.len();
        for (style, text) in ranges {
            let len = text.len().min(left);
            if len == 0 {
                continue;
            }
            left -= len;
            let c = style.foreground;
            spans.push(Span {
                len,
                rgb: [c.r, c.g, c.b],
                italic: style.font_style.contains(FontStyle::ITALIC),
                underline: style.font_style.contains(FontStyle::UNDERLINE),
            });
        }
        out.push(spans);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_recognised() {
        let by_name = |n: &str| detect(n, "");
        assert_eq!(by_name("data.json").as_deref(), Some("JSON"));
        assert_eq!(by_name("Build.XML").as_deref(), Some("XML"));
        assert_eq!(by_name("index.html").as_deref(), Some("HTML"));
        assert_eq!(by_name("skript.ps1").as_deref(), Some("PowerShell"));
        assert_eq!(by_name("install.sh").as_deref(), Some("Bourne Again Shell (bash)"));
        assert_eq!(by_name("Cargo.toml").as_deref(), Some("TOML"));
        assert_eq!(by_name("nastavení.ini").as_deref(), Some("INI"));
        assert_eq!(by_name("Makefile").as_deref(), Some("Makefile"));
        assert_eq!(by_name("poznámky.txt"), None);
        assert_eq!(detect("run", "#!/bin/bash").as_deref(), Some("Bourne Again Shell (bash)"));
        assert_eq!(detect("data", "<?xml version=\"1.0\"?>").as_deref(), Some("XML"));
        assert!(names().iter().any(|n| n == "PowerShell") && names().len() > 150);
    }

    #[test]
    fn lines_get_colors() {
        let lines: Vec<String> = ["{", "  \"název\": \"žluťoučký kůň 🦎\",", "  \"počet\": 42", "}"].map(String::from).to_vec();
        let stop = AtomicBool::new(false);
        let spans = highlight(&lines, "JSON", true, &stop).unwrap();
        assert_eq!(spans.len(), 4);
        for (line, s) in lines.iter().zip(&spans) {
            assert_eq!(s.iter().map(|s| s.len).sum::<usize>(), line.len(), "{line}");
        }
        let colors: std::collections::HashSet<[u8; 3]> = spans[1].iter().chain(&spans[2]).map(|s| s.rgb).collect();
        assert!(colors.len() >= 3, "keys, strings and numbers differ: {colors:?}");
        // the light theme differs
        let light = highlight(&lines, "JSON", false, &stop).unwrap();
        assert_ne!(light[1], spans[1]);
        // stopping gives nothing
        stop.store(true, Ordering::Relaxed);
        assert!(highlight(&lines, "JSON", true, &stop).is_none());
        assert!(highlight(&lines, "No such syntax", true, &AtomicBool::new(false)).is_none());
    }
}
