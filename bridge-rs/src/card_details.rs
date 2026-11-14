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
/// length (Visa, Mastercard, CB on both, American Express, Discover, UnionPay, JCB, Diners, Mir, Maestro). The network
/// check keeps a French SIRET (14 digits, Luhn by design) or a phone's IMEI (15, Luhn too) from counting; UATP (15
/// digits from 1, airlines' cards) is left out: a man's French social security number reads the same (review of 14/11).
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
}

/// What may stand between a card number's groups: up to three spaces (any, line breaks and tabs included: a number
/// shown in blocks, a table's cells), dashes (« – » too), dots, slashes, underscores, bars (review of 14/11).
pub const SEPARATORS: &str = r"[\s\-./_|\u{2010}-\u{2015}\u{2212}]{1,3}";
/// The same written as a JSON string's escapes (« \n », « \t », « \u00a0 », « \/ »): what the real server's tools
/// answer is often JSON read as text (execute_script's `{"result": "4242\n4242…"}`, review of 14/11).
const ESCAPED_SEPARATORS: &str = r"\\[ntrfv/]|\\u(?:00[aA]0|200[0-9abAB]|201[0-5]|202[fF]|205[fF]|2212|3000)";
/// Runs of digits split by `SEPARATORS` or their escapes (« 4242 4242 4242 4242 », « 4242-4242-… », « 4242 / 4242… »,
/// « 4242\n4242… »), each with its groups' places in the text.
static RUNS: LazyLock<Regex> = LazyLock::new(|| {
    let one = SEPARATORS.strip_suffix("{1,3}").expect("a class, then how many");
    Regex::new(&format!(r"\d+(?:(?:{one}|{ESCAPED_SEPARATORS}){{1,3}}\d+)*")).expect("valid regex")
});
/// An IBAN as it is written (its country, its check digits, then its account, spaced or not): its digits may make a
/// « card number » (~15 % of French ones in groups of four: review of 14/11); one whose check digits hold (mod 97) is
/// never one.
static IBANS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Z]{2}[0-9]{2}(?:[ \u{a0}\-]?[A-Z0-9]){11,30}").expect("valid regex"));

/// An IBAN's check (ISO 13616: the country and check digits moved to the end, letters as 10–35, mod 97 = 1).
pub fn iban_valid(text: &str) -> bool {
    let plain: String = text.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_uppercase();
    if !(15..=34).contains(&plain.len()) || !plain[..2].bytes().all(|b| b.is_ascii_uppercase()) {
        return false;
    }
    let moved = format!("{}{}", &plain[4..], &plain[..4]);
    let mut rest: u32 = 0;
    for c in moved.chars() {
        let value = match c.to_digit(36) {
            Some(v) => v,
            None => return false,
        };
        for digit in value.to_string().chars() {
            rest = (rest * 10 + digit.to_digit(10).unwrap_or(0)) % 97;
        }
    }
    rest == 1
}

/// Where `text` holds a valid IBAN (byte ranges): the longest valid one from each place an IBAN may start (what
/// follows it, « BIC AGRIFRPP », is not of it).
fn ibans(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for found in IBANS.find_iter(text) {
        let ends = found.as_str().char_indices().filter(|(_, c)| c.is_ascii_alphanumeric()).map(|(i, c)| i + c.len_utf8());
        let mut ends: Vec<usize> = ends.collect();
        ends.reverse();
        if let Some(end) = ends.into_iter().find(|end| iban_valid(&found.as_str()[..*end])) {
            out.push((found.start(), found.start() + end));
        }
    }
    out
}
/// A run's groups of digits; an escape's (« \u2010 ») matched too, to be skipped.
static GROUPS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\u[0-9a-fA-F]{4}|\d+").expect("valid regex"));

/// Where `text` holds a card's number (byte ranges): any run of whole groups of digits that makes one
/// (`is_card_number`), so « 4242 4242 4242 4242 12/30 » and « carte 4242424242424242 » count, and a longer number
/// written without spaces does not.
pub fn card_numbers(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let ibans = ibans(text);
    for run in RUNS.find_iter(text) {
        if ibans.iter().any(|(start, end)| run.start() < *end && *start < run.end()) {
            continue; // an IBAN's digits
        }
        let groups: Vec<(usize, usize)> = GROUPS
            .find_iter(run.as_str())
            .filter(|g| !g.as_str().starts_with('\\'))
            .map(|g| (run.start() + g.start(), run.start() + g.end()))
            .collect();
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
|| (f(4) >= 2200 && f(4) <= 2204 && long) || ((f(2) === 50 || (f(2) >= 56 && f(2) <= 69)) && long); }}; \
const ibanOk = (t) => {{ const p = t.replace(/[^A-Za-z0-9]/g, '').toUpperCase(); if (p.length < 15 || p.length > 34 || !/^[A-Z]{{2}}/.test(p)) return false; \
let r = 0; for (const c of p.slice(4) + p.slice(0, 4)) {{ const v = parseInt(c, 36); if (isNaN(v)) return false; for (const x of String(v)) r = (r * 10 + +x) % 97; }} return r === 1; }}; \
const ibanSpans = (v) => {{ const out = []; for (const m of v.matchAll(/[A-Z]{{2}}[0-9]{{2}}(?:[ \\u00a0\\-]?[A-Z0-9]){{11,30}}/g)) {{ \
for (let e = m[0].length; e >= 15; e--) {{ if (/[A-Za-z0-9]/.test(m[0][e - 1]) && ibanOk(m[0].slice(0, e))) {{ out.push([m.index, m.index + e]); break; }} }} }} return out; }}; \
const RUN = new RegExp({runs}, 'g'); \
const holdsCard = (v) => {{ v = String(v || ''); const ib = ibanSpans(v); for (const m of v.matchAll(RUN)) {{ \
if (ib.some(([s, e]) => m.index < e && s < m.index + m[0].length)) continue; const g = m[0].match(/\\d+/g); \
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
        runs = serde_json::to_string(&format!(r"\d+(?:{}\d+)*", js_separators())).unwrap_or_default(),
    )
}

/// `SEPARATORS` for a JavaScript RegExp (no `\u{…}`: `\u2010`).
fn js_separators() -> String {
    SEPARATORS.replace(r"\u{2010}", r"\u2010").replace(r"\u{2015}", r"\u2015").replace(r"\u{2212}", r"\u2212")
}

/// The secure-field card's kinds an agent might give for a card's details (`ask_secret`'s `kind`): not a password,
/// a token or a code.
static CARD_KINDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"card|carte|karte|tarjeta|\bcc\b|\bcb\b|\bpan\b|cvc|cvv|csc|crypto|expir|\bexp\b|holder|titulaire|payment|paiement|credit|debit|visa|mastercard|amex")
        .expect("valid regex")
});

/// A card, in the words of a request (`ask_secret`; on `folded` text): in French, English, Spanish, German, Italian,
/// Portuguese and Dutch, or by a network's name (review of 14/11: « Les 16 chiffres de ta carte », « Le numéro de ta
/// Visa », « Numero della carta », « Número do cartão », « Kaartnummer van je bankpas »…).
static ANY_CARD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(cartes?|cards?|cartas?|cartao|cartoes|kaart(en)?|karten?|tarjetas?|bankpas|pinpas|betaalpas|mastercard|amex|maestro|girocard|cb|cc)\b|kartennummer|kaartnummer|creditcard|debitcard|cardnumber|card ?holder|carte ?bleue|american express|\bvisa\b",
    )
    .expect("valid regex")
});
/// A card that is no payment card (loyalty, gift, identity, health, transport, library, SIM, membership…) in a
/// request's words: such a card's number may be asked.
static OTHER_CARD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(cartes?|cards?|cartas?|cartao|kaart|karten?|tarjetas?) (de |d |di |da |do |van |der |des )?(la |el |le )?(fidelite|fidelidad|fedelta|fidelidade|loyalty|klanten|cadeau|regalo|presente|geschenk|kado|vitale|grise|identite|identita|identidade|identidad|bibliotheque|biblioteca|library|navigo|transport|sim|adherent|membre|socio|member|membership|student|etudiant|estudiante|club|bonus)|(loyalty|gift|library|member|membership|student|sim|identity|id|bonus|club|payback|fidelity|navigo|kunden|klanten|geschenk|cadeau|vitale) ?(cards?|cartes?|karten?|kaart)|carte vitale|carte grise|carte d identite|personalausweis|gift ?card",
    )
    .expect("valid regex")
});
/// A travel visa, not the card network (« ta demande de visa », France-Visas).
static TRAVEL_VISA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"demande de visa|visa (pour|de |d |touristique|etudiant|de travail|schengen|application)|e ?visa\b|ambassade|consulat|embassy|immigration")
        .expect("valid regex")
});

/// A secure-field card asked for a payment card's details (`ask_secret`): its kind (« card », « cvc »…); its name,
/// identifier label, identifier or reason, each alone or all together (a meaning split over two fields), naming a
/// card at all, unless only a card that is no payment card (loyalty, gift, identity…: `OTHER_CARD`), or naming a
/// card's number, expiry, security code or holder, or a payment card; or holding a card's number. Never asked: the
/// person gives only the security code, and only in the payment's own flow (« Oui, payer »).
pub fn asks_for_card(kind: &str, texts: &[&str]) -> bool {
    let kind_folded = folded(kind);
    if !matches!(kind_folded.trim(), "password" | "token" | "code" | "") && CARD_KINDS.is_match(&kind_folded) {
        return true;
    }
    if texts.iter().any(|t| holds_card_number(t) || card_field(t, false).is_some()) {
        return true;
    }
    let all = folded(&texts.join(" \n "));
    if PAYMENT_CARD.is_match(&all) {
        return true;
    }
    let other = OTHER_CARD.is_match(&all);
    let cards: Vec<&str> = ANY_CARD.find_iter(&all).map(|m| m.as_str()).collect();
    let visa_only = !cards.is_empty() && cards.iter().all(|c| *c == "visa");
    !cards.is_empty() && !other && !(visa_only && TRAVEL_VISA.is_match(&all))
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
        // written otherwise (review of 14/11): double spaces, slashes, dashes, line breaks, a table's cells
        for text in ["4242  4242  4242  4242", "4242/4242/4242/4242", "4242 – 4242 – 4242 – 4242", "4242\n4242\n4242\n4242",
                     "4242\t4242\t4242\t4242", "carte_4242_4242_4242_4242"] {
            assert!(holds_card_number(text), "{text:?}");
        }
        // an IBAN (its check digits hold) or a man's social security number is no card's number
        assert!(iban_valid("FR98 0378 3840 1165 3551 5752 774") && !iban_valid("FR99 0378 3840 1165 3551 5752 774"));
        for text in ["FR98 0378 3840 1165 3551 5752 774", "IBAN : FR98 0378 3840 1165 3551 5752 774 BIC AGRIFRPP",
                     "1 48 10 09 262 121 10", "148100926212110"] {
            assert!(!holds_card_number(text), "{text:?}");
        }
        assert!(holds_card_number("IBAN FR99 0378 3840 1165 3551 5752 774, carte 4242 4242 4242 4242"));
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
        // in a JSON answer read as text (execute_script's, review of 14/11): line breaks, tabs and spaces as escapes
        let answer = r#"{"success": true, "result": "4242\n4242\n4242\n4242\n4242\t4242\t4242\t4242", "error": null}"#;
        assert_eq!(
            hide_card_numbers(answer),
            format!(r#"{{"success": true, "result": "{HIDDEN}\n{HIDDEN}", "error": null}}"#)
        );
        for text in [r#""4242\u00a04242\u202f4242\u00A04242""#, r#""4242\r\n4242\r\n4242\r\n4242""#,
                     r#""4242\/4242\/4242\/4242""#] {
            assert!(holds_card_number(text), "{text}");
        }
        // an escape's own digits are no group: « 4242 » then « \u2010 » (a dash) then the rest
        assert!(holds_card_number(r#""4242\u20104242\u20104242\u20104242""#));
        assert!(!holds_card_number(r#""n° 4242\u00e94242\u00e94242\u00e94242""#), "a letter between them");
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
        // in other words, other languages, or split over two fields (review of 14/11)
        for (kind, texts) in [
            ("password", ["visa", "", "", "Le numéro de ta Visa, pour finir la commande"]),
            ("password", ["pharmacie", "Les 16 chiffres de ta carte", "", "Pour payer ta commande"]),
            ("code", ["pharmacie_date", "", "", "Jusqu'à quand ta carte est valable ?"]),
            ("code", ["pharmacie_nom", "", "", "Le nom imprimé sur ta carte"]),
            ("code", ["pharmacie_code", "", "", "Les 3 chiffres derrière ta carte"]),
            ("password", ["negozio", "", "", "Numero della carta di credito"]),
            ("password", ["loja", "", "", "Número do cartão de crédito"]),
            ("password", ["winkel", "", "", "Kaartnummer van je bankpas"]),
            ("password", ["carte", "Numéro", "", "Pour la pharmacie"]),
            ("password", ["cb", "", "", "pour la commande"]),
        ] {
            assert!(asks_for_card(kind, &texts), "{texts:?}");
        }
        // a password, a token, a code for other things
        for words in ["E-mail de ton compte Garmin", "garmin", "Accès au portail France-Visas", "Code reçu par SMS",
                      "Numéro de carte de fidélité Carrefour", "Identifiant", "ovh_api", "Ta demande de visa pour le Canada",
                      "Le code PIN de ta carte SIM", "Numéro de ta carte Navigo", "Code de ta carte cadeau Fnac",
                      "Accès à ton espace client EDF", "Le mot de passe de ta box"] {
            assert!(!asks_for_card("password", &[words]), "{words}");
            assert!(!asks_for_card("code", &[words]), "{words}");
        }
    }
}
