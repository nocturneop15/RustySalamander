//! Wildcard masks: the syntax and renaming by mask are ports of Salamander's
//! `masks.cpp`; matching is written anew (see [`agree_mask`]).
//!
//! * `?` any character, `*` any string, `#` any digit (extended mode only)
//! * groups are separated by `;` (a literal `;` is written as `;;`)
//! * masks after `|` exclude; a leading `|` implies `*` before it
//!
//! Matching works on Unicode scalar values with Unicode case folding, so
//! `*.ŽÁK` matches `žák.žák` — the ANSI original could not.

use std::fmt;

/// A character for case-insensitive matching.
fn fold(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

/// A name for case-insensitive matching.
fn fold_all(s: &str) -> Vec<char> {
    s.chars().map(fold).collect()
}

/// Port of `PrepareMask`: trims spaces, `**` → `*`, `*?` → `?*`.
pub fn prepare_mask(src: &str) -> Vec<char> {
    let mut out: Vec<char> = Vec::with_capacity(src.len());
    let mut last = '\0';
    for c in src.trim_start_matches(' ').chars() {
        if c == '*' && last == '*' {
            continue;
        } else if c == '?' && last == '*' {
            let n = out.len();
            out[n - 1] = '?';
            out.push('*');
        } else {
            out.push(c);
            last = c;
        }
    }
    while out.last() == Some(&' ') {
        out.pop();
    }
    out
}

/// Whether `name` matches `mask` (as `AgreeMask` decides). Both slices must
/// already be case-folded.
///
/// A `*` first takes nothing, and only the last `*` seen takes one more
/// character when the rest doesn't fit: never more than name length × mask
/// length steps (trying every way for every `*` took exponential time:
/// minutes for a mask with a dozen of them).
pub fn agree_mask(name: &[char], mask: &[char], has_ext: bool, extended: bool) -> bool {
    let (mut n, mut m) = (0usize, 0usize);
    // the last `*`, and the name position it took up to
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match mask.get(m) {
            Some('*') => {
                star = Some((m, n));
                m += 1;
            }
            Some(&mc) if mc == name[n] || mc == '?' || (extended && mc == '#' && name[n].is_ascii_digit()) => {
                n += 1;
                m += 1;
            }
            _ => match star {
                Some((sm, sn)) => {
                    star = Some((sm, sn + 1));
                    (m, n) = (sm + 1, sn + 1);
                }
                None => return false,
            },
        }
    }
    // the name is used up: what is left of the mask must fit nothing
    let rest = &mask[m..];
    let rest = &rest[rest.iter().take_while(|&&c| c == '*').count()..];
    if !has_ext && rest.first() == Some(&'.') {
        // without an extension, "*.*" must still match
        rest[1..].iter().all(|&c| c == '*')
    } else {
        rest.is_empty()
    }
}

/// Whether `name` has an extension (`..` doesn't).
fn has_extension(name: &str) -> bool {
    name != ".." && name.rfind('.').is_some_and(|i| i + 1 < name.len())
}

/// Port of `MaskName`: rename `name` by `mask` the way Win2K `copy` does
/// (`*.bak`, `a?c.*`, …). Trailing dots are trimmed from the result.
pub fn mask_name(name: &str, mask: &str) -> String {
    let name: Vec<char> = name.chars().collect();
    let mask: Vec<char> = mask.chars().collect();

    // the first dot in the mask splits it into name and extension parts;
    // dots in the name beyond that belong to the name part
    let mut ign_points = name.iter().filter(|&&c| c == '.').count() as isize;
    if mask.contains(&'.') {
        ign_points -= 1;
    }
    let mut ign_points = ign_points.max(0);

    let mut out: Vec<char> = Vec::with_capacity(name.len() + mask.len());
    let mut n = 0usize;
    for &mc in &mask {
        match mc {
            '*' => {
                while n < name.len() {
                    if name[n] == '.' {
                        if ign_points > 0 {
                            ign_points -= 1;
                        } else {
                            break;
                        }
                    }
                    out.push(name[n]);
                    n += 1;
                }
            }
            '?' => {
                if n < name.len() {
                    if name[n] == '.' {
                        if ign_points > 0 {
                            ign_points -= 1;
                            out.push(name[n]);
                            n += 1;
                        }
                    } else {
                        out.push(name[n]);
                        n += 1;
                    }
                }
            }
            '.' => {
                out.push('.');
                while n < name.len() {
                    if name[n] == '.' {
                        if ign_points > 0 {
                            ign_points -= 1;
                        } else {
                            break;
                        }
                    }
                    n += 1;
                }
                if n < name.len() && name[n] == '.' {
                    n += 1;
                }
            }
            c => {
                out.push(c);
                if n < name.len() {
                    if name[n] != '.' {
                        n += 1;
                    } else if ign_points > 0 {
                        ign_points -= 1;
                        n += 1;
                    }
                }
            }
        }
    }
    while out.last() == Some(&'.') {
        out.pop();
    }
    out.into_iter().collect()
}

/// True if the string contains mask wildcards.
pub fn has_wildcards(s: &str) -> bool {
    s.contains(['*', '?'])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskError {
    /// Character index in the mask string where the error was detected.
    pub position: usize,
}

impl fmt::Display for MaskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid mask syntax at position {}", self.position + 1)
    }
}

impl std::error::Error for MaskError {}

/// Port of `CMaskGroup`.
#[derive(Debug, Clone, Default)]
pub struct MaskGroup {
    source: String,
    extended: bool,
    include: Vec<Vec<char>>,
    exclude: Vec<Vec<char>>,
}

impl MaskGroup {
    /// Masks as typed: `*.txt;*.md|old*` (to include, then after `|` to
    /// leave out); `extended` lets `*.*` match names without a dot too.
    pub fn new(masks: &str, extended: bool) -> Result<Self, MaskError> {
        let chars: Vec<char> = masks.chars().collect();
        let mut include = Vec::new();
        let mut exclude = Vec::new();
        let mut exclude_pos: Option<usize> = None;
        let mut i = 0usize;
        loop {
            let mut buf = String::new();
            while i < chars.len() {
                let c = chars[i];
                if (c as u32) <= 31 || matches!(c, '\\' | '/' | '<' | '>' | ':' | '"') || c == '|' {
                    break;
                }
                if c == ';' {
                    if chars.get(i + 1) == Some(&';') {
                        i += 1;
                    } else {
                        break;
                    }
                }
                buf.push(chars[i]);
                i += 1;
            }
            if i < chars.len() && chars[i] != ';' && (chars[i] != '|' || exclude_pos.is_some()) {
                return Err(MaskError { position: i });
            }
            let trimmed = buf.trim_matches(|c: char| c <= ' ');
            if !trimmed.is_empty() {
                let prepared: Vec<char> = prepare_mask(trimmed).into_iter().map(fold).collect();
                if !prepared.is_empty() {
                    if exclude_pos.is_some() {
                        exclude.push(prepared);
                    } else {
                        include.push(prepared);
                    }
                }
            }
            if i >= chars.len() {
                if let Some(pos) = exclude_pos {
                    if exclude.is_empty() {
                        return Err(MaskError { position: pos });
                    }
                }
                break;
            }
            if chars[i] == '|' {
                if include.is_empty() {
                    include.push(vec!['*']);
                }
                exclude_pos = Some(i);
            }
            i += 1;
        }
        Ok(MaskGroup { source: masks.to_owned(), extended, include, exclude })
    }

    /// The masks as typed.
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Port of `CMaskGroup::AgreeMasks`.
    pub fn matches(&self, file_name: &str) -> bool {
        let name = fold_all(file_name);
        let has_ext = has_extension(file_name);
        if self.exclude.iter().any(|m| agree_mask(&name, m, has_ext, self.extended)) {
            return false;
        }
        self.include.iter().any(|m| agree_mask(&name, m, has_ext, self.extended))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(m: &str) -> MaskGroup {
        MaskGroup::new(m, false).unwrap()
    }

    #[test]
    fn basic_masks() {
        assert!(g("*").matches("anything.txt"));
        assert!(g("*.*").matches("noext"));
        assert!(g("*.txt").matches("a.b.TXT"));
        assert!(!g("*.txt").matches("a.txt.bak"));
        assert!(g("*.t??").matches("x.tmp"));
        assert!(!g("*.t??").matches("x.tm"));
        assert!(g("a*b*c").matches("aXXbYYc"));
    }

    #[test]
    fn groups_and_exclude() {
        let m = g("*.txt;*.cpp");
        assert!(m.matches("a.cpp") && m.matches("b.txt") && !m.matches("c.h"));
        let m = g("*.h*|*.html");
        assert!(m.matches("x.hpp") && !m.matches("x.html"));
        let m = g("|*.txt");
        assert!(m.matches("x.doc") && !m.matches("x.txt"));
        let m = g("a;;b.txt");
        assert!(m.matches("a;b.txt"));
    }

    #[test]
    fn mask_errors() {
        assert!(MaskGroup::new("*.txt|", false).is_err());
        assert!(MaskGroup::new("*.a|*.b|*.c", false).is_err());
        assert_eq!(MaskGroup::new("a<b", false).unwrap_err().position, 1);
    }

    /// Many `*` that almost fit: once minutes, now at once.
    #[test]
    fn many_stars_stay_fast() {
        let start = std::time::Instant::now();
        let m = g(&format!("{}b", "*a".repeat(14)));
        assert!(!m.matches(&"a".repeat(40)));
        assert!(m.matches(&format!("{}b", "a".repeat(40))));
        assert!(start.elapsed() < std::time::Duration::from_millis(100), "{:?}", start.elapsed());
        // the end of the name: what the stars leave
        assert!(g("a*.*").matches("abc") && g("*.*").matches("noext") && !g("*.x").matches("noext"));
        assert!(g("*a*").matches("banana") && g("b*n*a").matches("banana") && !g("b*n*x").matches("banana"));
    }

    #[test]
    fn extended_digits() {
        let m = MaskGroup::new("*.r##", true).unwrap();
        assert!(m.matches("arch.r01") && !m.matches("arch.rar"));
    }

    #[test]
    fn unicode_masks() {
        assert!(g("*.ŽÁK").matches("soubor.žák"));
        assert!(g("日本*").matches("日本語.txt"));
        assert!(g("*😀*").matches("party 😀 time.png"));
        assert!(g("?.txt").matches("é.txt"));
    }

    #[test]
    fn prepare() {
        assert_eq!(prepare_mask("  **.txt  ").into_iter().collect::<String>(), "*.txt");
        assert_eq!(prepare_mask("*?a").into_iter().collect::<String>(), "?*a");
        assert_eq!(prepare_mask("*??").into_iter().collect::<String>(), "??*");
    }

    #[test]
    fn mask_names() {
        assert_eq!(mask_name("readme.txt", "*.bak"), "readme.bak");
        assert_eq!(mask_name("a.b.c.d", "*.*.old"), "a.b.c.d.old");
        assert_eq!(mask_name("report.doc", "*"), "report.doc");
        assert_eq!(mask_name("report.doc", "*.*"), "report.doc");
        assert_eq!(mask_name("abc.txt", "x?z.*"), "xbz.txt");
        assert_eq!(mask_name("příliš.txt", "*_old.*"), "příliš_old.txt");
    }
}
