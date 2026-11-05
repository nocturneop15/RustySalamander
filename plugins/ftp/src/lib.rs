//! The FTP / SFTP plugin: files on FTP, FTPS and SFTP servers, shown in a
//! panel.
//!
//! **Plugins → Connect to FTP or SFTP server…** asks for the protocol, the
//! server, the user name and a password (or, for SFTP, a private key),
//! connects and opens the folder the server starts in, in the active panel,
//! as `sftp://me@host/home/me`, `ftp://…` or `ftps://…`. Copying in and
//! out, dragging out of the window, deleting, renaming and new folders then
//! work as in any folder.
//!
//! - **SFTP** runs over SSH (pure Rust, `russh` and `russh-sftp`); the
//!   server's key is checked against `known_hosts` as in the SCP plugin.
//! - **FTPS** is FTP with TLS (`AUTH TLS`, explicit); the certificate is
//!   checked, and one no authority vouches for is shown to the user first.
//! - **FTP** sends everything, the password too, unencrypted.
//!
//! The protocols differ only in [`remote::Remote`]; walking folders,
//! progress and the rules of [`FolderPlugin`] are here, once for all.

mod ftp;
mod remote;
mod sftp;

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sal_plugin_api::{export_plugin, join_inner, mount_of, n_, tr, trf, Command, Field, FieldKind, FolderPlugin, Form, Index, Item, Mount, Plugin, Progress, Reply, Sink, Updates, Values, ATTR_DIRECTORY, ATTR_HIDDEN};

use crate::ftp::{Certificates, Ftp, Settings, Transfer};
use crate::remote::{inner_path, join_remote, remote_path, Entry, Remote};
use crate::sftp::{Auth, HostKeys, Sftp, Unverified};

export_plugin!(FtpPlugin);

/// How long connecting may take before it is given up.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Where to connect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub user: String,
}

/// Why connecting failed.
#[derive(Debug)]
pub enum ConnectError {
    /// A key or certificate the user has to decide about.
    Unverified(Unverified),
    Io(io::Error),
}

impl From<io::Error> for ConnectError {
    fn from(e: io::Error) -> Self {
        ConnectError::Io(e)
    }
}

/// The protocols, by the scheme of their connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protocol {
    Sftp,
    Ftps,
    Ftp,
}

impl Protocol {
    fn of(value: &str) -> Protocol {
        match value {
            "ftp" => Protocol::Ftp,
            "ftps" => Protocol::Ftps,
            _ => Protocol::Sftp,
        }
    }
    fn scheme(self) -> &'static str {
        match self {
            Protocol::Sftp => "sftp",
            Protocol::Ftps => "ftps",
            Protocol::Ftp => "ftp",
        }
    }
    fn default_port(self) -> u16 {
        match self {
            Protocol::Sftp => 22,
            Protocol::Ftps | Protocol::Ftp => 21,
        }
    }
}

/// The plugin: its SSH runtime and the open connections by root
/// (`sftp://me@host`).
pub struct FtpPlugin {
    rt: Option<tokio::runtime::Runtime>,
    /// Where the plugin keeps the servers' keys and certificates the user
    /// accepted, and the last connection's details.
    data: PathBuf,
    conns: Mutex<HashMap<String, Arc<dyn Remote>>>,
}

impl Drop for FtpPlugin {
    fn drop(&mut self) {
        for (_, c) in self.conns.get_mut().unwrap().drain() {
            c.close();
        }
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

/// A folder the plugin may write to: its own, or
/// `%APPDATA%\RustySalamander\<name>` when that isn't writable (e.g. under
/// Program Files).
fn data_dir(plugin_dir: &Path, name: &str) -> PathBuf {
    let probe = plugin_dir.join(".write-test");
    if std::fs::write(&probe, b"").is_ok() {
        let _ = std::fs::remove_file(&probe);
        return plugin_dir.to_path_buf();
    }
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("RustySalamander").join(name)
}

/// The user's home folder, where `~/.ssh` keys are looked for.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

impl Plugin for FtpPlugin {
    const ID: &'static str = "ftp";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const UPDATES: Updates = Updates { repo: "nocturneop15/RustySalamander", tag: "ftp-v", changelog: "plugins/ftp/CHANGELOG.md" };
    // its form needs choices that follow others ("ftp|ftps", a disabled choice counting as its first option)
    const PROGRAM_MIN: &'static str = "0.9.2-beta";
    const NAME: &'static str = n_("FTP / SFTP");
    const DESCRIPTION: &'static str = n_("Connects to FTP, FTPS (FTP with TLS) and SFTP (SSH) servers and works with their files. Start with Plugins → Connect to FTP or SFTP server…");

    fn start(dir: &Path) -> Result<Self, String> {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("ftp").enable_all().build().map_err(|e| e.to_string())?;
        Ok(FtpPlugin { rt: Some(rt), data: data_dir(dir, "ftp"), conns: Mutex::new(HashMap::new()) })
    }
}

/// The parent folder and name of an inner path: `a\b\c` → (`a\b`, `c`).
fn split_inner(inner: &str) -> (&str, &str) {
    inner.rsplit_once('\\').unwrap_or(("", inner))
}

/// An item and where it is: its inner path, its server path, what the
/// server says about it.
struct Found {
    inner: String,
    remote: String,
    entry: Entry,
}

/// Bytes in a file or folder tree on disk.
fn disk_size(p: &Path) -> u64 {
    match std::fs::metadata(p) {
        Ok(m) if m.is_dir() => std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| disk_size(&e.path())).sum()).unwrap_or(0),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

/// A local file read for an upload, counting for the progress bar; the
/// progress saying stop ends it with `Interrupted`.
struct Counted<'a, 'p> {
    file: File,
    sent: &'a mut u64,
    total: u64,
    progress: &'a mut Progress<'p>,
}

impl Read for Counted<'_, '_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.file.read(buf)?;
        *self.sent += n as u64;
        if !(self.progress)(*self.sent, self.total.max(*self.sent)) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, tr("Cancelled.")));
        }
        Ok(n)
    }
}

impl FtpPlugin {
    fn runtime(&self) -> tokio::runtime::Handle {
        self.rt.as_ref().expect("runtime").handle().clone()
    }

    /// The connection a root (or a path inside it) belongs to.
    fn conn(&self, root: &Path) -> io::Result<Arc<dyn Remote>> {
        let (scheme, authority, _) = mount_of(root).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not an FTP or SFTP location"))?;
        self.conns.lock().unwrap().get(&format!("{scheme}://{authority}")).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                trf("Not connected to {server}. Connect again: Plugins → Connect to FTP or SFTP server…", &[("server", &authority)]),
            )
        })
    }

    /// The file of the SSH server keys the user accepted.
    fn known_hosts(&self) -> PathBuf {
        self.data.join("known_hosts")
    }

    /// The file of the TLS certificates the user accepted.
    fn known_certificates(&self) -> PathBuf {
        self.data.join("known_certificates")
    }

    /// The file the last connection's details are kept in (no password).
    fn last_file(&self) -> PathBuf {
        self.data.join("last-connection")
    }

    /// The keys trusted for SFTP: the plugin's own, OpenSSH's, and those the
    /// user accepted in the SCP plugin.
    fn trusted_keys(&self) -> Vec<PathBuf> {
        let mut read = vec![self.known_hosts()];
        read.extend(home_dir().map(|h| h.join(".ssh").join("known_hosts")));
        if let Some(plugins) = self.data.parent() {
            read.push(plugins.join("scp").join("known_hosts"));
        }
        read
    }

    /// The details of the last connection (never its password).
    fn last_values(&self) -> Values {
        const KEYS: [&str; 7] = ["protocol", "host", "port", "user", "auth", "key", "mode"];
        let saved = std::fs::read_to_string(self.last_file()).ok().and_then(|s| sal_plugin_api::ui::decode(&s)).unwrap_or_default();
        let mut v: Values = KEYS.iter().zip(saved).map(|(k, v)| (k.to_string(), v)).collect();
        v.entry("protocol".into()).or_insert_with(|| "sftp".into());
        v.entry("auth".into()).or_insert_with(|| "password".into());
        v.entry("mode".into()).or_insert_with(|| "auto".into());
        if v.get("key").is_none_or(|k| k.is_empty()) {
            let ssh = home_dir().map(|h| h.join(".ssh"));
            if let Some(k) = ssh.iter().flat_map(|d| ["id_ed25519", "id_ecdsa", "id_rsa"].map(|n| d.join(n))).find(|k| k.is_file()) {
                v.insert("key".into(), k.to_string_lossy().into_owned());
            }
        }
        v
    }

    /// Remember the connection's details for the next form (never the
    /// password or the passphrase).
    fn save_last(&self, v: &Values) {
        let parts: Vec<&str> = ["protocol", "host", "port", "user", "auth", "key", "mode"].iter().map(|k| v.get(*k).map_or("", String::as_str)).collect();
        let _ = std::fs::create_dir_all(&self.data);
        let _ = std::fs::write(self.last_file(), sal_plugin_api::ui::encode(&parts));
    }

    /// The form that asks where to connect, filled with `v`.
    fn connect_form(&self, v: &Values) -> Form {
        let val = |k: &str| v.get(k).cloned().unwrap_or_default();
        let field = |id: &str, label: &str, kind: FieldKind, enabled_if: Option<(&str, &str)>| Field {
            id: id.into(),
            label: tr(label),
            kind,
            value: val(id),
            enabled_if: enabled_if.map(|(a, b)| (a.into(), b.into())),
        };
        let choices = |c: &[(&str, &str)]| FieldKind::Choice(c.iter().map(|(v, l)| (v.to_string(), tr(l))).collect());
        Form {
            title: tr("Connect to FTP or SFTP server"),
            ok: tr("Connect"),
            command: "login".into(),
            fields: vec![
                field("protocol", n_("Protocol:"), choices(&[("sftp", n_("SFTP (SSH)")), ("ftps", n_("FTPS (FTP with TLS)")), ("ftp", n_("FTP (unencrypted)"))]), None),
                field("host", n_("Server:"), FieldKind::Text, None),
                field("port", n_("Port:"), FieldKind::Text, None),
                field(
                    "mode",
                    n_("Data connection:"),
                    choices(&[("auto", n_("Automatic")), ("passive", n_("Passive")), ("active", n_("Active"))]),
                    Some(("protocol", "ftp|ftps")),
                ),
                field("user", n_("User name:"), FieldKind::Text, None),
                // FTP signs in with a password only: the choice is for SFTP, and
                // off for FTP it counts as its first option, the password
                field("auth", n_("Sign in with:"), choices(&[("password", n_("Password")), ("key", n_("Private key"))]), Some(("protocol", "sftp"))),
                field("password", n_("Password:"), FieldKind::Password, Some(("auth", "password"))),
                field("key", n_("Key file:"), FieldKind::File, Some(("auth", "key"))),
                field("passphrase", n_("Key passphrase:"), FieldKind::Password, Some(("auth", "key"))),
                Field {
                    id: "help".into(),
                    label: tr("An empty port is the protocol's usual one: 22 for SFTP, 21 for FTP and FTPS. With FTP, an empty user name signs in as anonymous. Plain FTP sends the password unencrypted. Automatic data connections try passive mode first and active mode when passive doesn't get through."),
                    kind: FieldKind::Note,
                    ..Default::default()
                },
            ],
        }
    }

    /// The question about a key or certificate the user has to decide
    /// about, carrying what was entered over to the next try.
    fn trust_form(v: &Values, protocol: Protocol, server: &str, fingerprint: String, about: &str) -> Form {
        let text = if protocol == Protocol::Sftp {
            trf(
                "The server {server} is not known yet. Its {algorithm} key has the fingerprint\n\n{fingerprint}\n\nCompare it with the fingerprint your server's administrator gives you, and connect only if they match. The key is then remembered.",
                &[("server", &server), ("algorithm", &about), ("fingerprint", &fingerprint)],
            )
        } else {
            trf(
                "The certificate of {server} isn't vouched for by a known authority ({reason}). Its SHA-256 fingerprint is\n\n{fingerprint}\n\nCompare it with the fingerprint your server's administrator gives you, and connect only if they match. The certificate is then remembered.",
                &[("server", &server), ("reason", &about), ("fingerprint", &fingerprint)],
            )
        };
        let mut fields = vec![Field { id: "note".into(), label: text, kind: FieldKind::Note, ..Default::default() }];
        fields.extend(v.iter().filter(|(k, _)| *k != "fingerprint").map(|(k, val)| Field { id: k.clone(), kind: FieldKind::Hidden, value: val.clone(), ..Default::default() }));
        fields.push(Field { id: "fingerprint".into(), kind: FieldKind::Hidden, value: fingerprint, ..Default::default() });
        Form { title: tr("Unknown server"), ok: tr("Trust and connect"), command: "login".into(), fields }
    }

    /// Connect with what the form says.
    fn login(&self, v: &Values) -> io::Result<Reply> {
        let get = |k: &str| v.get(k).map_or("", |s| s.trim());
        let invalid = |msg: &str| io::Error::new(io::ErrorKind::InvalidInput, tr(msg));
        let protocol = Protocol::of(get("protocol"));
        let (mut user, mut host) = (get("user").to_owned(), get("host").to_owned());
        // "sftp://me@host:2222/" or "me@host" typed as the server
        if let Some((scheme, rest)) = host.clone().split_once("://") {
            if !matches!(scheme, "sftp" | "ftp" | "ftps") {
                return Err(invalid(n_("Enter the server's name or address.")));
            }
            host = rest.trim_end_matches('/').to_owned();
        }
        if let Some((u, h)) = host.clone().rsplit_once('@') {
            if user.is_empty() {
                user = u.to_owned();
            }
            host = h.to_owned();
        }
        let mut port_text = get("port").to_owned();
        if let Some((h, p)) = host.clone().rsplit_once(':').filter(|(h, p)| !h.contains(':') && p.bytes().all(|b| b.is_ascii_digit()) && !p.is_empty()) {
            host = h.to_owned();
            if port_text.is_empty() {
                port_text = p.to_owned();
            }
        }
        let host = host.trim_start_matches('[').trim_end_matches(']').to_owned();
        if host.is_empty() || host.contains(['/', '\\', ' ', '@']) {
            return Err(invalid(n_("Enter the server's name or address.")));
        }
        if user.contains(['/', '\\', '@', ':', '\r', '\n']) || (protocol == Protocol::Sftp && (user.is_empty() || user.contains(' '))) {
            return Err(invalid(n_("Enter a user name.")));
        }
        let port: u16 = match port_text.as_str() {
            "" => protocol.default_port(),
            p => p.parse().ok().filter(|&p| p > 0).ok_or_else(|| invalid(n_("The port must be a number from 1 to 65535.")))?,
        };
        // only SFTP signs in with a key (the form offers the choice only there)
        let key_auth = protocol == Protocol::Sftp && get("auth") == "key";
        let transfer = match get("mode") {
            "passive" => Transfer::Passive,
            "active" => Transfer::Active,
            _ => Transfer::Auto,
        };
        let shown_host = if host.contains(':') { format!("[{host}]") } else { host.clone() };
        let at = if user.is_empty() { shown_host.clone() } else { format!("{user}@{shown_host}") };
        let authority = if port == protocol.default_port() { at } else { format!("{at}:{port}") };
        let root = format!("{}://{authority}", protocol.scheme());
        let mut saved = v.clone();
        saved.insert("user".into(), user.clone());
        saved.insert("host".into(), host.clone());
        saved.insert("port".into(), if port == protocol.default_port() { String::new() } else { port.to_string() });
        self.save_last(&saved);
        let mount = |home: &str| Reply::Mount(Mount { scheme: protocol.scheme().into(), authority: authority.clone(), folder: inner_path(home) });
        if let Some(c) = self.conns.lock().unwrap().get(&root) {
            return Ok(mount(&c.home()));
        }

        let target = Target { host: host.clone(), port, user };
        let accept = v.get("fingerprint").filter(|f| !f.is_empty()).cloned();
        let opened: Result<Arc<dyn Remote>, ConnectError> = match protocol {
            Protocol::Sftp => {
                let auth = if key_auth {
                    if get("key").is_empty() {
                        return Err(invalid(n_("Choose the private key file.")));
                    }
                    Auth::Key { file: PathBuf::from(get("key")), passphrase: v.get("passphrase").cloned().unwrap_or_default() }
                } else {
                    Auth::Password(v.get("password").cloned().unwrap_or_default())
                };
                let keys = HostKeys { read: self.trusted_keys(), learn: self.known_hosts(), accept };
                Sftp::open(self.runtime(), &target, &auth, keys).map(|c| Arc::new(c) as Arc<dyn Remote>)
            }
            Protocol::Ftps | Protocol::Ftp => {
                let (tls, certs) = if protocol == Protocol::Ftps {
                    let (config, certs) = Certificates::config(&host, port, self.known_certificates(), accept)?;
                    (Some(config), Some(certs))
                } else {
                    (None, None)
                };
                let settings = Settings { target, password: v.get("password").cloned().unwrap_or_default(), tls, transfer };
                Ftp::open(settings, certs).map(|c| Arc::new(c) as Arc<dyn Remote>)
            }
        };
        match opened {
            Ok(c) => {
                let reply = mount(&c.home());
                self.conns.lock().unwrap().insert(root, c);
                Ok(reply)
            }
            Err(ConnectError::Unverified(Unverified::Unknown { fingerprint, about })) => Ok(Reply::Form(Self::trust_form(v, protocol, &shown_host, fingerprint, &about))),
            Err(ConnectError::Unverified(Unverified::Changed { fingerprint })) => {
                let msg = if protocol == Protocol::Sftp {
                    trf(
                        "WARNING: the key of {server} has changed (it is now {fingerprint}). Someone may be listening in on the connection, or the server was reinstalled. The connection was refused. If the change is expected, remove the server's old key from known_hosts.",
                        &[("server", &shown_host), ("fingerprint", &fingerprint)],
                    )
                } else {
                    trf(
                        "WARNING: the certificate of {server} has changed (it is now {fingerprint}). Someone may be listening in on the connection, or the server got a new certificate. The connection was refused. If the change is expected, remove the server's line from known_certificates in the plugin's folder.",
                        &[("server", &shown_host), ("fingerprint", &fingerprint)],
                    )
                };
                Err(io::Error::new(io::ErrorKind::PermissionDenied, msg))
            }
            Err(ConnectError::Io(e)) => Err(io::Error::new(e.kind(), trf("Cannot connect to {server}: {error}", &[("server", &shown_host), ("error", &e)]))),
        }
    }

    /// What the server says about `items` (inner paths), listing each
    /// parent folder once; `NotFound` for one that is gone.
    fn find(c: &dyn Remote, items: &[String]) -> io::Result<Vec<Found>> {
        let mut listed: HashMap<&str, Vec<Entry>> = HashMap::new();
        let mut out = Vec::with_capacity(items.len());
        for inner in items {
            if inner.is_empty() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, tr("The server's root folder can't be used here.")));
            }
            let (parent, name) = split_inner(inner);
            if !listed.contains_key(parent) {
                listed.insert(parent, c.list(&remote_path(parent))?);
            }
            let entry = listed[parent].iter().find(|e| e.name == name).cloned().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, trf("\"{name}\" is no longer on the server.", &[("name", &name)])))?;
            out.push(Found { inner: inner.clone(), remote: remote_path(inner), entry });
        }
        Ok(out)
    }

    /// Whether `inner` exists on the server.
    fn exists(c: &dyn Remote, inner: &str) -> io::Result<bool> {
        let (parent, name) = split_inner(inner);
        Ok(c.list(&remote_path(parent))?.iter().any(|e| e.name == name))
    }

    /// `items` with everything inside their folders, each folder before
    /// what it holds.
    fn walk(c: &dyn Remote, items: Vec<Found>) -> io::Result<Vec<Found>> {
        let mut out = Vec::new();
        let mut todo: Vec<Found> = items.into_iter().rev().collect();
        while let Some(f) = todo.pop() {
            if f.entry.dir {
                let mut children: Vec<Found> = c
                    .list(&f.remote)?
                    .into_iter()
                    .map(|e| Found { inner: join_inner(&f.inner, &e.name), remote: join_remote(&f.remote, &e.name), entry: e })
                    .collect();
                children.reverse();
                out.push(f);
                todo.extend(children);
            } else {
                out.push(f);
            }
        }
        Ok(out)
    }

    /// Copy the file or folder tree `disk` to the server path `remote`;
    /// `existing` is what is there now.
    fn put(c: &dyn Remote, disk: &Path, remote: &str, existing: Option<&Entry>, sent: &mut u64, total: u64, progress: &mut Progress) -> io::Result<()> {
        let md = std::fs::metadata(disk)?;
        let name = remote.rsplit('/').next().unwrap_or(remote);
        if md.is_dir() {
            let there: HashMap<String, Entry> = match existing {
                Some(e) if e.dir => c.list(remote)?.into_iter().map(|e| (e.name.clone(), e)).collect(),
                Some(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists, trf("\"{name}\" already exists.", &[("name", &name)]))),
                None => {
                    c.mkdir(remote)?;
                    HashMap::new()
                }
            };
            let mut entries: Vec<_> = std::fs::read_dir(disk)?.collect::<Result<_, _>>()?;
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                let n = e.file_name().to_string_lossy().into_owned();
                Self::put(c, &e.path(), &join_remote(remote, &n), there.get(&n), sent, total, progress)?;
            }
            return Ok(());
        }
        if existing.is_some_and(|e| e.dir) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, trf("\"{name}\" already exists.", &[("name", &name)])));
        }
        let mut from = Counted { file: File::open(disk)?, sent, total, progress };
        c.write(remote, &mut from, md.modified().ok())
    }
}

impl FolderPlugin for FtpPlugin {
    fn id(&self) -> &str {
        Self::ID
    }

    fn handles(&self, _name: &str) -> bool {
        false // only connections, opened from the Plugins menu
    }

    fn can_write(&self, _file: &Path) -> bool {
        true
    }

    fn status(&self) -> String {
        match self.conns.lock().unwrap().len() {
            0 => String::new(),
            n => trf("Connections open: {n}", &[("n", &n)]),
        }
    }

    fn list(&self, file: &Path, folder: &str) -> io::Result<Index> {
        let c = self.conn(file)?;
        let items = c
            .list(&remote_path(folder))?
            .into_iter()
            .map(|e| {
                let mut attrs = if e.dir { ATTR_DIRECTORY } else { 0 };
                if e.name.starts_with('.') {
                    attrs |= ATTR_HIDDEN;
                }
                Item { path: join_inner(folder, &e.name), dir: e.dir, size: e.size, modified: e.modified, attrs, index: None }
            })
            .collect();
        Ok(Index { items, complete: false })
    }

    fn extract(&self, file: &Path, items: &[String], out: &Path, progress: &mut Progress) -> io::Result<()> {
        let c = self.conn(file)?;
        let all = Self::walk(&*c, Self::find(&*c, items)?)?;
        let total: u64 = all.iter().map(|f| f.entry.size).sum();
        let mut done = 0u64;
        for f in &all {
            let local = out.join(&f.inner);
            if f.entry.dir {
                std::fs::create_dir_all(&local)?;
                continue;
            }
            if let Some(p) = local.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut disk = File::create(&local)?;
            let mut failed = None;
            let r = c.read(&f.remote, &mut |bytes| {
                if let Err(e) = std::io::Write::write_all(&mut disk, bytes) {
                    failed = Some(e);
                    return false;
                }
                done += bytes.len() as u64;
                progress(done, total.max(done))
            });
            drop(disk);
            if let Err(e) = r {
                // a file cut short isn't left behind
                let _ = std::fs::remove_file(&local);
                return Err(failed.unwrap_or(e));
            }
            if let Some(t) = f.entry.modified {
                let _ = File::options().write(true).open(&local).and_then(|f| f.set_modified(t));
            }
        }
        progress(done, done);
        Ok(())
    }

    fn stream(&self, file: &Path, items: &[String], sink: &mut dyn Sink) -> io::Result<()> {
        let c = self.conn(file)?;
        for f in Self::walk(&*c, Self::find(&*c, items)?)? {
            if !f.entry.dir && sink.begin(&f.inner, f.entry.size) {
                c.read(&f.remote, &mut |bytes| sink.write(bytes))?;
            }
        }
        Ok(())
    }

    fn add(&self, file: &Path, items: &[(PathBuf, String)], progress: &mut Progress) -> io::Result<()> {
        let c = self.conn(file)?;
        let total: u64 = items.iter().map(|(d, _)| disk_size(d)).sum();
        let mut sent = 0;
        let mut listed: HashMap<String, Vec<Entry>> = HashMap::new();
        for (disk, inner) in items {
            let (parent, name) = split_inner(inner);
            if !listed.contains_key(parent) {
                listed.insert(parent.to_owned(), c.list(&remote_path(parent))?);
            }
            let existing = listed[parent].iter().find(|e| e.name == name).cloned();
            Self::put(&*c, disk, &remote_path(inner), existing.as_ref(), &mut sent, total, progress)?;
        }
        progress(sent, sent);
        Ok(())
    }

    fn delete(&self, file: &Path, items: &[String], _progress: &mut Progress) -> io::Result<()> {
        if items.is_empty() || items.iter().any(|i| i.is_empty()) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, tr("The server's root folder can't be deleted.")));
        }
        let c = self.conn(file)?;
        // what a folder holds goes before the folder
        for f in Self::walk(&*c, Self::find(&*c, items)?)?.iter().rev() {
            if f.entry.dir {
                c.remove_dir(&f.remote)?;
            } else {
                c.remove_file(&f.remote)?;
            }
        }
        Ok(())
    }

    fn rename(&self, file: &Path, old: &str, new: &str, _progress: &mut Progress) -> io::Result<()> {
        let c = self.conn(file)?;
        let name = split_inner(new).1;
        if Self::exists(&*c, new)? {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, trf("\"{name}\" already exists.", &[("name", &name)])));
        }
        c.rename(&remote_path(old), &remote_path(new))
    }

    fn mkdir(&self, file: &Path, inner: &str, _progress: &mut Progress) -> io::Result<()> {
        let c = self.conn(file)?;
        let name = split_inner(inner).1;
        if Self::exists(&*c, inner)? {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, trf("\"{name}\" already exists.", &[("name", &name)])));
        }
        c.mkdir(&remote_path(inner))
    }

    fn commands(&self) -> Vec<Command> {
        vec![Command { id: "connect".into(), label: tr("Connect to FTP or SFTP server…") }]
    }

    fn run(&self, command: &str, input: &Values) -> io::Result<Reply> {
        match command {
            "connect" => Ok(Reply::Form(self.connect_form(&self.last_values()))),
            "login" => self.login(input),
            _ => Err(io::Error::new(io::ErrorKind::Unsupported, format!("unknown command \"{command}\""))),
        }
    }

    fn close(&self, root: &Path) {
        let Some((scheme, authority, _)) = mount_of(root) else { return };
        let c = self.conns.lock().unwrap().remove(&format!("{scheme}://{authority}"));
        if let Some(c) = c {
            c.close();
        }
    }
}

#[cfg(test)]
mod tests;
