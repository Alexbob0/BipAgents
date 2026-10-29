//! tests/test_vault.py: the person's credentials, given only to agents that can hold them, never read back.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};

use bipbridge::config::{parse_config, Config};
use bipbridge::vault::{normalize_origin, Vault, VaultError, VaultRun};
use bipbridge::Dict;
use common::{auth, config_dict, TestApp, Urls};
use serde_json::{json, Value};

type Calls = Arc<Mutex<Vec<(String, Dict)>>>;

fn setup(dir: &std::path::Path) -> (Config, Vault, Calls) {
    let mut data = config_dict(dir, &Urls::default());
    data["agents"]["Vie"] = json!({"hermes_url": "http://127.0.0.1:8641", "hermes_key": "k".repeat(32), "isolated": true,
                                   "python": "podman exec -i hermes-vie /opt/hermes/venv/bin/python",
                                   "hermes_home": "/home/hermes/.hermes"});
    data["agents"]["Budget"] = json!({"hermes_url": "http://127.0.0.1:8646/p/budget", "hermes_key": "b".repeat(32)});
    // the agents cannot read the bridge's files
    let home = dir.join("hermes-home");
    data["hermes"] = json!({"isolated": true, "bin": "/opt/hermes", "hub_url": "http://127.0.0.1:8646",
                            "home": home.to_string_lossy()});
    // Vie's own browser on: its « Coordonnées » reach its Hermes vault (vault.rs `follow_browser`)
    browser(dir, "vie", Some(true));
    let config = parse_config(&data, None).unwrap();
    let calls: Calls = Arc::default();
    let seen = calls.clone();
    let run: VaultRun = Arc::new(move |agent, request: Dict| {
        let mut calls = seen.lock().unwrap();
        calls.push((agent.name.clone(), request.clone()));
        let answer = if request.get("op") == Some(&json!("add")) {
            json!({"id": format!("vault_{}", calls.len())})
        } else {
            json!({"removed": true})
        };
        Box::pin(async move { Ok(answer.as_object().unwrap().clone()) })
    });
    let vault = Vault::with_run(&dir.join("vault").to_string_lossy(), run);
    (config, vault, calls)
}

/// The agent's browser in its Hermes profile: on, off, or none.
fn browser(dir: &std::path::Path, agent: &str, on: Option<bool>) {
    let folder = dir.join("hermes-home").join("profiles").join(agent);
    std::fs::create_dir_all(&folder).unwrap();
    let text = match on {
        Some(on) => format!("mcp_servers:\n  stealth:\n    url: http://127.0.0.1:9601/mcp\n    enabled: {on}\n"),
        None => "model: test\n".to_string(),
    };
    std::fs::write(folder.join("config.yaml"), text).unwrap();
}

fn login(extra: Value) -> Dict {
    let mut body = json!({"label": "SNCF", "site": "sncf-connect.com/login", "identifier": "alex@example.org",
                          "password": "s3cret-Pass!", "otp_secret": "JBSWY3DPEHPK3PXP"});
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    body.as_object().unwrap().clone()
}

fn ops(calls: &Calls) -> Vec<(String, String)> {
    calls.lock().unwrap().iter().map(|(n, r)| (n.clone(), r["op"].as_str().unwrap().to_string())).collect()
}

#[tokio::test]
async fn items_reach_only_the_agents_allowed_and_never_come_back() {
    let dir = tempfile::tempdir().unwrap();
    let (config, vault, calls) = setup(dir.path());
    let item = vault.create(&config, &login(json!({"agents": ["vie"]}))).await.unwrap();
    assert_eq!(item.origin.as_deref(), Some("https://sncf-connect.com"));
    assert!(item.has_otp);
    assert_eq!(item.agents.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect::<Vec<_>>(), vec![("vie", "vault_1")]);
    let (name, request) = calls.lock().unwrap()[0].clone();
    assert_eq!(name, "vie");
    assert_eq!(request["op"], "add");
    assert_eq!(request["kind"], "login");
    assert_eq!(
        request["secret"],
        json!({"password": "s3cret-Pass!", "otp_secret": "JBSWY3DPEHPK3PXP",
                                         "identifier_type": "email", "identifier": "alex@example.org"})
    );
    assert_eq!(config.agent(Some("vie")).unwrap().python[..3], ["podman", "exec", "-i"]);
    // The app only ever reads metadata; on disk, the secret is encrypted.
    let public = vault.items()[0].public();
    assert!(public.get("secret").is_none() && !public.to_string().contains("s3cret"));
    assert_eq!(public["agents"], json!(["vie"]));
    let blob = std::fs::read(dir.path().join("vault/vault.json.enc")).unwrap();
    assert!(!blob.windows(6).any(|w| w == b"s3cret"));
    let mode = std::fs::metadata(dir.path().join("vault/vault.key")).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[tokio::test]
async fn a_shared_hub_agent_cannot_hold_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let (config, vault, calls) = setup(dir.path());
    let err = vault.create(&config, &login(json!({"agents": ["vie", "budget"]}))).await.unwrap_err();
    match err.downcast_ref::<VaultError>() {
        Some(VaultError::CannotHold(reasons)) => {
            assert_eq!(
                reasons.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect::<Vec<_>>(),
                vec![("budget", "needs_own_space")]
            )
        }
        other => panic!("{other:?}"),
    }
    assert!(calls.lock().unwrap().is_empty() && vault.items().is_empty());
}

#[tokio::test]
async fn update_rewrites_the_item_and_follows_the_agents() {
    let dir = tempfile::tempdir().unwrap();
    let (config, vault, calls) = setup(dir.path());
    let item = vault.create(&config, &login(json!({"agents": ["vie"]}))).await.unwrap();
    calls.lock().unwrap().clear();
    let body = |v: Value| v.as_object().unwrap().clone();
    // Only the agents change: nothing to rewrite for Vie.
    let same = vault.update(&config, &item.id, &body(json!({"agents": ["vie"]}))).await.unwrap();
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(same.agents.get("vie").map(String::as_str), Some("vault_1"));
    // A new password: taken back from Vie, then given again; the password left out of later edits is kept.
    vault.update(&config, &item.id, &body(json!({"password": "n3w"}))).await.unwrap();
    assert_eq!(ops(&calls), vec![("vie".into(), "remove".into()), ("vie".into(), "add".into())]);
    assert_eq!(calls.lock().unwrap()[1].1["secret"]["password"], "n3w");
    calls.lock().unwrap().clear();
    vault.update(&config, &item.id, &body(json!({"label": "SNCF Connect"}))).await.unwrap();
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls[1].1["secret"]["password"], "n3w");
        assert_eq!(calls[1].1["label"], "SNCF Connect");
    }
    calls.lock().unwrap().clear();
    vault.update(&config, &item.id, &body(json!({"agents": []}))).await.unwrap();
    assert_eq!(ops(&calls), vec![("vie".into(), "remove".into())]);
    assert!(vault.items()[0].agents.is_empty());
    vault.delete(&config, &item.id).await.unwrap();
    assert!(vault.items().is_empty());
}

#[tokio::test]
async fn what_the_app_must_send() {
    let dir = tempfile::tempdir().unwrap();
    let (config, vault, _) = setup(dir.path());
    for extra in [
        json!({"password": ""}),
        json!({"identifier": " "}),
        json!({"site": "ftp://x"}),
        json!({"label": ""}),
        json!({"kind": "ssh"}),
    ] {
        let mut body = login(extra);
        body.insert("agents".into(), json!([]));
        let err = vault.create(&config, &body).await.unwrap_err();
        assert!(err.downcast_ref::<VaultError>().is_some(), "{err}");
    }
    assert_eq!(normalize_origin("HTTPS://Example.com:443/a?b").unwrap(), "https://example.com");
    assert_eq!(normalize_origin("example.com:8443").unwrap(), "https://example.com:8443");
}

#[tokio::test]
async fn routes() {
    let dir = tempfile::tempdir().unwrap();
    let mut data = config_dict(dir.path(), &Urls::default());
    data["hermes"] = json!({"isolated": true});
    let app = TestApp::new(parse_config(&data, None).unwrap());
    let (name, key) = auth();
    let headers = [(name, key.as_str())];
    let listing = app.get("/v1/vault", &headers).await;
    assert_eq!(listing.status, 200);
    assert_eq!(listing.json()["protected"], true);
    // Set up by hand (not in the hub): up to its administrator, the app offers no move.
    assert_eq!(
        listing.json()["agents"],
        json!([{"id": "wellness", "name": "Wellness", "can_hold": false, "why": "not_configured"}])
    );
    let refused =
        app.send_json("POST", "/v1/vault", &headers, &Value::Object(login(json!({"agents": ["wellness"]})))).await;
    assert_eq!(refused.status, 409);
    assert_eq!(refused.json()["detail"]["agents"], json!({"wellness": "not_configured"}));
    let kept = app.send_json("POST", "/v1/vault", &headers, &Value::Object(login(json!({"agents": []})))).await;
    assert_eq!(kept.status, 201);
    assert!(!kept.text().contains("password"));
    let id = kept.json()["id"].as_str().unwrap().to_string();
    assert_eq!(app.request("DELETE", &format!("/v1/vault/{id}"), &headers, None).await.status, 204);
    assert_eq!(app.get("/v1/vault", &[]).await.status, 401);
}

#[tokio::test]
async fn an_install_whose_agents_can_read_the_bridge_keeps_no_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(common::config(dir.path(), &Urls::default()));
    assert!(!app.app.state.config().hermes.isolated); // the default: nothing says the agents are isolated
    let (name, key) = auth();
    let headers = [(name, key.as_str())];
    let listing = app.get("/v1/vault", &headers).await.json();
    assert_eq!(listing["protected"], false);
    assert_eq!(listing["agents"][0]["why"], "unprotected_install");
    let refused = app.send_json("POST", "/v1/vault", &headers, &Value::Object(login(json!({"agents": []})))).await;
    assert_eq!(refused.status, 409);
    assert_eq!(refused.json()["detail"], json!({"code": "unprotected_install"}));
    assert!(app.app.state.vault.items().is_empty() && !app.app.state.vault.path.exists()); // nothing written
    assert_eq!(app.get("/v1/status", &headers).await.json()["protected"], false);
}

fn contact(extra: Value) -> Dict {
    let mut body = json!({"kind": "address", "label": "Kerfy Studio (pro)", "hint": "pour la société",
        "full_name": "Camille Exemple", "address_line1": "1 rue de l'Essai", "city": "Testville", "postal_code": "00100",
        "country": "fr", "phones": ["+33 6 00 00 00 01", "01 00 00 00 02"], "emails": ["camille@example.org"]});
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    body.as_object().unwrap().clone()
}

fn dict(v: Value) -> Dict {
    v.as_object().unwrap().clone()
}

#[tokio::test]
async fn contact_details_are_read_back_and_given_to_the_agents_browser() {
    let dir = tempfile::tempdir().unwrap();
    let (config, vault, calls) = setup(dir.path());
    let item = vault.create(&config, &contact(json!({"agents": ["vie"]}))).await.unwrap();
    let public = item.public();
    assert_eq!(public["kind"], "address");
    assert_eq!(public["hint"], "pour la société");
    assert_eq!(public["contact"]["country"], "FR");
    assert_eq!(public["contact"]["phones"], json!(["+33 6 00 00 00 01", "01 00 00 00 02"]));
    assert_eq!(public["agents"], json!(["vie"]));
    // Hermes keeps strings: the first phone and e-mail, for its browser fill
    let (_, request) = calls.lock().unwrap()[0].clone();
    assert_eq!(request["kind"], "address");
    assert_eq!(
        request["secret"],
        json!({"full_name": "Camille Exemple", "address_line1": "1 rue de l'Essai", "city": "Testville",
               "postal_code": "00100", "country": "FR", "phone": "+33 6 00 00 00 01", "email": "camille@example.org"})
    );
    // an edit: one field changed, the rest kept, written again into Hermes
    calls.lock().unwrap().clear();
    let edited = vault.update(&config, &item.id, &dict(json!({"emails": [], "address_line2": "Bât. B"}))).await.unwrap();
    assert_eq!(edited.public()["contact"]["emails"], json!([]));
    assert_eq!(edited.public()["contact"]["address_line2"], "Bât. B");
    assert_eq!(edited.public()["contact"]["full_name"], "Camille Exemple");
    assert_eq!(ops(&calls), vec![("vie".into(), "remove".into()), ("vie".into(), "add".into())]);
    // only the hint: nothing to rewrite
    calls.lock().unwrap().clear();
    let hinted = vault.update(&config, &item.id, &dict(json!({"hint": "factures"}))).await.unwrap();
    assert_eq!(hinted.hint.as_deref(), Some("factures"));
    assert!(calls.lock().unwrap().is_empty());
    // the agent reads it, an agent without access does not
    let entries = vault.contacts_for(&config, "vie").unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].label, "Kerfy Studio (pro)");
    assert!(vault.contacts_for(&config, "budget").unwrap().is_empty());
    for bad in [json!({"country": "Francia"}), json!({"phones": ["call me"]}), json!({"emails": ["x@y"]})] {
        let err = vault.update(&config, &item.id, &dict(bad.clone())).await.unwrap_err();
        assert!(matches!(err.downcast_ref::<VaultError>(), Some(VaultError::Invalid(_))), "{bad}");
    }
}

#[tokio::test]
async fn an_old_address_item_stays_valid() {
    let dir = tempfile::tempdir().unwrap();
    let (config, vault, _) = setup(dir.path());
    let old = json!({"kind": "address", "label": "Maison", "address_line1": "2 allée Test", "city": "Lyon",
                     "postal_code": "69001", "country": "FR", "agents": []});
    let item = vault.create(&config, &dict(old)).await.unwrap();
    assert_eq!(item.public()["contact"]["city"], "Lyon");
    assert_eq!(item.public()["hint"], "");
    let renamed = vault.update(&config, &item.id, &dict(json!({"label": "Domicile"}))).await.unwrap();
    assert_eq!(renamed.public()["contact"]["postal_code"], "69001");
}

#[tokio::test]
async fn a_hermes_that_refuses_an_incomplete_address_leaves_it_to_the_bridge() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _, _) = setup(dir.path());
    let run: VaultRun = Arc::new(|_, _| {
        Box::pin(async {
            Err(VaultError::Invalid("vie: Hermes refused the item (VaultError: address items require city)".into()).into())
        })
    });
    let vault = Vault::with_run(&dir.path().join("vault2").to_string_lossy(), run);
    let body = json!({"kind": "address", "label": "Pro", "emails": ["pro@example.org"], "agents": ["vie"]});
    let item = vault.create(&config, &dict(body)).await.unwrap();
    assert_eq!(item.agents.get("vie").map(String::as_str), Some("bridge"));
    assert_eq!(vault.contacts_for(&config, "vie").unwrap().len(), 1);
    // a login is still refused as before
    assert!(vault.create(&config, &login(json!({"agents": ["vie"]}))).await.is_err());
}

#[tokio::test]
async fn an_agent_saves_what_the_person_gave_it_and_never_changes_the_persons_entries() {
    let dir = tempfile::tempdir().unwrap();
    let (config, vault, _) = setup(dir.path());
    let vie = config.agent(Some("vie")).unwrap().clone();
    let args = json!({"label": "Adresse pro", "hint": "factures", "address_line1": "3 rue Test", "city": "Testville",
                      "postal_code": "00100", "country": "FR", "emails": ["pro@example.org"]});
    let saved = vault.save_contact(&config, &vie, &dict(args)).await.unwrap();
    assert_eq!(saved.created_by.as_deref(), Some("vie"));
    assert_eq!(saved.public()["created_by"], "vie");
    assert_eq!(saved.public()["agents"], json!(["vie"]));
    // the same label: its own entry changed
    let again =
        vault.save_contact(&config, &vie, &dict(json!({"label": "adresse PRO", "phones": ["06 00 00 00 03"]}))).await.unwrap();
    assert_eq!(again.id, saved.id);
    assert_eq!(again.public()["contact"]["phones"], json!(["06 00 00 00 03"]));
    assert_eq!(again.public()["contact"]["city"], "Testville");
    assert_eq!(again.public()["created_by"], "vie");
    // the person's own entry: never changed by an agent
    let mine = vault.create(&config, &contact(json!({"agents": ["vie"]}))).await.unwrap();
    let err = vault
        .save_contact(&config, &vie, &dict(json!({"label": "Kerfy Studio (pro)", "city": "Ailleurs"})))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("only the person changes it"), "{err}");
    assert_eq!(vault.items().iter().find(|i| i.id == mine.id).unwrap().public()["contact"]["city"], "Testville");
    // an agent that can't hold vault items: nothing kept
    let budget = config.agent(Some("budget")).unwrap().clone();
    let err = vault.save_contact(&config, &budget, &dict(json!({"label": "X", "city": "Y"}))).await.unwrap_err();
    assert!(matches!(err.downcast_ref::<VaultError>(), Some(VaultError::CannotHold(_))));
    let err = vault.save_contact(&config, &vie, &dict(json!({"label": " "}))).await.unwrap_err();
    assert_eq!(err.to_string(), "give the entry a label");
}

#[tokio::test]
async fn an_agent_other_people_talk_to_reads_only_what_the_person_confirmed_for_it() {
    let dir = tempfile::tempdir().unwrap();
    let (config, vault, _) = setup(dir.path());
    // given while Vie was the person's alone
    vault.create(&config, &contact(json!({"agents": ["vie"]}))).await.unwrap();
    assert_eq!(vault.contacts_for(&config, "vie").unwrap().len(), 1);
    // then Vie is shared with a relative: it reads nothing it wasn't confirmed for
    let data = config.data_dir();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(format!("{data}/households.json"), r#"{"households": {"lea": {"name": "Léa"}}}"#).unwrap();
    std::fs::write(format!("{data}/agent-shares.json"), r#"{"shared": {"vie": ["lea"]}}"#).unwrap();
    assert!(bipbridge::vault::others_facing(&config, "vie"));
    assert!(vault.contacts_for(&config, "vie").unwrap().is_empty());
    // a new entry for Vie: refused without the confirmation, read once confirmed
    let err = vault.create(&config, &contact(json!({"label": "Maison", "agents": ["vie"]}))).await.unwrap_err();
    assert!(matches!(err.downcast_ref::<VaultError>(), Some(VaultError::ConfirmOthers(_))));
    vault.create(&config, &contact(json!({"label": "Maison", "agents": ["vie"], "confirmed_others": true}))).await.unwrap();
    let read: Vec<String> = vault.contacts_for(&config, "vie").unwrap().into_iter().map(|e| e.label).collect();
    assert_eq!(read, vec!["Maison".to_string()]);
}

#[tokio::test]
async fn contact_details_reach_the_agents_hermes_vault_only_while_its_browser_is_on() {
    // 2026-10-29 (model bench): each entry in an agent's Hermes vault shows Hermes' five vault tools on every call
    let dir = tempfile::tempdir().unwrap();
    let (config, vault, calls) = setup(dir.path());
    let vie = config.agent(Some("vie")).unwrap().clone();
    browser(dir.path(), "vie", Some(false));
    let item = vault.create(&config, &contact(json!({"agents": ["vie"]}))).await.unwrap();
    assert!(calls.lock().unwrap().is_empty(), "browser off: nothing written into Hermes");
    assert_eq!(item.agents.get("vie").map(String::as_str), Some("bridge"));
    assert_eq!(vault.contacts_for(&config, "vie").unwrap().len(), 1, "the agent still reads it");
    // a login next to it keeps today's way: into Hermes, browser or not
    let sncf = vault.create(&config, &login(json!({"agents": ["vie"]}))).await.unwrap();
    assert_eq!(ops(&calls), vec![("vie".into(), "add".into())]);
    calls.lock().unwrap().clear();
    // nothing to do while the browser stays off
    assert!(!vault.follow_browser(&config, &vie).await.unwrap());
    // the browser turned on: the entry written, its handle kept
    browser(dir.path(), "vie", Some(true));
    assert!(vault.follow_browser(&config, &vie).await.unwrap());
    let (_, request) = calls.lock().unwrap()[0].clone();
    assert_eq!((request["op"].as_str(), request["kind"].as_str()), (Some("add"), Some("address")));
    let handle = vault.items().into_iter().find(|i| i.id == item.id).unwrap().agents["vie"].clone();
    assert!(handle.starts_with("vault_"), "{handle}");
    assert_eq!(calls.lock().unwrap().len(), 1, "the login was not written again");
    assert!(!vault.follow_browser(&config, &vie).await.unwrap(), "once");
    calls.lock().unwrap().clear();
    // turned off: taken out of Hermes, the bridge's copy kept, the login left
    browser(dir.path(), "vie", Some(false));
    assert!(vault.follow_browser(&config, &vie).await.unwrap());
    let removed = calls.lock().unwrap().clone();
    assert_eq!(removed.len(), 1);
    assert_eq!((removed[0].1["op"].as_str(), removed[0].1["id"].as_str()), (Some("remove"), Some(handle.as_str())));
    let items = vault.items();
    assert_eq!(items.iter().find(|i| i.id == item.id).unwrap().agents["vie"], "bridge");
    assert!(items.iter().find(|i| i.id == sncf.id).unwrap().agents["vie"].starts_with("vault_"));
    assert_eq!(vault.contacts_for(&config, "vie").unwrap().len(), 1);
    // unshared while off: nothing to remove from Hermes; an agent with no browser at all gets nothing written
    calls.lock().unwrap().clear();
    vault.update(&config, &item.id, &dict(json!({"agents": []}))).await.unwrap();
    assert!(calls.lock().unwrap().is_empty());
    browser(dir.path(), "vie", None);
    vault.update(&config, &item.id, &dict(json!({"agents": ["vie"]}))).await.unwrap();
    assert!(calls.lock().unwrap().is_empty());
    // on again (the bridge's start), then unshared: removed from Hermes
    browser(dir.path(), "vie", Some(true));
    assert!(vault.follow_browsers(&config).await);
    calls.lock().unwrap().clear();
    vault.update(&config, &item.id, &dict(json!({"agents": []}))).await.unwrap();
    assert_eq!(ops(&calls), vec![("vie".into(), "remove".into())]);
}
