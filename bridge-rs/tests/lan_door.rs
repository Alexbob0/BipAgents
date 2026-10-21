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
