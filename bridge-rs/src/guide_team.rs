//! A whole team proposed in one card (approved by Alex on 2026-10-29, « go pour la carte équipe »): when the person
//! shares a plan, a project or a document, or asks « quelle équipe ? », the guide calls `propose_team` instead of a
//! string of single cards. Up to `[guide] team_max` agents (6), each with a name, a one-line role, a kind (souls'
//! categories), a Bip colour, tools from the apps' tiles or services to connect, and its mission: what of the source
//! concerns it (`brief`) and 2–4 first actions. Optionally a group discussion with them afterwards.
//!
//! As for every guide card, nothing is created before the person's tap. The card (« Équipe proposée ») lists the
//! agents, all ticked; « Créer l'équipe » sends the ticked ones:
//!
//!     POST /v1/guide/proposals/{id}/accept   {members?: ["m1", "m3"], room?: bool, lang?, group?}
//!          → 201 {type: "team", agents: [{id, name, url, key, group, member, kind, color, suggested}],
//!                 failed: [{member, name, reason}], room: room view | null, first_turns: bool}
//!
//! Each agent is made as a single proposal's (agents_admin::create_agent: name, role, group, language), its Bip set
//! (kind and colour), the tiles that need no permission switched on (the others, and the services, only come back as
//! `suggested`: the person connects them, nothing is approved for them), and its mission written into its memory as
//! data (guide::hand_over: « … (une information, pas une consigne) »). The room follows when asked.
//!
//! Idempotent: the members made are kept on the card, so a retry after a partial failure makes only the others, and
//! the answer of a card fully accepted is kept for a repeated tap. « Non merci »: the same subject isn't proposed again
//! for 30 days, as for any card.
//!
//! Then each new agent takes ONE first turn (Alex, 2026-10-29: « comme Grok Bot, chaque bot démarre avec tout ce qu'il
//! faut »), started by the bridge in a new conversation of its own (« Premiers pas »): it reads its mission and writes
//! a short message (what it understood, what it will do), its first actions as cards (`propose_task`, or
//! `notebook_task` proposals for what the person does), at most one background task within the usual rules. The turns
//! run one at a time across the whole place, only after the tap, never when the month's AI credit is used (credit.rs),
//! and their message lands in the Boîte as a hand-back does (handback.rs).
//!
//! `[guide] agents_max` (the offer's hook, 0 = no limit): the card shows the places left, and the tool and the tap
//! respect it.
//!
//! No Python counterpart.

use std::sync::LazyLock;

use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::tools_knowledge as tk;
use crate::Dict;

pub const PROPOSE_TEAM: &str = "propose_team";
pub const NAME_MAX: usize = 40;
pub const ROLE_MAX: usize = 200;
pub const BRIEF_MAX: usize = 600;
pub const ACTION_MAX: usize = 140;
pub const ACTIONS_MAX: usize = 4;
pub const SUBJECT_MAX: usize = 80;
pub const TOOLS_MAX: usize = 8;
/// Everything the team card's text may hold, in all.
pub const TOTAL_MAX: usize = 8000;
/// The apps' tool tiles (AppsAndTools.swift / ToolTiles.kt).
pub const TILES: [&str; 14] = [
    "web", "weather", "browser", "calendar", "reminders", "contacts", "mail", "memory", "cronjob", "health", "file",
    "image_gen", "vision", "notebook",
];
/// The tiles switched on at creation: nothing to allow, connect or confirm (the apps' `onAtCreation`).
pub const ON_AT_CREATION: [&str; 7] = ["web", "weather", "memory", "cronjob", "vision", "image_gen", "notebook"];
/// Fully accepted cards whose answer is kept for a repeated tap.
pub const ANSWERS_KEPT: usize = 10;

/// One team accepted at a time (two phones, a double tap).
static ACCEPTING: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));
/// One first turn at a time on the whole place.
static TURNS: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// The services an agent may be offered to connect (« Se connecter avec… » and the app's catalog).
fn services() -> Vec<String> {
    let mut out: Vec<String> = crate::connections::catalog().iter().map(|s| s.id.clone()).collect();
    for item in crate::services::catalog() {
        let id = tk::str_or_empty(item.get("id"));
        if !id.is_empty() && id != "custom" && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

pub fn tool(config: &Config) -> Value {
    let mut tools: Vec<String> = TILES.iter().map(|t| t.to_string()).collect();
    tools.extend(services());
    json!({
        "name": PROPOSE_TEAM,
        "description": "Propose a team of agents in ONE card (each ticked, created with one tap), e.g. from a plan or \
    document they shared. Each gets its mission in its memory and starts on it.",
        "inputSchema": {"type": "object", "properties": {
            "subject": {"type": "string", "description": "what the team is for, short, in their language"},
            "agents": {"type": "array", "maxItems": config.guide.team_max, "items": {"type": "object", "properties": {
                "name": {"type": "string"},
                "kind": {"type": "string", "enum": crate::souls::CATEGORIES},
                "role": {"type": "string", "description": "one line, in their language"},
                "color": {"type": "string", "description": "#RRGGBB, optional"},
                "tools": {"type": "array", "items": {"type": "string", "enum": tools}},
                "brief": {"type": "string", "description": "what of the source concerns it, short"},
                "actions": {"type": "array", "items": {"type": "string"}, "description": "2 to 4 first actions"}},
                "required": ["name", "kind", "role", "brief", "actions"]}},
            "room": {"type": "boolean", "description": "a group discussion with them after"}},
            "required": ["subject", "agents"]},
    })
}

fn text(v: Option<&Value>, most: usize) -> Result<String, ()> {
    let t = tk::squeeze(&tk::str_or_empty(v));
    if tk::chars(&t) > most {
        return Err(());
    }
    Ok(t)
}

/// A secret, a token in a link or an account number: never on a card nor in a memory.
fn secret_in(text: &str) -> bool {
    crate::templates_scrub::scan(text).iter().any(|f| matches!(f.kind, "key" | "url_token" | "iban"))
}

/// `#RRGGBB` in capitals, None when not given. Err: not a colour.
fn color_of(v: Option<&Value>) -> Result<Option<String>, ()> {
    let c = tk::str_or_empty(v).trim().to_string();
    if c.is_empty() {
        return Ok(None);
    }
    if c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|h| h.is_ascii_hexdigit()) {
        return Ok(Some(c.to_uppercase()));
    }
    Err(())
}

/// The agents this place may still get (None: no limit).
pub fn places_left(config: &Config) -> Option<usize> {
    let most = config.guide.agents_max;
    (most > 0).then(|| {
        let have = config.agents.values().filter(|a| a.display_name.trim() != crate::cloud_setup::PREPARED_NAME).count();
        most.saturating_sub(have)
    })
}

/// The card's fields from the guide's arguments, every one checked. Err: what to tell the guide.
pub fn build(config: &Config, arguments: &Dict) -> Result<Dict, String> {
    let subject = text(arguments.get("subject"), SUBJECT_MAX).map_err(|_| format!("subject: {SUBJECT_MAX} characters at most"))?;
    if subject.is_empty() {
        return Err("give the team's subject".into());
    }
    let given = arguments.get("agents").and_then(Value::as_array).cloned().unwrap_or_default();
    let most = config.guide.team_max;
    if given.len() < 2 || given.len() > most {
        return Err(format!("a team has 2 to {most} agents (one agent: propose_agent)"));
    }
    if let Some(left) = places_left(config) {
        if given.len() > left {
            return Err(format!("only {left} more agent(s) fit on their place: propose fewer"));
        }
    }
    let allowed = services();
    let mut names: Vec<String> = Vec::new();
    let mut members: Vec<Value> = Vec::new();
    let mut total = tk::chars(&subject);
    for (n, item) in given.iter().enumerate() {
        let Some(item) = item.as_object() else { return Err("each agent is an object".into()) };
        let name = text(item.get("name"), NAME_MAX).map_err(|_| format!("name: {NAME_MAX} characters at most"))?;
        let role = text(item.get("role"), ROLE_MAX).map_err(|_| format!("{name}: role in one line"))?;
        let kind = tk::str_or_empty(item.get("kind"));
        if name.is_empty() || role.is_empty() || !crate::souls::CATEGORIES.contains(&kind.as_str()) {
            return Err("each agent needs a name, a kind from the list and a one-line role".into());
        }
        let low = name.to_lowercase();
        if names.contains(&low) {
            return Err(format!("two agents named {name}: give each its own name"));
        }
        if crate::guide::find_agent(config, &name).is_some() {
            return Err(format!("they already have an agent named {name}: leave it out or name it otherwise"));
        }
        names.push(low);
        let color = color_of(item.get("color")).map_err(|_| format!("{name}: color as #RRGGBB"))?;
        let mut tools: Vec<String> = Vec::new();
        for t in item.get("tools").and_then(Value::as_array).into_iter().flatten() {
            let t = tk::str_or_empty(Some(t));
            if !TILES.contains(&t.as_str()) && !allowed.contains(&t) {
                return Err(format!("{name}: unknown tool {}", tk::head(&t, 40)));
            }
            if !tools.contains(&t) {
                tools.push(t);
            }
        }
        if tools.len() > TOOLS_MAX {
            return Err(format!("{name}: {TOOLS_MAX} tools at most"));
        }
        let brief = text(item.get("brief"), BRIEF_MAX).map_err(|_| format!("{name}: brief in {BRIEF_MAX} characters"))?;
        let mut actions: Vec<String> = Vec::new();
        for a in item.get("actions").and_then(Value::as_array).into_iter().flatten() {
            let a = text(Some(a), ACTION_MAX).map_err(|_| format!("{name}: each action in {ACTION_MAX} characters"))?;
            if !a.is_empty() {
                actions.push(a);
            }
        }
        if brief.is_empty() || actions.is_empty() || actions.len() > ACTIONS_MAX {
            return Err(format!("{name}: a short brief and 2 to {ACTIONS_MAX} first actions"));
        }
        let all = [name.as_str(), role.as_str(), brief.as_str()].into_iter().chain(actions.iter().map(String::as_str));
        let words: Vec<&str> = all.collect();
        if words.iter().any(|t| secret_in(t)) || secret_in(&subject) {
            return Err("leave out keys, passwords, tokens and account numbers".into());
        }
        total += words.iter().map(|t| tk::chars(t)).sum::<usize>();
        members.push(json!({"id": format!("m{}", n + 1), "name": name, "kind": kind, "role": role, "color": color,
                            "tools": tools, "mission": {"brief": brief, "actions": actions}}));
    }
    if total > TOTAL_MAX {
        return Err("too long: shorter briefs".into());
    }
    let room = arguments.get("room").and_then(Value::as_bool).unwrap_or(false);
    let mut out = Dict::new();
    out.insert("type".into(), json!("team"));
    out.insert("subject".into(), json!(subject));
    out.insert("members".into(), json!(members));
    out.insert("room".into(), json!(room));
    out.insert("done".into(), json!({}));
    Ok(out)
}

/// The subject a « Non merci » keeps from coming back.
pub fn key(p: &Dict) -> String {
    format!("team:{}", tk::squeeze(&tk::str_or_empty(p.get("subject"))).to_lowercase())
}

/// The mission as it goes into the agent's memory (data, through guide::hand_over).
pub fn mission_text(member: &Value, lang: &str) -> String {
    let brief = tk::str_or_empty(member.pointer("/mission/brief"));
    let actions: Vec<String> = member
        .pointer("/mission/actions")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|x| tk::str_or_empty(Some(x))).collect())
        .unwrap_or_default();
    let suggested: Vec<String> = member
        .get("tools")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|x| tk::str_or_empty(Some(x))).filter(|t| !ON_AT_CREATION.contains(&t.as_str())).collect())
        .unwrap_or_default();
    let fr = crate::souls::language(Some(lang)) == "fr";
    let numbered: Vec<String> = actions.iter().enumerate().map(|(n, a)| format!("{}. {a}", n + 1)).collect();
    let mut out = if fr {
        format!("Ta mission : {brief} Premières actions : {}.", numbered.join(" ; "))
    } else {
        format!("Your mission: {brief} First actions: {}.", numbered.join("; "))
    };
    if !suggested.is_empty() {
        out.push_str(&if fr {
            format!(" À brancher par ton utilisateur : {}.", suggested.join(", "))
        } else {
            format!(" For your user to connect: {}.", suggested.join(", "))
        });
    }
    out
}

/// The first conversation's title.
pub fn first_title(lang: &str) -> &'static str {
    match crate::souls::language(Some(lang)) {
        "fr" => "Premiers pas",
        "de" => "Erste Schritte",
        "es" => "Primeros pasos",
        _ => "First steps",
    }
}

/// The hidden note of the first turn (the rules travel in it, as hand-backs do: the prompt cache stays).
pub fn first_note(from: &str) -> String {
    format!(
        "<!--bip:team\nYou were just created by {from}, with a team, for your person; your mission is in your memory. \
         Write ONE short first message now, in their language, plain words: what you understood (one sentence), what \
         you will do. Then the first actions as cards, not prose: propose_task for work you can do, notebook_task \
         action propose for what they do themselves. Start at most one background task, only if the mission clearly \
         calls for it.\n-->"
    )
}

/// The message when the agent can't write it.
pub fn fallback(lang: &str, name: &str, member: &Value) -> String {
    let brief = tk::str_or_empty(member.pointer("/mission/brief"));
    let actions: Vec<String> = member
        .pointer("/mission/actions")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|x| format!("- {}", tk::str_or_empty(Some(x)))).collect())
        .unwrap_or_default();
    let (hello, first) = match crate::souls::language(Some(lang)) {
        "fr" => (format!("Bonjour, je suis {name}."), "Pour commencer :"),
        "de" => (format!("Hallo, ich bin {name}."), "Zum Anfang:"),
        "es" => (format!("Hola, soy {name}."), "Para empezar:"),
        _ => (format!("Hello, I'm {name}."), "To begin:"),
    };
    format!("{hello} {brief}\n\n{first}\n{}", actions.join("\n"))
}

fn member_of<'a>(proposal: &'a Dict, id: &str) -> Option<&'a Value> {
    proposal.get("members").and_then(Value::as_array)?.iter().find(|m| tk::str_or_empty(m.get("id")) == id)
}

fn members(proposal: &Dict) -> Vec<Value> {
    proposal.get("members").and_then(Value::as_array).cloned().unwrap_or_default()
}

/// Its look, its tools that need nothing, its mission. Failures here leave a note, never the agent undone.
async fn finish(state: &AppState, agent: &AgentConfig, member: &Value, from: &str, lang: &str) -> Vec<String> {
    let mut notes = Vec::new();
    let kind = tk::str_or_empty(member.get("kind"));
    let color = member.get("color").and_then(Value::as_str).map(str::to_string);
    let look = crate::voices::AgentAppearance { category: kind, color, outfit: None, costumes: None };
    if crate::voices::set_agent_appearance(
        axum::extract::State(state.clone()),
        axum::extract::Path(agent.name.clone()),
        crate::web::Json(look),
    )
    .await
    .is_err()
    {
        notes.push("look".to_string());
    }
    let tools: Vec<String> = member
        .get("tools")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|x| tk::str_or_empty(Some(x))).collect())
        .unwrap_or_default();
    let toolsets: indexmap::IndexMap<String, crate::agents_admin::LaxBool> = tools
        .iter()
        .filter(|t| ON_AT_CREATION.contains(&t.as_str()) && *t != "weather" && *t != "notebook")
        .map(|t| (t.clone(), crate::agents_admin::LaxBool(true)))
        .collect();
    let weather = tools.iter().any(|t| t == "weather");
    if !toolsets.is_empty() || weather {
        let settings = crate::agents_admin::AgentSettings {
            toolsets: Some(toolsets).filter(|t| !t.is_empty()),
            weather: weather.then_some(true),
            ..Default::default()
        };
        let config = state.config();
        let agent = config.agent(Some(&agent.name)).cloned().unwrap_or_else(|| agent.clone());
        if let Err(exc) =
            crate::agents_admin::write_settings(&state.config, &agent, &settings, &crate::agents_admin::default_run()).await
        {
            tracing::warn!(target: "bipbridge::guide", agent = %agent.name, error = %tk::head(&exc.to_string(), 200), "team agent's tools not set");
            notes.push("tools".to_string());
        }
    }
    let config = state.config();
    let agent = config.agent(Some(&agent.name)).cloned().unwrap_or_else(|| agent.clone());
    if let Err(exc) = crate::guide::hand_over(&config, &agent, from, &mission_text(member, lang), lang) {
        tracing::warn!(target: "bipbridge::guide", agent = %agent.name, error = %exc, "team agent's mission not kept");
        notes.push("mission".to_string());
    }
    notes
}

fn failed_reason(exc: &anyhow::Error) -> String {
    match exc.downcast_ref::<crate::agents_admin::AgentError>() {
        Some(e) => e.0.clone(),
        None => "it could not be created: try again in a moment".into(),
    }
}

/// « Créer l'équipe ». `chosen`: the members ticked (None: all), `room`: the card's choice unless the app says.
pub async fn accept(
    state: &AppState,
    id: &str,
    from: &AgentConfig,
    chosen: Option<Vec<String>>,
    room: Option<bool>,
    group: Option<String>,
    lang: &str,
) -> crate::web::ApiResult<axum::response::Response> {
    use crate::web::HttpError;
    let _turn = ACCEPTING.lock().await;
    let config = state.config();
    let guide_state = crate::guide::load(&config);
    if let Some(answer) = guide_state.team_answers.get(id) {
        return Ok(crate::web::ok(answer.clone()));
    }
    let Some(proposal) = guide_state.proposals.into_iter().find(|p| tk::str_or_empty(p.get("id")) == id) else {
        return Err(HttpError::new(404, "no such proposal"));
    };
    if !config.hermes.enabled() {
        return Err(HttpError::new(409, "this install does not manage its agents (set up by hand)"));
    }
    let all = members(&proposal);
    let wanted: Vec<String> = match chosen {
        Some(list) => all
            .iter()
            .map(|m| tk::str_or_empty(m.get("id")))
            .filter(|m| list.contains(m))
            .collect(),
        None => all.iter().map(|m| tk::str_or_empty(m.get("id"))).collect(),
    };
    if wanted.is_empty() {
        return Err(HttpError::new(400, "tick at least one agent"));
    }
    let mut done: Dict = proposal.get("done").and_then(Value::as_object).cloned().unwrap_or_default();
    let missing = wanted.iter().filter(|m| !done.contains_key(*m)).count();
    if let Some(left) = places_left(&config) {
        if missing > left {
            return Err(HttpError::new(409, format!("only {left} more agent(s) fit on this place")));
        }
    }
    let mut failed: Vec<Value> = Vec::new();
    for mid in &wanted {
        if done.contains_key(mid) {
            continue;
        }
        let Some(member) = member_of(&proposal, mid).cloned() else { continue };
        let name = tk::str_or_empty(member.get("name"));
        let request = crate::agents_admin::NewAgent {
            name: name.clone(),
            kind: Some(tk::str_or_empty(member.get("kind"))),
            lang: Some(lang.to_string()),
            group: group.clone(),
            role: Some(tk::str_or_empty(member.get("role"))),
            ..Default::default()
        };
        let made = {
            let _guard = crate::agents_admin::LOCK.lock().await;
            crate::agents_admin::create_agent(&state.config, &request, &state.http, &crate::agents_admin::default_run())
                .await
        };
        match made {
            Ok(agent) => {
                done.insert(mid.clone(), json!(agent.name));
                // kept at once: a retry after a later failure makes only the others
                let kept = done.clone();
                crate::guide::change(&state.config(), |s| {
                    if let Some(p) = s.proposals.iter_mut().find(|p| tk::str_or_empty(p.get("id")) == id) {
                        p.insert("done".into(), Value::Object(kept));
                    }
                })?;
                finish(state, &agent, &member, &from.display_name, lang).await;
            }
            Err(exc) => {
                tracing::warn!(target: "bipbridge::guide", member = %mid, error = %tk::head(&exc.to_string(), 200), "team agent not created");
                failed.push(json!({"member": mid, "name": name, "reason": failed_reason(&exc)}));
            }
        }
    }
    let config = state.config();
    let mut agents: Vec<Value> = Vec::new();
    let mut made: Vec<(AgentConfig, Value)> = Vec::new();
    for mid in &wanted {
        let Some(agent_id) = done.get(mid).and_then(Value::as_str) else { continue };
        let (Some(agent), Some(member)) = (config.agents.get(agent_id), member_of(&proposal, mid)) else { continue };
        let mut view = crate::guide::created_json(&config, agent);
        if let Some(o) = view.as_object_mut() {
            o.insert("member".into(), json!(mid));
            o.insert("kind".into(), member.get("kind").cloned().unwrap_or(Value::Null));
            o.insert("color".into(), json!(agent.color));
            let suggested: Vec<Value> = member
                .get("tools")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter(|t| !ON_AT_CREATION.contains(&t.as_str().unwrap_or(""))).cloned().collect())
                .unwrap_or_default();
            o.insert("suggested".into(), json!(suggested));
        }
        agents.push(view);
        made.push((agent.clone(), member.clone()));
    }
    if agents.is_empty() {
        let reason = failed.first().and_then(|f| f.get("reason")).and_then(Value::as_str).unwrap_or("try again");
        return Err(HttpError::new(502, format!("the team could not be created: {reason}")));
    }
    let mut room_view = Value::Null;
    if failed.is_empty() && room.unwrap_or_else(|| proposal.get("room").and_then(Value::as_bool).unwrap_or(false)) {
        let mut given: Vec<String> = made.iter().map(|(a, _)| a.name.clone()).collect();
        if given.len() < crate::rooms::PARTICIPANTS_MIN {
            given.push(from.name.clone());
        }
        given.truncate(crate::rooms::PARTICIPANTS_MAX);
        let subject = tk::str_or_empty(proposal.get("subject"));
        match crate::rooms::create_room(state, &given, Some(&subject), None).await {
            Ok(room) => room_view = crate::rooms::room_view(&state.config(), &room, Some(&*state.hub)),
            Err(_) => failed.push(json!({"member": "room", "name": subject, "reason": "the discussion could not be opened"})),
        }
    }
    let turns = config.guide.first_turns;
    let answer = json!({"type": "team", "agents": agents, "failed": failed, "room": room_view,
                        "first_turns": config.guide.first_turns});
    if failed.is_empty() {
        let kept = answer.clone();
        crate::guide::change(&state.config(), |s| {
            s.proposals.retain(|p| tk::str_or_empty(p.get("id")) != id);
            s.team_answers.insert(id.to_string(), kept);
            while s.team_answers.len() > ANSWERS_KEPT {
                let Some(oldest) = s.team_answers.keys().next().cloned() else { break };
                s.team_answers.remove(&oldest);
            }
        })?;
    }
    crate::guide::refresh(&state.config());
    if turns {
        // only the agents made by this tap (a retry doesn't start the earlier ones again)
        let fresh: Vec<(AgentConfig, Value)> = made
            .into_iter()
            .filter(|(a, _)| !proposal.get("done").and_then(Value::as_object).is_some_and(|d| d.values().any(|v| v == &json!(a.name))))
            .collect();
        spawn_first_turns(state.clone(), id.to_string(), fresh, from.display_name.clone(), lang.to_string());
    }
    Ok(crate::web::json_response(201, &answer))
}

/// The first turns, in the background, one at a time on the whole place.
pub fn spawn_first_turns(state: AppState, card: String, agents: Vec<(AgentConfig, Value)>, from: String, lang: String) {
    if agents.is_empty() {
        return;
    }
    tokio::spawn(async move {
        for (agent, member) in agents {
            let _one = TURNS.lock().await;
            if crate::credit::now_at(None).paused() {
                tracing::info!(target: "bipbridge::guide", agent = %agent.name, "first turn skipped: the month's credit is used");
                continue;
            }
            first_turn(&state, &card, &agent, &member, &from, &lang).await;
        }
    });
}

async fn first_turn(state: &AppState, card: &str, agent: &AgentConfig, member: &Value, from: &str, lang: &str) {
    let title = first_title(lang);
    // a new agent's Hermes may take a moment to answer
    let mut session = None;
    for _ in 0..20 {
        let agent = state.config().agent(Some(&agent.name)).cloned().unwrap_or_else(|| agent.clone());
        match state.hermes.create_session(&agent, title).await {
            Ok(id) => {
                session = Some(id);
                break;
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_secs(3)).await,
        }
    }
    let Some(session) = session else {
        tracing::warn!(target: "bipbridge::guide", agent = %agent.name, "first turn: no conversation could be opened");
        return;
    };
    let agent = state.config().agent(Some(&agent.name)).cloned().unwrap_or_else(|| agent.clone());
    let mut handback = crate::handback::Handback::of(state);
    handback.poll = 1.0;
    handback.turn_max = 240.0;
    let request = crate::handback::Request {
        agent: agent.clone(),
        session,
        key: format!("team:{card}:{}", agent.name),
        title: title.to_string(),
        note: first_note(from),
        links: Vec::new(),
        budget: None,
        fallback: fallback(lang, &agent.display_name, member),
        recap: None,
        fallback_session: None,
    };
    handback.deliver(&request).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::parse_config(
            &json!({"bridge_key": "k".repeat(32), "data_dir": dir.path().to_string_lossy()}),
            None,
        )
        .unwrap();
        config.agents.insert(
            "bip".into(),
            AgentConfig { name: "bip".into(), display_name: "Bip".into(), ..AgentConfig::default() },
        );
        (dir, config)
    }

    fn member(name: &str) -> Value {
        json!({"name": name, "kind": "work", "role": "Suit les ventes", "brief": "Le plan vise 200 abonnés en mars.",
               "actions": ["Lister les prospects", "Préparer un mail"], "tools": ["web", "mail"]})
    }

    fn args(agents: Vec<Value>) -> Dict {
        json!({"subject": "Plan d'affaires", "agents": agents, "room": true}).as_object().unwrap().clone()
    }

    #[test]
    fn a_team_is_checked_whole() {
        let (_d, mut config) = config();
        let card = build(&config, &args(vec![member("Ventes"), member("Compta")])).unwrap();
        let members = card["members"].as_array().unwrap();
        assert_eq!(members.len(), 2);
        assert_eq!(members[1]["id"], "m2");
        assert_eq!(members[0]["mission"]["actions"][1], "Préparer un mail");
        assert_eq!(key(&card), "team:plan d'affaires");
        // one agent, or more than team_max
        assert!(build(&config, &args(vec![member("Ventes")])).is_err());
        let seven: Vec<Value> = (0..7).map(|n| member(&format!("A{n}"))).collect();
        assert!(build(&config, &args(seven)).unwrap_err().contains("2 to 6"));
        config.guide.team_max = 3;
        let four: Vec<Value> = (0..4).map(|n| member(&format!("A{n}"))).collect();
        assert!(build(&config, &args(four)).is_err());
        // names unique, among them and against theirs
        assert!(build(&config, &args(vec![member("Ventes"), member("ventes")])).unwrap_err().contains("two agents"));
        assert!(build(&config, &args(vec![member("Bip"), member("Compta")])).unwrap_err().contains("already"));
        // kinds, tools, colours, sizes
        let mut bad = member("X");
        bad["kind"] = json!("magic");
        assert!(build(&config, &args(vec![bad, member("Y")])).is_err());
        let mut bad = member("X");
        bad["tools"] = json!(["rm_rf"]);
        assert!(build(&config, &args(vec![bad, member("Y")])).unwrap_err().contains("unknown tool"));
        let mut bad = member("X");
        bad["color"] = json!("red");
        assert!(build(&config, &args(vec![bad, member("Y")])).is_err());
        let mut ok = member("X");
        ok["color"] = json!("#a1b2c3");
        ok["tools"] = json!(["notion", "web"]);
        assert_eq!(build(&config, &args(vec![ok, member("Y")])).unwrap()["members"][0]["color"], "#A1B2C3");
        let mut bad = member("X");
        bad["brief"] = json!("x".repeat(BRIEF_MAX + 1));
        assert!(build(&config, &args(vec![bad, member("Y")])).is_err());
        let mut bad = member("X");
        bad["actions"] = json!(["a", "b", "c", "d", "e"]);
        assert!(build(&config, &args(vec![bad, member("Y")])).is_err());
        // no secrets
        let mut bad = member("X");
        bad["brief"] = json!("La clé est sk-abcdefghijklmnopqrstuvwxyz0123");
        assert!(build(&config, &args(vec![bad, member("Y")])).unwrap_err().contains("keys"));
        // the place's limit
        config.guide.team_max = 6;
        config.guide.agents_max = 2;
        assert_eq!(places_left(&config), Some(1));
        assert!(build(&config, &args(vec![member("A"), member("B")])).unwrap_err().contains("only 1"));
        config.guide.agents_max = 0;
        assert_eq!(places_left(&config), None);
    }

    #[test]
    fn the_mission_is_data_with_its_actions() {
        let (_d, config) = config();
        let card = build(&config, &args(vec![member("Ventes"), member("Compta")])).unwrap();
        let m = &card["members"][0];
        let text = mission_text(m, "fr");
        assert_eq!(
            text,
            "Ta mission : Le plan vise 200 abonnés en mars. Premières actions : 1. Lister les prospects ; 2. Préparer \
             un mail. À brancher par ton utilisateur : mail."
        );
        assert!(mission_text(m, "en").starts_with("Your mission: "));
        assert!(first_note("Bip").starts_with("<!--bip:team\n") && first_note("Bip").ends_with("-->"));
        assert!(first_note("Bip").contains("propose_task"));
        assert_eq!(first_title("fr-FR"), "Premiers pas");
        assert!(fallback("fr", "Ventes", m).contains("- Lister les prospects"));
    }
}
