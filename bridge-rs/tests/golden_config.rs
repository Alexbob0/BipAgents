//! config.parse_config against the Python's own results (tests/golden/config.json, made by gen_config.py), and
//! Python's tests/test_config.py.

use bipbridge::config::{load_config, parse_config, ConfigError};
use serde_json::Value;

fn with_home(value: &Value, home: &str) -> Value {
    match value {
        Value::String(s) => Value::String(s.replace("$HOME", home)),
        Value::Array(items) => Value::Array(items.iter().map(|v| with_home(v, home)).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), with_home(v, home))).collect()),
        other => other.clone(),
    }
}

#[test]
fn parse_config_matches_python() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("golden/config.json")).unwrap();
    let home = bipbridge::py::expanduser("~");
    let mut failures = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let got = parse_config(&case["input"], None);
        match (got, case.get("config"), case.get("error")) {
            (Ok(config), Some(expected), _) => {
                let got = serde_json::to_value(&config).unwrap();
                let expected = with_home(expected, &home);
                if got != expected {
                    failures.push(format!("case {i}:\n  got      {got}\n  expected {expected}"));
                }
            }
            (Err(ConfigError(message)), _, Some(expected)) => {
                if message != expected.as_str().unwrap() {
                    failures.push(format!("case {i}: error {message:?}, expected {expected}"));
                }
            }
            (Ok(_), _, expected) => failures.push(format!("case {i}: no error, expected {expected:?}")),
            (Err(e), _, _) => failures.push(format!("case {i}: unexpected error {e}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const EXAMPLE: &str = include_str!("../../bridge/bridge.example.toml");

#[test]
fn example_config_parses_but_refuses_placeholders() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bridge.toml");
    std::fs::write(&path, EXAMPLE).unwrap();
    let err = load_config(Some(path.to_str().unwrap())).unwrap_err();
    assert!(err.0.contains("placeholder"), "{err}");
}

#[test]
fn example_config_with_real_values() {
    let mut text = EXAMPLE.replace("\"CHANGE-ME-run-bipbridge-genkey\"", &format!("\"k{}\"", "x".repeat(40)));
    for name in ["wellness", "vie"] {
        text = text.replace(&format!("\"CHANGE-ME-API_SERVER_KEY-of-{name}\""), &format!("\"key-{name}\""));
    }
    text = text.replace("\"CHANGE-ME-tk_token-with-read-access\"", "\"tk_abc\"");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bridge.toml");
    std::fs::write(&path, text).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    // load_config() reads $BRIDGE_CONFIG; set_var would race with other tests: give the path instead.
    let cfg = load_config(Some(path.to_str().unwrap())).unwrap();
    assert_eq!((cfg.port, cfg.host.as_str()), (8643, "127.0.0.1"));
    assert_eq!(cfg.kyutai_url.as_deref(), Some("http://127.0.0.1:8097"));
    assert_eq!(cfg.ntfy_url, "http://127.0.0.1:8645");
    assert_eq!(cfg.apns.bundle_id, "com.kerfystudio.bipagents");
    assert!(!cfg.apns.enabled());
    let mut names: Vec<&String> = cfg.agents.keys().collect();
    names.sort();
    assert_eq!(names, ["vie", "wellness"]);
    assert_eq!(cfg.agents["vie"].hermes_url, "http://127.0.0.1:8644");
    assert_eq!(
        cfg.agents["wellness"].upload_dir_container.as_deref(),
        Some("/home/hermes/.hermes/profiles/wellness/uploads")
    );
    assert_eq!(cfg.limits.upload_max_bytes, 50 * 1024 * 1024);
    assert_eq!(cfg.limits.upload_file_mode, 0o640);
    assert_eq!(cfg.source_path.as_deref(), Some(path.to_str().unwrap()));
}

#[test]
fn secret_files_and_lowercase_agents() {
    let dir = tempfile::tempdir().unwrap();
    let key_file = dir.path().join("hermes.key");
    std::fs::write(&key_file, "from-file\n").unwrap();
    let cfg = parse_config(
        &serde_json::json!({"bridge_key": "b".repeat(40),
                            "agents": {"Vie": {"hermes_url": "http://h/", "hermes_key_file": key_file.to_str().unwrap()}}}),
        None,
    )
    .unwrap();
    assert_eq!(cfg.agent(Some("VIE")).unwrap().hermes_key, "from-file");
    assert_eq!(cfg.agent(Some("vie")).unwrap().hermes_url, "http://h");
    assert_eq!(cfg.agent(Some("vie")).unwrap().display_name, "Vie");
    let missing = parse_config(
        &serde_json::json!({"bridge_key": "b".repeat(40), "agents": {"v": {"hermes_url": "http://h", "hermes_key_file": "/no/such/key"}}}),
        None,
    )
    .unwrap_err();
    assert_eq!(missing.0, "agents.v.hermes_key_file: cannot read /no/such/key: [Errno 2] No such file or directory");
}

#[test]
fn missing_file_and_bad_toml() {
    let err = load_config(Some("/no/such/bridge.toml")).unwrap_err();
    assert_eq!(err.0, "config file not found: /no/such/bridge.toml");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.toml");
    std::fs::write(&path, "bridge_key = ").unwrap();
    assert!(load_config(Some(path.to_str().unwrap()))
        .unwrap_err()
        .0
        .starts_with(&format!("invalid TOML in {}", path.display())));
}
