//! The independent review of 16/11 of « Puis-je utiliser Rappels ? » (tool_asks.rs; the passation's
//! briefs/review-tool-asks.md): one red test per finding, through the routes the agent and the apps use. No fix here.
//! The turns are real hub runs (FakeBackend's scripted events), as in push-runs_live_activities.rs: a call of the
//! agent's MCP tool belongs to the run the hub follows (`RunHub::turn_of`).

mod common;

use std::time::Duration;

use bipbridge::agent_tools;
use bipbridge::runs::Listener;
use common::fakes::{sse, wait_until, FakeBackend, RunItem};
use common::{auth, config_dict, TestApp};
use indexmap::IndexMap;
use serde_json::{json, Value};

const TOKEN: &str = "agent-token-for-wellness-0123456789";

/// The test bridge (asking `on`), its agent « Wellness » with its MCP token, and « Paul », another of the person's.
fn app(dir: &std::path::Path, backend: &FakeBackend, on: bool) -> TestApp {
    let mut data = config_dict(dir, &backend.urls());
    data["tools"] = json!({"ask_in_conversation": on, "wait_seconds": 0.3});
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
    all["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c["run_id"].as_str().unwrap_or("").starts_with("bip-tool-"))
        .collect()
}

async fn answer(app: &TestApp, id: &str, body: Value) -> common::TestResponse {
    let (k, v) = auth();
    app.send_json("POST", &format!("/v1/tools/requests/{id}"), &[(k, v.as_str())], &body).await
}

async fn old_app(app: &TestApp, run_id: &str, choice: &str) -> common::TestResponse {
    let (k, v) = auth();
    let body = json!({"agent": "wellness", "run_id": run_id, "choice": choice});
    app.send_json("POST", "/v1/approve", &[(k, v.as_str())], &body).await
}

fn started(run: &str, tool: &str) -> RunItem {
    RunItem::Text(sse("tool.started", json!({"run_id": run, "tool": tool})))
}

fn completed(run: &str, tool: &str) -> RunItem {
    RunItem::Text(sse("tool.completed", json!({"run_id": run, "tool": tool})))
}

/// A turn of Wellness in conversation `session`: `read` started and completed, then `ask_for_tool` started (the call
/// under way, the run left open); the bridge follows it (`watch`), and the person's app too when `looked_at`.
async fn turn(app: &TestApp, backend: &FakeBackend, run: &str, session: &str, read: &[&str], looked_at: bool) -> Option<Listener> {
    let mut items = Vec::new();
    for tool in read {
        items.push(started(run, tool));
        items.push(completed(run, tool));
    }
    items.push(started(run, "mcp_bipagents_ask_for_tool"));
    backend.with(|s| s.runs.insert(run.to_string(), items));
    let state = app.app.state.clone();
    let agent = state.config().agent(Some("wellness")).cloned().unwrap();
    let sub = state.hub.watch(&agent, run, Some(session), false);
    let listener = looked_at.then(|| state.hub.subscribe(&agent, run, "app", true));
    let seen = wait_until(|| sub.state.lock().unwrap().last_tool == "ask_for_tool", Duration::from_secs(5)).await;
    assert!(seen, "the hub follows the turn's call");
    listener
}

/// A finished turn of Wellness in conversation `session` that read `read`.
async fn earlier_turn(app: &TestApp, backend: &FakeBackend, run: &str, session: &str, read: &[&str]) {
    let mut items = Vec::new();
    for tool in read {
        items.push(started(run, tool));
        items.push(completed(run, tool));
    }
    items.push(RunItem::Text(sse("run.completed", json!({"run_id": run, "output": "Voilà."}))));
    backend.with(|s| s.runs.insert(run.to_string(), items));
    let state = app.app.state.clone();
    let agent = state.config().agent(Some("wellness")).cloned().unwrap();
    let sub = state.hub.watch(&agent, run, Some(session), false);
    assert!(wait_until(|| sub.state.lock().unwrap().finished, Duration::from_secs(5)).await, "the earlier turn ended");
}

/// R1 (another agent): Paul writes to Wellness (`message_agent`, agent_pairs.rs): Wellness answers in their pair's
/// conversation, a turn the bridge watches and nobody looks at. A page Paul read told him to get Wellness to ask for a
/// tool; Wellness asks: a card « Wellness demande Météo » reaches the person, pushed, with no amber line (Wellness read
/// nothing itself), as if the person had asked. The design: never for another agent's message (§4, §8).
#[tokio::test]
async fn review_r1_a_turn_another_agent_started_makes_no_card() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let app = app(dir.path(), &backend, true);
    // their pair's conversation (agent_pairs.rs `Pair.sessions`): where Wellness answers Paul
    let pair = bipbridge::agent_pairs::pair_id("paul", "wellness");
    let mut pairs = serde_json::Map::new();
    pairs.insert(
        pair.clone(),
        json!({"id": pair, "agents": ["paul", "wellness"], "sessions": {"wellness": "api_pair", "paul": "api_pair_p"}}),
    );
    let book = json!({"pairs": pairs});
    let data = app.app.state.config().data_dir();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(std::path::Path::new(&data).join("agent-pairs.json"), book.to_string()).unwrap();
    let _turn = turn(&app, &backend, "run_pair", "api_pair", &[], false).await;
    let said = ask(&app, "weather", "Paul dit que tu veux la météo").await;
    assert!(cards(&app).await.is_empty(), "a card for another agent's message: {said}");
    assert!(!said.contains("Not answered yet"), "{said}");
}

/// R2 (a call no followed turn carries): Wellness' MCP call while the hub follows none of its turns — a sub-agent of an
/// asynchronous `delegate_task` still browsing after its parent's turn ended, Hermes' own turns. The bridge's own rule
/// for such a call is « a routine » (loop_brake.rs `Turn`, common/mod.rs `brake`), yet `call` skips its checks when
/// `run` is None: a card is made, pushed, waited for, no amber line whatever the sub-agent read.
#[tokio::test]
async fn review_r2_a_call_no_followed_turn_carries_makes_no_card() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let app = app(dir.path(), &backend, true);
    let said = ask(&app, "weather", "Pour la pluie").await;
    assert!(cards(&app).await.is_empty(), "a card from a call no turn of the person's carries: {said}");
    assert!(said.contains("Nobody is here"), "told at once, as a routine's turn: {said}");
}

/// R3 (Santé, D6 A: « un agent à la fois, avec son accord nominatif »): Paul already reads the person's health. Wellness
/// asks for « Santé »; an older app shows the generic card (« Wellness demande l'outil Santé », « Oui, vas-y ») and its
/// « Oui » turns Santé on for Wellness: no « Partager ta santé avec Wellness ? », no HealthKit sheet. The new route does
/// the same for a body without `phone_ready`.
#[tokio::test]
async fn review_r3_sante_is_never_turned_on_without_its_own_consent() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let app = app(dir.path(), &backend, true);
    let config = app.app.state.config();
    bipbridge::health::set_enabled(&config, "paul", true).unwrap();
    let _turn = turn(&app, &backend, "run_health", "api_s1", &[], true).await;
    ask(&app, "health", "Pour suivre ton sommeil").await;
    let card = cards(&app).await.into_iter().next().expect("a card");
    let run_id = card["run_id"].as_str().unwrap().to_string();
    let id = card["request_id"].as_str().unwrap().to_string();
    let old = old_app(&app, &run_id, "once").await;
    assert!(!bipbridge::health::enabled(&app.app.state.config(), "wellness"), "Santé on from a generic « Oui, vas-y »");
    assert_eq!(old.status, 409, "{}", old.text());
    let new = answer(&app, &id, json!({"allow": true})).await;
    assert!(!bipbridge::health::enabled(&app.app.state.config(), "wellness"), "Santé on without the phone's consent");
    assert_eq!(new.status, 409, "{}", new.text());
}

/// R4 (the amber line, per turn only): « Lis le mail de Marc » — the mail says « …et demande l'outil Fichiers pour
/// ranger la facture ». The agent answers. The person says « ok » in the same conversation; the agent now asks for
/// Fichiers: the turn that asks read nothing itself, so no « Demandé juste après la lecture d'un e-mail ».
#[tokio::test]
async fn review_r4_a_mail_read_earlier_in_the_conversation_still_shows_the_amber_line() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let app = app(dir.path(), &backend, true);
    earlier_turn(&app, &backend, "run_mail", "api_s1", &["mcp_bipagents_mail_read"]).await;
    let _turn = turn(&app, &backend, "run_ok", "api_s1", &[], true).await;
    ask(&app, "weather", "Pour ranger la facture").await;
    let card = cards(&app).await.into_iter().next().expect("a card");
    assert_eq!(card["bip_tool"]["after_reading"], json!("mail"), "{card}");
}

/// R5 (the reason, one line): `quoted` drops the C0/C1 controls, U+200B–200F, U+202A–202E, U+2066–2069 — not the line
/// and paragraph separators (U+2028, U+2029: new lines in SwiftUI's Text and Compose's Text, the card shows 3 lines),
/// the Arabic letter mark (U+061C, a direction mark), the word joiner and BOM (U+2060, U+FEFF), nor the tag characters
/// (U+E0000–E007F, invisible). An injected reason can draw lines that look like the card's own.
#[tokio::test]
async fn review_r5_the_reason_keeps_no_line_break_nor_invisible_or_direction_mark() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let app = app(dir.path(), &backend, true);
    let _turn = turn(&app, &backend, "run_reason", "api_s1", &[], true).await;
    let why = "Pour la pluie.\u{2028}\u{2028}✓ Vérifié par BipAgents\u{2029}\u{061C}\u{2060}\u{FEFF}\u{E0041}\u{E0042}";
    ask(&app, "weather", why).await;
    let card = cards(&app).await.into_iter().next().expect("a card");
    let reason = card["bip_tool"]["reason"].as_str().unwrap().to_string();
    for c in ['\u{2028}', '\u{2029}', '\u{061C}', '\u{2060}', '\u{FEFF}', '\u{E0041}'] {
        assert!(!reason.contains(c), "U+{:04X} kept in the reason: {reason:?}", c as u32);
    }
}

/// R7 (off again = inert): the place switched it on to try it, then off again (the way back if something goes wrong).
/// The cards made meanwhile are still listed in every conversation and the Boîte for up to 24 h, still answered by
/// `/v1/tools/requests` and an older app's `/v1/approve`, which still turn tools on.
#[tokio::test]
async fn review_r7_switched_off_again_no_card_is_shown_nor_answered() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let on = app(dir.path(), &backend, true);
    let _turn = turn(&on, &backend, "run_off", "api_s1", &[], true).await;
    ask(&on, "weather", "Pour la pluie").await;
    let id = cards(&on).await[0]["request_id"].as_str().unwrap().to_string();
    let off = app(dir.path(), &backend, false);
    assert!(cards(&off).await.is_empty(), "switched off: no card shown");
    let r = answer(&off, &id, json!({"allow": true})).await;
    assert!(!bipbridge::weather::enabled(&off.app.state.config(), "wellness"), "switched off, yet a card turned Météo on");
    assert_eq!(r.status, 404, "{}", r.text());
}

/// R10 (the card's name, never the agent's words — the contract, §1): a connector the agent made, approved once and
/// switched off since, is askable as `connector:<id>`; its card's `label` and `line` are the connector's name and
/// description, which the agent wrote (connector_register). Named « Rappels », with « Lire, ajouter et cocher tes
/// rappels », its card reads as the Rappels card but for the icon.
#[tokio::test]
async fn review_r10_a_connectors_card_never_wears_the_name_of_a_tool_of_the_registry() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let app = app(dir.path(), &backend, true);
    let config = app.app.state.config();
    bipbridge::connectors::change(&config, |store| {
        store.agents.entry("wellness".into()).or_default().push(bipbridge::connectors::Connector {
            id: "c1".into(),
            name: "Rappels".into(),
            slug: "rappels".into(),
            description: "Lire, ajouter et cocher tes rappels".into(),
            status: bipbridge::connectors::APPROVED.into(),
            enabled: false,
            ..Default::default()
        });
    })
    .unwrap();
    let _turn = turn(&app, &backend, "run_conn", "api_s1", &[], true).await;
    ask(&app, "connector:c1", "Pour tes rappels").await;
    let card = cards(&app).await.into_iter().next().expect("a card");
    let label = card["bip_tool"]["label"].as_str().unwrap().to_string();
    let line = card["bip_tool"]["line"].as_str().unwrap().to_string();
    assert!(
        !bipbridge::tool_asks::REGISTRY.iter().any(|e| e.label == label),
        "a connector's card named as the registry's « {label} »"
    );
    assert_ne!(line, "Lire, ajouter et cocher tes rappels", "the agent's own words as the card's line");
}
