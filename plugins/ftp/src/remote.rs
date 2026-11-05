//! What the protocols share: server paths from inner paths, one listed
//! item ([`Entry`]), the [`Remote`] each protocol implements, and reading
//! FTP listings (MLSD, and LIST in the Unix and DOS forms).

use std::io::{self, Read};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use suppaftp::list::ListParser;

/// The server path of an inner path: `home\me` → `/home/me`, `""` → `/`.
pub fn remote_path(inner: &str) -> String {
    format!("/{}", inner.replace('\\', "/"))
}

/// The inner path of a server path: `/home/me/` → `home\me`.
pub fn inner_path(remote: &str) -> String {
    remote.split('/').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\\")
}

/// `name` inside the server folder `dir`.
pub fn join_remote(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// One item of a server folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// Whether a name from the server can be shown and used on Windows'
/// side: no path separators, no `.`/`..`, no line breaks.
pub fn usable_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\n', '\r', '\0'])
}

/// The bytes of a file as they arrive; false stops the transfer.
pub type Chunks<'a> = &'a mut dyn FnMut(&[u8]) -> bool;

/// One connection to a server, whatever its protocol. Paths are server
/// paths (`/home/me`). Every call blocks until it is done.
pub trait Remote: Send + Sync {
    /// The folder the server starts in (the user's home).
    fn home(&self) -> String;
    /// The items of the folder `dir`; `NotFound` when it isn't a folder.
    fn list(&self, dir: &str) -> io::Result<Vec<Entry>>;
    /// Read the file `path`, giving its bytes to `to`; `Interrupted` when
    /// `to` said stop.
    fn read(&self, path: &str, to: Chunks) -> io::Result<()>;
    /// Create or replace the file `path` with what `from` reads, and give
    /// it the time `modified` where the server can.
    fn write(&self, path: &str, from: &mut dyn Read, modified: Option<SystemTime>) -> io::Result<()>;
    fn mkdir(&self, path: &str) -> io::Result<()>;
    fn remove_file(&self, path: &str) -> io::Result<()>;
    fn remove_dir(&self, path: &str) -> io::Result<()>;
    fn rename(&self, old: &str, new: &str) -> io::Result<()>;
    /// End the connection.
    fn close(&self);
}

/// The items of an MLSD listing (RFC 3659); the folder itself, its parent
/// and names that can't be used are left out.
pub fn parse_mlsd(lines: &[String]) -> Vec<(Entry, bool)> {
    lines.iter().filter_map(|l| mlsd_line(l.trim_end_matches(['\r', '\n']))).collect()
}

/// One MLSD line: the item and whether it is a link (its kind unknown).
fn mlsd_line(line: &str) -> Option<(Entry, bool)> {
    // "fact=value;fact=value; name": the first "; " ends the facts
    let (facts, name) = match line.strip_prefix(' ') {
        Some(name) => ("", name),
        None => {
            let end = line.find("; ")?;
            (&line[..=end], &line[end + 2..])
        }
    };
    let (mut kind, mut size, mut modified) = (String::new(), 0, None);
    for fact in facts.split(';') {
        let Some((key, value)) = fact.split_once('=') else { continue };
        match key.to_ascii_lowercase().as_str() {
            "type" => kind = value.to_ascii_lowercase(),
            "size" | "sizd" => size = value.parse().unwrap_or(0),
            "modify" => modified = mlsd_time(value),
            _ => {}
        }
    }
    if kind == "cdir" || kind == "pdir" || !usable_name(name) {
        return None;
    }
    let dir = kind == "dir";
    // "OS.unix=slink:/target", "OS.unix=symlink": what it points to is unknown
    let link = kind.contains("link");
    Some((Entry { name: name.to_owned(), dir, size: if dir { 0 } else { size }, modified }, link))
}

/// MLSD's time, always UTC: `YYYYMMDDHHMMSS[.sss]`.
pub fn mlsd_time(s: &str) -> Option<SystemTime> {
    let s = s.split('.').next()?;
    if s.len() != 14 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s[r].parse::<u32>().ok();
    let (y, mo, d, h, mi, se) = (n(0..4)? as i64, n(4..6)?, n(6..8)?, n(8..10)? as u64, n(10..12)? as u64, n(12..14)? as u64);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let days = days_from_civil(y, mo, d);
    let secs = u64::try_from(days).ok()? * 86_400 + h * 3600 + mi * 60 + se;
    Some(UNIX_EPOCH + Duration::from_secs(secs))
}

/// `t` as MLSD writes it (for MFMT): `YYYYMMDDHHMMSS`, UTC.
pub fn ftp_time(t: SystemTime) -> Option<String> {
    let secs = t.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    let (y, m, d) = civil_from_days(days);
    Some(format!("{y:04}{m:02}{d:02}{:02}{:02}{:02}", rest / 3600, rest / 60 % 60, rest % 60))
}

/// The items of a LIST listing, Unix (`ls -l`) or DOS/IIS form; lines
/// neither can read ("total 12") are left out.
pub fn parse_list(lines: &[String]) -> Vec<(Entry, bool)> {
    lines
        .iter()
        .filter_map(|l| {
            let l = l.trim_end_matches(['\r', '\n']);
            if l.starts_with("total ") {
                return None;
            }
            let f = ListParser::parse_posix(l).or_else(|_| ListParser::parse_dos(l)).ok()?;
            let link = f.is_symlink();
            let dir = f.is_directory();
            let name = f.name().to_owned();
            usable_name(&name).then(|| (Entry { name, dir, size: if dir { 0 } else { f.size() as u64 }, modified: Some(f.modified()) }, link))
        })
        .collect()
}

/// Days since 1970-01-01 of a date in the proleptic Gregorian calendar.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day since 1970-01-01: (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(remote_path(""), "/");
        assert_eq!(remote_path("home\\já 🦎"), "/home/já 🦎");
        assert_eq!(inner_path("/home/me/"), "home\\me");
        assert_eq!(inner_path("/"), "");
        assert_eq!(join_remote("/", "a"), "/a");
        assert_eq!(join_remote("/a", "b c"), "/a/b c");
        assert!(!usable_name("..") && !usable_name("a/b") && !usable_name("a\\b") && usable_name("a b; c.txt"));
    }

    #[test]
    fn times() {
        let t = mlsd_time("20261105143059.123").unwrap();
        assert_eq!(ftp_time(t).unwrap(), "20261105143059");
        assert_eq!(mlsd_time("19700101000000"), Some(UNIX_EPOCH));
        assert_eq!(mlsd_time("2026110514305"), None);
        assert_eq!(mlsd_time("20261305143059"), None);
        for days in [0i64, 59, 60, 365, 10_957, 20_000, 30_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
    }

    #[test]
    fn mlsd() {
        let lines: Vec<String> = [
            "type=cdir;modify=20261105100000; /home/me",
            "type=pdir;modify=20261105100000; ..",
            "type=file;size=12;modify=20261105100000;UNIX.mode=0644; zpráva 🦎  s; mezerami.txt",
            "Type=dir;Modify=20261105100000; složka\r",
            "type=OS.unix=slink:/var/www;modify=20261105100000; www",
            "type=file;size=1; back\\slash",
        ]
        .map(String::from)
        .to_vec();
        let got = parse_mlsd(&lines);
        let names: Vec<(&str, bool, u64, bool)> = got.iter().map(|(e, l)| (e.name.as_str(), e.dir, e.size, *l)).collect();
        assert_eq!(names, [("zpráva 🦎  s; mezerami.txt", false, 12, false), ("složka", true, 0, false), ("www", false, 0, true)]);
        assert_eq!(got[0].0.modified, mlsd_time("20261105100000"));
    }

    #[test]
    fn list_unix_and_dos() {
        let lines: Vec<String> = [
            "total 12",
            "drwxr-xr-x    2 ftp      ftp          4096 Nov 05 10:00 složka",
            "-rw-r--r--    1 ftp      ftp            12 Nov 05  2024 soubor s mezerou.txt",
            "lrwxrwxrwx    1 ftp      ftp            11 Nov 05 10:00 www -> /var/www",
            "11-05-26  10:00AM       <DIR>          Dokumenty",
            "11-05-26  10:00AM                  345 zpráva.txt",
        ]
        .map(String::from)
        .to_vec();
        let got = parse_list(&lines);
        let names: Vec<(&str, bool, u64, bool)> = got.iter().map(|(e, l)| (e.name.as_str(), e.dir, e.size, *l)).collect();
        assert_eq!(
            names,
            [("složka", true, 0, false), ("soubor s mezerou.txt", false, 12, false), ("www", false, 11, true), ("Dokumenty", true, 0, false), ("zpráva.txt", false, 345, false)]
        );
    }
}
