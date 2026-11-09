//! « Dépenses partagées » (Alex, 9/11), Tricount-style: the third kind of friends' card (friends_cards.rs). A project
//! agent shared with friends notes who paid what (its tool `expense`); the bridge keeps the account and works out the
//! balances and the fewest transfers to settle, exactly, in cents — the model never splits in its head (it got sums
//! wrong and corrected itself mid-reply). Design: internal/notes/shared-expenses-card.md.
//!
//! **The card.** One per project agent, kept with its other cards in `<data>/friends-cards.json` (same file, same
//! lock): `{id, type: "expenses", title, currency, people: [name], expenses: [{id, what, by, cents, for: [name],
//! at}], next, made}`. It travels like a vote: a `MEDIA:/bipagents/cards/<agent>/<id>` line in the agent's reply, the
//! owner's app reads it (`GET /v1/project-agents/{agent}/cards/{id}`), the share's copy sends it as its `card` item,
//! sealed with the share's key, again whenever it changes. Friends only read it: no action is taken on it.
//!
//! **The sums.** An expense is split equally between its people; the cents that don't divide go one each, starting
//! at a place that turns with the expense's number (so the same person doesn't always pay the odd cent), in the order
//! of the names. Balance = paid − owed (they add up to 0, exactly). The transfers: the most groups of people whose
//! balances cancel out among themselves (exact up to `EXACT_MAX` people with a balance; a group of k settles in
//! k − 1 transfers, which is the fewest), the largest debtor paying the largest creditor inside each group.
//!
//! **The owner** deletes an expense from the card (`DELETE …/cards/{card}/expenses/{id}`); the agent hears of it at
//! its next turn (friends_cards.rs `take_note`). To correct one, they say it in the chat: the agent edits by its id.
//!
//! No Python counterpart.

use std::collections::BTreeMap;

use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::delete;
use axum::Router;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::friends_cards as fc;
use crate::tools_knowledge as tk;
use crate::web::{self, ApiResult, HttpError};
use crate::Dict;

pub const TOOL_NAME: &str = "expense";
pub const KIND: &str = "expenses";
pub const WHAT_MAX: usize = 60;
pub const NAME_MAX: usize = 30;
pub const PEOPLE_MAX: usize = 30;
/// Expenses one project keeps (refused beyond: the agent says so).
pub const EXPENSES_MAX: usize = 300;
/// The largest amount of one expense, in cents (1 000 000.00).
pub const CENTS_MAX: i64 = 100_000_000;
/// Up to this many people with a balance, the fewest transfers are found exactly; beyond, a greedy settling (at most
/// one transfer fewer than people).
pub const EXACT_MAX: usize = 16;

fn s(v: &Value, key: &str) -> String {
    tk::str_or_empty(v.get(key))
}

/// How names compare: case and spaces aside.
pub fn key(name: &str) -> String {
    tk::squeeze(name).to_lowercase()
}

// -- the sums (pure) ------------------------------------------------------------------------------------------------

/// One expense, as the sums need it.
#[derive(Clone, Debug, PartialEq)]
pub struct Expense {
    /// Its number (`e<n>`): where its odd cents start.
    pub n: usize,
    pub by: String,
    pub cents: i64,
    pub people: Vec<String>,
}

/// What each of `people` owes of `cents`: equal parts, the odd cents one each from place `n % len` in the names'
/// order. Sorted by name; adds up to `cents` exactly.
pub fn split(cents: i64, people: &[String], n: usize) -> Vec<(String, i64)> {
    let mut names: Vec<String> = Vec::new();
    for p in people {
        if !names.iter().any(|q| key(q) == key(p)) {
            names.push(p.clone());
        }
    }
    names.sort_by_key(|p| key(p));
    if names.is_empty() {
        return Vec::new();
    }
    let len = names.len() as i64;
    let (base, odd) = (cents.div_euclid(len), cents.rem_euclid(len));
    let start = (n % names.len()) as i64;
    names
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            let place = (i as i64 - start).rem_euclid(len);
            (p, base + i64::from(place < odd))
        })
        .collect()
}

/// Each person's balance (paid − owed), by name key, in name order; everyone named appears (0 included).
pub fn balances(expenses: &[Expense]) -> Vec<(String, i64)> {
    let mut out: BTreeMap<String, (String, i64)> = BTreeMap::new();
    for e in expenses {
        out.entry(key(&e.by)).or_insert_with(|| (e.by.clone(), 0)).1 += e.cents;
        for (p, owed) in split(e.cents, &e.people, e.n) {
            out.entry(key(&p)).or_insert_with(|| (p.clone(), 0)).1 -= owed;
        }
    }
    out.into_values().collect()
}

/// The fewest transfers `(from, to, cents)` that bring every balance to 0 (balances adding up to 0, in name order).
pub fn settle(balances: &[(String, i64)]) -> Vec<(String, String, i64)> {
    let open: Vec<(String, i64)> = balances.iter().filter(|(_, c)| *c != 0).cloned().collect();
    let groups = if open.len() <= EXACT_MAX { zero_groups(&open) } else { vec![(0..open.len()).collect()] };
    let mut out = Vec::new();
    for group in groups {
        let mut left: Vec<(String, i64)> = group.iter().map(|&i| open[i].clone()).collect();
        loop {
            // the largest debtor pays the largest creditor (ties: name order, as given)
            let debtor = (0..left.len()).filter(|&i| left[i].1 < 0).min_by_key(|&i| (left[i].1, i));
            let creditor = (0..left.len()).filter(|&i| left[i].1 > 0).max_by_key(|&i| (left[i].1, std::cmp::Reverse(i)));
            let (Some(d), Some(c)) = (debtor, creditor) else { break };
            let amount = (-left[d].1).min(left[c].1);
            out.push((left[d].0.clone(), left[c].0.clone(), amount));
            left[d].1 += amount;
            left[c].1 -= amount;
        }
    }
    out.sort_by_key(|(from, to, _)| (key(from), key(to)));
    out
}

/// `open` (every balance non-zero, adding up to 0) cut into the most groups that each add up to 0 (each settles in
/// one transfer fewer than its people, so the most groups make the fewest transfers). Deterministic.
fn zero_groups(open: &[(String, i64)]) -> Vec<Vec<usize>> {
    let n = open.len();
    if n == 0 {
        return Vec::new();
    }
    let full = (1usize << n) - 1;
    let mut sum = vec![0i64; full + 1];
    for mask in 1..=full {
        let low = mask.trailing_zeros() as usize;
        sum[mask] = sum[mask & (mask - 1)] + open[low].1;
    }
    // best[mask]: the most zero-sum prefixes of an order of mask's people (= the most zero-sum groups it splits into)
    let mut best = vec![0u8; full + 1];
    for mask in 1..=full {
        let mut most = 0u8;
        let mut rest = mask;
        while rest != 0 {
            let bit = rest.isolate_lowest_one();
            most = most.max(best[mask ^ bit]);
            rest ^= bit;
        }
        best[mask] = most + u8::from(sum[mask] == 0);
    }
    // the order back: take people off the full set, keeping the count, then cut where a prefix adds up to 0
    let mut order = Vec::with_capacity(n);
    let mut mask = full;
    while mask != 0 {
        let here = best[mask] - u8::from(sum[mask] == 0);
        let i = (0..n).find(|&i| mask & (1 << i) != 0 && best[mask ^ (1 << i)] == here).expect("a person");
        order.push(i);
        mask ^= 1 << i;
    }
    order.reverse();
    let mut groups = Vec::new();
    let (mut group, mut total) = (Vec::new(), 0i64);
    for i in order {
        group.push(i);
        total += open[i].1;
        if total == 0 {
            group.sort_unstable();
            groups.push(std::mem::take(&mut group));
        }
    }
    if !group.is_empty() {
        groups.push(group); // not reached when the balances add up to 0
    }
    groups
}

// -- the words --------------------------------------------------------------------------------------------------------

fn lang_of(lang: &str) -> String {
    lang.split(['-', '_']).next().unwrap_or("").to_lowercase()
}

fn symbol(currency: &str) -> &str {
    match currency {
        "EUR" => "€",
        "USD" => "$",
        "GBP" => "£",
        "JPY" => "¥",
        other => other,
    }
}

/// An amount as the project's language writes it: « 1 234,50 € », « €1,234.50 », « 1.234,50 € », « -12,00 € ».
pub fn money(cents: i64, currency: &str, lang: &str, signed: bool) -> String {
    let lang = lang_of(lang);
    let (units, rest) = (cents.unsigned_abs() / 100, cents.unsigned_abs() % 100);
    let (group, decimal) = match lang.as_str() {
        "fr" => ('\u{202f}', ','),
        "de" | "es" => ('.', ','),
        _ => (',', '.'),
    };
    let digits = units.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(group);
        }
        grouped.push(c);
    }
    let number = format!("{grouped}{decimal}{rest:02}");
    let sign = match (signed, cents.signum()) {
        (true, 1) => "+",
        (_, -1) => "−",
        _ => "",
    };
    let sym = symbol(currency);
    let english = !matches!(lang.as_str(), "fr" | "de" | "es");
    match (english, sym.chars().count() == 1) {
        (true, true) => format!("{sign}{sym}{number}"),
        (true, false) => format!("{sign}{sym} {number}"),
        (false, _) => format!("{sign}{number}\u{a0}{sym}"),
    }
}

/// The card's title in the project's language.
fn title(lang: &str) -> &'static str {
    match lang_of(lang).as_str() {
        "fr" => "Dépenses partagées",
        "de" => "Geteilte Ausgaben",
        "es" => "Gastos compartidos",
        _ => "Shared expenses",
    }
}

// -- the card ---------------------------------------------------------------------------------------------------------

fn expenses_of(card: &Value) -> Vec<Value> {
    card.get("expenses").and_then(Value::as_array).cloned().unwrap_or_default()
}

fn names(v: &Value) -> Vec<String> {
    v.as_array().map(|l| l.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

fn sums_of(card: &Value) -> Vec<Expense> {
    expenses_of(card)
        .iter()
        .map(|e| Expense {
            n: s(e, "id").trim_start_matches('e').parse().unwrap_or(0),
            by: s(e, "by"),
            cents: e["cents"].as_i64().unwrap_or(0),
            people: names(&e["for"]),
        })
        .collect()
}

/// The ready-to-draw part of the card (both apps and the friends' copy): `{currency, total, expenses: [{id, what, by,
/// amount, for, all}] latest first, balances: [{name, amount, cents}], transfers: [{from, to, amount}]}`.
pub fn view_fields(card: &Value, lang: &str) -> Dict {
    let currency = match s(card, "currency") {
        c if c.is_empty() => "EUR".to_string(),
        c => c,
    };
    let people = names(&card["people"]);
    let everyone: Vec<String> = { let mut k: Vec<String> = people.iter().map(|p| key(p)).collect(); k.sort(); k };
    let sums = sums_of(card);
    let total: i64 = sums.iter().map(|e| e.cents).sum();
    let list: Vec<Value> = expenses_of(card)
        .iter()
        .rev()
        .map(|e| {
            let shared = names(&e["for"]);
            let mut keys: Vec<String> = shared.iter().map(|p| key(p)).collect();
            keys.sort();
            json!({"id": e["id"], "what": e["what"], "by": e["by"],
                   "amount": money(e["cents"].as_i64().unwrap_or(0), &currency, lang, false),
                   "for": shared, "all": keys == everyone && everyone.len() > 1})
        })
        .collect();
    let owed = balances(&sums);
    let transfers: Vec<Value> = settle(&owed)
        .into_iter()
        .map(|(from, to, c)| json!({"from": from, "to": to, "amount": money(c, &currency, lang, false)}))
        .collect();
    let balances: Vec<Value> = owed
        .into_iter()
        .map(|(name, c)| json!({"name": name, "cents": c, "amount": money(c, &currency, lang, true)}))
        .collect();
    let mut out = Dict::new();
    out.insert("currency".into(), json!(currency));
    out.insert("total".into(), json!(money(total, &currency, lang, false)));
    out.insert("expenses".into(), json!(list));
    out.insert("balances".into(), json!(balances));
    out.insert("transfers".into(), json!(transfers));
    out
}

/// The project's language.
fn lang(config: &Config, agent: &str) -> String {
    crate::project_agent::entry(config, agent).map(|e| s(&e, "lang")).unwrap_or_default()
}

/// The people of the project the bridge knows: the card's, then the share's owner and friends (first names).
fn known_people(config: &Config, agent: &str, card: Option<&Value>) -> Vec<String> {
    let mut out: Vec<String> = card.map(|c| names(&c["people"])).unwrap_or_default();
    if let Some(share) = crate::read_shares::thread_share(config, agent) {
        let mut more = vec![fc::first_name(&s(&share, "from"))];
        for v in share.get("viewers").and_then(Value::as_array).cloned().unwrap_or_default() {
            if s(&v, "state") != "refused" {
                more.push(fc::first_name(&s(&v, "name")));
            }
        }
        for p in more {
            if !p.is_empty() && !out.iter().any(|q| key(q) == key(&p)) {
                out.push(p);
            }
        }
    }
    out
}

/// A name as the card writes it: the spelling already known for it, else as given (cleaned).
fn as_known(name: &str, known: &[String]) -> String {
    let name: String = tk::squeeze(name).chars().filter(|c| !c.is_control()).take(NAME_MAX).collect();
    let name = name.trim().to_string();
    known.iter().find(|k| key(k) == key(&name)).cloned().unwrap_or(name)
}

/// An amount as the model may give it (12.5, "12,50", "12.50 €") → cents; None when it isn't one.
pub fn cents_of(v: &Value) -> Option<i64> {
    let x = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(t) => {
            let t: String = t.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',' || *c == '-').collect();
            t.replace(',', ".").parse::<f64>().ok()?
        }
        _ => return None,
    };
    let cents = (x * 100.0).round();
    (cents.is_finite() && cents >= 1.0 && cents <= CENTS_MAX as f64).then_some(cents as i64)
}

/// A currency as the model may give it: an ISO code, or €/$/£.
fn currency_of(v: Option<&Value>) -> Option<Option<String>> {
    let t = tk::str_or_empty(v).trim().to_uppercase();
    match t.as_str() {
        "" => Some(None),
        "€" => Some(Some("EUR".into())),
        "$" => Some(Some("USD".into())),
        "£" => Some(Some("GBP".into())),
        c if c.len() == 3 && c.bytes().all(|b| b.is_ascii_uppercase()) => Some(Some(c.to_string())),
        _ => None,
    }
}

// -- the tool ---------------------------------------------------------------------------------------------------------

pub fn tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Note a shared expense of this project (or fix/delete one by id). The bridge keeps the account \
and works out who owes whom: never split sums yourself, repeat its result. Put the MEDIA: line it gives alone in your \
reply to show the card.",
        "inputSchema": {"type": "object", "properties": {
            "paid_by": {"type": "string"},
            "amount": {"type": "number"},
            "what": {"type": "string"},
            "for": {"type": "array", "items": {"type": "string"}, "description": "names; default: everyone"},
            "currency": {"type": "string", "description": "default EUR"},
            "id": {"type": "string", "description": "to fix or delete one"},
            "delete": {"type": "boolean"}}},
    })
}

fn err(text: &str) -> Dict {
    tk::tool_text(text, true)
}

/// The agent's call: an expense added, fixed or deleted; the account as it stands in words, and the card's line.
pub fn call(config: &Config, agent: &AgentConfig, arguments: &Dict) -> Dict {
    if !crate::project_agent::is_project(config, &agent.name) {
        return err("Only a project agent keeps shared expenses.");
    }
    if !fc::open(config, &agent.name) {
        return err("This project has ended: its account is closed.");
    }
    let lang = lang(config, &agent.name);
    let Some(currency) = currency_of(arguments.get("currency")) else {
        return err("currency: a code like EUR, USD, GBP.");
    };
    let fix = tk::str_or_empty(arguments.get("id")).trim().to_string();
    let deleting = arguments.get("delete").and_then(Value::as_bool) == Some(true);
    if deleting && fix.is_empty() {
        return err("delete: give the expense's id (e3…).");
    }
    let existing = fc::cards(config, &agent.name).into_iter().find(|c| s(c, "type") == KIND);
    let known = known_people(config, &agent.name, existing.as_ref());
    let what: String = tk::squeeze(&tk::str_or_empty(arguments.get("what"))).chars().filter(|c| !c.is_control()).take(WHAT_MAX).collect();
    let what = what.trim().to_string();
    let by = as_known(&tk::str_or_empty(arguments.get("paid_by")), &known);
    let amount = arguments.get("amount").filter(|v| !v.is_null());
    let cents = match amount {
        Some(v) => match cents_of(v) {
            Some(c) => Some(c),
            None => return err("amount: a positive number, like 42.50."),
        },
        None => None,
    };
    let shared: Option<Vec<String>> = arguments.get("for").and_then(Value::as_array).map(|l| {
        let mut out: Vec<String> = Vec::new();
        for p in l.iter().filter_map(Value::as_str).map(|p| as_known(p, &known)) {
            if !p.is_empty() && !out.iter().any(|q| key(q) == key(&p)) {
                out.push(p);
            }
        }
        out
    });
    if shared.as_ref().is_some_and(|l| l.is_empty()) {
        return err("for: the names of who shares it (or leave it out: everyone).");
    }
    if fix.is_empty() && (by.is_empty() || what.is_empty() || cents.is_none()) {
        return err("paid_by, amount and what are needed to note an expense.");
    }

    let changed = fc::update(config, |data| -> Result<(String, String), String> {
        let cards = data.get_mut("cards").and_then(Value::as_object_mut).expect("an object");
        let mine = cards.entry(agent.name.clone()).or_insert_with(|| json!({}));
        let mine = mine.as_object_mut().expect("an object");
        let id = match mine.iter().find(|(_, c)| s(c, "type") == KIND) {
            Some((id, _)) => id.clone(),
            None => {
                if fix.is_empty() {
                    let id = fc::new_id();
                    mine.insert(id.clone(), json!({"id": id, "type": KIND, "title": title(&lang), "currency": "EUR",
                        "people": [], "expenses": [], "next": 1, "options": [], "made": crate::py::time_time()}));
                    id
                } else {
                    return Err("No expense has been noted in this project yet.".into());
                }
            }
        };
        let card = mine.get_mut(&id).expect("the card");
        let mut list = expenses_of(card);
        let kept_currency = s(card, "currency");
        if let Some(c) = &currency {
            if *c != kept_currency && !list.is_empty() {
                let shown = symbol(&kept_currency).to_string();
                return Err(format!(
                    "This project's account is in {kept_currency} ({shown}): convert the amount to {kept_currency} \
                     first and say the rate you used in your reply."
                ));
            }
        }
        let everyone: Vec<String> = {
            let mut all = known.clone();
            if !by.is_empty() && !all.iter().any(|q| key(q) == key(&by)) {
                all.push(by.clone());
            }
            all
        };
        let said;
        if fix.is_empty() {
            if list.len() >= EXPENSES_MAX {
                return Err(format!("This account holds {EXPENSES_MAX} expenses at most."));
            }
            let people = shared.clone().unwrap_or_else(|| everyone.clone());
            if people.len() < 2 && people.iter().all(|p| key(p) == key(&by)) {
                return Err("for: who shares it? Nobody else is known in this project yet: give the names.".into());
            }
            let n = card["next"].as_u64().unwrap_or(1).max(1);
            let eid = format!("e{n}");
            card["next"] = json!(n + 1);
            list.push(json!({"id": eid, "what": what, "by": by, "cents": cents, "for": people,
                             "at": crate::py::time_time()}));
            said = format!("Noted {eid}");
        } else {
            let Some(at) = list.iter().position(|e| s(e, "id") == fix) else {
                return Err(format!("No expense {fix} in this account."));
            };
            if deleting {
                list.remove(at);
                said = format!("Deleted {fix}");
            } else {
                let e = &mut list[at];
                if !what.is_empty() {
                    e["what"] = json!(what);
                }
                if !by.is_empty() {
                    e["by"] = json!(by);
                }
                if let Some(c) = cents {
                    e["cents"] = json!(c);
                }
                if let Some(p) = &shared {
                    e["for"] = json!(p);
                }
                said = format!("Fixed {fix}");
            }
        }
        if let Some(c) = &currency {
            card["currency"] = json!(c);
        }
        // the people: those known, then anyone the expenses name
        let mut people = names(&card["people"]);
        for e in &list {
            for p in std::iter::once(s(e, "by")).chain(names(&e["for"])) {
                if !p.is_empty() && !people.iter().any(|q| key(q) == key(&p)) {
                    people.push(p);
                }
            }
        }
        people.truncate(PEOPLE_MAX);
        card["people"] = json!(people);
        card["expenses"] = json!(list);
        Ok((id, said))
    });
    let (id, said) = match changed {
        Ok(Ok(done)) => done,
        Ok(Err(why)) => return err(&why),
        Err(e) => {
            tracing::warn!(error = %e, "shared expense not kept");
            return err("The account could not be kept: try again in a moment.");
        }
    };
    let card = fc::card(config, &agent.name, &id).unwrap_or(Value::Null);
    tk::tool_text(&format!("{said}.\n{}\nMEDIA:{}/{}/{id}", account_words(&card, &lang), fc::PREFIX, agent.name), false)
}

/// The account in a few lines for the agent: the last expenses with their ids, the balances, the transfers.
pub fn account_words(card: &Value, lang: &str) -> String {
    let v = Value::Object(view_fields(card, lang));
    let mut out = Vec::new();
    let list = v["expenses"].as_array().cloned().unwrap_or_default();
    let line = |e: &Value| {
        let shared = if e["all"] == json!(true) { "everyone".to_string() } else { names(&e["for"]).join(", ") };
        format!("{} {} · {} paid {} · for {shared}", s(e, "id"), s(e, "what"), s(e, "by"), s(e, "amount"))
    };
    out.push(format!("Total {} in {} expenses. Latest:", s(&v, "total"), list.len()));
    out.extend(list.iter().take(5).map(line));
    let balances: Vec<String> = v["balances"].as_array().cloned().unwrap_or_default().iter()
        .map(|b| format!("{} {}", s(b, "name"), s(b, "amount"))).collect();
    out.push(format!("Balances: {}", balances.join(", ")));
    let transfers: Vec<String> = v["transfers"].as_array().cloned().unwrap_or_default().iter()
        .map(|t| format!("{} → {} {}", s(t, "from"), s(t, "to"), s(t, "amount"))).collect();
    out.push(if transfers.is_empty() { "All settled.".to_string() } else { format!("To settle: {}", transfers.join("; ")) });
    out.join("\n")
}

// -- the owner deletes one --------------------------------------------------------------------------------------------

/// The owner's « Supprimer » on an expense: gone from the account; the agent hears of it at its next turn.
pub fn delete_by_owner(config: &Config, agent: &str, card_id: &str, expense: &str) -> Result<(), (u16, &'static str)> {
    if !fc::open(config, agent) {
        return Err((409, "the project has ended"));
    }
    let lang = lang(config, agent);
    let owner = crate::read_shares::thread_share(config, agent).map(|sh| fc::first_name(&s(&sh, "from"))).unwrap_or_default();
    let done = fc::update(config, |data| {
        let card = data.get_mut("cards").and_then(|c| c.get_mut(agent)).and_then(|m| m.get_mut(card_id));
        let Some(card) = card.filter(|c| s(c, "type") == KIND) else { return Err((404, "no such card")) };
        let currency = s(card, "currency");
        let mut list = expenses_of(card);
        let Some(at) = list.iter().position(|e| s(e, "id") == expense) else { return Err((404, "no such expense")) };
        let gone = list.remove(at);
        card["expenses"] = json!(list);
        let amount = money(gone["cents"].as_i64().unwrap_or(0), &currency, &lang, false);
        let what = s(&gone, "what");
        let line = match lang_of(&lang).as_str() {
            "fr" => format!("{} a supprimé la dépense {expense} « {what} » ({amount})", who(&owner, "Ta personne")),
            "de" => format!("{} hat die Ausgabe {expense} „{what}“ ({amount}) gelöscht", who(&owner, "Deine Person")),
            "es" => format!("{} ha borrado el gasto {expense} «{what}» ({amount})", who(&owner, "Tu persona")),
            _ => format!("{} deleted the expense {expense} “{what}” ({amount})", who(&owner, "Your person")),
        };
        fc::queue_news(data, agent, line);
        Ok(())
    });
    match done {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "shared expense not deleted");
            Err((500, "not kept"))
        }
    }
}

fn who(name: &str, otherwise: &'static str) -> String {
    if name.is_empty() { otherwise.to_string() } else { name.to_string() }
}

async fn delete_route(
    State(state): State<AppState>,
    UrlPath((agent, card, expense)): UrlPath<(String, String, String)>,
) -> ApiResult<Response> {
    let config = state.config();
    if config.agent(Some(&agent)).is_none() || !crate::project_agent::is_project(&config, &agent) {
        return Err(HttpError::new(404, "no such project"));
    }
    delete_by_owner(&config, &agent, &card, &expense).map_err(|(code, why)| HttpError::new(code, why))?;
    fc::sync_soon(&state, &config, &agent);
    let found = fc::card(&config, &agent, &card).ok_or_else(|| HttpError::new(404, "no such card"))?;
    Ok(web::ok(fc::view(&config, &agent, &found)))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/project-agents/{agent}/cards/{card}/expenses/{expense}", delete(delete_route))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(n: usize, by: &str, cents: i64, people: &[&str]) -> Expense {
        Expense { n, by: by.into(), cents, people: people.iter().map(|p| p.to_string()).collect() }
    }

    fn total(b: &[(String, i64)]) -> i64 {
        b.iter().map(|(_, c)| c).sum()
    }

    /// Applying the transfers brings every balance to 0.
    fn settles(b: &[(String, i64)], t: &[(String, String, i64)]) -> bool {
        let mut left: BTreeMap<String, i64> = b.iter().map(|(n, c)| (key(n), *c)).collect();
        for (from, to, c) in t {
            assert!(*c > 0);
            *left.get_mut(&key(from)).unwrap() += c;
            *left.get_mut(&key(to)).unwrap() -= c;
        }
        left.values().all(|c| *c == 0)
    }

    #[test]
    fn a_split_adds_up_to_the_cent_and_its_odd_cents_turn() {
        let three = ["Sam", "léa", "Chi"].map(String::from);
        // 100.00 / 3: 33.34 + 33.33 + 33.33, the odd cent at a place that turns with the expense's number
        assert_eq!(split(10000, &three, 0), vec![("Chi".into(), 3334), ("léa".into(), 3333), ("Sam".into(), 3333)]);
        assert_eq!(split(10000, &three, 1), vec![("Chi".into(), 3333), ("léa".into(), 3334), ("Sam".into(), 3333)]);
        assert_eq!(split(10000, &three, 5), vec![("Chi".into(), 3333), ("léa".into(), 3333), ("Sam".into(), 3334)]);
        let two_odd = split(10001, &three, 2);
        assert_eq!(two_odd.iter().map(|(_, c)| c).sum::<i64>(), 10001);
        assert_eq!(two_odd, vec![("Chi".into(), 3334), ("léa".into(), 3333), ("Sam".into(), 3334)]);
        // the same name twice counts once (case aside)
        assert_eq!(split(1000, &["Léa".into(), "léa".into(), "Sam".into()], 0).len(), 2);
        assert_eq!(split(1, &three, 0).iter().map(|(_, c)| c).sum::<i64>(), 1);
        assert!(split(1000, &[], 0).is_empty());
    }

    #[test]
    fn balances_add_up_to_zero_and_everyone_named_appears() {
        let all = ["Alex", "Léa", "Sam", "Chi"];
        let list = vec![e(1, "Léa", 8400, &all), e(2, "Sam", 3000, &["Sam", "Chi"]), e(3, "Alex", 10000, &["Alex", "Léa", "Sam"])];
        let b = balances(&list);
        assert_eq!(total(&b), 0);
        // Alex: +100 − 21 − 33.34 (e3's odd cent at place 3 % 3 = 0: Alex) = 45.66; Léa: +84 − 21 − 33.33 = 29.67
        assert_eq!(b, vec![("Alex".into(), 4566), ("Chi".into(), -3600), ("Léa".into(), 2967), ("Sam".into(), -3933)]);
        // someone who only paid for others
        let b = balances(&[e(1, "Inès", 900, &["Léa", "Sam"])]);
        assert_eq!(b, vec![("Inès".into(), 900), ("Léa".into(), -450), ("Sam".into(), -450)]);
    }

    #[test]
    fn the_fewest_transfers_and_they_settle_everything() {
        // two pairs that cancel out: 2 transfers, not 3
        let b: Vec<(String, i64)> =
            vec![("A".into(), 500), ("B".into(), -700), ("C".into(), -500), ("D".into(), 700)];
        let t = settle(&b);
        assert_eq!(t, vec![("B".into(), "D".into(), 700), ("C".into(), "A".into(), 500)]);
        assert!(settles(&b, &t));
        // a greedy largest-first would make 4 here; the exact one finds 3 ({A, C, D} and {B, E} cancel out)
        let b: Vec<(String, i64)> = vec![
            ("A".into(), 600), ("B".into(), 400), ("C".into(), -300), ("D".into(), -300), ("E".into(), -400)];
        let t = settle(&b);
        assert!(settles(&b, &t));
        assert_eq!(t.len(), 3, "{t:?}");
        // all settled: nothing
        assert!(settle(&[("A".into(), 0), ("B".into(), 0)]).is_empty());
        // the same input, the same answer
        assert_eq!(settle(&b), t);
        // many people (beyond the exact search): still settles, at most n − 1
        let mut many: Vec<(String, i64)> = (0..20).map(|i| (format!("P{i:02}"), if i % 2 == 0 { 137 * (i + 1) } else { -137 * i })).collect();
        let fix = -total(&many);
        many.push(("Z".into(), fix));
        let t = settle(&many);
        assert!(settles(&many, &t) && t.len() < many.len());
    }

    #[test]
    fn a_random_account_always_settles_exactly() {
        // a small deterministic generator: many accounts, every balance back to 0, never more than n − 1 transfers
        let mut seed = 0x2545F4914F6CDD1Du64;
        let mut next = |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m
        };
        let people = ["Alex", "Léa", "Sam", "Chi", "Inès", "Noé", "Zoé"];
        for _ in 0..300 {
            let list: Vec<Expense> = (0..(1 + next(12) as usize))
                .map(|n| {
                    let mut who: Vec<&str> = people.iter().copied().filter(|_| next(2) == 0).collect();
                    if who.is_empty() {
                        who.push("Sam");
                    }
                    e(n + 1, people[next(7) as usize], 1 + next(50_000) as i64, &who)
                })
                .collect();
            let b = balances(&list);
            assert_eq!(total(&b), 0);
            let t = settle(&b);
            assert!(settles(&b, &t));
            assert!(t.len() < b.iter().filter(|(_, c)| *c != 0).count().max(1));
        }
    }

    #[test]
    fn money_as_each_language_writes_it() {
        assert_eq!(money(123450, "EUR", "fr", false), "1\u{202f}234,50\u{a0}€");
        assert_eq!(money(-1200, "EUR", "fr", true), "−12,00\u{a0}€");
        assert_eq!(money(4210, "EUR", "fr", true), "+42,10\u{a0}€");
        assert_eq!(money(0, "EUR", "fr", true), "0,00\u{a0}€");
        assert_eq!(money(123450, "EUR", "en", false), "€1,234.50");
        assert_eq!(money(5, "CHF", "en", false), "CHF 0.05");
        assert_eq!(money(123450, "EUR", "de", false), "1.234,50\u{a0}€");
        assert_eq!(money(99, "USD", "es-ES", false), "0,99\u{a0}$");
        assert_eq!(cents_of(&json!(12.5)), Some(1250));
        assert_eq!(cents_of(&json!("12,50 €")), Some(1250));
        assert_eq!(cents_of(&json!(0.1 + 0.2)), Some(30));
        assert_eq!(cents_of(&json!(-3)), None);
        assert_eq!(cents_of(&json!("abc")), None);
        assert_eq!(cents_of(&json!(2_000_000)), None);
    }
}
