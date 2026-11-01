//! What each agent may do with what the phone shares (phone_rights.rs): a file from before the rights gives them all,
//! a right taken back is refused at the tool's call (and its requests waiting are cancelled), the contacts' details
//! taken back are neither shown nor searched in, and those no agent reads any more aren't kept.

mod common;

use bipbridge::agent_tools;
use bipbridge::config::{parse_config, Config};
use common::{auth, config_dict, TestApp, Urls};
use indexmap::IndexMap;
use serde_json::{json, Value};

const TOKEN: &str = "rights-agent-token";

fn with_agents(dir: &std::path::Path) -> Config {
    let mut dict = config_dict(dir, &Urls::default());
    dict["hermes"] = json!({"bin": "/opt/hermes", "home": dir.join(".hermes").to_string_lossy(), "hub_url": "http://127.0.0.1:8642"});
    for (name, display) in [("coach", "Coach"), ("chef", "Chef")] {
        dict["agents"][name] = json!({"display_name": display, "hermes_url": format!("http://127.0.0.1:8642/p/{name}"),
                                      "hermes_key": "k"});
    }
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

/// A tool called by coach: (failed, its text).
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

#[tokio::test]
async fn the_reminders_read_and_changed_as_the_person_lets_each_agent() {
    let dir = tempfile::tempdir().unwrap();
    let config = with_agents(dir.path());
    let app = TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    app.send_json("PUT", "/v1/reminders/agents", &h, &json!({"agents": ["coach", "chef"]})).await;
    let copy =
        json!({"lists": [{"id": "L1", "title": "Courses"}], "items": [{"id": "r1", "list": "L1", "title": "Lait"}]});
    app.send_json("PUT", "/v1/reminders", &h, &copy).await;

    // A file from before the rights: everything, as before.
    let status = app.get("/v1/reminders", &h).await.json();
    assert_eq!(status["rights"]["coach"], json!({"read": true, "write": true}));
    assert!(tool(&app, "reminders_list", json!({})).await.1.contains("Lait"));
    let (failed, _) = tool(&app, "reminders_add", json!({"title": "Pain"})).await;
    assert!(!failed);

    assert_eq!(app.send_json("PUT", "/v1/reminders/rights", &h, &json!({"agent": "ghost"})).await.status, 400);
    let set =
        app.send_json("PUT", "/v1/reminders/rights", &h, &json!({"agent": "Coach", "rights": {"write": false}})).await;
    assert_eq!(set.json(), json!({"agent": "coach", "rights": {"read": true, "write": false}}));
    // Its request still waiting never reaches the phone.
    assert_eq!(app.get("/v1/reminders/queue", &h).await.json()["queue"], json!([]));
    let (failed, said) = tool(&app, "reminders_complete", json!({"id": "r1"})).await;
    assert!(failed && said.contains("doesn't let you add or tick off"), "{said}");
    assert!(tool(&app, "reminders_list", json!({})).await.1.contains("Lait"));

    app.send_json("PUT", "/v1/reminders/rights", &h, &json!({"agent": "coach", "rights": {"read": false}})).await;
    let (failed, said) = tool(&app, "reminders_list", json!({})).await;
    assert!(failed && !said.contains("Lait"), "{said}");
    // The other agent keeps everything.
    assert_eq!(app.get("/v1/reminders", &h).await.json()["rights"]["chef"], json!({"read": true, "write": true}));
}

#[tokio::test]
async fn the_phone_calendar_read_and_changed_as_the_person_lets_each_agent() {
    let dir = tempfile::tempdir().unwrap();
    let config = with_agents(dir.path());
    let app = TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    app.send_json("PUT", "/v1/calendar/agents", &h, &json!({"agents": ["coach"]})).await;
    let copy = json!({"from": "2026-10-14", "to": "2026-12-20", "calendars": [{"id": "C1", "title": "Perso"}],
        "events": [{"id": "e1", "calendar": "C1", "title": "Dentiste", "start": "2026-10-22T10:00:00+02:00",
                    "end": "2026-10-22T10:30:00+02:00", "all_day": false}]});
    app.send_json("PUT", "/v1/calendar", &h, &copy).await;
    let range = json!({"from": "2026-10-20", "to": "2026-10-25"});
    assert!(tool(&app, "calendar_list", range.clone()).await.1.contains("Dentiste"));
    assert_eq!(app.get("/v1/calendar", &h).await.json()["rights"], json!({"coach": {"read": true, "write": true}}));

    let (failed, _) = tool(&app, "calendar_add", json!({"title": "Yoga", "start": "2026-10-24T18:00"})).await;
    assert!(!failed);
    app.send_json("PUT", "/v1/calendar/rights", &h, &json!({"agent": "coach", "rights": {"write": false}})).await;
    assert_eq!(app.get("/v1/calendar/queue", &h).await.json()["queue"], json!([]));
    for (name, arguments) in [
        ("calendar_add", json!({"title": "Yoga", "start": "2026-10-24T18:00"})),
        ("calendar_update", json!({"id": "e1", "title": "Dentiste (annulé)"})),
        ("calendar_delete", json!({"id": "e1"})),
    ] {
        let (failed, said) = tool(&app, name, arguments).await;
        assert!(failed && said.contains("doesn't let you add, move or delete"), "{name}: {said}");
    }
    assert!(tool(&app, "calendar_list", range.clone()).await.1.contains("Dentiste"));

    app.send_json("PUT", "/v1/calendar/rights", &h, &json!({"agent": "coach", "rights": {"read": false}})).await;
    let (failed, said) = tool(&app, "calendar_list", range).await;
    assert!(failed && !said.contains("Dentiste"), "{said}");
    assert!(!bipbridge::calendar::reads(&config, "coach"));
}

fn copy() -> Value {
    json!({"groups": [{"id": "g1", "title": "Famille"}],
        "contacts": [
            {"id": "c1", "name": "Hélène Martin", "phones": [{"label": "mobile", "value": "+33 6 12 34 56 78"}],
             "emails": [{"value": "helene@example.org"}], "addresses": [{"value": "3 rue Basse, Lyon"}],
             "birthday": "1990-05-12", "organization": "Acme", "groups": ["g1"]},
            {"id": "c2", "name": "Zoé", "phones": [{"value": "07 00 00 00 01"}], "groups": ["g1"]}]})
}

#[tokio::test]
async fn the_contacts_details_shown_as_the_person_lets_each_agent() {
    let dir = tempfile::tempdir().unwrap();
    let config = with_agents(dir.path());
    let app = TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let path = std::path::PathBuf::from(config.data_dir()).join("contacts.json");
    let kept = || -> Value { serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap() };
    app.send_json("PUT", "/v1/contacts/agents", &h, &json!({"agents": ["coach", "chef"]})).await;
    app.send_json("PUT", "/v1/contacts", &h, &copy()).await;
    let status = app.get("/v1/contacts", &h).await.json();
    let all = json!({"addresses": true, "birthdays": true, "emails": true, "phones": true, "write": true});
    assert_eq!(status["rights"]["coach"], all);
    assert_eq!(status["shared"], json!({"addresses": true, "birthdays": true, "emails": true, "phones": true}));
    assert_eq!(tool_names(&app).await.iter().filter(|n| n.starts_with("contacts_")).count(), 5);
    let (_, found) = tool(&app, "contacts_search", json!({"query": "helene"})).await;
    assert!(found.contains("+33 6 12") && found.contains("Lyon") && found.contains("birthday: 1990-05-12"), "{found}");

    // Coach: no phones, no birthdays, no changes.
    let set = app
        .send_json(
            "PUT",
            "/v1/contacts/rights",
            &h,
            &json!({"agent": "coach", "rights": {"phones": false, "birthdays": false, "write": false}}),
        )
        .await;
    assert_eq!(set.json()["rights"]["phones"], false);
    let (_, found) = tool(&app, "contacts_search", json!({"query": "helene"})).await;
    assert!(!found.contains("+33") && !found.contains("birthday") && found.contains("helene@example.org"), "{found}");
    assert!(found.contains("Lyon") && found.contains("Acme"), "{found}");
    // Not found by the number it doesn't see either.
    assert!(tool(&app, "contacts_search", json!({"query": "06 12 34 56 78"})).await.1.starts_with("No contact"));
    let (_, family) = tool(&app, "contacts_list_group", json!({"group": "Famille"})).await;
    assert!(!family.contains("07 00") && family.contains("helene@example.org"), "{family}");
    let (failed, said) = tool(&app, "contacts_birthdays", json!({})).await;
    assert!(failed && said.contains("birthdays"), "{said}");
    let (failed, said) = tool(&app, "contacts_add", json!({"name": "Marc"})).await;
    assert!(failed && said.contains("add or change"), "{said}");
    let names = tool_names(&app).await;
    assert!(names.contains(&"contacts_search".to_string()) && !names.contains(&"contacts_birthdays".to_string()));
    assert!(!names.contains(&"contacts_add".to_string()) && !names.contains(&"contacts_update".to_string()));

    // Chef still reads them all: kept, and the phone keeps sending them.
    assert_eq!(kept()["contacts"][0]["phones"][0]["value"], "+33 6 12 34 56 78");
    // Chef stops reading the postal addresses too: nobody does, they go and the phone stops sending them.
    app.send_json("PUT", "/v1/contacts/rights", &h, &json!({"agent": "chef", "rights": {"addresses": false}})).await;
    app.send_json("PUT", "/v1/contacts/rights", &h, &json!({"agent": "coach", "rights": {"addresses": false}})).await;
    assert!(kept()["contacts"][0].get("addresses").is_none());
    assert_eq!(app.get("/v1/contacts", &h).await.json()["shared"]["addresses"], false);
    app.send_json("PUT", "/v1/contacts", &h, &copy()).await; // a phone that sends them anyway: not kept
    assert!(kept()["contacts"][0].get("addresses").is_none());
    assert_eq!(kept()["contacts"][0]["birthday"], "1990-05-12"); // chef reads them

    // Chef no longer allowed: its choices don't count, coach's do (no phones, no birthdays).
    app.send_json("PUT", "/v1/contacts/agents", &h, &json!({"agents": ["coach"]})).await;
    let contact = kept()["contacts"][0].clone();
    assert!(contact.get("phones").is_none() && contact.get("birthday").is_none(), "{contact}");
    assert_eq!(contact["emails"][0]["value"], "helene@example.org");
    let status = app.get("/v1/contacts", &h).await.json();
    assert_eq!(status["shared"], json!({"addresses": false, "birthdays": false, "emails": true, "phones": false}));
    assert_eq!(status["rights"]["chef"]["addresses"], false); // its choice kept for next time
}
