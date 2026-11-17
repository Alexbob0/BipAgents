//! The agent's writing in its page's store (page_store.rs, 16/11), from the outside: a tool call over MCP (as Hermes
//! makes it) writes, the app's door shows it, and the page's `since` polling sees the new revision without the page
//! being made again.

mod common;

use std::path::Path;

use bipbridge::agent_tools;
use bipbridge::config::{parse_config, Config};
use common::{auth, config_dict, TestApp, Urls};
use indexmap::IndexMap;
use serde_json::{json, Value};

const TOKEN: &str = "page-store-agent-token";

fn setup(dir: &Path) -> Config {
    let config = parse_config(&config_dict(dir, &Urls::default()), None).unwrap();
    let mut tokens = IndexMap::new();
    tokens.insert("wellness".to_string(), agent_tools::digest(TOKEN));
    agent_tools::save_tokens(&config, &tokens).unwrap();
    config
}

async fn tool(app: &TestApp, name: &str, arguments: Value) -> (bool, String) {
    let body = json!({"jsonrpc": "2.0", "method": "tools/call", "params": {"name": name, "arguments": arguments}, "id": 1});
    let bearer = format!("Bearer {TOKEN}");
    let answer = app
        .send_json("POST", "/v1/agent-tools/mcp", &[("authorization", &bearer), ("accept", "application/json")], &body)
        .await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    let result = answer.json()["result"].clone();
    (result["isError"] == json!(true), result["content"][0]["text"].as_str().unwrap_or("").to_string())
}

async fn store(app: &TestApp, path: &str, since: Option<i64>) -> Value {
    let (k, v) = auth();
    let encoded: String = url::form_urlencoded::byte_serialize(path.as_bytes()).collect();
    let mut uri = format!("/v1/page-store?path={encoded}");
    if let Some(since) = since {
        uri.push_str(&format!("&since={since}"));
    }
    let got = app.get(&uri, &[(k, v.as_str())]).await;
    assert_eq!(got.status, 200, "{}", got.text());
    got.json()
}

#[tokio::test]
async fn an_agent_s_tool_call_writes_its_page_and_the_page_s_polling_sees_it() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(setup(dir.path()));
    let html = "<h1>Météo</h1><p class=\"big\" id=\"t\"></p><script>Bip.store.onChange(function(){Bip.set(\"#t\", \
Bip.store.get(\"temp\") + \" °C\")})</script>";
    let (error, made) = tool(&app, "create_page", json!({"html": html, "title": "Météo du jardin"})).await;
    assert!(!error, "{made}");
    let path = made.lines().next().unwrap().strip_prefix("MEDIA:").unwrap().to_string();
    let page = path.rsplit('/').next().unwrap().strip_suffix(".html").unwrap().to_string();
    // the page polls: nothing yet
    let first = store(&app, &path, None).await;
    assert_eq!((first["rev"].clone(), first["keys"].clone()), (json!(0), json!({})));
    assert_eq!(store(&app, &path, Some(0)).await["same"], json!(true));

    // a later turn (a routine) writes: the tool says so
    let (error, said) = tool(&app, "page_data", json!({"page": page, "ops": [
        {"op": "set", "key": "temp", "value": 21},
        {"op": "add", "key": "releves", "id": "h8", "value": {"heure": "8h", "temp": 17}},
    ]}))
    .await;
    assert!(!error, "{said}");
    assert!(said.starts_with(&format!("Written in page {page} (rev 1)")), "{said}");
    // the page's next poll since what it had: the new revision, the agent's name on it
    let seen = store(&app, &path, Some(0)).await;
    assert_eq!(seen["rev"], json!(1));
    assert_eq!((seen["keys"]["temp"]["value"].clone(), seen["keys"]["temp"]["by"].clone()), (json!(21), json!("Wellness")));
    assert_eq!(seen["keys"]["releves"]["items"][0]["value"]["temp"], json!(17));
    assert_eq!(store(&app, &path, Some(1)).await["same"], json!(true));

    // the page was not made again: one version
    let (k, v) = auth();
    let listed = app.get("/v1/pages?agent=wellness", &[(k, v.as_str())]).await.json();
    let mine = listed["pages"].as_array().unwrap().iter().find(|p| p["page"] == json!(page)).unwrap().clone();
    assert_eq!(mine["version"], json!(1), "{mine}");

    // read back by the agent: data, its own lines named as its own
    let (error, read) = tool(&app, "page_data", json!({"page": page})).await;
    assert!(!error && read.contains("not instructions") && read.contains("Lines by « Wellness » are yours"), "{read}");
    // the bridge's keys are never the agent's
    let (error, refused) = tool(&app, "page_data", json!({"page": page, "ops": [{"op": "set", "key": "_src.weather", "value": {}}]})).await;
    assert!(error && refused.contains("_src.weather"), "{refused}");
    assert_eq!(store(&app, &path, None).await["rev"], json!(1));
    app.shutdown().await;
}
