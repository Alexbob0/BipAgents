//! An agent's browser tools, started when used (browsers.py): the MCP server Hermes keeps connected in place of the
//! one inside the browser container.
//!
//! It answers Hermes' handshake and tool list with what the real server said last time, starts the agent's browser
//! container and the real server (stealth-browser-mcp, in it) at the first call that needs them, relays everything
//! while they run, and stops both after `idle` seconds of nobody using that browser. A container stopped this way
//! costs no memory; the next call starts it again (a few seconds) with the same profile, logins included.
//!
//! Sooner when the work that used it is over (2026-10-26, a server's memory shared by its places): the bridge touches
//! the `ended` file when a run that called the browser's tools completes or fails and no other run of the agent that
//! used it still goes on (runs.rs, a background task's included); nobody using it since (`busy` older than `ended`:
//! no call, nobody watching or driving its desktop), it stops `after_run` seconds (180) after that end.
//!
//! « Using » is shared through the `busy` file, touched at each call (and every 30 s of a long one) by every copy of
//! this server (the agent's gateway, a `hermes chat` beside it), every minute by the bridge while the person watches or
//! drives that desktop, and here while something holds a connection to Chrome's DevTools (`cdp_port`: the vault,
//! Hermes' own browser tools). None stops the browser while another uses it.
//!
//!     python3 browser_mcp.py <config.json>
//!
//! `config.json` (written by browsers.setup): `start`, `ready`, `active`, `server` and `stop` (commands),
//! `cache` (the file keeping the real server's answers), `busy` (a file), `cdp_port`, `idle` (seconds). Standard
//! library only: Hermes runs it with the bridge's Python, from anywhere.
//!
//! Rust only: `shared_cache` (the answers any agent's server gave: the same image serves the same tools, so a new
//! agent's Hermes connects without starting anything), `ready_seconds` (how long its container may take to start),
//! and `unit` (`{path, text}`), `profile` and `reload`: the container's systemd unit and its Chrome profile folder,
//! written at the first start rather than with the agent (browsers.rs), then `reload` run once.
//!
//! Port of `bridge/bipbridge/browser_mcp.py` (owner group: browsers-places).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::{BoxFuture, FutureExt, Shared};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt};
use tokio::sync::{oneshot, Notify};

use crate::Dict;

pub const LISTS: [&str; 4] = ["tools/list", "resources/list", "resources/templates/list", "prompts/list"];
/// A screenshot comes back as one JSON line.
pub const LINE_LIMIT: usize = 64 * 1024 * 1024;
/// How long a container may take to show its screen, by default: a cold start on a loaded server (several browsers
/// at once, memory in swap) takes far longer than the few seconds of an idle one.
pub const READY_SECONDS: u64 = 90;
pub const TCP_TABLES: [&str; 2] = ["/proc/net/tcp", "/proc/net/tcp6"];
/// How long after the end of the work that used it the browser stops, unless used again (`after_run`).
pub const AFTER_RUN_SECONDS: u64 = 180;

/// What Hermes asks while connecting, answered without starting anything once the real server has said it (the
/// others, never asked while it ran, as empty).
fn empty(method: &str) -> Option<Value> {
    match method {
        "resources/list" => Some(json!({"resources": []})),
        "resources/templates/list" => Some(json!({"resourceTemplates": []})),
        "prompts/list" => Some(json!({"prompts": []})),
        _ => None,
    }
}

pub fn log(text: &str) {
    let mut err = std::io::stderr();
    let _ = writeln!(err, "[browser-mcp] {text}");
    let _ = err.flush();
}

/// `json.dumps(message, separators=(",", ":"))`.
fn line(message: &Value) -> Vec<u8> {
    let mut out = crate::py::Dumps::new().compact().dumps(message).into_bytes();
    out.push(b'\n');
    out
}

fn strs(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items.iter().map(crate::py::py_str).collect(),
        _ => Vec::new(),
    }
}

/// Python's text for an `OSError` from starting a program.
fn os_error_text(err: &std::io::Error, program: &str) -> String {
    match err.raw_os_error() {
        Some(code) => {
            let text = err.to_string();
            let text = text.split(" (os error").next().unwrap_or(&text).to_string();
            format!("[Errno {code}] {text}: {}", crate::py::py_repr(&json!(program)))
        }
        None => err.to_string(),
    }
}

/// `run(command, check)`: the command's exit code (stdout and stderr to /dev/null); `check`: a failure is an error.
pub async fn run(command: &[String], check: bool) -> Result<i32, String> {
    use std::os::unix::process::ExitStatusExt;
    let Some((program, rest)) = command.split_first() else { return Err("list index out of range".into()) };
    let status = tokio::process::Command::new(program)
        .args(rest)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map_err(|e| os_error_text(&e, program))?;
    let code = status.code().unwrap_or_else(|| -status.signal().unwrap_or(0));
    if check && code != 0 {
        let name = Path::new(program).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        return Err(format!("{name} {} failed ({code})", rest.iter().take(2).cloned().collect::<Vec<_>>().join(" ")));
    }
    Ok(code)
}

/// Something on this machine holds a connection to the browser's DevTools port (Linux: /proc/net/tcp).
pub fn devtools_connected(port: Option<&Value>, tables: &[String]) -> bool {
    let Some(port) = port.filter(|p| crate::py::truthy(p)) else { return false };
    let Ok(port) = crate::py::py_int(port) else { return false };
    let wanted = format!(":{port:04X}");
    for table in tables {
        let Ok(text) = std::fs::read_to_string(table) else { continue };
        for line in crate::hermes::splitlines(&text).iter().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            // local address, remote address, state (01: established)
            if parts.len() > 3 && parts[3] == "01" && (parts[1].ends_with(&wanted) || parts[2].ends_with(&wanted)) {
                return true;
            }
        }
    }
    false
}

/// The real server while it runs.
struct Server {
    pid: Option<u32>,
    stdin: Arc<tokio::sync::Mutex<Option<tokio::process::ChildStdin>>>,
    exited: Arc<AtomicBool>,
    exit: Arc<Notify>,
}

impl Server {
    fn running(&self) -> bool {
        !self.exited.load(Ordering::SeqCst)
    }

    async fn wait(&self) {
        while self.running() {
            let notified = self.exit.notified();
            if !self.running() {
                break;
            }
            notified.await;
        }
    }
}

type Pending = HashMap<i64, oneshot::Sender<Result<Dict, String>>>;
type Starting = Shared<BoxFuture<'static, Result<(), String>>>;

pub struct Proxy {
    pub config: Dict,
    pub idle: f64,
    /// Seconds after the end of the run that used it (the `ended` file, beside `busy` unless set); 0: never sooner.
    pub after_run: f64,
    pub ended_path: PathBuf,
    pub cache_path: PathBuf,
    pub cache: Mutex<Dict>,
    output: Mutex<Box<dyn Write + Send>>,
    server: Mutex<Option<Arc<Server>>>,
    client_init: Mutex<Value>,
    pending: Arc<Mutex<Pending>>,
    next_id: AtomicI64,
    /// tool calls under way
    pub calls: AtomicI64,
    starting: Mutex<Option<Starting>>,
    /// starting and stopping the container, one at a time
    switching: tokio::sync::Mutex<()>,
    /// /proc/net/tcp and tcp6 (tests give their own)
    pub tcp_tables: Mutex<Vec<String>>,
}

impl Proxy {
    pub fn new(config: Dict, output: Box<dyn Write + Send>) -> Arc<Proxy> {
        let idle = config.get("idle").and_then(|v| crate::py::py_float(v).ok()).unwrap_or(600.0);
        let after_run =
            config.get("after_run").and_then(|v| crate::py::py_float(v).ok()).unwrap_or(AFTER_RUN_SECONDS as f64);
        let busy = PathBuf::from(config.get("busy").map(crate::py::py_str).unwrap_or_default());
        let ended_path = match config.get("ended").map(crate::py::py_str).filter(|p| !p.is_empty()) {
            Some(path) => PathBuf::from(path),
            None => busy.parent().map(|p| p.join("ended")).unwrap_or_else(|| PathBuf::from("ended")),
        };
        let cache_path = PathBuf::from(config.get("cache").map(crate::py::py_str).unwrap_or_default());
        let mut cache = read_cache(&cache_path);
        if !cache.contains_key("initialize") {
            // never started yet: what another agent's server said (the same image), so connecting starts nothing
            if let Some(shared) = config.get("shared_cache").map(crate::py::py_str).filter(|p| !p.is_empty()) {
                cache = read_cache(Path::new(&shared));
            }
        }
        Arc::new(Proxy {
            config,
            idle,
            after_run,
            ended_path,
            cache_path,
            cache: Mutex::new(cache),
            output: Mutex::new(output),
            server: Mutex::new(None),
            client_init: Mutex::new(json!({})),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicI64::new(1),
            calls: AtomicI64::new(0),
            starting: Mutex::new(None),
            switching: tokio::sync::Mutex::new(()),
            tcp_tables: Mutex::new(TCP_TABLES.iter().map(|s| s.to_string()).collect()),
        })
    }

    fn command(&self, name: &str) -> Vec<String> {
        strs(self.config.get(name))
    }

    // -- the two sides -------------------------------------------------------------------------------------------

    pub fn to_client(&self, message: &Value) {
        let mut out = self.output.lock().unwrap_or_else(|e| e.into_inner());
        let _ = out.write_all(&line(message));
        let _ = out.flush();
    }

    async fn to_server(&self, message: &Value) -> Result<(), String> {
        let server = self.server.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(server) = server else { return Err(String::new()) }; // AssertionError
        let mut stdin = server.stdin.lock().await;
        let Some(pipe) = stdin.as_mut() else { return Err(String::new()) };
        pipe.write_all(&line(message)).await.map_err(|e| e.to_string())?;
        pipe.flush().await.map_err(|e| e.to_string())
    }

    /// A request to the real server; its whole response (`result` or `error`).
    async fn request(&self, method: &str, params: Option<&Value>) -> Result<Dict, String> {
        let number = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(number, tx);
        let mut message = json!({"jsonrpc": "2.0", "id": number, "method": method});
        if let Some(params) = params.filter(|p| !p.is_null()) {
            message["params"] = params.clone();
        }
        self.to_server(&message).await?;
        rx.await.unwrap_or_else(|_| Err("its server stopped".into()))
    }

    // -- Hermes' messages ----------------------------------------------------------------------------------------

    pub async fn handle(self: &Arc<Self>, message: Value) {
        let Value::Object(message) = message else { return };
        let ident = message.get("id").cloned().unwrap_or(Value::Null);
        let method = match message.get("method") {
            None | Some(Value::Null) => {
                // Hermes answering a request of the real server (ids of the server's own numbering).
                if self.running() {
                    let _ = self.to_server(&Value::Object(message)).await;
                }
                return;
            }
            Some(Value::String(m)) => m.clone(),
            Some(_) => return,
        };
        if method == "initialize" {
            *self.client_init.lock().unwrap_or_else(|e| e.into_inner()) =
                message.get("params").filter(|p| crate::py::truthy(p)).cloned().unwrap_or_else(|| json!({}));
            let result: Result<(), String> = async {
                if !self.cache_has("initialize") {
                    self.ensure_server().await?; // the first time ever: what it serves is learned once
                }
                let init = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get("initialize").cloned();
                let init = init.ok_or_else(|| "'initialize'".to_string())?;
                self.to_client(&json!({"jsonrpc": "2.0", "id": ident, "result": init}));
                Ok(())
            }
            .await;
            if let Err(exc) = result {
                self.fail(&ident, &exc); // told to Hermes, which retries later
            }
            return;
        }
        if method == "notifications/initialized" {
            return; // the real server gets its own after its handshake
        }
        if method == "ping" {
            self.to_client(&json!({"jsonrpc": "2.0", "id": ident, "result": {}}));
            return;
        }
        if method.starts_with("notifications/") {
            if self.running() {
                let _ = self.to_server(&Value::Object(message)).await;
            }
            return;
        }
        if LISTS.contains(&method.as_str()) && !self.running() && (self.cache_has(&method) || empty(&method).is_some())
        {
            let cached = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(&method).cloned();
            let result = cached.or_else(|| empty(&method)).unwrap_or(Value::Null);
            self.to_client(&json!({"jsonrpc": "2.0", "id": ident, "result": result}));
            return;
        }
        let calling = method == "tools/call";
        if calling && self.credit_used() {
            // The month's AI credit used (credit.rs): the browser isn't started, the agent told plainly (chat goes on).
            self.to_client(&json!({"jsonrpc": "2.0", "id": ident, "result": {
                "content": [{"type": "text", "text": crate::credit::BROWSER_REFUSED}], "isError": true}}));
            return;
        }
        if calling {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.touch();
        }
        let result: Result<(), String> = async {
            self.ensure_server().await?;
            let mut response = self.request(&method, message.get("params")).await?;
            if LISTS.contains(&method.as_str()) {
                if let Some(result) = response.get("result").cloned() {
                    self.remember(&method, result);
                }
            }
            response.insert("id".into(), ident.clone());
            self.to_client(&Value::Object(response));
            Ok(())
        }
        .await;
        if let Err(exc) = result {
            self.fail(&ident, &exc);
        }
        if calling {
            self.calls.fetch_sub(1, Ordering::SeqCst);
            self.touch();
        }
    }

    /// The plan's AI credit used: the config's `credit` file (tests), else the place's own (credit.rs).
    fn credit_used(&self) -> bool {
        let file = self.config.get("credit").and_then(Value::as_str).filter(|p| !p.is_empty()).map(PathBuf::from);
        crate::credit::now_at(file.as_deref()).paused()
    }

    fn cache_has(&self, key: &str) -> bool {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).contains_key(key)
    }

    fn fail(&self, ident: &Value, exc: &str) {
        if !ident.is_null() {
            self.to_client(&json!({"jsonrpc": "2.0", "id": ident,
                                   "error": {"code": -32603, "message": format!(
                                       "the agent's browser could not start: {exc}\nTell your person simply, in \
their language: « I can't open web pages right now; I'll try again in a moment. »")}}));
        }
    }

    pub fn remember(&self, key: &str, value: Value) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let own = read_cache(&self.cache_path);
        if cache.get(key) == Some(&value) && own.get(key) == Some(&value) {
            return;
        }
        cache.insert(key.to_string(), value);
        let mut paths = vec![self.cache_path.clone()];
        if let Some(shared) = self.config.get("shared_cache").map(crate::py::py_str).filter(|p| !p.is_empty()) {
            paths.push(PathBuf::from(shared));
        }
        for path in paths {
            if let Err(exc) = write_cache(&path, &cache) {
                log(&format!("cache not saved: {exc}"));
            }
        }
    }

    // -- the real server -----------------------------------------------------------------------------------------

    pub fn running(&self) -> bool {
        self.server.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.running()).unwrap_or(false)
    }

    pub async fn ensure_server(self: &Arc<Self>) -> Result<(), String> {
        if self.running() {
            return Ok(());
        }
        let starting = {
            let mut slot = self.starting.lock().unwrap_or_else(|e| e.into_inner());
            match slot.as_ref().filter(|s| s.peek().is_none()) {
                Some(s) => s.clone(),
                None => {
                    let this = self.clone();
                    let task = tokio::spawn(async move { this.start().await });
                    let shared: Starting =
                        async move { task.await.unwrap_or_else(|e| Err(e.to_string())) }.boxed().shared();
                    *slot = Some(shared.clone());
                    shared
                }
            }
        };
        starting.await
    }

    async fn start(self: Arc<Self>) -> Result<(), String> {
        let _switching = self.switching.lock().await;
        if self.running() {
            return Ok(());
        }
        let result = self.start_server().await;
        if result.is_err() {
            self.stop_child().await;
        }
        result
    }

    async fn start_server(self: &Arc<Self>) -> Result<(), String> {
        let started = crate::py::monotonic();
        if write_unit(&self.config)? {
            // its first use: the container's unit was only written now
            run(&self.command("reload"), true).await?;
            log("container set up");
        }
        run(&self.command("start"), true).await?;
        let patience =
            self.config.get("ready_seconds").and_then(|v| crate::py::py_float(v).ok()).unwrap_or(READY_SECONDS as f64);
        loop {
            match run(&self.command("ready"), false).await {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => return Err(e),
            }
            if crate::py::monotonic() - started > patience {
                return Err(format!("its container did not start within {patience:.0} s"));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let command = self.command("server");
        let Some((program, rest)) = command.split_first() else { return Err("list index out of range".into()) };
        let mut child = tokio::process::Command::new(program)
            .args(rest)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| os_error_text(&e, program))?;
        let stdout = child.stdout.take();
        let server = Arc::new(Server {
            pid: child.id(),
            stdin: Arc::new(tokio::sync::Mutex::new(child.stdin.take())),
            exited: Arc::new(AtomicBool::new(false)),
            exit: Arc::new(Notify::new()),
        });
        let (exited, exit) = (server.exited.clone(), server.exit.clone());
        tokio::spawn(async move {
            let _ = child.wait().await;
            exited.store(true, Ordering::SeqCst);
            exit.notify_waiters();
        });
        *self.server.lock().unwrap_or_else(|e| e.into_inner()) = Some(server.clone());
        if let Some(stdout) = stdout {
            tokio::spawn(read_server(self.clone(), server, stdout));
        }
        let client_init = self.client_init.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let params = if crate::py::truthy(&client_init) {
            client_init
        } else {
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "bipagents", "version": "1"}})
        };
        let init = self.request("initialize", Some(&params)).await?;
        let Some(result) = init.get("result").cloned() else {
            let error = init.get("error").cloned().unwrap_or(Value::Null);
            return Err(format!("its server refused to start: {}", crate::py::py_repr(&error)));
        };
        self.remember("initialize", result);
        self.to_server(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await?;
        let tools = self.request("tools/list", Some(&json!({}))).await?;
        if let Some(result) = tools.get("result").cloned() {
            self.remember("tools/list", result);
        }
        self.touch();
        log(&format!("started in {:.1} s", crate::py::monotonic() - started));
        Ok(())
    }

    pub async fn stop(&self) {
        let _switching = self.switching.lock().await;
        self.stop_child().await;
        let _ = run(&self.command("stop"), false).await;
    }

    pub async fn stop_child(&self) {
        let server = self.server.lock().unwrap_or_else(|e| e.into_inner()).take();
        let Some(server) = server else { return };
        if !server.running() {
            return;
        }
        server.stdin.lock().await.take(); // closed
        if tokio::time::timeout(Duration::from_secs(5), server.wait()).await.is_err() {
            if let Some(pid) = server.pid {
                // SAFETY: kill(2) on the child this proxy started.
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGKILL);
                }
            }
        }
    }

    pub fn touch(&self) {
        let busy = PathBuf::from(self.config.get("busy").map(crate::py::py_str).unwrap_or_default());
        let touch = |p: &Path| -> std::io::Result<()> {
            let file = std::fs::OpenOptions::new().create(true).append(true).open(p)?;
            file.set_modified(std::time::SystemTime::now())
        };
        match touch(&busy) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = busy.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = touch(&busy);
            }
            _ => {}
        }
    }

    pub fn unused_for(&self) -> f64 {
        let busy = PathBuf::from(self.config.get("busy").map(crate::py::py_str).unwrap_or_default());
        match std::fs::metadata(&busy).and_then(|m| m.modified()) {
            Ok(mtime) => {
                let mtime = mtime.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
                crate::py::time_time() - mtime
            }
            Err(_) => f64::INFINITY,
        }
    }

    /// The work that used it ended `after_run` seconds ago or more, and nobody used it since.
    pub fn done_with(&self) -> bool {
        if self.after_run <= 0.0 {
            return false;
        }
        let Ok(ended) = std::fs::metadata(&self.ended_path).and_then(|m| m.modified()) else { return false };
        let ended = ended.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let now = crate::py::time_time();
        // `busy` touched after the end: used again (a call, the person watching or driving its desktop)
        now - ended >= self.after_run && self.unused_for() >= now - ended
    }

    pub async fn watch_idle(self: Arc<Self>, every: f64) {
        loop {
            tokio::time::sleep(Duration::from_secs_f64(every)).await;
            let tables = self.tcp_tables.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if self.calls.load(Ordering::SeqCst) != 0 || devtools_connected(self.config.get("cdp_port"), &tables) {
                if self.unused_for() > 30.0 {
                    self.touch();
                }
            } else if (self.unused_for() >= self.idle || self.done_with())
                && (self.running() || run(&self.command("active"), false).await.map(|c| c == 0).unwrap_or(false))
            {
                // Also when another started it (the desktop in the app, a `hermes chat` since closed).
                if self.unused_for() >= self.idle {
                    log(&format!("unused for {:.0} s: stopped", self.idle));
                } else {
                    log(&format!("its work over for {:.0} s: stopped", self.after_run));
                }
                self.stop().await;
            }
        }
    }

    pub async fn serve<R: AsyncRead + Unpin>(self: &Arc<Self>, reader: R) {
        let idle = (self.idle > 0.0).then(|| tokio::spawn(self.clone().watch_idle(15.0)));
        let mut reader = tokio::io::BufReader::new(reader);
        let mut tasks = Vec::new();
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let Ok(message) = serde_json::from_slice::<Value>(&buf) else { continue };
            let this = self.clone();
            tasks.push(tokio::spawn(async move { this.handle(message).await }));
            tasks.retain(|t| !t.is_finished());
        }
        if !tasks.is_empty() {
            // what it asked last, answered before leaving
            let _ = tokio::time::timeout(Duration::from_secs(30), futures::future::join_all(tasks)).await;
        }
        if let Some(idle) = idle {
            idle.abort();
        }
        // Hermes gone: its server too. The browser stays for whoever else uses it, until it is unused.
        self.stop_child().await;
    }
}

fn read_cache(path: &Path) -> Dict {
    match std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        Some(Value::Object(map)) => map,
        _ => Dict::new(),
    }
}

fn write_cache(path: &Path, cache: &Dict) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // a name of its own: two agents' proxies may write the shared one at once
    let partial = path.with_extension(format!("part{}", std::process::id()));
    std::fs::write(&partial, crate::py::dumps(&Value::Object(cache.clone())))?;
    std::fs::rename(&partial, path)
}

/// The container's systemd unit (and its Chrome profile folder), written if it is not there yet: an agent's browser
/// is set up at its first use, not with the agent. Returns whether it was written (`reload` is then due).
pub fn write_unit(config: &Dict) -> Result<bool, String> {
    let Some(unit) = config.get("unit").and_then(Value::as_object) else { return Ok(false) };
    let path = PathBuf::from(unit.get("path").map(crate::py::py_str).unwrap_or_default());
    if path.as_os_str().is_empty() || path.exists() {
        return Ok(false);
    }
    let text = unit.get("text").map(crate::py::py_str).unwrap_or_default();
    let written: std::io::Result<()> = (|| {
        if let Some(profile) = config.get("profile").map(crate::py::py_str).filter(|p| !p.is_empty()) {
            std::fs::create_dir_all(profile)?;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let partial = path.with_extension("part");
        std::fs::write(&partial, text)?;
        std::fs::rename(&partial, &path)
    })();
    written.map(|_| true).map_err(|e| format!("its container could not be set up: {e}"))
}

async fn read_server(proxy: Arc<Proxy>, server: Arc<Server>, stdout: tokio::process::ChildStdout) {
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(Value::Object(message)) = serde_json::from_slice::<Value>(&buf) else { continue };
        if message.contains_key("method") {
            proxy.to_client(&Value::Object(message)); // its notifications and requests go to Hermes as they are
            continue;
        }
        let id = message
            .get("id")
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)));
        if let Some(id) = id {
            let sender = proxy.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            if let Some(sender) = sender {
                let _ = sender.send(Ok(message));
            }
        }
    }
    server.wait().await;
    let pending: Vec<_> = proxy.pending.lock().unwrap_or_else(|e| e.into_inner()).drain().collect();
    for (_, sender) in pending {
        let _ = sender.send(Err("its server stopped".into()));
    }
}

/// `bip-browser-mcp <config.json>` (Python: `python3 browser_mcp.py <config.json>`): the process exit code.
pub fn main(args: Vec<String>) -> i32 {
    let Some(path) = args.first() else {
        eprintln!("usage: bip-browser-mcp <config.json>");
        return 1;
    };
    let config = match std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()))
    {
        Ok(Value::Object(map)) => map,
        Ok(_) => {
            eprintln!("{path}: not a JSON object");
            return 1;
        }
        Err(e) => {
            eprintln!("{path}: {e}");
            return 1;
        }
    };
    // Hermes starts MCP servers with few variables: `systemctl --user` and rootless podman find the session here.
    // SAFETY: getuid cannot fail.
    let runtime_dir = PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() }));
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() && runtime_dir.is_dir() {
        std::env::set_var("XDG_RUNTIME_DIR", &runtime_dir);
    }
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    runtime.block_on(async move {
        let proxy = Proxy::new(config, Box::new(std::io::stdout()));
        proxy.serve(tokio::io::stdin()).await;
    });
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devtools_connections_are_read_from_the_tcp_tables() {
        let dir = tempfile::tempdir().unwrap();
        let table = dir.path().join("tcp");
        let header =
            "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";
        // 127.0.0.1:47000 -> 127.0.0.1:19602 (0x4C92), established
        std::fs::write(
            &table,
            format!("{header}   0: 0100007F:B798 0100007F:4C92 01 00000000:00000000 00:00000000 00000000  1000 0 1\n"),
        )
        .unwrap();
        let tables = vec![table.to_string_lossy().into_owned()];
        assert!(devtools_connected(Some(&json!(19602)), &tables));
        assert!(!devtools_connected(Some(&json!(19604)), &tables));
        std::fs::write(
            &table,
            format!("{header}   0: 0100007F:B798 0100007F:4C92 06 00000000:00000000 00:00000000 00000000  1000 0 1\n"),
        )
        .unwrap();
        assert!(!devtools_connected(Some(&json!(19602)), &tables)); // TIME_WAIT: gone
        assert!(!devtools_connected(None, &tables));
    }
}
