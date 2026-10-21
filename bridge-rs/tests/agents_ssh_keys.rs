//! test_ssh_keys.py: the person adds, lists and removes their keys on a server of the offer. Its own test binary:
//! it points HOME at a temporary folder for the whole process.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use common::{auth, TestApp, Urls};
use serde_json::json;

fn a_key(dir: &Path, name: &str, kind: &str) -> (String, String) {
    let path = dir.join(name);
    let made = Command::new("ssh-keygen")
        .args(["-q", "-t", kind, "-N", "", "-C", &format!("{name}@laptop"), "-f"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(made.success());
    let public = std::fs::read_to_string(dir.join(format!("{name}.pub"))).unwrap().trim().to_string();
    let listed = Command::new("ssh-keygen").arg("-l").arg("-f").arg(dir.join(format!("{name}.pub"))).output().unwrap();
    let shown = String::from_utf8_lossy(&listed.stdout).split_whitespace().nth(1).unwrap().to_string();
    (public, shown)
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[tokio::test]
async fn the_person_adds_lists_and_removes_their_keys_on_a_server_of_the_offer() {
    if Command::new("ssh-keygen").arg("-?").output().is_err() {
        return; // no ssh-keygen here
    }
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    std::env::set_var("HOME", &home);
    let app = TestApp::new(common::config(dir.path(), &Urls::default()));
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(app.get("/v1/ssh", &h).await.json(), json!({"enabled": false, "keys": []})); // someone's own computer
    let (public, _) = a_key(dir.path(), "one", "ed25519");
    assert_eq!(app.send_json("POST", "/v1/ssh/keys", &h, &json!({"key": public})).await.status, 403);
    app.app.state.config.update(|c| {
        c.cloud = true;
        c.public_url = Some("https://203.0.113.7".into());
    });
    assert_eq!(app.get("/v1/ssh", &[]).await.status, 401);
    std::fs::create_dir(home.join(".ssh")).unwrap();
    std::fs::set_permissions(home.join(".ssh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(home.join(".ssh/authorized_keys"), "# kept\n").unwrap();
    let added = app.send_json("POST", "/v1/ssh/keys", &h, &json!({"key": public})).await.json();
    let (other, shown) = a_key(dir.path(), "two", "rsa");
    app.send_json("POST", "/v1/ssh/keys", &h, &json!({"key": format!("{other}\n")})).await;
    app.send_json("POST", "/v1/ssh/keys", &h, &json!({"key": public})).await; // once only
    let status = app.get("/v1/ssh", &h).await.json();
    assert_eq!(status["enabled"], true);
    assert!(status["login"].as_str().unwrap().ends_with("@203.0.113.7"));
    let listed: Vec<(String, String)> = status["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| (k["type"].as_str().unwrap().to_string(), k["comment"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(
        listed,
        [("ssh-ed25519".to_string(), "one@laptop".to_string()), ("ssh-rsa".to_string(), "two@laptop".to_string())]
    );
    assert_eq!(status["keys"][1]["fingerprint"], json!(shown));
    assert_eq!(added["fingerprint"], status["keys"][0]["fingerprint"]);
    let text = std::fs::read_to_string(home.join(".ssh/authorized_keys")).unwrap();
    assert!(text.starts_with("# kept\n") && text.matches('\n').count() == 3);
    assert_eq!(mode(&home.join(".ssh")), 0o700);
    assert_eq!(mode(&home.join(".ssh/authorized_keys")), 0o600);
    let query =
        format!("/v1/ssh/keys?fingerprint={}", shown.replace('+', "%2B").replace('/', "%2F").replace('=', "%3D"));
    let gone = app.request("DELETE", &query, &h, None).await;
    assert_eq!(gone.status, 200);
    assert!(!std::fs::read_to_string(home.join(".ssh/authorized_keys")).unwrap().contains("two@laptop"));
    assert_eq!(app.request("DELETE", &query, &h, None).await.status, 404);
}
