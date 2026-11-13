//! A site's terms or consent box, ticked with the person's « Oui » (Alex, 13/11: « une case CGU passe par une carte
//! Oui/Non », no longer by the person taking over the browser). The agents' `accept_terms`, on the vault's MCP entry
//! (vault_logins.rs, same token), for the agents with their own browser.
//!
//! 1. the agent meets a box accepting the site's terms (or giving a consent: privacy policy, data use) in a form it
//!    fills; its browser refuses to tick it itself (browser_mcp.rs `guard`, `TERMS_REFUSED`, `TERMS_UNDONE`). It calls
//!    `accept_terms(site, box, terms_url?)` with the box's label as the page shows it;
//! 2. the bridge checks the page open in the agent's browser is on that site (Chrome's own address) and finds there
//!    exactly one box not ticked with that label, a terms or consent box (a newsletter's is not: the agent ticks such a
//!    box itself when the person asked);
//! 3. ONE card for the person, listed with the approvals (`GET /v1/approvals`, run id `bip-terms-<id>`, choices
//!    `once`/`deny`, no « Toujours »: approval_level.rs knows no such kind) and pushed: « Vie veut accepter les
//!    conditions de shop.example : « J'accepte les conditions générales de vente » », the terms' link when the page
//!    has one. Answered in the app (`POST /v1/approve`);
//! 4. « Oui »: the bridge ticks that box itself, in that page, over DevTools; nothing else is unlocked (the agent's
//!    browser still refuses any terms box). « Non »: the agent stops that sign-up and says so. No answer within
//!    `ASK_TIMEOUT`: nothing ticked, the agent says it waits;
//! 5. a turn nobody follows (a routine, a watcher, a background task: runs.rs `nobody_there_for`): the card is held
//!    like the night's other approvals (held_approvals.rs: up to 24 h, or the routine's next run); the turn ends with
//!    its report. « Oui » later grants exactly that box on that site once (`GRANT_SECONDS`) and the agent goes on in
//!    that conversation: its next `accept_terms` for it ticks the box without a new card.
//!
//! Captchas stay the person's (they take over the browser); payments go through pay_with_card (« Oui, payer »).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::app::AppState;
use crate::card_fill::{self, FillError, Page};
use crate::config::{AgentConfig, Config};
use crate::tools_knowledge::tool_text;
use crate::{py, Dict};

pub const TOOL: &str = "accept_terms";
/// The run id of a card (the apps show it as any card of the bridge's).
pub const RUN_PREFIX: &str = "bip-terms-";
/// How long the person has to answer while the agent's turn waits, by default (`[browser] consent_seconds`, at most 540:
/// Hermes waits 600 s for the vault's tools).
pub const ASK_TIMEOUT: Duration = Duration::from_secs(330);

pub fn tool() -> Value {
    json!({
        "name": TOOL,
        "description": "A form you fill has a box accepting the site's terms (CGU, CGV, conditions) or giving a \
    consent (privacy policy, use of data): never tick it yourself (your browser refuses it). Call this with the site, the \
    box's label as the page shows it, and the link to the terms when the page has one: a card asks your person « Oui / \
    Non ». On « Oui » that box is ticked for you, go on with the form; on « Non » stop that sign-up and tell them plainly. \
    Captchas stay theirs (they take over your browser); a payment only through pay_with_card.",
        "inputSchema": {"type": "object", "properties": {
            "site": {"type": "string", "description": "the site's address, as your browser shows it, e.g. https://www.example.com"},
            "box": {"type": "string", "description": "the box's label, as the page shows it, e.g. « J'accepte les conditions générales de vente »"},
            "terms_url": {"type": "string", "description": "the link to the terms the box is about, when the page has one"},
        }, "required": ["site", "box"]},
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "openWorldHint": true},
    })
}

// --- the box in the page --------------------------------------------------------------------------------------------

/// Finds, in one frame, the box not ticked whose label holds `box` (or is held in it), accents, case and punctuation
/// aside: its `<label>`, the text it is labelled by (`aria-labelledby`), else the text right after it up to the next
/// field or line break (second review 13/11: a box whose text is a span beside it; third: two boxes in one paragraph),
/// or its parent's when that holds this box only, and its aria-label; among several, the one shown (a form's hidden
/// mobile copy left out). With `apply`, ticks it if it is a terms or consent box (a cookie banner's choice is not,
/// unless it also names the terms). Run in an isolated world: it reads no value, and its click is the page's own `click`.
pub fn box_script(wanted: &str, apply: bool) -> String {
    let wanted = serde_json::to_string(wanted).unwrap_or_else(|_| "\"\"".into());
    format!(
        "(() => {{ \
const norm = (t) => (t || '').toLowerCase().normalize('NFD').replace(/[\\u0300-\\u036f]/g, '').replace(/[^a-z0-9]+/g, ' ').trim(); \
const TERMS = /\\b(cgu|cgv|agb|rgpd|gdpr)\\b|terms|conditions|consent|privacy|confidentialit|datenschutz|terminos|condiciones|politique de/; \
const STRONG = /\\b(cgu|cgv|agb)\\b|conditions|terms|terminos|condiciones|nutzungsbedingungen/; \
const want = norm({wanted}); \
const label = (e) => {{ const own = [...(e.labels || [])].map((l) => l.innerText).join(' '); \
const by = ((e.getAttribute && e.getAttribute('aria-labelledby')) || '').split(/\\s+/).map((i) => ((document.getElementById(i) || {{}}).innerText || '')).join(' '); \
let around = ''; if (!own && !by.trim()) {{ for (let n = e.nextSibling; n && around.length < 300; n = n.nextSibling) {{ if (n.nodeType === 1 && n.matches('input, select, textarea, button, br, label')) break; around += ' ' + n.textContent; }} \
const p = e.parentElement; if (!around.trim() && p && p.innerText.length <= 300 && p.querySelectorAll('input').length === 1) around = p.innerText; }} \
return [own, by, around, (e.getAttribute && e.getAttribute('aria-label')) || ''].join(' ').replace(/\\s+/g, ' ').trim(); }}; \
const seen = (e) => e.getClientRects().length > 0 || [...(e.labels || [])].some((l) => l.getClientRects().length > 0); \
let found = [...document.querySelectorAll('input[type=checkbox]')].filter((e) => !e.disabled && !e.checked).filter((e) => {{ const t = norm(label(e)); return t && want && (t.includes(want) || want.includes(t)); }}); \
if (found.length > 1) {{ const shownOnes = found.filter(seen); if (shownOnes.length >= 1) found = shownOnes; }} \
if (found.length !== 1) return JSON.stringify({{count: found.length}}); \
const box = found[0]; const shown = label(box).slice(0, 300); const all = norm(shown + ' ' + (box.name || '') + ' ' + (box.id || '')); \
const terms = TERMS.test(all) && (!/cookie/.test(all) || STRONG.test(all)); \
if (!{apply} || !terms) return JSON.stringify({{count: 1, label: shown, terms: terms}}); \
box.scrollIntoView({{block: 'center'}}); box.click(); \
if (!box.checked) {{ box.checked = true; box.dispatchEvent(new Event('input', {{bubbles: true}})); box.dispatchEvent(new Event('change', {{bubbles: true}})); }} \
return JSON.stringify({{count: 1, label: shown, terms: terms, ticked: box.checked}}); }})()"
    )
}

/// What `box_script` answered in the frames of `origin` (the first frame that has the box), or what to tell the agent.
async fn find_box(page: &mut Page, origin: &str, wanted: &str, apply: bool) -> Result<Value, FillError> {
    let mut best = json!({"count": 0});
    for (session, frame_id, frame_url) in page.frames().await? {
        if !crate::login_fill::frame_on(&frame_url, origin) {
            continue; // another site's frame: never looked at
        }
        let world = json!({"frameId": frame_id, "worldName": "bipagents-terms", "grantUniveralAccess": false});
        let Ok(created) = page.cdp.call("Page.createIsolatedWorld", world, Some(&session)).await else { continue };
        let Some(context) = created.get("executionContextId").and_then(Value::as_i64) else { continue };
        let Ok(answer) = page.cdp.evaluate(&session, context, &box_script(wanted, apply)).await else { continue };
        let found = match answer {
            Value::String(text) => serde_json::from_str::<Value>(&text).unwrap_or(Value::Null),
            other => other,
        };
        let count = found.get("count").and_then(Value::as_u64).unwrap_or(0);
        if count == 1 {
            return Ok(found);
        }
        if count > best.get("count").and_then(Value::as_u64).unwrap_or(0) {
            best = found;
        }
    }
    Ok(best)
}

// --- the person's cards ---------------------------------------------------------------------------------------------

struct Ask {
    agent: String,
    host: String,
    label: String,
    terms_url: String,
    at: f64,
    answer: Option<bool>,
    notify: Arc<Notify>,
}

static ASKS: LazyLock<Mutex<HashMap<String, Ask>>> = LazyLock::new(Default::default);

fn asks() -> std::sync::MutexGuard<'static, HashMap<String, Ask>> {
    ASKS.lock().unwrap_or_else(|e| e.into_inner())
}

/// The card's sentence.
pub fn sentence(shown: &str, host: &str, label: &str) -> String {
    format!("{shown} veut accepter les conditions de {host} : « {label} »")
}

/// What a « Oui » held for a turn nobody follows grants (held_approvals.rs `take_grant`): this box on this site only.
pub fn command(origin: &str, label: &str) -> String {
    format!("{TOOL} {origin} « {label} »")
}

fn shown_name(config: &Config, agent: &str) -> String {
    config.agent(Some(agent)).map(|a| a.display_name.clone()).filter(|n| !n.is_empty()).unwrap_or(agent.to_string())
}

/// The cards waiting, in Hermes' approvals' shape (`GET /v1/approvals`), oldest first.
pub fn pending_items(config: &Config, agent: Option<&str>) -> Vec<Dict> {
    let mut items: Vec<(f64, Dict)> = asks()
        .iter()
        .filter(|(_, a)| a.answer.is_none() && agent.is_none_or(|n| n == a.agent))
        .filter_map(|(id, a)| {
            let summary = sentence(&shown_name(config, &a.agent), &a.host, &a.label);
            let item = json!({
                "agent": a.agent, "run_id": format!("{RUN_PREFIX}{id}"), "request_id": id,
                "command": if a.terms_url.is_empty() { a.host.clone() } else { a.terms_url.clone() },
                "description": summary, "bip_summary": summary, "choices": ["once", "deny"],
                "created_at": a.at, "session_id": null,
            });
            item.as_object().cloned().map(|d| (a.at, d))
        })
        .collect();
    items.sort_by(|a, b| a.0.total_cmp(&b.0));
    items.into_iter().map(|(_, d)| d).collect()
}

/// A card answered (`POST /v1/approve`). False: no such card waiting.
pub fn answer(config: &Config, agent: &str, id: &str, allow: bool, via: &str) -> bool {
    let (host, label) = {
        let mut table = asks();
        let Some(ask) = table.get_mut(id).filter(|a| a.agent == agent && a.answer.is_none()) else { return false };
        ask.answer = Some(allow);
        ask.notify.notify_waiters();
        (ask.host.clone(), ask.label.clone())
    };
    let fields = json!({
        "session_id": null, "run_id": format!("{RUN_PREFIX}{id}"), "request_id": id, "command": host,
        "description": sentence(&shown_name(config, agent), &host, &label),
        "topic": "terms_consent", "choice": if allow { "once" } else { "deny" }, "via": via,
    });
    if let Some(map) = fields.as_object() {
        if let Err(exc) = crate::approvals_log::record(config, agent, map) {
            tracing::warn!(agent, error = %exc, "terms decision not kept");
        }
    }
    tracing::info!(agent, site = %host, allowed = allow, "terms card answered");
    true
}

/// Takes the card away when the call ends, whatever way.
struct CardGone(String);

impl Drop for CardGone {
    fn drop(&mut self) {
        asks().remove(&self.0);
    }
}

/// The person's answer: Some(yes or no), None when nobody answered within `ASK_TIMEOUT`.
async fn ask_person(state: &AppState, agent: &AgentConfig, host: &str, label: &str, terms_url: &str) -> Option<bool> {
    let id = py::uuid4_hex()[..12].to_string();
    let notify = Arc::new(Notify::new());
    asks().insert(
        id.clone(),
        Ask {
            agent: agent.name.clone(),
            host: host.to_string(),
            label: label.to_string(),
            terms_url: terms_url.to_string(),
            at: py::time_time(),
            answer: None,
            notify: notify.clone(),
        },
    );
    let _gone = CardGone(id.clone());
    let run = format!("{RUN_PREFIX}{id}");
    let turn = state.hub.turn_of(&agent.name, TOOL).map(|t| t.run_id);
    crate::card_place::place_for(state, agent, &run, turn.as_deref()).await;
    let line = sentence(&shown_name(&state.config(), &agent.name), host, label);
    state.push.notify_bridge_card(&agent.name, &line, "mail_request", &run).await;
    tracing::info!(agent = %agent.name, site = %host, "terms asked of the person");
    let wait = Duration::from_secs(u64::from(state.config().browser.consent_seconds.max(1)));
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let wake = notify.notified();
        match asks().get(&id).map(|a| a.answer) {
            Some(Some(answer)) => return Some(answer),
            Some(None) => {}
            None => return Some(false),
        }
        if tokio::time::timeout_at(deadline, wake).await.is_err() {
            return None;
        }
    }
}

// --- the tool -------------------------------------------------------------------------------------------------------

fn refusal(text: &str) -> Dict {
    tool_text(text, true)
}

fn arg(arguments: &Dict, key: &str) -> String {
    arguments.get(key).filter(|v| !v.is_null()).map(py::py_str).unwrap_or_default().trim().to_string()
}

const NO_BROWSER: &str = "A site's terms are accepted in your own visible browser, and you have none.";
const WAIT: &str = "Your person is using your browser in the app right now: nothing was ticked. Wait until they hand \
it back, then call again.";
pub const CLOSED: &str = "Someone else is talking to you in this conversation: their terms are theirs to accept. \
Nothing was ticked.";
pub const REFUSED: &str = "Your person said no: the box is not ticked. Stop this sign-up here and tell them plainly; \
don't ask again unless they tell you to.";
pub const NO_ANSWER: &str = "Your person did not answer in time: nothing was ticked. Tell them their « Oui » waits in \
the app, and call again once they say so.";
pub const HELD: &str = "Your person is not here now (this turn runs on its own): a card asks them to accept these \
terms; it was not refused. When they say yes you will be told: then call accept_terms again for the same box and it is \
ticked. Don't tick it another way and don't submit the form meanwhile; end with your report saying what waits for their \
OK.";

fn unreachable(exc: &FillError) -> String {
    match exc {
        FillError::NoPage => "No web page is open in your browser: open the form first.".into(),
        FillError::Unreachable => "Your browser can't be reached right now (stopped, or your person is using it in \
             the app). Open the form in your browser first; if they have the browser, wait until they hand it back."
            .into(),
        other => format!("Your browser did not answer as expected ({other})."),
    }
}

/// The page open in the agent's browser, checked to be on `origin`.
async fn page_on(port: u16, origin: &str) -> Result<Page, String> {
    let mut page = Page::open(port, None).await.map_err(|e| unreachable(&e))?;
    let url = page.url().await.map_err(|e| unreachable(&e))?;
    let Some((scheme, host)) = card_fill::origin_of(&url) else {
        return Err("The page open in your browser is not a website.".into());
    };
    if !card_fill::secure(&scheme, &host) {
        return Err("This page is not secure (no https): never accept terms on it.".into());
    }
    let shown = crate::vault::normalize_origin(&url).unwrap_or_default();
    if shown != origin {
        return Err(format!("Your browser shows {shown}, not {origin}: open that site's form first, then call again."));
    }
    Ok(page)
}

pub async fn call(state: &AppState, agent: &AgentConfig, arguments: &Dict) -> Dict {
    if crate::shared_pages::who(state, &agent.name, TOOL) != crate::shared_pages::Who::Owner {
        return refusal(CLOSED);
    }
    let config = state.config();
    let origin = match crate::vault::normalize_origin(&arg(arguments, "site")) {
        Ok(o) => o,
        Err(exc) => return refusal(&format!("site: {exc}")),
    };
    let host = origin.split("://").nth(1).unwrap_or(&origin).to_string();
    let wanted = crate::cards::quoted(&arg(arguments, "box"), 300);
    if wanted.is_empty() {
        return refusal("Give the box's label (box), as the page shows it.");
    }
    let terms_url = Some(crate::cards::quoted(&arg(arguments, "terms_url"), 500))
        .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
        .unwrap_or_default();
    let Some(port) = crate::browsers::ports_of(&config, &agent.name).and_then(|p| p.get("cdp").copied()) else {
        return refusal(NO_BROWSER);
    };
    if crate::browsers::hold_token(&config, &agent.name).is_some() {
        return refusal(WAIT);
    }
    crate::browsers::busy(&config, &agent.name);
    let found = match page_on(port, &origin).await {
        Ok(mut page) => match find_box(&mut page, &origin, &wanted, false).await {
            Ok(found) => found,
            Err(exc) => return refusal(&unreachable(&exc)),
        },
        Err(say) => return refusal(&say),
    };
    match found.get("count").and_then(Value::as_u64).unwrap_or(0) {
        0 => return refusal(&format!(
            "No box not ticked yet with the label « {wanted} » on this page: read the box's label as the page shows \
                 it (a form folded in a panel opens first), then call again."
        )),
        1 => {}
        _ => return refusal("Several boxes not ticked have that label: give the box's whole label."),
    }
    let label = found.get("label").and_then(Value::as_str).unwrap_or(&wanted).to_string();
    if found.get("terms").and_then(Value::as_bool) != Some(true) {
        return refusal(&format!(
            "« {label} » is not the site's terms nor a consent: no card for it. Tick it yourself only if your person \
             asked for it (a newsletter, « remember me »)."
        ));
    }
    if crate::browsers::hold_token(&config, &agent.name).is_some() {
        return refusal(WAIT); // before a held « Oui » is used: it stays for the next call (second review 13/11)
    }
    let key = command(&origin, &label);
    let held_grant = {
        let (c, name, k) = (config.clone(), agent.name.clone(), key.clone());
        tokio::task::spawn_blocking(move || crate::held_approvals::take_grant(&c, &name, &k)).await.ok().flatten()
    };
    if held_grant.is_none() {
        let turn = state.hub.turn_of(&agent.name, TOOL).map(|t| t.run_id);
        let unattended = turn.as_deref().and_then(|run| state.hub.nobody_there_for(&agent.name, run).map(|s| (run, s)));
        if let Some((run, session)) = unattended {
            hold(state, agent, run, session, &host, &label, &terms_url, &key).await;
            return tool_text(HELD, false);
        }
        match ask_person(state, agent, &host, &label, &terms_url).await {
            Some(true) => {}
            Some(false) => return refusal(REFUSED),
            None => return refusal(NO_ANSWER),
        }
    }
    if crate::browsers::hold_token(&config, &agent.name).is_some() {
        return refusal(WAIT);
    }
    let ticked = match page_on(port, &origin).await {
        Ok(mut page) => match find_box(&mut page, &origin, &label, true).await {
            Ok(done) => done.get("ticked").and_then(Value::as_bool) == Some(true),
            Err(exc) => return refusal(&unreachable(&exc)),
        },
        Err(say) => return refusal(&format!("Your person said yes, but nothing was ticked: {say}")),
    };
    if !ticked {
        return refusal(
            "Your person said yes, but the box could not be ticked (the page changed?): read the page, then \
                        call again for the same box.",
        );
    }
    tracing::info!(agent = %agent.name, site = %host, "terms box ticked with the person's yes");
    tool_text(
        &format!("Ticked « {label} » on {host}, with your person's « Oui ». Go on with the form (submit it yourself)."),
        false,
    )
}

/// The card held for a turn nobody follows (held_approvals.rs), placed in that turn's conversation and pushed once.
#[allow(clippy::too_many_arguments)]
async fn hold(
    state: &AppState,
    agent: &AgentConfig,
    run: &str,
    session: Option<String>,
    host: &str,
    label: &str,
    terms_url: &str,
    key: &str,
) {
    let config = state.config();
    let summary = sentence(&shown_name(&config, &agent.name), host, label);
    let data = json!({"command": key, "description": summary, "bip_summary": summary, "bip_detail": terms_url});
    let Some(data) = data.as_object().cloned() else { return };
    let routine = match session.as_deref() {
        Some(s) => crate::routines::of_session(state, agent, s).await,
        None => None,
    };
    let now = py::time_time();
    let until = routine
        .as_ref()
        .and_then(|r| r.next_run)
        .filter(|next| *next > now)
        .map_or(now + crate::held_approvals::KEEP_SECONDS, |next| next.min(now + crate::held_approvals::KEEP_SECONDS));
    let (c, name, r, s, rid) =
        (config.clone(), agent.name.clone(), run.to_string(), session.clone(), routine.map(|r| r.id));
    let Ok((card, joined)) = tokio::task::spawn_blocking(move || {
        crate::held_approvals::hold(&c, &name, &r, s.as_deref(), &data, rid.as_deref(), until)
    })
    .await
    else {
        return;
    };
    let card_run = format!("{}{}", crate::held_approvals::RUN_PREFIX, card.id);
    crate::card_place::place_for(state, agent, &card_run, Some(run)).await;
    if !joined {
        state.push.notify_bridge_card(&agent.name, &card.summary, "mail_request", &card_run).await;
    }
    tracing::info!(agent = %agent.name, site = %host, card = %card.id, "terms held for the person (nobody follows the turn)");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tool_says_what_it_is_for() {
        let tool = tool();
        let description = tool["description"].as_str().unwrap();
        assert!(description.contains("never tick it yourself") && description.contains("« Oui / Non »"));
        assert!(description.contains("Captchas stay theirs") && description.contains("pay_with_card"));
        assert_eq!(tool["inputSchema"]["required"], json!(["site", "box"]));
    }

    #[test]
    fn the_box_is_passed_as_a_string_and_only_ticked_when_asked() {
        let script = box_script("J'accepte\"); alert(1); (\"", false);
        assert!(script.contains("const want = norm(\"J'accepte\\\"); alert(1); (\\\"\");"), "{script}");
        assert!(script.contains("if (!false || !terms)"));
        assert!(box_script("x", true).contains("if (!true || !terms)"));
        assert!(script.contains("!/cookie/.test(all) || STRONG.test(all)") && script.contains("aria-labelledby"));
        assert!(script.contains("found.filter(seen)"));
    }

    #[test]
    fn a_held_yes_grants_that_box_on_that_site_only_once_and_not_for_ever() {
        // what a « Oui » held for a turn nobody follows unlocks (held_approvals.rs): exactly this command
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        let key = command("https://shop.example", "J'accepte les conditions générales de vente");
        let summary = sentence("Vie", "shop.example", "J'accepte les conditions générales de vente");
        let data = json!({"command": key, "description": summary, "bip_summary": summary});
        let now = py::time_time();
        let (card, _) = crate::held_approvals::hold(
            &config,
            "vie",
            "run_1",
            Some("s1"),
            data.as_object().unwrap(),
            None,
            now + 60.0,
        );
        // no « Toujours » for a consent
        let item = crate::approval_level::with_always(&config, crate::held_approvals::item_of(&card));
        assert!(item.get("bip_always").is_none(), "{item:?}");
        assert!(crate::held_approvals::take_grant(&config, "vie", &key).is_none(), "nothing before the « Oui »");
        crate::held_approvals::answer(&config, "vie", &card.id, true).unwrap();
        let other_box = command("https://shop.example", "J'accepte de recevoir la newsletter");
        let other_site = command("https://other.example", "J'accepte les conditions générales de vente");
        assert!(crate::held_approvals::take_grant(&config, "vie", &other_box).is_none());
        assert!(crate::held_approvals::take_grant(&config, "vie", &other_site).is_none());
        assert!(crate::held_approvals::take_grant(&config, "coach", &key).is_none());
        assert!(crate::held_approvals::take_grant(&config, "vie", &key).is_some());
        assert!(crate::held_approvals::take_grant(&config, "vie", &key).is_none(), "once");
    }
}
