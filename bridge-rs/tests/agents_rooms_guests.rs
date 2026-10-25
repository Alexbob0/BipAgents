//! Group discussions, phase 2 (rooms_remote.rs): a relative's agent in a person's room, only along links both people
//! ticked, its turn run by its own bridge in a hidden conversation there, text only, its words marked as another
//! household's, its tokens on its own side.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use axum::Router;
use bipbridge::config::parse_config;
use common::{auth, config_dict, TestApp, Urls};
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

#[derive(Clone, Default)]
struct Fake {
    /// (path, body) of each run started and each session deleted.
    runs: Arc<Mutex<Vec<(String, Value)>>>,
    deleted: Arc<Mutex<Vec<String>>>,
}

async fn hermes(State(fake): State<Fake>, request: Request) -> Response {
    let method = request.method().as_str().to_string();
    let path = request.uri().path().to_string();
    let bytes = axum::body::to_bytes(request.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let out = |v: Value| axum::Json(v).into_response();
    if method == "POST" && path.ends_with("/api/sessions") {
        let n = fake.runs.lock().unwrap().len();
        return out(json!({"id": format!("s{n}-{}", path.replace('/', "_"))}));
    }
    if method == "DELETE" {
        fake.deleted.lock().unwrap().push(path);
        return out(json!({}));
    }
    if method == "POST" && path.ends_with("/v1/runs") {
        let mut runs = fake.runs.lock().unwrap();
        runs.push((path.clone(), body));
        return out(json!({"run_id": format!("run-{}", runs.len())}));
    }
    if method == "GET" && path.contains("/v1/runs/") {
        let output = if path.starts_with("/b/p/cuisine") {
            "Je propose une raclette.\nMEDIA:/root/raclette.png"
        } else {
            "D'accord."
        };
        return out(
            json!({"status": "completed", "output": output, "usage": {"input_tokens": 500, "output_tokens": 40}}),
        );
    }
    out(json!({"ok": true}))
}

async fn settled(app: &TestApp, room: &str) -> Value {
    let (k, v) = auth();
    for _ in 0..200 {
        let messages = app.get(&format!("/v1/rooms/{room}/messages"), &[(k, v.as_str())]).await.json();
        if messages["answering"] == json!([]) {
            return messages;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the room never settled");
}

#[tokio::test]
async fn a_relatives_agent_takes_part_only_where_both_people_allowed_it() {
    let fake = Fake::default();
    let hermes_url = common::spawn_fake(Router::new().fallback(hermes).with_state(fake.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let (_, a) = serve_bridge(dir.path(), "a", &["vie", "budget"], &hermes_url).await;
    let (_, b) = serve_bridge(dir.path(), "b", &["cuisine"], &hermes_url).await;
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(a.send_json("PUT", "/v1/family/name", &h, &json!({"name": "Alex"})).await.status, 200);
    assert_eq!(b.send_json("PUT", "/v1/family/name", &h, &json!({"name": "Léa"})).await.status, 200);
    let invitation = a.send_json("POST", "/v1/family/invite", &h, &json!({})).await.json();
    let joined = b
        .send_json("POST", "/v1/family/join", &h, &json!({"url": invitation["url"], "code": invitation["code"]}))
        .await;
    assert_eq!(joined.status, 200, "{}", joined.text());

    // Not before Alex ticks a link with Cuisine.
    let room = json!({"participants": ["vie", "cuisine@lea"], "title": "Dîner"});
    assert_eq!(a.send_json("POST", "/v1/rooms", &h, &room).await.status, 403);
    let links = json!({"links": [{"from": "vie", "to": "cuisine__lea"}]});
    assert_eq!(a.send_json("PUT", "/v1/agent-links", &h, &links).await.status, 200);
    assert_eq!(a.send_json("POST", "/v1/rooms", &h, &json!({"participants": ["cuisine@lea"]})).await.status, 422);
    let made = a.send_json("POST", "/v1/rooms", &h, &room).await;
    assert_eq!(made.status, 200, "{}", made.text());
    let made = made.json();
    let id = made["id"].as_str().unwrap().to_string();
    let guest = made["participants"][1].clone();
    assert_eq!(guest["id"], "cuisine__lea");
    assert_eq!(guest["name"], "Cuisine (Léa)");
    assert_eq!(guest["household_name"], "Léa");
    assert_eq!(guest["remote"], true);

    // Léa hasn't allowed it: her bridge refuses the turn, the room says Cuisine couldn't answer.
    let path = format!("/v1/rooms/{id}/messages");
    let posted = a.send_json("POST", &path, &h, &json!({"text": "@cuisine une idée pour ce soir ?"})).await;
    assert_eq!(posted.status, 200, "{}", posted.text());
    assert_eq!(posted.json()["route"]["agents"], json!(["cuisine__lea"]));
    let messages = settled(&a, &id).await;
    let last = messages["messages"].as_array().unwrap().last().unwrap().clone();
    assert_eq!((last["from"].clone(), last["kind"].clone()), (json!("cuisine__lea"), json!("notice")));
    assert_eq!(last["error"], "their person has not allowed this discussion");
    assert!(fake.runs.lock().unwrap().iter().all(|(p, _)| !p.starts_with("/b/")));

    // She allows « Vie (Alex) → Cuisine »: Cuisine answers from her bridge, text only, on her credit.
    let theirs = json!({"links": [{"from": "vie__alex", "to": "cuisine"}]});
    assert_eq!(b.send_json("PUT", "/v1/agent-links", &h, &theirs).await.status, 200);
    let posted = a.send_json("POST", &path, &h, &json!({"text": "@cuisine et maintenant ?"})).await;
    assert_eq!(posted.status, 200);
    let messages = settled(&a, &id).await;
    let last = messages["messages"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["from"], "cuisine__lea");
    assert_eq!(last["kind"], "agent", "{last}");
    assert_eq!(last["text"], "Je propose une raclette.");
    assert_eq!(last["files"], json!([]));
    let (run_path, run) =
        fake.runs.lock().unwrap().iter().find(|(p, _)| p.starts_with("/b/p/cuisine")).cloned().unwrap();
    assert_eq!(run_path, "/b/p/cuisine/v1/runs");
    let input = run["input"].as_str().unwrap();
    assert!(input.contains("[Alex, a relative of your person, who created this discussion]"), "{input}");
    assert!(input.contains("@cuisine et maintenant ?"));
    assert!(run["instructions"].as_str().unwrap().contains("Alex is not your person"));
    let view = a.get(&format!("/v1/rooms/{id}"), &h).await.json();
    assert_eq!(view["tokens"], json!({"input": 0, "output": 0}));

    // Vie's next turn reads Cuisine's words as another household's.
    let posted = a.send_json("POST", &path, &h, &json!({"text": "@vie qu'en dis-tu ?"})).await;
    assert_eq!(posted.status, 200);
    settled(&a, &id).await;
    let (_, vie) = fake.runs.lock().unwrap().iter().rev().find(|(p, _)| p.starts_with("/a/p/vie")).cloned().unwrap();
    assert!(vie["input"].as_str().unwrap().contains("(@cuisine__lea), an agent of another household"));

    // Deleted: Cuisine's hidden conversation on Léa's side goes too.
    assert_eq!(a.request("DELETE", &format!("/v1/rooms/{id}"), &h, None).await.status, 200);
    assert!(fake.deleted.lock().unwrap().iter().any(|p| p.starts_with("/b/p/cuisine/api/sessions/")));
    // A wrong key reaches nothing.
    let nope = b.send_json("POST", "/family/r/cuisine/turn", &[("authorization", "Bearer nope")], &json!({})).await;
    assert_eq!(nope.status, 401);
}
