//! « Coordonnées » in the vault (Alex, 2026-10-29): a name, a postal address, phones and e-mails, with a label
//! (« Kerfy Studio (pro) ») and « Indications » (`hint`: when the agent should use the entry). The vault's `address`
//! kind, extended: an address item of before (address fields only) stays valid.
//!
//! - **Kept** like every item of the vault (vault.rs: Fernet, the agents the person chose). Personal data, not
//!   secrets: the app reads the fields back to edit them (`contact` in the listing), and an agent that was given the
//!   entry reads it with `vault_contact_details` (a letter, an e-mail, a form in a message). Passwords and cards stay
//!   unreadable by the model.
//! - **Browser**: written into the agent's own Hermes vault as an `address` item (`hermes_secret`) while its browser
//!   is on, and only then (vault.rs `follow_browser`: an item there shows Hermes' five vault tools on every call), so
//!   `browser_vault_fill` fills a form's name, address, phone and e-mail (Hermes patch 0017 maps the name, phone and
//!   e-mail fields; an unpatched Hermes fills the address only, and one that refuses an incomplete address leaves the
//!   entry to the bridge's own read: handle `bridge`).
//! - **Others-facing agents** (vault.rs `others_facing`: the family's, project, shared agents, agents linked to another
//!   household) read an entry only when the person gave it to that very agent after the app's « … parle aussi avec …
//!   Continuer ? » (`confirmed_others`, kept as `confirmed_for` on the item). An agent that became others-facing after
//!   it got the entry reads nothing until the person gives it again. During a relative's turn or a shared conversation
//!   the tools are closed, as the person's other data.
//! - **Saved by an agent** (`vault_save_contact_details`): what the person gave it in the conversation (« garde mon
//!   adresse pro »), kept with `created_by`, given to that agent only, listed and editable in the app like any entry.
//!   No card first: the person typed the details themselves; the reply's chip (below) shows what was kept, and the
//!   tool's rule forbids saving details read in a page, a mail or a document. It never changes an entry the person
//!   made.
//! - **The chip**: each entry an agent saves or uses (read by its label) adds `<!--bip:vault <id> <label>-->` to the
//!   reply that ends the turn (made_files.rs, as the files the bridge made), the apps' hidden marks: they draw
//!   « 🔒 <label> », which opens the vault on that entry. The id and label only, never a field. A login an agent created
//!   (vault_logins.rs) and a card it paid with (card_pay.rs) get the same chip.
//! - **A listing used** (model bench, 2026-10-29: GLM-5.3 Flash, DeepSeek V4.1 Flash and a local model all called
//!   `vault_contact_details` without a label, then wrote the letter with the entry's address): the entries a listing
//!   returned in the turn are kept (`LISTED`), and at the turn's end each one whose distinctive details are in the
//!   reply gets its chip (`used_marks`: address line 1, postal code + city, a phone, an e-mail, the full name; case,
//!   spaces and accents ignored). A listing of exactly one entry: any of its details. An entry none of whose details
//!   are in the reply never gets a chip.
//!
//! No Python counterpart (owner group: browsers-places).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use serde_json::{json, Value};

use crate::browsers::pyx;
use crate::config::{AgentConfig, Config};
use crate::vault::VaultError;
use crate::{py, Dict};

/// The vault's kind of these entries (Hermes' own name).
pub const KIND: &str = "address";
/// Single-line fields, with their longest length.
pub const TEXT_FIELDS: [(&str, usize); 7] = [
    ("full_name", 200),
    ("address_line1", 200),
    ("address_line2", 200),
    ("city", 100),
    ("state", 100),
    ("postal_code", 20),
    ("country", 2),
];
pub const PHONES: &str = "phones";
pub const EMAILS: &str = "emails";
/// At most so many phones and e-mails per entry.
pub const MAX_EACH: usize = 5;
pub const MAX_PHONE: usize = 30;
pub const MAX_EMAIL: usize = 254;
pub const MAX_HINT: usize = 300;
/// The hidden mark of an entry in a reply (the apps draw it as a chip).
pub const MARK_OPEN: &str = "<!--bip:vault ";

/// ISO 3166-1 alpha-2, and XK (Kosovo), which phones list too.
pub const COUNTRIES: &str = "AD AE AF AG AI AL AM AO AQ AR AS AT AU AW AX AZ BA BB BD BE BF BG BH BI BJ BL BM BN BO BQ \
BR BS BT BV BW BY BZ CA CC CD CF CG CH CI CK CL CM CN CO CR CU CV CW CX CY CZ DE DJ DK DM DO DZ EC EE EG EH ER ES ET FI \
FJ FK FM FO FR GA GB GD GE GF GG GH GI GL GM GN GP GQ GR GS GT GU GW GY HK HM HN HR HT HU ID IE IL IM IN IO IQ IR IS IT \
JE JM JO JP KE KG KH KI KM KN KP KR KW KY KZ LA LB LC LI LK LR LS LT LU LV LY MA MC MD ME MF MG MH MK ML MM MN MO MP MQ \
MR MS MT MU MV MW MX MY MZ NA NC NE NF NG NI NL NO NP NR NU NZ OM PA PE PF PG PH PK PL PM PN PR PS PT PW PY QA RE RO RS \
RU RW SA SB SC SD SE SG SH SI SJ SK SL SM SN SO SR SS ST SV SX SY SZ TC TD TF TG TH TJ TK TL TM TN TO TR TT TV TW TZ UA \
UG UM US UY UZ VA VC VE VG VI VN VU WF WS XK YE YT ZA ZM ZW";

fn invalid(text: &str) -> VaultError {
    VaultError::Invalid(text.to_string())
}

/// One line, its control characters (line breaks included) as spaces, trimmed.
fn line(value: &Value) -> String {
    let text: String =
        py::py_str(value).chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    pyx::strip(&text).to_string()
}

pub fn valid_country(code: &str) -> bool {
    code.len() == 2 && COUNTRIES.split(' ').any(|c| c == code)
}

/// A phone number as people write it: digits, spaces, « . - ( ) / », one « + » first; 3 to 20 digits.
pub fn valid_phone(phone: &str) -> bool {
    let digits = phone.chars().filter(char::is_ascii_digit).count();
    let shape = phone.char_indices().all(|(i, c)| {
        c.is_ascii_digit() || matches!(c, ' ' | '.' | '-' | '(' | ')' | '/') || (c == '+' && i == 0)
    });
    shape && (3..=20).contains(&digits) && phone.chars().count() <= MAX_PHONE
}

/// `local@domain.tld`: no spaces, one « @ », a dot in the domain, no empty label.
pub fn valid_email(email: &str) -> bool {
    if email.chars().count() > MAX_EMAIL || email.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some((local, domain)) = email.split_once('@') else { return false };
    if local.is_empty() || local.len() > 64 || domain.contains('@') || !domain.contains('.') {
        return false;
    }
    domain.split('.').all(|label| {
        !label.is_empty() && !label.starts_with('-') && !label.ends_with('-') && !label.contains(['<', '>', ',', ';'])
    }) && !local.contains(['<', '>', ',', ';', '"'])
}

/// A list the app (or the agent) sent: an array of strings, or one string; empty ones dropped.
fn list_of(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items.iter().map(line).filter(|s| !s.is_empty()).collect(),
        Value::Null => Vec::new(),
        other => Some(line(other)).filter(|s| !s.is_empty()).into_iter().collect(),
    }
}

fn given<'a>(body: &'a Dict, key: &str) -> Option<&'a Value> {
    body.get(key).filter(|v| !v.is_null())
}

/// The entry's fields from `body` into `secret` (fields left out are kept; "" or [] clears one), its hint into
/// `record`. Each given field is checked; the entry must keep at least one.
pub fn parse(body: &Dict, record: &mut Dict, secret: &mut Dict) -> Result<(), VaultError> {
    for (name, limit) in TEXT_FIELDS {
        let Some(value) = given(body, name) else { continue };
        let mut text = line(value);
        if name == "country" {
            text = text.to_uppercase();
            if !text.is_empty() && !valid_country(&text) {
                return Err(invalid("country must be a two-letter country code"));
            }
        } else if text.chars().count() > limit {
            return Err(VaultError::Invalid(format!("{} is too long (at most {limit} characters)", words(name))));
        }
        if name == "postal_code" && !text.chars().all(|c| c.is_alphanumeric() || c == ' ' || c == '-') {
            return Err(invalid("postal code: letters, digits, spaces and dashes only"));
        }
        if text.is_empty() {
            secret.shift_remove(name);
        } else {
            secret.insert(name.into(), json!(text));
        }
    }
    for (key, valid, what) in [
        (PHONES, valid_phone as fn(&str) -> bool, "phone number"),
        (EMAILS, valid_email as fn(&str) -> bool, "e-mail address"),
    ] {
        let Some(value) = given(body, key) else { continue };
        let mut items: Vec<String> = Vec::new();
        for item in list_of(value) {
            let item = if key == EMAILS { item.to_string() } else { item };
            if !valid(&item) {
                return Err(VaultError::Invalid(format!("not a valid {what}: {}", pyx::head(&item, 40))));
            }
            if !items.iter().any(|i| i.eq_ignore_ascii_case(&item)) {
                items.push(item);
            }
        }
        if items.len() > MAX_EACH {
            return Err(VaultError::Invalid(format!("at most {MAX_EACH} of each ({key})")));
        }
        if items.is_empty() {
            secret.shift_remove(key);
        } else {
            secret.insert(key.into(), json!(items));
        }
    }
    // Hermes' own address items of before, kept as they are: `phone`/`email` never come from the app.
    if let Some(value) = given(body, "hint") {
        let hint = line(value);
        if hint.chars().count() > MAX_HINT {
            return Err(VaultError::Invalid(format!("the hint is too long (at most {MAX_HINT} characters)")));
        }
        if hint.is_empty() {
            record.shift_remove("hint");
        } else {
            record.insert("hint".into(), json!(hint));
        }
    }
    if !has_any(secret) {
        return Err(invalid("add at least one detail"));
    }
    Ok(())
}

fn words(name: &str) -> &'static str {
    match name {
        "full_name" => "the name",
        "address_line1" | "address_line2" => "an address line",
        "city" => "the city",
        "state" => "the state or region",
        "postal_code" => "the postal code",
        _ => "a field",
    }
}

fn has_any(secret: &Dict) -> bool {
    secret.values().any(|v| match v {
        Value::Array(items) => !items.is_empty(),
        other => !py::py_str(other).trim().is_empty(),
    })
}

/// The fields as the app reads them back (`contact`): every text field ("" when empty) and both lists.
pub fn public(secret: &Dict) -> Value {
    let mut out = Dict::new();
    for (name, _) in TEXT_FIELDS {
        out.insert(name.into(), json!(secret.get(name).map(py::py_str).unwrap_or_default()));
    }
    for key in [PHONES, EMAILS] {
        let items: Vec<String> = match secret.get(key) {
            Some(value) => list_of(value),
            None => Vec::new(),
        };
        out.insert(key.into(), json!(items));
    }
    Value::Object(out)
}

/// What the agent's Hermes vault keeps for its browser fill (agent/vault_store.py ADDRESS_FIELDS, patch 0017): the
/// text fields, the first phone and the first e-mail (Hermes keeps strings).
pub fn hermes_secret(secret: &Dict) -> Dict {
    let mut out = Dict::new();
    for (name, _) in TEXT_FIELDS {
        if let Some(text) = secret.get(name).map(py::py_str).filter(|t| !t.trim().is_empty()) {
            out.insert(name.into(), json!(text));
        }
    }
    for (key, single) in [(PHONES, "phone"), (EMAILS, "email")] {
        if let Some(first) = secret.get(key).map(list_of).and_then(|l| l.into_iter().next()) {
            out.insert(single.into(), json!(first));
        }
    }
    out
}

/// Whether `agent` may read the entry `record`: it was given the entry, and, when other people talk to it, the person
/// confirmed that for this very agent (`confirmed_for`).
pub fn readable_by(config: &Config, record: &Dict, agent: &str) -> bool {
    if record.get("kind").and_then(Value::as_str) != Some(KIND) {
        return false;
    }
    let given = matches!(record.get("agents"), Some(Value::Object(a)) if a.contains_key(agent));
    if !given {
        return false;
    }
    if !crate::vault::others_facing(config, agent) {
        return true;
    }
    matches!(record.get("confirmed_for"), Some(Value::Array(a)) if a.iter().any(|v| v.as_str() == Some(agent)))
}

/// The hidden mark of entry `id` (« Kerfy Studio (pro) » kept readable, nothing that could close the note).
pub fn mark(id: &str, label: &str) -> String {
    let label: String = label.replace("--", "—").chars().filter(|c| !c.is_control() && *c != '>').collect();
    format!("{MARK_OPEN}{id} {}-->", pyx::head(pyx::strip(&label), 80))
}

/// The item id of a mark made by `mark`.
pub fn mark_id(mark: &str) -> Option<&str> {
    mark.strip_prefix(MARK_OPEN)?.split([' ', '-']).next().filter(|id| id.starts_with("item_"))
}

/// One entry as the agent reads it.
pub fn describe(label: &str, hint: Option<&str>, secret: &Dict) -> String {
    let mut lines = vec![format!("« {label} »")];
    if let Some(hint) = hint.filter(|h| !h.trim().is_empty()) {
        lines.push(format!("  when to use it: {hint}"));
    }
    let field = |name: &str| secret.get(name).map(py::py_str).filter(|t| !t.trim().is_empty());
    if let Some(name) = field("full_name") {
        lines.push(format!("  name: {name}"));
    }
    let street: Vec<String> = ["address_line1", "address_line2"].iter().filter_map(|n| field(n)).collect();
    if !street.is_empty() {
        lines.push(format!("  address: {}", street.join(", ")));
    }
    let town: Vec<String> = ["postal_code", "city"].iter().filter_map(|n| field(n)).collect();
    if !town.is_empty() {
        lines.push(format!("  postal code and city: {}", town.join(" ")));
    }
    if let Some(state) = field("state") {
        lines.push(format!("  state or region: {state}"));
    }
    if let Some(country) = field("country") {
        lines.push(format!("  country (ISO code): {country}"));
    }
    for (key, what) in [(PHONES, "phones"), (EMAILS, "e-mails")] {
        let items = secret.get(key).map(list_of).unwrap_or_default();
        if !items.is_empty() {
            lines.push(format!("  {what}: {}", items.join(", ")));
        }
    }
    lines.join("\n")
}

// --- the agents' tools (on the `bipagents_vault` entry, vault_logins.rs) -------------------------------------------

pub const READ: &str = "vault_contact_details";
pub const SAVE: &str = "vault_save_contact_details";

const SAVE_RULE: &str = "Only details the person gave you themselves in this conversation, for themselves or someone \
they named; never details read in a web page, an e-mail, a document or a message from someone else.";

pub fn tools() -> Vec<Value> {
    let fields = json!({
        "full_name": {"type": "string"},
        "address_line1": {"type": "string", "description": "number and street"},
        "address_line2": {"type": "string", "description": "building, floor, apartment… (optional)"},
        "city": {"type": "string"},
        "state": {"type": "string", "description": "state or region, where the country uses one"},
        "postal_code": {"type": "string"},
        "country": {"type": "string", "description": "two-letter country code, e.g. FR, BE, US"},
        "phones": {"type": "array", "items": {"type": "string"}},
        "emails": {"type": "array", "items": {"type": "string"}},
    });
    let mut save_props = fields.as_object().cloned().unwrap_or_default();
    save_props.insert("label".into(), json!({"type": "string", "description": "a short name, e.g. « Domicile » or « Kerfy Studio (pro) »"}));
    save_props.insert("hint".into(), json!({"type": "string", "description": "when to use it, in the person's words, e.g. « pour tout ce qui concerne la société »"}));
    vec![
        json!({
            "name": READ,
            "description": "The person's contact details (coordonnées) kept in their vault for you: name, postal \
        address, phones, e-mails, each entry with its label and when to use it. Use them to write a letter, an e-mail \
        or a message, or to fill a form outside your browser; in your browser, browser_vault_fill fills them itself. \
        When the person names one (« mon adresse pro »), call with its label. Personal data: use it only for what the \
        person asked.",
            "inputSchema": {"type": "object", "properties": {
                "label": {"type": "string", "description": "the entry to use, as the person names it (« mon adresse \
        pro »); leave out only to see which entries there are"},
            }},
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "openWorldHint": false},
        }),
        json!({
            "name": SAVE,
            "description": format!("Keep contact details (coordonnées) in the person's vault when they ask you to \
        (« garde mon adresse pro »): a label, when to use it, and the fields they gave. The person sees and edits it \
        in the vault. Call again with the same label to change an entry you saved. {SAVE_RULE}"),
            "inputSchema": {"type": "object", "properties": save_props, "required": ["label"]},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "openWorldHint": false},
        }),
    ]
}

/// The line the tool's result ends with, for the reply's chip.
fn chip_line(mark: &str) -> String {
    format!("End your reply with this line, exactly as it is (the app shows it as a button to the entry): {mark}")
}

/// Whether the person's own data is closed for `agent` right now (a relative's turn, a shared conversation).
fn closed_now(state: &crate::app::AppState, agent: &str) -> bool {
    crate::agent_shares::relative_turn(&state.config(), agent) || crate::read_shares::shared_turn(state, agent)
}

const CLOSED: &str = "Your person's own data, the vault included, is closed in this conversation: other people read it. \
Answer from what you know.";

/// The entry's mark noted for the reply that ends the agent's turn (made_files.rs): « Coordonnées » saved or used, a
/// login an agent created (vault_logins.rs), a card it paid with (card_pay.rs).
pub fn note_mark(state: &crate::app::AppState, agent: &str, tool: &str, mark: &str) {
    let run = state.hub.turn_of(agent, tool).map(|t| t.run_id);
    crate::made_files::MADE.note(agent, mark, run.as_deref(), py::time_time());
}

pub async fn call(state: &crate::app::AppState, agent: &AgentConfig, name: &str, arguments: &Dict) -> Dict {
    use crate::tools_knowledge::tool_text;
    if closed_now(state, &agent.name) {
        return tool_text(CLOSED, true);
    }
    let config = state.config();
    match name {
        READ => {
            let wanted = arguments.get("label").filter(|v| !v.is_null()).map(line).unwrap_or_default();
            let entries = match state.vault.contacts_for(&config, &agent.name) {
                Ok(entries) => entries,
                Err(exc) => {
                    tracing::warn!(agent = %agent.name, error = %exc, "contact details not read");
                    return tool_text("The vault could not be read right now. Tell the person.", true);
                }
            };
            if entries.is_empty() {
                return tool_text(
                    "The person keeps no contact details for you in their vault. Ask them, or tell them they can add \
                     some in « Coffre-fort » → « Coordonnées » and give you access.",
                    false,
                );
            }
            if wanted.is_empty() {
                note_listed(state, &agent.name, name, &entries);
                let all: Vec<String> = entries.iter().map(|e| describe(&e.label, e.hint.as_deref(), &e.secret)).collect();
                return tool_text(
                    &format!(
                        "Contact details the person gave you (personal data: only for what they ask):\n{}\nCall again \
                         with the label of the one you use.",
                        all.join("\n")
                    ),
                    false,
                );
            }
            let found = entries.iter().find(|e| pyx::strip(&e.label).to_lowercase() == wanted.to_lowercase());
            let Some(entry) = found else {
                let labels: Vec<String> = entries.iter().map(|e| format!("« {} »", e.label)).collect();
                return tool_text(&format!("No entry « {wanted} ». Yours: {}.", labels.join(", ")), true);
            };
            let mark = mark(&entry.id, &entry.label);
            note_mark(state, &agent.name, name, &mark);
            tracing::info!(agent = %agent.name, item = %entry.id, "contact details read by the agent");
            tool_text(
                &format!(
                    "{}\nPersonal data: only for what the person asked.\n{}",
                    describe(&entry.label, entry.hint.as_deref(), &entry.secret),
                    chip_line(&mark)
                ),
                false,
            )
        }
        SAVE => match state.vault.save_contact(&config, agent, arguments).await {
            Ok(item) => {
                let mark = mark(&item.id, &item.label);
                note_mark(state, &agent.name, name, &mark);
                tracing::info!(agent = %agent.name, item = %item.id, "contact details saved by the agent");
                tool_text(
                    &format!(
                        "Saved as « {} » in the person's vault (« Coordonnées »); you can use it now (in your browser, \
                         if you have it on, browser_vault_fill fills it). Tell the person in a few words.\n{}",
                        item.label,
                        chip_line(&mark)
                    ),
                    false,
                )
            }
            Err(exc) => match exc.downcast_ref::<VaultError>() {
                Some(VaultError::Invalid(text)) => tool_text(&format!("Not saved: {text}."), true),
                Some(VaultError::Unprotected) | Some(VaultError::CannotHold(_)) => tool_text(
                    "Contact details can't be kept for you on this server. Tell the person they can add them in \
                     « Coffre-fort » → « Coordonnées ».",
                    true,
                ),
                _ => {
                    tracing::warn!(agent = %agent.name, error = %exc, "contact details not saved");
                    tool_text("The details could not be saved. Tell the person.", true)
                }
            },
        },
        other => tool_text(&format!("unknown tool: {other}"), true),
    }
}

// --- a listing used in the reply (module doc) ------------------------------------------------------------------

/// What is looked for in a reply: folded words (`folded`), or a phone number's digits (`phone_digits`).
#[derive(Debug, Clone, PartialEq)]
enum Needle {
    Words(String),
    Phone(String),
}

/// An entry a listing returned in a turn.
#[derive(Debug, Clone, PartialEq)]
pub struct Listed {
    /// Its chip (`mark`).
    pub mark: String,
    /// Address line 1, postal code + city, each phone and e-mail, the full name.
    distinctive: Vec<Needle>,
    /// Every detail but the country (for a listing of this one entry only).
    any: Vec<Needle>,
    /// A listing returned this entry alone.
    sole: bool,
    run_id: Option<String>,
    at: f64,
}

/// The entries listed, by agent (in memory: a restart forgets them, as made_files.rs's files).
static LISTED: LazyLock<Mutex<HashMap<String, Vec<Listed>>>> = LazyLock::new(Default::default);

/// A day: a turn's reply comes long before.
const KEEP_SECONDS: f64 = 86_400.0;

/// Lower case, without accents, every character but letters, digits and « @ » a space, spaces collapsed, one around:
/// « 8 Allée des Essais, Bât. C » and « 8 allee des essais bat c » are the same.
fn folded(text: &str) -> String {
    let plain = crate::tools_knowledge::ascii_fold(&crate::tools_knowledge::spaced_punctuation(text)).to_lowercase();
    let mapped: String = plain.chars().map(|c| if c.is_ascii_alphanumeric() || c == '@' { c } else { ' ' }).collect();
    format!(" {} ", mapped.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn text_needle(text: &str) -> Option<Needle> {
    let text = folded(text).trim().to_string();
    (text.chars().count() >= 3).then_some(Needle::Words(text))
}

/// A phone's last 9 digits (« +33 6 00 00 00 01 » and « 06 00 00 00 01 » the same), all of a shorter one; None under
/// 6 digits (too common in a text).
fn phone_digits(phone: &str) -> Option<Needle> {
    let digits: Vec<char> = phone.chars().filter(char::is_ascii_digit).collect();
    (digits.len() >= 6).then(|| Needle::Phone(digits[digits.len().saturating_sub(9)..].iter().collect()))
}

/// Digits as phones are written in a text (spaces, non-breaking ones, « . - ( ) / » between them).
static DIGIT_RUN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d[\d \x{a0}\x{202f}.\-()/]*\d").expect("valid regex"));

/// What an entry's details are looked for as: (distinctive ones, all but the country).
fn needles(secret: &Dict) -> (Vec<Needle>, Vec<Needle>) {
    let field = |name: &str| secret.get(name).map(py::py_str).filter(|t| !t.trim().is_empty());
    let listed = |key: &str| secret.get(key).map(list_of).unwrap_or_default();
    let mut distinctive: Vec<Needle> = Vec::new();
    distinctive.extend(field("address_line1").and_then(|t| text_needle(&t)));
    if let (Some(code), Some(city)) = (field("postal_code"), field("city")) {
        distinctive.extend(text_needle(&format!("{code} {city}")));
    }
    distinctive.extend(listed(PHONES).iter().filter_map(|p| phone_digits(p)));
    distinctive.extend(listed(EMAILS).iter().filter_map(|e| text_needle(e)));
    distinctive.extend(field("full_name").and_then(|t| text_needle(&t)));
    let mut any = distinctive.clone();
    for name in ["address_line2", "city", "postal_code", "state"] {
        any.extend(field(name).and_then(|t| text_needle(&t)));
    }
    (distinctive, any)
}

fn found(needle: &Needle, folded_reply: &str, digit_runs: &[String]) -> bool {
    match needle {
        Needle::Words(text) => folded_reply.contains(&format!(" {text} ")),
        Needle::Phone(digits) => digit_runs.iter().any(|run| run.contains(digits.as_str())),
    }
}

/// The entries one listing gave `agent` in its current turn: the run the hub follows, or none (a routine).
pub fn note_listed(state: &crate::app::AppState, agent: &str, tool: &str, entries: &[Entry]) {
    let run = state.hub.turn_of(agent, tool).map(|t| t.run_id);
    note_listed_in(agent, entries, run.as_deref(), py::time_time());
}

pub fn note_listed_in(agent: &str, entries: &[Entry], run_id: Option<&str>, at: f64) {
    let mut all = LISTED.lock().unwrap_or_else(|e| e.into_inner());
    let kept = all.entry(agent.to_string()).or_default();
    kept.retain(|l| at - l.at < KEEP_SECONDS);
    let sole = entries.len() == 1;
    for entry in entries {
        let (distinctive, any) = needles(&entry.secret);
        let mark = mark(&entry.id, &entry.label);
        // listed again in the same turn: kept once, alone if one of the listings had it alone
        if let Some(before) = kept.iter_mut().find(|l| l.mark == mark && l.run_id.as_deref() == run_id) {
            before.distinctive = distinctive;
            before.any = any;
            before.sole |= sole;
            before.at = at;
            continue;
        }
        kept.push(Listed { mark, distinctive, any, sole, run_id: run_id.map(str::to_string), at });
    }
}

/// The entries listed in the run `run_id`.
pub fn listed_in_run(agent: &str, run_id: &str) -> Vec<Listed> {
    let all = LISTED.lock().unwrap_or_else(|e| e.into_inner());
    all.get(agent)
        .map(|l| l.iter().filter(|l| l.run_id.as_deref() == Some(run_id)).cloned().collect())
        .unwrap_or_default()
}

/// The entries listed in a turn no run carries (a routine), between `from` and `to` (Unix seconds).
pub fn listed_in_routine(agent: &str, from: f64, to: f64) -> Vec<Listed> {
    let all = LISTED.lock().unwrap_or_else(|e| e.into_inner());
    all.get(agent)
        .map(|l| l.iter().filter(|l| l.run_id.is_none() && l.at >= from && l.at <= to).cloned().collect())
        .unwrap_or_default()
}

/// The chips of the `listed` entries whose details `reply` uses (module doc), each once.
pub fn used_marks(reply: &str, listed: &[Listed]) -> Vec<String> {
    if listed.is_empty() {
        return Vec::new();
    }
    let folded_reply = folded(reply);
    let digit_runs: Vec<String> =
        DIGIT_RUN.find_iter(reply).map(|m| m.as_str().chars().filter(char::is_ascii_digit).collect()).collect();
    let mut out: Vec<String> = Vec::new();
    for entry in listed {
        let needles = if entry.sole { &entry.any } else { &entry.distinctive };
        if !out.contains(&entry.mark) && needles.iter().any(|n| found(n, &folded_reply, &digit_runs)) {
            out.push(entry.mark.clone());
        }
    }
    out
}

/// An entry as the agents' tools read it.
pub struct Entry {
    pub id: String,
    pub label: String,
    pub hint: Option<String>,
    pub secret: Dict,
}

/// The entries of `records` `agent` may read (`readable_by`).
pub fn entries_for(config: &Config, records: &[Dict], agent: &str) -> Vec<Entry> {
    records
        .iter()
        .filter(|r| readable_by(config, r, agent))
        .map(|r| Entry {
            id: r.get("id").map(py::py_str).unwrap_or_default(),
            label: r.get("label").map(py::py_str).unwrap_or_default(),
            hint: r.get("hint").map(py::py_str),
            secret: match r.get("secret") {
                Some(Value::Object(m)) => m.clone(),
                _ => Dict::new(),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(v: Value) -> Dict {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn fields_are_checked_and_kept() {
        let mut record = Dict::new();
        let mut secret = Dict::new();
        let body = dict(json!({"full_name": " Camille Martin ", "address_line1": "12 rue des Exemples",
            "city": "Paris", "postal_code": "75011", "country": "fr", "phones": ["+33 6 00 00 00 00", "01.02.03.04.05"],
            "emails": ["camille@example.org", "CAMILLE@example.org"], "hint": "pour la société"}));
        parse(&body, &mut record, &mut secret).unwrap();
        assert_eq!(secret["full_name"], "Camille Martin");
        assert_eq!(secret["country"], "FR");
        assert_eq!(secret["phones"], json!(["+33 6 00 00 00 00", "01.02.03.04.05"]));
        assert_eq!(secret["emails"], json!(["camille@example.org"])); // the same address once
        assert_eq!(record["hint"], "pour la société");
        // cleared fields go, the others stay
        parse(&dict(json!({"phones": [], "address_line1": "", "hint": ""})), &mut record, &mut secret).unwrap();
        assert!(!secret.contains_key("phones") && !secret.contains_key("address_line1") && !record.contains_key("hint"));
        assert_eq!(secret["city"], "Paris");
    }

    #[test]
    fn what_is_refused() {
        for (body, why) in [
            (json!({"country": "France"}), "country must be a two-letter country code"),
            (json!({"country": "ZZ"}), "country must be a two-letter country code"),
            (json!({"phones": ["06 00 abc"]}), "not a valid phone number: 06 00 abc"),
            (json!({"phones": ["0+6 00 00"]}), "not a valid phone number: 0+6 00 00"),
            (json!({"emails": ["camille@example"]}), "not a valid e-mail address: camille@example"),
            (json!({"emails": ["a b@example.org"]}), "not a valid e-mail address: a b@example.org"),
            (json!({"postal_code": "75<011"}), "postal code: letters, digits, spaces and dashes only"),
            (json!({"city": "x".repeat(101)}), "the city is too long (at most 100 characters)"),
            (json!({"hint": "x".repeat(301)}), "the hint is too long (at most 300 characters)"),
            (json!({"emails": (0..6).map(|i| format!("a{i}@example.org")).collect::<Vec<_>>()}), "at most 5 of each (emails)"),
            (json!({"full_name": "  "}), "add at least one detail"),
        ] {
            let err = parse(&dict(body.clone()), &mut Dict::new(), &mut Dict::new()).unwrap_err();
            assert_eq!(err.to_string(), why, "{body}");
        }
        assert!(valid_phone("+1 (555) 010-0000") && !valid_phone("12") && !valid_phone(&"1".repeat(21)));
        assert!(valid_email("prenom.nom+pro@sous.example.fr") && !valid_email("@example.org") && !valid_email("a@b@c.d"));
    }

    #[test]
    fn hermes_gets_strings_and_the_app_every_field() {
        let secret = dict(json!({"full_name": "Camille Martin", "city": "Paris",
            "phones": ["+33 6 00 00 00 00", "01 02 03 04 05"], "emails": ["camille@example.org"]}));
        assert_eq!(
            Value::Object(hermes_secret(&secret)),
            json!({"full_name": "Camille Martin", "city": "Paris", "phone": "+33 6 00 00 00 00", "email": "camille@example.org"})
        );
        let shown = public(&secret);
        assert_eq!(shown["address_line1"], "");
        assert_eq!(shown["phones"][1], "01 02 03 04 05");
        // an address item of before: the address fields only
        let old = dict(json!({"address_line1": "1 rue Test", "city": "Lyon", "postal_code": "69001", "country": "FR"}));
        assert_eq!(public(&old)["emails"], json!([]));
        assert!(describe("Maison", None, &old).contains("postal code and city: 69001 Lyon"));
    }

    #[test]
    fn the_mark_carries_the_id_and_label_only() {
        let mark = mark("item_0123456789ab", "Kerfy Studio (pro) --> <!--bip:x");
        assert_eq!(mark, "<!--bip:vault item_0123456789ab Kerfy Studio (pro) — <!—bip:x-->");
        assert_eq!(mark_id(&mark), Some("item_0123456789ab"));
        assert_eq!(mark.matches("-->").count(), 1);
        assert_eq!(mark_id("<!--bip:vault nope-->"), None);
    }

    fn entry(id: &str, label: &str, secret: Value) -> Entry {
        Entry { id: id.into(), label: label.into(), hint: None, secret: dict(secret) }
    }

    fn pro() -> Entry {
        entry("item_000000000001", "Kerfy Studio (pro)", json!({"full_name": "Camille Exemple",
            "address_line1": "8 allée des Essais", "address_line2": "Bâtiment C", "city": "Testville",
            "postal_code": "00100", "country": "FR", "phones": ["+33 6 00 00 00 01"], "emails": ["pro@example.org"]}))
    }

    fn home() -> Entry {
        entry("item_000000000002", "Domicile", json!({"full_name": "Camille Exemple", "address_line1": "3 rue du Test",
            "city": "Lyon", "postal_code": "69001", "country": "FR", "phones": ["04 00 00 00 09"]}))
    }

    #[test]
    fn a_listing_used_in_the_reply_gets_the_chip_of_what_it_used() {
        let both = [pro(), home()];
        note_listed_in("lettres", &both, Some("run_a"), 1_000.0);
        let listed = listed_in_run("lettres", "run_a");
        assert_eq!(listed.len(), 2);
        let pro_mark = mark("item_000000000001", "Kerfy Studio (pro)");
        let home_mark = mark("item_000000000002", "Domicile");
        // the letter with the pro address: its chip only (accents, case, non-breaking spaces ignored)
        let letter = "Kerfy Studio\n8 ALLEE DES ESSAIS, Bât. C\n00100\u{a0}Testville\n\nObjet : résiliation";
        assert_eq!(used_marks(letter, &listed), vec![pro_mark.clone()]);
        // one phone is enough, however it is written
        assert_eq!(used_marks("Appelle le 04.00.00.00.09 demain.", &listed), vec![home_mark.clone()]);
        assert_eq!(used_marks("Tél. +33 (0)6 00 00 00 01", &listed), vec![pro_mark.clone()]);
        // an e-mail, the full name: both entries carry the name
        assert_eq!(used_marks("Écris à PRO@example.org", &listed), vec![pro_mark.clone()]);
        assert_eq!(used_marks("Camille Exemple", &listed), vec![pro_mark.clone(), home_mark.clone()]);
        // with two entries, the city alone or part of a word is not distinctive; nothing used, nothing added
        assert!(used_marks("Rendez-vous à Lyon.", &listed).is_empty());
        assert!(used_marks("J'ai deux adresses pour toi : laquelle ?", &listed).is_empty());
        assert!(used_marks("8 allée des Essaisons", &listed).is_empty());
        // another run, or a routine: not this turn's
        assert!(listed_in_run("lettres", "run_b").is_empty() && listed_in_routine("lettres", 0.0, 2_000.0).is_empty());
    }

    #[test]
    fn a_listing_of_one_entry_counts_any_of_its_details() {
        note_listed_in("seule", &[home()], None, 5_000.0);
        let listed = listed_in_routine("seule", 4_000.0, 6_000.0);
        let home_mark = mark("item_000000000002", "Domicile");
        assert_eq!(used_marks("Le colis part pour Lyon.", &listed), vec![home_mark]);
        assert!(used_marks("Le colis part demain.", &listed).is_empty());
        // the country code alone is no use of the entry
        assert!(used_marks("FR", &listed).is_empty());
        // listed again with another entry in the same turn: still kept once, still alone once
        note_listed_in("seule", &[home(), pro()], None, 5_001.0);
        assert_eq!(listed_in_routine("seule", 4_000.0, 6_000.0).len(), 2);
        assert_eq!(used_marks("à Lyon", &listed_in_routine("seule", 4_000.0, 6_000.0)).len(), 1);
    }

    #[test]
    fn the_tools_are_described_for_tool_search() {
        let tools = tools();
        assert_eq!(tools.iter().map(|t| t["name"].as_str().unwrap()).collect::<Vec<_>>(), [READ, SAVE]);
        assert!(tools[0]["description"].as_str().unwrap().contains("coordonnées"));
        assert_eq!(tools[1]["inputSchema"]["required"], json!(["label"]));
        // the person names an entry: the model is asked for its label
        assert!(tools[0]["description"].as_str().unwrap().contains("call with its label"));
        assert!(tools[0]["inputSchema"]["properties"]["label"]["description"].as_str().unwrap().contains("mon adresse"));
    }
}
