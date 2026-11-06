//! What the person decided and what was carried (docs/phone-exit.md §2–§4): their settings (« Autoriser », « Seulement
//! en Wi-Fi », the minimal route), the per-site list (Toujours / Jamais), the cards waiting, the sessions alive and the
//! journal of ended ones. One file, `<data_dir>/phone-exit.json` (0600; NOT in `phone-exit/`, which the agents' proxy
//! mounts). The proxy's routes are written into its policy file (sandbox_sites.rs) after each change.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::Config;
use crate::py;

use super::checks::{covers, is_static};

pub const RUN_PREFIX: &str = "bip-exit-";
/// A card waits this long, then goes (a relay started the next day would be pointless).
pub const CARD_TTL: f64 = 2.0 * 3600.0;
/// Cards waiting per agent at most.
pub const MAX_CARDS: usize = 3;
/// Ended sessions kept in the journal.
pub const JOURNAL_MAX: usize = 200;
/// An ended session's reason answers its sites' streams this long (then `not_allowed`).
pub const RECENT_END: f64 = 600.0;

static LOCK: Mutex<()> = Mutex::new(());

fn yes() -> bool {
    true
}

/// The person's settings (the app's Settings › « Navigation par mon téléphone »).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Person {
    /// « Autoriser mes agents à passer par ce téléphone »: off ends every session (`off`) and routes nothing.
    #[serde(default = "yes")]
    pub allowed: bool,
    /// « Seulement en Wi-Fi » (off by default): the phone pauses on mobile data.
    #[serde(default)]
    pub wifi_only: bool,
    /// Static-file hosts of a routed site go direct (a site may say otherwise).
    #[serde(default = "yes")]
    pub minimal_route: bool,
}

impl Default for Person {
    fn default() -> Self {
        Person { allowed: true, wifi_only: false, minimal_route: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Site {
    /// `always` (through the phone whenever an own agent opens it), `never` (never asked), `ask` (kept in the list).
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimal: Option<bool>,
    #[serde(default)]
    pub at: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Session {
    pub id: String,
    pub agent: String,
    pub sites: Vec<String>,
    /// `accepted`, `active`, `paused`, `ended`.
    pub state: String,
    pub created_at: f64,
    pub accepted_at: f64,
    #[serde(default)]
    pub ended_at: Option<f64>,
    #[serde(default)]
    pub ended: Option<String>,
    pub max_minutes: f64,
    #[serde(default)]
    pub wifi_only: bool,
    #[serde(default)]
    pub minimal: bool,
    #[serde(default)]
    pub up: u64,
    #[serde(default)]
    pub down: u64,
    #[serde(default)]
    pub streams: u64,
    #[serde(default)]
    pub hosts: BTreeMap<String, u64>,
    #[serde(default)]
    pub network: String,
    #[serde(default)]
    pub device: String,
    /// `app` (started from Settings), `notification`/`card` (a card answered).
    #[serde(default)]
    pub via: String,
    /// It carried a tunnel at least once.
    #[serde(default)]
    pub ever_active: bool,
    /// Its line is in the Boîte (ended sessions).
    #[serde(default)]
    pub told: bool,
}

impl Session {
    pub fn live(&self) -> bool {
        self.state != "ended"
    }

    pub fn covers(&self, host: &str) -> bool {
        self.sites.iter().any(|s| covers(s, host))
    }

    pub fn json(&self) -> Value {
        let ends_at = self.accepted_at + self.max_minutes * 60.0;
        json!({
            "id": self.id, "agent": self.agent, "sites": self.sites, "state": self.state,
            "max_minutes": self.max_minutes, "wifi_only": self.wifi_only, "minimal": self.minimal,
            "up": self.up, "down": self.down, "used_bytes": self.up + self.down, "streams": self.streams,
            "hosts": self.hosts, "created_at": self.created_at, "accepted_at": self.accepted_at, "ends_at": ends_at,
            "ended_at": self.ended_at, "ended": self.ended, "network": self.network, "via": self.via,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Card {
    pub id: String,
    pub agent: String,
    pub site: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub reason: String,
    /// `ask` (after a refusal) or `restart` (a « Toujours » site with no phone relaying).
    pub kind: String,
    pub at: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Store {
    #[serde(default)]
    pub settings: Person,
    #[serde(default)]
    pub sites: BTreeMap<String, Site>,
    #[serde(default)]
    pub cards: Vec<Card>,
    /// Alive (not ended).
    #[serde(default)]
    pub sessions: Vec<Session>,
    /// Ended, oldest first.
    #[serde(default)]
    pub journal: Vec<Session>,
}

impl Store {
    pub fn mode(&self, site: &str) -> &str {
        self.sites.get(site).map(|s| s.mode.as_str()).unwrap_or("ask")
    }

    pub fn minimal(&self, site: &str) -> bool {
        self.sites.get(site).and_then(|s| s.minimal).unwrap_or(self.settings.minimal_route)
    }

    /// The agent's live session that covers this host (the newest).
    pub fn session_for(&self, agent: &str, host: &str) -> Option<&Session> {
        self.sessions.iter().rev().find(|s| s.agent == agent && s.live() && s.covers(host))
    }

    /// The agent's session for this host that ended in the last 10 minutes.
    pub fn recent_end(&self, agent: &str, host: &str, now: f64) -> Option<&Session> {
        self.journal
            .iter()
            .rev()
            .find(|s| s.agent == agent && s.covers(host) && s.ended_at.is_some_and(|at| now - at < RECENT_END))
    }

    /// The « Toujours » site covering this host.
    pub fn always_site(&self, host: &str) -> Option<String> {
        self.sites.iter().find(|(site, s)| s.mode == "always" && covers(site, host)).map(|(site, _)| site.clone())
    }

    /// The sites this agent's browsing sends to the phone (the proxy's `phone` entries).
    pub fn routes(&self, agent: &str) -> Vec<(String, bool)> {
        let mut out: Vec<(String, bool)> = Vec::new();
        let sites = self
            .sessions
            .iter()
            .filter(|s| s.agent == agent && s.live())
            .flat_map(|s| s.sites.iter().cloned())
            .chain(self.sites.iter().filter(|(_, s)| s.mode == "always").map(|(site, _)| site.clone()));
        for site in sites {
            if !out.iter().any(|(s, _)| *s == site) {
                let minimal = self.minimal(&site);
                out.push((site, minimal));
            }
        }
        out
    }
}

pub fn store_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("phone-exit.json")
}

pub fn load(config: &Config) -> Store {
    std::fs::read(store_path(config)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_private(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("part");
    let _ = std::fs::remove_file(&tmp);
    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    out.write_all(body)?;
    drop(out);
    std::fs::rename(&tmp, path)
}

/// A change to the store, saved; the proxy's routes written again when they changed.
pub fn change<T>(config: &Config, f: impl FnOnce(&mut Store) -> T) -> std::io::Result<T> {
    let (out, routes_changed) = {
        let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut store = load(config);
        let before = routing(&store);
        let out = f(&mut store);
        let body = serde_json::to_vec_pretty(&store).map_err(std::io::Error::other)?;
        write_private(&store_path(config), &body)?;
        (out, routing(&store) != before)
    };
    if routes_changed {
        crate::sandbox_sites::ensure_policy(config)?;
    }
    Ok(out)
}

/// What the proxy's file depends on.
fn routing(store: &Store) -> Value {
    let agents: std::collections::BTreeSet<&str> = store.sessions.iter().map(|s| s.agent.as_str()).collect();
    json!({
        "allowed": store.settings.allowed, "minimal": store.settings.minimal_route, "sites": store.sites,
        "sessions": agents.iter().map(|a| json!([a, store.routes(a)])).collect::<Vec<_>>(),
    })
}

/// The agent may use its person's phone: the feature on, the person allowing it, and the agent theirs alone (never a
/// shared, family, project or other-household-linked agent).
pub fn owned(config: &Config, agent: &str) -> bool {
    config.agent(Some(agent)).is_some() && !crate::vault::others_facing(config, agent)
}

/// The proxy's `phone` entries for this agent (sandbox_sites.rs `policy_of`): `[{"site", "minimal"}]`.
pub fn routes_for(config: &Config, agent: &str) -> Vec<Value> {
    if !config.phone_exit.enabled || !owned(config, agent) {
        return Vec::new();
    }
    let store = load(config);
    if !store.settings.allowed {
        return Vec::new();
    }
    store.routes(agent).into_iter().map(|(site, minimal)| json!({"site": site, "minimal": minimal})).collect()
}

/// The proxy's decision for a host (mirrors egress_proxy.py `phone_route`): through the phone or direct.
pub fn routed(routes: &[(String, bool)], host: &str) -> bool {
    routes.iter().any(|(site, minimal)| covers(site, host) && !(*minimal && is_static(site, host)))
}

pub fn new_id() -> String {
    py::token_urlsafe(16).replace(['-', '_'], "x")
}

/// A new session accepted for `agent` on `sites` (the person's settings copied into it).
pub fn new_session(config: &Config, store: &Store, agent: &str, sites: Vec<String>, via: &str) -> Session {
    let now = py::time_time();
    let minimal = sites.iter().all(|s| store.minimal(s));
    Session {
        id: new_id(),
        agent: agent.into(),
        sites,
        state: "accepted".into(),
        created_at: now,
        accepted_at: now,
        max_minutes: config.phone_exit.max_minutes,
        wifi_only: store.settings.wifi_only,
        minimal,
        via: via.into(),
        ..Session::default()
    }
}

/// The session ended: out of the live ones, into the journal (None: not live).
pub fn end_in(store: &mut Store, id: &str, why: &str) -> Option<Session> {
    let i = store.sessions.iter().position(|s| s.id == id)?;
    let mut session = store.sessions.remove(i);
    session.state = "ended".into();
    session.ended = Some(why.into());
    session.ended_at = Some(py::time_time());
    store.journal.push(session.clone());
    if store.journal.len() > JOURNAL_MAX {
        let extra = store.journal.len() - JOURNAL_MAX;
        store.journal.drain(..extra);
    }
    Some(session)
}

// --- words ----------------------------------------------------------------------------------------------------------

pub fn shown_name(config: &Config, agent: &str) -> String {
    config.agent(Some(agent)).map(|a| a.display_name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| agent.into())
}

pub fn lang_of(config: &Config, agent: &str) -> &'static str {
    crate::names::names(&crate::names::agent_language(config, agent)).lang
}

/// « Ameli » from « ameli.fr ».
fn site_word(site: &str) -> String {
    let first = site.split('.').next().unwrap_or(site);
    let mut c = first.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// The card's sentence in the agent's language.
pub fn card_sentence(lang: &str, agent: &str, card: &Card) -> String {
    let (site, word) = (card.site.as_str(), site_word(&card.site));
    match (lang, card.kind.as_str()) {
        ("fr", "restart") => format!(
            "{agent} passe par ton téléphone pour {site} (réglé sur « Toujours »), mais le relais est arrêté. Le relancer ?"
        ),
        ("fr", _) => format!("{word} refuse les serveurs. {agent} peut passer par ta connexion (ce téléphone) pour {site}."),
        ("de", "restart") => format!(
            "{agent} geht für {site} über dein Telefon (« Immer »), aber die Weiterleitung ist aus. Wieder starten?"
        ),
        ("de", _) => format!("{word} lehnt Server ab. {agent} kann für {site} über deine Verbindung (dieses Telefon) gehen."),
        ("es", "restart") => format!(
            "{agent} pasa por tu teléfono para {site} (« Siempre »), pero el relé está parado. ¿Reanudarlo?"
        ),
        ("es", _) => format!("{word} rechaza los servidores. {agent} puede pasar por tu conexión (este teléfono) para {site}."),
        (_, "restart") => {
            format!("{agent} goes through your phone for {site} (set to « Always »), but the relay is stopped. Start it again?")
        }
        _ => format!("{word} refuses servers. {agent} can go through your connection (this phone) for {site}."),
    }
}

/// The push's line.
pub fn push_line(lang: &str, card: &Card) -> String {
    match lang {
        "fr" => format!("Passer par ton téléphone pour {} ?", card.site),
        "de" => format!("Für {} über dein Telefon gehen?", card.site),
        "es" => format!("¿Pasar por tu teléfono para {}?", card.site),
        _ => format!("Go through your phone for {}?", card.site),
    }
}

/// « 6,2 Mo » (decimal megabytes, the way the apps show them).
pub fn megabytes(lang: &str, bytes: u64) -> String {
    // a session that carried a little shows 0,1 Mo, never 0,0
    let mb = if bytes > 0 { (bytes as f64 / 1_000_000.0).max(0.1) } else { 0.0 };
    let n = if bytes == 0 {
        "0".to_string()
    } else if mb < 10.0 { format!("{mb:.1}") } else { format!("{}", mb.round() as u64) };
    match lang {
        "en" => format!("{n} MB"),
        "fr" => format!("{} Mo", n.replace('.', ",")),
        _ => format!("{} MB", n.replace('.', ",")),
    }
}

fn why_words(lang: &str, why: &str) -> &'static str {
    match (lang, why) {
        ("fr", "user_stop") => "arrêté par toi",
        ("fr", "idle") => "plus rien à faire depuis 10 min",
        ("fr", "expired") => "durée maximale atteinte",
        ("fr", "off") => "désactivé",
        ("fr", "task_done") => "tâche finie",
        ("fr", _) => "terminé",
        ("de", "user_stop") => "von dir gestoppt",
        ("de", "idle") => "10 Min. nichts mehr zu tun",
        ("de", "expired") => "Höchstdauer erreicht",
        ("de", "off") => "ausgeschaltet",
        ("de", "task_done") => "Aufgabe erledigt",
        ("de", _) => "beendet",
        ("es", "user_stop") => "parado por ti",
        ("es", "idle") => "nada que hacer desde hace 10 min",
        ("es", "expired") => "duración máxima alcanzada",
        ("es", "off") => "desactivado",
        ("es", "task_done") => "tarea terminada",
        ("es", _) => "terminado",
        (_, "user_stop") => "stopped by you",
        (_, "idle") => "nothing to do for 10 min",
        (_, "expired") => "longest time reached",
        (_, "off") => "switched off",
        (_, "task_done") => "task done",
        _ => "ended",
    }
}

/// The Boîte's line for an ended session: « Vie a navigué via ton téléphone : ameli.fr · 6,2 Mo · 4 min · arrêté :
/// tâche finie ».
pub fn journal_line(lang: &str, agent: &str, s: &Session) -> String {
    let sites = s.sites.join(", ");
    let minutes = ((s.ended_at.unwrap_or(s.accepted_at) - s.accepted_at) / 60.0).round().max(1.0) as u64;
    let mb = megabytes(lang, s.up + s.down);
    let why = why_words(lang, s.ended.as_deref().unwrap_or(""));
    if !s.ever_active {
        return match lang {
            "fr" => format!("{agent} n'a pas pu passer par ton téléphone pour {sites} : aucun téléphone n'a relayé ({why})."),
            "de" => format!("{agent} konnte für {sites} nicht über dein Telefon gehen: kein Telefon hat weitergeleitet ({why})."),
            "es" => format!("{agent} no pudo pasar por tu teléfono para {sites}: ningún teléfono hizo de relé ({why})."),
            _ => format!("{agent} could not go through your phone for {sites}: no phone relayed ({why})."),
        };
    }
    match lang {
        "fr" => format!("{agent} a navigué via ton téléphone : {sites} · {mb} · {minutes} min · {why}"),
        "de" => format!("{agent} hat über dein Telefon gesurft: {sites} · {mb} · {minutes} Min. · {why}"),
        "es" => format!("{agent} navegó a través de tu teléfono: {sites} · {mb} · {minutes} min · {why}"),
        _ => format!("{agent} browsed through your phone: {sites} · {mb} · {minutes} min · {why}"),
    }
}

pub fn journal_title(lang: &str) -> &'static str {
    match lang {
        "fr" => "Navigation par ton téléphone",
        "de" => "Surfen über dein Telefon",
        "es" => "Navegación por tu teléfono",
        _ => "Browsing through your phone",
    }
}

/// The cards waiting, in Hermes' approvals' shape (`GET /v1/approvals`), oldest first.
pub fn pending_items(config: &Config, agent: Option<&str>) -> Vec<crate::Dict> {
    if !config.phone_exit.enabled {
        return Vec::new();
    }
    let now = py::time_time();
    let store = load(config);
    store
        .cards
        .iter()
        .filter(|c| agent.is_none_or(|a| a == c.agent) && now - c.at < CARD_TTL)
        .filter_map(|c| {
            let lang = lang_of(config, &c.agent);
            let choices = if store.mode(&c.site) == "always" { json!(["once", "deny"]) } else { json!(["once", "always", "deny"]) };
            json!({
                "agent": c.agent,
                "run_id": format!("{RUN_PREFIX}{}", c.id),
                "request_id": c.id,
                "command": c.site,
                "description": card_sentence(lang, &shown_name(config, &c.agent), c),
                "choices": choices,
                "created_at": c.at,
                "session_id": null,
                "bip_exit": {"site": c.site, "host": c.host, "reason": c.reason, "kind": c.kind,
                             "wifi_only": store.settings.wifi_only},
            })
            .as_object()
            .cloned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(agent: &str, sites: &[&str], state: &str) -> Session {
        Session {
            id: new_id(),
            agent: agent.into(),
            sites: sites.iter().map(|s| s.to_string()).collect(),
            state: state.into(),
            max_minutes: 120.0,
            ..Session::default()
        }
    }

    #[test]
    fn routes_come_from_live_sessions_and_always_sites() {
        let mut store = Store::default();
        store.sessions.push(session("vie", &["ameli.fr"], "paused"));
        store.sites.insert("leboncoin.fr".into(), Site { mode: "always".into(), minimal: None, at: 0.0 });
        store.sites.insert("doctolib.fr".into(), Site { mode: "never".into(), minimal: None, at: 0.0 });
        let routes = store.routes("vie");
        assert_eq!(routes, vec![("ameli.fr".to_string(), true), ("leboncoin.fr".to_string(), true)]);
        assert!(routed(&routes, "www.ameli.fr") && routed(&routes, "dd.leboncoin.fr"));
        assert!(!routed(&routes, "img.leboncoin.fr"), "static files go direct with the minimal route");
        assert!(!routed(&routes, "www.doctolib.fr") && !routed(&routes, "ameli.fr.evil.com"));
        store.sites.get_mut("leboncoin.fr").unwrap().minimal = Some(false);
        assert!(routed(&store.routes("vie"), "img.leboncoin.fr"));
        // another agent: only the « Toujours » sites
        assert_eq!(store.routes("budget"), vec![("leboncoin.fr".to_string(), false)]);
    }

    #[test]
    fn a_session_ends_into_the_journal() {
        let mut store = Store::default();
        let s = session("vie", &["ameli.fr"], "active");
        let id = s.id.clone();
        store.sessions.push(s);
        let ended = end_in(&mut store, &id, "user_stop").unwrap();
        assert_eq!(ended.state, "ended");
        assert!(store.sessions.is_empty() && store.journal.len() == 1);
        assert!(store.recent_end("vie", "www.ameli.fr", py::time_time()).is_some());
        assert!(end_in(&mut store, &id, "idle").is_none());
        for _ in 0..JOURNAL_MAX + 5 {
            let s = session("vie", &["x.fr"], "active");
            let id = s.id.clone();
            store.sessions.push(s);
            end_in(&mut store, &id, "idle");
        }
        assert_eq!(store.journal.len(), JOURNAL_MAX);
    }

    #[test]
    fn words_in_four_languages() {
        let card = Card {
            id: "x".into(),
            agent: "vie".into(),
            site: "ameli.fr".into(),
            host: "www.ameli.fr".into(),
            reason: String::new(),
            kind: "ask".into(),
            at: 0.0,
        };
        assert_eq!(
            card_sentence("fr", "Vie", &card),
            "Ameli refuse les serveurs. Vie peut passer par ta connexion (ce téléphone) pour ameli.fr."
        );
        assert!(card_sentence("en", "Vie", &Card { kind: "restart".into(), ..card.clone() }).contains("Start it again"));
        let mut s = session("vie", &["ameli.fr"], "ended");
        s.accepted_at = 1000.0;
        s.ended_at = Some(1240.0);
        s.ended = Some("task_done".into());
        s.down = 6_000_000;
        s.up = 200_000;
        s.ever_active = true;
        assert_eq!(journal_line("fr", "Vie", &s), "Vie a navigué via ton téléphone : ameli.fr · 6,2 Mo · 4 min · tâche finie");
        assert_eq!(megabytes("en", 123_456_789), "123 MB");
        s.ever_active = false;
        assert!(journal_line("de", "Vie", &s).contains("kein Telefon"));
    }
}
