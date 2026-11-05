//! SFTP: an SSH connection (connecting, checking the server's key against
//! known_hosts, signing in) with the SFTP subsystem on it, as a [`Remote`].
//!
//! Signing in and the key check work as in the SCP plugin; the keys the
//! user accepted there (and in OpenSSH's `~/.ssh/known_hosts`) are trusted
//! here too.

use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use russh::client::{self, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::{self, known_hosts, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, StatusCode};
use sal_plugin_api::{n_, tr, trf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Handle as Runtime;

use crate::remote::{join_remote, usable_name, Chunks, Entry, Remote};
use crate::{ConnectError, Target, CONNECT_TIMEOUT};

/// How to sign in.
pub enum Auth {
    /// A password; also answers keyboard-interactive prompts with it.
    Password(String),
    /// A private key file (OpenSSH or PEM), with its passphrase if any.
    Key { file: PathBuf, passphrase: String },
}

/// The known_hosts files to trust, the one new keys go to, and a key the
/// user has just accepted (by its fingerprint).
#[derive(Debug, Clone)]
pub struct HostKeys {
    pub read: Vec<PathBuf>,
    pub learn: PathBuf,
    pub accept: Option<String>,
}

/// A server key or certificate the user has to decide about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unverified {
    /// Not known yet: its fingerprint, and what kind of key it is or why
    /// the certificate isn't trusted.
    Unknown { fingerprint: String, about: String },
    /// Different from the one recorded for this server: possibly an attack.
    Changed { fingerprint: String },
}

struct Client {
    host: String,
    port: u16,
    keys: HostKeys,
    seen: Arc<Mutex<Option<Unverified>>>,
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
                    *self.seen.lock().unwrap() = Some(Unverified::Changed { fingerprint });
                    return Ok(false);
                }
                Err(_) => {} // an unreadable file trusts nothing
            }
        }
        if self.keys.accept.as_deref() == Some(fingerprint.as_str()) {
            if let Some(dir) = self.keys.learn.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = known_hosts::learn_known_hosts_path(&self.host, self.port, key, &self.keys.learn);
            return Ok(true);
        }
        *self.seen.lock().unwrap() = Some(Unverified::Unknown { fingerprint, about: key.algorithm().to_string() });
        Ok(false)
    }
}

/// An SSH error as an I/O error.
fn ssh_error(e: russh::Error) -> io::Error {
    match e {
        russh::Error::IO(e) => e,
        e => io::Error::other(e.to_string()),
    }
}

/// An SFTP error as an I/O error of the right kind.
fn sftp_error(e: russh_sftp::client::error::Error) -> io::Error {
    use russh_sftp::client::error::Error as E;
    match e {
        E::Status(s) => {
            let kind = match s.status_code {
                StatusCode::NoSuchFile => io::ErrorKind::NotFound,
                StatusCode::PermissionDenied => io::ErrorKind::PermissionDenied,
                StatusCode::NoConnection | StatusCode::ConnectionLost => io::ErrorKind::NotConnected,
                StatusCode::OpUnsupported => io::ErrorKind::Unsupported,
                _ => io::ErrorKind::Other,
            };
            let text = if s.error_message.trim().is_empty() { s.status_code.to_string() } else { s.error_message.trim().to_owned() };
            io::Error::new(kind, text)
        }
        E::Timeout => io::Error::new(io::ErrorKind::TimedOut, tr("The server didn't answer in time.")),
        e => io::Error::other(e.to_string()),
    }
}

/// A sign-in failure with `msg` (translated).
fn denied(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, tr(msg))
}

/// A signed-in SSH connection with its SFTP session.
pub struct Sftp {
    rt: Runtime,
    handle: Handle<Client>,
    sftp: SftpSession,
    home: String,
}

impl Sftp {
    /// Connect, check the server's key, sign in and start SFTP.
    pub fn open(rt: Runtime, target: &Target, auth: &Auth, keys: HostKeys) -> Result<Sftp, ConnectError> {
        let (handle, sftp, home) = rt.block_on(Self::connect(target, auth, keys))?;
        Ok(Sftp { rt, handle, sftp, home })
    }

    async fn connect(target: &Target, auth: &Auth, keys: HostKeys) -> Result<(Handle<Client>, SftpSession, String), ConnectError> {
        let seen = Arc::new(Mutex::new(None));
        let config = Arc::new(client::Config { keepalive_interval: Some(Duration::from_secs(30)), keepalive_max: 4, ..Default::default() });
        let client = Client { host: target.host.clone(), port: target.port, keys, seen: seen.clone() };
        let connecting = client::connect(config, (target.host.as_str(), target.port), client);
        let mut handle = match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
            Err(_) => return Err(io::Error::new(io::ErrorKind::TimedOut, tr("The server didn't answer in time.")).into()),
            Ok(Err(e)) => {
                if let Some(k) = seen.lock().unwrap().take() {
                    return Err(ConnectError::Unverified(k));
                }
                return Err(ssh_error(e).into());
            }
            Ok(Ok(h)) => h,
        };
        let signed_in = match auth {
            Auth::Password(password) => password_sign_in(&mut handle, &target.user, password).await?,
            Auth::Key { file, passphrase } => {
                let key = keys::load_secret_key(file, (!passphrase.is_empty()).then_some(passphrase.as_str())).map_err(|e| match e {
                    keys::Error::KeyIsEncrypted => io::Error::new(io::ErrorKind::InvalidInput, tr("The key file is protected by a passphrase: enter it.")),
                    e => io::Error::new(io::ErrorKind::InvalidData, trf("Cannot read the key {file}: {error}", &[("file", &file.display()), ("error", &e)])),
                })?;
                let hash = handle.best_supported_rsa_hash().await.ok().flatten().flatten();
                let r = handle.authenticate_publickey(&target.user, PrivateKeyWithHashAlg::new(Arc::new(key), hash)).await.map_err(ssh_error)?;
                if !r.success() {
                    return Err(denied(n_("The server didn't accept the key for this user name.")).into());
                }
                true
            }
        };
        if !signed_in {
            return Err(denied(n_("The server didn't accept the user name and password.")).into());
        }
        let ch = handle.channel_open_session().await.map_err(ssh_error)?;
        ch.request_subsystem(true, "sftp").await.map_err(ssh_error)?;
        let sftp = SftpSession::new(ch.into_stream())
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Unsupported, trf("The server doesn't offer SFTP: {error}", &[("error", &e)])))?;
        let home = sftp.canonicalize(".").await.ok().filter(|h| h.starts_with('/')).unwrap_or_else(|| "/".into());
        Ok((handle, sftp, home))
    }
}

/// Password sign-in; servers that ask through keyboard-interactive prompts
/// get the password for each hidden prompt.
async fn password_sign_in(handle: &mut Handle<Client>, user: &str, password: &str) -> io::Result<bool> {
    if handle.authenticate_password(user, password).await.map_err(ssh_error)?.success() {
        return Ok(true);
    }
    let mut r = handle.authenticate_keyboard_interactive_start(user, None).await.map_err(ssh_error)?;
    for _ in 0..5 {
        match r {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|p| if p.echo { String::new() } else { password.to_owned() }).collect();
                r = handle.authenticate_keyboard_interactive_respond(answers).await.map_err(ssh_error)?;
            }
        }
    }
    Ok(false)
}

/// SFTP's time (seconds since 1970) as a time.
fn time_of(secs: Option<u32>) -> Option<SystemTime> {
    secs.map(|s| UNIX_EPOCH + Duration::from_secs(s.into()))
}

impl Remote for Sftp {
    fn home(&self) -> String {
        self.home.clone()
    }

    fn list(&self, dir: &str) -> io::Result<Vec<Entry>> {
        self.rt.block_on(async {
            let rd = self.sftp.read_dir(dir).await.map_err(sftp_error)?;
            let mut out = Vec::new();
            for e in rd {
                let name = e.file_name();
                if !usable_name(&name) {
                    continue;
                }
                let mut md = e.metadata();
                // a link shows as what it points to, so links to folders open
                if md.is_symlink() {
                    if let Ok(target) = self.sftp.metadata(join_remote(dir, &name)).await {
                        md = target;
                    }
                }
                let dir = md.is_dir();
                out.push(Entry { name, dir, size: if dir { 0 } else { md.size.unwrap_or(0) }, modified: time_of(md.mtime) });
            }
            Ok(out)
        })
    }

    // `to` and `from` run outside the runtime, between the chunks, so they
    // may block and even call the plugin again

    fn read(&self, path: &str, to: Chunks) -> io::Result<()> {
        let mut f = self.rt.block_on(self.sftp.open(path)).map_err(sftp_error)?;
        let mut buf = vec![0u8; 256 * 1024];
        let r = loop {
            match self.rt.block_on(f.read(&mut buf)) {
                Ok(0) => break Ok(()),
                Ok(n) if !to(&buf[..n]) => break Err(io::Error::new(io::ErrorKind::Interrupted, tr("Cancelled."))),
                Ok(_) => {}
                Err(e) => break Err(e),
            }
        };
        let _ = self.rt.block_on(f.close());
        r
    }

    fn write(&self, path: &str, from: &mut dyn Read, modified: Option<SystemTime>) -> io::Result<()> {
        let mut f = self.rt.block_on(self.sftp.create(path)).map_err(sftp_error)?;
        let mut buf = vec![0u8; 256 * 1024];
        let written = loop {
            match from.read(&mut buf) {
                Ok(0) => break self.rt.block_on(async { f.flush().await.and(f.shutdown().await) }),
                Ok(n) => {
                    if let Err(e) = self.rt.block_on(f.write_all(&buf[..n])) {
                        break Err(e);
                    }
                }
                Err(e) => break Err(e),
            }
        };
        let _ = self.rt.block_on(f.close());
        if let Err(e) = written {
            // cancelled or failed: a part isn't left behind
            let _ = self.rt.block_on(self.sftp.remove_file(path));
            return Err(e);
        }
        if let Some(secs) = modified.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).and_then(|d| u32::try_from(d.as_secs()).ok()) {
            let attrs = FileAttributes { atime: Some(secs), mtime: Some(secs), ..FileAttributes::empty() };
            let _ = self.rt.block_on(self.sftp.set_metadata(path, attrs));
        }
        Ok(())
    }

    fn mkdir(&self, path: &str) -> io::Result<()> {
        self.rt.block_on(self.sftp.create_dir(path)).map_err(sftp_error)
    }

    fn remove_file(&self, path: &str) -> io::Result<()> {
        self.rt.block_on(self.sftp.remove_file(path)).map_err(sftp_error)
    }

    fn remove_dir(&self, path: &str) -> io::Result<()> {
        self.rt.block_on(self.sftp.remove_dir(path)).map_err(sftp_error)
    }

    fn rename(&self, old: &str, new: &str) -> io::Result<()> {
        self.rt.block_on(self.sftp.rename(old, new)).map_err(sftp_error)
    }

    fn close(&self) {
        self.rt.block_on(async {
            let _ = tokio::time::timeout(Duration::from_secs(3), self.sftp.close()).await;
            let _ = tokio::time::timeout(Duration::from_secs(3), self.handle.disconnect(russh::Disconnect::ByApplication, "", "en")).await;
        });
    }
}
