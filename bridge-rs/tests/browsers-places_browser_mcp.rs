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
    let config = config(tmp); // idle 600 s, `ended` beside `busy`, after_run 180 s by default
    let proxy = Proxy::new(config.clone(), Box::new(Out::default()));
    assert_eq!(proxy.after_run, browser_mcp::AFTER_RUN_SECONDS as f64);
    assert_eq!(proxy.ended_path, tmp.join("ended"));
    let (busy, ended) = (tmp.join("busy"), tmp.join("ended"));
    // No run told its end: only the 600 s rule.
    set_mtime(&busy, 400);
    assert!(!proxy.done_with());
    // The run ended 2 minutes ago, its last call before: not yet.
    set_mtime(&ended, 120);
    assert!(!proxy.done_with());
    // 3 minutes: done with.
    set_mtime(&ended, 181);
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
    assert!(answer["error"]["message"].as_str().unwrap().contains("could not start"), "{answer}");
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
    let message = out.messages().last().unwrap()["error"]["message"].as_str().unwrap().to_string();
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
    assert_eq!((answer["id"].clone(), answer["result"]["isError"].clone()), (json!(1), json!(true)));
    assert!(answer["result"]["content"][0]["text"].as_str().unwrap().contains("credit is used up"), "{answer}");
    assert!(ran(tmp).is_empty() && !proxy.running());
    // stopped (the grace used too): the same
    std::fs::write(&credit, json!({"state": "stopped"}).to_string()).unwrap();
    proxy.handle(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "navigate"}})).await;
    assert_eq!(out.messages().last().unwrap()["result"]["isError"], json!(true));
    // the credit back: the next call starts it as usual
    std::fs::write(&credit, json!({"state": "ok"}).to_string()).unwrap();
    proxy.handle(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "navigate"}})).await;
    assert_eq!(out.messages().last().unwrap()["result"]["content"][0]["text"], "did navigate");
    assert_eq!(ran(tmp), vec!["start"]);
    proxy.stop_child().await;
}
