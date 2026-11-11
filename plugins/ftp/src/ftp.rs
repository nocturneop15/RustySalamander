//! FTP and FTPS (FTP with TLS, "explicit": `AUTH TLS` on the usual port)
//! as a [`Remote`].
//!
//! An FTP connection does one thing at a time, so the plugin keeps a few
//! of them per server ([`MAX_CONNECTIONS`]): a listing doesn't wait for a
//! copy to finish. A connection that was idle for a while is checked
//! (`NOOP`) and opened again when the server has closed it, and one that
//! broke during a command is not used again.
//!
//! Data connections ([`Transfer`]): passive mode is tried first, EPSV and
//! then PASV (connecting to the server's own address when PASV names
//! another, as servers behind NAT do), and active mode (PORT/EPRT) last.
//! The first way that opens a data connection is used from then on, by
//! every connection to the server. The user can also ask for passive or
//! active mode only.
//!
//! TLS certificates: one a public authority vouches for, for this server's
//! name, is trusted. Any other (self-signed, another name, expired) is
//! shown to the user with its SHA-256 fingerprint first; accepting it adds
//! it to the plugin's `known_certificates`. A certificate different from
//! the one remembered for the server is refused.

use std::io::{self, Read};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use sal_plugin_api::{tr, trf};
use sha2::{Digest, Sha256};
use suppaftp::types::FileType;
use suppaftp::{FtpError, Mode, RustlsConnector, RustlsFtpStream, Status, TlsStream, TransferStream};

use crate::remote::{ftp_time, join_remote, parse_list, parse_mlsd, Chunks, Entry, Remote};
use crate::sftp::Unverified;
use crate::{ConnectError, Target, CONNECT_TIMEOUT};

/// The most connections kept to one server.
pub const MAX_CONNECTIONS: usize = 3;
/// How long the server may take to answer a command or send data.
const IO_TIMEOUT: Duration = Duration::from_secs(60);
/// A connection idle longer than this is checked before it is used.
const CHECK_AFTER: Duration = Duration::from_secs(20);
/// How long opening a data connection may take before the next way is
/// tried (passive), or how long the server has to call (active).
const DATA_TIMEOUT: Duration = Duration::from_secs(10);

/// How data connections are opened, as the user chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    /// Passive, and active when no passive way works.
    Auto,
    /// Passive only (EPSV, then PASV).
    Passive,
    /// Active only: the server connects to this computer.
    Active,
}

impl Transfer {
    /// The ways to try, in order; PASV can't reach IPv6 servers.
    fn ways(self, ipv6: bool) -> Vec<Way> {
        let passive = if ipv6 { vec![Way::Epsv] } else { vec![Way::Epsv, Way::Pasv] };
        match self {
            Transfer::Auto => passive.into_iter().chain([Way::Active]).collect(),
            Transfer::Passive => passive,
            Transfer::Active => vec![Way::Active],
        }
    }
}

/// One way of opening data connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    /// Extended passive (EPSV): the server names only a port.
    Epsv,
    /// Passive (PASV): the server names an address and a port.
    Pasv,
    /// Active (PORT/EPRT): the server connects to this computer.
    Active,
}

/// Where to connect and how to sign in.
#[derive(Clone)]
pub struct Settings {
    pub target: Target,
    pub password: String,
    /// FTPS: the TLS settings with the certificate check.
    pub tls: Option<Arc<ClientConfig>>,
    pub transfer: Transfer,
}

/// One signed-in control connection.
struct Conn {
    s: RustlsFtpStream,
    /// The server lists with MLSD (exact times and sizes).
    mlsd: bool,
    /// The server sets file times with MFMT.
    mfmt: bool,
    used: Instant,
    /// Broke during a command: not to be used again.
    broken: bool,
    /// The code of the last reply that refused a command.
    refused: u32,
    /// The ways to try for data connections, in order.
    ways: Vec<Way>,
    /// The way that worked, shared by the server's connections.
    found: Arc<Mutex<Option<Way>>>,
    /// Set when a passive data connection couldn't be opened (the control
    /// connection is still fine then).
    data_failed: Arc<AtomicBool>,
}

/// An FTP error as an I/O error; `broken` is set when the connection
/// itself failed (not just the command).
fn ftp_error(e: FtpError, broken: &mut bool) -> io::Error {
    match e {
        FtpError::UnexpectedResponse(r) => {
            let text = String::from_utf8_lossy(&r.body);
            // "550 No such file or directory" → "No such file or directory"
            let text = text.trim().get(4..).map(str::trim).filter(|t| !t.is_empty()).unwrap_or(text.trim()).to_owned();
            let kind = match r.status.code() {
                530 | 532 => io::ErrorKind::PermissionDenied,
                550 | 450 if text.to_ascii_lowercase().contains("permission") || text.to_ascii_lowercase().contains("denied") => io::ErrorKind::PermissionDenied,
                550 | 450 => io::ErrorKind::NotFound,
                553 => io::ErrorKind::InvalidInput,
                421 => {
                    *broken = true;
                    io::ErrorKind::NotConnected
                }
                _ => io::ErrorKind::Other,
            };
            io::Error::new(kind, text)
        }
        FtpError::ConnectionError(e) => {
            *broken = true;
            e
        }
        e => {
            *broken = true;
            io::Error::other(e.to_string())
        }
    }
}

/// Open a TCP connection to `host:port`, trying each of its addresses.
fn tcp(host: &str, port: u16) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, trf("The server {server} wasn't found.", &[("server", &host)]));
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => return timeouts(s),
            Err(e) => last = e,
        }
    }
    Err(if last.kind() == io::ErrorKind::TimedOut { io::Error::new(io::ErrorKind::TimedOut, tr("The server didn't answer in time.")) } else { last })
}

/// `s` with the read and write timeouts set.
fn timeouts(s: TcpStream) -> io::Result<TcpStream> {
    s.set_read_timeout(Some(IO_TIMEOUT))?;
    s.set_write_timeout(Some(IO_TIMEOUT))?;
    Ok(s)
}

impl Conn {
    /// Connect, start TLS for FTPS, sign in, binary mode and UTF-8.
    fn open(set: &Settings, certs: Option<&Certificates>, found: Arc<Mutex<Option<Way>>>) -> Result<Conn, ConnectError> {
        let t = &set.target;
        let mut ignored = false;
        let control = tcp(&t.host, t.port)?;
        let data_failed = Arc::new(AtomicBool::new(false));
        let failed = data_failed.clone();
        let mut s = RustlsFtpStream::connect_with_stream(control)
            .map_err(|e| ftp_error(e, &mut ignored))?
            .active_mode(DATA_TIMEOUT)
            .passive_stream_builder(move |addr: SocketAddr| {
                TcpStream::connect_timeout(&addr, DATA_TIMEOUT).and_then(timeouts).map_err(|e| {
                    failed.store(true, Ordering::Relaxed);
                    FtpError::ConnectionError(e)
                })
            });
        if let Some(tls) = &set.tls {
            s = match s.into_secure(RustlsConnector::from(tls.clone()), &t.host) {
                Ok(s) => s,
                Err(e) => {
                    if let Some(seen) = certs.and_then(|c| c.seen.lock().unwrap().take()) {
                        return Err(ConnectError::Unverified(seen));
                    }
                    let e = ftp_error(e, &mut ignored);
                    return Err(io::Error::new(e.kind(), trf("The server can't start TLS (FTPS): {error}", &[("error", &e)])).into());
                }
            };
        }
        // PASV from a server behind NAT names its inner address: the
        // address in use is connected to instead
        s.set_passive_nat_workaround(true);
        let (user, password) = if t.user.is_empty() { ("anonymous", "anonymous@") } else { (t.user.as_str(), set.password.as_str()) };
        if let Err(e) = s.login(user, password) {
            let e = ftp_error(e, &mut ignored);
            return Err(if e.kind() == io::ErrorKind::PermissionDenied || e.kind() == io::ErrorKind::NotFound {
                io::Error::new(io::ErrorKind::PermissionDenied, trf("The server didn't accept the user name and password: {error}", &[("error", &e)]))
            } else {
                e
            }
            .into());
        }
        s.transfer_type(FileType::Binary).map_err(|e| ftp_error(e, &mut ignored))?;
        let feat = s.feat().unwrap_or_default();
        let has = |f: &str| feat.keys().any(|k| k.eq_ignore_ascii_case(f));
        if has("UTF8") {
            let _ = s.opts("UTF8", Some("ON"));
        }
        let ways = set.transfer.ways(t.host.contains(':'));
        Ok(Conn { mlsd: has("MLST"), mfmt: has("MFMT"), s, used: Instant::now(), broken: false, refused: 0, ways, found, data_failed })
    }

    /// Run `op`, a command with a data connection, opening it the way that
    /// worked before, or trying the ways in turn until one does (see the
    /// module's notes).
    fn data<T>(&mut self, mut op: impl FnMut(&mut RustlsFtpStream) -> Result<T, FtpError>) -> io::Result<T> {
        let known = *self.found.lock().unwrap();
        let ways = known.map_or_else(|| self.ways.clone(), |w| vec![w]);
        let mut tried: Vec<String> = Vec::new();
        self.refused = 0;
        for (i, &way) in ways.iter().enumerate() {
            self.s.set_mode(match way {
                Way::Epsv => Mode::ExtendedPassive,
                Way::Pasv => Mode::Passive,
                Way::Active => Mode::Active,
            });
            self.data_failed.store(false, Ordering::Relaxed);
            let last = i + 1 == ways.len();
            let e = match op(&mut self.s) {
                Ok(v) => {
                    *self.found.lock().unwrap() = Some(way);
                    return Ok(v);
                }
                Err(e) => e,
            };
            match e {
                // EPSV not known to the server: PASV next. If PASV is refused
                // the same way, it was the command, not the data connection.
                FtpError::UnexpectedResponse(r) if way == Way::Epsv && known.is_none() && !last && (500..=504).contains(&r.status.code()) => {
                    tried.push(String::from_utf8_lossy(&r.body).trim().to_owned());
                }
                // the data connection couldn't be opened; the control
                // connection is fine, as nothing was sent after EPSV/PASV
                FtpError::ConnectionError(e) if self.data_failed.load(Ordering::Relaxed) && !last => tried.push(e.to_string()),
                FtpError::ConnectionError(e) if self.data_failed.load(Ordering::Relaxed) || way == Way::Active => {
                    // a failed active connection leaves the server's answer
                    // to the command unread: this connection isn't used again
                    self.broken |= way == Way::Active;
                    tried.push(e.to_string());
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionRefused,
                        trf("No data connection to the server could be opened ({error}). Choose another data connection mode when connecting.", &[("error", &tried.join("; "))]),
                    ));
                }
                FtpError::UnexpectedResponse(r) => {
                    self.refused = r.status.code();
                    return Err(self.err(FtpError::UnexpectedResponse(r)));
                }
                e => return Err(self.err(e)),
            }
        }
        unreachable!("the last way returns")
    }

    fn err(&mut self, e: FtpError) -> io::Error {
        ftp_error(e, &mut self.broken)
    }

    /// End a transfer cut short; whether the connection can go on.
    fn end<T: TlsStream>(&mut self, stream: TransferStream<T>) -> bool {
        match stream.finish() {
            Ok(()) | Err(FtpError::UnexpectedResponse(_)) => true,
            Err(_) => {
                self.broken = true;
                false
            }
        }
    }

    /// The items of the folder `dir`.
    fn list(&mut self, dir: &str) -> io::Result<Vec<Entry>> {
        self.s.cwd(dir).map_err(|e| self.err(e))?;
        let mut found = None;
        if self.mlsd {
            match self.data(|s| s.mlsd(None)) {
                Ok(lines) => found = Some(parse_mlsd(&lines)),
                // announced but refused: LIST from now on
                Err(_) if (500..=504).contains(&self.refused) => self.mlsd = false,
                Err(e) => return Err(e),
            }
        }
        let found = match found {
            Some(f) => f,
            None => parse_list(&self.data(|s| s.list(None))?),
        };
        let mut out = Vec::with_capacity(found.len());
        for (mut e, link) in found {
            // a link is a folder when the server can go into it
            if link {
                e.dir = self.s.cwd(join_remote(dir, &e.name)).is_ok();
                if e.dir {
                    e.size = 0;
                }
            }
            out.push(e);
        }
        Ok(out)
    }

    fn read(&mut self, path: &str, to: Chunks) -> io::Result<()> {
        let mut stream = self.data(|s| s.retr_as_stream(path))?;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = match stream.read(&mut buf) {
                Ok(n) => n,
                Err(e) => {
                    self.broken = true;
                    return Err(e);
                }
            };
            if n == 0 {
                break;
            }
            if !to(&buf[..n]) {
                // closing the data connection ends the transfer; the server's
                // one reply to it (426 or 226) keeps the connection in step
                self.end(stream);
                return Err(io::Error::new(io::ErrorKind::Interrupted, tr("Cancelled.")));
            }
        }
        stream.finish().map_err(|e| self.err(e))
    }

    fn write(&mut self, path: &str, from: &mut dyn Read, modified: Option<SystemTime>) -> io::Result<()> {
        let mut stream = self.data(|s| s.put_with_stream(path))?;
        // not io::copy: it retries on Interrupted, which is how a cancel comes
        let mut buf = vec![0u8; 64 * 1024];
        let copied: io::Result<()> = loop {
            match from.read(&mut buf) {
                Ok(0) => break Ok(()),
                Ok(n) => {
                    if let Err(e) = io::Write::write_all(&mut stream, &buf[..n]) {
                        self.broken = true;
                        break Err(e);
                    }
                }
                Err(e) => break Err(e),
            }
        };
        if let Err(e) = copied {
            // cancelled, or the local file can't be read: a part isn't left behind
            if self.end(stream) {
                let _ = self.s.rm(path);
            }
            return Err(e);
        }
        stream.finish().map_err(|e| self.err(e))?;
        if let Some(t) = modified.filter(|_| self.mfmt).and_then(ftp_time) {
            let _ = self.s.custom_command(format!("MFMT {t} {path}"), &[Status::File]);
        }
        Ok(())
    }
}

/// The connections to one server.
pub struct Ftp {
    set: Settings,
    certs: Option<Arc<Certificates>>,
    home: String,
    /// The way of opening data connections that worked.
    found: Arc<Mutex<Option<Way>>>,
    idle: Mutex<(Vec<Conn>, usize)>,
    freed: Condvar,
}

impl Ftp {
    /// Connect and sign in; `certs` checks the TLS certificate (FTPS).
    pub fn open(set: Settings, certs: Option<Arc<Certificates>>) -> Result<Ftp, ConnectError> {
        let found = Arc::new(Mutex::new(None));
        let mut c = Conn::open(&set, certs.as_deref(), found.clone())?;
        let home = c.s.pwd().ok().filter(|h| h.starts_with('/')).unwrap_or_else(|| "/".into());
        Ok(Ftp { set, certs, home, found, idle: Mutex::new((vec![c], 1)), freed: Condvar::new() })
    }

    /// Run `op` on a free connection: an idle one (checked when idle for a
    /// while), a new one while there are fewer than [`MAX_CONNECTIONS`], or
    /// the next one freed.
    fn with<T>(&self, op: impl FnOnce(&mut Conn) -> io::Result<T>) -> io::Result<T> {
        let mut c = loop {
            let mut g = self.idle.lock().unwrap();
            if let Some(mut c) = g.0.pop() {
                drop(g);
                if c.used.elapsed() < CHECK_AFTER || c.s.noop().is_ok() {
                    break c;
                }
                // closed by the server: open another one in its place
                let _ = self.idle.lock().map(|mut g| g.1 -= 1);
                continue;
            }
            if g.1 < MAX_CONNECTIONS {
                g.1 += 1;
                drop(g);
                match Conn::open(&self.set, self.certs.as_deref(), self.found.clone()) {
                    Ok(c) => break c,
                    Err(e) => {
                        self.idle.lock().unwrap().1 -= 1;
                        self.freed.notify_one();
                        return Err(match e {
                            ConnectError::Io(e) => io::Error::new(io::ErrorKind::NotConnected, trf("The connection was lost ({error}). Disconnect and connect again.", &[("error", &e)])),
                            ConnectError::Unverified(_) => io::Error::new(io::ErrorKind::PermissionDenied, tr("The server's certificate has changed. Disconnect and connect again.")),
                        });
                    }
                }
            }
            drop(self.freed.wait(g).unwrap());
        };
        let r = op(&mut c);
        c.used = Instant::now();
        let mut g = self.idle.lock().unwrap();
        if c.broken {
            g.1 -= 1;
        } else {
            g.0.push(c);
        }
        drop(g);
        self.freed.notify_one();
        r
    }
}

/// A path as an FTP command can carry it: a line break would end the
/// command and start another one (`DELE …`), so such a path is refused.
pub(crate) fn one_line(path: &str) -> io::Result<&str> {
    if path.contains(['\r', '\n', '\0']) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, tr("A name or path on an FTP server can't contain line breaks.")));
    }
    Ok(path)
}

impl Remote for Ftp {
    fn home(&self) -> String {
        self.home.clone()
    }

    fn list(&self, dir: &str) -> io::Result<Vec<Entry>> {
        let dir = one_line(dir)?;
        self.with(|c| c.list(dir))
    }

    fn read(&self, path: &str, to: Chunks) -> io::Result<()> {
        let path = one_line(path)?;
        self.with(|c| c.read(path, to))
    }

    fn write(&self, path: &str, from: &mut dyn Read, modified: Option<SystemTime>) -> io::Result<()> {
        let path = one_line(path)?;
        self.with(|c| c.write(path, from, modified))
    }

    fn mkdir(&self, path: &str) -> io::Result<()> {
        let path = one_line(path)?;
        self.with(|c| c.s.mkdir(path).map_err(|e| c.err(e)))
    }

    fn remove_file(&self, path: &str) -> io::Result<()> {
        let path = one_line(path)?;
        self.with(|c| c.s.rm(path).map_err(|e| c.err(e)))
    }

    fn remove_dir(&self, path: &str) -> io::Result<()> {
        let path = one_line(path)?;
        self.with(|c| c.s.rmdir(path).map_err(|e| c.err(e)))
    }

    fn rename(&self, old: &str, new: &str) -> io::Result<()> {
        let (old, new) = (one_line(old)?, one_line(new)?);
        self.with(|c| c.s.rename(old, new).map_err(|e| c.err(e)))
    }

    fn close(&self) {
        let conns = std::mem::take(&mut self.idle.lock().unwrap().0);
        for mut c in conns {
            let _ = c.s.quit();
        }
    }
}

/// The SHA-256 fingerprint of a certificate: `AB:CD:…`.
pub fn fingerprint(cert: &[u8]) -> String {
    Sha256::digest(cert).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// The TLS certificate check of one server (see the module's notes).
#[derive(Debug)]
pub struct Certificates {
    host: String,
    port: u16,
    /// The file of the certificates the user accepted: `host:port fingerprint`.
    file: PathBuf,
    /// The fingerprint of a certificate the user has just accepted.
    accept: Option<String>,
    /// What the user has to decide about, from the last check.
    seen: Mutex<Option<Unverified>>,
    authorities: Arc<WebPkiServerVerifier>,
}

impl Certificates {
    /// The TLS settings for `host:port` with this check.
    pub fn config(host: &str, port: u16, file: PathBuf, accept: Option<String>) -> io::Result<(Arc<ClientConfig>, Arc<Certificates>)> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let authorities = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone()).build().map_err(io::Error::other)?;
        let certs = Arc::new(Certificates { host: host.to_owned(), port, file, accept, seen: Mutex::new(None), authorities });
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(io::Error::other)?
            .dangerous()
            .with_custom_certificate_verifier(certs.clone())
            .with_no_client_auth();
        Ok((Arc::new(config), certs))
    }

    /// This server's line in the file: `host:port`.
    fn key(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// The fingerprint remembered for this server.
    fn remembered(&self) -> Option<String> {
        let text = std::fs::read_to_string(&self.file).ok()?;
        let key = self.key();
        text.lines().find_map(|l| {
            let (k, f) = l.trim().split_once(' ')?;
            (k == key).then(|| f.trim().to_owned())
        })
    }

    /// Remember `fingerprint` for this server, in place of an older one.
    fn remember(&self, fingerprint: &str) {
        let key = self.key();
        let mut lines: Vec<String> = std::fs::read_to_string(&self.file).unwrap_or_default().lines().filter(|l| l.split(' ').next() != Some(&key)).map(str::to_owned).collect();
        lines.push(format!("{key} {fingerprint}"));
        if let Some(dir) = self.file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&self.file, lines.join("\n") + "\n");
    }
}

impl ServerCertVerifier for Certificates {
    fn verify_server_cert(&self, cert: &CertificateDer<'_>, intermediates: &[CertificateDer<'_>], name: &ServerName<'_>, ocsp: &[u8], now: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        let why = match self.authorities.verify_server_cert(cert, intermediates, name, ocsp, now) {
            Ok(v) => return Ok(v),
            Err(e) => e,
        };
        let fingerprint = fingerprint(cert);
        match self.remembered() {
            Some(f) if f == fingerprint => return Ok(ServerCertVerified::assertion()),
            Some(_) => {
                *self.seen.lock().unwrap() = Some(Unverified::Changed { fingerprint });
                return Err(why);
            }
            None => {}
        }
        if self.accept.as_deref() == Some(fingerprint.as_str()) {
            self.remember(&fingerprint);
            return Ok(ServerCertVerified::assertion());
        }
        *self.seen.lock().unwrap() = Some(Unverified::Unknown { fingerprint, about: why.to_string() });
        Err(why)
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.authorities.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.authorities.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.authorities.supported_verify_schemes()
    }
}
