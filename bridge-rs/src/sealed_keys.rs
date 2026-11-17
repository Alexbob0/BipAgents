//! The vault's keys, sealed (design note « Clé du coffre », 16/11/2026, choices 1 A and 2 A; lot 1): for a place
//! enrolled with `vault_keys`, no key of the vault, the mailboxes or the connections is ever written on the server's
//! disk in clear. Off unless enrolled: everywhere else (self-hosted included) today's layout stays as it is, byte for
//! byte (`vault.key`, `mail.key`, `connections.key` beside their files).
//!
//! The keys:
//!
//!     data keys   the three Fernet keys of vault.json.enc, accounts.json.enc, connections.json.enc (as today)
//!     SK          the place key (32 random bytes): seals each data key, AES-256-GCM
//!     KEK         HKDF-SHA256(this server's half ⊕ the key service's half, info "bipagents-vault-v1"): seals SK
//!
//! On the disk:
//!
//!     <data>/keys.sealed.json                {v, sk, keys: {vault, mail, connections}}: each base64url(nonce ‖
//!                                            ciphertext ‖ tag), its purpose bound as associated data (0600)
//!     <space>/vault.share                    this server's half, base64url (0600); useless alone
//!     <space>/keyservice.json                where the other half is and the place's own token: the same enrolment as
//!                                            the encrypted space's (space.rs; `space` = ~/.local/share/bipagents/space)
//!     $XDG_RUNTIME_DIR/bipagents/vault-<id>.sk   SK, in memory (tmpfs: /run/user/<uid>), 0600 in a 0700 folder
//!
//! The place key stays in the tmpfs: a bridge update asks nothing; a server restart empties it, and the bridge asks
//! the key service once (its own half: 32 bytes, each release checked by the guichet, keys-rs). Until it answers, the
//! vault is closed: the stores refuse (never a key made again in its place), /health says `vault: locked` and why,
//! the agents' vault tools answer in plain words (« le coffre se rouvre dans quelques minutes, ne demande pas le mot
//! de passe »), the app's routes 503 `vault_locked`; tries after 5 s, 10 s, 20 s… then every 5 minutes; the rest of
//! the place works.
//!
//! The enrolment (`bipbridge vault-keys enable`, the bridge stopped; [`enrol`]) is the migration: every store read
//! with its clear key, new data keys made (a key left in an image or a backup opens only what was there then), the
//! new files and keys.sealed.json written with the originals set aside in a journal, everything read back from
//! scratch (this server's half from its file, the key service asked again, each entry compared), and only then the
//! clear keys deleted. Any failure before that puts everything back; a run cut off (a crash) is put back, or
//! finished once past its commit, at the next start ([`recover`]).
//!
//! What it protects against, and what not (the design note says it in full): a copy of the disk, the provider's
//! image or snapshots, a leaked backup, even with the backup key; the rescue mode where the provider's API shows it.
//! Not root on the running server, nor its memory, nor Kerfy Studio (which keeps the other half).

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::config::Config;
use crate::space::{KeyService, ShareError, ShareSource};

pub const SEALED_FILE: &str = "keys.sealed.json";
/// This server's half, in the space's folder (beside keyservice.json).
pub const SHARE_FILE: &str = "vault.share";
pub const KEY_SERVICE_FILE: &str = "keyservice.json";
const JOURNAL_DIR: &str = ".keys-migration";
const JOURNAL: &str = "journal.json";
const INFO: &str = "bipagents-vault-v1";
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;
/// Linux's statfs magic numbers of the memory filesystems the place key may be kept in.
const TMPFS_MAGIC: i64 = 0x0102_1994;
const RAMFS_MAGIC: i64 = 0x8584_58f6;

/// The vault is closed (sealed, its place key not in memory yet, or a sealing under way): nothing is read or written.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("the vault is closed: it reopens by itself in a few minutes")]
pub struct Locked;

/// Whether `err` is (or carries) [`Locked`].
pub fn is_locked(err: &anyhow::Error) -> bool {
    err.chain().any(|e| e.is::<Locked>())
}

/// The three stores whose keys are sealed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataKey {
    Vault,
    Mail,
    Connections,
}

impl DataKey {
    pub const ALL: [DataKey; 3] = [DataKey::Vault, DataKey::Mail, DataKey::Connections];

    /// Its name in keys.sealed.json, which is also its folder under the data folder.
    pub fn name(self) -> &'static str {
        match self {
            DataKey::Vault => "vault",
            DataKey::Mail => "mail",
            DataKey::Connections => "connections",
        }
    }

    /// Today's key file, beside its store's file.
    pub fn key_file(self) -> &'static str {
        match self {
            DataKey::Vault => "vault.key",
            DataKey::Mail => "mail.key",
            DataKey::Connections => "connections.key",
        }
    }

    pub fn data_file(self) -> &'static str {
        match self {
            DataKey::Vault => "vault.json.enc",
            DataKey::Mail => "accounts.json.enc",
            DataKey::Connections => "connections.json.enc",
        }
    }

    fn aad(self) -> Vec<u8> {
        format!("{INFO} key {}", self.name()).into_bytes()
    }
}

fn sk_aad() -> Vec<u8> {
    format!("{INFO} sk").into_bytes()
}

/// Where a place keeps its data (the bridge's data folder) and its key service's enrolment (the space's folder).
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub data: PathBuf,
    pub space: PathBuf,
}

impl Place {
    pub fn new(data: impl Into<PathBuf>, space: impl Into<PathBuf>) -> Place {
        Place { data: data.into(), space: space.into() }
    }

    /// This bridge's: its config's data folder, the space's folder (`$BIPAGENTS_SPACE_DIR`, else
    /// ~/.local/share/bipagents/space).
    pub fn of(config: &Config) -> Place {
        Place::new(config.data_dir(), crate::space::default_dir())
    }

    pub fn sealed(&self) -> PathBuf {
        self.data.join(SEALED_FILE)
    }

    pub fn share(&self) -> PathBuf {
        self.space.join(SHARE_FILE)
    }

    pub fn key_service_file(&self) -> PathBuf {
        self.space.join(KEY_SERVICE_FILE)
    }

    fn journal_dir(&self) -> PathBuf {
        self.data.join(JOURNAL_DIR)
    }
}

/// The settings of this place's key service (keyservice.json).
pub fn key_service(place: &Place) -> Result<KeyService> {
    let text = Zeroizing::new(
        std::fs::read_to_string(place.key_service_file()).context("the key service's settings are missing")?,
    );
    serde_json::from_str(&text).context("the key service's settings are not valid")
}

// -- the state, per data folder, in this process ------------------------------------------------------------------------

/// Why the vault is closed, since when, and how many tries.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Closed {
    /// « opening » (not asked yet), « unreachable », « refused », « held », « wrong_key » (the service's half doesn't
    /// open it), « unreadable » (this server's half or keys.sealed.json), « enrolling » (a sealing under way or cut off).
    pub reason: String,
    pub since: f64,
    pub tries: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// Not enrolled: today's layout.
    Off,
    Open,
    Closed(Closed),
}

#[derive(Default)]
struct Memory {
    /// Boxed: the key stays where it was made while the table grows (no copy left behind), wiped when dropped.
    sk: Option<Box<Zeroizing<[u8; KEY_LEN]>>>,
    closed: Option<Closed>,
}

static PLACES: LazyLock<Mutex<HashMap<PathBuf, Memory>>> = LazyLock::new(Default::default);

fn places() -> std::sync::MutexGuard<'static, HashMap<PathBuf, Memory>> {
    PLACES.lock().unwrap_or_else(|e| e.into_inner())
}

/// The data folder as one name (it may be given with a link or `..`).
fn id(data: &Path) -> PathBuf {
    std::fs::canonicalize(data).unwrap_or_else(|_| data.to_path_buf())
}

fn now() -> f64 {
    crate::py::time_time()
}

fn remember(data: &Path, sk: Box<Zeroizing<[u8; KEY_LEN]>>) {
    let mut places = places();
    let memory = places.entry(id(data)).or_default();
    memory.sk = Some(sk);
    memory.closed = None;
}

fn set_closed(data: &Path, reason: &str, tried: bool) {
    let mut places = places();
    let memory = places.entry(id(data)).or_default();
    let closed = memory.closed.get_or_insert_with(|| Closed { reason: String::new(), since: now(), tries: 0 });
    closed.reason = reason.to_string();
    if tried {
        closed.tries += 1;
    }
}

/// Tests: what this process holds for `data` forgotten (a new bridge process).
#[doc(hidden)]
pub fn forget_memory(data: &Path) {
    places().remove(&id(data));
}

/// The place key: in this process's memory, else from the tmpfs (only if it opens every sealed key), and `f` given it.
fn with_sk<T>(data: &Path, f: impl FnOnce(&[u8; KEY_LEN]) -> T) -> Option<T> {
    {
        let places = places();
        if let Some(sk) = places.get(&id(data)).and_then(|m| m.sk.as_ref()) {
            return Some(f(sk));
        }
    }
    let sk = read_cache(data)?;
    let sealed = Sealed::read(data).ok()?;
    if !sealed.opens_all(&sk) {
        // another sealing's (none is written before its keys are read back): of no use, removed
        tracing::warn!(target: "bipbridge::vault", "vault: the place key in memory doesn't open this place's keys: removed");
        remove_cache(data);
        return None;
    }
    let out = f(&sk);
    remember(data, sk);
    Some(out)
}

/// The vault's state for `data` (the bridge's data folder).
pub fn state(data: &Path) -> State {
    if data.join(JOURNAL_DIR).join(JOURNAL).exists() {
        return State::Closed(Closed { reason: "enrolling".into(), since: now(), tries: 0 });
    }
    if !data.join(SEALED_FILE).exists() {
        return State::Off;
    }
    if with_sk(data, |_| ()).is_some() {
        return State::Open;
    }
    let mut places = places();
    let memory = places.entry(id(data)).or_default();
    let closed = memory.closed.get_or_insert_with(|| Closed { reason: "opening".into(), since: now(), tries: 0 });
    State::Closed(closed.clone())
}

/// The vault of this bridge is sealed and closed now.
pub fn closed(config: &Config) -> bool {
    matches!(state(Path::new(&config.data_dir())), State::Closed(_))
}

/// What /health adds: nothing when not enrolled (as today), `vault: "open"`, or `vault: "locked"` and `vault_reason`.
pub fn health_fields(config: &Config) -> Map<String, Value> {
    let mut out = Map::new();
    match state(Path::new(&config.data_dir())) {
        State::Off => {}
        State::Open => {
            out.insert("vault".into(), json!("open"));
        }
        State::Closed(closed) => {
            out.insert("vault".into(), json!("locked"));
            out.insert("vault_reason".into(), json!(closed.reason));
        }
    }
    out
}

/// The data key of `which`, for its store in `folder` (`<data>/<vault|mail|connections>`): None when the place is not
/// enrolled (the store keeps today's key file), Err([`Locked`]) when it is closed. Never a key file made when sealed.
pub fn data_key(folder: &Path, which: DataKey) -> Result<Option<Zeroizing<String>>> {
    let Some(data) = folder.parent() else { return Ok(None) };
    if data.join(JOURNAL_DIR).join(JOURNAL).exists() {
        return Err(Locked.into());
    }
    let sealed_path = data.join(SEALED_FILE);
    if !sealed_path.exists() {
        return Ok(None);
    }
    let sealed = Sealed::read(data)?;
    match with_sk(data, |sk| sealed.data_key(sk, which)) {
        Some(key) => Ok(Some(key?)),
        None => Err(Locked.into()),
    }
}

// -- the sealed file -----------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Sealed {
    v: u32,
    made: f64,
    kdf: String,
    info: String,
    aead: String,
    sk: String,
    keys: BTreeMap<String, String>,
}

impl Sealed {
    fn read(data: &Path) -> Result<Sealed> {
        let text = std::fs::read_to_string(data.join(SEALED_FILE)).context("keys.sealed.json can't be read")?;
        let sealed: Sealed = serde_json::from_str(&text).context("keys.sealed.json is not valid")?;
        if sealed.v != 1 || sealed.info != INFO {
            bail!("keys.sealed.json: version {} not known", sealed.v);
        }
        Ok(sealed)
    }

    fn sk(&self, kek: &[u8; KEY_LEN]) -> Result<Box<Zeroizing<[u8; KEY_LEN]>>> {
        let plain = open(kek, &self.sk, &sk_aad())?;
        let mut sk = Box::new(Zeroizing::new([0u8; KEY_LEN]));
        if plain.len() != KEY_LEN {
            bail!("the place key has the wrong length");
        }
        sk.copy_from_slice(&plain);
        Ok(sk)
    }

    fn data_key(&self, sk: &[u8; KEY_LEN], which: DataKey) -> Result<Zeroizing<String>> {
        let sealed =
            self.keys.get(which.name()).ok_or_else(|| anyhow!("keys.sealed.json has no {} key", which.name()))?;
        let plain = open(sk, sealed, &which.aad())?;
        let text = std::str::from_utf8(&plain).map_err(|_| anyhow!("the {} key is not text", which.name()))?;
        Ok(Zeroizing::new(text.to_string()))
    }

    fn opens_all(&self, sk: &[u8; KEY_LEN]) -> bool {
        DataKey::ALL.iter().all(|k| self.data_key(sk, *k).is_ok())
    }
}

fn random<const N: usize>() -> Zeroizing<[u8; N]> {
    let mut out = Zeroizing::new([0u8; N]);
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, out.as_mut_slice());
    out
}

/// AES-256-GCM, a random nonce, `aad` bound: base64url(nonce ‖ ciphertext ‖ tag).
fn seal(key: &[u8; KEY_LEN], plain: &[u8], aad: &[u8]) -> Result<String> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| anyhow!("key length"))?;
    let nonce = random::<NONCE_LEN>();
    let sealed = cipher
        .encrypt(Nonce::from_slice(nonce.as_slice()), Payload { msg: plain, aad })
        .map_err(|_| anyhow!("not sealed"))?;
    let mut out = nonce.to_vec();
    out.extend(sealed);
    Ok(B64.encode(out))
}

fn open(key: &[u8; KEY_LEN], sealed: &str, aad: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let raw = B64.decode(sealed.trim()).map_err(|_| anyhow!("a sealed key is not base64url"))?;
    if raw.len() < NONCE_LEN + TAG_LEN {
        bail!("a sealed key is too short");
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| anyhow!("key length"))?;
    let plain = cipher
        .decrypt(Nonce::from_slice(&raw[..NONCE_LEN]), Payload { msg: &raw[NONCE_LEN..], aad })
        .map_err(|_| WrongKey)?;
    Ok(Zeroizing::new(plain))
}

/// A sealed key that the key given doesn't open.
#[derive(Debug, thiserror::Error)]
#[error("the key doesn't open the sealed keys")]
struct WrongKey;

/// KEK = HKDF-SHA256(this server's half ⊕ the key service's half, info "bipagents-vault-v1").
fn kek(local: &[u8], service: &[u8]) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    if local.len() != KEY_LEN || service.len() != KEY_LEN {
        bail!("a half has the wrong length");
    }
    let mut both = Zeroizing::new([0u8; KEY_LEN]);
    for (b, (l, s)) in both.iter_mut().zip(local.iter().zip(service)) {
        *b = l ^ s;
    }
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    hkdf::Hkdf::<sha2::Sha256>::new(None, both.as_slice())
        .expand(INFO.as_bytes(), out.as_mut_slice())
        .map_err(|_| anyhow!("hkdf"))?;
    Ok(out)
}

fn read_share(place: &Place) -> Result<Zeroizing<Vec<u8>>> {
    let text = Zeroizing::new(std::fs::read_to_string(place.share()).context("this server's half is missing")?);
    let share = Zeroizing::new(B64.decode(text.trim()).map_err(|_| anyhow!("this server's half is not base64url"))?);
    if share.len() != KEY_LEN {
        bail!("this server's half has the wrong length");
    }
    Ok(share)
}

// -- files -------------------------------------------------------------------------------------------------------------

/// Written beside, synced, then renamed over: 0600, never half a file.
fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    file.write_all(data)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    sync_dir(path);
    Ok(())
}

fn sync_dir(path: &Path) {
    if let Some(dir) = path.parent() {
        if let Ok(dir) = std::fs::File::open(dir) {
            let _ = dir.sync_all();
        }
    }
}

fn private_dir(dir: &Path) -> Result<()> {
    crate::py::makedirs(&dir.to_string_lossy(), 0o700)?;
    Ok(())
}

/// Removed, its bytes overwritten first (best effort: an SSD or a journaling filesystem may keep a copy).
fn wipe(path: &Path) -> Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.is_file() {
            if let Ok(mut file) = std::fs::OpenOptions::new().write(true).open(path) {
                let _ = file.write_all(&vec![0u8; meta.len() as usize]);
                let _ = file.sync_all();
            }
        }
        std::fs::remove_file(path)?;
        sync_dir(path);
    }
    Ok(())
}

/// The running bridges' hold on their data folder, kept until they stop (shared: an update or a test may hand a folder
/// from one bridge to another).
static BRIDGE_LOCKS: LazyLock<Mutex<HashMap<PathBuf, std::fs::File>>> = LazyLock::new(Default::default);

fn bridge_locks() -> std::sync::MutexGuard<'static, HashMap<PathBuf, std::fs::File>> {
    BRIDGE_LOCKS.lock().unwrap_or_else(|e| e.into_inner())
}

/// `flock(how | LOCK_NB)` on `file`; a lock it holds already is converted (shared to alone, or back).
fn flock(file: &std::fs::File, how: libc::c_int) -> bool {
    use std::os::unix::io::AsRawFd;
    // SAFETY: flock on a descriptor `file` owns; its lock goes with the last descriptor of that open file.
    unsafe { libc::flock(file.as_raw_fd(), how | libc::LOCK_NB) == 0 }
}

/// The data folder held alone (`flock` on the folder itself: no file of its own) by a sealing or a putting back. A
/// running bridge holds it shared ([`hold_for_bridge`]): a sealing is refused then; its own putting back (at its start)
/// converts the bridge's hold for that time.
struct DirLock {
    file: std::fs::File,
    /// The bridge's own hold, converted: shared again when done.
    bridge: bool,
}

impl DirLock {
    fn take(dir: &Path, recovery: bool) -> Result<DirLock> {
        let held = bridge_locks().get(&id(dir)).map(|f| f.try_clone()).transpose()?;
        if let Some(file) = held {
            if !recovery {
                bail!("the bridge is running: stop it first");
            }
            if !flock(&file, libc::LOCK_EX) {
                if !flock(&file, libc::LOCK_SH) {
                    tracing::error!(target: "bipbridge::vault", "vault: the bridge lost its hold on its data folder");
                }
                bail!("another bridge holds this data folder too");
            }
            return Ok(DirLock { file, bridge: true });
        }
        let file = std::fs::File::open(dir).with_context(|| format!("{} can't be opened", dir.display()))?;
        if !flock(&file, libc::LOCK_EX) {
            bail!("the bridge is running (or another vault-keys enable): stop it first");
        }
        Ok(DirLock { file, bridge: false })
    }
}

impl Drop for DirLock {
    fn drop(&mut self) {
        if self.bridge && !flock(&self.file, libc::LOCK_SH) {
            tracing::error!(target: "bipbridge::vault", "vault: the bridge lost its hold on its data folder");
        }
    }
}

/// The bridge starting: its data folder held (shared) until it stops, so that `vault-keys enable` (which takes it
/// alone) is refused while it runs; while an enable runs, it waits `wait`, then is refused (systemd starts it again).
/// Then, for an enrolled place, a sealing cut off put back (or finished) and the place key from the tmpfs (a bridge
/// update asks nothing). Not enrolled: the hold only (no file, nothing else).
pub fn hold_for_bridge(place: &Place, wait: Duration) -> Result<()> {
    let key = id(&place.data);
    if !bridge_locks().contains_key(&key) {
        let Ok(file) = std::fs::File::open(&place.data) else { return Ok(()) }; // no data yet: nothing to hold
        let deadline = std::time::Instant::now() + wait;
        while !flock(&file, libc::LOCK_SH) {
            if std::time::Instant::now() >= deadline {
                bail!("vault-keys enable is sealing this place's keys: the bridge starts once it is done");
            }
            std::thread::sleep(Duration::from_millis(100).min(wait));
        }
        bridge_locks().insert(key, file);
    }
    if !place.journal_dir().exists() && !place.sealed().exists() {
        return Ok(());
    }
    if let Err(e) = recover(place) {
        tracing::error!(target: "bipbridge::vault", error = %format!("{e:#}"), "vault: a sealing cut off not put back");
    }
    if place.sealed().exists() && !open_from_cache(place) {
        tracing::info!(target: "bipbridge::vault", "vault: closed until the key service answers");
    }
    Ok(())
}

// -- the place key in memory (tmpfs) -----------------------------------------------------------------------------------

/// The folder kept in memory by the system: $XDG_RUNTIME_DIR, else /run/user/<uid>; ours (not a link, not writable by
/// others: never /dev/shm) and on a memory filesystem (tmpfs, ramfs).
fn runtime_dir() -> Option<PathBuf> {
    let dir = match std::env::var("XDG_RUNTIME_DIR") {
        Ok(d) if !d.is_empty() => PathBuf::from(d),
        // SAFETY: getuid has no failure mode.
        _ => PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })),
    };
    (ours(&dir) && memory_folder(&dir)).then_some(dir)
}

/// A folder of this account's own: a folder (not a link to one), its owner this account, nobody else may write in it.
fn ours(dir: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::symlink_metadata(dir) else { return false };
    // SAFETY: getuid has no failure mode.
    meta.is_dir() && meta.uid() == unsafe { libc::getuid() } && meta.mode() & 0o022 == 0
}

/// On a memory filesystem; tests (a debug build, with the space's test switch) use a folder on the disk instead.
fn memory_folder(dir: &Path) -> bool {
    let switch = std::env::var(crate::space::FAKE_ENV).map(|v| v == "1").unwrap_or(false);
    memory_folder_accepted(in_memory(dir), switch, cfg!(debug_assertions))
}

/// Whether a folder may keep the place key: in memory, or (tests only) the switch on in a debug build. A release build
/// never keeps it on a disk, whatever its environment says.
fn memory_folder_accepted(in_memory: bool, switch: bool, debug_build: bool) -> bool {
    in_memory || (switch && debug_build)
}

#[cfg(target_os = "linux")]
fn in_memory(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return false };
    // SAFETY: a zeroed statfs is a valid out-parameter; the path is NUL-terminated.
    let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(path.as_ptr(), &mut stats) } != 0 {
        return false;
    }
    #[allow(clippy::unnecessary_cast)]
    let kind = stats.f_type as i64;
    kind == TMPFS_MAGIC || kind == RAMFS_MAGIC
}

#[cfg(not(target_os = "linux"))]
fn in_memory(_dir: &Path) -> bool {
    let _ = (TMPFS_MAGIC, RAMFS_MAGIC);
    false
}

/// Where this place's key is kept in memory (None: no memory folder here).
pub fn cache_path(data: &Path) -> Option<PathBuf> {
    use sha2::Digest;
    let runtime = runtime_dir()?;
    let digest = sha2::Sha256::digest(id(data).to_string_lossy().as_bytes());
    Some(runtime.join("bipagents").join(format!("vault-{}.sk", &hex::encode(digest)[..16])))
}

/// The place key's own folder in memory, as it must be (made if `make`): ours, 0700, on a memory filesystem itself (a
/// link or a mount put there would send the key to a disk).
fn cache_dir(path: &Path, make: bool) -> Result<PathBuf> {
    let dir = path.parent().unwrap_or(Path::new("/")).to_path_buf();
    if make && std::fs::symlink_metadata(&dir).is_err() {
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    }
    if !ours(&dir) || !memory_folder(&dir) {
        bail!("{} is not this account's own folder in memory", dir.display());
    }
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

fn read_cache(data: &Path) -> Option<Box<Zeroizing<[u8; KEY_LEN]>>> {
    let path = cache_path(data)?;
    cache_dir(&path, false).ok()?;
    if !std::fs::symlink_metadata(&path).ok()?.is_file() {
        return None;
    }
    let text = Zeroizing::new(std::fs::read_to_string(path).ok()?);
    let raw = Zeroizing::new(B64.decode(text.trim()).ok()?);
    if raw.len() != KEY_LEN {
        return None;
    }
    let mut sk = Box::new(Zeroizing::new([0u8; KEY_LEN]));
    sk.copy_from_slice(&raw);
    Some(sk)
}

fn write_cache(data: &Path, sk: &[u8; KEY_LEN]) -> Result<()> {
    let path =
        cache_path(data).ok_or_else(|| anyhow!("no folder in memory (XDG_RUNTIME_DIR on tmpfs) for the place key"))?;
    cache_dir(&path, true)?;
    write_private(&path, Zeroizing::new(B64.encode(sk)).as_bytes())
}

fn remove_cache(data: &Path) {
    if let Some(path) = cache_path(data) {
        let _ = std::fs::remove_file(path);
    }
}

// -- opening -----------------------------------------------------------------------------------------------------------

/// At a start: the place key from the tmpfs, if it is there and opens this place's keys. Not enrolled: false.
pub fn open_from_cache(place: &Place) -> bool {
    place.sealed().exists() && !place.journal_dir().join(JOURNAL).exists() && with_sk(&place.data, |_| ()).is_some()
}

/// One try: the key service's half, the place key unsealed, kept in the tmpfs and in memory. False (and why, for
/// /health) when still closed.
pub async fn try_open(place: &Place, source: &dyn ShareSource) -> bool {
    if place.journal_dir().exists() {
        if let Err(e) = recover(place) {
            tracing::warn!(target: "bipbridge::vault", error = %format!("{e:#}"), "vault: a sealing cut off not put back yet");
            set_closed(&place.data, "enrolling", true);
            return false;
        }
    }
    if !place.sealed().exists() {
        return true; // not enrolled (or put back): nothing to open
    }
    if open_from_cache(place) {
        return true;
    }
    let opened: Result<Box<Zeroizing<[u8; KEY_LEN]>>> = async {
        let local = read_share(place)?;
        let sealed = Sealed::read(&place.data)?;
        let service = source.share().await?;
        let kek = kek(&local, &service)?;
        let sk = sealed.sk(&kek)?;
        if !sealed.opens_all(&sk) {
            return Err(WrongKey.into());
        }
        Ok(sk)
    }
    .await;
    match opened {
        Ok(sk) => {
            if let Err(e) = write_cache(&place.data, &sk) {
                // kept in this process only: the next bridge start asks again
                tracing::warn!(target: "bipbridge::vault", error = %format!("{e:#}"), "vault: the place key not kept in memory");
            }
            remember(&place.data, sk);
            tracing::info!(target: "bipbridge::vault", "vault: open");
            true
        }
        Err(e) => {
            let reason = match e.downcast_ref::<ShareError>() {
                Some(ShareError::Unreachable(_)) => "unreachable",
                // keys-rs holds a release while the guichet can't be asked, and gives it as soon as it answers
                Some(ShareError::Refused(why)) if why.contains("can't confirm") => "unconfirmed",
                // keys-rs: « held: … » (the guichet denied or can't be asked), « held after a rescue boot … »
                Some(ShareError::Refused(why)) if why.contains(": held") => "held",
                Some(ShareError::Refused(_)) => "refused",
                None if e.is::<WrongKey>() => "wrong_key",
                None => "unreadable",
            };
            tracing::warn!(target: "bipbridge::vault", reason, error = %format!("{e:#}"), "vault: still closed");
            set_closed(&place.data, reason, true);
            false
        }
    }
}

/// The wait after try `tries` (from 1): 5 s, 10 s, 20 s… then every 5 minutes (as the encrypted space's).
pub fn wait_after(tries: u64) -> Duration {
    let seconds =
        crate::space::BACKOFF.get(tries.saturating_sub(1) as usize).copied().unwrap_or(crate::space::BACKOFF_MAX);
    Duration::from_secs(seconds)
}

/// The wait after try `tries` of a closed vault, by why it is closed. The key service out of reach, or the guichet that
/// can't confirm (keys-rs gives the release once it answers): [`wait_after`]. What doesn't mend by itself waits long:
/// a place stopped or held after a rescue boot (an operator lifts it), a file unreadable, once an hour; a share that
/// doesn't open the place key every 6 hours, since each such try is one of the place's 24 daily releases, shared with
/// the encrypted space. An operator's `bipbridge vault-keys open` tries at once (a waiting bridge sees it).
pub fn wait_for(reason: &str, tries: u64) -> Duration {
    match reason {
        "wrong_key" => Duration::from_secs(6 * 3600),
        "refused" | "held" | "unreadable" => Duration::from_secs(3600),
        _ => wait_after(tries),
    }
}

/// Tries until open, at once then after [`wait_for`] (`pace`: the unit of its seconds, a second but in tests); between
/// tries, every 5 minutes at most, the tmpfs looked at (another process may have opened it).
pub async fn reopen(place: Place, source: Arc<dyn ShareSource>, pace: Duration) {
    let mut tries = 0u64;
    loop {
        tries += 1;
        if try_open(&place, source.as_ref()).await {
            return;
        }
        let reason = match state(&place.data) {
            State::Closed(closed) => closed.reason,
            State::Open | State::Off => return,
        };
        let mut left = wait_for(&reason, tries).as_secs();
        while left > 0 {
            let slice = left.min(crate::space::BACKOFF_MAX);
            tokio::time::sleep(pace * slice as u32).await;
            left -= slice;
            if open_from_cache(&place) {
                return;
            }
        }
    }
}

/// The key service of keyservice.json, read again at each try (a file fixed by hand is taken at the next one).
struct FromFile(Place);

#[async_trait::async_trait]
impl ShareSource for FromFile {
    async fn share(&self) -> Result<Zeroizing<Vec<u8>>, ShareError> {
        let service = key_service(&self.0).map_err(|e| ShareError::Refused(format!("{e:#}")))?;
        let http = crate::space::HttpShare::new(service).map_err(|e| ShareError::Refused(format!("{e:#}")))?;
        http.share().await
    }
}

/// At the bridge's start, before anything reads a store: [`hold_for_bridge`] (up to 2 minutes while an enable runs).
pub fn at_start(config: &Config) -> Result<()> {
    hold_for_bridge(&Place::of(config), Duration::from_secs(120))
}

/// Closed at the start: reopened in the background; then what was left as it was meanwhile (the agents' souls and
/// tool diets, contact details given to the browsers on, removals waiting) brought up to date.
pub fn start(app: &crate::app::AppState) {
    let config = app.config();
    let place = Place::of(&config);
    if !matches!(state(&place.data), State::Closed(_)) {
        return;
    }
    let source: Arc<dyn ShareSource> = Arc::new(FromFile(place.clone()));
    let app = app.clone();
    let reopening = app.clone();
    app.spawn(async move {
        reopen(place, source, Duration::from_secs(1)).await;
        let config = reopening.config();
        if config.hermes.isolated {
            let _ = reopening.vault.retry_removals(&config).await;
            let _ = reopening.vault.follow_browsers(&config).await;
        }
        crate::profile_files::reconcile_all(&config);
    });
}

// -- the enrolment: the migration --------------------------------------------------------------------------------------

/// What a sealing sets aside and makes, to put it back. Written first (nothing changed yet), then once every original
/// is set aside, synced and checked (`complete`): only a complete journal is ever put back; an incomplete one means
/// nothing was changed, and its copies are thrown away.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Journal {
    started: f64,
    space: PathBuf,
    complete: bool,
    /// The stores whose file existed, with that file's SHA-256 (its copy: `<name>.enc` in the journal's folder).
    originals: BTreeMap<String, String>,
    /// vault.share as it was: its SHA-256 (its copy: `share.old`); None: made by this sealing.
    share_before: Option<String>,
    made_key_service: bool,
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// A file's SHA-256, None when it can't be read.
fn file_sha256(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|bytes| sha256_hex(&bytes))
}

/// The originals set aside: the journal first (incomplete), each copy written and synced, read back and checked, then
/// the journal complete. Nothing of the place is changed before this returns.
fn set_aside(place: &Place, old: &[Old], made_key_service: bool) -> Result<()> {
    let journal_dir = place.journal_dir();
    private_dir(&journal_dir)?;
    let mut originals = BTreeMap::new();
    let mut contents = Vec::new();
    for store in old.iter().filter(|o| o.plain.is_some()) {
        let bytes = Zeroizing::new(std::fs::read(&store.data_path)?);
        originals.insert(store.which.name().to_string(), sha256_hex(&bytes));
        contents.push((format!("{}.enc", store.which.name()), bytes));
    }
    let share_before = match std::fs::read(place.share()).map(Zeroizing::new) {
        Ok(bytes) => {
            let sum = sha256_hex(&bytes);
            contents.push(("share.old".into(), bytes));
            Some(sum)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let mut journal = Journal {
        started: now(),
        space: place.space.clone(),
        complete: false,
        originals,
        share_before,
        made_key_service,
    };
    write_private(&journal_dir.join(JOURNAL), serde_json::to_string_pretty(&journal)?.as_bytes())?;
    for (name, bytes) in &contents {
        write_private(&journal_dir.join(name), bytes)?;
        if file_sha256(&journal_dir.join(name)).as_deref() != Some(sha256_hex(bytes).as_str()) {
            bail!("the copy of {name} set aside differs");
        }
    }
    if stops_at("setting aside") {
        return Err(Stopped.into()); // as a crash would: nothing tidied
    }
    journal.complete = true;
    write_private(&journal_dir.join(JOURNAL), serde_json::to_string_pretty(&journal)?.as_bytes())
}

/// Tests: the next sealing stops (as a crash would, nothing tidied) once at `step`: « setting aside » (the copies
/// written, the journal not complete), « written » (the new files there, not read back yet) or « committed »
/// (everything read back, the clear keys not deleted yet).
#[doc(hidden)]
pub fn stop_enrolment_at(step: Option<&'static str>) {
    *STOP_AT.lock().unwrap_or_else(|e| e.into_inner()) = step;
}

static STOP_AT: Mutex<Option<&'static str>> = Mutex::new(None);

/// A sealing stopped by [`stop_enrolment_at`] (tests).
#[derive(Debug, thiserror::Error)]
#[error("stopped (tests)")]
struct Stopped;

fn stops_at(step: &str) -> bool {
    *STOP_AT.lock().unwrap_or_else(|e| e.into_inner()) == Some(step)
}

/// A store as it is today: its clear key, its file's plain text.
struct Old {
    which: DataKey,
    data_path: PathBuf,
    plain: Option<Zeroizing<Vec<u8>>>,
    entries: usize,
}

fn entries_of(which: DataKey, plain: &[u8]) -> Result<(usize, Value)> {
    let value: Value =
        serde_json::from_slice(plain).with_context(|| format!("the {} store is not JSON", which.name()))?;
    let count = match which {
        DataKey::Vault | DataKey::Mail => value.as_array().map(Vec::len),
        DataKey::Connections => {
            serde_json::from_value::<crate::connections::Stored>(value.clone()).ok().map(|s| s.connections.len())
        }
    };
    let count = count.ok_or_else(|| anyhow!("the {} store doesn't hold what it should", which.name()))?;
    Ok((count, value))
}

fn read_old(place: &Place) -> Result<Vec<Old>> {
    let mut out = Vec::new();
    for which in DataKey::ALL {
        let folder = place.data.join(which.name());
        let key_path = folder.join(which.key_file());
        let data_path = folder.join(which.data_file());
        let plain = if data_path.exists() {
            let key = Zeroizing::new(
                std::fs::read_to_string(&key_path).with_context(|| format!("the {} store has no key", which.name()))?,
            );
            let fernet = fernet::Fernet::new(key.trim())
                .ok_or_else(|| anyhow!("the {} store's key is not a Fernet key", which.name()))?;
            let token = std::fs::read_to_string(&data_path)?;
            let plain = fernet
                .decrypt(token.trim())
                .map_err(|_| anyhow!("the {} store can't be read with its key", which.name()))?;
            Some(Zeroizing::new(plain))
        } else {
            None
        };
        let entries = match &plain {
            Some(p) => entries_of(which, p)?.0,
            None => 0,
        };
        out.push(Old { which, data_path, plain, entries });
    }
    Ok(out)
}

/// The place enrolled: its vault's, mailboxes' and connections' keys sealed, as the module says. `key_service`: the
/// settings to keep when this place has none yet (an encrypted space has them already: used as they are). The bridge
/// must be stopped (`bipbridge vault-keys enable` checks it). Its report: {state, entries, removed}.
pub async fn enrol(place: &Place, key_service_given: Option<KeyService>, source: &dyn ShareSource) -> Result<Value> {
    if !place.data.is_dir() {
        bail!("no data folder at {}: nothing changed", place.data.display());
    }
    if let Ok(Some(space)) = crate::space::Space::load(&place.space) {
        if space.settings.enabled && !space.is_open() {
            bail!("the encrypted space is closed: open it first (nothing changed)");
        }
    }
    let _lock = DirLock::take(&place.data, false)?;
    recover_locked(place)?;
    if place.sealed().exists() {
        bail!("this place's vault keys are already sealed");
    }
    if cache_path(&place.data).is_none() {
        bail!("no folder in memory (XDG_RUNTIME_DIR on tmpfs) to keep the place key in: nothing changed");
    }
    let has_key_service = place.key_service_file().exists();
    if has_key_service {
        // the one the bridge will read at each start: it must be readable now
        key_service(place).context("keyservice.json can't be read: nothing changed")?;
    } else if key_service_given.is_none() {
        bail!("no key service settings ({KEY_SERVICE_FILE} or BIPAGENTS_KEYS_URL and BIPAGENTS_KEYS_TOKEN): nothing changed");
    }
    // 1. Every store read with its clear key: nothing changed yet.
    let old = read_old(place).context("nothing changed")?;
    // 2. The key service's half, then the new keys, sealed.
    let service = source.share().await.map_err(|e| anyhow!("{e}"))?;
    let local = random::<KEY_LEN>();
    let sk = Box::new(random::<KEY_LEN>());
    let new_keys: Vec<Zeroizing<String>> =
        DataKey::ALL.iter().map(|_| Zeroizing::new(fernet::Fernet::generate_key())).collect();
    let mut sealed = Sealed {
        v: 1,
        made: now(),
        kdf: "hkdf-sha256".into(),
        info: INFO.into(),
        aead: "aes-256-gcm".into(),
        sk: String::new(),
        keys: BTreeMap::new(),
    };
    {
        let kek = kek(local.as_slice(), &service)?;
        sealed.sk = seal(&kek, sk.as_slice(), &sk_aad())?;
    }
    drop(service);
    for (which, key) in DataKey::ALL.iter().zip(&new_keys) {
        sealed.keys.insert(which.name().into(), seal(&sk, key.as_bytes(), &which.aad())?);
    }
    // 3. The originals set aside (the journal first, then the copies, synced and checked, then the journal complete).
    if let Err(e) = set_aside(place, &old, !has_key_service) {
        if e.is::<Stopped>() {
            return Err(e);
        }
        return Err(undone(e, put_back(place)));
    }
    // 4. The new files: each store's text encrypted with its new key, this server's half, the settings, the keys.
    let written: Result<()> = async {
        for (store, key) in old.iter().zip(&new_keys) {
            if let Some(plain) = &store.plain {
                let fernet = fernet::Fernet::new(key).ok_or_else(|| anyhow!("a new key is not a Fernet key"))?;
                write_private(&store.data_path, fernet.encrypt(plain).as_bytes())?;
            }
        }
        private_dir(&place.space)?;
        write_private(&place.share(), Zeroizing::new(B64.encode(local.as_slice())).as_bytes())?;
        if let (false, Some(given)) = (has_key_service, &key_service_given) {
            write_private(&place.key_service_file(), Zeroizing::new(serde_json::to_string(given)?).as_bytes())?;
        }
        write_private(&place.sealed(), serde_json::to_string_pretty(&sealed)?.as_bytes())?;
        Ok(())
    }
    .await;
    if let Err(e) = written {
        return Err(undone(e, put_back(place)));
    }
    if stops_at("written") {
        bail!("stopped once written (tests)");
    }
    // 5. Everything read back from scratch: this server's half from its file, the key service asked again, the sealed
    //    keys from the disk, each store's file compared with what it held, entry by entry.
    let checked: Result<()> = async {
        let local = read_share(place)?;
        let service = source.share().await.map_err(|e| anyhow!("{e}"))?;
        let kek = kek(&local, &service)?;
        drop(service);
        let again = Sealed::read(&place.data)?;
        let sk_again =
            again.sk(&kek).map_err(|_| anyhow!("the key service's half read back doesn't open the place key"))?;
        if !bool::from(sk_again.as_slice().ct_eq(sk.as_slice())) {
            bail!("the place key read back differs");
        }
        for (store, key) in old.iter().zip(&new_keys) {
            let key_again = again.data_key(&sk_again, store.which)?;
            if !bool::from(key_again.as_bytes().ct_eq(key.as_bytes())) {
                bail!("the {} key read back differs", store.which.name());
            }
            let Some(plain) = &store.plain else {
                if store.data_path.exists() {
                    bail!("a {} file appeared meanwhile", store.which.name());
                }
                continue;
            };
            let token = std::fs::read_to_string(&store.data_path)?;
            let fernet =
                fernet::Fernet::new(&key_again).ok_or_else(|| anyhow!("a key read back is not a Fernet key"))?;
            let read = Zeroizing::new(
                fernet
                    .decrypt(token.trim())
                    .map_err(|_| anyhow!("the {} store can't be read back", store.which.name()))?,
            );
            let (count, entries) = entries_of(store.which, &read)?;
            let (count_before, entries_before) = entries_of(store.which, plain)?;
            if count != count_before
                || entries != entries_before
                || !bool::from(read.as_slice().ct_eq(plain.as_slice()))
            {
                bail!("the {} store read back differs", store.which.name());
            }
        }
        Ok(())
    }
    .await;
    if let Err(e) = checked {
        return Err(undone(e, put_back(place)));
    }
    // 6. The place key in memory, then the commit (the journal's file removed), then the clear keys deleted.
    if let Err(e) = write_cache(&place.data, &sk) {
        return Err(undone(e, put_back(place)));
    }
    remember(&place.data, sk);
    let journal_file = place.journal_dir().join(JOURNAL);
    if let Err(e) = std::fs::remove_file(&journal_file) {
        return Err(undone(e.into(), put_back(place)));
    }
    sync_dir(&journal_file);
    if stops_at("committed") {
        bail!("stopped once committed (tests)");
    }
    let removed = finish(place).context(
        "the vault's keys are sealed, but the clear keys could not all be deleted: the next start deletes them",
    )?;
    let entries: Map<String, Value> = old.iter().map(|o| (o.which.name().to_string(), json!(o.entries))).collect();
    let counts = Value::Object(entries.clone());
    tracing::info!(target: "bipbridge::vault", entries = %counts, "vault: keys sealed, the clear keys deleted");
    Ok(json!({"state": "sealed", "entries": entries, "removed": removed}))
}

/// A sealing's failure, said with what became of its putting back.
fn undone(e: anyhow::Error, put_back: Result<()>) -> anyhow::Error {
    match put_back {
        Ok(()) => e.context("the vault's keys were not sealed: everything is as before"),
        Err(why) => e.context(format!(
            "the vault's keys were not sealed, and putting everything back failed ({why:#}): the vault stays closed, the \
             next start puts it back"
        )),
    }
}

/// After the commit: the clear keys and the originals (encrypted with them) deleted. The deleted keys, by name.
fn finish(place: &Place) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    for which in DataKey::ALL {
        let path = place.data.join(which.name()).join(which.key_file());
        if std::fs::symlink_metadata(&path).is_ok() {
            wipe(&path)?;
            removed.push(format!("{}/{}", which.name(), which.key_file()));
        }
    }
    let journal_dir = place.journal_dir();
    if journal_dir.exists() {
        for entry in std::fs::read_dir(&journal_dir)?.flatten() {
            wipe(&entry.path())?;
        }
        std::fs::remove_dir(&journal_dir)?;
        sync_dir(&journal_dir);
    }
    Ok(removed)
}

/// Before the commit: everything as before the sealing, from a complete journal only, each original checked against
/// its SHA-256 before it is put back (a copy that doesn't match is never put over anything: the journal stays, the
/// vault closed). An incomplete journal (or none): nothing was changed yet, its copies are thrown away.
fn put_back(place: &Place) -> Result<()> {
    let journal_dir = place.journal_dir();
    let journal = std::fs::read_to_string(journal_dir.join(JOURNAL))
        .ok()
        .and_then(|text| serde_json::from_str::<Journal>(&text).ok())
        .filter(|j| j.complete);
    let Some(journal) = journal else {
        discard(place)?;
        tracing::warn!(target: "bipbridge::vault", "vault: a sealing stopped before it changed anything: its copies thrown away");
        return Ok(());
    };
    // 1. What goes where, checked before anything moves.
    let mut renames = Vec::new();
    let mut removals = Vec::new();
    for which in DataKey::ALL {
        let path = place.data.join(which.name()).join(which.data_file());
        match journal.originals.get(which.name()) {
            Some(sum) if file_sha256(&path).as_ref() == Some(sum) => {} // untouched, or put back already
            Some(sum) => {
                let copy = journal_dir.join(format!("{}.enc", which.name()));
                if file_sha256(&copy).as_ref() != Some(sum) {
                    bail!("the {} store's original set aside doesn't match: nothing put back", which.name());
                }
                renames.push((copy, path));
            }
            None => removals.push(path), // made by this sealing
        }
    }
    let space = Place::new(&place.data, &journal.space);
    match &journal.share_before {
        Some(sum) if file_sha256(&space.share()).as_ref() == Some(sum) => {}
        Some(sum) => {
            let copy = journal_dir.join("share.old");
            if file_sha256(&copy).as_ref() != Some(sum) {
                bail!("this server's half set aside doesn't match: nothing put back");
            }
            renames.push((copy, space.share()));
        }
        None => removals.push(space.share()),
    }
    // 2. The mode first (no sealed file: the clear keys, never touched, are used again), then the files.
    let _ = std::fs::remove_file(place.sealed());
    sync_dir(&place.sealed());
    for (copy, path) in renames {
        std::fs::rename(&copy, &path)?;
        sync_dir(&path);
    }
    for path in removals {
        wipe(&path)?;
    }
    if journal.made_key_service {
        let _ = std::fs::remove_file(space.key_service_file());
    }
    remove_cache(&place.data);
    forget_memory(&place.data);
    discard(place)?;
    tracing::warn!(target: "bipbridge::vault", "vault: sealing undone, the keys as before");
    Ok(())
}

/// The journal's folder removed, its file last.
fn discard(place: &Place) -> Result<()> {
    let journal_dir = place.journal_dir();
    if !journal_dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&journal_dir)?.flatten() {
        if entry.file_name() != JOURNAL {
            wipe(&entry.path())?;
        }
    }
    wipe(&journal_dir.join(JOURNAL))?;
    std::fs::remove_dir(&journal_dir)?;
    sync_dir(&journal_dir);
    Ok(())
}

/// A sealing cut off (a crash, the server stopped): put back before its commit, finished after it. Nothing to do
/// otherwise. Run at the bridge's start and before a sealing.
pub fn recover(place: &Place) -> Result<()> {
    if !place.journal_dir().exists() {
        return Ok(());
    }
    let _lock = DirLock::take(&place.data, true)?;
    recover_locked(place)
}

fn recover_locked(place: &Place) -> Result<()> {
    let journal_dir = place.journal_dir();
    if !journal_dir.exists() {
        return Ok(());
    }
    if journal_dir.join(JOURNAL).exists() {
        tracing::warn!(target: "bipbridge::vault", "vault: a sealing was cut off before its end: put back");
        return put_back(place);
    }
    if place.sealed().exists() {
        tracing::warn!(target: "bipbridge::vault", "vault: a sealing was cut off after its commit: finished");
        return finish(place).map(|_| ());
    }
    // no journal's file and nothing sealed: a putting back that ended, or a sealing stopped before its journal
    discard(place)
}

// -- the agents' and the app's words ------------------------------------------------------------------------------------

/// What an agent is told when it reaches the vault (or the mailboxes, the connectors, the cards) while it is closed:
/// « in a few minutes » only while the key service is out of reach and asked again ([`SOON_TEXTS`]); held, stopped, a
/// wrong share or anything else that waits for someone: no delay promised ([`CHECK_TEXTS`]). Never « give me the
/// password ».
pub const SOON_TEXTS: [&str; 4] = [SOON_FR, SOON_EN, SOON_DE, SOON_ES];
pub const CHECK_TEXTS: [&str; 4] = [CHECK_FR, CHECK_EN, CHECK_DE, CHECK_ES];
pub const AGENT_TEXTS: [&str; 8] = [SOON_FR, SOON_EN, SOON_DE, SOON_ES, CHECK_FR, CHECK_EN, CHECK_DE, CHECK_ES];

const SOON_FR: &str =
    "Le coffre-fort est fermé pour le moment : il se rouvre tout seul dans quelques minutes (le serveur \
a redémarré). Surtout, ne demande pas le mot de passe à ta personne : dis-lui simplement que le coffre se rouvre dans \
quelques minutes, et réessaie plus tard.";
const SOON_EN: &str =
    "The vault is closed for now: it reopens by itself in a few minutes (the server restarted). Don't \
ask your person for the password: just tell them the vault reopens in a few minutes, and try again later.";
const SOON_DE: &str = "Der Tresor ist gerade geschlossen: Er öffnet sich in ein paar Minuten von selbst wieder (der \
Server wurde neu gestartet). Frag deine Person nicht nach dem Passwort: Sag ihr einfach, dass der Tresor in ein paar \
Minuten wieder offen ist, und versuch es später noch einmal.";
const SOON_ES: &str = "La caja fuerte está cerrada por ahora: se vuelve a abrir sola en unos minutos (el servidor se \
reinició). No pidas la contraseña a tu persona: dile simplemente que la caja fuerte se vuelve a abrir en unos minutos, \
y vuelve a intentarlo más tarde.";
const CHECK_FR: &str =
    "Le coffre-fort est fermé pour le moment, le temps d'une vérification. Surtout, ne demande pas le \
mot de passe à ta personne : dis-lui simplement que le coffre est fermé et qu'il se rouvrira une fois cette \
vérification faite.";
const CHECK_EN: &str =
    "The vault is closed for now, pending a check. Don't ask your person for the password: just tell \
them the vault is closed and will reopen once that check is done.";
const CHECK_DE: &str =
    "Der Tresor ist gerade geschlossen, bis eine Prüfung erledigt ist. Frag deine Person nicht nach \
dem Passwort: Sag ihr einfach, dass der Tresor geschlossen ist und nach dieser Prüfung wieder öffnet.";
const CHECK_ES: &str =
    "La caja fuerte está cerrada por ahora, a la espera de una comprobación. No pidas la contraseña a \
tu persona: dile simplemente que la caja fuerte está cerrada y que se volverá a abrir tras esa comprobación.";

/// The words for an agent speaking `lang`, the vault closed for `reason` ([`Closed::reason`]).
pub fn agent_text(lang: &str, reason: &str) -> &'static str {
    let soon = matches!(reason, "opening" | "unreachable");
    match (lang, soon) {
        ("fr", true) => SOON_FR,
        ("de", true) => SOON_DE,
        ("es", true) => SOON_ES,
        (_, true) => SOON_EN,
        ("fr", false) => CHECK_FR,
        ("de", false) => CHECK_DE,
        ("es", false) => CHECK_ES,
        (_, false) => CHECK_EN,
    }
}

/// [`agent_text`] for this bridge's vault as it is now.
pub fn closed_text(config: &Config, lang: &str) -> &'static str {
    match state(Path::new(&config.data_dir())) {
        State::Closed(closed) => agent_text(lang, &closed.reason),
        _ => agent_text(lang, "opening"),
    }
}

/// [`closed_text`] as an MCP tool result.
pub fn closed_result(config: &Config, lang: &str) -> Value {
    json!({"content": [{"type": "text", "text": closed_text(config, lang)}], "isError": false})
}

/// What the app's routes answer while it is closed (the app says it in its own words).
pub fn http_locked() -> crate::web::HttpError {
    crate::web::HttpError::json(503, json!({"code": "vault_locked", "message": "Ton coffre est fermé pour le moment."}))
}

// -- the command line --------------------------------------------------------------------------------------------------

const USAGE: &str = "usage: bipbridge vault-keys status | enable | open [--config FILE]

  status   the vault's keys: off (today's layout), open, or locked (and why)
  enable   seal this place's vault, mailbox and connection keys (the bridge stopped): the key service's settings
           from the space's keyservice.json, else BIPAGENTS_KEYS_URL, BIPAGENTS_KEYS_TOKEN (and _GRANT, _SIGNATURE)
  open     ask the key service now (an operator, once a place is resumed): a running bridge sees it open
";

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// `bipbridge vault-keys …`: the exit code.
pub fn cli(argv: &[String]) -> i32 {
    let Some(action) = argv.first().cloned() else {
        eprint!("{USAGE}");
        return 2;
    };
    let mut config_path = None;
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--config" if i + 1 < argv.len() => {
                config_path = Some(argv[i + 1].clone());
                i += 1;
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return 0;
            }
            other => {
                eprint!("{USAGE}");
                eprintln!("bipbridge vault-keys: unknown option {other}");
                return 2;
            }
        }
        i += 1;
    }
    let config = match crate::config::load_config(config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return 2;
        }
    };
    let place = Place::of(&config);
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let result: Result<Value> = runtime.block_on(async {
        match action.as_str() {
            "status" => Ok(match state(&place.data) {
                State::Off => json!({"state": "off"}),
                State::Open => json!({"state": "open"}),
                State::Closed(c) => json!({"state": "locked", "reason": c.reason}),
            }),
            "enable" => {
                let address = std::net::SocketAddr::new(
                    config.host.parse().unwrap_or(std::net::IpAddr::from([127, 0, 0, 1])),
                    config.port,
                );
                if std::net::TcpStream::connect_timeout(&address, Duration::from_millis(500)).is_ok() {
                    bail!("the bridge is running: stop it first");
                }
                let given = match (env("BIPAGENTS_KEYS_URL"), env("BIPAGENTS_KEYS_TOKEN")) {
                    (Some(url), Some(token)) => Some(KeyService {
                        url,
                        token,
                        grant: env("BIPAGENTS_KEYS_GRANT"),
                        signature: env("BIPAGENTS_KEYS_SIGNATURE"),
                    }),
                    _ => None,
                };
                let service = match key_service(&place) {
                    Ok(kept) => kept,
                    Err(_) => given
                        .clone()
                        .ok_or_else(|| anyhow!("BIPAGENTS_KEYS_URL and BIPAGENTS_KEYS_TOKEN are not set"))?,
                };
                let source = crate::space::HttpShare::new(service)?;
                enrol(&place, given, &source).await
            }
            "open" => {
                if !place.sealed().exists() {
                    bail!("this place's vault keys are not sealed");
                }
                if try_open(&place, &FromFile(place.clone())).await {
                    Ok(json!({"state": "open"}))
                } else {
                    match state(&place.data) {
                        State::Closed(c) => Ok(json!({"state": "locked", "reason": c.reason})),
                        _ => Ok(json!({"state": "open"})),
                    }
                }
            }
            other => bail!("unknown action {other}"),
        }
    });
    match result {
        Ok(value) => {
            println!("{}", serde_json::to_string_pretty(&value).unwrap_or_default());
            0
        }
        Err(e) => {
            eprintln!("bipbridge vault-keys {action}: {e:#}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_key_opens_only_with_its_key_and_its_purpose() {
        let key = random::<KEY_LEN>();
        let sealed = seal(&key, b"fake data key", &DataKey::Mail.aad()).unwrap();
        assert_eq!(open(&key, &sealed, &DataKey::Mail.aad()).unwrap().as_slice(), b"fake data key");
        // another purpose (a mail key put in the vault's place), another key, a flipped byte: refused
        assert!(open(&key, &sealed, &DataKey::Vault.aad()).is_err());
        assert!(open(&random::<KEY_LEN>(), &sealed, &DataKey::Mail.aad()).is_err());
        let mut raw = B64.decode(&sealed).unwrap();
        raw[NONCE_LEN] ^= 1;
        assert!(open(&key, &B64.encode(raw), &DataKey::Mail.aad()).is_err());
        // a new nonce each time
        assert_ne!(seal(&key, b"x", b"a").unwrap(), seal(&key, b"x", b"a").unwrap());
    }

    #[test]
    fn the_kek_needs_both_halves() {
        let (a, b) = ([1u8; 32], [2u8; 32]);
        let k = kek(&a, &b).unwrap();
        assert_eq!(k.as_slice(), kek(&a, &b).unwrap().as_slice());
        assert_ne!(k.as_slice(), kek(&a, &[3u8; 32]).unwrap().as_slice());
        // not the halves' XOR itself
        assert_ne!(k.as_slice(), &[3u8; 32]);
        assert!(kek(&a, &[2u8; 31]).is_err());
    }

    #[test]
    fn the_test_switch_never_lets_a_release_build_keep_the_key_on_a_disk() {
        // (a folder in memory, the switch on, a debug build): only a debug build with the switch skips the check
        assert!(!memory_folder_accepted(false, true, false), "release: the switch is ignored");
        assert!(!memory_folder_accepted(false, false, false));
        assert!(!memory_folder_accepted(false, false, true));
        assert!(memory_folder_accepted(false, true, true), "tests (debug) with the switch");
        assert!(memory_folder_accepted(true, false, false));
    }

    #[test]
    fn locked_is_found_through_context() {
        let e = anyhow::Error::from(Locked).context("reading the mailboxes");
        assert!(is_locked(&e));
        assert!(!is_locked(&anyhow!("other")));
    }
}
