//! Updates: looking for newer releases of the program and of its plugins on
//! GitHub, showing what changed, and installing them.
//!
//! The check asks the GitHub API for the releases of each repository that
//! publishes something installed (alpha releases are pre-releases, so those
//! count too):
//!
//! - **the program**: tags `v<version>` with a `RustySalamander-…-win64.zip`;
//! - **each plugin** that says where it is published
//!   ([`sal_plugin_api::Updates`]): tags `<prefix><version>` with a zip
//!   `<id>-<version>-api<interface>.zip`, offered only for an interface
//!   version this program has.
//!
//! **Official plugins not installed yet** are offered too: the list of them,
//! [`OFFICIAL`] in the program's repository, is read from its main branch
//! at each check, so a new plugin reaches every program as soon as it is
//! on the list and released, without a new program. A plugin the user
//! turned down ([`Offer::declined`]) isn't offered by the check at start.
//!
//! The changelog of an update is read from the repository at the release's
//! tag: the sections between the installed version and the new one.
//!
//! Installing the program never touches the running version: the zip is
//! downloaded, unpacked into a new folder named by the version, next to this
//! program's folder (`…\RustySalamander\0.2.1-alpha` →
//! `…\RustySalamander\0.3.0-alpha`), and the settings are copied there:
//! `config.toml` and the files plugins keep in their folders (such as the
//! SCP plugin's `known_hosts`).
//!
//! A plugin is updated in place, but its DLL is in use while the program
//! runs: its zip is unpacked into `plugins\.updates\<folder>`, and the next
//! start moves its files into the plugin's folder before loading the plugins
//! ([`at_start`]). What the plugin saved in its folder stays.
//!
//! Files are replaced by renames only ([`replace_files`]), with a plan
//! written first, so an update stopped half way (a crash, the power) is
//! finished or undone at the next start; one running copy of the program
//! updates at a time ([`lock`]).
//!
//! Downloads go through WinHTTP, so the system's proxy settings and
//! certificates apply.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use eframe::egui;
use sal_plugin_api::abi;

/// The GitHub repository the program's releases come from.
pub const REPO: &str = "nocturneop15/RustySalamander";

/// The folder in `plugins` where downloaded plugin updates wait for the
/// next start.
pub const STAGED: &str = ".updates";

/// The list of official plugins, in [`REPO`] on its main branch.
pub const OFFICIAL: &str = "plugins/official.toml";

/// An official plugin, as [`OFFICIAL`] describes it.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct Official {
    /// Its id, which is also its folder in `plugins` ("ftp").
    pub id: String,
    /// What the tags of its releases start with ("ftp-v").
    pub tag: String,
    /// Its changelog's path in the repository.
    #[serde(default)]
    pub changelog: String,
    /// Its name and a sentence about it, by language code ("en", "cs").
    pub name: BTreeMap<String, String>,
    #[serde(default)]
    pub description: BTreeMap<String, String>,
}

#[derive(serde::Deserialize)]
struct OfficialList {
    #[serde(default)]
    plugin: Vec<Official>,
}

/// The official plugins in the text of [`OFFICIAL`]; ids that can't be a
/// folder name are left out.
pub fn parse_official(text: &str) -> Result<Vec<Official>, String> {
    let list: OfficialList = toml::from_str(text).map_err(|e| format!("{OFFICIAL}: {e}"))?;
    Ok(list.plugin.into_iter().filter(|p| !p.id.is_empty() && p.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')).collect())
}

/// The text for the language `lang`, else English, else any.
fn in_language(texts: &BTreeMap<String, String>, lang: &str) -> String {
    texts.get(lang).or_else(|| texts.get("en")).or_else(|| texts.values().next()).cloned().unwrap_or_default()
}

/// What the check offers besides updates: the official plugins that aren't
/// in `plugins` (the plugins folder) yet, named in `lang`, except those in
/// `declined`.
#[derive(Debug, Clone, Default)]
pub struct Offer {
    pub plugins: PathBuf,
    pub lang: String,
    /// Plugins the user turned down (by id).
    pub declined: Vec<String>,
}

/// A release newer than what is installed.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// "0.3.0-alpha" (the tag without its prefix).
    pub version: String,
    /// The tag ("v0.3.0-alpha", "viewer-v0.6.0").
    pub tag: String,
    /// The release's page on GitHub.
    pub page: String,
    /// The zip: its name, download address and size.
    pub asset: String,
    pub asset_url: String,
    pub asset_size: u64,
    /// The program versions a plugin release works with, from its notes
    /// ([`program_range`]); empty for no limit.
    pub program_min: String,
    pub program_max: String,
}

/// The line in a plugin release's notes saying which program versions it
/// works with: `<!-- rustysalamander-program: 0.6.0 .. 0.9.0 -->`, either
/// side empty for no limit. Written when the release is published, from the
/// plugin's [`sal_plugin_api::Plugin::PROGRAM_MIN`] and `PROGRAM_MAX`.
pub const PROGRAM_MARK: &str = "rustysalamander-program:";

/// The program versions a release's notes say it works with (`min`, `max`);
/// empty for no limit, also when the notes say nothing.
pub fn program_range(notes: &str) -> (String, String) {
    let Some(at) = notes.find(PROGRAM_MARK) else { return Default::default() };
    let rest = &notes[at + PROGRAM_MARK.len()..];
    let line = rest.split(['\n', '\r']).next().unwrap_or_default();
    let line = line.split("-->").next().unwrap_or_default();
    match line.split_once("..") {
        Some((min, max)) => (min.trim().to_owned(), max.trim().to_owned()),
        None => (line.trim().to_owned(), String::new()),
    }
}

/// What an update is for.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// RustySalamander itself.
    Program,
    /// A plugin: its id, its name as shown, and its folder.
    Plugin { id: String, name: String, dir: PathBuf },
}

/// Something installed that has a newer release, or an official plugin
/// not installed yet ([`Update::new`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub target: Target,
    /// The version installed; empty for a new plugin.
    pub current: String,
    /// A plugin not installed yet, offered from the official list.
    pub new: bool,
    /// What a new plugin does (from the official list); empty otherwise.
    pub about: String,
    pub release: Release,
    /// The repository, and the changelog's path in it ("" for none).
    pub repo: String,
    pub changelog: String,
}

impl Update {
    /// Whether this program can run it; if not, why (a plugin made for other
    /// program versions).
    pub fn fits(&self) -> Result<(), String> {
        match self.target {
            Target::Program => Ok(()),
            Target::Plugin { .. } => crate::plugins::program_fits(&self.release.program_min, &self.release.program_max),
        }
    }

    /// "RustySalamander", or the plugin's name.
    pub fn name(&self) -> &str {
        match &self.target {
            Target::Program => "RustySalamander",
            Target::Plugin { name, .. } => name,
        }
    }
}

/// A plugin as the check sees it.
#[derive(Debug, Clone)]
pub struct PluginSource {
    pub id: String,
    pub name: String,
    pub dir: PathBuf,
    pub version: String,
    pub source: sal_plugin_api::host::Source,
}

/// The updates found (empty when everything is up to date).
pub type CheckResult = Result<Vec<Update>, String>;

/// A check running in the background; `manual` when asked for from the menu
/// (then "no update" and errors are shown too).
pub struct Check {
    pub manual: bool,
    result: Arc<Mutex<Option<CheckResult>>>,
}

impl Check {
    /// Look for updates of the program and of `plugins`, and for the
    /// official plugins `offer` says, on a thread.
    pub fn start(ctx: &egui::Context, manual: bool, plugins: Vec<PluginSource>, offer: Option<Offer>) -> Check {
        let result = Arc::new(Mutex::new(None));
        let (out, ctx) = (result.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = check(&plugins, offer.as_ref(), &mut |repo| releases(repo), &mut official_list);
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

/// The release list of a repository, as the GitHub API sends it.
fn releases(repo: &str) -> Result<String, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page=100");
    let mut body = Vec::new();
    http_get(&url, "application/vnd.github+json", &mut |chunk, _| {
        body.extend_from_slice(chunk);
        Ok(())
    })?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// The text of [`OFFICIAL`] on the main branch.
fn official_list() -> Result<String, String> {
    let url = format!("https://raw.githubusercontent.com/{REPO}/main/{OFFICIAL}");
    let mut body = Vec::new();
    http_get(&url, "text/plain", &mut |chunk, _| {
        body.extend_from_slice(chunk);
        Ok(())
    })?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Look for updates of the program and of `plugins`, and for the official
/// plugins not installed yet when `offer` is given; `list` fetches the
/// release list of a repository (once per repository), `official` the list
/// of official plugins. A repository that can't be read leaves its plugins
/// out, and an official list that can't be read leaves out new plugins;
/// only when nothing could be checked is that an error.
pub fn check(
    plugins: &[PluginSource],
    offer: Option<&Offer>,
    list: &mut dyn FnMut(&str) -> Result<String, String>,
    official: &mut dyn FnMut() -> Result<String, String>,
) -> CheckResult {
    let mut lists: BTreeMap<String, Result<String, String>> = BTreeMap::new();
    let mut get = |repo: &str| lists.entry(repo.to_owned()).or_insert_with(|| list(repo)).clone();
    let mut found = Vec::new();
    let program = get(REPO).and_then(|json| newest(&json, "v", env!("CARGO_PKG_VERSION"), &|name| program_zip(name)));
    let mut error = None;
    match program {
        Ok(Some(release)) => found.push(Update {
            target: Target::Program,
            current: env!("CARGO_PKG_VERSION").into(),
            new: false,
            about: String::new(),
            release,
            repo: REPO.into(),
            changelog: "CHANGELOG.md".into(),
        }),
        Ok(None) => {}
        Err(e) => error = Some(e),
    }
    let mut checked_any = error.is_none();
    for p in plugins {
        let id = p.id.clone();
        let r = get(&p.source.repo).and_then(|json| newest(&json, &p.source.tag, &p.version, &|name| plugin_zip(&id, name)));
        match r {
            Ok(Some(release)) => {
                checked_any = true;
                found.push(Update {
                    target: Target::Plugin { id: p.id.clone(), name: p.name.clone(), dir: p.dir.clone() },
                    current: p.version.clone(),
                    new: false,
                    about: String::new(),
                    release,
                    repo: p.source.repo.clone(),
                    changelog: p.source.changelog.clone(),
                });
            }
            Ok(None) => checked_any = true,
            Err(e) => {
                crate::debuglog::write(format!("update check of {}: {e}", p.id));
                error.get_or_insert(e);
            }
        }
    }
    if let Some(offer) = offer {
        match official().and_then(|text| parse_official(&text)) {
            Ok(list) => {
                for o in list {
                    // installed (loaded or not), waiting for the next start, or turned down
                    let dir = offer.plugins.join(&o.id);
                    if plugins.iter().any(|p| p.id == o.id) || dir.exists() || offer.plugins.join(STAGED).join(&o.id).exists() || offer.declined.contains(&o.id) {
                        continue;
                    }
                    let id = o.id.clone();
                    match get(REPO).and_then(|json| newest(&json, &o.tag, "0.0.0-0", &|name| plugin_zip(&id, name))) {
                        Ok(Some(release)) => found.push(Update {
                            target: Target::Plugin { id: o.id.clone(), name: in_language(&o.name, &offer.lang), dir },
                            current: String::new(),
                            new: true,
                            about: in_language(&o.description, &offer.lang),
                            release,
                            repo: REPO.into(),
                            changelog: o.changelog.clone(),
                        }),
                        Ok(None) => {}
                        Err(e) => crate::debuglog::write(format!("update check of the new plugin {id}: {e}")),
                    }
                }
            }
            Err(e) => crate::debuglog::write(format!("the official plugins: {e}")),
        }
    }
    match error {
        Some(e) if !checked_any => Err(e),
        _ => Ok(found),
    }
}

/// Whether `name` is the program's Windows zip.
fn program_zip(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("rustysalamander-") && n.ends_with("-win64.zip")
}

/// Whether `name` is a zip of the plugin `id` built for an interface this
/// program has: `<id>-<version>-api<n>.zip`.
fn plugin_zip(id: &str, name: &str) -> bool {
    let Some(rest) = name.strip_prefix(&format!("{id}-")).and_then(|r| r.strip_suffix(".zip")) else { return false };
    let Some((_, api)) = rest.rsplit_once("-api") else { return false };
    api.parse::<u32>().is_ok_and(|n| (abi::MIN_VERSION..=abi::VERSION).contains(&n))
}

/// The newest release in a GitHub API releases list whose tag is `prefix`
/// and a version higher than `current`, with a zip `accept` takes (drafts
/// are skipped).
pub fn newest(json: &str, prefix: &str, current: &str, accept: &dyn Fn(&str) -> bool) -> Result<Option<Release>, String> {
    let current = semver::Version::parse(current).map_err(|e| format!("{current}: {e}"))?;
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
        let Some(version) = tag.strip_prefix(prefix) else { continue };
        let Ok(v) = semver::Version::parse(version) else { continue };
        if v <= current || best.as_ref().is_some_and(|(b, _)| v <= *b) {
            continue;
        }
        let assets = r["assets"].as_array().map(Vec::as_slice).unwrap_or_default();
        let Some(zip) = assets.iter().find(|a| a["name"].as_str().is_some_and(accept)) else { continue };
        let (program_min, program_max) = program_range(r["body"].as_str().unwrap_or_default());
        let release = Release {
            program_min,
            program_max,
            version: version.to_owned(),
            tag: tag.to_owned(),
            page: r["html_url"].as_str().unwrap_or_default().to_owned(),
            asset: zip["name"].as_str().unwrap_or_default().to_owned(),
            asset_url: zip["browser_download_url"].as_str().unwrap_or_default().to_owned(),
            asset_size: zip["size"].as_u64().unwrap_or(0),
        };
        best = Some((v, release));
    }
    Ok(best.map(|(_, r)| r))
}

/// Where a program release is installed: a folder named by its version,
/// next to the folder this program runs from.
pub fn target_dir(release: &Release) -> Option<PathBuf> {
    let app = sal_core::config::app_dir()?;
    Some(app.parent()?.join(&release.version))
}

// ---------------------------------------------------------------- changelog

/// A changelog being read in the background.
pub type Changelog = Arc<Mutex<Option<Result<String, String>>>>;

/// Read what changed from `update.current` to the new version, from the
/// changelog at the release's tag.
pub fn changelog(ctx: &egui::Context, update: &Update) -> Changelog {
    let out: Changelog = Arc::new(Mutex::new(None));
    let (slot, ctx, u) = (out.clone(), ctx.clone(), update.clone());
    std::thread::spawn(move || {
        let r = if u.changelog.is_empty() {
            Err("there is no changelog".to_owned())
        } else {
            let url = format!("https://raw.githubusercontent.com/{}/{}/{}", u.repo, u.release.tag, u.changelog);
            let mut body = Vec::new();
            http_get(&url, "text/plain", &mut |c, _| {
                body.extend_from_slice(c);
                Ok(())
            })
            .map(|()| changes_between(&String::from_utf8_lossy(&body), if u.new { "0.0.0-0" } else { &u.current }, &u.release.version))
        };
        *slot.lock().unwrap() = Some(r);
        ctx.request_repaint();
    });
    out
}

/// The sections of a changelog (`## <version> …` headings) for versions
/// after `current` up to `new`, newest first as written.
pub fn changes_between(text: &str, current: &str, new: &str) -> String {
    let (Ok(current), Ok(new)) = (semver::Version::parse(current), semver::Version::parse(new)) else { return String::new() };
    let mut out = String::new();
    let mut keep = false;
    for line in text.lines() {
        if let Some(head) = line.strip_prefix("## ") {
            let v = head.split_whitespace().next().unwrap_or_default().trim_start_matches('v');
            keep = semver::Version::parse(v).is_ok_and(|v| v > current && v <= new);
        }
        if keep {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim_end().to_owned()
}

// ---------------------------------------------------------------- installing

#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Downloading {
        done: u64,
        total: u64,
    },
    Unpacking,
    /// The program installed in `folder`; `copied` are the settings files
    /// (and the user's own plugins and languages) taken over. `in_place`:
    /// this program's folder was updated, a restart starts the new version.
    Done {
        folder: PathBuf,
        copied: Vec<String>,
        in_place: bool,
    },
    /// A plugin's update waits for the next start.
    Staged,
    Failed(String),
}

pub type Install = Arc<Mutex<Progress>>;

/// How a new version of the program is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// Over this version, in its folder: settings and every plugin
    /// (including the user's own) stay where they are.
    InPlace,
    /// Into a new folder next to this one, which stays as it is; the
    /// settings are copied, and with `own` also the plugins and languages
    /// the new version doesn't bring (the user's own).
    NewFolder { own: bool },
}

/// Download and install `update` on a background thread: the program as
/// `how` says, a plugin staged for the next start.
pub fn install(ctx: &egui::Context, update: Update, how: How) -> Install {
    let release = update.release.clone();
    let state = Arc::new(Mutex::new(Progress::Downloading { done: 0, total: release.asset_size }));
    let (out, ctx) = (state.clone(), ctx.clone());
    std::thread::spawn(move || {
        let set = |p: Progress| {
            *out.lock().unwrap() = p;
            ctx.request_repaint();
        };
        let r = (|| -> Result<Progress, String> {
            let target = match &update.target {
                Target::Program if how == How::InPlace => {
                    let app = sal_core::config::app_dir().ok_or("the program's folder is unknown")?;
                    if !writable(&app) {
                        return Err(format!(
                            "the program's folder {} can't be written to. Install the new version in a new folder, or start RustySalamander as administrator.",
                            app.display()
                        ));
                    }
                    app
                }
                Target::Program => {
                    let target = target_dir(&release).ok_or("the program's folder has no parent folder")?;
                    if target.exists() {
                        return Err(format!("{} already exists.", target.display()));
                    }
                    target
                }
                Target::Plugin { dir, .. } => staged_dir(dir).ok_or("the plugin's folder is unknown")?,
            };
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
            // one install of this program at a time (they wait their turn:
            // the lock below is the same file for all of them, and would make
            // the others give up); then one running copy of the program at a
            // time changes files, nor while another is loading its plugins
            // (only while it changes them: not while it downloads)
            let _turn = INSTALL_TURN.lock().unwrap_or_else(|e| e.into_inner());
            let _lock = match sal_core::config::app_dir() {
                Some(app) => lock_waiting(&app, LOCK_WAIT)?,
                None => None,
            };
            match (&update.target, how) {
                (Target::Program, How::InPlace) => {
                    install_in_place(&zip, &target)?;
                    Ok(Progress::Done { folder: target, copied: Vec::new(), in_place: true })
                }
                (Target::Program, How::NewFolder { own }) => {
                    unpack_release(&zip, &target)?;
                    let app = sal_core::config::app_dir().ok_or("the program's folder is unknown")?;
                    let copied = migrate(&app, &target, own).map_err(|e| format!("copying the settings: {e}"))?;
                    Ok(Progress::Done { folder: target, copied, in_place: false })
                }
                (Target::Plugin { .. }, _) => {
                    let _ = fs::remove_dir_all(&target);
                    if let Some(dir) = target.parent() {
                        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                    }
                    unpack_plugin(&zip, &target)?;
                    Ok(Progress::Staged)
                }
            }
        })();
        crate::debuglog::write(format!("update install of {}: {r:?}", update.name()));
        set(r.unwrap_or_else(Progress::Failed));
    });
    state
}

/// Whether files can be made in `dir`.
fn writable(dir: &Path) -> bool {
    let probe = dir.join(".update-write-test");
    let ok = fs::write(&probe, b"").is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

/// What a file replaced by an update is renamed to: a running program, or a
/// plugin it has loaded, can be renamed but not overwritten. The next start
/// deletes it ([`remove_replaced`]).
const REPLACED: &str = ".replaced-by-update";

/// The folder an update in place is unpacked into first.
const IN_PLACE_STAGE: &str = ".update";

/// The plan of an update in place, in the program's folder while it runs
/// ([`replace_files`]).
const PROGRAM_PLAN: &str = ".update-plan";

/// Held while an update changes files, by one running copy of the program
/// at a time ([`lock`]).
const LOCK: &str = ".update-lock";

/// The program, put in place first: which one is there says which version
/// a half-done update in place is finished to ([`recover`]).
const PROGRAM: &str = "rustysalamander.exe";

/// What a [`Mover`] does with a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    /// Rename it (over a file there: replaced in one step).
    Move,
    /// Give it a second name (a hard link): the first stays.
    Link,
}

/// Moves or links a file (creating the folder it goes to): `fs::rename` and
/// `fs::hard_link`, or in tests one that fails on cue.
type Mover<'a> = &'a dyn Fn(Op, &Path, &Path) -> io::Result<()>;

fn move_file(op: Op, from: &Path, to: &Path) -> io::Result<()> {
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir)?;
    }
    match op {
        Op::Move => fs::rename(from, to),
        Op::Link => fs::hard_link(from, to),
    }
}

/// Keep the file `file` (relative to the folder) at `from` also as `to`: a
/// hard link, so `from` stays where it is (the program can still be started
/// while it is replaced: Windows replaces a running program's name once it
/// has a second one). Other files are renamed where links can't be made
/// (FAT, some network drives); the program never is
/// ([`install_in_place`] checks first that links can be made).
fn keep_as(file: &Path, from: &Path, to: &Path, mv: Mover) -> io::Result<()> {
    let linked = mv(Op::Link, from, to);
    if linked.is_err() && !is_program(file) {
        return mv(Op::Move, from, to);
    }
    linked
}

/// Whether `file` (relative to the program's folder) is the program.
fn is_program(file: &Path) -> bool {
    file.as_os_str().eq_ignore_ascii_case(PROGRAM)
}

/// Whether hard links can be made in `dir` (they can't on FAT, nor on
/// some network drives).
fn links_work(dir: &Path) -> bool {
    let (a, b) = (dir.join(".update-link-test"), dir.join(".update-link-test2"));
    let _ = (fs::remove_file(&a), fs::remove_file(&b));
    let ok = fs::write(&a, b"").is_ok() && fs::hard_link(&a, &b).is_ok();
    let _ = (fs::remove_file(&a), fs::remove_file(&b));
    ok
}

/// One running copy of the program changing files by an update.
pub struct UpdateLock(#[allow(dead_code)] fs::File);

/// Take the update lock of the program folder `app`: an error when another
/// running copy of the program holds it. `None` when the folder can't be
/// written to (nothing there can be updated then either).
pub fn lock(app: &Path) -> Result<Option<UpdateLock>, String> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        match fs::OpenOptions::new().write(true).create(true).truncate(false).share_mode(0).open(app.join(LOCK)) {
            Ok(f) => Ok(Some(UpdateLock(f))),
            // ERROR_SHARING_VIOLATION
            Err(e) if e.raw_os_error() == Some(32) => Err("another running RustySalamander is updating; try again when it is done".into()),
            Err(_) => Ok(None),
        }
    }
    #[cfg(not(windows))]
    {
        Ok(fs::OpenOptions::new().write(true).create(true).truncate(false).open(app.join(LOCK)).ok().map(UpdateLock))
    }
}

/// How long the start, or an update about to change files, waits for
/// another running copy of the program to let go of the update lock (it
/// holds it only while it changes files or loads its plugins).
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// Installs of this program take turns ([`install`]): several started at
/// once (one by one, or one of them the program) don't fail on the update
/// lock, they wait for the one before.
static INSTALL_TURN: Mutex<()> = Mutex::new(());

/// [`lock`], waiting up to `wait` while another running copy holds it.
pub fn lock_waiting(app: &Path, wait: std::time::Duration) -> Result<Option<UpdateLock>, String> {
    let until = std::time::Instant::now() + wait;
    loop {
        match lock(app) {
            Err(_) if std::time::Instant::now() < until => std::thread::sleep(std::time::Duration::from_millis(100)),
            r => return r,
        }
    }
}

/// What [`replace_files`] does, written down before it changes anything:
/// each file (relative to the target folder) and the name the one it
/// replaces is renamed aside to (none for a new file).
type Plan = Vec<(PathBuf, Option<PathBuf>)>;

/// Write `plan` to `path`, on the disk before anything else happens: into
/// another file first, so `path` is whole or not there.
fn write_plan(path: &Path, plan: &Plan) -> io::Result<()> {
    let mut text = String::new();
    for (file, aside) in plan {
        let name = |p: &Path| p.to_str().map(str::to_owned).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("not a Unicode name: {}", p.display())));
        text.push_str(&name(file)?);
        text.push('\t');
        if let Some(a) = aside {
            text.push_str(&name(a)?);
        }
        text.push('\n');
    }
    let partial = path.with_extension("partial");
    {
        use std::io::Write;
        let mut f = fs::File::create(&partial)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&partial, path)
}

/// The plan at `path`, if one is there.
/// `Ok(None)` only when there is none: a plan that is there but can't be
/// read, or isn't one, is an error (what it would recover stays).
fn read_plan(path: &Path) -> Result<Option<Plan>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let bad = |l: &str| format!("{}: not a plan of an update: {l:?}", path.display());
    let mut plan = Vec::new();
    for l in text.lines().filter(|l| !l.is_empty()) {
        // `<file>\t<aside>`, both inside the folder (no aside for a new file)
        let (file, aside) = l.split_once('\t').ok_or_else(|| bad(l))?;
        let file = safe_path(file).filter(|f| !f.as_os_str().is_empty()).ok_or_else(|| bad(l))?;
        let aside = if aside.is_empty() { None } else { Some(safe_path(aside).filter(|a| !a.as_os_str().is_empty()).ok_or_else(|| bad(l))?) };
        plan.push((file, aside));
    }
    // a plan always has a file (one emptied is no plan to trust)
    if plan.is_empty() {
        return Err(format!("{}: the plan of an update is empty", path.display()));
    }
    Ok(Some(plan))
}

/// Put one file of a plan in place: the old one kept aside ([`keep_as`]),
/// then the new one renamed over it from `stage`, so the file is there at
/// every moment. Done already when it isn't in `stage` any more.
fn step_forward(stage: &Path, target: &Path, (file, aside): &(PathBuf, Option<PathBuf>), mv: Mover) -> Result<(), String> {
    let (staged, to) = (stage.join(file), target.join(file));
    let err = |e: io::Error| format!("{}: {e}", to.display());
    if !staged.exists() {
        return Ok(());
    }
    if let Some(a) = aside {
        let a = target.join(a);
        if to.exists() && !a.exists() {
            keep_as(file, &to, &a, mv).map_err(err)?;
        }
    }
    mv(Op::Move, &staged, &to).map_err(err)
}

/// Undo what [`step_forward`] did to one file, as far as it got: the new
/// file back into `stage`, the old one back in its place (over the new
/// one, so the file is there at every moment).
fn step_back(stage: &Path, target: &Path, (file, aside): &(PathBuf, Option<PathBuf>), mv: Mover) -> Result<(), String> {
    let (staged, to) = (stage.join(file), target.join(file));
    let err = |e: io::Error| format!("{}: {e}", to.display());
    let Some(a) = aside.as_ref().map(|a| target.join(a)) else {
        // a new file: back into `stage` when it was put in place
        return if to.exists() && !staged.exists() { mv(Op::Move, &to, &staged).map_err(err) } else { Ok(()) };
    };
    if !a.exists() {
        // never put aside, or put back already: what is there is the old file
        return Ok(());
    }
    // which file is in place is told by what it is, never by which names
    // are there (an undo stopped half way leaves the new one in place and
    // in `stage` both)
    if to.exists() && sal_core::fileops::is_same_file(&to, &a) {
        // the old one is in place: `a` is only its second name
        if staged.exists() {
            let _ = fs::remove_file(&a);
        }
        return Ok(());
    }
    // the new one in place (or none): kept in `stage`, the old one renamed
    // over it, so the file is there at every moment
    if to.exists() && !staged.exists() {
        keep_as(file, &to, &staged, mv).map_err(err)?;
    }
    if let Err(e) = mv(Op::Move, &a, &to) {
        // never neither: the new one stays rather than none
        if !to.exists() && staged.exists() {
            let _ = mv(Op::Move, &staged, &to);
        }
        return Err(err(e));
    }
    Ok(())
}

/// Undo a whole plan, last file first, as far as it can: an error listing
/// the files it couldn't put back.
fn undo(stage: &Path, target: &Path, plan: &Plan, mv: Mover) -> Result<(), String> {
    let errors: Vec<String> = plan.iter().rev().filter_map(|e| step_back(stage, target, e, mv).err()).collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Move the files of `stage` into `target`, each file it replaces renamed
/// aside (deleted at the next start, [`remove_replaced`]): programs first,
/// the program itself before everything. The plan is written to
/// `plan_path` before anything changes and removed when done; when a file
/// can't be put in place, everything done is undone. If that fails too
/// (or the program is stopped half way), the plan stays for [`recover`].
/// Returns how many files were put in place.
fn replace_files(stage: &Path, target: &Path, plan_path: &Path, mv: Mover) -> Result<usize, String> {
    let mut files = Vec::new();
    list_files(stage, Path::new(""), &mut files);
    let rank = |f: &PathBuf| match f.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        _ if f.as_os_str().eq_ignore_ascii_case(PROGRAM) => 0,
        Some("spl" | "dll" | "exe") => 1,
        _ => 2,
    };
    files.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
    if files.is_empty() {
        // nothing to put in place: no plan (an empty one isn't one)
        return Ok(0);
    }
    let plan: Plan = files
        .iter()
        .map(|file| {
            let to = target.join(file);
            let aside = to.exists().then(|| free_aside(&to)).and_then(|a| a.strip_prefix(target).ok().map(Path::to_path_buf));
            (file.clone(), aside)
        })
        .collect();
    write_plan(plan_path, &plan).map_err(|e| format!("{}: {e}", plan_path.display()))?;
    match plan.iter().try_for_each(|e| step_forward(stage, target, e, mv)) {
        Ok(()) => {
            let _ = fs::remove_file(plan_path);
            Ok(files.len())
        }
        Err(e) => match undo(stage, target, &plan, mv) {
            Ok(()) => {
                let _ = fs::remove_file(plan_path);
                Err(e)
            }
            Err(u) => Err(format!("{e}. Not everything could be put back ({u}); that is done when RustySalamander starts next time.")),
        },
    }
}

/// Finish or undo what [`replace_files`] left half done (stopped, or its
/// undo failed), by the plan at `plan_path`; the plan is removed when that
/// worked. An update of the program is finished when the new program is in
/// place already (it is the one running now), else undone; any other is
/// undone (it is tried again after). Nothing to do without a plan.
fn recover(stage: &Path, target: &Path, plan_path: &Path, mv: Mover) -> Result<(), String> {
    let Some(plan) = read_plan(plan_path)? else { return Ok(()) };
    let program = plan.iter().any(|(f, _)| f.as_os_str().eq_ignore_ascii_case(PROGRAM));
    let forward = program && !stage.join(PROGRAM).exists();
    crate::debuglog::write(format!("update: {} the interrupted update of {}", if forward { "finishing" } else { "undoing" }, target.display()));
    let r = if forward {
        let errors: Vec<String> = plan.iter().filter_map(|e| step_forward(stage, target, e, mv).err()).collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    } else {
        undo(stage, target, &plan, mv)
    };
    if r.is_ok() {
        let _ = fs::remove_file(plan_path);
    }
    r
}

/// Update the program in its folder `app` from a release zip: unpacked next
/// to it first (a damaged download changes nothing), then each file of the
/// release put in place by [`replace_files`]. Everything else stays:
/// settings, what plugins saved, plugins and languages the release doesn't
/// bring. If a file can't be replaced, every file already replaced is put
/// back. Returns how many files were replaced or added.
pub fn install_in_place(zip: &[u8], app: &Path) -> Result<usize, String> {
    install_in_place_with(zip, app, &move_file)
}

fn install_in_place_with(zip: &[u8], app: &Path, mv: Mover) -> Result<usize, String> {
    let plan = app.join(PROGRAM_PLAN);
    if plan.exists() {
        return Err("an earlier update didn't finish: start RustySalamander again (that finishes it), then update".into());
    }
    // the program is replaced under its name, never without it: that needs
    // a second name for the old one while it runs
    if !links_work(app) {
        return Err(format!(
            "the drive of {} can't keep the running program aside while it is replaced (it has no hard links). Install the new version in a new folder.",
            app.display()
        ));
    }
    let stage = app.join(IN_PLACE_STAGE);
    let _ = fs::remove_dir_all(&stage);
    unpack_release(zip, &stage)?;
    let r = replace_files(&stage, app, &plan, mv);
    // a plan left needs what is staged
    if !plan.exists() {
        let _ = fs::remove_dir_all(&stage);
    }
    r
}

/// A free name to rename `file` aside to.
fn free_aside(file: &Path) -> PathBuf {
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut n = 0;
    loop {
        let aside = file.with_file_name(if n == 0 { format!("{name}{REPLACED}") } else { format!("{name}{REPLACED}{n}") });
        // one there is left alone: it may be what another update needs to
        // be undone (the start deletes it when nothing does)
        if fs::symlink_metadata(&aside).is_err() {
            return aside;
        }
        n += 1;
    }
}

/// What [`at_start`] did with the updates.
#[derive(Debug, Default, PartialEq)]
pub struct Started {
    /// The plugin folders updated.
    pub updated: Vec<String>,
    /// What couldn't be updated or put back, and why.
    pub failed: Vec<String>,
    /// Plugin folders left half updated: not to be loaded this time.
    pub skip: Vec<String>,
    /// No plugin is to be looked for nor loaded this time: another running
    /// copy of the program is changing files, so any may be half there
    /// (even one it is only now putting in place). `failed` says so.
    pub defer_plugins: bool,
}

/// At start, before the plugins are loaded: finish or undo an update in
/// place that an interruption left half done ([`recover`]), delete what
/// updates renamed aside ([`remove_replaced`]), and put the plugin updates
/// waiting in `plugins` in place ([`apply_staged`]). Under the update lock,
/// which it returns: hold it until the plugins are loaded, so no other
/// running copy of the program changes them meanwhile. While another one
/// holds it longer than [`LOCK_WAIT`], nothing is touched and no plugin is
/// loaded (which of them are being changed can't be told).
pub fn at_start(app: Option<&Path>, plugins: Option<&Path>) -> (Started, Option<UpdateLock>) {
    at_start_waiting(app, plugins, &move_file, LOCK_WAIT)
}

/// [`at_start`] without waiting for the lock, and letting go of it.
#[cfg(test)]
fn at_start_with(app: Option<&Path>, plugins: Option<&Path>, mv: Mover) -> Started {
    at_start_waiting(app, plugins, mv, std::time::Duration::ZERO).0
}

fn at_start_waiting(app: Option<&Path>, plugins: Option<&Path>, mv: Mover, wait: std::time::Duration) -> (Started, Option<UpdateLock>) {
    let lock = match app.map(|app| lock_waiting(app, wait)) {
        Some(Err(e)) => {
            // another running copy is changing files: any plugin may be one,
            // also one it puts in place after now
            let started = Started { failed: vec![format!("{e}; plugins are loaded at the next start")], defer_plugins: true, ..Default::default() };
            return (started, None);
        }
        Some(Ok(lock)) => lock,
        None => None,
    };
    (started_locked(app, plugins, mv), lock)
}

/// The plugins in `plugins`: folders and plugin files.
fn all_plugins(plugins: &Path) -> Vec<String> {
    fs::read_dir(plugins).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with('.')).collect()).unwrap_or_default()
}

/// [`at_start`] with the update lock held.
fn started_locked(app: Option<&Path>, plugins: Option<&Path>, mv: Mover) -> Started {
    let mut started = Started::default();
    // half-done updates first, the program's and the plugins': what they
    // put aside is needed to undo them
    let program_ok = match app.map(|app| recover(&app.join(IN_PLACE_STAGE), app, &app.join(PROGRAM_PLAN), mv)) {
        Some(Err(e)) => {
            started.failed.push(format!("the update of the program: {e}"));
            false
        }
        _ => true,
    };
    if let (Some(plugins), false) = (plugins, program_ok) {
        // the program's update may have put plugin files aside too: plugin
        // updates wait until it is finished or undone, so nothing it needs
        // is reused; plugins half updated aren't loaded meanwhile, the
        // program's or their own
        started.skip = half_updated(plugins);
        skip_program_plugins(&mut started.skip, app, Some(plugins));
        if fs::read_dir(plugins.join(STAGED)).is_ok_and(|mut d| d.next().is_some()) {
            started.failed.push("plugin updates wait until the program's update is finished or undone".into());
        }
        return started;
    }
    if let Some(plugins) = plugins {
        let (failed, skip) = recover_plugins(plugins, mv);
        started.failed.extend(failed);
        started.skip = skip;
    }
    // then what was put aside goes, but what a plan left unresolved needs
    // (all of it while the program's is)
    if let (Some(app), true) = (app, program_ok) {
        let keep: Vec<PathBuf> = plugins.map(|p| started.skip.iter().map(|s| p.join(s)).collect()).unwrap_or_default();
        remove_replaced(app, &keep);
    }
    if let Some(plugins) = plugins {
        let s = apply_staged_with(plugins, &started.skip, mv);
        started.updated = s.updated;
        started.failed.extend(s.failed);
        started.skip.extend(s.skip);
    }
    started
}

/// Add to `skip` the plugins an update in place of the program in `app`
/// changes ([`plugins_of_program_update`]), when `plugins` is the program's
/// own folder of them.
fn skip_program_plugins(skip: &mut Vec<String>, app: Option<&Path>, plugins: Option<&Path>) {
    let (Some(app), Some(plugins)) = (app, plugins) else { return };
    if plugins != app.join("plugins") {
        return;
    }
    for name in plugins_of_program_update(app, plugins) {
        if !skip.contains(&name) {
            skip.push(name);
        }
    }
}

/// The plugins (folders, or plugin files, in `plugins`, the program's own
/// folder of them) an update in place of the program in `app` changes:
/// those its plan names; none without a plan; every one when the plan
/// can't be read (which of them are whole can't be told then: the staging
/// folder no longer lists what went into place).
fn plugins_of_program_update(app: &Path, plugins: &Path) -> Vec<String> {
    match read_plan(&app.join(PROGRAM_PLAN)) {
        Ok(Some(plan)) => {
            let mut names: Vec<String> = plan
                .iter()
                .filter_map(|(file, _)| {
                    let mut parts = file.components();
                    (parts.next()?.as_os_str() == "plugins").then_some(())?;
                    Some(parts.next()?.as_os_str().to_string_lossy().into_owned())
                })
                .collect();
            names.sort();
            names.dedup();
            names
        }
        Ok(None) => Vec::new(),
        Err(_) => all_plugins(plugins),
    }
}

/// Delete what an update in place renamed aside, and a staging folder an
/// interrupted one left (call at start, before the plugins are loaded, and
/// only when no plan of the program's is left: [`at_start`]), except below
/// the folders `keep` (plugins whose update is half done). Files still in
/// use (another copy of the old version runs) stay for the next time.
pub fn remove_replaced(app: &Path, keep: &[PathBuf]) {
    let _ = fs::remove_dir_all(app.join(IN_PLACE_STAGE));
    let _ = fs::remove_dir_all(app.join(format!(".{IN_PLACE_STAGE}.partial")));
    let mut todo = vec![app.to_path_buf()];
    while let Some(dir) = todo.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                // the program's own folders only: lang, plugins and theirs
                if dir == app && !matches!(e.file_name().to_str(), Some("lang" | "plugins")) || keep.contains(&e.path()) {
                    continue;
                }
                todo.push(e.path());
            } else if e.file_name().to_string_lossy().contains(REPLACED) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

/// Delete the files below `dir` that an update renamed aside, as far as
/// they can be (one still in use stays for the next time).
fn remove_aside_below(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            remove_aside_below(&e.path());
        } else if e.file_name().to_string_lossy().contains(REPLACED) {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// Where the update of the plugin in `dir` waits: `plugins\.updates\<folder>`.
pub fn staged_dir(dir: &Path) -> Option<PathBuf> {
    Some(dir.parent()?.join(STAGED).join(dir.file_name()?))
}

/// The plan of putting the update of the plugin folder `name` in place.
fn plugin_plan(plugins: &Path, name: &str) -> PathBuf {
    plugins.join(STAGED).join(format!("{name}.plan"))
}

/// The plugin folders whose update is half done (a plan is there).
fn half_updated(plugins: &Path) -> Vec<String> {
    let Ok(rd) = fs::read_dir(plugins.join(STAGED)) else { return Vec::new() };
    rd.flatten().filter_map(|e| e.file_name().to_str()?.strip_suffix(".plan").map(str::to_owned)).collect()
}

/// Put the plugin updates waiting in `plugins\.updates` in place of their
/// files, before the plugins are loaded ([`replace_files`]: the old files
/// renamed aside, so a plugin still in use by another running copy of the
/// program is updated too). One left half done is undone first and tried
/// again. Returns what was updated, and what failed.
#[cfg(test)]
pub fn apply_staged(plugins: &Path) -> (Vec<String>, Vec<String>) {
    let (mut failed, skip) = recover_plugins(plugins, &move_file);
    let s = apply_staged_with(plugins, &skip, &move_file);
    failed.extend(s.failed);
    (s.updated, failed)
}

/// Undo the plugin updates left half done ([`recover`]), so they can be
/// put in place again: what couldn't be undone, and those plugins' folders
/// (not to be loaded, nor what they put aside deleted).
fn recover_plugins(plugins: &Path, mv: Mover) -> (Vec<String>, Vec<String>) {
    let (mut failed, mut skip) = (Vec::new(), Vec::new());
    for name in half_updated(plugins) {
        if let Err(e) = recover(&plugins.join(STAGED).join(&name), &plugins.join(&name), &plugin_plan(plugins, &name), mv) {
            failed.push(format!("{name}: {e}"));
            skip.push(name);
        }
    }
    (failed, skip)
}

/// Put the plugin updates waiting in place, but those of the folders
/// `skip` ([`recover_plugins`] first).
fn apply_staged_with(plugins: &Path, skip: &[String], mv: Mover) -> Started {
    let mut s = Started::default();
    let staged = plugins.join(STAGED);
    let Ok(dirs) = fs::read_dir(&staged) else { return s };
    // not a folder being unpacked (`.<name>.partial`)
    for d in dirs.flatten().filter(|d| d.path().is_dir() && !d.file_name().to_string_lossy().starts_with('.')) {
        let name = d.file_name().to_string_lossy().into_owned();
        if skip.contains(&name) {
            continue;
        }
        let plan = plugin_plan(plugins, &name);
        match replace_files(&d.path(), &plugins.join(&name), &plan, mv) {
            Ok(_) => {
                let _ = fs::remove_dir_all(d.path());
                // the old files, unless another running copy still uses them
                remove_aside_below(&plugins.join(&name));
                s.updated.push(name);
            }
            Err(e) => {
                s.failed.push(format!("{name}: {e}"));
                if plan.exists() {
                    s.skip.push(name);
                }
            }
        }
    }
    let _ = fs::remove_dir(&staged);
    s
}

/// The files below `root\rel`, as paths relative to `root`.
fn list_files(root: &Path, rel: &Path, out: &mut Vec<PathBuf>) {
    let Ok(dir) = fs::read_dir(root.join(rel)) else { return };
    for e in dir.flatten() {
        let rel = rel.join(e.file_name());
        if e.path().is_dir() {
            list_files(root, &rel, out);
        } else {
            out.push(rel);
        }
    }
}

/// Unpack a release zip into `target` (which must not exist yet): into a
/// hidden folder next to it first, renamed when complete, so a failure
/// leaves no half-installed version behind. The zip's single top folder
/// (`RustySalamander-<version>-win64`) is left out.
pub fn unpack_release(zip: &[u8], target: &Path) -> Result<(), String> {
    unpack(zip, target, &|stage| stage.join("rustysalamander.exe").is_file(), "the zip has no rustysalamander.exe")
}

/// Unpack a plugin's zip into `target` (a staging folder that must not
/// exist yet); it must hold the plugin (`*.spl`).
pub fn unpack_plugin(zip: &[u8], target: &Path) -> Result<(), String> {
    let has_plugin = |stage: &Path| {
        fs::read_dir(stage).is_ok_and(|d| d.flatten().any(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case(abi::EXTENSION))))
    };
    unpack(zip, target, &has_plugin, "the zip has no plugin")
}

/// Unpack `zip` into `target` through a staging folder, renamed only when
/// `complete` says the result is whole; else nothing is left behind.
fn unpack(zip: &[u8], target: &Path, complete: &dyn Fn(&Path) -> bool, incomplete: &str) -> Result<(), String> {
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
        if !complete(&stage) {
            return Err(incomplete.into());
        }
        fs::rename(&stage, target).map_err(|err| format!("{}: {err}", target.display()))
    })();
    if r.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    r
}

/// Copy the settings from the program folder `old` into `new`: `config.toml`
/// and `multiuser.toml`, and, in the folder of each plugin the new version
/// brings, the files it doesn't (what the plugin saved there, such as
/// `known_hosts`), never programs. With `own`, also the user's own: plugins
/// (folders, or plugin files) and languages the new version doesn't bring,
/// whole. Returns what was copied, relative to the folder (a plugin's
/// folder ends with `\`).
pub fn migrate(old: &Path, new: &Path, own: bool) -> io::Result<Vec<String>> {
    let mut copied = Vec::new();
    // the settings, and the switch to multi-user mode (whose settings are in
    // %APPDATA% and stay there)
    for name in ["config.toml", sal_core::config::MULTI_USER_FILE] {
        if old.join(name).is_file() {
            fs::copy(old.join(name), new.join(name))?;
            copied.push(name.to_owned());
        }
    }
    /// Copy the settings files below `rel`.
    fn walk(old: &Path, new: &Path, rel: &Path, copied: &mut Vec<String>) -> io::Result<()> {
        let Ok(dir) = fs::read_dir(old.join(rel)) else { return Ok(()) };
        for de in dir.flatten() {
            let rel = rel.join(de.file_name());
            if de.file_type()?.is_dir() {
                // translations come with the program; updates waiting for
                // the next start (plugins\.updates) belong to this version;
                // a plugin the new version doesn't bring is copied whole, or not
                if de.file_name() != "lang" && !de.file_name().to_string_lossy().starts_with('.') && new.join(&rel).is_dir() {
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
    if own {
        copy_own(old, new, &mut copied)?;
    }
    Ok(copied)
}

/// Copy what the user added to the program folder `old` and the new
/// version `new` doesn't bring: plugins (a folder below `plugins`, or a
/// plugin file there) and languages (`lang\*.toml`).
fn copy_own(old: &Path, new: &Path, copied: &mut Vec<String>) -> io::Result<()> {
    fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
        fs::create_dir_all(to)?;
        for e in fs::read_dir(from)?.flatten() {
            if e.file_type()?.is_dir() {
                copy_tree(&e.path(), &to.join(e.file_name()))?;
            } else {
                fs::copy(e.path(), to.join(e.file_name()))?;
            }
        }
        Ok(())
    }
    if let Ok(dir) = fs::read_dir(old.join("plugins")) {
        for e in dir.flatten() {
            let name = e.file_name();
            let rel = Path::new("plugins").join(&name);
            if name.to_string_lossy().starts_with('.') || new.join(&rel).exists() {
                continue;
            }
            if e.file_type()?.is_dir() {
                copy_tree(&e.path(), &new.join(&rel))?;
                copied.push(format!("{}\\", rel.to_string_lossy()));
            } else if e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case(abi::EXTENSION)) {
                fs::create_dir_all(new.join("plugins"))?;
                fs::copy(e.path(), new.join(&rel))?;
                copied.push(rel.to_string_lossy().into_owned());
            }
        }
    }
    if let Ok(dir) = fs::read_dir(old.join("lang")) {
        for e in dir.flatten() {
            let rel = Path::new("lang").join(e.file_name());
            if e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("toml")) && !new.join(&rel).exists() {
                fs::create_dir_all(new.join("lang"))?;
                fs::copy(e.path(), new.join(&rel))?;
                copied.push(rel.to_string_lossy().into_owned());
            }
        }
    }
    Ok(())
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

/// A little-endian number in a zip's bytes.
fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

/// A little-endian number in a zip's bytes.
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

/// Elsewhere there are no downloads.
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

    fn program(json: &str, current: &str) -> Result<Option<Release>, String> {
        newest(json, "v", current, &program_zip)
    }

    #[test]
    fn newest_release_is_picked() {
        // drafts and releases without a zip are skipped
        let r = program(LIST, "0.2.1-alpha").unwrap().unwrap();
        assert_eq!((r.version.as_str(), r.tag.as_str()), ("0.2.2-alpha", "v0.2.2-alpha"));
        assert_eq!(r.page, "https://github.com/r/releases/tag/v0.2.2-alpha");
        assert_eq!((r.asset_url.as_str(), r.asset_size), ("https://github.com/r/z.zip", 7126108));
        // nothing newer
        assert_eq!(program(LIST, "0.2.2-alpha").unwrap(), None);
        assert_eq!(program(LIST, "0.2.2").unwrap(), None, "a final release is newer than its alphas");
        assert!(program(r#"{"message": "API rate limit exceeded"}"#, "0.1.0").unwrap_err().contains("rate limit"));
    }

    /// Plugin releases live beside the program's, told apart by their tags;
    /// a zip for an interface this program lacks is not offered.
    #[test]
    fn plugin_releases() {
        let json = format!(
            r#"[
            {{"tag_name": "viewer-v0.7.0", "draft": false, "html_url": "p7", "assets": [{{"name": "viewer-0.7.0-api{newer}.zip", "browser_download_url": "x7", "size": 3}}]}},
            {{"tag_name": "viewer-v0.6.0", "draft": false, "html_url": "p6", "assets": [{{"name": "viewer-0.6.0-api{now}.zip", "browser_download_url": "x6", "size": 2}}]}},
            {{"tag_name": "search-v9.0.0", "draft": false, "html_url": "s", "assets": [{{"name": "search-9.0.0-api{now}.zip", "browser_download_url": "s", "size": 1}}]}},
            {{"tag_name": "v9.0.0", "draft": false, "html_url": "v", "assets": [{{"name": "RustySalamander-9.0.0-win64.zip", "browser_download_url": "v", "size": 1}}]}}
        ]"#,
            now = abi::VERSION,
            newer = abi::VERSION + 1
        );
        let source = |tag: &str| sal_plugin_api::host::Source { repo: "me/rs".into(), tag: tag.into(), changelog: "plugins/x/CHANGELOG.md".into() };
        let plugins = [
            PluginSource { id: "viewer".into(), name: "Prohlížeč".into(), dir: PathBuf::from(r"C:\rs\plugins\viewer"), version: "0.5.0".into(), source: source("viewer-v") },
            PluginSource { id: "search".into(), name: "Hledání".into(), dir: PathBuf::from(r"C:\rs\plugins\search"), version: "9.0.0".into(), source: source("search-v") },
        ];
        let mut asked = Vec::new();
        let found = check(&plugins, None, &mut |repo| {
            asked.push(repo.to_owned());
            if repo == REPO {
                Err("offline".into())
            } else {
                Ok(json.clone())
            }
        }, &mut || unreachable!("no offer, no list"))
        .unwrap();
        assert_eq!(asked, [REPO, "me/rs"], "each repository once");
        assert_eq!(found.len(), 1, "the program's list failed, the search plugin is up to date");
        let u = &found[0];
        assert_eq!((u.name(), u.current.as_str(), u.release.version.as_str(), u.release.tag.as_str()), ("Prohlížeč", "0.5.0", "0.6.0", "viewer-v0.6.0"));
        assert_eq!(u.release.asset, format!("viewer-0.6.0-api{}.zip", abi::VERSION), "not the zip for a newer interface");
        // nothing could be read: an error
        assert_eq!(check(&plugins, None, &mut |_| Err("offline".into()), &mut || Err("offline".into())).unwrap_err(), "offline");
        assert!(!plugin_zip("viewer", "viewer-0.6.0.zip") && !plugin_zip("viewer", "viewerx-0.6.0-api5.zip"));
    }

    /// Official plugins not installed yet are offered, named in the user's
    /// language; installed, waiting, turned down or unreleased ones aren't.
    #[test]
    fn new_official_plugins() {
        let json = format!(
            r#"[
            {{"tag_name": "ftp-v0.1.0-alpha", "draft": false, "html_url": "f", "body": "<!-- rustysalamander-program: 0.9.2-beta .. -->", "assets": [{{"name": "ftp-0.1.0-alpha-api{now}.zip", "browser_download_url": "fz", "size": 5}}]}},
            {{"tag_name": "scp-v0.6.0-alpha", "draft": false, "html_url": "s", "assets": [{{"name": "scp-0.6.0-alpha-api{now}.zip", "browser_download_url": "sz", "size": 4}}]}},
            {{"tag_name": "gone-v1.0.0", "draft": false, "html_url": "g", "assets": [{{"name": "gone-1.0.0-api{now}.zip", "browser_download_url": "gz", "size": 1}}]}},
            {{"tag_name": "staged-v1.0.0", "draft": false, "html_url": "w", "assets": [{{"name": "staged-1.0.0-api{now}.zip", "browser_download_url": "wz", "size": 1}}]}}
        ]"#,
            now = abi::VERSION
        );
        let official = r#"
            # comment
            [[plugin]]
            id = "ftp"
            tag = "ftp-v"
            changelog = "plugins/ftp/CHANGELOG.md"
            name = { en = "FTP / SFTP", cs = "FTP / SFTP (cz)" }
            description = { en = "Servers.", cs = "Servery." }

            [[plugin]]
            id = "scp"
            tag = "scp-v"
            name = { en = "SCP" }

            [[plugin]]
            id = "unreleased"
            tag = "unreleased-v"
            name = { en = "Not yet" }

            [[plugin]]
            id = "gone"
            tag = "gone-v"
            name = { en = "Turned down" }

            [[plugin]]
            id = "staged"
            tag = "staged-v"
            name = { en = "Waiting" }

            [[plugin]]
            id = "../evil"
            tag = "evil-v"
            name = { en = "Evil" }
        "#;
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("plugins");
        fs::create_dir_all(dir.join(STAGED).join("staged")).unwrap();
        let scp = PluginSource {
            id: "scp".into(),
            name: "SCP".into(),
            dir: dir.join("scp"),
            version: "0.6.0-alpha".into(),
            source: sal_plugin_api::host::Source { repo: REPO.into(), tag: "scp-v".into(), changelog: String::new() },
        };
        let offer = Offer { plugins: dir.clone(), lang: "cs".into(), declined: vec!["gone".into()] };
        let mut lists = 0;
        let found = check(
            &[scp],
            Some(&offer),
            &mut |_| {
                lists += 1;
                Ok(json.clone())
            },
            &mut || Ok(official.to_owned()),
        )
        .unwrap();
        assert_eq!(lists, 1, "the repository is read once");
        assert_eq!(found.len(), 1, "{found:?}");
        let u = &found[0];
        assert!(u.new && u.current.is_empty());
        assert_eq!((u.name(), u.about.as_str(), u.release.version.as_str()), ("FTP / SFTP (cz)", "Servery.", "0.1.0-alpha"));
        assert_eq!(u.target, Target::Plugin { id: "ftp".into(), name: "FTP / SFTP (cz)".into(), dir: dir.join("ftp") });
        assert_eq!(u.release.program_min, "0.9.2-beta");
        // in a language the list lacks: English
        let en = Offer { lang: "de".into(), ..offer.clone() };
        let found = check(&[], Some(&en), &mut |_| Ok(json.clone()), &mut || Ok(official.to_owned())).unwrap();
        assert!(found.iter().any(|u| u.name() == "FTP / SFTP" && u.about == "Servers."));
        // the list unreadable: updates still come, without new plugins
        let found = check(&[], Some(&offer), &mut |_| Ok(json.clone()), &mut || Err("offline".into())).unwrap();
        assert!(found.is_empty());
        // a plugin folder already there counts as installed
        fs::create_dir_all(dir.join("ftp")).unwrap();
        let found = check(&[], Some(&offer), &mut |_| Ok(json.clone()), &mut || Ok(official.to_owned())).unwrap();
        assert!(!found.iter().any(|u| u.name().starts_with("FTP")));
        assert!(parse_official("[[plugin]]\nid = 1").is_err());
    }

    /// The official list in the repository reads, and names every plugin
    /// published from here, with its tag and changelog as the plugin says.
    #[test]
    fn the_official_list_matches_the_plugins() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let list = parse_official(&fs::read_to_string(root.join(OFFICIAL)).unwrap()).unwrap();
        let mut ids: Vec<&str> = list.iter().map(|p| p.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, ["7zip", "compare", "ftp", "scp", "search", "viewer"]);
        for p in &list {
            assert_eq!(p.tag, format!("{}-v", p.id));
            assert!(root.join(&p.changelog).is_file(), "{}", p.changelog);
            assert!(!in_language(&p.name, "en").is_empty() && !in_language(&p.description, "en").is_empty(), "{}", p.id);
            assert!(p.name.contains_key("cs") && p.description.contains_key("cs"), "{} in Czech", p.id);
            // the plugin's own source says the same
            let src = fs::read_to_string(root.join(p.changelog.replace("CHANGELOG.md", "src/lib.rs"))).unwrap();
            assert!(src.contains(&format!("tag: \"{}\", changelog: \"{}\"", p.tag, p.changelog)), "{}", p.id);
        }
    }

    /// A plugin release's notes say which program versions it works with.
    #[test]
    fn program_versions_from_the_notes() {
        assert_eq!(program_range("Text.\n<!-- rustysalamander-program: 0.6.0 .. 0.9.0 -->\nMore."), ("0.6.0".into(), "0.9.0".into()));
        assert_eq!(program_range("<!-- rustysalamander-program: 0.6.0-alpha .. -->"), ("0.6.0-alpha".into(), String::new()));
        assert_eq!(program_range("<!-- rustysalamander-program: .. 0.7.0 -->"), (String::new(), "0.7.0".into()));
        assert_eq!(program_range("rustysalamander-program: 0.6.0\r\n"), ("0.6.0".into(), String::new()));
        assert_eq!(program_range("Nothing said."), (String::new(), String::new()));
        // the release list carries them to the update
        let json = format!(
            r#"[{{"tag_name": "viewer-v0.9.0", "draft": false, "html_url": "p", "body": "New.\r\n<!-- rustysalamander-program: 99.0.0 .. -->",
                 "assets": [{{"name": "viewer-0.9.0-api{now}.zip", "browser_download_url": "x", "size": 2}}]}}]"#,
            now = abi::VERSION
        );
        let r = newest(&json, "viewer-v", "0.5.0", &|n| plugin_zip("viewer", n)).unwrap().unwrap();
        assert_eq!((r.program_min.as_str(), r.program_max.as_str()), ("99.0.0", ""));
    }

    #[test]
    fn changes_since_the_installed_version() {
        let log = "# Changelog\n\n## 0.6.0 (2026-11-02)\n\n- **Search** in the viewer.\n\n## 0.5.1\n\n- Fix.\n\n## 0.5.0-alpha (2026-10-31)\n\n- First.\n";
        assert_eq!(changes_between(log, "0.5.0-alpha", "0.6.0"), "## 0.6.0 (2026-11-02)\n\n- **Search** in the viewer.\n\n## 0.5.1\n\n- Fix.");
        assert_eq!(changes_between(log, "0.5.1", "0.5.1"), "");
        assert_eq!(changes_between(log, "0.4.0", "0.5.0-alpha"), "## 0.5.0-alpha (2026-10-31)\n\n- First.");
    }

    /// A plugin's update waits in plugins\.updates and replaces its files at
    /// the next start; what the plugin saved stays.
    #[cfg(windows)]
    #[test]
    fn plugin_updates_wait_for_the_next_start() {
        let t = tempfile::tempdir().unwrap();
        let plugins = t.path().join("plugins");
        let dir = plugins.join("scp");
        fs::create_dir_all(dir.join("lang")).unwrap();
        fs::write(dir.join("scp.spl"), "old").unwrap();
        fs::write(dir.join("lang").join("cs.toml"), "old").unwrap();
        fs::write(dir.join("known_hosts"), "keys").unwrap();
        // the plugin's zip, as released: its folder
        let src = t.path().join("zip").join("scp");
        fs::create_dir_all(src.join("lang")).unwrap();
        fs::write(src.join("scp.spl"), "new").unwrap();
        fs::write(src.join("lang").join("cs.toml"), "nové").unwrap();
        let zip = t.path().join("scp-0.6.0-api5.zip");
        let ok = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &format!("Compress-Archive -Path '{}' -DestinationPath '{}'", src.display(), zip.display())])
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let staged = staged_dir(&dir).unwrap();
        assert_eq!(staged, plugins.join(".updates").join("scp"));
        fs::create_dir_all(staged.parent().unwrap()).unwrap();
        unpack_plugin(&fs::read(&zip).unwrap(), &staged).unwrap();
        assert_eq!(fs::read_to_string(dir.join("scp.spl")).unwrap(), "old", "nothing changes before the next start");
        assert!(unpack_plugin(&fs::read(&zip).unwrap()[..100], &plugins.join(".updates").join("x")).is_err());

        let (done, failed) = apply_staged(&plugins);
        assert_eq!((done, failed), (vec!["scp".to_owned()], Vec::<String>::new()));
        assert_eq!(fs::read_to_string(dir.join("scp.spl")).unwrap(), "new");
        assert_eq!(fs::read_to_string(dir.join("lang").join("cs.toml")).unwrap(), "nové");
        assert_eq!(fs::read_to_string(dir.join("known_hosts")).unwrap(), "keys", "the plugin's own files stay");
        assert!(!plugins.join(".updates").exists());
        assert_eq!(apply_staged(&plugins), (vec![], vec![]));
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
        fs::create_dir_all(old.join("plugins").join(".updates").join("scp")).unwrap();
        fs::write(old.join("plugins").join(".updates").join("scp").join("CHANGELOG.md"), "staged").unwrap();
        let mut copied = migrate(&old, &target, false).unwrap();
        copied.sort();
        assert_eq!(copied, ["config.toml", r"plugins\scp\known_hosts", r"plugins\scp\last-connection"]);
        assert_eq!(fs::read_to_string(target.join("config.toml")).unwrap(), "theme = \"Dark\"");
        assert_eq!(fs::read_to_string(target.join("plugins/scp/scp.spl")).unwrap(), "new plugin", "programs are never copied back");
        assert_eq!(fs::read_to_string(target.join("plugins/scp/lang/cs.toml")).unwrap(), "x = 1", "the new version's files win");
        assert_eq!(fs::read(target.join("rustysalamander.exe")).unwrap(), vec![7u8; 100_000]);
    }

    /// A program folder of an older version: settings, what a plugin saved,
    /// the user's own plugin (a folder and a loose file) and language.
    #[cfg(windows)]
    fn old_version(app: &Path) {
        fs::create_dir_all(app.join("plugins").join("scp").join("lang")).unwrap();
        fs::create_dir_all(app.join("plugins").join("ltocatalog").join("lang")).unwrap();
        fs::create_dir_all(app.join("lang")).unwrap();
        fs::write(app.join("rustysalamander.exe"), "old program").unwrap();
        fs::write(app.join("README.md"), "old readme").unwrap();
        fs::write(app.join("config.toml"), "theme = \"Dark\"").unwrap();
        fs::write(app.join("lang").join("de.toml"), "[language]\ncode = \"de\"").unwrap();
        fs::write(app.join("plugins").join("scp").join("scp.spl"), "old plugin").unwrap();
        fs::write(app.join("plugins").join("scp").join("known_hosts"), "host key").unwrap();
        fs::write(app.join("plugins").join("ltocatalog").join("ltocatalog.spl"), "own plugin").unwrap();
        fs::write(app.join("plugins").join("ltocatalog").join("catalog.sqlite"), "tapes").unwrap();
        fs::write(app.join("plugins").join("ltocatalog").join("lang").join("cs.toml"), "own").unwrap();
        fs::write(app.join("plugins").join("loose.spl"), "loose plugin").unwrap();
    }

    /// Update in place: the release's files replace the old ones (the old
    /// ones renamed aside, gone at the next start); settings, what plugins
    /// saved, own plugins and languages stay untouched.
    #[cfg(windows)]
    #[test]
    fn update_in_place_keeps_everything_else() {
        let t = tempfile::tempdir().unwrap();
        let zip = release_zip(t.path(), "0.9.9");
        let app = t.path().join("Rusty Salamander 🦎");
        old_version(&app);
        // the running program can be renamed, not overwritten: so it is here too
        let running = fs::File::open(app.join("rustysalamander.exe")).unwrap();
        assert_eq!(install_in_place(&zip, &app).unwrap(), 4);
        drop(running);
        assert_eq!(fs::read(app.join("rustysalamander.exe")).unwrap(), vec![7u8; 100_000]);
        assert_eq!(fs::read_to_string(app.join("plugins/scp/scp.spl")).unwrap(), "new plugin");
        assert_eq!(fs::read_to_string(app.join("README.md")).unwrap(), "čtení ".repeat(500));
        for (file, kept) in [
            ("config.toml", "theme = \"Dark\""),
            ("lang/de.toml", "[language]\ncode = \"de\""),
            ("plugins/scp/known_hosts", "host key"),
            ("plugins/ltocatalog/ltocatalog.spl", "own plugin"),
            ("plugins/ltocatalog/catalog.sqlite", "tapes"),
            ("plugins/loose.spl", "loose plugin"),
        ] {
            assert_eq!(fs::read_to_string(app.join(file)).unwrap(), kept, "{file}");
        }
        assert!(app.join(format!("rustysalamander.exe{REPLACED}")).is_file(), "the old program aside");
        assert!(!app.join(IN_PLACE_STAGE).exists());
        // the next start clears what was put aside, nothing else
        remove_replaced(&app, &[]);
        let mut files = Vec::new();
        list_files(&app, Path::new(""), &mut files);
        assert!(files.iter().all(|f| !f.to_string_lossy().contains(REPLACED)), "{files:?}");
        assert_eq!(files.len(), 11, "{files:?}");

        // a second update in place over the first
        assert_eq!(install_in_place(&release_zip(&t.path().join("again"), "1.0.0"), &app).unwrap(), 4);

        // a damaged download changes nothing
        let mut broken = zip.clone();
        let mid = broken.len() / 3;
        broken[mid] ^= 0xFF;
        fs::write(app.join("rustysalamander.exe"), "current").unwrap();
        assert!(install_in_place(&broken, &app).is_err());
        assert_eq!(fs::read_to_string(app.join("rustysalamander.exe")).unwrap(), "current");
        assert!(!app.join(IN_PLACE_STAGE).exists());
    }

    /// A file that can't be replaced: every file already replaced is put back.
    #[cfg(windows)]
    #[test]
    fn update_in_place_rolls_back() {
        let t = tempfile::tempdir().unwrap();
        let zip = release_zip(t.path(), "0.9.9");
        let app = t.path().join("app");
        old_version(&app);
        // a file another program holds without sharing: it can't be renamed aside
        use std::os::windows::fs::OpenOptionsExt;
        let held = fs::OpenOptions::new().read(true).share_mode(0).open(app.join("README.md")).unwrap();
        assert!(install_in_place(&zip, &app).is_err());
        drop(held);
        assert_eq!(fs::read_to_string(app.join("README.md")).unwrap(), "old readme");
        assert_eq!(fs::read_to_string(app.join("rustysalamander.exe")).unwrap(), "old program", "put back");
        assert_eq!(fs::read_to_string(app.join("plugins/scp/scp.spl")).unwrap(), "old plugin");
        let mut files = Vec::new();
        list_files(&app, Path::new(""), &mut files);
        assert!(files.iter().all(|f| !f.to_string_lossy().contains(REPLACED)), "{files:?}");
        assert!(!app.join(PROGRAM_PLAN).exists() && !app.join(IN_PLACE_STAGE).exists(), "undone whole: no plan left");
    }

    /// Moves (or links) files `after` times, then nothing moves any more:
    /// the power went off (nor can what was done be undone).
    fn stops_after(after: usize) -> impl Fn(Op, &Path, &Path) -> io::Result<()> {
        let n = std::cell::Cell::new(0);
        move |op, from, to| {
            if n.get() >= after {
                return Err(io::Error::other("stopped"));
            }
            n.set(n.get() + 1);
            move_file(op, from, to)
        }
    }

    /// The program folder `app` is the old version (`old_version`) or the
    /// new one (`release_zip`), wholly: which one.
    #[cfg(windows)]
    fn whole_version(app: &Path) -> &'static str {
        let read = |f: &str| fs::read(app.join(f)).ok();
        let old = [
            read("rustysalamander.exe") == Some(b"old program".to_vec()),
            read("README.md") == Some(b"old readme".to_vec()),
            read("plugins/scp/scp.spl") == Some(b"old plugin".to_vec()),
            !app.join("plugins/scp/lang/cs.toml").exists(),
        ];
        let new = [
            read("rustysalamander.exe") == Some(vec![7u8; 100_000]),
            read("README.md") == Some("čtení ".repeat(500).into_bytes()),
            read("plugins/scp/scp.spl") == Some(b"new plugin".to_vec()),
            read("plugins/scp/lang/cs.toml") == Some(b"x = 1".to_vec()),
        ];
        match (old.iter().all(|x| *x), new.iter().all(|x| *x)) {
            (true, _) => "old",
            (_, true) => "new",
            _ => panic!("a mixture: old {old:?}, new {new:?}"),
        }
    }

    /// An update in place stopped after each of its steps (and unable to
    /// undo any): the next start finishes it when the new program is in
    /// place already, else undoes it. Never a mixture, nothing lost.
    #[cfg(windows)]
    #[test]
    fn update_in_place_survives_interruption() {
        let t = tempfile::tempdir().unwrap();
        let zip = release_zip(t.path(), "0.9.9");
        let mut outcomes = Vec::new();
        for n in 0.. {
            let app = t.path().join(format!("app{n}"));
            old_version(&app);
            let r = install_in_place_with(&zip, &app, &stops_after(n));
            if r.is_ok() {
                assert_eq!(whole_version(&app), "new");
                break;
            }
            // the program is there to be started, which finishes or undoes it
            assert!(app.join(PROGRAM).is_file(), "no program after {n} steps");
            let half_done = app.join(PROGRAM_PLAN).exists();
            if half_done {
                assert!(install_in_place(&zip, &app).unwrap_err().contains("didn't finish"), "no update over a half-done one");
            }
            let s = at_start_with(Some(&app), None, &move_file);
            assert!(s.failed.is_empty(), "{s:?}");
            outcomes.push(whole_version(&app));
            assert!(!app.join(PROGRAM_PLAN).exists() && !app.join(IN_PLACE_STAGE).exists());
            let mut files = Vec::new();
            list_files(&app, Path::new(""), &mut files);
            assert!(files.iter().all(|f| !f.to_string_lossy().contains(REPLACED)), "{files:?}");
            for (file, kept) in [("config.toml", "theme = \"Dark\""), ("plugins/scp/known_hosts", "host key"), ("plugins/ltocatalog/catalog.sqlite", "tapes")] {
                assert_eq!(fs::read_to_string(app.join(file)).unwrap(), kept, "{file} after {n} steps");
            }
            // and then it updates
            install_in_place(&zip, &app).unwrap();
            assert_eq!(whole_version(&app), "new");
        }
        // stopped before the program was in place: undone; after: finished
        assert_eq!(outcomes.first(), Some(&"old"));
        assert_eq!(outcomes.last(), Some(&"new"));
        assert!(outcomes.len() >= 6, "{outcomes:?}");
    }

    /// An undo stopped after keeping the new file in `stage` but before
    /// putting the old one back: the next try sees the new one at both
    /// names, puts the old one back, and deletes nothing.
    #[test]
    fn an_undo_stopped_half_way_loses_nothing() {
        let t = tempfile::tempdir().unwrap();
        let (stage, target) = (t.path().join("stage"), t.path().join("app"));
        fs::create_dir_all(&stage).unwrap();
        fs::create_dir_all(&target).unwrap();
        // put in place: the new file, the old one aside
        fs::write(target.join("README.md"), "new").unwrap();
        fs::write(target.join("README.md.aside"), "old").unwrap();
        let plan_path = t.path().join("plan");
        write_plan(&plan_path, &vec![(PathBuf::from("README.md"), Some(PathBuf::from("README.md.aside")))]).unwrap();
        // links work, renames don't: the new file kept, the old not back
        let links_only = |op: Op, from: &Path, to: &Path| if op == Op::Link { move_file(op, from, to) } else { Err(io::Error::other("stopped")) };
        assert!(recover(&stage, &target, &plan_path, &links_only).is_err());
        assert!(plan_path.exists(), "the plan stays");
        assert_eq!(fs::read_to_string(stage.join("README.md")).unwrap(), "new");
        assert_eq!(fs::read_to_string(target.join("README.md.aside")).unwrap(), "old", "the old one kept");
        // again, and again stopped: still nothing lost
        assert!(recover(&stage, &target, &plan_path, &stops_after(0)).is_err());
        assert_eq!(fs::read_to_string(target.join("README.md.aside")).unwrap(), "old", "the old one kept");
        // and then it works
        recover(&stage, &target, &plan_path, &move_file).unwrap();
        assert_eq!(fs::read_to_string(target.join("README.md")).unwrap(), "old", "put back");
        assert_eq!(fs::read_to_string(stage.join("README.md")).unwrap(), "new");
        assert!(!target.join("README.md.aside").exists() && !plan_path.exists());
    }

    /// A plan that is there but can't be read, or isn't one, is an error:
    /// what it would recover (staged and put aside) is kept, the plugin
    /// isn't loaded.
    #[test]
    fn a_plan_that_cant_be_read_keeps_everything() {
        for plan in [&b"README.md\tREADME.md.aside\n\xff\xfe broken"[..], b"README.md README.md.aside\n", b"..\\evil.exe\t\n", b"README.md\tC:\\x\n", b"", b"\n"] {
            let t = tempfile::tempdir().unwrap();
            let app = t.path().join("app");
            let plugins = app.join("plugins");
            fs::create_dir_all(app.join(IN_PLACE_STAGE)).unwrap();
            fs::write(app.join(IN_PLACE_STAGE).join("README.md"), "new").unwrap();
            fs::write(app.join("README.md"), "old").unwrap();
            fs::write(app.join(format!("README.md{REPLACED}")), "old").unwrap();
            fs::write(app.join(PROGRAM_PLAN), plan).unwrap();
            staged_plugin(&plugins);
            fs::write(plugins.join("scp").join(format!("scp.spl{REPLACED}")), "old").unwrap();
            fs::write(plugin_plan(&plugins, "scp"), plan).unwrap();
            let s = at_start_with(Some(&app), Some(&plugins), &move_file);
            assert_eq!(s.failed.len(), 2, "{plan:?}: {s:?}");
            assert_eq!(s.skip, ["scp"]);
            for kept in [
                app.join(IN_PLACE_STAGE).join("README.md"),
                app.join(format!("README.md{REPLACED}")),
                app.join(PROGRAM_PLAN),
                plugins.join("scp").join(format!("scp.spl{REPLACED}")),
                plugins.join(STAGED).join("scp").join("scp.spl"),
            ] {
                assert!(kept.exists(), "{plan:?}: {} deleted", kept.display());
            }
        }
    }

    /// While the program's update can't be finished or undone, a plugin
    /// update waiting to be put in place waits too: it would reuse the
    /// names (and so lose the files) the program's update put aside.
    #[test]
    fn plugin_updates_wait_for_the_programs() {
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("app");
        let plugins = app.join("plugins");
        staged_plugin(&plugins);
        // the program's update put the plugin's old file aside, then stopped;
        // its plan is there but can't be followed now
        let aside = plugins.join("scp").join(format!("scp.spl{REPLACED}"));
        fs::write(&aside, "the program's old plugin").unwrap();
        fs::create_dir_all(app.join(IN_PLACE_STAGE)).unwrap();
        fs::write(app.join(PROGRAM_PLAN), "plugins\\scp\\scp.spl\tplugins\\scp\\scp.spl.replaced-by-update\n").unwrap();
        // links work, renames don't: the program's undo fails
        let links_only = |op: Op, from: &Path, to: &Path| if op == Op::Link { move_file(op, from, to) } else { Err(io::Error::other("stopped")) };
        let s = at_start_with(Some(&app), Some(&plugins), &links_only);
        assert!(s.updated.is_empty() && s.failed.len() == 2, "{s:?}");
        assert_eq!(s.skip, ["scp"], "half updated by the program's update: not loaded");
        assert_eq!(fs::read_to_string(&aside).unwrap(), "the program's old plugin", "kept");
        assert_eq!(fs::read_to_string(plugins.join("scp").join("scp.spl")).unwrap(), "old", "not updated");
        assert!(plugins.join(STAGED).join("scp").join("scp.spl").exists(), "waits");
        // once the program's update is undone, the plugin's goes in
        let s = at_start_with(Some(&app), Some(&plugins), &move_file);
        assert_eq!(s.updated, ["scp"], "{s:?}");
        assert_eq!(fs::read_to_string(plugins.join("scp").join("scp.spl")).unwrap(), "new");
    }

    /// While the program's update can't be finished or undone, the plugins
    /// it changes aren't loaded: those its plan names, every one when the
    /// plan can't be read.
    #[test]
    fn plugins_of_an_unfinished_program_update_arent_loaded() {
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("app");
        let plugins = app.join("plugins");
        for p in ["scp", "viewer", "ltocatalog"] {
            fs::create_dir_all(plugins.join(p)).unwrap();
        }
        fs::write(plugins.join("loose.spl"), "").unwrap();
        // the program's update put scp's old file aside: undoing it needs to move it
        fs::write(plugins.join("scp").join("scp.spl"), "new").unwrap();
        fs::write(plugins.join("scp").join("scp.spl.aside"), "old").unwrap();
        let skip = |plan: &[u8], staged: &[&str]| {
            let _ = fs::remove_dir_all(app.join(IN_PLACE_STAGE));
            for s in staged {
                fs::create_dir_all(app.join(IN_PLACE_STAGE).join("plugins").join(s)).unwrap();
            }
            fs::write(app.join(PROGRAM_PLAN), plan).unwrap();
            // stopped: nothing can be moved
            let mut s = at_start_with(Some(&app), Some(&plugins), &stops_after(0)).skip;
            s.sort();
            s
        };
        // the plan says which
        assert_eq!(skip(b"plugins\\scp\\scp.spl\tplugins\\scp\\scp.spl.aside\n", &[]), ["scp"]);
        // unreadable: none is known to be whole (what is staged doesn't say)
        assert_eq!(skip(b"\xff\xfe", &["scp", "viewer"]), ["loose.spl", "ltocatalog", "scp", "viewer"]);
        assert_eq!(skip(b"\xff\xfe", &[]), ["loose.spl", "ltocatalog", "scp", "viewer"]);
    }

    /// Another running copy holds the update lock (it changes files, or
    /// loads its plugins): the start waits for it, and holds it itself until
    /// its plugins are loaded; while it can't be had, no plugin is loaded.
    #[cfg(windows)]
    #[test]
    fn a_second_copy_waits_for_the_first() {
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("app");
        let plugins = app.join("plugins");
        for p in ["scp", "viewer"] {
            fs::create_dir_all(plugins.join(p)).unwrap();
        }
        // held all along: none is loaded
        let held = lock(&app).unwrap().expect("the folder can be written to");
        let (s, got) = at_start_waiting(Some(&app), Some(&plugins), &move_file, std::time::Duration::from_millis(300));
        assert!(s.defer_plugins && s.failed[0].contains("another running RustySalamander"), "{s:?}");
        assert!(got.is_none());
        // let go of a moment later: waited for, then held while loading
        let first = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            drop(held);
        });
        let (s, got) = at_start_waiting(Some(&app), Some(&plugins), &move_file, std::time::Duration::from_secs(10));
        first.join().unwrap();
        assert!(s.skip.is_empty() && s.failed.is_empty() && !s.defer_plugins, "{s:?}");
        assert!(got.is_some() && lock(&app).is_err(), "held until the plugins are loaded");
        drop(got);
        assert!(lock(&app).unwrap().is_some());
    }

    /// The program is never renamed aside: where it can't be linked, the
    /// update fails before anything changed.
    #[cfg(windows)]
    #[test]
    fn the_program_is_never_without_its_name() {
        let t = tempfile::tempdir().unwrap();
        let zip = release_zip(t.path(), "0.9.9");
        let app = t.path().join("app");
        old_version(&app);
        // links fail, renames work: as on a FAT drive
        let no_links = |op: Op, from: &Path, to: &Path| if op == Op::Link { Err(io::Error::other("no links")) } else { move_file(op, from, to) };
        assert!(install_in_place_with(&zip, &app, &no_links).is_err());
        assert_eq!(whole_version(&app), "old");
        assert!(!app.join(PROGRAM_PLAN).exists());
    }

    /// A plugin's folder before an update, and the update waiting for it.
    fn staged_plugin(plugins: &Path) {
        let dir = plugins.join("scp");
        fs::create_dir_all(dir.join("lang")).unwrap();
        fs::write(dir.join("scp.spl"), "old").unwrap();
        fs::write(dir.join("lang").join("cs.toml"), "old").unwrap();
        fs::write(dir.join("known_hosts"), "keys").unwrap();
        let staged = plugins.join(STAGED).join("scp");
        fs::create_dir_all(staged.join("lang")).unwrap();
        fs::write(staged.join("scp.spl"), "new").unwrap();
        fs::write(staged.join("lang").join("cs.toml"), "new").unwrap();
        fs::write(staged.join("CHANGELOG.md"), "new").unwrap();
    }

    /// A plugin update stopped after each step: the next start undoes it
    /// and puts it in place again. One that can't be undone isn't loaded,
    /// and what it put aside stays (the start deletes what updates put
    /// aside in the program's plugins folder, but not that).
    #[test]
    fn plugin_update_survives_interruption() {
        let t = tempfile::tempdir().unwrap();
        for n in 0.. {
            let app = t.path().join(format!("app{n}"));
            let plugins = app.join("plugins");
            staged_plugin(&plugins);
            let s = apply_staged_with(&plugins, &[], &stops_after(n));
            let dir = plugins.join("scp");
            if s.failed.is_empty() {
                assert_eq!(s.updated, ["scp"]);
                break;
            }
            let half_done = !s.skip.is_empty();
            assert_eq!(half_done, plugin_plan(&plugins, "scp").exists(), "{s:?}");
            if half_done {
                // undoing fails too: still not loaded, nothing deleted
                let s = at_start_with(Some(&app), Some(&plugins), &stops_after(0));
                assert_eq!(s.skip, ["scp"], "{s:?}");
                let (failed, skip) = recover_plugins(&plugins, &move_file);
                assert!(failed.is_empty() && skip.is_empty(), "{failed:?}");
            }
            // undone whole: the old plugin as it was
            for (file, text) in [("scp.spl", "old"), ("lang/cs.toml", "old"), ("known_hosts", "keys")] {
                assert_eq!(fs::read_to_string(dir.join(file)).unwrap(), text, "{file} after {n} steps");
            }
            assert!(!dir.join("CHANGELOG.md").exists());
            let s = at_start_with(Some(&app), Some(&plugins), &move_file);
            assert_eq!((s.updated.as_slice(), s.failed.len(), s.skip.len()), (&["scp".to_owned()][..], 0, 0), "after {n} steps: {s:?}");
            for (file, text) in [("scp.spl", "new"), ("lang/cs.toml", "new"), ("CHANGELOG.md", "new"), ("known_hosts", "keys")] {
                assert_eq!(fs::read_to_string(dir.join(file)).unwrap(), text, "{file} after {n} steps");
            }
            assert!(!plugins.join(STAGED).exists());
            let mut files = Vec::new();
            list_files(&dir, Path::new(""), &mut files);
            assert_eq!(files.len(), 4, "{files:?}");
        }
    }

    /// One running copy of the program updates at a time; another one
    /// starting meanwhile leaves the files alone and doesn't load a plugin
    /// being updated.
    #[cfg(windows)]
    #[test]
    fn updates_take_turns() {
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("app");
        let plugins = app.join("plugins");
        staged_plugin(&plugins);
        fs::write(plugin_plan(&plugins, "scp"), "scp.spl\tscp.spl.replaced-by-update\n").unwrap();
        let held = lock(&app).unwrap().expect("the folder can be written to");
        assert!(lock(&app).is_err());
        let s = at_start_with(Some(&app), Some(&plugins), &move_file);
        assert!(s.defer_plugins, "no plugin is loaded");
        assert!(s.failed[0].contains("another running RustySalamander"), "{s:?}");
        assert_eq!(fs::read_to_string(plugins.join("scp").join("scp.spl")).unwrap(), "old", "untouched");
        drop(held);
        let s = at_start_with(Some(&app), Some(&plugins), &move_file);
        assert_eq!((s.updated.as_slice(), s.skip.len()), (&["scp".to_owned()][..], 0), "{s:?}");
        assert_eq!(fs::read_to_string(plugins.join("scp").join("scp.spl")).unwrap(), "new");
    }

    /// Into a new folder: with `own`, the user's plugins and languages go
    /// along whole; without, only settings and what bundled plugins saved.
    #[cfg(windows)]
    #[test]
    fn new_folder_takes_own_plugins_when_asked() {
        let t = tempfile::tempdir().unwrap();
        let zip = release_zip(t.path(), "0.9.9");
        let old = t.path().join("apps").join("0.9.8");
        old_version(&old);
        let without = t.path().join("apps").join("0.9.9");
        unpack_release(&zip, &without).unwrap();
        let mut copied = migrate(&old, &without, false).unwrap();
        copied.sort();
        assert_eq!(copied, ["config.toml", r"plugins\scp\known_hosts"]);
        assert!(!without.join("plugins").join("ltocatalog").exists(), "no half plugin without its program");
        let with = t.path().join("apps").join("0.9.9b");
        unpack_release(&zip, &with).unwrap();
        let mut copied = migrate(&old, &with, true).unwrap();
        copied.sort();
        assert_eq!(copied, ["config.toml", r"lang\de.toml", r"plugins\loose.spl", r"plugins\ltocatalog\", r"plugins\scp\known_hosts"]);
        assert_eq!(fs::read_to_string(with.join("plugins/ltocatalog/ltocatalog.spl")).unwrap(), "own plugin");
        assert_eq!(fs::read_to_string(with.join("plugins/ltocatalog/lang/cs.toml")).unwrap(), "own");
        assert_eq!(fs::read_to_string(with.join("plugins/ltocatalog/catalog.sqlite")).unwrap(), "tapes");
        assert_eq!(fs::read_to_string(with.join("plugins/scp/scp.spl")).unwrap(), "new plugin", "a bundled plugin is the new one");
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
        let r = newest(&list, "v", "0.0.1", &program_zip).unwrap().expect("a release with a zip");
        assert!(r.asset_size > 1_000_000, "{r:?}");
        // the official plugins, once the list is on the main branch
        match official_list().and_then(|t| parse_official(&t)) {
            Ok(list) => assert!(list.iter().any(|p| p.id == "viewer"), "{list:?}"),
            Err(e) => eprintln!("the official list can't be read (yet?): {e}"),
        }
        let mut got = 0u64;
        http_get(&r.asset_url, "application/octet-stream", &mut |c, _| {
            got += c.len() as u64;
            Ok(())
        })
        .unwrap();
        assert_eq!(got, r.asset_size, "the redirect to the download host is followed");
    }
}
