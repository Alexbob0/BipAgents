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
