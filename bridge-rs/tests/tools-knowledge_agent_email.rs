//! An agent's own e-mail address (agent_email.rs, agent_email_smtp.rs): turned on with a mailbox of the person's or
//! one asked of the guichet, its gateway's settings in the profile, and the guard Hermes sends through: allowed
//! addresses only, new ones held for the person, a daily limit. Fake SMTP server and guichet on 127.0.0.1.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::Path as UrlPath;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::put;
use axum::{Json, Router};
use base64::Engine;
use bipbridge::agent_email::{self, Deps};
use bipbridge::agent_email_smtp::{self, Guard};
use bipbridge::agents_admin::Run;
use bipbridge::app::AppState;
use bipbridge::config::{parse_config, Config};
use common::{config_dict, spawn_fake, TestApp, Urls};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tower::ServiceExt;

type Ran = Arc<Mutex<Vec<(Vec<String>, BTreeMap<String, String>)>>>;

fn hub_config(dir: &Path) -> Config {
    let mut d = config_dict(dir, &Urls::default());
    d["hermes"] = json!({"bin": "/opt/hermes", "home": dir.join(".hermes").to_string_lossy(),
                         "hub_url": "http://127.0.0.1:8642"});
    d["agents"]["lea"] = json!({"display_name": "Léa", "hermes_url": "http://127.0.0.1:8642/p/lea", "hermes_key": "k"});
    std::fs::create_dir_all(dir.join(".hermes/profiles/lea")).unwrap();
    std::fs::write(dir.join(".hermes/profiles/lea/.env"), "API_SERVER_KEY=k\n").unwrap();
    parse_config(&d, None).unwrap()
}

fn deps(home: &Path, ran: &Ran) -> Deps {
    let ran = ran.clone();
    let run: Run = Arc::new(move |args, env| {
        ran.lock().unwrap().push((args, env));
        Box::pin(async { Ok(String::new()) })
    });
    Deps { run, home: home.to_path_buf(), http: reqwest::Client::new(), guard_port: 25_999 }
}

async fn send(state: &AppState, deps: &Deps, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let router: Router = Router::new().nest("/v1", agent_email::router_with(deps.clone())).with_state(state.clone());
    let mut builder = Request::builder().method(method).uri(path);
    let body = match body {
        Some(b) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&b).unwrap())
        }
        None => Body::empty(),
    };
    let response = router.oneshot(builder.body(body).unwrap()).await.unwrap();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// A plain SMTP server that takes everything: (login, from, rcpts, message) for each mail.
type Received = Arc<Mutex<Vec<(String, String, Vec<String>, String)>>>;

async fn fake_smtp() -> (u16, Received) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let got: Received = Default::default();
    let kept = got.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let got = kept.clone();
            tokio::spawn(async move {
                let (r, mut w) = stream.into_split();
                let mut r = BufReader::new(r);
                w.write_all(b"220 fake ESMTP\r\n").await.unwrap();
                let (mut login, mut from, mut rcpts) = (String::new(), String::new(), Vec::new());
                let mut line = String::new();
                loop {
                    line.clear();
                    if r.read_line(&mut line).await.unwrap() == 0 {
                        return;
                    }
                    let l = line.trim_end().to_string();
                    let out: &[u8] = if l.starts_with("EHLO") {
                        b"250-fake\r\n250 AUTH PLAIN\r\n"
                    } else if let Some(b) = l.strip_prefix("AUTH PLAIN ") {
                        let d = base64::engine::general_purpose::STANDARD.decode(b).unwrap();
                        login = String::from_utf8(d).unwrap().replace('\0', "|");
                        b"235 ok\r\n"
                    } else if let Some(f) = l.strip_prefix("MAIL FROM:") {
                        from = f.to_string();
                        b"250 ok\r\n"
                    } else if let Some(t) = l.strip_prefix("RCPT TO:") {
                        rcpts.push(t.to_string());
                        b"250 ok\r\n"
                    } else if l == "DATA" {
                        w.write_all(b"354 go\r\n").await.unwrap();
                        let mut message = String::new();
                        loop {
                            line.clear();
                            r.read_line(&mut line).await.unwrap();
                            if line == ".\r\n" {
                                break;
                            }
                            message.push_str(&line);
                        }
                        got.lock().unwrap().push((login.clone(), from.clone(), rcpts.clone(), message));
                        rcpts.clear();
                        b"250 queued\r\n"
                    } else if l == "QUIT" {
                        w.write_all(b"221 bye\r\n").await.unwrap();
                        return;
                    } else {
                        b"500 ?\r\n"
                    };
                    w.write_all(out).await.unwrap();
                }
            });
        }
    });
    (port, got)
}

/// One SMTP conversation with the guard: each line sent, its answer's last line read.
struct Client {
    r: BufReader<tokio::net::tcp::OwnedReadHalf>,
    w: tokio::net::tcp::OwnedWriteHalf,
}

impl Client {
    async fn open(port: u16) -> Client {
        let (r, w) = TcpStream::connect(("127.0.0.1", port)).await.unwrap().into_split();
        let mut c = Client { r: BufReader::new(r), w };
        assert!(c.answer().await.starts_with("220"));
        c
    }

    async fn answer(&mut self) -> String {
        loop {
            let mut line = String::new();
            self.r.read_line(&mut line).await.unwrap();
            if line.as_bytes().get(3) != Some(&b'-') {
                return line.trim_end().to_string();
            }
        }
    }

    async fn say(&mut self, line: &str) -> String {
        self.w.write_all(format!("{line}\r\n").as_bytes()).await.unwrap();
        self.answer().await
    }

    async fn login(&mut self, user: &str, password: &str) -> String {
        self.say("EHLO hermes").await;
        let plain = base64::engine::general_purpose::STANDARD.encode(format!("\0{user}\0{password}"));
        self.say(&format!("AUTH PLAIN {plain}")).await
    }

    async fn mail(&mut self, from: &str, to: &[&str], subject: &str) -> String {
        let a = self.say(&format!("MAIL FROM:<{from}>")).await;
        if !a.starts_with("250") {
            return a;
        }
        for t in to {
            assert!(self.say(&format!("RCPT TO:<{t}>")).await.starts_with("250"));
        }
        assert!(self.say("DATA").await.starts_with("354"));
        self.say(&format!("From: {from}\r\nTo: {}\r\nSubject: {subject}\r\n\r\n.Hello\r\n.", to.join(", "))).await
    }
}

#[tokio::test]
async fn a_mailbox_of_the_person_s_and_the_guard_hermes_sends_through() {
    let dir = tempfile::tempdir().unwrap();
    let config = hub_config(dir.path());
    let app = TestApp::new(config.clone());
    let state = &app.app.state;
    let ran: Ran = Default::default();
    let deps = deps(dir.path(), &ran);
    let (smtp_port, received) = fake_smtp().await;

    let (status, view) = send(state, &deps, "GET", "/v1/agents/lea/email", None).await;
    assert_eq!((status, &view["enabled"], &view["offered"]), (200, &json!(false), &json!(false)));
    assert_eq!(send(state, &deps, "GET", "/v1/agents/nobody/email", None).await.0, 404);

    let own = json!({"kind": "own", "address": "Lea@Own.test", "password": "app-pw", "imap_host": "imap.own.test",
                     "smtp_host": "127.0.0.1", "smtp_port": smtp_port, "smtp_security": "plain",
                     "authserv_id": "mx.own.test", "allowed": ["Alex@Example.com"]});
    let (status, view) = send(state, &deps, "POST", "/v1/agents/lea/email", Some(own.clone())).await;
    assert_eq!(status, 201, "{view}");
    assert_eq!(view["address"], "lea@own.test");
    assert_eq!(view["allowed"], json!(["alex@example.com"]));
    assert_eq!(view["daily_limit"], 20);
    assert!(view.get("password").is_none());
    assert_eq!(send(state, &deps, "POST", "/v1/agents/lea/email", Some(own)).await.0, 409);
    let env = std::fs::read_to_string(dir.path().join(".hermes/profiles/lea/.env")).unwrap();
    for line in [
        "API_SERVER_KEY=k",
        "EMAIL_ADDRESS=lea@own.test",
        "EMAIL_PASSWORD=app-pw",
        "EMAIL_IMAP_HOST=imap.own.test",
        "EMAIL_SMTP_HOST=127.0.0.1",
        "EMAIL_SMTP_PORT=25999",
        "EMAIL_SMTP_SECURITY=plain",
        "EMAIL_ALLOWED_USERS=alex@example.com",
        "EMAIL_AUTHSERV_ID=mx.own.test",
    ] {
        assert!(env.lines().any(|l| l == line), "{line} in {env}");
    }
    assert_eq!(ran.lock().unwrap()[0].0, ["/opt/hermes", "gateway", "restart"]); // the hub's gateway

    // the guard
    let notified: Arc<Mutex<Vec<(String, String, String)>>> = Default::default();
    let told = notified.clone();
    let guard = Guard {
        folder: agent_email::folder(&config),
        notify: Arc::new(move |a, t, i| told.lock().unwrap().push((a, t, i))),
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(agent_email_smtp::run(guard, listener));

    let mut c = Client::open(port).await;
    assert!(c.login("lea@own.test", "wrong").await.starts_with("535"));
    assert!(c.say("MAIL FROM:<lea@own.test>").await.starts_with("530"));
    assert!(c.login("lea@own.test", "app-pw").await.starts_with("235"));
    assert!(c.mail("someone@else.test", &["alex@example.com"], "x").await.starts_with("553"));
    let sent = c.mail("lea@own.test", &["alex@example.com"], "Coucou").await;
    assert!(sent.starts_with("250"), "{sent}");
    {
        let got = received.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "|lea@own.test|app-pw");
        assert_eq!(got[0].2, ["<alex@example.com>"]);
        assert!(got[0].3.contains("Subject: Coucou\r\n") && got[0].3.contains("\r\n..Hello\r\n"));
        // dots kept stuffed
    }

    // to someone new: held, the person told, nothing sent
    let held = c.mail("lea@own.test", &["alex@example.com", "new@x.test"], "Réunion").await;
    assert!(held.starts_with("550 5.7.1 Not sent yet: new@x.test"), "{held}");
    assert_eq!(received.lock().unwrap().len(), 1);
    let (_, view) = send(state, &deps, "GET", "/v1/agents/lea/email", None).await;
    assert_eq!(view["held"].as_array().unwrap().len(), 1);
    assert_eq!(view["held"][0]["subject"], "Réunion");
    assert_eq!(view["sent_today"], 1);
    let id = view["held"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(notified.lock().unwrap()[0], ("lea".into(), "new@x.test".into(), id.clone()));

    // the person sends it: it goes, and that address is allowed from now on
    let (status, view) =
        send(state, &deps, "POST", &format!("/v1/agents/lea/email/held/{id}"), Some(json!({"send": true}))).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["allowed"], json!(["alex@example.com", "new@x.test"]));
    assert_eq!(view["held"], json!([]));
    assert_eq!(view["sent_today"], 2);
    assert_eq!(received.lock().unwrap()[1].2, ["<alex@example.com>", "<new@x.test>"]);
    let env = std::fs::read_to_string(dir.path().join(".hermes/profiles/lea/.env")).unwrap();
    assert!(env.contains("EMAIL_ALLOWED_USERS=alex@example.com,new@x.test"));
    assert_eq!(send(state, &deps, "POST", "/v1/agents/lea/email/held/nope", Some(json!({"send": true}))).await.0, 404);

    // the daily limit
    let (status, view) = send(state, &deps, "PATCH", "/v1/agents/lea/email", Some(json!({"daily_limit": 2}))).await;
    assert_eq!((status, &view["daily_limit"]), (200, &json!(2)));
    assert_eq!(send(state, &deps, "PATCH", "/v1/agents/lea/email", Some(json!({"daily_limit": 500}))).await.0, 422);
    assert_eq!(send(state, &deps, "PATCH", "/v1/agents/lea/email", Some(json!({"allowed": ["nope"]}))).await.0, 400);
    let over = c.mail("lea@own.test", &["alex@example.com"], "encore").await;
    assert!(over.starts_with("550 5.7.1 Not sent: the daily limit of 2"), "{over}");
    assert_eq!(received.lock().unwrap().len(), 2);
    c.say("QUIT").await;

    // a held mail refused is dropped
    let mut c = Client::open(port).await;
    c.login("lea@own.test", "app-pw").await;
    assert!(c.mail("lea@own.test", &["other@x.test"], "y").await.starts_with("550"));
    let (_, view) = send(state, &deps, "GET", "/v1/agents/lea/email", None).await;
    let id = view["held"][0]["id"].as_str().unwrap().to_string();
    let (_, view) =
        send(state, &deps, "POST", &format!("/v1/agents/lea/email/held/{id}"), Some(json!({"send": false}))).await;
    assert_eq!(view["held"], json!([]));
    assert!(!view["allowed"].as_array().unwrap().contains(&json!("other@x.test")));

    // turned off: the profile's settings gone, the guard refuses its login
    let (status, _) = send(state, &deps, "DELETE", "/v1/agents/lea/email", None).await;
    assert_eq!(status, 200);
    assert_eq!(std::fs::read_to_string(dir.path().join(".hermes/profiles/lea/.env")).unwrap(), "API_SERVER_KEY=k\n");
    let mut c = Client::open(port).await;
    assert!(c.login("lea@own.test", "app-pw").await.starts_with("535"));
}

#[tokio::test]
async fn a_kerfy_studio_address_asked_of_the_guichet_with_the_place_s_token() {
    let dir = tempfile::tempdir().unwrap();
    let config = hub_config(dir.path());
    let app = TestApp::new(config.clone());
    let state = &app.app.state;
    let ran: Ran = Default::default();
    let deps = deps(dir.path(), &ran);

    // not a place of the offer
    let (status, out) = send(state, &deps, "POST", "/v1/agents/lea/email", Some(json!({"kind": "bipagents"}))).await;
    assert_eq!(status, 503, "{out}");

    let asked: Arc<Mutex<Vec<(String, String, Value)>>> = Default::default();
    let seen = asked.clone();
    let seen2 = asked.clone();
    let guichet = Router::new().route(
        "/v1/mailboxes/{agent}",
        put(move |UrlPath(agent): UrlPath<String>, headers: HeaderMap, Json(body): Json<Value>| {
            let seen = seen.clone();
            async move {
                let auth = headers.get("authorization").unwrap().to_str().unwrap().to_string();
                seen.lock().unwrap().push((format!("PUT {agent}"), auth, body));
                Json(json!({"address": "lea.k7m2@a.bipagents.test", "password": "given-once",
                            "imap": {"host": "mail.bipagents.test", "port": 993, "security": "tls"},
                            "smtp": {"host": "mail.bipagents.test", "port": 465, "security": "tls"},
                            "authserv_id": "mail.bipagents.test", "retention_days": 90}))
            }
        })
        .delete(move |UrlPath(agent): UrlPath<String>, headers: HeaderMap| {
            let seen = seen2.clone();
            async move {
                let auth = headers.get("authorization").unwrap().to_str().unwrap().to_string();
                seen.lock().unwrap().push((format!("DELETE {agent}"), auth, Value::Null));
                Json(json!({"ok": true}))
            }
        }),
    );
    let url = spawn_fake(guichet).await;
    let settings = dir.path().join(".config/bipagents/backup.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(
        &settings,
        json!({"url": format!("{url}/v1/backup/"), "token": "place-token", "password": "p"}).to_string(),
    )
    .unwrap();

    let (_, view) = send(state, &deps, "GET", "/v1/agents/lea/email", None).await;
    assert_eq!(view["offered"], true);
    let (status, view) = send(
        state,
        &deps,
        "POST",
        "/v1/agents/lea/email",
        Some(json!({"kind": "bipagents", "allowed": ["alex@example.com"]})),
    )
    .await;
    assert_eq!(status, 201, "{view}");
    assert_eq!(view["address"], "lea.k7m2@a.bipagents.test");
    assert_eq!(view["retention_days"], 90);
    assert_eq!(view["last_error"], Value::Null);
    assert_eq!(asked.lock().unwrap()[0], ("PUT lea".into(), "Bearer place-token".into(), json!({"name": "Léa"})));
    let env = std::fs::read_to_string(dir.path().join(".hermes/profiles/lea/.env")).unwrap();
    assert!(env.contains("EMAIL_PASSWORD=given-once\n") && env.contains("EMAIL_AUTHSERV_ID=mail.bipagents.test\n"));
    assert!(env.contains("EMAIL_IMAP_HOST=mail.bipagents.test\n") && env.contains("EMAIL_SMTP_HOST=127.0.0.1\n"));

    let (status, _) = send(state, &deps, "DELETE", "/v1/agents/lea/email", None).await;
    assert_eq!(status, 200);
    assert_eq!(asked.lock().unwrap()[1].0, "DELETE lea");
    assert!(agent_email::load(&agent_email::folder(&config), "lea").is_none());
}

/// The agent's address exists at the guichet but this place lost its password (an old restore): refused as « can't
/// be taken back here » and nothing deleted, until the person starts again (`start_again`, their OK in the app): the
/// old address deleted at the guichet, a new one asked and kept. Turning off with no record here deletes it there too.
#[tokio::test]
async fn an_address_whose_password_was_lost_starts_again_only_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    let config = hub_config(dir.path());
    let app = TestApp::new(config.clone());
    let state = &app.app.state;
    let ran: Ran = Default::default();
    let deps = deps(dir.path(), &ran);

    // the guichet: lea's address made, its password not on this place
    let made: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(Some("lea.old1@a.bipagents.test".into())));
    let calls: Arc<Mutex<Vec<String>>> = Default::default();
    let (m1, m2, c1, c2) = (made.clone(), made.clone(), calls.clone(), calls.clone());
    let guichet = Router::new().route(
        "/v1/mailboxes/{agent}",
        put(move |Json(body): Json<Value>| {
            let (made, calls) = (m1.clone(), c1.clone());
            async move {
                calls.lock().unwrap().push(format!("PUT {}", body.get("current").map_or("", |_| "current")));
                let mut made = made.lock().unwrap();
                if made.is_some() {
                    let refused = json!({"detail": "this address exists: its current password is needed"});
                    return (StatusCode::FORBIDDEN, Json(refused));
                }
                *made = Some("lea.new2@a.bipagents.test".into());
                (
                    StatusCode::OK,
                    Json(json!({"address": "lea.new2@a.bipagents.test", "password": "new-once",
                                "imap": {"host": "mail.bipagents.test", "port": 993, "security": "tls"},
                                "smtp": {"host": "mail.bipagents.test", "port": 465, "security": "tls"},
                                "authserv_id": "mail.bipagents.test", "retention_days": 90})),
                )
            }
        })
        .delete(move |headers: HeaderMap| {
            let (made, calls) = (m2.clone(), c2.clone());
            async move {
                assert_eq!(headers.get("authorization").unwrap(), "Bearer place-token");
                calls.lock().unwrap().push("DELETE".into());
                match made.lock().unwrap().take() {
                    Some(_) => (StatusCode::OK, Json(json!({"ok": true}))),
                    None => (StatusCode::NOT_FOUND, Json(json!({"detail": "no such address"}))),
                }
            }
        }),
    );
    let url = spawn_fake(guichet).await;
    let settings = dir.path().join(".config/bipagents/backup.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(&settings, json!({"url": format!("{url}/v1/backup"), "token": "place-token"}).to_string()).unwrap();

    // asked as usual: refused, and nothing deleted
    let ask = json!({"kind": "bipagents", "allowed": ["alex@example.com"]});
    let (status, out) = send(state, &deps, "POST", "/v1/agents/lea/email", Some(ask.clone())).await;
    assert_eq!(status, 409, "{out}");
    assert!(out["detail"].as_str().unwrap().contains("can't be taken back here"), "{out}");
    assert_eq!(*calls.lock().unwrap(), vec!["PUT ".to_string()]);
    assert!(made.lock().unwrap().is_some());
    let folder = agent_email::folder(&config);
    assert!(agent_email::load(&folder, "lea").is_none());

    // the person starts again: deleted, then a new address kept like any new one
    let mut again = ask.clone();
    again["start_again"] = json!(true);
    let (status, view) = send(state, &deps, "POST", "/v1/agents/lea/email", Some(again)).await;
    assert_eq!(status, 201, "{view}");
    assert_eq!(view["address"], "lea.new2@a.bipagents.test");
    assert_eq!(*calls.lock().unwrap(), vec!["PUT ".to_string(), "PUT ".into(), "DELETE".into(), "PUT ".into()]);
    assert_eq!(agent_email::current_password(&folder, "lea").as_deref(), Some("new-once"));
    let env = std::fs::read_to_string(dir.path().join(".hermes/profiles/lea/.env")).unwrap();
    assert!(env.contains("EMAIL_PASSWORD=new-once\n") && env.contains("EMAIL_ADDRESS=lea.new2@a.bipagents.test\n"));

    // an old restore again: no record here, the address still at the guichet: turning off deletes it there
    std::fs::remove_file(folder.join("lea.json")).unwrap();
    calls.lock().unwrap().clear();
    let (status, out) = send(state, &deps, "DELETE", "/v1/agents/lea/email", None).await;
    assert_eq!(status, 200, "{out}");
    assert_eq!(*calls.lock().unwrap(), vec!["DELETE".to_string()]);
    assert!(made.lock().unwrap().is_none());
    assert!(agent_email::current_password(&folder, "lea").is_none());

    // not a place of the offer, no record: nothing to turn off
    std::fs::remove_file(&settings).unwrap();
    assert_eq!(send(state, &deps, "DELETE", "/v1/agents/lea/email", None).await.0, 404);
}
