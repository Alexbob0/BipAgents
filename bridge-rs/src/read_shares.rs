//! « Partager en lecture » (decided 2026-10-27): one conversation of an agent shown read-only to up to 20 people,
//! until an end date. The design: internal/read-only-shares.md.
//!
//! The owner's app makes the share at the guichet with its account (`{share, writer}`) and hands it here. From then
//! on this bridge keeps the conversation's encrypted copy at the guichet up to date: a content key it makes (never
//! sent anywhere), the messages, pages and map pictures sealed with it (AES-256-GCM), and the key wrapped for each
//! viewer's own device key (the accounts' box: P-256, HKDF-SHA256, AES-256-GCM) once that viewer proved, with the
//! secret only its link carried (`#…&k=`, never sent to a server), that its key is the one invited. So the guichet
//! (Kerfy) stores and passes on what it can't read, and can't slip in a key of its own. Removing a viewer: a new key,
//! everything sent again under it.
//!
//! What viewers get: the person's typed messages and the agent's final replies (no `MEDIA:` lines, no tool calls,
//! cards, approvals, reasoning), the pages the replies link to (not the health ones, not the Carnet's) and the map
//! pictures. While a run goes on in a shared conversation, the agent's private tools are closed as during a
//! relative's turn (agent_shares.rs `RELATIVE_TOOLS`), and the app adds `instructions` (this view's) to the run.
//!
//! `<data>/read-shares.json` (0600): each share's guichet, writer token, content key, invitations' secrets.
//!
//! No Python counterpart.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::Router;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::config::Config;
use crate::tools_knowledge as tk;
use crate::web::{self, ApiResult, HttpError, QueryMap};
use crate::{py, Dict};

/// Each live share is brought up to date this often (new viewers keyed, new messages sent).
pub const SYNC_SECONDS: u64 = 20;
/// A map picture or a page bigger than this stays out: sealed, its base64 in JSON in base64 again stays under the
/// 4.2 MB the guichet takes per item.
pub const ITEM_MAX: usize = 1_500_000;
/// A photo the person sent, at most (bigger ones stay out: the apps send theirs smaller).
pub const PHOTO_MAX: usize = 1_500_000;
/// Items sent to the guichet in one request, by size and by number (it takes 300 at once).
const BATCH_BYTES: usize = 8_000_000;
const BATCH_ITEMS: usize = 250;
const STD: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
const URL: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

static LOCK: Mutex<()> = Mutex::new(());
/// One sync at a time (the loop and the app's calls).
static SYNCING: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

fn now() -> f64 {
    py::time_time()
}

fn s(v: &Value, key: &str) -> String {
    tk::str_or_empty(v.get(key))
}

// -- the crypto (the apps' twins: ReadShare.swift, ReadShareCrypto.kt) -----------------------------------------------

type HmacSha256 = Hmac<Sha256>;

fn hmac(key: &[u8], parts: &[&[u8]]) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key");
    for (n, part) in parts.iter().enumerate() {
        if n > 0 {
            mac.update(&[0]);
        }
        mac.update(part);
    }
    mac
}

/// The viewer's proof: HMAC-SHA256(S, "bipagents-view-v1" · 0 · invite id · 0 · public key raw).
pub fn proof(secret: &[u8], invite: &str, public_key: &[u8]) -> String {
    hex::encode(hmac(secret, &[b"bipagents-view-v1", invite.as_bytes(), public_key]).finalize().into_bytes())
}

pub fn proof_ok(secret: &[u8], invite: &str, public_key: &[u8], given: &str) -> bool {
    let Ok(given) = hex::decode(given.trim()) else { return false };
    hmac(secret, &[b"bipagents-view-v1", invite.as_bytes(), public_key]).verify_slice(&given).is_ok()
}

/// The tag a viewer checks its content key with: HMAC-SHA256(S, "bipagents-view-key-v1" · 0 · share · 0 · epoch · 0 · K).
pub fn key_tag(secret: &[u8], share: &str, epoch: i64, key: &[u8]) -> String {
    let epoch = epoch.to_string();
    hex::encode(
        hmac(secret, &[b"bipagents-view-key-v1", share.as_bytes(), epoch.as_bytes(), key]).finalize().into_bytes(),
    )
}

fn hkdf_one(ikm: &[u8], salt: &[u8], info: &[u8]) -> [u8; 32] {
    let prk = hmac(salt, &[ikm]).finalize().into_bytes();
    let mut expand = HmacSha256::new_from_slice(&prk).expect("HMAC takes any key");
    expand.update(info);
    expand.update(&[1]);
    expand.finalize().into_bytes().into()
}

fn gcm_seal(key: &[u8; 32], plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
    use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
    use ring::rand::SecureRandom;
    let mut nonce = [0u8; 12];
    ring::rand::SystemRandom::new().fill(&mut nonce).map_err(|_| anyhow::anyhow!("no randomness"))?;
    let sealing = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).map_err(|_| anyhow::anyhow!("bad key"))?);
    let mut data = plaintext.to_vec();
    sealing
        .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut data)
        .map_err(|_| anyhow::anyhow!("not sealed"))?;
    let mut out = nonce.to_vec();
    out.extend(data);
    Ok(out)
}

/// The accounts' box for a P-256 public key (raw, 65 bytes): ephemeral key · nonce · ciphertext · tag, base64.
pub fn seal_box(plaintext: &[u8], recipient: &[u8]) -> anyhow::Result<String> {
    use ring::agreement::{agree_ephemeral, EphemeralPrivateKey, UnparsedPublicKey, ECDH_P256};
    let rng = ring::rand::SystemRandom::new();
    let ephemeral = EphemeralPrivateKey::generate(&ECDH_P256, &rng).map_err(|_| anyhow::anyhow!("no key"))?;
    let public = ephemeral.compute_public_key().map_err(|_| anyhow::anyhow!("no key"))?;
    let public = public.as_ref().to_vec();
    let secret = agree_ephemeral(ephemeral, &UnparsedPublicKey::new(&ECDH_P256, recipient), |shared| shared.to_vec())
        .map_err(|_| anyhow::anyhow!("not a P-256 key"))?;
    let salt = [public.as_slice(), recipient].concat();
    let key = hkdf_one(&secret, &salt, b"bipagents-box-v1");
    let mut out = public;
    out.extend(gcm_seal(&key, plaintext)?);
    Ok(STD.encode(out))
}

/// An item sealed with the content key: nonce · ciphertext · tag, base64.
pub fn seal_item(key: &[u8; 32], plaintext: &Value) -> anyhow::Result<String> {
    Ok(STD.encode(gcm_seal(key, serde_json::to_string(plaintext)?.as_bytes())?))
}

fn random_bytes<const N: usize>() -> [u8; N] {
    use ring::rand::SecureRandom;
    let mut out = [0u8; N];
    ring::rand::SystemRandom::new().fill(&mut out).expect("randomness");
    out
}

/// The content key wrapped for one viewer.
pub fn wrap_key(share: &str, epoch: i64, key: &[u8; 32], secret: &[u8], public_key: &[u8]) -> anyhow::Result<String> {
    let inner = json!({"s": share, "e": epoch, "k": STD.encode(key), "t": key_tag(secret, share, epoch, key)});
    seal_box(inner.to_string().as_bytes(), public_key)
}

// -- the file ---------------------------------------------------------------------------------------------------------

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("read-shares.json")
}

fn load(path: &PathBuf) -> Dict {
    let data: Value =
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
    let mut data = data.as_object().cloned().unwrap_or_default();
    if !matches!(data.get("shares"), Some(Value::Object(_))) {
        data.insert("shares".into(), json!({}));
    }
    data
}

fn save(path: &PathBuf, data: &Dict) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    py::write_with_mode(&tmp.to_string_lossy(), Value::Object(data.clone()).to_string().as_bytes(), 0o600)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn shares_of(config: &Config) -> Map<String, Value> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load(&path(config)).get("shares").and_then(Value::as_object).cloned().unwrap_or_default()
}

fn share_of(config: &Config, id: &str) -> Option<Value> {
    shares_of(config).get(id).cloned()
}

/// Changes one share (None: it goes).
fn put_share(config: &Config, id: &str, share: Option<Value>) -> anyhow::Result<()> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let file = path(config);
    let mut data = load(&file);
    let shares = data.get_mut("shares").and_then(Value::as_object_mut).expect("an object");
    match share {
        Some(v) => shares.insert(id.to_string(), v),
        None => shares.remove(id),
    };
    save(&file, &data)
}

fn content_key(share: &Value) -> Option<[u8; 32]> {
    STD.decode(s(share, "key")).ok().and_then(|k| <[u8; 32]>::try_from(k.as_slice()).ok())
}

/// Whether a share is of an agent's whole thread (a project agent's, project_agent.rs) rather than one conversation.
pub fn is_thread(share: &Value) -> bool {
    share.get("thread").and_then(Value::as_bool) == Some(true)
}

/// The live shares of this conversation.
pub fn shares_for(config: &Config, agent: &str, session: &str) -> Vec<Value> {
    let at = now();
    shares_of(config)
        .into_values()
        .filter(|v| {
            s(v, "agent") == agent
                && !is_thread(v)
                && s(v, "session") == session
                && v["ends"].as_f64().unwrap_or(0.0) > at
        })
        .collect()
}

/// The live shares of this agent's whole thread.
pub fn thread_shares(config: &Config, agent: &str) -> Vec<Value> {
    let at = now();
    shares_of(config)
        .into_values()
        .filter(|v| s(v, "agent") == agent && is_thread(v) && v["ends"].as_f64().unwrap_or(0.0) > at)
        .collect()
}

/// The share of this agent's thread kept here, if any.
pub fn thread_share(config: &Config, agent: &str) -> Option<Value> {
    shares_of(config).into_values().find(|v| s(v, "agent") == agent && is_thread(v))
}

// -- the agent in a shared conversation ---------------------------------------------------------------------------------

/// How many people see a share (joined), from the last look at the guichet.
fn seen_by(share: &Value) -> usize {
    share.get("viewers").and_then(Value::as_array).map(|v| v.len()).unwrap_or(0)
}

/// What the app adds to the run's instructions in a shared conversation.
pub fn instructions(count: usize) -> String {
    let who = match count {
        0 => "people your person invited (none has opened it yet)".to_string(),
        1 => "1 person your person invited".to_string(),
        n => format!("{n} people your person invited"),
    };
    format!(
        "This conversation is shown read-only to {who}: they read your person's messages and your replies here. Keep \
your person's private things out of it: don't mention or use their mail, calendar, contacts, health data, notebook, \
archives, files, vault or their other conversations (those tools are closed here), and don't put personal details in \
pages or maps made in this conversation. Weather, routes, maps and the web are fine. {}",
        crate::souls::CONTENT_RULE
    )
}

/// Whether a run goes on in one of this agent's shared conversations (its private tools are then closed).
pub fn shared_turn(state: &AppState, agent: &str) -> bool {
    let at = now();
    shares_of(&state.config()).values().any(|v| {
        s(v, "agent") == agent
            && v["ends"].as_f64().unwrap_or(0.0) > at
            && (is_thread(v) || state.hub.session_busy(agent, &s(v, "session")))
    })
}

/// The answer a closed tool gives in a shared conversation.
pub fn closed_tool(name: &str) -> Option<Dict> {
    if crate::agent_shares::RELATIVE_TOOLS.contains(&name) {
        return None;
    }
    Some(tk::tool_text(
        "This conversation is shown to people your person invited: your person's own data and tools (mail, calendar, \
         contacts, health, notebook, archives, files, pages, tasks) are closed here. Answer from what you know, or say \
         your person can ask in another conversation.",
        true,
    ))
}

// -- the conversation as items ------------------------------------------------------------------------------------------

static MEDIA_LINE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^\s*MEDIA:\s*(\S+)\s*$").expect("valid regex"));

/// A message's text without its `MEDIA:` lines, and the paths they named.
pub fn split_media(text: &str) -> (String, Vec<String>) {
    let mut paths = Vec::new();
    let mut kept = Vec::new();
    for line in text.lines() {
        match MEDIA_LINE.captures(line) {
            Some(found) => paths.push(found[1].to_string()),
            None => kept.push(line),
        }
    }
    (kept.join("\n").trim().to_string(), paths)
}

/// The item a `MEDIA:` path of this agent becomes: a page (not a health one, not the Carnet's), a map picture.
fn media_item(config: &Config, agent: &str, path: &str) -> Option<(String, Value)> {
    if path.starts_with(crate::friends_cards::PREFIX) {
        // a card the friends tap (friends_cards.rs): only in a project agent's thread share
        return crate::friends_cards::media_item(config, agent, path);
    }
    if let Some(rest) = path.strip_prefix(&format!("{}/{agent}/", crate::pages::PREFIX)) {
        let page = rest.strip_suffix(".html")?;
        if !crate::pages::ID.is_match(page) || page.contains('\n') {
            return None;
        }
        let folder = crate::pages::root(config).join(agent);
        let meta: Dict = std::fs::read_to_string(folder.join(format!("{page}.json")))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        if crate::pages::private(&meta) || crate::pages::link(&meta).is_some() {
            return None;
        }
        let html = std::fs::read_to_string(folder.join(format!("{page}.html"))).ok()?;
        if html.len() > ITEM_MAX {
            return None;
        }
        let title = meta.get("title").and_then(Value::as_str).unwrap_or(page).to_string();
        let id = format!("p-{agent}-{page}");
        let map = html.contains("bipmap:");
        return Some((id.clone(), json!({"id": id, "kind": "page", "title": title, "html": html, "map": map})));
    }
    if let Some(rest) = path.strip_prefix(&format!("{}/{agent}/", crate::maps::PREFIX)) {
        let map = rest.strip_suffix(".png")?;
        let file = crate::maps::file(config, agent, map)?;
        let data = std::fs::read(file).ok()?;
        if data.len() > ITEM_MAX {
            return None;
        }
        let id = format!("f-{}", &hex::encode(Sha256::digest(&data))[..16]);
        return Some((
            id.clone(),
            json!({"id": id, "kind": "photo", "name": format!("{map}.png"), "mime": "image/png", "data": STD.encode(data)}),
        ));
    }
    None
}

/// A message's key as Hermes gives it (its id), else its place in the conversation.
fn message_key(message: &Dict, n: usize) -> String {
    match message.get("id") {
        Some(Value::Number(id)) => id.to_string(),
        Some(Value::String(t)) if !t.is_empty() && t.len() <= 60 => {
            t.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect()
        }
        _ => n.to_string(),
    }
}

fn image_part(part: &Value) -> bool {
    matches!(part.get("type").and_then(Value::as_str), Some("image_url" | "input_image" | "image"))
}

/// Whether a message carries pictures (the person's photos): image parts in its content.
pub fn has_images(message: &Dict) -> bool {
    message.get("content").and_then(Value::as_array).is_some_and(|parts| parts.iter().any(image_part))
}

/// The pictures of a message read with `inline_images=true`: (mime, bytes) of each `data:` image.
pub fn images_of(message: &Dict) -> Vec<(String, Vec<u8>)> {
    let Some(parts) = message.get("content").and_then(Value::as_array) else { return Vec::new() };
    parts
        .iter()
        .filter(|p| image_part(p))
        .filter_map(|p| {
            let url = p.get("image_url")?;
            let url = url.as_str().or_else(|| url.get("url").and_then(Value::as_str))?;
            let (head, data) = url.strip_prefix("data:")?.split_once(',')?;
            let mime = head.strip_suffix(";base64")?;
            if !mime.starts_with("image/") {
                return None;
            }
            STD.decode(data.trim()).ok().map(|bytes| (mime.to_string(), bytes))
        })
        .collect()
}

/// The person's own words in a user row, or None for rows the person didn't type (a sub-task's report, the bridge's
/// nudges). Words added while the agent answered are unwrapped from Hermes' markers. The notes for the agent only
/// (`<!--bip:…-->`: the current time, a call's recap, family news…) are left out, as the apps do; a row of notes only
/// is not the person's.
pub fn typed_words(text: &str) -> Option<String> {
    const OPEN: &str = "[OUT-OF-BAND USER MESSAGE";
    const CLOSE: &str = "[/OUT-OF-BAND USER MESSAGE]";
    let shown = crate::live_brief::visible(text);
    if shown.is_empty() {
        return None;
    }
    let text = shown.as_str();
    let text = match text.strip_prefix(OPEN).and_then(|rest| rest.strip_suffix(CLOSE)) {
        Some(inner) => inner.split_once(']').map(|(_, words)| words.trim()).unwrap_or(""),
        None => text,
    };
    if text.starts_with("[bipagents:") || crate::cronwatch::REPORT_PREFIXES.iter().any(|p| text.starts_with(p)) {
        return None;
    }
    Some(text.to_string())
}

/// The conversation's items: its meta, then the person's messages (with the photos already sealed for them:
/// `photos` in the share, message key -> photo item ids) and the agent's final replies with their pages and map
/// pictures.
pub fn items_of(config: &Config, share: &Value, messages: &[Dict]) -> Vec<(String, Value)> {
    let agent = s(share, "agent");
    let found = config.agent(Some(&agent));
    let shown = found.map(|a| a.display_name.clone()).unwrap_or_else(|| agent.clone());
    // the agent's look (its Bip's category and colour), so the viewer's app draws it as the owner sees it
    let (category, color) = found.map(|a| (a.category.clone(), a.color.clone())).unwrap_or_default();
    let mut items = vec![(
        "meta".to_string(),
        json!({"id": "meta", "kind": "meta", "title": s(share, "title"), "agent": shown, "from": s(share, "from"),
               "ends": share["ends"], "category": category, "color": color}),
    )];
    let photos = share.get("photos").and_then(Value::as_object);
    let mut media_seen = HashSet::new();
    for (n, message) in messages.iter().enumerate() {
        let role = s(&Value::Object(message.clone()), "role").to_lowercase();
        if role != "user" && role != "assistant" {
            continue;
        }
        if role == "assistant" && message.get("tool_calls").map(py::truthy).unwrap_or(false) {
            continue; // a step of the agent's work, not its reply
        }
        let key = message_key(message, n);
        let raw = crate::cronwatch::message_text(message);
        let (text, paths) = if role == "user" {
            match typed_words(&raw) {
                Some(words) => (words, Vec::new()),
                None => continue,
            }
        } else {
            split_media(&raw)
        };
        let mut media = Vec::new();
        if role == "assistant" {
            for p in paths {
                if let Some((id, item)) = media_item(config, &agent, &p) {
                    media.push(json!(id));
                    if media_seen.insert(id.clone()) {
                        items.push((id, item));
                    }
                }
            }
        } else if let Some(ids) = photos.and_then(|all| all.get(&key)).and_then(Value::as_array) {
            media.extend(ids.iter().cloned());
        }
        if text.is_empty() && media.is_empty() {
            continue;
        }
        let id = format!("m-{key}");
        let at = message
            .get("timestamp")
            .and_then(Value::as_f64)
            .or_else(|| message.get("created_at").and_then(Value::as_f64));
        items
            .push((id.clone(), json!({"id": id, "kind": "msg", "role": role, "text": text, "at": at, "media": media})));
    }
    items
}

/// Which of the person's messages carry photos not sealed yet (by key).
pub fn photos_wanted(messages: &[Dict], done: &Map<String, Value>) -> HashSet<String> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, m)| s(&Value::Object((*m).clone()), "role").eq_ignore_ascii_case("user") && has_images(m))
        .map(|(n, m)| message_key(m, n))
        .filter(|key| !done.contains_key(key))
        .collect()
}

/// The photos of those messages, from the conversation read with its images: (photo items, message key -> ids). A
/// photo bigger than `PHOTO_MAX` stays out.
pub fn new_photos(inline: &[Dict], wanted: &HashSet<String>) -> (Vec<(String, Value)>, Map<String, Value>) {
    let mut items = Vec::new();
    let mut found = Map::new();
    for (n, message) in inline.iter().enumerate() {
        let key = message_key(message, n);
        if !wanted.contains(&key) {
            continue;
        }
        let mut ids = Vec::new();
        for (k, (mime, bytes)) in images_of(message).into_iter().enumerate() {
            if bytes.len() > PHOTO_MAX {
                continue;
            }
            let id = format!("f-{}", &hex::encode(Sha256::digest(&bytes))[..16]);
            let ext = mime.trim_start_matches("image/").replace("jpeg", "jpg");
            items.push((
                id.clone(),
                json!({"id": id, "kind": "photo", "name": format!("photo-{key}-{k}.{ext}"), "mime": mime,
                       "data": STD.encode(&bytes)}),
            ));
            ids.push(json!(id));
        }
        found.insert(key, json!(ids));
    }
    (items, found)
}

fn digest(item: &Value) -> String {
    hex::encode(&Sha256::digest(item.to_string().as_bytes())[..12])
}

// -- the guichet ------------------------------------------------------------------------------------------------------

fn guichet_ok(url: &str) -> bool {
    match url::Url::parse(url) {
        Ok(u) if u.scheme() == "https" => u.host_str().is_some(),
        Ok(u) if u.scheme() == "http" => matches!(u.host_str(), Some("127.0.0.1") | Some("localhost")),
        _ => false,
    }
}

async fn guichet(
    state: &AppState,
    share: &Value,
    method: reqwest::Method,
    rest: &str,
    body: Option<Value>,
) -> Result<(u16, Value), String> {
    let url = format!("{}/v1/rshares/{}{rest}", s(share, "guichet").trim_end_matches('/'), s(share, "id"));
    let mut request = state.http.request(method, url).timeout(Duration::from_secs(60)).bearer_auth(s(share, "writer"));
    if let Some(body) = body {
        request = request.json(&body);
    }
    let resp = request.send().await.map_err(|e| format!("the guichet can't be reached: {e}"))?;
    let status = resp.status().as_u16();
    let body = resp.json::<Value>().await.unwrap_or(Value::Null);
    Ok((status, body))
}

fn refused(status: u16, body: &Value) -> HttpError {
    let detail = body.get("detail").and_then(Value::as_str).unwrap_or("the guichet refused");
    HttpError::new(if status >= 500 { 502 } else { status }, detail)
}

/// The messages a share shows: its conversation's, or for a share of a whole thread, those of the agent's « Bot
/// Chat » session(s), oldest first (one list, so a message's place is the same with or without its images).
async fn messages_of(
    state: &AppState,
    agent: &crate::config::AgentConfig,
    share: &Value,
    images: bool,
) -> Option<Vec<Dict>> {
    let sessions: Vec<String> = if is_thread(share) {
        let rows = state.hermes.bot_chat_sessions(agent).await.ok()?;
        rows.iter().map(|r| tk::str_or_empty(r.get("id"))).filter(|id| !id.is_empty()).collect()
    } else {
        vec![s(share, "session")]
    };
    let mut all = Vec::new();
    for session in sessions {
        let found = if images {
            state.hermes.session_messages_with_images(agent, &session).await
        } else {
            state.hermes.session_messages(agent, &session).await
        };
        all.extend(found.ok()?);
    }
    Some(all)
}

/// Brings one share up to date: new viewers keyed (their proof checked), a new key after a removal, new or changed
/// items sent (`notify`: viewers told of new messages). The share as kept then; None when it is gone (stopped, ended).
pub async fn sync(state: &AppState, id: &str, notify: bool) -> Result<Option<Value>, HttpError> {
    let _one = SYNCING.lock().await;
    let config = state.config();
    let Some(mut share) = share_of(&config, id) else { return Ok(None) };
    if config.agent(Some(&s(&share, "agent"))).is_none() {
        // its agent deleted: the share stops (deleted at the guichet, so in the viewers' apps)
        let (status, _) =
            guichet(state, &share, reqwest::Method::DELETE, "", None).await.map_err(|e| HttpError::new(502, e))?;
        if status == 200 || status == 404 || status == 410 {
            put_share(&config, id, None).map_err(|e| HttpError::new(500, e.to_string()))?;
        }
        return Ok(None);
    }
    let (status, remote) =
        guichet(state, &share, reqwest::Method::GET, "", None).await.map_err(|e| HttpError::new(502, e))?;
    if status == 404 || status == 410 {
        put_share(&config, id, None).map_err(|e| HttpError::new(500, e.to_string()))?;
        return Ok(None);
    }
    if status != 200 {
        return Err(refused(status, &remote));
    }
    let epoch = remote["epoch"].as_i64().unwrap_or(1);
    let mut first = share.get("sent").is_none();
    if share["epoch"].as_i64() != Some(epoch) || content_key(&share).is_none() {
        // a viewer was removed (or the first time): a new key, everything sent again under it
        share["epoch"] = json!(epoch);
        share["key"] = json!(STD.encode(random_bytes::<32>()));
        share["sent"] = json!({});
        share["photos"] = json!({}); // read again: sealed under the new key
        first = true;
    }
    let key = content_key(&share).expect("just made");
    let secrets: HashMap<String, (String, Vec<u8>)> = share
        .get("invites")
        .and_then(Value::as_object)
        .map(|all| {
            all.iter()
                .filter_map(|(hash, v)| URL.decode(s(v, "secret")).ok().map(|k| (hash.clone(), (s(v, "invite"), k))))
                .collect()
        })
        .unwrap_or_default();
    let mut refused_ones: Vec<Value> = share.get("refused").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut wraps = Vec::new();
    let viewers: Vec<Value> = remote["viewers"].as_array().cloned().unwrap_or_default();
    for v in &viewers {
        if v["keyed"].as_bool() == Some(true) {
            continue;
        }
        let viewer = v["viewer"].clone();
        let raw = STD.decode(s(v, "public_key")).unwrap_or_default();
        let checked =
            secrets.get(&s(v, "invite")).filter(|(invite, secret)| proof_ok(secret, invite, &raw, &s(v, "proof")));
        match checked {
            Some((_, secret)) => match wrap_key(id, epoch, &key, secret, &raw) {
                Ok(wrapped) => wraps.push(json!({"viewer": viewer, "wrapped": wrapped})),
                Err(e) => tracing::warn!(error = %e, "a read share's key not wrapped"),
            },
            None => {
                // a key the link didn't vouch for (not the invited device's): never keyed
                if !refused_ones.contains(&viewer) {
                    tracing::warn!(share = id, "a viewer's proof doesn't check: not keyed");
                    refused_ones.push(viewer);
                }
            }
        }
    }
    share["refused"] = json!(refused_ones);
    if is_thread(&share) {
        // the friends' votes and ticks waiting at the guichet (friends_cards.rs): applied before the items go
        crate::friends_cards::take_actions(state, &share, epoch, &key, &viewers, &secrets).await;
    }
    if !wraps.is_empty() {
        let (status, body) =
            guichet(state, &share, reqwest::Method::PUT, "/keys", Some(json!({"epoch": epoch, "keys": wraps})))
                .await
                .map_err(|e| HttpError::new(502, e))?;
        if status != 200 {
            return Err(refused(status, &body));
        }
    }
    // the viewers as the owner's app shows them
    share["viewers"] = json!(viewers
        .iter()
        .map(|v| {
            let state = if v["keyed"].as_bool() == Some(true) || wraps.iter().any(|w| w["viewer"] == v["viewer"]) {
                "ok"
            } else if share["refused"].as_array().is_some_and(|r| r.contains(&v["viewer"])) {
                "refused"
            } else {
                "waiting"
            };
            json!({"id": v["viewer"], "name": v["name"], "joined": v["joined"], "state": state})
        })
        .collect::<Vec<_>>());
    let open: Vec<Value> = remote["invites"]
        .as_array()
        .map(|all| {
            all.iter()
                .filter(|i| i["used"].is_null() && i["expires"].as_f64().unwrap_or(0.0) > now())
                .map(|i| json!({"invite": i["invite"], "name": i["name"], "expires": i["expires"]}))
                .collect()
        })
        .unwrap_or_default();
    share["invites_open"] = json!(open);
    if let Some(ends) = remote["ends"].as_f64() {
        share["ends"] = json!(ends);
    }
    // the items: what changed since last time
    let messages = match config.agent(Some(&s(&share, "agent"))) {
        Some(agent) => messages_of(state, agent, &share, false).await,
        None => None,
    };
    if let Some(messages) = messages {
        // the person's photos in this conversation: read with their bytes once, sealed like the rest
        let done = share.get("photos").and_then(Value::as_object).cloned().unwrap_or_default();
        let wanted = photos_wanted(&messages, &done);
        let mut photo_items = Vec::new();
        if !wanted.is_empty() {
            if let Some(agent) = config.agent(Some(&s(&share, "agent"))) {
                if let Some(inline) = messages_of(state, agent, &share, true).await {
                    let (items, found) = new_photos(&inline, &wanted);
                    photo_items = items;
                    let mut all = done.clone();
                    all.extend(found);
                    share["photos"] = Value::Object(all);
                }
            }
        }
        let sent = share.get("sent").and_then(Value::as_object).cloned().unwrap_or_default();
        let mut now_sent = sent.clone();
        let mut batch: Vec<Value> = Vec::new();
        let mut size = 0;
        let mut new_message = false;
        let mut batches = Vec::new();
        let mut all_items = photo_items;
        all_items.extend(items_of(&config, &share, &messages));
        for (item_id, mut item) in all_items {
            let mark = digest(&item);
            if sent.get(&item_id).and_then(Value::as_str) == Some(mark.as_str()) {
                continue;
            }
            if item["kind"] == "msg" && !sent.contains_key(&item_id) {
                new_message = true;
            }
            let kind = s(&item, "kind");
            item["s"] = json!(id);
            let data = seal_item(&key, &item).map_err(|e| HttpError::new(500, e.to_string()))?;
            if !batch.is_empty() && (size + data.len() > BATCH_BYTES || batch.len() >= BATCH_ITEMS) {
                batches.push(std::mem::take(&mut batch));
                size = 0;
            }
            size += data.len();
            batch.push(json!({"id": item_id, "kind": kind, "data": data}));
            now_sent.insert(item_id, json!(mark));
        }
        if !batch.is_empty() {
            batches.push(batch);
        }
        let count = batches.len();
        for (n, items) in batches.into_iter().enumerate() {
            let tell = notify && !first && new_message && n + 1 == count;
            let (status, body) = guichet(
                state,
                &share,
                reqwest::Method::POST,
                "/items",
                Some(json!({"epoch": epoch, "items": items, "notify": tell})),
            )
            .await
            .map_err(|e| HttpError::new(502, e))?;
            if status == 409 {
                break; // a removal meanwhile: the next sync makes the new key
            }
            if status != 200 {
                return Err(refused(status, &body));
            }
        }
        share["sent"] = Value::Object(now_sent);
    }
    share["synced"] = json!(now());
    put_share(&config, id, Some(share.clone())).map_err(|e| HttpError::new(500, e.to_string()))?;
    Ok(Some(share))
}

/// Every live share kept up to date, forever (only when there are some).
pub async fn run_forever(state: AppState) {
    loop {
        tokio::time::sleep(Duration::from_secs(SYNC_SECONDS)).await;
        let ids: Vec<String> = shares_of(&state.config()).keys().cloned().collect();
        for id in ids {
            if let Err(e) = sync(&state, &id, true).await {
                tracing::debug!(share = %id, error = ?e.detail, "read share not synced");
            }
        }
    }
}

// -- the owner's app ----------------------------------------------------------------------------------------------------

/// The share as the owner's app shows it.
pub fn view(share: &Value) -> Value {
    let count = share
        .get("viewers")
        .and_then(Value::as_array)
        .map(|v| v.iter().filter(|x| x["state"] == "ok").count())
        .unwrap_or(0);
    json!({
        "id": share["id"], "agent": share["agent"], "session": share.get("session").cloned().unwrap_or(Value::Null),
        "thread": is_thread(share), "title": share["title"],
        "ends": share["ends"], "count": count,
        "viewers": share.get("viewers").cloned().unwrap_or_else(|| json!([])),
        "invites": share.get("invites_open").cloned().unwrap_or_else(|| json!([])),
        "instructions": instructions(seen_by(share)),
    })
}

fn body_of(body: &Bytes) -> ApiResult<Value> {
    let value: Value = serde_json::from_slice(body).map_err(|_| HttpError::new(400, "a JSON body"))?;
    if !value.is_object() {
        return Err(HttpError::new(400, "a JSON object"));
    }
    Ok(value)
}

fn text_field(body: &Value, name: &str, max: usize) -> ApiResult<String> {
    let text = s(body, name);
    let text = text.trim();
    if text.is_empty() || text.chars().count() > max {
        return Err(HttpError::new(422, format!("{name}: 1 to {max} characters")));
    }
    Ok(text.to_string())
}

fn id_ok(id: &str) -> bool {
    (16..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn known(config: &Config, id: &str) -> ApiResult<Value> {
    share_of(config, id).ok_or_else(|| HttpError::new(404, "no such share"))
}

async fn list(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let agent = web::query_required(&q, "agent")?;
    let session = q.get("session").map(str::to_string).filter(|s| !s.is_empty());
    let config = state.config();
    let mut out = Vec::new();
    // no session: the shares of the agent's whole thread (a project agent's)
    let found = match &session {
        Some(session) => shares_for(&config, &agent, session),
        None => thread_shares(&config, &agent),
    };
    for share in found {
        let id = s(&share, "id");
        match sync(&state, &id, true).await {
            Ok(Some(fresh)) => out.push(view(&fresh)),
            Ok(None) => {}
            Err(_) => out.push(view(&share)), // the guichet out of reach: as last seen
        }
    }
    Ok(web::ok(json!({"shares": out})))
}

/// The owner's app made the share at the guichet: kept here, and the conversation sent a first time.
async fn create(State(state): State<AppState>, body: Bytes) -> ApiResult<Response> {
    let body = body_of(&body)?;
    let config = state.config();
    let agent = crate::app::agent_or_404(&config, Some(&text_field(&body, "agent", 80)?))?;
    // a project agent's whole thread (project_agent.rs), never another agent's: its main thread holds private things
    let thread = body.get("thread").and_then(Value::as_bool) == Some(true);
    if thread && !crate::project_agent::is_project(&config, &agent.name) {
        return Err(HttpError::new(422, "thread: only a project agent's thread is shared"));
    }
    if thread && !thread_shares(&config, &agent.name).is_empty() {
        return Err(HttpError::new(409, "this project is shared already"));
    }
    let session = if thread { String::new() } else { text_field(&body, "session", 200)? };
    let title = text_field(&body, "title", 120)?;
    let from = text_field(&body, "from", 40)?;
    let id = text_field(&body, "share", 64)?;
    let writer = text_field(&body, "writer", 100)?;
    let guichet_url = text_field(&body, "guichet", 300)?;
    let ends =
        body["ends"].as_f64().filter(|e| *e > now()).ok_or_else(|| HttpError::new(422, "ends: in the future"))?;
    if !id_ok(&id) {
        return Err(HttpError::new(422, "share: an id from the guichet"));
    }
    if !guichet_ok(&guichet_url) {
        return Err(HttpError::new(422, "guichet: an https address"));
    }
    let mut share = json!({"id": id, "agent": agent.name, "session": session, "title": title, "from": from,
                       "guichet": guichet_url.trim_end_matches('/'), "writer": writer, "ends": ends,
                       "created": now(), "invites": {}});
    if thread {
        share["thread"] = json!(true);
        share.as_object_mut().expect("an object").remove("session");
    }
    put_share(&config, &id, Some(share)).map_err(|e| HttpError::new(500, e.to_string()))?;
    match sync(&state, &id, false).await {
        Ok(Some(share)) => Ok(web::ok(view(&share))),
        Ok(None) => Err(HttpError::new(404, "the guichet doesn't know this share")),
        Err(e) => {
            let _ = put_share(&config, &id, None);
            Err(e)
        }
    }
}

/// One person's invitation: the link to send them (the secret only it carries, kept here to check their device).
async fn invite(State(state): State<AppState>, UrlPath(id): UrlPath<String>, body: Bytes) -> ApiResult<Response> {
    let body = body_of(&body)?;
    let config = state.config();
    let mut share = known(&config, &id)?;
    let name = text_field(&body, "name", 40)?;
    let (status, made) = guichet(
        &state,
        &share,
        reqwest::Method::POST,
        "/invites",
        Some(json!({"name": name, "from": s(&share, "from")})),
    )
    .await
    .map_err(|e| HttpError::new(502, e))?;
    if status != 200 {
        return Err(refused(status, &made));
    }
    let invite = s(&made, "invite");
    let secret = URL.encode(random_bytes::<32>());
    let hash = hex::encode(Sha256::digest(invite.as_bytes()));
    if !share["invites"].is_object() {
        share["invites"] = json!({});
    }
    share["invites"][&hash] = json!({"invite": invite, "secret": secret, "name": name, "expires": made["expires"]});
    put_share(&config, &id, Some(share)).map_err(|e| HttpError::new(500, e.to_string()))?;
    let link = format!("https://bipagents.com/v#i={invite}&k={secret}");
    Ok(web::ok(json!({"link": link, "expires": made["expires"], "name": name})))
}

async fn cancel_invite(
    State(state): State<AppState>,
    UrlPath((id, hash)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let config = state.config();
    let mut share = known(&config, &id)?;
    if !(hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())) {
        return Err(HttpError::new(422, "an invitation"));
    }
    let (status, body) = guichet(&state, &share, reqwest::Method::DELETE, &format!("/invites/{hash}"), None)
        .await
        .map_err(|e| HttpError::new(502, e))?;
    if status != 200 {
        return Err(refused(status, &body));
    }
    if let Some(all) = share["invites"].as_object_mut() {
        all.remove(&hash);
    }
    put_share(&config, &id, Some(share)).map_err(|e| HttpError::new(500, e.to_string()))?;
    after(&state, &id).await
}

/// A viewer removed: a new key for the others, everything sent again under it.
async fn remove_viewer(
    State(state): State<AppState>,
    UrlPath((id, viewer)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    if viewer.is_empty() || viewer.len() > 20 || !viewer.bytes().all(|b| b.is_ascii_digit()) {
        return Err(HttpError::new(422, "a viewer"));
    }
    let config = state.config();
    let share = known(&config, &id)?;
    let (status, body) = guichet(&state, &share, reqwest::Method::DELETE, &format!("/viewers/{viewer}"), None)
        .await
        .map_err(|e| HttpError::new(502, e))?;
    if status != 200 {
        return Err(refused(status, &body));
    }
    after(&state, &id).await
}

async fn set_ends(State(state): State<AppState>, UrlPath(id): UrlPath<String>, body: Bytes) -> ApiResult<Response> {
    let body = body_of(&body)?;
    let ends = body["ends"].as_f64().ok_or_else(|| HttpError::new(422, "ends: a date"))?;
    let share = known(&state.config(), &id)?;
    let project = is_thread(&share) && crate::project_agent::is_project(&state.config(), &s(&share, "agent"));
    let ends = if project { crate::project_agent::ends_ok(Some(ends))? } else { ends };
    change_ends(&state, &id, ends).await?;
    if project {
        // a project's share ends with the project (project_agent.rs)
        crate::project_agent::set_ends_of(&state.config(), &s(&share, "agent"), ends)?;
    }
    after(&state, &id).await
}

/// A share's end date changed at the guichet.
pub async fn change_ends(state: &AppState, id: &str, ends: f64) -> ApiResult<()> {
    let share = known(&state.config(), id)?;
    let (status, answer) = guichet(state, &share, reqwest::Method::POST, "/ends", Some(json!({"ends": ends})))
        .await
        .map_err(|e| HttpError::new(502, e))?;
    if status != 200 {
        return Err(refused(status, &answer));
    }
    Ok(())
}

/// A share stopped: deleted at the guichet (the viewers' apps then delete their copy) and here.
pub async fn stop_share(state: &AppState, id: &str) -> ApiResult<()> {
    let config = state.config();
    let share = known(&config, id)?;
    let (status, body) =
        guichet(state, &share, reqwest::Method::DELETE, "", None).await.map_err(|e| HttpError::new(502, e))?;
    if status != 200 && status != 404 && status != 410 {
        return Err(refused(status, &body));
    }
    put_share(&config, id, None).map_err(|e| HttpError::new(500, e.to_string()))?;
    Ok(())
}

/// Stopped by the owner.
async fn stop(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    stop_share(&state, &id).await?;
    Ok(web::ok(json!({"ok": true})))
}

async fn sync_now(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    known(&state.config(), &id)?;
    after(&state, &id).await
}

async fn after(state: &AppState, id: &str) -> ApiResult<Response> {
    match sync(state, id, true).await? {
        Some(share) => Ok(web::ok(view(&share))),
        None => Err(HttpError::new(410, "this share has ended")),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/read-shares", get(list).post(create))
        .route("/read-shares/{id}", delete(stop))
        .route("/read-shares/{id}/invites", post(invite))
        .route("/read-shares/{id}/invites/{hash}", delete(cancel_invite))
        .route("/read-shares/{id}/viewers/{viewer}", delete(remove_viewer))
        .route("/read-shares/{id}/ends", post(set_ends))
        .route("/read-shares/{id}/sync", post(sync_now))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_box(sealed: &str, private: ring::agreement::EphemeralPrivateKey, mine: &[u8]) -> Vec<u8> {
        use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
        use ring::agreement::{agree_ephemeral, UnparsedPublicKey, ECDH_P256};
        let raw = STD.decode(sealed).unwrap();
        let (ephemeral, rest) = raw.split_at(65);
        let secret = agree_ephemeral(private, &UnparsedPublicKey::new(&ECDH_P256, ephemeral), |s| s.to_vec()).unwrap();
        let key = hkdf_one(&secret, &[ephemeral, mine].concat(), b"bipagents-box-v1");
        let (nonce, data) = rest.split_at(12);
        let opening = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).unwrap());
        let mut data = data.to_vec();
        let plain =
            opening.open_in_place(Nonce::try_assume_unique_for_key(nonce).unwrap(), Aad::empty(), &mut data).unwrap();
        plain.to_vec()
    }

    #[test]
    fn the_spec_s_vectors_hold() {
        let secret = [1u8; 32];
        let mut public = vec![4u8];
        public.extend([2u8; 64]);
        let p = proof(&secret, "abcdefghijklmnop0123", &public);
        assert_eq!(p, "169681e562378f18a491b705c3ea695d61b812c20b0c97eaa0f173f40c59e679");
        assert!(proof_ok(&secret, "abcdefghijklmnop0123", &public, &p));
        assert!(!proof_ok(&[9u8; 32], "abcdefghijklmnop0123", &public, &p));
        assert!(!proof_ok(&secret, "abcdefghijklmnop0124", &public, &p));
        assert_eq!(
            key_tag(&secret, "share0123456789abcdef", 1, &[3u8; 32]),
            "d54377bf85358afa38c7bcaf98f871e8cf4746d98620ee6c5b57d9abe2409c1b"
        );
        assert_eq!(URL.encode(secret), "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE");
    }

    #[test]
    fn a_wrapped_key_opens_only_with_the_viewer_s_key_and_carries_its_tag() {
        use ring::agreement::{EphemeralPrivateKey, ECDH_P256};
        let rng = ring::rand::SystemRandom::new();
        let viewer = EphemeralPrivateKey::generate(&ECDH_P256, &rng).unwrap();
        let public = viewer.compute_public_key().unwrap().as_ref().to_vec();
        let key = [7u8; 32];
        let wrapped = wrap_key("share0123456789abcdef", 3, &key, &[1u8; 32], &public).unwrap();
        let inner: Value = serde_json::from_slice(&open_box(&wrapped, viewer, &public)).unwrap();
        assert_eq!(inner["s"], "share0123456789abcdef");
        assert_eq!(inner["e"], 3);
        assert_eq!(STD.decode(inner["k"].as_str().unwrap()).unwrap(), key.to_vec());
        assert_eq!(inner["t"], json!(key_tag(&[1u8; 32], "share0123456789abcdef", 3, &key)));
        // an item opens with the content key
        let sealed = STD.decode(seal_item(&key, &json!({"id": "m-1", "text": "Salut"})).unwrap()).unwrap();
        use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
        let opening = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).unwrap());
        let mut data = sealed[12..].to_vec();
        let plain = opening
            .open_in_place(Nonce::try_assume_unique_for_key(&sealed[..12]).unwrap(), Aad::empty(), &mut data)
            .unwrap();
        assert_eq!(serde_json::from_slice::<Value>(plain).unwrap()["text"], "Salut");
    }

    #[test]
    fn the_person_s_words_and_photos_are_found_and_nothing_they_didn_t_type() {
        assert_eq!(typed_words("  Salut  ").as_deref(), Some("Salut"));
        assert_eq!(
            typed_words(
                "[OUT-OF-BAND USER MESSAGE — sent while you worked]\nEt le musée ?\n[/OUT-OF-BAND USER MESSAGE]"
            )
            .as_deref(),
            Some("Et le musée ?")
        );
        assert_eq!(typed_words("[bipagents:handoff] hand it over"), None);
        assert_eq!(typed_words("[SUBAGENT report] done"), None);
        // the agent's notes never reach a viewer
        let now = "<!--bip:now jeudi 29 octobre 2026, 08:40, Europe/Paris (UTC+01:00)-->";
        let typed = typed_words(&format!("{now}\n<!--bip:call\nVoice call\n-->\n\nOn part ?"));
        assert_eq!(typed.as_deref(), Some("On part ?"));
        assert_eq!(typed_words(&format!("{now}\n<!--bip:watch\nYour watcher\n-->")), None);
        let jpeg = vec![0xffu8, 0xd8, 0xff, 1, 2, 3];
        let message: Dict = json!({"id": 7, "role": "user", "content": [
            {"type": "text", "text": "Regarde"},
            {"type": "image_url", "image_url": {"url": format!("data:image/jpeg;base64,{}", STD.encode(&jpeg))}},
            {"type": "image_url", "image_url": {"url": "https://example.com/x.png"}}]})
        .as_object()
        .unwrap()
        .clone();
        assert_eq!(images_of(&message), vec![("image/jpeg".to_string(), jpeg.clone())]);
        let wanted = photos_wanted(std::slice::from_ref(&message), &Map::new());
        assert_eq!(wanted, HashSet::from(["7".to_string()]));
        let (items, found) = new_photos(std::slice::from_ref(&message), &wanted);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].1["mime"], "image/jpeg");
        assert_eq!(STD.decode(items[0].1["data"].as_str().unwrap()).unwrap(), jpeg);
        assert_eq!(found["7"], json!([items[0].0]));
        assert!(photos_wanted(std::slice::from_ref(&message), &found).is_empty());
        // the message then names its photo
        let mut config = Config::with_key("k");
        config.outbox.db_path = std::env::temp_dir().join("rshare-photos.db").to_string_lossy().into_owned();
        let share = json!({"agent": "bip", "title": "T", "from": "Alex", "ends": 2.0e9, "photos": found});
        let listed = items_of(&config, &share, std::slice::from_ref(&message));
        assert_eq!(listed[1].1["media"], json!([items[0].0]));
        assert_eq!(listed[1].1["text"], "Regarde");
    }

    #[test]
    fn only_the_typed_messages_and_final_replies_go_without_their_media_lines() {
        let dir = std::env::temp_dir().join(format!("rshare-{}", std::process::id()));
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.join("outbox.db").to_string_lossy().into_owned();
        let share = json!({"agent": "bip", "title": "Voyage à Lisbonne", "from": "Alex", "ends": 2.0e9});
        let messages: Vec<Dict> = [
            json!({"role": "user", "content": "On part à Lisbonne ?", "id": 1}),
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": "x"}], "id": 2}),
            json!({"role": "tool", "content": "secret mail", "id": 3}),
            json!({"role": "assistant", "content": "Voici le plan.\nMEDIA:/bipagents/pages/bip/plan.html\nMEDIA:/somewhere/else.pdf", "id": 4}),
        ]
        .into_iter()
        .map(|v| v.as_object().unwrap().clone())
        .collect();
        let items = items_of(&config, &share, &messages);
        let ids: Vec<&str> = items.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["meta", "m-1", "m-4"]);
        assert_eq!(items[2].1["text"], "Voici le plan.");
        assert_eq!(items[0].1["title"], "Voyage à Lisbonne");
        assert!(!items.iter().any(|(_, v)| v.to_string().contains("secret mail")));
        assert_eq!(split_media("a\nMEDIA: /x.png\nb"), ("a\nb".to_string(), vec!["/x.png".to_string()]));
        assert!(
            guichet_ok("https://guichet.bipagents.com")
                && !guichet_ok("http://evil.test")
                && guichet_ok("http://127.0.0.1:9")
        );
        assert!(instructions(4).contains("4 people") && closed_tool("mail_search").is_some());
        assert!(closed_tool(crate::weather::TOOL_NAME).is_none());
    }
}
