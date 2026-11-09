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
        assert_eq!(refused["result"]["isError"], json!(true));
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
    assert_eq!(calls(tmp).iter().map(|c| c["name"].as_str().unwrap()).collect::<Vec<_>>(), ["execute_script"]);
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
    assert_eq!(calls(tmp).len(), 2);
    assert_eq!(calls(tmp)[0]["arguments"]["timeout"], json!(3000), "3 s to find it, not the real server's 10");
    proxy.handle(tool(5, "list_tabs", json!({"instance_id": "319ceedb"}))).await;
    proxy.handle(tool(6, "click_element", json!({"selector": "a", "timeout": 8000}))).await;
    assert_eq!(calls(tmp).len(), 4);
    assert_eq!(calls(tmp)[3]["arguments"]["timeout"], json!(8000), "the agent's own time kept");

    // a screenshot: an image block, the agent's path never passed on (it would land in the container)
    std::fs::remove_file(tmp.join("calls.jsonl")).unwrap();
    proxy.handle(tool(7, "take_screenshot", json!({"instance_id": "319ceedb", "file_path": "/root/shot.png"}))).await;
    let answer = out.messages().last().unwrap().clone();
    assert_eq!(answer["id"], json!(7));
    let content = answer["result"]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "image");
    assert_eq!(content[0]["mimeType"], "image/jpeg");
    assert!(content[1]["text"].as_str().unwrap().contains("not written to /root/shot.png"));
    assert_eq!(calls(tmp)[0]["arguments"], json!({"instance_id": "319ceedb", "format": "jpeg"}));
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
