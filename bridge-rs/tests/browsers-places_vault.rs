//! tests/test_vault.py: the person's credentials, given only to agents that can hold them, never read back.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};

use bipbridge::config::{parse_config, Config};
use bipbridge::vault::{normalize_origin, Vault, VaultError, VaultRun};
use bipbridge::Dict;
use common::{auth, config_dict, TestApp, Urls};
use serde_json::{json, Value};

type Calls = Arc<Mutex<Vec<(String, Dict)>>>;

fn setup(dir: &std::path::Path) -> (Config, Vault, Calls) {
    let mut data = config_dict(dir, &Urls::default());
    data["agents"]["Vie"] = json!({"hermes_url": "http://127.0.0.1:8641", "hermes_key": "k".repeat(32), "isolated": true,
                                   "python": "podman exec -i hermes-vie /opt/hermes/venv/bin/python",
                                   "hermes_home": "/home/hermes/.hermes"});
    data["agents"]["Budget"] = json!({"hermes_url": "http://127.0.0.1:8646/p/budget", "hermes_key": "b".repeat(32)});
    // the agents cannot read the bridge's files
    data["hermes"] = json!({"isolated": true, "bin": "/opt/hermes", "hub_url": "http://127.0.0.1:8646"});
    let config = parse_config(&data, None).unwrap();
    let calls: Calls = Arc::default();
    let seen = calls.clone();
    let run: VaultRun = Arc::new(move |agent, request: Dict| {
        let mut calls = seen.lock().unwrap();
        calls.push((agent.name.clone(), request.clone()));
        let answer = if request.get("op") == Some(&json!("add")) {
            json!({"id": format!("vault_{}", calls.len())})
        } else {
            json!({"removed": true})
        };
        Box::pin(async move { Ok(answer.as_object().unwrap().clone()) })
    });
    let vault = Vault::with_run(&dir.join("vault").to_string_lossy(), run);
    (config, vault, calls)
}

fn login(extra: Value) -> Dict {
    let mut body = json!({"label": "SNCF", "site": "sncf-connect.com/login", "identifier": "alex@example.org",
                          "password": "s3cret-Pass!", "otp_secret": "JBSWY3DPEHPK3PXP"});
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    body.as_object().unwrap().clone()
}

fn ops(calls: &Calls) -> Vec<(String, String)> {
    calls.lock().unwrap().iter().map(|(n, r)| (n.clone(), r["op"].as_str().unwrap().to_string())).collect()
}

#[tokio::test]
async fn items_reach_only_the_agents_allowed_and_never_come_back() {
    let dir = tempfile::tempdir().unwrap();
    let (config, vault, calls) = setup(dir.path());
    let item = vault.create(&config, &login(json!({"agents": ["vie"]}))).await.unwrap();
    assert_eq!(item.origin.as_deref(), Some("https://sncf-connect.com"));
    assert!(item.has_otp);
    assert_eq!(item.agents.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect::<Vec<_>>(), vec![("vie", "vault_1")]);
    let (name, request) = calls.lock().unwrap()[0].clone();
    assert_eq!(name, "vie");
    assert_eq!(request["op"], "add");
    assert_eq!(request["kind"], "login");
    assert_eq!(
        request["secret"],
        json!({"password": "s3cret-Pass!", "otp_secret": "JBSWY3DPEHPK3PXP",
                                         "identifier_type": "email", "identifier": "alex@example.org"})
    );
    assert_eq!(config.agent(Some("vie")).unwrap().python[..3], ["podman", "exec", "-i"]);
    // The app only ever reads metadata; on disk, the secret is encrypted.
    let public = vault.items()[0].public();
    assert!(public.get("secret").is_none() && !public.to_string().contains("s3cret"));
    assert_eq!(public["agents"], json!(["vie"]));
    let blob = std::fs::read(dir.path().join("vault/vault.json.enc")).unwrap();
    assert!(!blob.windows(6).any(|w| w == b"s3cret"));
    let mode = std::fs::metadata(dir.path().join("vault/vault.key")).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[tokio::test]
async fn a_shared_hub_agent_cannot_hold_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let (config, vault, calls) = setup(dir.path());
    let err = vault.create(&config, &login(json!({"agents": ["vie", "budget"]}))).await.unwrap_err();
    match err.downcast_ref::<VaultError>() {
        Some(VaultError::CannotHold(reasons)) => {
            assert_eq!(
                reasons.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect::<Vec<_>>(),
                vec![("budget", "needs_own_space")]
            )
        }
        other => panic!("{other:?}"),
    }
    assert!(calls.lock().unwrap().is_empty() && vault.items().is_empty());
}

#[tokio::test]
async fn update_rewrites_the_item_and_follows_the_agents() {
    let dir = tempfile::tempdir().unwrap();
    let (config, vault, calls) = setup(dir.path());
    let item = vault.create(&config, &login(json!({"agents": ["vie"]}))).await.unwrap();
    calls.lock().unwrap().clear();
    let body = |v: Value| v.as_object().unwrap().clone();
    // Only the agents change: nothing to rewrite for Vie.
    let same = vault.update(&config, &item.id, &body(json!({"agents": ["vie"]}))).await.unwrap();
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(same.agents.get("vie").map(String::as_str), Some("vault_1"));
    // A new password: taken back from Vie, then given again; the password left out of later edits is kept.
    vault.update(&config, &item.id, &body(json!({"password": "n3w"}))).await.unwrap();
    assert_eq!(ops(&calls), vec![("vie".into(), "remove".into()), ("vie".into(), "add".into())]);
    assert_eq!(calls.lock().unwrap()[1].1["secret"]["password"], "n3w");
    calls.lock().unwrap().clear();
    vault.update(&config, &item.id, &body(json!({"label": "SNCF Connect"}))).await.unwrap();
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls[1].1["secret"]["password"], "n3w");
        assert_eq!(calls[1].1["label"], "SNCF Connect");
    }
    calls.lock().unwrap().clear();
    vault.update(&config, &item.id, &body(json!({"agents": []}))).await.unwrap();
    assert_eq!(ops(&calls), vec![("vie".into(), "remove".into())]);
    assert!(vault.items()[0].agents.is_empty());
    vault.delete(&config, &item.id).await.unwrap();
    assert!(vault.items().is_empty());
}

#[tokio::test]
async fn what_the_app_must_send() {
    let dir = tempfile::tempdir().unwrap();
    let (config, vault, _) = setup(dir.path());
    for extra in [
        json!({"password": ""}),
        json!({"identifier": " "}),
        json!({"site": "ftp://x"}),
        json!({"label": ""}),
        json!({"kind": "ssh"}),
    ] {
        let mut body = login(extra);
        body.insert("agents".into(), json!([]));
        let err = vault.create(&config, &body).await.unwrap_err();
        assert!(err.downcast_ref::<VaultError>().is_some(), "{err}");
    }
    assert_eq!(normalize_origin("HTTPS://Example.com:443/a?b").unwrap(), "https://example.com");
    assert_eq!(normalize_origin("example.com:8443").unwrap(), "https://example.com:8443");
}

#[tokio::test]
async fn routes() {
    let dir = tempfile::tempdir().unwrap();
    let mut data = config_dict(dir.path(), &Urls::default());
    data["hermes"] = json!({"isolated": true});
    let app = TestApp::new(parse_config(&data, None).unwrap());
    let (name, key) = auth();
    let headers = [(name, key.as_str())];
    let listing = app.get("/v1/vault", &headers).await;
    assert_eq!(listing.status, 200);
    assert_eq!(listing.json()["protected"], true);
    // Set up by hand (not in the hub): up to its administrator, the app offers no move.
    assert_eq!(
        listing.json()["agents"],
        json!([{"id": "wellness", "name": "Wellness", "can_hold": false, "why": "not_configured"}])
    );
    let refused =
        app.send_json("POST", "/v1/vault", &headers, &Value::Object(login(json!({"agents": ["wellness"]})))).await;
    assert_eq!(refused.status, 409);
    assert_eq!(refused.json()["detail"]["agents"], json!({"wellness": "not_configured"}));
    let kept = app.send_json("POST", "/v1/vault", &headers, &Value::Object(login(json!({"agents": []})))).await;
    assert_eq!(kept.status, 201);
    assert!(!kept.text().contains("password"));
    let id = kept.json()["id"].as_str().unwrap().to_string();
    assert_eq!(app.request("DELETE", &format!("/v1/vault/{id}"), &headers, None).await.status, 204);
    assert_eq!(app.get("/v1/vault", &[]).await.status, 401);
}

#[tokio::test]
async fn an_install_whose_agents_can_read_the_bridge_keeps_no_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(common::config(dir.path(), &Urls::default()));
    assert!(!app.app.state.config().hermes.isolated); // the default: nothing says the agents are isolated
    let (name, key) = auth();
    let headers = [(name, key.as_str())];
    let listing = app.get("/v1/vault", &headers).await.json();
    assert_eq!(listing["protected"], false);
    assert_eq!(listing["agents"][0]["why"], "unprotected_install");
    let refused = app.send_json("POST", "/v1/vault", &headers, &Value::Object(login(json!({"agents": []})))).await;
    assert_eq!(refused.status, 409);
    assert_eq!(refused.json()["detail"], json!({"code": "unprotected_install"}));
    assert!(app.app.state.vault.items().is_empty() && !app.app.state.vault.path.exists()); // nothing written
    assert_eq!(app.get("/v1/status", &headers).await.json()["protected"], false);
}
