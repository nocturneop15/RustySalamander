//! What differs inside a pair of lines: their words compared, not only the
//! lines. Marks the parts changed on each side, and tells how alike two
//! lines are (whether they are a pair at all).

use std::ops::Range;
use std::time::{Duration, Instant};

use similar::{capture_diff_slices_deadline, Algorithm, DiffOp};

/// How long comparing the words of two lines may take (a very long line):
/// after that a coarser answer.
const DEADLINE: Duration = Duration::from_millis(20);

/// Longer lines (the two together, in bytes) aren't taken apart into words
/// or characters (which takes time and memory before any deadline): they
/// are compared by their start and end only, and marked whole.
pub const MAX_LINES: usize = 20_000;

/// Lines at least this alike (the share of their text, spaces aside, that
/// both have) are a pair: a line changed. Less alike, one was removed and
/// the other added.
pub const ALIKE: f32 = 0.5;

/// The parts of a line: words (letters, digits, `_`), runs of spaces, and
/// each other character on its own.
fn tokens(s: &str) -> Vec<Range<usize>> {
    #[derive(PartialEq)]
    enum Kind {
        Word,
        Space,
        Other,
    }
    let kind = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Kind::Word
        } else if c.is_whitespace() {
            Kind::Space
        } else {
            Kind::Other
        }
    };
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut last = None;
    for (i, c) in s.char_indices() {
        let k = kind(c);
        let end = i + c.len_utf8();
        match out.last_mut() {
            Some(r) if last.as_ref() == Some(&k) && k != Kind::Other => r.end = end,
            _ => out.push(i..end),
        }
        last = Some(k);
    }
    out
}

/// What differs between lines `a` and `b`: the parts of each changed, in
/// order, neighbours joined (spaces changed too, but when `ignore_space`).
pub fn changed(a: &str, b: &str, ignore_space: bool) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    if a.len() + b.len() > MAX_LINES {
        // too long to take apart: the lines marked whole
        let whole = |s: &str| if s.is_empty() { vec![] } else { vec![0..s.len()] };
        return (whole(a), whole(b));
    }
    let keep = |s: &str, r: &Range<usize>| !ignore_space || !s[r.clone()].trim().is_empty();
    let (ta, tb): (Vec<_>, Vec<_>) = (tokens(a).into_iter().filter(|r| keep(a, r)).collect(), tokens(b).into_iter().filter(|r| keep(b, r)).collect());
    let (ka, kb): (Vec<&str>, Vec<&str>) = (ta.iter().map(|r| &a[r.clone()]).collect(), tb.iter().map(|r| &b[r.clone()]).collect());
    let ops = capture_diff_slices_deadline(Algorithm::Myers, &ka, &kb, Some(Instant::now() + DEADLINE));
    let (mut left, mut right) = (Vec::new(), Vec::new());
    let add = |out: &mut Vec<Range<usize>>, parts: &[Range<usize>]| {
        let (Some(first), Some(last)) = (parts.first(), parts.last()) else { return };
        match out.last_mut() {
            Some(r) if r.end == first.start => r.end = last.end,
            _ => out.push(first.start..last.end),
        }
    };
    for op in ops {
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete { old_index, old_len, .. } => add(&mut left, &ta[old_index..old_index + old_len]),
            DiffOp::Insert { new_index, new_len, .. } => add(&mut right, &tb[new_index..new_index + new_len]),
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                // a word for a word alike (a code changed in a few places): the
                // characters changed in it, not the whole word
                let (wa, wb) = (&ta[old_index], &tb[new_index]);
                if let (1, 1, Some((l, r))) = (old_len, new_len, inside(&a[wa.clone()], &b[wb.clone()])) {
                    left.extend(l.into_iter().map(|x| wa.start + x.start..wa.start + x.end));
                    right.extend(r.into_iter().map(|x| wb.start + x.start..wb.start + x.end));
                    continue;
                }
                add(&mut left, &ta[old_index..old_index + old_len]);
                add(&mut right, &tb[new_index..new_index + new_len]);
            }
        }
    }
    (left, right)
}

/// The shortest run of characters in common that counts towards two lines
/// being alike: a letter here and there is common to any two lines.
const RUN: usize = 3;

/// How alike lines `a` and `b` are, 0 to 1: the share of their characters
/// (spaces aside, upper and lower case alike) in runs of at least [`RUN`]
/// both have, in the same order (a long word changed inside, a code or a
/// hash, stays alike). Two lines the same but for spaces and case are alike.
pub fn alike(a: &str, b: &str) -> f32 {
    if a.len() + b.len() > MAX_LINES {
        return alike_ends(a, b);
    }
    let chars = |s: &str| -> Vec<char> { s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect() };
    let (ca, cb) = (chars(a), chars(b));
    if ca == cb {
        return 1.0;
    }
    let run = RUN.min(ca.len().min(cb.len())).max(1);
    let ops = capture_diff_slices_deadline(Algorithm::Myers, &ca, &cb, Some(Instant::now() + DEADLINE));
    let same: usize = ops
        .iter()
        .map(|op| match *op {
            DiffOp::Equal { len, .. } if len >= run => len,
            _ => 0,
        })
        .sum();
    (2 * same) as f32 / (ca.len() + cb.len()) as f32
}

/// How alike two long lines are by what they share at the start and the
/// end (bytes), taking nothing apart: a line changed in one place stays
/// alike, two other lines don't.
fn alike_ends(a: &str, b: &str) -> f32 {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let start = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let end = a[start..].iter().rev().zip(b[start..].iter().rev()).take_while(|(x, y)| x == y).count();
    (2 * (start + end)) as f32 / (a.len() + b.len()).max(1) as f32
}

/// The characters changed inside word `a` replaced by word `b` (byte
/// ranges in each), when the two are alike (a code changed in a few
/// places); `None` when they aren't: the whole words are marked then.
fn inside(a: &str, b: &str) -> Option<(Vec<Range<usize>>, Vec<Range<usize>>)> {
    if alike(a, b) < ALIKE {
        return None;
    }
    let (ia, ib): (Vec<(usize, char)>, Vec<(usize, char)>) = (a.char_indices().collect(), b.char_indices().collect());
    let (ca, cb): (Vec<char>, Vec<char>) = (ia.iter().map(|p| p.1).collect(), ib.iter().map(|p| p.1).collect());
    // the bytes of characters `from..to` of a word
    let bytes = |idx: &[(usize, char)], len: usize, from: usize, to: usize| idx.get(from).map_or(len, |p| p.0)..idx.get(to).map_or(len, |p| p.0);
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for op in capture_diff_slices_deadline(Algorithm::Myers, &ca, &cb, Some(Instant::now() + DEADLINE)) {
        let (_, old, new) = op.as_tag_tuple();
        if op.tag() != similar::DiffTag::Equal {
            if !old.is_empty() {
                left.push(bytes(&ia, a.len(), old.start, old.end));
            }
            if !new.is_empty() {
                right.push(bytes(&ib, b.len(), new.start, new.end));
            }
        }
    }
    Some((left, right))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_spaces_and_signs() {
        let s = "let x  = foo(a_b, 12);";
        let parts: Vec<&str> = tokens(s).into_iter().map(|r| &s[r]).collect();
        assert_eq!(parts, ["let", " ", "x", "  ", "=", " ", "foo", "(", "a_b", ",", " ", "12", ")", ";"]);
        assert_eq!(tokens("řádek 🦎"), [0..7, 7..8, 8..12], "by characters, not bytes");
    }

    #[test]
    fn the_words_changed_are_marked() {
        let (a, b) = ("let x = foo(1);", "let y = foo(1, 2);");
        let (l, r) = changed(a, b, false);
        assert_eq!(l.iter().map(|r| &a[r.clone()]).collect::<Vec<_>>(), ["x"]);
        assert_eq!(r.iter().map(|r| &b[r.clone()]).collect::<Vec<_>>(), ["y", ", 2"], "neighbours joined");
        // spaces: marked, unless ignored
        assert_eq!(changed("a  b", "a b", false), (vec![2..3], vec![]), "a space more on the left");
        assert_eq!(changed("a  b", "a b", true), (vec![], vec![]));
        assert_eq!(changed("start", "START", false), (vec![0..5], vec![0..5]), "case counts here");
        // a long code changed inside: only the characters changed
        let (a, b) = ("<u>EncX D82F2CA16164B152B7C50665FA97CC5BA08</u>", "<u>EncX D82F2CA16164B152B7C50665FAokokooo97CC5BA08</u>");
        let (l, r) = changed(a, b, false);
        assert!(l.is_empty(), "nothing taken away: {l:?}");
        assert_eq!(r.iter().map(|r| &b[r.clone()]).collect::<Vec<_>>(), ["okokooo"]);
    }

    #[test]
    fn how_alike() {
        assert_eq!(alike("", ""), 1.0);
        assert_eq!(alike("dva", "DVA"), 1.0, "case aside");
        assert_eq!(alike("a  b", "a b"), 1.0, "spaces aside");
        assert!(alike("let x = foo(1);", "let y = foo(1);") >= ALIKE);
        assert!(alike("fn main() {", "// a comment here") < ALIKE);
        assert_eq!(alike("abc", ""), 0.0);
        assert_eq!(alike("f", "F"), 1.0);
        // a long code changed inside stays alike; unrelated text with
        // letters in common doesn't
        assert!(alike("<username>EncX D82F2CA16164B152B7C50665FA97CC5BA08</username>", "<username>EncX D82F2CA16164B152B7C50665FAokokooo97CC5BA08</username>") >= ALIKE);
        assert!(alike("the quick brown fox jumps", "a lazy dog sleeps in the sun") < ALIKE);
    }

    /// Lines longer than MAX_LINES together aren't taken apart: compared by
    /// their ends (at once, whatever their length), marked whole.
    #[test]
    fn very_long_lines() {
        let long = "ab".repeat(2_000_000);
        let changed_inside = format!("{}XYZ{}", &long[..2_000_000], &long[2_000_000..]);
        let started = Instant::now();
        assert!(alike(&long, &changed_inside) >= ALIKE, "changed in one place: alike");
        assert!(alike(&long, &"cd".repeat(2_000_000)) < ALIKE);
        assert_eq!(changed(&long, &changed_inside, false), (vec![0..long.len()], vec![0..changed_inside.len()]), "marked whole");
        assert!(started.elapsed() < Duration::from_secs(1), "at once: {:?}", started.elapsed());
    }
}
