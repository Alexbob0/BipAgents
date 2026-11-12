//! « Adopter un agent » (Alex, 2026-11-12): Kerfy's ready-made agents, inside the bridge (`assets/catalogue/*.json`,
//! embedded at build), so the catalogue needs no guichet. Contract: internal/notes/agent-create-bip.md §3.
//!
//! One file per agent: its look, the tools it switches on (those that need nothing), the tiles it uses (shown, the
//! others connected by the person), its routines' schedules, and per language (fr, en, de, es) its name, one-line
//! pitch, abilities in plain words, personality, first message and routines' names and prompts. Adopting one makes the
//! standard model bundle (templates.rs, `FORMAT`) in the person's language and imports it as any model: routines
//! paused, connectors « à approuver », health off. Its first conversation opens with its first message (welcome.rs).
//! « Maison » may be made as the family's agent instead (agent_create::family_from_card), without its routines.
//!
//!     GET  /v1/templates/catalogue?lang=fr     {templates: [item]}
//!     POST /v1/templates/import {catalogue, lang?, name?, family?}   (templates.rs' route)
//!
//! Models shared by relatives « comme modèle » travel as links (bipagents.com/m/<id>): the bridge knows none, so the
//! list holds Kerfy's only.
//!
//! No Python counterpart.

use std::sync::LazyLock;

use axum::extract::{Query, State};
use axum::response::Response;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::tools_knowledge as tk;
use crate::web::{self, ApiResult, HttpError};

/// The catalogue's languages; any other gets English.
pub const LANGUAGES: [&str; 4] = ["fr", "en", "de", "es"];

/// The catalogue, sorted by its `order`.
static ITEMS: LazyLock<Vec<Value>> = LazyLock::new(|| {
    let mut items: Vec<Value> = crate::assets::CATALOGUE
        .files()
        .filter(|f| f.path().extension().is_some_and(|e| e == "json"))
        .filter_map(|f| serde_json::from_slice(f.contents()).ok())
        .collect();
    items.sort_by_key(|i: &Value| i.get("order").and_then(Value::as_i64).unwrap_or(99));
    items
});

pub fn items() -> &'static [Value] {
    &ITEMS
}

/// The catalogue's language for `lang` (« fr-FR » → fr, « nl » → en).
pub fn language(lang: &str) -> &'static str {
    let low = lang.trim().to_lowercase();
    LANGUAGES.iter().find(|l| low.starts_with(**l)).copied().unwrap_or("en")
}

pub fn item(id: &str) -> Option<&'static Value> {
    ITEMS.iter().find(|i| i.get("id").and_then(Value::as_str) == Some(id))
}

fn words<'a>(item: &'a Value, lang: &str) -> &'a Value {
    &item["langs"][language(lang)]
}

fn s(v: &Value, key: &str) -> String {
    tk::str_or_empty(v.get(key))
}

/// A routine's schedule as people read it (« 0 7 * * 1-5 » is shown by the apps as the routines' screen does).
fn display(schedule: &Value) -> String {
    s(schedule, "expr")
}

/// One item of `GET /v1/templates/catalogue`.
pub fn view(item: &Value, lang: &str) -> Value {
    let w = words(item, lang);
    let routines: Vec<Value> = w
        .get("routines")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .zip(item.get("schedules").and_then(Value::as_array).into_iter().flatten())
        .map(|(r, sch)| json!({"name": s(r, "name"), "display": display(sch)}))
        .collect();
    let mut look = item.get("look").cloned().unwrap_or(json!({}));
    if look.get("color").is_none() {
        look["color"] = Value::Null;
    }
    json!({"id": s(item, "id"), "name": s(w, "name"), "pitch": s(w, "pitch"),
           "abilities": w.get("abilities").cloned().unwrap_or(json!([])), "look": look,
           "tools": item.get("uses").cloned().unwrap_or(json!([])), "routines": routines,
           "family": item.get("family").and_then(Value::as_bool).unwrap_or(false),
           "source": "kerfy", "languages": LANGUAGES})
}

pub fn listing(lang: &str) -> Value {
    json!({"templates": ITEMS.iter().map(|i| view(i, lang)).collect::<Vec<_>>()})
}

/// The model bundle of `item` in `lang` (templates.rs' format), named `name` (its personality follows the name).
pub fn bundle(item: &Value, lang: &str, name: Option<&str>) -> Value {
    let w = words(item, lang);
    let own = s(w, "name");
    let name = name.map(tk::squeeze).filter(|n| !n.is_empty()).unwrap_or_else(|| own.clone());
    let instructions = s(w, "instructions").replacen(&own, &name, 1);
    let routines: Vec<Value> = w
        .get("routines")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .zip(item.get("schedules").and_then(Value::as_array).into_iter().flatten())
        .map(|(r, sch)| json!({"name": s(r, "name"), "prompt": s(r, "prompt"), "schedule": sch}))
        .collect();
    json!({"format": crate::templates::FORMAT, "version": crate::templates::VERSION, "name": name,
           "lang": language(lang), "look": item.get("look").cloned().unwrap_or(json!({})),
           "intro": s(w, "pitch"), "instructions": instructions,
           "tools": item.get("tools").cloned().unwrap_or(json!({})), "routines": routines,
           "skills": [], "connectors": []})
}

/// Its first message, its name changed to `name` when the person gave another.
pub fn first_message(item: &Value, lang: &str, name: &str) -> String {
    let w = words(item, lang);
    s(w, "first_message").replacen(&s(w, "name"), name, 1)
}

/// The tiles it uses that the person connects (mail, calendar, health…), for the app to offer.
pub fn suggested(item: &Value) -> Vec<String> {
    item.get("uses")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|t| !crate::guide_team::ON_AT_CREATION.contains(t))
        .map(str::to_string)
        .collect()
}

/// The items whose words match `need` best (any word of 3 letters or more, accents kept), at most `most`.
pub fn search(need: &str, lang: &str, most: usize) -> Vec<Value> {
    let wanted: Vec<String> =
        need.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 3).map(str::to_string).collect();
    let mut scored: Vec<(usize, &Value)> = ITEMS
        .iter()
        .map(|item| {
            let all: String = LANGUAGES
                .iter()
                .map(|l| {
                    let w = words(item, l);
                    format!("{} {} {}", s(w, "name"), s(w, "pitch"), w.get("abilities").map(Value::to_string).unwrap_or_default())
                })
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            (wanted.iter().filter(|w| all.contains(w.as_str())).count(), item)
        })
        .filter(|(n, _)| *n > 0)
        .collect();
    scored.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
    scored.into_iter().take(most).map(|(_, i)| view(i, lang)).collect()
}

#[derive(Debug, Default, Deserialize)]
pub struct CatalogueQuery {
    #[serde(default)]
    pub lang: Option<String>,
}

pub async fn catalogue_route(State(state): State<AppState>, Query(q): Query<CatalogueQuery>) -> ApiResult<Response> {
    let lang = q.lang.filter(|l| !l.is_empty()).unwrap_or_else(|| crate::notebook::language(&state.config()));
    Ok(web::ok(listing(&lang)))
}

/// `POST /v1/templates/import {catalogue, lang?, name?, family?}`: one of Kerfy's agents adopted.
pub async fn adopt(state: &AppState, body: &Value) -> ApiResult<Value> {
    let id = s(body, "catalogue");
    let Some(item) = item(&id) else { return Err(HttpError::new(404, "no such agent in the catalogue")) };
    let lang = Some(s(body, "lang")).filter(|l| !l.is_empty()).unwrap_or_else(|| crate::notebook::language(&state.config()));
    let given = Some(tk::squeeze(&s(body, "name"))).filter(|n| !n.is_empty());
    if given.as_ref().is_some_and(|n| n.chars().count() > crate::templates::MAX_NAME) {
        return Err(HttpError::new(400, format!("name: 1 to {} characters", crate::templates::MAX_NAME)));
    }
    let name = given.clone().unwrap_or_else(|| s(words(item, &lang), "name"));
    let family = body.get("family").and_then(Value::as_bool).unwrap_or(false);
    if family {
        if item.get("family").and_then(Value::as_bool) != Some(true) {
            return Err(HttpError::new(400, "this agent can't be the family's"));
        }
        let pitch = s(words(item, &lang), "pitch");
        let card = crate::agent_create::FamilyCard {
            name: &name,
            role: &pitch,
            tone: "",
            lang: &lang,
            first_names: &[],
            people: None,
        };
        let agent = crate::agent_create::family_from_card(state, &card).await?;
        crate::guide::note_requested(&state.config(), &agent.name);
        let config = state.config();
        let left_out: Vec<String> = words(item, &lang)
            .get("routines")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|r| left_out_routine(&lang, &s(r, "name")))
            .collect();
        tracing::info!(target: "bipbridge::templates", agent = %agent.name, model = %id, "family agent adopted from the catalogue");
        return Ok(json!({"agent": crate::guide::created_json(&config, &agent), "connectors": [], "routines": [],
                         "skills": [], "left_out": left_out, "notes": [], "family": true,
                         "first_message": Value::Null, "suggested": []}));
    }
    let template = bundle(item, &lang, given.as_deref());
    let mut answer = crate::templates::import(state, &json!({"template": template, "source": format!("kerfy:{id}")})).await?;
    let config = state.config();
    let agent_id = answer["agent"]["id"].as_str().unwrap_or_default().to_string();
    let text = first_message(item, &lang, answer["agent"]["name"].as_str().unwrap_or(&name));
    if let Some(agent) = config.agents.get(&agent_id) {
        crate::welcome::wait_for_text(&config, agent, &lang, &text);
        crate::guide::note_requested(&config, &agent.name);
    }
    answer["family"] = json!(false);
    answer["first_message"] = json!(text);
    answer["suggested"] = json!(suggested(item));
    Ok(answer)
}

fn left_out_routine(lang: &str, name: &str) -> String {
    match language(lang) {
        "fr" => format!("la routine « {name} » (demande-la-lui dans la conversation si tu la veux)"),
        "de" => format!("die Routine „{name}“ (bitte ihn im Gespräch darum, wenn du sie willst)"),
        "es" => format!("la rutina «{name}» (pídesela en la conversación si la quieres)"),
        _ => format!("the routine “{name}” (ask it for it in the conversation if you want it)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_agents_whole_in_four_languages() {
        let ids: Vec<String> = items().iter().map(|i| s(i, "id")).collect();
        assert_eq!(ids, ["quotidien", "sante", "budget", "voyages", "cuisine", "devoirs", "papiers", "maison"]);
        for item in items() {
            let id = s(item, "id");
            assert!(crate::souls::CATEGORIES.contains(&item["look"]["category"].as_str().unwrap_or("")), "{id}");
            let schedules = item["schedules"].as_array().unwrap();
            assert!((2..=3).contains(&schedules.len()), "{id}");
            for lang in LANGUAGES {
                let w = words(item, lang);
                for key in ["name", "pitch", "instructions", "first_message"] {
                    assert!(!s(w, key).trim().is_empty(), "{id} {lang} {key}");
                }
                assert!(s(w, "instructions").contains(&s(w, "name")), "{id} {lang}: its name in its personality");
                assert!(s(w, "first_message").contains(&s(w, "name")), "{id} {lang}: its name in its first message");
                assert!(tk::chars(&s(w, "instructions")) < 500, "{id} {lang}: a short personality");
                assert!(tk::chars(&s(w, "pitch")) <= 90, "{id} {lang}: one line");
                assert_eq!(w["routines"].as_array().unwrap().len(), schedules.len(), "{id} {lang}");
                assert!((3..=4).contains(&w["abilities"].as_array().unwrap().len()), "{id} {lang}");
                // every bundle is one the import takes
                crate::templates::check_bundle(&bundle(item, lang, None)).unwrap_or_else(|e| panic!("{id} {lang}: {e}"));
            }
            for t in item["uses"].as_array().unwrap() {
                assert!(crate::guide_team::TILES.contains(&t.as_str().unwrap()), "{id}: {t}");
            }
            assert!(!item["tools"]["toolsets"].as_object().unwrap().contains_key("health"), "{id}: health stays off");
        }
        assert_eq!(items().iter().filter(|i| i["family"] == true).map(|i| s(i, "id")).collect::<Vec<_>>(), ["maison"]);
    }

    #[test]
    fn a_bundle_follows_the_name_and_the_language() {
        let cuisine = item("cuisine").unwrap();
        let b = bundle(cuisine, "fr-FR", Some("Chef Léon"));
        assert_eq!(b["name"], "Chef Léon");
        assert!(b["instructions"].as_str().unwrap().starts_with("Tu es Chef Léon,"));
        assert_eq!(b["routines"][0]["schedule"]["expr"], "0 10 * * 6");
        assert_eq!(bundle(cuisine, "nl", None)["name"], "Cooking");
        assert!(first_message(cuisine, "fr", "Chef Léon").starts_with("Bonjour, je suis Chef Léon."));
        assert_eq!(suggested(item("sante").unwrap()), ["health", "reminders"]);
        let listed = listing("de");
        assert_eq!(listed["templates"].as_array().unwrap().len(), 8);
        assert_eq!(listed["templates"][4]["name"], "Küche");
        assert_eq!(listed["templates"][4]["look"]["outfit"], "chef");
        assert_eq!(listed["templates"][0]["routines"][0]["name"], "Der Morgenüberblick");
    }

    #[test]
    fn the_guide_finds_them_by_their_words() {
        let found = |need: &str| search(need, "fr", 3).iter().map(|v| s(v, "id")).collect::<Vec<_>>();
        assert_eq!(found("recettes et courses").first().map(String::as_str), Some("cuisine"));
        assert_eq!(found("help with homework").first().map(String::as_str), Some("devoirs"));
        assert_eq!(found("impôts").first().map(String::as_str), Some("papiers"));
        assert!(found("zzz").is_empty());
    }
}
