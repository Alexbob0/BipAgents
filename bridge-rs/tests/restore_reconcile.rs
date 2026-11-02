//! After a restore, the messages the phones kept put back into the agent's Hermes conversation
//! (src/restore_reconcile.rs, internal/next/restore-reconcile.md).

mod common;

use std::path::Path;

use bipbridge::config::parse_config;
use bipbridge::restore_reconcile::{
    reconcile, restore_of, visible_words, Batch, PhoneAttachment, PhoneMessage, Refusal, Restore, RESTORED,
};
use chrono_tz::Tz;
use common::{auth, config_dict, TestApp, Urls};
use rusqlite::Connection;
use serde_json::{json, Value};

/// The backup, the restore an hour and a half later.
const BACKUP: f64 = 1_793_365_200.0; // 30 Oct 2026 13:00 UTC
const RESTORED_AT: f64 = BACKUP + 5_400.0;
const ZONE: Tz = chrono_tz::Europe::Paris;

fn restore() -> Restore {
    Restore { restored_at: RESTORED_AT, backup_at: Some(BACKUP) }
}

/// Hermes' store as far as these writes go: its columns, its triggers for `message_uid` and the display order, its
/// search indexes (the older plain layout), its turn leases.
fn make_store(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT NOT NULL, parent_session_id TEXT, model_config TEXT,
             started_at REAL NOT NULL, ended_at REAL, end_reason TEXT, message_count INTEGER DEFAULT 0, title TEXT,
             hidden INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL REFERENCES sessions(id),
             role TEXT NOT NULL, content TEXT, tool_call_id TEXT, tool_calls TEXT, tool_name TEXT,
             timestamp REAL NOT NULL, token_count INTEGER, finish_reason TEXT, reasoning TEXT,
             active INTEGER NOT NULL DEFAULT 1, compacted INTEGER NOT NULL DEFAULT 0, api_content TEXT,
             display_kind TEXT, display_metadata TEXT, display_identity BLOB, display_order INTEGER,
             message_uid TEXT);
         CREATE TABLE session_turn_leases (conversation_id TEXT PRIMARY KEY, holder TEXT NOT NULL,
             acquired_at REAL NOT NULL, expires_at REAL NOT NULL);
         CREATE TRIGGER messages_message_uid_insert AFTER INSERT ON messages WHEN new.message_uid IS NULL
         BEGIN UPDATE messages SET message_uid = lower(hex(randomblob(16))) WHERE id = new.id; END;
         CREATE TRIGGER messages_display_order_insert AFTER INSERT ON messages WHEN new.display_order IS NULL
         BEGIN UPDATE messages SET display_order = COALESCE((SELECT display_order FROM messages
             WHERE session_id = new.session_id AND id <> new.id AND (active = 1 OR compacted = 1)
             AND display_identity = new.display_identity AND display_order IS NOT NULL ORDER BY display_order LIMIT 1),
             new.id) WHERE id = new.id; END;
         CREATE VIRTUAL TABLE messages_fts USING fts5(content);
         CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages BEGIN
             INSERT INTO messages_fts(rowid, content) VALUES (new.id, COALESCE(new.content, '')); END;
         CREATE TRIGGER messages_fts_delete AFTER DELETE ON messages BEGIN
             DELETE FROM messages_fts WHERE rowid = old.id; END;
         CREATE VIRTUAL TABLE messages_fts_trigram USING fts5(content, tokenize='trigram');
         CREATE TRIGGER messages_fts_trigram_insert AFTER INSERT ON messages BEGIN
             INSERT INTO messages_fts_trigram(rowid, content) VALUES (new.id, COALESCE(new.content, '')); END;
         CREATE TRIGGER messages_fts_trigram_delete AFTER DELETE ON messages BEGIN
             DELETE FROM messages_fts_trigram WHERE rowid = old.id; END;",
    )
    .unwrap();
    conn
}

fn session(conn: &Connection, id: &str, parent: Option<&str>, title: &str) {
    conn.execute(
        "INSERT INTO sessions (id, source, parent_session_id, started_at, title) VALUES (?1, 'api_server', ?2, ?3, ?4)",
        rusqlite::params![id, parent, BACKUP - 86_400.0, title],
    )
    .unwrap();
}

fn row(conn: &Connection, session: &str, role: &str, content: &str, at: f64) -> i64 {
    conn.execute(
        "INSERT INTO messages (session_id, role, content, timestamp) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![session, role, content, at],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn phone(id: &str, role: &str, text: &str, at: f64) -> PhoneMessage {
    PhoneMessage { id: id.into(), role: role.into(), text: text.into(), at, ..PhoneMessage::default() }
}

/// (role, content, timestamp) of a session's rows in Hermes' order.
fn rows(conn: &Connection, session: &str) -> Vec<(String, String, f64)> {
    let mut statement =
        conn.prepare("SELECT role, content, timestamp FROM messages WHERE session_id = ?1 ORDER BY id").unwrap();
    let found = statement
        .query_map([session], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default(), r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    found
}

fn is_note(content: &str) -> bool {
    content.starts_with("<!--bip:restore\n") && content.ends_with("-->")
}

/// The place as the backup left it, and the phone's copy: two old messages both have, then what the place lost.
fn the_lost_hour() -> Vec<PhoneMessage> {
    let mut mail = phone("12", "assistant", "C'est envoyé à Léa, avec le devis.", BACKUP + 610.25);
    mail.tools = vec!["send_email".into(), "send_email".into(), "calendar".into()];
    let mut photo = phone("13", "user", "", BACKUP + 700.0);
    photo.attachments = vec![PhoneAttachment { kind: "image".into(), name: String::new() }];
    vec![
        phone("10", "user", "Rappelle-moi le vétérinaire", BACKUP - 600.0),
        phone("11", "assistant", "C'est noté pour jeudi.", BACKUP - 590.0),
        phone("12", "user", "<!--bip:now vendredi 30 octobre 2026, 14:10-->\nEnvoie le devis à Léa", BACKUP + 600.5),
        mail,
        photo,
    ]
}

fn the_place(path: &Path) -> Connection {
    let conn = make_store(path);
    session(&conn, "s1", None, "Véto");
    row(
        &conn,
        "s1",
        "user",
        "<!--bip:now vendredi 30 octobre 2026, 13:50-->\nRappelle-moi le vétérinaire",
        BACKUP - 600.0,
    );
    row(&conn, "s1", "tool", "{\"ok\": true}", BACKUP - 595.0);
    row(&conn, "s1", "assistant", "C'est noté pour jeudi.", BACKUP - 590.0);
    conn
}

#[test]
fn the_words_compared_are_the_ones_the_person_sees() {
    assert_eq!(visible_words("<!--bip:now jeudi 29 octobre 2026, 08:40-->\n  Le   devis "), "Le devis");
    assert_eq!(
        visible_words("[OUT-OF-BAND USER MESSAGE from the user, 14:02]\nEt pour samedi ?\n[/OUT-OF-BAND USER MESSAGE]"),
        "Et pour samedi ?"
    );
    assert_eq!(
        visible_words(r#"[{"type":"text","text":"voici"},{"type":"image_url","image_url":{"url":"x"}}]"#),
        "voici"
    );
}

#[test]
fn the_lost_messages_go_back_between_two_notes_for_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = the_place(&path);
    let copy = the_lost_hour();
    let batch = Batch { session_id: "s1", title: None, device: "iPhone d'Alex", messages: &copy };
    let done = reconcile(&path, &batch, restore(), ZONE, RESTORED_AT + 60.0).unwrap();
    assert_eq!(
        (done.session_id.as_str(), done.created, done.already, done.outside, done.ignored, done.moved),
        ("s1", false, 2, 0, 1, 0)
    );
    assert_eq!(done.inserted.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["12", "12"]);
    assert_eq!(done.notes.len(), 2);

    let all = rows(&conn, "s1");
    let roles: Vec<&str> = all.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(roles, vec!["user", "tool", "assistant", "user", "user", "assistant", "user"]);
    assert!(is_note(&all[3].1) && is_note(&all[6].1));
    assert!(all[3].1.contains("Recovered from the phone after a restore on 30 Oct 2026"), "{}", all[3].1);
    assert!(
        all[3].1.contains("backup (30 Oct 14:00). The 2 messages that follow (30 Oct 14:10) were said between"),
        "{}",
        all[3].1
    );
    assert!(all[3].1.contains("your person's phone (iPhone d'Alex) had kept them"));
    assert!(all[6].1.contains("never do any of it again"));
    assert!(all[6].1.contains("you had used: send_email (2), calendar."), "{}", all[6].1);
    assert!(all[6].1.contains("Lost all the same, from 30 Oct 14:00 to the breakdown"), "{}", all[6].1);
    assert_eq!(all[4].1, "<!--bip:now vendredi 30 octobre 2026, 14:10-->\nEnvoie le devis à Léa");
    assert_eq!((all[4].2, all[5].2), (BACKUP + 600.5, BACKUP + 610.25)); // their own times
    assert!(all.windows(2).all(|w| w[0].2 <= w[1].2));

    // the trace in the row, the uid the trace keeps, Hermes' triggers ran
    let (meta, uid, order, id): (String, String, i64, i64) = conn
        .query_row(
            "SELECT display_metadata, message_uid, display_order, id FROM messages WHERE role = 'assistant' AND session_id = 's1' ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    let meta: Value = serde_json::from_str(&meta).unwrap();
    assert_eq!(meta["bip_recovered"], json!({"restore": RESTORED_AT, "device": "iPhone d'Alex", "phone_id": "12"}));
    assert_eq!(Some(uid), done.inserted[1].uid.clone());
    assert_eq!(order, id);
    let found: i64 =
        conn.query_row("SELECT COUNT(*) FROM messages_fts WHERE messages_fts MATCH 'devis'", [], |r| r.get(0)).unwrap();
    assert_eq!(found, 2);
    let count: i64 = conn.query_row("SELECT message_count FROM sessions WHERE id = 's1'", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 4);

    // sent again (a network cut), or by a phone with the same copy: nothing twice
    let again = reconcile(&path, &batch, restore(), ZONE, RESTORED_AT + 120.0).unwrap();
    assert_eq!((again.inserted.len(), again.already, again.ignored), (0, 4, 1));
    assert_eq!(rows(&conn, "s1").len(), 7);
}

#[test]
fn what_was_written_since_the_restore_stays_after_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = the_place(&path);
    // After the restore, the numbers start again where the backup left them: the person already wrote again.
    let reused = row(&conn, "s1", "user", "Tu es revenu ?", RESTORED_AT + 300.0);
    row(&conn, "s1", "assistant", "Oui, je suis là.", RESTORED_AT + 305.0);
    let uid: String = conn.query_row("SELECT message_uid FROM messages WHERE id = ?1", [reused], |r| r.get(0)).unwrap();
    let copy = the_lost_hour();
    let batch = Batch { session_id: "s1", title: None, device: "", messages: &copy };
    let done = reconcile(&path, &batch, restore(), ZONE, RESTORED_AT + 600.0).unwrap();
    assert_eq!((done.inserted.len(), done.moved), (2, 2));
    let all = rows(&conn, "s1");
    let words: Vec<String> =
        all.iter().map(|r| if is_note(&r.1) { "note".into() } else { visible_words(&r.1) }).collect();
    assert_eq!(
        words,
        vec![
            "Rappelle-moi le vétérinaire",
            "{\"ok\": true}",
            "C'est noté pour jeudi.",
            "note",
            "Envoie le devis à Léa",
            "C'est envoyé à Léa, avec le devis.",
            "note",
            "Tu es revenu ?",
            "Oui, je suis là."
        ]
    );
    let (moved_uid, moved_id): (String, i64) = conn
        .query_row("SELECT message_uid, id FROM messages WHERE content = 'Tu es revenu ?'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(moved_uid, uid); // the same message for Hermes, at a new place
    assert!(moved_id > reused);
    let indexed: i64 = conn
        .query_row("SELECT COUNT(*) FROM messages_fts WHERE messages_fts MATCH 'revenu'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(indexed, 1);
}

#[test]
fn a_second_phone_adds_only_what_is_still_missing_at_its_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = the_place(&path);
    let first = the_lost_hour();
    reconcile(
        &path,
        &Batch { session_id: "s1", title: None, device: "iPhone", messages: &first },
        restore(),
        ZONE,
        RESTORED_AT,
    )
    .unwrap();
    // The other phone: the same messages (Android: milliseconds), and one the first did not have, in between.
    let second = vec![
        phone("12", "user", "Envoie le devis à Léa", BACKUP + 600.5 - 0.0004),
        phone("90", "assistant", "Je cherche le devis…", BACKUP + 605.0),
        phone("12", "assistant", "C'est envoyé à Léa, avec le devis.", BACKUP + 610.25),
    ];
    let done = reconcile(
        &path,
        &Batch { session_id: "s1", title: None, device: "Pixel", messages: &second },
        restore(),
        ZONE,
        RESTORED_AT + 1.0,
    )
    .unwrap();
    assert_eq!((done.inserted.len(), done.already, done.moved), (1, 2, 2));
    let all = rows(&conn, "s1");
    let spoken: Vec<String> =
        all.iter().filter(|r| !is_note(&r.1) && r.0 != "tool").map(|r| visible_words(&r.1)).collect();
    assert_eq!(
        spoken,
        vec![
            "Rappelle-moi le vétérinaire",
            "C'est noté pour jeudi.",
            "Envoie le devis à Léa",
            "Je cherche le devis…",
            "C'est envoyé à Léa, avec le devis."
        ]
    );
    assert!(all.windows(2).all(|w| w[0].2 <= w[1].2), "{all:?}");
    assert_eq!(all.iter().filter(|r| is_note(&r.1)).count(), 4);
}

#[test]
fn only_the_window_between_the_backup_and_the_restore_comes_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = the_place(&path);
    let copy = vec![
        phone("5", "user", "Un message effacé avant la sauvegarde", BACKUP - 300.0),
        phone("6", "tool", "{\"secret\": 1}", BACKUP + 100.0),
        phone("7", "system", "règle", BACKUP + 100.0),
        phone("8", "user", "Écrit bien après la restauration", RESTORED_AT + 3_600.0),
    ];
    let done = reconcile(
        &path,
        &Batch { session_id: "s1", title: None, device: "", messages: &copy },
        restore(),
        ZONE,
        RESTORED_AT,
    )
    .unwrap();
    assert_eq!((done.inserted.len(), done.outside, done.ignored), (0, 2, 2));
    assert_eq!(rows(&conn, "s1").len(), 3);
    // Without the backup's time (an older backup): after the last message both sides have.
    let older = Restore { restored_at: RESTORED_AT, backup_at: None };
    let copy = vec![
        phone("10", "assistant", "C'est noté pour jeudi.", BACKUP - 590.0),
        phone("11", "user", "Merci !", BACKUP - 580.0),
    ];
    let done = reconcile(
        &path,
        &Batch { session_id: "s1", title: None, device: "", messages: &copy },
        older,
        ZONE,
        RESTORED_AT,
    )
    .unwrap();
    assert_eq!(done.inserted.len(), 1);
    assert!(rows(&conn, "s1")[3]
        .1
        .contains("restored from its last backup. The message that follows (30 Oct 13:50) was said"));
}

#[test]
fn not_while_the_agent_answers_there() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = the_place(&path);
    conn.execute(
        "INSERT INTO session_turn_leases VALUES ('s1', 'gateway', ?1, ?2)",
        rusqlite::params![RESTORED_AT, RESTORED_AT + 600.0],
    )
    .unwrap();
    let copy = the_lost_hour();
    let batch = Batch { session_id: "s1", title: None, device: "", messages: &copy };
    assert_eq!(reconcile(&path, &batch, restore(), ZONE, RESTORED_AT + 60.0).unwrap_err(), Refusal::Busy);
    assert_eq!(rows(&conn, "s1").len(), 3);
    // the lease ran out
    assert_eq!(reconcile(&path, &batch, restore(), ZONE, RESTORED_AT + 700.0).unwrap().inserted.len(), 2);
}

#[test]
fn a_compressed_conversation_goes_on_in_its_continuation_and_a_new_one_is_made_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = the_place(&path);
    conn.execute("UPDATE sessions SET end_reason = 'compression' WHERE id = 's1'", []).unwrap();
    session(&conn, "s1b", Some("s1"), "Véto");
    session(&conn, "branch", Some("s1"), "Véto (copie)");
    conn.execute(
        "UPDATE sessions SET started_at = ?1, model_config = '{\"_branched_from\": \"s1\"}' WHERE id = 'branch'",
        [BACKUP],
    )
    .unwrap();
    row(&conn, "s1b", "user", "Et le chat ?", BACKUP - 100.0);
    let mut copy = the_lost_hour();
    copy.push(phone("14", "user", "Et le chat ?", BACKUP - 100.0));
    let done = reconcile(
        &path,
        &Batch { session_id: "s1", title: None, device: "", messages: &copy },
        restore(),
        ZONE,
        RESTORED_AT,
    )
    .unwrap();
    assert_eq!((done.session_id.as_str(), done.inserted.len(), done.already), ("s1b", 2, 3));
    assert_eq!(rows(&conn, "s1b").len(), 5);
    assert_eq!(rows(&conn, "branch").len(), 0);

    // A conversation begun after the backup: made again with its id and title.
    let copy = vec![
        phone("1", "user", "Une nouvelle idée", BACKUP + 1_000.0),
        phone("2", "assistant", "Dis-moi", BACKUP + 1_001.0),
    ];
    let done = reconcile(
        &path,
        &Batch { session_id: "api_new", title: Some("Idées"), device: "", messages: &copy },
        restore(),
        ZONE,
        RESTORED_AT,
    )
    .unwrap();
    assert!(done.created);
    let (title, started): (String, f64) = conn
        .query_row("SELECT title, started_at FROM sessions WHERE id = 'api_new'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!((title.as_str(), started), ("Idées", BACKUP + 1_000.0));
    assert_eq!(rows(&conn, "api_new").len(), 4);
    // nothing to put back: no empty conversation made
    let nothing = vec![phone("1", "user", "Avant", BACKUP - 10.0)];
    let done = reconcile(
        &path,
        &Batch { session_id: "api_other", title: None, device: "", messages: &nothing },
        restore(),
        ZONE,
        RESTORED_AT,
    )
    .unwrap();
    assert!(!done.created);
    assert!(conn.query_row("SELECT 1 FROM sessions WHERE id = 'api_other'", [], |_| Ok(())).is_err());
}

#[tokio::test]
async fn the_route_takes_a_phone_s_copy_and_says_what_it_put_back() {
    let dir = tempfile::tempdir().unwrap();
    let hermes_home = dir.path().join("hermes");
    let mut data = config_dict(dir.path(), &Urls::default());
    data["agents"]["Wellness"]["host_home"] = json!(hermes_home.to_string_lossy());
    let mut config = parse_config(&data, None).unwrap();
    config.browser.language = "fr-FR".into(); // a French place: the sentence in French
    let agent = config.agents.values().next().unwrap().clone();
    let profile = bipbridge::agents_admin::profile_folder(&config, &agent);
    std::fs::create_dir_all(&profile).unwrap();
    let conn = the_place(&profile.join("state.db"));
    let data_dir = std::path::PathBuf::from(config.data_dir());
    let app = TestApp::new(config);
    let (k, v) = auth();
    let messages: Vec<Value> = the_lost_hour().iter().map(|m| serde_json::to_value(m).unwrap()).collect();
    let body = json!({"agent": agent.name, "session_id": "s1", "device": "iPhone d'Alex", "messages": messages});

    // no restore: nothing to put back
    let status = app.get("/v1/restore/reconcile", &[(k, &v)]).await;
    assert_eq!(status.json(), json!({"restore": null, "open": false, "sessions": []}));
    let refused = app.send_json("POST", "/v1/restore/reconcile", &[(k, &v)], &body).await;
    assert_eq!(refused.status, 409);
    assert_eq!(app.send_json("POST", "/v1/restore/reconcile", &[], &body).await.status, 401);

    std::fs::create_dir_all(&data_dir).unwrap();
    let now = bipbridge::py::time_time();
    std::fs::write(data_dir.join(RESTORED), json!({"restored_at": now - 60.0, "backup_at": BACKUP}).to_string())
        .unwrap();
    let restore = restore_of(&data_dir).unwrap();
    assert_eq!(restore.backup_at, Some(BACKUP));

    let done = app.send_json("POST", "/v1/restore/reconcile", &[(k, &v)], &body).await;
    assert_eq!(done.status, 200, "{}", done.text());
    let done = done.json();
    assert_eq!(done["inserted"], 2);
    assert_eq!(done["already"], 2);
    assert_eq!(done["session_id"], "s1");
    assert_eq!(done["recovered"][1], json!({"id": "12", "role": "assistant", "at": BACKUP + 610.25}));
    let sentence = done["sentence"].as_str().unwrap();
    assert!(sentence.starts_with(
        "2 messages récupérés depuis ton téléphone ; ce que tes agents ont créé ou noté après le 30/10 à "
    ));
    assert!(sentence.ends_with(" (fichiers, Carnet, mémoire) n'a pas pu être récupéré."), "{sentence}");
    assert_eq!(rows(&conn, "s1").len(), 7);

    let status = app.get("/v1/restore/reconcile?session_id=s1", &[(k, &v)]).await.json();
    assert_eq!(status["open"], true);
    assert_eq!(status["sessions"][0]["count"], 2);
    assert_eq!(status["sessions"][0]["devices"], json!(["iPhone d'Alex"]));
    assert_eq!(status["sessions"][0]["recovered"][0], json!({"at": BACKUP + 600.5, "role": "user"}));
    assert_eq!(app.get("/v1/restore/reconcile?agent=other", &[(k, &v)]).await.json()["sessions"], json!([]));

    // an agent this place doesn't have (made after the backup), too much at once
    let mut unknown = body.clone();
    unknown["agent"] = json!("nobody");
    assert_eq!(app.send_json("POST", "/v1/restore/reconcile", &[(k, &v)], &unknown).await.status, 404);
    let mut many = body.clone();
    many["messages"] = Value::Array(vec![json!({"id": "1", "role": "user", "text": "x", "at": BACKUP + 1.0}); 301]);
    assert_eq!(app.send_json("POST", "/v1/restore/reconcile", &[(k, &v)], &many).await.status, 422);

    // too long after the restore
    std::fs::write(
        data_dir.join(RESTORED),
        json!({"restored_at": now - 15.0 * 86_400.0, "backup_at": BACKUP}).to_string(),
    )
    .unwrap();
    assert_eq!(app.send_json("POST", "/v1/restore/reconcile", &[(k, &v)], &body).await.status, 409);
    app.shutdown().await;
}

/// Against a real Hermes store (made by Hermes' own `SessionDB`, read back by it after): run by hand with
/// `RECONCILE_STATE_DB=<state.db> cargo test --test restore_reconcile -- --ignored`. The store has `s1` with two
/// messages before `BACKUP` and two written after `RESTORED_AT`.
#[test]
#[ignore]
fn on_a_real_hermes_store() {
    let path = std::path::PathBuf::from(std::env::var("RECONCILE_STATE_DB").expect("RECONCILE_STATE_DB"));
    let mut copy = vec![
        phone("1", "user", "Rappelle-moi le vétérinaire", BACKUP - 600.0),
        phone("2", "assistant", "C'est noté pour jeudi.", BACKUP - 590.0),
        phone("3", "user", "Envoie le devis à Léa", BACKUP + 600.5),
        phone("4", "assistant", "C'est envoyé à Léa, avec le devis.", BACKUP + 610.25),
    ];
    copy[3].tools = vec!["send_email".into()];
    let done = reconcile(
        &path,
        &Batch { session_id: "s1", title: None, device: "iPhone", messages: &copy },
        restore(),
        ZONE,
        RESTORED_AT + 900.0,
    )
    .unwrap();
    assert_eq!((done.inserted.len(), done.already, done.moved), (2, 2, 2), "{done:?}");
}
