//! Payment cards in the agents' vault (owner decision 2026-10-22): a virtual card the person made for it (a Revolut
//! virtual or single-use card with its own spending limit), which an agent may pay small things with, each payment
//! approved by the person, without ever seeing the card's numbers.
//!
//! What is kept, and where: the card's number, expiry, cardholder name and billing postcode are the item's `secret`
//! in the bridge's encrypted vault (vault.rs), like a password, and nowhere else: unlike a login, a card is never
//! written into the agent's own Hermes vault. Only the bridge reads it, to type it into the agent's visible browser
//! (card_fill.rs) once the person said yes (card_pay.rs). The security code is never kept (PCI DSS, CNIL
//! recommendation 2018-303): the person types it in the approval card of each payment, the bridge holds it in memory
//! for that fill only. The apps see the label, the last 4 digits, the expiry, the brand, the flags, the agents and
//! sites allowed and what the agents paid with it; prompts, tool results, logs and the journal only ever carry the
//! label and the last 4 digits.
//!
//! Off by default everywhere: the person turns « Paiements par tes agents » on in the app (after a warning), and
//! « Suspendre tous les paiements » turns it off at once. An install can forbid it (`[vault] cards = "off"`). A card is
//! only ever used by the agents of this install (its own configuration's agents), never across a household.
//!
//! On a place of the all-included offer (decided 2026-10-27, « expérimental » during the beta), only when Kerfy Studio
//! switched it on there (`[payments] offer_cards`, off by default), and with the numbers on the phone: the card's
//! number, expiry, cardholder name and postcode stay on the device that added it (iOS Keychain / Android Keystore,
//! behind Face ID or the phone's lock). The bridge keeps a record without them (`on_phone`): label, brand, last 4
//! digits, a cap in euros the person must set (`[payments] offer_cap_max_eur` at most), allowed sites, agents, spent,
//! single-use state, and which device holds it. At each payment the person approves, that device sends them with the
//! security code (card_pay.rs `approve`), held in memory for that one fill and wiped when it ends. The person also
//! ticks that it is a virtual card with a cap, not their main card: the bank's cap is the real limit, ours a second.

use indexmap::IndexMap;
use serde_json::{json, Value};

use crate::browsers::pyx;
use crate::config::Config;
use crate::vault::VaultError;
use crate::Dict;

pub const KIND: &str = "card";
/// The payments kept on a card (the newest; older ones still count in « spent so far » through `spent`).
pub const PAYMENTS_KEPT: usize = 50;
/// The largest amount an agent may ask for at once (the card's own limit, set at the bank, is the real one).
pub const MAX_AMOUNT_CENTS: i64 = 10_000_000;

/// Whether this install may offer card payments at all: not when `[vault] cards = "off"`, nor on a « no computer »
/// server (`cloud`); on a place of the all-included offer (its guichet token, `place_backup::SETTINGS`) only when
/// Kerfy Studio switched it on (`[payments] offer_cards`), with the numbers on the phone (`phone_rules`).
pub fn allowed(config: &Config) -> bool {
    allowed_on(config, on_offer())
}

/// `allowed`, with whether this is a place of the offer given.
pub fn allowed_on(config: &Config, offer: bool) -> bool {
    config.hermes.isolated && config.cards != "off" && !config.cloud && (!offer || config.payments.offer_cards)
}

/// A place of the all-included offer (Kerfy Studio's servers).
pub fn on_offer() -> bool {
    crate::handover::on_offer()
}

/// How cards are kept on a place of the offer: numbers on the phone, a cap required (in cents, at most).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhoneRules {
    pub cap_max_cents: i64,
}

/// The offer's rules when this is one of its places; None on a self-hosted install (numbers in the bridge's vault).
pub fn phone_rules(config: &Config) -> Option<PhoneRules> {
    phone_rules_on(config, on_offer())
}

pub fn phone_rules_on(config: &Config, offer: bool) -> Option<PhoneRules> {
    offer.then(|| PhoneRules { cap_max_cents: config.payments.offer_cap_max_eur.clamp(1, 1000) * 100 })
}

/// The person's switch, « Paiements par tes agents » (off until they turn it on, after the warning in the app).
fn switch_path(config: &Config) -> std::path::PathBuf {
    std::path::PathBuf::from(config.data_dir()).join("vault").join("payments.json")
}

/// Card payments are on: allowed here, and switched on by the person. Off, nothing is paid and no card is added.
pub fn enabled(config: &Config) -> bool {
    allowed(config)
        && std::fs::read(switch_path(config))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v.get("enabled").and_then(Value::as_bool))
            .unwrap_or(false)
}

/// The person turns payments on or off (« Suspendre tous les paiements »: off at once).
pub fn set_enabled(config: &Config, on: bool) -> std::io::Result<()> {
    let path = switch_path(config);
    crate::py::makedirs(&path.parent().unwrap_or(&path).to_string_lossy(), 0o700)?;
    let text = crate::py::dumps(&json!({"enabled": on, "changed_at": crate::vault::now()}));
    let tmp = path.with_extension("tmp");
    crate::py::write_with_mode(&tmp.to_string_lossy(), text.as_bytes(), 0o600)?;
    std::fs::rename(&tmp, &path)
}

// -- the journal: every payment asked, approved or not, without any card number ----------------------------------

/// Kept 3 years (at least 13 months is required).
pub const JOURNAL_DAYS: i64 = 3 * 366;

fn journal_path(config: &Config) -> std::path::PathBuf {
    std::path::PathBuf::from(config.data_dir()).join("vault").join("payments-journal.jsonl")
}

/// One line in the journal: `entry` holds the card's label and last 4 digits, the site, the amount as read, the agent
/// and the outcome (asked, approved, denied, typed, clicked, cancelled, failed). Never a number or a code.
pub fn journal(config: &Config, entry: Value) {
    use std::io::Write;
    let mut entry = entry;
    if let Value::Object(map) = &mut entry {
        map.insert("at".into(), json!(crate::vault::now()));
    }
    let path = journal_path(config);
    let written = (|| -> std::io::Result<()> {
        crate::py::makedirs(&path.parent().unwrap_or(&path).to_string_lossy(), 0o700)?;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&path)?;
        writeln!(file, "{}", crate::py::dumps(&entry))?;
        if file.metadata()?.len() > 2_000_000 {
            prune_journal(&path)?;
        }
        Ok(())
    })();
    if let Err(err) = written {
        tracing::warn!(error = %err, "payment journal not written");
    }
}

fn prune_journal(path: &std::path::Path) -> std::io::Result<()> {
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(JOURNAL_DAYS)).format("%Y-%m-%dT%H:%M:%S").to_string();
    let text = std::fs::read_to_string(path)?;
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| {
            serde_json::from_str::<Value>(l).ok().and_then(|v| v.get("at")?.as_str().map(|a| a >= cutoff.as_str()))
                != Some(false)
        })
        .collect();
    let tmp = path.with_extension("tmp");
    crate::py::write_with_mode(&tmp.to_string_lossy(), format!("{}\n", kept.join("\n")).as_bytes(), 0o600)?;
    std::fs::rename(&tmp, path)
}

/// The journal, newest first (`limit` lines at most).
pub fn journal_entries(config: &Config, limit: usize) -> Vec<Value> {
    let text = std::fs::read_to_string(journal_path(config)).unwrap_or_default();
    text.lines().rev().filter_map(|l| serde_json::from_str::<Value>(l).ok()).take(limit).collect()
}

fn invalid(text: &str) -> VaultError {
    VaultError::Invalid(text.to_string())
}

/// The digits of what was typed (spaces and dashes dropped); None when anything else is in it.
pub fn digits(text: &str) -> Option<String> {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '0'..='9' => out.push(c),
            ' ' | '-' | '\u{a0}' | '\u{2009}' | '\u{202f}' => {}
            _ => return None,
        }
    }
    Some(out)
}

/// The Luhn check every payment card number passes.
pub fn luhn(number: &str) -> bool {
    if number.len() < 12 || number.len() > 19 || !number.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let sum: u32 = number
        .bytes()
        .rev()
        .enumerate()
        .map(|(i, b)| {
            let d = (b - b'0') as u32;
            if i % 2 == 1 {
                let x = d * 2;
                if x > 9 {
                    x - 9
                } else {
                    x
                }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// The card's network, from its first digits (what the apps show next to « •••• 4242 »).
pub fn brand(number: &str) -> &'static str {
    let n = |len: usize| number.get(..len).and_then(|p| p.parse::<u32>().ok()).unwrap_or(0);
    if number.starts_with('4') {
        "visa"
    } else if (51..=55).contains(&n(2)) || (2221..=2720).contains(&n(4)) {
        "mastercard"
    } else if n(2) == 34 || n(2) == 37 {
        "amex"
    } else if number.starts_with("6011") || number.starts_with("65") || (644..=649).contains(&n(3)) {
        "discover"
    } else if (3528..=3589).contains(&n(4)) {
        "jcb"
    } else if n(2) == 36 || n(2) == 38 || (300..=305).contains(&n(3)) {
        "diners"
    } else if number.starts_with("62") {
        "unionpay"
    } else if number.starts_with("50") || (56..=69).contains(&n(2)) {
        "maestro"
    } else {
        "card"
    }
}

/// (year, month) now, in UTC.
pub fn this_month() -> (u32, u32) {
    use chrono::Datelike;
    let now = chrono::Utc::now();
    (now.year() as u32, now.month())
}

/// A month (1–12) and a year (2 or 4 digits) as the app sent them; Err when the card has already expired.
pub fn expiry(month: &str, year: &str, now: (u32, u32)) -> Result<(u32, u32), VaultError> {
    let month: u32 = pyx::strip(month).parse().map_err(|_| invalid("expiry month must be 1 to 12"))?;
    if !(1..=12).contains(&month) {
        return Err(invalid("expiry month must be 1 to 12"));
    }
    let year_text = pyx::strip(year);
    let mut year: u32 = year_text.parse().map_err(|_| invalid("expiry year is not valid"))?;
    if year_text.len() <= 2 {
        year += 2000;
    }
    if !(2000..=2099).contains(&year) {
        return Err(invalid("expiry year is not valid"));
    }
    if (year, month) < now {
        return Err(invalid("this card has expired"));
    }
    Ok((month, year))
}

/// Still valid this month.
pub fn current(month: u32, year: u32) -> bool {
    (year, month) >= this_month()
}

/// « 12/27 » or « 12 / 2027 » in one field.
fn split_expiry(text: &str) -> Option<(String, String)> {
    let (m, y) = text.split_once('/')?;
    Some((pyx::strip(m).to_string(), pyx::strip(y).to_string()))
}

fn text_of(body: &Dict, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| body.get(*k).filter(|v| !v.is_null()).map(crate::py::py_str))
}

/// A site the card may be used on, as typed: its host (« www.sncf-connect.com » also allows « sncf-connect.com »'s
/// other subdomains only when the person typed the bare domain).
pub fn site_host(text: &str) -> Result<String, VaultError> {
    let origin = crate::vault::normalize_origin(text)?;
    let host = origin.split("://").nth(1).unwrap_or("").split(':').next().unwrap_or("").to_string();
    if host.is_empty() {
        return Err(invalid("site must be a web address"));
    }
    Ok(host)
}

/// The page's host is one the card allows (the host itself or one of its subdomains). No sites: any.
pub fn site_allowed(sites: &[String], host: &str) -> bool {
    let host = host.trim_end_matches('.').to_lowercase();
    sites.is_empty() || sites.iter().any(|s| host == *s || host.ends_with(&format!(".{s}")))
}

/// The card's public part (`record["card"]`) and its secret, from what the app sent; `existing` for an update
/// (fields left out are kept, the numbers included). Never logs anything it reads.
pub fn parse(body: &Dict, record: &mut Dict, secret: &mut Dict) -> Result<(), VaultError> {
    let mut meta: Dict = match record.get("card") {
        Some(Value::Object(map)) => map.clone(),
        _ => Dict::new(),
    };
    let new_numbers = text_of(body, &["number", "card_number"]);
    if let Some(number) = &new_numbers {
        let number = digits(number).ok_or_else(|| invalid("card number is not valid"))?;
        if !luhn(&number) {
            return Err(invalid("card number is not valid"));
        }
        if secret.get("number").and_then(Value::as_str) != Some(number.as_str()) {
            // New numbers (a single-use card regenerated): usable again.
            meta.insert("used".into(), json!(false));
        }
        meta.insert("last4".into(), json!(&number[number.len() - 4..]));
        meta.insert("brand".into(), json!(brand(&number)));
        secret.insert("number".into(), json!(number));
    }
    if !secret.contains_key("number") {
        return Err(invalid("card number is required"));
    }
    let (mut month, mut year) = (text_of(body, &["exp_month"]), text_of(body, &["exp_year"]));
    if let Some((m, y)) = text_of(body, &["expiry"]).as_deref().and_then(split_expiry) {
        (month, year) = (Some(m), Some(y));
    }
    if month.is_some() || year.is_some() {
        let month = month.or_else(|| secret.get("exp_month").map(crate::py::py_str)).unwrap_or_default();
        let year = year.or_else(|| secret.get("exp_year").map(crate::py::py_str)).unwrap_or_default();
        let (m, y) = expiry(&month, &year, this_month())?;
        secret.insert("exp_month".into(), json!(m));
        secret.insert("exp_year".into(), json!(y));
        meta.insert("exp_month".into(), json!(m));
        meta.insert("exp_year".into(), json!(y));
    }
    if !secret.contains_key("exp_month") {
        return Err(invalid("expiry is required"));
    }
    if text_of(body, &["csc", "cvc", "cvv"]).is_some() {
        // PCI DSS and the CNIL (2018-303): never kept, even with consent. The person types it at each payment.
        return Err(invalid("the security code is never kept: it is asked at each payment"));
    }
    secret.shift_remove("csc");
    if let Some(name) = text_of(body, &["name", "cardholder_name"]) {
        let name = pyx::strip(&name).to_string();
        if name.chars().count() > 100 || name.chars().any(char::is_control) {
            return Err(invalid("cardholder name is not valid"));
        }
        secret.insert("name".into(), json!(name));
    }
    if let Some(postal) = text_of(body, &["postal_code", "billing_postal_code"]) {
        let postal = pyx::strip(&postal).to_string();
        if postal.chars().count() > 20 || postal.chars().any(char::is_control) {
            return Err(invalid("postcode is not valid"));
        }
        secret.insert("postal_code".into(), json!(postal));
    }
    finish(body, record, meta, None)
}

/// The fields any card may hold on the phone or in the bridge's vault: never these on a place of the offer.
const PHONE_SECRETS: [&str; 13] = [
    "number",
    "card_number",
    "expiry",
    "exp_month",
    "exp_year",
    "name",
    "cardholder_name",
    "postal_code",
    "billing_postal_code",
    "csc",
    "cvc",
    "cvv",
    "secret",
];

/// A card on a place of the offer (`rules`): what the app sent, WITHOUT number or expiry (they stay on the phone
/// that added it). On creation: the device that holds it, the last 4 digits, the brand, a cap in euros and the
/// person's word that it is a virtual card with a cap. `secret` is emptied: nothing of the card's is kept here.
pub fn parse_phone(body: &Dict, record: &mut Dict, secret: &mut Dict, rules: PhoneRules) -> Result<(), VaultError> {
    if PHONE_SECRETS.iter().any(|k| body.get(*k).is_some_and(|v| !v.is_null())) {
        return Err(invalid("on this server, the card's number stays on your phone: it is never sent to be kept"));
    }
    secret.clear();
    let mut meta: Dict = match record.get("card") {
        Some(Value::Object(map)) => map.clone(),
        _ => Dict::new(),
    };
    let creating = meta.get("device_id").is_none();
    if let Some(device) = text_of(body, &["device_id"]) {
        let device = pyx::strip(&device).to_string();
        if !(8..=64).contains(&device.len())
            || !device.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(invalid("device is not valid"));
        }
        meta.insert("device_id".into(), json!(device));
        meta.insert("missing".into(), json!(false));
    } else if creating {
        return Err(invalid("the phone that keeps the card is required"));
    }
    if let Some(name) = text_of(body, &["device_name"]) {
        meta.insert("device_name".into(), json!(quoted(&name, 60)));
    }
    meta.entry("device_name").or_insert(json!(""));
    if let Some(last4) = text_of(body, &["last4"]) {
        let last4 = pyx::strip(&last4).to_string();
        if last4.len() != 4 || !last4.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid("last4 must be the card's last 4 digits"));
        }
        if meta.get("last4").and_then(Value::as_str) != Some(last4.as_str()) {
            meta.insert("used".into(), json!(false)); // new numbers (a single-use card regenerated): usable again
        }
        meta.insert("last4".into(), json!(last4));
    } else if creating {
        return Err(invalid("last4 must be the card's last 4 digits"));
    }
    if body.get("renewed").is_some_and(crate::py::truthy) {
        meta.insert("used".into(), json!(false));
    }
    if let Some(b) = text_of(body, &["brand"]) {
        let b = pyx::strip(&b).to_lowercase();
        let known = ["visa", "mastercard", "amex", "discover", "jcb", "diners", "unionpay", "maestro"];
        meta.insert("brand".into(), json!(if known.contains(&b.as_str()) { b.as_str() } else { "card" }));
    }
    meta.entry("brand").or_insert(json!("card"));
    if let Some(v) = body.get("missing").filter(|v| !v.is_null()) {
        meta.insert("missing".into(), json!(crate::py::truthy(v)));
    }
    if creating && !body.get("virtual_card").is_some_and(crate::py::truthy) {
        return Err(invalid("confirm that this is a virtual card with a cap, not your main card"));
    }
    meta.insert("on_phone".into(), json!(true));
    meta.shift_remove("exp_month");
    meta.shift_remove("exp_year");
    finish(body, record, meta, Some(rules))
}

/// What both kinds of cards share: single use, sites, the cap (required on a place of the offer, `rules`), the
/// defaults; and the record's other fields.
fn finish(body: &Dict, record: &mut Dict, mut meta: Dict, rules: Option<PhoneRules>) -> Result<(), VaultError> {
    if let Some(v) = body.get("cap_eur").filter(|v| !v.is_null()) {
        let max = rules.map(|r| r.cap_max_cents).unwrap_or(MAX_AMOUNT_CENTS);
        let wrong = || invalid(&format!("the cap must be between 1 and {} euros", max / 100));
        let cents = amount_cents(v).map_err(|_| wrong())?;
        if !(100..=max).contains(&cents) {
            return Err(wrong());
        }
        meta.insert("cap_cents".into(), json!(cents));
    }
    if rules.is_some() && !meta.contains_key("cap_cents") {
        return Err(invalid("a cap in euros is required"));
    }
    if let Some(v) = body.get("single_use").filter(|v| !v.is_null()) {
        meta.insert("single_use".into(), json!(crate::py::truthy(v)));
    }
    if let Some(v) = body.get("sites").filter(|v| !v.is_null()) {
        let Value::Array(items) = v else { return Err(invalid("sites must be a list")) };
        let mut hosts: Vec<String> = Vec::new();
        for item in items {
            let text = crate::py::py_str(item);
            if pyx::strip(&text).is_empty() {
                continue;
            }
            let host = site_host(&text)?;
            if !hosts.contains(&host) {
                hosts.push(host);
            }
        }
        meta.insert("sites".into(), json!(hosts));
    }
    for (key, default) in [("single_use", json!(false)), ("used", json!(false)), ("sites", json!([]))] {
        meta.entry(key).or_insert(default);
    }
    meta.entry("payments").or_insert(json!([]));
    meta.entry("spent").or_insert(json!({}));
    record.insert("card".into(), Value::Object(meta));
    record.insert("origin".into(), Value::Null);
    record.insert("identifier_type".into(), Value::Null);
    record.insert("identifier".into(), Value::Null);
    record.insert("has_otp".into(), json!(false));
    Ok(())
}

/// What the apps see of a card.
pub fn public(meta: &Value) -> Value {
    let get = |k: &str| meta.get(k).cloned().unwrap_or(Value::Null);
    let payments: Vec<Value> = match meta.get("payments") {
        Some(Value::Array(items)) => items.iter().rev().take(20).cloned().collect(),
        _ => Vec::new(),
    };
    let mut out = json!({"last4": get("last4"), "brand": get("brand"), "exp_month": get("exp_month"),
           "exp_year": get("exp_year"),
           "single_use": meta.get("single_use").and_then(Value::as_bool).unwrap_or(false),
           "used": meta.get("used").and_then(Value::as_bool).unwrap_or(false),
           "sites": meta.get("sites").cloned().unwrap_or(json!([])),
           "spent": meta.get("spent").cloned().unwrap_or(json!({})),
           "payments": payments});
    if let Value::Object(map) = &mut out {
        if meta.get("cap_cents").is_some() {
            map.insert("cap_cents".into(), get("cap_cents"));
        }
        if is_on_phone(meta) {
            map.insert("on_phone".into(), json!(true));
            map.insert("device_id".into(), get("device_id"));
            map.insert("device_name".into(), get("device_name"));
            map.insert("missing".into(), json!(meta.get("missing").and_then(Value::as_bool).unwrap_or(false)));
        }
    }
    out
}

/// A card whose numbers stay on a phone (a place of the offer).
pub fn is_on_phone(meta: &Value) -> bool {
    meta.get("on_phone").and_then(Value::as_bool) == Some(true)
}

/// Why a payment of `cents` in `currency` goes over the card's cap (spent so far included), in plain words for the
/// agent; None when the card has no cap or the payment fits.
pub fn over_cap(meta: &Value, label: &str, cents: i64, currency: &str) -> Option<String> {
    let cap = meta.get("cap_cents").and_then(Value::as_i64)?;
    if currency != "EUR" {
        return Some(format!(
            "The card « {label} » has a cap in euros: it only pays in EUR. Don't pay; hand the payment over to the \
             person with hand_over_payment."
        ));
    }
    let spent = meta.get("spent").and_then(|s| s.get("EUR")).and_then(Value::as_i64).unwrap_or(0);
    (spent.saturating_add(cents) > cap).then(|| {
        format!(
            "The card « {label} » has a cap of {} EUR and {} EUR were already paid with it: {} EUR is over it. Don't \
             pay; tell the person (they can raise the cap in the app, up to what their bank allows) or hand the \
             payment over with hand_over_payment.",
            format_cents(cap),
            format_cents(spent),
            format_cents(cents)
        )
    })
}

/// The card's numbers the phone sent with an approval (a card kept on the phone): checked against the card (the
/// device that holds it, its last 4 digits), with the security code typed in the approval. Err: (status, what the
/// app reads). Nothing of `body` is logged.
pub fn phone_values(body: &Value, meta: &Value, code: &str) -> Result<crate::card_fill::CardValues, (u16, Value)> {
    let text = |k: &str| body.get(k).filter(|v| !v.is_null()).map(crate::py::py_str);
    let bad = |what: &str| (400, json!(what));
    let device = text("device_id").unwrap_or_default();
    if meta.get("device_id").and_then(Value::as_str) != Some(device.as_str()) || device.is_empty() {
        return Err((
            409,
            json!({"code": "card_on_other_device",
                   "device_name": meta.get("device_name").cloned().unwrap_or(Value::Null)}),
        ));
    }
    let raw = zeroize::Zeroizing::new(text("number").unwrap_or_default());
    let mut number = zeroize::Zeroizing::new(digits(&raw).unwrap_or_default());
    if number.is_empty() {
        return Err(bad("the card's number is sent with the approval, from the phone that keeps it"));
    }
    if !luhn(&number) || meta.get("last4").and_then(Value::as_str) != number.get(number.len().saturating_sub(4)..) {
        return Err(bad("this is not this card's number"));
    }
    let (month, year) = (text("exp_month").unwrap_or_default(), text("exp_year").unwrap_or_default());
    let (exp_month, exp_year) = expiry(&month, &year, this_month()).map_err(|e| bad(&e.to_string()))?;
    let name = text("name").map(|n| pyx::strip(&n).to_string()).unwrap_or_default();
    let postal_code = text("postal_code").map(|p| pyx::strip(&p).to_string()).unwrap_or_default();
    if name.chars().count() > 100 || name.chars().any(char::is_control) {
        return Err(bad("cardholder name is not valid"));
    }
    if postal_code.chars().count() > 20 || postal_code.chars().any(char::is_control) {
        return Err(bad("postcode is not valid"));
    }
    let number = std::mem::take(&mut *number);
    Ok(crate::card_fill::CardValues { number, exp_month, exp_year, csc: code.to_string(), name, postal_code })
}

/// An amount as an agent read it (« 12,50 », « 12.5 », 12.5): in cents. Err in plain words for the agent.
pub fn amount_cents(value: &Value) -> Result<i64, String> {
    let text = match value {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.trim().replace([' ', '\u{a0}', '\u{202f}'], ""),
        _ => return Err("give the amount as a number, e.g. 12.50".into()),
    };
    let text = text.replace(',', ".");
    let (whole, frac) = text.split_once('.').unwrap_or((&text, ""));
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return Err("give the amount as a number, e.g. 12.50".into());
    }
    if frac.len() > 2 {
        return Err("the amount has more than 2 decimals".into());
    }
    let whole: i64 = whole.parse().map_err(|_| "the amount is too large".to_string())?;
    let frac: i64 = format!("{frac:0<2}").parse().unwrap_or(0);
    let cents = whole.checked_mul(100).and_then(|w| w.checked_add(frac)).ok_or("the amount is too large")?;
    if cents <= 0 {
        return Err("the amount must be more than 0".into());
    }
    if cents > MAX_AMOUNT_CENTS {
        return Err("the amount is too large".into());
    }
    Ok(cents)
}

/// « 12.50 ».
pub fn format_cents(cents: i64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}

/// An ISO 4217 code (« EUR »), from what the agent gave (« eur », « € », « $ »…).
pub fn currency(text: &str) -> Option<String> {
    let t = text.trim();
    let code = match t {
        "€" => "EUR".to_string(),
        "$" => "USD".to_string(),
        "£" => "GBP".to_string(),
        "CHF" | "chf" => "CHF".to_string(),
        _ => t.to_uppercase(),
    };
    (code.len() == 3 && code.bytes().all(|b| b.is_ascii_uppercase())).then_some(code)
}

/// A payment kept on the card, and the totals per currency.
pub fn record_payment(meta: &mut Dict, payment: Value) {
    let (Some(cents), Some(cur)) =
        (payment.get("cents").and_then(Value::as_i64), payment.get("currency").and_then(Value::as_str))
    else {
        return;
    };
    let mut spent: IndexMap<String, i64> = match meta.get("spent") {
        Some(Value::Object(map)) => map.iter().filter_map(|(k, v)| v.as_i64().map(|c| (k.clone(), c))).collect(),
        _ => IndexMap::new(),
    };
    *spent.entry(cur.to_string()).or_insert(0) += cents;
    meta.insert("spent".into(), json!(spent));
    let mut payments = match meta.remove("payments") {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    };
    payments.push(payment);
    let excess = payments.len().saturating_sub(PAYMENTS_KEPT);
    payments.drain(..excess);
    meta.insert("payments".into(), Value::Array(payments));
    if meta.get("single_use").and_then(Value::as_bool).unwrap_or(false) {
        meta.insert("used".into(), json!(true));
    }
}

/// What the person reads when asked, and what the apps parse to show their payment card (both apps:
/// `CardPayment`). One line, English, stable: `Card payment pay_0123456789ab: 12.50 EUR on sncf-connect.com with card
/// "Petits achats" ending 4242, for "Billet Paris-Lyon". Approve in the BipAgents app with the card's security code.`
/// The id is where the app sends the code (`POST /v1/vault/payments/<id>/approve`).
pub fn approval_message(
    id: &str,
    cents: i64,
    currency: &str,
    host: &str,
    label: &str,
    last4: &str,
    merchant: &str,
) -> String {
    format!(
        "Card payment {id}: {} {currency} on {host} with card \"{}\" ending {last4}, for \"{}\". Approve in the \
         BipAgents app with the card's security code.",
        format_cents(cents),
        quoted(label, 60),
        quoted(merchant, 80)
    )
}

/// A text shown between quotes on one line: no quotes, no line breaks, no control characters, cut.
pub fn quoted(text: &str, limit: usize) -> String {
    let clean: String = text
        .chars()
        .map(|c| match c {
            '"' | '\u{201c}' | '\u{201d}' => '\'',
            c if c.is_control() => ' ',
            c => c,
        })
        .filter(|c| !matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .collect();
    pyx::head(pyx::strip(&clean), limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_numbers_passing_luhn() {
        // The networks' published test numbers only.
        for good in ["4242424242424242", "4000056655665556", "5555555555554444", "378282246310005", "6011111111111117"]
        {
            assert!(luhn(good), "{good}");
        }
        assert!(!luhn("4242424242424241"));
        assert!(!luhn("4242"));
        assert_eq!(digits("4242 4242-4242 4242").as_deref(), Some("4242424242424242"));
        assert_eq!(digits("4242x"), None);
    }

    #[test]
    fn brands_from_the_first_digits() {
        assert_eq!(brand("4242424242424242"), "visa");
        assert_eq!(brand("5555555555554444"), "mastercard");
        assert_eq!(brand("2223003122003222"), "mastercard");
        assert_eq!(brand("378282246310005"), "amex");
        assert_eq!(brand("6011111111111117"), "discover");
        assert_eq!(brand("3566002020360505"), "jcb");
    }

    #[test]
    fn expiry_is_checked_against_this_month() {
        assert_eq!(expiry("12", "27", (2026, 10)).unwrap(), (12, 2027));
        assert_eq!(expiry("10", "2026", (2026, 10)).unwrap(), (10, 2026));
        assert!(matches!(expiry("9", "2026", (2026, 10)), Err(VaultError::Invalid(t)) if t == "this card has expired"));
        assert!(expiry("13", "27", (2026, 10)).is_err());
        assert!(expiry("ab", "27", (2026, 10)).is_err());
    }

    #[test]
    fn a_card_is_parsed_and_its_secret_kept_apart() {
        let body = json!({"number": "4242 4242 4242 4242", "expiry": "12/30", "name": "Alex Martin",
                          "postal_code": "75011", "single_use": true, "sites": ["https://www.SNCF-connect.com/a", ""]});
        let (mut record, mut secret) = (Dict::new(), Dict::new());
        parse(body.as_object().unwrap(), &mut record, &mut secret).unwrap();
        assert_eq!(secret["number"], "4242424242424242");
        assert_eq!((secret["exp_month"].clone(), secret["exp_year"].clone()), (json!(12), json!(2030)));
        let meta = &record["card"];
        assert_eq!(meta["last4"], "4242");
        assert_eq!(meta["brand"], "visa");
        assert_eq!(meta["sites"], json!(["www.sncf-connect.com"]));
        assert!(!crate::py::dumps(&public(meta)).contains("4242424242424242"));
        assert!(!crate::py::dumps(&public(meta)).contains("123"));
        // An update without the numbers keeps them; new numbers make a used single-use card usable again.
        let mut meta_map = meta.as_object().unwrap().clone();
        record_payment(&mut meta_map, json!({"cents": 1250, "currency": "EUR"}));
        assert_eq!(meta_map["used"], json!(true));
        record.insert("card".into(), Value::Object(meta_map));
        parse(json!({"single_use": true}).as_object().unwrap(), &mut record, &mut secret).unwrap();
        assert_eq!(record["card"]["used"], json!(true));
        parse(json!({"number": "4000056655665556"}).as_object().unwrap(), &mut record, &mut secret).unwrap();
        assert_eq!(record["card"]["used"], json!(false));
        assert_eq!(record["card"]["spent"], json!({"EUR": 1250}));
    }

    #[test]
    fn what_a_card_needs() {
        let parse_one = |body: Value| {
            let (mut record, mut secret) = (Dict::new(), Dict::new());
            parse(body.as_object().unwrap(), &mut record, &mut secret).err().map(|e| e.to_string())
        };
        assert_eq!(
            parse_one(json!({"number": "4242424242424241", "expiry": "12/30", "csc": "123"})).as_deref(),
            Some("card number is not valid")
        );
        assert_eq!(parse_one(json!({"expiry": "12/30", "csc": "123"})).as_deref(), Some("card number is required"));
        assert_eq!(
            parse_one(json!({"number": "4242424242424242", "csc": "123"})).as_deref(),
            Some("expiry is required")
        );
        assert_eq!(
            parse_one(json!({"number": "4242424242424242", "expiry": "12/30", "csc": "123"})).as_deref(),
            Some("the security code is never kept: it is asked at each payment")
        );
        assert_eq!(
            parse_one(json!({"number": "4242424242424242", "expiry": "12/30", "sites": ["ftp://x"]})).as_deref(),
            Some("site must be a web address")
        );
    }

    fn isolated() -> Config {
        let mut config = Config::with_key("k".repeat(32));
        config.hermes.isolated = true;
        config
    }

    #[test]
    fn the_offer_s_places_only_when_kerfy_switched_it_on() {
        let mut config = isolated();
        assert!(allowed_on(&config, false)); // self-hosted: as before
        assert!(!allowed_on(&config, true)); // the offer: off by default
        assert_eq!(phone_rules_on(&config, false), None);
        config.payments.offer_cards = true;
        assert!(allowed_on(&config, true));
        assert_eq!(phone_rules_on(&config, true), Some(PhoneRules { cap_max_cents: 15_000 }));
        config.cards = "off".into();
        assert!(!allowed_on(&config, true) && !allowed_on(&config, false));
    }

    const RULES: PhoneRules = PhoneRules { cap_max_cents: 15_000 };

    fn phone_body() -> Value {
        json!({"kind": "card", "label": "Petits achats", "last4": "4242", "brand": "visa",
               "device_id": "phone-a-0123456789", "device_name": "iPhone d'Alex", "cap_eur": "50",
               "virtual_card": true, "sites": ["shop.example"]})
    }

    fn parse_phone_one(body: Value) -> Result<(Dict, Dict), String> {
        let (mut record, mut secret) = (Dict::new(), Dict::new());
        parse_phone(body.as_object().unwrap(), &mut record, &mut secret, RULES).map_err(|e| e.to_string())?;
        Ok((record, secret))
    }

    #[test]
    fn on_the_offer_no_number_is_kept() {
        let (record, secret) = parse_phone_one(phone_body()).unwrap();
        assert!(secret.is_empty());
        let card = public(&record["card"]);
        assert_eq!(card["on_phone"], true);
        assert_eq!(card["device_id"], "phone-a-0123456789");
        assert_eq!((card["last4"].clone(), card["cap_cents"].clone()), (json!("4242"), json!(5000)));
        assert_eq!((card["exp_month"].clone(), card["exp_year"].clone()), (Value::Null, Value::Null));
        for field in ["number", "expiry", "exp_month", "name", "postal_code", "csc"] {
            let mut body = phone_body();
            body[field] = json!(if field == "number" { "4242424242424242" } else { "12" });
            assert!(parse_phone_one(body).unwrap_err().contains("stays on your phone"), "{field}");
        }
        // An update without numbers keeps the record as it is, with the phone it is on.
        let (mut record, mut secret) = parse_phone_one(phone_body()).unwrap();
        parse_phone(json!({"cap_eur": 80}).as_object().unwrap(), &mut record, &mut secret, RULES).unwrap();
        assert_eq!(record["card"]["cap_cents"], 8000);
        assert_eq!(record["card"]["device_id"], "phone-a-0123456789");
        parse_phone(json!({"missing": true}).as_object().unwrap(), &mut record, &mut secret, RULES).unwrap();
        assert_eq!(public(&record["card"])["missing"], true);
    }

    #[test]
    fn on_the_offer_a_capped_virtual_card_is_required() {
        let without = |key: &str| {
            let mut body = phone_body();
            body.as_object_mut().unwrap().remove(key);
            parse_phone_one(body).unwrap_err()
        };
        assert_eq!(without("cap_eur"), "a cap in euros is required");
        assert!(without("virtual_card").starts_with("confirm that this is a virtual card"));
        assert_eq!(without("device_id"), "the phone that keeps the card is required");
        assert_eq!(without("last4"), "last4 must be the card's last 4 digits");
        let mut over = phone_body();
        over["cap_eur"] = json!(151);
        assert_eq!(parse_phone_one(over).unwrap_err(), "the cap must be between 1 and 150 euros");
        let mut not_ticked = phone_body();
        not_ticked["virtual_card"] = json!(false);
        assert!(parse_phone_one(not_ticked).is_err());
    }

    #[test]
    fn the_cap_counts_what_was_already_paid() {
        let mut meta = json!({"cap_cents": 5000, "spent": {"EUR": 4000}});
        assert_eq!(over_cap(&meta, "Petits achats", 1000, "EUR"), None);
        let over = over_cap(&meta, "Petits achats", 1001, "EUR").unwrap();
        assert!(over.contains("cap of 50.00 EUR") && over.contains("40.00 EUR were already paid"), "{over}");
        assert!(over_cap(&meta, "Petits achats", 100, "USD").unwrap().contains("only pays in EUR"));
        meta.as_object_mut().unwrap().remove("cap_cents");
        assert_eq!(over_cap(&meta, "x", 9_999_999, "USD"), None); // no cap (self-hosted): as before
    }

    #[test]
    fn the_phone_s_numbers_must_be_this_card_s() {
        let meta = json!({"on_phone": true, "device_id": "phone-a-0123456789", "device_name": "iPhone d'Alex",
                          "last4": "4242"});
        let sent = json!({"device_id": "phone-a-0123456789", "number": "4242 4242 4242 4242", "exp_month": 12,
                          "exp_year": 30, "name": "Alex Martin"});
        let values = phone_values(&sent, &meta, "737").ok().unwrap();
        assert_eq!(
            (values.number.as_str(), values.exp_year, values.name.as_str()),
            ("4242424242424242", 2030, "Alex Martin")
        );
        let mut other = sent.clone();
        other["device_id"] = json!("phone-b-0123456789");
        let (status, detail) = phone_values(&other, &meta, "737").err().unwrap();
        assert_eq!(
            (status, detail["code"].clone(), detail["device_name"].clone()),
            (409, json!("card_on_other_device"), json!("iPhone d'Alex"))
        );
        let mut wrong = sent.clone();
        wrong["number"] = json!("4000056655665556");
        assert_eq!(phone_values(&wrong, &meta, "737").err().unwrap().0, 400);
        let mut expired = sent.clone();
        expired["exp_year"] = json!(2020);
        assert_eq!(phone_values(&expired, &meta, "737").err().unwrap().0, 400);
    }

    #[test]
    fn sites_allow_their_subdomains() {
        let sites = vec!["sncf-connect.com".to_string()];
        assert!(site_allowed(&sites, "sncf-connect.com"));
        assert!(site_allowed(&sites, "www.sncf-connect.com"));
        assert!(!site_allowed(&sites, "sncf-connect.com.evil.example"));
        assert!(!site_allowed(&sites, "evilsncf-connect.com"));
        assert!(site_allowed(&[], "anything.example"));
    }

    #[test]
    fn amounts_as_agents_read_them() {
        assert_eq!(amount_cents(&json!("12,50")), Ok(1250));
        assert_eq!(amount_cents(&json!("12.5")), Ok(1250));
        assert_eq!(amount_cents(&json!(12.5)), Ok(1250));
        assert_eq!(amount_cents(&json!("1 234,00")), Ok(123400));
        assert!(amount_cents(&json!("0")).is_err());
        assert!(amount_cents(&json!("12.505")).is_err());
        assert!(amount_cents(&json!("-3")).is_err());
        assert!(amount_cents(&json!("abc")).is_err());
        assert_eq!(currency("eur").as_deref(), Some("EUR"));
        assert_eq!(currency("€").as_deref(), Some("EUR"));
        assert_eq!(currency("euros"), None);
    }

    #[test]
    fn the_approval_line_is_one_stable_line() {
        let line = approval_message(
            "pay_0123456789ab",
            1250,
            "EUR",
            "sncf-connect.com",
            "Petits \"achats\"",
            "4242",
            "Billet\nParis-Lyon",
        );
        assert_eq!(
            line,
            "Card payment pay_0123456789ab: 12.50 EUR on sncf-connect.com with card \"Petits 'achats'\" ending 4242, for \
             \"Billet Paris-Lyon\". Approve in the BipAgents app with the card's security code."
        );
    }
}
