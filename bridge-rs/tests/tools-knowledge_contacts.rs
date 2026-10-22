//! The person's address book (contacts.rs): the copy the phone sends, the person's choice (all or some groups, the
//! notes or not), the agents allowed (and their tools, listed only for them), the agents' requests waiting for the
//! phone.

mod common;

use bipbridge::config::{parse_config, Config};
use bipbridge::{agent_tools, contacts};
use common::{auth, config_dict, TestApp, Urls};
use indexmap::IndexMap;
use serde_json::{json, Value};

const TOKEN: &str = "contacts-agent-token";

fn with_agent(dir: &std::path::Path) -> Config {
    let mut dict = config_dict(dir, &Urls::default());
    dict["hermes"] = json!({"bin": "/opt/hermes", "home": dir.join(".hermes").to_string_lossy(), "hub_url": "http://127.0.0.1:8642"});
    dict["agents"]["coach"] =
        json!({"display_name": "Coach", "hermes_url": "http://127.0.0.1:8642/p/coach", "hermes_key": "k"});
    let config = parse_config(&dict, None).unwrap();
    let mut tokens = IndexMap::new();
    tokens.insert("coach".to_string(), agent_tools::digest(TOKEN));
    agent_tools::save_tokens(&config, &tokens).unwrap();
    config
}

async fn rpc(app: &TestApp, method: &str, params: Value) -> Value {
    let body = json!({"jsonrpc": "2.0", "method": method, "params": params, "id": 1});
    let bearer = format!("Bearer {TOKEN}");
    let answer = app
        .send_json("POST", "/v1/agent-tools/mcp", &[("authorization", &bearer), ("accept", "application/json")], &body)
        .await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    answer.json()["result"].clone()
}

async fn tool(app: &TestApp, name: &str, arguments: Value) -> (bool, String) {
    let result = rpc(app, "tools/call", json!({"name": name, "arguments": arguments})).await;
    (result["isError"] == true, result["content"][0]["text"].as_str().unwrap_or("").to_string())
}

async fn tool_names(app: &TestApp) -> Vec<String> {
    rpc(app, "tools/list", json!({})).await["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

/// A birthday `days` from today, as the phone writes it (YYYY-MM-DD).
fn in_days(days: i64, year: i32) -> String {
    let day = chrono::Local::now().date_naive() + chrono::Duration::days(days);
    format!("{year:04}-{}", day.format("%m-%d"))
}

fn copy() -> Value {
    json!({
        "groups": [{"id": "g1", "title": "Famille"}, {"id": "g2", "title": "Travail"}],
        "contacts": [
            {"id": "c1", "name": "Hélène Martin", "nickname": "Lili",
             "phones": [{"label": "mobile", "value": "+33 6 12 34 56 78"}],
             "emails": [{"label": "home", "value": "helene@example.org"}],
             "addresses": [{"label": "home", "value": "3 rue Basse, Lyon"}],
             "birthday": in_days(3, 1990), "relations": [{"label": "mother", "name": "Anne"}],
             "groups": ["g1"], "notes": "aime les tulipes"},
            {"id": "c2", "name": "Paul Durand", "organization": "Acme", "job": "comptable",
             "emails": [{"value": "paul@acme.test"}], "birthday": format!("--{}", &in_days(40, 2000)[5..]),
             "groups": ["g2"]},
            {"id": "c3", "name": "Zoé", "phones": [{"value": "07 00 00 00 01"}], "birthday": in_days(0, 2015),
             "groups": ["g1"]}]})
}

#[tokio::test]
async fn the_phone_shares_its_address_book_and_does_what_the_agents_ask() {
    let dir = tempfile::tempdir().unwrap();
    let config = with_agent(dir.path());
    let app = TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(app.get("/v1/contacts", &[]).await.status, 401);
    assert!(!tool_names(&app).await.iter().any(|n| n.starts_with("contacts_"))); // not shared with it: not listed
    assert!(tool(&app, "contacts_search", json!({"query": "x"})).await.1.contains("hasn't shared"));
    let tools_before = agent_tools::version_of(&agent_tools::tools_for(&config, config.agent(Some("coach")).unwrap()));

    // By default: the whole address book, without the notes.
    let seen = app.get("/v1/contacts", &h).await.json();
    assert_eq!(seen["scope"], json!({"all": true, "groups": [], "notes": false}));
    assert_eq!(app.send_json("PUT", "/v1/contacts/agents", &h, &json!({"agents": ["ghost"]})).await.status, 400);
    let allowed = app.send_json("PUT", "/v1/contacts/agents", &h, &json!({"agents": ["Coach", "coach"]})).await;
    assert_eq!(allowed.json(), json!({"agents": ["coach"]}));
    assert_eq!(tool_names(&app).await.iter().filter(|n| n.starts_with("contacts_")).count(), 5);
    let tools_after = agent_tools::version_of(&agent_tools::tools_for(&config, config.agent(Some("coach")).unwrap()));
    assert_ne!(tools_before, tools_after); // a new entry: its Hermes fetches the list again

    assert_eq!(app.send_json("PUT", "/v1/contacts", &h, &copy()).await.json(), json!({"contacts": 3}));
    let too_long = json!({"contacts": [{"id": "x".repeat(301)}]});
    assert_eq!(app.send_json("PUT", "/v1/contacts", &h, &too_long).await.status, 422);
    let too_many = json!({"contacts": [{"id": "x", "phones": vec![json!({"value": "1"}); 31]}]});
    assert_eq!(app.send_json("PUT", "/v1/contacts", &h, &too_many).await.status, 422);

    let (failed, found) = tool(&app, "contacts_search", json!({"query": "helene"})).await;
    assert!(!failed, "{found}");
    assert!(found.starts_with("1 found, as the phone shared it 0 min ago:"), "{found}");
    assert!(found.contains("- id c1 · Hélène Martin (« Lili »)"), "{found}");
    assert!(found.contains("phones: mobile: +33 6 12 34 56 78") && found.contains("groups: Famille"), "{found}");
    assert!(found.contains("relations: mother: Anne") && found.contains("address: home: 3 rue Basse, Lyon"));
    assert!(!found.contains("tulipes"), "{found}"); // the notes stay on the phone unless ticked
    assert!(tool(&app, "contacts_search", json!({"query": "06 12 34 56 78"})).await.1.contains("c1"));
    assert!(tool(&app, "contacts_search", json!({"query": "acme"})).await.1.contains("Paul Durand · Acme, comptable"));
    assert!(tool(&app, "contacts_search", json!({"query": "nobody"})).await.1.starts_with("No contact matches"));
    assert!(tool(&app, "contacts_search", json!({})).await.0);
    let (_, family) = tool(&app, "contacts_search", json!({"query": "", "group": "famille"})).await;
    assert!(family.starts_with("2 found"), "{family}");
    assert!(tool(&app, "contacts_search", json!({"query": "a", "group": "Voisins"})).await.1.contains("no such group"));

    let (_, soon) = tool(&app, "contacts_birthdays", json!({})).await;
    let lines: Vec<&str> = soon.lines().skip(1).collect();
    assert_eq!(lines.len(), 2, "{soon}"); // Paul's is in 40 days
    use chrono::Datelike;
    let turns = (chrono::Local::now().date_naive() + chrono::Duration::days(3)).year() - 1990;
    assert!(lines[0].contains("(today) · Zoé"));
    assert!(lines[1].contains(&format!("(in 3 days) · Hélène Martin · turns {turns}")), "{soon}");
    let (_, later) = tool(&app, "contacts_birthdays", json!({"days": 60, "group": "Travail"})).await;
    assert!(later.contains("(in 40 days) · Paul Durand · id c2") && !later.contains("turns"), "{later}");
    assert!(tool(&app, "contacts_birthdays", json!({"days": 400})).await.0);

    let (_, groups) = tool(&app, "contacts_list_group", json!({})).await;
    assert!(groups.contains("- Famille (2)") && groups.contains("- Travail (1)"), "{groups}");
    let (_, family) = tool(&app, "contacts_list_group", json!({"group": "Famille"})).await;
    assert!(family.contains("- id c1 · Hélène Martin · +33 6 12 34 56 78 · helene@example.org"), "{family}");
    assert!(family.contains("- id c3 · Zoé · 07 00 00 00 01"));

    let (failed, asked) = tool(
        &app,
        "contacts_add",
        json!({"name": "Marc Petit", "phones": ["06 99 99 99 99"], "emails": "marc@example.org",
               "birthday": "--03-15", "group": "famille"}),
    )
    .await;
    assert!(!failed && asked.contains("next sync"), "{asked}");
    assert!(tool(&app, "contacts_add", json!({"name": " "})).await.0);
    assert!(tool(&app, "contacts_add", json!({"name": "x", "birthday": "bientôt"})).await.0);
    assert!(tool(&app, "contacts_add", json!({"name": "x", "group": "Voisins"})).await.0);
    assert!(tool(&app, "contacts_update", json!({"id": "zz", "job": "x"})).await.0);
    assert!(tool(&app, "contacts_update", json!({"id": "c2"})).await.1.contains("nothing to change"));
    assert!(tool(&app, "contacts_update", json!({"id": "c1", "notes": "x"})).await.1.contains("notes"));
    assert!(tool(&app, "contacts_update", json!({"id": "c1", "name": ""})).await.0);
    assert!(
        !tool(&app, "contacts_update", json!({"id": "c2", "job": "directeur", "add_emails": ["p@d.test"]})).await.0
    );
    assert!(tool(&app, "contacts_delete", json!({"id": "c2"})).await.0); // no such tool

    let queue = app.get("/v1/contacts/queue", &h).await.json()["queue"].clone();
    let ops: Vec<(String, Value)> = queue
        .as_array()
        .unwrap()
        .iter()
        .map(|op| (op["op"].as_str().unwrap().to_string(), op["args"].clone()))
        .collect();
    assert_eq!(
        ops,
        vec![
            (
                "add".into(),
                json!({"name": "Marc Petit", "phones": ["06 99 99 99 99"], "emails": ["marc@example.org"],
                       "birthday": "--03-15", "group": "g1"})
            ),
            ("update".into(), json!({"id": "c2", "job": "directeur", "emails": ["p@d.test"]})),
        ]
    );
    assert_eq!(queue[0]["agent"], "coach");
    let first = queue[0]["id"].as_str().unwrap();
    let answered = app.send_json("POST", &format!("/v1/contacts/queue/{first}"), &h, &json!({"done": true})).await;
    assert_eq!(answered.json(), json!({"ok": true}));
    assert_eq!(app.send_json("POST", "/v1/contacts/queue/nope", &h, &json!({"done": true})).await.status, 404);
    let seen = app.get("/v1/contacts", &h).await.json();
    assert_eq!(
        (&seen["agents"], &seen["groups"], &seen["contacts"], &seen["pending"]),
        (&json!(["coach"]), &json!(2), &json!(3), &json!(1))
    );

    use std::os::unix::fs::PermissionsExt;
    let stored = dir.path().join("data/contacts.json");
    assert_eq!(std::fs::metadata(&stored).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(!std::fs::read_to_string(&stored).unwrap().contains("tulipes"));

    // Only the family, with the notes: the rest goes at once, and the next copies are narrowed the same.
    let scope = json!({"all": false, "groups": ["g1"], "notes": true});
    assert_eq!(app.send_json("PUT", "/v1/contacts/scope", &h, &scope).await.json(), json!({"scope": scope}));
    assert_eq!(app.get("/v1/contacts", &h).await.json()["contacts"], 2);
    assert_eq!(app.send_json("PUT", "/v1/contacts", &h, &copy()).await.json(), json!({"contacts": 2}));
    assert!(tool(&app, "contacts_search", json!({"query": "helene"})).await.1.contains("notes: aime les tulipes"));
    assert!(tool(&app, "contacts_search", json!({"query": "paul"})).await.1.starts_with("No contact"));
    assert!(tool(&app, "contacts_list_group", json!({"group": "Travail"})).await.0);
    assert!(!tool(&app, "contacts_update", json!({"id": "c1", "notes": "aime les roses"})).await.0);

    // No longer shared with any agent: its tools go, it is told so, and the copy goes.
    assert_eq!(
        app.send_json("PUT", "/v1/contacts/agents", &h, &json!({"agents": []})).await.json(),
        json!({"agents": []})
    );
    assert!(!contacts::enabled(&config, "coach"));
    assert!(!tool_names(&app).await.iter().any(|n| n.starts_with("contacts_")));
    assert!(tool(&app, "contacts_search", json!({"query": "helene"})).await.1.contains("hasn't shared"));
    assert_eq!(app.get("/v1/contacts", &h).await.json()["contacts"], 0);
}

#[tokio::test]
async fn health_advertises_the_contacts() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(with_agent(dir.path()));
    let features = app.get("/health", &[]).await.json()["features"].clone();
    assert!(features.as_array().unwrap().contains(&json!("contacts")));
}
