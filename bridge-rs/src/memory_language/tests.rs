//! Synthetic memories only, a fake model answering canned translations.

use std::sync::atomic::{AtomicUsize, Ordering};

use axum::routing::post;

use super::*;

const NOTES_FR: [&str; 2] = [
    "Elle préfère les rappels le matin, avant 8h30, et jamais le dimanche.",
    "Le loyer de 1 240 € est payé le 5 de chaque mois pour l'appartement de la rue des Lilas.",
];
const USER_FR: [&str; 1] = ["Camille habite à Lyon avec son mari Karim et ses deux enfants, Inès (9 ans) et Tom."];
const NOTES_EN: [&str; 2] = [
    "She prefers reminders in the morning, before 8:30, and never on Sundays.",
    "The rent of €1,240 is paid on the 5th of each month for the flat on rue des Lilas.",
];
const USER_EN: [&str; 1] = ["Camille lives in Lyon with her husband Karim and their two children, Inès (9) and Tom."];

fn owned(entries: &[&str]) -> Vec<String> {
    entries.iter().map(|e| e.to_string()).collect()
}

#[test]
fn every_telling_word_belongs_to_one_language() {
    for (i, (code, words)) in WORDS.iter().enumerate() {
        for (other, others) in &WORDS[i + 1..] {
            let shared: Vec<&&str> = words.iter().filter(|w| others.contains(w)).collect();
            assert!(shared.is_empty(), "{code} and {other} share {shared:?}");
        }
    }
}

#[test]
fn the_language_is_told_only_when_clear() {
    assert_eq!(detect(&[NOTES_FR.join("\n"), USER_FR.join("\n")].join("\n")), Some("fr"));
    assert_eq!(detect(&[NOTES_EN.join("\n"), USER_EN.join("\n")].join("\n")), Some("en"));
    assert_eq!(
        detect("Sie wohnt mit ihrem Mann und den Kindern in Berlin und ist sehr gern im Garten, auch im Winter."),
        Some("de")
    );
    assert_eq!(detect("Camille, Lyon, 1 240 €"), None); // too few words
                                                        // half and half: unsure
    assert_eq!(detect(&format!("{}\n{}", NOTES_FR.join("\n"), NOTES_EN.join("\n"))), None);
}

#[test]
fn an_answer_is_used_only_when_everything_is_kept() {
    let before = (owned(&NOTES_FR), owned(&USER_FR));
    let good = json!({"notes": NOTES_EN, "user": USER_EN}).to_string();
    let limits = (2200, 1375);
    assert_eq!(
        checked(&format!("```json\n{good}\n```"), &before, "en", limits).unwrap(),
        (owned(&NOTES_EN), owned(&USER_EN))
    );

    let missing = json!({"notes": [NOTES_EN[0]], "user": USER_EN}).to_string();
    assert!(checked(&missing, &before, "en", limits).unwrap_err().contains("every entry"));
    let wrong_number =
        json!({"notes": [NOTES_EN[0], NOTES_EN[1].replace("1,240", "1,420")], "user": USER_EN}).to_string();
    assert!(checked(&wrong_number, &before, "en", limits).unwrap_err().contains("number"));
    assert!(checked(&good, &before, "en", (100, 1375)).unwrap_err().contains("limit"));
    assert!(checked(&good, &before, "de", limits).unwrap_err().contains("language"));
    assert!(checked("Sorry, I can't.", &before, "en", limits).is_err());
}

/// A model at 127.0.0.1 (a local server needs no key) answering `NOTES_EN`/`USER_EN`; counts its calls.
async fn fake_model(calls: Arc<AtomicUsize>) -> String {
    async fn chat(State(calls): State<Arc<AtomicUsize>>, axum::Json(body): axum::Json<Value>) -> axum::Json<Value> {
        calls.fetch_add(1, Ordering::SeqCst);
        assert!(body.pointer("/messages/0/content").and_then(Value::as_str).unwrap_or("").contains("into English"));
        let content = json!({"notes": NOTES_EN, "user": USER_EN}).to_string();
        axum::Json(json!({"choices": [{"message": {"content": content}}],
                          "usage": {"prompt_tokens": 420, "completion_tokens": 380}}))
    }
    let app = Router::new().route("/v1/chat/completions", post(chat)).with_state(calls);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{address}/v1")
}

fn setup(dir: &Path, base: &str) -> (Config, AgentConfig, std::path::PathBuf) {
    let mut config = Config::with_key("k".repeat(32));
    config.hermes.home = dir.join("hermes").to_string_lossy().into_owned();
    let agent = AgentConfig { name: "camille".into(), display_name: "Camille".into(), ..Default::default() };
    config.agents.insert("camille".into(), agent.clone());
    let folder = crate::agents_admin::profile_folder(&config, &agent);
    std::fs::create_dir_all(folder.join("memories")).unwrap();
    std::fs::write(
        folder.join("config.yaml"),
        format!("model:\n  provider: custom\n  base_url: {base}\n  default: small\n"),
    )
    .unwrap();
    (config, agent, folder)
}

#[tokio::test]
async fn a_french_memory_is_translated_once_kept_aside_and_can_be_put_back() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let base = fake_model(calls.clone()).await;
    let (config, agent, folder) = setup(dir.path(), &base);
    write_entries(&folder, "notes", &owned(&NOTES_FR)).unwrap();
    write_entries(&folder, "user", &owned(&USER_FR)).unwrap();
    let memories = folder.join("memories");
    let original = std::fs::read_to_string(memories.join("MEMORY.md")).unwrap();
    let http = reqwest::Client::new();

    let done = follow(&config, &http, &agent, "en").await;
    assert_eq!(done, Outcome::Translated { from: "fr", tokens_in: 420, tokens_out: 380 });
    assert_eq!(entries_of(&folder, "notes").unwrap(), owned(&NOTES_EN));
    assert_eq!(entries_of(&folder, "user").unwrap(), owned(&USER_EN));
    assert_eq!(std::fs::read_to_string(memories.join(format!("MEMORY.md{BACKUP_SUFFIX}"))).unwrap(), original);
    assert!(memories.join(format!("USER.md{BACKUP_SUFFIX}")).exists());

    // already in English: the model isn't called again
    assert_eq!(follow(&config, &http, &agent, "en").await, Outcome::Kept("its memory is already in that language"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    assert_eq!(undo(&folder).unwrap(), vec!["MEMORY.md".to_string(), "USER.md".to_string()]);
    assert_eq!(entries_of(&folder, "notes").unwrap(), owned(&NOTES_FR));
    assert_eq!(entries_of(&folder, "user").unwrap(), owned(&USER_FR));
}

#[tokio::test]
async fn an_unclear_or_empty_memory_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let base = fake_model(calls.clone()).await;
    let (config, agent, folder) = setup(dir.path(), &base);
    let http = reqwest::Client::new();
    assert_eq!(follow(&config, &http, &agent, "en").await, Outcome::Kept("its memory is empty"));
    let mixed = [NOTES_FR[0], NOTES_EN[1]];
    write_entries(&folder, "notes", &owned(&mixed)).unwrap();
    assert_eq!(follow(&config, &http, &agent, "en").await, Outcome::Kept("its memory's language is not clear"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(entries_of(&folder, "notes").unwrap(), owned(&mixed));
    assert!(!folder.join("memories").join(format!("MEMORY.md{BACKUP_SUFFIX}")).exists());
    assert!(undo(&folder).unwrap().is_empty());
}
