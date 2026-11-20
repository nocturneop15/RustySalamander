//! Selecting text in the Text view: a selection runs from where it was
//! started (the anchor) to the caret, both a line and a byte offset in it.
//! The keys move the caret as in an editor; the text is the lines between.

/// A place in the text: a line and a byte offset in it (on a character's edge).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPos {
    pub line: usize,
    pub col: usize,
}

impl TextPos {
    pub fn new(line: usize, col: usize) -> TextPos {
        TextPos { line, col }
    }
}

/// Where a key moves the caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    WordLeft,
    WordRight,
    Up,
    Down,
    /// Up / down by this many lines.
    PageUp(usize),
    PageDown(usize),
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
}

/// The selection: from `anchor` to `caret` (either may come first).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: TextPos,
    pub caret: TextPos,
    /// The column (in characters) Up and Down keep to.
    goal: Option<usize>,
}

impl Selection {
    /// Nothing selected, the caret at `at`.
    pub fn at(at: TextPos) -> Selection {
        Selection { anchor: at, caret: at, goal: None }
    }

    /// All of `lines`.
    pub fn all(lines: &[String]) -> Selection {
        let end = lines.len().saturating_sub(1);
        Selection { anchor: TextPos::default(), caret: TextPos::new(end, lines.get(end).map_or(0, |l| l.len())), goal: None }
    }

    /// Start and end, in order.
    pub fn range(&self) -> (TextPos, TextPos) {
        if self.anchor <= self.caret {
            (self.anchor, self.caret)
        } else {
            (self.caret, self.anchor)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.caret
    }

    /// The part of line `line` (`len` bytes) selected, as a byte range, and
    /// whether the selection goes on past its end (its line break too).
    pub fn on_line(&self, line: usize, len: usize) -> Option<(usize, usize, bool)> {
        let (a, b) = self.range();
        if self.is_empty() || line < a.line || line > b.line {
            return None;
        }
        let from = if line == a.line { a.col.min(len) } else { 0 };
        let (to, on) = if line == b.line { (b.col.min(len), false) } else { (len, true) };
        Some((from, to, on))
    }

    /// The selected text, lines joined by `eol`.
    pub fn text(&self, lines: &[String], eol: &str) -> String {
        let (a, b) = self.range();
        let mut out = String::new();
        for line in a.line..=b.line.min(lines.len().saturating_sub(1)) {
            let Some(l) = lines.get(line) else { break };
            let Some((from, to, on)) = self.on_line(line, l.len()) else { continue };
            out.push_str(&l[from..to]);
            if on {
                out.push_str(eol);
            }
        }
        out
    }

    /// Keep both ends inside `lines` (which may have changed), on
    /// characters' edges.
    pub fn clamp(&mut self, lines: &[String]) {
        for p in [&mut self.anchor, &mut self.caret] {
            *p = clamp(*p, lines);
        }
    }

    /// Move the caret; `extend` keeps the anchor (Shift), otherwise the
    /// selection is just the caret.
    pub fn go(&mut self, lines: &[String], to: Move, extend: bool) {
        let caret = clamp(self.caret, lines);
        let vertical = matches!(to, Move::Up | Move::Down | Move::PageUp(_) | Move::PageDown(_));
        let goal = self.goal.unwrap_or_else(|| chars_before(lines, caret));
        self.caret = step(lines, caret, to, goal);
        self.goal = vertical.then_some(goal);
        if !extend {
            self.anchor = self.caret;
        }
    }

    /// The caret to `at`; `extend` keeps the anchor (Shift+click, a drag).
    pub fn put(&mut self, at: TextPos, extend: bool) {
        self.caret = at;
        self.goal = None;
        if !extend {
            self.anchor = at;
        }
    }

    /// The word at `at` (a double-click), or the character there.
    pub fn word_at(lines: &[String], at: TextPos) -> Selection {
        let at = clamp(at, lines);
        let Some(l) = lines.get(at.line) else { return Selection::at(at) };
        let kind = |c: char| if is_word(c) { 0 } else if c.is_whitespace() { 1 } else { 2 };
        let Some(here) = l[at.col..].chars().next().or_else(|| l[..at.col].chars().next_back()) else { return Selection::at(at) };
        let k = kind(here);
        let mut from = at.col;
        while let Some(c) = l[..from].chars().next_back().filter(|&c| kind(c) == k) {
            from -= c.len_utf8();
        }
        let mut to = at.col;
        while let Some(c) = l[to..].chars().next().filter(|&c| kind(c) == k) {
            to += c.len_utf8();
        }
        Selection { anchor: TextPos::new(at.line, from), caret: TextPos::new(at.line, to), goal: None }
    }
}

/// Letters, digits and `_` make words.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `p` inside `lines`, on a character's edge.
fn clamp(p: TextPos, lines: &[String]) -> TextPos {
    let Some(last) = lines.len().checked_sub(1) else { return TextPos::default() };
    let line = p.line.min(last);
    let l = &lines[line];
    let mut col = p.col.min(l.len());
    while !l.is_char_boundary(col) {
        col -= 1;
    }
    TextPos::new(line, col)
}

/// How many characters come before `p` on its line.
fn chars_before(lines: &[String], p: TextPos) -> usize {
    lines.get(p.line).map_or(0, |l| l[..p.col].chars().count())
}

/// The byte offset of character `n` of line `line` (its end when shorter).
fn col_of(lines: &[String], line: usize, n: usize) -> usize {
    let l = &lines[line];
    l.char_indices().nth(n).map_or(l.len(), |(b, _)| b)
}

/// Where `to` takes the caret from `p`; `goal`: the column Up and Down keep to.
fn step(lines: &[String], p: TextPos, to: Move, goal: usize) -> TextPos {
    let Some(last) = lines.len().checked_sub(1) else { return TextPos::default() };
    let l = &lines[p.line];
    let vertical = |line: usize| TextPos::new(line, col_of(lines, line, goal));
    match to {
        Move::Left => match l[..p.col].chars().next_back() {
            Some(c) => TextPos::new(p.line, p.col - c.len_utf8()),
            None if p.line > 0 => TextPos::new(p.line - 1, lines[p.line - 1].len()),
            None => p,
        },
        Move::Right => match l[p.col..].chars().next() {
            Some(c) => TextPos::new(p.line, p.col + c.len_utf8()),
            None if p.line < last => TextPos::new(p.line + 1, 0),
            None => p,
        },
        // to the start of this word or the one before; at the line's start, the line before's end
        Move::WordLeft => {
            if p.col == 0 {
                return step(lines, p, Move::Left, goal);
            }
            let mut col = p.col;
            while let Some(c) = l[..col].chars().next_back().filter(|&c| !is_word(c)) {
                col -= c.len_utf8();
            }
            while let Some(c) = l[..col].chars().next_back().filter(|&c| is_word(c)) {
                col -= c.len_utf8();
            }
            TextPos::new(p.line, col)
        }
        // past this word and the spaces after it, to the next one; at the line's end, the next line
        Move::WordRight => {
            if p.col == l.len() {
                return step(lines, p, Move::Right, goal);
            }
            let mut col = p.col;
            let word = l[col..].chars().next().is_some_and(is_word);
            while let Some(c) = l[col..].chars().next().filter(|&c| if word { is_word(c) } else { !is_word(c) && !c.is_whitespace() }) {
                col += c.len_utf8();
            }
            while let Some(c) = l[col..].chars().next().filter(|c| c.is_whitespace()) {
                col += c.len_utf8();
            }
            TextPos::new(p.line, col)
        }
        Move::Up if p.line > 0 => vertical(p.line - 1),
        Move::Up => TextPos::new(0, 0),
        Move::Down if p.line < last => vertical(p.line + 1),
        Move::Down => TextPos::new(last, lines[last].len()),
        Move::PageUp(n) if p.line > 0 => vertical(p.line.saturating_sub(n.max(1))),
        Move::PageUp(_) => TextPos::new(0, 0),
        Move::PageDown(n) if p.line < last => vertical((p.line + n.max(1)).min(last)),
        Move::PageDown(_) => TextPos::new(last, lines[last].len()),
        Move::LineStart => TextPos::new(p.line, 0),
        Move::LineEnd => TextPos::new(p.line, l.len()),
        Move::DocStart => TextPos::new(0, 0),
        Move::DocEnd => TextPos::new(last, lines[last].len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &[&str]) -> Vec<String> {
        s.iter().map(|l| l.to_string()).collect()
    }

    /// The selection's text across lines, and of a part of one line.
    #[test]
    fn text_of_a_selection() {
        let l = lines(&["Kůň skáče", "", "přes plot"]);
        // after "Kůň " up to "přes"
        let mut s = Selection::at(TextPos::new(0, "Kůň ".len()));
        s.put(TextPos::new(2, "přes".len()), true);
        assert_eq!(s.text(&l, "\r\n"), "skáče\r\n\r\npřes");
        assert_eq!(s.on_line(0, l[0].len()), Some(("Kůň ".len(), l[0].len(), true)));
        assert_eq!(s.on_line(1, 0), Some((0, 0, true)), "an empty line between: its line break");
        assert_eq!(s.on_line(2, l[2].len()), Some((0, "přes".len(), false)));
        // backwards the same
        let back = Selection { anchor: s.caret, caret: s.anchor, goal: None };
        assert_eq!(back.text(&l, "\n"), "skáče\n\npřes");
        assert_eq!(Selection::all(&l).text(&l, "\n"), "Kůň skáče\n\npřes plot");
        assert!(Selection::at(TextPos::new(1, 0)).on_line(1, 0).is_none(), "nothing selected");
    }

    /// Shift+arrows go a character (round the line ends), a line keeping
    /// the column, Home/End the line, Ctrl+Home/End the text.
    #[test]
    fn keys_move_the_caret() {
        let l = lines(&["Kůň", "a", "dlouhý řádek"]);
        let mut s = Selection::at(TextPos::new(0, 0));
        s.go(&l, Move::Right, true);
        s.go(&l, Move::Right, true);
        assert_eq!(s.text(&l, "\n"), "Ků");
        s.go(&l, Move::Right, true);
        s.go(&l, Move::Right, true);
        assert_eq!(s.caret, TextPos::new(1, 0), "round the line's end");
        s.go(&l, Move::Left, true);
        assert_eq!(s.caret, TextPos::new(0, "Kůň".len()));
        // the column is kept through a shorter line
        let mut s = Selection::at(TextPos::new(0, "Kůň".len()));
        s.go(&l, Move::Down, true);
        assert_eq!(s.caret, TextPos::new(1, 1));
        s.go(&l, Move::Down, true);
        assert_eq!(s.caret, TextPos::new(2, "dlo".len()));
        s.go(&l, Move::Down, true);
        assert_eq!(s.caret, TextPos::new(2, l[2].len()), "down from the last line: its end");
        s.go(&l, Move::LineStart, true);
        assert_eq!(s.caret, TextPos::new(2, 0));
        s.go(&l, Move::DocStart, true);
        assert_eq!(s.text(&l, "\n"), "Kůň", "back to the start: what is before the anchor");
        s.go(&l, Move::DocEnd, false);
        assert!(s.is_empty() && s.caret == TextPos::new(2, l[2].len()), "without Shift nothing stays selected");
        s.go(&l, Move::PageUp(10), true);
        assert_eq!(s.caret.line, 0);
    }

    /// Ctrl+Shift+arrows go by words; a double-click takes a word.
    #[test]
    fn words() {
        let l = lines(&["let x = foo_bar(1);", "next"]);
        let mut s = Selection::at(TextPos::new(0, 0));
        s.go(&l, Move::WordRight, true);
        assert_eq!(s.text(&l, "\n"), "let ");
        s.go(&l, Move::WordRight, true);
        s.go(&l, Move::WordRight, true);
        assert_eq!(s.text(&l, "\n"), "let x = ");
        s.go(&l, Move::WordRight, true);
        assert_eq!(s.text(&l, "\n"), "let x = foo_bar");
        s.go(&l, Move::WordLeft, true);
        assert_eq!(s.text(&l, "\n"), "let x = ");
        s.go(&l, Move::LineEnd, false);
        s.go(&l, Move::WordRight, true);
        assert_eq!(s.caret, TextPos::new(1, 0), "at the end of a line: the next one");
        let w = Selection::word_at(&l, TextPos::new(0, 10));
        assert_eq!(w.text(&l, "\n"), "foo_bar");
        assert_eq!(Selection::word_at(&l, TextPos::new(1, 4)).text(&l, "\n"), "next", "past a line's end: its last word");
    }

    /// A selection outlives text that got shorter (a file read anew).
    #[test]
    fn clamped_to_the_text() {
        let mut s = Selection::all(&lines(&["one", "two", "three"]));
        let l = lines(&["ů"]);
        s.caret = TextPos::new(5, 1);
        s.clamp(&l);
        assert_eq!(s.caret, TextPos::new(0, 0), "inside the line, on a character's edge");
        let mut e = Selection::all(&[]);
        e.go(&[], Move::Down, true);
        assert!(e.is_empty());
    }
}
