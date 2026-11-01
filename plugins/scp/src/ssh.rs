//! One SSH connection: connecting and checking the server's key, signing in,
//! running commands, and copying files with the SCP protocol.
//!
//! SCP has no listing or file management of its own; like other SCP
//! clients, the plugin runs `ls`, `rm`, `mv` and `mkdir` on the server and
//! uses `scp -f` / `scp -t` on the server's side for the transfers.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use russh::client::{self, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::{self, known_hosts, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::ChannelMsg;
use sal_plugin_api::{n_, tr, trf};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::remote::quote;

/// How long connecting may take before it is given up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Where to connect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub user: String,
}

/// How to sign in.
pub enum Auth {
    /// A password; also answers keyboard-interactive prompts with it.
    Password(String),
    /// A private key file (OpenSSH or PEM), with its passphrase if any.
    Key { file: PathBuf, passphrase: String },
}

/// A server key the user has to decide about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKey {
    /// Not in any known_hosts file yet.
    Unknown { fingerprint: String, algorithm: String },
    /// Different from the key recorded for this server: possibly an attack.
    Changed { fingerprint: String },
}

/// Why connecting failed.
#[derive(Debug)]
pub enum ConnectError {
    HostKey(HostKey),
    Io(io::Error),
}

impl From<io::Error> for ConnectError {
    fn from(e: io::Error) -> Self {
        ConnectError::Io(e)
    }
}

/// The known_hosts files to trust, the one new keys go to, and a key the
/// user has just accepted (by its fingerprint).
#[derive(Debug, Clone)]
pub struct HostKeys {
    pub read: Vec<PathBuf>,
    pub learn: PathBuf,
    pub accept: Option<String>,
}

struct Client {
    host: String,
    port: u16,
    keys: HostKeys,
    seen: Arc<Mutex<Option<HostKey>>>,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return Ok(false); // host certificates aren't supported
        };
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        for path in self.keys.read.iter().filter(|p| p.is_file()) {
            match known_hosts::check_known_hosts_path(&self.host, self.port, key, path) {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(keys::Error::KeyChanged { .. }) => {
                    *self.seen.lock().unwrap() = Some(HostKey::Changed { fingerprint });
                    return Ok(false);
                }
                Err(_) => {} // an unreadable file trusts nothing
            }
        }
        if self.keys.accept.as_deref() == Some(fingerprint.as_str()) {
            let _ = known_hosts::learn_known_hosts_path(&self.host, self.port, key, &self.keys.learn);
            return Ok(true);
        }
        *self.seen.lock().unwrap() = Some(HostKey::Unknown { fingerprint, algorithm: key.algorithm().to_string() });
        Ok(false)
    }
}

/// What a command printed, and how it ended.
pub struct Output {
    pub code: u32,
    pub out: Vec<u8>,
    pub err: Vec<u8>,
}

/// A signed-in connection.
pub struct Connection {
    handle: Handle<Client>,
    /// The user's home folder on the server (`/home/me`).
    pub home: String,
}

/// An SSH error as an I/O error.
fn io_error(e: russh::Error) -> io::Error {
    match e {
        russh::Error::IO(e) => e,
        e => io::Error::other(e.to_string()),
    }
}

/// A sign-in failure with `msg` (translated).
fn denied(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, tr(msg))
}

impl Connection {
    /// Connect, check the server's key and sign in.
    pub async fn open(target: &Target, auth: &Auth, keys: HostKeys) -> Result<Connection, ConnectError> {
        let seen = Arc::new(Mutex::new(None));
        let config = Arc::new(client::Config { keepalive_interval: Some(Duration::from_secs(30)), keepalive_max: 4, ..Default::default() });
        let client = Client { host: target.host.clone(), port: target.port, keys, seen: seen.clone() };
        let connecting = client::connect(config, (target.host.as_str(), target.port), client);
        let mut handle = match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
            Err(_) => return Err(io::Error::new(io::ErrorKind::TimedOut, tr("The server didn't answer in time.")).into()),
            Ok(Err(e)) => {
                if let Some(k) = seen.lock().unwrap().take() {
                    return Err(ConnectError::HostKey(k));
                }
                return Err(io_error(e).into());
            }
            Ok(Ok(h)) => h,
        };
        let signed_in = match auth {
            Auth::Password(password) => Self::password(&mut handle, &target.user, password).await?,
            Auth::Key { file, passphrase } => {
                let key = keys::load_secret_key(file, (!passphrase.is_empty()).then_some(passphrase.as_str())).map_err(|e| match e {
                    keys::Error::KeyIsEncrypted => io::Error::new(io::ErrorKind::InvalidInput, tr("The key file is protected by a passphrase: enter it.")),
                    e => io::Error::new(io::ErrorKind::InvalidData, trf("Cannot read the key {file}: {error}", &[("file", &file.display()), ("error", &e)])),
                })?;
                let hash = handle.best_supported_rsa_hash().await.ok().flatten().flatten();
                let r = handle.authenticate_publickey(&target.user, PrivateKeyWithHashAlg::new(Arc::new(key), hash)).await.map_err(io_error)?;
                if !r.success() {
                    return Err(denied(n_("The server didn't accept the key for this user name.")).into());
                }
                true
            }
        };
        if !signed_in {
            return Err(denied(n_("The server didn't accept the user name and password.")).into());
        }
        let mut c = Connection { handle, home: "/".into() };
        if let Ok(o) = c.exec("pwd").await {
            let home = String::from_utf8_lossy(&o.out).trim().to_owned();
            if o.code == 0 && home.starts_with('/') {
                c.home = home;
            }
        }
        Ok(c)
    }

    /// Password sign-in; servers that ask through keyboard-interactive
    /// prompts get the password for each hidden prompt.
    async fn password(handle: &mut Handle<Client>, user: &str, password: &str) -> io::Result<bool> {
        if handle.authenticate_password(user, password).await.map_err(io_error)?.success() {
            return Ok(true);
        }
        let mut r = handle.authenticate_keyboard_interactive_start(user, None).await.map_err(io_error)?;
        for _ in 0..5 {
            match r {
                KeyboardInteractiveAuthResponse::Success => return Ok(true),
                KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
                KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                    let answers = prompts.iter().map(|p| if p.echo { String::new() } else { password.to_owned() }).collect();
                    r = handle.authenticate_keyboard_interactive_respond(answers).await.map_err(io_error)?;
                }
            }
        }
        Ok(false)
    }

    /// End the connection.
    pub async fn close(&self) {
        let _ = tokio::time::timeout(Duration::from_secs(3), self.handle.disconnect(russh::Disconnect::ByApplication, "", "en")).await;
    }

    /// Run a shell command on the server.
    pub async fn exec(&self, command: &str) -> io::Result<Output> {
        let mut ch = self.handle.channel_open_session().await.map_err(|e| self.lost(e))?;
        ch.exec(true, command).await.map_err(io_error)?;
        let mut o = Output { code: 0, out: Vec::new(), err: Vec::new() };
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } => o.out.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, ext: 1 } => o.err.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => o.code = exit_status,
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        Ok(o)
    }

    /// Run a command that must succeed; its output.
    pub async fn run(&self, command: &str) -> io::Result<String> {
        let o = self.exec(command).await?;
        if o.code != 0 {
            return Err(remote_error(&o.err, o.code));
        }
        Ok(String::from_utf8_lossy(&o.out).into_owned())
    }

    /// The error when the connection broke during `e`.
    fn lost(&self, e: russh::Error) -> io::Error {
        io::Error::new(io::ErrorKind::NotConnected, trf("The connection was lost ({error}). Disconnect and connect again.", &[("error", &e)]))
    }

    /// Copy the file or folder `remote` into the folder `out` with
    /// `scp -f`. `progress` gets the bytes received in this session
    /// (counted in `done`); false cancels.
    pub async fn download(&self, remote: &str, out: &Path, progress: &mut dyn FnMut(u64) -> bool, done: &mut u64) -> io::Result<()> {
        let ch = self.handle.channel_open_session().await.map_err(|e| self.lost(e))?;
        ch.exec(true, format!("scp -r -p -f -- {}", quote(remote))).await.map_err(io_error)?;
        let (r, mut w) = tokio::io::split(ch.into_stream());
        let mut r = BufReader::new(r);
        ack(&mut w).await?;
        let mut dirs = vec![out.to_path_buf()];
        let mut times: Option<SystemTime> = None;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut line = Vec::new();
            if r.read_until(b'\n', &mut line).await? == 0 {
                break; // the server is done
            }
            let text = String::from_utf8_lossy(&line[1..]).trim_end_matches('\n').to_owned();
            match line[0] {
                1 | 2 => return Err(remote_error(text.as_bytes(), 1)),
                b'T' => {
                    times = text.split(' ').next().and_then(|s| s.parse().ok()).map(|s: u64| UNIX_EPOCH + Duration::from_secs(s));
                    ack(&mut w).await?;
                }
                b'D' => {
                    let (_, name) = header(&text)?;
                    let dir = dirs.last().expect("dirs").join(name);
                    std::fs::create_dir_all(&dir)?;
                    dirs.push(dir);
                    times = None;
                    ack(&mut w).await?;
                }
                b'E' => {
                    if dirs.len() > 1 {
                        dirs.pop();
                    }
                    ack(&mut w).await?;
                }
                b'C' => {
                    let (size, name) = header(&text)?;
                    let path = dirs.last().expect("dirs").join(name);
                    ack(&mut w).await?;
                    let mut file = std::fs::File::create(&path)?;
                    let mut left = size;
                    while left > 0 {
                        let want = left.min(buf.len() as u64) as usize;
                        let n = r.read(&mut buf[..want]).await?;
                        if n == 0 {
                            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, tr("The connection ended in the middle of a file.")));
                        }
                        std::io::Write::write_all(&mut file, &buf[..n])?;
                        left -= n as u64;
                        *done += n as u64;
                        if !progress(*done) {
                            drop(file);
                            let _ = std::fs::remove_file(&path);
                            return Err(io::Error::new(io::ErrorKind::Interrupted, tr("Cancelled.")));
                        }
                    }
                    drop(file);
                    let mut status = [0u8];
                    r.read_exact(&mut status).await?;
                    if status[0] != 0 {
                        return Err(remote_error(b"", 1));
                    }
                    if let Some(t) = times.take() {
                        let _ = std::fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(t));
                    }
                    ack(&mut w).await?;
                }
                _ => return Err(io::Error::new(io::ErrorKind::InvalidData, tr("The server answered in a way SCP doesn't expect."))),
            }
        }
        Ok(())
    }

    /// Copy the file or folder `disk` into the server's folder `remote_dir`
    /// as `name` with `scp -t`. `progress` gets the bytes sent in this
    /// session; false cancels.
    pub async fn upload(&self, disk: &Path, remote_dir: &str, name: &str, progress: &mut dyn FnMut(u64) -> bool, sent: &mut u64) -> io::Result<()> {
        let ch = self.handle.channel_open_session().await.map_err(|e| self.lost(e))?;
        ch.exec(true, format!("scp -r -t -- {}", quote(remote_dir))).await.map_err(io_error)?;
        let (r, mut w) = tokio::io::split(ch.into_stream());
        let mut r = BufReader::new(r);
        read_ack(&mut r).await?;
        send(&mut r, &mut w, disk, name, progress, sent).await?;
        w.shutdown().await?;
        // let the server finish writing before the channel goes away
        let mut rest = Vec::new();
        let _ = r.read_to_end(&mut rest).await;
        match rest.first() {
            Some(1) | Some(2) => Err(remote_error(&rest[1..], 1)),
            _ => Ok(()),
        }
    }
}

/// `scp`'s "C0644 123 name" / "D0755 0 name": (size, safe name).
fn header(text: &str) -> io::Result<(u64, String)> {
    let mut parts = text.splitn(3, ' ');
    let (_mode, size, name) = (parts.next(), parts.next(), parts.next());
    let size = size.and_then(|s| s.parse().ok());
    match (size, name) {
        // a name from the server must stay inside the target folder
        (Some(size), Some(name)) if !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':', '\0']) => Ok((size, name.to_owned())),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, trf("The server sent an unsafe name: {name}", &[("name", &text)]))),
    }
}

/// Send the "go on" byte.
async fn ack<W: AsyncWrite + Unpin>(w: &mut W) -> io::Result<()> {
    w.write_all(&[0]).await?;
    w.flush().await
}

/// Wait for the other side's "go on"; an error message otherwise.
async fn read_ack<R: tokio::io::AsyncBufRead + Unpin>(r: &mut R) -> io::Result<()> {
    let mut b = [0u8];
    r.read_exact(&mut b).await?;
    if b[0] == 0 {
        return Ok(());
    }
    let mut line = Vec::new();
    r.read_until(b'\n', &mut line).await?;
    Err(remote_error(&line, 1))
}

/// Send a file or a folder tree with the SCP protocol.
async fn send<R, W>(r: &mut R, w: &mut W, disk: &Path, name: &str, progress: &mut dyn FnMut(u64) -> bool, sent: &mut u64) -> io::Result<()>
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    if name.contains(['\n', '/']) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, trf("The name can't be used on the server: {name}", &[("name", &name)])));
    }
    let md = std::fs::metadata(disk)?;
    if md.is_dir() {
        w.write_all(format!("D0755 0 {name}\n").as_bytes()).await?;
        read_ack(r).await?;
        let mut entries: Vec<_> = std::fs::read_dir(disk)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            Box::pin(send(r, w, &e.path(), &e.file_name().to_string_lossy(), progress, sent)).await?;
        }
        w.write_all(b"E\n").await?;
        return read_ack(r).await;
    }
    w.write_all(format!("C0644 {} {name}\n", md.len()).as_bytes()).await?;
    read_ack(r).await?;
    let mut file = tokio::fs::File::open(disk).await?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n]).await?;
        *sent += n as u64;
        if !progress(*sent) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, tr("Cancelled.")));
        }
    }
    w.write_all(&[0]).await?;
    w.flush().await?;
    read_ack(r).await
}

/// An error from what a command wrote to stderr.
pub fn remote_error(stderr: &[u8], code: u32) -> io::Error {
    let text = String::from_utf8_lossy(stderr);
    let text = text.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("").to_owned();
    let kind = if text.contains("No such file") {
        io::ErrorKind::NotFound
    } else if text.contains("Permission denied") {
        io::ErrorKind::PermissionDenied
    } else if text.contains("File exists") {
        io::ErrorKind::AlreadyExists
    } else {
        io::ErrorKind::Other
    };
    let msg = if text.is_empty() { trf("The command failed on the server (exit code {code}).", &[("code", &code)]) } else { text };
    io::Error::new(kind, msg)
}
