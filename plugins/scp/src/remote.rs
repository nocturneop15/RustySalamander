//! The server side of paths and listings: Unix paths from inner paths,
//! shell quoting, and reading `ls -l` output (GNU, BusyBox and BSD forms).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sal_plugin_api::{join_inner, Item, ATTR_DIRECTORY, ATTR_HIDDEN};

/// The server path of an inner path: `home\me` → `/home/me`, `""` → `/`.
pub fn remote_path(inner: &str) -> String {
    format!("/{}", inner.replace('\\', "/"))
}

/// The inner path of a server path: `/home/me/` → `home\me`.
pub fn inner_path(remote: &str) -> String {
    remote.split('/').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\\")
}

/// `s` quoted for a POSIX shell: `it's` → `'it'\''s'`.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The command that lists the folder `remote`: one line per item, sizes and
/// times as numbers where `ls` can do that (GNU), dates as text otherwise.
/// `-L` shows links as what they point to, so links to folders open.
pub fn list_command(remote: &str) -> String {
    let dir = quote(&format!("{}/", remote.trim_end_matches('/')));
    format!("LC_ALL=C ls -lanL --time-style=+%s -- {dir} 2>/dev/null || LC_ALL=C ls -lanL -- {dir}")
}

/// The items `ls -lan` printed for the folder `folder` (inner path); `.`,
/// `..`, the "total" line and names that can't be shown (with `\` or line
/// breaks) are left out.
pub fn parse_listing(text: &str, folder: &str, now: SystemTime) -> Vec<Item> {
    text.lines()
        .filter_map(|line| parse_line(line, now))
        .filter(|(name, ..)| name != "." && name != ".." && !name.contains('\\'))
        .map(|(name, dir, size, modified)| {
            let mut attrs = if dir { ATTR_DIRECTORY } else { 0 };
            if name.starts_with('.') {
                attrs |= ATTR_HIDDEN;
            }
            Item { path: join_inner(folder, &name), dir, size: if dir { 0 } else { size }, modified, attrs, index: None }
        })
        .collect()
}

/// Whitespace-separated words of `line` with where each starts.
fn words(line: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in line.char_indices() {
        match (c.is_whitespace(), start) {
            (true, Some(s)) => {
                out.push((s, &line[s..i]));
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, &line[s..]));
    }
    out
}

/// One `ls -lan` line: (name, folder?, size, time).
fn parse_line(line: &str, now: SystemTime) -> Option<(String, bool, u64, Option<SystemTime>)> {
    let w = words(line);
    let mode = w.first()?.1;
    let kind = mode.chars().next()?;
    if !"-dlcbps".contains(kind) || mode.len() < 10 {
        return None; // "total 12" and the like
    }
    // mode, links, owner, group, then size (devices: "major, minor")
    let mut i = 4;
    let mut size: u64 = 0;
    if let Some((_, s)) = w.get(i) {
        if let Some(major) = s.strip_suffix(',') {
            major.parse::<u64>().ok()?;
            i += 1;
        } else {
            size = s.parse().ok()?;
        }
    }
    i += 1;
    // the time: seconds since 1970 (GNU --time-style=+%s), or "Mon DD HH:MM" / "Mon DD YYYY"
    let (modified, name_at) = match w.get(i) {
        Some((_, t)) if t.bytes().all(|b| b.is_ascii_digit()) => (t.parse().ok().map(|s: u64| UNIX_EPOCH + Duration::from_secs(s)), w.get(i + 1)?.0),
        _ => (text_date(w.get(i)?.1, w.get(i + 1)?.1, w.get(i + 2)?.1, now), w.get(i + 3)?.0),
    };
    let mut name = &line[name_at..];
    if kind == 'l' {
        // a link -L couldn't follow: "name -> target"
        name = name.split(" -> ").next().unwrap_or(name);
    }
    Some((name.to_owned(), kind == 'd', size, modified))
}

/// "Oct 30 12:34" (within the last year) or "Oct 30 2023", read as UTC.
fn text_date(month: &str, day: &str, time_or_year: &str, now: SystemTime) -> Option<SystemTime> {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let m = MONTHS.iter().position(|x| x.eq_ignore_ascii_case(month))? as u32 + 1;
    let d: u32 = day.parse().ok()?;
    let at = |y: i64, h: u64, min: u64| UNIX_EPOCH + Duration::from_secs(days_from_civil(y, m, d) as u64 * 86_400 + h * 3600 + min * 60);
    match time_or_year.split_once(':') {
        Some((h, min)) => {
            let (h, min) = (h.parse().ok()?, min.parse().ok()?);
            let this_year = 1970 + now.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64 / 31_556_952;
            // a date shown with a time is within the last six months
            let t = at(this_year, h, min);
            Some(if t > now + Duration::from_secs(86_400) { at(this_year - 1, h, min) } else { t })
        }
        None => Some(at(time_or_year.parse().ok()?, 0, 0)),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_quoting() {
        assert_eq!(remote_path(""), "/");
        assert_eq!(remote_path("home\\já 🦎"), "/home/já 🦎");
        assert_eq!(inner_path("/home/me/"), "home\\me");
        assert_eq!(inner_path("/"), "");
        assert_eq!(quote("it's $HOME"), r"'it'\''s $HOME'");
        assert!(list_command("/").contains("-- '/'"));
    }

    #[test]
    fn listings() {
        let now = UNIX_EPOCH + Duration::from_secs(days_from_civil(2026, 10, 30) as u64 * 86_400);
        let gnu = "total 12\n\
                   drwxr-xr-x  2 1000 1000 4096 1761782400 .\n\
                   drwxr-xr-x  9 0    0    4096 1761782400 ..\n\
                   -rw-r--r--  1 1000 1000   12 1761782400 zpráva 🦎  s mezerami.txt\n\
                   drwx------  2 1000 1000 4096 1761782400 .ssh\n\
                   crw-rw-rw-  1 0    0    1, 3 1761782400 null\n\
                   lrwxrwxrwx  1 1000 1000   11 1761782400 broken -> nowhere\n\
                   -rw-r--r--  1 1000 1000    5 1761782400 back\\slash\n";
        let items = parse_listing(gnu, "home\\me", now);
        let got: Vec<(&str, bool, u64, u32)> = items.iter().map(|i| (i.path.as_str(), i.dir, i.size, i.attrs)).collect();
        assert_eq!(
            got,
            [
                ("home\\me\\zpráva 🦎  s mezerami.txt", false, 12, 0),
                ("home\\me\\.ssh", true, 0, ATTR_DIRECTORY | ATTR_HIDDEN),
                ("home\\me\\null", false, 0, 0),
                ("home\\me\\broken", false, 11, 0),
            ]
        );
        assert_eq!(items[0].modified, Some(UNIX_EPOCH + Duration::from_secs(1_761_782_400)));

        // BusyBox / BSD: dates as text
        let busybox = "-rw-r--r--    1 0        0              7 Oct 29 12:34 a.txt\n\
                       -rw-r--r--    1 0        0              8 Dec 31  2025 b.txt\n\
                       drwxr-xr-x    2 0        0              0 Jan  1  2001 2001\n";
        let items = parse_listing(busybox, "", now);
        assert_eq!(items.iter().map(|i| (i.path.as_str(), i.size)).collect::<Vec<_>>(), [("a.txt", 7), ("b.txt", 8), ("2001", 0)]);
        let day = |y, m, d| UNIX_EPOCH + Duration::from_secs(days_from_civil(y, m, d) as u64 * 86_400);
        assert_eq!(items[0].modified, Some(day(2026, 10, 29) + Duration::from_secs(12 * 3600 + 34 * 60)));
        assert_eq!(items[1].modified, Some(day(2025, 12, 31)));
        // a time shown for a date later this year is last year's
        let items = parse_listing("-rw-r--r-- 1 0 0 1 Dec 24 10:00 x\n", "", now);
        assert_eq!(items[0].modified, Some(day(2025, 12, 24) + Duration::from_secs(10 * 3600)));
    }
}
