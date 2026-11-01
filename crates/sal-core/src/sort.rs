//! Panel sorting.
//!
//! Names are in the alphabetical order of the user's language, as Explorer
//! has them: `Čeština` between `Cena` and `Dům` (not after `Zeta`, where
//! the letters' code numbers put it), in Czech `Chata` after `Hora`, case
//! not mattering, and numbers by their value (`a2` before `a10`). Windows
//! orders them ([`Collated`]); elsewhere [`logical_cmp`] does, a port of
//! Salamander's `StrCmpLogicalEx` (letters by their code numbers).

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

/// A text's place in the alphabetical order of the user's language, case
/// not mattering and numbers by their value; compared as bytes, so working
/// it out once per name makes sorting many names fast.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Collated(
    #[cfg(windows)] Vec<u8>,
    #[cfg(not(windows))] String,
);

impl Collated {
    /// The place of `text`.
    #[cfg(windows)]
    pub fn of(text: &str) -> Collated {
        use windows_sys::Win32::Globalization::{LCMapStringEx, LCMAP_SORTKEY, NORM_IGNORECASE, SORT_DIGITSASNUMBERS, SORT_STRINGSORT};
        if text.is_empty() {
            return Collated(Vec::new());
        }
        let wide: Vec<u16> = text.encode_utf16().collect();
        // the user's locale (a null name); symbols sort before letters
        // instead of being skipped (SORT_STRINGSORT), as in a file list
        let flags = LCMAP_SORTKEY | NORM_IGNORECASE | SORT_DIGITSASNUMBERS | SORT_STRINGSORT;
        let map = |out: *mut u16, len: i32| unsafe { LCMapStringEx(std::ptr::null(), flags, wide.as_ptr(), wide.len() as i32, out, len, std::ptr::null(), std::ptr::null(), 0) };
        // the length of the key in bytes, then the key itself
        let len = map(std::ptr::null_mut(), 0);
        if len <= 0 {
            return Collated(text.to_lowercase().into_bytes());
        }
        let mut key = vec![0u8; len as usize];
        let written = map(key.as_mut_ptr().cast(), len);
        key.truncate(written.max(0) as usize);
        // the key ends with a 0 byte; it doesn't change the order
        Collated(key)
    }

    /// The place of `text`: elsewhere [`logical_cmp`] compares.
    #[cfg(not(windows))]
    pub fn of(text: &str) -> Collated {
        Collated(text.to_owned())
    }
}

#[cfg(not(windows))]
impl Ord for Collated {
    fn cmp(&self, other: &Self) -> Ordering {
        logical_cmp(&self.0, &other.0, true)
    }
}

/// An item's name worked out for sorting: base name and extension.
struct NameKey {
    base: Collated,
    ext: Collated,
}

impl NameKey {
    fn of(e: &FileEntry) -> NameKey {
        NameKey { base: Collated::of(e.base()), ext: Collated::of(e.ext()) }
    }
}

/// By name: base name, then extension (as Salamander's `CmpNameExt`), in
/// the user's alphabetical order; the exact names last, so the order is
/// total.
fn cmp_name_ext(a: (&FileEntry, &NameKey), b: (&FileEntry, &NameKey)) -> Ordering {
    a.1.base.cmp(&b.1.base).then_with(|| a.1.ext.cmp(&b.1.ext)).then_with(|| a.0.name.cmp(&b.0.name))
}

/// The size of a folder, when known (calculated, or from an archive's
/// index); `None` sorts it as empty.
pub type DirSizeOf<'a> = &'a dyn Fn(&FileEntry) -> Option<u64>;

/// Compare by one key (names break ties elsewhere); folders by the sizes
/// `dir_size` knows.
fn cmp_by(a: (&FileEntry, &NameKey), b: (&FileEntry, &NameKey), key: SortKey, dir_size: DirSizeOf) -> Ordering {
    let size = |e: &FileEntry| if e.kind == EntryKind::File { e.size } else { dir_size(e).unwrap_or(0) };
    let ((a, ka), (b, kb)) = (a, b);
    match key {
        SortKey::Name => Ordering::Equal,
        SortKey::Ext => ka.ext.cmp(&kb.ext),
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
fn cmp_entries(a: (&FileEntry, &NameKey), b: (&FileEntry, &NameKey), spec: SortSpec, dir_size: DirSizeOf) -> Ordering {
    group(a.0).cmp(&group(b.0)).then_with(|| {
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
    // each name worked out once, then the items put in order
    let keys: Vec<NameKey> = entries.iter().map(NameKey::of).collect();
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by(|&i, &j| cmp_entries((&entries[i], &keys[i]), (&entries[j], &keys[j]), spec, dir_size));
    // move each item to its place (position i gets the item at order[i])
    for i in 0..order.len() {
        let mut from = order[i];
        while from < i {
            from = order[from];
        }
        entries.swap(i, from);
    }
}

/// Whether `entries` are in the order [`sort_entries_sized`] gives.
pub fn is_sorted(entries: &[FileEntry], spec: SortSpec, dir_size: DirSizeOf) -> bool {
    let keys: Vec<NameKey> = entries.iter().map(NameKey::of).collect();
    (1..entries.len()).all(|i| cmp_entries((&entries[i - 1], &keys[i - 1]), (&entries[i], &keys[i]), spec, dir_size) != Ordering::Greater)
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

    /// Names in the alphabetical order of the user's language: letters with
    /// accents among their plain ones (not after Z), case not mattering,
    /// numbers by value, the extension after the name.
    #[cfg(windows)]
    #[test]
    fn alphabetical_order() {
        let mk = |n: &str| FileEntry::new(PathBuf::from(n), EntryKind::File, 0, None, 0);
        let mut v: Vec<FileEntry> = ["Zeta", "Čeština", "Cena", "Ábel", "Adam", "Šárka", "Sára", "émile", "Eva", "file10.txt", "File2.txt", "a b.txt", "a.txt", "a.TXT", "_x", "1 začátek"].into_iter().map(mk).collect();
        sort_entries(&mut v, SortSpec::default());
        let names: Vec<&str> = v.iter().map(|e| e.name.as_str()).collect();
        let at = |n: &str| names.iter().position(|&x| x == n).unwrap();
        assert!(at("Čeština") < at("Zeta") && at("Cena") < at("Čeština"), "{names:?}");
        assert!(at("Ábel") < at("Cena") && at("Šárka") < at("Zeta"), "{names:?}");
        assert!(at("File2.txt") < at("file10.txt"), "numbers by value: {names:?}");
        assert!(at("a.txt") < at("a b.txt"), "the name before the extension: {names:?}");
        assert!(at("a.TXT") < at("a.txt"), "the same but for case: always the same way round");
        assert!(at("_x") < at("1 začátek") && at("1 začátek") < at("Adam"), "symbols, then numbers, then letters: {names:?}");
        assert!(is_sorted(&v, SortSpec::default(), &|_| None));
    }

    /// Sorting a big folder takes a moment, not more.
    #[test]
    fn many_names_quickly() {
        let mut v: Vec<FileEntry> = (0..20_000).map(|i| FileEntry::new(PathBuf::from(format!("Soubor č. {} – ŽÁDOST.pdf", (i * 7919) % 20_000)), EntryKind::File, i, None, 0)).collect();
        let start = std::time::Instant::now();
        sort_entries(&mut v, SortSpec::default());
        assert!(start.elapsed() < std::time::Duration::from_secs(2), "{:?}", start.elapsed());
        assert!(v[0].name.contains(" 0 ") && v[1].name.contains(" 1 ") && v[19_999].name.contains(" 19999 "));
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
