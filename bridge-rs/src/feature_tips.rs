//! Features discovered over the first days and weeks, almost by themselves (Alex, 2026-10-28; design:
//! internal/next/feature-discovery.md). The bridge notices moments in the person's normal use with cheap signals (words
//! of their message matched by simple rules, the kinds of attachments, the tools their agents call, the day of the week)
//! and proposes the fitting feature once, through the agent they talk to most: a note joined to their next message
//! (`/v1/live/recaps/take`, like storage_watch.rs) and a card « Essayer / Plus tard / Pas pour moi » (`bip-tip-<id>`,
//! with the approvals). A calendar is the safety net for what never came up (J1 voice… J28). No model call; no content
//! kept (the message is matched in memory and dropped: counters, dates, a service id from a closed list and short
//! salted hashes only).
//!
//! Rules: one proposal a day; never while a run of that agent goes on in the conversation or one of its background
//! tasks runs, never at night (22:00–08:00), never in a routine's run; « Pas pour moi » never again for that feature,
//! « Plus tard » not before 7 days; the guide (« Bip ») proposes when there is one, else the agent written to most.
//!
//!     GET  /v1/tips                 {enabled, day, features, costumes, stats}
//!     PUT  /v1/tips/settings        {enabled} → the same
//!     POST /v1/tips/{feature}/try   « Essayer » on the page → {ok, feature, action, service?, costume?}
//!     POST /v1/tips/used            {feature}: the apps report a use (a Live call, a reply read aloud, the Carnet…)
//!     data_dir/tips.json            the state and the counters (0600)
//!
//! No Python counterpart.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::Router;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::config::Config;
use crate::web::{self, ApiResult, FieldError, HttpError, Json, Validate};
use crate::{py, Dict};

pub const RUN_PREFIX: &str = "bip-tip-";
pub const CHOICES: [&str; 3] = ["try", "later", "never"];
const DAY: f64 = 86400.0;
/// « Plus tard »: not before this long.
pub const LATER: f64 = 7.0 * DAY;
/// A card left unanswered goes after this long (as « Plus tard »); a noticed moment waits this long at most.
pub const CARD_TTL: f64 = 3.0 * DAY;
pub const QUEUE_TTL: f64 = 3.0 * DAY;
/// Quiet hours (local): from 22:00 to 08:00.
pub const QUIET: [u32; 2] = [22, 8];
/// Friday (chrono's Monday = 1).
pub const PODCAST_DAY: u32 = 5;
/// Nothing from the calendar after this day.
pub const LAST_DAY: i64 = 35;
/// The words of a message read, at most (the rest is ignored).
pub const TEXT_MAX: usize = 4000;
/// A `cronjob` call this soon after trying the morning brief or the podcast counts for it.
const TRY_WINDOW: f64 = 1800.0;
const TALK_DAYS: i64 = 14;
const REPEATS_MAX: usize = 200;

static LOCK: Mutex<()> = Mutex::new(());

// --- the features ----------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct Feature {
    pub id: &'static str,
    /// The card's title and line (French, like the bridge's other cards: the apps rebuild them in their language).
    pub title: &'static str,
    pub line: &'static str,
    /// For the agent's note (English, like the bridge's other notes).
    pub hint: &'static str,
    /// The sentence the agent may say, as an example.
    pub example: &'static str,
    /// « Essayer »: the app opens this place (`Some`), or sends the agent its starter message (`None`).
    pub open: Option<&'static str>,
    /// The Bip costume its first try unlocks (« Au quotidien »).
    pub costume: &'static str,
    /// One use is a lasting setup (a routine, a connector, an agent): adopted at once.
    pub lasting: bool,
}

/// In the calendar's order of « the next one not seen ».
pub const FEATURES: [Feature; 13] = [
    Feature {
        id: "voice",
        title: "Les réponses à voix haute",
        line: "Tes agents peuvent te lire leurs réponses, avec leur propre voix.",
        hint: "you can read your replies aloud, with your own voice (in the app: the speaker on a reply, or the voice settings)",
        example: "Au fait, je peux aussi te lire mes réponses à voix haute, si tu veux.",
        open: Some("voice"),
        costume: "musique",
        lasting: false,
    },
    Feature {
        id: "morning_brief",
        title: "Le point du matin",
        line: "Chaque matin, un petit point : météo, agenda et ce qui t'attend.",
        hint: "a short brief every morning (weather, agenda, what's coming), set up as a routine",
        example: "Si tu veux, je peux te faire un petit point chaque matin : météo, agenda et ce qui t'attend.",
        open: None,
        costume: "chef",
        lasting: true,
    },
    Feature {
        id: "carnet",
        title: "Le Carnet",
        line: "Je range les personnes, les sujets et les choses à retenir, faciles à retrouver.",
        hint: "the Carnet: people, subjects and things to remember, kept tidy and easy to find",
        example: "Je peux noter ça dans ton Carnet, pour qu'on le retrouve facilement.",
        open: Some("carnet"),
        costume: "apprendre",
        lasting: false,
    },
    Feature {
        id: "connectors",
        title: "Les connecteurs",
        line: "Je peux relier un service que tu utilises, pour m'en occuper directement.",
        hint: "you can make a connector to a service they use, to work with it directly",
        example: "Je peux te faire un connecteur pour que je m'en occupe directement.",
        open: None,
        costume: "chantier",
        lasting: true,
    },
    Feature {
        id: "family",
        title: "L'agent de la famille",
        line: "Un agent commun à la famille, et les invitations pour tes proches.",
        hint: "a family agent shared with their household, and invitations for their close ones",
        example: "On peut aussi créer un agent pour toute la famille et inviter tes proches.",
        open: Some("family"),
        costume: "beret",
        lasting: true,
    },
    Feature {
        id: "project",
        title: "Un agent pour un projet",
        line: "Un voyage, une fête entre amis : un agent rien que pour l'organiser, avec eux.",
        hint: "a project agent (create_project_agent) to organise a trip or a party, shared with their friends",
        example: "Pour organiser ça avec tes amis, je peux créer un agent rien que pour ce projet.",
        open: None,
        costume: "beret",
        lasting: true,
    },
    Feature {
        id: "archives",
        title: "Les Archives",
        line: "Tes documents rangés et retrouvés en un mot : factures, ordonnances, contrats…",
        hint: "the Archives: their documents (bills, prescriptions, contracts) kept, sorted and found in a word",
        example: "Je peux ranger ce document dans tes Archives, pour le retrouver en un mot.",
        open: Some("archives"),
        costume: "apprendre",
        lasting: false,
    },
    Feature {
        id: "routine",
        title: "Les routines",
        line: "Ce que tu me demandes souvent, je peux le faire tout seul au bon moment.",
        hint: "a routine: what they ask regularly, done by itself at the right time",
        example: "Si tu veux, je peux le faire tout seul, chaque fois au bon moment.",
        open: None,
        costume: "chantier",
        lasting: true,
    },
    Feature {
        id: "podcast",
        title: "Le podcast de la semaine",
        line: "Chaque vendredi, un petit podcast sur ta semaine ou un sujet qui te plaît.",
        hint: "a weekly podcast every Friday (create_podcast), about their week or a topic they like",
        example: "C'est vendredi : je peux te préparer un petit podcast de la semaine, si ça te dit.",
        open: None,
        costume: "musique",
        lasting: false,
    },
    Feature {
        id: "vault",
        title: "Le coffre",
        line: "Tes mots de passe gardés dans ton coffre, jamais montrés aux agents.",
        hint: "the vault: their passwords kept safe, filled in for them, never shown to the agents (never ask for a password in the chat)",
        example: "Pour tes mots de passe, il y a ton coffre : ils y sont gardés et je ne les vois jamais.",
        open: Some("vault"),
        costume: "beret",
        lasting: true,
    },
    Feature {
        id: "live",
        title: "Les appels Live",
        line: "Parle avec ton agent comme au téléphone, sans rien taper.",
        hint: "Live calls: talking with you like on the phone, hands free",
        example: "Tu peux aussi m'appeler en Live : on parle comme au téléphone.",
        open: Some("live"),
        costume: "musique",
        lasting: false,
    },
    Feature {
        id: "jobs",
        title: "Les tâches de fond",
        line: "Une longue recherche se fait en arrière-plan, je te préviens quand c'est prêt.",
        hint: "background tasks: a long search done in the background, they get told when it's ready",
        example: "Pour une recherche plus longue, je peux m'en occuper en arrière-plan et te prévenir.",
        open: Some("jobs"),
        costume: "chantier",
        lasting: false,
    },
    // watchers (watchers.rs, 2026-10-28): « préviens-moi quand… »
    Feature {
        id: "watchers",
        title: "Les veilleurs",
        line: "Je surveille un site pour toi et je te préviens dès que ça change : un créneau, un prix, un colis.",
        hint: "a watcher: you check a site for them on your own (a slot, a price, a parcel, a grade) and tell them only when it changes, without a routine",
        example: "Si tu veux, je peux surveiller ça pour toi et te prévenir dès que ça bouge.",
        open: None,
        costume: "chantier",
        lasting: true,
    },
];

pub const COSTUMES: [&str; 5] = ["chef", "musique", "apprendre", "chantier", "beret"];

pub fn feature(id: &str) -> Option<&'static Feature> {
    FEATURES.iter().find(|f| f.id == id)
}

/// The calendar: (day, feature); « next »: the next one not seen; « week »: the first week's recap.
pub const CALENDAR: [(i64, &str); 8] = [
    (1, "voice"),
    (2, "morning_brief"),
    (4, "carnet"),
    (7, "week"),
    (10, "connectors"),
    (14, "family"),
    (21, "next"),
    (28, "next"),
];

/// The services a connector may be proposed for (id, shown name, words).
const SERVICES: [(&str, &str, &str); 18] = [
    ("gmail", "Gmail", "gmail"),
    ("outlook", "Outlook", "outlook|hotmail"),
    ("google_drive", "Google Drive", "google drive|gdrive"),
    ("google_calendar", "Google Agenda", "google agenda|google calendar|google kalender"),
    ("dropbox", "Dropbox", "dropbox"),
    ("onedrive", "OneDrive", "onedrive"),
    ("icloud", "iCloud", "icloud"),
    ("notion", "Notion", "notion"),
    ("trello", "Trello", "trello"),
    ("todoist", "Todoist", "todoist"),
    ("slack", "Slack", "slack"),
    ("github", "GitHub", "github"),
    ("spotify", "Spotify", "spotify"),
    ("deezer", "Deezer", "deezer"),
    ("strava", "Strava", "strava"),
    ("garmin", "Garmin", "garmin"),
    ("finary", "Finary", "finary"),
    ("airtable", "Airtable", "airtable"),
];

pub fn service_name(id: &str) -> Option<&'static str> {
    SERVICES.iter().find(|(i, _, _)| *i == id).map(|(_, n, _)| *n)
}

fn rx(pattern: &str) -> Regex {
    Regex::new(&format!("(?i){pattern}")).expect("valid regex")
}

static PROJECT: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"\b(voyage|vacances|week-?end (à|a|en|au|chez|entre)|road ?trip|anniversaire|soirée|fête|mariage|evjf|evg|crémaillère|trip|holidays?|vacation|birthday party|party|reise|urlaub|geburtstag|feier|viaje|vacaciones|cumpleaños|fiesta|boda)\b",
    )
});
static DOCUMENT: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"\b(factures?|ordonnances?|contrats?|quittances?|reçus?|attestations?|passeport|carte d'identité|carte grise|bulletin de (salaire|paie)|fiche de paie|avis d'imposition|relevés?|documents?|papiers?|invoices?|receipts?|contracts?|prescriptions?|passport|payslips?|rechnung|vertrag|quittung|rezept|reisepass|dokument|factura|recibo|contrato|receta|pasaporte|nómina)\b",
    )
});
static ROUTINE: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(rappelle[- ]moi|fais[- ]moi penser|tous les (jours|matins|soirs|lundis|mardis|mercredis|jeudis|vendredis|samedis|dimanches|week-?ends)|chaque (jour|matin|soir|semaine|mois|lundi|mardi|mercredi|jeudi|vendredi|samedi|dimanche)|remind me|every (day|morning|evening|night|week|monday|tuesday|wednesday|thursday|friday|saturday|sunday)|erinnere mich|jeden (tag|morgen|abend|montag|dienstag|mittwoch|donnerstag|freitag)|jede woche|recuérdame|recuerdame|cada (día|dia|mañana|noche|semana|lunes|martes|miércoles|jueves|viernes))",
    )
});
static VAULT: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"\b(mots? de passe|mdp|identifiants?|code d'accès|passwords?|login|passwort|kennwort|zugangsdaten|contraseñas?)\b",
    )
});
static CARNET: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(retiens (que|bien)|souviens[- ]toi|n'oublie pas que|note (que|bien)|remember (that|this)|keep in mind|merk dir|vergiss nicht|recuerda que|no olvides que)",
    )
});
static WATCH: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(préviens[- ]moi (quand|dès que|si)|previens[- ]moi (quand|dès que|des que|si)|alerte[- ]moi|dis[- ]moi (quand|dès que|des que)|surveille[- ]moi|let me know (when|if)|notify me (when|if)|tell me when|alert me|sag mir bescheid|benachrichtige mich|avísame|avisame|dime cuando)",
    )
});
static JOBS: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"\b(compare[rz]?|comparatif|fais (une|des) recherches?|cherche[- ]moi|trouve[- ]moi les meilleur|research|look into|vergleiche|recherchiere|compara|investiga)\b",
    )
});
static MORNING: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(météo|meteo|quel temps|mon agenda|programme (du jour|d'aujourd'hui)|qu'est-ce que j'ai aujourd'hui|weather|what's on today|my agenda|wetter|termine heute|el tiempo|mi agenda)",
    )
});
static FAMILY: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"\b(ma femme|mon mari|ma compagne|mon compagnon|ma fille|mon fils|mes enfants|les enfants|ma mère|mon père|maman|papa|ma s(œ|oe)ur|mon frère|mes parents|my wife|my husband|my partner|my daughter|my son|my kids|my children|my mom|my mum|my dad|my parents|meine frau|mein mann|meine tochter|mein sohn|meine kinder|meine mutter|mein vater|mi esposa|mi mujer|mi marido|mi hija|mi hijo|mis hijos|mi madre|mi padre|mamá|papá)\b",
    )
});
static SERVICE_RX: LazyLock<Vec<(&'static str, Regex)>> =
    LazyLock::new(|| SERVICES.iter().map(|(id, _, words)| (*id, rx(&format!(r"\b({words})\b")))).collect());

/// What a message hints at (in memory only): (feature, a signal key counted over days or None for « at once »).
fn signals(text: &str, kinds: &[String], hour: u32) -> Vec<(&'static str, Option<String>)> {
    let mut out: Vec<(&'static str, Option<String>)> = Vec::new();
    let has = |k: &str| kinds.iter().any(|x| x == k);
    if PROJECT.is_match(text) {
        out.push(("project", None));
    }
    if has("pdf") || ((has("image") || has("file")) && DOCUMENT.is_match(text)) {
        out.push(("archives", None));
    }
    if ROUTINE.is_match(text) {
        out.push(("routine", None));
    }
    if VAULT.is_match(text) {
        out.push(("vault", None));
    }
    if CARNET.is_match(text) {
        out.push(("carnet", None));
    }
    if JOBS.is_match(text) {
        out.push(("jobs", None));
    }
    if WATCH.is_match(text) {
        out.push(("watchers", None));
    }
    if hour < 11 && MORNING.is_match(text) {
        out.push(("morning_brief", None));
    }
    if FAMILY.is_match(text) {
        out.push(("family", Some("family".into())));
    }
    if has("voice") {
        out.push(("live", Some("voice".into())));
    }
    for (id, re) in SERVICE_RX.iter() {
        if re.is_match(text) {
            out.push(("connectors", Some(format!("service:{id}"))));
        }
    }
    out
}

/// How many different days a counted signal needs.
fn days_needed(key: &str) -> usize {
    match key {
        "family" => 2,
        "voice" => 1,
        _ => 3, // a service
    }
}

/// The tool a run called → the feature it is a use of (and whether it is a lasting setup).
pub fn feature_for_tool(tool: &str) -> Option<(&'static str, bool)> {
    let t = crate::loop_brake::bare(tool).to_ascii_lowercase();
    Some(match t.as_str() {
        "create_podcast" => ("podcast", false),
        "archive_records" => ("archives", false),
        "create_project_agent" => ("project", true),
        "tell_family" => ("family", false),
        "start_task" | "propose_task" => ("jobs", false),
        "cronjob" => ("routine", true),
        _ if t.starts_with("notebook_") || t == "notebook_fill" => ("carnet", false),
        _ if t.contains("vault") => ("vault", false),
        _ if t.contains("connector") => ("connectors", true),
        _ => return None,
    })
}

// --- the store ---------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FeatureState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tried_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopted_at: Option<f64>,
    #[serde(default)]
    pub declined: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub later_until: Option<f64>,
    /// The different days it was used (the last few).
    #[serde(default)]
    pub use_days: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Stats {
    #[serde(default)]
    pub proposed: u32,
    #[serde(default)]
    pub accepted: u32,
    #[serde(default)]
    pub later: u32,
    #[serde(default)]
    pub declined: u32,
    #[serde(default)]
    pub tried: u32,
    #[serde(default)]
    pub adopted: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    pub agent: String,
    pub feature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    pub at: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Queued {
    feature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    service: Option<String>,
    at: f64,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default)]
    started_at: Option<f64>,
    #[serde(default)]
    features: BTreeMap<String, FeatureState>,
    #[serde(default)]
    queued: Vec<Queued>,
    #[serde(default)]
    cards: Vec<Card>,
    #[serde(default)]
    notes: BTreeMap<String, Vec<String>>,
    /// The local date of the last proposal (one a day).
    #[serde(default)]
    last_day: Option<String>,
    /// The calendar's days done.
    #[serde(default)]
    calendar_done: Vec<i64>,
    /// Messages per agent per day (the last 14 days): who the person talks to most.
    #[serde(default)]
    talks: BTreeMap<String, BTreeMap<String, u32>>,
    /// Counted signals: the different days each was seen (family, a service…).
    #[serde(default)]
    signal_days: BTreeMap<String, Vec<String>>,
    /// Repeated requests: a salted hash of the first words → (day, hour) seen.
    #[serde(default)]
    repeats: BTreeMap<String, Vec<(String, u32)>>,
    #[serde(default)]
    salt: String,
    #[serde(default)]
    stats: BTreeMap<String, Stats>,
    /// The last « Essayer » (feature, when): a routine made right after counts for it.
    #[serde(default)]
    last_try: Option<(String, f64)>,
    /// Quiet hours and the podcast's day (settings kept here; the defaults are QUIET and PODCAST_DAY).
    #[serde(default)]
    quiet: Option<[u32; 2]>,
    #[serde(default)]
    podcast_day: Option<u32>,
}

impl Store {
    /// A new store: suggestions on (the derived default would have them off).
    fn fresh() -> Self {
        Self { enabled: true, ..Self::default() }
    }

    fn state(&mut self, id: &str) -> &mut FeatureState {
        self.features.entry(id.to_string()).or_default()
    }

    fn stat(&mut self, id: &str) -> &mut Stats {
        self.stats.entry(id.to_string()).or_default()
    }

    /// Not tried, not adopted, not declined, not waiting (« Plus tard », a card).
    fn eligible(&self, id: &str, now: f64) -> bool {
        let s = self.features.get(id).cloned().unwrap_or_default();
        s.tried_at.is_none()
            && s.adopted_at.is_none()
            && !s.declined
            && s.later_until.is_none_or(|t| now >= t)
            && !self.cards.iter().any(|c| c.feature == id)
    }
}

fn store_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("tips.json")
}

fn load(config: &Config) -> Store {
    std::fs::read_to_string(store_path(config))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(Store::fresh)
}

fn save(config: &Config, store: &Store) -> std::io::Result<()> {
    let file = store_path(config);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = file.with_extension("tmp");
    let text = serde_json::to_string(store).map_err(std::io::Error::other)?;
    py::write_with_mode(&temporary.to_string_lossy(), text.as_bytes(), 0o600)?;
    std::fs::rename(&temporary, &file)
}

fn change<T>(config: &Config, f: impl FnOnce(&mut Store) -> T) -> std::io::Result<T> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = load(config);
    let out = f(&mut store);
    save(config, &store)?;
    Ok(out)
}

// --- time --------------------------------------------------------------------------------------------------------------

fn local(now: f64) -> chrono::DateTime<chrono::Local> {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(now as i64, 0).single().unwrap_or_else(chrono::Local::now)
}

fn day_of(now: f64) -> String {
    local(now).format("%Y-%m-%d").to_string()
}

fn hour_of(now: f64) -> u32 {
    use chrono::Timelike;
    local(now).hour()
}

fn weekday_of(now: f64) -> u32 {
    use chrono::Datelike;
    local(now).weekday().number_from_monday()
}

fn days_between(from: f64, to: f64) -> i64 {
    let a = local(from).date_naive();
    let b = local(to).date_naive();
    (b - a).num_days()
}

fn quiet(store: &Store, hour: u32) -> bool {
    let [from, to] = store.quiet.unwrap_or(QUIET);
    if from == to {
        return false;
    }
    if from < to {
        hour >= from && hour < to
    } else {
        hour >= from || hour < to
    }
}

// --- uses and adoption ---------------------------------------------------------------------------------------------------

/// A use of `id` recorded. Returns the costume its first try unlocks, if any.
fn record_use(store: &mut Store, id: &str, now: f64, setup: bool) -> Option<&'static str> {
    let f = feature(id)?;
    let today = day_of(now);
    let unlocked = |s: &Store| -> Vec<&'static str> {
        COSTUMES
            .iter()
            .copied()
            .filter(|c| {
                FEATURES.iter().any(|f| f.costume == *c && s.features.get(f.id).is_some_and(|x| x.tried_at.is_some()))
            })
            .collect()
    };
    let before = unlocked(store);
    let state = store.state(id);
    let first = state.tried_at.is_none();
    if first {
        state.tried_at = Some(now);
    }
    if !state.use_days.contains(&today) {
        state.use_days.push(today);
        if state.use_days.len() > 5 {
            state.use_days.remove(0);
        }
    }
    let adopt = state.adopted_at.is_none() && ((setup && f.lasting) || state.use_days.len() >= 3);
    if adopt {
        state.adopted_at = Some(now);
    }
    if first {
        store.stat(id).tried += 1;
    }
    if adopt {
        store.stat(id).adopted += 1;
    }
    // used: its card and its noticed moment are moot
    store.cards.retain(|c| c.feature != id);
    store.queued.retain(|q| q.feature != id);
    let after = unlocked(store);
    after.into_iter().find(|c| !before.contains(c))
}

/// A tool an agent called (run hub `tool.started`): a use of its feature. Cheap when the tool maps to none.
pub fn tool_used(config: &Config, tool: &str, now: f64) {
    let Some((id, setup)) = feature_for_tool(tool) else { return };
    let _ = change(config, |store| {
        record_use(store, id, now, setup);
        // a routine made right after trying the morning brief or the podcast is that feature, set up
        if id == "routine" {
            if let Some((tried, at)) = store.last_try.clone() {
                if (tried == "morning_brief" || tried == "podcast") && now - at < TRY_WINDOW {
                    record_use(store, &tried, now, true);
                    if let Some(s) = store.features.get_mut(&tried) {
                        if s.adopted_at.is_none() {
                            s.adopted_at = Some(now);
                            store.stats.entry(tried.clone()).or_default().adopted += 1;
                        }
                    }
                }
            }
        }
    });
}

/// The apps report a use (`POST /v1/tips/used`), or the bridge does (a reply read aloud, a Live call).
pub fn used(config: &Config, id: &str, now: f64) -> anyhow::Result<Option<&'static str>> {
    if feature(id).is_none() {
        anyhow::bail!("feature: unknown");
    }
    Ok(change(config, |store| record_use(store, id, now, false))?)
}

// --- the person's message ------------------------------------------------------------------------------------------------

/// What the bridge knows about the moment of the person's message.
#[derive(Debug, Clone, Default)]
pub struct Moment {
    /// A run of this agent goes on in this conversation, or one of its background tasks runs.
    pub busy: bool,
    /// The place's guide (« Bip »), when it has one.
    pub guide: Option<String>,
}

fn salted(store: &mut Store, text: &str) -> Option<String> {
    let words: Vec<String> = text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(4)
        .map(str::to_string)
        .collect();
    if words.len() < 2 {
        return None;
    }
    if store.salt.is_empty() {
        store.salt = py::token_hex(8);
    }
    let digest = Sha256::digest(format!("{}:{}", store.salt, words.join(" ")).as_bytes());
    Some(digest.iter().take(6).map(|b| format!("{b:02x}")).collect())
}

fn queue(store: &mut Store, id: &str, service: Option<String>, now: f64) {
    if !store.eligible(id, now) || store.queued.iter().any(|q| q.feature == id) {
        return;
    }
    store.queued.push(Queued { feature: id.to_string(), service, at: now });
}

/// The signals of one message counted and the moments queued; the text is not kept.
fn notice(store: &mut Store, text: &str, kinds: &[String], now: f64) {
    let today = day_of(now);
    let hour = hour_of(now);
    for (id, key) in signals(text, kinds, hour) {
        match key {
            None => queue(store, id, None, now),
            Some(key) => {
                let days = store.signal_days.entry(key.clone()).or_default();
                if key == "voice" {
                    // voice notes: the second one (any day)
                    days.push(today.clone());
                    if days.len() > 5 {
                        days.remove(0);
                    }
                } else if !days.contains(&today) {
                    days.push(today.clone());
                    if days.len() > 5 {
                        days.remove(0);
                    }
                }
                let needed = if key == "voice" { 2 } else { days_needed(&key) };
                if days.len() >= needed {
                    let service = key.strip_prefix("service:").map(str::to_string);
                    queue(store, id, service, now);
                }
            }
        }
    }
    // the same request on 3 different days around the same hour: a routine
    if let Some(hash) = salted(store, text) {
        let seen = store.repeats.entry(hash).or_default();
        if !seen.iter().any(|(d, _)| *d == today) {
            seen.push((today.clone(), hour));
        }
        let near = seen.iter().filter(|(_, h)| (*h as i64 - hour as i64).abs() <= 1).count();
        if near >= 3 {
            queue(store, "routine", None, now);
        }
        // the oldest go
        let cutoff = local(now - TALK_DAYS as f64 * DAY).format("%Y-%m-%d").to_string();
        store.repeats.retain(|_, seen| {
            seen.retain(|(d, _)| *d >= cutoff);
            !seen.is_empty()
        });
        while store.repeats.len() > REPEATS_MAX {
            let Some(first) = store.repeats.keys().next().cloned() else { break };
            store.repeats.remove(&first);
        }
    }
}

fn count_talk(store: &mut Store, agent: &str, now: f64) {
    let today = day_of(now);
    *store.talks.entry(agent.to_string()).or_default().entry(today).or_default() += 1;
    let cutoff = local(now - TALK_DAYS as f64 * DAY).format("%Y-%m-%d").to_string();
    store.talks.retain(|_, days| {
        days.retain(|d, _| *d >= cutoff);
        !days.is_empty()
    });
}

/// Who proposes: the guide, else the agent written to most these 14 days (`fallback` on a tie or none).
fn proposer(store: &Store, config: &Config, moment: &Moment, fallback: &str) -> String {
    if let Some(guide) = moment.guide.as_deref().filter(|g| config.agent(Some(g)).is_some()) {
        return guide.to_string();
    }
    let total = |a: &str| store.talks.get(a).map(|d| d.values().sum::<u32>()).unwrap_or(0);
    let mut best = (fallback.to_string(), total(fallback));
    for agent in store.talks.keys() {
        if config.agent(Some(agent)).is_some() && total(agent) > best.1 {
            best = (agent.clone(), total(agent));
        }
    }
    best.0
}

/// The next feature not seen, in the table's order.
fn next_unseen(store: &Store, now: f64) -> Option<&'static str> {
    FEATURES
        .iter()
        .map(|f| f.id)
        .find(|id| store.features.get(*id).is_none_or(|s| s.proposed_at.is_none()) && store.eligible(id, now))
}

/// What to propose now: a noticed moment first, then Friday's podcast, then the calendar. (feature, service, week).
fn pick(store: &mut Store, now: f64) -> Option<(&'static str, Option<String>, bool)> {
    store.queued.retain(|q| now - q.at < QUEUE_TTL);
    let queued: Vec<Queued> = store.queued.clone();
    for q in queued {
        if let Some(f) = feature(&q.feature).filter(|f| store.eligible(f.id, now)) {
            store.queued.retain(|x| x.feature != q.feature);
            return Some((f.id, q.service, false));
        }
    }
    let started = store.started_at.unwrap_or(now);
    let day = days_between(started, now);
    let active_days: usize = {
        let mut days: Vec<&String> = store.talks.values().flat_map(|d| d.keys()).collect();
        days.sort();
        days.dedup();
        days.len()
    };
    if weekday_of(now) == store.podcast_day.unwrap_or(PODCAST_DAY) && active_days >= 3 && store.eligible("podcast", now)
    {
        return Some(("podcast", None, false));
    }
    if day > LAST_DAY {
        return None;
    }
    let step = CALENDAR.iter().rev().find(|(d, _)| *d <= day && !store.calendar_done.contains(d)).copied()?;
    // the steps passed by meanwhile are done too
    for (d, _) in CALENDAR.iter().filter(|(d, _)| *d <= step.0) {
        if !store.calendar_done.contains(d) {
            store.calendar_done.push(*d);
        }
    }
    let (id, week) = match step.1 {
        "week" => (next_unseen(store, now)?, true),
        "next" => (next_unseen(store, now)?, false),
        "family"
            if store.features.get("family").is_some_and(|s| s.tried_at.is_some()) || !store.eligible("family", now) =>
        {
            if store.eligible("project", now) {
                ("project", false)
            } else {
                (next_unseen(store, now)?, false)
            }
        }
        id if store.eligible(id, now) => (feature(id)?.id, false),
        _ => (next_unseen(store, now)?, false),
    };
    Some((id, None, week))
}

fn hint_for(f: &Feature, service: Option<&str>) -> String {
    match (f.id, service.and_then(service_name)) {
        ("connectors", Some(name)) => {
            format!("you can make a connector to {name}, which they mention often, to work with it directly")
        }
        _ => f.hint.to_string(),
    }
}

fn example_for(f: &Feature, service: Option<&str>) -> String {
    match (f.id, service.and_then(service_name)) {
        ("connectors", Some(name)) => {
            format!("Comme tu parles souvent de {name}, je peux te faire un connecteur pour m'en occuper directement.")
        }
        _ => f.example.to_string(),
    }
}

fn tip_note(f: &Feature, service: Option<&str>) -> String {
    format!(
        "A tip for your person, at a good moment (the bridge picked it): {}. After answering what they asked, add ONE short, \
warm sentence in their language that offers it, e.g. « {} ». A card « Essayer / Plus tard / Pas pour moi » waits for \
them in the app: don't explain it, don't insist, don't bring it up again.",
        hint_for(f, service),
        example_for(f, service)
    )
}

fn week_note(store: &Store, f: &Feature, now: f64) -> String {
    let week_ago = day_of(now - 7.0 * DAY);
    let messages: u32 =
        store.talks.values().flat_map(|d| d.iter()).filter(|(d, _)| **d > week_ago).map(|(_, n)| *n).sum();
    let used: Vec<&str> = FEATURES
        .iter()
        .filter(|x| store.features.get(x.id).is_some_and(|s| s.tried_at.is_some()))
        .map(|x| x.title)
        .collect();
    let used = if used.is_empty() { String::new() } else { format!("; they used: {}", used.join(", ")) };
    format!(
        "Your person started with their agents one week ago ({messages} messages this week{used}). After answering what \
they asked, give them a short, warm « ta première semaine » recap in their language (2 or 3 sentences): what you did \
together this week, from your conversation. Then offer ONE thing to try: {} (e.g. « {} »). A card « Essayer / Plus \
tard / Pas pour moi » waits for them in the app: don't explain it, don't insist.",
        f.hint, f.example
    )
}

fn sentence(f: &Feature, service: Option<&str>) -> String {
    match (f.id, service.and_then(service_name)) {
        ("connectors", Some(name)) => format!("{} : je peux relier {name}, pour m'en occuper directement.", f.title),
        _ => format!("{} : {}", f.title, f.line),
    }
}

fn action(f: &Feature) -> Value {
    match f.open {
        Some(target) => json!({"kind": "open", "target": target}),
        None => json!({"kind": "message"}),
    }
}

fn wrap(lines: &[String]) -> String {
    format!("<!--bip:tip\n{}\n-->", lines.join("\n"))
}

/// The person wrote to `agent` (`/v1/live/recaps/take`): the message's signals counted (the text dropped), and what
/// waits for this agent's turn: its notes, and a tip when the moment allows one. Returns the hidden part, if any.
pub fn on_message(
    config: &Config,
    agent: &str,
    session: &str,
    text: &str,
    kinds: &[String],
    moment: &Moment,
    now: f64,
) -> Option<String> {
    if session.starts_with("cron_") {
        return None; // a routine's run: never
    }
    let text: String = text.chars().take(TEXT_MAX).collect();
    let out = change(config, |store| {
        if !store.enabled {
            return None;
        }
        store.started_at.get_or_insert(now);
        count_talk(store, agent, now);
        notice(store, &text, kinds, now);
        // the cards left unanswered go (as « Plus tard »)
        let expired: Vec<String> =
            store.cards.iter().filter(|c| now - c.at >= CARD_TTL).map(|c| c.feature.clone()).collect();
        store.cards.retain(|c| now - c.at < CARD_TTL);
        for id in expired {
            store.state(&id).later_until = Some(now + LATER);
        }
        let mut lines = store.notes.remove(agent).unwrap_or_default();
        let today = day_of(now);
        let allowed = !moment.busy
            && !quiet(store, hour_of(now))
            && store.last_day.as_deref() != Some(today.as_str())
            && store.cards.is_empty()
            && proposer(store, config, moment, agent) == agent;
        if allowed {
            if let Some((id, service, week)) = pick(store, now) {
                let f = feature(id)?;
                let card = Card {
                    id: py::token_hex(6),
                    agent: agent.to_string(),
                    feature: id.to_string(),
                    service: service.clone(),
                    at: now,
                };
                lines.push(if week { week_note(store, f, now) } else { tip_note(f, service.as_deref()) });
                store.state(id).proposed_at = Some(now);
                store.stat(id).proposed += 1;
                store.last_day = Some(today);
                store.cards.push(card);
                tracing::info!(target: "bipbridge::tips", agent, feature = id, week, "feature proposed");
            }
        }
        (!lines.is_empty()).then(|| wrap(&lines))
    });
    out.ok().flatten()
}

// --- the cards -------------------------------------------------------------------------------------------------------------

fn item_of(card: &Card) -> Option<Dict> {
    let f = feature(&card.feature)?;
    let mut tip = json!({"feature": f.id, "title": f.title, "line": f.line, "action": action(f), "costume": f.costume});
    if let Some(service) = &card.service {
        tip["service"] = json!(service);
        tip["service_name"] = json!(service_name(service));
    }
    json!({
        "agent": card.agent,
        "run_id": format!("{RUN_PREFIX}{}", card.id),
        "request_id": card.id,
        "command": format!("tip · {}", f.id),
        "description": sentence(f, card.service.as_deref()),
        "choices": CHOICES,
        "created_at": card.at,
        "session_id": null,
        "bip_tip": tip,
    })
    .as_object()
    .cloned()
}

/// The cards waiting, in Hermes' approvals' shape (`GET /v1/approvals`).
pub fn pending_items(config: &Config, agent: Option<&str>) -> Vec<Dict> {
    let now = py::time_time();
    let store = load(config);
    if !store.enabled {
        return Vec::new();
    }
    store
        .cards
        .iter()
        .filter(|c| agent.is_none_or(|a| a == c.agent) && now - c.at < CARD_TTL)
        .filter_map(item_of)
        .collect()
}

fn try_body(f: &Feature, service: Option<&str>, costume: Option<&str>) -> Value {
    let mut out = json!({"ok": true, "choice": "try", "feature": f.id, "action": action(f)});
    if let Some(s) = service {
        out["service"] = json!(s);
        out["service_name"] = json!(service_name(s));
    }
    if let Some(c) = costume {
        out["costume"] = json!(c);
    }
    out
}

/// « Essayer » (from the card or the page): tried, the last try kept, the agent told. Returns the costume unlocked.
fn tried(store: &mut Store, f: &Feature, agent: Option<&str>, now: f64) -> Option<&'static str> {
    store.stat(f.id).accepted += 1;
    store.last_try = Some((f.id.to_string(), now));
    let costume = record_use(store, f.id, now, false);
    if let Some(agent) = agent {
        let line = match f.open {
            Some(_) => format!("Your person tapped « Essayer » on your tip ({}): the app opened it for them.", f.title),
            None => {
                format!("Your person tapped « Essayer » on your tip ({}): their next message asks for it.", f.title)
            }
        };
        store.notes.entry(agent.to_string()).or_default().push(line);
    }
    costume
}

/// A card answered. Ok(None): no such card. Err: a choice that does not apply.
pub fn answer(config: &Config, agent: &str, id: &str, choice: &str, via: &str) -> Result<Option<Value>, String> {
    let choice = choice.trim().to_lowercase();
    // Hermes' words too (a notification's buttons): once/always → try, deny → later
    let choice = match choice.as_str() {
        "once" | "session" | "always" => "try".to_string(),
        "deny" => "later".to_string(),
        other => other.to_string(),
    };
    if !CHOICES.contains(&choice.as_str()) {
        return Err("choice: one of try, later, never".into());
    }
    let now = py::time_time();
    let done = change(config, |store| {
        let card = store.cards.iter().find(|c| c.id == id && c.agent == agent).cloned()?;
        store.cards.retain(|c| c.id != id);
        let f = feature(&card.feature)?;
        let out = match choice.as_str() {
            "try" => {
                let costume = tried(store, f, Some(agent), now);
                try_body(f, card.service.as_deref(), costume)
            }
            "later" => {
                store.state(f.id).later_until = Some(now + LATER);
                store.stat(f.id).later += 1;
                json!({"ok": true, "choice": "later", "feature": f.id})
            }
            _ => {
                store.state(f.id).declined = true;
                store.stat(f.id).declined += 1;
                store.queued.retain(|q| q.feature != f.id);
                store
                    .notes
                    .entry(agent.to_string())
                    .or_default()
                    .push(format!("Your person doesn't want {} (« Pas pour moi »): never propose it again.", f.title));
                json!({"ok": true, "choice": "never", "feature": f.id})
            }
        };
        Some((card, out))
    })
    .map_err(|e| e.to_string())?;
    let Some((card, out)) = done else { return Ok(None) };
    if let Some(f) = feature(&card.feature) {
        let fields = json!({
            "session_id": null, "run_id": format!("{RUN_PREFIX}{id}"), "request_id": id,
            "command": format!("tip · {}", f.id), "description": sentence(f, card.service.as_deref()),
            "topic": "tip", "choice": choice, "via": via,
        });
        if let Some(map) = fields.as_object() {
            if let Err(exc) = crate::approvals_log::record(config, agent, map) {
                tracing::warn!(target: "bipbridge::tips", agent, error = %exc, "tip answer not kept");
            }
        }
    }
    tracing::info!(target: "bipbridge::tips", agent, feature = %card.feature, choice = %choice, "tip answered");
    Ok(Some(out))
}

/// « Essayer » on the page.
pub fn try_feature(config: &Config, id: &str) -> anyhow::Result<Value> {
    let Some(f) = feature(id) else { anyhow::bail!("feature: unknown") };
    let now = py::time_time();
    let costume = change(config, |store| {
        store.cards.retain(|c| c.feature != id);
        store.state(id).later_until = None;
        store.state(id).declined = false;
        tried(store, f, None, now)
    })?;
    Ok(try_body(f, None, costume))
}

pub fn set_enabled(config: &Config, on: bool) -> std::io::Result<()> {
    change(config, |store| {
        store.enabled = on;
        if !on {
            store.cards.clear();
            store.queued.clear();
            store.notes.clear();
        }
    })
}

/// `GET /v1/tips`.
pub fn view(config: &Config) -> Value {
    let now = py::time_time();
    let store = load(config);
    let features: Vec<Value> = FEATURES
        .iter()
        .map(|f| {
            let s = store.features.get(f.id).cloned().unwrap_or_default();
            let state = if s.adopted_at.is_some() {
                "adopted"
            } else if s.tried_at.is_some() {
                "tried"
            } else if s.proposed_at.is_some() {
                "proposed"
            } else {
                "unseen"
            };
            json!({
                "id": f.id, "title": f.title, "line": f.line, "state": state, "declined": s.declined,
                "later_until": s.later_until.filter(|t| *t > now), "proposed_at": s.proposed_at, "tried_at": s.tried_at,
                "adopted_at": s.adopted_at, "costume": f.costume, "action": action(f),
            })
        })
        .collect();
    let unlocked: Vec<&str> = COSTUMES
        .iter()
        .copied()
        .filter(|c| {
            !store.enabled
                || FEATURES
                    .iter()
                    .any(|f| f.costume == *c && store.features.get(f.id).is_some_and(|s| s.tried_at.is_some()))
        })
        .collect();
    let locked: Vec<&str> = COSTUMES.iter().copied().filter(|c| !unlocked.contains(c)).collect();
    json!({
        "enabled": store.enabled,
        "day": store.started_at.map(|s| days_between(s, now)),
        "features": features,
        "costumes": {"unlocked": unlocked, "locked": locked},
        "stats": store.stats,
    })
}

// --- routes ------------------------------------------------------------------------------------------------------------

/// `POST /v1/approve` for a `bip-tip-` card (app.rs).
pub async fn approve(state: &AppState, agent: &str, id: &str, choice: &str) -> ApiResult<Response> {
    let (config, name, id, choice) = (state.config(), agent.to_string(), id.to_string(), choice.to_string());
    let done = tokio::task::spawn_blocking(move || answer(&config, &name, &id, &choice, "app"))
        .await
        .map_err(anyhow::Error::from)?
        .map_err(|message| HttpError::new(422, message))?
        .ok_or_else(|| HttpError::new(404, "no such request (answered already?)"))?;
    Ok(web::ok(done))
}

/// Whether the agent is busy for a tip: a run in this conversation, or one of its background tasks running.
pub fn busy(state: &AppState, agent: &str, session: &str) -> bool {
    if !session.is_empty() && state.hub.session_busy(agent, session) {
        return true;
    }
    crate::jobs::tasks(&state.config()).iter().any(|job| {
        let st = job.get("state").and_then(Value::as_str).unwrap_or("");
        job.get("agent").and_then(Value::as_str) == Some(agent) && (st == "running" || st == "stopping")
    })
}

/// The guide's name, when the place has one.
pub fn guide_of(config: &Config) -> Option<String> {
    crate::guide::load(config).guide
}

async fn get_tips(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let out = tokio::task::spawn_blocking(move || view(&config)).await.map_err(anyhow::Error::from)?;
    Ok(web::ok(out))
}

#[derive(Debug, Clone, Deserialize)]
pub struct SettingsBody {
    pub enabled: bool,
}

impl Validate for SettingsBody {
    fn validate(&self, _errors: &mut Vec<FieldError>) {}
}

async fn put_settings(State(state): State<AppState>, Json(body): Json<SettingsBody>) -> ApiResult<Response> {
    let config = state.config();
    let out = tokio::task::spawn_blocking(move || -> std::io::Result<Value> {
        set_enabled(&config, body.enabled)?;
        Ok(view(&config))
    })
    .await
    .map_err(anyhow::Error::from)?
    .map_err(anyhow::Error::from)?;
    Ok(web::ok(out))
}

async fn post_try(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    if feature(&id).is_none() {
        return Err(HttpError::not_found());
    }
    let out = tokio::task::spawn_blocking(move || try_feature(&config, &id))
        .await
        .map_err(anyhow::Error::from)?
        .map_err(|e| HttpError::new(422, e.to_string()))?;
    Ok(web::ok(out))
}

#[derive(Debug, Clone, Deserialize)]
pub struct UsedBody {
    pub feature: String,
}

impl Validate for UsedBody {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        if feature(&self.feature).is_none() {
            errors.push(FieldError::new(
                "literal_error",
                vec![json!("body"), json!("feature")],
                "Input should be a known feature",
                json!(self.feature),
            ));
        }
    }
}

async fn post_used(State(state): State<AppState>, Json(body): Json<UsedBody>) -> ApiResult<Response> {
    let config = state.config();
    let costume = tokio::task::spawn_blocking(move || used(&config, &body.feature, py::time_time()))
        .await
        .map_err(anyhow::Error::from)?
        .map_err(|e| HttpError::new(422, e.to_string()))?;
    Ok(web::ok(json!({"ok": true, "costume": costume})))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tips", get(get_tips))
        .route("/tips/settings", put(put_settings))
        .route("/tips/used", post(post_used))
        .route("/tips/{feature}/try", post(post_try))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentConfig;

    fn setup() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        for name in ["vie", "max"] {
            let agent = AgentConfig { name: name.into(), display_name: name.to_uppercase(), ..AgentConfig::default() };
            config.agents.insert(name.into(), agent);
        }
        (dir, config)
    }

    /// Today at `hour`, local, plus `days`.
    fn at(days: i64, hour: u32) -> f64 {
        use chrono::TimeZone;
        let d = chrono::Local::now().date_naive() + chrono::Duration::days(days);
        let t = d.and_hms_opt(hour, 0, 0).unwrap();
        chrono::Local.from_local_datetime(&t).earliest().unwrap().timestamp() as f64
    }

    fn free() -> Moment {
        Moment::default()
    }

    fn say(config: &Config, agent: &str, text: &str, kinds: &[&str], now: f64) -> Option<String> {
        let kinds: Vec<String> = kinds.iter().map(|k| k.to_string()).collect();
        on_message(config, agent, "s1", text, &kinds, &free(), now)
    }

    fn no_podcast_day(config: &Config) {
        let _ = change(config, |s| s.podcast_day = Some(9));
    }

    #[test]
    fn a_trip_proposes_the_project_agent_once_a_day() {
        let (_dir, config) = setup();
        no_podcast_day(&config);
        let note = say(&config, "vie", "On prépare un voyage à Lisbonne avec des amis", &[], at(0, 10)).unwrap();
        assert!(note.starts_with("<!--bip:tip\n") && note.contains("project agent"), "{note}");
        let cards = pending_items(&config, Some("vie"));
        assert_eq!(cards.len(), 1);
        assert!(cards[0]["run_id"].as_str().unwrap().starts_with(RUN_PREFIX));
        assert_eq!(cards[0]["bip_tip"]["feature"], json!("project"));
        assert_eq!(cards[0]["choices"], json!(["try", "later", "never"]));
        // a PDF the same day: noticed, not proposed (one a day, a card waits)
        assert!(say(&config, "vie", "voici ma facture", &["pdf"], at(0, 11)).is_none());
        let id = cards[0]["request_id"].as_str().unwrap().to_string();
        assert_eq!(answer(&config, "vie", &id, "later", "app").unwrap().unwrap()["choice"], json!("later"));
        assert!(say(&config, "vie", "merci", &[], at(0, 12)).is_none(), "one a day");
        // the next day the PDF's moment comes
        let note = say(&config, "vie", "salut", &[], at(1, 10)).unwrap();
        assert!(note.contains("Archives"), "{note}");
    }

    #[test]
    fn tell_me_when_hints_at_a_watcher() {
        let ids = |t: &str| signals(t, &[], 12).into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        assert!(ids("Préviens-moi quand un créneau se libère chez le dentiste").contains(&"watchers"));
        assert!(ids("let me know when the price drops").contains(&"watchers"));
        assert!(ids("avísame cuando llegue el paquete").contains(&"watchers"));
        assert!(!ids("quel temps fait-il ?").contains(&"watchers"));
        assert_eq!(feature("watchers").map(|f| f.lasting), Some(true));
    }

    #[test]
    fn quiet_hours_and_busy_wait() {
        let (_dir, config) = setup();
        no_podcast_day(&config);
        assert!(say(&config, "vie", "rappelle-moi d'appeler le garage", &[], at(0, 23)).is_none(), "night");
        assert!(say(&config, "vie", "et demain ?", &[], at(1, 6)).is_none(), "still night");
        let busy = Moment { busy: true, guide: None };
        assert!(on_message(&config, "vie", "s1", "ok", &[], &busy, at(1, 9)).is_none(), "a task goes on");
        let note = say(&config, "vie", "ok", &[], at(1, 9)).unwrap();
        assert!(note.contains("routine"), "{note}");
    }

    #[test]
    fn never_in_a_routine_and_never_again() {
        let (_dir, config) = setup();
        no_podcast_day(&config);
        let kinds: Vec<String> = vec![];
        assert!(on_message(&config, "vie", "cron_abc_20261028_080000", "mon mot de passe", &kinds, &free(), at(0, 10))
            .is_none());
        assert!(load(&config).started_at.is_none(), "a routine's run isn't even counted");
        say(&config, "vie", "j'ai oublié mon mot de passe", &[], at(0, 10)).unwrap();
        let card = pending_items(&config, None)[0].clone();
        assert_eq!(card["bip_tip"]["feature"], json!("vault"));
        let id = card["request_id"].as_str().unwrap();
        answer(&config, "vie", id, "never", "app").unwrap().unwrap();
        // told to the agent, never proposed again
        let note = say(&config, "vie", "encore mon mot de passe", &[], at(1, 10)).unwrap();
        assert!(note.contains("never propose it again"));
        for d in 2..40 {
            if let Some(n) = say(&config, "vie", "mot de passe", &[], at(d, 10)) {
                assert!(!n.contains("the vault"), "day {d}: {n}");
            }
            for c in pending_items(&config, None) {
                let id = c["request_id"].as_str().unwrap().to_string();
                answer(&config, "vie", &id, "later", "app").unwrap();
            }
        }
        assert_eq!(load(&config).stats["vault"].declined, 1);
        assert_eq!(load(&config).stats["vault"].proposed, 1);
    }

    #[test]
    fn later_waits_seven_days() {
        let (_dir, config) = setup();
        no_podcast_day(&config);
        say(&config, "vie", "mon mot de passe", &[], at(0, 10)).unwrap();
        let id = pending_items(&config, None)[0]["request_id"].as_str().unwrap().to_string();
        answer(&config, "vie", &id, "later", "app").unwrap();
        let mut again = None;
        for d in 1..12 {
            let n = say(&config, "vie", "mon mot de passe", &[], at(d, 10));
            if n.as_deref().is_some_and(|n| n.contains("the vault")) {
                again = Some(d);
                break;
            }
            for c in pending_items(&config, None) {
                let id = c["request_id"].as_str().unwrap().to_string();
                answer(&config, "vie", &id, "never", "app").unwrap();
            }
        }
        assert!(again.is_some_and(|d| d >= 7), "{again:?}");
    }

    #[test]
    fn the_calendar_skips_what_is_used() {
        let (_dir, config) = setup();
        no_podcast_day(&config);
        assert!(say(&config, "vie", "bonjour", &[], at(0, 10)).is_none(), "day 0: nothing");
        used(&config, "voice", at(0, 12)).unwrap();
        let note = say(&config, "vie", "bonjour", &[], at(1, 10)).unwrap();
        assert!(note.contains("brief every morning"), "voice already used, the next: {note}");
        let id = pending_items(&config, None)[0]["request_id"].as_str().unwrap().to_string();
        let done = answer(&config, "vie", &id, "try", "app").unwrap().unwrap();
        assert_eq!(done["action"], json!({"kind": "message"}));
        assert_eq!(done["costume"], json!("chef"));
        // a routine made right after: the brief adopted
        tool_used(&config, "mcp_cronjob", at(1, 10) + 60.0);
        tool_used(&config, "cronjob", py::time_time() + 60.0); // the try was just now
        let v = view(&config);
        let brief = v["features"].as_array().unwrap().iter().find(|f| f["id"] == "morning_brief").unwrap().clone();
        assert_eq!(brief["state"], json!("adopted"));
        assert!(v["costumes"]["unlocked"].as_array().unwrap().contains(&json!("chef")));
        assert!(v["costumes"]["unlocked"].as_array().unwrap().contains(&json!("musique")), "voice was used");
        // day 7: the first week's recap
        let mut week = None;
        for d in 2..=7 {
            if let Some(n) = say(&config, "vie", "salut", &[], at(d, 10)) {
                if n.contains("ta première semaine") {
                    week = Some(d);
                }
            }
            for c in pending_items(&config, None) {
                let id = c["request_id"].as_str().unwrap().to_string();
                answer(&config, "vie", &id, "later", "app").unwrap();
            }
        }
        assert_eq!(week, Some(7));
        // nothing after day 35
        let _ = change(&config, |s| {
            s.started_at = Some(at(-40, 10));
            s.queued.clear();
        });
        assert!(say(&config, "vie", "salut", &[], at(30, 10)).is_none());
    }

    #[test]
    fn the_guide_or_the_agent_talked_to_most_proposes() {
        let (_dir, config) = setup();
        no_podcast_day(&config);
        for h in 9..14 {
            let _ = change(&config, |s| count_talk(s, "max", at(0, h)));
        }
        assert!(say(&config, "vie", "mon mot de passe", &[], at(0, 15)).is_none(), "max is talked to most");
        let note = say(&config, "max", "bonjour", &[], at(0, 15)).unwrap();
        assert!(note.contains("the vault"));
        // with a guide, the guide
        let (_dir2, config2) = setup();
        no_podcast_day(&config2);
        let guide = Moment { busy: false, guide: Some("vie".into()) };
        let none: Vec<String> = vec![];
        assert!(on_message(&config2, "max", "s", "mon mot de passe", &none, &guide, at(0, 10)).is_none());
        assert!(on_message(&config2, "vie", "s", "salut", &none, &guide, at(0, 10)).is_some());
    }

    #[test]
    fn counted_signals_need_several_days() {
        let (_dir, config) = setup();
        no_podcast_day(&config);
        let _ = change(&config, |s| s.calendar_done = CALENDAR.iter().map(|(d, _)| *d).collect());
        assert!(say(&config, "vie", "J'ai rangé ça dans Notion", &[], at(0, 10)).is_none());
        assert!(say(&config, "vie", "Notion encore", &[], at(0, 11)).is_none(), "same day");
        assert!(say(&config, "vie", "et Notion", &[], at(1, 10)).is_none());
        let note = say(&config, "vie", "toujours Notion", &[], at(2, 10)).unwrap();
        assert!(note.contains("connector to Notion"), "{note}");
        assert_eq!(pending_items(&config, None)[0]["bip_tip"]["service"], json!("notion"));
        let tips = std::fs::read_to_string(store_path(&config)).unwrap();
        assert!(!tips.contains("toujours") && !tips.contains("rangé"), "no content kept");
    }

    #[test]
    fn repeated_requests_and_voice_notes() {
        let (_dir, config) = setup();
        no_podcast_day(&config);
        let _ = change(&config, |s| s.calendar_done = CALENDAR.iter().map(|(d, _)| *d).collect());
        assert!(say(&config, "vie", "donne-moi les nouvelles du jour", &[], at(0, 8)).is_none());
        assert!(say(&config, "vie", "donne-moi les nouvelles du jour stp", &[], at(1, 8)).is_none());
        let note = say(&config, "vie", "donne-moi les nouvelles du jour", &[], at(2, 9)).unwrap();
        assert!(note.contains("routine"), "{note}");
        let (_dir2, config) = setup();
        no_podcast_day(&config);
        let _ = change(&config, |s| s.calendar_done = CALENDAR.iter().map(|(d, _)| *d).collect());
        assert!(say(&config, "vie", "", &["voice"], at(0, 10)).is_none());
        let note = say(&config, "vie", "", &["voice"], at(0, 11)).unwrap();
        assert!(note.contains("Live calls"));
    }

    #[test]
    fn adoption_from_tools_and_the_switch() {
        let (_dir, config) = setup();
        tool_used(&config, "mcp_bipagents_notebook_note", at(0, 10));
        tool_used(&config, "notebook_search", at(1, 10));
        assert_eq!(view(&config)["features"][2]["state"], json!("tried"));
        tool_used(&config, "notebook_search", at(2, 10));
        assert_eq!(view(&config)["features"][2]["state"], json!("adopted"));
        tool_used(&config, "create_project_agent", at(0, 10));
        let v = view(&config);
        let project = v["features"].as_array().unwrap().iter().find(|f| f["id"] == "project").unwrap().clone();
        assert_eq!(project["state"], json!("adopted"), "a lasting setup");
        assert_eq!(v["stats"]["carnet"]["adopted"], json!(1));
        set_enabled(&config, false).unwrap();
        assert!(say(&config, "vie", "mon mot de passe", &[], at(3, 10)).is_none());
        assert_eq!(view(&config)["costumes"]["locked"], json!([]), "off: every costume open");
        assert!(feature_for_tool("terminal").is_none());
    }

    #[test]
    fn friday_podcast() {
        let (_dir, config) = setup();
        let now = at(0, 10);
        let _ = change(&config, |s| {
            s.podcast_day = Some(weekday_of(now));
            s.calendar_done = CALENDAR.iter().map(|(d, _)| *d).collect();
            for d in 1..=3 {
                count_talk(s, "vie", now - d as f64 * DAY);
            }
        });
        let note = say(&config, "vie", "salut", &[], now).unwrap();
        assert!(note.contains("weekly podcast"), "{note}");
    }

    #[test]
    fn page_try_unlocks_a_costume() {
        let (_dir, config) = setup();
        let out = try_feature(&config, "archives").unwrap();
        assert_eq!(out["action"], json!({"kind": "open", "target": "archives"}));
        assert_eq!(out["costume"], json!("apprendre"));
        let out = try_feature(&config, "carnet").unwrap();
        assert!(out.get("costume").is_none(), "already unlocked");
        assert!(try_feature(&config, "nope").is_err());
    }
}
