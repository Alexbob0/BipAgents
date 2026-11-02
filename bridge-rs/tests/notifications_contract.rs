//! The notifications contract (contracts/notifications/ at the repository's root): for every type and variant, the
//! bridge's builder (push::notices::build) gives exactly the v1 payload written there, Android's FCM data and the push
//! options; the « Me prévenir » switch policy matches; every `notify_*` of push.rs and every `Notice` has its
//! contract file; the files follow schema.json.
//!
//! `UPDATE_CONTRACTS=1 cargo test --test notifications_contract` rewrites `payload_v1`, `fcm_data` and `push` from the
//! code (after a deliberate change of a payload: review the diff, then update the apps' expectations).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use bipbridge::fcm;
use bipbridge::push::notices::{self, Context, Notice, BUILDERS};
use serde_json::{json, Value};

fn contracts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../contracts/notifications")
}

fn type_files() -> Vec<(PathBuf, Value)> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(contracts().join("types"))
        .expect("contracts/notifications/types")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let text = std::fs::read_to_string(&p).unwrap();
            let doc: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            (p, doc)
        })
        .collect()
}

fn built(variant: &Value) -> (Notice, notices::Built) {
    let bridge = &variant["bridge"];
    let notice: Notice = serde_json::from_value(bridge["input"].clone())
        .unwrap_or_else(|e| panic!("{}: input: {e}", variant["name"]));
    let context: Context = serde_json::from_value(bridge["context"].clone()).unwrap();
    let agent = bridge["agent"].as_str().unwrap();
    let built = notices::build(agent, &context, &notice);
    (notice, built)
}

#[test]
fn every_variant_gives_its_payload() {
    let update = std::env::var("UPDATE_CONTRACTS").is_ok_and(|v| !v.is_empty() && v != "0");
    let mut checked = 0;
    for (path, mut doc) in type_files() {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let switch = doc["policy"]["per_agent_switch"].as_bool().unwrap();
        for variant in doc["variants"].as_array_mut().unwrap() {
            let label = format!("{name}/{}", variant["name"].as_str().unwrap());
            let (notice, built) = built(variant);
            assert_eq!(notice.follows_switch(), switch, "{label}: policy.per_agent_switch");
            let payload = Value::Object(built.payload.clone());
            let fcm = Value::Object(fcm::data_message(&payload));
            let push = json!({"push_type": built.push_type, "priority": built.priority, "collapse_id": built.collapse_id});
            if update {
                variant["payload_v1"] = payload;
                variant["fcm_data"] = fcm;
                variant["push"] = push;
            } else {
                assert_eq!(payload, variant["payload_v1"], "{label}: payload_v1");
                assert_eq!(fcm, variant["fcm_data"], "{label}: fcm_data");
                assert_eq!(push, variant["push"], "{label}: push");
            }
            checked += 1;
        }
        if update {
            let text = serde_json::to_string_pretty(&doc).unwrap() + "\n";
            std::fs::write(&path, text).unwrap();
        }
    }
    assert!(checked >= 17, "{checked} variants");
}

/// The `notify_*` names the contract files use.
fn contract_builders() -> BTreeSet<String> {
    type_files()
        .iter()
        .flat_map(|(_, doc)| {
            doc["variants"].as_array().unwrap().iter().map(|v| v["bridge"]["input"]["fn"].as_str().unwrap().to_string())
        })
        .collect()
}

#[test]
fn every_notice_has_a_contract() {
    let used = contract_builders();
    for builder in BUILDERS {
        assert!(used.contains(builder), "{builder} has no variant in contracts/notifications/types (add one)");
    }
    for builder in &used {
        assert!(BUILDERS.contains(&builder.as_str()), "{builder}: no such Notice in push::notices");
    }
}

/// A `notify_*` added to PushService without going through `Notice` (and so without a contract) fails here.
#[test]
fn every_notify_method_has_a_contract() {
    let source = include_str!("../src/push.rs");
    let methods = regex::Regex::new(r"pub async fn (notify_[a-z_]+)\(").unwrap();
    let used = contract_builders();
    let mut found = 0;
    for m in methods.captures_iter(source) {
        let name = &m[1];
        // `notify_approval` is `notify_approval_values` with the choices as strings.
        let name = if name == "notify_approval" { "notify_approval_values" } else { name };
        assert!(used.contains(name), "PushService::{name} has no contract file (contracts/notifications/types)");
        found += 1;
    }
    assert!(found >= 16, "{found} notify_* methods found in push.rs");
}

#[test]
fn the_audit_rows_are_all_there() {
    let files = type_files();
    assert_eq!(files.len(), 17, "one contract file per row 1-17 of the audit");
    let mut rows = BTreeSet::new();
    for (path, doc) in &files {
        assert_eq!(doc["type"].as_str().unwrap(), path.file_stem().unwrap().to_str().unwrap());
        assert!(rows.insert(doc["audit_row"].as_i64().unwrap()), "{}: audit_row twice", path.display());
    }
    assert_eq!(rows, (1..=17).collect());
}

#[test]
fn the_files_follow_the_schema() {
    let schema: Value = serde_json::from_str(&std::fs::read_to_string(contracts().join("schema.json")).unwrap()).unwrap();
    for (path, doc) in type_files() {
        let mut errors = Vec::new();
        validate(&schema, &schema, &doc, "$", &mut errors);
        assert!(errors.is_empty(), "{}:\n{}", path.display(), errors.join("\n"));
    }
}

/// The part of JSON Schema schema.json uses: type, enum, required, properties, additionalProperties, items, minItems,
/// minimum, maximum, pattern, $ref to #/$defs.
fn validate(root: &Value, schema: &Value, value: &Value, at: &str, errors: &mut Vec<String>) {
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        let name = r.strip_prefix("#/$defs/").expect("local $ref");
        return validate(root, &root["$defs"][name], value, at, errors);
    }
    if let Some(kinds) = schema.get("type") {
        let kinds: Vec<&str> = match kinds {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        let fits = kinds.iter().any(|k| match *k {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            _ => false,
        });
        if !fits {
            errors.push(format!("{at}: not {kinds:?}: {value}"));
            return;
        }
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array) {
        if !options.contains(value) {
            errors.push(format!("{at}: {value} not in {}", Value::Array(options.clone())));
        }
    }
    if let (Some(min), Some(n)) = (schema.get("minimum").and_then(Value::as_f64), value.as_f64()) {
        if n < min {
            errors.push(format!("{at}: {n} < {min}"));
        }
    }
    if let (Some(max), Some(n)) = (schema.get("maximum").and_then(Value::as_f64), value.as_f64()) {
        if n > max {
            errors.push(format!("{at}: {n} > {max}"));
        }
    }
    if let (Some(pattern), Some(s)) = (schema.get("pattern").and_then(Value::as_str), value.as_str()) {
        if !regex::Regex::new(pattern).unwrap().is_match(s) {
            errors.push(format!("{at}: {s:?} does not match {pattern}"));
        }
    }
    if let Value::Object(map) = value {
        for key in schema.get("required").and_then(Value::as_array).into_iter().flatten() {
            let key = key.as_str().unwrap();
            if !map.contains_key(key) {
                errors.push(format!("{at}: missing {key}"));
            }
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        for (key, item) in map {
            let here = format!("{at}.{key}");
            match properties.and_then(|p| p.get(key)) {
                Some(sub) => validate(root, sub, item, &here, errors),
                None => match schema.get("additionalProperties") {
                    Some(Value::Bool(false)) => errors.push(format!("{here}: not allowed")),
                    Some(sub @ Value::Object(_)) => validate(root, sub, item, &here, errors),
                    _ => {}
                },
            }
        }
    }
    if let Value::Array(items) = value {
        if let Some(min) = schema.get("minItems").and_then(Value::as_u64) {
            if (items.len() as u64) < min {
                errors.push(format!("{at}: fewer than {min} items"));
            }
        }
        if let Some(sub) = schema.get("items") {
            for (i, item) in items.iter().enumerate() {
                validate(root, sub, item, &format!("{at}[{i}]"), errors);
            }
        }
    }
}
