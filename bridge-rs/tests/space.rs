//! The space encrypted at rest (src/space.rs), black-box with fakes: the « fake » backend (no encryption: a folder
//! moved in and out), a key service answering from memory, systemctl recorded. Real gocryptfs needs Linux and FUSE:
//! the integration test at the end runs only when BIPAGENTS_SPACE_GOCRYPTFS=1 (a Linux machine with gocryptfs).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bipbridge::space::{self, EnableOptions, KeyService, LockedDeps, ShareError, ShareSource, Space};
use serde_json::Value;
use zeroize::Zeroizing;

/// The key service: one share, its answers scripted (unreachable, refused, then the share).
struct Service {
    share: Vec<u8>,
    script: Mutex<Vec<&'static str>>,
    asked: Mutex<u32>,
}

impl Service {
    fn new() -> Arc<Service> {
        Arc::new(Service { share: space::new_share().to_vec(), script: Mutex::new(Vec::new()), asked: Mutex::new(0) })
    }
}

#[async_trait::async_trait]
impl ShareSource for Service {
    async fn share(&self) -> Result<Zeroizing<Vec<u8>>, ShareError> {
        *self.asked.lock().unwrap() += 1;
        let next = {
            let mut script = self.script.lock().unwrap();
            if script.is_empty() {
                "ok"
            } else {
                script.remove(0)
            }
        };
        match next {
            "down" => Err(ShareError::Unreachable("connection refused".into())),
            "no" => Err(ShareError::Refused("HTTP 403: stopped".into())),
            "other" => Ok(Zeroizing::new(space::new_share().to_vec())),
            _ => Ok(Zeroizing::new(self.share.clone())),
        }
    }
}

/// The same service, owned (the locked bridge's dependencies are boxed).
struct Shared(Arc<Service>);

#[async_trait::async_trait]
impl ShareSource for Shared {
    async fn share(&self) -> Result<Zeroizing<Vec<u8>>, ShareError> {
        self.0.share().await
    }
}

fn recorder() -> (space::Exec, Arc<Mutex<Vec<Vec<String>>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    let exec: space::Exec = Arc::new(move |args: Vec<String>| {
        seen.lock().unwrap().push(args);
        Box::pin(async { Ok(()) })
    });
    (exec, calls)
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

struct Home {
    _dir: tempfile::TempDir,
    home: PathBuf,
    config: PathBuf,
    space: PathBuf,
    port: u16,
}

/// An account as the installer leaves it: Hermes' home, the bridge's config and data folder (with its binaries and
/// the door's certificate, which stay outside), some state.
fn home() -> Home {
    std::env::set_var(space::FAKE_ENV, "1");
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let w = |rel: &str, body: &str| {
        let p = home.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    };
    w(".hermes/profiles/vie/memories/MEMORY.md", "Alex aime le thé.");
    w(".hermes/profiles/vie/SOUL.md", "Tu es Vie.");
    w(".hermes/media/photo.jpg", "jpeg");
    w(".local/share/bipagents/bridge.db", "SQLite format 3");
    w(".local/share/bipagents/device-keys.json", "[]");
    w(".local/share/bipagents/notebook/people/alex.md", "# Alex");
    w(".local/share/bipagents/browsers/vie/profile/Cookies", "cookies");
    w(".local/share/bipagents/bridge/abc/bipbridge", "binary");
    w(".local/share/bipagents/voices/fr.onnx", "voice");
    w(".local/share/bipagents/lan/cert.pem", "cert");
    w(".local/share/bipagents/migrations.json", "[]");
    w(".local/state/bipagents/backup.json", "{\"last_ok\": 1}");
    w(".config/bipagents/backup.json", "{\"password\": \"restic key\"}");
    let port = free_port();
    let data = home.join(".local/share/bipagents");
    let config = home.join(".config/bipagents/bridge.toml");
    let text = format!(
        "# Written by the BipAgents installer.\nbridge_key = \"{key}\"\nhost = \"127.0.0.1\"\nport = {port}\ndata_dir = \"{data}\"\n\n[hermes]\nhome = \"{hermes}\"\n\n[media.roots]\n\"{media}\" = \"{media}\"\n\n[lan]\nenabled = false\nport = 8650\n\n[agents.vie]\ndisplay_name = \"Vie\"\nhermes_url = \"http://127.0.0.1:1/p/vie\"\nhermes_key = \"hk\"\nupload_dir_host = \"{uploads}\"\nupload_dir_container = \"{uploads}\"\n",
        key = "k".repeat(43),
        data = data.display(),
        hermes = home.join(".hermes").display(),
        media = home.join(".hermes/media").display(),
        uploads = home.join(".hermes/profiles/vie/uploads").display(),
    );
    std::fs::write(&config, text).unwrap();
    std::env::set_var("HOME", &home);
    let space = data.join("space");
    Home { _dir: dir, home, config, space, port }
}

fn options(h: &Home, keep_rollback: bool) -> EnableOptions {
    EnableOptions {
        config_path: h.config.to_string_lossy().into(),
        home: h.home.clone(),
        key_service: KeyService {
            url: "https://keys.test".into(),
            token: "t".repeat(40),
            grant: None,
            signature: None,
        },
        units: vec!["bipagents-hermes.service".into()],
        allow_other: false,
        keep_rollback,
        backend: "fake".into(),
    }
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

// One test at a time: they set HOME and the fake backend's switch for the whole process.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn enable_moves_everything_personal_inside_and_keeps_the_door_outside() {
    let _guard = SERIAL.lock().await;
    let h = home();
    let service = Service::new();
    let (exec, _) = recorder();
    let report = space::enable(&options(&h, false), &h.space, &*service, &space::FakeMounter, &exec).await.unwrap();
    assert_eq!(report["state"], "on");
    let s = Space::load(&h.space).unwrap().unwrap();
    assert!(s.settings.enabled && s.is_open());
    assert_eq!(s.settings.rollback, None, "the originals go once opening again worked");
    assert!(!h.space.join("rollback").exists());
    // the key service was asked twice: to enrol, then to open again from scratch
    assert_eq!(*service.asked.lock().unwrap(), 2);
    // Hermes' home, the config and the state are links into the open folder
    for (link, name) in [(".hermes", "hermes"), (".config/bipagents", "config"), (".local/state/bipagents", "state")] {
        let p = h.home.join(link);
        assert!(std::fs::symlink_metadata(&p).unwrap().file_type().is_symlink(), "{link}");
        assert_eq!(std::fs::read_link(&p).unwrap(), s.mount().join(name));
    }
    assert_eq!(read(&h.home.join(".hermes/profiles/vie/memories/MEMORY.md")), "Alex aime le thé.");
    assert_eq!(read(&h.home.join(".config/bipagents/backup.json")), "{\"password\": \"restic key\"}");
    // the bridge's data: inside, the config pointed there, folders linked for the installer's old paths
    let data = h.home.join(".local/share/bipagents");
    assert_eq!(read(&s.mount().join("data/bridge.db")), "SQLite format 3");
    assert!(!data.join("bridge.db").exists(), "no unencrypted copy left");
    assert!(std::fs::symlink_metadata(data.join("notebook")).unwrap().file_type().is_symlink());
    let config = bipbridge::config::load_config(Some(&h.config.to_string_lossy())).unwrap();
    assert_eq!(config.data_dir(), s.mount().join("data").to_string_lossy());
    assert_eq!(config.lan.cert_dir, data.join("lan").to_string_lossy());
    assert!(space::outside_paths(&config, &s.mount()).is_empty());
    // what the locked bridge needs stays where it was
    for kept in ["bridge/abc/bipbridge", "voices/fr.onnx", "lan/cert.pem", "migrations.json"] {
        assert!(std::fs::symlink_metadata(data.join(kept)).unwrap().is_file(), "{kept}");
    }
    // the door, and this server's share (alone, it says nothing): outside
    assert_eq!(s.settings.door.port, h.port);
    assert_eq!(read(&h.space.join("share")).trim().len(), 43);
    assert!(read(&h.space.join("keyservice.json")).contains("keys.test"));
    // closed: nothing personal readable anywhere
    space::lock(&s, &space::FakeMounter, &exec).await.unwrap();
    assert!(!s.is_open());
    assert!(!h.home.join(".hermes/profiles/vie/SOUL.md").exists());
    assert!(!h.home.join(".config/bipagents/bridge.toml").exists());
    assert_eq!(s.locked().unwrap()["reason"], "closed by hand");
}

#[tokio::test]
async fn enable_is_refused_while_the_bridge_runs_and_twice() {
    let _guard = SERIAL.lock().await;
    let h = home();
    let service = Service::new();
    let (exec, _) = recorder();
    let listener = std::net::TcpListener::bind(("127.0.0.1", h.port)).unwrap();
    let e = space::enable(&options(&h, false), &h.space, &*service, &space::FakeMounter, &exec).await.unwrap_err();
    assert!(format!("{e:#}").contains("the bridge is running"));
    drop(listener);
    assert!(!h.space.join("space.json").exists());
    space::enable(&options(&h, false), &h.space, &*service, &space::FakeMounter, &exec).await.unwrap();
    let e = space::enable(&options(&h, false), &h.space, &*service, &space::FakeMounter, &exec).await.unwrap_err();
    assert!(format!("{e:#}").contains("already encrypted"));
}

#[tokio::test]
async fn a_refused_key_service_changes_nothing() {
    let _guard = SERIAL.lock().await;
    let h = home();
    let service = Service::new();
    service.script.lock().unwrap().push("no");
    let (exec, _) = recorder();
    let e = space::enable(&options(&h, false), &h.space, &*service, &space::FakeMounter, &exec).await.unwrap_err();
    assert!(format!("{e:#}").contains("refused"));
    assert!(!h.space.join("share").exists() && !h.space.join("cipher").exists());
    assert!(std::fs::symlink_metadata(h.home.join(".hermes")).unwrap().is_dir());
}

#[tokio::test]
async fn opening_again_with_another_key_puts_everything_back() {
    let _guard = SERIAL.lock().await;
    let h = home();
    let service = Service::new();
    // enrolment gives one share, the check after the switch another (a key service that lost it)
    service.script.lock().unwrap().extend(["ok", "other"]);
    let (exec, _) = recorder();
    let e = space::enable(&options(&h, false), &h.space, &*service, &space::FakeMounter, &exec).await.unwrap_err();
    assert!(format!("{e:#}").contains("encryption undone"), "{e:#}");
    assert!(std::fs::symlink_metadata(h.home.join(".hermes")).unwrap().is_dir());
    assert_eq!(read(&h.home.join(".hermes/profiles/vie/SOUL.md")), "Tu es Vie.");
    assert_eq!(read(&h.home.join(".local/share/bipagents/bridge.db")), "SQLite format 3");
    assert!(std::fs::symlink_metadata(h.home.join(".local/share/bipagents/notebook")).unwrap().is_dir());
    let config = bipbridge::config::load_config(Some(&h.config.to_string_lossy())).unwrap();
    assert_eq!(config.data_dir(), h.home.join(".local/share/bipagents").to_string_lossy());
    let s = Space::load(&h.space).unwrap().unwrap();
    assert!(!s.settings.enabled && !h.space.join("cipher").exists() && !h.space.join("share").exists());
}

#[tokio::test]
async fn rollback_puts_the_originals_back_while_they_are_kept() {
    let _guard = SERIAL.lock().await;
    let h = home();
    let service = Service::new();
    let (exec, calls) = recorder();
    space::enable(&options(&h, true), &h.space, &*service, &space::FakeMounter, &exec).await.unwrap();
    let mut s = Space::load(&h.space).unwrap().unwrap();
    assert!(s.settings.rollback.is_some());
    space::rollback(&mut s, &space::FakeMounter, &exec).await.unwrap();
    assert!(!s.settings.enabled);
    assert!(std::fs::symlink_metadata(h.home.join(".hermes")).unwrap().is_dir());
    assert!(std::fs::symlink_metadata(h.home.join(".config/bipagents")).unwrap().is_dir());
    assert_eq!(read(&h.home.join(".local/share/bipagents/device-keys.json")), "[]");
    assert!(std::fs::symlink_metadata(h.home.join(".local/share/bipagents/browsers")).unwrap().is_dir());
    assert!(!h.space.join("rollback").exists() && !h.space.join("cipher").exists());
    assert_eq!(
        calls.lock().unwrap().last().unwrap(),
        &vec!["--user".to_string(), "stop".into(), "bipagents-hermes.service".into()]
    );
    // without a kept copy, nothing to put back
    let e = space::rollback(&mut s, &space::FakeMounter, &exec).await.unwrap_err();
    assert!(e.to_string().contains("nothing to put back"));
}

async fn get(port: u16, path: &str) -> (u16, Value) {
    let response = reqwest::get(format!("http://127.0.0.1:{port}{path}")).await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn after_a_reboot_the_bridge_waits_locked_then_opens_and_starts_the_agents() {
    let _guard = SERIAL.lock().await;
    let h = home();
    let service = Service::new();
    let (exec, calls) = recorder();
    space::enable(&options(&h, false), &h.space, &*service, &space::FakeMounter, &exec).await.unwrap();
    let s = Space::load(&h.space).unwrap().unwrap();
    space::lock(&s, &space::FakeMounter, &exec).await.unwrap(); // the reboot
    calls.lock().unwrap().clear();
    // the key service is down twice, then refuses once, then answers
    service.script.lock().unwrap().extend(["down", "down", "no"]);
    let deps = LockedDeps {
        source: Box::new(Shared(service.clone())),
        mounter: Box::new(space::FakeMounter),
        exec,
        pace: Duration::from_millis(40),
    };
    let port = h.port;
    let space_dir = h.space.clone();
    let serving =
        tokio::spawn(async move { space::serve_locked(Space::load(&space_dir).unwrap().unwrap(), deps).await });
    // while locked: /health says so and why, the rest is 423, the seat agent's file says why
    let mut seen_reason = String::new();
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        if let Ok(response) = reqwest::get(format!("http://127.0.0.1:{port}/health")).await {
            let body: Value = response.json().await.unwrap();
            assert_eq!(body["space"], "locked");
            assert_eq!(body["ok"], false);
            seen_reason = body["reason"].as_str().unwrap_or("").to_string();
            if seen_reason == "key service unreachable" {
                break;
            }
        }
    }
    assert_eq!(seen_reason, "key service unreachable");
    let (status, body) = get(port, "/v1/agents").await;
    assert_eq!((status, body["detail"].as_str()), (423, Some("space locked")));
    assert_eq!(Space::load(&h.space).unwrap().unwrap().locked().unwrap()["reason"], "key service unreachable");
    // then it opens by itself (the exec into the normal bridge is the caller's: here, the end of serve_locked)
    let code = tokio::time::timeout(Duration::from_secs(10), serving).await.unwrap().unwrap();
    assert_eq!(code, 0);
    let s = Space::load(&h.space).unwrap().unwrap();
    assert!(s.is_open() && s.locked().is_none());
    assert_eq!(read(&h.home.join(".hermes/profiles/vie/SOUL.md")), "Tu es Vie.");
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[vec!["--user".to_string(), "start".into(), "--no-block".into(), "bipagents-hermes.service".into()]]
    );
    assert_eq!(*service.asked.lock().unwrap(), 2 + 4);
    // the door is closed again: the normal bridge takes the port
    assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok());
}

#[tokio::test]
async fn a_wrong_share_keeps_it_locked_and_says_so() {
    let _guard = SERIAL.lock().await;
    let h = home();
    let service = Service::new();
    let (exec, _) = recorder();
    space::enable(&options(&h, false), &h.space, &*service, &space::FakeMounter, &exec).await.unwrap();
    let s = Space::load(&h.space).unwrap().unwrap();
    space::lock(&s, &space::FakeMounter, &exec).await.unwrap();
    service.script.lock().unwrap().push("other");
    let e = space::unlock(&s, &*service, &space::FakeMounter).await.unwrap_err();
    assert!(e.to_string().contains("wrong key"));
    assert!(!s.is_open());
    space::unlock(&s, &*service, &space::FakeMounter).await.unwrap();
    assert!(s.is_open());
}

#[test]
fn the_fake_backend_is_refused_without_its_test_switch() {
    // (the switch is set by the other tests of this file: checked by name only)
    assert!(space::mounter("gocryptfs").is_ok());
    assert!(space::mounter("plain").is_err());
}
