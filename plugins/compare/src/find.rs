//! Finding text in the compared files: every match of what the user typed,
//! row by row, the left side before the right, in any case or matching
//! case, with `*` and `?` as wildcards (as the viewer finds). No window
//! here, so it can be tested.

use regex::{Regex, RegexBuilder};

use crate::diff::Row;

/// At most this many matches are found (a common letter in two big files).
pub const MAX_HITS: usize = 100_000;

/// A match: row `row`, on the right side or the left, bytes `start..end`
/// of that side's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub row: usize,
    pub right: bool,
    pub start: usize,
    pub end: usize,
}

/// What to look for and how.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    pub text: String,
    /// Upper and lower case must match (`Žluť` doesn't find `žluť`).
    pub match_case: bool,
    /// `*` stands for any characters, `?` for one.
    pub wildcards: bool,
}

impl Query {
    /// The query as a pattern; `None` when there is nothing to look for.
    pub fn pattern(&self) -> Option<Regex> {
        if self.text.is_empty() {
            return None;
        }
        let pattern = if self.wildcards {
            // the shortest run: `a*b` ends at the first b, as a find should
            let mut p = String::new();
            for c in self.text.chars() {
                match c {
                    '*' => p.push_str(".*?"),
                    '?' => p.push('.'),
                    c => p.push_str(&regex::escape(c.encode_utf8(&mut [0; 4]))),
                }
            }
            p
        } else {
            regex::escape(&self.text)
        };
        // Unicode case folding: `ŽLUŤ` finds `žluť`
        RegexBuilder::new(&pattern).case_insensitive(!self.match_case).build().ok()
    }
}

/// Every match of `query` in `rows`, in order (in a row, the left side's
/// first); whether there were more than [`MAX_HITS`]. Empty matches (a
/// lone `*`) don't count.
#[cfg(test)]
pub fn find_all(rows: &[Row], query: &Query) -> (Vec<Hit>, bool) {
    let never = std::sync::atomic::AtomicBool::new(false);
    find_until(rows, query, &never).unwrap_or_default()
}

/// [`find_all`], given up (`None`) as soon as `stop` is set: a search
/// runs beside the window, and one for what was typed before is no longer
/// wanted.
pub fn find_until(rows: &[Row], query: &Query, stop: &std::sync::atomic::AtomicBool) -> Option<(Vec<Hit>, bool)> {
    use std::sync::atomic::Ordering;
    let Some(re) = query.pattern() else { return Some((Vec::new(), false)) };
    let mut hits = Vec::new();
    for (row, r) in rows.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        for (right, side) in [(false, &r.left), (true, &r.right)] {
            let Some((_, text)) = side else { continue };
            for m in re.find_iter(text).filter(|m| m.start() < m.end()) {
                if hits.len() == MAX_HITS {
                    return Some((hits, true));
                }
                if hits.len() % 4096 == 4095 && stop.load(Ordering::Relaxed) {
                    return None;
                }
                hits.push(Hit { row, right, start: m.start(), end: m.end() });
            }
        }
    }
    Some((hits, false))
}

/// The matches on one side of row `row` (`hits` in order).
pub fn on_side(hits: &[Hit], row: usize, right: bool) -> impl Iterator<Item = &Hit> {
    let from = hits.partition_point(|h| h.row < row);
    hits[from..].iter().take_while(move |h| h.row == row).filter(move |h| h.right == right)
}

/// The first match in row `row` or below, else the first one: where a new
/// search starts.
pub fn first_from(hits: &[Hit], row: usize) -> Option<usize> {
    if hits.is_empty() {
        return None;
    }
    let i = hits.partition_point(|h| h.row < row);
    Some(if i < hits.len() { i } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{text, Outcome};

    fn rows(a: &str, b: &str) -> Vec<Row> {
        let Outcome::Text { rows, .. } = text(a, b, false) else { panic!("text") };
        rows.to_vec()
    }

    fn q(text: &str, match_case: bool, wildcards: bool) -> Query {
        Query { text: text.into(), match_case, wildcards }
    }

    /// What the hits cover, as (row, side, text).
    fn found(rows: &[Row], query: &Query) -> Vec<(usize, bool, String)> {
        find_all(rows, query)
            .0
            .iter()
            .map(|h| {
                let side = if h.right { &rows[h.row].right } else { &rows[h.row].left };
                (h.row, h.right, side.as_ref().unwrap().1[h.start..h.end].to_owned())
            })
            .collect()
    }

    #[test]
    fn both_sides_in_order() {
        // rows: Same, Changed, Removed (left only), Added (right only)
        let r = rows("kůň\nŽluťoučký kůň\njen vlevo kůň\nkonec\n", "kůň\nžluťoučký KŮŇ\nkonec\nnový kůň\n");
        let all = found(&r, &q("kůň", false, false));
        assert_eq!(
            all,
            [
                (0, false, "kůň".to_owned()),
                (0, true, "kůň".to_owned()),
                (1, false, "kůň".to_owned()),
                (1, true, "KŮŇ".to_owned()),
                (2, false, "kůň".to_owned()),
                (4, true, "kůň".to_owned()),
            ]
        );
        assert_eq!(found(&r, &q("kůň", true, false)).len(), 5, "match case");
        assert_eq!(found(&r, &q("ž*ý", false, true)), [(1, false, "Žluťoučký".to_owned()), (1, true, "žluťoučký".to_owned())]);
        assert_eq!(found(&r, &q("k?ň", false, true)).len(), 6);
        assert!(found(&r, &q("*", false, true)).is_empty(), "a lone * matches nothing");
        assert!(found(&r, &q("", false, false)).is_empty(), "nothing to look for");
        // hostile text is found literally
        let r = rows("a(b)[c]{d}.*+?^$|\\\n", "x\n");
        assert_eq!(found(&r, &q("(b)[c]{d}.*+?^$|\\", true, false)).len(), 1);
    }

    #[test]
    fn where_a_search_starts_and_sides() {
        let r = rows("a\nb\na\n", "a\nb\na\n");
        let (hits, more) = find_all(&r, &q("a", false, false));
        assert!(!more);
        assert_eq!(hits.len(), 4);
        assert_eq!(first_from(&hits, 1), Some(2), "the next row with a match");
        assert_eq!(first_from(&hits, 3), Some(0), "round the end");
        assert_eq!(on_side(&hits, 2, true).count(), 1);
        assert_eq!(on_side(&hits, 1, false).count(), 0);
        // no longer wanted: given up
        let stop = std::sync::atomic::AtomicBool::new(true);
        assert!(find_until(&r, &q("a", false, false), &stop).is_none(), "given up");
        let many = "a".repeat(1000) + "\n";
        let r = rows(&many.repeat(120), "");
        let (hits, more) = find_all(&r, &q("a", false, false));
        assert!(more && hits.len() == MAX_HITS);
    }
}
