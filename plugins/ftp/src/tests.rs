//! The plugin against real servers, one session each, the same steps for
//! every protocol ([`session`]):
//!
//! - SFTP: a small russh server that starts Git for Windows' `sftp-server`
//!   for the SFTP subsystem (skipped without Git for Windows);
//! - FTPS: a small FTP server here ([`FtpServer`]) with TLS on a
//!   self-signed certificate, listing with MLSD and setting times (MFMT);
//! - FTP: the same server without TLS, listing with LIST only.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use russh::keys;
use russh::server::{self, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use super::*;
use crate::remote::{ftp_time, mlsd_time};

const GIT_BIN: &str = r"C:\Program Files\Git\usr\bin";
const SFTP_SERVER: &str = r"C:\Program Files\Git\usr\lib\ssh\sftp-server.exe";
const USER: &str = "tester";
const PASSWORD: &str = "tajné heslo";

fn values(pairs: &[(&str, &str)]) -> Values {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// The files every session starts with, in `home`.
fn fill(home: &Path) {
    fs::create_dir_all(home.join("složka 日本")).unwrap();
    fs::write(home.join("složka 日本").join("soubor 🦎.txt"), "ahoj").unwrap();
    fs::write(home.join("velký.bin"), vec![7u8; 3_000_000]).unwrap();
    fs::write(home.join(".skrytý"), "x").unwrap();
}

/// The steps of a session on a connected server: `home` on disk is the
/// folder `home_inner` the plugin started in. `exact_times`: the server
/// lists times to the second; `keeps_times`: it sets the times of
/// uploaded files.
fn session(p: &FtpPlugin, root: &Path, home_inner: &str, home: &Path, scratch: &Path, exact_times: bool, keeps_times: bool) {
    let at = |n: &str| join_inner(home_inner, n);
    assert!(!p.status().is_empty());

    // listing
    let ix = p.list(root, home_inner).unwrap();
    let mut got: Vec<(String, bool, u64, bool)> = ix.items.iter().map(|i| (i.path.rsplit('\\').next().unwrap().to_owned(), i.dir, i.size, i.attrs & ATTR_HIDDEN != 0)).collect();
    got.sort();
    assert_eq!(got, [(".skrytý".into(), false, 1, true), ("složka 日本".into(), true, 0, false), ("velký.bin".into(), false, 3_000_000, false)]);
    assert!(!ix.complete && ix.items.iter().all(|i| i.path.starts_with(home_inner) && i.modified.is_some()));
    assert_eq!(p.list(root, &at("nic")).unwrap_err().kind(), io::ErrorKind::NotFound);

    // copy out: a file and a folder, with progress and the server's times
    let out = scratch.join("out");
    fs::create_dir_all(&out).unwrap();
    let mut seen = (0, 0);
    let items = [at("velký.bin"), at("složka 日本")];
    p.extract(root, &items, &out, &mut |d, total| {
        seen = (d, total);
        true
    })
    .unwrap();
    let local = out.join(home_inner);
    assert_eq!(fs::read(local.join("velký.bin")).unwrap(), vec![7u8; 3_000_000]);
    assert_eq!(fs::read_to_string(local.join("složka 日本").join("soubor 🦎.txt")).unwrap(), "ahoj");
    assert_eq!(seen, (3_000_004, 3_000_004));
    if exact_times {
        let server = fs::metadata(home.join("velký.bin")).unwrap().modified().unwrap();
        let copied = fs::metadata(local.join("velký.bin")).unwrap().modified().unwrap();
        assert_eq!(secs(copied), secs(server), "the copy has the server's time");
    }
    let cancelled = p.extract(root, &items[..1], &scratch.join("out2"), &mut |_, _| false);
    assert_eq!(cancelled.unwrap_err().kind(), io::ErrorKind::Interrupted);
    assert!(!scratch.join("out2").join(home_inner).join("velký.bin").exists(), "a file cut short isn't left");
    // the connection still works after a cancelled transfer
    assert_eq!(p.list(root, home_inner).unwrap().items.len(), 3);

    // streamed without disk (a drag out of the window): each file under
    // its inner path; skipped ones aren't read. While one file is read,
    // listing the folder doesn't wait for it (FTP: another connection).
    struct Got<'a> {
        files: Vec<(String, u64, usize)>,
        stop_after: usize,
        p: &'a FtpPlugin,
        root: &'a Path,
        folder: &'a str,
        listed: bool,
    }
    impl Sink for Got<'_> {
        fn begin(&mut self, inner: &str, size: u64) -> bool {
            self.files.push((inner.to_owned(), size, 0));
            true
        }
        fn write(&mut self, data: &[u8]) -> bool {
            if !self.listed {
                self.listed = true;
                assert_eq!(self.p.list(self.root, self.folder).unwrap().items.len(), 3, "listed during a transfer");
            }
            self.files.last_mut().unwrap().2 += data.len();
            self.files.last().unwrap().2 < self.stop_after
        }
    }
    let mut got = Got { files: Vec::new(), stop_after: usize::MAX, p, root, folder: home_inner, listed: false };
    p.stream(root, &items, &mut got).unwrap();
    got.files.sort();
    assert!(got.listed);
    assert_eq!(got.files, [(at("složka 日本\\soubor 🦎.txt"), 4, 4), (at("velký.bin"), 3_000_000, 3_000_000)]);
    let mut stopped = Got { files: Vec::new(), stop_after: 1000, p, root, folder: home_inner, listed: true };
    assert_eq!(p.stream(root, &items[..1], &mut stopped).unwrap_err().kind(), io::ErrorKind::Interrupted);
    assert!(p.list(root, home_inner).is_ok());

    // copy in: a file and a folder tree; an existing file is replaced
    let src = scratch.join("src");
    fs::create_dir_all(src.join("strom").join("větev")).unwrap();
    fs::write(src.join("strom").join("větev").join("list 🍃.txt"), "zelený").unwrap();
    fs::write(src.join("nový.txt"), "nový obsah").unwrap();
    let old = SystemTime::now() - Duration::from_secs(86_400 * 400);
    fs::File::options().write(true).open(src.join("nový.txt")).unwrap().set_modified(old).unwrap();
    fs::write(src.join(".skrytý"), "nahrazen").unwrap();
    let up = [(src.join("nový.txt"), at("nový.txt")), (src.join("strom"), at("strom")), (src.join(".skrytý"), at(".skrytý"))];
    p.add(root, &up, &mut |_, _| true).unwrap();
    assert_eq!(fs::read_to_string(home.join("nový.txt")).unwrap(), "nový obsah");
    assert_eq!(fs::read_to_string(home.join("strom").join("větev").join("list 🍃.txt")).unwrap(), "zelený");
    assert_eq!(fs::read_to_string(home.join(".skrytý")).unwrap(), "nahrazen");
    if keeps_times {
        assert_eq!(secs(fs::metadata(home.join("nový.txt")).unwrap().modified().unwrap()), secs(old), "the server keeps the file's time");
    }
    // a folder copied onto the folder already there merges into it
    fs::write(src.join("strom").join("druhý.txt"), "2").unwrap();
    p.add(root, &[(src.join("strom"), at("strom"))], &mut |_, _| true).unwrap();
    assert!(home.join("strom").join("druhý.txt").is_file() && home.join("strom").join("větev").join("list 🍃.txt").is_file());
    // cancelled: the part isn't left on the server
    fs::write(src.join("velký nový.bin"), vec![1u8; 3_000_000]).unwrap();
    let e = p.add(root, &[(src.join("velký nový.bin"), at("velký nový.bin"))], &mut |sent, _| sent < 100_000).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::Interrupted);
    assert!(!home.join("velký nový.bin").exists(), "no part left on the server");

    // rename, new folder, delete; what exists already is refused
    p.rename(root, &at("nový.txt"), &at("přejmenovaný it's.txt"), &mut |_, _| true).unwrap();
    assert!(home.join("přejmenovaný it's.txt").is_file() && !home.join("nový.txt").exists());
    assert_eq!(p.rename(root, &at("strom"), &at("velký.bin"), &mut |_, _| true).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    p.mkdir(root, &at("prázdná"), &mut |_, _| true).unwrap();
    assert!(home.join("prázdná").is_dir());
    assert_eq!(p.mkdir(root, &at("prázdná"), &mut |_, _| true).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    p.delete(root, &[at("strom"), at("prázdná"), at("přejmenovaný it's.txt")], &mut |_, _| true).unwrap();
    assert!(!home.join("strom").exists() && !home.join("prázdná").exists() && !home.join("přejmenovaný it's.txt").exists());
    assert!(p.delete(root, &[String::new()], &mut |_, _| true).is_err(), "never the root");
    assert_eq!(p.delete(root, &[at("nic")], &mut |_, _| true).unwrap_err().kind(), io::ErrorKind::NotFound);

    // closing; the folder is then unreachable until connecting again
    p.close(root);
    assert_eq!(p.list(root, home_inner).unwrap_err().kind(), io::ErrorKind::NotConnected);
    assert!(p.status().is_empty());
}

fn secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap().as_secs()
}

// ---------------------------------------------------------------- SFTP

/// A key made by Git's ssh-keygen.
fn keygen(path: &Path) {
    let ok = std::process::Command::new(Path::new(GIT_BIN).join("ssh-keygen.exe"))
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(path)
        .stdin(Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(ok, "ssh-keygen failed");
}

/// Where each channel's input goes: data, then `None` at its end.
type Stdins = Arc<Mutex<HashMap<ChannelId, mpsc::UnboundedSender<Option<Vec<u8>>>>>>;

#[derive(Clone)]
struct SshServer {
    home: PathBuf,
    stdins: Stdins,
}

impl server::Server for SshServer {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        self.clone()
    }
}

impl server::Handler for SshServer {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<server::Auth, Self::Error> {
        Ok(if user == USER && password == PASSWORD { server::Auth::Accept } else { server::Auth::reject() })
    }

    async fn channel_open_session(&mut self, _: Channel<Msg>, reply: server::ChannelOpenHandle, _: &mut Session) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(&mut self, channel: ChannelId, name: &str, session: &mut Session) -> Result<(), Self::Error> {
        if name != "sftp" {
            session.channel_failure(channel)?;
            return Ok(());
        }
        let mut child = tokio::process::Command::new(SFTP_SERVER)
            .current_dir(&self.home)
            .env("PATH", GIT_BIN)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(russh::Error::IO)?;
        let (tx, mut rx) = mpsc::unbounded_channel::<Option<Vec<u8>>>();
        self.stdins.lock().unwrap().insert(channel, tx);
        let mut stdin = child.stdin.take().unwrap();
        tokio::spawn(async move {
            while let Some(Some(d)) = rx.recv().await {
                if stdin.write_all(&d).await.is_err() {
                    break;
                }
            }
        });
        let handle = session.handle();
        let mut out = child.stdout.take().unwrap();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 64 * 1024];
            while let Ok(n @ 1..) = out.read(&mut buf).await {
                let _ = handle.data(channel, buf[..n].to_vec()).await;
            }
            let _ = child.wait().await;
            let _ = handle.eof(channel).await;
            let _ = handle.close(channel).await;
        });
        session.channel_success(channel)?;
        Ok(())
    }

    async fn data(&mut self, channel: ChannelId, data: &[u8], _: &mut Session) -> Result<(), Self::Error> {
        if let Some(tx) = self.stdins.lock().unwrap().get(&channel) {
            let _ = tx.send(Some(data.to_vec()));
        }
        Ok(())
    }

    async fn channel_eof(&mut self, channel: ChannelId, _: &mut Session) -> Result<(), Self::Error> {
        if let Some(tx) = self.stdins.lock().unwrap().remove(&channel) {
            let _ = tx.send(None);
        }
        Ok(())
    }
}

#[test]
fn a_session_with_an_sftp_server() {
    if !Path::new(SFTP_SERVER).is_file() {
        eprintln!("Git for Windows' sftp-server not found; skipping");
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let (host_key, other_key) = (t.path().join("host"), t.path().join("other"));
    keygen(&host_key);
    keygen(&other_key);
    let home = t.path().join("home");
    fill(&home);

    let server_rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let config = Arc::new(server::Config {
        keys: vec![keys::load_secret_key(&host_key, None).unwrap()],
        auth_rejection_time: Duration::from_millis(10),
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let mut server = SshServer { home: home.clone(), stdins: Arc::default() };
    let listener = server_rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let port = listener.local_addr().unwrap().port();
    server_rt.spawn(async move {
        let _ = server.run_on_socket(config, &listener).await;
    });

    // the plugin in plugins\ftp, beside the SCP plugin's known_hosts
    let data = t.path().join("plugins").join("ftp");
    fs::create_dir_all(&data).unwrap();
    let p = FtpPlugin::start(&data).unwrap();
    let port_s = port.to_string();
    let login = |extra: &[(&str, &str)]| {
        let mut v = values(&[("protocol", "sftp"), ("host", "127.0.0.1"), ("port", &port_s), ("user", USER), ("auth", "password"), ("password", PASSWORD)]);
        v.extend(values(extra));
        p.run("login", &v)
    };

    // the menu command shows the form
    let Reply::Form(form) = p.run("connect", &Values::new()).unwrap() else { panic!("no form") };
    assert_eq!(form.command, "login");
    assert!(form.fields.iter().any(|f| f.id == "protocol" && matches!(&f.kind, FieldKind::Choice(c) if c.len() == 3)));

    // an unknown server: the user sees its fingerprint first
    let Reply::Form(trust) = login(&[]).unwrap() else { panic!("expected the trust question") };
    let fingerprint = trust.fields.iter().find(|f| f.id == "fingerprint").unwrap().value.clone();
    assert!(fingerprint.starts_with("SHA256:"), "{fingerprint}");
    assert!(trust.fields.iter().any(|f| f.kind == FieldKind::Note && f.label.contains(&fingerprint)));
    // accepted, with a wrong password: refused, but the key is remembered
    let e = login(&[("fingerprint", &fingerprint), ("password", "špatně")]).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::PermissionDenied, "{e}");
    assert!(fs::read_to_string(data.join("known_hosts")).unwrap().contains(&format!("[127.0.0.1]:{port}")));
    let Reply::Mount(m) = login(&[]).unwrap() else { panic!("not connected") };
    assert_eq!((m.scheme.as_str(), m.authority.clone()), ("sftp", format!("{USER}@127.0.0.1:{port}")));
    let root = m.root();
    assert!(m.folder.ends_with("home"), "starts in the user's home: {}", m.folder);
    session(&p, &root, &m.folder.clone(), &home, &t.path().join("scratch"), true, true);

    // a key the SCP plugin accepted is trusted here too
    fs::rename(data.join("known_hosts"), t.path().join("plugins").join("scp-known_hosts")).unwrap();
    fs::create_dir_all(t.path().join("plugins").join("scp")).unwrap();
    fs::rename(t.path().join("plugins").join("scp-known_hosts"), t.path().join("plugins").join("scp").join("known_hosts")).unwrap();
    let Reply::Mount(_) = login(&[]).unwrap() else { panic!("the SCP plugin's key isn't trusted") };
    p.close(&root);

    // a server whose key changed is refused
    let other = fs::read_to_string(other_key.with_extension("pub")).unwrap();
    let other: Vec<&str> = other.split_whitespace().collect();
    fs::write(data.join("known_hosts"), format!("[127.0.0.1]:{port} {} {}\n", other[0], other[1])).unwrap();
    let e = login(&[]).unwrap_err();
    assert!(e.kind() == io::ErrorKind::PermissionDenied && e.to_string().starts_with("WARNING"), "{e}");

    // the last connection is remembered, its password never
    let saved = p.last_values();
    assert_eq!((saved["protocol"].as_str(), saved["host"].as_str(), saved["port"].as_str()), ("sftp", "127.0.0.1", port_s.as_str()));
    assert!(!fs::read_to_string(data.join("last-connection")).unwrap().contains(PASSWORD));
    drop(p);
    server_rt.shutdown_background();
}

// ---------------------------------------------------------- FTP / FTPS

trait Rw: Read + Write + Send {}
impl<T: Read + Write + Send> Rw for T {}

/// A small FTP server for the tests: one user (and anonymous), the folder
/// `root` as `/`, starting in `/home`.
#[derive(Clone)]
struct FtpServer {
    root: PathBuf,
    /// Lists with MLSD and sets times with MFMT; otherwise LIST only.
    modern: bool,
    tls: Option<Arc<rustls::ServerConfig>>,
    /// Knows EPSV.
    epsv: bool,
    /// Passive data connections can't get through (as behind a firewall).
    passive_blocked: bool,
    /// The data connection commands the clients sent (EPSV, PASV, PORT, EPRT).
    log: Arc<Mutex<Vec<String>>>,
}

impl FtpServer {
    fn new(root: PathBuf, modern: bool, tls: Option<Arc<rustls::ServerConfig>>) -> FtpServer {
        FtpServer { root, modern, tls, epsv: true, passive_blocked: false, log: Arc::default() }
    }

    /// The data connection commands sent so far, and forget them.
    fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut self.log.lock().unwrap())
    }
}

/// One control connection's state.
struct FtpConn {
    srv: FtpServer,
    tcp: TcpStream,
    ctl: Box<dyn Rw>,
    cwd: String,
    user: Option<String>,
    signed_in: bool,
    private_data: bool,
    passive: Option<TcpListener>,
    /// Where to connect for the next data connection (PORT/EPRT).
    active: Option<std::net::SocketAddr>,
    from: Option<PathBuf>,
}

impl FtpServer {
    /// Start it; its port.
    fn start(self) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let srv = self.clone();
                std::thread::spawn(move || {
                    let ctl = Box::new(s.try_clone().unwrap());
                    let mut c = FtpConn { srv, tcp: s, ctl, cwd: "/home".into(), user: None, signed_in: false, private_data: false, passive: None, active: None, from: None };
                    let _ = c.run();
                });
            }
        });
        port
    }
}

impl FtpConn {
    fn reply(&mut self, text: &str) -> io::Result<()> {
        self.ctl.write_all(format!("{text}\r\n").as_bytes())?;
        self.ctl.flush()
    }

    fn line(&mut self) -> io::Result<Option<String>> {
        let mut out = Vec::new();
        let mut b = [0u8];
        loop {
            match self.ctl.read(&mut b) {
                Ok(0) => return Ok(None),
                Ok(_) if b[0] == b'\n' => return Ok(Some(String::from_utf8_lossy(&out).trim_end_matches('\r').to_owned())),
                Ok(_) => out.push(b[0]),
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(e) => return Err(e),
            }
        }
    }

    /// A path as the client gave it, resolved: (server path, path on disk).
    fn path(&self, arg: &str) -> (String, PathBuf) {
        let full = if arg.starts_with('/') { arg.to_owned() } else { format!("{}/{arg}", self.cwd) };
        let mut parts: Vec<&str> = Vec::new();
        for p in full.split('/') {
            match p {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                p => parts.push(p),
            }
        }
        let disk = parts.iter().fold(self.srv.root.clone(), |d, p| d.join(p));
        (format!("/{}", parts.join("/")), disk)
    }

    /// The data connection the client opens after PASV/EPSV.
    fn data(&mut self) -> io::Result<Data> {
        let s = match self.active.take() {
            Some(addr) => TcpStream::connect_timeout(&addr, Duration::from_secs(5))?,
            None => self.passive.take().ok_or_else(|| io::Error::other("no PASV"))?.accept()?.0,
        };
        s.set_read_timeout(Some(Duration::from_secs(20)))?;
        Ok(match (&self.srv.tls, self.private_data) {
            (Some(tls), true) => Data::Tls(Box::new(rustls::StreamOwned::new(rustls::ServerConnection::new(tls.clone()).unwrap(), s))),
            _ => Data::Plain(s),
        })
    }

    /// Send `bytes` over a data connection and close it.
    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        let mut d = self.data()?;
        self.reply("150 Here it comes")?;
        let sent = d.write_all(bytes).and_then(|_| d.flush());
        d.close();
        self.reply(if sent.is_ok() { "226 Done" } else { "426 Connection closed; transfer aborted" })
    }

    fn listing(&self, disk: &Path) -> io::Result<Vec<u8>> {
        let mut out = String::new();
        for e in fs::read_dir(disk)?.flatten() {
            let md = e.metadata()?;
            let name = e.file_name().to_string_lossy().into_owned();
            let t = ftp_time(md.modified()?).unwrap();
            if self.srv.modern {
                let kind = if md.is_dir() { "dir" } else { "file" };
                out += &format!("type={kind};size={};modify={t};UNIX.mode=0644; {name}\r\n", md.len());
            } else {
                const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
                let (y, m, d) = (&t[0..4], t[4..6].parse::<usize>().unwrap(), &t[6..8]);
                let mode = if md.is_dir() { "drwxr-xr-x" } else { "-rw-r--r--" };
                out += &format!("{mode} 1 ftp ftp {:>8} {} {d} {y:>5} {name}\r\n", md.len(), MONTHS[m - 1]);
            }
        }
        Ok(out.into_bytes())
    }

    fn run(&mut self) -> io::Result<()> {
        self.reply("220 Test server")?;
        while let Some(line) = self.line()? {
            let (cmd, arg) = line.split_once(' ').unwrap_or((&line, ""));
            let cmd = cmd.to_ascii_uppercase();
            let open = ["USER", "PASS", "AUTH", "PBSZ", "PROT", "FEAT", "QUIT"].contains(&cmd.as_str());
            if !open && !self.signed_in {
                self.reply("530 Please sign in")?;
                continue;
            }
            match cmd.as_str() {
                "AUTH" => match self.srv.tls.clone() {
                    Some(tls) => {
                        self.reply("234 Go ahead")?;
                        self.ctl = Box::new(rustls::StreamOwned::new(rustls::ServerConnection::new(tls).unwrap(), self.tcp.try_clone()?));
                    }
                    None => self.reply("502 No TLS here")?,
                },
                "PBSZ" => self.reply("200 PBSZ=0")?,
                "PROT" => {
                    self.private_data = arg == "P";
                    self.reply("200 OK")?;
                }
                "USER" => {
                    self.user = Some(arg.to_owned());
                    self.reply("331 Password?")?;
                }
                "PASS" => {
                    let ok = matches!(self.user.as_deref(), Some("anonymous")) || (self.user.as_deref() == Some(USER) && arg == PASSWORD);
                    self.signed_in = ok;
                    self.reply(if ok { "230 Welcome" } else { "530 Wrong user name or password" })?;
                }
                "FEAT" => {
                    let extra = if self.srv.modern { " MLST type*;size*;modify*;\r\n MFMT\r\n" } else { "" };
                    self.reply(&format!("211-Features:\r\n UTF8\r\n EPSV\r\n{extra}211 End"))?;
                }
                "TYPE" | "OPTS" | "NOOP" => self.reply("200 OK")?,
                "QUIT" => {
                    self.reply("221 Bye")?;
                    break;
                }
                "PWD" => {
                    let r = format!("257 \"{}\" is the current folder", self.cwd);
                    self.reply(&r)?;
                }
                "CWD" => {
                    let (p, disk) = self.path(arg);
                    if disk.is_dir() {
                        self.cwd = p;
                        self.reply("250 OK")?;
                    } else {
                        self.reply("550 No such folder")?;
                    }
                }
                "EPSV" if !self.srv.epsv => self.reply("500 Unknown command")?,
                "PORT" | "EPRT" => {
                    self.srv.log.lock().unwrap().push(cmd.clone());
                    // "127,0,0,1,p1,p2" or "|1|127.0.0.1|port|"
                    let addr = if cmd == "PORT" {
                        let n: Vec<u16> = arg.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                        (n.len() == 6).then(|| std::net::SocketAddr::from(([n[0] as u8, n[1] as u8, n[2] as u8, n[3] as u8], n[4] * 256 + n[5])))
                    } else {
                        let p: Vec<&str> = arg.split('|').collect();
                        p.get(2).zip(p.get(3)).and_then(|(ip, port)| format!("{ip}:{port}").parse().ok())
                    };
                    self.active = addr;
                    self.passive = None;
                    self.reply(if addr.is_some() { "200 OK" } else { "501 Bad address" })?;
                }
                "PASV" | "EPSV" => {
                    self.srv.log.lock().unwrap().push(cmd.clone());
                    let l = TcpListener::bind("127.0.0.1:0")?;
                    let port = l.local_addr()?.port();
                    // blocked: a port nobody answers on (closed before the reply)
                    self.passive = if self.srv.passive_blocked {
                        drop(l);
                        None
                    } else {
                        Some(l)
                    };
                    self.active = None;
                    let r = if cmd == "PASV" { format!("227 Entering Passive Mode (127,0,0,1,{},{})", port >> 8, port & 255) } else { format!("229 Entering Extended Passive Mode (|||{port}|)") };
                    self.reply(&r)?;
                }
                "MLSD" if !self.srv.modern => self.reply("500 Unknown command")?,
                "MLSD" | "LIST" => {
                    let (_, disk) = self.path(if arg.is_empty() || arg.starts_with('-') { "." } else { arg });
                    match self.listing(&disk) {
                        Ok(bytes) => self.send(&bytes)?,
                        Err(_) => self.reply("550 No such folder")?,
                    }
                }
                "RETR" => {
                    let (_, disk) = self.path(arg);
                    match fs::read(&disk) {
                        Ok(bytes) if disk.is_file() => self.send(&bytes)?,
                        _ => self.reply("550 No such file")?,
                    }
                }
                "STOR" => {
                    let (_, disk) = self.path(arg);
                    let mut d = self.data()?;
                    self.reply("150 Send it")?;
                    let mut bytes = Vec::new();
                    let mut buf = [0u8; 64 * 1024];
                    loop {
                        match d.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => bytes.extend_from_slice(&buf[..n]),
                        }
                    }
                    d.close();
                    fs::write(&disk, bytes)?;
                    self.reply("226 Stored")?;
                }
                "DELE" => {
                    let (_, disk) = self.path(arg);
                    let r = if disk.is_file() && fs::remove_file(&disk).is_ok() { "250 Deleted" } else { "550 No such file" };
                    self.reply(r)?;
                }
                "RMD" => {
                    let (_, disk) = self.path(arg);
                    let r = if fs::remove_dir(&disk).is_ok() { "250 Removed" } else { "550 Can't remove the folder" };
                    self.reply(r)?;
                }
                "MKD" => {
                    let (p, disk) = self.path(arg);
                    let r = if !disk.exists() && fs::create_dir(&disk).is_ok() { format!("257 \"{p}\" created") } else { "550 Can't create the folder".into() };
                    self.reply(&r)?;
                }
                "RNFR" => {
                    let (_, disk) = self.path(arg);
                    let ok = disk.exists();
                    self.from = ok.then_some(disk);
                    self.reply(if ok { "350 Where to?" } else { "550 No such file" })?;
                }
                "RNTO" => {
                    let (_, disk) = self.path(arg);
                    let r = match self.from.take() {
                        Some(from) if fs::rename(&from, &disk).is_ok() => "250 Renamed",
                        _ => "550 Can't rename",
                    };
                    self.reply(r)?;
                }
                "MFMT" if self.srv.modern => {
                    let (t, p) = arg.split_once(' ').unwrap_or((arg, ""));
                    let (_, disk) = self.path(p);
                    let set = mlsd_time(t).and_then(|t| fs::File::options().write(true).open(&disk).and_then(|f| f.set_modified(t)).ok());
                    let r = if set.is_some() { format!("213 Modify={t}; {p}") } else { "550 Can't set the time".into() };
                    self.reply(&r)?;
                }
                _ => self.reply("502 Not implemented")?,
            }
        }
        Ok(())
    }
}

/// A data connection, with TLS after `PROT P`.
enum Data {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ServerConnection, TcpStream>>),
}

impl Data {
    /// Close it: TLS says it is the end (close_notify), then TCP closes.
    fn close(self) {
        if let Data::Tls(mut s) = self {
            s.conn.send_close_notify();
            let _ = s.flush();
        }
    }
}

impl Read for Data {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Data::Plain(s) => s.read(buf),
            Data::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Data {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Data::Plain(s) => s.write(buf),
            Data::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Data::Plain(s) => s.flush(),
            Data::Tls(s) => s.flush(),
        }
    }
}

/// A self-signed certificate for "localhost" (so 127.0.0.1 doesn't match
/// it either): what the plugin must ask about.
fn tls_config() -> (Arc<rustls::ServerConfig>, Vec<u8>) {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let cert = ck.cert.der().clone();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(ck.key_pair.serialize_der().into());
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.clone()], key)
        .unwrap();
    (Arc::new(config), cert.to_vec())
}

#[test]
fn a_session_with_an_ftps_server() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("srv").join("home");
    fill(&home);
    let (tls, cert) = tls_config();
    let port = FtpServer::new(t.path().join("srv"), true, Some(tls)).start();
    let data = t.path().join("plugin");
    fs::create_dir_all(&data).unwrap();
    let p = FtpPlugin::start(&data).unwrap();
    let port_s = port.to_string();
    let login = |extra: &[(&str, &str)]| {
        let mut v = values(&[("protocol", "ftps"), ("host", "127.0.0.1"), ("port", &port_s), ("user", USER), ("auth", "password"), ("password", PASSWORD)]);
        v.extend(values(extra));
        p.run("login", &v)
    };

    // a certificate no authority vouches for: the user sees its fingerprint first
    let Reply::Form(trust) = login(&[]).unwrap() else { panic!("expected the trust question") };
    let fingerprint = trust.fields.iter().find(|f| f.id == "fingerprint").unwrap().value.clone();
    assert_eq!(fingerprint, ftp::fingerprint(&cert));
    assert!(trust.fields.iter().any(|f| f.kind == FieldKind::Note && f.label.contains(&fingerprint)));
    let e = login(&[("fingerprint", &fingerprint), ("password", "špatně")]).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::PermissionDenied, "{e}");
    assert!(fs::read_to_string(data.join("known_certificates")).unwrap().contains(&format!("127.0.0.1:{port} {fingerprint}")));
    let Reply::Mount(m) = login(&[]).unwrap() else { panic!("not connected") };
    assert_eq!((m.scheme.as_str(), m.authority.clone(), m.folder.as_str()), ("ftps", format!("{USER}@127.0.0.1:{port}"), "home"));
    session(&p, &m.root(), "home", &home, &t.path().join("scratch"), true, true);

    // a certificate different from the one remembered is refused
    fs::write(data.join("known_certificates"), format!("127.0.0.1:{port} AB:CD\n")).unwrap();
    let e = login(&[]).unwrap_err();
    assert!(e.kind() == io::ErrorKind::PermissionDenied && e.to_string().starts_with("WARNING"), "{e}");
    assert!(login(&[("fingerprint", &fingerprint)]).is_err(), "accepting can't override a changed certificate");
}

#[test]
fn a_session_with_a_plain_ftp_server() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("srv").join("home");
    fill(&home);
    let port = FtpServer::new(t.path().join("srv"), false, None).start();
    let data = t.path().join("plugin");
    fs::create_dir_all(&data).unwrap();
    let p = FtpPlugin::start(&data).unwrap();
    let port_s = port.to_string();

    // FTPS asked of a server without TLS: said so
    let e = p.run("login", &values(&[("protocol", "ftps"), ("host", "127.0.0.1"), ("port", &port_s), ("user", USER), ("password", PASSWORD)])).unwrap_err();
    assert!(e.to_string().contains("TLS"), "{e}");
    // a wrong password
    let e = p.run("login", &values(&[("protocol", "ftp"), ("host", "127.0.0.1"), ("port", &port_s), ("user", USER), ("password", "špatně")])).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::PermissionDenied, "{e}");
    // "user@host:port" typed as the server; a private key picked before (for
    // SFTP) doesn't count: FTP signs in with the password
    let server = format!("{USER}@127.0.0.1:{port}");
    let Reply::Mount(m) = p.run("login", &values(&[("protocol", "ftp"), ("host", &server), ("password", PASSWORD), ("auth", "key"), ("key", "C:\\nic")])).unwrap() else { panic!("not connected") };
    assert_eq!((m.scheme.as_str(), m.authority.as_str()), ("ftp", server.as_str()));
    session(&p, &m.root(), "home", &home, &t.path().join("scratch"), false, false);

    // anonymous: no user name
    let Reply::Mount(m) = p.run("login", &values(&[("protocol", "ftp"), ("host", "127.0.0.1"), ("port", &port_s)])).unwrap() else { panic!("anonymous") };
    assert_eq!(m.authority, format!("127.0.0.1:{port}"));
    assert_eq!(p.list(&m.root(), "home").unwrap().items.len(), 3);
}

/// The ways of opening data connections: found automatically and kept,
/// or as the user chose.
#[test]
fn data_connections_found_automatically() {
    let t = tempfile::tempdir().unwrap();
    fill(&t.path().join("srv").join("home"));
    let data = t.path().join("plugin");
    fs::create_dir_all(&data).unwrap();
    let p = FtpPlugin::start(&data).unwrap();
    let connect = |srv: &FtpServer, mode: &str| {
        let port = srv.clone().start().to_string();
        let v = values(&[("protocol", "ftp"), ("host", "127.0.0.1"), ("port", &port), ("user", USER), ("password", PASSWORD), ("mode", mode)]);
        let Reply::Mount(m) = p.run("login", &v).unwrap() else { panic!("not connected") };
        m.root()
    };

    // a server that knows EPSV: EPSV
    let srv = FtpServer::new(t.path().join("srv"), true, None);
    let root = connect(&srv, "auto");
    assert_eq!(p.list(&root, "home").unwrap().items.len(), 3);
    assert_eq!(srv.take_log(), ["EPSV"]);
    p.close(&root);

    // one that doesn't: PASV, and PASV straight away from then on
    let srv = FtpServer { epsv: false, ..FtpServer::new(t.path().join("srv"), true, None) };
    let root = connect(&srv, "auto");
    assert_eq!(p.list(&root, "home").unwrap().items.len(), 3);
    assert_eq!(p.list(&root, "home").unwrap().items.len(), 3);
    assert_eq!(srv.take_log(), ["PASV", "PASV"], "EPSV is refused before any data connection");
    p.close(&root);

    // passive blocked: active mode, found once and kept, for the other
    // connections to the server too
    let srv = FtpServer { passive_blocked: true, ..FtpServer::new(t.path().join("srv"), true, None) };
    let root = connect(&srv, "auto");
    assert_eq!(p.list(&root, "home").unwrap().items.len(), 3);
    assert_eq!(srv.take_log(), ["EPSV", "PASV", "PORT"]);
    let out = t.path().join("out");
    p.extract(&root, &["home\\velký.bin".to_owned()], &out, &mut |_, _| true).unwrap();
    assert_eq!(fs::read(out.join("home").join("velký.bin")).unwrap().len(), 3_000_000);
    assert!(srv.take_log().iter().all(|c| c == "PORT"));
    p.close(&root);

    // passive only, as asked: no way through, and the user is told to choose another
    let root = connect(&srv, "passive");
    let e = p.list(&root, "home").unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::ConnectionRefused, "{e}");
    assert!(e.to_string().contains("data connection"), "{e}");
    assert_eq!(srv.take_log(), ["EPSV", "PASV"]);
    p.close(&root);

    // active only, as asked, on a server where passive would work
    let srv = FtpServer::new(t.path().join("srv"), true, None);
    let root = connect(&srv, "active");
    assert_eq!(p.list(&root, "home").unwrap().items.len(), 3);
    assert_eq!(srv.take_log(), ["PORT"]);
    p.close(&root);
}

/// Hostile text in the connect form: refused before anything is sent, with
/// a message; a line break never reaches a command to the server.
#[test]
fn hostile_connect_form() {
    let t = tempfile::tempdir().unwrap();
    let data = t.path().join("plugins").join("ftp");
    fs::create_dir_all(&data).unwrap();
    let p = FtpPlugin::start(&data).unwrap();
    let bad_hosts = ["", " ", "a b", "a/b", "a\\b", "me@", "host\r\nQUIT", "host\0", "\u{1b}[31mx", "& calc.exe", "| calc"];
    let long_host = "a".repeat(300);
    for host in bad_hosts.iter().copied().chain([long_host.as_str()]) {
        let e = p.run("login", &values(&[("protocol", "sftp"), ("host", host), ("user", "me"), ("auth", "password"), ("password", "x")])).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "host {host:?}: {e}");
        assert!(!e.to_string().is_empty());
    }
    for user in ["me\r\nDELE x", "me\0", "a/b", "a:b", "a@b", "tab\there"] {
        let e = p.run("login", &values(&[("protocol", "sftp"), ("host", "127.0.0.1"), ("port", "1"), ("user", user), ("auth", "password"), ("password", "x")])).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "user {user:?}: {e}");
    }
    for port in ["0", "-1", "65536", "99999999999999999999", "22a", "0x16", " 1 2", "%PORT%"] {
        let e = p.run("login", &values(&[("protocol", "sftp"), ("host", "127.0.0.1"), ("port", port), ("user", "me"), ("auth", "password"), ("password", "x")])).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "port {port:?}: {e}");
    }
    // a line break in the password would start another FTP command
    for pw in ["x\r\nDELE important", "x\nQUIT", "x\0"] {
        let e = p.run("login", &values(&[("protocol", "ftp"), ("host", "127.0.0.1"), ("port", "1"), ("user", "me"), ("password", pw)])).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "{e}");
        assert!(e.to_string().contains("line breaks"), "{e}");
    }
    // and in a path, wherever it comes from
    for path in ["/a\r\nDELE b", "/a\nb", "/a\0"] {
        assert_eq!(crate::ftp::one_line(path).unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }
    assert!(crate::ftp::one_line("/Příliš & | ; $(x) `y` %z% 'q'").is_ok());
}
