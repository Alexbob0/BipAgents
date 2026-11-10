//! Written language and spoken language differ (Alex 10/11): when a reply is listened to (Live, a reply to a voice
//! message, « Écouter »), the Bips must be clear for a human ear. Three parts, all here:
//!
//! - **the radio rules** (`radio_rule`): the essential first, structure announced, three things at most in a row,
//!   short sentences, spoken dates, rounded numbers, the key point again at the end; never a table, a long list, a
//!   link, a code or an address read out: shown, and said so. For a Live call in its brief (live_brief.rs, both apps
//!   put the brief in the call's prelude) and for a reply to a voice message in a hidden note joined to it
//!   (`voice_note`, live_brief.rs `message_notes`); never in the system prompt (its cache).
//! - **the hidden spoken version** of a very structured written reply: the agent ends it with `<!--bip:say …-->`
//!   (`SAY_RULE`, one line in the soul's block). It is never shown: the bridge takes it out wherever it passes a
//!   reply on (`SayFilter` in media.rs's `MediaFilter`: a run's and a Live turn's deltas, even when the mark is cut
//!   across deltas, and the end's `output`; `strip` for push notifications; the general `<!--bip:…-->` notes rule of
//!   the timeline, the Boîte, shares, search and the Carnet's reading of conversations) and gives it as `spoken` next
//!   to the text (the end event, the timeline's message items). The apps' « Écouter » and voice replies read `spoken`
//!   when there is one.
//! - **the speech structure** (`structure`): without a spoken version, markdown's structure becomes spoken structure
//!   before the speech-text rules (tts.rs `prepare`, also `/v1/tts/prepare` for the phones' Bip voices): lists as
//!   « d'abord… ensuite… enfin » (three or fewer) or « premièrement… deuxièmement… » (announced), headings as short
//!   transitions, tables as « un tableau de N lignes » and their first row, code blocks and bare links named, not
//!   read (parentheses, abbreviations and numbers: numbers_fr.rs / speech_intl.rs, as before).

use std::sync::LazyLock;

use regex::Regex;

/// The hidden spoken version's mark (`<!--bip:say …-->`), like the apps' other `<!--bip:…-->` notes.
pub const OPEN: &str = "<!--bip:say";
const CLOSE: &str = "-->";

/// What the agent reads in its soul (souls.rs, the language block): a spoken version for structured replies only.
/// Short: paid on every call.
pub const SAY_RULE: &str = "Only a long structured written reply (a list of more than 3 items, a table, several \
links, or over 150 words) ends with its hidden spoken version, read aloud when it is listened to, the sentences inside \
the mark: <!--bip:say 2 or 3 short spoken sentences, what is there and the essential, never the list itself-->. Any \
other reply has none (a short answer, a number, thanks, small talk), nor a voice call.";

/// A text without its spoken versions, and those (joined; a mark never closed takes the rest of the text).
pub fn split(text: &str) -> (String, Option<String>) {
    if !text.contains("bip:say") {
        return (text.to_string(), None);
    }
    let mut filter = SayFilter::default();
    let mut shown = filter.feed(text);
    shown.push_str(&filter.finish());
    let shown = if filter.spoken().is_some() { shown.trim_end().to_string() } else { shown };
    (shown, filter.spoken())
}

/// A text without its spoken versions.
pub fn strip(text: &str) -> String {
    split(text).0
}

/// The spoken versions taken out of a text that comes in pieces (a run's deltas): what may still become a mark is
/// held back until it is known.
#[derive(Debug, Default)]
pub struct SayFilter {
    held: String,
    inside: bool,
    /// An empty mark met (`<!--bip:say-->`, 10/11: GLM wrote its spoken version after it): the rest is spoken.
    rest: bool,
    spoken: Vec<String>,
}

impl SayFilter {
    /// A piece of text as it can be shown now.
    pub fn feed(&mut self, chunk: &str) -> String {
        let mut text = std::mem::take(&mut self.held);
        text.push_str(chunk);
        let mut out = String::new();
        let mut rest = text.as_str();
        loop {
            if self.rest {
                self.held = rest.to_string();
                return out;
            }
            if self.inside {
                let Some(end) = rest.find(CLOSE) else {
                    self.held = rest.to_string();
                    return out;
                };
                self.rest = rest[..end].trim().is_empty();
                self.said(&rest[..end]);
                self.inside = false;
                rest = &rest[end + CLOSE.len()..];
                continue;
            }
            match find_open(rest) {
                Open::At(at, len) => {
                    out.push_str(&rest[..at]);
                    rest = &rest[at + len..];
                    self.inside = true;
                }
                Open::Maybe(at) => {
                    out.push_str(&rest[..at]);
                    self.held = rest[at..].to_string();
                    return out;
                }
                Open::No => {
                    out.push_str(rest);
                    return out;
                }
            }
        }
    }

    /// The end of the text: what was held back and never became a mark (a mark never closed is spoken, not shown).
    pub fn finish(&mut self) -> String {
        let held = std::mem::take(&mut self.held);
        if self.inside || self.rest {
            (self.inside, self.rest) = (false, false);
            self.said(&held);
            return String::new();
        }
        held
    }

    /// The spoken versions met so far.
    pub fn spoken(&self) -> Option<String> {
        let all = self.spoken.join(" ");
        (!all.is_empty()).then_some(all)
    }

    fn said(&mut self, words: &str) {
        let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
        if !words.is_empty() {
            self.spoken.push(words);
        }
    }
}

enum Open {
    /// A mark starts at this byte, its opening this long.
    At(usize, usize),
    /// What starts at this byte may still become one.
    Maybe(usize),
    No,
}

/// `<!--`, spaces, `bip:say`, then a space or the end of the note.
fn find_open(text: &str) -> Open {
    let mut from = 0;
    while let Some(i) = text[from..].find('<').map(|i| i + from) {
        match opening(&text[i..]) {
            Some(Some(len)) => return Open::At(i, len),
            Some(None) => return Open::Maybe(i),
            None => from = i + 1,
        }
    }
    Open::No
}

/// The opening at the start of `text`: Some(Some(its length)), Some(None) when the text ends before it is known,
/// None when it is not one.
fn opening(text: &str) -> Option<Option<usize>> {
    let mut rest = text;
    for (i, part) in ["<!--", "", "bip:say"].iter().enumerate() {
        if i == 1 {
            rest = rest.trim_start();
            if rest.is_empty() {
                return Some(None);
            }
            continue;
        }
        if rest.len() < part.len() {
            return part.starts_with(rest).then_some(None);
        }
        if !rest.starts_with(part) {
            return None;
        }
        rest = &rest[part.len()..];
    }
    match rest.chars().next() {
        None => Some(None),
        Some(c) if c.is_whitespace() => Some(Some(text.len() - rest.len() + c.len_utf8())),
        Some('-') if rest.len() < CLOSE.len() => CLOSE.starts_with(rest).then_some(None),
        Some('-') if rest.starts_with(CLOSE) => Some(Some(text.len() - rest.len())),
        _ => None,
    }
}

/// The radio rules (Alex 10/11), the instructions in English and their examples in the agent's language. `live`: a
/// call (what is shown goes to the conversation with `after_call`), else a reply that will be listened to.
pub fn radio_rule(lang: &str, live: bool) -> String {
    let [three, eight, date, round, again] = examples(lang);
    let show = if live {
        "note it with the after_call tool (it lands in the conversation when the call ends; all you write here is \
said aloud)"
    } else {
        "keep it in your written reply with its hidden spoken version"
    };
    format!(
        "Spoken, for the ear: the essential first in one sentence, details only if asked. Short sentences, one idea \
each. Several things: announce them ({three}); three at most in a row, more: say how many and show them ({eight}). \
Never read out a table, a long list, a link, a code or an address: {show}, and say so with the key fact. Dates and \
times as spoken ({date}, never 12/11 09:00); round when it changes nothing ({round}). A long answer ends on its key \
point again ({again})."
    )
}

/// The note joined to a voice message (live_brief.rs `message_notes`): its reply will be listened to.
pub fn voice_note(lang: &str) -> String {
    format!(
        "<!--bip:voice\nYour person sent a voice message: your reply will be listened to. {}\n-->",
        radio_rule(lang, false)
    )
}

fn examples(lang: &str) -> [&'static str; 5] {
    match lang {
        "fr" => [
            "« J'ai trois choses. D'abord…, ensuite…, pour finir… »",
            "« J'en ai trouvé huit, je te mets la liste dans la conversation »",
            "« jeudi à 9 h »",
            "« environ 420 euros »",
            "« Donc : plombier jeudi 9 h »",
        ],
        "de" => [
            "« Ich habe drei Dinge. Erstens…, dann…, zum Schluss… »",
            "« Ich habe acht gefunden, die Liste schreibe ich dir in den Chat »",
            "« Donnerstag um 9 »",
            "« etwa 420 Euro »",
            "« Also: Klempner Donnerstag um 9 »",
        ],
        "es" => [
            "« Tengo tres cosas. Primero…, luego…, para terminar… »",
            "« He encontrado ocho, te pongo la lista en la conversación »",
            "« el jueves a las 9 »",
            "« unos 420 euros »",
            "« O sea: fontanero el jueves a las 9 »",
        ],
        _ => [
            "« I have three things. First…, then…, finally… »",
            "« I found eight, I'll put the list in our chat »",
            "« Thursday at 9 »",
            "« about 420 euros »",
            "« So: plumber Thursday at 9 »",
        ],
    }
}

/// The words a spoken structure uses, per language.
struct Words {
    /// Three items or fewer.
    few: [&'static str; 3],
    /// More, announced (`count`).
    ordinals: [&'static str; 10],
    then: &'static str,
    count: fn(usize) -> String,
    table: fn(usize) -> String,
    first_row: &'static str,
    detail: &'static str,
    code: &'static str,
    link: &'static str,
    links: &'static str,
}

fn words(lang: &str) -> Words {
    match lang {
        "fr" => Words {
            few: ["D'abord", "Ensuite", "Enfin"],
            ordinals: [
                "Premièrement",
                "Deuxièmement",
                "Troisièmement",
                "Quatrièmement",
                "Cinquièmement",
                "Sixièmement",
                "Septièmement",
                "Huitièmement",
                "Neuvièmement",
                "Dixièmement",
            ],
            then: "Puis",
            count: |n| format!("Il y a {n} points."),
            table: |n| if n == 1 { "Un tableau d'une ligne.".into() } else { format!("Un tableau de {n} lignes.") },
            first_row: "Première ligne :",
            detail: "Le détail est dans le message.",
            code: "Il y a un bout de code dans le message.",
            link: "Le lien est dans le message.",
            links: "Les liens sont dans le message.",
        },
        "de" => Words {
            few: ["Zuerst", "Dann", "Zum Schluss"],
            ordinals: [
                "Erstens",
                "Zweitens",
                "Drittens",
                "Viertens",
                "Fünftens",
                "Sechstens",
                "Siebtens",
                "Achtens",
                "Neuntens",
                "Zehntens",
            ],
            then: "Dann",
            count: |n| format!("Es sind {n} Punkte."),
            table: |n| {
                if n == 1 {
                    "Eine Tabelle mit einer Zeile.".into()
                } else {
                    format!("Eine Tabelle mit {n} Zeilen.")
                }
            },
            first_row: "Erste Zeile:",
            detail: "Die Details stehen in der Nachricht.",
            code: "In der Nachricht steht ein Stück Code.",
            link: "Der Link steht in der Nachricht.",
            links: "Die Links stehen in der Nachricht.",
        },
        "es" => Words {
            few: ["Primero", "Luego", "Por último"],
            ordinals: [
                "Primero", "Segundo", "Tercero", "Cuarto", "Quinto", "Sexto", "Séptimo", "Octavo", "Noveno", "Décimo",
            ],
            then: "Luego",
            count: |n| format!("Hay {n} puntos."),
            table: |n| if n == 1 { "Una tabla de una fila.".into() } else { format!("Una tabla de {n} filas.") },
            first_row: "Primera fila:",
            detail: "El detalle está en el mensaje.",
            code: "Hay un trozo de código en el mensaje.",
            link: "El enlace está en el mensaje.",
            links: "Los enlaces están en el mensaje.",
        },
        "nl" => Words {
            few: ["Eerst", "Dan", "Tot slot"],
            ordinals: [
                "Ten eerste",
                "Ten tweede",
                "Ten derde",
                "Ten vierde",
                "Ten vijfde",
                "Ten zesde",
                "Ten zevende",
                "Ten achtste",
                "Ten negende",
                "Ten tiende",
            ],
            then: "Dan",
            count: |n| format!("Er zijn {n} punten."),
            table: |n| if n == 1 { "Een tabel met één rij.".into() } else { format!("Een tabel met {n} rijen.") },
            first_row: "Eerste rij:",
            detail: "De details staan in het bericht.",
            code: "Er staat een stukje code in het bericht.",
            link: "De link staat in het bericht.",
            links: "De links staan in het bericht.",
        },
        _ => Words {
            few: ["First", "Then", "Finally"],
            ordinals: ["First", "Second", "Third", "Fourth", "Fifth", "Sixth", "Seventh", "Eighth", "Ninth", "Tenth"],
            then: "Then",
            count: |n| format!("There are {n} items."),
            table: |n| if n == 1 { "A table with one row.".into() } else { format!("A table with {n} rows.") },
            first_row: "First row:",
            detail: "The details are in the message.",
            code: "There is a piece of code in the message.",
            link: "The link is in the message.",
            links: "The links are in the message.",
        },
    }
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid regex")
}

/// A code block with code in it (a lone fence, a call's sentence cut there, is left to the speech rules).
static FENCE: LazyLock<Regex> = LazyLock::new(|| re(r"(?s)```[^\n]*\n(.*?)(?:```|\z)"));
static ITEM: LazyLock<Regex> = LazyLock::new(|| re(r"^(\s*)(?:[-*+•▪◦]|\d{1,3}[.)])\s+(.*)$"));
static HEADING: LazyLock<Regex> = LazyLock::new(|| re(r"^\s{0,3}#{1,6}\s+(.+?)\s*#*\s*$"));
static BOLD_LINE: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*(?:\*\*|__)([^*_\n.!?]{1,40}?)(?:\*\*|__)\s*:?\s*$"));
static TABLE_SEP: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*\|?\s*:?-{2,}:?\s*(?:\|\s*:?-{2,}:?\s*)*\|?\s*$"));
static LINK: LazyLock<Regex> = LazyLock::new(|| re(r"!?\[([^\]\n]+)\]\([^)\n]*\)"));
static AUTOLINK: LazyLock<Regex> = LazyLock::new(|| re(r"<((?:https?://|www\.)[^>\s]+)>"));
static BARE_URL: LazyLock<Regex> = LazyLock::new(|| re(r"\b(?:https?://|www\.)[^\s)\]>]+"));
static BLANKS: LazyLock<Regex> = LazyLock::new(|| re(r"\n{3,}"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| re(r"[ \t]{2,}"));

fn is_row(line: &str) -> bool {
    line.matches('|').count() >= 2
}

fn cells(line: &str) -> Vec<String> {
    line.trim().trim_matches('|').split('|').map(|c| c.trim().to_string()).collect()
}

/// A markdown text with its structure said in words (`lang`: fr, en, de, es, nl): see the module's doc. A plain text
/// is unchanged; a single list item stays as it is (a call's sentences come one by one).
pub fn structure(text: &str, lang: &str) -> String {
    let w = words(lang);
    let text = text.replace("\r\n", "\n");
    let text = FENCE.replace_all(&text, |c: &regex::Captures| match c[1].trim().is_empty() {
        true => c[0].to_string(),
        false => format!("\n\n{}\n\n", w.code),
    });
    let text = LINK.replace_all(&text, "$1");
    let text = AUTOLINK.replace_all(&text, "$1");
    let urls = BARE_URL.find_iter(&text).count();
    let text = BARE_URL.replace_all(&text, "");
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        // a table: its header, its separator, its rows
        if is_row(line) && lines.get(i + 1).is_some_and(|l| TABLE_SEP.is_match(l)) {
            let header = cells(line);
            let mut rows = Vec::new();
            i += 2;
            while i < lines.len() && is_row(lines[i]) {
                rows.push(cells(lines[i]));
                i += 1;
            }
            out.push(String::new());
            out.push((w.table)(rows.len()));
            if let Some(first) = rows.first() {
                let said: Vec<String> = header
                    .iter()
                    .zip(first)
                    .filter(|(_, c)| !c.is_empty())
                    .map(|(h, c)| match (h.is_empty(), lang) {
                        (true, _) => c.clone(),
                        (_, "fr") => format!("{h} : {c}"),
                        _ => format!("{h}: {c}"),
                    })
                    .collect();
                out.push(format!("{} {}.", w.first_row, said.join(", ")));
                if rows.len() > 1 {
                    out.push(w.detail.to_string());
                }
            }
            out.push(String::new());
            continue;
        }
        // a list: its items (a deeper line or item joins the one above), blank lines between them allowed
        if let Some(first) = ITEM.captures(line) {
            let depth = first[1].len();
            let mut items: Vec<String> = vec![first[2].trim().to_string()];
            let mut j = i + 1;
            let mut end = j;
            while j < lines.len() {
                let next = lines[j];
                if next.trim().is_empty() {
                    j += 1;
                    continue;
                }
                match ITEM.captures(next) {
                    Some(c) if c[1].len() <= depth => items.push(c[2].trim().to_string()),
                    Some(c) => join(items.last_mut(), c[2].trim()),
                    None if next.starts_with([' ', '\t']) && j == end => join(items.last_mut(), next.trim()),
                    None => break,
                }
                j += 1;
                end = j;
            }
            if items.len() < 2 {
                out.push(line.to_string());
                i += 1;
                continue;
            }
            out.push(String::new());
            let n = items.len();
            if n > 3 {
                out.push((w.count)(n));
            }
            for (k, item) in items.iter().enumerate() {
                let lead = if n <= 3 { w.few[k] } else { w.ordinals.get(k).copied().unwrap_or(w.then) };
                out.push(format!("{lead}, {}", stopped(item)));
                out.push(String::new());
            }
            i = end;
            continue;
        }
        if let Some(c) = HEADING.captures(line).or_else(|| BOLD_LINE.captures(line)) {
            out.push(String::new());
            out.push(format!("{} :", c[1].trim().trim_end_matches([':', '.']).trim_end()));
            i += 1;
            continue;
        }
        out.push(line.to_string());
        i += 1;
    }
    let mut text = out.iter().map(|l| SPACES.replace_all(l.trim_end(), " ")).collect::<Vec<_>>().join("\n");
    if urls > 0 {
        text.push_str("\n\n");
        text.push_str(if urls == 1 { w.link } else { w.links });
    }
    BLANKS.replace_all(text.trim(), "\n\n").into_owned()
}

/// An item ended with a full stop (« Lait » → « Lait. »), so each is said apart.
fn stopped(item: &str) -> String {
    let item = item.trim_end();
    if item.ends_with(['.', '!', '?', '…', ':', ';']) {
        item.to_string()
    } else {
        format!("{item}.")
    }
}

fn join(item: Option<&mut String>, more: &str) {
    if let Some(item) = item {
        item.push_str(", ");
        item.push_str(more);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn streamed(text: &str, step: usize) -> (String, Option<String>) {
        let mut filter = SayFilter::default();
        let chars: Vec<char> = text.chars().collect();
        let mut shown = String::new();
        for piece in chars.chunks(step) {
            shown.push_str(&filter.feed(&piece.iter().collect::<String>()));
        }
        shown.push_str(&filter.finish());
        (shown, filter.spoken())
    }

    #[test]
    fn the_spoken_version_never_shows_whatever_the_cuts() {
        let reply =
            "Trois hôtels :\n- A, 95 €\n- B, 120 €\n- C, 140 €\n\n<!--bip:say J'ai trouvé trois hôtels. Le moins \
cher, A, a une terrasse : 95 euros.-->";
        let spoken = Some("J'ai trouvé trois hôtels. Le moins cher, A, a une terrasse : 95 euros.".to_string());
        assert_eq!(split(reply), ("Trois hôtels :\n- A, 95 €\n- B, 120 €\n- C, 140 €".to_string(), spoken.clone()));
        for step in [1, 2, 3, 5, 7, 11, 64] {
            let (shown, said) = streamed(reply, step);
            assert_eq!(said, spoken, "step {step}");
            assert!(!shown.contains("bip") && !shown.contains("<!") && !shown.contains("-->"), "step {step}: {shown}");
            assert_eq!(shown.trim_end(), "Trois hôtels :\n- A, 95 €\n- B, 120 €\n- C, 140 €");
        }
    }

    #[test]
    fn other_notes_and_comparisons_pass_and_a_mark_never_closed_is_spoken() {
        for text in ["a < b et <!--bip:end-->", "<!--bip:renamed bip Léon-->", "<!-- autre -->", "x <", "<!--bip:sa"] {
            assert_eq!(split(text), (text.to_string(), None), "{text}");
            assert_eq!(streamed(text, 1), (text.to_string(), None), "{text}");
        }
        assert_eq!(split("Voilà.\n<!--bip:say La suite"), ("Voilà.".to_string(), Some("La suite".to_string())));
        assert_eq!(streamed("Voilà.\n<!-- bip:say\nDeux\nlignes -->\nFin", 2).1, Some("Deux lignes".to_string()));
        // an empty mark (GLM, 10/11): what follows it is the spoken version
        let glm = "| a | b |\n\n<!--bip:say-->\nTu as quatre hôtels.\nLe moins cher : 95 euros.";
        let spoken = Some("Tu as quatre hôtels. Le moins cher : 95 euros.".to_string());
        assert_eq!(split(glm), ("| a | b |".to_string(), spoken.clone()));
        for step in [1, 2, 5] {
            assert_eq!(streamed(glm, step).1, spoken, "step {step}");
            assert_eq!(streamed(glm, step).0.trim_end(), "| a | b |");
        }
        assert_eq!(strip("Ok <!--bip:say un--> et <!--bip:say deux-->"), "Ok  et");
        assert_eq!(split("Ok <!--bip:say un--> et <!--bip:say deux-->").1.as_deref(), Some("un deux"));
    }

    #[test]
    fn lists_become_spoken_structure() {
        assert_eq!(
            structure("Pour demain :\n- Lait\n- Œufs\n- Pain", "fr"),
            "Pour demain :\n\nD'abord, Lait.\n\nEnsuite, Œufs.\n\nEnfin, Pain."
        );
        assert_eq!(structure("1. Call\n2. Pay", "en"), "First, Call.\n\nThen, Pay.");
        let eight = (1..=8).map(|n| format!("- item{n}")).collect::<Vec<_>>().join("\n");
        let said = structure(&eight, "fr");
        assert!(said.starts_with("Il y a 8 points.\nPremièrement, item1.\n\nDeuxièmement, item2."), "{said}");
        assert!(said.ends_with("Huitièmement, item8."));
        let said = structure("- Hôtel A\n  - terrasse\n- Hôtel B", "fr");
        assert_eq!(said, "D'abord, Hôtel A, terrasse.\n\nEnsuite, Hôtel B.");
        // one item alone (a call's sentence) and plain text: unchanged
        assert_eq!(structure("- Hôtel A", "fr"), "- Hôtel A");
        assert_eq!(structure("Bonjour, ça va ?", "fr"), "Bonjour, ça va ?");
        assert!(structure("- uno\n- dos\n- tres\n- cuatro", "es").starts_with("Hay 4 puntos.\nPrimero, uno."));
        assert!(structure("- a\n- b\n- c\n- d", "de").contains("Viertens, d."));
    }

    #[test]
    fn tables_code_links_and_headings() {
        let table = "| Hôtel | Prix |\n|---|---:|\n| Ibis | 95 € |\n| Mercure | 120 € |\n| Novotel | 140 € |";
        assert_eq!(
            structure(table, "fr"),
            "Un tableau de 3 lignes.\nPremière ligne : Hôtel : Ibis, Prix : 95 €.\nLe détail est dans le message."
        );
        assert_eq!(
            structure("Voilà :\n```sh\nls -la\n```", "en"),
            "Voilà :\n\nThere is a piece of code in the message."
        );
        assert_eq!(structure("```", "fr"), "```", "a lone fence (a call's sentence): left to the speech rules");
        assert_eq!(
            structure("Lis [cet article](https://a.b/c) et https://x.y/z ou www.w.fr", "fr"),
            "Lis cet article et ou\n\nLes liens sont dans le message."
        );
        assert_eq!(structure("## Mardi\nPiscine", "fr"), "Mardi :\nPiscine");
        assert_eq!(structure("**Budget :**\n420 €", "fr"), "Budget :\n420 €");
    }

    #[test]
    fn the_rules_are_short_and_in_the_agents_language() {
        for lang in ["fr", "en", "de", "es"] {
            let rule = radio_rule(lang, true);
            assert!(rule.contains("after_call") && rule.len() < 900, "{lang}: {}", rule.len());
            assert!(!radio_rule(lang, false).contains("after_call"));
            // never the mark's name in a note: the Spark then echoed the whole note (bench 10/11)
            assert!(!radio_rule(lang, true).contains("bip:") && !voice_note(lang).contains("bip:say"));
            let note = voice_note(lang);
            assert!(note.starts_with("<!--bip:voice\n") && note.ends_with("\n-->") && note.matches("-->").count() == 1);
        }
        assert!(radio_rule("fr", true).contains("« environ 420 euros »"));
        assert!(radio_rule("de", true).contains("« etwa 420 Euro »"));
        assert!(SAY_RULE.contains(OPEN) && SAY_RULE.len() < 420, "{}", SAY_RULE.len());
    }
}
