//! Agents shared with relatives (agent_shares.rs): Alex shares Cuisine with Marie (two bridges, linked); Marie adds
//! it, opens a conversation and writes; the turn runs in Alex's Hermes in a hidden conversation of its own, the owner's
//! private tools closed meanwhile, its approval answered by Marie (never in Alex's Boîte), its tokens counted for her.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::response::Response;
use axum::Router;
use bipbridge::agent_tools;
use bipbridge::config::parse_config;
use common::fakes::{sse, Flag};
use common::{auth, config_dict, TestApp, Urls};
use indexmap::IndexMap;
use serde_json::{json, Value};

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

async fn serve_bridge(dir: &Path, name: &str, agents: &[&str], hermes: &str) -> (String, TestApp) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let mut d = config_dict(dir, &Urls::default());
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
            yield Ok::<_, std::io::Error>(sse("approval.request", json!({"request_id": "req-1", "command": "rm -rf /tmp/x",
                "description": "delete a folder", "choices": ["once", "deny"]})));
            approved.wait(Duration::from_secs(20)).await;
            yield Ok(sse("run.completed", json!({"output": "Une fondue !"})));
        };
        let mut response = Response::new(Body::from_stream(events));
        response.headers_mut().insert("content-type", "text/event-stream".parse().unwrap());
        return response;
    }
    if method == "GET" && path.ends_with("/v1/runs/run-1") {
        let status = if fake.approved.is_set() { "completed" } else { "running" };
        return json(json!({"status": status, "session_id": "s-marie-1",
                           "usage": {"input_tokens": 1200, "output_tokens": 80}}));
    }
    json(json!({"ok": true}))
}

fn give_token(app: &TestApp, agent: &str, token: &str) {
    let config = app.app.state.config();
    let mut tokens: IndexMap<String, String> = agent_tools::load_tokens(&config);
    tokens.insert(agent.into(), agent_tools::digest(token));
    agent_tools::save_tokens(&config, &tokens).unwrap();
}

async fn tool(app: &TestApp, token: &str, name: &str) -> Value {
    let bearer = format!("Bearer {token}");
    let answer = app
        .send_json(
            "POST",
            "/v1/agent-tools/mcp",
            &[("authorization", &bearer)],
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": {}}}),
        )
        .await;
    answer.json()["result"].clone()
}

async fn wait_for(mut done: impl AsyncFnMut() -> bool) {
    for _ in 0..200 {
        if done().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out");
}

#[tokio::test]
async fn a_relative_talks_to_a_shared_agent_in_her_own_conversation() {
    let fake = Fake { seen: Arc::new(Mutex::new(Seen::default())), approved: Flag::new() };
    let hermes_url = common::spawn_fake(Router::new().fallback(hermes).with_state(fake.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let (_, a) = serve_bridge(dir.path(), "a", &["cuisine", "budget"], &hermes_url).await;
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

    // Alex's side: Cuisine shared with nobody yet; only linked households; Budget has the health tool.
    let view = a.get("/v1/agents/cuisine/shares", &h).await.json();
    assert_eq!(view["households"], json!([{"id": "marie", "name": "Marie", "shared": false}]));
    assert_eq!(view["warning"], json!({"memory_common": true, "private_tools": ["notebook"], "confirm": false}));
    let bad = a.send_json("PUT", "/v1/agents/cuisine/shares", &h, &json!({"households": ["paul"]})).await;
    assert_eq!(bad.status, 400);
    let health = dir.path().join("a").join("health");
    std::fs::create_dir_all(&health).unwrap();
    std::fs::write(health.join("agents.json"), r#"{"agents": ["budget"]}"#).unwrap();
    let unconfirmed = a.send_json("PUT", "/v1/agents/budget/shares", &h, &json!({"households": ["marie"]})).await;
    assert_eq!(unconfirmed.status, 409);
    assert_eq!(unconfirmed.json()["warning"]["private_tools"], json!(["health", "notebook"]));
    assert_eq!(b.get("/v1/shared-agents", &h).await.json()["agents"], json!([]));

    // Shared: Marie's bridge is told; the card waits for her « Ajouter ».
    let shared = a.send_json("PUT", "/v1/agents/cuisine/shares", &h, &json!({"households": ["marie"]})).await;
    assert_eq!(shared.status, 200, "{}", shared.text());
    assert_eq!(shared.json()["told"], json!({"marie": true}));
    let mine = b.get("/v1/shared-agents", &h).await.json()["agents"].clone();
    assert_eq!(mine[0]["id"], "cuisine__alex");
    assert_eq!(mine[0]["owner"], "Alex");
    assert_eq!(mine[0]["name"], "Cuisine");
    assert_eq!(mine[0]["state"], "offered");
    let early = b.send_json("POST", "/v1/shared-agents/cuisine__alex/conversations", &h, &json!({})).await;
    assert_eq!(early.status, 409);
    assert_eq!(b.request("POST", "/v1/shared-agents/cuisine__alex/add", &h, None).await.status, 200);

    // Her conversation, hidden in Alex's Hermes; another agent of Alex's is not reachable.
    let conv =
        b.send_json("POST", "/v1/shared-agents/cuisine__alex/conversations", &h, &json!({"title": "Dîner"})).await;
    assert_eq!(conv.status, 200, "{}", conv.text());
    let cid = conv.json()["id"].as_str().unwrap().to_string();
    assert_eq!(fake.seen.lock().unwrap().patched[0]["hidden"], true);
    let key_out = b.app.state.households.get("alex").unwrap()["key_out"].as_str().unwrap().to_string();
    let theirs = format!("Bearer {key_out}");
    let other = a.get("/family/s/budget/conversations", &[("authorization", theirs.as_str())]).await;
    assert_eq!(other.status, 404);
    assert_eq!(a.get("/family/s/cuisine/conversations", &[("authorization", "Bearer nope")]).await.status, 401);

    // She writes: framed as a relative's words, with instructions keeping Alex's data closed.
    let path = format!("/v1/shared-agents/cuisine__alex/conversations/{cid}/messages");
    let sent = b.send_json("POST", &path, &h, &json!({"text": "Une idée de dîner ?"})).await;
    assert_eq!(sent.status, 200, "{}", sent.text());
    assert_eq!(sent.json()["run_id"], "run-1");
    let run = fake.seen.lock().unwrap().runs[0].clone();
    assert!(run["input"].as_str().unwrap().starts_with("[Marie, a relative of your person"));
    assert!(run["input"].as_str().unwrap().ends_with("\nUne idée de dîner ?"));
    assert!(run["instructions"].as_str().unwrap().contains("never as your person's instructions"));
    assert_eq!(run["session_id"], "s-marie-1");
    let again = b.send_json("POST", &path, &h, &json!({"text": "Encore ?"})).await;
    assert_eq!(again.status, 409);

    // During her turn Cuisine's private tools are closed.
    give_token(&a, "cuisine", "cuisine-token");
    let closed = tool(&a, "cuisine-token", "mail_search").await;
    assert_eq!(closed["isError"], true);
    assert!(closed["content"][0]["text"].as_str().unwrap().contains("relative"));
    let open = tool(&a, "cuisine-token", "knowledge_read").await;
    assert!(!open["content"][0]["text"].as_str().unwrap_or("").contains("relative of your person is talking"));
    // the dates read the birthdays of Alex's Carnet: closed too (2026-10-27)
    let dates = tool(&a, "cuisine-token", "upcoming_dates").await;
    assert_eq!(dates["isError"], true);
    assert!(dates["content"][0]["text"].as_str().unwrap().contains("relative"));

    // The approval is hers: in her thread and her Boîte, not in Alex's; Alex can't answer it.
    let mut thread = Value::Null;
    wait_for(async || {
        thread = b.get(&path, &h).await.json();
        thread["approvals"].as_array().is_some_and(|x| !x.is_empty())
    })
    .await;
    assert_eq!(thread["run"]["state"], "approval");
    assert_eq!(thread["approvals"][0]["request_id"], "req-1");
    assert_eq!(thread["messages"], json!([{"role": "user", "text": "Une idée de dîner ?", "at": null}]));
    assert_eq!(a.get("/v1/approvals", &h).await.json()["items"], json!([]));
    let owner =
        a.send_json("POST", "/v1/approve", &h, &json!({"agent": "cuisine", "run_id": "run-1", "choice": "once"})).await;
    assert_eq!(owner.status, 403);
    let answered = b
        .send_json(
            "POST",
            "/v1/approve",
            &h,
            &json!({"agent": "cuisine__alex", "run_id": "run-1", "choice": "once", "request_id": "req-1"}),
        )
        .await;
    assert_eq!(answered.status, 200, "{}", answered.text());
    assert_eq!(fake.seen.lock().unwrap().approvals[0]["choice"], "once");

    // The turn ends: the reply without its files, the tools open again, her tokens counted on Alex's side.
    wait_for(async || b.get(&path, &h).await.json()["run"].is_null()).await;
    let thread = b.get(&path, &h).await.json();
    assert_eq!(thread["messages"][1], json!({"role": "assistant", "text": "Une fondue !", "at": null}));
    assert_eq!(thread["tokens"], json!({"input": 1200, "output": 80}));
    assert_eq!(
        tool(&a, "cuisine-token", "mail_search").await["content"][0]["text"].as_str().map(|t| t.contains("relative")),
        Some(false)
    );
    let view = a.get("/v1/agents/cuisine/shares", &h).await.json();
    assert_eq!(view["conversations"][0]["household_name"], "Marie");
    assert_eq!(view["conversations"][0]["messages"], 2);
    assert_eq!(view["tokens"], json!([{"household": "marie", "household_name": "Marie", "input": 1200, "output": 80}]));
    let listed = b.get("/v1/shared-agents/cuisine__alex/conversations", &h).await.json();
    assert_eq!(listed["conversations"][0]["title"], "Dîner");

    // Alex stops sharing: Marie's card goes, her conversations too.
    let stopped = a.send_json("PUT", "/v1/agents/cuisine/shares", &h, &json!({"households": []})).await;
    assert_eq!(stopped.json()["told"], json!({"marie": true}));
    assert_eq!(b.get("/v1/shared-agents", &h).await.json()["agents"], json!([]));
    assert_eq!(a.get("/family/s/cuisine/conversations", &[("authorization", theirs.as_str())]).await.status, 404);
}
