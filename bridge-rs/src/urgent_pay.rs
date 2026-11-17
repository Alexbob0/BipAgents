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
/// Said so that a real payment refused by mistake (a turn the hub can't tell from the rehearsal's) keeps its cart.
pub const REHEARSAL_NO_PAY: &str = "A purchase rehearsal of yours is running: no payment now. If this is that \
rehearsal, stop at the payment page, then empty the cart; otherwise keep the cart and try pay_with_card again in a few \
minutes.";

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
    let mut steps = vec![
        json!({"id": "account", "state": if account { "ok" } else { "missing" }}),
        address_step(&entries),
        card_step(config, &items, agent, purchase),
        rehearsal,
    ];
    let locked = crate::sealed_keys::closed(config);
    if locked {
        // the vault's keys sealed and not reopened yet: the account, the address and the card unknown, not missing
        for step in steps.iter_mut().take(3) {
            *step = json!({"id": step["id"], "state": "vault_locked"});
        }
    }
    let mut view = json!({
        "site": purchase.site, "what": purchase.what,
        "price": (purchase.cents > 0).then(|| cards::format_cents(purchase.cents)), "currency": purchase.currency,
        "payments_allowed": cards::allowed(config) && cards::enabled(config),
        "steps": steps,
    });
    if locked {
        view["vault"] = json!("locked");
    }
    view
}

/// The note of an account set-up (hidden in the apps). `email`: the first e-mail of the person's « Coordonnées » given to
/// the agent, and that entry's label (13/11, the lab: without it the agent looked for their e-mail in the Carnet until
/// the brake stopped it).
pub fn account_note(purchase: &Purchase, email: Option<(&str, &str)>) -> String {
    let email = match email {
        Some((label, email)) if label.trim().is_empty() => format!("sign up with their e-mail {email}, from their « Coordonnées »"),
        Some((label, email)) => format!("sign up with their e-mail {email}, from their « Coordonnées » « {label} »"),
        None => "their « Coordonnées » have no e-mail: ask them for one in your last message".to_string(),
    };
    format!(
        "<!--bip:purchase\nNot from a site: your person gets ready to buy « {} » on {} as soon as your watcher sees \
it. Create their customer account on {} now with vault_create_login: open the shop's sign-up page in your own \
browser and {email}. vault_contact_details reads the rest of their « Coordonnées »: never look for their details in \
the Carnet, your memory or your files, and never make one up. Focus the password field, then call vault_create_login: \
it makes and types the password, they approve one card; vault_confirm_login once the site took the account; a terms \
box: accept_terms asks them. Never ask them for a password (no ask_secret) and never type one yourself. Fill the \
profile and delivery address the shop needs from the same entry; ask only for what is missing, in your last message \
(nobody follows this turn live: no clarify question), and offer to keep it in their vault. Don't buy anything. End \
with one short sentence: what is ready, what is missing.\n-->",
        crate::tools_knowledge::in_note_line(&purchase.what),
        purchase.site,
        purchase.site
    )
}

/// The rehearsal's verdict, first line of its reply: a code word for the app, read by `read_rehearsal` and never shown.
pub const REHEARSAL_OK: &str = "[REHEARSAL OK]";
pub const REHEARSAL_BLOCKED: &str = "[REHEARSAL BLOCKED]";

/// How a rehearsal's note starts (`rehearsal_note_in`).
const REHEARSAL_NOTE_MARK: &str = "<!--bip:purchase\nA rehearsal your person asked for";

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
during a rehearsal and you never press the final button. Then empty the cart. The first line of your reply is a code \
word for the app, never translated, kept exactly as written (in English, in brackets), whatever the conversation's \
language: {REHEARSAL_OK} when the card form showed, else {REHEARSAL_BLOCKED} followed by what blocked, in a few plain \
words in your person's language. Then one short sentence for them (they never see the code word).\n-->",
        purchase.site,
        crate::tools_knowledge::in_note_line(&purchase.what),
    )
}

/// What a rehearsal is called, in the languages its verdict comes back translated in (13/11, the lab: GLM-5.3 Flash
/// wrote « RÉPÉTITION RÉUSSIE » once in three), accents and case aside.
const REHEARSAL_WORDS: &[&str] =
    &["REHEARSAL", "REPETITION", "PROBEDURCHLAUF", "PROBELAUF", "GENERALPROBE", "PROBE", "TESTLAUF", "ENSAYO", "SIMULACRO"];
/// Words a model puts between a rehearsal's name and its verdict (« RÉPÉTITION GÉNÉRALE OK », « ENSAYO GENERAL OK »,
/// « Rehearsal result: OK », « Probe ist erfolgreich »), accents and case aside.
const BETWEEN_WORDS: &[&str] = &[
    "GENERALE", "GENERAL", "RESULT", "RESULTAT", "RESULTADO", "ERGEBNIS", "STATUS", "STATUT", "ESTADO", "VERDICT",
    "VEREDICTO", "IST", "EST", "IS", "ES", "WAS", "WAR", "ETAIT", "FUE",
];
/// Its verdict, the same way: it went through, or it was blocked.
const PASSED_WORDS: &[&str] = &[
    "OK", "OKAY", "PASSED", "PASS", "SUCCESSFUL", "SUCCEEDED", "SUCCESS", "DONE", "REUSSIE", "REUSSI", "VALIDEE", "ERFOLGREICH",
    "BESTANDEN", "EXITOSO", "EXITOSA", "SUPERADO",
];
const BLOCKED_WORDS: &[&str] = &[
    "BLOCKED", "FAILED", "FAIL", "BLOQUEE", "BLOQUE", "ECHOUEE", "ECHEC", "BLOCKIERT", "GESCHEITERT", "FEHLGESCHLAGEN",
    "BLOQUEADO", "BLOQUEADA", "FALLIDO", "FALLIDA", "FALLO", "KO",
];

/// Words that turn a blocked word into its opposite when they come just before it (four words at most: « rien n'a
/// été bloqué »), accents and case aside.
const NEGATIONS: &[&str] = &[
    "RIEN", "PAS", "AUCUN", "AUCUNE", "SANS", "JAMAIS", "NI", "NOT", "NO", "NOTHING", "NEVER", "NONE", "WITHOUT", "NICHT",
    "NICHTS", "KEIN", "KEINE", "KEINEN", "OHNE", "NIE", "SIN", "NADA", "NINGUN", "NINGUNA", "NUNCA",
];

/// Words a negation's reach ends at, looking back from a blocked word (« pas bloquée mais KO »: blocked).
const CONJUNCTIONS: &[&str] = &[
    "MAIS", "BUT", "ABER", "PERO", "SINO", "SONDERN", "HOWEVER", "YET", "SAUF", "EXCEPT", "CEPENDANT", "POURTANT", "DOCH",
    "TOUTEFOIS", "ALLERDINGS",
];

/// Every bracketed verdict in a line, wherever it stands: the code word (« Tout est prêt… [REHEARSAL OK] », « No
/// blocking issue…, but [REHEARSAL BLOCKED] the site asks for an SMS code ») or a bracketed translation (« … [RÉPÉTITION
/// GÉNÉRALE OK] », the lab of 14/11), each one taken out, and the line's own words without them (None: nothing but
/// them, markdown or an emoji). « [… BLOCKED] » with no reason inside takes the words after it, up to the next verdict,
/// as its reason. None: no bracketed verdict in the line (a line quoted from a page, « > … », never has one).
fn verdicts_in(line: &str, codes_decide: bool) -> Option<(Vec<Verdict>, Option<String>)> {
    let orig: Vec<char> = line.chars().collect();
    if orig.iter().find(|c| !c.is_whitespace()) == Some(&'>') {
        return None;
    }
    // the bracketed verdicts: (start, end past its « ] », the verdict)
    let mut found: Vec<(usize, usize, Verdict)> = Vec::new();
    let mut i = 0;
    while i < orig.len() {
        if orig[i] == '[' {
            match (i + 1..orig.len()).find(|j| orig[*j] == ']') {
                Some(close) => {
                    let segment: String = orig[i..=close].iter().collect();
                    if let Some(v) = verdict_of(&segment) {
                        found.push((i, close + 1, v));
                        i = close + 1;
                        continue;
                    }
                }
                // the code word's bracket never closed (« Désolé. [REHEARSAL BLOCKED: le site demande un code SMS »):
                // it runs to the end of the line (fifth review of 14/11, as 36d5329d0 read it)
                None => {
                    let segment: String = orig[i..].iter().collect();
                    if let Some(v) = verdict_of(&segment).filter(|v| v.code) {
                        found.push((i, orig.len(), v));
                        break;
                    }
                }
            }
        }
        i += 1;
    }
    if found.is_empty() {
        return None;
    }
    let tidy = |t: &str| -> String {
        t.trim_start_matches(|c: char| c.is_whitespace() || c.is_ascii_punctuation() || "\u{2014}\u{2013}".contains(c))
            .trim_end_matches(|c: char| c.is_whitespace() || "]*`".contains(c))
            .trim()
            .chars()
            .take(200)
            .collect()
    };
    let mut kept: Vec<char> = Vec::new();
    let mut verdicts = Vec::new();
    let mut at = 0;
    for (n, (start, end, mut v)) in found.iter().cloned().enumerate() {
        kept.extend(&orig[at..start]);
        kept.push(' ');
        at = end;
        // only a deciding verdict takes them: the code word, or a translation when the reply has no code word (one the
        // code word overrides keeps the person's words: « [REHEARSAL OK] (la [RÉPÉTITION BLOQUÉE] d'hier est réglée) »)
        if !v.ok && v.why.is_empty() && (v.code || !codes_decide) {
            // « [REHEARSAL BLOCKED] why »: the words after it, up to the next verdict, are what blocked
            let until = found.get(n + 1).map_or(orig.len(), |next| next.0);
            v.why = tidy(&orig[end..until].iter().collect::<String>());
            at = until;
        }
        verdicts.push(v);
    }
    kept.extend(&orig[at..]);
    let text: String = kept.iter().collect();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = text.trim_matches(|c: char| !c.is_alphanumeric() && !".!?»)…".contains(c)).trim().to_string();
    let words = text.chars().any(char::is_alphanumeric).then_some(text);
    Some((verdicts, words))
}

/// `c` upper-case without its accent, one char for one (so a position in it is one in the original).
fn plain_char(c: char) -> char {
    let up = c.to_uppercase().next().unwrap_or(c);
    match up {
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'A',
        'È' | 'É' | 'Ê' | 'Ë' => 'E',
        'Ì' | 'Í' | 'Î' | 'Ï' => 'I',
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' => 'O',
        'Ù' | 'Ú' | 'Û' | 'Ü' => 'U',
        'Ç' => 'C',
        'Ñ' => 'N',
        '_' => ' ',
        other => other,
    }
}

/// One line read as a verdict. None: not one (a line quoted from a page, « > Rehearsal OK », is never one).
#[derive(Debug, Clone, PartialEq)]
struct Verdict {
    ok: bool,
    /// What blocked, as the line says it.
    why: String,
    /// The code word itself (« [REHEARSAL …] »), not a translation.
    code: bool,
}

fn verdict_of(line: &str) -> Option<Verdict> {
    let orig: Vec<char> = line.chars().collect();
    let plain: Vec<char> = orig.iter().map(|c| plain_char(*c)).collect();
    let first = plain.iter().position(|c| !c.is_whitespace())?;
    if plain[first] == '>' {
        return None;
    }
    // what opens the line: markdown, a bullet, brackets, an emoji (« ✅ [REHEARSAL OK] »)
    let mut at = plain.iter().position(|c| c.is_alphanumeric())?;
    let bracket = plain[..at].contains(&'[');
    let word_at = |at: usize, words: &[&str]| -> Option<usize> {
        words.iter().find_map(|w| {
            let w: Vec<char> = w.chars().collect();
            let end = at + w.len();
            let whole = plain.get(at..end) == Some(&w[..]) && plain.get(end).is_none_or(|c| !c.is_alphanumeric());
            whole.then_some(end)
        })
    };
    let named = word_at(at, REHEARSAL_WORDS)?;
    let code = bracket && plain[at..named].iter().collect::<String>() == "REHEARSAL";
    at = named;
    // « REHEARSAL: OK », « Répétition — réussie », « Probe ist erfolgreich », « RÉPÉTITION GÉNÉRALE OK » (GLM-5.3
    // Flash, the lab of 14/11), « Rehearsal result: OK »: a few known words between the name and the verdict
    let gap = |at: &mut usize| {
        while plain.get(*at).is_some_and(|c| c.is_whitespace() || ":\u{2014}-".contains(*c)) {
            *at += 1;
        }
    };
    gap(&mut at);
    for _ in 0..3 {
        match word_at(at, BETWEEN_WORDS) {
            Some(end) => {
                at = end;
                gap(&mut at);
            }
            None => break,
        }
    }
    // what blocked, never opening on punctuation (« Rehearsal blocked on …: , le formulaire… », second review of 14/11)
    let reason = |end: usize| -> String {
        let why: String = orig[end..].iter().collect();
        why.trim_start_matches(|c: char| c.is_whitespace() || c.is_ascii_punctuation() || "\u{2014}\u{2013}".contains(c))
            .trim_end_matches(|c: char| c.is_whitespace() || "]*`".contains(c))
            .trim()
            .chars()
            .take(200)
            .collect()
    };
    // a blocked word anywhere after the name: blocked (« Rehearsal done but blocked at the payment page »), unless a
    // negation comes just before it (« rien n'a été bloqué », « nothing failed », second review of 14/11)
    // (the words of its own clause only: « le paiement n'est pas passé, bloqué » is blocked)
    // (never past a conjunction nor another verdict word: « pas bloquée mais KO », « without any SMS but blocked »)
    let words_before = |i: usize| -> Vec<String> {
        let head: String = plain[..i].iter().collect();
        let clause = head.rsplit(|c: char| ",;.:!?()[]".contains(c)).next().unwrap_or("");
        clause
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .rev()
            .take(4)
            .take_while(|w| ![CONJUNCTIONS, BLOCKED_WORDS, PASSED_WORDS].iter().any(|list| list.contains(w)))
            .map(str::to_string)
            .collect()
    };
    let blocked_from = |from: usize| {
        (from..plain.len())
            .filter(|i| *i == 0 || !plain[*i - 1].is_alphanumeric())
            .filter_map(|i| word_at(i, BLOCKED_WORDS).map(|end| (i, end)))
            .find(|(i, _)| !words_before(*i).iter().any(|w| NEGATIONS.contains(&w.as_str())))
            .map(|(_, end)| end)
    };
    if word_at(at, PASSED_WORDS).is_some() {
        // the code word as written is its own verdict (« [REHEARSAL OK] rien n'a échoué » stays ok)
        let after = if code { None } else { blocked_from(at) };
        return Some(match after {
            Some(end) => Verdict { ok: false, why: reason(end), code },
            None => Verdict { ok: true, why: String::new(), code },
        });
    }
    let end = word_at(at, BLOCKED_WORDS)?;
    Some(Verdict { ok: false, why: reason(end), code })
}

/// The rehearsal's reply read: (ok, what blocked, the words for the person). Every line that reads as a verdict (the
/// code word or one of its translations, anywhere in the reply) is the app's: never in the words. The exact code word
/// decides when there is one (a blocked one wins among them: « Étapes : - Répétition OK jusqu'au panier » then
/// « [REHEARSAL BLOCKED] … » is blocked; « [REHEARSAL OK] » then « Répétition réussie : rien n'a été bloqué » is ok);
/// else the translations, a blocked one anywhere winning. None: no verdict.
pub fn read_rehearsal(text: &str) -> Option<(bool, String, String)> {
    let text = crate::tools_knowledge::strip(text).trim().to_string();
    // the code word anywhere in the reply decides: a translation it overrides takes no words as its reason
    let codes_decide = text.lines().any(|l| {
        verdicts_in(l, true).is_some_and(|(found, _)| found.iter().any(|v| v.code)) || verdict_of(l).is_some_and(|v| v.code)
    });
    // each line: its bracketed verdicts wherever they stand (its own words kept), else a verdict at its start, else words
    let read: Vec<(String, Option<Verdict>)> = text
        .lines()
        .flat_map(|l| match verdicts_in(l, codes_decide) {
            Some((found, left)) => {
                let mut out: Vec<(String, Option<Verdict>)> = found.into_iter().map(|v| (String::new(), Some(v))).collect();
                out.extend(left.map(|w| (w, None)));
                out
            }
            None => vec![(l.to_string(), verdict_of(l))],
        })
        .collect();
    let all: Vec<&Verdict> = read.iter().filter_map(|(_, v)| v.as_ref()).collect();
    if all.is_empty() {
        return None;
    }
    let codes: Vec<&Verdict> = all.iter().copied().filter(|v| v.code).collect();
    let verdicts = if codes.is_empty() { all } else { codes };
    let blocked: Vec<&&Verdict> = verdicts.iter().filter(|v| !v.ok).collect();
    let ok = blocked.is_empty();
    let why = blocked
        .iter()
        .find(|v| v.code && !v.why.is_empty())
        .or_else(|| blocked.iter().find(|v| !v.why.is_empty()))
        .map(|v| v.why.clone())
        .unwrap_or_default();
    let rest = read.iter().filter(|(_, v)| v.is_none()).map(|(l, _)| l.as_str()).collect::<Vec<_>>().join("\n");
    Some((ok, why, rest.trim().to_string()))
}

/// `text` without its rehearsal verdict lines (`read_rehearsal`), for the rehearsal's turn as the apps show it
/// (threads/timeline.rs). None: it has none.
pub fn without_verdict(text: &str) -> Option<String> {
    read_rehearsal(text).map(|(_, _, rest)| rest)
}

/// Whether a user row of Hermes is a rehearsal's note (`rehearsal_note`): the reply that follows carries its verdict
/// (threads/timeline.rs hides its lines).
pub fn rehearsal_note_in(raw: &str) -> bool {
    raw.contains(REHEARSAL_NOTE_MARK)
}

/// The person's line after a rehearsal that said nothing besides its verdict.
pub fn rehearsal_line(lang: &str, site: &str, ok: bool, blocked: &str) -> String {
    let blocked = if blocked.is_empty() { String::new() } else { format!(" : {blocked}") };
    match (lang, ok) {
        ("de", true) => format!("Probe auf {site} gemacht: Bis zur Bezahlung ist alles bereit."),
        ("de", false) => format!("Probe auf {site} blockiert{}.", blocked.replacen(" :", ":", 1)),
        ("es", true) => format!("Ensayo hecho en {site}: todo está listo hasta el pago."),
        ("es", false) => format!("Ensayo bloqueado en {site}{}.", blocked.replacen(" :", ":", 1)),
        ("en", true) => format!("Rehearsal done on {site}: everything is ready up to the payment."),
        ("en", false) => format!("Rehearsal blocked on {site}{}.", blocked.replacen(" :", ":", 1)),
        (_, true) => format!("Répétition faite sur {site} : tout est prêt jusqu'au paiement."),
        (_, false) => format!("Répétition bloquée sur {site}{blocked}."),
    }
}

/// What blocked a rehearsal the brake stopped, or that ended without a word (`stopped`: false), for the checklist.
fn cut_reason(lang: &str, stopped: bool) -> &'static str {
    match (lang, stopped) {
        ("de", true) => "abgebrochen, ich habe mich im Kreis gedreht",
        ("de", false) => "nicht zu Ende gebracht",
        ("es", true) => "detenido, estaba dando vueltas",
        ("es", false) => "no terminado",
        ("en", true) => "stopped, I was going round in circles",
        ("en", false) => "not finished",
        (_, true) => "arrêtée, je tournais en rond",
        (_, false) => "pas terminée",
    }
}

/// The person's line after a set-up turn that ended without a word (failed, out of time, cut).
pub fn unfinished_line(lang: &str, site: &str) -> String {
    match lang {
        "de" => format!("Ich konnte auf {site} nicht fertig werden. Versuch es gleich noch einmal."),
        "es" => format!("No he podido terminar en {site}. Inténtalo de nuevo en un momento."),
        "en" => format!("I couldn't finish on {site}. Try again in a moment."),
        _ => format!("Je n'ai pas pu finir sur {site}. Réessaie dans un moment."),
    }
}

/// What a set-up turn came to.
#[derive(Debug, Clone, PartialEq)]
pub enum SetUp {
    /// Its reply.
    Words(String),
    /// The brake stopped it: its plain line (runs.rs `stop_looping`).
    Stopped(String),
    /// It ended without a word, or never started.
    Unfinished,
}

/// One turn of the agent with `note` in the watcher's conversation (`started` told its run once Hermes took it).
/// Returns that conversation (None: none could be opened) and what the turn came to.
async fn setup_turn(
    state: &AppState,
    agent: &AgentConfig,
    conn: &Connector,
    note: String,
    started: impl FnOnce(&str),
) -> Option<(String, SetUp)> {
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
    let came = match pipeline.run_then(&turn, started).await {
        // the brake's line rather than the loop's last words
        Ok(ran) if ran.stopped.is_some() => SetUp::Stopped(ran.stopped.unwrap_or_default()),
        Ok(ran) if ran.answered() => SetUp::Words(ran.text),
        Ok(ran) => {
            tracing::info!(target: "bipbridge::urgent_pay", agent = %agent.name, watcher = %conn.id, status = %ran.status,
                "set-up turn ended without a word");
            SetUp::Unfinished
        }
        Err(why) => {
            tracing::info!(target: "bipbridge::urgent_pay", agent = %agent.name, watcher = %conn.id, why = %why,
                "set-up turn not run");
            SetUp::Unfinished
        }
    };
    tracing::info!(target: "bipbridge::urgent_pay", agent = %agent.name, watcher = %conn.id,
        stopped = matches!(came, SetUp::Stopped(_)), "set-up turn done");
    Some((session, came))
}

/// `text` for the person in the watcher's conversation `session` (a Boîte message, pushed when `notify`).
async fn tell(state: &AppState, agent: &AgentConfig, conn: &Connector, session: String, text: String, notify: bool) {
    if text.trim().is_empty() {
        return;
    }
    let delivery = Delivery {
        agent: agent.clone(),
        session,
        key: format!("purchase:{}:{}", conn.id, py::uuid4_hex()),
        title: conn.name.clone(),
        text,
        as_written: false,
        run_id: None,
        notify,
        files: Vec::new(),
    };
    Pipeline::of(state).deliver(&delivery).await;
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

/// The first e-mail of the « Coordonnées » `agent` may read (one complete for a delivery first), and its entry's label.
fn signup_email(state: &AppState, config: &Config, agent: &str) -> Option<(String, String)> {
    let mut entries = state.vault.contacts_for(config, agent).unwrap_or_default();
    entries.sort_by_key(|e| crate::vault_contacts::missing_for_delivery(&e.secret).len());
    entries.iter().find_map(|e| {
        let email = crate::vault_contacts::hermes_secret(&e.secret).get("email").map(py::py_str)?;
        Some((e.label.clone(), email)).filter(|(_, m)| !m.trim().is_empty())
    })
}

async fn account_route(State(state): State<AppState>, Path((agent, id)): Path<(String, String)>) -> ApiResult<Response> {
    let config = state.config();
    let (agent, conn, purchase) = purchase_of(&config, &agent, &id)?;
    if crate::sealed_keys::closed(&config) {
        return Err(crate::sealed_keys::http_locked()); // its logins and « Coordonnées » wait for the vault to reopen
    }
    let email = signup_email(&state, &config, &agent.name);
    let note = account_note(&purchase, email.as_ref().map(|(l, m)| (l.as_str(), m.as_str())));
    let lang = crate::handback::language_of(&config, &agent);
    tokio::spawn(async move {
        let Some((session, came)) = setup_turn(&state, &agent, &conn, note, |_| {}).await else { return };
        // never silence: its words, the brake's line, or that it could not finish (pushed: they tapped and wait)
        let (text, notify) = match came {
            SetUp::Words(text) => (text, false),
            SetUp::Stopped(line) => (line, true),
            SetUp::Unfinished => (unfinished_line(&lang, &purchase.site), true),
        };
        tell(&state, &agent, &conn, session, text, notify).await;
    });
    Ok(web::json_response(202, &json!({"ok": true})))
}

async fn rehearse_route(State(state): State<AppState>, Path((agent, id)): Path<(String, String)>) -> ApiResult<Response> {
    let config = state.config();
    let (agent, conn, purchase) = purchase_of(&config, &agent, &id)?;
    if crate::sealed_keys::closed(&config) {
        return Err(crate::sealed_keys::http_locked()); // a rehearsal reads the card and the login: once reopened
    }
    let Some(marked) = Rehearsal::start(&agent.name) else {
        return Err(HttpError::json(409, json!({"code": "rehearsal_running"})));
    };
    let lang = crate::handback::language_of(&config, &agent);
    let note = rehearsal_note(&purchase, &lang);
    tokio::spawn(async move {
        // pay_with_card refused to the rehearsal's own turn only, from its start (`refuses_payment`)
        let came = setup_turn(&state, &agent, &conn, note, |run| marked.running(run)).await;
        // the verdict line is the app's: never shown, the person's words only (or a plain line of ours)
        let (ok, blocked, said) = match came.as_ref().map(|(_, c)| c) {
            Some(SetUp::Words(text)) => match read_rehearsal(text) {
                Some((ok, blocked, rest)) if rest.is_empty() => {
                    let line = rehearsal_line(&lang, &purchase.site, ok, &blocked);
                    (ok, blocked, line)
                }
                Some((ok, blocked, rest)) => (ok, blocked, rest),
                None => (false, String::new(), text.clone()),
            },
            Some(SetUp::Stopped(line)) => (false, cut_reason(&lang, true).to_string(), line.clone()),
            Some(SetUp::Unfinished) => (false, cut_reason(&lang, false).to_string(), unfinished_line(&lang, &purchase.site)),
            None => (false, cut_reason(&lang, false).to_string(), String::new()),
        };
        let config = state.config();
        crate::watchers::change(&config, &agent.name, &conn.id, |rt| {
            rt.rehearsal = Some(Rehearsed { at: py::time_time(), ok, blocked: blocked.clone() })
        });
        drop(marked); // its verdict kept first: the checklist never shows « never » in between
        tracing::info!(target: "bipbridge::urgent_pay", agent = %agent.name, watcher = %conn.id, ok, "rehearsal done");
        if let Some((session, came)) = came {
            let notify = !matches!(came, SetUp::Words(_));
            tell(&state, &agent, &conn, session, said, notify).await;
        }
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
        assert!(note.contains("NEVER pay") && note.contains("45,00 €") && note.contains(REHEARSAL_OK));
        assert!(rehearsal_note_in(&format!("<!--bip:now samedi-->\n{note}")), "the timeline knows its turn");
        assert!(!rehearsal_note_in(&account_note(&Purchase::default(), None)));
        assert!(note.contains(REHEARSAL_BLOCKED) && note.contains("never translated"));
    }

    #[test]
    fn a_rehearsal_verdict_is_read_translated_and_never_shown() {
        let ok = read_rehearsal;
        // the code word, as asked
        assert_eq!(ok("[REHEARSAL OK]\nTout est prêt."), Some((true, String::new(), "Tout est prêt.".into())));
        assert_eq!(
            ok("[REHEARSAL BLOCKED] le site demande un code par SMS\nIl faudra ton téléphone."),
            Some((false, "le site demande un code par SMS".into(), "Il faudra ton téléphone.".into()))
        );
        assert_eq!(ok("[REHEARSAL BLOCKED: rupture de stock]"), Some((false, "rupture de stock".into(), String::new())));
        // translated anyway (13/11, the lab: « RÉPÉTITION RÉUSSIE » from GLM-5.3 Flash), any case, accents or none
        for line in [
            "RÉPÉTITION RÉUSSIE",
            "**Répétition réussie** ✅",
            "[RÉPÉTITION OK]",
            "Repetition : reussie",
            "PROBE OK",
            "Probedurchlauf erfolgreich",
            "Die Probe ist erfolgreich",
            "ENSAYO OK",
            "Ensayo exitoso",
            "rehearsal passed",
            "REHEARSAL_OK",
            "[RÉPÉTITION GÉNÉRALE OK]",
            "Ensayo general: exitoso",
            "Rehearsal result: OK",
        ] {
            let read = ok(&format!("{line}\nTout est prêt jusqu'au paiement."));
            if line.starts_with("Die ") {
                assert_eq!(read, None, "{line}: a sentence, not a verdict");
                continue;
            }
            assert_eq!(read, Some((true, String::new(), "Tout est prêt jusqu'au paiement.".into())), "{line}");
        }
        for (line, why) in [
            ("RÉPÉTITION BLOQUÉE : le site demande un SMS", "le site demande un SMS"),
            ("Répétition — bloquée", ""),
            ("PROBE BLOCKIERT: Captcha", "Captcha"),
            ("Ensayo bloqueado: sin stock", "sin stock"),
            ("[RÉPÉTITION GÉNÉRALE BLOQUÉE] le site demande un SMS", "le site demande un SMS"),
            ("**[RÉPÉTITION ÉCHOUÉE : connexion refusée]**", "connexion refusée"),
        ] {
            assert_eq!(ok(&format!("{line}\nDésolé.")), Some((false, why.into(), "Désolé.".into())), "{line}");
        }
        // the verdict at the end: still read, still not shown
        assert_eq!(
            ok("Je suis allé jusqu'au formulaire de carte.\n\n[REHEARSAL OK]"),
            Some((true, String::new(), "Je suis allé jusqu'au formulaire de carte.".into()))
        );
        // words that only look like one are the person's
        assert_eq!(ok("La répétition est terminée, tout va bien."), None);
        assert_eq!(ok("Répétitions OK pour demain"), None);
        assert_eq!(ok("Répétition de demain OK"), None);
        // a reply that is only its verdict: a plain line of ours
        assert_eq!(rehearsal_line("fr", "shop.example", true, ""), "Répétition faite sur shop.example : tout est prêt jusqu'au paiement.");
        assert_eq!(rehearsal_line("fr", "shop.example", false, "code SMS"), "Répétition bloquée sur shop.example : code SMS.");
        assert_eq!(rehearsal_line("en", "shop.example", false, "SMS code"), "Rehearsal blocked on shop.example: SMS code.");
        assert_eq!(rehearsal_line("de", "shop.example", false, ""), "Probe auf shop.example blockiert.");
    }

    #[test]
    fn the_account_note_names_the_email_and_keeps_out_of_the_carnet() {
        let purchase = Purchase { site: "shop.example".into(), what: "T-shirt".into(), cents: 0, currency: "EUR".into() };
        let note = account_note(&purchase, Some(("Maison", "camille@example.org")));
        assert!(note.contains("sign up with their e-mail camille@example.org, from their « Coordonnées » « Maison »"), "{note}");
        assert!(note.contains("never look for their details in the Carnet") && note.contains("vault_contact_details"));
        assert!(note.contains("vault_create_login") && note.contains("no ask_secret") && !note.contains("tool_search"));
        let note = account_note(&purchase, None);
        assert!(note.contains("have no e-mail: ask them for one"), "{note}");
        assert_eq!(unfinished_line("fr", "shop.example"), "Je n'ai pas pu finir sur shop.example. Réessaie dans un moment.");
    }
}

// -- independent re-review of cards-lab-fixes (14/11): its probes, failing on 8de44ad67, kept as tests ----------------
#[cfg(test)]
mod rcl_review_probes {
    use super::*;

    /// The code word says blocked; a translated « OK » line before it (a list of steps) must not win.
    #[test]
    fn rcl_blocked_code_word_wins_over_an_earlier_ok_line() {
        let reply = "Étapes :\n- Répétition OK jusqu'au panier\n[REHEARSAL BLOCKED] le site demande un code SMS";
        let read = read_rehearsal(reply).expect("a verdict");
        assert!(!read.0, "read as ok: {read:?}");
    }

    /// A line quoted from the page the agent read (« > Rehearsal OK ») must not win over the code word.
    #[test]
    fn rcl_a_quoted_line_never_overrides_the_blocked_code_word() {
        let reply = "Le site affichait ce bandeau :\n> Rehearsal OK\n[REHEARSAL BLOCKED] paiement impossible";
        let read = read_rehearsal(reply).expect("a verdict");
        assert!(!read.0, "read as ok: {read:?}");
    }

    /// « KO » after the in-between words: blocked, with its reason, and the line never shown.
    #[test]
    fn rcl_ko_is_a_blocked_verdict() {
        let read = read_rehearsal("[RÉPÉTITION GÉNÉRALE KO] le site demande un code SMS\nIl faudra ton téléphone.");
        assert_eq!(read, Some((false, "le site demande un code SMS".into(), "Il faudra ton téléphone.".into())));
    }

    /// The code word after an emoji (« ✅ [REHEARSAL OK] »): read, never shown.
    #[test]
    fn rcl_code_word_after_an_emoji_is_read() {
        let read = read_rehearsal("✅ [REHEARSAL OK]\nTout est prêt jusqu'au paiement.");
        assert_eq!(read, Some((true, String::new(), "Tout est prêt jusqu'au paiement.".into())));
    }

    /// A translated verdict first, the code word at the end as well: the code word never left in the person's words.
    #[test]
    fn rcl_code_word_never_left_in_the_words() {
        let read = read_rehearsal("Répétition réussie !\nTout est prêt.\n[REHEARSAL OK]").expect("a verdict");
        assert!(!read.2.contains("REHEARSAL"), "the person reads: {:?}", read.2);
    }
}

#[cfg(test)]
mod rcl_review_probes_2 {
    use super::*;

    /// Only the word right after the name is read: « done … but blocked » is read as ok.
    #[test]
    fn rcl_done_but_blocked_is_not_ok() {
        let read = read_rehearsal("Rehearsal done but blocked at the payment page: SMS code required.");
        assert!(read.as_ref().is_none_or(|r| !r.0), "read as ok: {read:?}");
    }
}


// -- second re-review of cards-lab-fixes (14/11, aeb2dc414): its probes, failing on aeb2dc414, kept as tests --
#[cfg(test)]
mod rcl2_review_probes {
    use super::*;

    /// The code word says OK; the translated line under it reassures « rien n'a été bloqué »: no blocked verdict.
    #[test]
    fn rcl2_ok_code_word_with_nothing_blocked_stays_ok() {
        let read = read_rehearsal(
            "[REHEARSAL OK]\nRépétition réussie : rien n'a été bloqué, le formulaire de carte s'est affiché.",
        );
        assert_eq!(read.as_ref().map(|r| r.0), Some(true), "{read:?}");
    }

    /// A translated verdict that says nothing failed: ok.
    #[test]
    fn rcl2_passed_and_nothing_failed_is_ok() {
        let read = read_rehearsal("Rehearsal passed: nothing failed, the card form showed.");
        assert_eq!(read.as_ref().map(|r| r.0), Some(true), "{read:?}");
    }

    /// A negation only counts in its own clause; a reason never opens on punctuation.
    #[test]
    fn rcl2_a_negation_elsewhere_and_a_reason_s_punctuation() {
        let read = read_rehearsal("Répétition OK mais le paiement n'est pas passé, bloqué par un code SMS").unwrap();
        assert_eq!((read.0, read.1.as_str()), (false, "par un code SMS"));
        let read = read_rehearsal("Rehearsal passed: nothing failed, the card form showed.").unwrap();
        assert_eq!(read, (true, String::new(), String::new()));
        let read = read_rehearsal("[REHEARSAL BLOCKED]: , le site demande un SMS").unwrap();
        assert_eq!(read.1, "le site demande un SMS");
        assert_eq!(read_rehearsal("Probe erfolgreich, nichts blockiert.").map(|r| r.0), Some(true));
    }
}


// -- third re-review of cards-lab-fixes (14/11, 0bdc7263a): its probes, failing on 0bdc7263a, kept as tests ---
#[cfg(test)]
mod rcl3_review_probes {
    use super::*;

    /// « pas bloquée mais KO »: the negation is of « bloquée », not of « KO » after « mais ».
    #[test]
    fn rcl3_not_blocked_but_ko_is_blocked() {
        let read = read_rehearsal("Répétition réussie jusqu'au panier, pas bloquée mais KO au paiement : code SMS demandé");
        assert_eq!(read.as_ref().map(|r| r.0), Some(false), "{read:?}");
    }

    /// « without any SMS but blocked »: « without » is not the negation of « blocked ».
    #[test]
    fn rcl3_without_sms_but_blocked_is_blocked() {
        let read = read_rehearsal("Rehearsal OK without any SMS but blocked at the payment page");
        assert_eq!(read.as_ref().map(|r| r.0), Some(false), "{read:?}");
    }

    /// The exact code word at the end of a line of words: read, and never left in the words.
    #[test]
    fn rcl3_code_word_at_the_end_of_a_line_is_read() {
        let read = read_rehearsal("Tout est prêt jusqu'à la page de paiement. [REHEARSAL OK]");
        assert_eq!(read, Some((true, String::new(), "Tout est prêt jusqu'à la page de paiement.".into())));
    }

    /// « No blocking issue, but [REHEARSAL BLOCKED] … » on one line: blocked, the code word never in the words.
    #[test]
    fn rcl3_code_word_inside_a_line_is_read() {
        let read = read_rehearsal("No blocking issue until the card form, but [REHEARSAL BLOCKED] the site asks for an SMS code");
        let read = read.expect("a verdict");
        assert!(!read.0 && !read.2.contains("REHEARSAL"), "{read:?}");
    }

    /// The bracketed reason, the words around the code word, markdown and emoji around it.
    #[test]
    fn rcl3_the_words_around_the_code_word() {
        assert_eq!(
            read_rehearsal("Voilà. [REHEARSAL BLOCKED: le site demande un SMS] Il faudra ton téléphone."),
            Some((false, "le site demande un SMS".into(), "Voilà. Il faudra ton téléphone.".into()))
        );
        assert_eq!(read_rehearsal("**[REHEARSAL OK]** ✅\nTout est prêt."), Some((true, String::new(), "Tout est prêt.".into())));
        assert_eq!(
            read_rehearsal("Rien n'a été bloqué. [REHEARSAL OK]").map(|r| (r.0, r.2)),
            Some((true, "Rien n'a été bloqué.".into()))
        );
    }
}


// -- fourth re-review of cards-lab-fixes (14/11, 36d5329d0): its probes, failing on 36d5329d0, kept as tests ---
#[cfg(test)]
mod rcl4_review_probes {
    use super::*;

    /// Both code words on one line: the blocked one wins (as among lines), and neither is left in the words.
    #[test]
    fn rcl4_blocked_and_ok_code_words_on_one_line_is_blocked() {
        let read = read_rehearsal("[REHEARSAL BLOCKED] le site demande un SMS, donc pas de [REHEARSAL OK] possible")
            .expect("a verdict");
        assert!(!read.0 && !read.2.contains("REHEARSAL"), "{read:?}");
    }

    /// The code word twice on one line: none of it left in the person's words.
    #[test]
    fn rcl4_a_repeated_code_word_never_left_in_the_words() {
        let read = read_rehearsal("[REHEARSAL OK] Tout est prêt jusqu'au paiement. [REHEARSAL OK]").expect("a verdict");
        assert!(read.0 && !read.2.contains("REHEARSAL"), "{read:?}");
    }

    /// The lab's own translation (« [RÉPÉTITION GÉNÉRALE OK] », 14/11) at the end of a line of words: read, never shown.
    #[test]
    fn rcl4_a_bracketed_translation_at_the_end_of_a_line_is_read() {
        let read = read_rehearsal("Tout est prêt jusqu'à la page de paiement. [RÉPÉTITION GÉNÉRALE OK]");
        assert_eq!(read, Some((true, String::new(), "Tout est prêt jusqu'à la page de paiement.".into())));
    }

    /// A bracketed blocked translation inside a line, its reason after it; an ok code word with a blocked translation
    /// on the same line (the code word decides); a bracket that is no verdict stays the person's.
    #[test]
    fn rcl4_more_forms_on_one_line() {
        assert_eq!(
            read_rehearsal("Pas de chance. [RÉPÉTITION BLOQUÉE] code SMS demandé"),
            Some((false, "code SMS demandé".into(), "Pas de chance.".into()))
        );
        assert_eq!(read_rehearsal("[REHEARSAL OK] (la [RÉPÉTITION BLOQUÉE] d'hier est réglée)").map(|r| r.0), Some(true));
        assert_eq!(
            read_rehearsal("Voir [la page 2] du site. [REHEARSAL OK]"),
            Some((true, String::new(), "Voir [la page 2] du site.".into()))
        );
    }
}


// -- fifth re-review of cards-lab-fixes (14/11, 4a8b2af11): its probes, failing on 4a8b2af11, kept as tests ----
#[cfg(test)]
mod rcl5_review_probes {
    use super::*;

    /// The code word decides; a blocked translation it overrides takes no words of the person's as its reason.
    #[test]
    fn rcl5_an_overridden_translation_swallows_no_words() {
        let read = read_rehearsal("[REHEARSAL OK] (la [RÉPÉTITION BLOQUÉE] d'hier est réglée)").expect("a verdict");
        assert!(read.0 && read.2.contains("d'hier est réglée"), "the person reads {:?}", read.2);
    }

    /// A blocked code word whose closing bracket is missing, inside a line (read before, by 36d5329d0): still read,
    /// never shown.
    #[test]
    fn rcl5_an_unclosed_blocked_code_word_inside_a_line_is_read() {
        let read = read_rehearsal("Désolé. [REHEARSAL BLOCKED: le site demande un code SMS");
        assert_eq!(read, Some((false, "le site demande un code SMS".into(), "Désolé.".into())));
    }

    /// Brackets that are no verdict stay the person's (check, expected to pass).
    #[test]
    fn rcl5_other_brackets_untouched() {
        let read = read_rehearsal("Voir [1] et [lien] ✅ [✅] [Billet Stromae](https://shop.example/b). [REHEARSAL OK]");
        assert_eq!(read.map(|r| r.2), Some("Voir [1] et [lien] ✅ [✅] [Billet Stromae](https://shop.example/b).".into()));
    }

    /// Across lines too: the code word on one line, an overridden translation on the next keeps its words; an unclosed
    /// bracket that is no code word stays the person's.
    #[test]
    fn rcl5_across_lines_and_other_unclosed_brackets() {
        let read = read_rehearsal("[REHEARSAL OK]\n(la [RÉPÉTITION BLOQUÉE] d'hier est réglée)").unwrap();
        assert!(read.0 && read.2.contains("d'hier est réglée"), "{read:?}");
        let read = read_rehearsal("Voir [la page 2 du site. [REHEARSAL OK]").unwrap();
        assert_eq!(read.2, "Voir [la page 2 du site.");
    }
}
