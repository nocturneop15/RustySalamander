//! Finding text in the viewer: every match of what the user typed, line by
//! line, in any case or matching case, with `*` and `?` as wildcards. No
//! window here, so it can be tested.

use regex::{Regex, RegexBuilder};

/// At most this many matches are found (a common letter in a 32 MB file).
pub const MAX_HITS: usize = 100_000;

/// A match: line `line`, bytes `start..end` of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub line: usize,
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
        // Unicode case folding: `ŽLUŤ` finds `žluť`, `ΣΟΦΊΑ` finds `σοφία`
        RegexBuilder::new(&pattern).case_insensitive(!self.match_case).build().ok()
    }
}

/// Every match of `query` in `lines`, in order; whether there were more than
/// [`MAX_HITS`]. Empty matches (a lone `*`) don't count.
pub fn find_all(lines: &[String], query: &Query) -> (Vec<Hit>, bool) {
    let Some(re) = query.pattern() else { return (Vec::new(), false) };
    let mut hits = Vec::new();
    for (line, text) in lines.iter().enumerate() {
        for m in re.find_iter(text).filter(|m| m.start() < m.end()) {
            if hits.len() == MAX_HITS {
                return (hits, true);
            }
            hits.push(Hit { line, start: m.start(), end: m.end() });
        }
    }
    (hits, false)
}

/// The matches on line `line` (`hits` in order).
pub fn on_line(hits: &[Hit], line: usize) -> &[Hit] {
    let from = hits.partition_point(|h| h.line < line);
    let to = from + hits[from..].partition_point(|h| h.line == line);
    &hits[from..to]
}

/// The first match on line `line` or below, else the first one: where a new
/// search starts.
pub fn first_from(hits: &[Hit], line: usize) -> Option<usize> {
    if hits.is_empty() {
        return None;
    }
    let i = hits.partition_point(|h| h.line < line);
    Some(if i < hits.len() { i } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hostile text in Find: found literally (or by its wildcards), never a
    /// panic, never a pattern of the person's making (it is escaped).
    #[test]
    fn hostile_find_text() {
        let text: Vec<String> = sal_core::hostile::inputs().into_iter().filter(|s| s.len() < 2_000).collect();
        for s in sal_core::hostile::inputs() {
            for (case, wild) in [(false, false), (true, false), (false, true)] {
                let _ = find_all(&text, &q(&s, case, wild));
            }
        }
        let (hits, _) = find_all(&lines("a(b)[c]{d}.*+?^$|\\"), &q("(b)[c]{d}.*+?^$|\\", true, false));
        assert_eq!(hits.len(), 1, "regular expression characters are found as typed");
        assert!(find_all(&lines("x & calc"), &q("& calc", false, false)).0.len() == 1);
    }

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    fn q(text: &str, match_case: bool, wildcards: bool) -> Query {
        Query { text: text.into(), match_case, wildcards }
    }

    fn found(lines: &[String], query: &Query) -> Vec<String> {
        find_all(lines, query).0.iter().map(|h| lines[h.line][h.start..h.end].to_owned()).collect()
    }

    #[test]
    fn any_case_or_matching_case() {
        let l = lines("Žluťoučký kůň\nžluťoučký KŮŇ úpěl\nnic");
        assert_eq!(found(&l, &q("ŽLUŤ", false, false)), ["Žluť", "žluť"], "in any case, in any language");
        assert_eq!(found(&l, &q("Žluť", true, false)), ["Žluť"], "matching case");
        let (hits, more) = find_all(&l, &q("kůň", false, false));
        assert_eq!((hits, more), (vec![Hit { line: 0, start: 14, end: 19 }, Hit { line: 1, start: 14, end: 19 }], false));
        assert!(find_all(&l, &q("", false, false)).0.is_empty(), "nothing to look for");
        assert!(find_all(&l, &q("xyz", false, false)).0.is_empty());
    }

    #[test]
    fn wildcards_or_plain_text() {
        let l = lines("a.b a+b axb\nfile1.txt file22.txt (x)*");
        assert_eq!(found(&l, &q("a?b", false, true)), ["a.b", "a+b", "axb"], "? is one character");
        assert_eq!(found(&l, &q("a?b", false, false)), Vec::<String>::new(), "without wildcards ? is a question mark");
        assert_eq!(found(&l, &q("file*.txt", false, true)), ["file1.txt", "file22.txt"], "* is the shortest run");
        assert_eq!(found(&l, &q("(x)*", false, false)), ["(x)*"], "regex characters are plain text");
        assert_eq!(found(&l, &q("(x)", false, true)), ["(x)"]);
        assert!(find_all(&l, &q("*", false, true)).0.is_empty(), "a lone * matches nothing");
    }

    #[test]
    fn lines_and_where_to_start() {
        let l = lines("ab\nx\nab ab\nx\nab");
        let (hits, _) = find_all(&l, &q("ab", false, false));
        assert_eq!(hits.len(), 4);
        assert_eq!(on_line(&hits, 2).len(), 2);
        assert!(on_line(&hits, 1).is_empty());
        assert_eq!(on_line(&hits, 4), &[Hit { line: 4, start: 0, end: 2 }]);
        assert_eq!(first_from(&hits, 1), Some(1), "from the line shown on top");
        assert_eq!(first_from(&hits, 3), Some(3));
        assert_eq!(first_from(&hits, 9), Some(0), "below the last: from the start");
        assert_eq!(first_from(&[], 0), None);
    }

    #[test]
    fn at_most_max_hits() {
        let l = vec!["a".repeat(MAX_HITS + 5)];
        let (hits, more) = find_all(&l, &q("a", false, false));
        assert_eq!((hits.len(), more), (MAX_HITS, true));
    }
}
