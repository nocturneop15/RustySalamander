//! Panel locations: typed path input, UNC servers and shares, parents and
//! breadcrumbs (replaces the path handling around `salamdr*.cpp`).
//!
//! UNC levels: `\\server` is a virtual directory listing the server's shares;
//! `\\server\share` is a real directory whose parent is `\\server`.

use std::io;
use std::path::{Path, PathBuf, MAIN_SEPARATOR};

use crate::encoding::display_path;
use crate::entry::{EntryKind, FileEntry, ATTR_DIRECTORY, ATTR_HIDDEN};

/// `\\server` (optionally with a trailing separator) → `server`.
pub fn unc_server(p: &Path) -> Option<String> {
    let s = display_path(p);
    let rest = s.strip_prefix(r"\\").or_else(|| s.strip_prefix("//"))?;
    if rest.starts_with(['?', '.']) && rest[1..].starts_with(['\\', '/']) {
        return None; // \\?\ and \\.\ device paths
    }
    let server = rest.trim_end_matches(['\\', '/']);
    (!server.is_empty() && !server.contains(['\\', '/'])).then(|| server.to_owned())
}

/// `\\server\share[\]` → `(server, share)`.
pub fn unc_share_root(p: &Path) -> Option<(String, String)> {
    let s = display_path(p);
    let rest = s.strip_prefix(r"\\")?;
    let mut parts = rest.split(['\\', '/']).filter(|x| !x.is_empty());
    let (server, share) = (parts.next()?, parts.next()?);
    if server == "?" || server == "." || parts.next().is_some() {
        return None;
    }
    Some((server.to_owned(), share.to_owned()))
}

pub fn server_path(server: &str) -> PathBuf {
    PathBuf::from(format!(r"\\{server}"))
}

/// Parent location and the name to focus there. A share root's parent is
/// its server; a server and drive roots have none.
pub fn parent_of(p: &Path) -> Option<(PathBuf, String)> {
    if unc_server(p).is_some() {
        return None;
    }
    if let Some((server, share)) = unc_share_root(p) {
        return Some((server_path(&server), share));
    }
    let parent = p.parent()?;
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Some((parent.to_path_buf(), name))
}

/// Breadcrumb segments: (label, target), from the root down.
pub fn breadcrumbs(p: &Path) -> Vec<(String, PathBuf)> {
    let s = display_path(p);
    let mut out = Vec::new();
    if let Some(rest) = s.strip_prefix(r"\\") {
        let mut parts = rest.split(['\\', '/']).filter(|x| !x.is_empty());
        let Some(server) = parts.next() else { return out };
        out.push((format!(r"\\{server}"), server_path(server)));
        let mut acc = server_path(server);
        for (i, part) in parts.enumerate() {
            acc = if i == 0 { PathBuf::from(format!(r"\\{server}\{part}\")) } else { acc.join(part) };
            out.push((part.to_owned(), acc.clone()));
        }
        return out;
    }
    if let Some(rest) = s.strip_prefix('/') {
        out.push(("/".to_owned(), PathBuf::from("/")));
        let mut acc = PathBuf::from("/");
        for part in rest.split('/').filter(|x| !x.is_empty()) {
            acc.push(part);
            out.push((part.to_owned(), acc.clone()));
        }
        return out;
    }
    let mut parts = s.split(['\\', '/']).filter(|x| !x.is_empty());
    let Some(first) = parts.next() else { return out };
    let mut acc = PathBuf::from(format!("{first}{MAIN_SEPARATOR}"));
    out.push((first.to_owned(), acc.clone()));
    for part in parts {
        acc.push(part);
        out.push((part.to_owned(), acc.clone()));
    }
    out
}

/// Expand `%VAR%` references; unknown variables are left as typed.
fn expand_env(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => out.push_str(&rest[start..start + end + 2]),
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// What a typed location means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A bare drive letter such as `D:`: switch drive (last directory there).
    Drive(char),
    /// A directory (or `\\server` share list) to open.
    Dir(PathBuf),
    /// An existing file: open its directory and focus it.
    File { dir: PathBuf, name: String },
}

/// Interpret text typed into the path bar, relative to the panel's `cwd`.
/// Accepts quotes, `%VAR%`, `~`, `/` separators, relative paths and UNC.
pub fn resolve_input(input: &str, cwd: &Path) -> io::Result<Target> {
    let mut s = input.trim().trim_matches('"').trim().to_owned();
    if s.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "no path entered"));
    }
    s = expand_env(&s);
    if s == "~" || s.starts_with("~/") || s.starts_with("~\\") {
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
        s = format!("{home}{}", &s[1..]);
    }
    if cfg!(windows) {
        s = s.replace('/', "\\");
        let b = s.as_bytes();
        if b.len() == 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
            return Ok(Target::Drive(b[0].to_ascii_uppercase() as char));
        }
    }
    let p = PathBuf::from(&s);
    if unc_server(&p).is_some() {
        return Ok(Target::Dir(server_path(&unc_server(&p).unwrap())));
    }
    let joined = if p.is_absolute() || s.starts_with(r"\\") { p } else { cwd.join(p) };
    let full = std::path::absolute(&joined).unwrap_or(joined);
    if full.is_file() {
        let name = full.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let dir = full.parent().map(Path::to_path_buf).unwrap_or_else(|| cwd.to_path_buf());
        return Ok(Target::File { dir, name });
    }
    Ok(Target::Dir(full))
}

#[derive(Debug, Clone)]
pub struct Share {
    pub name: String,
    pub remark: String,
    /// Administrative/hidden share such as `C$`.
    pub special: bool,
}

/// Disk shares offered by `server` (blocks while the server is contacted).
#[cfg(windows)]
pub fn list_shares(server: &str) -> io::Result<Vec<Share>> {
    use windows_sys::Win32::NetworkManagement::NetManagement::{NetApiBufferFree, MAX_PREFERRED_LENGTH};
    use windows_sys::Win32::Storage::FileSystem::{NetShareEnum, SHARE_INFO_1, STYPE_SPECIAL};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let from_w = |p: *const u16| {
        if p.is_null() {
            return String::new();
        }
        let mut n = 0;
        // SAFETY: NUL-terminated strings owned by the NetShareEnum buffer
        unsafe {
            while *p.add(n) != 0 {
                n += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
        }
    };
    let srv = wide(&format!(r"\\{server}"));
    let mut buf: *mut u8 = std::ptr::null_mut();
    let (mut read, mut total, mut resume) = (0u32, 0u32, 0u32);
    let r = unsafe { NetShareEnum(srv.as_ptr(), 1, &mut buf, MAX_PREFERRED_LENGTH, &mut read, &mut total, &mut resume) };
    if r != 0 {
        if !buf.is_null() {
            unsafe { NetApiBufferFree(buf.cast()) };
        }
        return Err(match r {
            5 => io::Error::new(io::ErrorKind::PermissionDenied, format!(r"access to \\{server} denied")),
            53 => io::Error::new(io::ErrorKind::NotFound, format!(r"network path \\{server} not found")),
            _ => io::Error::other(format!(r"cannot list shares of \\{server} (error {r})")),
        });
    }
    let mut shares = Vec::new();
    // SAFETY: `read` SHARE_INFO_1 records allocated by NetShareEnum
    let items = unsafe { std::slice::from_raw_parts(buf as *const SHARE_INFO_1, read as usize) };
    for it in items {
        // only disk shares (type 0, possibly with the special bit)
        if it.shi1_type & !STYPE_SPECIAL != 0 {
            continue;
        }
        shares.push(Share { name: from_w(it.shi1_netname), remark: from_w(it.shi1_remark), special: it.shi1_type & STYPE_SPECIAL != 0 });
    }
    unsafe { NetApiBufferFree(buf.cast()) };
    shares.sort_by_key(|a| a.name.to_lowercase());
    Ok(shares)
}

#[cfg(not(windows))]
pub fn list_shares(_server: &str) -> io::Result<Vec<Share>> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "share browsing is only supported on Windows"))
}

/// Panel entries for a server's shares; administrative (`$`) shares only with `show_hidden`.
pub fn share_entries(server: &str, show_hidden: bool) -> io::Result<Vec<FileEntry>> {
    let shares = list_shares(server)?;
    Ok(shares
        .into_iter()
        .filter(|s| show_hidden || !s.special)
        .map(|s| {
            let path = PathBuf::from(format!(r"\\{server}\{}\", s.name));
            let attrs = ATTR_DIRECTORY | if s.special { ATTR_HIDDEN } else { 0 };
            let mut e = FileEntry::new(path, EntryKind::Dir, 0, None, attrs);
            e.ext_start = s.name.len();
            e.name = s.name;
            e
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unc_detection() {
        assert_eq!(unc_server(Path::new(r"\\nas")), Some("nas".into()));
        assert_eq!(unc_server(Path::new(r"\\nas\")), Some("nas".into()));
        assert_eq!(unc_server(Path::new("//nas")), Some("nas".into()));
        assert_eq!(unc_server(Path::new(r"\\nas\share")), None);
        assert_eq!(unc_server(Path::new(r"\\?\C:\")), None);
        assert_eq!(unc_server(Path::new(r"C:\")), None);
        assert_eq!(unc_share_root(Path::new(r"\\nas\data\")), Some(("nas".into(), "data".into())));
        assert_eq!(unc_share_root(Path::new(r"\\nas\data\x")), None);
    }

    #[cfg(windows)]
    #[test]
    fn parents() {
        assert_eq!(parent_of(Path::new(r"\\nas\data\")), Some((PathBuf::from(r"\\nas"), "data".into())));
        assert_eq!(parent_of(Path::new(r"\\nas\data\docs")), Some((PathBuf::from(r"\\nas\data\"), "docs".into())));
        assert_eq!(parent_of(Path::new(r"\\nas")), None);
        assert_eq!(parent_of(Path::new(r"C:\Users\ž")), Some((PathBuf::from(r"C:\Users"), "ž".into())));
        assert_eq!(parent_of(Path::new(r"C:\")), None);
    }

    #[cfg(windows)]
    #[test]
    fn crumbs() {
        let c = breadcrumbs(Path::new(r"C:\Users\Žluťoučký\Docs"));
        let labels: Vec<_> = c.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["C:", "Users", "Žluťoučký", "Docs"]);
        assert_eq!(c[0].1, PathBuf::from(r"C:\"));
        assert_eq!(c[2].1, PathBuf::from(r"C:\Users\Žluťoučký"));

        let c = breadcrumbs(Path::new(r"\\nas\data\a\b"));
        let labels: Vec<_> = c.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, [r"\\nas", "data", "a", "b"]);
        assert_eq!(c[0].1, PathBuf::from(r"\\nas"));
        assert_eq!(c[1].1, PathBuf::from(r"\\nas\data\"));
        assert_eq!(c[3].1, PathBuf::from(r"\\nas\data\a\b"));
        assert_eq!(breadcrumbs(Path::new(r"\\?\C:\x")).len(), 2);
        assert_eq!(breadcrumbs(Path::new(r"\\nas")).len(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn input_resolution() {
        let t = tempfile::tempdir().unwrap();
        let cwd = t.path();
        std::fs::create_dir_all(cwd.join("sub dir")).unwrap();
        std::fs::write(cwd.join("sub dir").join("f.txt"), "").unwrap();
        assert_eq!(resolve_input("d:", cwd).unwrap(), Target::Drive('D'));
        assert_eq!(resolve_input("  \"sub dir\"  ", cwd).unwrap(), Target::Dir(cwd.join("sub dir")));
        assert_eq!(resolve_input("sub dir/../sub dir", cwd).unwrap(), Target::Dir(cwd.join("sub dir")));
        assert_eq!(resolve_input("sub dir/f.txt", cwd).unwrap(), Target::File { dir: cwd.join("sub dir"), name: "f.txt".into() });
        assert_eq!(resolve_input(r"\\nas", cwd).unwrap(), Target::Dir(PathBuf::from(r"\\nas")));
        assert_eq!(resolve_input("//nas/", cwd).unwrap(), Target::Dir(PathBuf::from(r"\\nas")));
        assert_eq!(resolve_input(r"\\nas\data\docs", cwd).unwrap(), Target::Dir(PathBuf::from(r"\\nas\data\docs")));
        let win = std::env::var("SystemRoot").unwrap();
        assert_eq!(resolve_input("%SystemRoot%", cwd).unwrap(), Target::Dir(PathBuf::from(&win)));
        assert_eq!(resolve_input("%NO_SUCH_VAR_X%", cwd).unwrap(), Target::Dir(cwd.join("%NO_SUCH_VAR_X%")));
        assert!(resolve_input("   ", cwd).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn localhost_shares() {
        // the Server service may be off, or the machine policy may deny it
        let Ok(shares) = list_shares("localhost") else { return };
        let visible = share_entries("localhost", false).unwrap();
        assert!(visible.iter().all(|e| !e.name.ends_with('$')));
        assert!(visible.len() <= shares.len());
        for e in share_entries("localhost", true).unwrap() {
            assert_eq!(e.kind, EntryKind::Dir);
            assert!(e.path.to_string_lossy().starts_with(r"\\localhost\"));
        }
    }
}
