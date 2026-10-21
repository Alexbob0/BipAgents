//! A mailbox's calendars by CalDAV (caldav.rs): found from an address, tried with the mailbox's credentials, read and
//! written through the same calendar tools, against a small fake CalDAV server (canned PROPFIND/REPORT answers).

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::Response;
use axum::Router;
use base64::Engine;
use bipbridge::config::{parse_config, Config};
use bipbridge::{agent_tools, mailbox};
use common::{auth, config_dict, TestApp, Urls};
use indexmap::IndexMap;
use serde_json::{json, Value};

const TOKEN: &str = "caldav-agent-token";

#[derive(Default)]
struct Dav {
    requests: Vec<(String, String)>,
    events: BTreeMap<String, String>,
}

type Shared = Arc<Mutex<Dav>>;

fn multistatus(responses: &str) -> Response {
    Response::builder()
        .status(207)
        .header("content-type", "application/xml; charset=utf-8")
        .body(Body::from(format!(r#"<?xml version="1.0" encoding="utf-8"?><d:multistatus xmlns:d="DAV:" xmlns:cal="urn:ietf:params:xml:ns:caldav" xmlns:ic="http://apple.com/ns/ical/">{responses}</d:multistatus>"#)))
        .unwrap()
}

fn status(code: u16) -> Response {
    Response::builder().status(code).body(Body::empty()).unwrap()
}

async fn serve(State(dav): State<Shared>, method: Method, uri: Uri, headers: HeaderMap, body: String) -> Response {
    let path = uri.path().to_string();
    dav.lock().unwrap().requests.push((method.to_string(), path.clone()));
    let expected = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode("alex@exemple.org:app-pass"));
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(expected.as_str()) {
        return status(401);
    }
    match (method.as_str(), path.as_str()) {
        ("PROPFIND", "/.well-known/caldav") => {
            Response::builder().status(StatusCode::MOVED_PERMANENTLY).header("location", "/dav/").body(Body::empty()).unwrap()
        }
        ("PROPFIND", "/dav/") => multistatus(
            "<d:response><d:href>/dav/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/dav/principals/alex/</d:href></d:current-user-principal></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>",
        ),
        ("PROPFIND", "/dav/principals/alex/") => multistatus(
            "<d:response><d:href>/dav/principals/alex/</d:href><d:propstat><d:prop><cal:calendar-home-set><d:href>/dav/calendars/alex/</d:href></cal:calendar-home-set></d:prop></d:propstat></d:response>",
        ),
        ("PROPFIND", "/dav/calendars/alex/") => multistatus(concat!(
            "<d:response><d:href>/dav/calendars/alex/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response>",
            "<d:response><d:href>/dav/calendars/alex/work/</d:href><d:propstat><d:prop><d:displayname>Travail</d:displayname>",
            "<d:resourcetype><d:collection/><cal:calendar/></d:resourcetype><ic:calendar-color>#E8574AFF</ic:calendar-color>",
            "<cal:supported-calendar-component-set><cal:comp name=\"VEVENT\"/></cal:supported-calendar-component-set></d:prop></d:propstat></d:response>",
            "<d:response><d:href>/dav/calendars/alex/tasks/</d:href><d:propstat><d:prop><d:displayname>Tâches</d:displayname>",
            "<d:resourcetype><d:collection/><cal:calendar/></d:resourcetype>",
            "<cal:supported-calendar-component-set><cal:comp name=\"VTODO\"/></cal:supported-calendar-component-set></d:prop></d:propstat></d:response>",
        )),
        ("REPORT", "/dav/calendars/alex/work/") => {
            assert!(body.contains("calendar-query") && body.contains("time-range"), "{body}");
            let responses: String = dav
                .lock()
                .unwrap()
                .events
                .iter()
                .map(|(href, ics)| {
                    format!(
                        "<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:getetag>\"1\"</d:getetag><cal:calendar-data>{}</cal:calendar-data></d:prop></d:propstat></d:response>",
                        bipbridge::caldav::xml_escape(ics)
                    )
                })
                .collect();
            multistatus(&responses)
        }
        ("PUT", p) if p.starts_with("/dav/calendars/alex/work/") => {
            dav.lock().unwrap().events.insert(p.to_string(), body);
            status(201)
        }
        ("GET", p) => match dav.lock().unwrap().events.get(p) {
            Some(ics) => Response::builder().status(200).body(Body::from(ics.clone())).unwrap(),
            None => status(404),
        },
        ("DELETE", p) => match dav.lock().unwrap().events.remove(p) {
            Some(_) => status(204),
            None => status(404),
        },
        _ => status(405),
    }
}

async fn fake_dav() -> (String, Shared) {
    let dav: Shared = Arc::default();
    dav.lock().unwrap().events.insert(
        "/dav/calendars/alex/work/r1.ics".into(),
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:r1\r\nDTSTART:20261023T120000Z\r\nDTEND:20261023T130000Z\r\n\
SUMMARY:Réunion client\r\nLOCATION:Salle 2\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
            .into(),
    );
    dav.lock().unwrap().events.insert(
        "/dav/calendars/alex/work/w1.ics".into(),
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:w1\r\nRECURRENCE-ID:20261026T080000Z\r\nDTSTART:20261026T080000Z\r\n\
DURATION:PT30M\r\nSUMMARY:Point hebdo\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
            .into(),
    );
    let router = Router::new().fallback(serve).with_state(dav.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, dav)
}

fn with_mailboxes(dir: &std::path::Path) -> Config {
    let mut dict = config_dict(dir, &Urls::default());
    dict["hermes"] = json!({"bin": "/opt/hermes", "home": dir.join(".hermes").to_string_lossy(), "hub_url": "http://127.0.0.1:8642"});
    dict["agents"]["coach"] =
        json!({"display_name": "Coach", "hermes_url": "http://127.0.0.1:8642/p/coach", "hermes_key": "k"});
    let config = parse_config(&dict, None).unwrap();
    let mut tokens = IndexMap::new();
    tokens.insert("coach".to_string(), agent_tools::digest(TOKEN));
    agent_tools::save_tokens(&config, &tokens).unwrap();
    let account = |id: &str, label: &str, username: &str, password: &str, host: &str| {
        json!({"id": id, "provider": "imap", "host": host, "port": 993, "starttls": false, "username": username,
               "password": password, "label": label, "agents": ["coach"], "created": 0.0, "last_ok": null,
               "last_error": null})
        .as_object()
        .unwrap()
        .clone()
    };
    mailbox::of(&config)
        .write(&[
            account("m1", "Mail perso", "alex@exemple.org", "app-pass", "imap.exemple.org"),
            account("m2", "Autre", "alex@exemple.org", "wrong", "imap.exemple.org"),
            account("m3", "Gmail", "alex@gmail.com", "x", "imap.gmail.com"),
        ])
        .unwrap();
    config
}

async fn tool(app: &TestApp, name: &str, arguments: Value) -> (bool, String) {
    let body =
        json!({"jsonrpc": "2.0", "method": "tools/call", "params": {"name": name, "arguments": arguments}, "id": 1});
    let bearer = format!("Bearer {TOKEN}");
    let answer = app
        .send_json("POST", "/v1/agent-tools/mcp", &[("authorization", &bearer), ("accept", "application/json")], &body)
        .await
        .json();
    let result = &answer["result"];
    (result["isError"] == true, result["content"][0]["text"].as_str().unwrap_or("").to_string())
}

async fn calendar_tools(app: &TestApp) -> usize {
    let body = json!({"jsonrpc": "2.0", "method": "tools/list", "params": {}, "id": 1});
    let bearer = format!("Bearer {TOKEN}");
    let answer = app
        .send_json("POST", "/v1/agent-tools/mcp", &[("authorization", &bearer), ("accept", "application/json")], &body)
        .await
        .json();
    answer["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["name"].as_str().unwrap().starts_with("calendar_"))
        .count()
}

#[tokio::test]
async fn a_mailboxs_calendar_found_read_and_written() {
    let (dav_url, dav) = fake_dav().await;
    let dir = tempfile::tempdir().unwrap();
    let config = with_mailboxes(dir.path());
    let app = TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(calendar_tools(&app).await, 0);
    let listed = app.get("/v1/mail/accounts", &h).await.json();
    assert!(listed["accounts"].as_array().unwrap().iter().all(|a| a.get("calendar").is_none()));

    // Google and Microsoft: not this way. A plain http address elsewhere: refused. A wrong password: said.
    let gmail = app.send_json("PATCH", "/v1/mail/accounts/m3", &h, &json!({"calendar": true})).await;
    assert_eq!(gmail.status, 400);
    assert!(gmail.json()["detail"].as_str().unwrap().contains("share the phone's calendar"));
    let plain = app
        .send_json(
            "PATCH",
            "/v1/mail/accounts/m1",
            &h,
            &json!({"calendar": true, "calendar_url": "http://dav.exemple.org/"}),
        )
        .await;
    assert_eq!(plain.status, 400);
    let wrong = app
        .send_json(
            "PATCH",
            "/v1/mail/accounts/m2",
            &h,
            &json!({"calendar": true, "calendar_url": format!("{dav_url}/dav/")}),
        )
        .await;
    assert_eq!(wrong.status, 400);
    assert!(wrong.json()["detail"].as_str().unwrap().starts_with("login refused"));

    // Found from the well-known address (a redirect followed), its event calendars kept.
    let on = app
        .send_json(
            "PATCH",
            "/v1/mail/accounts/m1",
            &h,
            &json!({"calendar": true, "calendar_url": format!("{dav_url}/.well-known/caldav")}),
        )
        .await;
    assert_eq!(on.status, 200, "{}", on.text());
    assert_eq!(on.json()["calendar"], json!({"calendars": ["Travail"]}));
    assert!(!on.text().contains("app-pass"));
    assert_eq!(calendar_tools(&app).await, 4);

    let (failed, text) = tool(&app, "calendar_list", json!({"from": "2026-10-21", "to": "2026-10-31"})).await;
    assert!(!failed, "{text}");
    assert!(text.contains("read now from Mail perso") && !text.contains("phone"), "{text}");
    assert!(
        text.contains(" · Travail (Mail perso) · 2026-10-23 ") && text.contains("Réunion client · at Salle 2"),
        "{text}"
    );
    assert!(text.contains("Point hebdo · repeats"), "{text}");
    let id = |title: &str| {
        text.lines()
            .find(|l| l.contains(title))
            .unwrap()
            .split(" · ")
            .next()
            .unwrap()
            .trim_start_matches("- id ")
            .to_string()
    };
    let (single, weekly) = (id("Réunion client"), id("Point hebdo"));
    assert!(single.starts_with("mail:m1:http://127.0.0.1:"), "{single}");

    let (failed, added) = tool(
        &app,
        "calendar_add",
        json!({"title": "Banque", "start": "2026-10-30T10:00:00+01:00",
                                                           "duration_minutes": 30, "calendar": "travail",
                                                           "location": "Agence; centre"}),
    )
    .await;
    assert!(!failed && added.contains("Added to « Travail » (Mail perso)"), "{added}");
    let (failed, _) =
        tool(&app, "calendar_add", json!({"title": "Congés", "start": "2026-12-21", "end": "2026-12-24"})).await;
    assert!(!failed); // no phone copy: the mailbox's first calendar
    {
        let dav = dav.lock().unwrap();
        let new: Vec<&String> =
            dav.events.iter().filter(|(k, _)| k.ends_with("@bipagents.ics")).map(|(_, v)| v).collect();
        assert_eq!(new.len(), 2);
        let bank = new.iter().find(|i| i.contains("SUMMARY:Banque")).unwrap();
        assert!(
            bank.contains("DTSTART:20261030T090000Z\r\nDTEND:20261030T093000Z")
                && bank.contains("LOCATION:Agence\\; centre"),
            "{bank}"
        );
        assert!(new.iter().any(|i| i.contains("DTSTART;VALUE=DATE:20261221\r\nDTEND;VALUE=DATE:20261225")));
    }
    assert!(tool(&app, "calendar_add", json!({"title": "x", "start": "bientôt", "calendar": "Travail"})).await.0);

    let (failed, refused) = tool(&app, "calendar_delete", json!({"id": weekly})).await;
    assert!(failed && refused.contains("repeating"), "{refused}");
    assert!(tool(&app, "calendar_update", json!({"id": single, "title": "x"})).await.0);
    let (failed, deleted) = tool(&app, "calendar_delete", json!({"id": single})).await;
    assert!(!failed && deleted.contains("Deleted from Mail perso"), "{deleted}");
    assert!(!dav.lock().unwrap().events.contains_key("/dav/calendars/alex/work/r1.ics"));
    assert!(tool(&app, "calendar_delete", json!({"id": "mail:m1:http://elsewhere.example/x.ics"})).await.0);

    // Off: its calendars forgotten, the tools gone.
    let off = app.send_json("PATCH", "/v1/mail/accounts/m1", &h, &json!({"calendar": false})).await;
    assert!(off.json().get("calendar").is_none());
    assert_eq!(calendar_tools(&app).await, 0);
    assert!(tool(&app, "calendar_list", json!({})).await.1.contains("hasn't shared"));
}
