//! The vault's sealed keys (src/sealed_keys.rs; design note « Clé du coffre », 16/11, §4 and §6 point 1), black-box
//! with fakes: a key service answering from memory (as tests/space.rs), a folder standing for the tmpfs
//! (`XDG_RUNTIME_DIR`, accepted with the space's test switch), today's layout written as the bridge writes it.
//!
//! What is checked: once a place is enrolled, no data key, place key or key-encryption key is anywhere under its data
//! folder (byte search, every encoding); the vault reads after a bridge restart without a new request to the key
//! service; after a server restart (the tmpfs wiped) it stays closed until the service answers; a refusing, holding or
//! unreachable service keeps it closed, says so on /health and to the agents in plain words, then it reopens by
//! itself; the migration keeps every entry, rotates the data keys, deletes the clear keys only once everything was read
//! back, and puts everything back on any failure, an interrupted one included. A place that is not enrolled keeps
//! today's layout and behaviour, byte for byte.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use bipbridge::connections::{self, Connection, Stored, Tokens};
use bipbridge::mailbox::Mailboxes;
use bipbridge::sealed_keys::{self, Place, State};
use bipbridge::space::{self, KeyService, ShareError, ShareSource};
use bipbridge::vault::Vault;
use bipbridge::Dict;
use common::{auth, config_dict, TestApp, Urls};
use indexmap::IndexMap;
use serde_json::{json, Value};
use zeroize::Zeroizing;

const B64URL: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

// -- the key service ---------------------------------------------------------------------------------------------------

/// One share, its answers scripted: « down » (unreachable), « no » (stopped), « held » (a rescue boot or the guichet
/// out of reach), « other » (a share that isn't this place's), else the share.
struct Service {
    share: Vec<u8>,
    script: Mutex<Vec<&'static str>>,
    asked: Mutex<u32>,
}

impl Service {
    fn new() -> Arc<Service> {
        Arc::new(Service { share: space::new_share().to_vec(), script: Mutex::new(Vec::new()), asked: Mutex::new(0) })
    }

    fn then(&self, answers: &[&'static str]) {
        self.script.lock().unwrap().extend(answers);
    }

    fn asked(&self) -> u32 {
        *self.asked.lock().unwrap()
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
            "no" => Err(ShareError::Refused("HTTP 403: stopped: abuse".into())),
            "held" => Err(ShareError::Refused("HTTP 403: held: the guichet can't confirm this server's boot".into())),
            "other" => Ok(Zeroizing::new(space::new_share().to_vec())),
            _ => Ok(Zeroizing::new(self.share.clone())),
        }
    }
}

// -- a place as the bridge leaves it today -------------------------------------------------------------------------------

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    place: Place,
    runtime: PathBuf,
}

impl Fixture {
    fn data(&self) -> &Path {
        &self.place.data
    }

    fn vault(&self) -> Vault {
        Vault::new(&self.data().join("vault").to_string_lossy())
    }

    fn mail(&self) -> Mailboxes {
        Mailboxes::new(&self.data().join("mail"))
    }

    fn connections(&self) -> connections::Store {
        connections::Store::new(&self.data().join("connections"))
    }

    fn key_files(&self) -> [PathBuf; 3] {
        [
            self.data().join("vault/vault.key"),
            self.data().join("mail/mail.key"),
            self.data().join("connections/connections.key"),
        ]
    }

    fn data_files(&self) -> [PathBuf; 3] {
        [
            self.data().join("vault/vault.json.enc"),
            self.data().join("mail/accounts.json.enc"),
            self.data().join("connections/connections.json.enc"),
        ]
    }

    /// The bridge's config for this place (its data folder), for /health, the routes and the agents' tools.
    fn config(&self) -> bipbridge::config::Config {
        bipbridge::config::parse_config(&config_dict(&self.root, &Urls::default()), None).unwrap()
    }

    /// A server restart: the tmpfs is empty again and the bridge is a new process.
    fn reboot(&self) {
        let _ = std::fs::remove_dir_all(self.runtime.join("bipagents"));
        sealed_keys::forget_memory(self.data());
    }
}

fn vault_records() -> Value {
    json!([
        {"id": "item_aaaaaaaaaaaa", "kind": "login", "label": "Impôts", "origin": "https://impots.example",
         "identifier_type": "email", "identifier": "alex@example.test", "has_otp": false,
         "secret": {"password": "fake-Pw-0001-impots"}, "agents": {},
         "created_at": "2026-11-01T10:00:00Z", "updated_at": "2026-11-01T10:00:00Z"},
        {"id": "item_bbbbbbbbbbbb", "kind": "address", "label": "Maison", "origin": null,
         "identifier_type": null, "identifier": null, "has_otp": false,
         "secret": {"name": "Alex Test", "address_line1": "1 rue de l'Exemple", "city": "Paris", "postal_code": "75000",
                    "country": "FR"},
         "agents": {}, "created_at": "2026-11-02T10:00:00Z", "updated_at": "2026-11-02T10:00:00Z"}
    ])
}

fn mail_account() -> Dict {
    json!({"id": "mail_cccccccccccc", "provider": "imap", "label": "Perso", "username": "alex@example.test",
           "password": "fake-imap-pw-0002", "host": "imap.example.test", "port": 993, "agents": ["wellness"]})
    .as_object()
    .unwrap()
    .clone()
}

fn stored_connections() -> Stored {
    let mut stored = Stored::default();
    stored.connections.insert(
        "notion".into(),
        Connection {
            service: "notion".into(),
            tokens: Tokens {
                access_token: "fake-access-0003".into(),
                refresh_token: Some("fake-refresh-0004".into()),
                scope: None,
                expires_at: Some(2_000_000_000.0),
            },
            agents: vec!["wellness".into()],
            connected_at: 1_762_000_000.0,
            state: "connected".into(),
            ..Connection::default()
        },
    );
    stored
}

/// A place with today's layout: the vault written as vault.rs writes it (Fernet, its key beside it), the mailboxes and
/// the connections through their own stores (which make their keys). Nothing enrolled yet.
fn fixture() -> Fixture {
    std::env::set_var(space::FAKE_ENV, "1");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let runtime = root.join("run");
    std::fs::create_dir_all(&runtime).unwrap();
    std::env::set_var("XDG_RUNTIME_DIR", &runtime);
    let place = Place::new(root.join("data"), root.join("space"));
    sealed_keys::forget_memory(&place.data);
    let vault = place.data.join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let key = fernet::Fernet::generate_key();
    std::fs::write(vault.join("vault.key"), &key).unwrap();
    let token = fernet::Fernet::new(&key).unwrap().encrypt(&serde_json::to_vec(&vault_records()).unwrap());
    std::fs::write(vault.join("vault.json.enc"), token).unwrap();
    let h = Fixture { _dir: dir, root, place, runtime };
    h.mail().write(&[mail_account()]).unwrap();
    h.connections().write(&stored_connections()).unwrap();
    for key in h.key_files() {
        assert!(key.is_file(), "today's layout: {}", key.display());
    }
    h
}

fn key_service() -> KeyService {
    KeyService { url: "https://keys.test".into(), token: "t".repeat(40), grant: None, signature: None }
}

/// Every file under `dirs` (links not followed), with its bytes.
fn snapshot(dirs: &[&Path]) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                walk(&path, out);
            } else if meta.is_file() {
                out.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    for dir in dirs {
        walk(dir, &mut out);
    }
    out
}

/// What the stores hold, as they read it.
fn contents(h: &Fixture) -> (Vec<Value>, Vec<Dict>, Stored) {
    let items = h.vault().try_items().unwrap().iter().map(|i| serde_json::to_value(i).unwrap()).collect();
    (items, h.mail().read().unwrap(), h.connections().read().unwrap())
}

fn old_keys(h: &Fixture) -> Vec<Zeroizing<String>> {
    h.key_files().iter().map(|p| Zeroizing::new(std::fs::read_to_string(p).unwrap().trim().to_string())).collect()
}

/// The keys of an enrolled place, found again from its files and the service's share without the bridge's code:
/// HKDF-SHA256 over both halves, AES-256-GCM, as the design note says.
struct Unsealed {
    combined: Zeroizing<Vec<u8>>,
    kek: Zeroizing<Vec<u8>>,
    sk: Zeroizing<Vec<u8>>,
    data_keys: Vec<(String, Zeroizing<String>)>,
}

fn open_box(key: &[u8], sealed: &str, aad: &[u8]) -> Vec<u8> {
    let raw = B64URL.decode(sealed).expect("base64url");
    let cipher = Aes256Gcm::new_from_slice(key).unwrap();
    cipher.decrypt(Nonce::from_slice(&raw[..12]), Payload { msg: &raw[12..], aad }).expect("opens")
}

fn unseal_independently(h: &Fixture, service: &Service) -> Unsealed {
    let local = B64URL.decode(std::fs::read_to_string(h.place.space.join("vault.share")).unwrap().trim()).unwrap();
    assert_eq!(local.len(), 32);
    let combined: Zeroizing<Vec<u8>> = Zeroizing::new(local.iter().zip(&service.share).map(|(a, b)| a ^ b).collect());
    let mut kek = Zeroizing::new(vec![0u8; 32]);
    hkdf::Hkdf::<sha2::Sha256>::new(None, &combined).expand(b"bipagents-vault-v1", &mut kek).unwrap();
    let sealed: Value =
        serde_json::from_str(&std::fs::read_to_string(h.data().join("keys.sealed.json")).unwrap()).unwrap();
    assert_eq!(sealed["v"], 1);
    let sk = Zeroizing::new(open_box(&kek, sealed["sk"].as_str().unwrap(), b"bipagents-vault-v1 sk"));
    assert_eq!(sk.len(), 32);
    let mut data_keys = Vec::new();
    for name in ["vault", "mail", "connections"] {
        let aad = format!("bipagents-vault-v1 key {name}");
        let key = open_box(&sk, sealed["keys"][name].as_str().unwrap(), aad.as_bytes());
        data_keys.push((name.to_string(), Zeroizing::new(String::from_utf8(key).unwrap())));
    }
    Unsealed { combined, kek, sk, data_keys }
}

/// A secret's spellings worth searching for: its bytes, hex, base64 and base64url with and without padding.
fn spellings(label: &str, bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE};
    let mut out = vec![(format!("{label} (bytes)"), bytes.to_vec()), (format!("{label} (hex)"), hex::encode(bytes).into_bytes())];
    for (name, engine) in [("b64", STANDARD), ("b64 no pad", STANDARD_NO_PAD), ("b64url", URL_SAFE), ("b64url no pad", B64URL)] {
        out.push((format!("{label} ({name})"), engine.encode(bytes).into_bytes()));
    }
    out
}

/// A Fernet key's spellings: its text, its 32 bytes, and each half (signing, encryption).
fn fernet_spellings(label: &str, key: &str) -> Vec<(String, Vec<u8>)> {
    let raw = base64::engine::general_purpose::URL_SAFE.decode(key.trim()).unwrap();
    let mut out = vec![(format!("{label} (text)"), key.trim().as_bytes().to_vec())];
    out.extend(spellings(label, &raw));
    out.extend(spellings(&format!("{label} signing half"), &raw[..16]));
    out.extend(spellings(&format!("{label} encryption half"), &raw[16..]));
    out
}

fn found_in(files: &BTreeMap<PathBuf, Vec<u8>>, needles: &[(String, Vec<u8>)]) -> Vec<String> {
    let mut found = Vec::new();
    for (path, bytes) in files {
        for (label, needle) in needles {
            if needle.len() >= 16 && bytes.windows(needle.len()).any(|w| w == needle.as_slice()) {
                found.push(format!("{label} in {}", path.display()));
            }
        }
    }
    found
}

// -- the agents' tools and the routes, through the bridge --------------------------------------------------------------

const TOKEN: &str = "agent-token-for-wellness-0123456789";

fn app(h: &Fixture) -> TestApp {
    let config = h.config();
    assert_eq!(Path::new(&config.data_dir()), h.data());
    let mut tokens = IndexMap::new();
    tokens.insert("wellness".to_string(), bipbridge::agent_tools::digest(TOKEN));
    bipbridge::agent_tools::save_tokens(&config, &tokens).unwrap();
    TestApp::new(config)
}

async fn tool_text(app: &TestApp, path: &str, name: &str) -> String {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": {}}});
    let bearer = format!("Bearer {TOKEN}");
    let response =
        app.send_json("POST", path, &[("authorization", &bearer), ("accept", "application/json")], &body).await;
    let answer = response.json();
    answer["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string()
}

/// The vault's tools for the agents, through each of the bridge's MCP servers that reaches the vault, the mailboxes
/// or the connectors: (path, tool).
const VAULT_TOOLS: [(&str, &str); 5] = [
    ("/v1/agent-tools/mcp", "mail_search"),
    ("/v1/agent-tools/vault", "vault_contact_details"),
    ("/v1/agent-tools/vault", "vault_login"),
    ("/v1/agent-tools/pay", "pay_with_card"),
    ("/v1/agent-tools/connectors", "connector_list"),
];

async fn health(app: &TestApp) -> Value {
    app.get("/health", &[]).await.json()
}

// One test at a time: they set XDG_RUNTIME_DIR and the space's test switch for the whole process.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// -- enrolment ---------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn once_enrolled_no_clear_key_is_left_anywhere_under_the_data_folder() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    let before = contents(&h);
    let old = old_keys(&h);
    let service = Service::new();
    let report = sealed_keys::enrol(&h.place, Some(key_service()), &*service).await.unwrap();
    assert_eq!(report["state"], "sealed");
    assert_eq!(report["entries"], json!({"vault": 2, "mail": 1, "connections": 1}));
    // asked twice: once to seal, once more to read everything back from scratch
    assert_eq!(service.asked(), 2);
    for key in h.key_files() {
        assert!(!key.exists(), "{} still there", key.display());
    }
    assert!(!h.data().join(".keys-migration").exists());
    // on the disk: the encrypted files, keys.sealed.json, this server's half and the service's settings (0600)
    use std::os::unix::fs::PermissionsExt;
    for file in [h.data().join("keys.sealed.json"), h.place.space.join("vault.share"), h.place.space.join("keyservice.json")] {
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600, "{}", file.display());
    }
    assert!(std::fs::read_to_string(h.place.space.join("keyservice.json")).unwrap().contains("keys.test"));
    // the same entries, read through the stores
    assert_eq!(contents(&h), before);
    // no key in clear, in any spelling, under the data folder or beside this server's half
    let keys = unseal_independently(&h, &service);
    let mut needles = Vec::new();
    needles.extend(spellings("SK", &keys.sk));
    needles.extend(spellings("KEK", &keys.kek));
    needles.extend(spellings("both halves", &keys.combined));
    needles.extend(spellings("service half", &service.share));
    for (name, key) in &keys.data_keys {
        needles.extend(fernet_spellings(&format!("new {name} key"), key));
    }
    for (i, key) in old.iter().enumerate() {
        needles.extend(fernet_spellings(&format!("old key {i}"), key));
    }
    let on_disk = snapshot(&[h.data(), &h.place.space]);
    assert!(on_disk.len() >= 6, "{:?}", on_disk.keys().collect::<Vec<_>>());
    let found = found_in(&on_disk, &needles);
    assert!(found.is_empty(), "keys found on the disk: {found:#?}");
    // SK only in the tmpfs: its own folder (0700), its file (0600)
    let cache = sealed_keys::cache_path(h.data()).unwrap();
    assert!(cache.starts_with(&h.runtime));
    assert_eq!(std::fs::metadata(&cache).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(cache.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    let in_memory = snapshot(&[&h.runtime]);
    assert!(!found_in(&in_memory, &spellings("SK", &keys.sk)).is_empty(), "SK is kept in the tmpfs");
    assert!(found_in(&in_memory, &needles.iter().filter(|(l, _)| !l.starts_with("SK")).cloned().collect::<Vec<_>>()).is_empty());
    // enrolled once: a second enrolment is refused, nothing changes
    let e = sealed_keys::enrol(&h.place, Some(key_service()), &*service).await.unwrap_err();
    assert!(format!("{e:#}").contains("already sealed"), "{e:#}");
    assert_eq!(snapshot(&[h.data(), &h.place.space]), on_disk);
}

#[tokio::test]
async fn the_migration_keeps_every_entry_and_rotates_the_data_keys() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    let before = contents(&h);
    let old = old_keys(&h);
    let service = Service::new();
    sealed_keys::enrol(&h.place, Some(key_service()), &*service).await.unwrap();
    let after = contents(&h);
    assert_eq!(after.0, before.0, "the vault's items");
    assert_eq!(after.1, before.1, "the mailboxes");
    assert_eq!(after.2, before.2, "the connections");
    // the secrets themselves, entry by entry (the items above hide them)
    let raw = |file: &Path, key: &str| fernet::Fernet::new(key).unwrap().decrypt(std::fs::read_to_string(file).unwrap().trim());
    let keys = unseal_independently(&h, &service);
    let plain = raw(&h.data_files()[0], &keys.data_keys[0].1).unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&plain).unwrap(), vault_records());
    // rotation: new keys, and the old ones open none of the new files
    for (i, (name, new)) in keys.data_keys.iter().enumerate() {
        assert_ne!(new.as_str(), old[i].as_str(), "{name}: the same key");
        assert!(raw(&h.data_files()[i], &old[i]).is_err(), "{name}: the old key still opens it");
        assert!(raw(&h.data_files()[i], new).is_ok(), "{name}");
    }
    // the stores go on with the sealed keys: a write, read back, and no key file made again
    let mut accounts = h.mail().read().unwrap();
    accounts[0].insert("label".into(), json!("Perso (modifié)"));
    h.mail().write(&accounts).unwrap();
    assert_eq!(h.mail().read().unwrap()[0]["label"], "Perso (modifié)");
    let mut stored = h.connections().read().unwrap();
    stored.connections.get_mut("notion").unwrap().tokens.access_token = "fake-access-0005".into();
    h.connections().write(&stored).unwrap();
    assert_eq!(h.connections().read().unwrap().connections["notion"].tokens.access_token, "fake-access-0005");
    for key in h.key_files() {
        assert!(!key.exists(), "{} made again", key.display());
    }
}

#[tokio::test]
async fn a_read_back_that_fails_puts_everything_back_as_it_was() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    std::fs::create_dir_all(&h.place.space).unwrap();
    let before = snapshot(&[h.data(), &h.place.space]);
    let contents_before = contents(&h);
    let service = Service::new();
    // sealed with one share, read back with another (a key service that lost this place's): nothing is lost
    service.then(&["ok", "other"]);
    let e = sealed_keys::enrol(&h.place, Some(key_service()), &*service).await.unwrap_err();
    assert!(format!("{e:#}").contains("everything is as before"), "{e:#}");
    assert_eq!(snapshot(&[h.data(), &h.place.space]), before, "every file as it was, the clear keys included");
    assert_eq!(sealed_keys::state(h.data()), State::Off);
    assert!(snapshot(&[&h.runtime]).is_empty(), "nothing left in the tmpfs");
    assert_eq!(contents(&h), contents_before);
    // the service refusing at once: nothing even started
    service.then(&["no"]);
    let e = sealed_keys::enrol(&h.place, Some(key_service()), &*service).await.unwrap_err();
    assert!(format!("{e:#}").contains("refused"), "{e:#}");
    assert_eq!(snapshot(&[h.data(), &h.place.space]), before);
    // unreachable for the read-back: the same
    service.then(&["ok", "down"]);
    sealed_keys::enrol(&h.place, Some(key_service()), &*service).await.unwrap_err();
    assert_eq!(snapshot(&[h.data(), &h.place.space]), before);
    assert_eq!(sealed_keys::state(h.data()), State::Off);
    // then it works
    sealed_keys::enrol(&h.place, Some(key_service()), &*service).await.unwrap();
    assert_eq!(contents(&h), contents_before);
}

#[tokio::test]
async fn a_store_its_key_cannot_open_is_never_migrated() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    std::fs::write(&h.data_files()[1], "not a fernet token").unwrap();
    let before = snapshot(&[h.data()]);
    let service = Service::new();
    let e = sealed_keys::enrol(&h.place, Some(key_service()), &*service).await.unwrap_err();
    assert!(format!("{e:#}").contains("mail"), "{e:#}");
    assert_eq!(snapshot(&[h.data()]), before);
    assert!(!h.place.space.join("vault.share").exists() && !h.place.space.join("keyservice.json").exists());
    assert_eq!(service.asked(), 0, "nothing asked before the stores are read");
}

#[tokio::test]
async fn an_interrupted_migration_is_put_back_or_finished_at_the_next_start() {
    let _guard = SERIAL.lock().await;
    // stopped (as by a crash) once the new files are written, before they are read back: put back
    let h = fixture();
    std::fs::create_dir_all(&h.place.space).unwrap();
    let before = snapshot(&[h.data(), &h.place.space]);
    let contents_before = contents(&h);
    let service = Service::new();
    sealed_keys::stop_enrolment_at(Some("written"));
    let e = sealed_keys::enrol(&h.place, Some(key_service()), &*service).await;
    sealed_keys::stop_enrolment_at(None);
    assert!(e.is_err());
    assert!(h.data().join(".keys-migration").exists(), "left as a crash leaves it");
    // meanwhile, nothing reads or writes with half a migration
    assert!(matches!(sealed_keys::state(h.data()), State::Closed(_)));
    assert!(h.vault().try_items().is_err());
    assert!(h.mail().write(&[mail_account()]).is_err());
    h.reboot();
    sealed_keys::recover(&h.place).unwrap();
    let mut after = snapshot(&[h.data(), &h.place.space]);
    after.retain(|p, _| !p.ends_with(".keys.lock"));
    assert_eq!(after, before);
    assert_eq!(sealed_keys::state(h.data()), State::Off);
    assert_eq!(contents(&h), contents_before);

    // stopped once everything was read back and committed, before the clear keys went: finished
    let h = fixture();
    let contents_before = contents(&h);
    let service = Service::new();
    sealed_keys::stop_enrolment_at(Some("committed"));
    let e = sealed_keys::enrol(&h.place, Some(key_service()), &*service).await;
    sealed_keys::stop_enrolment_at(None);
    assert!(e.is_err());
    assert!(h.key_files()[0].exists());
    sealed_keys::forget_memory(h.data());
    sealed_keys::recover(&h.place).unwrap();
    for key in h.key_files() {
        assert!(!key.exists(), "{}", key.display());
    }
    assert!(!h.data().join(".keys-migration").exists());
    assert!(sealed_keys::open_from_cache(&h.place));
    assert_eq!(contents(&h), contents_before);
}

// -- restarts ----------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn after_a_bridge_restart_the_vault_reads_without_asking_the_service_again() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    let before = contents(&h);
    let service = Service::new();
    sealed_keys::enrol(&h.place, Some(key_service()), &*service).await.unwrap();
    let asked = service.asked();
    // a new bridge process (an update): the tmpfs still holds the place key
    sealed_keys::forget_memory(h.data());
    sealed_keys::recover(&h.place).unwrap();
    assert!(sealed_keys::open_from_cache(&h.place));
    assert_eq!(sealed_keys::state(h.data()), State::Open);
    assert_eq!(contents(&h), before);
    assert_eq!(service.asked(), asked, "no new request to the key service");
    let app = app(&h);
    assert_eq!(health(&app).await["vault"], "open");
    let (k, v) = auth();
    assert_eq!(app.get("/v1/vault", &[(k, v.as_str())]).await.status, 200);
    app.shutdown().await;
}

#[tokio::test]
async fn after_a_server_restart_the_vault_stays_closed_until_the_service_answers() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    let before = contents(&h);
    let service = Service::new();
    sealed_keys::enrol(&h.place, Some(key_service()), &*service).await.unwrap();
    h.reboot();
    sealed_keys::recover(&h.place).unwrap();
    assert!(!sealed_keys::open_from_cache(&h.place));
    let State::Closed(closed) = sealed_keys::state(h.data()) else { panic!("not closed") };
    assert_eq!(closed.reason, "opening");
    // closed: nothing reads, nothing writes, and no key is made again in its place
    let on_disk = snapshot(&[h.data()]);
    let e = h.vault().try_items().unwrap_err();
    assert!(sealed_keys::is_locked(&e), "{e:#}");
    assert!(sealed_keys::is_locked(&h.mail().read().unwrap_err()));
    assert!(sealed_keys::is_locked(&h.connections().read().unwrap_err()));
    assert!(sealed_keys::is_locked(&h.mail().write(&[]).unwrap_err()));
    assert!(sealed_keys::is_locked(&h.connections().write(&Stored::default()).unwrap_err()));
    let config = h.config();
    let e = h.vault().forget_agent(&config, "wellness").await.unwrap_err();
    assert!(sealed_keys::is_locked(&e), "{e:#}");
    assert_eq!(snapshot(&[h.data()]), on_disk, "nothing written while closed");
    for key in h.key_files() {
        assert!(!key.exists(), "{} made while closed", key.display());
    }
    // the bridge says so, the agents get plain words, the app a code it knows
    let app = app(&h);
    let body = health(&app).await;
    assert_eq!((body["ok"].clone(), body["vault"].clone(), body["vault_reason"].clone()), (json!(true), json!("locked"), json!("opening")));
    for (path, tool) in VAULT_TOOLS {
        let text = tool_text(&app, path, tool).await;
        assert!(sealed_keys::AGENT_TEXTS.contains(&text.as_str()), "{path} {tool}: {text}");
    }
    let (k, v) = auth();
    for route in ["/v1/vault", "/v1/mail/accounts"] {
        let response = app.get(route, &[(k, v.as_str())]).await;
        assert_eq!(response.status, 503, "{route}");
        assert_eq!(response.json()["detail"]["code"], "vault_locked", "{route}");
    }
    // the rest of the place works
    assert_eq!(app.get("/v1/agents", &[(k, v.as_str())]).await.status, 200);
    // the service answers on the third try: open, the tmpfs holds the place key again
    service.then(&["down", "down"]);
    let asked = service.asked();
    let source: Arc<dyn ShareSource> = service.clone();
    tokio::time::timeout(Duration::from_secs(20), sealed_keys::reopen(h.place.clone(), source, Duration::from_millis(5)))
        .await
        .expect("opens");
    assert_eq!(service.asked(), asked + 3);
    assert_eq!(sealed_keys::state(h.data()), State::Open);
    assert!(sealed_keys::cache_path(h.data()).unwrap().is_file());
    assert_eq!(contents(&h), before);
    assert_eq!(health(&app).await["vault"], "open");
    assert_eq!(app.get("/v1/vault", &[(k, v.as_str())]).await.status, 200);
    for (path, tool) in VAULT_TOOLS {
        let text = tool_text(&app, path, tool).await;
        assert!(!sealed_keys::AGENT_TEXTS.contains(&text.as_str()), "{path} {tool} still closed");
    }
    app.shutdown().await;
}

#[tokio::test]
async fn refused_held_or_unreachable_it_stays_closed_says_why_then_reopens() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    let service = Service::new();
    sealed_keys::enrol(&h.place, Some(key_service()), &*service).await.unwrap();
    h.reboot();
    sealed_keys::recover(&h.place).unwrap();
    let app = app(&h);
    for (answer, reason) in [("down", "unreachable"), ("no", "refused"), ("held", "held"), ("other", "wrong_key")] {
        service.then(&[answer]);
        assert!(!sealed_keys::try_open(&h.place, &*service).await, "{answer}");
        let State::Closed(closed) = sealed_keys::state(h.data()) else { panic!("{answer}: not closed") };
        assert_eq!(closed.reason, reason);
        let body = health(&app).await;
        assert_eq!((body["vault"].as_str(), body["vault_reason"].as_str()), (Some("locked"), Some(reason)));
        let text = tool_text(&app, "/v1/agent-tools/vault", "vault_contact_details").await;
        assert!(sealed_keys::AGENT_TEXTS.contains(&text.as_str()), "{answer}: {text}");
        assert!(h.vault().try_items().is_err());
    }
    // nothing of it reached the disk
    assert!(snapshot(&[&h.runtime]).is_empty());
    assert!(sealed_keys::try_open(&h.place, &*service).await);
    assert_eq!(sealed_keys::state(h.data()), State::Open);
    assert_eq!(health(&app).await["vault"], "open");
    app.shutdown().await;
}

#[tokio::test]
async fn the_retries_wait_5_10_20_seconds_and_then_every_5_minutes() {
    assert_eq!(
        (1..=9).map(sealed_keys::wait_after).collect::<Vec<_>>(),
        [5, 10, 20, 40, 80, 160, 300, 300, 300].map(Duration::from_secs)
    );
}

#[tokio::test]
async fn while_closed_the_agents_tool_diet_is_left_as_it_is() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    let service = Service::new();
    sealed_keys::enrol(&h.place, Some(key_service()), &*service).await.unwrap();
    let mut d = config_dict(&h.root, &Urls::default());
    d["hermes"] = json!({"bin": "/opt/hermes", "home": h.root.join(".hermes").to_string_lossy(), "hub_url": "http://127.0.0.1:8642"});
    d["agents"]["Wellness"]["hermes_url"] = json!("http://127.0.0.1:8642/p/wellness");
    let config = bipbridge::config::parse_config(&d, None).unwrap();
    let agent = config.agent(Some("wellness")).unwrap().clone();
    let folder = h.root.join(".hermes/profiles/wellness");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("config.yaml"), "model:\n  default: x\n").unwrap();
    std::fs::write(folder.join("SOUL.md"), "Tu es Wellness.\n").unwrap();
    assert!(bipbridge::tool_diet::current(&config, &agent).is_some(), "open: the diet as usual");
    h.reboot();
    assert!(bipbridge::tool_diet::current(&config, &agent).is_none(), "closed: nothing to change");
    let before = snapshot(&[&folder]);
    bipbridge::profile_files::reconcile_all(&config);
    assert_eq!(snapshot(&[&folder]), before);
}

// -- not enrolled: today, exactly ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_place_that_is_not_enrolled_keeps_todays_layout_and_behaviour() {
    let _guard = SERIAL.lock().await;
    let h = fixture();
    let before = snapshot(&[h.data()]);
    assert_eq!(sealed_keys::state(h.data()), State::Off);
    assert!(sealed_keys::recover(&h.place).is_ok());
    assert!(!sealed_keys::open_from_cache(&h.place));
    let first = contents(&h);
    assert_eq!(snapshot(&[h.data()]), before, "reading changes nothing");
    // the stores make their keys beside their files, as today
    let fresh = h.root.join("fresh");
    Mailboxes::new(&fresh.join("mail")).write(&[mail_account()]).unwrap();
    assert!(fresh.join("mail/mail.key").is_file());
    connections::Store::new(&fresh.join("connections")).write(&stored_connections()).unwrap();
    assert!(fresh.join("connections/connections.key").is_file());
    // the bridge: /health as before (no « vault »), the tools and routes as before
    let app = app(&h);
    let body = health(&app).await;
    assert!(body.get("vault").is_none() && body.get("vault_reason").is_none(), "{body}");
    for (path, tool) in VAULT_TOOLS {
        let text = tool_text(&app, path, tool).await;
        assert!(!sealed_keys::AGENT_TEXTS.contains(&text.as_str()), "{path} {tool}");
    }
    let (k, v) = auth();
    assert_eq!(app.get("/v1/vault", &[(k, v.as_str())]).await.status, 200);
    assert_eq!(app.get("/v1/mail/accounts", &[(k, v.as_str())]).await.status, 200);
    app.shutdown().await;
    // nothing of the sealed keys anywhere: no sealed file, no half, nothing in the tmpfs
    assert!(!h.data().join("keys.sealed.json").exists());
    assert!(!h.place.space.exists());
    assert!(snapshot(&[&h.runtime]).is_empty());
    assert_eq!(contents(&h), first);
    for key in h.key_files() {
        assert!(key.is_file());
    }
}

#[test]
fn the_agents_words_say_it_reopens_and_not_to_ask_for_the_password() {
    for (lang, words) in [("fr", "ne demande pas le mot de passe"), ("en", "Don't ask"), ("de", "Frag"), ("es", "No pidas")] {
        let text = sealed_keys::agent_text(lang);
        assert!(text.contains(words), "{lang}: {text}");
        assert!(sealed_keys::AGENT_TEXTS.contains(&text));
    }
    assert!(sealed_keys::agent_text("fr").contains("se rouvre dans quelques minutes"));
    assert_eq!(sealed_keys::agent_text("it"), sealed_keys::agent_text("en"));
}
