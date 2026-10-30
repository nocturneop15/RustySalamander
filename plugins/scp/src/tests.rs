//! The plugin against a real SSH server: a small russh server that signs
//! in with a password or a key and runs every command through Git for
//! Windows' bash, which brings real `ls`, `rm`, `mv`, `mkdir`, `du` and
//! `scp`. Without Git for Windows the test is skipped.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::keys::{self, PublicKey};
use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use super::*;

const GIT_BIN: &str = r"C:\Program Files\Git\usr\bin";
const USER: &str = "tester";
const PASSWORD: &str = "tajné heslo";
const PASSPHRASE: &str = "fraze pro klic";

/// A key made by Git's ssh-keygen.
fn keygen(path: &Path, passphrase: &str) {
    let ok = std::process::Command::new(Path::new(GIT_BIN).join("ssh-keygen.exe"))
        .args(["-q", "-t", "ed25519", "-N", passphrase, "-f"])
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
struct TestServer {
    home: PathBuf,
    user_key: PublicKey,
    stdins: Stdins,
}

impl server::Server for TestServer {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        self.clone()
    }
}

impl server::Handler for TestServer {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == USER && password == PASSWORD { Auth::Accept } else { Auth::reject() })
    }

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        Ok(if user == USER && key.key_data() == self.user_key.key_data() { Auth::Accept } else { Auth::reject() })
    }

    async fn channel_open_session(&mut self, _: Channel<Msg>, reply: server::ChannelOpenHandle, _: &mut Session) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).into_owned();
        let mut child = tokio::process::Command::new(Path::new(GIT_BIN).join("bash.exe"))
            .arg("-c")
            .arg(&command)
            .current_dir(&self.home)
            .env("PATH", GIT_BIN)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
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
        let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
        tokio::spawn(async move {
            let h2 = handle.clone();
            let pump_out = async {
                let mut buf = vec![0u8; 32 * 1024];
                while let Ok(n @ 1..) = out.read(&mut buf).await {
                    let _ = handle.data(channel, buf[..n].to_vec()).await;
                }
            };
            let pump_err = async {
                let mut buf = vec![0u8; 4096];
                while let Ok(n @ 1..) = err.read(&mut buf).await {
                    let _ = h2.extended_data(channel, 1, buf[..n].to_vec()).await;
                }
            };
            tokio::join!(pump_out, pump_err);
            let code = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(1) as u32;
            let _ = handle.exit_status_request(channel, code).await;
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

/// Start a server with `host_key` serving `home`; its port.
fn serve(rt: &tokio::runtime::Runtime, host_key: &Path, user_key: PublicKey, home: &Path) -> u16 {
    let config = Arc::new(server::Config {
        keys: vec![keys::load_secret_key(host_key, None).unwrap()],
        auth_rejection_time: Duration::from_millis(10),
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let mut server = TestServer { home: home.to_path_buf(), user_key, stdins: Arc::default() };
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let port = listener.local_addr().unwrap().port();
    rt.spawn(async move {
        let _ = server.run_on_socket(config, &listener).await;
    });
    port
}

fn values(pairs: &[(&str, &str)]) -> Values {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[test]
fn a_session_with_a_real_ssh_server() {
    if !Path::new(GIT_BIN).join("scp.exe").is_file() {
        eprintln!("Git for Windows not found; skipping");
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let (host_key, other_host_key, user_key) = (t.path().join("host"), t.path().join("other"), t.path().join("user"));
    keygen(&host_key, "");
    keygen(&other_host_key, "");
    keygen(&user_key, PASSPHRASE);
    let home = t.path().join("home");
    fs::create_dir_all(home.join("složka 日本")).unwrap();
    fs::write(home.join("složka 日本").join("soubor 🦎.txt"), "ahoj").unwrap();
    fs::write(home.join("velký.bin"), vec![7u8; 300_000]).unwrap();
    fs::write(home.join(".skrytý"), "x").unwrap();

    let server_rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let public = keys::load_secret_key(&user_key, Some(PASSPHRASE)).unwrap().public_key().clone();
    let port = serve(&server_rt, &host_key, public, &home);
    let data = t.path().join("plugin");
    fs::create_dir_all(&data).unwrap();
    let scp = Scp::start(&data).unwrap();
    let port_s = port.to_string();
    let login = |extra: &[(&str, &str)]| {
        let mut v = values(&[("host", "127.0.0.1"), ("port", &port_s), ("user", USER), ("auth", "password"), ("password", PASSWORD)]);
        v.extend(values(extra));
        scp.run("login", &v)
    };

    // the menu command shows the form
    let Reply::Form(form) = scp.run("connect", &Values::new()).unwrap() else { panic!("no form") };
    assert_eq!(form.command, "login");
    assert!(form.fields.iter().any(|f| f.id == "password" && f.kind == FieldKind::Password));

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
    assert_eq!(m.authority, format!("{USER}@127.0.0.1:{port}"));
    let root = m.root();
    let home_inner = m.folder.clone();
    assert!(home_inner.ends_with("home"), "starts in the user's home: {home_inner}");
    assert!(!scp.status().is_empty());

    // listing
    let ix = scp.list(&root, &home_inner).unwrap();
    let mut got: Vec<(String, bool, u64, bool)> =
        ix.items.iter().map(|i| (i.path.rsplit('\\').next().unwrap().to_owned(), i.dir, i.size, i.attrs & sal_plugin_api::ATTR_HIDDEN != 0)).collect();
    got.sort();
    assert_eq!(got, [(".skrytý".into(), false, 1, true), ("složka 日本".into(), true, 0, false), ("velký.bin".into(), false, 300_000, false)]);
    assert!(!ix.complete && ix.items[0].modified.is_some());
    assert_eq!(scp.list(&root, &format!("{home_inner}\\nic")).unwrap_err().kind(), io::ErrorKind::NotFound);

    // copy out: a file and a folder, with progress
    let out = t.path().join("out");
    fs::create_dir_all(&out).unwrap();
    let mut seen = (0, 0);
    let items = [format!("{home_inner}\\velký.bin"), format!("{home_inner}\\složka 日本")];
    scp.extract(&root, &items, &out, &mut |d, total| {
        seen = (d, total);
        true
    })
    .unwrap();
    let local = out.join(&home_inner);
    assert_eq!(fs::read(local.join("velký.bin")).unwrap().len(), 300_000);
    assert_eq!(fs::read_to_string(local.join("složka 日本").join("soubor 🦎.txt")).unwrap(), "ahoj");
    assert_eq!(seen, (300_004, 300_004));
    let cancelled = scp.extract(&root, &items[..1], &t.path().join("out2"), &mut |_, _| false);
    assert_eq!(cancelled.unwrap_err().kind(), io::ErrorKind::Interrupted);

    // copy in: a file and a folder tree
    let src = t.path().join("src");
    fs::create_dir_all(src.join("strom").join("větev")).unwrap();
    fs::write(src.join("strom").join("větev").join("list 🍃.txt"), "zelený").unwrap();
    fs::write(src.join("nový.txt"), "nový obsah").unwrap();
    scp.add(&root, &[(src.join("nový.txt"), format!("{home_inner}\\nový.txt")), (src.join("strom"), format!("{home_inner}\\strom"))], &mut |_, _| true).unwrap();
    assert_eq!(fs::read_to_string(home.join("nový.txt")).unwrap(), "nový obsah");
    assert_eq!(fs::read_to_string(home.join("strom").join("větev").join("list 🍃.txt")).unwrap(), "zelený");

    // rename, new folder, delete; what exists already is refused
    let at = |n: &str| format!("{home_inner}\\{n}");
    scp.rename(&root, &at("nový.txt"), &at("přejmenovaný it's.txt"), &mut |_, _| true).unwrap();
    assert!(home.join("přejmenovaný it's.txt").is_file() && !home.join("nový.txt").exists());
    assert_eq!(scp.rename(&root, &at("strom"), &at("velký.bin"), &mut |_, _| true).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    scp.mkdir(&root, &at("prázdná"), &mut |_, _| true).unwrap();
    assert!(home.join("prázdná").is_dir());
    assert_eq!(scp.mkdir(&root, &at("prázdná"), &mut |_, _| true).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    scp.delete(&root, &[at("strom"), at("prázdná")], &mut |_, _| true).unwrap();
    assert!(!home.join("strom").exists() && !home.join("prázdná").exists());
    assert!(scp.delete(&root, &[String::new()], &mut |_, _| true).is_err(), "never the root");

    // closing; the folder is then unreachable until connecting again
    scp.close(&root);
    assert_eq!(scp.list(&root, &home_inner).unwrap_err().kind(), io::ErrorKind::NotConnected);

    // a private key with a passphrase
    let key = user_key.to_string_lossy().into_owned();
    let e = login(&[("auth", "key"), ("key", &key), ("passphrase", "")]).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "{e}");
    let Reply::Mount(_) = login(&[("auth", "key"), ("key", &key), ("passphrase", PASSPHRASE)]).unwrap() else { panic!("key sign-in") };
    scp.close(&root);

    // a server whose key changed is refused
    let other = fs::read_to_string(other_host_key.with_extension("pub")).unwrap();
    let other: Vec<&str> = other.split_whitespace().collect();
    fs::write(data.join("known_hosts"), format!("[127.0.0.1]:{port} {} {}\n", other[0], other[1])).unwrap();
    let e = login(&[]).unwrap_err();
    assert!(e.kind() == io::ErrorKind::PermissionDenied && e.to_string().starts_with("WARNING"), "{e}");
    assert!(login(&[("fingerprint", &fingerprint)]).is_err(), "accepting can't override a changed key");

    // the last connection is remembered, its password never
    let saved = scp.last_values();
    assert_eq!((saved["host"].as_str(), saved["user"].as_str(), saved["port"].as_str()), ("127.0.0.1", USER, port_s.as_str()));
    assert!(!fs::read_to_string(data.join("last-connection")).unwrap().contains(PASSWORD));
    drop(scp);
    server_rt.shutdown_background();
}
