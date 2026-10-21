//! tests/test_desktop.py: the routes (the protocol pieces are unit tests in src/desktop.rs).

mod common;

use common::{auth, config, TestApp, Urls};

#[tokio::test]
async fn routes() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(config(dir.path(), &Urls::default()));
    let (name, key) = auth();
    let headers = [(name, key.as_str())];
    // set up by hand, no viewer page
    assert_eq!(app.request("POST", "/v1/agents/wellness/desktop", &headers, None).await.status, 409);
    assert_eq!(app.request("POST", "/v1/agents/nobody/desktop", &headers, None).await.status, 404);
    assert_eq!(app.request("POST", "/v1/agents/wellness/desktop", &[], None).await.status, 401);
    assert_eq!(app.get("/desktop/nope/", &[]).await.status, 404);
    assert_eq!(app.get("/desktop/static/novnc/core/rfb.js", &[]).await.status, 200);
}
