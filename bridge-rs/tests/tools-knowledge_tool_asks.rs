//! « Puis-je utiliser Rappels ? » (tool_asks.rs, 16/11): an agent asks for a tool it has off; the person answers on a
//! card. Through the routes the agent and the apps use: the MCP tool, `GET /v1/approvals`, `POST /v1/tools/requests`,
//! an older app's `/v1/approve`.

mod common;

use bipbridge::agent_tools;
use common::fakes::FakeBackend;
use common::{auth, config_dict, TestApp};
use indexmap::IndexMap;
use serde_json::{json, Value};

const TOKEN: &str = "agent-token-for-wellness-0123456789";

/// The test bridge, its agent « Wellness » with its MCP token; asking on (`on`), the call's wait `wait` seconds.
fn app(dir: &std::path::Path, backend: &FakeBackend, on: bool, wait: f64) -> TestApp {
    let mut data = config_dict(dir, &backend.urls());
    data["tools"] = json!({"ask_in_conversation": on, "wait_seconds": wait});
    // a second agent of the person's, for « Aussi pour… »
    let mut paul = data["agents"]["Wellness"].clone();
    paul["display_name"] = json!("Paul");
    data["agents"]["paul"] = paul;
    let config = bipbridge::config::parse_config(&data, None).expect("valid config");
    let mut tokens = IndexMap::new();
    tokens.insert("wellness".to_string(), agent_tools::digest(TOKEN));
    agent_tools::save_tokens(&config, &tokens).unwrap();
    TestApp::new(config)
}

async fn rpc(app: &TestApp, method: &str, params: Value) -> Value {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let bearer = format!("Bearer {TOKEN}");
    app.send_json("POST", "/v1/agent-tools/mcp", &[("authorization", &bearer), ("accept", "application/json")], &body)
        .await
        .json()
}

async fn ask(app: &TestApp, tool: &str, why: &str) -> String {
    let answer = rpc(app, "tools/call", json!({"name": "ask_for_tool", "arguments": {"tool": tool, "why": why}})).await;
    answer["result"]["content"][0]["text"].as_str().unwrap_or("").to_string()
}

async fn cards(app: &TestApp) -> Vec<Value> {
    let (k, v) = auth();
    let all = app.get("/v1/approvals?agent=wellness", &[(k, v.as_str())]).await.json();
    all["items"].as_array().cloned().unwrap_or_default().into_iter().filter(|c| c["run_id"].as_str().unwrap_or("").starts_with("bip-tool-")).collect()
}

async fn answer(app: &TestApp, id: &str, body: Value) -> common::TestResponse {
    let (k, v) = auth();
    app.send_json("POST", &format!("/v1/tools/requests/{id}"), &[(k, v.as_str())], &body).await
}

fn enum_of(tools: &Value) -> Option<Vec<String>> {
    let list = tools["result"]["tools"].as_array()?;
    let ask = list.iter().find(|t| t["name"] == "ask_for_tool")?;
    Some(ask["inputSchema"]["properties"]["tool"]["enum"].as_array()?.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
}

#[tokio::test]
async fn the_tool_is_listed_only_where_the_place_offers_it_with_what_the_agent_lacks() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let off = app(dir.path(), &backend, false, 0.3);
    assert!(enum_of(&rpc(&off, "tools/list", json!({})).await).is_none(), "off by default: not listed, no token spent");
    let text = ask(&off, "reminders", "Pour le pain").await;
    assert!(text.contains("isn't available"), "{text}");
    let on = app(dir.path(), &backend, true, 0.3);
    let ids = enum_of(&rpc(&on, "tools/list", json!({})).await).expect("listed");
    for id in ["reminders", "calendar", "contacts", "health", "mail", "weather"] {
        assert!(ids.contains(&id.to_string()), "{id} in {ids:?}");
    }
    assert!(!ids.iter().any(|i| i == "apple_notes" || i == "custom"), "never askable: {ids:?}");
}

#[tokio::test]
async fn a_card_waits_then_oui_turns_the_phone_tool_on_for_the_agent_and_paul() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let app = app(dir.path(), &backend, true, 0.3);
    let text = ask(&app, "reminders", "Pour te rappeler le pain demain à 18 h").await;
    // the app's parts in the agent's language (names.rs: « Reminders » for an English one)
    assert!(text.contains("Not answered yet") && (text.contains("Rappels") || text.contains("Reminders")), "{text}");
    let waiting = cards(&app).await;
    assert_eq!(waiting.len(), 1, "{waiting:?}");
    let card = &waiting[0];
    assert_eq!(card["bip_tool"]["tool"], "reminders");
    assert_eq!(card["bip_tool"]["kind"], "phone");
    assert_eq!(card["bip_tool"]["reason"], "Pour te rappeler le pain demain à 18 h");
    assert_eq!(card["bip_tool"]["also"][0]["agent"], "paul");
    assert_eq!(card["choices"], json!(["once", "deny"]));
    let id = card["request_id"].as_str().unwrap().to_string();
    // no phone shares the reminders yet and the app didn't say it just did: the phone's part first
    let early = answer(&app, &id, json!({"allow": true})).await;
    assert_eq!(early.status, 409);
    assert_eq!(early.json()["detail"]["code"], "needs_phone");
    // an agent the card didn't offer
    assert_eq!(answer(&app, &id, json!({"allow": true, "also": ["stranger"], "phone_ready": true})).await.status, 400);
    let done = answer(&app, &id, json!({"allow": true, "also": ["paul"], "phone_ready": true})).await;
    assert_eq!(done.status, 200, "{}", done.text());
    let body = done.json();
    assert_eq!(body["state"], "on");
    assert_eq!(body["agents"], json!(["wellness", "paul"]));
    assert_eq!(body["line"], "Rappels activé pour Wellness et Paul");
    let config = app.app.state.config();
    assert!(bipbridge::reminders::allowed(&config, "wellness") && bipbridge::reminders::allowed(&config, "paul"));
    assert!(cards(&app).await.is_empty(), "answered: gone");
    assert_eq!(answer(&app, &id, json!({"allow": true})).await.status, 404, "answered once");
    // on now: not asked any more
    let ids = enum_of(&rpc(&app, "tools/list", json!({})).await).unwrap_or_default();
    assert!(!ids.contains(&"reminders".to_string()), "{ids:?}");
}

#[tokio::test]
async fn the_waiting_call_gets_the_answer_and_the_agent_goes_on_in_the_same_turn() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let app = app(dir.path(), &backend, true, 20.0);
    let asking = ask(&app, "weather", "Pour savoir s'il pleut demain");
    let answering = async {
        for _ in 0..100 {
            if let Some(card) = cards(&app).await.first() {
                let id = card["request_id"].as_str().unwrap().to_string();
                return answer(&app, &id, json!({"allow": true})).await.status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("no card");
    };
    let (text, status) = tokio::join!(asking, answering);
    assert_eq!(status, 200);
    assert!(text.contains("is on for you now") && (text.contains("Météo") || text.contains("Weather")), "{text}");
    assert!(bipbridge::weather::enabled(&app.app.state.config(), "wellness"));
}

#[tokio::test]
async fn non_holds_for_the_conversation_and_one_card_waits_at_a_time() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let app = app(dir.path(), &backend, true, 0.3);
    ask(&app, "contacts", "Pour trouver le numéro de Marc").await;
    let other = ask(&app, "weather", "Pour la pluie").await;
    assert!(other.contains("already waits"), "{other}");
    let id = cards(&app).await[0]["request_id"].as_str().unwrap().to_string();
    let refused = answer(&app, &id, json!({"allow": false})).await;
    assert_eq!(refused.json()["state"], "refused");
    let again = ask(&app, "contacts", "Vraiment besoin").await;
    assert!(again.contains("said no"), "{again}");
    assert!(cards(&app).await.is_empty());
}

#[tokio::test]
async fn an_older_app_turns_on_what_the_bridge_can_alone_and_is_told_where_to_go_for_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let app = app(dir.path(), &backend, true, 0.3);
    let (k, v) = auth();
    // a server tool: on from the generic card's « Oui, vas-y »
    ask(&app, "weather", "Pour la pluie").await;
    let run = cards(&app).await[0]["run_id"].as_str().unwrap().to_string();
    let ok = app.send_json("POST", "/v1/approve", &[(k, v.as_str())], &json!({"agent": "wellness", "run_id": run, "choice": "once"})).await;
    assert_eq!(ok.status, 200, "{}", ok.text());
    assert!(bipbridge::weather::enabled(&app.app.state.config(), "wellness"));
    // the phone's calendar no phone shares yet: plain words, the card stays
    ask(&app, "calendar", "Pour ton rendez-vous").await;
    let run = cards(&app).await[0]["run_id"].as_str().unwrap().to_string();
    let no = app.send_json("POST", "/v1/approve", &[(k, v.as_str())], &json!({"agent": "wellness", "run_id": run, "choice": "once"})).await;
    assert_eq!(no.status, 409);
    assert!(no.text().contains("Applis et outils"), "{}", no.text());
    assert_eq!(cards(&app).await.len(), 1);
    // its notification's « Refuser » (or the card's « Non »)
    let deny = app.send_json("POST", "/v1/approve", &[(k, v.as_str())], &json!({"agent": "wellness", "run_id": run, "choice": "deny"})).await;
    assert_eq!(deny.status, 200);
    assert!(cards(&app).await.is_empty());
}

#[tokio::test]
async fn after_arreter_a_late_oui_turns_the_tool_on_but_never_starts_the_agent_again() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let app = app(dir.path(), &backend, true, 0.3);
    ask(&app, "weather", "Pour la pluie").await;
    let card = cards(&app).await[0].clone();
    let id = card["request_id"].as_str().unwrap().to_string();
    let run_id = card["run_id"].as_str().unwrap().to_string();
    let config = app.app.state.config();
    // the person stopped the turn that asked (« Arrêter »)
    assert!(bipbridge::tool_asks::mark_stopped(&config, &id));
    assert_eq!(cards(&app).await.len(), 1, "still answerable");
    let done = answer(&app, &id, json!({"allow": true})).await;
    assert_eq!(done.status, 200, "{}", done.text());
    assert!(bipbridge::weather::enabled(&config, "wellness"), "« Oui » still turns it on");
    let place = bipbridge::card_place::of(&config, &run_id).expect("placed where it was asked");
    assert!(place.taken, "kept for no turn: nothing resumes the agent after a stop");
}

#[tokio::test]
async fn an_account_is_connected_in_the_apps_screen_never_by_the_card_alone() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let app = app(dir.path(), &backend, true, 0.3);
    ask(&app, "mail", "Pour chercher la facture de mars").await;
    let card = cards(&app).await[0].clone();
    assert_eq!(card["bip_tool"]["kind"], "account");
    assert_eq!(card["bip_tool"]["connected"], false, "« Oui, connecter »: no mailbox yet");
    let id = card["request_id"].as_str().unwrap().to_string();
    let r = answer(&app, &id, json!({"allow": true})).await;
    assert_eq!(r.status, 409);
    assert_eq!(r.json()["detail"]["code"], "needs_connection");
    assert_eq!(cards(&app).await.len(), 1, "still waiting for the connection");
}
