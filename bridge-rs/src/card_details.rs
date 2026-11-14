//! A payment card's details told apart from everything else (card-guards, 14/11): its number in a text, and the words
//! of a card's fields (number, expiry, security code, holder) in English, French, German and Spanish.
//!
//! Why: on 14/11 Alex's agent (Vie, his own test card, pharmaciedesdrakkars.com) could not get the vault's card fill to
//! work on the shop's payment page; it then asked him for the card's number through a secure-field card, typed it into
//! the form itself, and asked for the holder, the expiry and the security code the same way. Alex's rule: through a
//! card the person gives only the security code, inside the payment's own flow (« Oui, payer »); never the number, the
//! expiry or the holder; and the agent never types any of them. Payments go only through `pay_with_card`; when it
//! can't fill the form, the agent stops and hands the payment over (« Panier prêt »): the person pays themselves.
//!
//! Used by the browser proxy's guard (browser_mcp.rs: what the agent types, pastes, selects or scripts), by the
//! secure-field card (`ask_secret`, connectors.rs and secrets.rs: what the agent may ask for, what the person gives),
//! and to hide a card's number from what the agent reads.

use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

/// A card's number among digits: 13 to 19 of them, the Luhn check passed, and a card network's first digits for that
/// length (Visa, Mastercard, CB on both, American Express, Discover, UnionPay, JCB, Diners, Mir, Maestro, UATP). The
/// network check keeps a French SIRET (14 digits, Luhn by design) or a phone's IMEI (15, Luhn too) from counting.
pub fn is_card_number(digits: &str) -> bool {
    let len = digits.len();
    if !(13..=19).contains(&len) || !digits.bytes().all(|b| b.is_ascii_digit()) || !crate::cards::luhn(digits) {
        return false;
    }
    let first = |n: usize| digits[..n].parse::<u32>().unwrap_or(0);
    let long = (16..=19).contains(&len);
    (digits.starts_with('4') && matches!(len, 13 | 16 | 19))
        || (((51..=55).contains(&first(2)) || (2221..=2720).contains(&first(4))) && len == 16)
        || (matches!(first(2), 34 | 37) && len == 15)
        || ((digits.starts_with("6011") || digits.starts_with("65") || (644..=649).contains(&first(3))) && long)
        || (digits.starts_with("62") && long)
        || ((3528..=3589).contains(&first(4)) && long)
        || ((matches!(first(2), 36 | 38 | 39) || (300..=305).contains(&first(3)) || first(4) == 3095)
            && (14..=19).contains(&len))
        || ((2200..=2204).contains(&first(4)) && long)
        || ((first(2) == 50 || (56..=69).contains(&first(2))) && long)
        || (digits.starts_with('1') && len == 15)
}

/// Runs of digits split by single spaces, dashes or dots (« 4242 4242 4242 4242 », « 4242-4242-… »), each with its
/// groups' places in the text.
static RUNS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d+(?:[ \-.\u{a0}\u{2009}\u{202f}]\d+)*").expect("valid regex"));
static GROUPS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").expect("valid regex"));

/// Where `text` holds a card's number (byte ranges): any run of whole groups of digits that makes one
/// (`is_card_number`), so « 4242 4242 4242 4242 12/30 » and « carte 4242424242424242 » count, and a longer number
/// written without spaces does not.
pub fn card_numbers(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for run in RUNS.find_iter(text) {
        let groups: Vec<(usize, usize)> =
            GROUPS.find_iter(run.as_str()).map(|g| (run.start() + g.start(), run.start() + g.end())).collect();
        let mut from = 0;
        while from < groups.len() {
            let mut found = None;
            let mut digits = String::new();
            for (to, (start, end)) in groups.iter().enumerate().skip(from) {
                digits.push_str(&text[*start..*end]);
                if digits.len() > 19 {
                    break;
                }
                if is_card_number(&digits) {
                    found = Some(to);
                }
            }
            match found {
                Some(to) => {
                    out.push((groups[from].0, groups[to].1));
                    from = to + 1;
                }
                None => from += 1,
            }
        }
    }
    out
}

/// Whether `text` holds a card's number (`card_numbers`).
pub fn holds_card_number(text: &str) -> bool {
    !card_numbers(text).is_empty()
}

/// What a card's number becomes in what the agent reads.
pub const HIDDEN: &str = "[card number hidden]";

/// `text` with every card number in it replaced by `HIDDEN`.
pub fn hide_card_numbers(text: &str) -> String {
    let spans = card_numbers(text);
    if spans.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (start, end) in spans {
        out.push_str(&text[at..start]);
        out.push_str(HIDDEN);
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

/// Lower case, accents off, `_`, `-` and `.` as spaces: « Numéro_de-la carte » → « numero de la carte ».
pub fn folded(text: &str) -> String {
    text.nfd()
        .filter(|c| !matches!(*c, '\u{300}'..='\u{36f}'))
        .map(|c| if matches!(c, '_' | '-' | '.' | '\'' | '\u{2019}') { ' ' } else { c })
        .collect::<String>()
        .to_lowercase()
}

/// A card that is not a payment card: loyalty, gift, health, identity, transport, a SIM… (« numéro de carte de
/// fidélité », « Kartennummer der Krankenkasse »), or a passport's expiry date.
const NOT_PAYMENT_SRC: &str = r"fidelit|loyalt|treue|fidelidad|bonus ?card|club|cadeau|gift|geschenk|regalo|vitale|carte grise|identit|ausweis|identidad|\bdni\b|\bnie\b|passe?port|reisepass|pasaporte|permis|licen[cs]|fuhrerschein|membre|member|mitglied|socio|adherent|abonne|abonnement|subscri|etudiant|student|estudiante|biblioth|library|navigo|transport|\bsim\b|mutuelle|krankenkasse|versicherung|seguro|assurance|insurance|electeur|siret|siren|imei|\biban\b|\bbic\b|\btva\b|\bvat\b";
static NOT_PAYMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(NOT_PAYMENT_SRC).expect("valid regex"));

/// A field for a number that is no payment card's and may look like one (a SIRET or an IMEI pass Luhn by design, a
/// loyalty card's or a parcel's number may): a card's number is typed there (browser_mcp.rs).
const OTHER_NUMBER_SRC: &str = r"siret|siren|imei|\biban\b|\bbic\b|\btva\b|\bvat\b|ust ?id|\bnif\b|fidelit|loyalt|treue|fidelidad|adherent|member|mitglied|socio|suivi|tracking|sendungsnummer|seguimiento|colis|parcel";
static OTHER_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(OTHER_NUMBER_SRC).expect("valid regex"));

/// A field's words (`card_field`'s) say it holds a number that is no payment card's (`OTHER_NUMBER_SRC`).
pub fn other_number(text: &str) -> bool {
    OTHER_NUMBER.is_match(&folded(text))
}

/// Words that name a payment card itself (« carte bancaire », « Kreditkarte », a network's name with « card »).
const PAYMENT_CARD_SRC: &str = r"carte (bancaire|de credit|de debit|de paiement|bleue|cb)|\bcb\b|credit ?card|debit ?card|bank ?card|payment ?card|kreditkarte|bankkarte|debitkarte|\bec ?karte|girocard|tarjeta (de credito|de debito|bancaria)|(visa|mastercard|master card|american express|\bamex|maestro) ?(card|carte|karte|tarjeta)|(card|carte|karte|tarjeta) (visa|mastercard|american express|amex|maestro)";
static PAYMENT_CARD: LazyLock<Regex> = LazyLock::new(|| Regex::new(PAYMENT_CARD_SRC).expect("valid regex"));

/// The words of each part of a card's form, on folded text (`folded`). The security code and the expiry before the
/// number (« card security code »). `strong`: enough alone; the others only beside a card (`card_context`).
const CSC_STRONG_SRC: &str = r"\b(cvc2?|cvv2?|csc|cvn|cav2)\b|cc ?csc|card ?verification|cryptogramme|kartenprufnummer|codigo de seguridad de la tarjeta|card ?security ?code|code de securite de (la|votre|ta) carte|3 chiffres au dos|chiffres au dos|digits on the back|ziffern auf der ruckseite|digitos (al|del) dorso";
static CSC_STRONG: LazyLock<Regex> = LazyLock::new(|| Regex::new(CSC_STRONG_SRC).expect("valid regex"));
const CSC_WEAK_SRC: &str = r"security ?code|code de securite|sicherheitscode|prufnummer|prufziffer|codigo de seguridad|\bcrypto\b";
static CSC_WEAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(CSC_WEAK_SRC).expect("valid regex"));
const EXP_STRONG_SRC: &str = r"cc ?exp|card ?expir|expir\w* (date|month|year)|exp ?date|exp (month|year|mm|yy)|date d ?expiration|date de (fin de )?validite|valid ?(thru|through)|valable jusqu|gultig bis|ablaufdatum|ablauf|fecha de (caducidad|vencimiento|expiracion)|caducidad|vencimiento|\bmm ?/ ?(yy|aa|jj|yyyy|aaaa)\b";
static EXP_STRONG: LazyLock<Regex> = LazyLock::new(|| Regex::new(EXP_STRONG_SRC).expect("valid regex"));
const EXP_WEAK_SRC: &str = r"expir|\bexp\b|validit|gultig|\b(month|mois|monat|mes|year|annee|jahr|ano)\b";
static EXP_WEAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(EXP_WEAK_SRC).expect("valid regex"));
const NUMBER_STRONG_SRC: &str = r"cc ?number|cc ?num\b|ccnum|card ?num|cardnum|card ?no\b|kartennummer|numero (de )?(la |votre |ta )?carte|\bnum (de )?carte|\bn ?° ?(de )?carte|numero (de )?(la |su )?tarjeta|\bn ?° ?(de )?tarjeta|\bpan\b";
static NUMBER_STRONG: LazyLock<Regex> = LazyLock::new(|| Regex::new(NUMBER_STRONG_SRC).expect("valid regex"));
const HOLDER_STRONG_SRC: &str = r"cc ?name|cc ?(given|family) ?name|card ?holder|cardholder|name on (the |your )?card|nameoncard|titulaire de (la|votre|ta) carte|nom (du |de )?(titulaire|porteur)|nom sur la carte|porteur de (la )?carte|karteninhaber|name (des |auf der )?karte|titular de la tarjeta|nombre (del titular|en la tarjeta)";
static HOLDER_STRONG: LazyLock<Regex> = LazyLock::new(|| Regex::new(HOLDER_STRONG_SRC).expect("valid regex"));
const HOLDER_WEAK_SRC: &str = r"titulaire|porteur|holder|inhaber|titular";
static HOLDER_WEAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(HOLDER_WEAK_SRC).expect("valid regex"));
/// Something says the field is a card's: its own words, or the form around it (`card_context`).
const CARD_WORD_SRC: &str = r"\bcard|\bcarte|karte|tarjeta|\bcc\b|\bcb\b";
static CARD_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(CARD_WORD_SRC).expect("valid regex"));

/// The part of a card's form these words name (`number`, `exp`, `csc`, `name`), or None. `text`: what names a field
/// or a request (its autocomplete token, name, id, label, placeholder…); `card_context`: the form around it is a
/// card's (another of its fields is a card number), so « Code de sécurité » or « Titulaire » alone count. A card that
/// is not a payment card (loyalty, gift, identity…) never counts, unless the words name a payment card too.
pub fn card_field(text: &str, card_context: bool) -> Option<&'static str> {
    let t = folded(text);
    if t.trim().is_empty() {
        return None;
    }
    let payment = PAYMENT_CARD.is_match(&t);
    if NOT_PAYMENT.is_match(&t) && !payment {
        return None;
    }
    let card = card_context || payment || CARD_WORD.is_match(&t);
    if CSC_STRONG.is_match(&t) || (card && CSC_WEAK.is_match(&t) && !t.contains("postal") && !t.contains("promo")) {
        return Some("csc");
    }
    if EXP_STRONG.is_match(&t) || (card && EXP_WEAK.is_match(&t)) {
        return Some("exp");
    }
    if HOLDER_STRONG.is_match(&t) || (card && HOLDER_WEAK.is_match(&t)) {
        return Some("name");
    }
    if NUMBER_STRONG.is_match(&t) || payment {
        return Some("number");
    }
    None
}

/// What the agent reads when a secure-field card asks for a card's details (`asks_for_card`: `ask_secret`).
pub const SECRET_REFUSED: &str = "Refused: a card in the app never asks your person for a payment card's number, \
expiry, holder or security code, and you never type them yourself. Payments go only through pay_with_card: your \
person confirms in the app with « Oui, payer » (the security code is given there, and nowhere else). If pay_with_card \
can't fill the payment form, stop and hand the payment over with hand_over_payment (« Panier prêt »): your person pays \
themselves, on their phone or by taking over your browser in the app. Nothing was asked.";

/// What the person's app is told when the value or identifier given in a secure-field card is a card's number.
pub const GIVEN_REFUSED: &str = "a payment card's number is never kept here: payments go only through « Oui, payer »";

/// The same rules for the page's scripts (browser_mcp.rs: the guard's look at a field), with the same words:
/// `cardKind(text, ctx)` as `card_field`, `fold(text)` as `folded`, and `holdsCard(text)` as `holds_card_number`.
pub fn js_rules() -> String {
    let re = |src: &str| format!("new RegExp({})", serde_json::to_string(src).unwrap_or_else(|_| "\"$^\"".into()));
    format!(
        "const CD = {{np: {np}, pc: {pc}, cs: {cs}, cw: {cw}, es: {es}, ew: {ew}, ns: {ns}, hs: {hs}, hw: {hw}, card: {card}, other: {other}}}; \
const fold = (t) => String(t || '').normalize('NFD').replace(/[\\u0300-\\u036f]/g, '').replace(/[_\\-.'\\u2019]/g, ' ').toLowerCase(); \
const cardKind = (text, ctx) => {{ const t = fold(text); if (!t.trim()) return ''; const pay = CD.pc.test(t); \
if (CD.np.test(t) && !pay) return ''; const card = ctx || pay || CD.card.test(t); \
if (CD.cs.test(t) || (card && CD.cw.test(t) && !t.includes('postal') && !t.includes('promo'))) return 'csc'; \
if (CD.es.test(t) || (card && CD.ew.test(t))) return 'exp'; if (CD.hs.test(t) || (card && CD.hw.test(t))) return 'name'; \
if (CD.ns.test(t) || pay) return 'number'; return ''; }}; \
const luhn = (d) => {{ let s = 0; for (let i = 0; i < d.length; i++) {{ let x = +d[d.length - 1 - i]; if (i % 2) {{ x *= 2; if (x > 9) x -= 9; }} s += x; }} return s % 10 === 0; }}; \
const network = (d) => {{ const L = d.length, f = (n) => +d.slice(0, n), long = L >= 16 && L <= 19; \
return (d[0] === '4' && (L === 13 || L === 16 || L === 19)) || (((f(2) >= 51 && f(2) <= 55) || (f(4) >= 2221 && f(4) <= 2720)) && L === 16) \
|| ((f(2) === 34 || f(2) === 37) && L === 15) || ((d.startsWith('6011') || d.startsWith('65') || (f(3) >= 644 && f(3) <= 649)) && long) \
|| (d.startsWith('62') && long) || (f(4) >= 3528 && f(4) <= 3589 && long) \
|| ((f(2) === 36 || f(2) === 38 || f(2) === 39 || (f(3) >= 300 && f(3) <= 305) || f(4) === 3095) && L >= 14 && L <= 19) \
|| (f(4) >= 2200 && f(4) <= 2204 && long) || ((f(2) === 50 || (f(2) >= 56 && f(2) <= 69)) && long) || (d[0] === '1' && L === 15); }}; \
const holdsCard = (v) => {{ for (const r of (String(v || '').match(/\\d+(?:[ \\-.\\u00a0\\u2009\\u202f]\\d+)*/g) || [])) {{ const g = r.match(/\\d+/g); \
for (let a = 0; a < g.length; a++) {{ let d = ''; for (let b = a; b < g.length; b++) {{ d += g[b]; if (d.length > 19) break; \
if (d.length >= 13 && luhn(d) && network(d)) return true; }} }} }} return false; }}; ",
        np = re(NOT_PAYMENT_SRC),
        pc = re(PAYMENT_CARD_SRC),
        cs = re(CSC_STRONG_SRC),
        cw = re(CSC_WEAK_SRC),
        es = re(EXP_STRONG_SRC),
        ew = re(EXP_WEAK_SRC),
        ns = re(NUMBER_STRONG_SRC),
        hs = re(HOLDER_STRONG_SRC),
        hw = re(HOLDER_WEAK_SRC),
        card = re(CARD_WORD_SRC),
        other = re(OTHER_NUMBER_SRC),
    )
}

/// The secure-field card's kinds an agent might give for a card's details (`ask_secret`'s `kind`): not a password,
/// a token or a code.
static CARD_KINDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"card|carte|karte|tarjeta|\bcc\b|\bcb\b|\bpan\b|cvc|cvv|csc|crypto|expir|\bexp\b|holder|titulaire|payment|paiement|credit|debit|visa|mastercard|amex")
        .expect("valid regex")
});

/// A secure-field card asked for a payment card's details (`ask_secret`): its kind (« card », « cvc »…), or its name,
/// identifier label, identifier or reason naming a card's number, expiry, security code or holder, or a payment card
/// (« carte bancaire », « credit card »…), or holding a card's number. Never asked: the person gives only the security
/// code, and only in the payment's own flow (« Oui, payer »).
pub fn asks_for_card(kind: &str, texts: &[&str]) -> bool {
    let kind_folded = folded(kind);
    if !matches!(kind_folded.trim(), "password" | "token" | "code" | "") && CARD_KINDS.is_match(&kind_folded) {
        return true;
    }
    texts.iter().any(|t| holds_card_number(t) || card_field(t, false).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_number_is_luhn_valid_13_to_19_digits_of_a_network() {
        for good in [
            "4242424242424242",    // Visa (Stripe's test card)
            "4330264936344675",    // Visa (CAWL's test card)
            "4150557357382737",    // CB on Visa (CAWL)
            "5555555555554444",    // Mastercard
            "2223003122003222",    // Mastercard 2-series
            "378282246310005",     // American Express
            "6011111111111117",    // Discover
            "3530111333300000",    // JCB
            "36227206271667",      // Diners, 14 digits
            "6200000000000005",    // UnionPay
            "4000056655665556",    // Visa debit
            "4222222222222",       // Visa, 13 digits
        ] {
            assert!(is_card_number(good), "{good}");
        }
        for bad in [
            "4242424242424241",    // Luhn fails
            "424242424242",        // 12 digits
            "42424242424242424242", // 20 digits
            "73282932000074",      // a SIRET (Luhn-valid, no network for 14 digits starting with 7)
            "352099001761481",     // an IMEI (Luhn-valid, 15 digits starting with 35)
            "0000000000000000",
        ] {
            assert!(!is_card_number(bad), "{bad}");
        }
    }

    #[test]
    fn a_card_number_is_found_however_it_is_written() {
        for text in [
            "4242424242424242",
            "4242 4242 4242 4242",
            "4242-4242-4242-4242",
            "carte : 4242 4242 4242 4242, exp 12/30, cvc 123",
            "4242 4242 4242 4242 12/30",
            "{\"text\": \"5555 5555 5555 4444\"}",
            "3782 822463 10005",
            "4242\u{a0}4242\u{a0}4242\u{a0}4242",
        ] {
            assert!(holds_card_number(text), "{text}");
        }
        for text in [
            "Commande n° 4242424242424241",
            "SIRET 732 829 320 00074",
            "Appelle le 06 12 34 56 78",
            "12/30",
            "123",
            "IMEI 35 209900 176148 1",
            "424242424242424242424242", // a longer number written without spaces
        ] {
            assert!(!holds_card_number(text), "{text}");
        }
        assert_eq!(
            hide_card_numbers("ta carte 4242 4242 4242 4242 expire le 12/30"),
            format!("ta carte {HIDDEN} expire le 12/30")
        );
        assert_eq!(hide_card_numbers("rien ici 12/30"), "rien ici 12/30");
    }

    #[test]
    fn a_card_field_is_named_in_four_languages() {
        for (words, kind) in [
            ("cc-number", "number"),
            ("cardnumber", "number"),
            ("Numéro de carte", "number"),
            ("Numéro de la carte bancaire", "number"),
            ("Kartennummer", "number"),
            ("Número de tarjeta", "number"),
            ("Card number", "number"),
            ("Carte bancaire", "number"),
            ("cc-exp", "exp"),
            ("Date d'expiration", "exp"),
            ("Date de validité", "exp"),
            ("MM / AA", "exp"),
            ("Ablaufdatum", "exp"),
            ("Gültig bis", "exp"),
            ("Fecha de caducidad", "exp"),
            ("Expiry date", "exp"),
            ("cc-csc", "csc"),
            ("CVV", "csc"),
            ("Cryptogramme visuel", "csc"),
            ("Kartenprüfnummer", "csc"),
            ("Código de seguridad de la tarjeta", "csc"),
            ("cc-name", "name"),
            ("Titulaire de la carte", "name"),
            ("Nom du porteur", "name"),
            ("Karteninhaber", "name"),
            ("Titular de la tarjeta", "name"),
            ("Cardholder name", "name"),
        ] {
            assert_eq!(card_field(words, false), Some(kind), "{words}");
        }
        // Beside a card's number only: words that mean other things elsewhere.
        assert_eq!(card_field("Code de sécurité", true), Some("csc"));
        assert_eq!(card_field("Code d'accès (digicode)", true), None, "a delivery's door code, even beside a card");
        assert_eq!(card_field("Code de sécurité", false), None);
        assert_eq!(card_field("Titulaire", true), Some("name"));
        assert_eq!(card_field("Titulaire du compte", false), None);
        assert_eq!(card_field("Mois", true), Some("exp"));
        // Not a payment card's.
        assert!(other_number("SIRET") && other_number("Numéro de suivi du colis") && other_number("IMEI"));
        assert!(!other_number("Numéro de carte cadeau") && !other_number("Instructions de livraison"));
        for other in [
            "Numéro de carte de fidélité",
            "Numéro de carte Vitale",
            "Date d'expiration du passeport",
            "Gift card number",
            "Kartennummer der Krankenkasse",
            "Numéro de carte d'adhérent",
            "Code postal",
            "E-mail",
            "Mot de passe",
            "SIRET",
            "",
        ] {
            assert_eq!(card_field(other, false), None, "{other}");
        }
    }

    #[test]
    fn a_secure_field_card_never_asks_for_a_cards_details() {
        // by kind
        for kind in ["card", "cvc", "credit_card", "carte", "expiry", "card_number"] {
            assert!(asks_for_card(kind, &["boutique"]), "{kind}");
        }
        // by its words, in four languages
        for words in [
            "Numéro de carte",
            "cryptogramme",
            "Date d'expiration de ta carte",
            "Titulaire de la carte",
            "carte bancaire",
            "Kreditkarte",
            "Kartennummer",
            "número de tarjeta",
            "tarjeta de crédito",
            "credit card number",
            "CVC",
            "card_number",
            "pour payer avec ta carte bancaire",
            "4242 4242 4242 4242",
        ] {
            assert!(asks_for_card("password", &["", words]), "{words}");
        }
        // a password, a token, a code for other things
        for words in ["E-mail de ton compte Garmin", "garmin", "Accès au portail France-Visas", "Code reçu par SMS",
                      "Numéro de carte de fidélité Carrefour", "Identifiant", "ovh_api"] {
            assert!(!asks_for_card("password", &[words]), "{words}");
            assert!(!asks_for_card("code", &[words]), "{words}");
        }
    }
}
