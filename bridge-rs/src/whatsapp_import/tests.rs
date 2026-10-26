//! The parser on sample exports (fictional names only), the dedupe and the model's answer checks.

use std::io::Write;

use chrono::NaiveDate;
use serde_json::json;

use super::fill::{self, Window};
use super::parse::{self, Chat, Message};
use super::*;

/// An iPhone export in French (inside a .zip as `_chat.txt`): system lines start with a left-to-right mark.
const IOS_FR: &str = "\u{200e}[12/09/2026 09:15:02] Léa Martin: \u{200e}Les messages et les appels sont chiffrés de bout en bout. Aucun tiers, pas même WhatsApp, ne peut les lire ou les écouter.
[12/09/2026 09:15:40] Léa Martin: Coucou ! Tu viens samedi pour l'anniversaire de Hugo ?
[12/09/2026 09:17:05] Alex Dubois: Oui avec plaisir
Je t'envoie le devis du traiteur ce soir
\u{200e}[12/09/2026 09:18:11] Léa Martin: \u{200e}image absente
[13/09/2026 21:02:00] Alex Dubois: Le devis : 480 € pour 20 personnes \u{200e}<Ce message a été modifié>
[13/09/2026 21:03:30] Léa Martin: Ce message a été supprimé
[14/10/2026 08:00:00] Léa Martin: Code de la porte : mot de passe: Tr0ub4dor&3
";

/// An Android export in American English: month first, a 12-hour clock with a narrow no-break space.
const ANDROID_EN: &str = "10/2/26, 9:15\u{202f}AM - Messages and calls are end-to-end encrypted. No one outside of this chat, not even WhatsApp, can read or listen to them. Tap to learn more.
10/2/26, 9:15\u{202f}AM - Sam Carter created group \"Hiking club\"
10/2/26, 9:16\u{202f}AM - Sam Carter: Who's in for the Sunday hike?
10/2/26, 12:05\u{202f}PM - Priya Shah: Me!
I'll bring the map
10/2/26, 12:06\u{202f}PM - Priya Shah: <Media omitted>
10/14/26, 7:45\u{202f}PM - Alex Dubois: I'll book the minibus for the 25th
";

const ANDROID_DE: &str = "25.10.26, 14:03 - Jonas Weber: Hallo! Treffen wir uns am Freitag?
25.10.26, 14:05 - Alex Dubois: Ja, um 19 Uhr
25.10.26, 14:06 - Jonas Weber: <Medien ausgeschlossen>
";

const ANDROID_ES: &str = "25/10/26, 2:03 p. m. - Lucía Gómez: ¿Comemos el domingo?
25/10/26, 2:10 p. m. - Alex Dubois: Vale, llevo el postre
";

fn zip_of(name: &str, text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
        writer.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        writer.write_all(text.as_bytes()).unwrap();
        writer.finish().unwrap();
    }
    out
}

#[test]
fn an_iphone_export_in_french_is_read_from_its_zip() {
    let raw = zip_of("_chat.txt", IOS_FR);
    let text = parse::text_of("WhatsApp Chat - Léa Martin.zip", &raw).unwrap();
    let chat = parse::parse(&text, "WhatsApp Chat - Léa Martin.zip").unwrap();
    assert_eq!(chat.title, "Léa Martin");
    assert_eq!(chat.participants, vec!["Léa Martin", "Alex Dubois"]);
    let texts: Vec<&str> = chat.messages.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(
        texts,
        vec![
            "Coucou ! Tu viens samedi pour l'anniversaire de Hugo ?",
            "Oui avec plaisir\nJe t'envoie le devis du traiteur ce soir",
            "Le devis : 480 € pour 20 personnes",
            "Code de la porte : mot de passe: Tr0ub4dor&3",
        ]
    );
    assert_eq!(chat.messages[0].at, "2026-09-12 09:15:40");
    assert_eq!(chat.messages[3].at, "2026-10-14 08:00:00");
    assert_eq!((chat.media, chat.system), (1, 2));
    assert!(chat.messages.iter().all(|m| !m.text.contains('\u{200e}')));
}

#[test]
fn an_android_export_in_english_reads_month_first_and_the_12_hour_clock() {
    let chat = parse::parse(ANDROID_EN, "WhatsApp Chat with Hiking club.txt").unwrap();
    assert_eq!(chat.title, "Hiking club");
    assert_eq!(chat.messages.len(), 3);
    assert_eq!(chat.messages[0].at, "2026-10-02 09:16:00");
    assert_eq!(chat.messages[1].at, "2026-10-02 12:05:00");
    assert_eq!(chat.messages[1].text, "Me!\nI'll bring the map");
    assert_eq!(chat.messages[2].at, "2026-10-14 19:45:00");
    assert_eq!(chat.participants.len(), 3);
    assert_eq!((chat.media, chat.system), (1, 2));
}

#[test]
fn german_and_spanish_android_exports() {
    let de = parse::parse(ANDROID_DE, "WhatsApp-Chat mit Jonas Weber.txt").unwrap();
    assert_eq!(de.title, "Jonas Weber");
    assert_eq!(de.messages.len(), 2);
    assert_eq!(de.messages[1].at, "2026-10-25 14:05:00");
    assert_eq!(de.media, 1);
    let es = parse::parse(ANDROID_ES, "Chat de WhatsApp con Lucía Gómez.txt").unwrap();
    assert_eq!(es.title, "Lucía Gómez");
    assert_eq!(es.messages[0].at, "2026-10-25 14:03:00");
    assert_eq!(es.messages[1].text, "Vale, llevo el postre");
}

#[test]
fn titles_from_the_file_names_whatsapp_gives() {
    for (name, title) in [
        ("WhatsApp Chat - Léa Martin.zip", "Léa Martin"),
        ("Discussion WhatsApp avec Léa Martin.txt", "Léa Martin"),
        ("WhatsApp Chat with Sam Carter (2).txt", "Sam Carter"),
        ("WhatsApp Chat mit Foot 2026.txt", "Foot 2026"),
        ("_chat.txt", ""),
        ("notes.txt", ""),
    ] {
        assert_eq!(parse::title_of(name), title, "{name}");
    }
}

#[test]
fn ambiguous_dates_take_the_day_first_unless_a_12_hour_clock_says_otherwise() {
    let dmy = parse::parse("03/04/26 10:00 - Ana: a\n05/04/26 10:00 - Ana: b\n", "x.txt").unwrap();
    assert_eq!(dmy.messages[0].at, "2026-04-03 10:00:00");
    let mdy = parse::parse("03/04/26, 10:00 AM - Ana: a\n03/05/26, 1:00 PM - Ana: b\n", "x.txt").unwrap();
    assert_eq!(mdy.messages[0].at, "2026-03-04 10:00:00");
    assert_eq!(mdy.messages[1].at, "2026-03-05 13:00:00");
}

#[test]
fn files_that_are_not_exports_are_refused() {
    assert!(parse::parse("hello\nworld\n", "notes.txt").is_err());
    assert!(parse::text_of("photo.jpg", b"\xff\xd8\xff").is_err());
    assert!(parse::text_of("x.zip", &zip_of("readme.md", "hi")).is_err());
}

fn chat_of(lines: &[(&str, &str, &str)]) -> Chat {
    Chat {
        title: "Léa Martin".into(),
        participants: vec!["Léa Martin".into(), "Alex Dubois".into()],
        messages: lines
            .iter()
            .map(|(at, s, t)| Message { at: (*at).into(), sender: (*s).into(), text: (*t).into() })
            .collect(),
        ..Chat::default()
    }
}

#[test]
fn a_chat_exported_again_adds_only_its_new_part() {
    let first = chat_of(&[("2026-10-01 10:00:00", "Léa Martin", "a"), ("2026-10-02 10:00:00", "Alex Dubois", "b")]);
    let known = vec![Known {
        id: "lea-martin".into(),
        key: identity(&first),
        last_at: "2026-10-02 10:00:00".into(),
        last_fp: first.messages[1].fingerprint(),
        ..Known::default()
    }];
    // the same chat later, renamed on the phone (another title): found by its last message
    let mut again = chat_of(&[
        ("2026-10-01 10:00:00", "Léa Martin", "a"),
        ("2026-10-02 10:00:00", "Alex Dubois", "b"),
        ("2026-10-02 10:00:00", "Léa Martin", "c"),
        ("2026-11-03 08:00:00", "Léa Martin", "d"),
    ]);
    again.title = "Léa ❤️".into();
    let found = find_known(&known, &again).unwrap();
    let texts: Vec<&str> = new_part(&again, Some(found)).iter().map(|m| m.text.as_str()).collect();
    assert_eq!(texts, vec!["c", "d"]);
    // its last message gone from the export (deleted on the phone): by its time, under the same name
    let other = chat_of(&[("2026-10-01 10:00:00", "Léa Martin", "a"), ("2026-10-05 10:00:00", "Léa Martin", "e")]);
    let found = find_known(&known, &other).unwrap();
    assert_eq!(new_part(&other, Some(found)).len(), 1);
    // another chat entirely
    let mut stranger = chat_of(&[("2026-10-01 10:00:00", "Sam Carter", "x")]);
    stranger.title = "Sam Carter".into();
    assert!(find_known(&known, &stranger).is_none());
}

#[test]
fn month_files_are_appended_and_secrets_taken_out() {
    let dir = tempfile::tempdir().unwrap();
    let chat = chat_of(&[
        ("2026-09-30 22:00:00", "Léa Martin", "On se voit samedi ?"),
        ("2026-10-01 08:00:00", "Alex Dubois", "mot de passe: Tr0ub4dor&3"),
    ]);
    let months = write_months(dir.path(), &chat, &chat.messages[..1]).unwrap();
    assert_eq!(months, vec!["2026-09"]);
    let months = write_months(dir.path(), &chat, &chat.messages[1..]).unwrap();
    assert_eq!(months, vec!["2026-10"]);
    let sept = std::fs::read_to_string(dir.path().join("2026-09.md")).unwrap();
    assert!(sept.starts_with("---\ntitle: WhatsApp · Léa Martin · 2026-09\ndate: 2026-09-01\n"));
    assert!(sept.ends_with("[2026-09-30 22:00] Léa Martin: On se voit samedi ?\n"));
    let oct = std::fs::read_to_string(dir.path().join("2026-10.md")).unwrap();
    assert!(!oct.contains("Tr0ub4dor") && oct.contains("[•••]"), "{oct}");
    let doc = crate::archives::extract::note("lea/2026-09.md", &sept);
    assert_eq!((doc.title.as_str(), doc.date.as_str()), ("WhatsApp · Léa Martin · 2026-09", "2026-09-01"));
}

fn window(text: &str, last: &str) -> Window {
    Window {
        text: text.into(),
        months: ["2026-10".to_string()].into(),
        first: last.into(),
        last: last.into(),
        messages: 1,
    }
}

#[test]
fn the_answer_keeps_only_what_the_chat_holds() {
    let w = window(
        "[2026-10-22 10:00] Léa Martin: Hugo fête ses 7 ans samedi, on se voit chez Marc ?\n",
        "2026-10-22 10:00:00",
    );
    let answer = json!({
        "people": [
            {"name": "Léa Martin", "relation": "friend", "facts": ["Organise l'anniversaire de Hugo"]},
            {"name": "Hugo", "relation": "family", "facts": []},
            {"name": "Inventé Personne", "relation": "friend", "facts": ["N'existe pas"]},
            {"name": "Alex Dubois", "relation": "other", "facts": ["C'est la personne"]},
            {"name": "Marc", "facts": ["Carte 4970 1012 3456 7890"]}
        ],
        "subjects": [{"title": "Anniversaire de Hugo", "facts": ["Samedi chez Marc", "", "a", "b"]}],
        "dates": [
            {"date": "2026-10-24", "what": "Anniversaire de Hugo chez Marc", "person": "Léa Martin"},
            {"date": "2026-01-02", "what": "Une réunion passée", "person": ""}
        ],
        "tasks": [{"title": "Envoyer le devis", "due": ""}, {"title": "Vieux truc", "due": "2025-01-01"}]
    })
    .to_string();
    let today = NaiveDate::from_ymd_opt(2026, 10, 23).unwrap();
    let found = fill::parse_answer(&format!("Voici : {answer}"), &w, "Alex Dubois", today).unwrap();
    let names: Vec<&str> = found.people.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["Léa Martin", "Hugo"]);
    assert_eq!(found.people[1].facts, vec!["De la famille (WhatsApp)"]);
    assert_eq!(found.subjects[0].facts.len(), fill::FACTS_PER_PAGE);
    assert_eq!(found.dates, vec![("2026-10-24".into(), "Anniversaire de Hugo chez Marc".into(), "Léa Martin".into())]);
    assert_eq!(found.tasks, vec![("Envoyer le devis".to_string(), String::new())]);
    // an undated task from a chat months old is left out
    let old = window("[2026-03-01 10:00] Léa Martin: x\n", "2026-03-01 10:00:00");
    let found = fill::parse_answer(&answer, &old, "", today).unwrap();
    assert!(found.tasks.is_empty());
    assert!(fill::parse_answer("pas de JSON", &w, "", today).is_err());
}

#[test]
fn windows_and_what_the_carnet_reads() {
    let mut messages = Vec::new();
    for day in 1..=28 {
        messages.push(Message {
            at: format!("2026-02-{day:02} 10:00:00"),
            sender: "Léa Martin".into(),
            text: "x".repeat(900),
        });
    }
    messages.insert(0, Message { at: "2024-01-01 10:00:00".into(), sender: "Léa Martin".into(), text: "vieux".into() });
    // a first import: the last year only
    assert_eq!(fill::to_fill(&messages, "").len(), 28);
    // a later one: after what was read
    assert_eq!(fill::to_fill(&messages, "2026-02-20 10:00:00").len(), 8);
    let parts = fill::windows(fill::to_fill(&messages, ""));
    assert!(parts.len() >= 3 && parts.iter().all(|w| w.text.len() < fill::WINDOW_CHARS));
    assert_eq!(parts.iter().map(|w| w.messages).sum::<usize>(), 28);
    let estimate = fill::estimate("Léa", &["Léa Martin".into()], fill::to_fill(&messages, ""), None);
    assert_eq!((estimate.calls, estimate.eur), (parts.len(), 0.0));
    assert!(estimate.tokens_in > 7000);
}

#[test]
fn the_report_line_counts_what_was_added() {
    let counts = json!({"people": 12, "subjects": 5, "dates": 3, "tasks": 1});
    assert_eq!(
        report_line("fr", "Léa", &counts, 40, None),
        "Discussion « Léa » importée (40 nouveaux messages dans les Archives) : 12 personnes, 5 sujets, 3 dates, \
         1 tâche ajoutés au Carnet."
    );
    let none = json!({"people": 0, "subjects": 0, "dates": 0, "tasks": 0});
    assert!(report_line("en", "Sam", &none, 2, Some("budget"))
        .ends_with("nothing new for the Notebook. Stopped at the cost limit: share the chat again to go on."));
}

#[test]
fn the_home_of_a_chat() {
    let one = fill::home_of("Léa Martin", &["Léa Martin".into(), "Alex Dubois".into()], "Alex Dubois");
    assert_eq!((one.kind, one.title.as_str()), ("person", "Léa Martin"));
    let group = fill::home_of("Hiking club", &["Sam".into(), "Priya".into(), "Alex".into()], "Alex");
    assert_eq!((group.kind, group.title.as_str()), ("subject", "Hiking club"));
}
