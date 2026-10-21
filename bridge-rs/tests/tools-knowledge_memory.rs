//! Port of bridge/tests/test_memory_edit.py, test_notes_import.py and test_reminders.py: what an agent remembers
//! (entries and knowledge pages), notes imported into its knowledge, the iPhone's reminders.

mod common;

use std::io::Write;

use base64::Engine;
use bipbridge::config::{parse_config, Config};
use bipbridge::reminders;
use bipbridge::Dict;
use common::{auth, config_dict, TestApp, Urls};
use serde_json::{json, Value};

/// An agent this install manages (served by the hub under /p/), its profile in a temporary HERMES_HOME.
fn with_agent(dir: &std::path::Path) -> (Config, std::path::PathBuf) {
    let mut dict = config_dict(dir, &Urls::default());
    dict["hermes"] = json!({"bin": "/opt/hermes", "home": dir.join(".hermes").to_string_lossy(), "hub_url": "http://127.0.0.1:8642"});
    dict["agents"]["coach"] =
        json!({"display_name": "Coach", "hermes_url": "http://127.0.0.1:8642/p/coach", "hermes_key": "k"});
    (parse_config(&dict, None).unwrap(), dir.join(".hermes/profiles/coach"))
}

#[tokio::test]
async fn the_person_reads_and_edits_what_the_agent_remembers() {
    let dir = tempfile::tempdir().unwrap();
    let (config, folder) = with_agent(dir.path());
    let app = TestApp::new(config);
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    std::fs::create_dir_all(folder.join("memories")).unwrap();
    std::fs::write(folder.join("memories/MEMORY.md"), "Alex court le dimanche.\n§\nLe chat s'appelle Pixel.").unwrap();
    std::fs::write(folder.join("config.yaml"), "memory:\n  user_char_limit: 40\n").unwrap();
    let seen = app.get("/v1/agents/coach/memory", &h).await.json();
    assert_eq!(seen["notes"]["entries"], json!(["Alex court le dimanche.", "Le chat s'appelle Pixel."]));
    assert_eq!(seen["notes"]["limit"], 2200);
    assert_eq!(seen["user"], json!({"entries": [], "limit": 40, "used": 0}));
    let edited = app
        .send_json(
            "PUT",
            "/v1/agents/coach/memory/notes",
            &h,
            &json!({"entries": ["Le chat s'appelle Pixel.", " ", "Né en 1985."]}),
        )
        .await
        .json();
    assert_eq!(edited["entries"], json!(["Le chat s'appelle Pixel.", "Né en 1985."]));
    assert_eq!(
        std::fs::read_to_string(folder.join("memories/MEMORY.md")).unwrap(),
        "Le chat s'appelle Pixel.\n§\nNé en 1985."
    );
    assert!(folder.join("memories/MEMORY.md.lock").exists()); // Hermes' own lock, taken
    let too_long =
        app.send_json("PUT", "/v1/agents/coach/memory/user", &h, &json!({"entries": ["x".repeat(41)]})).await;
    assert_eq!(too_long.status, 413);
    let sneaky =
        app.send_json("PUT", "/v1/agents/coach/memory/user", &h, &json!({"entries": ["a\n§\nb"]})).await.json();
    assert_eq!(sneaky["entries"], json!(["ab"])); // one entry stays one entry
    assert_eq!(app.get("/v1/agents/nobody/memory", &h).await.status, 404);
    assert_eq!(app.get("/v1/agents/coach/memory", &[]).await.status, 401);
}

#[tokio::test]
async fn knowledge_pages_are_listed_read_edited_and_removed_inside_their_folder() {
    let dir = tempfile::tempdir().unwrap();
    let (config, folder) = with_agent(dir.path());
    let app = TestApp::new(config);
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    std::fs::create_dir_all(folder.join("knowledge/mail/people")).unwrap();
    std::fs::write(folder.join("knowledge/mail/index.md"), "# Index\n- [Sam](people/sam.md)\n").unwrap();
    std::fs::write(folder.join("knowledge/mail/people/sam.md"), "# Sam\nVoisin.\n").unwrap();
    std::fs::write(folder.join("secret.md"), "outside").unwrap();
    let listed = app.get("/v1/agents/coach/memory", &h).await.json()["pages"].clone();
    let paths: Vec<&str> = listed.as_array().unwrap().iter().map(|p| p["path"].as_str().unwrap()).collect();
    assert_eq!(paths, vec!["mail/index.md", "mail/people/sam.md"]);
    assert_eq!(
        app.get("/v1/agents/coach/memory/pages/mail/people/sam.md", &h).await.json()["text"],
        "# Sam\nVoisin.\n"
    );
    app.send_json(
        "PUT",
        "/v1/agents/coach/memory/pages/mail/people/sam.md",
        &h,
        &json!({"text": "# Sam\nVoisin, jardinier.\n"}),
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(folder.join("knowledge/mail/people/sam.md")).unwrap(),
        "# Sam\nVoisin, jardinier.\n"
    );
    assert_eq!(app.get("/v1/agents/coach/memory/pages/..%2F..%2Fsecret.md", &h).await.status, 404);
    assert_eq!(
        app.send_json("PUT", "/v1/agents/coach/memory/pages/mail/run.sh", &h, &json!({"text": "x"})).await.status,
        404
    );
    let removed = app.request("DELETE", "/v1/agents/coach/memory/pages/mail/people/sam.md", &h, None).await;
    assert_eq!(removed.json(), json!({"ok": true}));
    assert!(!folder.join("knowledge/mail/people/sam.md").exists());
}

fn takeout() -> String {
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut archive = zip::ZipWriter::new(&mut out);
        let options = zip::write::SimpleFileOptions::default();
        let mut put = |name: &str, data: &[u8]| {
            archive.start_file(name, options).unwrap();
            archive.write_all(data).unwrap();
        };
        put("Takeout/Keep/Courses.json", json!({"title": "Courses", "listContent": [{"text": "Lait", "isChecked": true}, {"text": "Pain", "isChecked": false}],
            "labels": [{"name": "Maison"}], "userEditedTimestampUsec": 1760000000000000i64, "isTrashed": false}).to_string().as_bytes());
        put("Takeout/Keep/Courses.html", b"<html><body>Courses</body></html>"); // the JSON wins
        put(
            "Takeout/Keep/Idee.json",
            json!({"title": "", "textContent": "Offrir un vélo à Léa", "isTrashed": false}).to_string().as_bytes(),
        );
        put(
            "Takeout/Keep/Vieux.json",
            json!({"title": "Vieux", "textContent": "x", "isTrashed": true}).to_string().as_bytes(),
        );
        put("Takeout/Keep/photo.jpg", b"\xff\xd8");
        archive.finish().unwrap();
    }
    base64::engine::general_purpose::STANDARD.encode(out.into_inner())
}

#[tokio::test]
async fn keep_s_takeout_and_text_notes_become_knowledge_pages() {
    let dir = tempfile::tempdir().unwrap();
    let (config, folder) = with_agent(dir.path());
    let notes = folder.join("knowledge/notes");
    let app = TestApp::new(config);
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let done = app
        .send_json(
            "POST",
            "/v1/agents/coach/memory/import",
            &h,
            &json!({"name": "takeout-20261020.zip", "data": takeout()}),
        )
        .await
        .json();
    assert_eq!(done, json!({"notes": 2, "skipped": 2})); // the trashed note and the photo left out
    let courses = std::fs::read_to_string(notes.join("keep/courses.md")).unwrap();
    assert!(courses.starts_with("# Courses\n") && courses.contains("- [x] Lait") && courses.contains("- [ ] Pain"));
    assert!(courses.contains("Étiquettes : Maison") && courses.contains("2025-10-09"));
    assert!(std::fs::read_to_string(notes.join("keep/offrir-un-velo-a-lea.md"))
        .unwrap()
        .contains("Offrir un vélo à Léa"));
    let text = base64::engine::general_purpose::STANDARD.encode("Penser à rappeler le plombier.");
    app.send_json(
        "POST",
        "/v1/agents/coach/memory/import",
        &h,
        &json!({"name": "Plombier.txt", "data": text, "source": "apple-notes"}),
    )
    .await;
    let index = std::fs::read_to_string(notes.join("index.md")).unwrap();
    assert!(
        index.contains("## apple-notes (1)")
            && index.contains("## keep (2)")
            && index.contains("[Courses](keep/courses.md)")
    );
    let pages = app.get("/v1/agents/coach/memory", &h).await.json()["pages"].clone();
    let paths: Vec<&str> = pages.as_array().unwrap().iter().map(|p| p["path"].as_str().unwrap()).collect();
    assert!(paths.contains(&"notes/apple-notes/plombier.md") && paths.contains(&"notes/index.md"));
    let empty =
        app.send_json("POST", "/v1/agents/coach/memory/import", &h, &json!({"name": "vide.txt", "data": ""})).await;
    assert_eq!(empty.status, 422);
    let not_zip =
        app.send_json("POST", "/v1/agents/coach/memory/import", &h, &json!({"name": "x.zip", "data": text})).await;
    assert_eq!(not_zip.status, 400);
}

fn call(config: &Config, agent: &str, name: &str, arguments: Value) -> Value {
    let arguments: Dict = serde_json::from_value(arguments).unwrap();
    Value::Object(reminders::call(config, agent, name, &arguments, None, None).unwrap())
}

#[tokio::test]
async fn the_iphone_shares_its_reminders_and_does_what_the_agents_ask() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = with_agent(dir.path());
    let app = TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(call(&config, "coach", "reminders_list", json!({}))["isError"], true); // not shared with it yet
    assert_eq!(
        app.send_json("PUT", "/v1/reminders/agents", &h, &json!({"agents": ["coach"]})).await.json(),
        json!({"agents": ["coach"]})
    );
    let snapshot = json!({"lists": [{"id": "L1", "title": "Courses"}, {"id": "L2", "title": "Maison"}],
        "items": [{"id": "r1", "list": "L1", "title": "Lait", "due": "2026-10-21"},
                  {"id": "r2", "list": "L2", "title": "Appeler le plombier", "notes": "Fuite sous l'évier"},
                  {"id": "r3", "list": "L1", "title": "Pain", "completed": true}]});
    assert_eq!(app.send_json("PUT", "/v1/reminders", &h, &snapshot).await.json(), json!({"items": 3}));
    let listed =
        call(&config, "coach", "reminders_list", json!({}))["content"][0]["text"].as_str().unwrap().to_string();
    assert!(
        listed.contains("id r1 · Courses · Lait · due 2026-10-21")
            && listed.contains("Fuite sous l'évier")
            && !listed.contains("Pain")
    );
    let courses = call(&config, "coach", "reminders_list", json!({"list": "courses", "include_done": true}))["content"]
        [0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(courses.contains("Pain") && !courses.contains("plombier"));
    let asked = call(
        &config,
        "coach",
        "reminders_add",
        json!({"title": "Rendez-vous banque", "due": "2026-10-30T10:00", "list": "Maison"}),
    );
    assert!(asked["isError"] == false && asked["content"][0]["text"].as_str().unwrap().contains("next sync"));
    assert_eq!(call(&config, "coach", "reminders_complete", json!({"id": "zz"}))["isError"], true);
    call(&config, "coach", "reminders_complete", json!({"id": "r1"}));
    let queue = app.get("/v1/reminders/queue", &h).await.json()["queue"].clone();
    let ops: Vec<(String, String)> = queue
        .as_array()
        .unwrap()
        .iter()
        .map(|op| {
            let what = op["args"].get("title").or_else(|| op["args"].get("id")).and_then(Value::as_str).unwrap_or("");
            (op["op"].as_str().unwrap().to_string(), what.to_string())
        })
        .collect();
    assert_eq!(ops, vec![("add".into(), "Rendez-vous banque".into()), ("complete".into(), "r1".into())]);
    let first = queue[0]["id"].as_str().unwrap();
    assert_eq!(
        app.send_json("POST", &format!("/v1/reminders/queue/{first}"), &h, &json!({"done": true})).await.json(),
        json!({"ok": true})
    );
    let left = app.get("/v1/reminders/queue", &h).await.json()["queue"].clone();
    assert_eq!(left.as_array().unwrap().iter().map(|op| op["op"].clone()).collect::<Vec<_>>(), vec![json!("complete")]);
    let seen = app.get("/v1/reminders", &h).await.json();
    assert_eq!((&seen["agents"], &seen["items"], &seen["pending"]), (&json!(["coach"]), &json!(3), &json!(1)));
    assert_eq!(call(&config, "vie", "reminders_add", json!({"title": "x"}))["isError"], true);
    // another agent: not shared
}
