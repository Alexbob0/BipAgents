//! A watcher's payment, quickly (Alex 13/11, internal/notes/watcher-urgent-pay.md): what card_pay.rs `urgent` needs
//! around it.
//!
//! - **Its words**: the urgent push (« A trouvé : Billet Stromae à 45,00 € sur ticketmaster.fr »), the « trop tard »
//!   line, the test alert, in the agent's language.
//! - **Too late**: after `card_pay::URGENT_WINDOW` without the person's « Oui, payer », the payment is refused to the
//!   agent and a « Panier prêt » card of the bridge's own (`bip-cart-<hov id>`, the hand-over's text with the
//!   checkout address the bridge read in Chrome) waits `KEEP_SECONDS` in the turn's conversation, pushed with the
//!   « trop tard » line. The apps draw it as the hand-over's card: « Payer sur mon téléphone » opens the shop,
//!   « Payer dans le navigateur de l'agent » the live view (the agent's browser is still on that page); their
//!   `/v1/approve` only puts it away.
//! - **« Prêt à acheter »**: a watcher made for a purchase (`watch.purchase`) shows a checklist (`purchase_view`):
//!   the shop's customer account, the card, the rehearsal; the phone adds its own steps (the code remembered there,
//!   urgent alerts allowed). Its actions: `POST …/connectors/<id>/purchase/account` (the agent creates the account
//!   with `vault_create_login`), `…/purchase/rehearse` (the agent goes up to the payment page without paying, then
//!   empties the cart; `pay_with_card` is refused meanwhile), `POST /v1/agents/<a>/urgent-test` (a test alert).
//!
//!     <data>/urgent-carts.json (0600)   [{id, agent, site, url, merchant, cents, currency, session, at, until}]
//!
//! No Python counterpart.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::background_turn::{Delivery, Kind, Pipeline, Turn};
use crate::card_pay::About;
use crate::cards;
use crate::config::{AgentConfig, Config};
use crate::connectors::{Connector, Purchase};
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

pub const CART_PREFIX: &str = "bip-cart-";
const FILE: &str = "urgent-carts.json";
/// A « Panier prêt » of a payment come too late waits this long.
pub const KEEP_SECONDS: f64 = 24.0 * 3600.0;
/// How long the agent's turn of a rehearsal or an account set-up may take.
const SETUP_MAX_SECONDS: f64 = 900.0;

/// What the agent reads when the window is over.
pub const TOO_LATE: &str = "The person did not answer within 10 minutes: nothing was paid or typed. A « Panier prêt » \
card now waits for them to buy it themselves (don't call hand_over_payment). End your turn with one short sentence.";

/// What `pay_with_card` answers during a rehearsal.
pub const REHEARSAL_NO_PAY: &str = "This is a rehearsal: never pay. Stop at the payment page, then empty the cart.";

// --- words -----------------------------------------------------------------------------------------------------------

/// « 45,00 € » (fr, de, es), « €45.00 » (en); another currency by its code.
pub fn money(lang: &str, cents: i64, currency: &str) -> String {
    let plain = cards::format_cents(cents);
    let symbol = match currency {
        "EUR" => "€",
        "USD" => "$",
        "GBP" => "£",
        "CHF" => "CHF",
        other => other,
    };
    match lang {
        "en" if symbol.chars().count() == 1 => format!("{symbol}{plain}"),
        "en" => format!("{plain} {symbol}"),
        _ => format!("{} {symbol}", plain.replace('.', ",")),
    }
}

/// The urgent push's words (the agent's name is its title): what, how much, where.
pub fn found_line(lang: &str, about: &About) -> String {
    let price = money(lang, about.cents, &about.currency);
    let (what, site) = (about.merchant.as_str(), about.site.as_str());
    match lang {
        "de" => format!("Gefunden: {what} für {price} auf {site}"),
        "es" => format!("Encontrado: {what} a {price} en {site}"),
        "en" => format!("Found: {what} for {price} on {site}"),
        _ => format!("A trouvé : {what} à {price} sur {site}"),
    }
}

/// The « trop tard » line (the « Panier prêt » card's push).
pub fn late_line(lang: &str, about: &About) -> String {
    let (what, site) = (about.merchant.as_str(), about.site.as_str());
    match lang {
        "de" => format!("Zu spät zum Bezahlen, der Warenkorb wartet auf dich: {what} auf {site}"),
        "es" => format!("Demasiado tarde para pagar, el carrito te espera: {what} en {site}"),
        "en" => format!("Too late to pay, the cart is waiting for you: {what} on {site}"),
        _ => format!("Trop tard pour payer, le panier t'attend : {what} sur {site}"),
    }
}

/// « Faire un essai »'s alert.
pub fn test_line(lang: &str) -> &'static str {
    match lang {
        "de" => "Test: So warnt dich dein Agent, wenn schnell bezahlt werden muss.",
        "es" => "Prueba: así te avisa tu agente cuando hay que pagar rápido.",
        "en" => "Test: this is how your agent alerts you when you need to pay fast.",
        _ => "Essai : c'est ainsi que ton agent te prévient quand il faut payer vite.",
    }
}

// --- « Panier prêt » after the window --------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cart {
    pub id: String,
    pub agent: String,
    pub site: String,
    pub url: String,
    pub merchant: String,
    pub cents: i64,
    pub currency: String,
    pub session: Option<String>,
    pub at: f64,
    pub until: f64,
}

static LOCK: Mutex<()> = Mutex::new(());

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join(FILE)
}

fn load(config: &Config) -> Vec<Cart> {
    std::fs::read_to_string(path(config)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn change<T>(config: &Config, f: impl FnOnce(&mut Vec<Cart>) -> T) -> T {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut carts = load(config);
    let out = f(&mut carts);
    let now = py::time_time();
    carts.retain(|c| now < c.until);
    let file = path(config);
    let saved = (|| -> std::io::Result<()> {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = file.with_extension("tmp");
        let text = serde_json::to_string(&carts).map_err(std::io::Error::other)?;
        py::write_with_mode(&temporary.to_string_lossy(), text.as_bytes(), 0o600)?;
        std::fs::rename(&temporary, &file)
    })();
    if let Err(err) = saved {
        tracing::warn!(target: "bipbridge::urgent_pay", error = %err, "carts not kept");
    }
    out
}

/// A « Panier prêt » kept for `agent` (the payment `about`, asked in `session`).
pub fn keep(config: &Config, agent: &str, about: &About, session: Option<&str>) -> Cart {
    let now = py::time_time();
    let cart = Cart {
        id: format!("hov_{}", &py::uuid4_hex()[..12]),
        agent: agent.to_string(),
        site: about.site.clone(),
        url: about.url.clone(),
        merchant: about.merchant.clone(),
        cents: about.cents,
        currency: about.currency.clone(),
        session: session.map(str::to_string),
        at: now,
        until: now + KEEP_SECONDS,
    };
    change(config, |carts| carts.push(cart.clone()));
    cart
}

/// The « Panier prêt » cards waiting, in Hermes' approvals' shape: the hand-over's own text (the apps draw it so).
pub fn pending_items(config: &Config, agent: Option<&str>) -> Vec<Dict> {
    if !path(config).exists() {
        return Vec::new();
    }
    let now = py::time_time();
    load(config)
        .iter()
        .filter(|c| now < c.until && agent.is_none_or(|a| a == c.agent))
        .filter_map(|c| {
            let message = crate::handover::approval_message(&c.id, c.cents, &c.currency, &c.site, &c.merchant, &c.url);
            let lang = crate::mail_tidy::language(config, &c.agent);
            let about = About { merchant: c.merchant.clone(), site: c.site.clone(), ..About::default() };
            json!({
                "agent": c.agent, "run_id": format!("{CART_PREFIX}{}", c.id), "request_id": c.id,
                "command": message, "description": message, "choices": ["once", "deny"], "created_at": c.at,
                "session_id": c.session, "pattern_key": "mcp_elicitation", "bip_summary": late_line(&lang, &about),
            })
            .as_object()
            .cloned()
        })
        .collect()
}

/// A « Panier prêt » put away (the person opened the shop, or said no). False: none waiting by that id.
pub fn answer(config: &Config, agent: &str, id: &str) -> bool {
    change(config, |carts| {
        let before = carts.len();
        carts.retain(|c| !(c.agent == agent && c.id == id));
        carts.len() < before
    })
}

/// The window is over (card_pay.rs `pay`): the « Panier prêt » card in the turn's conversation, pushed with the « trop
/// tard » line.
pub async fn too_late(state: &AppState, agent: &AgentConfig, about: &About, run: Option<&str>, session: Option<&str>) {
    let config = state.config();
    let (c, name, a, s) = (config.clone(), agent.name.clone(), about.clone(), session.map(str::to_string));
    let Ok(cart) = tokio::task::spawn_blocking(move || keep(&c, &name, &a, s.as_deref())).await else { return };
    let card = format!("{CART_PREFIX}{}", cart.id);
    crate::card_place::place_for(state, agent, &card, run).await;
    let line = late_line(&crate::handback::language_of(&config, agent), about);
    state.push.notify_bridge_card(&agent.name, &line, "mail_request", &card).await;
    tracing::info!(target: "bipbridge::urgent_pay", agent = %agent.name, card = %card, "too late to pay: the cart waits");
}

// --- « Prêt à acheter » ------------------------------------------------------------------------------------------------

/// The agents rehearsing a purchase now (`pay_with_card` refused).
static REHEARSING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

pub fn rehearsing(agent: &str) -> bool {
    REHEARSING.lock().unwrap_or_else(|e| e.into_inner()).contains(agent)
}

struct Rehearsal(String);

impl Rehearsal {
    fn start(agent: &str) -> Option<Rehearsal> {
        REHEARSING.lock().unwrap_or_else(|e| e.into_inner()).insert(agent.to_string()).then(|| Rehearsal(agent.into()))
    }
}

impl Drop for Rehearsal {
    fn drop(&mut self) {
        REHEARSING.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

/// The last rehearsal, kept with the watcher's runtime (watchers.rs `Runtime::rehearsal`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rehearsed {
    pub at: f64,
    pub ok: bool,
    /// What blocked, in the agent's plain words.
    pub blocked: String,
}

/// A host as the person wrote it (« https://www.Ticketmaster.fr/x »): « ticketmaster.fr ».
pub fn bare_host(text: &str) -> String {
    let text = text.trim().to_lowercase();
    let text = text.trim_start_matches("https://").trim_start_matches("http://");
    let host = text.split(['/', ':', '?', '#']).next().unwrap_or("");
    host.trim_end_matches('.').trim_start_matches("www.").to_string()
}

fn same_site(origin: &str, site: &str) -> bool {
    let host = bare_host(origin);
    !site.is_empty() && (host == site || host.ends_with(&format!(".{site}")))
}

/// The card step: ok, missing (no card for this agent), other_site, cap_low, off (payments not allowed or suspended).
fn card_step(config: &Config, cards: &[crate::vault::Item], agent: &str, purchase: &Purchase) -> Value {
    if !cards::allowed(config) || !cards::enabled(config) {
        return json!({"id": "card", "state": "off"});
    }
    let mine: Vec<&crate::vault::Item> =
        cards.iter().filter(|i| i.kind == cards::KIND && i.agents.contains_key(agent)).collect();
    let sites_of = |i: &crate::vault::Item| -> Vec<String> {
        let meta = i.card.as_ref();
        meta.and_then(|m| m.get("sites")).and_then(Value::as_array).map(|s| s.iter().map(py::py_str).collect()).unwrap_or_default()
    };
    let on_site: Vec<&&crate::vault::Item> =
        mine.iter().filter(|i| cards::site_allowed(&sites_of(i), &purchase.site)).collect();
    let view = |i: &crate::vault::Item, state: &str| {
        let meta = i.card.clone().unwrap_or(Value::Null);
        let cap = meta.get("cap_cents").and_then(Value::as_i64);
        json!({"id": "card", "state": state, "card": i.label, "card_id": i.id,
               "cap": cap.map(cards::format_cents)})
    };
    let fits = |i: &crate::vault::Item| {
        let meta = i.card.clone().unwrap_or(Value::Null);
        purchase.cents == 0 || cards::over_cap(&meta, &i.label, purchase.cents, &purchase.currency).is_none()
    };
    if let Some(card) = on_site.iter().find(|i| fits(i)) {
        return view(card, "ok");
    }
    if let Some(card) = on_site.first() {
        return view(card, "cap_low");
    }
    match mine.first() {
        Some(card) => view(card, "other_site"),
        None => json!({"id": "card", "state": "missing"}),
    }
}

/// `watch.purchase` of the watcher's detail, with its checklist's steps the bridge knows (the phone adds its own).
pub fn purchase_view(state: &AppState, config: &Config, agent: &str, conn: &Connector, purchase: &Purchase) -> Value {
    let items = state.vault.items();
    let account = items.iter().any(|i| i.kind == "login" && i.origin.as_deref().is_some_and(|o| same_site(o, &purchase.site)));
    let contacts = state.vault.contacts_for(config, agent).map(|c| c.len()).unwrap_or(0);
    let rt = crate::watchers::load(config, agent, &conn.id);
    let rehearsal = match (&rt.rehearsal, rehearsing(agent)) {
        (_, true) => json!({"id": "rehearsal", "state": "running"}),
        (None, false) => json!({"id": "rehearsal", "state": "never"}),
        (Some(r), false) => json!({"id": "rehearsal", "state": if r.ok { "ok" } else { "blocked" }, "at": r.at,
                                   "blocked": if r.ok { Value::Null } else { json!(r.blocked) }}),
    };
    json!({
        "site": purchase.site, "what": purchase.what,
        "price": (purchase.cents > 0).then(|| cards::format_cents(purchase.cents)), "currency": purchase.currency,
        "payments_allowed": cards::allowed(config) && cards::enabled(config),
        "steps": [
            {"id": "account", "state": if account { "ok" } else { "missing" }, "contacts": contacts},
            card_step(config, &items, agent, purchase),
            rehearsal,
        ],
    })
}

/// The note of an account set-up (hidden in the apps).
pub fn account_note(purchase: &Purchase) -> String {
    format!(
        "<!--bip:purchase\nNot from a site: your person gets ready to buy « {} » on {} as soon as your watcher sees \
it. Create their customer account on {} now with vault_create_login (one card for them; the password is generated \
and kept in the vault). Fill the profile and delivery address the shop needs from their « Coordonnées » (browser_vault \
list shows each entry with when to use it); ask only for what is missing, and offer to keep it in their vault. Don't \
buy anything. End with one short sentence: what is ready, what is missing.\n-->",
        crate::tools_knowledge::in_note_line(&purchase.what),
        purchase.site,
        purchase.site
    )
}

/// The note of a rehearsal (hidden in the apps).
pub fn rehearsal_note(purchase: &Purchase, lang: &str) -> String {
    let price = if purchase.cents > 0 {
        format!(" (about {})", money(lang, purchase.cents, &purchase.currency))
    } else {
        String::new()
    };
    format!(
        "<!--bip:purchase\nA rehearsal your person asked for: go once through {} as if buying « {} »{price}: log in \
(browser_vault_fill), put it in the cart (or a similar item if it is not on sale yet), fill the delivery details from \
their « Coordonnées », and go up to the payment page where the card form shows. NEVER pay: pay_with_card is refused \
during a rehearsal and you never press the final button. Then empty the cart. The first line of your reply is \
exactly REHEARSAL OK, or REHEARSAL BLOCKED: <what blocked, in a few plain words for your person>; then one short \
sentence for them.\n-->",
        purchase.site,
        crate::tools_knowledge::in_note_line(&purchase.what),
    )
}

/// The rehearsal's reply read: (ok, what blocked, the words for the person). None: no verdict line.
pub fn read_rehearsal(text: &str) -> Option<(bool, String, String)> {
    let text = crate::tools_knowledge::strip(text).trim().to_string();
    let (first, rest) = text.split_once('\n').unwrap_or((text.as_str(), ""));
    let head = first.trim().trim_matches(|c: char| c == '*' || c == '`').trim();
    let upper = head.to_uppercase();
    let rest = rest.trim().to_string();
    if upper.starts_with("REHEARSAL OK") {
        return Some((true, String::new(), rest));
    }
    let blocked = upper.strip_prefix("REHEARSAL BLOCKED").map(|_| {
        let why = &head["REHEARSAL BLOCKED".len()..];
        why.trim_start_matches([':', ' ', '-', '—']).trim().chars().take(200).collect::<String>()
    })?;
    Some((false, blocked, rest))
}

/// One turn of the agent with `note` in the watcher's conversation; its words for the person (without a verdict line)
/// put there. Returns its reply (None: no turn).
async fn setup_turn(state: &AppState, agent: &AgentConfig, conn: &Connector, note: String) -> Option<String> {
    let config = state.config();
    let pipeline = Pipeline::of(state);
    let kept = crate::watchers::load(&config, &agent.name, &conn.id).session;
    let title = crate::watchers::session_title(&crate::handback::language_of(&config, agent), &conn.name);
    let session = pipeline.work_session(agent, &title, &kept).await.ok()?;
    if session != kept {
        crate::watchers::change(&config, &agent.name, &conn.id, |rt| rt.session = session.clone());
    }
    let mut turn = Turn::new(agent, Kind::Watcher, &session, note);
    turn.max_seconds = SETUP_MAX_SECONDS;
    turn.busy_wait = 120.0;
    let ran = pipeline.run(&turn).await.ok().filter(|r| r.answered())?;
    let shown = read_rehearsal(&ran.text).map(|(_, _, rest)| rest).unwrap_or_else(|| ran.text.clone());
    if !shown.trim().is_empty() {
        let delivery = Delivery {
            agent: agent.clone(),
            session,
            key: format!("purchase:{}:{}", conn.id, py::uuid4_hex()),
            title: conn.name.clone(),
            text: shown,
            as_written: false,
            run_id: None,
            notify: false,
            files: Vec::new(),
        };
        pipeline.deliver(&delivery).await;
    }
    tracing::info!(target: "bipbridge::urgent_pay", agent = %agent.name, watcher = %conn.id, "set-up turn done");
    Some(ran.text)
}

fn purchase_of(config: &Config, agent: &str, id: &str) -> ApiResult<(AgentConfig, Connector, Purchase)> {
    let agent = config.agent(Some(agent)).cloned().ok_or_else(|| HttpError::new(404, "no such agent"))?;
    let conn = crate::connectors::get(config, &agent.name, id).ok_or_else(|| HttpError::new(404, "no such connector"))?;
    let purchase = conn
        .watch
        .as_ref()
        .and_then(|w| w.purchase.clone())
        .ok_or_else(|| HttpError::json(409, json!({"code": "not_a_purchase_watcher"})))?;
    Ok((agent, conn, purchase))
}

async fn account_route(State(state): State<AppState>, Path((agent, id)): Path<(String, String)>) -> ApiResult<Response> {
    let (agent, conn, purchase) = purchase_of(&state.config(), &agent, &id)?;
    let note = account_note(&purchase);
    tokio::spawn(async move {
        setup_turn(&state, &agent, &conn, note).await;
    });
    Ok(web::json_response(202, &json!({"ok": true})))
}

async fn rehearse_route(State(state): State<AppState>, Path((agent, id)): Path<(String, String)>) -> ApiResult<Response> {
    let config = state.config();
    let (agent, conn, purchase) = purchase_of(&config, &agent, &id)?;
    let Some(marked) = Rehearsal::start(&agent.name) else {
        return Err(HttpError::json(409, json!({"code": "rehearsal_running"})));
    };
    let note = rehearsal_note(&purchase, &crate::handback::language_of(&config, &agent));
    tokio::spawn(async move {
        let _marked = marked;
        let text = setup_turn(&state, &agent, &conn, note).await;
        let (ok, blocked) = match text.as_deref().and_then(read_rehearsal) {
            Some((ok, blocked, _)) => (ok, blocked),
            None => (false, String::new()),
        };
        let config = state.config();
        crate::watchers::change(&config, &agent.name, &conn.id, |rt| {
            rt.rehearsal = Some(Rehearsed { at: py::time_time(), ok, blocked: blocked.clone() })
        });
        tracing::info!(target: "bipbridge::urgent_pay", agent = %agent.name, watcher = %conn.id, ok, "rehearsal done");
    });
    Ok(web::json_response(202, &json!({"ok": true})))
}

async fn test_route(State(state): State<AppState>, Path(agent): Path<String>) -> ApiResult<Response> {
    let config = state.config();
    let agent = config.agent(Some(&agent)).cloned().ok_or_else(|| HttpError::new(404, "no such agent"))?;
    let id = py::uuid4_hex()[..12].to_string();
    let line = test_line(&crate::handback::language_of(&config, &agent));
    let sent =
        state.push.notify_payment_urgent(&agent.name, &format!("bip-paytest-{id}"), &id, line, None, true).await;
    Ok(web::ok(json!({"ok": true, "sent": sent})))
}

/// The apps' routes (mounted under /v1 with the bridge key).
pub fn app_router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agent_id}/urgent-test", post(test_route))
        .route("/agents/{agent_id}/connectors/{id}/purchase/account", post(account_route))
        .route("/agents/{agent_id}/connectors/{id}/purchase/rehearse", post(rehearse_route))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_in(dir: &tempfile::TempDir) -> Config {
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        config
    }

    fn about() -> About {
        About {
            message: String::new(),
            merchant: "Billet Stromae".into(),
            site: "ticketmaster.fr".into(),
            url: "https://www.ticketmaster.fr/checkout".into(),
            cents: 4500,
            currency: "EUR".into(),
        }
    }

    #[test]
    fn the_words_say_what_where_and_how_much_once() {
        assert_eq!(found_line("fr", &about()), "A trouvé : Billet Stromae à 45,00 € sur ticketmaster.fr");
        assert_eq!(found_line("en", &about()), "Found: Billet Stromae for €45.00 on ticketmaster.fr");
        assert_eq!(found_line("de", &about()), "Gefunden: Billet Stromae für 45,00 € auf ticketmaster.fr");
        assert!(found_line("es", &about()).contains("45,00 €"));
        assert_eq!(late_line("fr", &about()), "Trop tard pour payer, le panier t'attend : Billet Stromae sur ticketmaster.fr");
        assert_eq!(money("fr", 1299, "USD"), "12,99 $");
        assert_eq!(money("en", 1299, "SEK"), "12.99 SEK");
        assert!(test_line("fr").starts_with("Essai"));
    }

    #[test]
    fn a_cart_come_too_late_waits_as_a_hand_over_card_then_goes() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir);
        let cart = keep(&config, "vie", &about(), Some("s_watch"));
        let items = pending_items(&config, Some("vie"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["run_id"], format!("{CART_PREFIX}{}", cart.id));
        // the hand-over's own text: the apps draw « Panier prêt » with its ways to pay
        let command = items[0]["command"].as_str().unwrap();
        assert!(command.starts_with(&format!("Payment hand-over {}: 45.00 EUR on ticketmaster.fr", cart.id)), "{command}");
        assert!(command.contains("<https://www.ticketmaster.fr/checkout>"));
        assert_eq!(items[0]["session_id"], "s_watch");
        assert!(pending_items(&config, Some("coach")).is_empty());
        assert!(!answer(&config, "coach", &cart.id));
        assert!(answer(&config, "vie", &cart.id));
        assert!(pending_items(&config, None).is_empty());
        assert!(!answer(&config, "vie", &cart.id), "put away once");
        // past its day: gone
        keep(&config, "vie", &about(), None);
        change(&config, |carts| carts.iter_mut().for_each(|c| c.until = py::time_time() - 1.0));
        assert!(pending_items(&config, None).is_empty());
    }

    #[test]
    fn a_rehearsal_says_ok_or_what_blocked() {
        assert_eq!(read_rehearsal("REHEARSAL OK\nTout est prêt."), Some((true, String::new(), "Tout est prêt.".into())));
        assert_eq!(
            read_rehearsal("**REHEARSAL BLOCKED: le site demande un code par SMS**\nIl faudra ton téléphone."),
            Some((false, "le site demande un code par SMS".into(), "Il faudra ton téléphone.".into()))
        );
        assert_eq!(read_rehearsal("J'ai fini."), None);
        assert_eq!(bare_host("https://www.TicketMaster.fr/x?y"), "ticketmaster.fr");
        assert!(same_site("https://secure.ticketmaster.fr", "ticketmaster.fr"));
        assert!(!same_site("https://ticketmaster.fr.evil.example", "ticketmaster.fr"));
        let marked = Rehearsal::start("vie").unwrap();
        assert!(rehearsing("vie") && Rehearsal::start("vie").is_none());
        drop(marked);
        assert!(!rehearsing("vie"));
        let note = rehearsal_note(&Purchase { site: "ticketmaster.fr".into(), what: "Billet".into(), cents: 4500, currency: "EUR".into() }, "fr");
        assert!(note.contains("NEVER pay") && note.contains("45,00 €") && note.contains("REHEARSAL OK"));
    }
}
