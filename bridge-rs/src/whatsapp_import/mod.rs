//! WhatsApp chats imported into an agent (decided with Alex 2026-10-25). WhatsApp gives no automatic access and
//! BipAgents never uses unofficial clients: the person exports a chat in WhatsApp (« Exporter la discussion » › « Sans
//! médias »: a `.zip` holding `_chat.txt` on iPhone, a `.txt` on Android), shares it to BipAgents and picks an agent.
//!
//!     POST /v1/import/whatsapp/parse   {name, data (base64), agent}
//!          → {upload, chat_title, participants, participants_count, messages, media_skipped, period: {from, to},
//!             already_imported, already_imported_until, new_messages, notebook, me,
//!             estimate: {messages, calls, tokens_in, tokens_out, eur, cap_eur} | null, model: {...} | null}
//!          nothing kept but the parsed upload, for an hour (data_dir/whatsapp-import/pending/<upload>.json)
//!     POST /v1/import/whatsapp/apply   {agent, upload, fill? (true), budget_eur? (the estimate's cap)}
//!          → 202 {state, chat_title, new_messages, archive, filling, estimate}
//!     GET  /v1/import/whatsapp/status?agent=
//!          → {state: idle|filling|done|paused|failed, chat_title, progress: {done, total}, counts: {people,
//!             subjects, dates, tasks}, eur, reason, error}
//!
//! Where it goes:
//!
//! - **The Archives, in full**: the chat is written as text files of the agent's own « WhatsApp » archive (a dossier
//!   archive, archives/mod.rs), one file a month: `<profile>/imports/whatsapp/<chat>/<YYYY-MM>.md`; the index is
//!   brought up to date at once, and the agent finds it with archive_search / archive_read, on demand. Secrets
//!   (passwords, card and bank numbers) are taken out first (assistant_import::scrub).
//! - **The Carnet**: people, subjects, dates to come and tasks, by the agent's own model a window at a time (fill.rs),
//!   with the estimate shown before and a cap it never passes. Only the last `fill::FILL_DAYS` days of a first import
//!   (the rest is in the Archives); a re-import reads only what wasn't read yet.
//! - **Done**: a line in the agent's Boîte (« 12 personnes, 5 sujets, 3 dates ajoutés au Carnet »).
//!
//! A chat already imported is recognised (its name, or the last message kept found again in the new export) and only
//! its new part is added: `data_dir/whatsapp-import/<agent>.json` keeps, per chat, its last message (time and
//! fingerprint) and how far the Carnet read it. Text only: media are never sent nor kept.
//!
//! Privacy: these chats hold other people's messages. They stay on the person's own server, and are never imported
//! into an agent shared with a relative (agent_shares.rs): its memory is common, the relative could ask about them.
//!
//! No Python counterpart (owner group: tools-knowledge).

pub mod fill;
pub mod parse;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::archives::{self, records_fill, Archive};
use crate::config::{AgentConfig, Config};
use crate::notebook::{self, Part};
use crate::tools_knowledge as tk;
use crate::web::{self, ApiResult, HttpError, JsonObject, QueryMap};
use crate::{py, Dict};

use parse::{Chat, Message};

/// The export as shared, at most.
pub const MAX_BYTES: usize = 60 * (1 << 20);
/// A parsed upload waits this long for its « Importer ».
pub const PENDING_SECONDS: f64 = 3600.0;
/// The archive's name in the agent's « Archives ».
pub const ARCHIVE_NAME: &str = "WhatsApp";
/// Chats remembered per agent at most.
pub const MAX_CHATS: usize = 500;

/// The plain refusal for an agent shared with a relative (the apps show their own words for `reason: shared`).
pub const SHARED: &str = "This agent is shared with a relative: a WhatsApp chat holds other people's messages, so it \
can only go into one of your own agents.";

static LOCK: Mutex<()> = Mutex::new(());
/// Each agent's last import as the app follows it.
static STATUS: LazyLock<Mutex<HashMap<String, Value>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A message for the person (400 / 409), not a failure of the bridge.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct ImportError(pub String);

// -- where things are ------------------------------------------------------------------------------------------------

pub fn state_dir(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("whatsapp-import")
}

fn pending_path(config: &Config, upload: &str) -> Option<PathBuf> {
    (upload.len() == 24 && upload.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| state_dir(config).join("pending").join(format!("{upload}.json")))
}

fn chats_path(config: &Config, agent: &str) -> PathBuf {
    state_dir(config).join(format!("{agent}.json"))
}

/// The folder of the agent's « WhatsApp » archive (its own profile: backed up with it, an archive like any other).
pub fn folder(config: &Config, agent: &AgentConfig) -> PathBuf {
    crate::agents_admin::profile_folder(config, agent).join("imports").join("whatsapp")
}

fn write_private(path: &Path, value: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        py::makedirs(&parent.to_string_lossy(), 0o700)?;
    }
    let tmp = path.with_extension("tmp");
    py::write_with_mode(&tmp.to_string_lossy(), py::dumps(value).as_bytes(), 0o600)?;
    std::fs::rename(&tmp, path)
}

// -- the chats already imported ---------------------------------------------------------------------------------------

/// A chat imported before.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Known {
    /// Its folder in the archive.
    pub id: String,
    /// Its identity: the title folded, else its participants folded and sorted.
    pub key: String,
    pub title: String,
    pub participants: Vec<String>,
    pub messages: usize,
    /// Its last message kept: time and fingerprint.
    pub last_at: String,
    pub last_fp: String,
    /// The last message the Carnet read (empty: none).
    #[serde(default)]
    pub filled_until: String,
    pub imported: String,
}

pub fn identity(chat: &Chat) -> String {
    if !chat.title.is_empty() {
        return format!("t:{}", notebook::fold(&chat.title));
    }
    let mut people: Vec<String> = chat.participants.iter().map(|p| notebook::fold(p)).collect();
    people.sort();
    format!("p:{}", people.join("|"))
}

pub fn known(config: &Config, agent: &str) -> Vec<Known> {
    tk::read_text(&chats_path(config, agent))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("chats").cloned())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn save_known(config: &Config, agent: &str, chats: &[Known]) -> std::io::Result<()> {
    write_private(&chats_path(config, agent), &json!({"chats": chats}))
}

/// The chat imported before that this export is (its last message found again in it, or the same identity).
pub fn find_known<'a>(chats: &'a [Known], chat: &Chat) -> Option<&'a Known> {
    let fingerprints: std::collections::HashSet<String> = chat.messages.iter().map(Message::fingerprint).collect();
    chats
        .iter()
        .find(|k| !k.last_fp.is_empty() && fingerprints.contains(&k.last_fp))
        .or_else(|| chats.iter().find(|k| k.key == identity(chat)))
}

/// The messages not imported yet: after the last one kept (found by its fingerprint), else after its time.
pub fn new_part<'a>(chat: &'a Chat, known: Option<&Known>) -> &'a [Message] {
    let Some(k) = known else { return &chat.messages };
    if let Some(at) = chat.messages.iter().rposition(|m| m.fingerprint() == k.last_fp) {
        return &chat.messages[at + 1..];
    }
    let start = chat.messages.partition_point(|m| m.at.as_str() <= k.last_at.as_str());
    &chat.messages[start..]
}

/// The person's own name in the chat: their « Toi » page's names, else the other one of a two-person chat named after
/// one of them. Empty when unknown.
pub fn guess_me(config: &Config, chat: &Chat) -> String {
    let w = notebook::words(&notebook::language(config));
    if let Some(found) = notebook::find(config, &[Part::Main], &w.self_path()) {
        let mut names = found.page.aliases();
        names.push(found.page.title.clone());
        if let Some(p) = chat.participants.iter().find(|p| names.iter().any(|n| notebook::fold(n) == notebook::fold(p)))
        {
            return p.clone();
        }
    }
    if chat.participants.len() == 2 && !chat.title.is_empty() {
        let title = notebook::fold(&chat.title);
        if let Some(at) = chat.participants.iter().position(|p| notebook::fold(p) == title) {
            return chat.participants[1 - at].clone();
        }
    }
    String::new()
}

// -- the archive ------------------------------------------------------------------------------------------------------

/// The agent's « WhatsApp » archive, connected the first time.
fn ensure_archive(config: &Config, agent: &AgentConfig) -> anyhow::Result<Archive> {
    let dir = folder(config, agent);
    std::fs::create_dir_all(&dir)?;
    let real = std::fs::canonicalize(&dir).unwrap_or(dir.clone());
    let same = |a: &Archive| {
        a.kind == "folder"
            && (Path::new(&a.folder) == real || Path::new(&a.folder) == dir || Path::new(&a.source) == dir)
    };
    if let Some(found) = archives::of_agent(config, &agent.name).into_iter().find(same) {
        return Ok(found);
    }
    let mut body = Dict::new();
    body.insert("folder".into(), json!(dir.to_string_lossy()));
    body.insert("name".into(), json!(ARCHIVE_NAME));
    archives::add(config, agent, &body)
}

/// A month file's head (a note the archive reads: its title, date and people).
fn month_head(chat: &Chat, month: &str) -> String {
    let title = format!("WhatsApp · {} · {month}", chat.name());
    let people: Vec<String> = chat.participants.iter().take(10).map(|p| p.replace(['\n', ','], " ")).collect();
    format!("---\ntitle: {title}\ndate: {month}-01\nfrom: {}\nsource: whatsapp\n---\n# {title}\n\n", people.join(", "))
}

/// The new messages written in their month files (appended to a month already there). Returns the months touched.
pub fn write_months(dir: &Path, chat: &Chat, messages: &[Message]) -> std::io::Result<Vec<String>> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let mut months: Vec<String> = Vec::new();
    let mut at = 0;
    while at < messages.len() {
        let month = messages[at].month().to_string();
        let end = at + messages[at..].iter().take_while(|m| m.month() == month).count();
        let path = dir.join(format!("{month}.md"));
        let mut text = if path.exists() { String::new() } else { month_head(chat, &month) };
        for m in &messages[at..end] {
            let (clean, _) = crate::assistant_import::scrub(&m.line());
            text.push_str(&clean);
            text.push('\n');
        }
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        file.write_all(text.as_bytes())?;
        months.push(month);
        at = end;
    }
    Ok(months)
}

/// The archive reference of a month's file (`<agent>/<archive>/<16 hex>`, as archive_read takes it).
pub fn reference(archive: &Archive, chat_id: &str, month: &str) -> String {
    format!("{}/{}/{}", archive.agent, archive.id, archives::index::key_of(&format!("{chat_id}/{month}.md")))
}

/// What one import kept.
#[derive(Debug, Clone)]
pub struct Archived {
    pub archive: Archive,
    pub known: Known,
    pub new_messages: usize,
}

/// The chat's new part written in the agent's archive, the chat remembered, the index brought up to date.
pub fn archive_chat(config: &Config, agent: &AgentConfig, chat: &Chat) -> anyhow::Result<Archived> {
    let archive = ensure_archive(config, agent)?;
    let _guard = lock();
    let mut chats = known(config, &agent.name);
    let before = find_known(&chats, chat).cloned();
    let fresh = new_part(chat, before.as_ref());
    let id = match &before {
        Some(k) => k.id.clone(),
        None => {
            let mut base = archives::slug(&chat.name());
            if ["vault", "secrets"].contains(&base.as_str()) {
                base.push_str("-chat"); // names the archives never read (sent_files::secret_looking)
            }
            let mut id = base.clone();
            let mut n = 2;
            while chats.iter().any(|k| k.id == id) || folder(config, agent).join(&id).exists() {
                id = format!("{base}-{n}");
                n += 1;
            }
            id
        }
    };
    write_months(&folder(config, agent).join(&id), chat, fresh)?;
    let last = chat.last().cloned();
    let mut entry = before.clone().unwrap_or_else(|| Known { id: id.clone(), ..Known::default() });
    entry.key = identity(chat);
    entry.title = chat.name();
    entry.participants = chat.participants.iter().take(50).cloned().collect();
    entry.messages = before.as_ref().map(|k| k.messages).unwrap_or(0) + fresh.len();
    if let Some(last) = last.filter(|_| !fresh.is_empty() || before.is_none()) {
        entry.last_at = last.at.clone();
        entry.last_fp = last.fingerprint();
    }
    entry.imported = tk::now_iso_seconds();
    chats.retain(|k| k.id != entry.id);
    chats.push(entry.clone());
    let excess = chats.len().saturating_sub(MAX_CHATS);
    chats.drain(..excess);
    save_known(config, &agent.name, &chats)?;
    drop(_guard);
    // indexed now (a few files): the agent finds it at once
    if let Err(err) = archives::update(config, &archive, &crate::mailbox::default_connect()) {
        tracing::warn!(target: "bipbridge::whatsapp", agent = %agent.name, error = %err, "whatsapp archive not indexed");
    }
    tracing::info!(target: "bipbridge::whatsapp", agent = %agent.name, chat = %entry.id, new = fresh.len(), "whatsapp chat archived");
    Ok(Archived { archive, known: entry, new_messages: fresh.len() })
}

fn set_filled(config: &Config, agent: &str, chat_id: &str, until: &str) {
    let _guard = lock();
    let mut chats = known(config, agent);
    if let Some(k) = chats.iter_mut().find(|k| k.id == chat_id) {
        if until > k.filled_until.as_str() {
            k.filled_until = until.to_string();
            let _ = save_known(config, agent, &chats);
        }
    }
}

// -- the parsed upload ------------------------------------------------------------------------------------------------

fn clean_pending(config: &Config) {
    let Ok(entries) = std::fs::read_dir(state_dir(config).join("pending")) else { return };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age.as_secs_f64() > PENDING_SECONDS);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn load_pending(config: &Config, upload: &str) -> Option<(String, Chat)> {
    let path = pending_path(config, upload)?;
    let value: Value = serde_json::from_str(&tk::read_text(&path).ok()?).ok()?;
    let agent = tk::str_or_empty(value.get("agent"));
    let chat: Chat = serde_json::from_value(value.get("chat")?.clone()).ok()?;
    Some((agent, chat))
}

/// Why `agent` can't take a chat (None: it can).
pub fn refused(config: &Config, agent: &str) -> Option<&'static str> {
    (!crate::agent_shares::shared_with(config, agent).is_empty()).then_some(SHARED)
}

fn model_of(config: &Config, agent: &AgentConfig) -> Option<records_fill::Model> {
    records_fill::model_for(config, agent, "", None).ok()
}

/// What the app shows before « Importer ».
pub fn summary(config: &Config, agent: &AgentConfig, chat: &Chat, upload: &str) -> Value {
    let chats = known(config, &agent.name);
    let before = find_known(&chats, chat);
    let fresh = new_part(chat, before);
    let filled = before.map(|k| k.filled_until.clone()).unwrap_or_default();
    let notebook_on = notebook::enabled(config, &agent.name);
    let model = model_of(config, agent);
    let me = guess_me(config, chat);
    let estimate = (notebook_on && model.is_some()).then(|| {
        let todo = fill::to_fill(&chat.messages, &filled);
        fill::estimate(&chat.name(), &chat.participants, todo, model.as_ref()).view()
    });
    json!({
        "upload": upload,
        "chat_title": chat.name(),
        "participants": chat.participants.iter().take(30).collect::<Vec<_>>(),
        "participants_count": chat.participants.len(),
        "messages": chat.messages.len(),
        "media_skipped": chat.media,
        "period": {"from": chat.first().map(|m| m.at.clone()), "to": chat.last().map(|m| m.at.clone())},
        "already_imported": before.is_some(),
        "already_imported_until": before.map(|k| k.last_at.clone()),
        "new_messages": fresh.len(),
        "notebook": notebook_on,
        "me": if me.is_empty() { Value::Null } else { json!(me) },
        "estimate": estimate,
        "model": model.map(|m| m.view()),
    })
}

// -- filling the Carnet -----------------------------------------------------------------------------------------------

fn set_status(agent: &str, value: Value) {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).insert(agent.to_string(), value);
}

fn status_of(agent: &str) -> Value {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).get(agent).cloned().unwrap_or_else(|| json!({"state": "idle"}))
}

fn filling(agent: &str) -> bool {
    status_of(agent).get("state").and_then(Value::as_str) == Some("filling")
}

/// How a finished import is told (the agent's Boîte): (agent, title, text).
pub type Tell = Arc<dyn Fn(AgentConfig, String, String) -> futures::future::BoxFuture<'static, ()> + Send + Sync>;

/// The report line, in the Carnet's language (« Discussion « Léa » importée : 12 personnes, 5 sujets, 3 dates et 2
/// tâches ajoutés au Carnet. »).
pub fn report_line(lang: &str, chat: &str, counts: &Value, new_messages: usize, stopped: Option<&str>) -> String {
    let n = |k: &str| counts.get(k).and_then(Value::as_u64).unwrap_or(0);
    let (p, s, d, t) = (n("people"), n("subjects"), n("dates"), n("tasks"));
    let plural = |n: u64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let (head, parts, tail, nothing, budget) = match lang {
        "de" => (
            format!("Chat « {chat} » importiert ({new_messages} neue Nachrichten im Archiv)"),
            vec![
                plural(p, "Person", "Personen"),
                plural(s, "Thema", "Themen"),
                plural(d, "Datum", "Daten"),
                plural(t, "Aufgabe", "Aufgaben"),
            ],
            "im Notizbuch hinzugefügt",
            "nichts Neues für das Notizbuch",
            "beim Kostenlimit angehalten: teile den Chat erneut, um weiterzumachen",
        ),
        "es" => (
            format!("Chat « {chat} » importado ({new_messages} mensajes nuevos en el archivo)"),
            vec![
                plural(p, "persona", "personas"),
                plural(s, "tema", "temas"),
                plural(d, "fecha", "fechas"),
                plural(t, "tarea", "tareas"),
            ],
            "añadidos al Cuaderno",
            "nada nuevo para el Cuaderno",
            "detenido en el límite de coste: vuelve a compartir el chat para seguir",
        ),
        "en" => (
            format!("Chat « {chat} » imported ({new_messages} new messages in the archive)"),
            vec![
                plural(p, "person", "people"),
                plural(s, "subject", "subjects"),
                plural(d, "date", "dates"),
                plural(t, "task", "tasks"),
            ],
            "added to the Notebook",
            "nothing new for the Notebook",
            "stopped at the cost limit: share the chat again to go on",
        ),
        _ => (
            format!("Discussion « {chat} » importée ({new_messages} nouveaux messages dans les Archives)"),
            vec![
                plural(p, "personne", "personnes"),
                plural(s, "sujet", "sujets"),
                plural(d, "date", "dates"),
                plural(t, "tâche", "tâches"),
            ],
            "ajoutés au Carnet",
            "rien de nouveau pour le Carnet",
            "arrêté à la limite de coût : partage à nouveau la discussion pour continuer",
        ),
    };
    let mut line = if p + s + d + t == 0 {
        format!("{head} : {nothing}.")
    } else {
        format!("{head} : {} {tail}.", parts.join(", "))
    };
    if lang != "fr" {
        line = line.replace(" : ", ": ");
    }
    if stopped == Some("budget") {
        line.push(' ');
        line.push_str(&tk::capitalize(budget));
        line.push('.');
    }
    line
}

/// The Carnet filled from `chat`'s messages not read yet (blocking parts in threads). Returns the final status.
pub async fn run_fill(
    config: Arc<Config>,
    agent: AgentConfig,
    chat: Chat,
    archived: Archived,
    model: records_fill::Model,
    budget_eur: f64,
) -> Value {
    let name = chat.name();
    let me = guess_me(&config, &chat);
    let todo = fill::to_fill(&chat.messages, &archived.known.filled_until).to_vec();
    let parts = fill::windows(&todo);
    let home = fill::home_of(&name, &chat.participants, &me);
    let links: HashMap<String, String> = parts
        .iter()
        .flat_map(|w| w.months.iter().cloned())
        .map(|m| (m.clone(), reference(&archived.archive, &archived.known.id, &m)))
        .collect();
    let today_text = notebook::today();
    let today = chrono::Local::now().date_naive();
    let mut counts = fill::Counts::default();
    let (mut eur, mut failures) = (0.0, 0);
    let mut stopped: Option<&str> = None;
    let mut error: Option<String> = None;
    let free = model.price_in + model.price_out <= 0.0;
    let status = |done: usize, counts: &fill::Counts, eur: f64, state: &str| {
        json!({"state": state, "chat_title": name, "progress": {"done": done, "total": parts.len()},
               "counts": counts.view(), "eur": (eur * 10_000.0_f64).round() / 10_000.0,
               "new_messages": archived.new_messages})
    };
    set_status(&agent.name, status(0, &counts, 0.0, "filling"));
    for (n, window) in parts.iter().enumerate() {
        let user = fill::prompt_for(&name, &chat.participants, &me, window, &today_text);
        let most = model.cost(records_fill::tokens_of(fill::INSTRUCTIONS.len() + user.len()), fill::MAX_OUTPUT_TOKENS);
        if !free && eur + most > budget_eur {
            stopped = Some("budget");
            break;
        }
        let (answer, tokens_in, tokens_out) = match fill::ask(&model, &user).await {
            Ok(found) => found,
            Err(why) => {
                failures += 1;
                if failures >= fill::MAX_FAILURES || why.contains("credit") {
                    stopped = Some("error");
                    error = Some(why);
                    break;
                }
                continue;
            }
        };
        failures = 0;
        eur += model.cost(tokens_in, tokens_out);
        match fill::parse_answer(&answer, window, &me, today) {
            Ok(found) => {
                let (c, a, h, w, l, mut k) =
                    (config.clone(), agent.name.clone(), home.clone(), window.clone(), links.clone(), counts.clone());
                let written = tokio::task::spawn_blocking(move || {
                    let result = fill::write(&c, &a, &found, &h, &w, &l, &mut k);
                    (result, k)
                })
                .await;
                match written {
                    Ok((Ok(()), k)) => counts = k,
                    Ok((Err(err), _)) => {
                        stopped = Some("error");
                        error = Some(err.to_string());
                        break;
                    }
                    Err(err) => {
                        stopped = Some("error");
                        error = Some(err.to_string());
                        break;
                    }
                }
            }
            Err(why) => {
                tracing::info!(target: "bipbridge::whatsapp", why = %why, "a bad answer: window skipped");
            }
        }
        let (c, a, id, until) = (config.clone(), agent.name.clone(), archived.known.id.clone(), window.last.clone());
        let _ = tokio::task::spawn_blocking(move || set_filled(&c, &a, &id, &until)).await;
        set_status(&agent.name, status(n + 1, &counts, eur, "filling"));
    }
    let done = parts.len().min(status_of(&agent.name)["progress"]["done"].as_u64().unwrap_or(0) as usize);
    let state = match stopped {
        Some("budget") => "paused",
        Some(_) => "failed",
        None => "done",
    };
    let mut out = status(done, &counts, eur, state);
    out["reason"] = json!(stopped);
    out["error"] = json!(error);
    set_status(&agent.name, out.clone());
    tracing::info!(target: "bipbridge::whatsapp", agent = %agent.name, state = %state, eur = eur,
        people = counts.people.len(), subjects = counts.subjects.len(), dates = counts.dates, tasks = counts.tasks,
        "whatsapp chat filled the Carnet");
    out
}

// -- routes -----------------------------------------------------------------------------------------------------------

fn agent_of(config: &Config, agent_id: &str) -> ApiResult<AgentConfig> {
    crate::agents_admin::editable(config, agent_id.trim())
        .map_err(|_| HttpError::new(404, "not an agent this install manages"))
}

/// The agent a chat goes to; None when it is shared with a relative (refused before anything else is looked at).
fn target(config: &Config, agent_id: &str) -> ApiResult<Option<AgentConfig>> {
    let agent = config.agent(Some(agent_id.trim())).ok_or_else(|| HttpError::new(404, "no such agent"))?;
    if refused(config, &agent.name).is_some() {
        return Ok(None);
    }
    agent_of(config, &agent.name).map(Some)
}

fn shared_refusal() -> Response {
    web::json_response(409, &json!({"detail": SHARED, "reason": "shared"}))
}

async fn parse_route(State(state): State<AppState>, JsonObject(body): JsonObject) -> ApiResult<Response> {
    let config = state.config();
    let Some(agent) = target(&config, &tk::str_or_empty(body.get("agent")))? else {
        return Ok(shared_refusal());
    };
    let name = tk::head(&tk::str_or_empty(body.get("name")), 300).to_string();
    let data = tk::str_or_empty(body.get("data"));
    if data.len() > MAX_BYTES / 3 * 4 + 8 {
        return Err(HttpError::new(413, "60 MB at most"));
    }
    let raw = crate::notes_import::b64decode(&data).ok_or_else(|| HttpError::new(400, "data: not base64"))?;
    let out = tokio::task::spawn_blocking(move || -> ApiResult<Value> {
        let text = parse::text_of(&name, &raw).map_err(|e| HttpError::new(422, e.0))?;
        let chat = parse::parse(&text, &name).map_err(|e| HttpError::new(422, e.0))?;
        clean_pending(&config);
        let upload = py::token_hex(12);
        let path = pending_path(&config, &upload).ok_or_else(|| HttpError::new(500, "no upload id"))?;
        write_private(&path, &json!({"agent": agent.name, "chat": chat})).map_err(anyhow::Error::from)?;
        Ok(summary(&config, &agent, &chat, &upload))
    })
    .await
    .map_err(anyhow::Error::from)??;
    Ok(web::ok(out))
}

/// Mounted with the app's state: the Boîte message at the end.
async fn apply_route(State(state): State<AppState>, JsonObject(body): JsonObject) -> ApiResult<Response> {
    let config = state.config();
    let Some(agent) = target(&config, &tk::str_or_empty(body.get("agent")))? else {
        return Ok(shared_refusal());
    };
    let upload = tk::str_or_empty(body.get("upload"));
    let (owner, chat) = load_pending(&config, &upload)
        .ok_or_else(|| HttpError::new(404, "this upload is gone: share the chat again"))?;
    if owner != agent.name {
        return Err(HttpError::new(400, "this upload was made for another agent"));
    }
    if filling(&agent.name) {
        return Err(HttpError::new(409, "this agent is already importing a chat"));
    }
    crate::place_backup::refuse_when_full().await?;
    let fill_wanted = body.get("fill").map(py::truthy).unwrap_or(true);
    let (c, a, ch) = (config.clone(), agent.clone(), chat.clone());
    let archived = tokio::task::spawn_blocking(move || archive_chat(&c, &a, &ch))
        .await
        .map_err(anyhow::Error::from)?
        .map_err(|e| match e.downcast_ref::<archives::ArchiveError>() {
        Some(m) => HttpError::new(400, m.0.clone()),
        None => e.into(),
    })?;
    if let Some(path) = pending_path(&config, &upload) {
        let _ = std::fs::remove_file(path);
    }
    crate::archives::reregister(&state, &agent).await; // its tools now name « WhatsApp »
    let model = model_of(&config, &agent);
    let notebook_on = notebook::enabled(&config, &agent.name);
    let todo = fill::to_fill(&chat.messages, &archived.known.filled_until);
    let estimate = fill::estimate(&chat.name(), &chat.participants, todo, model.as_ref());
    let lang = notebook::language(&config);
    let outbox = state.outbox.clone();
    let tell: Tell = Arc::new(move |agent: AgentConfig, title: String, text: String| {
        let outbox = outbox.clone();
        Box::pin(async move {
            let mut message = Dict::new();
            message.insert("message".into(), json!(text));
            message.insert("title".into(), json!(title));
            if let Err(err) = outbox.ingest(&agent, &message, true, false).await {
                tracing::warn!(target: "bipbridge::whatsapp", agent = %agent.name, error = %err, "import message not stored");
            }
        })
    });
    let will_fill = fill_wanted && notebook_on && estimate.calls > 0 && model.is_some();
    let out = json!({"state": if will_fill { "filling" } else { "done" }, "chat_title": chat.name(),
                     "new_messages": archived.new_messages, "archive": archived.archive.id,
                     "filling": will_fill, "estimate": estimate.view()});
    if let (true, Some(model)) = (will_fill, model) {
        let budget = body.get("budget_eur").and_then(Value::as_f64).unwrap_or(estimate.cap_eur).clamp(0.0, 50.0);
        set_status(
            &agent.name,
            json!({"state": "filling", "chat_title": chat.name(),
            "progress": {"done": 0, "total": estimate.calls}, "counts": fill::Counts::default().view(), "eur": 0.0,
            "new_messages": archived.new_messages}),
        );
        let (config, agent, name) = (config.clone(), agent.clone(), chat.name());
        let new_messages = archived.new_messages;
        tokio::spawn(async move {
            let end = run_fill(config, agent.clone(), chat, archived, model, budget).await;
            let line = report_line(&lang, &name, &end["counts"], new_messages, end["reason"].as_str());
            tell(agent, "WhatsApp".into(), line).await;
        });
    } else {
        let counts = fill::Counts::default().view();
        let line = report_line(&lang, &chat.name(), &counts, archived.new_messages, None);
        set_status(
            &agent.name,
            json!({"state": "done", "chat_title": chat.name(), "progress": {"done": 0, "total": 0},
            "counts": counts, "eur": 0.0, "new_messages": archived.new_messages}),
        );
        tell(agent.clone(), "WhatsApp".into(), line).await;
    }
    Ok(web::json_response(202, &out))
}

async fn status_route(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let agent_id = web::query_str(&q, "agent", true, Some(80))?.unwrap_or_default();
    let agent = agent_of(&config, &agent_id)?;
    Ok(web::ok(status_of(&agent.name)))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/import/whatsapp/parse", post(parse_route))
        .route("/import/whatsapp/apply", post(apply_route))
        .route("/import/whatsapp/status", get(status_route))
}

#[cfg(test)]
mod tests;
