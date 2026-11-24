//! Showing only the changes: the parts of the texts that are the same are
//! hidden, but for a few lines around each difference, each part in a line
//! of its own ("+ Same lines: 120") that shows it again when clicked; a part
//! shown keeps its line ("− Same lines: 120"), which hides it again.

use std::collections::BTreeSet;

use crate::diff::{Row, Tag};

/// The same lines left in sight before and after each difference.
pub const CONTEXT: usize = 3;

/// Fewer same lines than this are shown, not hidden: a line saying "1 line"
/// takes the room of the line itself.
const MIN_HIDDEN: usize = 2;

/// A line of the view: a row of the texts, a part hidden (rows
/// `start..end`), or the line above such a part shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Row(usize),
    Fold { start: usize, end: usize },
    Opened { start: usize, end: usize },
}

impl Line {
    /// The row the line shows, or the first one it hides.
    pub fn first_row(self) -> usize {
        match self {
            Line::Row(r) => r,
            Line::Fold { start, .. } | Line::Opened { start, .. } => start,
        }
    }
}

/// The lines of the view of `rows` with the same parts hidden, but those
/// opened (by their first row, `opened`).
pub fn lines(rows: &[Row], opened: &BTreeSet<usize>) -> Vec<Line> {
    let n = rows.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if rows[i].tag != Tag::Same {
            out.push(Line::Row(i));
            i += 1;
            continue;
        }
        let a = i;
        while i < n && rows[i].tag == Tag::Same {
            i += 1;
        }
        let b = i;
        // a few lines kept beside a difference; none at the texts' start or end
        let start = if a == 0 { 0 } else { a + CONTEXT };
        let end = if b == n { n } else { b.saturating_sub(CONTEXT) };
        if end >= start + MIN_HIDDEN {
            out.extend((a..start).map(Line::Row));
            if opened.contains(&start) {
                out.push(Line::Opened { start, end });
                out.extend((start..end).map(Line::Row));
            } else {
                out.push(Line::Fold { start, end });
            }
            out.extend((end..b).map(Line::Row));
        } else {
            out.extend((a..b).map(Line::Row));
        }
    }
    out
}

/// Where row `row` is among `lines` (in the rows' order): its line, or the
/// part hiding it (a row shown comes after the line above its part).
pub fn index_of(lines: &[Line], row: usize) -> usize {
    lines.partition_point(|l| l.first_row() <= row).saturating_sub(1)
}

/// The first row of the part hiding `row`, if one does.
pub fn hiding(lines: &[Line], row: usize) -> Option<usize> {
    match lines.get(index_of(lines, row)) {
        Some(&Line::Fold { start, end }) if (start..end).contains(&row) => Some(start),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(tags: &str) -> Vec<Row> {
        tags.chars()
            .map(|c| Row {
                left: None,
                right: None,
                tag: match c {
                    '=' => Tag::Same,
                    '~' => Tag::Changed,
                    '-' => Tag::Removed,
                    _ => Tag::Added,
                },
            })
            .collect()
    }

    #[test]
    fn same_parts_are_hidden_but_beside_a_difference() {
        // 10 same, a change, 10 same, an addition, 3 same
        let r = rows("==========~==========+===");
        let l = lines(&r, &BTreeSet::new());
        let fold = |start, end| Line::Fold { start, end };
        assert_eq!(l[0], fold(0, 7), "the start: no lines kept above");
        assert_eq!(&l[1..8], &(7..14).map(Line::Row).collect::<Vec<_>>()[..], "3 before, the change, 3 after");
        assert_eq!(l[8], fold(14, 18));
        assert_eq!(&l[9..], &(18..25).map(Line::Row).collect::<Vec<_>>()[..], "3 before, the addition, the 3 at the end: too few to hide");
        assert_eq!(index_of(&l, 3), 0);
        assert_eq!(hiding(&l, 3), Some(0));
        assert_eq!(index_of(&l, 10), 4);
        assert_eq!(hiding(&l, 10), None, "shown");
        assert_eq!(hiding(&l, 17), Some(14));
        assert_eq!(hiding(&l, 24), None);
        // opened: shown whole, below the line that hides it again
        let l = lines(&r, &BTreeSet::from([14]));
        assert_eq!(l.len(), 1 + 19, "only the first part still hidden");
        assert_eq!(l[8], Line::Opened { start: 14, end: 18 });
        assert_eq!(l[9], Line::Row(14));
        assert_eq!(hiding(&l, 17), None);
        assert_eq!(index_of(&l, 14), 9, "the row, not the line above it");
    }

    #[test]
    fn short_parts_and_all_same() {
        assert_eq!(lines(&rows("~=======~"), &BTreeSet::new()).len(), 9, "7 same: 3 + 1 + 3, nothing worth hiding");
        assert_eq!(lines(&rows("~========~"), &BTreeSet::new())[4], Line::Fold { start: 4, end: 6 });
        assert_eq!(lines(&rows("====="), &BTreeSet::new()), [Line::Fold { start: 0, end: 5 }], "the same texts: one part");
        assert!(lines(&[], &BTreeSet::new()).is_empty());
    }
}
