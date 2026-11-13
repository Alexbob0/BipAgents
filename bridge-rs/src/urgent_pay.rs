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
//!   the shop's customer account, the « Coordonnées » a checkout needs (filled, not only there), the card, the
//!   rehearsal; the phone adds its own steps (the code remembered there,
//!   urgent alerts allowed). Its actions: `POST …/connectors/<id>/purchase/account` (the agent creates the account
//!   with `vault_create_login`), `…/purchase/rehearse` (the agent goes up to the payment page without paying, then
//!   empties the cart; `pay_with_card` is refused meanwhile), `POST /v1/agents/<a>/urgent-test` (a test alert).
//!
//!     <data>/urgent-carts.json (0600)   [{id, agent, site, url, merchant, cents, currency, session, at, until}]
//!
//! No Python counterpart.

use std::collections::HashMap;
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
        "de" => "Test: So wirst du gewarnt, wenn schnell bezahlt werden muss.",
        "es" => "Prueba: así se te avisa cuando hay que pagar rápido.",
        "en" => "Test: this is how you are told when you need to pay fast.",
        _ => "Essai : voici comment tu es prévenu quand il faut payer vite.",
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
    // put away by the person, who buys it themselves: nothing to tell the agent (never resumed with « ALLOWED »)
    crate::card_place::taken(&config, &card);
    let line = late_line(&crate::handback::language_of(&config, agent), about);
    state.push.notify_bridge_card(&agent.name, &line, "mail_request", &card).await;
    tracing::info!(target: "bipbridge::urgent_pay", agent = %agent.name, card = %card, "too late to pay: the cart waits");
}

// --- « Prêt à acheter » ------------------------------------------------------------------------------------------------

/// The agents rehearsing a purchase now: the rehearsal's run once its turn started (None: asked, waiting for the
/// watcher's conversation). Only that run's `pay_with_card` is refused (`refuses_payment`).
static REHEARSING: LazyLock<Mutex<HashMap<String, Option<String>>>> = LazyLock::new(Default::default);

/// A rehearsal of `agent` asked or running (the checklist's « Répétition en cours… »).
pub fn rehearsing(agent: &str) -> bool {
    REHEARSING.lock().unwrap_or_else(|e| e.into_inner()).contains_key(agent)
}

/// A payment tool (`tool`: pay_with_card, pay_with_link) of `agent` is refused as a rehearsal's: its rehearsal's turn
/// runs and the call is that turn's (the run the hub sees inside that very call: its `tool.started` may come a moment
/// after the call, waited for up to `SEEN_WITHIN`), or no turn of the agent is seen making it. Another turn of the
/// agent (the watcher's own, a person's conversation) pays as usual, and nothing is refused while the rehearsal only
/// waits for its conversation.
pub async fn refuses_payment(state: &AppState, agent: &str, tool: &str) -> bool {
    let rehearsal = || REHEARSING.lock().unwrap_or_else(|e| e.into_inner()).get(agent).cloned().flatten();
    let Some(run) = rehearsal() else { return false };
    let until = tokio::time::Instant::now() + SEEN_WITHIN;
    loop {
        let calling = state.hub.runs_calling(agent, tool);
        if !calling.is_empty() {
            return calling.contains(&run);
        }
        if tokio::time::Instant::now() >= until {
            // no turn seen in that call (the hub follows none): the rehearsal's, to be safe, while it runs
            return rehearsal().as_deref() == Some(run.as_str());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// How long `refuses_payment` waits for the hub to see the calling turn inside the payment tool.
const SEEN_WITHIN: std::time::Duration = std::time::Duration::from_secs(2);

struct Rehearsal(String);

impl Rehearsal {
    fn start(agent: &str) -> Option<Rehearsal> {
        let mut table = REHEARSING.lock().unwrap_or_else(|e| e.into_inner());
        if table.contains_key(agent) {
            return None;
        }
        table.insert(agent.to_string(), None);
        Some(Rehearsal(agent.into()))
    }

    /// Its turn started: `run` is the rehearsal's.
    fn running(&self, run: &str) {
        REHEARSING.lock().unwrap_or_else(|e| e.into_inner()).insert(self.0.clone(), Some(run.to_string()));
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

/// The card step: ok, missing (no card for this agent), other_site, cap_low, renew (the card on the site can't pay as
/// it is: expired, single use already used, no longer on the person's phone: its new numbers to enter), off (payments
/// not allowed or suspended). « ok » only for a card pay_with_card would type (card_pay.rs `usable`).
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
    let usable = |i: &crate::vault::Item| crate::card_pay::usable(config, i.card.as_ref().unwrap_or(&Value::Null));
    if let Some(card) = on_site.iter().find(|i| usable(i) && fits(i)) {
        return view(card, "ok");
    }
    if let Some(card) = on_site.iter().find(|i| usable(i)) {
        return view(card, "cap_low");
    }
    if let Some(card) = on_site.first() {
        return view(card, "renew");
    }
    match mine.first() {
        Some(card) => view(card, "other_site"),
        None => json!({"id": "card", "state": "missing"}),
    }
}

/// The address step: the agent's « Coordonnées » with what a checkout needs filled (vault_contacts.rs
/// `DELIVERY_NEEDS`). ok: at least one entry is complete (its label when it is the only one, else how many);
/// incomplete: none is, the closest one and its empty fields (the agent would have to ask, or make one up); missing:
/// no entry given to this agent.
fn address_step(entries: &[crate::vault_contacts::Entry]) -> Value {
    let checked: Vec<(&crate::vault_contacts::Entry, Vec<&str>)> =
        entries.iter().map(|e| (e, crate::vault_contacts::missing_for_delivery(&e.secret))).collect();
    let complete: Vec<&crate::vault_contacts::Entry> =
        checked.iter().filter(|(_, missing)| missing.is_empty()).map(|(e, _)| *e).collect();
    match (complete.as_slice(), checked.iter().min_by_key(|(_, missing)| missing.len())) {
        ([one], _) => json!({"id": "address", "state": "ok", "entries": 1, "entry": one.label, "entry_id": one.id}),
        ([_, ..], _) => json!({"id": "address", "state": "ok", "entries": complete.len()}),
        ([], Some((closest, missing))) => json!({"id": "address", "state": "incomplete", "entries": 0,
            "entry": closest.label, "entry_id": closest.id, "missing": missing}),
        ([], None) => json!({"id": "address", "state": "missing", "entries": 0}),
    }
}

/// `watch.purchase` of the watcher's detail, with its checklist's steps the bridge knows (the phone adds its own).
pub fn purchase_view(state: &AppState, config: &Config, agent: &str, conn: &Connector, purchase: &Purchase) -> Value {
    let items = state.vault.items();
    // a login for the site that this agent may use (one kept from it can't log it in at checkout)
    let account = items.iter().any(|i| {
        i.kind == "login" && i.agents.contains_key(agent) && i.origin.as_deref().is_some_and(|o| same_site(o, &purchase.site))
    });
    let entries = state.vault.contacts_for(config, agent).unwrap_or_default();
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
            {"id": "account", "state": if account { "ok" } else { "missing" }},
            address_step(&entries),
            card_step(config, &items, agent, purchase),
            rehearsal,
        ],
    })
}

/// The note of an account set-up (hidden in the apps).
pub fn account_note(purchase: &Purchase) -> String {
    format!(
        "<!--bip:purchase\nNot from a site: your person gets ready to buy « {} » on {} as soon as your watcher sees \
it. Create their customer account on {} now with vault_create_login: open the shop's sign-up page in your own \
browser, fill the e-mail and the other fields from their « Coordonnées » (browser_vault list shows each entry with \
when to use it), focus the password field, then call vault_create_login (tool_search finds it if you don't see it): \
it makes and types the password, they approve one card. Never ask them for a password (no ask_secret) and never type \
one yourself. Fill the profile and delivery address the shop needs from the same entry; ask only for what is \
missing, in your last message (nobody follows this turn live: no clarify question), and offer to keep it in their \
vault. Don't buy anything. End with one short sentence: what is ready, what is missing.\n-->",
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

/// One turn of the agent with `note` in the watcher's conversation (`started` told its run once Hermes took it); its
/// words for the person (without a verdict line) put there. Returns its reply (None: no turn).
async fn setup_turn(
    state: &AppState,
    agent: &AgentConfig,
    conn: &Connector,
    note: String,
    started: impl FnOnce(&str),
) -> Option<String> {
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
    let ran = pipeline.run_then(&turn, started).await.ok().filter(|r| r.answered())?;
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
        setup_turn(&state, &agent, &conn, note, |_| {}).await;
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
        // pay_with_card refused to the rehearsal's own turn only, from its start (`refuses_payment`)
        let text = setup_turn(&state, &agent, &conn, note, |run| marked.running(run)).await;
        let (ok, blocked) = match text.as_deref().and_then(read_rehearsal) {
            Some((ok, blocked, _)) => (ok, blocked),
            None => (false, String::new()),
        };
        let config = state.config();
        crate::watchers::change(&config, &agent.name, &conn.id, |rt| {
            rt.rehearsal = Some(Rehearsed { at: py::time_time(), ok, blocked: blocked.clone() })
        });
        drop(marked); // its verdict kept first: the checklist never shows « never » in between
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
    fn the_address_step_wants_an_entry_a_checkout_can_use() {
        let entry = |id: &str, label: &str, secret: Value| crate::vault_contacts::Entry {
            id: id.into(),
            label: label.into(),
            hint: None,
            secret: secret.as_object().cloned().unwrap(),
        };
        let full = json!({"full_name": "Camille Exemple", "address_line1": "1 rue de l'Essai", "city": "Testville",
                          "postal_code": "00100", "country": "FR", "phones": ["+33 6 00 00 00 00"]});
        let no_phone = json!({"full_name": "Camille Exemple", "address_line1": "1 rue de l'Essai", "city": "Testville",
                              "postal_code": "00100", "country": "FR"});
        let bare = json!({"full_name": "Camille Exemple"});
        assert_eq!(address_step(&[]), json!({"id": "address", "state": "missing", "entries": 0}));
        // entries there, none a checkout can use: the closest one and what it lacks (never « ok » for a count)
        let step = address_step(&[entry("item_a", "Bureau", bare.clone()), entry("item_b", "Perso", no_phone.clone())]);
        assert_eq!(step, json!({"id": "address", "state": "incomplete", "entries": 0, "entry": "Perso",
                                "entry_id": "item_b", "missing": ["phone"]}));
        let step = address_step(&[entry("item_a", "Bureau", bare), entry("item_b", "Perso", full.clone())]);
        assert_eq!(step, json!({"id": "address", "state": "ok", "entries": 1, "entry": "Perso", "entry_id": "item_b"}));
        let step = address_step(&[entry("item_a", "Bureau", full.clone()), entry("item_b", "Perso", full)]);
        assert_eq!(step, json!({"id": "address", "state": "ok", "entries": 2}));
    }

    /// « Carte prête » (card step `ok`) only for a card pay_with_card would type (review 13/11). A single-use card
    /// already used, or an expired card, is refused at the checkout (card_pay.rs `card_state`): « renew », opening the
    /// card for its new numbers, never « ready » for a ticket that would then be lost.
    #[test]
    fn the_card_step_is_not_ready_for_a_card_pay_with_card_refuses() {
        if cards::on_offer() {
            return; // a place of the offer: no cards here
        }
        let dir = tempfile::tempdir().unwrap();
        let mut config = config_in(&dir);
        config.hermes.isolated = true;
        cards::set_enabled(&config, true).unwrap();
        assert!(cards::enabled(&config));
        let purchase =
            Purchase { site: "ticketmaster.fr".into(), what: "Billet".into(), cents: 4500, currency: "EUR".into() };
        let card = |id: &str, meta: Value| crate::vault::Item {
            id: id.into(),
            kind: cards::KIND.into(),
            label: "Petits achats".into(),
            agents: [("vie".to_string(), "h_vie".to_string())].into_iter().collect(),
            card: Some(meta),
            ..Default::default()
        };
        let ready = card("item_ready000001", json!({"last4": "4242", "exp_month": 12, "exp_year": 2030}));
        assert_eq!(card_step(&config, &[ready], "vie", &purchase)["state"], "ok", "a card that can pay: ready");
        let used = card(
            "item_used0000001",
            json!({"last4": "4242", "exp_month": 12, "exp_year": 2030, "single_use": true, "used": true}),
        );
        let expired = card("item_expired0001", json!({"last4": "4242", "exp_month": 1, "exp_year": 2020}));
        let shown: Vec<Value> =
            [used, expired].iter().map(|c| card_step(&config, std::slice::from_ref(c), "vie", &purchase)).collect();
        assert!(
            shown.iter().all(|s| s["state"] == "renew"),
            "« Carte prête » for cards pay_with_card refuses (single use already used, expired): {shown:?}"
        );
        assert_eq!(shown[0]["card_id"], "item_used0000001");
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
