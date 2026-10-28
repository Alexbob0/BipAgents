//! Shared test support (Python's tests/conftest.py): the config dict the tests start from, an app driven without
//! a socket, and fake backends (Hermes, Kyutai, ntfy, APNs…) as small axum servers on 127.0.0.1.
//!
//! Each test file declares `mod common;` and uses what it needs; add helpers here only when several groups need
//! them (other agents edit their own test files).
#![allow(dead_code)]

pub mod fakes;

use std::net::SocketAddr;
use std::path::Path;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::Router;
use bipbridge::app::{create_app, App, AppOptions};
use bipbridge::config::{parse_config, Config};
use serde_json::{json, Value};
use tower::ServiceExt;

pub const BRIDGE_KEY: &str = "test-bridge-key-0123456789abcdef0123456789";
pub const HERMES_KEY: &str = "hermes-wellness-key-xyz";
pub const NTFY_TOKEN: &str = "tk_testtoken";

/// `Authorization: Bearer <bridge key>`.
pub fn auth() -> (&'static str, String) {
    ("authorization", format!("Bearer {BRIDGE_KEY}"))
}

/// Python's `config_dict` fixture, with the backends' URLs given (fakes started by the test, or unreachable ones).
pub struct Urls {
    pub kyutai: String,
    pub pocket: String,
    pub ntfy: String,
    pub hermes: String,
}

impl Default for Urls {
    /// Nothing listens there (connections refused at once).
    fn default() -> Self {
        Urls {
            kyutai: "http://127.0.0.1:9".into(),
            pocket: "http://127.0.0.1:9".into(),
            ntfy: "http://127.0.0.1:9".into(),
            hermes: "http://127.0.0.1:9".into(),
        }
    }
}

/// A P-256 key in PKCS#8 PEM, as the `p8_path` fixture writes.
pub fn write_p8(dir: &Path) -> String {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("key");
    let path = dir.join("AuthKey_TESTKEY123.p8");
    std::fs::write(&path, key.serialize_pem()).expect("write p8");
    path.to_string_lossy().into_owned()
}

pub fn config_dict(dir: &Path, urls: &Urls) -> Value {
    let p8 = write_p8(dir);
    json!({
        "bridge_key": BRIDGE_KEY,
        "data_dir": dir.join("data").to_string_lossy(),
        "kyutai": {"url": urls.kyutai, "default_voice": "5476"},
        "pocket": {"url": urls.pocket},
        "ntfy": {"url": urls.ntfy},
        "apns": {"team_id": "TEAM123456", "key_id": "TESTKEY123", "p8_path": p8,
                 "bundle_id": "io.github.bipagents", "environment": "sandbox"},
        "outbox": {"audio_wait_seconds": 2},
        "limits": {"upload_max_mb": 1, "purge_interval_hours": 1000},
        // Never isolated by default: `auto` would isolate the sandboxes on Linux with podman (CI); sandbox tests say on.
        "sandbox": {"isolate": "off"},
        // The brake's counts are the process's (loop_brake.rs `TURNS`), shared by a file's tests running at once: a
        // test's many tool calls with no run followed would read as a routine going round in circles. Its own tests:
        // loop_brake.rs, runs.rs and the black-box suite (test_loop_brake.py).
        "brake": {"enabled": false},
        "agents": {
            "Wellness": {
                "display_name": "Wellness", "hermes_url": urls.hermes, "hermes_key": HERMES_KEY,
                "ntfy_topic": "hermes-wellness-out", "ntfy_token": NTFY_TOKEN,
                "upload_dir_host": dir.join("uploads").join("wellness").to_string_lossy(),
                "upload_dir_container": "/home/hermes/.hermes/profiles/wellness/uploads",
                "voice": "4193",
            },
        },
    })
}

pub fn config(dir: &Path, urls: &Urls) -> Config {
    parse_config(&config_dict(dir, urls), None).expect("valid test config")
}

/// A fake backend: `router` served on 127.0.0.1 (random port) until the test ends. Returns its base URL.
pub async fn spawn_fake(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await;
    });
    format!("http://{addr}")
}

/// The app without a socket (Python's TestClient): requests go straight to the router. The client address is
/// `peer` (default 198.51.100.7, counted by the guard as Python's « testclient » is).
pub struct TestApp {
    pub app: App,
    pub peer: SocketAddr,
}

pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl TestResponse {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&self.body)))
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
    }
}

impl TestApp {
    /// `create_app(config, start_background=False)`.
    pub fn new(config: Config) -> TestApp {
        Self::with_options(config, AppOptions { start_background: false })
    }

    pub fn with_options(config: Config, options: AppOptions) -> TestApp {
        let app = create_app(config, options).expect("app");
        TestApp { app, peer: "198.51.100.7:50000".parse().unwrap() }
    }

    pub async fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Body>,
    ) -> TestResponse {
        let mut builder = Request::builder().method(Method::from_bytes(method.as_bytes()).unwrap()).uri(path);
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        let mut request = builder.body(body.unwrap_or_else(Body::empty)).unwrap();
        request.extensions_mut().insert(ConnectInfo(self.peer));
        let response = self.app.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec();
        TestResponse { status, headers, body }
    }

    pub async fn get(&self, path: &str, headers: &[(&str, &str)]) -> TestResponse {
        self.request("GET", path, headers, None).await
    }

    /// A JSON body (`client.post(path, json=…)`).
    pub async fn send_json(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &Value) -> TestResponse {
        let mut all: Vec<(&str, &str)> = headers.to_vec();
        all.push(("content-type", "application/json"));
        self.request(method, path, &all, Some(Body::from(serde_json::to_vec(body).unwrap()))).await
    }

    pub async fn shutdown(self) {
        self.app.shutdown().await;
    }
}
