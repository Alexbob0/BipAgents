//! Family news between agents (family_news.rs): an agent's `tell_family` is only a proposal; the person's « Envoyer »
//! delivers it to the linked household's bridge, which keeps it, notes it for its agent and refuses a wrong key.

mod common;

use std::path::Path;

use axum::routing::any;
use axum::Router;
use bipbridge::config::parse_config;
use bipbridge::{agent_tools, family_news};
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

/// The agent's own MCP token (as `agent_tools::register` would have given it).
fn give_token(app: &TestApp, agent: &str, token: &str) {
    let config = app.app.state.config();
    let mut tokens: IndexMap<String, String> = agent_tools::load_tokens(&config);
    tokens.insert(agent.into(), agent_tools::digest(token));
    agent_tools::save_tokens(&config, &tokens).unwrap();
}

async fn mcp(app: &TestApp, token: &str, method: &str, params: Value) -> Value {
    let bearer = format!("Bearer {token}");
    let answer = app
        .send_json(
            "POST",
            "/v1/agent-tools/mcp",
            &[("authorization", &bearer)],
            &json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
        )
        .await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    answer.json()["result"].clone()
}

fn tool_names(result: &Value) -> Vec<String> {
    result["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
}

fn text_of(result: &Value) -> String {
    result["content"][0]["text"].as_str().unwrap_or("").to_string()
}

#[tokio::test]
async fn told_only_after_the_person_says_so() {
    let hermes = common::spawn_fake(Router::new().fallback(any(|| async { axum::Json(json!({"ok": true})) }))).await;
    let dir = tempfile::tempdir().unwrap();
    let (_, a) = serve_bridge(dir.path(), "a", &["vie"], &hermes).await;
    let (_, b) = serve_bridge(dir.path(), "b", &["cuisine", "budget"], &hermes).await;
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    give_token(&a, "vie", "vie-token");

    // No household linked: no family tools, and the tool says so if called anyway.
    assert!(!tool_names(&mcp(&a, "vie-token", "tools/list", json!({})).await).contains(&"tell_family".to_string()));
    let alone = mcp(
        &a,
        "vie-token",
        "tools/call",
        json!({"name": "tell_family", "arguments": {"to": ["lea"], "message": "x"}}),
    )
    .await;
    assert_eq!(alone["isError"], true);
    let before = agent_tools::tools_url(&a.app.state.config());

    // Linked (Alex invites, Léa joins).
    assert_eq!(a.send_json("PUT", "/v1/family/name", &h, &json!({"name": "Alex"})).await.status, 200);
    assert_eq!(b.send_json("PUT", "/v1/family/name", &h, &json!({"name": "Léa"})).await.status, 200);
    let invitation = a.send_json("POST", "/v1/family/invite", &h, &json!({"days": 7})).await.json();
    let joined = b
        .send_json("POST", "/v1/family/join", &h, &json!({"url": invitation["url"], "code": invitation["code"]}))
        .await;
    assert_eq!(joined.status, 200, "{}", joined.text());
    let names = tool_names(&mcp(&a, "vie-token", "tools/list", json!({})).await);
    assert!(names.contains(&"tell_family".to_string()) && names.contains(&"family_members".to_string()));
    assert_ne!(agent_tools::tools_url(&a.app.state.config()), before); // Hermes fetches the new list
    let listed = text_of(&mcp(&a, "vie-token", "tools/call", json!({"name": "family_members", "arguments": {}})).await);
    assert!(listed.contains("Léa (`lea`)") && listed.contains("Cuisine (`cuisine__lea`)"), "{listed}");

    // The agent proposes: recorded, nothing sent.
    let proposed = mcp(
        &a,
        "vie-token",
        "tools/call",
        json!({"name": "tell_family", "arguments": {"to": ["lea"], "message": "Je commande à manger ce midi, tu veux quelque chose ?"}}),
    )
    .await;
    assert_eq!(proposed["isError"], false, "{proposed}");
    assert!(text_of(&proposed).contains("nothing is sent yet"));
    let news = a.get("/v1/family/news?agent=vie", &h).await.json();
    assert_eq!(news["linked"], true);
    let proposal = news["proposals"][0].clone();
    assert_eq!(proposal["to"], json!([{"id": "lea", "name": "Léa"}]));
    assert_eq!(b.get("/v1/family/news", &h).await.json()["received"], json!([]));
    let wrong = mcp(
        &a,
        "vie-token",
        "tools/call",
        json!({"name": "tell_family", "arguments": {"to": ["marc"], "message": "x"}}),
    )
    .await;
    assert_eq!(wrong["isError"], true);

    // « Envoyer », the words changed: Léa's bridge keeps it for her first agent and notes it for its next turn.
    let id = proposal["id"].as_str().unwrap();
    let sent = a
        .send_json(
            "POST",
            &format!("/v1/family/proposals/{id}/send"),
            &h,
            &json!({"text": "Je commande des pizzas ce midi !"}),
        )
        .await;
    assert_eq!(sent.status, 200, "{}", sent.text());
    assert_eq!(sent.json(), json!({"sent": ["Léa"], "failed": []}));
    assert_eq!(a.get("/v1/family/news?agent=vie", &h).await.json()["proposals"], json!([]));
    let received = b.get("/v1/family/news?agent=cuisine", &h).await.json()["received"].clone();
    assert_eq!(received[0]["from"], "Alex");
    assert_eq!(received[0]["from_agent"], "Vie");
    assert_eq!(received[0]["text"], "Je commande des pizzas ce midi !");
    let recap =
        b.send_json("POST", "/v1/live/recaps/take", &h, &json!({"agent": "cuisine", "session": "s1"})).await.json();
    let note = recap["recap"].as_str().unwrap();
    assert!(
        note.starts_with("<!--bip:family")
            && note.contains("Alex (through their agent Vie): « Je commande des pizzas ce midi ! »")
    );
    // taken once
    let again =
        b.send_json("POST", "/v1/live/recaps/take", &h, &json!({"agent": "cuisine", "session": "s1"})).await.json();
    assert_eq!(again["recap"], Value::Null);
    // the sender's agent knows it went
    let mine = a.send_json("POST", "/v1/live/recaps/take", &h, &json!({"agent": "vie", "session": "s2"})).await.json();
    assert!(mine["recap"].as_str().unwrap().contains("passed on to Léa"));
    // closed by Léa
    let rid = received[0]["id"].as_str().unwrap();
    assert_eq!(b.request("POST", &format!("/v1/family/received/{rid}/seen"), &h, None).await.status, 200);
    assert_eq!(b.get("/v1/family/news?agent=cuisine", &h).await.json()["received"], json!([]));

    // To one of Léa's agents by name, then « Non merci »: nothing goes, the agent is told.
    mcp(
        &a,
        "vie-token",
        "tools/call",
        json!({"name": "tell_family", "arguments": {"to": ["budget__lea"], "message": "Loyer payé"}}),
    )
    .await;
    let second = a.get("/v1/family/news", &h).await.json()["proposals"][0].clone();
    assert_eq!(second["to"][0]["id"], "budget__lea");
    let sid = second["id"].as_str().unwrap();
    assert_eq!(a.request("DELETE", &format!("/v1/family/proposals/{sid}"), &h, None).await.status, 200);
    assert_eq!(a.get("/v1/family/news", &h).await.json()["proposals"], json!([]));
    assert_eq!(b.get("/v1/family/news?agent=budget", &h).await.json()["received"], json!([]));
    let declined =
        a.send_json("POST", "/v1/live/recaps/take", &h, &json!({"agent": "vie", "session": "s2"})).await.json();
    assert!(declined["recap"].as_str().unwrap().contains("chose not to pass on"));
    assert_eq!(a.request("DELETE", &format!("/v1/family/proposals/{sid}"), &h, None).await.status, 404);

    // The receiving route wants the household's own key.
    let bad = b.send_json("POST", "/family/tell", &[("authorization", "Bearer nope")], &json!({"text": "hi"})).await;
    assert_eq!(bad.status, 401);
    let none = b.send_json("POST", "/family/tell", &[], &json!({"text": "hi"})).await;
    assert_eq!(none.status, 401);
    assert_eq!(b.get("/v1/family/news", &h).await.json()["received"].as_array().unwrap().len(), 0);

    // « Nouvelles passées »: what went, what was refused (Alex's side), what came in (Léa's), the newest first.
    let past = a.get("/v1/family/history", &h).await.json();
    assert_eq!(past["more"], false);
    let items = past["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{past}");
    assert_eq!(items[0]["state"], "refused");
    assert_eq!(items[0]["household_name"], "Léa");
    assert_eq!(items[0]["other_agent"], "Budget");
    assert_eq!(items[0]["text"], "Loyer payé");
    assert_eq!(items[1]["state"], "sent");
    assert_eq!(items[1]["direction"], "sent");
    assert_eq!(items[1]["agent"], "vie");
    assert_eq!(items[1]["agent_name"], "Vie");
    assert_eq!(items[1]["household"], "lea");
    assert_eq!(items[1]["other_agent"], Value::Null);
    assert_eq!(items[1]["text"], "Je commande des pizzas ce midi !");
    let page = a.get("/v1/family/history?limit=1", &h).await.json();
    assert_eq!((page["items"].as_array().unwrap().len(), page["more"].clone()), (1, json!(true)));
    let before = page["items"][0]["at"].as_f64().unwrap();
    let next = a.get(&format!("/v1/family/history?limit=1&before={before}"), &h).await.json();
    assert_eq!(next["items"][0]["state"], "sent");
    let theirs = b.get("/v1/family/history?direction=received", &h).await.json();
    assert_eq!(theirs["items"][0]["household_name"], "Alex");
    assert_eq!(theirs["items"][0]["other_agent"], "Vie");
    assert_eq!(theirs["items"][0]["agent"], "cuisine");
    assert_eq!(theirs["items"][0]["state"], "received");
    assert_eq!(b.get("/v1/family/history?direction=sent", &h).await.json()["items"], json!([]));
    assert_eq!(b.get("/v1/family/history?direction=x", &h).await.status, 422);

    // Unlinked: the tools go.
    assert_eq!(b.request("DELETE", "/v1/family/alex", &h, None).await.status, 204);
    assert_eq!(a.request("DELETE", "/v1/family/lea", &h, None).await.status, 204);
    assert!(!tool_names(&mcp(&a, "vie-token", "tools/list", json!({})).await).contains(&"tell_family".to_string()));
}

#[tokio::test]
async fn a_proposal_waits_a_day_and_an_unreachable_family_keeps_it() {
    let dir = tempfile::tempdir().unwrap();
    let (_, a) = serve_bridge(dir.path(), "a", &["vie"], "http://127.0.0.1:9").await;
    let config = a.app.state.config();
    // A household whose bridge does not answer.
    let households = bipbridge::households::Households::new(&bipbridge::households::path_for(&config));
    households.set_name("Alex").unwrap();
    households.joined("Léa", "http://127.0.0.1:9", &"o".repeat(40), &"i".repeat(40), None).unwrap();
    let family = [family_news::Member { id: "lea".into(), name: "Léa".into(), agents: vec![] }];
    let agent = config.agent(Some("vie")).unwrap().clone();
    let now = bipbridge::py::time_time();
    let old = family_news::propose(&config, &agent, &family, &["lea".into()], "hier", now - 25.0 * 3600.0).unwrap();
    let fresh = family_news::propose(&config, &agent, &family, &["everyone".into()], "aujourd'hui", now).unwrap();
    let news = family_news::news(&config, None, now);
    let ids: Vec<&str> = news["proposals"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [fresh["id"].as_str().unwrap()]); // the day-old one is gone
    assert!(ids.iter().all(|i| *i != old["id"].as_str().unwrap()));
    // Nobody reached: 502, the proposal stays for « Envoyer » again.
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let id = fresh["id"].as_str().unwrap();
    assert_eq!(a.request("POST", &format!("/v1/family/proposals/{id}/send"), &h, None).await.status, 502);
    assert_eq!(family_news::news(&config, None, now)["proposals"].as_array().unwrap().len(), 1);
    // Too long, or empty: refused before anything is kept.
    assert!(family_news::propose(&config, &agent, &family, &["lea".into()], &"x".repeat(1001), now).is_err());
    assert!(family_news::propose(&config, &agent, &family, &["lea".into()], "", now).is_err());
}
