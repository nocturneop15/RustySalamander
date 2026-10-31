//! The SCP plugin: files on SSH servers, shown in a panel.
//!
//! **Plugins → Connect to server…** asks for the server, the user name and a
//! password or a private key. The plugin connects over SSH (pure Rust,
//! `russh`), checks the server's key against `known_hosts`, and opens the
//! user's home folder in the active panel as `scp://user@host/home/user`.
//! Copying in and out, deleting, renaming and new folders then work as in
//! any folder.
//!
//! Files go over the SCP protocol (`scp -f` / `scp -t` on the server);
//! listing and file management use `ls`, `rm`, `mv` and `mkdir`, as SCP
//! itself has none. The server needs a POSIX shell and those programs,
//! which every Linux, BSD and macOS server has.
//!
//! Server keys: a key found in `%USERPROFILE%\.ssh\known_hosts` or in the
//! plugin's own `known_hosts` is trusted. An unknown key is shown to the
//! user with its fingerprint; accepting it adds it to the plugin's
//! `known_hosts`. A changed key is refused.

mod remote;
mod ssh;

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use sal_plugin_api::{export_plugin, mount_of, n_, tr, trf, Command, Field, FieldKind, FolderPlugin, Form, Index, Mount, Plugin, Progress, Reply, Updates, Values};

use crate::remote::{inner_path, list_command, parse_listing, quote, remote_path};
use crate::ssh::{Auth, ConnectError, Connection, HostKey, HostKeys, Target};

export_plugin!(Scp);

/// The plugin: its SSH runtime and the open connections by authority
/// (`me@host` or `me@host:2222`).
pub struct Scp {
    rt: Option<tokio::runtime::Runtime>,
    /// Where the plugin keeps `known_hosts` and the last connection's details.
    data: PathBuf,
    conns: Mutex<HashMap<String, Arc<Connection>>>,
}

impl Drop for Scp {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

/// A folder the plugin may write to: its own, or `%APPDATA%\RustySalamander\scp`
/// when that isn't writable (e.g. under Program Files).
fn data_dir(plugin_dir: &Path) -> PathBuf {
    let probe = plugin_dir.join(".write-test");
    if std::fs::write(&probe, b"").is_ok() {
        let _ = std::fs::remove_file(&probe);
        return plugin_dir.to_path_buf();
    }
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("RustySalamander").join("scp")
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

impl Plugin for Scp {
    const ID: &'static str = "scp";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const UPDATES: Updates = Updates { repo: "nocturneop15/RustySalamander", tag: "scp-v", changelog: "plugins/scp/CHANGELOG.md" };
    const NAME: &'static str = n_("SCP (SSH servers)");
    const DESCRIPTION: &'static str = n_("Connects to servers over SSH and works with their files through the SCP protocol. Start with Plugins → Connect to server…");

    fn start(dir: &Path) -> Result<Self, String> {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("scp").enable_all().build().map_err(|e| e.to_string())?;
        Ok(Scp { rt: Some(rt), data: data_dir(dir), conns: Mutex::new(HashMap::new()) })
    }
}

impl Scp {
    fn block<F: Future>(&self, f: F) -> F::Output {
        self.rt.as_ref().expect("runtime").block_on(f)
    }

    /// The connection a root (or a path inside it) belongs to.
    fn conn(&self, root: &Path) -> io::Result<Arc<Connection>> {
        let (_, authority, _) = mount_of(root).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not an SCP location"))?;
        self.conns
            .lock()
            .unwrap()
            .get(&authority)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, trf("Not connected to {server}. Connect again: Plugins → Connect to server…", &[("server", &authority)])))
    }

    fn known_hosts(&self) -> PathBuf {
        self.data.join("known_hosts")
    }

    fn last_file(&self) -> PathBuf {
        self.data.join("last-connection")
    }

    /// The details of the last connection (never its password).
    fn last_values(&self) -> Values {
        let saved = std::fs::read_to_string(self.last_file()).ok().and_then(|s| sal_plugin_api::ui::decode(&s)).unwrap_or_default();
        let mut v: Values = ["host", "port", "user", "auth", "key"].iter().zip(saved).map(|(k, v)| (k.to_string(), v)).collect();
        v.entry("port".into()).or_insert_with(|| "22".into());
        v.entry("auth".into()).or_insert_with(|| "password".into());
        if v.get("key").is_none_or(|k| k.is_empty()) {
            let ssh = home_dir().map(|h| h.join(".ssh"));
            if let Some(k) = ssh.iter().flat_map(|d| ["id_ed25519", "id_ecdsa", "id_rsa"].map(|n| d.join(n))).find(|k| k.is_file()) {
                v.insert("key".into(), k.to_string_lossy().into_owned());
            }
        }
        v
    }

    fn save_last(&self, v: &Values) {
        let parts: Vec<&str> = ["host", "port", "user", "auth", "key"].iter().map(|k| v.get(*k).map_or("", String::as_str)).collect();
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
        Form {
            title: tr("Connect to server"),
            ok: tr("Connect"),
            command: "login".into(),
            fields: vec![
                field("host", n_("Server:"), FieldKind::Text, None),
                field("port", n_("Port:"), FieldKind::Text, None),
                field("user", n_("User name:"), FieldKind::Text, None),
                field("auth", n_("Sign in with:"), FieldKind::Choice(vec![("password".into(), tr("Password")), ("key".into(), tr("Private key"))]), None),
                field("password", n_("Password:"), FieldKind::Password, Some(("auth", "password"))),
                field("key", n_("Key file:"), FieldKind::File, Some(("auth", "key"))),
                field("passphrase", n_("Key passphrase:"), FieldKind::Password, Some(("auth", "key"))),
            ],
        }
    }

    /// Connect with what the form says.
    fn login(&self, v: &Values) -> io::Result<Reply> {
        let get = |k: &str| v.get(k).map_or("", |s| s.trim());
        let invalid = |msg: &str| io::Error::new(io::ErrorKind::InvalidInput, tr(msg));
        let (mut user, mut host) = (get("user").to_owned(), get("host").to_owned());
        // "me@host" typed as the server
        if let Some((u, h)) = host.clone().rsplit_once('@') {
            if user.is_empty() {
                user = u.to_owned();
            }
            host = h.to_owned();
        }
        if host.is_empty() || host.contains(['/', '\\', ' ', '@']) {
            return Err(invalid(n_("Enter the server's name or address.")));
        }
        if user.is_empty() || user.contains(['/', '\\', ' ', '@', ':']) {
            return Err(invalid(n_("Enter a user name.")));
        }
        let port: u16 = match get("port") {
            "" => 22,
            p => p.parse().ok().filter(|&p| p > 0).ok_or_else(|| invalid(n_("The port must be a number from 1 to 65535.")))?,
        };
        let shown_host = if host.contains(':') { format!("[{host}]") } else { host.clone() };
        let authority = if port == 22 { format!("{user}@{shown_host}") } else { format!("{user}@{shown_host}:{port}") };
        let mut saved = v.clone();
        saved.insert("user".into(), user.clone());
        saved.insert("host".into(), host.clone());
        self.save_last(&saved);
        if let Some(c) = self.conns.lock().unwrap().get(&authority) {
            return Ok(Reply::Mount(Mount { scheme: "scp".into(), authority, folder: inner_path(&c.home) }));
        }

        let auth = if get("auth") == "key" {
            if get("key").is_empty() {
                return Err(invalid(n_("Choose the private key file.")));
            }
            Auth::Key { file: PathBuf::from(get("key")), passphrase: v.get("passphrase").cloned().unwrap_or_default() }
        } else {
            Auth::Password(v.get("password").cloned().unwrap_or_default())
        };
        let mut read = vec![self.known_hosts()];
        read.extend(home_dir().map(|h| h.join(".ssh").join("known_hosts")));
        let keys = HostKeys { read, learn: self.known_hosts(), accept: v.get("fingerprint").filter(|f| !f.is_empty()).cloned() };
        let target = Target { host: host.clone(), port, user };
        match self.block(Connection::open(&target, &auth, keys)) {
            Ok(c) => {
                let folder = inner_path(&c.home);
                self.conns.lock().unwrap().insert(authority.clone(), Arc::new(c));
                Ok(Reply::Mount(Mount { scheme: "scp".into(), authority, folder }))
            }
            Err(ConnectError::HostKey(HostKey::Unknown { fingerprint, algorithm })) => {
                // ask, carrying what was entered over to the next try
                let mut fields: Vec<Field> = vec![Field {
                    id: "note".into(),
                    label: trf(
                        "The server {server} is not known yet. Its {algorithm} key has the fingerprint\n\n{fingerprint}\n\nCompare it with the fingerprint your server's administrator gives you, and connect only if they match. The key is then remembered.",
                        &[("server", &shown_host), ("algorithm", &algorithm), ("fingerprint", &fingerprint)],
                    ),
                    kind: FieldKind::Note,
                    ..Default::default()
                }];
                fields.extend(v.iter().filter(|(k, _)| *k != "fingerprint").map(|(k, val)| Field { id: k.clone(), kind: FieldKind::Hidden, value: val.clone(), ..Default::default() }));
                fields.push(Field { id: "fingerprint".into(), kind: FieldKind::Hidden, value: fingerprint, ..Default::default() });
                Ok(Reply::Form(Form { title: tr("Unknown server"), ok: tr("Trust and connect"), command: "login".into(), fields }))
            }
            Err(ConnectError::HostKey(HostKey::Changed { fingerprint })) => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                trf(
                    "WARNING: the key of {server} has changed (it is now {fingerprint}). Someone may be listening in on the connection, or the server was reinstalled. The connection was refused. If the change is expected, remove the server's old key from known_hosts.",
                    &[("server", &shown_host), ("fingerprint", &fingerprint)],
                ),
            )),
            Err(ConnectError::Io(e)) => Err(io::Error::new(e.kind(), trf("Cannot connect to {server}: {error}", &[("server", &shown_host), ("error", &e)]))),
        }
    }

    /// Run `command` on the server; exit code 17 means the target exists.
    fn run_checked(&self, file: &Path, command: &str, target: &str) -> io::Result<()> {
        let c = self.conn(file)?;
        let o = self.block(c.exec(command))?;
        match o.code {
            0 => Ok(()),
            17 => Err(io::Error::new(io::ErrorKind::AlreadyExists, trf("\"{name}\" already exists.", &[("name", &target)]))),
            code => Err(ssh::remote_error(&o.err, code)),
        }
    }
}

/// The parent folder and name of an inner path: `a\b\c` → (`a\b`, `c`).
fn split_inner(inner: &str) -> (&str, &str) {
    inner.rsplit_once('\\').unwrap_or(("", inner))
}

/// Bytes in a file or folder tree on disk.
fn disk_size(p: &Path) -> u64 {
    match std::fs::metadata(p) {
        Ok(m) if m.is_dir() => std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| disk_size(&e.path())).sum()).unwrap_or(0),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

impl FolderPlugin for Scp {
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
        let text = self.block(c.run(&list_command(&remote_path(folder))))?;
        Ok(Index { items: parse_listing(&text, folder, SystemTime::now()), complete: false })
    }

    fn extract(&self, file: &Path, items: &[String], out: &Path, progress: &mut Progress) -> io::Result<()> {
        let c = self.conn(file)?;
        // the total for the progress bar, in KB as POSIX du gives it
        let paths: Vec<String> = items.iter().map(|i| quote(&remote_path(i))).collect();
        let total: u64 = self
            .block(c.exec(&format!("du -sk -- {} 2>/dev/null", paths.join(" "))))
            .map(|o| String::from_utf8_lossy(&o.out).lines().filter_map(|l| l.split_whitespace().next()?.parse::<u64>().ok()).sum::<u64>() * 1024)
            .unwrap_or(0);
        let mut done = 0;
        for inner in items {
            let (parent, _) = split_inner(inner);
            let dest = out.join(parent);
            std::fs::create_dir_all(&dest)?;
            self.block(c.download(&remote_path(inner), &dest, &mut |d| progress(d, total.max(d)), &mut done))?;
        }
        progress(done, done);
        Ok(())
    }

    fn add(&self, file: &Path, items: &[(PathBuf, String)], progress: &mut Progress) -> io::Result<()> {
        let c = self.conn(file)?;
        let total: u64 = items.iter().map(|(d, _)| disk_size(d)).sum();
        let mut sent = 0;
        for (disk, inner) in items {
            let (parent, name) = split_inner(inner);
            self.block(c.upload(disk, &remote_path(parent), name, &mut |s| progress(s, total.max(s)), &mut sent))?;
        }
        progress(sent, sent);
        Ok(())
    }

    fn delete(&self, file: &Path, items: &[String], _progress: &mut Progress) -> io::Result<()> {
        if items.is_empty() || items.iter().any(|i| i.is_empty()) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, tr("The server's root folder can't be deleted.")));
        }
        let c = self.conn(file)?;
        let paths: Vec<String> = items.iter().map(|i| quote(&remote_path(i))).collect();
        self.block(c.run(&format!("rm -rf -- {}", paths.join(" ")))).map(drop)
    }

    fn rename(&self, file: &Path, old: &str, new: &str, _progress: &mut Progress) -> io::Result<()> {
        let (o, n) = (quote(&remote_path(old)), quote(&remote_path(new)));
        self.run_checked(file, &format!("if [ -e {n} ] || [ -L {n} ]; then exit 17; fi; mv -- {o} {n}"), split_inner(new).1)
    }

    fn mkdir(&self, file: &Path, inner: &str, _progress: &mut Progress) -> io::Result<()> {
        let p = quote(&remote_path(inner));
        self.run_checked(file, &format!("if [ -e {p} ]; then exit 17; fi; mkdir -p -- {p}"), split_inner(inner).1)
    }

    fn commands(&self) -> Vec<Command> {
        vec![Command { id: "connect".into(), label: tr("Connect to server…") }]
    }

    fn run(&self, command: &str, input: &Values) -> io::Result<Reply> {
        match command {
            "connect" => Ok(Reply::Form(self.connect_form(&self.last_values()))),
            "login" => self.login(input),
            _ => Err(io::Error::new(io::ErrorKind::Unsupported, format!("unknown command \"{command}\""))),
        }
    }

    fn close(&self, root: &Path) {
        let Some((_, authority, _)) = mount_of(root) else { return };
        let c = self.conns.lock().unwrap().remove(&authority);
        if let Some(c) = c {
            self.block(c.close());
        }
    }
}

#[cfg(test)]
mod tests;
