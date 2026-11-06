//! The agent's side of the phone exit (docs/phone-exit.md §1, §7): the card « Passer par mon téléphone pour <site> ? »
//! made after a refusal (the agent's `ask_phone_exit`, or the browser tool seeing a challenge page), its answer (a
//! session accepted, the site set to « Toujours »), and the plain lines the agent reads when the phone refused a
//! stream — so it never says « the site blocks servers » when the phone was only stopped.

use serde_json::{json, Value};

use crate::config::{AgentConfig, Config};
use crate::py;
use crate::Dict;

use super::store::{self, Card, Session, MAX_CARDS, RUN_PREFIX};

pub const TOOL_NAME: &str = "ask_phone_exit";

/// What asking gave.
#[derive(Debug, Clone, PartialEq)]
pub enum Asked {
    /// A card for the person (made now: true).
    Card(Card, bool),
    /// A session already covers the site.
    Live(Session),
    /// Not possible: `off_place` (feature off here), `not_own` (a shared/family/project agent), `off` (the person
    /// switched it off), `never` (the person's « Jamais » for this site), `too_many` (cards waiting), or a name's
    /// refusal code (`literal_ip`, `local_name`…).
    No(String),
}

/// A card for `agent` about the site of `raw` (a host or URL), unless one is waiting or a session covers it.
pub fn ask(config: &Config, agent: &str, raw: &str, reason: &str) -> std::io::Result<Asked> {
    if !config.phone_exit.enabled {
        return Ok(Asked::No("off_place".into()));
    }
    if !store::owned(config, agent) {
        return Ok(Asked::No("not_own".into()));
    }
    let site = match super::checks::site_of(raw) {
        Ok(site) => site,
        Err(why) => return Ok(Asked::No(why.into())),
    };
    let host = super::checks::host_of(raw);
    let reason: String = reason.trim().chars().take(200).collect();
    store::change(config, |s| {
        if !s.settings.allowed {
            return Asked::No("off".into());
        }
        let mode = s.mode(&site).to_string();
        if mode == "never" {
            return Asked::No("never".into());
        }
        if let Some(live) = s.session_for(agent, &site) {
            return Asked::Live(live.clone());
        }
        let now = py::time_time();
        s.cards.retain(|c| now - c.at < store::CARD_TTL);
        if let Some(card) = s.cards.iter().find(|c| c.agent == agent && c.site == site) {
            return Asked::Card(card.clone(), false);
        }
        if s.cards.iter().filter(|c| c.agent == agent).count() >= MAX_CARDS {
            return Asked::No("too_many".into());
        }
        let card = Card {
            id: py::token_urlsafe(9).replace(['-', '_'], "x"),
            agent: agent.into(),
            site: site.clone(),
            host: if host == site || super::checks::covers(&site, &host) { host.clone() } else { site.clone() },
            reason: reason.clone(),
            kind: if mode == "always" { "restart".into() } else { "ask".into() },
            at: now,
        };
        s.cards.push(card.clone());
        Asked::Card(card, true)
    })
}

/// The person answered card `id`: `once`, `always` (the site set to « Toujours ») or `deny`. Some(Some(session)):
/// the session the phone now carries (a live one covering the site is reused); Some(None): refused; None: no such
/// card (answered already, or gone).
pub fn answer(config: &Config, agent: &str, id: &str, choice: &str, via: &str) -> anyhow::Result<Option<Option<Session>>> {
    let choice = match choice.trim().to_lowercase().as_str() {
        "always" => "always",
        "deny" | "denied" | "no" | "refuse" | "refused" => "deny",
        _ => "once",
    };
    let answered = store::change(config, |s| {
        let i = s.cards.iter().position(|c| c.agent == agent && c.id == id)?;
        let card = s.cards.remove(i);
        if choice == "deny" {
            return Some((card, None));
        }
        if choice == "always" {
            s.sites.insert(
                card.site.clone(),
                store::Site { mode: "always".into(), minimal: s.sites.get(&card.site).and_then(|x| x.minimal), at: py::time_time() },
            );
        }
        if let Some(live) = s.session_for(agent, &card.site) {
            return Some((card, Some(live.clone())));
        }
        let session = store::new_session(config, s, agent, vec![card.site.clone()], via);
        s.sessions.push(session.clone());
        Some((card, Some(session)))
    })?;
    let Some((card, session)) = answered else { return Ok(None) };
    let lang = store::lang_of(config, agent);
    let fields = json!({"session_id": null, "run_id": format!("{RUN_PREFIX}{}", card.id), "request_id": card.id,
        "command": card.site, "description": store::card_sentence(lang, &store::shown_name(config, agent), &card),
        "topic": "phone exit", "choice": choice, "via": via});
    if let Some(fields) = fields.as_object() {
        crate::approvals_log::record(config, agent, fields)?;
    }
    tracing::info!(target: "bipbridge::phone_exit", agent, site = %card.site, choice, via,
        session = %session.as_ref().map(|s| s.id.as_str()).unwrap_or("-"), "phone exit card answered");
    Ok(Some(session))
}

/// The card's push (a card push: tapping opens the conversation on it).
pub async fn notify(push: &crate::push::PushService, config: &Config, card: &Card) {
    let line = store::push_line(store::lang_of(config, &card.agent), card);
    push.notify_bridge_card(&card.agent, &line, "phone_exit_request", &format!("{RUN_PREFIX}{}", card.id)).await;
}

/// Offered to the person's own agents while the place has the feature.
pub fn offered(config: &Config, agent: &str) -> bool {
    config.phone_exit.enabled && store::owned(config, agent)
}

pub fn tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "When a site refuses you because you browse from a server (a Cloudflare « Just a moment » \
or « Attention required » page, DataDome, « Access denied », a 403 page for bots), ask your person whether you may \
open THAT site through their own phone's internet connection: they get a card. Never solve a CAPTCHA or a challenge \
yourself, never retry in a loop. Give the site (its name or URL) and in a few plain words why. Then tell your person \
in one sentence and wait for their answer. When you are done with the site, call it again with `done: true`. If the \
browser says « Phone exit: … », follow what it says.",
        "inputSchema": {"type": "object", "properties": {
            "site": {"type": "string", "description": "the site's name or the page's URL, e.g. ameli.fr"},
            "reason": {"type": "string", "description": "why, in a few plain words in your person's language"},
            "done": {"type": "boolean", "description": "true: you no longer need their phone for this site"}},
            "required": ["site"]},
        "annotations": {"readOnlyHint": true, "destructiveHint": false, "openWorldHint": false},
    })
}

fn why_text(code: &str) -> &'static str {
    match code {
        "off_place" => "Browsing through your person's phone is not available on this server.",
        "not_own" => "Browsing through a person's phone is only for an agent of their own, never a shared or family \
agent. Tell your person the site refuses servers; they can do this step themselves (« Prendre la main »).",
        "off" => "Your person switched off browsing through their phone (Settings). Tell them the site refuses \
servers; they can switch it on, or do this step themselves (« Prendre la main »).",
        "never" => "Your person chose never to use their phone for this site. Tell them the site refuses servers; \
they can do this step themselves (« Prendre la main »).",
        "too_many" => "Your person already has several of these cards waiting: wait for their answers.",
        _ => "That is not a site name (give its name, like ameli.fr, or the page's URL).",
    }
}

/// One call (blocking: files only). The tool's text, whether it is an error, and the card to push.
pub fn call(config: &Config, agent: &AgentConfig, arguments: &Dict) -> (String, bool, Option<Card>) {
    let text = |k: &str| arguments.get(k).map(py::py_str).map(|s| s.trim().to_string()).unwrap_or_default();
    let site = text("site");
    if site.is_empty() {
        return ("Give the site, e.g. ameli.fr.".into(), true, None);
    }
    if arguments.get("done").is_some_and(py::truthy) {
        let Ok(site) = super::checks::site_of(&site) else { return (why_text("bad").to_string(), true, None) };
        let ended = super::end_sessions_of(config, &agent.name, &site, "task_done");
        let text = if ended > 0 {
            format!("Done: {site} no longer goes through your person's phone (unless they set it to « Toujours »).")
        } else {
            format!("No session through your person's phone was open for {site}.")
        };
        return (text, false, None);
    }
    match ask(config, &agent.name, &site, &text("reason")) {
        Err(exc) => (format!("Could not ask: {exc}"), true, None),
        Ok(Asked::Card(card, made)) => {
            let text = if made {
                format!(
                    "Your person got a card asking whether you may open {} through their phone (their own internet \
connection). Tell them in one short sentence, then wait for their answer before opening the site again. Never solve a \
CAPTCHA yourself.",
                    card.site
                )
            } else {
                format!("Your person already has a card about {}: wait for their answer.", card.site)
            };
            (text, false, made.then_some(card))
        }
        Ok(Asked::Live(session)) => {
            let text = match session.state.as_str() {
                "active" => format!(
                    "{} already goes through your person's phone. If it still shows a challenge, their address does \
not help for this site: never solve it; ask your person to do this step themselves (« Prendre la main »).",
                    session.sites.join(", ")
                ),
                _ => format!(
                    "Your person accepted, but their phone is not relaying {} right now (it is starting, or it lost its \
network). Wait a little and retry once; if it still fails, tell them their phone's relay is stopped (not that the site \
blocks you).",
                    session.sites.join(", ")
                ),
            };
            (text, false, None)
        }
        Ok(Asked::No(code)) => (why_text(&code).to_string(), false, None),
    }
}

/// The line the agent reads when the phone exit refused a stream to this site (the browser tool adds it).
pub fn refusal_text(code: &str, site: &str) -> String {
    let what = match code {
        "paused" => "this site goes through your person's phone, which is not relaying right now (stopped, starting, \
or it lost its network). Do NOT say the site blocks you. Wait a little and retry once; if it still fails, tell your \
person their phone's relay is stopped (they restart it from the card or the notification).",
        "wifi_only" => "your person's phone is on mobile data and they allow it on Wi-Fi only. Tell them; wait until \
they are on Wi-Fi or they change it.",
        "network_changed" => "your person's phone changed network and the page's connections were cut: reload the \
page once.",
        "expired" => "the session through your person's phone reached its time limit. Call ask_phone_exit again if \
you still need it.",
        "off" => "your person switched off browsing through their phone. Tell them; don't say the site blocks you.",
        "not_allowed" => "this site is not in your person's phone list for this task. Call ask_phone_exit if you \
need it.",
        "rate" => "too many new connections in a minute: go on more slowly.",
        "dns" | "connect" => "your person's phone could not reach the site (its network or the site is down). Retry \
later.",
        "private" => "your person's phone refused this address (a local or blocked address on their network: an \
ad blocker, for example). Not an error of the site.",
        _ => "your person's phone refused this connection. Don't say the site blocks you.",
    };
    format!("Phone exit ({site}, {code}): {what}")
}

/// The line the agent reads when its page is a challenge/block page for servers, after asking (`ASK` answer).
pub fn challenge_text(kind: &str, site: &str, answer: &str) -> String {
    let start = format!("Phone exit: this page is a block for servers ({kind}) on {site}. Never solve it or a CAPTCHA.");
    let rest = match answer.split_whitespace().next().unwrap_or("") {
        "CARD" => "Your person just got a card asking whether you may open this site through their phone. Tell them \
in one short sentence and wait for their answer.",
        "WAITING" => "Your person already has a card about it: wait for their answer.",
        "LIVE" => "This site already goes through your person's phone and still blocks: their address does not \
help here. Ask your person to do this step themselves (« Prendre la main »).",
        "NO" => match answer.split_whitespace().nth(1).unwrap_or("") {
            "never" => "Your person chose never to use their phone for this site: tell them the site refuses servers; \
they can do this step themselves (« Prendre la main »).",
            "too_many" => "Your person has several cards waiting: wait for their answers.",
            _ => "Tell your person the site refuses servers; they can do this step themselves (« Prendre la main »).",
        },
        _ => "Tell your person the site refuses servers; they can do this step themselves (« Prendre la main »).",
    };
    format!("{start} {rest}")
}

/// A page that is a challenge or block page for servers: its kind (None: an ordinary page). Markers of the pages
/// themselves, not of the scripts ordinary pages load (DataDome's tag is on leboncoin's every page).
pub fn challenge_kind(text: &str) -> Option<&'static str> {
    let has = |s: &str| text.contains(s);
    // Cloudflare's challenge page, its title in the visitor's language (« Vérification de sécurité » for ameli, 6/11)
    const CLOUDFLARE: [&str; 12] = [
        "Just a moment...", "Just a moment…", "cf_chl_opt", "cf-chl-widget", "Attention Required! | Cloudflare",
        "Vérification de sécurité", "Security verification", "Un instant…", "Un instant...",
        "Sicherheitsüberprüfung", "Verificación de seguridad", "Einen Moment…",
    ];
    if CLOUDFLARE.iter().any(|m| has(m)) {
        return Some("Cloudflare challenge");
    }
    if has("captcha-delivery.com") {
        return Some("DataDome");
    }
    if has("px-captcha") || has("Pardon Our Interruption") {
        return Some("PerimeterX");
    }
    if has("Access Denied") && has("errors.edgesuite.net") {
        return Some("Akamai « Access Denied »");
    }
    if has("_Incapsula_Resource") || (has("Request unsuccessful") && has("Incapsula")) {
        return Some("Imperva");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_pages_not_ordinary_ones() {
        assert_eq!(challenge_kind("<title>Just a moment...</title>"), Some("Cloudflare challenge"));
        assert_eq!(challenge_kind("{\"title\": \"Vérification de sécurité\"}"), Some("Cloudflare challenge"));
        assert_eq!(challenge_kind("{\"title\": \"Attention Required! | Cloudflare\"}"), Some("Cloudflare challenge"));
        assert_eq!(challenge_kind("<iframe src=\"https://geo.captcha-delivery.com/captcha/?x\">"), Some("DataDome"));
        assert_eq!(challenge_kind("<div id=\"px-captcha\">"), Some("PerimeterX"));
        assert!(challenge_kind("Access Denied ... https://errors.edgesuite.net/18.x").is_some());
        // an ordinary page loading DataDome's tag, or Cloudflare's analytics
        assert_eq!(challenge_kind("<script src=\"https://js.datadome.co/tags.js\"></script> Annonces"), None);
        assert_eq!(challenge_kind("static.cloudflareinsights.com/beacon.min.js"), None);
    }

    #[test]
    fn the_agent_is_told_why_never_that_the_site_blocks() {
        let paused = refusal_text("paused", "ameli.fr");
        assert!(paused.contains("not relaying") && paused.contains("Do NOT say the site blocks you"));
        assert!(refusal_text("wifi_only", "ameli.fr").contains("Wi-Fi only"));
        assert!(refusal_text("not_allowed", "x.fr").contains("ask_phone_exit"));
        assert!(challenge_text("DataDome", "leboncoin.fr", "CARD").contains("just got a card"));
        assert!(challenge_text("DataDome", "leboncoin.fr", "NO never").contains("never to use their phone"));
        assert!(challenge_text("DataDome", "leboncoin.fr", "LIVE").contains("Prendre la main"));
    }
}
