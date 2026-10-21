//! tests/test_console.py: the web console, logging in by a one-time link or a code approved in the app, its
//! session standing for the bridge key, and Hermes keys never reaching the browser.

mod common;

use common::fakes::FakeBackend;
use common::{auth, config, TestApp, TestResponse, Urls};
use serde_json::json;

fn session_cookie(resp: &TestResponse) -> String {
    let set = resp.header("set-cookie").expect("a session cookie");
    assert!(set.contains("HttpOnly") && set.contains("Secure") && set.contains("SameSite=strict"), "{set}");
    set.split(';').next().unwrap().to_string()
}

async fn login(app: &TestApp) -> (String, String) {
    let code = app.app.state.console.new_link().unwrap();
    let resp = app.send_json("POST", "/console/api/login", &[], &json!({"code": code, "name": "PC du salon"})).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    (code, session_cookie(&resp))
}

#[tokio::test]
async fn a_one_time_link_opens_a_session_once() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(config(dir.path(), &Urls::default()));
    let (code, cookie) = login(&app).await;
    assert_eq!(app.send_json("POST", "/console/api/login", &[], &json!({"code": code})).await.status, 401); // used up
    let made_up = app.send_json("POST", "/console/api/login", &[], &json!({"code": "made-up"})).await;
    assert_eq!(made_up.status, 401);
    assert_eq!(made_up.json(), json!({"detail": "this link was used or has expired: ask for a new one"}));
    let console = [("x-bip-console", "1"), ("cookie", cookie.as_str())];
    assert_eq!(app.get("/console/api/me", &console).await.json(), json!({"logged_in": true}));
    assert_eq!(app.get("/console/api/me", &[]).await.json(), json!({"logged_in": false}));
    let agents = app.get("/v1/agents", &console).await;
    assert_eq!(agents.status, 200);
    assert!(agents.json()["agents"].as_array().unwrap().iter().all(|a| a["key"].is_null()));
    // Without the header (a page from elsewhere), the cookie alone is not enough.
    assert_eq!(app.get("/v1/agents", &[("cookie", cookie.as_str())]).await.status, 401);
    // The app lists it and can revoke it.
    let (k, v) = auth();
    let devices = app.get("/v1/console/devices", &[(k, &v)]).await.json();
    let devices = devices["devices"].as_array().unwrap().clone();
    assert_eq!(devices.iter().map(|d| d["name"].clone()).collect::<Vec<_>>(), vec![json!("PC du salon")]);
    assert_eq!(devices[0].as_object().unwrap().keys().collect::<Vec<_>>(), ["id", "name", "created", "seen"]);
    let id = devices[0]["id"].as_str().unwrap();
    assert_eq!(app.request("DELETE", &format!("/v1/console/devices/{id}"), &[(k, &v)], None).await.status, 204);
    assert_eq!(
        app.request("DELETE", &format!("/v1/console/devices/{id}"), &[(k, &v)], None).await.json(),
        json!({"detail": "unknown device"})
    );
    assert_eq!(app.get("/v1/agents", &console).await.status, 401);
    // Logging out drops the cookie.
    let (_, cookie) = login(&app).await;
    let out = app.request("POST", "/console/api/logout", &[("cookie", cookie.as_str())], None).await;
    assert_eq!(out.json(), json!({"ok": true}));
    assert!(out.header("set-cookie").unwrap().starts_with("bip_console=\"\"; expires="));
    assert_eq!(
        app.get("/console/api/me", &[("x-bip-console", "1"), ("cookie", cookie.as_str())]).await.json(),
        json!({"logged_in": false})
    );
}

#[tokio::test]
async fn a_code_approved_in_the_app_opens_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(config(dir.path(), &Urls::default()));
    let pairing = app.request("POST", "/console/api/pairing", &[], None).await.json();
    let code = pairing["code"].as_str().unwrap().to_string();
    assert_eq!(code.len(), 6);
    assert_eq!(app.send_json("POST", "/console/api/pairing/wait", &[], &pairing).await.json(), json!({"ok": false})); // not yet
    let path = format!("/v1/console/pairings/{code}");
    assert_eq!(app.send_json("POST", &path, &[], &json!({"name": "Mac"})).await.status, 401);
    let (k, v) = auth();
    assert_eq!(app.send_json("POST", &path, &[(k, &v)], &json!({"name": "Mac"})).await.status, 200);
    assert_eq!(app.send_json("POST", &path, &[(k, &v)], &json!({"name": "Mac"})).await.status, 404); // approved once
    let mut wrong = pairing.clone();
    wrong["wait"] = json!("guess");
    assert_eq!(app.send_json("POST", "/console/api/pairing/wait", &[], &wrong).await.json(), json!({"ok": false}));
    let waited = app.send_json("POST", "/console/api/pairing/wait", &[], &pairing).await;
    assert_eq!(waited.json(), json!({"ok": true}));
    let cookie = session_cookie(&waited);
    assert_eq!(app.get("/v1/agents", &[("x-bip-console", "1"), ("cookie", cookie.as_str())]).await.status, 200);
    // Too long a code: FastAPI's 422.
    let long = app.send_json("POST", "/console/api/pairing/wait", &[], &json!({"code": "1234567", "wait": "x"})).await;
    assert_eq!(long.status, 422);
    assert_eq!(long.json()["detail"][0]["type"], "string_too_long");
}

#[tokio::test]
async fn the_console_reaches_hermes_without_its_key() {
    let backend = FakeBackend::start().await;
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(config(dir.path(), &backend.urls()));
    let (_, cookie) = login(&app).await;
    // The fake Hermes answers 401 to a wrong key: the bridge put the agent's own.
    let reached =
        app.get("/hermes/wellness/api/sessions", &[("x-bip-console", "1"), ("cookie", cookie.as_str())]).await;
    assert_ne!(reached.status, 401);
    assert_eq!(app.get("/hermes/wellness/api/sessions", &[]).await.status, 401);
    // no session, no key: Hermes refuses
}

#[tokio::test]
async fn the_page_runs_only_its_own_scripts() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(config(dir.path(), &Urls::default()));
    let page = app.get("/console/", &[]).await;
    assert_eq!(page.status, 200);
    assert!(page.text().contains("src=\"/console/static/app.js\""));
    assert_eq!(page.header("content-type").as_deref(), Some("text/html; charset=utf-8"));
    let csp = page.header("content-security-policy").unwrap();
    assert!(csp.contains("script-src 'self'") && csp.contains("frame-ancestors 'none'") && !csp.contains("unsafe"));
    assert_eq!(page.header("cache-control").as_deref(), Some("no-store"));
    for name in ["app.js", "api.js", "markdown.js", "bips.js", "style.css"] {
        assert_eq!(app.get(&format!("/console/static/{name}"), &[]).await.status, 200, "{name}");
    }
    let to_page = app.get("/console", &[]).await;
    assert_eq!(to_page.status, 307);
    assert_eq!(to_page.header("location").as_deref(), Some("/console/"));
}

#[test]
fn the_link_command_prints_a_working_link() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bridge.toml");
    let d = dir.path().to_string_lossy();
    std::fs::write(
        &path,
        format!(
            "bridge_key = \"{}\"\npublic_url = \"https://box.tailnet.ts.net:8643\"\n[pocket]\nurl = \"http://127.0.0.1:1\"\n[outbox]\n\
             db_path = \"{d}/bridge.db\"\naudio_dir = \"{d}/audio\"\n[agents.wellness]\nhermes_url = \"http://127.0.0.1:2\"\nhermes_key = \"x\"\n",
            "k".repeat(40)
        ),
    )
    .unwrap();
    bipbridge::py::chmod(&path.to_string_lossy(), 0o600).unwrap();
    assert_eq!(
        bipbridge::main_cli::main(vec!["console".into(), "--config".into(), path.to_string_lossy().into_owned()]),
        0
    );
    // The link was printed; the store it points to takes one link code.
    let store = bipbridge::console::ConsoleStore::new(&dir.path().join("console.json").to_string_lossy());
    let links = store.load()["links"].as_object().unwrap().len();
    assert_eq!(links, 1);
}
