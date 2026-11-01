//! Panel quick search: preparing what was typed is a port of the QS
//! functions in Salamander's `masks.cpp`; matching is written anew.
//!
//! The typed text matches name prefixes case-insensitively; `/`, `\` and `<`
//! stand for any sequence of characters (they cannot occur in file names).

/// A character for case-insensitive matching.
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Whether `c` stands for any characters in a quick search (`/`, `\`, `<`).
pub fn is_wild_char(c: char) -> bool {
    matches!(c, '/' | '\\' | '<')
}

/// Port of `PrepareQSMask`: collapse wild chars to a single `/`, drop a
/// trailing `/`, and case-fold.
pub fn prepare(src: &str) -> Vec<char> {
    let mut out = Vec::with_capacity(src.len());
    for c in src.chars() {
        if is_wild_char(c) {
            if out.last() != Some(&'/') {
                out.push('/');
            }
        } else {
            out.push(fold(c));
        }
    }
    if out.last() == Some(&'/') {
        out.pop();
    }
    out
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
pub fn matches(name: &str, mask: &[char]) -> Option<usize> {
    find_match(name, mask, false).map(|(_, end)| end)
}

/// Where `mask` matches in `name`, as a char range `(start, end)`: at the
/// beginning only, or — with `anywhere` — at the first position it fits.
pub fn find_match(name: &str, mask: &[char], anywhere: bool) -> Option<(usize, usize)> {
    let folded: Vec<char> = name.chars().map(fold).collect();
    let has_ext = name.rfind('.').is_some_and(|i| i + 1 < name.len());
    if !anywhere {
        return agree(&folded, 0, mask, has_ext).map(|end| (0, end));
    }
    (0..folded.len().max(1)).find_map(|start| agree(&folded, start, mask, has_ext).map(|end| (start, end)))
}

/// Index of the first name matching `mask`, searching from `start`
/// (inclusive) in the given direction and wrapping around.
pub fn find<'a>(names: impl Fn(usize) -> &'a str, len: usize, start: usize, mask: &[char], forward: bool, anywhere: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let start = start.min(len - 1);
    (0..len).map(|k| if forward { (start + k) % len } else { (start + len - k) % len }).find(|&i| find_match(names(i), mask, anywhere).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_and_wild() {
        let m = prepare("rea");
        assert_eq!(matches("README.md", &m), Some(3));
        assert_eq!(matches("xreadme", &m), None);
        let m = prepare("r/me");
        assert_eq!(matches("README.md", &m), Some(6));
        let m = prepare("ž");
        assert_eq!(matches("Žluťoučký kůň", &m), Some(1));
    }

    /// Many `/` that almost fit: once minutes, now at once.
    #[test]
    fn many_wild_chars_stay_fast() {
        let start = std::time::Instant::now();
        let m = prepare(&format!("{}b", "/a".repeat(12)));
        assert_eq!(find_match(&"a".repeat(40), &m, true), None);
        assert_eq!(find_match(&format!("x{}b", "a".repeat(40)), &m, true), Some((0, 42)), "a leading / takes the x");
        assert!(start.elapsed() < std::time::Duration::from_millis(100), "{:?}", start.elapsed());
        // `/` takes as little as it can: the highlight ends at the first fit
        assert_eq!(matches("banana split", &prepare("b/n")), Some(3));
        assert_eq!(matches("banana", &prepare("b/x")), None);
    }

    #[test]
    fn find_wraps() {
        let names = ["a", "bb", "ab", "c"];
        let m = prepare("a");
        assert_eq!(find(|i| names[i], 4, 1, &m, true, false), Some(2));
        assert_eq!(find(|i| names[i], 4, 3, &m, true, false), Some(0));
        assert_eq!(find(|i| names[i], 4, 1, &m, false, false), Some(0));
    }

    #[test]
    fn anywhere_in_name() {
        let m = prepare("port");
        assert_eq!(find_match("Report 2026.xlsx", &m, false), None);
        assert_eq!(find_match("Report 2026.xlsx", &m, true), Some((2, 6)));
        assert_eq!(find_match("Portrait.jpg", &m, true), Some((0, 4)), "the first position wins");
        // wildcards and case folding work anywhere too
        assert_eq!(find_match("Účetní ZPRÁVA 2026.docx", &prepare("zpr/26"), true), Some((7, 18)));
        assert_eq!(find_match("žluťoučký kůň", &prepare("kůň"), true), Some((10, 13)));
        let names = ["alfa", "beta", "gama"];
        assert_eq!(find(|i| names[i], 3, 0, &prepare("et"), true, true), Some(1));
        assert_eq!(find(|i| names[i], 3, 0, &prepare("et"), true, false), None);
    }
}
