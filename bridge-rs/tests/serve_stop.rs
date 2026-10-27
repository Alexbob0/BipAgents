//! `bipbridge serve` stops quickly when systemd or launchd asks (SIGTERM) or on Ctrl-C (SIGINT), even with an app's
//! event stream open: 2026-10-26, every `systemctl --user restart bipagents-bridge` waited systemd's 90 s.
//!
//! The binary itself runs here (a child process), with its own HOME and data folder.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const KEY: &str = "test-bridge-key-0123456789abcdef0123456789";

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// The bridge started on `port` with one agent whose Hermes nobody answers; returns once it answers /health (the
/// caller waits for it: `stop`).
#[allow(clippy::zombie_processes)]
fn start(dir: &Path, port: u16) -> Child {
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let config = dir.join("bridge.toml");
    std::fs::write(
        &config,
        format!(
            r#"bridge_key = "{KEY}"
data_dir = "{data}"
port = {port}

[ntfy]
url = "http://127.0.0.1:9"

[agents.Wellness]
display_name = "Wellness"
hermes_url = "http://127.0.0.1:9"
hermes_key = "hermes-wellness-key-xyz"
"#,
            data = dir.join("data").display(),
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_bipbridge"))
        .args(["serve", "--config"])
        .arg(&config)
        .env("HOME", &home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
            let _ = s.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
            let mut out = String::new();
            let _ = s.read_to_string(&mut out);
            if out.starts_with("HTTP/1.1 ") {
                return child;
            }
        }
        if started.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the bridge did not start");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A run's events, as the app follows it: the stream open, its headers read.
fn open_events(port: u16) -> TcpStream {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        s,
        "GET /v1/runs/run_abc123/events?agent=Wellness HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {KEY}\r\n\r\n"
    )
    .unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert_eq!(s.read(&mut byte).unwrap(), 1, "stream closed before its headers");
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).to_lowercase();
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(head.contains("text/event-stream"), "{head}");
    s
}

/// Sends `signal` and returns how long the process took to exit.
fn stop(child: &mut Child, signal: libc::c_int) -> Duration {
    let sent = Instant::now();
    // SAFETY: kill(2) on the child this test started.
    unsafe { libc::kill(child.id() as libc::pid_t, signal) };
    loop {
        if child.try_wait().unwrap().is_some() {
            return sent.elapsed();
        }
        if sent.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the bridge still runs 20 s after the signal");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn sigterm_stops_it_quickly_with_an_event_stream_open() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut child = start(dir.path(), port);
    let mut events = open_events(port);
    let took = stop(&mut child, libc::SIGTERM);
    assert!(took < Duration::from_secs(5), "stopped in {took:?}");
    assert!(child.wait().unwrap().success());
    // the app's stream ended (it comes back to the restarted bridge), not cut by a reset alone
    let mut rest = Vec::new();
    let _ = events.read_to_end(&mut rest);
}

#[test]
fn sigterm_stops_it_quickly_when_idle() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut child = start(dir.path(), port);
    let took = stop(&mut child, libc::SIGTERM);
    assert!(took < Duration::from_secs(5), "stopped in {took:?}");
    assert!(child.wait().unwrap().success());
}

#[test]
fn ctrl_c_stops_it_quickly_with_an_event_stream_open() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut child = start(dir.path(), port);
    let _events = open_events(port);
    let took = stop(&mut child, libc::SIGINT);
    assert!(took < Duration::from_secs(5), "stopped in {took:?}");
}
