//! Port of bridge/tests/test_pages.py: the agents' pages, locked before they are kept, versioned, served to the app.

mod common;

use bipbridge::{agent_tools, pages};
use common::{auth, config, TestApp, Urls};
use indexmap::IndexMap;
use serde_json::{json, Value};

const TOKEN: &str = "agent-token-for-wellness-0123456789";

const DASHBOARD: &str = r#"<!DOCTYPE html>
<html lang="fr"><head><title>Prises de sang</title>
<script src="https://cdn.jsdelivr.net/npm/chart.js@4.4.1/dist/chart.umd.min.js"></script>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Inter">
</head><body><canvas id="c"></canvas>
<script>const data = {"ferritine": [["2026-03-02", 41], ["2026-09-15", 58]]};</script></body></html>"#;

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

#[test]
fn a_page_is_locked_before_it_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), &Urls::default());
    let made = pages::save(&config, "wellness", DASHBOARD, "Prises de sang", None).unwrap();
    let kept =
        std::fs::read_to_string(pages::root(&config).join("wellness").join(format!("{}.html", s(&made["page"]))))
            .unwrap();
    // The policy comes before anything the agent wrote, the doctype stays first.
    assert!(kept.starts_with("<!DOCTYPE html>\n<meta http-equiv=\"Content-Security-Policy\""));
    let at = |t: &str| kept.find(t).unwrap();
    assert!(at("Content-Security-Policy") < at("<html") && at("<html") < at("<canvas"));
    assert!(kept.contains("connect-src 'none'") && kept.contains("form-action 'none'"));
    // Chart.js from the bridge, inline, instead of the CDN's; what else it asks from the network is reported.
    assert!(!kept.contains("cdn.jsdelivr.net") && kept.contains("Chart.js v4") && !kept[..200].contains("<\\/script"));
    assert_eq!(made["outside"], json!(["https://fonts.googleapis.com/css2?family=Inter"]));
    // The agent's own source is kept beside it, never served.
    assert_eq!(pages::source(&config, "wellness", s(&made["page"])).unwrap(), DASHBOARD.trim());
    assert_eq!((s(&made["page"]), &made["version"]), ("prises-de-sang", &json!(1)));
}

#[test]
fn a_fragment_becomes_a_whole_locked_document() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), &Urls::default());
    let made = pages::save(
        &config,
        "wellness",
        "<h1>Hello</h1><script>document.body.append('x')</script>",
        "Mon <b>titre</b>",
        None,
    )
    .unwrap();
    let kept = std::fs::read_to_string(pages::root(&config).join("wellness/mon-b-titre-b.html")).unwrap();
    assert!(kept.starts_with("<!doctype html>\n<meta http-equiv=\"Content-Security-Policy\""));
    assert!(kept.contains("<title>Mon &lt;b&gt;titre&lt;/b&gt;</title><body><h1>Hello</h1>"));
    assert_eq!(made["outside"], json!([]));
}

#[test]
fn an_update_keeps_the_path_and_a_new_page_never_takes_another_s() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), &Urls::default());
    let first = pages::save(&config, "wellness", "<p>v1</p>", "Suivi", None).unwrap();
    let second = pages::save(&config, "wellness", "<p>autre</p>", "Suivi", None).unwrap();
    assert_eq!((s(&first["page"]), s(&second["page"])), ("suivi", "suivi-2"));
    let updated = pages::save(&config, "wellness", "<p>v2</p>", "", Some("suivi")).unwrap();
    assert_eq!(updated["path"], first["path"]);
    assert_eq!((&updated["version"], s(&updated["title"])), (&json!(2), "Suivi"));
    assert_eq!(updated["created"], first["created"]);
    assert_eq!(pages::source(&config, "wellness", "suivi").unwrap(), "<p>v2</p>");
    assert!(["suivi", "suivi-2"].contains(&s(&pages::list_pages(&config, "wellness")[0]["page"])));
    for bad in ["../x", "A B", &"x".repeat(61)] {
        let err = pages::save(&config, "wellness", "<p>x</p>", "x", Some(bad)).unwrap_err();
        assert!(err.downcast_ref::<pages::ValueError>().is_some(), "{bad}");
    }
    let big = pages::save(&config, "wellness", &"x".repeat(pages::MAX_BYTES + 1), "big", None).unwrap_err();
    assert!(big.to_string().contains("too big"));
    assert!(pages::delete(&config, "wellness", "suivi-2") && !pages::delete(&config, "wellness", "suivi-2"));
    pages::forget(&config, "wellness");
    assert!(pages::list_pages(&config, "wellness").is_empty());
}

async fn call(app: &TestApp, name: &str, arguments: Value) -> Value {
    let bearer = format!("Bearer {TOKEN}");
    let body =
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}});
    app.send_json("POST", "/v1/agent-tools/mcp", &[("authorization", &bearer)], &body).await.json()["result"].clone()
}

#[tokio::test]
async fn an_agent_makes_a_page_and_the_app_gets_it_locked() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), &Urls::default());
    let mut tokens = IndexMap::new();
    tokens.insert("wellness".to_string(), agent_tools::digest(TOKEN));
    agent_tools::save_tokens(&config, &tokens).unwrap();
    let app = TestApp::new(config);
    let (k, v) = auth();
    let result = call(&app, "create_page", json!({"html": DASHBOARD, "title": "Prises de sang"})).await;
    let text = s(&result["content"][0]["text"]).to_string();
    assert!(
        result["isError"] == false && text.contains("id prises-de-sang") && text.contains("fonts.googleapis.com"),
        "{text}"
    );
    let media = text.split("MEDIA:").nth(1).unwrap().trim().to_string();
    assert_eq!(media, "/bipagents/pages/wellness/prises-de-sang.html");
    let page = app.get(&format!("/v1/media?path={media}"), &[(k, &v)]).await;
    assert_eq!(page.status, 200);
    assert!(page.header("content-type").unwrap().starts_with("text/html"));
    assert!(page.header("content-security-policy").unwrap().starts_with("sandbox allow-scripts; default-src 'none'"));
    assert_eq!(page.header("cache-control").as_deref(), Some("no-store"));
    assert_eq!(page.header("x-content-type-options").as_deref(), Some("nosniff"));
    assert!(page.text().contains("Chart.js v4"));
    // Its source and its record are not served.
    for other in ["prises-de-sang.source", "prises-de-sang.json"] {
        assert_eq!(app.get(&format!("/v1/media?path=/bipagents/pages/wellness/{other}"), &[(k, &v)]).await.status, 404);
    }
    // Updated with its id; listed, read back, deleted, by the agent or from the app.
    call(&app, "create_page", json!({"html": "<p>v2</p>", "title": "Prises de sang", "page": "prises-de-sang"})).await;
    let listed = call(&app, "list_pages", json!({})).await;
    assert!(s(&listed["content"][0]["text"]).starts_with("prises-de-sang: Prises de sang (changed now ago, version 2)"));
    assert_eq!(call(&app, "read_page", json!({"page": "prises-de-sang"})).await["content"][0]["text"], "<p>v2</p>");
    assert_eq!(call(&app, "read_page", json!({"page": "nope"})).await["isError"], true);
    let app_list = app.get("/v1/pages?agent=wellness", &[(k, &v)]).await.json()["pages"].clone();
    let rows: Vec<(String, String, i64)> = app_list
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (s(&p["page"]).to_string(), s(&p["path"]).to_string(), p["version"].as_i64().unwrap()))
        .collect();
    assert_eq!(rows, vec![("prises-de-sang".to_string(), media.clone(), 2)]);
    assert_eq!(app.get("/v1/pages?agent=nobody", &[(k, &v)]).await.status, 404);
    assert_eq!(app.request("DELETE", "/v1/pages/prises-de-sang?agent=wellness", &[(k, &v)], None).await.status, 200);
    assert_eq!(app.request("DELETE", "/v1/pages/prises-de-sang?agent=wellness", &[(k, &v)], None).await.status, 404);
    assert_eq!(call(&app, "list_pages", json!({})).await["content"][0]["text"], "No page yet.");
}

#[test]
fn every_page_carries_the_kit_after_the_policy() {
    let (kept, outside) = pages::lock_with(
        "<!doctype html><html lang=\"fr\"><head><title>Sorties</title><script src=\"chart.js\"></script></head>\
<body><div class=\"kpi\"><span class=\"v\">86</span></div><div id=\"w\"></div>\
<script>Bip.bars('#w', {labels: ['S1'], values: [18]})</script></body></html>",
        "Sorties",
        Some("wellness"),
    );
    assert!(outside.is_empty());
    let at = |t: &str| kept.find(t).unwrap_or_else(|| panic!("{t} missing"));
    // The policy first, then the kit (its look and its script), then what the agent wrote.
    assert!(at("Content-Security-Policy") < at("<style id=\"bip-kit\">"));
    assert!(at("<style id=\"bip-kit\">") < at("<script id=\"bip-kit-js\">"));
    assert!(at("<script id=\"bip-kit-js\">") < at("<html lang=\"fr\">"));
    assert!(kept.contains(".kpi .v") && kept.contains("window.Bip = {") && kept.contains("var TINT = \"wellness\";"));
    // Chart.js inline, then the app's colours for it before the page draws.
    assert!(at("Chart.js v4") < at(";window.Bip&&Bip._chart&&Bip._chart();"));
    assert!(at("Bip._chart();") < at("Bip.bars('#w'"));
    // An unknown kind gets the app's colours; nothing of it reaches the page.
    let (plain, _) = pages::lock_with("<p>x</p>", "x", Some("<script>"));
    assert!(plain.contains("var TINT = \"\";") && !plain.contains("TINT = \"<script>"));
    assert_eq!(plain.matches("</script>").count(), 1);
    assert_eq!(plain.matches("</style>").count(), 1);
}

#[test]
fn the_kit_never_outranks_the_agents_own_css() {
    // Every rule of the kit is inside :where() (no weight), except the tokens on :root.
    let css = pages::kit_css();
    let without_comments = regex::Regex::new(r"(?s)/\*.*?\*/").unwrap().replace_all(css, "");
    for block in without_comments.split('}') {
        let selector = block.rsplit_once('{').map(|(s, _)| s).unwrap_or("");
        let selector = selector.rsplit('{').next().unwrap_or(selector).trim();
        if selector.is_empty() || selector.starts_with(":root") {
            continue;
        }
        // the selector's own commas, not those inside :where(…)
        let (mut depth, mut start, mut parts) = (0, 0, Vec::new());
        for (i, c) in selector.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => {
                    parts.push(&selector[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        parts.push(&selector[start..]);
        for part in parts.into_iter().map(str::trim) {
            assert!(part.starts_with(":where("), "a kit rule with weight: {part}");
        }
    }
    assert!(pages::kit_js().contains("__BIP_TINT__") && pages::kit_js().len() < 30_000 && css.len() < 20_000);
}
