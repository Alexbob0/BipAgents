//! tests/test_place_backup.py: the app gets the backup key and when the last backup worked. The only test in this
//! binary: it sets $HOME, which the route reads at each call.

mod common;

use bipbridge::place_backup::{ALLOWANCE_BYTES, SETTINGS, STATE};
use common::{auth, config, TestApp, Urls};
use serde_json::json;

#[tokio::test]
async fn the_app_gets_the_key_and_when_the_last_backup_worked() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    let app = TestApp::new(config(dir.path(), &Urls::default()));
    let (k, v) = auth();
    assert_eq!(app.get("/v1/backup", &[(k, &v)]).await.json(), json!({"enabled": false}));
    assert_eq!(app.get("/v1/backup", &[]).await.status, 401);
    assert_eq!(app.request("POST", "/v1/backup/now", &[(k, &v)], None).await.json(), json!({"started": false}));
    std::fs::create_dir_all(home.join(SETTINGS).parent().unwrap()).unwrap();
    std::fs::write(home.join(SETTINGS), r#"{"url": "https://g.test/v1/backup/", "token": "t", "password": "the-key"}"#)
        .unwrap();
    assert_eq!(
        app.get("/v1/backup", &[(k, &v)]).await.json(),
        json!({"enabled": true, "key": "the-key", "allowance_bytes": ALLOWANCE_BYTES})
    );
    std::fs::create_dir_all(home.join(STATE).parent().unwrap()).unwrap();
    std::fs::write(home.join(STATE), r#"{"last_ok": 1790000000.0, "snapshot": "abc"}"#).unwrap();
    let status = app.get("/v1/backup", &[(k, &v)]).await.json();
    assert_eq!(
        status,
        json!({"enabled": true, "key": "the-key", "allowance_bytes": ALLOWANCE_BYTES, "last_ok": 1790000000.0})
    );
    // the token never leaves
}
