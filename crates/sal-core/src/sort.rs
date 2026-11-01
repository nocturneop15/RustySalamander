//! Panel sorting (port of `sort.cpp`).
//!
//! Names are compared "logically" (numbers by value, as Explorer does), with
//! Unicode-aware case folding instead of the original ANSI `LowerCase[]` table.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::entry::{EntryKind, FileEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortKey {
    Name,
    Ext,
    Time,
    Size,
    Attr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortSpec {
    pub key: SortKey,
    pub reverse: bool,
}

impl Default for SortSpec {
    fn default() -> Self {
        SortSpec { key: SortKey::Name, reverse: false }
    }
}

impl SortSpec {
    /// Clicking the same column again reverses it, another column resets.
    pub fn toggled(self, key: SortKey) -> SortSpec {
        if self.key == key {
            SortSpec { key, reverse: !self.reverse }
        } else {
            SortSpec { key, reverse: false }
        }
    }
}

/// Compare names, case-insensitive when asked.
fn cmp_text(a: &str, b: &str, ignore_case: bool) -> Ordering {
    if ignore_case {
        a.chars().flat_map(char::to_lowercase).cmp(b.chars().flat_map(char::to_lowercase))
    } else {
        a.cmp(b)
    }
}

/// Next segment end starting at `pos`: a run of ASCII digits, a single dot,
/// or a run of other text. Returns `(end, is_number, first_nonzero_digit)`.
/// Digits and '.' are ASCII, so byte-level scanning is UTF-8 safe.
fn segment(s: &[u8], pos: usize) -> (usize, bool, Option<usize>) {
    let mut end = pos;
    if end < s.len() && s[end].is_ascii_digit() {
        let mut nz = None;
        while end < s.len() && s[end].is_ascii_digit() {
            if nz.is_none() && s[end] != b'0' {
                nz = Some(end);
            }
            end += 1;
        }
        (end, true, nz)
    } else if end < s.len() && s[end] == b'.' {
        (end + 1, false, None)
    } else {
        while end < s.len() && !s[end].is_ascii_digit() && s[end] != b'.' {
            end += 1;
        }
        (end, false, None)
    }
}

/// Port of `StrCmpLogicalEx`: numbers compare by value, `"001" < "01"` only
/// as a tie-breaker, dots separate segments.
pub fn logical_cmp(a: &str, b: &str, ignore_case: bool) -> Ordering {
    let (s1, s2) = (a.as_bytes(), b.as_bytes());
    let (mut beg1, mut beg2) = (0usize, 0usize);
    let mut suggestion = Ordering::Equal;
    loop {
        let (end1, num1, nz1) = segment(s1, beg1);
        let (end2, num2, nz2) = segment(s2, beg2);
        if !num1 || !num2 {
            let r = cmp_text(&a[beg1..end1], &b[beg2..end2], ignore_case);
            if r != Ordering::Equal {
                return r;
            }
        } else {
            let len1 = end1 - beg1;
            let len2 = end2 - beg2;
            match (nz1, nz2) {
                (None, None) => {
                    if suggestion == Ordering::Equal {
                        suggestion = len2.cmp(&len1); // "000" < "00"
                    }
                }
                (None, Some(_)) => return Ordering::Less,
                (Some(_), None) => return Ordering::Greater,
                (Some(n1), Some(n2)) => {
                    let r = (end1 - n1).cmp(&(end2 - n2)).then_with(|| s1[n1..end1].cmp(&s2[n2..end2]));
                    if r != Ordering::Equal {
                        return r;
                    }
                    if suggestion == Ordering::Equal {
                        suggestion = len2.cmp(&len1); // "0001" < "001"
                    }
                }
            }
        }
        if end1 >= s1.len() && end2 >= s2.len() {
            return suggestion;
        }
        beg1 = end1;
        beg2 = end2;
    }
}

/// Port of `CmpNameExt`: base name, then extension, case-insensitive first,
/// case-sensitive as a final tie-breaker so the order is total.
pub fn cmp_name_ext(a: &FileEntry, b: &FileEntry) -> Ordering {
    logical_cmp(a.base(), b.base(), true).then_with(|| logical_cmp(a.ext(), b.ext(), true)).then_with(|| logical_cmp(&a.name, &b.name, false))
}

/// The size of a folder, when known (calculated, or from an archive's
/// index); `None` sorts it as empty.
pub type DirSizeOf<'a> = &'a dyn Fn(&FileEntry) -> Option<u64>;

/// Compare by one key (names break ties elsewhere); folders by the sizes
/// `dir_size` knows.
fn cmp_by(a: &FileEntry, b: &FileEntry, key: SortKey, dir_size: DirSizeOf) -> Ordering {
    let size = |e: &FileEntry| if e.kind == EntryKind::File { e.size } else { dir_size(e).unwrap_or(0) };
    match key {
        SortKey::Name => Ordering::Equal,
        SortKey::Ext => logical_cmp(a.ext(), b.ext(), true),
        SortKey::Time => a.modified.cmp(&b.modified),
        SortKey::Size => size(a).cmp(&size(b)),
        SortKey::Attr => a.attrs.cmp(&b.attrs),
    }
}

/// Sort group: `..` first, then folders, then files.
fn group(e: &FileEntry) -> u8 {
    match e.kind {
        EntryKind::UpDir => 0,
        EntryKind::Dir => 1,
        EntryKind::File => 2,
    }
}

/// The panel's order of two items: `..` first, then directories, then
/// files; `reverse` flips the order within each group only.
fn cmp_entries(a: &FileEntry, b: &FileEntry, spec: SortSpec, dir_size: DirSizeOf) -> Ordering {
    group(a).cmp(&group(b)).then_with(|| {
        let r = cmp_by(a, b, spec.key, dir_size).then_with(|| cmp_name_ext(a, b));
        if spec.reverse {
            r.reverse()
        } else {
            r
        }
    })
}

/// Sort a panel: `..` first, then directories, then files; `reverse` flips
/// the order within each group only. Folders count as empty when sorted by
/// size (see [`sort_entries_sized`]).
pub fn sort_entries(entries: &mut [FileEntry], spec: SortSpec) {
    sort_entries_sized(entries, spec, &|_| None);
}

/// Sort a panel as [`sort_entries`] does, folders by the sizes `dir_size`
/// knows.
pub fn sort_entries_sized(entries: &mut [FileEntry], spec: SortSpec, dir_size: DirSizeOf) {
    entries.sort_by(|a, b| cmp_entries(a, b, spec, dir_size));
}

/// Whether `entries` are in the order [`sort_entries_sized`] gives.
pub fn is_sorted(entries: &[FileEntry], spec: SortSpec, dir_size: DirSizeOf) -> bool {
    entries.windows(2).all(|w| cmp_entries(&w[0], &w[1], spec, dir_size) != Ordering::Greater)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use Ordering::*;

    #[test]
    fn logical_numbers() {
        assert_eq!(logical_cmp("file2", "file10", true), Less);
        assert_eq!(logical_cmp("file10", "file9", true), Greater);
        assert_eq!(logical_cmp("99", "100", true), Less);
        assert_eq!(logical_cmp("001", "01", true), Less);
        assert_eq!(logical_cmp("01", "001", true), Greater);
        assert_eq!(logical_cmp("00", "1", true), Less);
        assert_eq!(logical_cmp("abc", "ABC", true), Equal);
        assert_eq!(logical_cmp("a", "b", true), Less);
    }

    #[test]
    fn unicode_case_insensitive() {
        assert_eq!(logical_cmp("ŽLUŤOUČKÝ", "žluťoučký", true), Equal);
        assert_eq!(logical_cmp("ΑΒΓ", "αβγ", true), Equal);
        assert_eq!(logical_cmp("Straße 2", "straße 10", true), Less);
    }

    #[test]
    fn folders_by_their_sizes() {
        let mk = |n: &str, k, size| FileEntry::new(PathBuf::from(n), k, size, None, 0);
        let mut v = vec![mk("small", EntryKind::Dir, 0), mk("big", EntryKind::Dir, 0), mk("unknown", EntryKind::Dir, 0), mk("b.txt", EntryKind::File, 5), mk("a.txt", EntryKind::File, 9)];
        let spec = SortSpec { key: SortKey::Size, reverse: false };
        let sizes = |e: &FileEntry| match e.name.as_str() {
            "small" => Some(10),
            "big" => Some(1_000),
            _ => None,
        };
        sort_entries_sized(&mut v, spec, &sizes);
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["unknown", "small", "big", "b.txt", "a.txt"], "an unknown size counts as empty");
        assert!(is_sorted(&v, spec, &sizes));
        assert!(!is_sorted(&v, SortSpec { key: SortKey::Size, reverse: true }, &sizes));
        // without the sizes the folders are by name
        sort_entries(&mut v, spec);
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["big", "small", "unknown", "b.txt", "a.txt"]);
    }

    #[test]
    fn dirs_first_and_reverse() {
        let mk = |n: &str, k| FileEntry::new(PathBuf::from(n), k, 0, None, 0);
        let mut v = vec![
            mk("b.txt", EntryKind::File),
            mk("zdir", EntryKind::Dir),
            mk("a10.txt", EntryKind::File),
            mk("a2.txt", EntryKind::File),
            mk("adir", EntryKind::Dir),
            FileEntry::up_dir(PathBuf::from("..")),
        ];
        sort_entries(&mut v, SortSpec::default());
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["..", "adir", "zdir", "a2.txt", "a10.txt", "b.txt"]);
        sort_entries(&mut v, SortSpec { key: SortKey::Name, reverse: true });
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["..", "zdir", "adir", "b.txt", "a10.txt", "a2.txt"]);
    }
}
