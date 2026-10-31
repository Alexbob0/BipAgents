//! The LAN door: the same app over HTTPS with the pinned self-signed certificate (Python: the uvicorn « Door »).

mod common;

use common::*;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn the_door_serves_the_app_over_tls_with_the_pinned_certificate() {
    let dir = tempfile::tempdir().unwrap();
    let mut dict = config_dict(dir.path(), &Urls::default());
    dict["lan"] = json!({"enabled": true, "address": "192.168.1.177", "port": 8650});
    let config = bipbridge::config::parse_config(&dict, None).unwrap();
    let (cert, key) = bipbridge::lan::ensure_certificate(&config).unwrap();
    let print = bipbridge::lan::fingerprint(&config).unwrap();
    let app = bipbridge::app::create_app(config, bipbridge::app::AppOptions { start_background: false }).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let stop = CancellationToken::new();
    let acceptor = bipbridge::main_cli::tls_acceptor(&cert, &key).unwrap();
    let door = bipbridge::main_cli::serve_door_on(listener, app.router.clone(), acceptor, stop.clone());

    let client = reqwest::Client::builder().danger_accept_invalid_certs(true).tls_info(true).build().unwrap();
    let resp = client.get(format!("https://127.0.0.1:{port}/v1/pairing")).bearer_auth(BRIDGE_KEY).send().await.unwrap();
    // the app pins the certificate: the one served is the one whose hash travels in the QR code
    let info = resp.extensions().get::<reqwest::tls::TlsInfo>().unwrap();
    assert_eq!(hex::encode(Sha256::digest(info.peer_certificate().unwrap())), print);
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap(),
        json!({"lan": {"url": "https://192.168.1.177:8650", "fingerprint": print}})
    );
    let resp = client.get(format!("https://127.0.0.1:{port}/v1/pairing")).send().await.unwrap();
    assert_eq!(resp.status(), 401);
    stop.cancel();
    let _ = door.await;
    app.shutdown().await;
}

/// Root's router on 443 (installer/cloud/seat-agent.py, HAProxy `send-proxy-v2`), played by a small TCP relay that
/// puts a PROXY v2 header first: the door takes the client's address from it, so wrong keys count against that
/// client (guard.rs) as they would at a direct door, while the same wrong keys from 127.0.0.1 itself never do.
#[tokio::test]
async fn through_the_router_wrong_keys_count_against_the_real_client() {
    use tokio::io::AsyncWriteExt;
    let dir = tempfile::tempdir().unwrap();
    let mut dict = config_dict(dir.path(), &Urls::default());
    dict["lan"] = json!({"enabled": true, "address": "192.168.1.177", "port": 8650});
    let config = bipbridge::config::parse_config(&dict, None).unwrap();
    let (cert, key) = bipbridge::lan::ensure_certificate(&config).unwrap();
    let app = bipbridge::app::create_app(config, bipbridge::app::AppOptions { start_background: false }).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let door_port = listener.local_addr().unwrap().port();
    let stop = CancellationToken::new();
    let acceptor = bipbridge::main_cli::tls_acceptor(&cert, &key).unwrap();
    let door = bipbridge::main_cli::serve_door_on(listener, app.router.clone(), acceptor, stop.clone());

    let relay = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_port = relay.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut inbound, _)) = relay.accept().await else { return };
            tokio::spawn(async move {
                let mut outbound = tokio::net::TcpStream::connect(("127.0.0.1", door_port)).await.unwrap();
                let mut header = bipbridge::proxy_header::SIGNATURE.to_vec();
                header.extend([0x21, 0x11, 0x00, 12, 203, 0, 113, 50, 127, 0, 0, 1, 0xC3, 0x50, 0x01, 0xBB]);
                outbound.write_all(&header).await.unwrap();
                let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
            });
        }
    });

    let client = reqwest::Client::builder().danger_accept_invalid_certs(true).build().unwrap();
    let mut direct = Vec::new();
    for _ in 0..25 {
        let resp = client.get(format!("https://127.0.0.1:{door_port}/v1/pairing")).bearer_auth("wrong").send().await;
        direct.push(resp.unwrap().status().as_u16());
    }
    assert!(direct.iter().all(|s| *s == 401), "loopback is never blocked: {direct:?}");
    let mut routed = Vec::new();
    for _ in 0..25 {
        let fresh = reqwest::Client::builder().danger_accept_invalid_certs(true).build().unwrap();
        let resp = fresh.get(format!("https://127.0.0.1:{relay_port}/v1/pairing")).bearer_auth("wrong").send().await;
        routed.push(resp.unwrap().status().as_u16());
    }
    assert_eq!(routed[0], 401);
    assert_eq!(*routed.last().unwrap(), 429, "the routed client is blocked after its wrong keys: {routed:?}");
    // the right key still works through the router, the stream unharmed by the header
    let fresh = reqwest::Client::builder().danger_accept_invalid_certs(true).build().unwrap();
    let ok = fresh.get(format!("https://127.0.0.1:{door_port}/v1/pairing")).bearer_auth(BRIDGE_KEY).send().await.unwrap();
    assert_eq!(ok.status(), 200);
    stop.cancel();
    let _ = door.await;
    app.shutdown().await;
}
