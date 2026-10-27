//! An agent's memory follows its language (2026-10-27, Alex): when the language it writes first in changes (from the
//! app's options, or the app's new default: the app's own language before the phone's), its memory — Hermes'
//! `memories/MEMORY.md` and `USER.md`, read into every conversation — may still be in the old one and pull its
//! routines and reports back to it. When the memory is clearly in another language, it is translated once with the
//! agent's own model (its router, key and model: `mail_scan::model_of`), every fact, name, date and number kept; the
//! previous files are kept next to them (`<file>.before-translation`) and put back by the undo route. Unsure (too few
//! words, mixed languages) or no model the bridge can call: nothing is touched. The personality (SOUL.md) is never
//! translated: only the bridge's own block follows the language.
//!
//!     POST /v1/agents/{id}/memory/translation/undo    {restored: [files]}
//!
//! About 1 000 tokens in and as many out for full default memories (3 575 characters): well under a cent.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::memory_edit::{folder_of, http_error, read_entries, write_entries, DELIMITER, FILES};
use crate::tools_knowledge as tk;
use crate::web::{self, ApiResult};

/// Next to each memory file, its version before the last translation.
pub const BACKUP_SUFFIX: &str = ".before-translation";

/// Words that belong to one language only among the agents' (articles, pronouns, the verbs memories use most). A
/// word two languages share (« la », « de », « en », « is »…) counts for none.
const WORDS: [(&str, &[&str]); 5] = [
    (
        "fr",
        &[
            "le",
            "les",
            "des",
            "est",
            "et",
            "une",
            "pour",
            "dans",
            "qui",
            "pas",
            "avec",
            "sur",
            "au",
            "aux",
            "il",
            "elle",
            "sa",
            "ses",
            "mais",
            "ou",
            "très",
            "aime",
            "préfère",
            "habite",
            "travaille",
            "mari",
            "enfants",
            "sont",
            "été",
            "fait",
            "être",
            "cette",
            "ce",
            "ces",
            "leur",
            "nous",
            "vous",
            "mon",
            "ma",
            "mes",
            "chez",
            "ne",
            "fille",
            "fils",
            "veut",
            "souhaite",
            "utilise",
            "jamais",
            "toujours",
        ],
    ),
    (
        "en",
        &[
            "the", "and", "are", "to", "with", "for", "on", "at", "his", "she", "he", "they", "has", "have", "were",
            "likes", "prefers", "lives", "works", "husband", "wife", "children", "this", "that", "it", "from", "by",
            "an", "not", "but", "or", "my", "your", "their", "who", "which", "user", "wants", "uses", "never",
            "always", "daughter", "should", "when",
        ],
    ),
    (
        "es",
        &[
            "el", "los", "las", "del", "y", "está", "están", "con", "para", "por", "una", "pero", "muy", "su", "sus",
            "vive", "trabaja", "prefiere", "gusta", "hijos", "esposo", "esposa", "tiene", "también", "cuando", "como",
            "usuario", "quiere", "nunca", "siempre", "hija", "hijo",
        ],
    ),
    (
        "de",
        &[
            "der",
            "das",
            "und",
            "ist",
            "sind",
            "mit",
            "für",
            "auf",
            "ein",
            "eine",
            "nicht",
            "von",
            "zu",
            "den",
            "dem",
            "im",
            "sie",
            "hat",
            "haben",
            "wohnt",
            "arbeitet",
            "bevorzugt",
            "kinder",
            "frau",
            "auch",
            "wenn",
            "oder",
            "aber",
            "sehr",
            "nutzer",
            "möchte",
            "nutzt",
            "nie",
            "immer",
            "tochter",
        ],
    ),
    (
        "nl",
        &[
            "het",
            "een",
            "van",
            "zijn",
            "voor",
            "op",
            "niet",
            "ook",
            "heeft",
            "hebben",
            "woont",
            "werkt",
            "houdt",
            "kinderen",
            "vrouw",
            "maar",
            "naar",
            "bij",
            "dat",
            "deze",
            "dit",
            "wordt",
            "graag",
            "gebruiker",
            "wil",
            "gebruikt",
            "nooit",
            "altijd",
            "dochter",
            "zoon",
        ],
    ),
];

/// Fewer words of the lists than this: too little to tell.
const MIN_HITS: usize = 8;

/// The language `text` is clearly written in, or None when unsure: at least [`MIN_HITS`] telling words, the first
/// language with 60 % of them and twice the second's.
pub fn detect(text: &str) -> Option<&'static str> {
    let mut hits = [0usize; WORDS.len()];
    for word in text.to_lowercase().split(|c: char| !(c.is_alphabetic() || c == '\'')) {
        let word = word.trim_matches('\'');
        for (i, (_, words)) in WORDS.iter().enumerate() {
            if words.contains(&word) {
                hits[i] += 1;
            }
        }
    }
    let total: usize = hits.iter().sum();
    let mut order: Vec<usize> = (0..WORDS.len()).collect();
    order.sort_by_key(|i| std::cmp::Reverse(hits[*i]));
    let (first, second) = (hits[order[0]], hits[order[1]]);
    (total >= MIN_HITS && first * 10 >= total * 6 && first >= second * 2).then_some(WORDS[order[0]].0)
}

/// The digit runs of a text, sorted (« 27/10 à 18h30 » → 10, 18, 27, 30): a translation keeps them all.
fn numbers(text: &str) -> Vec<String> {
    let mut runs: Vec<String> =
        text.split(|c: char| !c.is_ascii_digit()).filter(|r| !r.is_empty()).map(str::to_string).collect();
    runs.sort();
    runs
}

/// The memory's entries: (notes, user).
type Memory = (Vec<String>, Vec<String>);

fn entries_of(folder: &Path, which: &str) -> anyhow::Result<Vec<String>> {
    let read = read_entries(folder, which)?;
    Ok(read
        .get("entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect())
}

fn limit_of(folder: &Path, which: &str) -> usize {
    read_entries(folder, which).ok().and_then(|r| r.get("limit").and_then(Value::as_u64)).unwrap_or(0) as usize
}

pub const INSTRUCTIONS: &str = "You translate an assistant's memory about its person. You get JSON {\"notes\": [...], \
\"user\": [...]}: each entry is one remembered fact. Translate every entry into {LANGUAGE}. Keep every fact and every \
detail. Keep names of people, pets, places, organisations and products, dates, times, numbers, amounts, addresses, \
e-mail addresses, phone numbers, links and code exactly as written. An entry already in {LANGUAGE} stays as it is. \
Answer with JSON only, the same shape: the same number of entries in each list, in the same order, one translation \
per entry.";

/// The model's answer checked: the same entries, translated, every number kept, the result in `target` and within the
/// memory's limits. Err: why it is not used.
pub fn checked(answer: &str, before: &Memory, target: &str, limits: (usize, usize)) -> Result<Memory, String> {
    let answer = answer.trim();
    let (Some(start), Some(end)) = (answer.find('{'), answer.rfind('}')) else {
        return Err("the model's answer is not JSON".into());
    };
    let data: Value = serde_json::from_str(&answer[start..=end]).map_err(|_| "the model's answer is not JSON")?;
    let list = |key: &str| -> Vec<String> {
        data.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|s| tk::strip(s).to_string())
            .collect()
    };
    let after = (list("notes"), list("user"));
    for (old, new, limit, file) in
        [(&before.0, &after.0, limits.0, "MEMORY.md"), (&before.1, &after.1, limits.1, "USER.md")]
    {
        if old.len() != new.len() || new.iter().any(String::is_empty) {
            return Err(format!("{file}: the model did not give back every entry"));
        }
        if old.iter().zip(new).any(|(o, n)| numbers(o) != numbers(n)) {
            return Err(format!("{file}: a number changed in the translation"));
        }
        if tk::chars(&new.join(DELIMITER)) > limit {
            return Err(format!("{file}: the translation is longer than the memory's limit"));
        }
    }
    let text = [after.0.join("\n"), after.1.join("\n")].join("\n");
    if detect(&text).is_some_and(|found| found != target) {
        return Err("the translation is not in the agent's language".into());
    }
    Ok(after)
}

/// The previous files kept, then the translation written (each file as Hermes writes it, under its lock).
pub fn apply(folder: &Path, after: &Memory) -> anyhow::Result<()> {
    let memories = folder.join("memories");
    for (_, file, _) in FILES {
        let path = memories.join(file);
        if path.exists() {
            std::fs::copy(&path, memories.join(format!("{file}{BACKUP_SUFFIX}")))?;
        }
    }
    write_entries(folder, "notes", &after.0)?;
    write_entries(folder, "user", &after.1)?;
    Ok(())
}

/// The files as they were before the last translation, put back (their backups kept). Returns the files restored.
pub fn undo(folder: &Path) -> anyhow::Result<Vec<String>> {
    let memories = folder.join("memories");
    let mut restored = Vec::new();
    for (which, file, _) in FILES {
        let Ok(text) = tk::read_text(&memories.join(format!("{file}{BACKUP_SUFFIX}"))) else { continue };
        let entries: Vec<String> =
            text.split(DELIMITER).map(|e| tk::strip(e).to_string()).filter(|e| !e.is_empty()).collect();
        write_entries(folder, which, &entries)?;
        restored.push(file.to_string());
    }
    Ok(restored)
}

/// What [`follow`] did, for its log line.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// Empty, already in the language, or unsure.
    Kept(&'static str),
    Translated {
        from: &'static str,
        tokens_in: i64,
        tokens_out: i64,
    },
    Failed(String),
}

/// The agent's memory translated into `target` when it is clearly in another language (see the module's doc).
pub async fn follow(config: &Config, http: &reqwest::Client, agent: &AgentConfig, target: &str) -> Outcome {
    let folder = crate::agents_admin::profile_folder(config, agent);
    let read = {
        let folder = folder.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<(Memory, (usize, usize))> {
            Ok((
                (entries_of(&folder, "notes")?, entries_of(&folder, "user")?),
                (limit_of(&folder, "notes"), limit_of(&folder, "user")),
            ))
        })
        .await
    };
    let Ok(Ok((before, limits))) = read else { return Outcome::Failed("its memory could not be read".into()) };
    if before.0.is_empty() && before.1.is_empty() {
        return Outcome::Kept("its memory is empty");
    }
    let Some(from) = detect(&[before.0.join("\n"), before.1.join("\n")].join("\n")) else {
        return Outcome::Kept("its memory's language is not clear");
    };
    if from == target {
        return Outcome::Kept("its memory is already in that language");
    }
    let model = match crate::mail_scan::model_of(config, agent) {
        Ok(model) => model,
        Err(err) => return Outcome::Failed(err.0),
    };
    let language = crate::souls::language_name(target);
    let user = json!({"notes": before.0, "user": before.1}).to_string();
    let mut body = json!({"model": model["model"], "temperature": 0,
                          "max_tokens": crate::archives::records_fill::tokens_of(tk::chars(&user)) * 2 + 400,
                          "messages": [{"role": "system", "content": INSTRUCTIONS.replace("{LANGUAGE}", language)},
                                       {"role": "user", "content": user}]});
    crate::model_route::add_to_body(&config.model_provider_order, &model["base_url"], &mut body);
    let mut request =
        http.post(format!("{}/chat/completions", model["base_url"])).json(&body).timeout(Duration::from_secs(120));
    if !model["key"].is_empty() {
        request = request.header("Authorization", format!("Bearer {}", model["key"]));
    }
    let resp = match request.send().await {
        Ok(resp) => resp,
        Err(err) => return Outcome::Failed(format!("the model can't be reached ({err})")),
    };
    let status = resp.status().as_u16();
    let data: Value = resp.json().await.unwrap_or(Value::Null);
    if status >= 400 {
        return Outcome::Failed(format!("the model answered {status}"));
    }
    let answer = data.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or("");
    let tokens_in = data.pointer("/usage/prompt_tokens").and_then(Value::as_i64).unwrap_or(0);
    let tokens_out = data.pointer("/usage/completion_tokens").and_then(Value::as_i64).unwrap_or(0);
    let after = match checked(answer, &before, target, limits) {
        Ok(after) => after,
        Err(why) => return Outcome::Failed(why),
    };
    let written = tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
        // a conversation may have added an entry meanwhile: then nothing is written, the next change tries again
        if (entries_of(&folder, "notes")?, entries_of(&folder, "user")?) != before {
            return Ok(false);
        }
        apply(&folder, &after)?;
        Ok(true)
    })
    .await;
    match written {
        Ok(Ok(true)) => Outcome::Translated { from, tokens_in, tokens_out },
        Ok(Ok(false)) => Outcome::Failed("its memory changed during the translation".into()),
        Ok(Err(err)) => Outcome::Failed(format!("the translation could not be written ({err})")),
        Err(err) => Outcome::Failed(err.to_string()),
    }
}

/// After the agent's language went from `previous` to `target`: [`follow`] in the background, logged.
pub fn after_change(
    config: Arc<Config>,
    http: reqwest::Client,
    agent: AgentConfig,
    previous: Option<String>,
    target: String,
) {
    if previous.as_deref().is_none_or(|p| p == target) {
        return; // its first language (a new agent: nothing remembered yet), or no change
    }
    tokio::spawn(async move {
        match follow(&config, &http, &agent, &target).await {
            Outcome::Kept(why) => {
                tracing::info!(target: "bipbridge::memory", agent = %agent.name, language = %target, "memory not translated: {why}")
            }
            Outcome::Translated { from, tokens_in, tokens_out } => tracing::info!(
                target: "bipbridge::memory", agent = %agent.name, from, to = %target, tokens_in, tokens_out,
                "memory translated to the agent's new language (previous files kept as *{BACKUP_SUFFIX})"
            ),
            Outcome::Failed(why) => {
                tracing::warn!(target: "bipbridge::memory", agent = %agent.name, language = %target, "memory not translated: {why}")
            }
        }
    });
}

async fn undo_route(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    let folder = folder_of(&state, &agent_id)?;
    let restored =
        tokio::task::spawn_blocking(move || undo(&folder)).await.map_err(anyhow::Error::from)?.map_err(http_error)?;
    tracing::info!(target: "bipbridge::memory", agent = %agent_id, files = ?restored, "memory put back as before its translation");
    Ok(web::ok(json!({"restored": restored})))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new().route("/agents/{agent_id}/memory/translation/undo", post(undo_route))
}

#[cfg(test)]
mod tests;
