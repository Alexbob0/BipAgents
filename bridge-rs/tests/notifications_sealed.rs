//! Sealed notifications (refonte phase 5, push_runs/seal.rs) against contracts/notifications/sealed.json, the file the
//! iPhone and Android apps' tests read too:
//!
//! - `vectors`: boxes made for the test device key, each opening (here with the RustCrypto P-256, the phones with
//!   CryptoKit / javax) to its plaintext with its `agent` as additional data; `bad`: boxes that must not open;
//! - `payloads`: for every variant of contracts/notifications/types, the push a device with the test key gets: ids as
//!   in the clear push, the neutral line, `enc` opening to `shown`; nothing the agent said in clear; within the size
//!   limits, also for a very long text;
//! - `loc-keys.json`: one neutral line per type of `seal::TYPES`.
//!
//! `UPDATE_SEALED=1 cargo test --test notifications_sealed` writes sealed.json again from the code (new random boxes).

use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD as STD;
use base64::Engine;
use bipbridge::fcm;
use bipbridge::push::notices::{self, Context, Notice};
use bipbridge::push::seal::{self, Sealed};
use hmac::{Hmac, Mac};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

fn contracts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../contracts/notifications")
}

fn update() -> bool {
    std::env::var("UPDATE_SEALED").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// The test device's key: SHA-256 of « bipagents-notif-test-key » as the private scalar (never a real phone's).
fn test_key() -> SecretKey {
    SecretKey::from_slice(&Sha256::digest(b"bipagents-notif-test-key")).unwrap()
}

fn test_public() -> Vec<u8> {
    test_key().public_key().to_encoded_point(false).as_bytes().to_vec()
}

fn hkdf(ikm: &[u8], salt: &[u8], info: &[u8]) -> [u8; 32] {
    let mut extract = <Hmac<Sha256> as Mac>::new_from_slice(salt).unwrap();
    extract.update(ikm);
    let prk = extract.finalize().into_bytes();
    let mut expand = <Hmac<Sha256> as Mac>::new_from_slice(&prk).unwrap();
    expand.update(info);
    expand.update(&[1]);
    expand.finalize().into_bytes().into()
}

/// What a phone does: the box opened with its private key, `agent` as additional data.
fn open(enc: &str, agent: &str, key: &SecretKey) -> Result<Vec<u8>, String> {
    let raw = STD.decode(enc).map_err(|e| e.to_string())?;
    if raw.len() < seal::OVERHEAD {
        return Err("short".into());
    }
    let ephemeral = PublicKey::from_sec1_bytes(&raw[..65]).map_err(|e| e.to_string())?;
    let shared = p256::ecdh::diffie_hellman(key.to_nonzero_scalar(), ephemeral.as_affine());
    let mine = key.public_key().to_encoded_point(false);
    let k = hkdf(shared.raw_secret_bytes(), &[&raw[..65], mine.as_bytes()].concat(), seal::INFO);
    Aes256Gcm::new_from_slice(&k)
        .unwrap()
        .decrypt(Nonce::from_slice(&raw[65..77]), Payload { msg: &raw[77..], aad: agent.as_bytes() })
        .map_err(|_| "does not open".to_string())
}

fn read(name: &str) -> Value {
    let text = std::fs::read_to_string(contracts().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    serde_json::from_str(&text).unwrap()
}

fn variants() -> Vec<(String, Value)> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(contracts().join("types"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        for variant in doc["variants"].as_array().unwrap() {
            out.push((format!("{name}/{}", variant["name"].as_str().unwrap()), variant["bridge"].clone()));
        }
    }
    out
}

/// The variant's clear push, what it says, and the push for the test device (`None`: a silent push, never sealed).
fn sealed_push(bridge: &Value) -> (String, notices::Built, Option<(Sealed, Map<String, Value>)>) {
    let notice: Notice = serde_json::from_value(bridge["input"].clone()).unwrap();
    let context: Context = serde_json::from_value(bridge["context"].clone()).unwrap();
    let agent = bridge["agent"].as_str().unwrap().to_string();
    let built = notices::build(&agent, &context, &notice);
    let sealed = notices::sealed(&context, &notice)
        .map(|s| {
            let out = seal::for_device(&built.payload, &s, &agent, &test_public()).unwrap_or_else(|e| {
                panic!("{e}: clear push {} bytes: {}", serde_json::to_vec(&built.payload).unwrap().len(),
                    serde_json::to_string(&built.payload).unwrap().chars().take(400).collect::<String>())
            });
            (s, out)
        });
    (agent, built, sealed)
}

fn shown(sealed: &Sealed) -> Value {
    serde_json::from_slice(&sealed.plaintext(seal::BODY_MAX)).unwrap()
}

fn without_enc(payload: &Map<String, Value>) -> Value {
    let mut clear = payload.clone();
    clear.remove("enc");
    Value::Object(clear)
}

#[test]
fn every_type_has_its_neutral_line() {
    let keys = read("loc-keys.json");
    let keys = keys["keys"].as_object().unwrap();
    let ours: Vec<String> = seal::TYPES.iter().map(|t| seal::loc_key(t)).collect();
    let theirs: Vec<String> = keys.keys().cloned().collect();
    assert_eq!(ours, theirs, "contracts/notifications/loc-keys.json = seal::TYPES (same order)");
    for (key, texts) in keys {
        for lang in ["fr", "en", "de", "es"] {
            assert!(texts[lang].as_str().is_some_and(|t| !t.is_empty()), "{key}: no {lang}");
        }
    }
}

#[test]
fn the_vectors_open_and_the_bad_ones_dont() {
    if update() {
        return; // being written by every_variant_is_sealed_as_the_contract_says
    }
    let doc = read("sealed.json");
    let key = test_key();
    assert_eq!(doc["test_key"]["public"].as_str().unwrap(), STD.encode(test_public()));
    assert_eq!(doc["test_key"]["private_hex"].as_str().unwrap(), hex::encode(key.to_bytes()));
    let vectors = doc["vectors"].as_array().unwrap();
    assert!(vectors.len() >= 3);
    for v in vectors {
        let plain = open(v["enc"].as_str().unwrap(), v["agent"].as_str().unwrap(), &key)
            .unwrap_or_else(|e| panic!("{}: {e}", v["name"]));
        assert_eq!(String::from_utf8(plain).unwrap(), v["plaintext"].as_str().unwrap(), "{}", v["name"]);
    }
    for v in doc["bad"].as_array().unwrap() {
        assert!(open(v["enc"].as_str().unwrap(), v["agent"].as_str().unwrap(), &key).is_err(), "{} opened", v["name"]);
    }
}

#[test]
fn a_box_made_now_opens_with_the_test_key_only() {
    let enc = seal::seal(b"{\"v\":1}", &test_public(), b"vie").unwrap();
    assert_eq!(open(&enc, "vie", &test_key()).unwrap(), b"{\"v\":1}");
    assert!(open(&enc, "wellness", &test_key()).is_err(), "another agent's push");
    let other = SecretKey::from_slice(&Sha256::digest(b"another phone")).unwrap();
    assert!(open(&enc, "vie", &other).is_err(), "another phone");
}

#[test]
fn every_variant_is_sealed_as_the_contract_says() {
    let doc = if update() { json!({"payloads": []}) } else { read("sealed.json") };
    let stored: Map<String, Value> = doc["payloads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["variant"].as_str().unwrap().to_string(), p.clone()))
        .collect();
    let mut sealed_count = 0;
    let mut largest = 0;
    let mut payloads = Vec::new();
    for (label, bridge) in variants() {
        let (agent, built, sealed) = sealed_push(&bridge);
        let Some((sealed, out)) = sealed else {
            assert_eq!(built.push_type, "background", "{label}: only silent pushes are not sealed");
            assert!(!stored.contains_key(&label), "{label}: a silent push in sealed.json");
            continue;
        };
        sealed_count += 1;
        // ids as they were; the alert neutral; no title, no words
        let code = seal::loc_key(sealed.kind);
        assert_eq!(out["aps"]["alert"], json!({"loc-key": code, "body": code}), "{label}");
        for (key, value) in &built.payload {
            if key != "aps" {
                assert_eq!(out.get(key), Some(value), "{label}: {key} kept");
            }
        }
        for (key, value) in built.payload["aps"].as_object().unwrap() {
            if key != "alert" {
                assert_eq!(&out["aps"][key], value, "{label}: aps.{key} kept");
            }
        }
        let clear = serde_json::to_string(&without_enc(&out)).unwrap();
        for words in [sealed.body.as_str(), sealed.title.as_str()] {
            for word in words.split_whitespace().filter(|w| w.chars().count() >= 5) {
                assert!(!clear.contains(word), "{label}: {word:?} in clear: {clear}");
            }
        }
        // the phone's view
        let plain = open(out["enc"].as_str().unwrap(), &agent, &test_key()).unwrap();
        let plain: Value = serde_json::from_slice(&plain).unwrap();
        assert_eq!(plain, shown(&sealed), "{label}");
        assert_eq!(plain["t"].as_str().unwrap(), sealed.kind);
        // Firebase's data: the code as body, `enc` as it is
        let data = fcm::data_message(&Value::Object(out.clone()));
        assert_eq!(data["body"], json!(code), "{label}");
        assert_eq!(data["enc"], out["enc"], "{label}");
        assert!(!data.contains_key("title"), "{label}: a title in Firebase's data");
        // sizes
        let size = serde_json::to_vec(&out).unwrap().len();
        largest = largest.max(size);
        assert!(size <= seal::PAYLOAD_MAX, "{label}: {size} bytes");
        if update() {
            payloads.push(json!({"variant": label, "agent": agent, "payload": Value::Object(out), "shown": plain}));
        } else {
            let kept = stored.get(&label).unwrap_or_else(|| panic!("{label}: not in sealed.json (UPDATE_SEALED=1)"));
            assert_eq!(kept["agent"], json!(agent), "{label}");
            let kept_payload = kept["payload"].as_object().unwrap();
            assert_eq!(without_enc(kept_payload), without_enc(&out), "{label}: clear part");
            assert_eq!(kept["shown"], plain, "{label}: shown");
            let opened = open(kept_payload["enc"].as_str().unwrap(), &agent, &test_key()).unwrap();
            assert_eq!(serde_json::from_slice::<Value>(&opened).unwrap(), plain, "{label}: stored enc");
        }
    }
    assert!(sealed_count >= 30, "{sealed_count} sealed variants");
    eprintln!("sealed variants: {sealed_count}, largest push {largest} bytes");
    if update() {
        write(payloads);
    } else {
        assert_eq!(stored.len(), sealed_count, "sealed.json has variants the contract no longer has");
    }
}

/// Every variant with a text far too long, in characters of 1 to 4 bytes: still within APNs' and Firebase's limits.
#[test]
fn every_variant_fits_with_a_huge_text() {
    let mut report = Vec::new();
    for (label, bridge) in variants() {
        for filler in ["a", "é", "€", "👋", "\"\n"] {
            let mut bridge = bridge.clone();
            if let Some(Value::Object(args)) = bridge["input"].get_mut("args") {
                for key in ["text", "line", "question", "command", "room_title", "to", "host"] {
                    if args.get(key).is_some_and(Value::is_string) {
                        args.insert(key.into(), json!(filler.repeat(4000)));
                    }
                }
                for key in ["session_id", "run_id", "request_id", "reply_id", "outbox_id", "job_id", "id", "held_id"] {
                    if args.get(key).is_some_and(Value::is_string) {
                        args.insert(key.into(), json!("x".repeat(64)));
                    }
                }
            }
            if let Some(ctx) = bridge["context"].as_object_mut() {
                ctx.insert("title".into(), json!("Ç".repeat(60)));
            }
            let (agent, _, sealed) = sealed_push(&bridge);
            let Some((sealed, out)) = sealed else { continue };
            let size = serde_json::to_vec(&out).unwrap().len();
            let enc = out["enc"].as_str().unwrap().len();
            let data = fcm::data_message(&Value::Object(out.clone()));
            let fcm_size: usize = data.iter().map(|(k, v)| k.len() + v.as_str().unwrap().len()).sum();
            assert!(size <= seal::PAYLOAD_MAX, "{label} {filler:?}: {size} bytes");
            assert!(enc <= seal::ENC_MAX, "{label} {filler:?}: enc {enc}");
            assert!(fcm_size <= 4000, "{label} {filler:?}: Firebase data {fcm_size} bytes");
            let plain: Value = serde_json::from_slice(&open(&out["enc"].as_str().unwrap(), &agent, &test_key()).unwrap()).unwrap();
            if !sealed.body.is_empty() {
                assert!(plain["body"].as_str().unwrap().chars().count() >= 100, "{label} {filler:?}: body cut too short");
            }
            if filler == "👋" || filler == "a" {
                report.push(format!("{label} {filler}: push {size} B, enc {enc}, body {} chars", plain["body"].as_str().unwrap().chars().count()));
            }
        }
    }
    eprintln!("{}", report.join("\n"));
}

fn write(payloads: Vec<Value>) {
    let key = test_key();
    let public = test_public();
    let mut vectors = Vec::new();
    for (name, agent, plaintext) in [
        ("reply", "vie", r#"{"v":1,"t":"reply","title":"Vie","body":"Le train de 9 h part du quai 3."}"#),
        ("accents_emoji", "wellness", r#"{"v":1,"t":"question","title":"Bien-être","body":"Tu préfères 7 h ou 8 h ? 🌅"}"#),
        ("shared_agent", "cuisine__alex", r#"{"v":1,"t":"approval","title":"Cuisine (Alex)","body":"Cuisine veut commander les courses (32 €)."}"#),
        ("boite_sub", "vie", r#"{"v":1,"t":"boite","title":"Vie","body":"Ton point du matin est prêt.","sub":"Point du matin"}"#),
        ("empty", "vie", ""),
    ] {
        let enc = seal::seal(plaintext.as_bytes(), &public, agent.as_bytes()).unwrap();
        vectors.push(json!({"name": name, "agent": agent, "plaintext": plaintext, "enc": enc}));
    }
    let good = vectors[0]["enc"].as_str().unwrap().to_string();
    let mut raw = STD.decode(&good).unwrap();
    let last = raw.len() - 1;
    raw[last] ^= 1;
    let tag = STD.encode(&raw);
    let mut raw = STD.decode(&good).unwrap();
    raw[80] ^= 1;
    let body = STD.encode(&raw);
    let other = SecretKey::from_slice(&Sha256::digest(b"another phone")).unwrap();
    let other = other.public_key().to_encoded_point(false);
    let elsewhere = seal::seal(b"{\"v\":1}", other.as_bytes(), b"vie").unwrap();
    let bad = json!([
        {"name": "tag_changed", "agent": "vie", "enc": tag},
        {"name": "ciphertext_changed", "agent": "vie", "enc": body},
        {"name": "other_agent", "agent": "wellness", "enc": good},
        {"name": "other_device", "agent": "vie", "enc": elsewhere},
        {"name": "too_short", "agent": "vie", "enc": STD.encode([4u8; 40])},
    ]);
    let doc = json!({
        "about": "Sealed notifications (refonte phase 5; bridge-rs/src/push_runs/seal.rs). Checked by bridge-rs/tests/notifications_sealed.rs, HermesKitTests/NotificationSealTests.swift and android …/push/PushSealTest.kt. UPDATE_SEALED=1 cargo test --test notifications_sealed writes it again.",
        "scheme": {
            "key": "the device's P-256 key pair; POST /v1/devices {push_key: base64 of the public key, uncompressed (65 bytes)}",
            "enc": "base64( E (65, uncompressed P-256 ephemeral public key) · nonce (12) · AES-256-GCM ciphertext · tag (16) )",
            "aes_key": "HKDF-SHA256(ikm = ECDH(e, device) x-coordinate (32), salt = E · device public key (65+65), info = \"bipagents-notif-v1\", length 32)",
            "aad": "the push's `agent` value, UTF-8",
            "plaintext": "JSON {v: 1, t: <type, loc-keys.json without notif.>, title, body (≤ 1000 characters, may be empty: show the neutral line), sub?}",
            "clear": "aps.alert = {loc-key: notif.<t>, body: notif.<t>}, no title; ids next to aps unchanged",
            "limits": {"enc_max_chars": seal::ENC_MAX, "payload_max_bytes": seal::PAYLOAD_MAX, "body_max_chars": seal::BODY_MAX}
        },
        "test_key": {"private_hex": hex::encode(key.to_bytes()), "public": STD.encode(&public)},
        "vectors": vectors,
        "bad": bad,
        "payloads": payloads,
    });
    std::fs::write(contracts().join("sealed.json"), serde_json::to_string_pretty(&doc).unwrap() + "\n").unwrap();
}
