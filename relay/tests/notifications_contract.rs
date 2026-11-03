//! The notifications contract (contracts/notifications/ at the repository's root): every v2 payload a bridge sends
//! passes the relay (bridge audience) with its neutral line (`loc-key`), never a title nor words, and keeps every id
//! the apps route by; the FCM data the relay makes carries the same ids.

use std::path::Path;

use bipagents_relay::payload::{fcm_data, scrub};
use bipagents_relay::seal::Audience;
use serde_json::Value;

fn variants() -> Vec<(String, Value)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../contracts/notifications/types");
    let mut files: Vec<_> =
        std::fs::read_dir(&dir).expect("contracts/notifications/types").map(|e| e.unwrap().path()).collect();
    files.sort();
    let mut out = Vec::new();
    for path in files.into_iter().filter(|p| p.extension().is_some_and(|e| e == "json")) {
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        for variant in doc["variants"].as_array().unwrap() {
            out.push((format!("{name}/{}", variant["name"].as_str().unwrap()), variant.clone()));
        }
    }
    out
}

#[test]
fn every_bridge_payload_passes_with_its_neutral_line_and_its_ids() {
    let all = variants();
    assert!(all.len() >= 30, "{} variants", all.len());
    for (label, variant) in all {
        let payload = &variant["payload"];
        assert!(payload.is_object(), "{label}: payload missing (UPDATE_CONTRACTS=1 on the bridge's test)");
        let clean = scrub(payload, Audience::Bridge).unwrap_or_else(|e| panic!("{label}: refused: {e}"));
        let alert = &clean["aps"]["alert"];
        match payload["aps"]["alert"]["loc-key"].as_str() {
            Some(code) => {
                assert_eq!(alert["body"].as_str(), Some(code), "{label}: neutral line");
                assert!(alert.get("title").is_none(), "{label}: a title passed");
            }
            None => assert!(alert.is_null(), "{label}: a silent push got an alert"),
        }
        for (key, value) in payload.as_object().unwrap() {
            if key != "aps" {
                assert_eq!(&clean[key], value, "{label}: {key} lost on the way");
            }
        }
        // Android through the relay: the pointer's ids as the bridge gave them.
        let data = fcm_data(&clean);
        for (key, value) in payload.as_object().unwrap() {
            if let Some(text) = value.as_str() {
                assert_eq!(data.get(key).and_then(Value::as_str), Some(text), "{label}: fcm {key}");
            }
        }
    }
}

/// Sealed notifications (contracts/notifications/sealed.json, made by the bridge's seal.rs): every variant's push for
/// a device with a key passes with `enc` untouched, its neutral line, its ids, and no words of the agent.
#[test]
fn every_sealed_payload_passes_untouched() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../contracts/notifications/sealed.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("sealed.json")).unwrap();
    let payloads = doc["payloads"].as_array().unwrap();
    assert!(payloads.len() >= 30, "{} sealed payloads", payloads.len());
    for item in payloads {
        let label = item["variant"].as_str().unwrap();
        let payload = &item["payload"];
        let clean = scrub(payload, Audience::Bridge).unwrap_or_else(|e| panic!("{label}: refused: {e}"));
        assert_eq!(clean["enc"], payload["enc"], "{label}: enc changed");
        let code = payload["aps"]["alert"]["loc-key"].as_str().unwrap();
        assert!(code.starts_with("notif."), "{label}");
        assert_eq!(clean["aps"]["alert"]["body"].as_str(), Some(code), "{label}: neutral line");
        for (key, value) in payload.as_object().unwrap() {
            if key != "aps" {
                assert_eq!(&clean[key], value, "{label}: {key} lost on the way");
            }
        }
        let data = fcm_data(&clean);
        assert_eq!(data.get("enc"), payload.get("enc"), "{label}: fcm enc");
        let body = item["shown"]["body"].as_str().unwrap();
        let clear = serde_json::to_string(&clean).unwrap().replace(payload["enc"].as_str().unwrap(), "");
        for word in body.split_whitespace().filter(|w| w.chars().count() >= 5) {
            assert!(!clear.contains(word), "{label}: {word:?} in clear");
        }
    }
}
