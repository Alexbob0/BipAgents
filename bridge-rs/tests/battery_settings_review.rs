//! Review of battery-settings (16/11): what the six battery settings must not break, as failing tests first
//! (BipAgents-reprise/passation-pro/briefs/review-battery-settings.md).

mod common;

use std::time::Duration;

use bipbridge::config::parse_config;
use common::{config_dict, Urls};
use serde_json::json;

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// R5 (D7). The Bureau's page view now closes its stream when it is hidden and opens it again when it is shown, with
/// the ticket in its address — issued when the Bureau opened, for 10 min (desktop.rs `TICKET_SECONDS`), never renewed
/// (the apps keep the page; they ask for a ticket only when the live view first comes). Watched for 10 min, then
/// hidden a moment (the app left for a code, the Bureau under another screen, Android's WebView paused), the page is
/// refused (403) when shown again, every 1.5 s, for as long as it stays open: its pictures never come back. Before
/// D7 its socket stayed open while hidden. A ticket in use must stay valid `TICKET_SECONDS` after its page's last
/// stream ended.
#[tokio::test]
async fn the_page_view_shown_again_after_ten_minutes_of_watching_gets_its_pictures_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut d = config_dict(dir.path(), &Urls::default());
    d["agents"]["Vie"] = json!({"hermes_url": "http://127.0.0.1:9", "hermes_key": "v".repeat(32),
                                "desktop_vnc": format!("tcp://127.0.0.1:{}", free_port())});
    // its own browser container (its DevTools port: nothing listens there, the page view says « stopped »)
    let folder = dir.path().join("data").join("browsers").join("vie");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("ports.json"), json!({"cdp": free_port(), "vnc": free_port()}).to_string()).unwrap();
    let app = bipbridge::app::create_app(parse_config(&d, None).unwrap(), bipbridge::app::AppOptions { start_background: false })
        .unwrap();
    let state = app.state.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await;
    });
    // the Bureau opened on a phone: its page view's ticket (what `POST /v1/agents/vie/desktop` issues)
    let ticket = state.desktop_tickets.issue("vie");
    let url = format!("ws://{addr}/desktop/{ticket}/page/ws?mobile=1&w=390&h=700&dpr=3");
    let (mut socket, _) = tokio_tungstenite::connect_async(url.as_str()).await.expect("the page view's stream opens");
    // watched for ten minutes: the ticket, issued when the Bureau opened, ends in 2 s
    state.desktop_tickets.items.lock().unwrap().get_mut(&ticket).unwrap().1 = bipbridge::py::time_time() + 2.0;
    tokio::time::sleep(Duration::from_millis(500)).await;
    // hidden a moment: the page closes its socket (D7)
    socket.close(None).await.ok();
    drop(socket);
    tokio::time::sleep(Duration::from_secs(3)).await;
    // shown again: its stream must open again
    let again = tokio_tungstenite::connect_async(url.as_str()).await;
    assert!(
        again.is_ok(),
        "shown again after 10 min of watching, the page view's stream is refused ({:?}): its pictures never come back",
        again.err().map(|e| e.to_string())
    );
    app.shutdown().await;
}
