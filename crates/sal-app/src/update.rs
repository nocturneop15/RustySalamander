//! Updates: looking for a newer release on GitHub, and installing one next
//! to this version.
//!
//! The check asks the GitHub API for the repository's releases (alpha
//! releases are pre-releases, so those count too) and picks the newest one
//! whose version is higher than this program's and which has a Windows zip.
//!
//! Installing never touches the running version: the zip is downloaded,
//! unpacked into a new folder named by the version, next to this program's
//! folder (`…\RustySalamander\0.2.1-alpha` → `…\RustySalamander\0.3.0-alpha`),
//! and the settings are copied there: `config.toml` and the files plugins
//! keep in their folders (such as the SCP plugin's `known_hosts`).
//!
//! Downloads go through WinHTTP, so the system's proxy settings and
//! certificates apply.

use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use eframe::egui;

/// The GitHub repository releases come from.
pub const REPO: &str = "nocturneop15/RustySalamander";

/// A release newer than this program.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// "0.3.0-alpha" (the tag without its "v").
    pub version: String,
    /// The release's page on GitHub.
    pub page: String,
    /// The Windows zip: its name, download address and size.
    pub asset: String,
    pub asset_url: String,
    pub asset_size: u64,
}

pub type CheckResult = Result<Option<Release>, String>;

/// A check running in the background; `manual` when asked for from the menu
/// (then "no update" and errors are shown too).
pub struct Check {
    pub manual: bool,
    result: Arc<Mutex<Option<CheckResult>>>,
}

impl Check {
    pub fn start(ctx: &egui::Context, manual: bool) -> Check {
        let result = Arc::new(Mutex::new(None));
        let (out, ctx) = (result.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = check();
            crate::debuglog::write(format!("update check: {r:?}"));
            *out.lock().unwrap() = Some(r);
            ctx.request_repaint();
        });
        Check { manual, result }
    }

    /// The answer, once it has arrived.
    pub fn take(&self) -> Option<CheckResult> {
        self.result.lock().unwrap().take()
    }
}

/// Ask GitHub for a release newer than this program.
pub fn check() -> CheckResult {
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=30");
    let mut body = Vec::new();
    http_get(&url, "application/vnd.github+json", &mut |chunk, _| {
        body.extend_from_slice(chunk);
        Ok(())
    })?;
    newest(&String::from_utf8_lossy(&body), env!("CARGO_PKG_VERSION"))
}

/// The newest release in a GitHub API releases list that is newer than
/// `current` and has a Windows zip (drafts are skipped).
pub fn newest(json: &str, current: &str) -> CheckResult {
    let current = semver::Version::parse(current).map_err(|e| e.to_string())?;
    let list: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("GitHub: {e}"))?;
    let Some(list) = list.as_array() else {
        let msg = list.get("message").and_then(|m| m.as_str()).unwrap_or("unexpected answer");
        return Err(format!("GitHub: {msg}"));
    };
    let mut best: Option<(semver::Version, Release)> = None;
    for r in list {
        if r["draft"].as_bool() == Some(true) {
            continue;
        }
        let tag = r["tag_name"].as_str().unwrap_or_default();
        let Ok(v) = semver::Version::parse(tag.trim_start_matches('v')) else { continue };
        if v <= current || best.as_ref().is_some_and(|(b, _)| v <= *b) {
            continue;
        }
        let assets = r["assets"].as_array().map(Vec::as_slice).unwrap_or_default();
        let Some(zip) = assets.iter().find(|a| a["name"].as_str().is_some_and(|n| n.to_ascii_lowercase().ends_with("-win64.zip"))) else { continue };
        let release = Release {
            version: tag.trim_start_matches('v').to_owned(),
            page: r["html_url"].as_str().unwrap_or_default().to_owned(),
            asset: zip["name"].as_str().unwrap_or_default().to_owned(),
            asset_url: zip["browser_download_url"].as_str().unwrap_or_default().to_owned(),
            asset_size: zip["size"].as_u64().unwrap_or(0),
        };
        best = Some((v, release));
    }
    Ok(best.map(|(_, r)| r))
}

/// Where a release is installed: a folder named by its version, next to the
/// folder this program runs from.
pub fn target_dir(release: &Release) -> Option<PathBuf> {
    let app = sal_core::config::app_dir()?;
    Some(app.parent()?.join(&release.version))
}

// ---------------------------------------------------------------- installing

#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Downloading {
        done: u64,
        total: u64,
    },
    Unpacking,
    /// Installed in `folder`; `copied` are the settings files taken over.
    Done {
        folder: PathBuf,
        copied: Vec<String>,
    },
    Failed(String),
}

pub type Install = Arc<Mutex<Progress>>;

/// Download `release`, unpack it next to this version and copy the settings
/// over, on a background thread.
pub fn install(ctx: &egui::Context, release: Release) -> Install {
    let state = Arc::new(Mutex::new(Progress::Downloading { done: 0, total: release.asset_size }));
    let (out, ctx) = (state.clone(), ctx.clone());
    std::thread::spawn(move || {
        let set = |p: Progress| {
            *out.lock().unwrap() = p;
            ctx.request_repaint();
        };
        let r = (|| -> Result<Progress, String> {
            let app = sal_core::config::app_dir().ok_or("the program's folder is unknown")?;
            let target = target_dir(&release).ok_or("the program's folder has no parent folder")?;
            if target.exists() {
                return Err(format!("{} already exists.", target.display()));
            }
            let mut zip = Vec::with_capacity(release.asset_size as usize);
            let mut last = 0;
            http_get(&release.asset_url, "application/octet-stream", &mut |chunk, total| {
                zip.extend_from_slice(chunk);
                // a repaint per 64 KB is plenty
                if zip.len() - last >= 64 * 1024 {
                    last = zip.len();
                    set(Progress::Downloading { done: zip.len() as u64, total: total.unwrap_or(release.asset_size) });
                }
                Ok(())
            })?;
            if release.asset_size != 0 && zip.len() as u64 != release.asset_size {
                return Err(format!("the download is incomplete ({} of {} bytes)", zip.len(), release.asset_size));
            }
            set(Progress::Unpacking);
            unpack_release(&zip, &target)?;
            let copied = migrate(&app, &target).map_err(|e| format!("copying the settings: {e}"))?;
            Ok(Progress::Done { folder: target, copied })
        })();
        crate::debuglog::write(format!("update install: {r:?}"));
        set(r.unwrap_or_else(Progress::Failed));
    });
    state
}

/// Unpack a release zip into `target` (which must not exist yet): into a
/// hidden folder next to it first, renamed when complete, so a failure
/// leaves no half-installed version behind. The zip's single top folder
/// (`RustySalamander-<version>-win64`) is left out.
pub fn unpack_release(zip: &[u8], target: &Path) -> Result<(), String> {
    let entries = zip_entries(zip)?;
    let name = target.file_name().ok_or("no folder name")?.to_string_lossy();
    let stage = target.with_file_name(format!(".{name}.partial"));
    let _ = fs::remove_dir_all(&stage);
    let top = common_top(&entries);
    let r = (|| -> Result<(), String> {
        for e in &entries {
            let rel = match &top {
                Some(t) => e.name.strip_prefix(t.as_str()).unwrap_or(&e.name),
                None => &e.name,
            };
            let rel = safe_path(rel).ok_or_else(|| format!("unsafe name in the zip: {}", e.name))?;
            if rel.as_os_str().is_empty() {
                continue;
            }
            let path = stage.join(rel);
            if e.name.ends_with('/') {
                fs::create_dir_all(&path).map_err(|err| format!("{}: {err}", path.display()))?;
                continue;
            }
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
            }
            let data = zip_read(zip, e)?;
            fs::write(&path, data).map_err(|err| format!("{}: {err}", path.display()))?;
        }
        if !stage.join("rustysalamander.exe").is_file() {
            return Err("the zip has no rustysalamander.exe".into());
        }
        fs::rename(&stage, target).map_err(|err| format!("{}: {err}", target.display()))
    })();
    if r.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    r
}

/// Copy the settings from the program folder `old` into `new`: `config.toml`
/// and, in each plugin's folder, the files the new version doesn't bring
/// (what the plugin saved there, such as `known_hosts`), never programs.
/// Returns the copied files, relative to the folder.
pub fn migrate(old: &Path, new: &Path) -> io::Result<Vec<String>> {
    let mut copied = Vec::new();
    if old.join("config.toml").is_file() {
        fs::copy(old.join("config.toml"), new.join("config.toml"))?;
        copied.push("config.toml".to_owned());
    }
    fn walk(old: &Path, new: &Path, rel: &Path, copied: &mut Vec<String>) -> io::Result<()> {
        let Ok(dir) = fs::read_dir(old.join(rel)) else { return Ok(()) };
        for de in dir.flatten() {
            let rel = rel.join(de.file_name());
            if de.file_type()?.is_dir() {
                // translations come with the program
                if de.file_name() != "lang" {
                    walk(old, new, &rel, copied)?;
                }
                continue;
            }
            let ext = rel.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let to = new.join(&rel);
            if matches!(ext.as_str(), "spl" | "dll" | "exe") || to.exists() {
                continue;
            }
            if let Some(dir) = to.parent() {
                fs::create_dir_all(dir)?;
            }
            fs::copy(old.join(&rel), &to)?;
            copied.push(rel.to_string_lossy().into_owned());
        }
        Ok(())
    }
    walk(old, new, Path::new("plugins"), &mut copied)?;
    Ok(copied)
}

/// A relative path from a zip entry name, or `None` for names that would
/// leave the folder ("..", absolute paths, drive letters).
fn safe_path(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(name.replace('\\', "/").trim_end_matches('/'));
    p.components().all(|c| matches!(c, Component::Normal(_))).then_some(p)
}

/// "Top/" when every entry is inside that one folder.
fn common_top(entries: &[ZipEntry]) -> Option<String> {
    let first = entries.first()?.name.split('/').next()?;
    let top = format!("{first}/");
    entries.iter().all(|e| e.name.starts_with(&top)).then_some(top)
}

// ---------------------------------------------------------------- zip

/// An entry of a zip's central directory.
#[derive(Debug)]
struct ZipEntry {
    name: String,
    method: u16,
    crc: u32,
    packed: usize,
    size: usize,
    header: usize,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The entries of a zip (stored or deflated, no zip64: release zips are a
/// few megabytes).
fn zip_entries(zip: &[u8]) -> Result<Vec<ZipEntry>, String> {
    let bad = || "the download is not a valid zip".to_owned();
    // end of central directory: 22 bytes plus a comment of up to 64 KB
    let from = zip.len().saturating_sub(22 + 0xFFFF);
    let end = (from..zip.len().saturating_sub(21)).rev().find(|&i| u32_at(zip, i) == Some(0x0605_4b50)).ok_or_else(bad)?;
    let count = u16_at(zip, end + 10).ok_or_else(bad)? as usize;
    let mut at = u32_at(zip, end + 16).ok_or_else(bad)? as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if u32_at(zip, at) != Some(0x0201_4b50) {
            return Err(bad());
        }
        let field = |off| u16_at(zip, at + off).map(usize::from).ok_or_else(bad);
        let (name_len, extra_len, comment_len) = (field(28)?, field(30)?, field(32)?);
        let name = zip.get(at + 46..at + 46 + name_len).ok_or_else(bad)?;
        out.push(ZipEntry {
            name: String::from_utf8_lossy(name).replace('\\', "/"),
            method: u16_at(zip, at + 10).ok_or_else(bad)?,
            crc: u32_at(zip, at + 16).ok_or_else(bad)?,
            packed: u32_at(zip, at + 20).ok_or_else(bad)? as usize,
            size: u32_at(zip, at + 24).ok_or_else(bad)? as usize,
            header: u32_at(zip, at + 42).ok_or_else(bad)? as usize,
        });
        at += 46 + name_len + extra_len + comment_len;
    }
    Ok(out)
}

/// An entry's contents, checked against its CRC.
fn zip_read(zip: &[u8], e: &ZipEntry) -> Result<Vec<u8>, String> {
    let bad = || format!("{}: damaged in the zip", e.name);
    if u32_at(zip, e.header) != Some(0x0403_4b50) {
        return Err(bad());
    }
    let start = e.header + 30 + u16_at(zip, e.header + 26).ok_or_else(bad)? as usize + u16_at(zip, e.header + 28).ok_or_else(bad)? as usize;
    let packed = zip.get(start..start + e.packed).ok_or_else(bad)?;
    let data = match e.method {
        0 => packed.to_vec(),
        8 => {
            let mut out = Vec::with_capacity(e.size);
            flate2::read::DeflateDecoder::new(packed).read_to_end(&mut out).map_err(|_| bad())?;
            out
        }
        m => return Err(format!("{}: unsupported compression ({m})", e.name)),
    };
    let mut crc = flate2::Crc::new();
    crc.update(&data);
    if data.len() != e.size || crc.sum() != e.crc {
        return Err(bad());
    }
    Ok(data)
}

// ---------------------------------------------------------------- HTTP

/// Takes the body of a download piece by piece, with its full length when
/// the server says it.
type Sink<'a> = dyn FnMut(&[u8], Option<u64>) -> Result<(), String> + 'a;

/// GET an https address, handing the body to `sink` in pieces together with
/// the full length when the server says it. Redirects are followed.
#[cfg(windows)]
fn http_get(url: &str, accept: &str, sink: &mut Sink) -> Result<(), String> {
    use std::ffi::c_void;
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Networking::WinHttp::*;

    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
    let open = |h: *mut c_void, what: &str| -> Result<Handle, String> {
        if h.is_null() {
            Err(format!("{what}: {}", windows::core::Error::from_win32().message()))
        } else {
            Ok(Handle(h))
        }
    };
    let err = |e: windows::core::Error| format!("{url}: {}", e.message().trim_end());

    let rest = url.strip_prefix("https://").ok_or_else(|| format!("{url}: not an https address"))?;
    let (host, path) = rest.split_once('/').map_or((rest, "/".to_owned()), |(h, p)| (h, format!("/{p}")));
    let agent = HSTRING::from(format!("RustySalamander/{}", env!("CARGO_PKG_VERSION")));
    unsafe {
        let session = open(WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0), "WinHTTP")?;
        let _ = WinHttpSetTimeouts(session.0, 15_000, 15_000, 30_000, 30_000);
        let conn = open(WinHttpConnect(session.0, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT, 0), host)?;
        let req = open(WinHttpOpenRequest(conn.0, &HSTRING::from("GET"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE), url)?;
        let headers: Vec<u16> = format!("Accept: {accept}\r\n").encode_utf16().collect();
        WinHttpSendRequest(req.0, Some(&headers), None, 0, 0, 0).map_err(err)?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(err)?;

        let number = |what: u32| -> Result<u64, String> {
            let mut v = 0u32;
            let mut len = 4u32;
            WinHttpQueryHeaders(req.0, what | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some((&mut v as *mut u32).cast()), &mut len, std::ptr::null_mut()).map_err(err)?;
            Ok(u64::from(v))
        };
        let status = number(WINHTTP_QUERY_STATUS_CODE)?;
        if status != 200 {
            return Err(format!("{url}: the server answered {status}"));
        }
        let total = number(WINHTTP_QUERY_CONTENT_LENGTH).ok();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut n = 0u32;
            WinHttpReadData(req.0, buf.as_mut_ptr().cast(), buf.len() as u32, &mut n).map_err(err)?;
            if n == 0 {
                return Ok(());
            }
            sink(&buf[..n as usize], total)?;
        }
    }
}

#[cfg(not(windows))]
fn http_get(url: &str, _accept: &str, _sink: &mut Sink) -> Result<(), String> {
    Err(format!("{url}: downloads work on Windows only"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"[
        {"tag_name": "v0.3.0-alpha", "draft": true, "html_url": "d", "assets": [{"name": "RustySalamander-0.3.0-alpha-win64.zip", "browser_download_url": "x", "size": 1}]},
        {"tag_name": "v0.2.2-alpha", "draft": false, "html_url": "https://github.com/r/releases/tag/v0.2.2-alpha",
         "assets": [{"name": "notes.txt", "browser_download_url": "n", "size": 5},
                    {"name": "RustySalamander-0.2.2-alpha-win64.zip", "browser_download_url": "https://github.com/r/z.zip", "size": 7126108}]},
        {"tag_name": "v0.2.3-beta", "draft": false, "html_url": "p", "assets": []},
        {"tag_name": "v0.2.1-alpha", "draft": false, "html_url": "o", "assets": [{"name": "RustySalamander-0.2.1-alpha-win64.zip", "browser_download_url": "o", "size": 1}]},
        {"tag_name": "nightly", "draft": false, "html_url": "q", "assets": []}
    ]"#;

    #[test]
    fn newest_release_is_picked() {
        // drafts and releases without a zip are skipped
        let r = newest(LIST, "0.2.1-alpha").unwrap().unwrap();
        assert_eq!(r.version, "0.2.2-alpha");
        assert_eq!(r.page, "https://github.com/r/releases/tag/v0.2.2-alpha");
        assert_eq!((r.asset_url.as_str(), r.asset_size), ("https://github.com/r/z.zip", 7126108));
        // nothing newer
        assert_eq!(newest(LIST, "0.2.2-alpha").unwrap(), None);
        assert_eq!(newest(LIST, "0.2.2").unwrap(), None, "a final release is newer than its alphas");
        assert!(newest(r#"{"message": "API rate limit exceeded"}"#, "0.1.0").unwrap_err().contains("rate limit"));
    }

    /// A zip in the release's layout, written by a real zip writer
    /// (PowerShell's Compress-Archive, as the releases are).
    #[cfg(windows)]
    fn release_zip(dir: &Path, version: &str) -> Vec<u8> {
        let top = dir.join(format!("RustySalamander-{version}-win64"));
        fs::create_dir_all(top.join("plugins").join("scp").join("lang")).unwrap();
        fs::write(top.join("rustysalamander.exe"), vec![7u8; 100_000]).unwrap();
        fs::write(top.join("README.md"), "čtení ".repeat(500)).unwrap();
        fs::write(top.join("plugins").join("scp").join("scp.spl"), "new plugin").unwrap();
        fs::write(top.join("plugins").join("scp").join("lang").join("cs.toml"), "x = 1").unwrap();
        let zip = dir.join("r.zip");
        let ok = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &format!("Compress-Archive -Path '{}' -DestinationPath '{}'", top.display(), zip.display())])
            .status()
            .unwrap()
            .success();
        assert!(ok);
        fs::read(zip).unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn release_unpacks_and_settings_move_over() {
        let t = tempfile::tempdir().unwrap();
        let zip = release_zip(t.path(), "0.9.0");
        let target = t.path().join("apps").join("0.9.0");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        unpack_release(&zip, &target).unwrap();
        assert_eq!(fs::read(target.join("rustysalamander.exe")).unwrap(), vec![7u8; 100_000]);
        assert_eq!(fs::read_to_string(target.join("README.md")).unwrap(), "čtení ".repeat(500));
        assert!(target.join("plugins/scp/lang/cs.toml").is_file());
        assert!(!t.path().join("apps").join(".0.9.0.partial").exists(), "the staging folder is renamed");

        // a damaged download leaves nothing behind
        let mut broken = zip.clone();
        let mid = broken.len() / 3;
        broken[mid] ^= 0xFF;
        let other = t.path().join("apps").join("0.9.1");
        assert!(unpack_release(&broken, &other).is_err());
        assert!(!other.exists() && !t.path().join("apps").join(".0.9.1.partial").exists());

        // the old version's settings: config and what the plugin saved
        let old = t.path().join("apps").join("0.8.0");
        fs::create_dir_all(old.join("plugins").join("scp").join("lang")).unwrap();
        fs::write(old.join("config.toml"), "theme = \"Dark\"").unwrap();
        fs::write(old.join("rustysalamander.exe"), "old").unwrap();
        fs::write(old.join("plugins").join("scp").join("known_hosts"), "host key").unwrap();
        fs::write(old.join("plugins").join("scp").join("last-connection"), "server").unwrap();
        fs::write(old.join("plugins").join("scp").join("scp.spl"), "old plugin").unwrap();
        fs::write(old.join("plugins").join("scp").join("lang").join("cs.toml"), "old").unwrap();
        let mut copied = migrate(&old, &target).unwrap();
        copied.sort();
        assert_eq!(copied, ["config.toml", r"plugins\scp\known_hosts", r"plugins\scp\last-connection"]);
        assert_eq!(fs::read_to_string(target.join("config.toml")).unwrap(), "theme = \"Dark\"");
        assert_eq!(fs::read_to_string(target.join("plugins/scp/scp.spl")).unwrap(), "new plugin", "programs are never copied back");
        assert_eq!(fs::read_to_string(target.join("plugins/scp/lang/cs.toml")).unwrap(), "x = 1", "the new version's files win");
        assert_eq!(fs::read(target.join("rustysalamander.exe")).unwrap(), vec![7u8; 100_000]);
    }

    #[test]
    fn unsafe_names_are_refused() {
        assert_eq!(safe_path("a/b.txt"), Some(PathBuf::from("a/b.txt")));
        assert_eq!(safe_path("dir/"), Some(PathBuf::from("dir")));
        assert!(safe_path("../evil.exe").is_none());
        assert!(safe_path("a/../../evil.exe").is_none());
        assert!(safe_path("/abs").is_none());
        assert!(safe_path(r"C:\Windows\x").is_none());
    }

    /// The real thing: GitHub's release list (needs the internet).
    #[test]
    #[ignore]
    fn live_check() {
        let list = {
            let mut body = Vec::new();
            http_get(&format!("https://api.github.com/repos/{REPO}/releases"), "application/vnd.github+json", &mut |c, _| {
                body.extend_from_slice(c);
                Ok(())
            })
            .unwrap();
            String::from_utf8(body).unwrap()
        };
        let r = newest(&list, "0.0.1").unwrap().expect("a release with a zip");
        assert!(r.asset_size > 1_000_000, "{r:?}");
        let mut got = 0u64;
        http_get(&r.asset_url, "application/octet-stream", &mut |c, _| {
            got += c.len() as u64;
            Ok(())
        })
        .unwrap();
        assert_eq!(got, r.asset_size, "the redirect to the download host is followed");
    }
}
