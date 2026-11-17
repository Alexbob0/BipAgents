//! tests/test_browser_mcp.py: the MCP proxy that answers Hermes' handshake from its cache, starts the agent's browser
//! at the first tool call and stops it once nobody uses it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use bipbridge::browser_mcp::{self, Proxy};
use bipbridge::Dict;
use serde_json::{json, Value};

const FAKE_SERVER: &str = r#"
import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    if m["method"] == "initialize":
        r = {"protocolVersion": m["params"]["protocolVersion"], "capabilities": {"tools": {}},
             "serverInfo": {"name": "stealth", "version": "1"}}
    elif m["method"] == "tools/list":
        r = {"tools": [{"name": "spawn_browser", "inputSchema": {"type": "object"}}]}
    elif m["method"] == "tools/call":
        r = {"content": [{"type": "text", "text": "did " + m["params"]["name"]}]}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "error": {"code": -32601, "message": "no"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#;

fn init() -> Value {
    json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
           "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "hermes"}}})
}

/// What the proxy writes to Hermes.
#[derive(Clone, Default)]
struct Out(Arc<Mutex<Vec<u8>>>);

impl Write for Out {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Out {
    fn messages(&self) -> Vec<Value> {
        let data = self.0.lock().unwrap().clone();
        String::from_utf8(data).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }
}

fn python() -> String {
    ["python3", "/usr/bin/python3"]
        .iter()
        .find(|p| std::process::Command::new(p).arg("-V").output().is_ok())
        .unwrap()
        .to_string()
}

fn config(tmp: &Path) -> Dict {
    std::fs::write(tmp.join("server.py"), FAKE_SERVER).unwrap();
    let log = tmp.join("log").to_string_lossy().into_owned();
    let running = tmp.join("running").to_string_lossy().into_owned();
    let value = json!({
        "start": ["sh", "-c", format!("echo start >> {log}; touch {running}")],
        "ready": ["true"],
        "active": ["test", "-e", running],
        "server": [python(), tmp.join("server.py").to_string_lossy()],
        "stop": ["sh", "-c", format!("echo stop >> {log}; rm -f {running}")],
        "cache": tmp.join("cache.json").to_string_lossy(), "busy": tmp.join("busy").to_string_lossy(),
        "cdp_port": 19602, "idle": 600,
    });
    value.as_object().unwrap().clone()
}

fn ran(tmp: &Path) -> Vec<String> {
    std::fs::read_to_string(tmp.join("log"))
        .map(|t| t.split_whitespace().map(String::from).collect())
        .unwrap_or_default()
}

fn ids(messages: &[Value]) -> Vec<i64> {
    messages.iter().map(|m| m["id"].as_i64().unwrap()).collect()
}

fn set_mtime(path: &PathBuf, ago: u64) {
    let file = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(ago)).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn hermes_connects_without_starting_the_browser_once_it_was_seen() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let config = config(tmp);
    // The first time ever: started to learn what it serves, then answered as it said.
    let out = Out::default();
    let proxy = Proxy::new(config.clone(), Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "navigate"}})).await;
    let first = out.messages();
    assert_eq!(ids(&first), vec![0, 1, 2]);
    assert_eq!(first[0]["result"]["serverInfo"]["name"], "stealth");
    assert_eq!(first[1]["result"]["tools"][0]["name"], "spawn_browser");
    assert_eq!(first[2]["result"]["content"][0]["text"], "did navigate");
    assert_eq!(ran(tmp), vec!["start"]);
    assert!(tmp.join("busy").exists());
    proxy.stop_child().await;

    // Its gateway restarted: the handshake and the tool list come from what it said, nothing starts.
    std::fs::remove_file(tmp.join("log")).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config.clone(), Box::new(out.clone()));
    let mut again = init();
    again["id"] = json!(10);
    proxy.handle(again).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 11, "method": "tools/list"})).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 12, "method": "ping"})).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 14, "method": "prompts/list"})).await; // never asked before: empty
    let messages = out.messages();
    assert_eq!(ids(&messages), vec![10, 11, 12, 14]);
    assert!(ran(tmp).is_empty() && !proxy.running());
    assert_eq!(messages[3]["result"], json!({"prompts": []}));
    assert_eq!(messages[1]["result"], first[1]["result"]);
    // The first tool call starts it.
    proxy
        .handle(json!({"jsonrpc": "2.0", "id": 13, "method": "tools/call", "params": {"name": "spawn_browser"}}))
        .await;
    assert_eq!(
        out.messages().last().unwrap(),
        &json!({"jsonrpc": "2.0", "id": 13, "result": {"content": [{"type": "text", "text": "did spawn_browser"}]}})
    );
    assert_eq!(ran(tmp), vec!["start"]);
    assert!(proxy.running());
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stopped_once_nobody_uses_it() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let mut config = config(tmp);
    config.insert("idle".into(), json!(0.4));
    let proxy = Proxy::new(config, Box::new(Out::default()));
    proxy.handle(init()).await;
    let watcher = tokio::spawn(proxy.clone().watch_idle(0.05));
    // The person watching its desktop (the bridge touching `busy`) keeps it on.
    let busy = tmp.join("busy");
    for _ in 0..8 {
        set_mtime(&busy, 0);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(proxy.running() && !ran(tmp).contains(&"stop".to_string()));
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(!proxy.running());
    assert_eq!(ran(tmp), vec!["start", "stop"]);
    // Started by someone else (the desktop) and left unused: stopped too.
    std::fs::write(tmp.join("running"), "").unwrap();
    set_mtime(&busy, 5);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(ran(tmp), vec!["start", "stop", "stop"]);
    assert!(!tmp.join("running").exists());
    watcher.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn stopped_soon_after_the_run_that_used_it_ended_unless_used_again() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let config = config(tmp); // idle 600 s, `ended` beside `busy`, after_run 480 s by default
    let proxy = Proxy::new(config.clone(), Box::new(Out::default()));
    assert_eq!(proxy.after_run, browser_mcp::AFTER_RUN_SECONDS as f64);
    assert_eq!(proxy.ended_path, tmp.join("ended"));
    let (busy, ended) = (tmp.join("busy"), tmp.join("ended"));
    // No run told its end: only the 600 s rule.
    set_mtime(&busy, 400);
    assert!(!proxy.done_with());
    // The run ended 7 minutes ago, its last call before: not yet (warm for the person's next message).
    set_mtime(&busy, 900);
    set_mtime(&ended, 420);
    assert!(!proxy.done_with());
    // 8 minutes: done with.
    set_mtime(&ended, 481);
    assert!(proxy.done_with());
    // Used since the end (a new call, the person watching or driving its desktop): the 600 s rule again.
    set_mtime(&busy, 100);
    assert!(!proxy.done_with());
    // `after_run` 0: never sooner.
    let mut never = config.clone();
    never.insert("after_run".into(), json!(0));
    set_mtime(&busy, 400);
    assert!(!Proxy::new(never, Box::new(Out::default())).done_with());

    // The watcher stops it, its idle time far from over.
    let mut quick = config.clone();
    quick.insert("after_run".into(), json!(0.3));
    let proxy = Proxy::new(quick, Box::new(Out::default()));
    proxy.handle(init()).await; // started once to learn its tools (touches `busy`)
    assert!(proxy.running());
    let watcher = tokio::spawn(proxy.clone().watch_idle(0.05));
    // a call under way keeps it on whatever the run's end says
    proxy.calls.store(1, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(20)).await;
    set_mtime(&busy, 2);
    set_mtime(&ended, 1);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(proxy.running());
    proxy.calls.store(0, Ordering::SeqCst);
    set_mtime(&busy, 2);
    set_mtime(&ended, 1);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!proxy.running());
    assert_eq!(ran(tmp), vec!["start", "stop"]);
    watcher.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_call_or_a_devtools_connection_keeps_it_on() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let table = tmp.join("tcp");
    let header = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";
    std::fs::write(
        &table,
        format!("{header}   0: 0100007F:B798 0100007F:4C92 01 00000000:00000000 00:00000000 00000000  1000 0 1\n"),
    )
    .unwrap();
    let tables = vec![table.to_string_lossy().into_owned()];
    assert!(browser_mcp::devtools_connected(Some(&json!(19602)), &tables));
    assert!(!browser_mcp::devtools_connected(Some(&json!(19604)), &tables));

    let mut config = config(tmp);
    config.insert("idle".into(), json!(0.3));
    let proxy = Proxy::new(config, Box::new(Out::default()));
    *proxy.tcp_tables.lock().unwrap() = vec![tmp.join("none").to_string_lossy().into_owned()];
    proxy.handle(init()).await;
    proxy.calls.store(1, Ordering::SeqCst); // a call under way
    set_mtime(&tmp.join("busy"), 60);
    let watcher = tokio::spawn(proxy.clone().watch_idle(0.05));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let age = SystemTime::now()
        .duration_since(std::fs::metadata(tmp.join("busy")).unwrap().modified().unwrap())
        .unwrap_or_default();
    assert!(proxy.running() && age < Duration::from_secs(1)); // touched for the others
    watcher.abort();
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_browser_that_cannot_start_is_said_to_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let mut config = config(tmp);
    config.insert("start".into(), json!(["false"]));
    let out = Out::default();
    std::fs::write(tmp.join("cache.json"), json!({"initialize": {"serverInfo": {"name": "stealth"}}}).to_string())
        .unwrap();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "navigate"}})).await;
    let answer = out.messages().last().unwrap().clone();
    assert_eq!(answer["id"], 1);
    // said in the answer, never a JSON-RPC error (Hermes' breaker counts one: browser_mcp `could_not_start`)
    assert!(answer.get("error").is_none(), "{answer}");
    assert!(answer["result"]["content"][0]["text"].as_str().unwrap().contains("could not start"), "{answer}");
    assert_eq!(proxy.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_agent_connects_from_what_another_agents_server_said_and_its_container_is_set_up_at_first_use() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let log = tmp.join("log").to_string_lossy().into_owned();
    let mut config = config(tmp);
    let shared = tmp.join("mcp-cache.json");
    let unit = tmp.join("units").join("bipagents-browser-new.service");
    config.insert("shared_cache".into(), json!(shared.to_string_lossy()));
    config.insert("unit".into(), json!({"path": unit.to_string_lossy(), "text": "[Unit]\n"}));
    config.insert("profile".into(), json!(tmp.join("profile").to_string_lossy()));
    config.insert("reload".into(), json!(["sh", "-c", format!("echo reload >> {log}")]));
    // Another agent's server, seen once: its answers kept for every agent.
    std::fs::write(
        &shared,
        json!({"initialize": {"serverInfo": {"name": "stealth"}}, "tools/list": {"tools": [{"name": "spawn_browser"}]}})
            .to_string(),
    )
    .unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config.clone(), Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).await;
    assert_eq!(out.messages()[0]["result"]["serverInfo"]["name"], "stealth");
    assert_eq!(out.messages()[1]["result"]["tools"][0]["name"], "spawn_browser");
    // Nothing set up nor started for an agent that never browses.
    assert!(ran(tmp).is_empty() && !unit.exists() && !tmp.join("profile").exists());
    // Its first tool call: its unit and profile written, systemd told, then started.
    proxy.handle(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "spawn_browser"}})).await;
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], "did spawn_browser");
    assert_eq!(ran(tmp), vec!["reload", "start"]);
    assert_eq!(std::fs::read_to_string(&unit).unwrap(), "[Unit]\n");
    assert!(tmp.join("profile").is_dir());
    // What its own server said is now its own too.
    assert!(tmp.join("cache.json").exists());
    proxy.stop_child().await;
    // Started again later: set up once only.
    let proxy = Proxy::new(config, Box::new(Out::default()));
    proxy.handle(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "navigate"}})).await;
    assert_eq!(ran(tmp), vec!["reload", "start", "start"]);
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_cold_start_is_waited_for_and_the_first_call_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let mut config = config(tmp);
    // Its screen shows up 2 s after the start (a loaded server): the call waits, it is not failed for a retry.
    let screen = tmp.join("screen").to_string_lossy().into_owned();
    config.insert("start".into(), json!(["sh", "-c", format!("(sleep 2; touch {screen}) >/dev/null 2>&1 &")]));
    config.insert("ready".into(), json!(["test", "-e", screen]));
    std::fs::write(tmp.join("cache.json"), json!({"initialize": {"serverInfo": {"name": "stealth"}}}).to_string())
        .unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config.clone(), Box::new(out.clone()));
    proxy.handle(init()).await;
    let started = std::time::Instant::now();
    proxy.handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "spawn_browser"}})).await;
    assert!(started.elapsed() >= Duration::from_secs(2));
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], "did spawn_browser");
    proxy.stop_child().await;
    // One that never shows its screen: a clear error once its deadline is over, not before.
    config.insert("ready".into(), json!(["false"]));
    config.insert("ready_seconds".into(), json!(1));
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    let started = std::time::Instant::now();
    proxy.handle(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "navigate"}})).await;
    assert!(started.elapsed() >= Duration::from_secs(1));
    // said in the answer, never a JSON-RPC error (Hermes counts one against the server: browser_mcp `could_not_start`)
    let answer = out.messages().last().unwrap().clone();
    assert!(answer.get("error").is_none(), "{answer}");
    let message = answer["result"]["content"][0]["text"].as_str().unwrap().to_string();
    assert!(message.contains("did not start within 1 s"), "{message}");
}

#[test]
fn hermes_runs_it_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let path = tmp.join("mcp.json");
    std::fs::write(&path, Value::Object(config(tmp)).to_string()).unwrap();
    let mut process = std::process::Command::new(env!("CARGO_BIN_EXE_bip-browser-mcp"))
        .arg(&path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let lines = [
        init(),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "take_screenshot"}}),
    ];
    {
        let mut stdin = process.stdin.take().unwrap();
        for m in &lines {
            stdin.write_all(format!("{m}\n").as_bytes()).unwrap();
        }
    }
    let output = process.wait_with_output().unwrap();
    let answers: Vec<Value> =
        String::from_utf8(output.stdout).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(ids(&answers), vec![0, 1]);
    assert_eq!(answers[1]["result"]["content"][0]["text"], "did take_screenshot");
    assert_eq!(ran(tmp), vec!["start"]); // Hermes gone: its server stops, the browser stays until unused
}

#[tokio::test(flavor = "multi_thread")]
async fn while_the_month_s_credit_is_used_its_tools_are_refused_without_starting_it() {
    // credit.rs (Alex 2026-10-26): the browser pauses while the plan's AI credit is used; the agent is told plainly.
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let mut config = config(tmp);
    let credit = tmp.join("credit.json");
    config.insert("credit".into(), json!(credit.to_string_lossy()));
    std::fs::write(&credit, json!({"state": "full", "resumes": 4.0e9}).to_string()).unwrap();
    std::fs::write(tmp.join("cache.json"), json!({"initialize": {"serverInfo": {"name": "stealth"}}}).to_string())
        .unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "navigate"}})).await;
    let answer = out.messages().last().unwrap().clone();
    // never `isError` (Hermes' breaker would pause the whole browser: browser_mcp.rs `soften`)
    assert_eq!((answer["id"].clone(), answer["result"].get("isError")), (json!(1), None));
    assert!(answer["result"]["content"][0]["text"].as_str().unwrap().contains("credit is used up"), "{answer}");
    assert!(ran(tmp).is_empty() && !proxy.running());
    // stopped (the grace used too): the same
    std::fs::write(&credit, json!({"state": "stopped"}).to_string()).unwrap();
    proxy.handle(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "navigate"}})).await;
    assert!(out.messages().last().unwrap()["result"]["content"][0]["text"].as_str().unwrap().contains("credit"));
    // the credit back: the next call starts it as usual
    std::fs::write(&credit, json!({"state": "ok"}).to_string()).unwrap();
    proxy.handle(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "navigate"}})).await;
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], "did navigate");
    assert_eq!(ran(tmp), vec!["start"]);
    proxy.stop_child().await;
}

fn now() -> f64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs_f64()
}

fn read_page(id: i64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
           "params": {"name": "get_page_content", "arguments": {"instance_id": "319ceedb"}}})
}

#[tokio::test(flavor = "multi_thread")]
async fn a_page_too_large_to_come_back_is_said_plainly_and_a_rescue_turn_reads_no_page() {
    // Token lab of 2026-11-07 (GLM-5.3 Flash, the French Wikipedia page of the Eiffel tower): the real server saved the
    // page to a file of its container and gave its path, out of the agent's reach; it read the page 16 times, then its
    // rescue turn read it again and waited the full 120 s.
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let config = config(tmp);
    let server = FAKE_SERVER.replace(
        r#"r = {"content": [{"type": "text", "text": "did " + m["params"]["name"]}]}"#,
        r#"if m["params"]["name"] == "get_page_content":
            saved = {"file_path": "/opt/stealth/stealth-browser-mcp/element_clones/page_content_1.json",
                     "file_size_kb": 1399.12, "estimated_tokens": 353880,
                     "reason": "Response too large, automatically saved to file"}
            r = {"content": [{"type": "text", "text": json.dumps(saved)}], "structuredContent": saved}
        else:
            r = {"content": [{"type": "text", "text": "did " + m["params"]["name"]}]}"#,
    );
    std::fs::write(tmp.join("server.py"), server).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config.clone(), Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(read_page(1)).await;
    let answer = out.messages().last().unwrap().clone();
    assert_eq!(answer["id"], json!(1));
    let content = answer["result"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    let text = content[0]["text"].as_str().unwrap();
    assert!(text.starts_with("This page is too large to come back whole (about 1.4 MB)"), "{text}");
    assert!(text.contains("Don't read it again") && text.contains(browser_mcp::AROUND_A_WORD));
    assert!(!answer.to_string().contains("element_clones"), "its path never shown");
    // its structured copy kept for the tool's output schema (Hermes' MCP client refuses a result without one)
    assert_eq!(answer["result"]["structuredContent"]["file_path"], json!(""));
    assert_eq!(answer["result"]["structuredContent"]["reason"], json!(browser_mcp::OUT_OF_REACH));
    // the other answers as they are
    proxy.handle(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "navigate"}})).await;
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], "did navigate");

    // The brake's rescue turn (the bridge's `rescue` file, beside `busy`): page reads refused at once, other calls go on.
    let rescue = tmp.join("rescue");
    assert_eq!(proxy.rescue_path, rescue);
    std::fs::write(&rescue, format!("{:.0}\n", now() + 180.0)).unwrap();
    for (id, name) in [(3, "get_page_content"), (4, "take_screenshot")] {
        proxy.handle(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name}})).await;
        let refused = out.messages().last().unwrap().clone();
        assert_eq!(refused["id"], json!(id));
        assert_eq!(refused["result"].get("isError"), None, "never an error (Hermes' breaker): {refused}");
        assert_eq!(refused["result"]["content"][0]["text"], browser_mcp::RESCUE_REFUSED);
    }
    assert!(browser_mcp::RESCUE_REFUSED.contains("Answer your person now with what you already have"));
    proxy.handle(json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "navigate"}})).await;
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], "did navigate");
    // past its deadline (a bridge stopped meanwhile), or gone: the page read again
    std::fs::write(&rescue, format!("{:.0}\n", now() - 1.0)).unwrap();
    proxy.handle(read_page(6)).await;
    assert!(out.messages().last().unwrap()["result"]["content"][0]["text"].as_str().unwrap().starts_with("This page"));
    std::fs::remove_file(&rescue).unwrap();
    proxy.handle(read_page(7)).await;
    assert!(out.messages().last().unwrap()["result"]["content"][0]["text"].as_str().unwrap().starts_with("This page"));
    proxy.stop_child().await;

    // Straight away: a browser stopped meanwhile is not started for a refused read.
    std::fs::remove_file(tmp.join("log")).unwrap();
    std::fs::write(&rescue, format!("{:.0}\n", now() + 180.0)).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await; // from its cache: nothing started
    proxy.handle(read_page(8)).await;
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], browser_mcp::RESCUE_REFUSED);
    assert!(ran(tmp).is_empty() && !proxy.running());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_phone_exit_s_refusal_and_a_challenge_page_are_said_to_the_agent() {
    // phone_exit (docs/phone-exit.md §7): a page that failed because the person's phone was not relaying gets the
    // hub's reason (never « the site blocks servers »); a challenge page gets the card's news; nothing otherwise.
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let mut config = config(tmp);
    let server = FAKE_SERVER.replace(
        r#"r = {"content": [{"type": "text", "text": "did " + m["params"]["name"]}]}"#,
        r#"url = (m["params"].get("arguments") or {}).get("url", "")
        page = "<title>Just a moment...</title>" if "challenge" in url else "did " + m["params"]["name"]
        r = {"content": [{"type": "text", "text": page}]}"#,
    );
    std::fs::write(tmp.join("server.py"), server).unwrap();
    // the hub: WHY answers « paused 3 » for ameli.fr, ASK answers CARD; every line kept
    let short = tempfile::Builder::new().prefix("pe").tempdir_in("/tmp").unwrap();
    let sock = short.path().join("hub.sock");
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let listener = tokio::net::UnixListener::bind(&sock).unwrap();
    {
        let asked = asked.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
            loop {
                let (conn, _) = listener.accept().await.unwrap();
                let (read, mut write) = conn.into_split();
                let mut line = String::new();
                tokio::io::BufReader::new(read).read_line(&mut line).await.unwrap();
                asked.lock().unwrap().push(line.trim().to_string());
                let answer = if line.starts_with("WHY vie www.ameli.fr") {
                    "paused 3\n"
                } else if line.starts_with("WHY") {
                    "NONE\n"
                } else {
                    "CARD\n"
                };
                write.write_all(answer.as_bytes()).await.unwrap();
            }
        });
    }
    config.insert("agent".into(), json!("vie"));
    config.insert("exit_sock".into(), json!(sock.to_string_lossy()));
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await;
    let navigate = |id: i64, url: &str| {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
               "params": {"name": "navigate", "arguments": {"url": url}}})
    };
    let texts = |out: &Out| -> Vec<String> {
        let last = out.messages().last().unwrap().clone();
        last["result"]["content"].as_array().unwrap().iter().map(|c| c["text"].as_str().unwrap().to_string()).collect()
    };
    proxy.handle(navigate(1, "https://www.ameli.fr/assure")).await;
    let said = texts(&out);
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(said[1].starts_with("Phone exit (ameli.fr, paused)") && said[1].contains("Do NOT say the site blocks you"));
    // a later read of the same page: about the same site (the last URL navigated to)
    proxy
        .handle(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "get_page_content"}}))
        .await;
    assert_eq!(texts(&out).len(), 2);
    // a challenge page: the hub asked, the card said
    proxy.handle(navigate(3, "https://www.leboncoin.fr/challenge")).await;
    let said = texts(&out);
    assert!(said[1].contains("Cloudflare challenge") && said[1].contains("just got a card"), "{said:?}");
    assert!(said[1].contains("Never solve"));
    // an ordinary page: nothing added
    proxy.handle(navigate(4, "https://www.doctolib.fr/")).await;
    assert_eq!(texts(&out), vec!["did navigate".to_string()]);
    let asked = asked.lock().unwrap().clone();
    assert!(asked.contains(&"ASK vie www.leboncoin.fr Cloudflare challenge".to_string()), "{asked:?}");
    assert!(asked.contains(&"WHY vie www.doctolib.fr".to_string()));
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn without_the_phone_exit_nothing_is_added() {
    let dir = tempfile::tempdir().unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config(dir.path()), Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy
        .handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                       "params": {"name": "navigate", "arguments": {"url": "https://www.ameli.fr/"}}}))
        .await;
    let last = out.messages().last().unwrap().clone();
    assert_eq!(last["result"]["content"].as_array().unwrap().len(), 1);
    proxy.stop_child().await;
}

/// The real server for the browser-speed answers: `execute_script` runs the page's scripts (its readable text,
/// `document.readyState`), `take_screenshot` gives its image inline or, with `"full_page": true`, saved in its
/// container; every call written to `calls.jsonl`.
const SPEED_SERVER: &str = r#"
import json, os, sys
here = os.path.dirname(os.path.abspath(__file__))
page = {"title": "Boîte de réception", "url": "https://mail.example.test/inbox", "total": 40,
        "text": "Facture d'octobre — Portail Exemple", "main": True, "links": ["Écrire"]}
states = ["loading", "interactive", "complete"]
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    if m["method"] == "initialize":
        r = {"protocolVersion": m["params"]["protocolVersion"], "capabilities": {"tools": {}},
             "serverInfo": {"name": "stealth", "version": "1"}}
    elif m["method"] == "tools/list":
        r = {"tools": []}
    else:
        name, args = m["params"]["name"], m["params"].get("arguments", {})
        with open(os.path.join(here, "calls.jsonl"), "a") as f:
            f.write(json.dumps({"name": name, "arguments": args}) + "\n")
        if name == "execute_script" and args["script"] == "document.readyState":
            answer = {"success": True, "result": states.pop(0) if states else "complete", "error": None}
            r = {"content": [{"type": "text", "text": json.dumps(answer)}], "structuredContent": answer}
        elif name == "execute_script" and "JSON.stringify" in args["script"]:
            answer = {"success": True, "result": json.dumps(page), "error": None}
            r = {"content": [{"type": "text", "text": json.dumps(answer)}], "structuredContent": answer}
        elif name == "take_screenshot" and args.get("full_page"):
            saved = {"file_path": "/opt/stealth/stealth-browser-mcp/element_clones/screenshot_1.jpeg",
                     "filename": "screenshot_1.jpeg", "file_size_kb": 180.2,
                     "reason": "Screenshot too large, automatically saved to file"}
            r = {"content": [{"type": "text", "text": json.dumps(saved)}], "structuredContent": {"result": saved}}
        elif name == "take_screenshot":
            data = "/9j/" + "A" * 200
            r = {"content": [{"type": "text", "text": data}], "structuredContent": {"result": data}}
        else:
            r = {"content": [{"type": "text", "text": "did " + name}]}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#;

/// `<exec prefix> cat|rm <path in the container>`: the container's clone folder is `clones/` here.
const FAKE_EXEC: &str = r#"
import os, sys
here = os.path.dirname(os.path.abspath(__file__))
command, path = sys.argv[1], sys.argv[-1]
local = os.path.join(here, "clones", os.path.basename(path))
if command == "cat":
    sys.stdout.buffer.write(open(local, "rb").read())
elif command == "rm":
    os.remove(local)
"#;

fn calls(tmp: &Path) -> Vec<Value> {
    std::fs::read_to_string(tmp.join("calls.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn tool(id: i64, name: &str, arguments: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
}

#[tokio::test(flavor = "multi_thread")]
async fn pages_come_back_as_text_screenshots_as_images_and_navigation_once_loaded() {
    // Alex's case of 8/11 (a test mailbox read after 3–4 minutes): a page « set aside », a screenshot « in the wrong
    // place », a pause after the click; each one more model call.
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let mut config = config(tmp);
    std::fs::write(tmp.join("server.py"), SPEED_SERVER).unwrap();
    std::fs::write(tmp.join("exec.py"), FAKE_EXEC).unwrap();
    config.insert("exec".into(), json!([python(), tmp.join("exec.py").to_string_lossy()]));
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await;

    // the page read: its readable text, never its HTML; the structured copy small (Hermes would send it too)
    proxy.handle(tool(1, "get_page_content", json!({"instance_id": "319ceedb"}))).await;
    let answer = out.messages().last().unwrap().clone();
    assert_eq!(answer["id"], json!(1));
    let text = answer["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(
        text,
        "Boîte de réception\nhttps://mail.example.test/inbox\n\nFacture d'octobre — Portail Exemple\n\n[5 more \
characters of this page. To read on, run execute_script with: "
            .to_string()
            + &browser_mcp::readable_script(35, browser_mcp::READABLE_CHARS)
            + " — or only the part around a word, in place of `word`: "
            + browser_mcp::AROUND_A_WORD
            + "]\n\nLinks and buttons: Écrire"
    );
    assert_eq!(answer["result"]["structuredContent"]["html"], json!(""));
    assert_eq!(answer["result"]["structuredContent"]["url"], json!("https://mail.example.test/inbox"));
    // the readable text, then the check that it holds no password (fifth review 13/11: one put in the page's title)
    let scripts = calls(tmp);
    assert_eq!(scripts.iter().map(|c| c["name"].as_str().unwrap()).collect::<Vec<_>>(), ["execute_script"; 2]);
    assert!(scripts[1]["arguments"]["script"].as_str().unwrap().starts_with("/* bipagents guard: shown */"));
    // with its frames: the real tool
    proxy.handle(tool(2, "get_page_content", json!({"instance_id": "319ceedb", "include_frames": true}))).await;
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], "did get_page_content");

    // navigate: answered once the page is loaded; `networkidle` (the real server's blind 2 s) not passed on
    std::fs::remove_file(tmp.join("calls.jsonl")).unwrap();
    let url = "https://mail.example.test/";
    proxy
        .handle(tool(3, "navigate", json!({"instance_id": "319ceedb", "url": url, "wait_until": "networkidle"})))
        .await;
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], "did navigate");
    let seen = calls(tmp);
    assert_eq!(seen[0]["arguments"]["wait_until"], json!("domcontentloaded"));
    let states: Vec<_> = seen[1..].iter().map(|c| c["arguments"]["script"].as_str().unwrap()).collect();
    assert_eq!(states, ["document.readyState"; 3], "loading, interactive, complete");
    // a click too; a call without an instance waits for nothing
    std::fs::remove_file(tmp.join("calls.jsonl")).unwrap();
    proxy.handle(tool(4, "click_element", json!({"instance_id": "319ceedb", "selector": "a"}))).await;
    // what the target is (never the site's terms box), the page's fingerprints before (browser_mcp `guard_before`),
    // the click, the page's state, then the check after it (`guard_after`)
    assert_eq!(calls(tmp).len(), 5);
    assert!(calls(tmp)[0]["arguments"]["script"].as_str().unwrap().contains("found: true"));
    assert!(calls(tmp)[1]["arguments"]["script"].as_str().unwrap().contains("bipagents guard: before"));
    assert_eq!(calls(tmp)[2]["arguments"]["timeout"], json!(3000), "3 s to find it, not the real server's 10");
    assert!(calls(tmp)[4]["arguments"]["script"].as_str().unwrap().contains("bipagents guard: after"));
    proxy.handle(tool(5, "list_tabs", json!({"instance_id": "319ceedb"}))).await;
    proxy.handle(tool(6, "click_element", json!({"selector": "a", "timeout": 8000}))).await;
    assert_eq!(calls(tmp).len(), 7);
    assert_eq!(calls(tmp)[6]["arguments"]["timeout"], json!(8000), "the agent's own time kept");

    // a screenshot: an image block, the agent's path never passed on (it would land in the container)
    std::fs::remove_file(tmp.join("calls.jsonl")).unwrap();
    proxy.handle(tool(7, "take_screenshot", json!({"instance_id": "319ceedb", "file_path": "/root/shot.png"}))).await;
    let answer = out.messages().last().unwrap().clone();
    assert_eq!(answer["id"], json!(7));
    let content = answer["result"]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "image");
    assert_eq!(content[0]["mimeType"], "image/jpeg");
    assert!(content[1]["text"].as_str().unwrap().contains("not written to /root/shot.png"));
    // the page's password fields out of sight first (vault-no-readback, 16/11: the site's eye shows one in clear)
    assert!(calls(tmp)[0]["arguments"]["script"].as_str().unwrap().starts_with("/* bipagents guard: conceal */"));
    assert_eq!(calls(tmp)[1]["arguments"], json!({"instance_id": "319ceedb", "format": "jpeg"}));
    // too large for the real server: saved in its container, fetched from it and removed there
    std::fs::create_dir_all(tmp.join("clones")).unwrap();
    std::fs::write(tmp.join("clones").join("screenshot_1.jpeg"), [0xFF, 0xD8, 0xFF, 0xE0]).unwrap();
    proxy.handle(tool(8, "take_screenshot", json!({"instance_id": "319ceedb", "full_page": true}))).await;
    let answer = out.messages().last().unwrap().clone();
    assert_eq!(answer["result"]["content"][0], json!({"type": "image", "data": "/9j/4A==", "mimeType": "image/jpeg"}));
    assert!(!answer.to_string().contains("element_clones"));
    assert!(!tmp.join("clones").join("screenshot_1.jpeg").exists(), "removed from the container");
    // gone before it was fetched: the real answer, as it was
    proxy.handle(tool(9, "take_screenshot", json!({"instance_id": "319ceedb", "full_page": true}))).await;
    let answer = out.messages().last().unwrap().clone();
    assert!(answer["result"]["content"][0]["text"].as_str().unwrap().contains("too large"));
    proxy.stop_child().await;
}

/// The real server's two launch tools as stealth-browser-mcp (patched) lists them; every call written to `calls.jsonl`.
const LAUNCH_SERVER: &str = r#"
import json, os, sys
here = os.path.dirname(os.path.abspath(__file__))
spawn = {"name": "spawn_browser",
         "description": "Spawn a new browser instance.\n\nArgs:\n    headless (bool): Run in headless mode.\n    proxy (str): Proxy server URL.\n    browser_args (list): Extra Chrome flags.\n    viewport_width (int): Width.\n    user_data_dir (str): Profile folder.",
         "inputSchema": {"type": "object", "required": ["browser_args"], "properties": {
             "headless": {"type": "boolean"}, "user_agent": {"type": "string"}, "viewport_width": {"type": "integer"},
             "viewport_height": {"type": "integer"}, "proxy": {"type": "string"}, "browser_args": {"type": "array"},
             "timezone_id": {"type": "string"}, "idle_timeout_seconds": {"type": "integer"},
             "block_resources": {"type": "array"}, "extra_headers": {"type": "object"},
             "user_data_dir": {"type": "string"}, "sandbox": {"type": "boolean"}}}}
close = {"name": "close_instance",
         "description": "Close a browser instance. Only when the person asks to quit the browser, pass quit_browser=True.\n\nArgs:\n    instance_id (str): Browser instance ID.\n    quit_browser (bool): Quit the browser itself.",
         "inputSchema": {"type": "object", "required": ["instance_id"], "properties": {
             "instance_id": {"type": "string"}, "quit_browser": {"type": "boolean"}}}}
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    if m["method"] == "initialize":
        r = {"protocolVersion": m["params"]["protocolVersion"], "capabilities": {"tools": {}},
             "serverInfo": {"name": "stealth", "version": "1"}}
    elif m["method"] == "tools/list":
        r = {"tools": [spawn, close, {"name": "navigate", "inputSchema": {"type": "object", "properties": {"url": {}}}}]}
    else:
        with open(os.path.join(here, "calls.jsonl"), "a") as f:
            f.write(json.dumps({"name": m["params"]["name"], "arguments": m["params"].get("arguments")}) + "\n")
        r = {"content": [{"type": "text", "text": "did " + m["params"]["name"]}]}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#;

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_never_chooses_how_its_browser_is_launched() {
    // Audit 11/11, BV-3: `spawn_browser(browser_args=["--no-proxy-server"])` after `close_instance(quit_browser=True)`
    // started Chrome without the egress proxy's flag (no send guard), steered by a page the agent read.
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let config = config(tmp);
    std::fs::write(tmp.join("server.py"), LAUNCH_SERVER).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config.clone(), Box::new(out.clone()));
    proxy.handle(init()).await;

    // the tool list: none of what the bridge takes off is offered
    proxy.handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).await;
    let check_list = |listed: &Value| {
        let tools = listed["result"]["tools"].as_array().unwrap();
        let spawn = &tools[0];
        let mut offered: Vec<&str> =
            spawn["inputSchema"]["properties"].as_object().unwrap().keys().map(String::as_str).collect();
        offered.sort();
        assert_eq!(offered, ["block_resources", "viewport_height", "viewport_width"]);
        assert_eq!(spawn["inputSchema"]["required"], json!([]));
        let text = spawn["description"].as_str().unwrap();
        assert!(text.contains("viewport_width (int)") && !text.contains("proxy") && !text.contains("browser_args"));
        assert!(!text.contains("headless") && !text.contains("user_data_dir"), "{text}");
        let close = &tools[1];
        assert_eq!(close["inputSchema"]["properties"], json!({"instance_id": {"type": "string"}}));
        assert!(!close["description"].as_str().unwrap().contains("quit_browser"));
        assert_eq!(tools[2]["inputSchema"]["properties"], json!({"url": {}})); // the other tools as they are
    };
    check_list(&out.messages().last().unwrap().clone());

    // the call: the launch arguments never reach the browser's container, the harmless ones do
    let asked = json!({"browser_args": ["--no-proxy-server"], "proxy": "http://203.0.113.9:1",
        "user_data_dir": "/tmp/elsewhere", "headless": true, "sandbox": true, "user_agent": "x",
        "extra_headers": {"Host": "x"}, "timezone_id": "UTC", "idle_timeout_seconds": 1,
        "viewport_width": 1280, "block_resources": ["image"]});
    proxy.handle(tool(2, "spawn_browser", asked)).await;
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], "did spawn_browser");
    proxy.handle(tool(3, "spawn_browser", json!(["--no-proxy-server"]))).await; // not even an object
    proxy.handle(tool(4, "close_instance", json!({"instance_id": "319ceedb", "quit_browser": true}))).await;
    let page = json!({"instance_id": "319ceedb", "url": "https://example.test/", "proxy": "p"});
    proxy.handle(tool(5, "navigate", page)).await;
    let seen = calls(tmp);
    assert_eq!(seen[0]["arguments"], json!({"viewport_width": 1280, "block_resources": ["image"]}));
    assert_eq!(seen[1]["arguments"], json!({}));
    assert_eq!(seen[2]["arguments"], json!({"instance_id": "319ceedb"}), "the person quits it from the app");
    assert_eq!(seen[3]["arguments"]["proxy"], json!("p"), "other tools' arguments untouched");
    proxy.stop_child().await;

    // its gateway restarted: the cached list is the same tamed one, the browser not started for it
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 6, "method": "tools/list"})).await;
    assert!(!proxy.running());
    check_list(&out.messages().last().unwrap().clone());
}

#[test]
fn a_list_cached_before_the_rule_is_tamed_too() {
    let mut listed = json!({"tools": [{"name": "spawn_browser", "description": "Args:\n    proxy (str): x\n    headless (bool): y",
        "inputSchema": {"type": "object", "properties": {"proxy": {}, "headless": {}, "viewport_width": {}}}}]});
    browser_mcp::tame_tool_list(&mut listed);
    assert_eq!(listed["tools"][0]["inputSchema"]["properties"], json!({"viewport_width": {}}));
    assert_eq!(listed["tools"][0]["description"], json!("Args:"));
    let mut call = json!({"name": "spawn_browser", "arguments": {"proxy": "x", "viewport_height": 900}});
    assert_eq!(browser_mcp::tame_call("spawn_browser", &mut call), ["proxy"]);
    assert_eq!(call["arguments"], json!({"viewport_height": 900}));
    let mut other = json!({"name": "navigate", "arguments": {"proxy": "x"}});
    assert!(browser_mcp::tame_call("navigate", &mut other).is_empty());
}

/// A browser's server as stealth-browser-mcp behind hold_gate.py: while `held` exists every call gets the gate's refusal
/// (`isError`); `#folded` can't be clicked (an error); the target script (browser_mcp `target_script`) says
/// `#register-password` is a password field and `#cgu` a terms box. Every call written to `calls.jsonl`.
const GUARD_SERVER: &str = r##"
import json, os, sys, time
here = os.path.dirname(os.path.abspath(__file__))
GATE = ("The person has taken over this browser in the BipAgents app (to log in, or to do something themselves). "
        "Do not use the browser until they hand it back: tell them you are waiting, or go on with something else.")
page = {"typed": 0, "ticked": 0, "opened": None}
def text(t, error=False):
    r = {"content": [{"type": "text", "text": t}]}
    if error:
        r["isError"] = True
    return r
def script_answer(value):
    answer = {"success": True, "result": value, "error": None}
    return {"content": [{"type": "text", "text": json.dumps(answer)}], "structuredContent": answer}
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    if m["method"] == "initialize":
        r = {"protocolVersion": m["params"]["protocolVersion"], "capabilities": {"tools": {}},
             "serverInfo": {"name": "stealth", "version": "1"}}
    elif m["method"] == "tools/list":
        wrapped = {"type": "object", "properties": {"result": {"type": "boolean"}}, "required": ["result"],
                   "x-fastmcp-wrap-result": True}
        shot = {"type": "object", "required": ["result"], "x-fastmcp-wrap-result": True,
                "properties": {"result": {"anyOf": [{"type": "string"}, {"type": "object", "additionalProperties": True}]}}}
        r = {"tools": [{"name": "navigate", "inputSchema": {"type": "object"},
                        "outputSchema": {"type": "object", "additionalProperties": True}},
                       {"name": "click_element", "inputSchema": {"type": "object"}, "outputSchema": wrapped},
                       {"name": "type_text", "inputSchema": {"type": "object"}, "outputSchema": wrapped},
                       {"name": "take_screenshot", "inputSchema": {"type": "object"}, "outputSchema": shot},
                       {"name": "execute_script", "inputSchema": {"type": "object"},
                        "outputSchema": {"type": "object", "additionalProperties": True}}]}
    else:
        name, args = m["params"]["name"], m["params"].get("arguments", {})
        with open(os.path.join(here, "calls.jsonl"), "a") as f:
            f.write(json.dumps({"name": name, "arguments": args}) + "\n")
        script = args.get("script", "")
        selector = args.get("selector", "")
        late_there = page["opened"] is not None and time.time() - page["opened"] >= 0.6
        if os.path.exists(os.path.join(here, "held")):
            r = text(GATE, True)
        elif name == "execute_script" and "bipagents guard: before" in script:
            # a page with a card's field when the test says so (`cardpage`)
            cards = {"#cc": "811c9dc5"} if os.path.exists(os.path.join(here, "cardpage")) else {}
            r = script_answer(json.dumps({"pw": {"#register-password": ""}, "known": [], "boxes": ["#cgu"], "cards": cards}))
        elif name == "execute_script" and "bipagents guard: after" in script:
            r = script_answer(json.dumps({"passwords": page["typed"], "terms": page["ticked"]}))
            page["typed"] = page["ticked"] = 0
        elif name == "execute_script" and "found: true" in script:
            if '"#late-password"' in script and not late_there:
                target = {"found": False}
            else:
                target = {"found": True, "password": "password" in script.split(";")[0],
                          "terms": "J'accepte les conditions générales de vente  cgu cgu"
                                   if '"#cgu"' in script or "span.box" in script else ""}
                # the contact form of contact.example (audit 1.2): its send button, and its message field (Enter)
                if '"button[type=submit]"' in script:
                    target["send"] = "contact.example"
                if '"#message"' in script:
                    target["enter"] = "contact.example"
            r = script_answer(json.dumps(target))
        elif name == "execute_script" and script == "document.readyState":
            r = script_answer("complete")
        elif name == "execute_script" and "querySelectorAll('input, select, textarea')" in script:
            r = script_answer("#lastname (text, Nom); #phone (tel, Téléphone)")
        elif name == "click_element" and selector == "#folded":
            r = text("Error calling tool 'click_element': Failed to click element: DOM Error while querying [code: -32000]", True)
        else:
            if name == "click_element" and selector == "#open-register":
                page["opened"] = time.time()
            if name == "click_element" and "conditions" in args.get("text_match", ""):
                page["ticked"] = 1            # the real server clicks by text, the selector unused
            if name in ("type_text", "paste_text") and "password" in selector:
                page["typed"] = 1
            if name == "press_key" and "password" in selector and len(args.get("key", "")) == 1:
                page["typed"] = 1
            if name == "execute_script" and ("Provisoire" in script + json.dumps(args.get("args", []))):
                page["typed"] = 1
            if name == "execute_script" and "'cgu'" in script and ".checked = true" in script:
                page["ticked"] = 1
            r = text("did " + name)
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"##;

/// The proxy over `GUARD_SERVER`, connected and its tool list (with their output schemas) learned, as Hermes does.
async fn guarded(tmp: &Path) -> (Arc<Proxy>, Out) {
    let config = config(tmp);
    std::fs::write(tmp.join("server.py"), GUARD_SERVER).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 99, "method": "tools/list"})).await;
    (proxy, out)
}

fn last(out: &Out) -> Value {
    out.messages().last().unwrap().clone()
}

fn text_of(answer: &Value) -> String {
    answer["result"]["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn handed_back_the_browser_answers_at_once_and_nothing_pauses_it() {
    // 13/11 (Vie on aibox, then the lab): three calls refused while the person held the browser opened Hermes' breaker
    // (« Paused for ~54s »), so the browser stayed paused a minute after « Rendre la main ». Never an error now.
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let (proxy, out) = guarded(tmp).await;
    std::fs::write(tmp.join("held"), "human t0k").unwrap();
    for id in 1..=4 {
        proxy.handle(tool(id, "navigate", json!({"instance_id": "i", "url": "https://shop.example/"}))).await;
        let answer = last(&out);
        assert_eq!(answer["id"], json!(id));
        assert_eq!(answer["result"].get("isError"), None, "{answer}");
        assert_eq!(text_of(&answer), browser_mcp::PERSON_HOLDS, "said plainly, no streak hint for a hold");
        // with what its output schema asks (Hermes' client refuses a result without: lab, 13/11)
        assert_eq!(answer["result"]["structuredContent"], json!({"success": false}));
    }
    // « Rendre la main »: the very next call goes through
    std::fs::remove_file(tmp.join("held")).unwrap();
    let started = std::time::Instant::now();
    proxy.handle(tool(5, "navigate", json!({"instance_id": "i", "url": "https://shop.example/"}))).await;
    assert_eq!(text_of(&last(&out)), "did navigate");
    assert!(started.elapsed() < Duration::from_secs(5));

    // fields still folded in a panel: errors, never `isError`; the third in a row says what to do
    for id in 6..=8 {
        proxy.handle(tool(id, "click_element", json!({"instance_id": "i", "selector": "#folded"}))).await;
        let answer = last(&out);
        assert_eq!(answer["result"].get("isError"), None, "{answer}");
        assert!(text_of(&answer).starts_with("Error calling tool 'click_element'"));
        assert_eq!(text_of(&answer).contains(browser_mcp::STREAK_HINT), id == 8, "{answer}");
        // with the page's real fields (lab 13/11: guessed names, then the panel clicked open again and again)
        let fields = "instructions): #lastname (text, Nom); #phone (tel, Téléphone)";
        assert_eq!(text_of(&answer).contains(fields), id == 8, "{answer}");
        assert_eq!(answer["result"]["structuredContent"], json!({"result": false}));
    }
    assert_eq!(proxy.failures.load(Ordering::SeqCst), 3);
    proxy.handle(tool(9, "click_element", json!({"instance_id": "i", "selector": "#open-register"}))).await;
    assert_eq!(text_of(&last(&out)), "did click_element");
    assert_eq!(proxy.failures.load(Ordering::SeqCst), 0, "a step that worked starts again from 0");
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_never_types_a_password_nor_ticks_the_terms() {
    // 13/11: Vie typed « a provisional password » itself; on the lab GLM typed into #register-password three times,
    // and agents ticked the terms with a script
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let (proxy, out) = guarded(tmp).await;
    let typed = |tmp: &Path| -> Vec<String> {
        calls(tmp)
            .iter()
            .filter(|c| c["name"] != "execute_script")
            .map(|c| format!("{} {}", c["name"].as_str().unwrap(), c["arguments"]["selector"].as_str().unwrap_or("")))
            .collect()
    };
    let password = json!({"instance_id": "i", "selector": "#register-password", "text": "Provisoire123!"});
    proxy.handle(tool(1, "type_text", password.clone())).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::PASSWORD_REFUSED);
    assert_eq!(last(&out)["result"].get("isError"), None);
    assert_eq!(last(&out)["result"]["structuredContent"], json!({"result": false}));
    proxy.handle(tool(2, "paste_text", password)).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::PASSWORD_REFUSED);
    proxy.handle(tool(3, "press_key", json!({"instance_id": "i", "selector": "#register-password", "key": "P"}))).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::PASSWORD_REFUSED);
    assert!(typed(tmp).is_empty(), "nothing typed into the password field: {:?}", typed(tmp));
    // the other fields, and a key (« Enter ») in the password field, go on
    proxy.handle(tool(4, "type_text", json!({"instance_id": "i", "selector": "#lastname", "text": "Testeur"}))).await;
    assert_eq!(text_of(&last(&out)), "did type_text");
    proxy
        .handle(tool(5, "press_key", json!({"instance_id": "i", "selector": "#register-password", "key": "Enter"})))
        .await;
    assert_eq!(text_of(&last(&out)), "did press_key");
    // a script writing the password field: refused before the page
    let script = "document.getElementById('register-password').value = 'Provisoire123!'";
    proxy.handle(tool(6, "execute_script", json!({"instance_id": "i", "script": script}))).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::SCRIPT_PASSWORD_REFUSED);
    assert!(!calls(tmp).iter().any(|c| c["arguments"]["script"].as_str().is_some_and(|s| s.contains("Provisoire"))));
    // the terms: the person's to tick, by a click or a script; the newsletter box is not the terms
    proxy.handle(tool(7, "click_element", json!({"instance_id": "i", "selector": "#cgu"}))).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::TERMS_REFUSED);
    proxy
        .handle(tool(
            8,
            "execute_script",
            json!({"instance_id": "i", "script": "document.getElementById('cgu').checked = true"}),
        ))
        .await;
    // a script that ticks: never run (hotfix v2: only a read V8 proves runs, and this proxy has no DevTools)
    assert_eq!(text_of(&last(&out)), browser_mcp::SCRIPT_UNCHECKED_REFUSED, "a script that writes is never run");
    assert!(!calls(tmp).iter().any(|c| c["arguments"]["script"].as_str().is_some_and(|s| s.contains(".checked = true"))));
    proxy.handle(tool(9, "click_element", json!({"instance_id": "i", "selector": "#newsletter"}))).await;
    assert_eq!(text_of(&last(&out)), "did click_element");
    assert_eq!(typed(tmp), ["type_text #lastname", "press_key #register-password", "click_element #newsletter"]);
    // a script sent under another tool's name is a script (DeepSeek on the lab, 13/11): checked as one, never given to
    // the real server (hotfix v2: only V8's proof of a read runs it; this proxy has no DevTools)
    proxy
        .handle(tool(
            10,
            "take_screenshot",
            json!({"instance_id": "i", "script": "return document.title", "full_page": true}),
        ))
        .await;
    assert_eq!(text_of(&last(&out)), browser_mcp::SCRIPT_UNCHECKED_REFUSED);
    assert!(!calls(tmp).iter().any(|c| c["name"] == "execute_script" && c["arguments"]["script"] == "return document.title"));
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn what_slips_past_the_checks_before_a_call_is_undone_after_it() {
    // review 13/11: a click on the terms' text (the real server finds it by `text_match`, the selector unused), the
    // field and the password in a script's `args`; a password field drawn 600 ms after the panel opens, waited for
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let (proxy, out) = guarded(tmp).await;
    let terms =
        json!({"instance_id": "i", "selector": "label", "text_match": "J'accepte les conditions générales de vente"});
    proxy.handle(tool(1, "click_element", terms)).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::TERMS_UNDONE);
    assert_eq!(last(&out)["result"]["structuredContent"], json!({"result": false}));
    // a box drawn inside the terms' label: refused before the click (its label's box looked at)
    proxy.handle(tool(2, "click_element", json!({"instance_id": "i", "selector": ".custom-checkbox span.box"}))).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::TERMS_REFUSED);
    let script = json!({"instance_id": "i", "script": "const e = document.querySelector(arguments[0]); e.value = arguments[1];",
                        "args": ["#register-password", "Provisoire123!"]});
    proxy.handle(tool(3, "execute_script", script)).await;
    // the page's password field named in its args: refused before it runs (fifth review 13/11)
    assert_eq!(text_of(&last(&out)), browser_mcp::SCRIPT_PASSWORD_REFUSED);
    // the panel opened, then at once its password field: waited for, then refused, never typed
    proxy.handle(tool(4, "click_element", json!({"instance_id": "i", "selector": "#open-register"}))).await;
    let late = json!({"instance_id": "i", "selector": "#late-password", "text": "Provisoire123!"});
    proxy.handle(tool(5, "type_text", late)).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::PASSWORD_REFUSED);
    assert!(!calls(tmp).iter().any(|c| c["name"] == "type_text"), "nothing typed");
    // « P » with Shift is a character too
    let shift = json!({"instance_id": "i", "selector": "#register-password", "key": "p", "modifiers": ["Shift"]});
    proxy.handle(tool(6, "press_key", shift)).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::PASSWORD_REFUSED);
    // refusing cookies is no terms box; a passenger's name no password
    let cookies = "document.querySelector('.fc-cta-do-not-consent').click()";
    // (past the guards: refused only as unproven reads, hotfix v2, this proxy having no DevTools)
    proxy.handle(tool(7, "execute_script", json!({"instance_id": "i", "script": cookies}))).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::SCRIPT_UNCHECKED_REFUSED);
    let passenger = "document.querySelector('#passenger-firstname').value = 'Camille'";
    proxy.handle(tool(8, "execute_script", json!({"instance_id": "i", "script": passenger}))).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::SCRIPT_UNCHECKED_REFUSED);
    // none of the proxy's own refusals counts as a step that failed
    assert_eq!(proxy.failures.load(Ordering::SeqCst), 0);
    proxy.stop_child().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn without_its_own_world_the_guard_refuses_a_script_that_acts_on_a_card_page() {
    // second review of card-guards, 14/11: the guard looked silently through the page's own world when DevTools
    // couldn't be reached (here: nothing listens on its port); now a script that writes or clicks is refused where the
    // page has card fields, and reading goes on, and so do pages without cards
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let (proxy, out) = guarded(tmp).await;
    std::fs::write(tmp.join("cardpage"), "").unwrap();
    let click = json!({"instance_id": "i", "script": "document.querySelector('#go').click()"});
    proxy.handle(tool(1, "execute_script", click.clone())).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::SCRIPT_BLIND_REFUSED);
    assert!(!calls(tmp).iter().any(|c| c["arguments"]["script"].as_str().is_some_and(|s| s.contains("#go"))));
    // a read there: refused too since the hotfix of 15/11 (no script on a page showing card fields)
    proxy.handle(tool(2, "execute_script", json!({"instance_id": "i", "script": "return document.title"}))).await;
    assert_eq!(text_of(&last(&out)), browser_mcp::SCRIPT_ON_CARDS_REFUSED);
    std::fs::remove_file(tmp.join("cardpage")).unwrap();
    proxy.handle(tool(3, "execute_script", click)).await;
    // past the guards there too, never run without V8's proof of a read (hotfix v2)
    assert_eq!(text_of(&last(&out)), browser_mcp::SCRIPT_UNCHECKED_REFUSED);
    proxy.stop_child().await;
}

/// hold_gate.py while a card payment (or the vault typing a password) holds the browser: `PAYING`, as an `isError`.
const PAYING_SERVER: &str = r##"
import json, sys
PAYING = ("A card payment is going on in this browser (or the bridge is typing a saved password): nobody may read the "
          "page. Your browser tools wait until it ends: if you started a payment, use confirm_payment (or "
          "cancel_payment) from the BipAgents tools; otherwise wait a few minutes, the bridge gives the browser back "
          "by itself.")
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    if m["method"] == "initialize":
        r = {"protocolVersion": m["params"]["protocolVersion"], "capabilities": {"tools": {}},
             "serverInfo": {"name": "stealth", "version": "1"}}
    elif m["method"] == "tools/list":
        r = {"tools": [{"name": "navigate", "inputSchema": {"type": "object"},
                        "outputSchema": {"type": "object", "additionalProperties": True}}]}
    else:
        r = {"content": [{"type": "text", "text": PAYING}], "isError": True}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"##;

#[tokio::test(flavor = "multi_thread")]
async fn a_payment_holding_the_browser_is_no_streak_of_failed_steps() {
    // review 13/11: like the person's hold, a payment's (or the vault fill's) is no step that failed on the page
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let config = config(tmp);
    std::fs::write(tmp.join("server.py"), PAYING_SERVER).unwrap();
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await;
    proxy.handle(json!({"jsonrpc": "2.0", "id": 99, "method": "tools/list"})).await;
    for id in 1..=3 {
        proxy.handle(tool(id, "navigate", json!({"instance_id": "i", "url": "https://shop.example/"}))).await;
    }
    let third = last(&out);
    proxy.stop_child().await;
    assert!(text_of(&third).starts_with("A card payment is going on"), "{third}");
    assert!(!text_of(&third).contains(browser_mcp::STREAK_HINT), "{third}");
    assert_eq!(third["result"].get("isError"), None);
    assert_eq!(third["result"]["structuredContent"], json!({"success": false}));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_browser_that_cannot_start_is_never_a_json_rpc_error() {
    // review 13/11: Hermes counts a JSON-RPC error (McpError) against the server, as `isError` (`_on_failure`)
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let mut config = config(tmp);
    let init_result = json!({"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                             "serverInfo": {"name": "stealth", "version": "1"}});
    let tools = json!({"tools": [{"name": "navigate", "inputSchema": {"type": "object"},
                                  "outputSchema": {"type": "object", "additionalProperties": true}}]});
    std::fs::write(tmp.join("cache.json"), json!({"initialize": init_result, "tools/list": tools}).to_string())
        .unwrap();
    config.insert("start".into(), json!(["false"])); // its container does not start
    let out = Out::default();
    let proxy = Proxy::new(config, Box::new(out.clone()));
    proxy.handle(init()).await;
    for id in 1..=3 {
        proxy.handle(tool(id, "navigate", json!({"instance_id": "i", "url": "https://shop.example/"}))).await;
        let answer = last(&out);
        assert!(answer.get("error").is_none(), "a strike for Hermes' breaker: {answer}");
        assert!(text_of(&answer).starts_with("the agent's browser could not start"), "{answer}");
        assert_eq!(answer["result"]["structuredContent"], json!({"success": false}));
    }
}

#[test]
fn a_terms_box_that_also_names_cookies_is_the_terms() {
    // second review 13/11: many French sign-up forms word their terms box this way since the CNIL's guidance (terms,
    // privacy and cookie policies in one sentence); a cookie banner's own choice is not the terms
    let not_terms = [
        "J'ai lu et j'accepte les conditions générales d'utilisation et la politique de confidentialité et de cookies  cgu cgu",
        "Ich akzeptiere die AGB und die Datenschutz- und Cookie-Richtlinie  agb agb",
        "I agree to the Terms of Use and the Privacy and Cookie Policy  terms terms",
    ]
    .into_iter()
    .filter(|label| !browser_mcp::terms_box(label))
    .collect::<Vec<_>>();
    assert!(not_terms.is_empty(), "left to the agent: {not_terms:#?}");
    assert!(!browser_mcp::terms_box("Accepter les cookies de mesure d'audience (consentement) cookie-consent"));
}

/// What `guard_before` answers on a sign-up page (a password field, the terms not ticked) and on a page with neither.
fn signup_page() -> Value {
    json!({"pw": {"#register-password": "", "#pass1": "", "#pass2": ""}, "known": [], "boxes": ["#cgu"]})
}
fn plain_page() -> Value {
    json!({"pw": {}, "known": [], "boxes": []})
}

#[test]
fn a_script_that_fills_a_form_and_sends_it_is_refused() {
    // second review 13/11: the fields (the password among them) set from a table, the terms ticked, then sent, in one
    // script: the site got everything before the check after the call
    let page = signup_page();
    let script = "const data = {'#lastname': 'Testeur', '#register-password': 'Provisoire123!'}; \
        for (const [s, v] of Object.entries(data)) document.querySelector(s).value = v; \
        document.querySelector('#cgu').checked = true; document.getElementById('register-form').requestSubmit();";
    assert!(browser_mcp::script_fills_and_sends(script, Some(&page)));
    // third review: an app without a form, its button clicked by the same script
    let app = "const data = {'#email': 'c@example.org', '#password': 'Provisoire123!'}; \
        for (const [s, v] of Object.entries(data)) document.querySelector(s).value = v; document.querySelector('#create').click();";
    assert!(browser_mcp::script_fills_and_sends(app, Some(&page)));
    assert!(browser_mcp::script_fills_and_sends(
        "document.querySelector('#cgu').checked = true; document.querySelector('#go').click()",
        Some(&page)
    ));
    // one step at a time goes on: a field set, the form sent
    assert!(!browser_mcp::script_fills_and_sends("document.querySelector('#lastname').value = 'Testeur'", Some(&page)));
    assert!(!browser_mcp::script_fills_and_sends(
        "document.getElementById('register-form').requestSubmit()",
        Some(&page)
    ));
    // the page can't say: as a sign-up page
    assert!(browser_mcp::script_fills_and_sends(app, None));
}

#[test]
fn a_search_typed_and_sent_by_a_script_goes_on() {
    // third review 13/11: a site's search box filled and sent in one script: no password, no box
    let page = plain_page();
    let refused = [
        "document.querySelector('input[name=q]').value = 'doliprane'; document.querySelector('#search-form').submit();",
        "const f = document.forms[0]; f.elements.q.value = 'horaires'; f.requestSubmit(); 'ok'",
    ]
    .into_iter()
    .filter(|script| browser_mcp::script_fills_and_sends(script, Some(&page)))
    .collect::<Vec<_>>();
    assert!(refused.is_empty(), "refused: {refused:#?}");
}

// --- fourth review of fix-signup-hold (81c8cbdbf) ---------------------------------------------------------------------

#[test]
fn review4_a_box_clicked_or_a_password_named_otherwise_then_a_button_clicked_is_refused() {
    // decided from the page (a password field, the terms not ticked), not the script's words
    let page = signup_page();
    let passed = [
        // ticking a box is a click on it
        "document.querySelector('#cgu').click(); document.querySelector('#create').click(); 'sent';",
        // in a loop
        "[document.querySelector('#cgu'), document.querySelector('#create')].forEach((e) => e.click());",
        // WordPress's password fields (pass1, pass2), set from a table
        "const data = {'#email': 'camille@example.org', '#pass1': 'Provisoire123!', '#pass2': 'Provisoire123!'}; \
         for (const [s, v] of Object.entries(data)) document.querySelector(s).value = v; \
         document.querySelector('#create').click(); 'sent';",
    ]
    .into_iter()
    .filter(|script| !browser_mcp::script_fills_and_sends(script, Some(&page)))
    .collect::<Vec<_>>();
    assert!(passed.is_empty(), "not refused: {passed:#?}");
}

#[test]
fn review4_a_shop_filter_or_a_reset_e_mail_sent_by_a_script_goes_on() {
    // no password field, no terms box on the page: a product filter applied, a « mot de passe oublié » e-mail asked for
    let page = plain_page();
    let refused = [
        "document.querySelector('#filter-bio').checked = true; document.querySelector('#apply-filters').click();",
        "document.querySelector('#forgot-password-email').value = 'camille@example.org'; \
         document.querySelector('#send-reset-link').click();",
    ]
    .into_iter()
    .filter(|script| browser_mcp::script_fills_and_sends(script, Some(&page)))
    .collect::<Vec<_>>();
    assert!(refused.is_empty(), "refused: {refused:#?}");
}

#[test]
fn a_scripts_answer_holding_a_password_is_checked_in_the_page() {
    // the value never leaves the page: the answer goes in, true or false comes back
    let script = browser_mcp::shows_password("{\"result\": \"[\\\"camille\\\", \\\"Xy7-secret\\\"]\"}");
    assert!(script.starts_with("/* bipagents guard: shown */"));
    assert!(script.contains("'password'") && script.contains("A.includes(f)"));
    assert!(!script.contains("return e.value") && !script.contains("JSON.stringify({"));
}

// --- fifth review of fix-signup-hold (80be896d5) ----------------------------------------------------------------------

#[test]
fn review5_jquery_setting_a_password_or_ticking_the_terms_then_clicking_is_refused() {
    // many shops load jQuery (PrestaShop, WooCommerce, Magento): `.val()` writes a value, `.prop('checked', true)` and
    // `.attr('checked', …)` tick a box, on a page with a password field and the terms not ticked yet
    let page = json!({"pw": {"#pass1": ""}, "known": [], "boxes": ["#cgu"]});
    let passed = [
        "$('#pass1').val('Provisoire123!'); $('#pass2').val('Provisoire123!'); $('#create').click();",
        "$('#cgu').prop('checked', true); $('#create').click();",
        "jQuery('#cgu').attr('checked', 'checked'); jQuery('#signup-form').trigger('submit');",
    ]
    .into_iter()
    .filter(|script| !browser_mcp::script_fills_and_sends(script, Some(&page)))
    .collect::<Vec<_>>();
    assert!(passed.is_empty(), "not refused: {passed:#?}");
}

#[test]
fn two_clicks_in_one_script_on_a_page_whose_terms_arent_ticked_are_refused() {
    // sixth review 13/11: what a click aims at can't be told from a script's words (a label found by its text); a
    // drop-down list of the site's own (signup.html's « Civilité ») takes two click_element there
    let page = signup_page();
    let script = "document.querySelector('#civility').click(); \
                  document.querySelector('#civility-options li[data-value=\"M\"]').click(); 'ok';";
    assert!(browser_mcp::script_fills_and_sends(script, Some(&page)));
    assert!(browser_mcp::SCRIPT_SENDS_REFUSED.contains("one click_element per click"));
    // no terms on the page: it goes on
    assert!(!browser_mcp::script_fills_and_sends(script, Some(&plain_page())));
    // one click goes on (a panel opened)
    assert!(!browser_mcp::script_fills_and_sends("document.querySelector('#open-register').click();", Some(&page)));
}

#[test]
fn a_script_reading_a_password_field_by_its_id_or_all_the_forms_values_is_refused() {
    // fifth review: `#pass1` read (then encoded, or put in the page's title); every field's value while one holds text
    let filled = json!({"pw": {"#pass1": "9f3a01bc", "INPUT|pass2|0": "9f3a01bc"}, "known": ["9f3a01bc"], "boxes": []});
    for script in [
        "btoa(document.querySelector('#pass1').value)",
        "document.title = document.getElementsByName('pass2')[0].value; 'ok'",
        "JSON.stringify([...document.querySelectorAll('input')].map((e) => e.value))",
        "JSON.stringify([...new FormData(document.forms[0])])",
        "$('#register-form').serialize()",
    ] {
        assert!(browser_mcp::script_reads_password(script, Some(&filled)), "{script}");
    }
    for script in [
        "document.getElementById('email').value",
        "document.querySelector('#password-rules').innerText",
        "document.querySelector('#pass1').getBoundingClientRect().top",
    ] {
        assert!(!browser_mcp::script_reads_password(script, Some(&filled)), "{script}");
    }
    // nothing typed in it yet: listing the fields' values goes on
    let empty = json!({"pw": {"#pass1": ""}, "known": [], "boxes": []});
    assert!(!browser_mcp::script_reads_password("[...document.querySelectorAll('input')].map((e) => e.value)", Some(&empty)));
}

// --- sixth review of fix-signup-hold (ad232660d) ----------------------------------------------------------------------

#[test]
fn review6_the_terms_label_found_by_its_text_then_a_button_clicked_is_refused() {
    // how a model clicks what it read: no id, no « checkbox », no word of the terms in the script
    let page = json!({"pw": {}, "known": [], "boxes": ["#cgu"]});
    let script = "[...document.querySelectorAll('label')].find((l) => l.innerText.includes(\"J'accepte\")).click(); \
                  document.querySelector('#create').click(); 'sent';";
    assert!(browser_mcp::script_fills_and_sends(script, Some(&page)), "{script}");
}

#[test]
fn review6_a_password_read_as_the_focused_field_is_refused() {
    // the password field the agent clicked into (before vault_create_login), read back as `document.activeElement`
    let page = json!({"pw": {"#pass1": "1a2b3c4d"}, "known": ["1a2b3c4d"], "boxes": []});
    let script = "btoa(document.activeElement.value)";
    assert!(browser_mcp::script_reads_password(script, Some(&page)), "{script}");
}

// --- seventh review of fix-signup-hold (e2070a5d6): a remark, not blocking ------------------------------------------

#[test]
fn review7_a_form_sent_through_the_prototype_after_one_click_is_refused() {
    // one click (the terms' label found by its text), then the form sent without `.submit(` / `requestSubmit` words
    let page = json!({"pw": {}, "known": [], "boxes": ["#cgu"]});
    let passed = [
        "[...document.querySelectorAll('label')].find((l) => l.innerText.includes(\"J'accepte\")).click(); \
         HTMLFormElement.prototype.submit.call(document.getElementById('register-form'));",
        "[...document.querySelectorAll('label')].find((l) => l.innerText.includes(\"J'accepte\")).click(); \
         document.getElementById('register-form')['submit']();",
    ]
    .into_iter()
    .filter(|script| !browser_mcp::script_fills_and_sends(script, Some(&page)))
    .collect::<Vec<_>>();
    assert!(passed.is_empty(), "not refused: {passed:#?}");
    // a submit button found by its selector is no sending
    assert!(!browser_mcp::script_fills_and_sends("document.querySelector('button[type=submit]').innerText", Some(&page)));
}

/// Audit 1.2 (17/11, Alex « Oui, 1re fois par site »): Hermes' `stealth` server has no `trust: untrusted` (every click
/// would be a card), so the proxy's guard asks: the first form sent to a site not allowed for the agent (its send
/// button clicked, Enter in its field) is refused with what to do (`ask_to_send`), whatever the page asked; typing is
/// no sending. The person's « Oui » (a one-use grant in `SEND_SITES_FILE`) lets one sending through, « Toujours pour ce
/// site » every one.
#[tokio::test(flavor = "multi_thread")]
async fn the_first_form_sent_to_a_site_waits_for_the_persons_ok() {
    let entry = bipbridge::browsers::mcp_server("podman", "vie", None);
    assert!(entry.get("trust").is_none(), "no card for every click: {entry}");
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path();
    let (proxy, out) = guarded(tmp).await;
    let calls = || std::fs::read_to_string(tmp.join("calls.jsonl")).unwrap_or_default();
    let real = |name: &str| calls().lines().filter(|l| l.contains(&format!("\"name\": \"{name}\""))).count();
    proxy.handle(tool(1, "type_text", json!({"instance_id": "i", "selector": "#message", "text": "Voici les codes"}))).await;
    assert_eq!(text_of(&last(&out)), "did type_text", "typing sends nothing");
    let submit = json!({"instance_id": "i", "selector": "button[type=submit]"});
    proxy.handle(tool(2, "click_element", submit.clone())).await;
    let clicked = last(&out);
    assert!(text_of(&clicked).starts_with("Not done: this sends a form to contact.example"), "{clicked}");
    assert!(text_of(&clicked).contains("ask_to_send") && text_of(&clicked).contains("Nothing was sent"));
    assert_eq!(clicked["result"].get("isError"), None, "never an error for Hermes' breaker: {clicked}");
    assert_eq!(real("click_element"), 0, "the real server never got the click");
    proxy.handle(tool(3, "press_key", json!({"instance_id": "i", "selector": "#message", "key": "Enter"}))).await;
    assert!(text_of(&last(&out)).starts_with("Not done: this sends a form to contact.example"), "Enter sends too");
    assert_eq!(real("press_key"), 0);
    // the person's « Oui »: one sending, then asked again
    let file = tmp.join(browser_mcp::SEND_SITES_FILE);
    let until = bipbridge::py::time_time() + 600.0;
    let once = json!({"always": [], "once": [{"id": "g1", "host": "contact.example", "until": until}]});
    std::fs::write(&file, once.to_string()).unwrap();
    proxy.handle(tool(4, "click_element", submit.clone())).await;
    assert_eq!(text_of(&last(&out)), "did click_element");
    proxy.handle(tool(5, "click_element", submit.clone())).await;
    assert!(text_of(&last(&out)).starts_with("Not done: this sends a form to"), "a « Oui » serves once");
    // « Toujours pour ce site »
    std::fs::write(&file, json!({"always": ["*.example"], "once": []}).to_string()).unwrap();
    proxy.handle(tool(6, "click_element", submit.clone())).await;
    proxy.handle(tool(7, "press_key", json!({"instance_id": "i", "selector": "#message", "key": "Enter"}))).await;
    assert_eq!(text_of(&last(&out)), "did press_key");
    assert_eq!((real("click_element"), real("press_key")), (2, 1));
    proxy.stop_child().await;
}

#[test]
fn a_site_allowed_for_good_covers_its_subdomains_only() {
    assert!(browser_mcp::send_covered(&["*.example.com".into()], "example.com"));
    assert!(browser_mcp::send_covered(&["*.example.com".into()], "Login.Example.com."));
    assert!(!browser_mcp::send_covered(&["*.example.com".into()], "badexample.com"));
    assert!(!browser_mcp::send_covered(&["shop.example".into()], ""));
}
