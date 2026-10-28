//! What a notification may carry through the relay: ids, never what the agent said, nor a title.
//!
//! The bridge's payloads are generic by default; one set to show previews puts a line of the message in the
//! alert's body. The relay replaces any body that is not one of the app's generic, translated texts (`loc-key`)
//! with « Nouveau message », and keeps only short id-like values next to `aps`. The app's notification extension
//! then fetches the real text from the person's own bridge. The alert's title never passes (the apps name the agent
//! from their own list); the guichet's texts (accounts, servers) pass only on a token sealed for the guichet, and
//! `category`, `sound` and `interruption-level` only with the values the apps use.

use serde_json::{Map, Value};

use crate::seal::Audience;

pub const GENERIC_BODY: &str = "Nouveau message";
/// The app's generic, translated texts a bridge sends (its string catalog's keys, bridge-rs push.rs).
const BRIDGE_BODIES: &[&str] = &[
    GENERIC_BODY,
    "Ta réponse est prête.",
    "Une question pour toi",
    "Approbation requise",
    "Sous-tâche terminée",
    "Message de ta famille",
];
/// The guichet's words (a server of the all-included offer, the account, the family, shared conversations): only on
/// a token sealed for the guichet, sent with its key, so a bridge's token can't make them up.
const GUICHET_BODIES: &[&str] = &[
    GENERIC_BODY,
    "Ton serveur ne répond plus : on s’en occupe.",
    "Tes agents ont une nouvelle place : ouvre l’app pour les y remettre.",
    "Ton serveur répond de nouveau.",
    "Ton abonnement ne sera pas renouvelé : pense à utiliser ton crédit d’IA.",
    "Ton abonnement annuel se renouvelle bientôt : la date, le prix et comment l’arrêter sont dans tes e-mails.",
    "Un nouvel appareil veut rejoindre ton compte : approuve-le ou refuse-le dans l’app.",
    "Ton compte est en cours de récupération sur un autre appareil : si ce n’est pas toi, arrête-la dans l’app.",
    "Quelqu’un de ta famille veut relier ses agents aux tiens : accepte ou refuse dans l’app.",
    // a conversation someone shows this person read-only (the guichet's read_shares.rs); the app names it itself
    "Nouveau message dans une conversation partagée",
];
pub const MAX_PAYLOAD_BYTES: usize = 4096;
/// The values the apps use; anything else is dropped (no « time-sensitive », no custom sound).
const CATEGORIES: &[&str] = &["MESSAGE", "APPROVAL"];
const SOUNDS: &[&str] = &["default"];
const INTERRUPTION_LEVELS: &[&str] = &["active", "passive"];
const VALUE_MAX: usize = 256;
const LIST_MAX: usize = 10;
const LIST_ITEM_MAX: usize = 64;
const APS_KEYS: &[&str] = &[
    "alert",
    "badge",
    "sound",
    "thread-id",
    "category",
    "content-available",
    "mutable-content",
    "interruption-level",
    "relevance-score",
];

fn id_like_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 40 && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn alert(value: &Value, aud: Audience) -> Value {
    let mut out = Map::new();
    let bodies = match aud {
        Audience::Bridge => BRIDGE_BODIES,
        Audience::Guichet => GUICHET_BODIES,
    };
    let generic = value.get("loc-key").and_then(Value::as_str).filter(|k| bodies.contains(k));
    let body = generic.unwrap_or(GENERIC_BODY).to_string();
    out.insert("body".into(), Value::String(body.clone()));
    out.insert("loc-key".into(), Value::String(body));
    Value::Object(out)
}

fn custom(value: &Value) -> Option<Value> {
    match value {
        Value::String(s) if s.chars().count() <= VALUE_MAX && !s.contains('\n') => Some(value.clone()),
        Value::Number(_) | Value::Bool(_) => Some(value.clone()),
        Value::Array(items)
            if items.len() <= LIST_MAX
                && items.iter().all(|i| i.as_str().is_some_and(|s| s.chars().count() <= LIST_ITEM_MAX)) =>
        {
            Some(value.clone())
        }
        _ => None,
    }
}

/// The payload as the relay forwards it, or why it refuses it.
pub fn scrub(payload: &Value, aud: Audience) -> Result<Value, &'static str> {
    let object = payload.as_object().ok_or("payload must be an object")?;
    let mut out = Map::new();
    if let Some(aps) = object.get("aps") {
        let aps = aps.as_object().ok_or("aps must be an object")?;
        let mut clean = Map::new();
        for (key, value) in aps {
            if !APS_KEYS.contains(&key.as_str()) {
                continue;
            }
            let allowed = match key.as_str() {
                "category" => Some(CATEGORIES),
                "sound" => Some(SOUNDS),
                "interruption-level" => Some(INTERRUPTION_LEVELS),
                _ => None,
            };
            if let Some(allowed) = allowed {
                if let Some(v) = value.as_str().filter(|v| allowed.contains(v)) {
                    clean.insert(key.clone(), Value::String(v.into()));
                }
            } else if key == "alert" {
                clean.insert(key.clone(), alert(value, aud));
            } else if matches!(value, Value::String(_) | Value::Number(_) | Value::Bool(_)) {
                clean.insert(key.clone(), custom(value).unwrap_or(Value::Null));
            }
        }
        clean.retain(|_, v| !v.is_null());
        out.insert("aps".into(), Value::Object(clean));
    }
    for (key, value) in object {
        if key == "aps" || !id_like_key(key) {
            continue;
        }
        if let Some(kept) = custom(value) {
            out.insert(key.clone(), kept);
        }
    }
    let out = Value::Object(out);
    if serde_json::to_vec(&out).map(|b| b.len()).unwrap_or(usize::MAX) > MAX_PAYLOAD_BYTES {
        return Err("payload too large");
    }
    Ok(out)
}

/// The phases of an agent's work a Live Activity shows (the bridge's live_activities.rs).
const LIVE_PHASES: &[&str] = &["working", "approval", "question", "done", "failed", "stopped"];

/// A Live Activity's update (`push_type: liveactivity`): Apple's timing keys and a `content-state` of codes only — the
/// phase, the running tool's name, a speaking agent's name, counts. The site a browser is on (`detail`, sent by a
/// bridge with previews on) never passes, nor any alert: the title the person sees was set on the phone.
pub fn scrub_live(payload: &Value) -> Result<Value, &'static str> {
    let aps = payload.get("aps").and_then(Value::as_object).ok_or("aps must be an object")?;
    let mut clean = Map::new();
    for key in ["timestamp", "dismissal-date", "stale-date"] {
        if let Some(n) = aps.get(key).and_then(Value::as_i64) {
            clean.insert(key.into(), Value::from(n));
        }
    }
    let event =
        aps.get("event").and_then(Value::as_str).filter(|e| matches!(*e, "update" | "end")).ok_or("invalid event")?;
    clean.insert("event".into(), Value::String(event.into()));
    let state = aps.get("content-state").and_then(Value::as_object).ok_or("content-state must be an object")?;
    let mut kept = Map::new();
    let phase = state.get("phase").and_then(Value::as_str).filter(|p| LIVE_PHASES.contains(p)).unwrap_or("working");
    kept.insert("phase".into(), Value::String(phase.into()));
    if let Some(tool) = state.get("tool").and_then(Value::as_str).filter(|t| t.len() <= 64 && id_like_key(t)) {
        kept.insert("tool".into(), Value::String(tool.into()));
    }
    if let Some(speaker) =
        state.get("speaker").and_then(Value::as_str).filter(|s| s.chars().count() <= 40 && !s.contains('\n'))
    {
        kept.insert("speaker".into(), Value::String(speaker.into()));
    }
    for key in ["subtasks", "subtasksDone"] {
        let n = state.get(key).and_then(Value::as_i64).unwrap_or(0).clamp(0, 999);
        kept.insert(key.into(), Value::from(n));
    }
    clean.insert("content-state".into(), Value::Object(kept));
    let mut out = Map::new();
    out.insert("aps".into(), Value::Object(clean));
    Ok(Value::Object(out))
}

/// An (already scrubbed) APNs payload as Firebase data: strings only, as the Android app reads it.
pub fn fcm_data(payload: &Value) -> Map<String, Value> {
    let mut data = Map::new();
    let aps = payload.get("aps").and_then(Value::as_object);
    let alert = aps.and_then(|a| a.get("alert")).and_then(Value::as_object);
    if let Some(alert) = alert {
        for key in ["title", "body"] {
            if let Some(text) = alert.get(key).and_then(Value::as_str).filter(|t| !t.is_empty()) {
                data.insert(key.into(), Value::String(text.into()));
            }
        }
    }
    if let Some(category) = aps.and_then(|a| a.get("category")).and_then(Value::as_str) {
        data.insert("category".into(), Value::String(category.into()));
    }
    let silent = aps.and_then(|a| a.get("content-available")).is_some_and(|v| v.as_i64() == Some(1) || v == true);
    if silent && alert.is_none() {
        data.insert("silent".into(), Value::String("1".into()));
    }
    if let Some(object) = payload.as_object() {
        for (key, value) in object {
            match value {
                _ if key == "aps" => {}
                Value::Null => {}
                Value::String(s) => {
                    data.insert(key.clone(), Value::String(s.clone()));
                }
                other => {
                    data.insert(key.clone(), Value::String(other.to_string()));
                }
            }
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_preview_never_passes_the_generic_texts_do() {
        let preview = json!({"aps": {"alert": {"title": "Vie", "body": "Ton rendez-vous chez le médecin est à 9h"},
                                     "mutable-content": 1, "category": "MESSAGE", "thread-id": "vie"},
                             "agent": "vie", "outbox_id": "abc123"});
        let clean = scrub(&preview, Audience::Bridge).unwrap();
        assert_eq!(clean["aps"]["alert"], json!({"body": "Nouveau message", "loc-key": "Nouveau message"}));
        assert_eq!(clean["aps"]["category"], "MESSAGE");
        assert_eq!(clean["outbox_id"], "abc123");
        let generic = json!({"aps": {"alert": {"title": "Vie", "body": "Approbation requise", "loc-key": "Approbation requise"}}});
        assert_eq!(scrub(&generic, Audience::Bridge).unwrap()["aps"]["alert"]["body"], "Approbation requise");
        let plain = json!({"aps": {"alert": "Un texte entier"}});
        assert_eq!(scrub(&plain, Audience::Bridge).unwrap()["aps"]["alert"]["body"], GENERIC_BODY);
        // The guichet's word on a server of the all-included offer.
        let down = "Ton serveur ne répond plus : on s’en occupe.";
        let server = json!({"aps": {"alert": {"title": "BipAgents", "body": down, "loc-key": down}}, "kind": "server"});
        let clean = scrub(&server, Audience::Guichet).unwrap();
        assert_eq!(clean["aps"]["alert"], json!({"body": down, "loc-key": down}));
        assert_eq!(clean["kind"], "server");
        // A conversation shown read-only (the guichet's read_shares.rs): the share's id only, the app names it.
        let shared = "Nouveau message dans une conversation partagée";
        let push = json!({"aps": {"alert": {"title": "BipAgents", "body": shared, "loc-key": shared},
                                  "thread-id": "Q2xxZm9vYmFyYmF6cXV4", "mutable-content": 1},
                          "kind": "rshare", "rshare": "Q2xxZm9vYmFyYmF6cXV4"});
        let clean = scrub(&push, Audience::Guichet).unwrap();
        assert_eq!(clean["aps"]["alert"]["body"], shared);
        assert_eq!((clean["kind"].clone(), clean["rshare"].clone()), (json!("rshare"), json!("Q2xxZm9vYmFyYmF6cXV4")));
    }

    #[test]
    fn a_bridges_token_cannot_speak_as_the_guichet_nor_set_a_title() {
        let account = "Un nouvel appareil veut rejoindre ton compte : approuve-le ou refuse-le dans l’app.";
        let forged = json!({"aps": {"alert": {"title": "Sécurité BipAgents", "body": account, "loc-key": account},
                                    "category": "ACCOUNT_ALERT", "sound": "alarm.caf",
                                    "interruption-level": "time-sensitive", "badge": 1}});
        let clean = scrub(&forged, Audience::Bridge).unwrap();
        assert_eq!(clean["aps"], json!({"alert": {"body": GENERIC_BODY, "loc-key": GENERIC_BODY}, "badge": 1}));
        let fine = json!({"aps": {"alert": {"loc-key": "Approbation requise"}, "category": "APPROVAL", "sound": "default",
                                  "interruption-level": "passive"}});
        let clean = scrub(&fine, Audience::Bridge).unwrap();
        assert_eq!(
            (&clean["aps"]["category"], &clean["aps"]["sound"], &clean["aps"]["interruption-level"]),
            (&json!("APPROVAL"), &json!("default"), &json!("passive"))
        );
        assert!(clean["aps"]["alert"].get("title").is_none());
    }

    #[test]
    fn a_live_activity_carries_codes_only() {
        let update = json!({"aps": {"timestamp": 1000, "event": "update", "stale-date": 2800,
            "alert": {"title": "Vie", "body": "secret"},
            "content-state": {"phase": "working", "tool": "browser_navigate", "detail": "kiabi.fr", "speaker": "Coach",
                              "subtasks": 3, "subtasksDone": 1, "note": "secret"}}, "agent": "vie"});
        assert_eq!(
            scrub_live(&update).unwrap(),
            json!({"aps": {"timestamp": 1000, "event": "update", "stale-date": 2800,
                "content-state": {"phase": "working", "tool": "browser_navigate", "speaker": "Coach",
                                  "subtasks": 3, "subtasksDone": 1}}})
        );
        let end = json!({"aps": {"timestamp": 1001, "event": "end", "dismissal-date": 1240,
            "content-state": {"phase": "later", "tool": "a tool with spaces"}}});
        assert_eq!(
            scrub_live(&end).unwrap()["aps"]["content-state"],
            json!({"phase": "working", "subtasks": 0, "subtasksDone": 0})
        );
        assert!(scrub_live(&json!({"aps": {"event": "start", "content-state": {}}})).is_err());
        assert!(scrub_live(&json!({"aps": {"event": "update"}})).is_err());
    }

    #[test]
    fn only_short_ids_travel_next_to_aps() {
        let payload = json!({"aps": {"content-available": 1, "unknown": "x"}, "agent": "vie",
                             "choices": ["once", "deny"], "note": "line one\nline two", "big": "x".repeat(300),
                             "nested": {"text": "secret"}, "bad key!": "x", "count": 3});
        let clean = scrub(&payload, Audience::Bridge).unwrap();
        assert_eq!(
            clean,
            json!({"aps": {"content-available": 1}, "agent": "vie", "choices": ["once", "deny"], "count": 3})
        );
        assert!(scrub(&json!("text"), Audience::Bridge).is_err());
    }

    #[test]
    fn firebase_gets_strings_as_the_android_app_reads_them() {
        let payload = scrub(
            &json!({"aps": {"alert": {"title": "Vie", "body": "x"}, "category": "APPROVAL"},
                                    "agent": "vie", "run_id": "r1", "choices": ["once", "deny"]}),
            Audience::Bridge,
        )
        .unwrap();
        let data = fcm_data(&payload);
        assert!(!data.contains_key("title"));
        assert_eq!(data["body"], GENERIC_BODY);
        assert_eq!(data["category"], "APPROVAL");
        assert_eq!(data["choices"], "[\"once\",\"deny\"]");
        let silent = fcm_data(&json!({"aps": {"content-available": 1}, "agent": "vie"}));
        assert_eq!(silent["silent"], "1");
    }
}
