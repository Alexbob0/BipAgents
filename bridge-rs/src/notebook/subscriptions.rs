//! Subscriptions kept once and read right (Alex, 2026-11-03: the same Claude Max 20x listed three times, « monthly »
//! in a French Carnet, « €196.57 » beside « 196,57 € TTC », a renewal of 38 days ago shown as the next one). The
//! records come from three sources that name things their own way (an invoice mail in English, a bank line, the
//! person), so:
//!
//! - **One service, one record**: names compared by their words, without what says nothing (« plan », « abonnement »,
//!   a company's legal form, what is in brackets), a brand and its maker one word (« Anthropic » is « Claude »). Two
//!   records are the same when one's words are all in the other's and they share two words (« Max plan - 20x » and
//!   « Claude Max 20x (Anthropic, PBC) »), or one word and the same price (« Claude » at 196,57 € and « Claude Max
//!   20x » at 196,57 €). Another tier is another service (« Max 5x » is not « Max 20x »).
//! - **One way to write a period and a price**: the period in the Carnet's language (« mensuel », « annuel »…) when it
//!   is one of the usual words in any language; a price as the Carnet's language writes it (« 196,57 € », « €196.57 »
//!   in English), its currency kept, « TTC » dropped, a « /mois » in it made the period. Anything else stays as
//!   written.
//! - **The next renewal**: a renewal date already past is moved on by its period while at most one renewal went by
//!   unnoticed (a yearly one of September is next September); two or more, or no period, and it is only the last
//!   known one (the apps say so), never « next ».
//!
//! `settle_locked` puts the tables written before right at each nightly pass (archive.rs `run`): values rewritten,
//! duplicates merged (the most precise name kept, every field kept), each change in the Carnet's history.

use std::collections::BTreeSet;

use chrono::{Datelike, Months, NaiveDate};
use serde_json::json;

use super::records::{self, Kind, Table};
use super::{Part, Who, fold};
use crate::Dict;
use crate::config::Config;
use crate::tools_knowledge as tk;

/// The periods: id, how many months (0: a week), the usual words for it (folded, any language), its word in fr, en,
/// de, es.
const PERIODS: [(&str, u32, &[&str], [&str; 4]); 4] = [
    (
        "week",
        0,
        &[
            "hebdomadaire",
            "par semaine",
            "semaine",
            "weekly",
            "week",
            "per week",
            "wochentlich",
            "pro woche",
            "semanal",
            "por semana",
        ],
        ["hebdomadaire", "weekly", "wöchentlich", "semanal"],
    ),
    (
        "month",
        1,
        &[
            "mensuel",
            "mensuelle",
            "par mois",
            "mois",
            "tous les mois",
            "chaque mois",
            "monthly",
            "month",
            "per month",
            "a month",
            "every month",
            "monatlich",
            "pro monat",
            "monat",
            "mensual",
            "al mes",
            "por mes",
            "cada mes",
        ],
        ["mensuel", "monthly", "monatlich", "mensual"],
    ),
    (
        "quarter",
        3,
        &[
            "trimestriel",
            "trimestrielle",
            "par trimestre",
            "quarterly",
            "quarter",
            "vierteljahrlich",
            "quartal",
            "trimestral",
        ],
        ["trimestriel", "quarterly", "vierteljährlich", "trimestral"],
    ),
    (
        "year",
        12,
        &[
            "annuel",
            "annuelle",
            "par an",
            "an",
            "annee",
            "par annee",
            "tous les ans",
            "chaque annee",
            "yearly",
            "annual",
            "annually",
            "year",
            "per year",
            "a year",
            "every year",
            "jahrlich",
            "pro jahr",
            "jahr",
            "anual",
            "al ano",
            "por ano",
            "cada ano",
        ],
        ["annuel", "yearly", "jährlich", "anual"],
    ),
];

fn lang_index(lang: &str) -> usize {
    match lang {
        "fr" => 0,
        "de" => 2,
        "es" => 3,
        _ => 1,
    }
}

fn words_of(text: &str) -> String {
    fold(text).split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

/// A period's id and months when its text is one of the usual words (« monthly », « par mois », « /mois »).
pub fn period_of(text: &str) -> Option<(&'static str, u32)> {
    let plain = words_of(text);
    PERIODS.iter().find(|p| p.2.contains(&plain.as_str())).map(|p| (p.0, p.1))
}

/// A period written in the Carnet's language (« monthly » → « mensuel »); another text as it was.
pub fn period_text(text: &str, lang: &str) -> String {
    match period_of(text) {
        Some((id, _)) => {
            PERIODS.iter().find(|p| p.0 == id).map(|p| p.3[lang_index(lang)].to_string()).unwrap_or_default()
        }
        None => text.trim().to_string(),
    }
}

/// The currencies a price may name: what is written (folded) → its sign or code.
const CURRENCIES: [(&[&str], &str); 7] = [
    (&["€", "eur", "euro", "euros"], "€"),
    (&["$", "usd", "us$", "dollar", "dollars"], "$"),
    (&["£", "gbp", "livre", "livres", "pound", "pounds"], "£"),
    (&["chf"], "CHF"),
    (&["cad", "c$", "ca$"], "CAD"),
    (&["ils", "₪", "nis"], "₪"),
    (&["sgd", "s$"], "SGD"),
];

/// What may follow a price and says nothing more (« TTC », « TVA incluse », « VAT included »).
const TAX_WORDS: [&str; 8] = [
    "ttc",
    "tva incluse",
    "tva comprise",
    "vat included",
    "incl vat",
    "inkl mwst",
    "iva incluido",
    "toutes taxes comprises",
];

/// A price written as the Carnet's language writes it, with the period it names (« 196,57 € TTC » → (« 196,57 € »,
/// None); « 12.99€/month » → (« 12,99 € », Some(month))). None: not a plain price (« gratuit puis 9,99 € »), kept as
/// written.
pub fn price_text(text: &str, lang: &str) -> Option<(String, Option<&'static str>)> {
    static PRICE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(concat!(
            r"(?i)^\s*(?P<pre>[€$£₪]|us\$|c\$|s\$|eur|usd|gbp|chf)?\s*",
            r"(?P<num>\d{1,3}(?:[ \u{a0}\u{202f}.,]\d{3})*(?:[.,]\d{1,2})?|\d+(?:[.,]\d{1,2})?)\s*",
            r"(?P<post>[€$£₪]|eur|euros?|usd|dollars?|gbp|chf|cad|sgd|ils)?\s*(?P<rest>.*)$",
        ))
        .expect("valid regex")
    });
    let c = PRICE.captures(text.trim())?;
    let pre = c.name("pre").map(|m| m.as_str().to_lowercase()).unwrap_or_default();
    let post = c.name("post").map(|m| m.as_str().to_lowercase()).unwrap_or_default();
    let named = if pre.is_empty() { post } else { pre };
    let currency = CURRENCIES.iter().find(|(names, _)| names.contains(&named.as_str())).map(|(_, sign)| *sign)?;
    // what follows: taxes said, a period (« /mois », « par mois »), else not a plain price
    let rest = words_of(c.name("rest").map(|m| m.as_str()).unwrap_or(""));
    let mut period = None;
    if !rest.is_empty() && !TAX_WORDS.contains(&rest.as_str()) {
        let without_tax = TAX_WORDS.iter().fold(rest.clone(), |r, w| r.replace(w, " "));
        let without_tax = without_tax.split_whitespace().collect::<Vec<_>>().join(" ");
        match period_of(&without_tax) {
            Some((id, _)) => period = Some(id),
            None if without_tax.is_empty() => {}
            None => return None,
        }
    }
    let amount = records::price_value(c.name("num")?.as_str())?;
    let cents = (amount * 100.0).round() as i64;
    let whole = cents / 100;
    let decimals = cents % 100 != 0;
    let group = |n: i64, sep: &str| {
        let digits = n.to_string();
        let mut out = String::new();
        for (i, ch) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                out.push_str(sep);
            }
            out.push(ch);
        }
        out
    };
    let english = lang_index(lang) == 1;
    let number = match (english, decimals) {
        (true, true) => format!("{}.{:02}", group(whole, ","), cents % 100),
        (true, false) => group(whole, ","),
        (false, true) => format!("{},{:02}", group(whole, " "), cents % 100),
        (false, false) => group(whole, " "),
    };
    let shown = if english && currency.chars().count() == 1 {
        format!("{currency}{number}")
    } else {
        format!("{number} {currency}")
    };
    Some((shown, period))
}

/// What a name's words say nothing about (folded).
const NOISE: [&str; 34] = [
    "plan",
    "abonnement",
    "abonnements",
    "subscription",
    "subscriptions",
    "abo",
    "forfait",
    "offre",
    "formule",
    "membership",
    "prlv",
    "prelevement",
    "sepa",
    "cb",
    "carte",
    "paiement",
    "payment",
    "inc",
    "pbc",
    "sas",
    "sarl",
    "ltd",
    "llc",
    "gmbh",
    "plc",
    "corp",
    "com",
    "www",
    "ai",
    "app",
    "le",
    "la",
    "the",
    "de",
];

/// A brand's other names (folded): its maker, its domain.
const SAME_BRAND: [(&str, &str); 4] =
    [("anthropic", "claude"), ("openai", "chatgpt"), ("spotifyab", "spotify"), ("netflixcom", "netflix")];

/// A service name's words that tell it apart (« Claude Max 20x (Anthropic, PBC) » → {claude, max, 20x}).
pub fn service_words(name: &str) -> BTreeSet<String> {
    let mut text = String::new();
    let mut depth = 0;
    for c in name.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => text.push(c),
            _ => {}
        }
    }
    words_of(&text)
        .split(' ')
        .filter(|w| !w.is_empty() && !NOISE.contains(w))
        .map(|w| SAME_BRAND.iter().find(|(from, _)| *from == w).map(|(_, to)| to.to_string()).unwrap_or(w.to_string()))
        .collect()
}

/// Whether two subscriptions are one service (see the module's text). `prices`: both records' prices.
pub fn same_service(a: &str, b: &str, prices: (&str, &str)) -> bool {
    let (x, y) = (service_words(a), service_words(b));
    if x.is_empty() || y.is_empty() {
        return fold(a).trim() == fold(b).trim();
    }
    if x == y {
        return true;
    }
    let (small, big) = if x.len() <= y.len() { (&x, &y) } else { (&y, &x) };
    if !small.is_subset(big) {
        return false;
    }
    let same_price = match (records::price_value(prices.0), records::price_value(prices.1)) {
        (Some(p), Some(q)) => (p - q).abs() < 0.005,
        _ => false,
    };
    small.len() >= 2 || same_price
}

/// A subscription's renewal as the apps show it: the next one (moved on by its period when one renewal went by
/// unnoticed), else the last known one.
#[derive(Debug, Clone, PartialEq)]
pub enum Renewal {
    Next(NaiveDate),
    Last(NaiveDate),
}

pub fn renewal(record: &Dict, today: NaiveDate) -> Option<Renewal> {
    let text = tk::str_or_empty(record.get("renewal"));
    let Some((m, d, Some(y))) = super::date_in(&text) else { return monthly_day(record, &text, today) };
    let date = NaiveDate::from_ymd_opt(y, m, d)?;
    if date >= today {
        return Some(Renewal::Next(date));
    }
    let Some((_, months)) = period_of(&tk::str_or_empty(record.get("period"))) else {
        return Some(Renewal::Last(date));
    };
    let next = if months == 0 {
        date.checked_add_days(chrono::Days::new(7))
    } else {
        date.checked_add_months(Months::new(months))
    };
    match next {
        Some(n) if n >= today => Some(Renewal::Next(n)),
        _ => Some(Renewal::Last(date)),
    }
}

/// A monthly renewal given as its day (« le 5 de chaque mois », « prélevé le 20 »): its next one.
fn monthly_day(record: &Dict, text: &str, today: NaiveDate) -> Option<Renewal> {
    static DAY: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(?:le|the|on|am|el|day)\s+(\d{1,2})(?:er|st|nd|rd|th|\.)?\b").expect("valid regex")
    });
    let monthly = period_of(&tk::str_or_empty(record.get("period"))).is_some_and(|p| p.0 == "month")
        || ["chaque mois", "tous les mois", "each month", "every month", "du mois", "jeden monat", "cada mes"]
            .iter()
            .any(|w| fold(text).contains(w));
    let day: u32 = DAY.captures(text)?[1].parse().ok()?;
    if !monthly || !(1..=31).contains(&day) {
        return None;
    }
    let on = |month: NaiveDate| {
        (0..4).find_map(|less| NaiveDate::from_ymd_opt(month.year(), month.month(), day.saturating_sub(less).max(1)))
    };
    let first = NaiveDate::from_ymd_opt(today.year(), today.month(), 1)?;
    let this = on(first)?;
    Some(Renewal::Next(if this >= today { this } else { on(first.checked_add_months(Months::new(1))?)? }))
}

/// « Dernier renouvellement connu » (the apps' line for a past renewal), in the Carnet's language.
pub fn last_title(lang: &str) -> &'static str {
    ["Dernier renouvellement connu", "Last known renewal", "Letzte bekannte Verlängerung", "Última renovación conocida"]
        [lang_index(lang)]
}

/// A record's period and price written the Carnet's way (any type with a price; the period: subscriptions).
pub fn settle_values(t: &Kind, record: &mut Dict, lang: &str) {
    let price = tk::str_or_empty(record.get("price"));
    if let Some((shown, period)) = price_text(&price, lang) {
        record.insert("price".into(), json!(shown));
        if t.id == "subscription" && tk::str_or_empty(record.get("period")).is_empty() {
            if let Some(id) = period {
                record.insert("period".into(), json!(id));
            }
        }
    }
    if t.id == "subscription" {
        let period = tk::str_or_empty(record.get("period"));
        if !period.is_empty() {
            record.insert("period".into(), json!(period_text(&period, lang)));
        }
    }
}

/// Two records of the same service made one: the most precise name (more words), every field the first lacks
/// taken from the second, the later renewal.
pub fn merge(a: &Dict, b: &Dict) -> Dict {
    let mut out = a.clone();
    for (k, v) in b {
        if k == "id" {
            continue;
        }
        let kept = tk::str_or_empty(out.get(k));
        let other = tk::str_or_empty(Some(v));
        let date = |t: &str| super::date_in(t).and_then(|(m, d, y)| NaiveDate::from_ymd_opt(y?, m, d));
        let take = kept.trim().is_empty()
            || (k == "service" && service_words(&other).len() > service_words(&kept).len())
            || (k == "renewal" && !other.is_empty() && date(&other) > date(&kept));
        if take {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

/// The tables written before put right (see the module's text): the live and the archived ones. Returns how many
/// rows changed.
pub fn settle_locked(config: &Config, who: &Who, _today: NaiveDate) -> anyhow::Result<usize> {
    let lang = super::language(config);
    let mut changed = 0;
    for t in records::TYPES.iter().filter(|t| t.fields.iter().any(|f| f.id == "price" || f.id == "period")) {
        for archived in [false, true] {
            let (path, before, mut page, table) = records::read_at(config, t, archived);
            if before.is_none() || table.rows.is_empty() {
                continue;
            }
            let mut rows: Vec<Dict> = Vec::new();
            let mut n = 0;
            for row in &table.rows {
                let mut row = row.clone();
                let was = row.clone();
                settle_values(t, &mut row, &lang);
                if row != was {
                    n += 1;
                }
                let same = (t.id == "subscription")
                    .then(|| {
                        rows.iter().position(|r| {
                            same_service(
                                &tk::str_or_empty(r.get("service")),
                                &tk::str_or_empty(row.get("service")),
                                (&tk::str_or_empty(r.get("price")), &tk::str_or_empty(row.get("price"))),
                            )
                        })
                    })
                    .flatten();
                match same {
                    Some(i) => {
                        rows[i] = merge(&rows[i], &row);
                        n += 1;
                    }
                    None => rows.push(row),
                }
            }
            if n == 0 {
                continue;
            }
            page.body = records::render_body(t, &lang, &Table { before: table.before, rows, after: table.after });
            page.touch();
            super::commit(
                config,
                Part::Main,
                &path,
                before.as_deref(),
                Some(&page.render()),
                who,
                "tidy",
                t.titles[lang_index(&lang)],
            )?;
            changed += n;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(pairs: &[(&str, &str)]) -> Dict {
        pairs.iter().map(|(k, v)| (k.to_string(), json!(v))).collect()
    }

    #[test]
    fn one_service_under_three_names() {
        let p = ("196,57 €", "€196.57");
        assert!(same_service("Max plan - 20x", "Claude Max 20x (Anthropic, PBC)", p));
        assert!(same_service("Claude Max 20x (Ultra)", "Claude Max 20x (Anthropic, PBC)", ("", "196,57 €")));
        assert!(same_service("CLAUDE.AI SUBSCRIPTION ANTHROPIC", "Claude Max 20x", ("196,57 EUR", "196,57 €")));
        assert!(!same_service("Anthropic Max plan - 5x", "Claude Max 20x", ("98 €", "196,57 €")), "another tier");
        assert!(!same_service("Spotify", "Spotify Family", ("10,99 €", "17,99 €")), "one word and another price");
        assert!(same_service("Spotify", "Spotify Premium", ("12,99 €", "12,99 €")));
        assert!(!same_service("Apple Music", "Apple TV+", ("10,99 €", "9,99 €")));
        assert!(!same_service("Netflix", "Disney+", ("", "")));
    }

    #[test]
    fn periods_and_prices_written_one_way() {
        assert_eq!(period_text("monthly", "fr"), "mensuel");
        assert_eq!(period_text("Yearly", "fr"), "annuel");
        assert_eq!(period_text("par mois", "en"), "monthly");
        assert_eq!(period_text("mensuel (engagement 12 mois)", "fr"), "mensuel (engagement 12 mois)");
        assert_eq!(price_text("€196.57", "fr"), Some(("196,57 €".into(), None)));
        assert_eq!(price_text("196,57 € TTC", "fr"), Some(("196,57 €".into(), None)));
        assert_eq!(price_text("196,57 EUR", "fr"), Some(("196,57 €".into(), None)));
        assert_eq!(price_text("12.99€/month", "fr"), Some(("12,99 €".into(), Some("month"))));
        assert_eq!(price_text("99 € par an", "fr"), Some(("99 €".into(), Some("year"))));
        assert_eq!(price_text("2,99 $", "fr"), Some(("2,99 $".into(), None)));
        assert_eq!(price_text("USD 2.99", "fr"), Some(("2,99 $".into(), None)));
        assert_eq!(price_text("1 299,00 €", "fr"), Some(("1 299 €".into(), None)));
        assert_eq!(price_text("196,57 €", "en"), Some(("€196.57".into(), None)));
        assert_eq!(price_text("8 CHF", "de"), Some(("8 CHF".into(), None)));
        assert_eq!(price_text("gratuit puis 9,99 €", "fr"), None);
        assert_eq!(price_text("9,99", "fr"), None, "no currency: as written");
        let mut r = rec(&[("service", "Max plan - 20x"), ("price", "€196.57"), ("period", "monthly")]);
        settle_values(records::kind("subscription").unwrap(), &mut r, "fr");
        assert_eq!(
            (tk::str_or_empty(r.get("price")), tk::str_or_empty(r.get("period"))),
            ("196,57 €".into(), "mensuel".into())
        );
    }

    #[test]
    fn a_past_renewal_is_moved_on_once_at_most() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        let day = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        // Le Monde, yearly, renewed in September: next September
        let r = rec(&[("renewal", "2026-09-15"), ("period", "annuel")]);
        assert_eq!(renewal(&r, today), Some(Renewal::Next(day("2027-09-15"))));
        // Max 5x, monthly, 26 September: two renewals unheard of → only the last known one
        let r = rec(&[("renewal", "2026-09-26"), ("period", "monthly")]);
        assert_eq!(renewal(&r, today), Some(Renewal::Last(day("2026-09-26"))));
        // monthly, 20 October: next on 20 November
        let r = rec(&[("renewal", "2026-10-20"), ("period", "mensuel")]);
        assert_eq!(renewal(&r, today), Some(Renewal::Next(day("2026-11-20"))));
        // no period: the last known one
        let r = rec(&[("renewal", "2026-10-30")]);
        assert_eq!(renewal(&r, today), Some(Renewal::Last(day("2026-10-30"))));
        let r = rec(&[("renewal", "2026-12-01"), ("period", "monthly")]);
        assert_eq!(renewal(&r, today), Some(Renewal::Next(day("2026-12-01"))));
        // a day of the month (the lab's GLM, 2026-11-03: « prélèvement mensuel le 5 »)
        let r = rec(&[("renewal", "prélèvement mensuel le 5"), ("period", "mensuel")]);
        assert_eq!(renewal(&r, today), Some(Renewal::Next(day("2026-11-05"))));
        let r = rec(&[("renewal", "le 2 de chaque mois")]);
        assert_eq!(renewal(&r, today), Some(Renewal::Next(day("2026-12-02"))));
        assert_eq!(renewal(&rec(&[("renewal", "le 5")]), today), None, "no period, no « each month »");
    }

    #[test]
    fn merged_keeps_the_precise_name_and_every_field() {
        let a = rec(&[("id", "r1"), ("service", "Max plan - 20x"), ("price", "196,57 €"), ("period", "mensuel")]);
        let b = rec(&[
            ("id", "r2"),
            ("service", "Claude Max 20x (Anthropic, PBC)"),
            ("renewal", "2026-12-01"),
            ("source", "archive:x"),
        ]);
        let m = merge(&a, &b);
        assert_eq!(tk::str_or_empty(m.get("id")), "r1");
        assert_eq!(tk::str_or_empty(m.get("service")), "Claude Max 20x (Anthropic, PBC)");
        assert_eq!(tk::str_or_empty(m.get("renewal")), "2026-12-01");
        assert_eq!(tk::str_or_empty(m.get("source")), "archive:x");
    }
}
