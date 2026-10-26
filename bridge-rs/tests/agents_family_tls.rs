//! Two households whose bridges are reached at a door (a place of the offer: an IP address, a self-signed
//! certificate the apps pin): the bridges pin each other's certificate too (door_tls.rs). Before, every call between
//! them went through the default client (WebPKI) and failed: `/family/accept` at the join, then `/family/agents`,
//! `/family/tell`, `/family/r/…` and `/family/shared`.

mod common;

use std::path::Path;

use bipbridge::config::parse_config;
use bipbridge::{door_tls, family_news};
use common::{auth, config_dict, TestApp, Urls};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A bridge on its door (HTTPS, its own self-signed certificate) when `door`, else on plain HTTP (a stand-in for a
/// self-hosted install behind `tailscale serve`). Returns its address, its certificate's fingerprint, the app.
async fn serve_bridge(dir: &Path, agents: &[&str], door: bool) -> (String, Option<String>, TestApp) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut d = config_dict(dir, &Urls::default());
    d["outbox"] = json!({"db_path": s(&dir.join("bridge.db"))});
    let url = if door {
        d["lan"] = json!({"enabled": true, "address": "127.0.0.1", "port": port});
        format!("https://127.0.0.1:{port}")
    } else {
        format!("http://127.0.0.1:{port}")
    };
    d["public_url"] = json!(url);
    let mut table = serde_json::Map::new();
    for a in agents {
        let title = format!("{}{}", a[..1].to_uppercase(), &a[1..]);
        table.insert(
            title,
            json!({"hermes_url": format!("http://127.0.0.1:9/{a}"), "hermes_key": format!("{a}-key-{}", "x".repeat(24))}),
        );
    }
    d["agents"] = Value::Object(table);
    let config = parse_config(&d, None).unwrap();
    let mut print = None;
    let app = if door {
        let (cert, key) = bipbridge::lan::ensure_certificate(&config).unwrap();
        print = bipbridge::lan::fingerprint(&config);
        let app =
            bipbridge::app::create_app(config, bipbridge::app::AppOptions { start_background: false }).unwrap();
        let acceptor = bipbridge::main_cli::tls_acceptor(&cert, &key).unwrap();
        bipbridge::main_cli::serve_door_on(listener, app.router.clone(), acceptor, CancellationToken::new());
        app
    } else {
        let app =
            bipbridge::app::create_app(config, bipbridge::app::AppOptions { start_background: false }).unwrap();
        let router = app.router.clone();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await;
        });
        app
    };
    (url, print, TestApp { app, peer: "198.51.100.7:50000".parse().unwrap() })
}

async fn agents_seen(from: &TestApp) -> Vec<String> {
    let state = &from.app.state;
    family_news::members(&state.http, &state.config())
        .await
        .into_iter()
        .flat_map(|m| m.agents.into_iter().map(|(id, _)| id))
        .collect()
}

#[tokio::test]
async fn two_places_of_the_offer_link_and_talk_with_pinned_certificates() {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a_url, a_print, a) = serve_bridge(da.path(), &["vie"], true).await;
    let (b_url, b_print, b) = serve_bridge(db.path(), &["cuisine"], true).await;
    let (a_print, b_print) = (a_print.unwrap(), b_print.unwrap());
    assert_ne!(a_print, b_print);
    let (k, v) = auth();
    let h = [(k, v.as_str())];

    // The bug: the client every bridge-to-bridge call used (WebPKI) cannot reach a door.
    let plain = reqwest::Client::builder().build().unwrap();
    let refused = plain.get(format!("{a_url}/family/agents")).send().await.unwrap_err();
    assert!(format!("{refused:?}").contains("InvalidCertificate"), "{refused:?}");

    assert_eq!(a.send_json("PUT", "/v1/family/name", &h, &json!({"name": "Alex"})).await.status, 200);
    assert_eq!(b.send_json("PUT", "/v1/family/name", &h, &json!({"name": "Léa"})).await.status, 200);
    let invitation = a.send_json("POST", "/v1/family/invite", &h, &json!({"days": 7})).await.json();
    // the invitation (QR code, or sealed through the guichet) carries the door's certificate, as pairing does
    assert_eq!(invitation["url"], json!(a_url));
    assert_eq!(invitation["fingerprint"], json!(a_print));

    // An app that drops it (older) gets plain words instead of « their bridge does not answer ».
    let old = b
        .send_json("POST", "/v1/family/join", &h, &json!({"url": invitation["url"], "code": invitation["code"]}))
        .await;
    assert_eq!(old.status, 409, "{}", old.text());
    // A wrong certificate is refused: nothing is linked.
    let wrong = b
        .send_json(
            "POST",
            "/v1/family/join",
            &h,
            &json!({"url": invitation["url"], "code": invitation["code"], "fingerprint": "00".repeat(32)}),
        )
        .await;
    assert_eq!(wrong.status, 502, "{}", wrong.text());
    assert_eq!(b.get("/v1/family", &h).await.json()["households"], json!([]));

    let joined = b.send_json("POST", "/v1/family/join", &h, &invitation).await;
    assert_eq!(joined.status, 200, "{}", joined.text());
    // each side keeps the other's certificate with its address
    assert_eq!(a.app.state.households.get("lea").unwrap()["fingerprint"], json!(b_print));
    assert_eq!(b.app.state.households.get("alex").unwrap()["fingerprint"], json!(a_print));
    assert_eq!(
        a.get("/v1/family", &h).await.json()["households"],
        json!([{"id": "lea", "name": "Léa", "url": b_url, "relink": false}])
    );

    // Both ways, over the pinned connection.
    assert_eq!(agents_seen(&a).await, vec!["cuisine".to_string()]);
    assert_eq!(agents_seen(&b).await, vec!["vie".to_string()]);

    // A certificate that is not the one kept: refused (the household looks silent).
    let mut data = a.app.state.households.load();
    data["households"]["lea"]["fingerprint"] = json!(a_print);
    a.app.state.households.save(&data).unwrap();
    assert!(agents_seen(&a).await.is_empty());
}

#[tokio::test]
async fn a_link_made_before_heals_when_the_place_announces_its_certificate() {
    // Léa at home (a public certificate: plain HTTP stands in for it here) invited Alex, whose place is on the offer.
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a_url, a_print, a) = serve_bridge(da.path(), &["vie"], true).await;
    let (_, b_print, b) = serve_bridge(db.path(), &["cuisine"], false).await;
    assert!(b_print.is_none());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(a.send_json("PUT", "/v1/family/name", &h, &json!({"name": "Alex"})).await.status, 200);
    assert_eq!(b.send_json("PUT", "/v1/family/name", &h, &json!({"name": "Léa"})).await.status, 200);
    let invitation = b.send_json("POST", "/v1/family/invite", &h, &json!({})).await.json();
    assert!(invitation.get("fingerprint").is_none()); // not a door: nothing to pin
    let joined = a.send_json("POST", "/v1/family/join", &h, &invitation).await;
    assert_eq!(joined.status, 200, "{}", joined.text());
    assert_eq!(b.app.state.households.get("alex").unwrap()["fingerprint"], json!(a_print.clone().unwrap()));

    // As a link made by older bridges: Léa's has no certificate for Alex's door, and cannot reach it.
    let mut data = b.app.state.households.load();
    data["households"]["alex"].as_object_mut().unwrap().shift_remove("fingerprint");
    b.app.state.households.save(&data).unwrap();
    assert_eq!(
        b.get("/v1/family", &h).await.json()["households"],
        json!([{"id": "alex", "name": "Alex", "url": a_url, "relink": true}])
    );
    assert!(agents_seen(&b).await.is_empty());
    assert_eq!(agents_seen(&a).await, vec!["cuisine".to_string()]); // the other way never needed it

    // Alex's bridge tells its certificate (at start) with the key Léa's gave it: kept, the link works again.
    let state = &a.app.state;
    door_tls::announce(&state.http, &state.config(), &state.households).await;
    assert_eq!(b.get("/v1/family", &h).await.json()["households"][0]["relink"], json!(false));
    assert_eq!(agents_seen(&b).await, vec!["vie".to_string()]);

    // Only a linked household's key may say so.
    let stranger = b
        .send_json("POST", "/family/door", &[("authorization", "Bearer nope")], &json!({"fingerprint": "00".repeat(32)}))
        .await;
    assert_eq!(stranger.status, 401);
    // and only for the address it is reached at
    let key = format!("Bearer {}", a.app.state.households.get("lea").unwrap()["key_out"].as_str().unwrap());
    let elsewhere = b
        .send_json(
            "POST",
            "/family/door",
            &[("authorization", key.as_str())],
            &json!({"fingerprint": "00".repeat(32), "url": "https://198.51.100.9"}),
        )
        .await;
    assert_eq!(elsewhere.status, 409);
    assert_eq!(b.app.state.households.get("alex").unwrap()["fingerprint"], json!(a_print.unwrap()));
}

#[tokio::test]
async fn pinned_or_public_certificates() {
    // A test authority stands in for a public one: the household without a fingerprint uses the usual client, which
    // trusts it; a pinned household accepts its own certificate only, whatever authority signed another.
    let ca_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let leaf = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .signed_by(&key, &ca, &ca_key)
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (cert_path, key_path) = (dir.path().join("c.pem"), dir.path().join("k.pem"));
    std::fs::write(&cert_path, leaf.pem()).unwrap();
    std::fs::write(&key_path, key.serialize_pem()).unwrap();
    let acceptor = bipbridge::main_cli::tls_acceptor(&s(&cert_path), &s(&key_path)).unwrap();
    let router = axum::Router::new().route("/family/agents", axum::routing::get(|| async { "ok" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    bipbridge::main_cli::serve_door_on(listener, router, acceptor, CancellationToken::new());
    let url = format!("https://localhost:{port}/family/agents");

    let base = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.pem().as_bytes()).unwrap())
        .build()
        .unwrap();
    let public = door_tls::household_client(&base, &serde_json::Map::new());
    assert_eq!(public.get(&url).send().await.unwrap().text().await.unwrap(), "ok");
    let print = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(leaf.der()));
    let pinned = door_tls::client_for(&base, Some(&print));
    assert_eq!(pinned.get(&url).send().await.unwrap().text().await.unwrap(), "ok");
    let other = door_tls::client_for(&base, Some(&"ab".repeat(32)));
    assert!(other.get(&url).send().await.is_err());
}
