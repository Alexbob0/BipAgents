//! tests/test_cloud_setup.py: a server the app created proves its certificate, then is claimed once with the
//! pairing secret (or restored with the backup key, or reopened after an account recovery).

mod browsers_places;
mod common;

use std::sync::{Arc, Mutex};

use axum::Router;
use base64::Engine;
use bipbridge::app::AppState;
use bipbridge::cloud_setup::{self, ClaimRequest, Hooks, CLAIM_CONTEXT, SETUP_CONTEXT};
use bipbridge::config::{parse_config, Config};
use browsers_places::{send, send_json, v1};
use common::{config_dict, TestApp, Urls, BRIDGE_KEY};
use serde_json::{json, Value};

const NONCE: &str = "abababababababababababababababab";

fn secret() -> Vec<u8> {
    (0u8..32).collect()
}

fn encoded(secret: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret)
}

/// The `cloud` fixture: a place of the all-included offer, its certificate made.
fn cloud(dir: &std::path::Path) -> Config {
    let setup = dir.join("setup.json");
    std::fs::write(&setup, json!({"pair": encoded(&secret())}).to_string()).unwrap();
    let path = dir.join("bridge.toml");
    std::fs::write(&path, "# bridge config\n").unwrap();
    let mut d = config_dict(dir, &Urls::default());
    let map = d.as_object_mut().unwrap();
    map.insert("cloud".into(), json!(true));
    map.insert("public_url".into(), json!("https://203.0.113.7"));
    map.insert(
        "lan".into(),
        json!({"enabled": true, "address": "203.0.113.7", "port": 443, "cert_dir": dir.join("lan").to_string_lossy()}),
    );
    map.insert(
        "setup".into(),
        json!({"file": setup.to_string_lossy(), "progress": dir.join("progress.json").to_string_lossy()}),
    );
    map.insert("hermes".into(), json!({"bin": "/opt/hermes", "home": dir.join(".hermes").to_string_lossy(), "hub_url": "http://127.0.0.1:8642"}));
    let config = parse_config(&d, Some(&path.to_string_lossy())).unwrap();
    bipbridge::lan::ensure_certificate(&config).unwrap();
    config
}

fn done(config: &Config, flag: bool) {
    let progress = config.setup_progress.clone().unwrap();
    std::fs::write(progress, json!({"step": 6, "total": 6, "label": "Checks", "done": flag}).to_string()).unwrap();
}

fn claim_body(challenge: &str, request: Option<&str>, secret: &[u8]) -> Value {
    let request = request.map(str::to_string).unwrap_or_else(|| {
        json!({"model": {"provider": "custom", "name": "glm-5.3", "key": "sk-eu-0123456789", "base_url": "https://api.eurouter.example/v1"},
               "agents": [{"name": "Quotidien", "kind": "daily"}], "language": "fr"})
        .to_string()
    });
    json!({"challenge": challenge, "mac": cloud_setup::proof(secret, &[CLAIM_CONTEXT, challenge, &request]), "request": request})
}

/// The `client` fixture: the setup routes with `claim_server` and `start_restore` replaced.
struct Client {
    app: TestApp,
    router: Router,
    claims: Arc<Mutex<Vec<ClaimRequest>>>,
    restores: Arc<Mutex<Vec<String>>>,
    rekeys: Arc<Mutex<Vec<String>>>,
}

impl Client {
    fn new(config: Config) -> Client {
        let app = TestApp::new(config);
        let state: AppState = app.app.state.clone();
        let claims: Arc<Mutex<Vec<ClaimRequest>>> = Arc::default();
        let restores: Arc<Mutex<Vec<String>>> = Arc::default();
        let rekeys: Arc<Mutex<Vec<String>>> = Arc::default();
        let (c, r, k) = (claims.clone(), restores.clone(), rekeys.clone());
        let hooks = Hooks {
            claim: Arc::new(move |_state, request: ClaimRequest| {
                let name = request.agents[0].name.clone();
                c.lock().unwrap().push(request);
                Box::pin(async move { Ok(vec![json!({"id": "quotidien", "name": name})]) })
            }),
            restore: Arc::new(move |_config: &Config, key: &str| {
                r.lock().unwrap().push(key.to_string());
                Ok(())
            }),
            rekey: Arc::new(move |_state, model: cloud_setup::ModelChoice| {
                k.lock().unwrap().push(model.key);
                Box::pin(async { Ok(()) })
            }),
        };
        let router = v1(&state, Router::new(), cloud_setup::router_with(hooks));
        Client { app, router, claims, restores, rekeys }
    }

    async fn setup(&self) -> Value {
        send(&self.router, "GET", &format!("/v1/setup?nonce={NONCE}"), &[], None).await.json()
    }

    async fn challenge(&self) -> String {
        self.setup().await["challenge"].as_str().unwrap().to_string()
    }

    async fn claim(&self, body: &Value) -> common::TestResponse {
        send_json(&self.router, "POST", "/v1/setup/claim", &[], body).await
    }
}

#[tokio::test]
async fn the_server_proves_its_certificate_before_anything() {
    let dir = tempfile::tempdir().unwrap();
    let config = cloud(dir.path());
    let client = Client::new(config.clone());
    let state = client.setup().await;
    let print = bipbridge::lan::fingerprint(&config).unwrap();
    assert_eq!(state["state"], "installing");
    assert_eq!(state["fingerprint"], print);
    let challenge = state["challenge"].as_str().unwrap();
    // Only a server that knows the secret from the user-data can make this proof for this certificate.
    assert_eq!(state["proof"], cloud_setup::proof(&secret(), &[SETUP_CONTEXT, NONCE, &print, challenge]));
    assert_ne!(state["proof"], cloud_setup::proof(&[b'x'; 32], &[SETUP_CONTEXT, NONCE, &print, challenge]));
    assert_eq!(
        state.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["state", "step", "total", "label", "fingerprint", "challenge", "proof", "prepared"]
    );
    assert_eq!(state["prepared"], false); // its agents are made at the claim (the fixture's place has some)
    done(&config, true);
    assert_eq!(client.setup().await["state"], "ready");
    let short = send(&client.router, "GET", "/v1/setup?nonce=short", &[], None).await;
    assert_eq!(short.status, 422);
    assert_eq!(
        short.json(),
        json!({"detail": [{"type": "string_too_short", "loc": ["query", "nonce"], "msg": "String should have at least 32 characters",
                           "input": "short", "ctx": {"min_length": 32}}]})
    );
    let not_hex = send(&client.router, "GET", &format!("/v1/setup?nonce={}", "z".repeat(40)), &[], None).await;
    assert_eq!(not_hex.json()["detail"][0]["type"], "string_pattern_mismatch");
}

#[tokio::test]
async fn a_claim_with_the_secret_pairs_the_app_once() {
    let dir = tempfile::tempdir().unwrap();
    let config = cloud(dir.path());
    let client = Client::new(config.clone());
    done(&config, true);
    let challenge = client.challenge().await;
    let paired = client.claim(&claim_body(&challenge, None, &secret())).await;
    assert_eq!(paired.status, 200, "{}", paired.text());
    assert_eq!(
        paired.json(),
        json!({"bridgeURL": "https://203.0.113.7", "bridgeKey": BRIDGE_KEY, "fingerprint": bipbridge::lan::fingerprint(&config),
               "agents": [{"id": "quotidien", "name": "Quotidien"}]})
    );
    assert_eq!(client.claims.lock().unwrap()[0].model.key, "sk-eu-0123456789");
    assert!(!dir.path().join("setup.json").exists());
    // The app uses its key: the secret is gone for good, the server says so, and a second claim, even with the same
    // proof, is refused.
    let key = format!("Bearer {BRIDGE_KEY}");
    assert_eq!(client.app.get("/v1/agents", &[("authorization", &key)]).await.status, 200);
    assert!(!dir.path().join("claim-recovery.json").exists());
    assert_eq!(client.setup().await, json!({"state": "claimed"}));
    assert_eq!(client.claim(&claim_body(&challenge, None, &secret())).await.status, 410);
}

fn claim_with_id(claim_id: &str, model_key: &str) -> String {
    json!({"model": {"provider": "custom", "name": "glm-5.3", "key": model_key, "base_url": "https://api.eurouter.example/v1"},
           "agents": [{"name": "Quotidien", "kind": "daily"}], "language": "fr", "claim_id": claim_id})
    .to_string()
}

#[tokio::test]
async fn a_claim_whose_answer_was_lost_is_recovered_by_its_claimer_only() {
    let dir = tempfile::tempdir().unwrap();
    let config = cloud(dir.path());
    let client = Client::new(config.clone());
    done(&config, true);
    let id = "app-made-claim-id-0123456789abcdef";
    let first = client
        .claim(&claim_body(&client.challenge().await, Some(&claim_with_id(id, "sk-eu-first-key")), &secret()))
        .await;
    assert_eq!(first.status, 200, "{}", first.text());
    assert_eq!(first.json()["bridgeKey"], BRIDGE_KEY);
    let record = std::fs::read_to_string(dir.path().join("claim-recovery.json")).unwrap();
    assert!(!record.contains(id) && !record.contains(BRIDGE_KEY) && !record.contains("sk-eu-first-key")); // hashes only
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(dir.path().join("claim-recovery.json")).unwrap().permissions().mode() & 0o777, 0o600);

    // The answer never reached the app. Its retry starts over: the place still proves its certificate and says ready.
    let state = client.setup().await;
    assert_eq!(state["state"], "ready");
    assert_eq!(state["claimed"], true);
    let print = bipbridge::lan::fingerprint(&config).unwrap();
    let challenge = state["challenge"].as_str().unwrap();
    assert_eq!(state["proof"], cloud_setup::proof(&secret(), &[SETUP_CONTEXT, NONCE, &print, challenge]));

    // Someone else (another secret, or the secret without the claim id): refused as before.
    let wrong = client.claim(&claim_body(challenge, Some(&claim_with_id(id, "sk-eu-first-key")), &[b'y'; 32])).await;
    assert_eq!(wrong.status, 401);
    let other = claim_with_id("someone-elses-id-0123456789abcdef", "sk-eu-first-key");
    assert_eq!(client.claim(&claim_body(&client.challenge().await, Some(&other), &secret())).await.status, 410);
    let without = json!({"model": {"provider": "custom", "name": "glm-5.3", "key": "sk-eu-first-key", "base_url": "https://x/v1"},
                         "agents": [{"name": "Quotidien"}]}).to_string();
    assert_eq!(client.claim(&claim_body(&client.challenge().await, Some(&without), &secret())).await.status, 410);
    let reopen = json!({"reopen": {"name": "iPhone"}}).to_string();
    assert_eq!(client.claim(&claim_body(&client.challenge().await, Some(&reopen), &secret())).await.status, 410);

    // The claimer, with a new AI key (the guichet made another one): a fresh bridge key, the same agents, the new
    // AI key written; the place isn't claimed a second time.
    let again = client
        .claim(&claim_body(&client.challenge().await, Some(&claim_with_id(id, "sk-eu-second-key")), &secret()))
        .await;
    assert_eq!(again.status, 200, "{}", again.text());
    let again = again.json();
    assert_eq!(again["recovered"], true);
    assert_eq!(again["fingerprint"], json!(print));
    assert_eq!(again["bridgeURL"], "https://203.0.113.7");
    assert_eq!(again["agents"], json!([{"id": "quotidien", "name": "Quotidien"}]));
    let new_key = again["bridgeKey"].as_str().unwrap().to_string();
    assert_ne!(new_key, BRIDGE_KEY);
    assert_eq!(client.claims.lock().unwrap().len(), 1);
    assert_eq!(*client.rekeys.lock().unwrap(), vec!["sk-eu-second-key".to_string()]);
    let toml = std::fs::read_to_string(config.source_path.clone().unwrap()).unwrap();
    assert!(toml.contains(&new_key));
    let old = format!("Bearer {BRIDGE_KEY}");
    assert_eq!(client.app.get("/v1/agents", &[("authorization", &old)]).await.status, 401); // the lost key: revoked

    // Lost again, the same AI key this time: recovered without writing the model again.
    let third = client
        .claim(&claim_body(&client.challenge().await, Some(&claim_with_id(id, "sk-eu-second-key")), &secret()))
        .await;
    assert_eq!(third.status, 200, "{}", third.text());
    assert_eq!(client.rekeys.lock().unwrap().len(), 1);
    let kept = format!("Bearer {}", third.json()["bridgeKey"].as_str().unwrap());

    // The app uses its key: no recovery any more.
    assert_eq!(client.app.get("/v1/agents", &[("authorization", &kept)]).await.status, 200);
    assert_eq!(client.setup().await, json!({"state": "claimed"}));
    let late = claim_with_id(id, "sk-eu-second-key");
    assert_eq!(client.claim(&claim_body(&"0".repeat(32), Some(&late), &secret())).await.status, 410);
}

#[tokio::test]
async fn an_app_without_claim_id_recovers_with_the_secret_within_a_day() {
    let dir = tempfile::tempdir().unwrap();
    let config = cloud(dir.path());
    let client = Client::new(config.clone());
    done(&config, true);
    assert_eq!(client.claim(&claim_body(&client.challenge().await, None, &secret())).await.status, 200);
    // the apps out before claim_id: the same proof of the secret is enough (the same AI key: nothing rewritten)
    let again = client.claim(&claim_body(&client.challenge().await, None, &secret())).await;
    assert_eq!(again.status, 200, "{}", again.text());
    assert_eq!(again.json()["recovered"], true);
    assert!(client.rekeys.lock().unwrap().is_empty());
    // A day later (the bridge key still unused): claimed, the record gone.
    let path = dir.path().join("claim-recovery.json");
    let mut record: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    record["at"] = json!(record["at"].as_f64().unwrap() - cloud_setup::RECOVERY_SECONDS - 1.0);
    std::fs::write(&path, record.to_string()).unwrap();
    assert_eq!(client.setup().await, json!({"state": "claimed"}));
    assert!(!path.exists());
    assert_eq!(client.claim(&claim_body(&"0".repeat(32), None, &secret())).await.status, 410);
}

#[tokio::test]
async fn challenges_are_single_use_and_claims_wait_for_the_install() {
    let dir = tempfile::tempdir().unwrap();
    let config = cloud(dir.path());
    let client = Client::new(config.clone());
    let challenge = client.challenge().await;
    assert_eq!(client.claim(&claim_body(&challenge, None, &secret())).await.status, 409); // still installing
    done(&config, true);
    let used = client.claim(&claim_body(&challenge, None, &secret())).await;
    assert_eq!(used.status, 400); // used up by the 409
    assert_eq!(used.json(), json!({"detail": "challenge unknown or expired: ask /v1/setup again"}));
    let fresh = client.challenge().await;
    let bad = json!({"model": {"provider": "custom", "name": "x", "key": "sk-0123456789"}, "agents": []}).to_string();
    let refused = client.claim(&claim_body(&fresh, Some(&bad), &secret())).await;
    assert_eq!(refused.status, 400); // no base_url
    assert_eq!(refused.json(), json!({"detail": "a custom model needs its base_url"}));
    let fresh = client.challenge().await;
    let bad = json!({"model": {"provider": "mistral", "name": "x", "key": "sk-0123456789"}}).to_string();
    let refused = client.claim(&claim_body(&fresh, Some(&bad), &secret())).await;
    assert_eq!(
        refused.json(),
        json!({"detail": "invalid request: String should match pattern '^(openrouter|custom)$'"})
    );
    let malformed = client.claim(&json!({"challenge": "x", "mac": "y", "request": ""})).await;
    assert_eq!(malformed.status, 422);
    assert_eq!(malformed.json()["detail"].as_array().unwrap().len(), 3);
    assert!(client.claims.lock().unwrap().is_empty());
}

#[tokio::test]
async fn wrong_proofs_get_the_address_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let config = cloud(dir.path());
    let client = Client::new(config.clone());
    done(&config, true);
    let limit = client.app.app.state.guard.limit;
    for _ in 0..limit {
        let challenge = client.challenge().await;
        let wrong = client.claim(&claim_body(&challenge, None, &[b'y'; 32])).await;
        assert_eq!(wrong.status, 401);
        assert_eq!(wrong.json(), json!({"detail": "wrong pairing proof"}));
    }
    let blocked = send(&client.router, "GET", &format!("/v1/setup?nonce={NONCE}"), &[], None).await;
    assert_eq!(blocked.status, 429);
    assert_eq!(blocked.json(), json!({"detail": "too many wrong proofs, try again later"}));
    assert!(client.claims.lock().unwrap().is_empty());
}

#[tokio::test]
async fn wrong_bridge_keys_get_the_address_blocked_and_the_console_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(cloud(dir.path()));
    let limit = app.app.state.guard.limit;
    for _ in 0..limit {
        assert_eq!(app.get("/v1/agents", &[("authorization", "Bearer nope")]).await.status, 401);
    }
    // Blocked for a while, even with the right key; requests without a key don't count.
    let key = format!("Bearer {BRIDGE_KEY}");
    assert_eq!(app.get("/v1/agents", &[("authorization", &key)]).await.status, 429);
    assert_eq!(app.get("/console/", &[]).await.status, 404);
    // The whole app has the setup routes, without the bridge key.
    assert_eq!(app.get(&format!("/v1/setup?nonce={NONCE}"), &[]).await.status, 429);
    // the same address, still blocked
}

#[tokio::test]
async fn a_moved_place_is_claimed_with_the_backup_key_and_says_it_is_restoring() {
    let dir = tempfile::tempdir().unwrap();
    let config = cloud(dir.path());
    let client = Client::new(config.clone());
    done(&config, true);
    let challenge = client.challenge().await;
    let request = json!({"restore": {"key": "the-key-the-phone-kept-0123456789"}}).to_string();
    let answer = client.claim(&claim_body(&challenge, Some(&request), &secret())).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    let answer = answer.json();
    assert_eq!(answer["restoring"], true);
    assert!(answer.get("bridgeKey").is_none()); // its own key comes back
    assert_eq!(*client.restores.lock().unwrap(), vec!["the-key-the-phone-kept-0123456789".to_string()]);
    assert!(client.claims.lock().unwrap().is_empty());
    let state = client.setup().await;
    assert_eq!(state["state"], "restoring");
    assert!(state["proof"].as_str().is_some()); // still proved with the secret
    let again = client.challenge().await;
    assert_eq!(client.claim(&claim_body(&again, Some(&request), &secret())).await.status, 409); // once at a time
                                                                                                // The restore failed (a wrong key): the place says why and takes another claim.
    std::fs::remove_file(dir.path().join("restoring.json")).unwrap();
    std::fs::write(
        config.setup_progress.clone().unwrap(),
        json!({"step": 0, "total": 4, "label": "this key doesn't open these backups", "done": false, "restoring": false,
               "error": "this key doesn't open these backups"})
        .to_string(),
    )
    .unwrap();
    let state = client.setup().await;
    assert_eq!(state["state"], "ready");
    assert_eq!(state["error"], "this key doesn't open these backups");
    let bad = json!({"restore": {"key": "short"}}).to_string();
    let retry = client.challenge().await;
    let refused = client.claim(&claim_body(&retry, Some(&bad), &secret())).await;
    assert_eq!(refused.status, 400);
    assert_eq!(refused.json(), json!({"detail": "invalid restore request"}));
}

#[tokio::test]
async fn a_device_key_opens_the_bridge_and_a_reopen_leaves_only_the_recovering_device() {
    let dir = tempfile::tempdir().unwrap();
    let config = cloud(dir.path());
    let app = TestApp::new(config.clone());
    let owner_key = format!("Bearer {BRIDGE_KEY}");
    let owner = [("authorization", owner_key.as_str())];
    let made = app.send_json("POST", "/v1/device-keys", &owner, &json!({"name": "iPad  d'Alex"})).await;
    assert_eq!(made.status, 200, "{}", made.text());
    let made = made.json();
    let ipad_key = format!("Bearer {}", made["key"].as_str().unwrap());
    let ipad = [("authorization", ipad_key.as_str())];
    let listed = app.get("/v1/device-keys", &ipad).await.json();
    assert_eq!(listed["devices"][0]["name"], "iPad d'Alex");
    assert_eq!(listed["devices"][0]["me"], true);
    assert_eq!(app.get("/v1/backup", &ipad).await.status, 200); // any route, as with the bridge key
    let stored = std::fs::read_to_string(dir.path().join("data").join("device-keys.json")).unwrap();
    assert!(!stored.contains(made["key"].as_str().unwrap())); // its hash only
    let other = app.send_json("POST", "/v1/device-keys", &owner, &json!({"name": "Pixel"})).await.json();
    let removed =
        app.request("DELETE", &format!("/v1/device-keys/{}", other["id"].as_str().unwrap()), &ipad, None).await;
    assert_eq!(removed.json(), json!({"ok": true}));
    let other_key = format!("Bearer {}", other["key"].as_str().unwrap());
    assert_eq!(app.get("/v1/device-keys", &[("authorization", other_key.as_str())]).await.status, 401);

    // The seat agent wrote a new secret (installer/cloud/seat-agent.py): the place is claimed again.
    let new_secret: Vec<u8> = (32u8..64).collect();
    std::fs::write(
        config.setup_file.clone().unwrap(),
        json!({"pair": encoded(&new_secret), "reopen": true}).to_string(),
    )
    .unwrap();
    done(&config, true);
    let challenge =
        app.get(&format!("/v1/setup?nonce={NONCE}"), &[]).await.json()["challenge"].as_str().unwrap().to_string();
    let request = json!({"reopen": {"name": "Nouvel iPhone"}}).to_string();
    let again =
        app.send_json("POST", "/v1/setup/claim", &[], &claim_body(&challenge, Some(&request), &new_secret)).await;
    assert_eq!(again.status, 200, "{}", again.text());
    let again = again.json();
    assert_eq!(again["reopened"], true);
    assert_eq!(again["fingerprint"], json!(bipbridge::lan::fingerprint(&config)));
    assert_ne!(again["bridgeKey"], BRIDGE_KEY);
    assert_eq!(app.get("/v1/device-keys", &owner).await.status, 401); // the old bridge key: gone
    assert_eq!(app.get("/v1/device-keys", &ipad).await.status, 401); // the other devices: gone
    let mine_key = format!("Bearer {}", again["bridgeKey"].as_str().unwrap());
    let mine = app.get("/v1/device-keys", &[("authorization", mine_key.as_str())]).await.json();
    let names: Vec<&Value> = mine["devices"].as_array().unwrap().iter().map(|d| &d["name"]).collect();
    assert_eq!(names, vec![&json!("Nouvel iPhone")]);
    let toml = std::fs::read_to_string(config.source_path.clone().unwrap()).unwrap();
    assert!(toml.contains("bridge_key = ") && !toml.contains(BRIDGE_KEY));
    assert_eq!(app.get(&format!("/v1/setup?nonce={NONCE}"), &[]).await.json(), json!({"state": "claimed"}));
}

#[tokio::test]
async fn claiming_sets_the_model_and_creates_agents_reached_through_the_door() {
    use axum::routing::{any, get};
    // The hub's gateway: its capabilities, and the sessions the new agents make.
    let gateway = common::spawn_fake(
        Router::new()
            .route("/v1/capabilities", get(|| async { axum::Json(json!({})) }))
            .route("/p/{profile}/v1/capabilities", get(|| async { axum::Json(json!({})) }))
            .fallback(any(|| async { (axum::http::StatusCode::CREATED, axum::Json(json!({"id": "s1"}))) })),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = cloud(dir.path());
    config.hermes.hub_url = gateway.clone();
    let shared = bipbridge::config::SharedConfig::new(config.clone());
    let commands: Arc<Mutex<Vec<Vec<String>>>> = Arc::default();
    let seen = commands.clone();
    let failing = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let fail = failing.clone();
    let run: bipbridge::agents_admin::Run = Arc::new(move |args: Vec<String>, _env| {
        seen.lock().unwrap().push(args.clone());
        let fails = fail.load(std::sync::atomic::Ordering::SeqCst)
            && args.get(1..3) == Some(&["profile".to_string(), "create".to_string()][..])
            && args.get(3).map(String::as_str) == Some("maison");
        Box::pin(async move {
            if fails {
                anyhow::bail!("disk full") // the second agent fails the first time
            }
            Ok(String::new())
        })
    });
    let wanted = ClaimRequest::from_value(&json!({
        "model": {"provider": "openrouter", "name": "z-ai/glm-5.3", "key": "sk-or-0123456789", "vision": true},
        "agents": [{"name": "Quotidien", "kind": "daily"}, {"name": "Maison", "kind": "home"}], "language": "fr"}))
    .unwrap();
    let http = reqwest::Client::new();
    assert!(cloud_setup::claim_server(&shared, &wanted, &http, Some(run.clone())).await.is_err());
    let ran = commands.lock().unwrap().clone();
    let hermes = |rest: &[&str]| -> Vec<String> {
        std::iter::once("/opt/hermes").chain(rest.iter().copied()).map(str::to_string).collect()
    };
    assert!(ran.contains(&hermes(&["config", "set", "model.provider", "openrouter"])));
    assert!(ran.contains(&hermes(&["config", "set", "model.base_url", "https://openrouter.ai/api/v1"])));
    assert!(ran.contains(&hermes(&["config", "set", "OPENROUTER_API_KEY", "sk-or-0123456789"])));
    assert!(ran.contains(&hermes(&["config", "set", "model.supports_vision", "true"])));
    assert!(ran.contains(&hermes(&["gateway", "restart"])));
    assert_eq!(
        shared.get().agent(Some("quotidien")).unwrap().public_url.as_deref(),
        Some("https://203.0.113.7/hermes/quotidien")
    );
    let toml = std::fs::read_to_string(config.source_path.clone().unwrap()).unwrap();
    assert!(
        toml.contains(
            "[model]\nhealth_url = \"https://openrouter.ai/api/v1/models\"\nhealth_key = \"sk-or-0123456789\""
        ),
        "{toml}"
    );
    assert_eq!(cloud_setup::secret_of(&shared.get()), Some(secret())); // not paired yet: the app may try again
                                                                       // Again: the model is not set twice, Quotidien is not made twice, Maison is made this time.
    commands.lock().unwrap().clear();
    failing.store(false, std::sync::atomic::Ordering::SeqCst);
    let agents = cloud_setup::claim_server(&shared, &wanted, &http, Some(run)).await.unwrap();
    assert_eq!(
        agents,
        vec![json!({"id": "quotidien", "name": "Quotidien"}), json!({"id": "maison", "name": "Maison"})]
    );
    let ran = commands.lock().unwrap().clone();
    assert!(!ran.iter().any(|c| c.get(1..3) == Some(&["config".to_string(), "set".to_string()][..])));
    assert_eq!(
        ran.iter().filter(|c| c.get(1..3) == Some(&["profile".to_string(), "create".to_string()][..])).count(),
        1
    );
}

/// A fake `hermes`: `profile create` clones the hub's folder, `config set` of a variable writes the profile's .env.
fn fake_hermes(home: std::path::PathBuf, commands: Arc<Mutex<Vec<Vec<String>>>>) -> bipbridge::agents_admin::Run {
    Arc::new(move |args: Vec<String>, _env| {
        commands.lock().unwrap().push(args.clone());
        if args.get(1..3) == Some(&["profile".to_string(), "create".to_string()][..]) {
            let folder = home.join("profiles").join(&args[3]);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::copy(home.join("config.yaml"), folder.join("config.yaml")).unwrap();
            std::fs::copy(home.join(".env"), folder.join(".env")).unwrap();
        }
        // `config set KEY [--] VALUE`: the bridge puts `--` before a key's value (one starting with « - »)
        let value = match args.len() {
            7 => Some(&args[6]),
            8 if args[6] == "--" => Some(&args[7]),
            _ => None,
        };
        if let (Some(value), true) = (value, args[1] == "-p" && args[3..5] == ["config".to_string(), "set".to_string()])
        {
            let env = home.join("profiles").join(&args[2]).join(".env");
            let text = std::fs::read_to_string(&env).unwrap_or_default();
            std::fs::write(&env, format!("{text}{}={}\n", args[5], value)).unwrap();
        }
        Box::pin(async { Ok(String::new()) })
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prepared_place_is_claimed_with_file_writes_and_its_other_agents_made_after() {
    use axum::routing::{any, get};
    let gateway = common::spawn_fake(
        Router::new()
            .route("/p/{profile}/v1/capabilities", get(|| async { axum::Json(json!({})) }))
            .fallback(any(|| async { (axum::http::StatusCode::CREATED, axum::Json(json!({"id": "s1"}))) })),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = cloud(dir.path());
    config.hermes.hub_url = gateway.clone();
    config.agents.clear(); // a place of the reserve: no agent yet
    let home = dir.path().join(".hermes");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("config.yaml"), "model:\n  provider: openrouter\n  default: openrouter/auto\n").unwrap();
    std::fs::write(home.join(".env"), "# Hermes\nAPI_SERVER_ENABLED=true\n").unwrap();
    let shared = bipbridge::config::SharedConfig::new(config.clone());
    let commands: Arc<Mutex<Vec<Vec<String>>>> = Arc::default();
    let run = fake_hermes(home.clone(), commands.clone());
    let http = reqwest::Client::new();
    // Still installing: nothing prepared yet. Once done: its first agent, under a neutral id.
    done(&config, false);
    assert_eq!(cloud_setup::prepare_place(&shared, &http, Some(run.clone())).await.unwrap(), Some("agent".into()));
    assert_eq!(cloud_setup::prepared_of(&shared.get()).as_deref(), Some("agent"));
    assert!(commands
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.get(1..4) == Some(&["profile".to_string(), "create".to_string(), "agent".to_string()][..])));
    assert_eq!(shared.get().agents["agent"].display_name, "Agent");
    commands.lock().unwrap().clear();

    // The claim: files written, no `hermes` command before the answer.
    let wanted = ClaimRequest::from_value(&json!({
        "model": {"provider": "custom", "name": "glm-5.3", "key": "sk-eu-0123456789", "base_url": "https://api.eurouter.example/v1"},
        "agents": [{"name": "Quotidien", "kind": "daily"}, {"name": "Maison", "kind": "home"}], "language": "en"}))
    .unwrap();
    let started = std::time::Instant::now();
    let agents = cloud_setup::claim_server(&shared, &wanted, &http, Some(run.clone())).await.unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
    assert_eq!(agents, vec![json!({"id": "agent", "name": "Quotidien"}), json!({"id": "maison", "name": "Maison"})]);
    for folder in [home.clone(), home.join("profiles/agent")] {
        let yaml = std::fs::read_to_string(folder.join("config.yaml")).unwrap();
        let data = bipbridge::agents_admin::pyyaml::to_json(&bipbridge::agents_admin::pyyaml::load(&yaml).unwrap());
        assert_eq!(
            data["model"],
            json!({"provider": "custom", "default": "glm-5.3", "base_url": "https://api.eurouter.example/v1",
                   "key_env": "CUSTOM_MODEL_API_KEY", "supports_vision": false})
        );
        let env = std::fs::read_to_string(folder.join(".env")).unwrap();
        assert!(env.contains("CUSTOM_MODEL_API_KEY=sk-eu-0123456789\n"), "{env}");
    }
    let key = shared.get().agents["agent"].hermes_key.clone();
    assert!(std::fs::read_to_string(home.join("profiles/agent/.env"))
        .unwrap()
        .contains(&format!("API_SERVER_KEY={key}")));
    assert!(std::fs::read_to_string(home.join("profiles/agent/SOUL.md")).unwrap().contains("Quotidien"));
    let toml = std::fs::read_to_string(config.source_path.clone().unwrap()).unwrap();
    assert!(toml.contains("display_name = \"Quotidien\"") && toml.contains("[agents.maison]"), "{toml}");
    // Maison is listed at once (the app adds it), made right after: once the gateway restarted directly.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::fs::read_dir(dir.path()).unwrap().flatten().any(|e| e.file_name() == "claim-pending.json") {
        assert!(std::time::Instant::now() < deadline, "Maison not made");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let ran = commands.lock().unwrap().clone();
    let restart = ran.iter().position(|c| {
        c.ends_with(&["gateway".into(), "restart".into()])
            || c.get(..3) == Some(&["systemctl".to_string(), "--user".to_string(), "restart".to_string()][..])
    });
    let create = ran
        .iter()
        .position(|c| c.get(1..4) == Some(&["profile".to_string(), "create".to_string(), "maison".to_string()][..]));
    assert!(restart.unwrap() < create.unwrap(), "{ran:?}");
    assert!(!ran.iter().any(|c| c.len() > 4 && c[1..3] == ["config", "set"]));
    let maison = shared.get().agents["maison"].clone();
    assert!(ran.contains(&vec![
        "/opt/hermes".into(),
        "-p".into(),
        "maison".into(),
        "config".into(),
        "set".into(),
        "API_SERVER_KEY".into(),
        "--".into(),
        maison.hermes_key.clone()
    ]));
    let started = std::time::Instant::now();
    cloud_setup::READINESS.wait("maison", std::time::Duration::from_secs(5)).await;
    assert!(started.elapsed() < std::time::Duration::from_secs(1));

    // A recovered claim with another AI key: written in the hub and every profile, the gateway restarted.
    commands.lock().unwrap().clear();
    let mut model = wanted.model.clone();
    model.key = "sk-eu-the-new-key-42".into();
    cloud_setup::rekey_model(&shared, &model, run.clone()).await.unwrap();
    for folder in [home.clone(), home.join("profiles/agent"), home.join("profiles/maison")] {
        let env = std::fs::read_to_string(folder.join(".env")).unwrap();
        assert!(
            env.contains("CUSTOM_MODEL_API_KEY=sk-eu-the-new-key-42\n") && !env.contains("sk-eu-0123456789"),
            "{env}"
        );
    }
    assert!(std::fs::read_to_string(home.join("profiles/agent/.env"))
        .unwrap()
        .contains(&format!("API_SERVER_KEY={key}")));
    assert_eq!(shared.get().model_health_key.as_deref(), Some("sk-eu-the-new-key-42"));
    assert_eq!(commands.lock().unwrap().len(), 1); // the restart
}

#[tokio::test]
async fn requests_through_the_door_wait_for_an_agent_the_claim_makes_ready() {
    let readiness = Arc::new(cloud_setup::Readiness::default());
    readiness.hold("lent");
    let r = readiness.clone();
    let waiter = tokio::spawn(async move {
        let started = std::time::Instant::now();
        r.wait("lent", std::time::Duration::from_secs(5)).await;
        started.elapsed()
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    readiness.release("lent");
    let waited = waiter.await.unwrap();
    assert!(waited >= std::time::Duration::from_millis(250) && waited < std::time::Duration::from_secs(2));
    // never longer than its limit
    readiness.hold(cloud_setup::GATEWAY);
    let started = std::time::Instant::now();
    readiness.wait("other", std::time::Duration::from_millis(200)).await;
    assert!(started.elapsed() >= std::time::Duration::from_millis(200));
}
