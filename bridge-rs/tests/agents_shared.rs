//! Agents shared with relatives (agent_shares.rs, agent_feed.rs): Alex shares Cuisine with Marie (two bridges, linked)
//! once he confirmed what she will read; Marie adds it and reads it, never writes (Alex 12/11: a personal shared agent
//! is read-only for relatives, no relative's turn, so no Hermes patch 0018 needed for it). The guest-turn check stays
//! for the family's agent and the other households' turns.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::response::Response;
use axum::Router;
use bipbridge::config::parse_config;
use common::fakes::{sse, Flag};
use common::{auth, config_dict, TestApp, Urls};
use serde_json::{json, Value};

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

async fn serve_bridge(dir: &Path, name: &str, agents: &[&str], hermes: &str) -> (String, TestApp) {
    serve_bridge_with(dir, name, agents, hermes, json!({})).await
}

/// The same, with `extra`'s top-level keys in its config.
async fn serve_bridge_with(dir: &Path, name: &str, agents: &[&str], hermes: &str, extra: Value) -> (String, TestApp) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let mut d = config_dict(dir, &Urls::default());
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        d[key] = value;
    }
    d["public_url"] = json!(url);
    d["outbox"] = json!({"db_path": s(&dir.join(name).join("bridge.db"))});
    let mut table = serde_json::Map::new();
    for a in agents {
        let title = format!("{}{}", a[..1].to_uppercase(), &a[1..]);
        table.insert(
            title,
            json!({"hermes_url": format!("{hermes}/{name}/p/{a}"), "hermes_key": format!("{a}-key-{}", "x".repeat(24))}),
        );
    }
    d["agents"] = Value::Object(table);
    let app = bipbridge::app::create_app(
        parse_config(&d, None).unwrap(),
        bipbridge::app::AppOptions { start_background: false },
    )
    .unwrap();
    let router = app.router.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await;
    });
    (url, TestApp { app, peer: "198.51.100.7:50000".parse().unwrap() })
}

#[derive(Default)]
struct Seen {
    runs: Vec<Value>,
    approvals: Vec<Value>,
    patched: Vec<Value>,
}

#[derive(Clone)]
struct Fake {
    seen: Arc<Mutex<Seen>>,
    approved: Arc<Flag>,
}

/// A Hermes with one run (`run-1`): it asks an approval, and completes once it is answered.
async fn hermes(State(fake): State<Fake>, request: Request) -> Response {
    let method = request.method().as_str().to_string();
    let path = request.uri().path().to_string();
    let bytes = axum::body::to_bytes(request.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let json = |v: Value| -> Response { axum::response::IntoResponse::into_response(axum::Json(v)) };
    if method == "POST" && path.ends_with("/api/sessions") {
        return json(json!({"id": "s-marie-1"}));
    }
    if method == "PATCH" {
        fake.seen.lock().unwrap().patched.push(body);
        return json(json!({"ok": true}));
    }
    if method == "GET" && path.ends_with("/api/sessions/s-marie-1") {
        // her conversation's session, as the threads read it (a root: its own thread)
        return json(json!({"session": {"id": "s-marie-1", "title": "BipAgents · proche", "source": "api_server"}}));
    }
    if method == "GET" && path.ends_with("/messages") {
        let runs = fake.seen.lock().unwrap().runs.clone();
        let mut messages = Vec::new();
        for r in runs {
            messages.push(json!({"role": "user", "content": r["input"]}));
            if fake.approved.is_set() {
                messages.push(json!({"role": "assistant", "content": "Une fondue !\nMEDIA:/root/fondue.png"}));
            }
        }
        return json(json!({"data": messages}));
    }
    if method == "POST" && path.ends_with("/v1/runs") {
        fake.seen.lock().unwrap().runs.push(body);
        return json(json!({"run_id": "run-1"}));
    }
    if method == "POST" && path.ends_with("/v1/runs/run-1/approval") {
        fake.seen.lock().unwrap().approvals.push(body);
        fake.approved.set();
        return json(json!({"ok": true}));
    }
    if method == "GET" && path.ends_with("/v1/runs/run-1/events") {
        let approved = fake.approved.clone();
        let events = async_stream::stream! {
            yield Ok::<_, std::io::Error>(sse("approval.request", json!({"prompt_id": "req-1", "execution_generation": 1,
                "command": "rm -rf /tmp/x", "description": "delete a folder", "choices": ["once", "deny"]})));
            approved.wait(Duration::from_secs(20)).await;
            yield Ok(sse("run.completed", json!({"output": "Une fondue !"})));
        };
        let mut response = Response::new(Body::from_stream(events));
        response.headers_mut().insert("content-type", "text/event-stream".parse().unwrap());
        return response;
    }
    if method == "GET" && path.ends_with("/v1/runs/run-1") {
        if fake.approved.is_set() {
            return json(json!({"status": "completed", "session_id": "s-marie-1",
                               "usage": {"input_tokens": 1200, "output_tokens": 80}}));
        }
        return json(json!({"status": "waiting_for_approval", "session_id": "s-marie-1", "pending_controls": [
            {"kind": "approval", "prompt_id": "req-1", "execution_generation": 1, "choices": ["once", "deny"]}]}));
    }
    json(json!({"ok": true}))
}

#[tokio::test]
async fn no_family_agent_on_a_hermes_without_the_guest_turn_patch() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = common::config(dir.path(), &Urls::default());
    config.hermes.bin = "/opt/hermes".into();
    config.hermes.hub_url = "http://127.0.0.1:9".into();
    config.hermes.root = s(&dir.path().join("unpatched"));
    let app = TestApp::new(config);
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let got = app.send_json("POST", "/v1/family-agent", &h, &json!({"name": "Famille"})).await;
    // 400 with a sentence: both apps show a family agent's refusal in the bridge's words (409 reads as « exists »)
    assert_eq!(got.status, 400, "{}", got.text());
    assert_eq!(got.json()["code"], bipbridge::agent_shares::GUEST_TURNS_OPEN);
    assert!(got.json()["detail"].as_str().unwrap().ends_with("sur ton ordinateur."));
    assert_eq!(app.app.state.config().agents.len(), 1);
}

#[test]
fn the_offer_s_places_always_close_a_relative_s_turn() {
    // The shared Hermes of the offer is built with every patch (a failed one stops the build): no file to look for.
    let dir = tempfile::tempdir().unwrap();
    let mut d = config_dict(dir.path(), &Urls::default());
    d["hermes"] = json!({"root": s(&dir.path().join("nowhere"))});
    assert!(!bipbridge::agent_shares::guest_turns_closed(&parse_config(&d, None).unwrap(), None));
    d["cloud"] = json!(true);
    assert!(bipbridge::agent_shares::guest_turns_closed(&parse_config(&d, None).unwrap(), None));
    // an agent in its own space: its own Hermes code is the one checked
    let own = dir.path().join("space");
    let own_root = common::patched_hermes(&own);
    d["cloud"] = json!(false);
    let config = parse_config(&d, None).unwrap();
    let mut agent = config.agent(Some("wellness")).unwrap().clone();
    agent.hermes_root = Some(own_root.clone());
    assert!(bipbridge::agent_shares::guest_turns_closed(&config, Some(&agent)));
    // the patch's new file left behind (untracked: a `git reset --hard` or a stash keeps it) while its wiring in the
    // run API is gone: open
    let (wired, _) = bipbridge::agent_shares::GUEST_TURN_WIRING;
    std::fs::write(Path::new(&own_root).join(wired), "# stock Hermes\n").unwrap();
    assert!(!bipbridge::agent_shares::guest_turns_closed(&config, Some(&agent)));
}

#[test]
fn a_hermes_whose_code_folder_is_unknown_counts_as_open() {
    // Owner's decision (11/11): when the bridge cannot tell, relatives' turns are refused, not run open.
    let dir = tempfile::tempdir().unwrap();
    let mut d = config_dict(dir.path(), &Urls::default());
    d["hermes"] = json!({}); // set up by hand, or Hermes in a container: no root, no home
    assert!(!bipbridge::agent_shares::guest_turns_closed(&parse_config(&d, None).unwrap(), None));
    // its home known: the official install's folder `<home>/hermes-agent` is the one looked at
    let home = dir.path().join(".hermes");
    d["hermes"] = json!({"home": s(&home)});
    assert!(!bipbridge::agent_shares::guest_turns_closed(&parse_config(&d, None).unwrap(), None));
    common::patched_hermes(&home);
    assert!(bipbridge::agent_shares::guest_turns_closed(&parse_config(&d, None).unwrap(), None));
}

#[tokio::test]
async fn a_relative_reads_a_shared_agent_and_never_writes_to_it() {
    let fake = Fake { seen: Arc::new(Mutex::new(Seen::default())), approved: Flag::new() };
    let hermes_url = common::spawn_fake(Router::new().fallback(hermes).with_state(fake.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    // Alex's Hermes without patch 0018: a personal share needs none (no relative's turn)
    let code = dir.path().join("alex").join("hermes-agent");
    std::fs::create_dir_all(code.join("agent")).unwrap();
    let extra = json!({"hermes": {"root": s(&code)}});
    let (_, a) = serve_bridge_with(dir.path(), "a", &["cuisine", "budget"], &hermes_url, extra).await;
    let (_, b) = serve_bridge(dir.path(), "b", &["vie"], &hermes_url).await;
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(a.send_json("PUT", "/v1/family/name", &h, &json!({"name": "Alex"})).await.status, 200);
    assert_eq!(b.send_json("PUT", "/v1/family/name", &h, &json!({"name": "Marie"})).await.status, 200);
    let invitation = a.send_json("POST", "/v1/family/invite", &h, &json!({})).await.json();
    let joined = b
        .send_json("POST", "/v1/family/join", &h, &json!({"url": invitation["url"], "code": invitation["code"]}))
        .await;
    assert_eq!(joined.status, 200, "{}", joined.text());
    assert!(!bipbridge::agent_shares::guest_turns_closed(&a.app.state.config(), None));

    // Alex's side: shared with nobody yet; only linked households; every share asks to confirm, naming the tools
    let view = a.get("/v1/agents/cuisine/shares", &h).await.json();
    assert_eq!(
        view["households"],
        json!([{"id": "marie", "name": "Marie", "shared": false, "history": null, "since": null}])
    );
    assert_eq!(view["warning"], json!({"private_tools": ["notebook"], "confirm": true}));
    let bad = a.send_json("PUT", "/v1/agents/cuisine/shares", &h, &json!({"households": ["paul"]})).await;
    assert_eq!(bad.status, 400);
    let health = dir.path().join("a").join("health");
    std::fs::create_dir_all(&health).unwrap();
    std::fs::write(health.join("agents.json"), r#"{"agents": ["budget"]}"#).unwrap();
    let unconfirmed = a.send_json("PUT", "/v1/agents/budget/shares", &h, &json!({"households": ["marie"]})).await;
    assert_eq!(unconfirmed.status, 409);
    assert_eq!(unconfirmed.json()["warning"]["private_tools"], json!(["health", "notebook"]));
    assert!(unconfirmed.json()["detail"].as_str().unwrap().contains("ta santé"), "{}", unconfirmed.text());
    let asked = a.send_json("PUT", "/v1/agents/cuisine/shares", &h, &json!({"households": ["marie"]})).await;
    assert_eq!(asked.status, 409);
    assert_eq!(b.get("/v1/shared-agents", &h).await.json()["agents"], json!([]));

    // Shared, confirmed: Marie's bridge is told; the card waits for her « Ajouter »
    let body = json!({"households": ["marie"], "confirmed": true});
    let shared = a.send_json("PUT", "/v1/agents/cuisine/shares", &h, &body).await;
    assert_eq!(shared.status, 200, "{}", shared.text());
    assert_eq!(shared.json()["told"], json!({"marie": true}));
    assert_eq!(shared.json()["households"][0]["history"], "from_now");
    let mine = b.get("/v1/shared-agents", &h).await.json()["agents"].clone();
    assert_eq!((mine[0]["id"].as_str(), mine[0]["owner"].as_str()), (Some("cuisine__alex"), Some("Alex")));
    assert_eq!((mine[0]["state"].as_str(), mine[0]["read_only"].as_bool()), (Some("offered"), Some(true)));
    assert_eq!(b.request("POST", "/v1/shared-agents/cuisine__alex/add", &h, None).await.status, 200);

    // She can't write: refused on her bridge and on Alex's; nothing runs in Alex's Hermes
    let first = json!({"agent": "cuisine__alex", "client_id": "C0FFEE00-0000-4000-8000-000000000001",
                       "text": "Une idée de dîner ?"});
    let sent = b.send_json("POST", "/v1/threads/new/messages", &h, &first).await;
    assert_eq!(sent.status, 403, "{}", sent.text());
    assert_eq!(sent.json()["code"], "shared_read_only");
    let answered =
        b.send_json("POST", "/v1/approve", &h, &json!({"agent": "cuisine__alex", "run_id": "run-1", "choice": "once"})).await;
    assert_eq!(answered.status, 403);
    let key_out = b.app.state.households.get("alex").unwrap()["key_out"].as_str().unwrap().to_string();
    let theirs = format!("Bearer {key_out}");
    let hers = [("authorization", theirs.as_str())];
    let direct = a.send_json("POST", "/family/s/cuisine/conversations", &hers, &json!({})).await;
    assert_eq!(direct.status, 403, "{}", direct.text());
    assert_eq!(a.get("/family/s/cuisine/conversations", &hers).await.status, 403);
    assert!(fake.seen.lock().unwrap().runs.is_empty());
    // another agent of Alex's is not hers; nothing without her household's key
    assert_eq!(a.get("/family/s/budget/feed", &hers).await.status, 404);
    assert_eq!(a.get("/family/s/cuisine/feed", &[("authorization", "Bearer nope")]).await.status, 401);

    // Alex stops sharing: Marie's card goes, and his conversations with it
    let stopped = a.send_json("PUT", "/v1/agents/cuisine/shares", &h, &json!({"households": []})).await;
    assert_eq!(stopped.json()["told"], json!({"marie": true}));
    assert_eq!(b.get("/v1/shared-agents", &h).await.json()["agents"], json!([]));
    assert_eq!(a.get("/family/s/cuisine/feed", &hers).await.status, 404);
}
