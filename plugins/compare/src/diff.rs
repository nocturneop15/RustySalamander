//! Two files compared: text line by line (side by side, lined up), or the
//! bytes of binary files. No window here, so it can be tested.

use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use sal_core::encoding;
use similar::{capture_diff_slices_deadline, Algorithm, DiffOp};

/// Larger files are compared as bytes only.
pub const MAX_TEXT: u64 = 32 << 20;
/// How long finding the smallest difference may take; after that a larger
/// one is shown (still correct, less tidy).
const DEADLINE: Duration = Duration::from_secs(5);

/// How a row's two sides differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Same,
    /// A line changed (both sides have one).
    Changed,
    /// A line only the left file has.
    Removed,
    /// A line only the right file has.
    Added,
}

/// A row of the side-by-side view: the left and the right line (number from
/// 1 and text), either missing where the other side has a line the first
/// doesn't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub left: Option<(usize, String)>,
    pub right: Option<(usize, String)>,
    pub tag: Tag,
}

/// What comparing two files found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Text, lined up; `changes` are the first rows of each block of
    /// differences.
    Text { rows: Vec<Row>, changes: Vec<usize> },
    /// Binary (or too large to show as text): the sizes, and the offset of
    /// the first differing byte (`None`: the same bytes).
    Binary { sizes: (u64, u64), first_difference: Option<u64> },
}

/// Compare two files; `ignore_space` treats lines differing only in spaces
/// and tabs as the same.
pub fn compare(left: &Path, right: &Path, ignore_space: bool) -> io::Result<Outcome> {
    let (ml, mr) = (fs::metadata(left)?, fs::metadata(right)?);
    if ml.len() > MAX_TEXT || mr.len() > MAX_TEXT {
        return binary(left, right, (ml.len(), mr.len()));
    }
    let (a, b) = (fs::read(left)?, fs::read(right)?);
    if is_binary(&a) || is_binary(&b) {
        return binary(left, right, (ml.len(), mr.len()));
    }
    let (ta, tb) = (encoding::decode_auto(&a).0, encoding::decode_auto(&b).0);
    Ok(text(&ta, &tb, ignore_space))
}

/// Whether bytes look binary: a NUL in the first 8 KiB, unless the text is
/// UTF-16 (which has NULs in every other byte).
fn is_binary(bytes: &[u8]) -> bool {
    let (enc, _) = encoding::detect(bytes);
    !matches!(enc, encoding::TextEncoding::Utf16Le | encoding::TextEncoding::Utf16Be) && bytes.iter().take(8192).any(|&b| b == 0)
}

/// Two files' sizes and where their bytes first differ.
fn binary(left: &Path, right: &Path, sizes: (u64, u64)) -> io::Result<Outcome> {
    use std::io::Read;
    let (mut fa, mut fb) = (io::BufReader::new(fs::File::open(left)?), io::BufReader::new(fs::File::open(right)?));
    let (mut ba, mut bb) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    let mut at = 0u64;
    loop {
        let n = fa.read(&mut ba)?;
        let m = fb.read_exact_or_less(&mut bb[..n.max(1)])?;
        let k = n.min(m);
        if let Some(i) = (0..k).find(|&i| ba[i] != bb[i]) {
            return Ok(Outcome::Binary { sizes, first_difference: Some(at + i as u64) });
        }
        if n != m {
            // one file ends where the other goes on
            return Ok(Outcome::Binary { sizes, first_difference: Some(at + k as u64) });
        }
        if n == 0 {
            return Ok(Outcome::Binary { sizes, first_difference: None });
        }
        at += n as u64;
    }
}

/// Reading exactly as many bytes as asked, or fewer at the end.
trait ReadFull {
    fn read_exact_or_less(&mut self, buf: &mut [u8]) -> io::Result<usize>;
}

impl<R: io::Read> ReadFull for R {
    fn read_exact_or_less(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut n = 0;
        while n < buf.len() {
            match self.read(&mut buf[n..])? {
                0 => break,
                k => n += k,
            }
        }
        Ok(n)
    }
}

/// Two texts lined up line by line.
pub fn text(a: &str, b: &str, ignore_space: bool) -> Outcome {
    let (la, lb): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    // what the lines are compared by
    let key = |l: &&str| -> String {
        if ignore_space {
            l.split_whitespace().collect::<Vec<_>>().join(" ")
        } else {
            (*l).to_owned()
        }
    };
    let (ka, kb): (Vec<String>, Vec<String>) = (la.iter().map(key).collect(), lb.iter().map(key).collect());
    let ops = capture_diff_slices_deadline(Algorithm::Myers, &ka, &kb, Some(Instant::now() + DEADLINE));
    let line = |lines: &[&str], i: usize| Some((i + 1, lines[i].replace('\t', "    ")));
    let mut rows = Vec::with_capacity(la.len().max(lb.len()));
    let mut changes = Vec::new();
    for op in ops {
        let start = rows.len();
        match op {
            DiffOp::Equal { old_index, new_index, len } => {
                rows.extend((0..len).map(|k| Row { left: line(&la, old_index + k), right: line(&lb, new_index + k), tag: Tag::Same }));
            }
            DiffOp::Delete { old_index, old_len, .. } => {
                rows.extend((0..old_len).map(|k| Row { left: line(&la, old_index + k), right: None, tag: Tag::Removed }));
            }
            DiffOp::Insert { new_index, new_len, .. } => {
                rows.extend((0..new_len).map(|k| Row { left: None, right: line(&lb, new_index + k), tag: Tag::Added }));
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                for k in 0..old_len.max(new_len) {
                    let (l, r) = ((k < old_len).then(|| old_index + k), (k < new_len).then(|| new_index + k));
                    let tag = match (l, r) {
                        (Some(_), Some(_)) => Tag::Changed,
                        (Some(_), None) => Tag::Removed,
                        _ => Tag::Added,
                    };
                    rows.push(Row { left: l.and_then(|i| line(&la, i)), right: r.and_then(|i| line(&lb, i)), tag });
                }
            }
        }
        // a block of differences starts here (neighbouring blocks count as one)
        if rows.len() > start && rows[start].tag != Tag::Same && (start == 0 || rows[start - 1].tag == Tag::Same) {
            changes.push(start);
        }
    }
    Outcome::Text { rows, changes }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_line_up() {
        let a = "jedna\ndva\ntři\nčtyři\npět\n";
        let b = "jedna\nDVA\ntři\npět\nšest 🦎\n";
        let Outcome::Text { rows, changes } = text(a, b, false) else { panic!("text") };
        let tags: Vec<Tag> = rows.iter().map(|r| r.tag).collect();
        assert_eq!(tags, [Tag::Same, Tag::Changed, Tag::Same, Tag::Removed, Tag::Same, Tag::Added]);
        assert_eq!(changes, [1, 3, 5]);
        assert_eq!(rows[3].left, Some((4, "čtyři".to_owned())));
        assert_eq!(rows[3].right, None);
        assert_eq!(rows[5].right, Some((5, "šest 🦎".to_owned())));
        // the same texts: no changes
        let Outcome::Text { changes, .. } = text(a, a, false) else { panic!() };
        assert!(changes.is_empty());
        // spaces ignored when asked
        let Outcome::Text { changes, .. } = text("a  b\n\tc\n", "a b\nc  \n", true) else { panic!() };
        assert!(changes.is_empty());
        let Outcome::Text { changes, .. } = text("a  b\n", "a b\n", false) else { panic!() };
        assert_eq!(changes, [0]);
    }

    #[test]
    fn files() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a.txt"), t.path().join("b.txt"));
        fs::write(&a, "\u{FEFF}řádek\n").unwrap();
        // the same text in UTF-16: compared as text, the same
        let mut u16 = vec![0xFF, 0xFE];
        u16.extend("řádek\n".encode_utf16().flat_map(u16::to_le_bytes));
        fs::write(&b, u16).unwrap();
        let Outcome::Text { changes, .. } = compare(&a, &b, false).unwrap() else { panic!("text") };
        assert!(changes.is_empty(), "encodings don't matter");

        let (x, y) = (t.path().join("x.bin"), t.path().join("y.bin"));
        fs::write(&x, [0u8, 1, 2, 3, 4]).unwrap();
        fs::write(&y, [0u8, 1, 9, 3]).unwrap();
        assert_eq!(compare(&x, &y, false).unwrap(), Outcome::Binary { sizes: (5, 4), first_difference: Some(2) });
        fs::write(&y, [0u8, 1, 2, 3]).unwrap();
        assert_eq!(compare(&x, &y, false).unwrap(), Outcome::Binary { sizes: (5, 4), first_difference: Some(4) }, "one ends first");
        assert_eq!(compare(&x, &x, false).unwrap(), Outcome::Binary { sizes: (5, 5), first_difference: None });
        assert!(compare(&x, &t.path().join("není"), false).is_err());
    }
}
