//! tests/test_place_import.py: a computer proves the one-time code, then sends its export, once.

mod browsers_places;
mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bipbridge::place_import::{self, Imports, ALPHABET, FOLDER, IMPORT_CONTEXT, UPLOAD_CONTEXT};
use browsers_places::{mac, send, v1};
use common::{auth, config, TestApp, Urls};
use serde_json::json;

#[tokio::test]
async fn a_computer_proves_the_code_then_sends_its_export_once() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let app = TestApp::new(config(dir.path(), &Urls::default()));
    let state = app.app.state.clone();
    let started: Arc<Mutex<Vec<(PathBuf, PathBuf)>>> = Arc::default();
    let seen = started.clone();
    let imports = Arc::new(Imports::with(
        Some(home.clone()),
        bipbridge::py::wall_clock(),
        Arc::new(move |_folder: &std::path::Path, archive: &std::path::Path, secret: &std::path::Path| {
            seen.lock().unwrap().push((archive.to_path_buf(), secret.to_path_buf()));
            Ok(())
        }),
    ));
    let (admin, open) = place_import::routers_with(imports);
    let router = v1(&state, admin, open);
    let (k, v) = auth();
    let key = [(k, v.as_str())];
    assert_eq!(send(&router, "POST", "/v1/import", &key, None).await.status, 404); // someone's own computer
    state.config.update(|c| {
        c.cloud = true;
        c.public_url = Some("https://203.0.113.7:8701".into());
    });
    bipbridge::lan::ensure_certificate(&state.config()).unwrap();
    let nonce = bipbridge::py::token_hex(16);
    let hello_path = format!("/v1/import/hello?nonce={nonce}");
    assert_eq!(send(&router, "GET", &hello_path, &[], None).await.status, 404); // no code yet
    assert_eq!(send(&router, "GET", "/v1/import/hello?nonce=short", &[], None).await.status, 422);
    assert_eq!(send(&router, "POST", "/v1/import", &[], None).await.status, 401);
    let given = send(&router, "POST", "/v1/import", &key, None).await.json();
    assert_eq!(given["state"], "waiting");
    assert_eq!(given["address"], "203.0.113.7:8701");
    let shown = given["code"].as_str().unwrap().to_string();
    assert_eq!(given["command"], format!("bipagents send 203.0.113.7:8701 {shown}"));
    assert!(given["command_alone"].as_str().unwrap().ends_with(&format!("| bash -s -- send 203.0.113.7:8701 {shown}")));
    let code = shown.replace('-', "");
    assert!(code.len() == 16 && code.chars().all(|c| ALPHABET.contains(c)));

    let hello = send(&router, "GET", &hello_path, &[], None).await.json();
    let print = bipbridge::lan::fingerprint(&state.config()).unwrap();
    assert_eq!(hello["fingerprint"], print);
    let challenge = hello["challenge"].as_str().unwrap().to_string();
    assert_eq!(hello["proof"], mac(code.as_bytes(), &[IMPORT_CONTEXT, &nonce, &print, &challenge]));

    let body = b"sealed export ".repeat(1000);
    let size = body.len().to_string();
    let wrong_mac = mac(b"0000000000000000", &[UPLOAD_CONTEXT, &challenge, &size]);
    let wrong = send(
        &router,
        "PUT",
        "/v1/import/archive",
        &[("x-import-challenge", &challenge), ("x-import-proof", &wrong_mac), ("content-length", &size)],
        Some(body.clone()),
    )
    .await;
    assert_eq!(wrong.status, 401);
    assert!(started.lock().unwrap().is_empty());
    let challenge =
        send(&router, "GET", &hello_path, &[], None).await.json()["challenge"].as_str().unwrap().to_string();
    let proof = mac(code.as_bytes(), &[UPLOAD_CONTEXT, &challenge, &size]);
    let headers = [
        ("x-import-challenge", challenge.as_str()),
        ("x-import-proof", proof.as_str()),
        ("content-length", size.as_str()),
    ];
    assert_eq!(send(&router, "PUT", "/v1/import/archive", &headers[..2], Some(body.clone())).await.status, 411);
    let challenge =
        send(&router, "GET", &hello_path, &[], None).await.json()["challenge"].as_str().unwrap().to_string();
    let proof = mac(code.as_bytes(), &[UPLOAD_CONTEXT, &challenge, &size]);
    let headers = [
        ("x-import-challenge", challenge.as_str()),
        ("x-import-proof", proof.as_str()),
        ("content-length", size.as_str()),
    ];
    let sent = send(&router, "PUT", "/v1/import/archive", &headers, Some(body.clone())).await;
    assert_eq!(sent.json(), json!({"started": true}));
    let (archive, secret) = started.lock().unwrap()[0].clone();
    assert_eq!(std::fs::read(&archive).unwrap(), body);
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), code);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&archive).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(&secret).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(send(&router, "GET", "/v1/import", &key, None).await.json()["state"], "importing");
    // Used: no second sending with it, and nothing to say hello to.
    assert_eq!(send(&router, "GET", &hello_path, &[], None).await.status, 404);
    assert_eq!(send(&router, "PUT", "/v1/import/archive", &headers, Some(body.clone())).await.status, 403);
    assert_eq!(send(&router, "POST", "/v1/import", &key, None).await.status, 409); // not while it is put in

    let progress = home.join(FOLDER).join("progress.json");
    std::fs::write(&progress, r#"{"step": 4, "total": 4, "label": "Ready", "done": true, "at": 1}"#).unwrap();
    assert_eq!(
        send(&router, "GET", "/v1/import", &key, None).await.json(),
        json!({"state": "done", "step": 4, "total": 4, "label": "Ready"})
    );
    assert_eq!(send(&router, "DELETE", "/v1/import", &key, None).await.status, 204);
    assert_eq!(send(&router, "GET", "/v1/import", &key, None).await.json(), json!({"state": "none"}));
}

#[tokio::test]
async fn the_app_s_routes_are_mounted_in_the_whole_app() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(config(dir.path(), &Urls::default()));
    let (k, v) = auth();
    assert_eq!(app.get("/v1/import", &[(k, &v)]).await.json(), json!({"detail": "not found"}));
    assert_eq!(app.get("/v1/import", &[]).await.status, 401);
    let nonce = "ab".repeat(16);
    assert_eq!(app.get(&format!("/v1/import/hello?nonce={nonce}"), &[]).await.status, 404);
}
