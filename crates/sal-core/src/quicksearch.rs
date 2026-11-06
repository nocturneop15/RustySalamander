//! Panel quick search: preparing what was typed is a port of the QS
//! functions in Salamander's `masks.cpp`; matching is written anew.
//!
//! The typed text matches name prefixes case-insensitively; `/`, `\` and `<`
//! stand for any sequence of characters (they cannot occur in file names).
//! Loose matching also ignores diacritics: `vladce` finds `vládce` and
//! `vládče`, `o` finds `ø`, `ss` finds `ß`.

use unicode_normalization::UnicodeNormalization;

/// A character for case-insensitive matching.
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// A character for matching without case and diacritics: its base letters
/// (one or two), lowercase.
fn fold_loose(c: char, out: &mut Vec<char>) {
    let c = fold(c);
    if c.is_ascii() {
        out.push(c);
        return;
    }
    // letters of their own in Unicode, not a letter with a mark
    let own = match c {
        'ø' => "o",
        'ł' | 'ŀ' => "l",
        'đ' | 'ð' => "d",
        'ħ' => "h",
        'ı' => "i",
        'ŧ' => "t",
        'ĸ' => "k",
        'ß' => "ss",
        'æ' => "ae",
        'œ' => "oe",
        'þ' => "th",
        _ => "",
    };
    if !own.is_empty() {
        out.extend(own.chars());
        return;
    }
    let before = out.len();
    out.extend(std::iter::once(c).nfd().filter(|&d| !unicode_normalization::char::is_combining_mark(d)));
    if out.len() == before {
        out.push(c);
    }
}

/// A prepared quick search: what was typed, folded for matching.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    chars: Vec<char>,
    /// Diacritics ignored (otherwise the letters must be the same).
    loose: bool,
}

impl Mask {
    /// The name folded like the mask, and for each folded character the
    /// index of the name character it comes from.
    fn fold_name(&self, name: &str) -> (Vec<char>, Vec<usize>) {
        let (mut folded, mut origin) = (Vec::with_capacity(name.len()), Vec::with_capacity(name.len()));
        for (i, c) in name.chars().enumerate() {
            if self.loose {
                fold_loose(c, &mut folded);
            } else {
                folded.push(fold(c));
            }
            origin.resize(folded.len(), i);
        }
        (folded, origin)
    }
}

/// Whether `c` stands for any characters in a quick search (`/`, `\`, `<`).
pub fn is_wild_char(c: char) -> bool {
    matches!(c, '/' | '\\' | '<')
}

/// Port of `PrepareQSMask`: collapse wild chars to a single `/`, drop a
/// trailing `/`, and case-fold; `loose` also drops diacritics.
pub fn prepare(src: &str, loose: bool) -> Mask {
    let mut out = Vec::with_capacity(src.len());
    for c in src.chars() {
        if is_wild_char(c) {
            if out.last() != Some(&'/') {
                out.push('/');
            }
        } else if loose {
            fold_loose(c, &mut out);
        } else {
            out.push(fold(c));
        }
    }
    if out.last() == Some(&'/') {
        out.pop();
    }
    Mask { chars: out, loose }
}

/// Where the match of `mask` from `pos` in `name` ends, if it matches there
/// (the mask covers the start of what follows `pos`; `/` takes as little as
/// it can).
///
/// A `/` first takes nothing, and only the last `/` seen takes one more
/// character when the rest doesn't fit: never more than name length × mask
/// length steps (trying every way for every `/` took exponential time).
fn agree(name: &[char], pos: usize, mask: &[char], has_ext: bool) -> Option<usize> {
    let (mut n, mut m) = (pos, 0usize);
    // the last `/`, and the name position it took up to
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match mask.get(m) {
            None => return Some(n),
            Some('/') => {
                star = Some((m, n));
                m += 1;
            }
            Some(&mc) if mc == name[n] => {
                n += 1;
                m += 1;
            }
            _ => match star {
                Some((sm, sn)) => {
                    star = Some((sm, sn + 1));
                    (m, n) = (sm + 1, sn + 1);
                }
                None => return None,
            },
        }
    }
    let rest = &mask[m..];
    if rest.is_empty() || (!has_ext && rest == ['.']) {
        Some(n)
    } else {
        None
    }
}

/// Returns the number of name characters covered by `mask` (for highlighting)
/// if `name` starts with a match.
pub fn matches(name: &str, mask: &Mask) -> Option<usize> {
    find_match(name, mask, false).map(|(_, end)| end)
}

/// Where `mask` matches in `name`, as a char range `(start, end)`: at the
/// beginning only, or — with `anywhere` — at the first position it fits.
/// A name letter folded to two (`ß`, `æ`) is covered whole when the match
/// takes any part of it.
pub fn find_match(name: &str, mask: &Mask, anywhere: bool) -> Option<(usize, usize)> {
    let (folded, origin) = mask.fold_name(name);
    let has_ext = name.rfind('.').is_some_and(|i| i + 1 < name.len());
    let range = |start: usize, end: usize| {
        let s = origin.get(start).copied().unwrap_or(name.chars().count());
        let e = if end == 0 { 0 } else { origin[end - 1] + 1 };
        (s, e.max(s))
    };
    if !anywhere {
        return agree(&folded, 0, &mask.chars, has_ext).map(|end| range(0, end));
    }
    (0..folded.len().max(1)).find_map(|start| agree(&folded, start, &mask.chars, has_ext).map(|end| range(start, end)))
}

/// Index of the first name matching `mask`, searching from `start`
/// (inclusive) in the given direction and wrapping around.
pub fn find<'a>(names: impl Fn(usize) -> &'a str, len: usize, start: usize, mask: &Mask, forward: bool, anywhere: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let start = start.min(len - 1);
    (0..len).map(|k| if forward { (start + k) % len } else { (start + len - k) % len }).find(|&i| find_match(names(i), mask, anywhere).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact(typed: &str) -> Mask {
        prepare(typed, false)
    }

    #[test]
    fn prefix_and_wild() {
        let m = exact("rea");
        assert_eq!(matches("README.md", &m), Some(3));
        assert_eq!(matches("xreadme", &m), None);
        let m = exact("r/me");
        assert_eq!(matches("README.md", &m), Some(6));
        let m = exact("ž");
        assert_eq!(matches("Žluťoučký kůň", &m), Some(1));
    }

    /// Many `/` that almost fit: once minutes, now at once.
    #[test]
    fn many_wild_chars_stay_fast() {
        let start = std::time::Instant::now();
        let m = exact(&format!("{}b", "/a".repeat(12)));
        assert_eq!(find_match(&"a".repeat(40), &m, true), None);
        assert_eq!(find_match(&format!("x{}b", "a".repeat(40)), &m, true), Some((0, 42)), "a leading / takes the x");
        assert!(start.elapsed() < std::time::Duration::from_millis(100), "{:?}", start.elapsed());
        // `/` takes as little as it can: the highlight ends at the first fit
        assert_eq!(matches("banana split", &exact("b/n")), Some(3));
        assert_eq!(matches("banana", &exact("b/x")), None);
    }

    #[test]
    fn find_wraps() {
        let names = ["a", "bb", "ab", "c"];
        let m = exact("a");
        assert_eq!(find(|i| names[i], 4, 1, &m, true, false), Some(2));
        assert_eq!(find(|i| names[i], 4, 3, &m, true, false), Some(0));
        assert_eq!(find(|i| names[i], 4, 1, &m, false, false), Some(0));
    }

    #[test]
    fn anywhere_in_name() {
        let m = exact("port");
        assert_eq!(find_match("Report 2026.xlsx", &m, false), None);
        assert_eq!(find_match("Report 2026.xlsx", &m, true), Some((2, 6)));
        assert_eq!(find_match("Portrait.jpg", &m, true), Some((0, 4)), "the first position wins");
        // wildcards and case folding work anywhere too
        assert_eq!(find_match("Účetní ZPRÁVA 2026.docx", &exact("zpr/26"), true), Some((7, 18)));
        assert_eq!(find_match("žluťoučký kůň", &exact("kůň"), true), Some((10, 13)));
        let names = ["alfa", "beta", "gama"];
        assert_eq!(find(|i| names[i], 3, 0, &exact("et"), true, true), Some(1));
        assert_eq!(find(|i| names[i], 3, 0, &exact("et"), true, false), None);
    }

    #[test]
    fn letters_without_diacritics() {
        let names = ["vládce", "vladce", "vládče", "Vládče.txt"];
        for typed in ["vládce", "vladce", "vládče", "VLADCE"] {
            let m = prepare(typed, true);
            for n in names {
                assert_eq!(matches(n, &m), Some(6), "{typed} finds {n}");
            }
        }
        // exact: only the same letters (case still doesn't matter)
        let m = exact("vládce");
        assert_eq!(matches("Vládce", &m), Some(6));
        assert_eq!(matches("vladce", &m), None);
        assert_eq!(matches("vládče", &m), None);
        // German, Nordic, Polish, French …
        let loose = |typed: &str, name: &str| find_match(name, &prepare(typed, true), true);
        assert_eq!(loose("grusse", "Grüße.txt"), Some((0, 5)), "ß is ss, and covers one letter");
        assert_eq!(loose("gruß", "Grusse"), Some((0, 5)));
        assert_eq!(loose("o", "Ø"), Some((0, 1)));
        assert_eq!(loose("blabaer", "Blåbær"), Some((0, 6)));
        assert_eq!(loose("lodz", "Łódź"), Some((0, 4)));
        assert_eq!(loose("francais", "Français"), Some((0, 8)));
        assert_eq!(loose("zluto", "Žluťoučký kůň"), Some((0, 5)));
        assert_eq!(loose("kun", "Žluťoučký kůň"), Some((10, 13)));
        assert_eq!(loose("a/e", "æ"), Some((0, 1)));
        assert_eq!(loose("e", "æble"), Some((0, 1)), "inside a letter folded to two");
        assert_eq!(loose("x", "æble"), None);
    }
}
