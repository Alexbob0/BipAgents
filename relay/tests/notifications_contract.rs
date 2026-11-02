//! The notifications contract (contracts/notifications/ at the repository's root): every payload a bridge sends today
//! passes the relay (bridge audience) with the generic body the contract says (`relay.body`), never a title nor a
//! preview, and keeps every id the apps route by; the FCM data the relay makes carries the same ids as the bridge's own.

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
fn every_bridge_payload_passes_with_its_generic_body_and_its_ids() {
    let all = variants();
    assert!(all.len() >= 30, "{} variants", all.len());
    for (label, variant) in all {
        let payload = &variant["payload_v1"];
        assert!(payload.is_object(), "{label}: payload_v1 missing (UPDATE_CONTRACTS=1 on the bridge's test)");
        let clean = scrub(payload, Audience::Bridge).unwrap_or_else(|e| panic!("{label}: refused: {e}"));
        let alert = &clean["aps"]["alert"];
        match variant["relay"]["body"].as_str() {
            Some(body) => {
                assert_eq!(alert["body"].as_str(), Some(body), "{label}: relay.body");
                assert!(alert.get("title").is_none(), "{label}: a title passed");
            }
            None => assert!(alert.is_null(), "{label}: a silent push got an alert"),
        }
        for (key, value) in payload.as_object().unwrap() {
            if key != "aps" {
                assert_eq!(&clean[key], value, "{label}: {key} lost on the way");
            }
        }
        // Android through the relay: the same ids and category as from the bridge's own FCM sender. (A list such
        // as `choices` is written `["once","deny"]` here, `["once", "deny"]` by the bridge: Android doesn't read it.)
        let data = fcm_data(&clean);
        for (key, value) in variant["fcm_data"].as_object().unwrap() {
            let listed = payload.get(key).is_some_and(|v| v.is_array() || v.is_object());
            if key != "title" && key != "body" && !listed {
                assert_eq!(data.get(key), Some(value), "{label}: fcm {key}");
            }
        }
    }
}
