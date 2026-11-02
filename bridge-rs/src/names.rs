//! The app's things by their names in the person's language (Alex, 2026-11-01: at his wife's place, in English, her
//! agents still said « in your Carnet (Notebook) »). The bridge's texts for the models are in English but named the
//! app's parts by their French names (« Carnet », « Boîte », « Tâches de fond »…), and the models repeated them. The
//! names decided on 2026-10-22 for the Carnet: Notebook, Notizbuch, Cuaderno; the others are the apps' own (iOS
//! Localizable.xcstrings, Android values*/strings.xml), so the agent says what the person reads on their screen.
//!
//! `localize` puts them in a text written for the model (the souls' blocks: souls.rs `for_language`; the tools'
//! descriptions and results: agent_tools.rs, card_pay.rs `serve`; the background tasks' instructions: jobs.rs) for an
//! agent writing in another language than French; French texts are left as they are. The tools' own names
//! (`notebook_note`…) are identifiers and stay. A Carnet's page and folder names are the person's data: never renamed
//! (a Carnet takes the place's language when it is made: notebook/mod.rs `WORDS`); the language block tells the agent
//! to say them in the person's language (`RULE`).
//!
//! No Python counterpart.

use std::sync::LazyLock;

use regex::{Captures, Regex};

#[derive(Debug)]
pub struct Names {
    pub lang: &'static str,
    /// « Carnet ».
    pub notebook: &'static str,
    /// « Boîte »: the agents' messages, cards and reports.
    pub inbox: &'static str,
    /// « Tâches de fond ».
    pub background_tasks: &'static str,
    /// « Tâche de fond terminée », « … en pause », « … proposée » (the pushes, jobs.rs).
    pub task_done: &'static str,
    pub task_paused: &'static str,
    pub task_proposed: &'static str,
    /// « Rappels » (the phone's reminders).
    pub reminders: &'static str,
    /// « Réglages ».
    pub settings: &'static str,
    /// « Boîtes mail ».
    pub mailboxes: &'static str,
    /// « Coffre-fort ».
    pub vault: &'static str,
    /// « Coordonnées » (the vault's contact details).
    pub contact_details: &'static str,
    /// « Coffre des agents ».
    pub agents_vault: &'static str,
    /// « Paiements par tes agents ».
    pub agent_payments: &'static str,
    /// « Archives » (the documents kept for the person: archives/).
    pub archives: &'static str,
    /// « Fiches » (the Carnet's records).
    pub records: &'static str,
    /// « veilleur », « connecteur ».
    pub watcher: &'static str,
    pub connector: &'static str,
    /// « Google Agenda ».
    pub google_calendar: &'static str,
    /// The background task that fills the Carnet (notebook/fill.rs): « Remplir le Carnet ».
    pub fill_notebook: &'static str,
    /// The line after filing: « Noté dans ton Carnet ».
    pub noted: &'static str,
    /// After it, the person's other agents told: « Léa est au courant », « … sont au courant ».
    pub knows: (&'static str, &'static str),
    /// A link to an archived document in a fact: « [[archive:…|voir le document]] ».
    pub see_document: &'static str,
}

pub const NAMES: [Names; 4] = [
    Names {
        lang: "fr",
        notebook: "Carnet",
        inbox: "Boîte",
        background_tasks: "Tâches de fond",
        task_done: "Tâche de fond terminée",
        task_paused: "Tâche de fond en pause",
        task_proposed: "Tâche de fond proposée",
        reminders: "Rappels",
        settings: "Réglages",
        mailboxes: "Boîtes mail",
        vault: "Coffre-fort",
        contact_details: "Coordonnées",
        agents_vault: "Coffre des agents",
        agent_payments: "Paiements par tes agents",
        archives: "Archives",
        records: "Fiches",
        watcher: "veilleur",
        connector: "connecteur",
        google_calendar: "Google Agenda",
        fill_notebook: "Remplir le Carnet",
        noted: "Noté dans ton Carnet",
        knows: ("est au courant", "sont au courant"),
        see_document: "voir le document",
    },
    Names {
        lang: "en",
        notebook: "Notebook",
        inbox: "Inbox",
        background_tasks: "Background tasks",
        task_done: "Background task done",
        task_paused: "Background task paused",
        task_proposed: "Background task suggested",
        reminders: "Reminders",
        settings: "Settings",
        mailboxes: "Mailboxes",
        vault: "Vault",
        contact_details: "Contact details",
        agents_vault: "Agents’ vault",
        agent_payments: "Payments by your agents",
        archives: "Archives",
        records: "Records",
        watcher: "watcher",
        connector: "connector",
        google_calendar: "Google Calendar",
        fill_notebook: "Fill the Notebook",
        noted: "Noted in your Notebook",
        knows: ("knows", "know"),
        see_document: "see the document",
    },
    Names {
        lang: "de",
        notebook: "Notizbuch",
        inbox: "Eingang",
        background_tasks: "Hintergrundaufgaben",
        task_done: "Hintergrundaufgabe erledigt",
        task_paused: "Hintergrundaufgabe pausiert",
        task_proposed: "Hintergrundaufgabe vorgeschlagen",
        reminders: "Erinnerungen",
        settings: "Einstellungen",
        mailboxes: "Postfächer",
        vault: "Tresor",
        contact_details: "Kontaktdaten",
        agents_vault: "Tresor der Agenten",
        agent_payments: "Zahlungen durch deine Agenten",
        archives: "Archive",
        records: "Unterlagen",
        watcher: "Wächter",
        connector: "Connector",
        google_calendar: "Google Kalender",
        fill_notebook: "Notizbuch füllen",
        noted: "Im Notizbuch notiert",
        knows: ("weiß Bescheid", "wissen Bescheid"),
        see_document: "Dokument ansehen",
    },
    Names {
        lang: "es",
        notebook: "Cuaderno",
        inbox: "Buzón",
        background_tasks: "Tareas en segundo plano",
        task_done: "Tarea en segundo plano terminada",
        task_paused: "Tarea en segundo plano en pausa",
        task_proposed: "Tarea en segundo plano propuesta",
        reminders: "Recordatorios",
        settings: "Ajustes",
        mailboxes: "Buzones",
        vault: "Caja fuerte",
        contact_details: "Datos de contacto",
        agents_vault: "Caja fuerte de los agentes",
        agent_payments: "Pagos por tus agentes",
        archives: "Archivos",
        records: "Registros",
        watcher: "vigilante",
        connector: "conector",
        google_calendar: "Google Calendar",
        fill_notebook: "Rellenar el Cuaderno",
        noted: "Anotado en tu Cuaderno",
        knows: ("está al tanto", "están al tanto"),
        see_document: "ver el documento",
    },
];

/// The names in `lang` (« fr-FR », « Deutsch »… as `souls::agent_language` reads them); English for another language.
pub fn names(lang: &str) -> &'static Names {
    let code = crate::souls::agent_language(Some(lang));
    NAMES.iter().find(|n| n.lang == code).unwrap_or(&NAMES[1])
}

/// The French names in the bridge's texts for the models, each with its counterpart: the longest first.
fn table(n: &'static Names) -> [(&'static str, &'static str); 14] {
    [
        ("Noté dans ton Carnet", n.noted),
        ("Remplir le Carnet", n.fill_notebook),
        ("|voir le document]]", n.see_document),
        ("Paiements par tes agents", n.agent_payments),
        ("Coffre des agents", n.agents_vault),
        ("Tâches de fond", n.background_tasks),
        ("Google Agenda", n.google_calendar),
        ("Boîtes mail", n.mailboxes),
        ("Coffre-fort", n.vault),
        ("Coordonnées", n.contact_details),
        ("« Archives »", n.archives),
        ("Réglages", n.settings),
        ("Rappels", n.reminders),
        ("Boîte", n.inbox),
    ]
}

/// The French names `localize` changes, as one pattern (whole words). « Carnet » apart: it is followed by what tells
/// a person's own French or Spanish words (« Carnet de vaccination », « carnet de conducir ») and the phone
/// reminders' mark (« BipAgents · Carnet ^t… », read back by the apps), which stay.
static FRENCH: LazyLock<Regex> = LazyLock::new(|| {
    let words: Vec<String> = table(&NAMES[0])
        .iter()
        .map(|(french, _)| {
            let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
            let (start, end) = (word(french.chars().next()), word(french.chars().last()));
            format!("{}{}{}", if start { r"\b" } else { "" }, regex::escape(french), if end { r"\b" } else { "" })
        })
        .collect();
    Regex::new(&format!(r"{}|\bCarnet\b(\s+(?:del?\b|d['’])|\s*\^)?", words.join("|"))).expect("valid regex")
});

/// The app's buttons and switches a text for the model quotes (« Essayer », « Qui peut écrire à qui »…), as each app
/// shows them (Localizable.xcstrings, values*/strings.xml), in `NAMES`' order: fr, en, de, es (Alex, 2026-11-01: an
/// English agent told of the card « Essayer / Plus tard / Pas pour moi » said it so). Only a whole quote changes:
/// « Santé » the switch, not « Domaines/Santé » a Carnet page.
pub const LABELS: [[&str; 4]; 10] = [
    ["Essayer / Plus tard / Pas pour moi", "Try it / Later / Not for me", "Ausprobieren / Später / Nichts für mich",
        "Probar / Más tarde / No es para mí"],
    ["Essayer", "Try it", "Ausprobieren", "Probar"],
    ["Plus tard", "Later", "Später", "Más tarde"],
    ["Pas pour moi", "Not for me", "Nichts für mich", "No es para mí"],
    ["Qui peut écrire à qui", "Who can write to whom", "Wer wem schreiben darf", "Quién puede escribir a quién"],
    ["Météo", "Weather", "Wetter", "Tiempo"],
    ["Santé", "Health", "Gesundheit", "Salud"],
    ["Envoyer", "Send", "Senden", "Enviar"],
    ["Modifier le mot de passe", "Change the password", "Passwort ändern", "Cambiar la contraseña"],
    ["Payer dans le navigateur de l'agent", "Pay in the agent’s browser", "Im Browser des Agenten bezahlen",
        "Pagar en el navegador del agente"],
];

/// A quoted label of `LABELS` (its spaces kept).
static QUOTED: LazyLock<Regex> = LazyLock::new(|| {
    let labels: Vec<String> = LABELS.iter().map(|l| regex::escape(l[0])).collect();
    Regex::new(&format!(r"«([\s\u{{a0}}\u{{202f}}]*)({})([\s\u{{a0}}\u{{202f}}]*)»", labels.join("|")))
        .expect("valid regex")
});

/// `text`, written for a model, with the app's names and quoted labels in `lang` (French: as it is).
pub fn localize(text: &str, lang: &str) -> String {
    let n = names(lang);
    if n.lang == "fr" || !(FRENCH.is_match(text) || QUOTED.is_match(text)) {
        return text.to_string();
    }
    let column = NAMES.iter().position(|x| x.lang == n.lang).unwrap_or(1);
    let text = QUOTED.replace_all(text, |c: &Captures| {
        let label = LABELS.iter().find(|l| l[0] == &c[2]).map_or(&c[2], |l| l[column]);
        format!("«{}{label}{}»", &c[1], &c[3])
    });
    FRENCH
        .replace_all(&text, |c: &Captures| {
            let found = &c[0];
            if c.get(1).is_some() {
                return found.to_string(); // « Carnet de … », the reminders' mark
            }
            if found == "Carnet" {
                return n.notebook.to_string();
            }
            match table(n).iter().find(|(french, _)| *french == found) {
                Some(("« Archives »", name)) => format!("« {name} »"),
                Some(("|voir le document]]", name)) => format!("|{name}]]"),
                Some((_, name)) => name.to_string(),
                None => found.to_string(),
            }
        })
        .into_owned()
}

/// Every description of a tool list (`tools/list`), its parameters' too, through `localize`.
pub fn localize_tools(tools: &mut [serde_json::Value], lang: &str) {
    fn walk(value: &mut serde_json::Value, lang: &str) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, v) in map.iter_mut() {
                    match v {
                        serde_json::Value::String(s) if key == "description" => *s = localize(s, lang),
                        _ => walk(v, lang),
                    }
                }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(|v| walk(v, lang)),
            _ => {}
        }
    }
    if names(lang).lang != "fr" {
        tools.iter_mut().for_each(|t| walk(t, lang));
    }
}

/// A tool's result (`{"content": [{"type": "text", "text"}…]}`): its texts through `localize`.
pub fn localize_result(result: &mut crate::Dict, lang: &str) {
    if names(lang).lang == "fr" {
        return;
    }
    if let Some(serde_json::Value::Array(items)) = result.get_mut("content") {
        for item in items {
            if let Some(serde_json::Value::String(text)) = item.get_mut("text") {
                *text = localize(text, lang);
            }
        }
    }
}

/// The language block's sentence on names, for an agent writing in another language than French (souls.rs).
pub fn rule(lang: &str) -> Option<String> {
    let n = names(lang);
    (n.lang != "fr").then(|| {
        format!(
            "Call the app's parts by the names your person sees in {}: the {}, the {}, {}, {}, the {}, {}; never by \
             another language's names, even when a tool or a page uses them. Your person's own pages and folders keep \
             their exact names in tool calls; in your words, say them in your person's language.",
            crate::souls::language_name(n.lang),
            n.notebook,
            n.inbox,
            n.background_tasks,
            n.reminders,
            n.vault,
            n.settings
        )
    })
}

/// The agent's language (its personality's language block, else the place's): « fr », « en », « de », « es »…
pub fn agent_language(config: &crate::config::Config, agent: &str) -> String {
    match config.agent(Some(agent)) {
        Some(a) => crate::handback::language_of(config, a),
        None => crate::notebook::language(config),
    }
}

/// What must never reach a person who doesn't read French: the French product names (whole words, case kept:
/// « Agenda », « Salon » or « Archives » are English too, « carnet » Spanish). The apps' strings have their own check
/// (scripts/french-names-check.py).
#[cfg(test)]
pub const FRENCH_NAMES: [&str; 14] = [
    "Carnet",
    "Boîte",
    "Tâches de fond",
    "Tâche de fond",
    "Rappels",
    "Réglages",
    "Coffre",
    "Coordonnées",
    "veilleur",
    "Fiches",
    "Noté dans",
    "voir le document",
    "Google Agenda",
    "Paiements par",
];

/// The `FRENCH_NAMES` found in `text`.
#[cfg(test)]
pub fn french_in(text: &str) -> Vec<&'static str> {
    FRENCH_NAMES
        .iter()
        .filter(|w| Regex::new(&format!(r"(?:^|[^\w-]){}(?:[^\w-]|$)", regex::escape(w))).unwrap().is_match(text))
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_in_each_language() {
        assert_eq!(names("en").notebook, "Notebook");
        assert_eq!(names("de-DE").notebook, "Notizbuch");
        assert_eq!(names("Español").notebook, "Cuaderno");
        assert_eq!(names("fr").notebook, "Carnet");
        assert_eq!(names("nl").notebook, "Notebook", "another language: English");
        let text = "Filed in the Carnet. The person turns it on in the app (Réglages › Coffre des agents); \
                    they follow it in « Tâches de fond », in their Boîte, or « Archives ». See your settings › Boîtes \
                    mail and « Coffre-fort » → « Coordonnées ». Their Rappels. e.g. « Noté dans ton Carnet : x ». \
                    [[archive:r1|voir le document]] Google Agenda";
        assert_eq!(
            localize(text, "en"),
            "Filed in the Notebook. The person turns it on in the app (Settings › Agents’ vault); they follow it in \
             « Background tasks », in their Inbox, or « Archives ». See your settings › Mailboxes and « Vault » → \
             « Contact details ». Their Reminders. e.g. « Noted in your Notebook : x ». [[archive:r1|see the \
             document]] Google Calendar"
        );
        assert!(localize(text, "de").contains("in the Notizbuch") && localize(text, "de").contains("« Archive »"));
        assert!(localize(text, "es").contains("« Tareas en segundo plano »"));
        assert_eq!(localize(text, "fr"), text);
        for lang in ["en", "de", "es"] {
            assert_eq!(french_in(&localize(text, lang)), Vec::<&str>::new(), "{lang}");
        }
    }

    #[test]
    fn the_persons_own_words_stay() {
        for kept in ["Carnet de vaccination", "Carnet del club", "Carnet d'adresses", "BipAgents · Carnet ^t12ab34cd"]
        {
            assert_eq!(localize(kept, "en"), kept);
        }
        assert_eq!(localize("Carnets, Carnet's page, MonCarnet", "en"), "Carnets, Notebook's page, MonCarnet");
    }

    #[test]
    fn the_quoted_labels() {
        let text = "A card « Essayer » / « Plus tard » / « Pas pour moi » waits. Allow it under « Qui peut écrire à \
                    qui »; switch « Météo » or « Santé » on; it waits for their « Envoyer » (« Modifier le mot de \
                    passe », « Payer dans le navigateur de l'agent »). Their page « Domaines/Santé », « Envoyer \
                    avec… ».";
        assert_eq!(
            localize(text, "en"),
            "A card « Try it » / « Later » / « Not for me » waits. Allow it under « Who can write to whom »; switch \
             « Weather » or « Health » on; it waits for their « Send » (« Change the password », « Pay in the agent’s \
             browser »). Their page « Domaines/Santé », « Envoyer avec… »."
        );
        let german = localize(text, "de");
        assert!(german.contains("« Ausprobieren » / « Später » / « Nichts für mich »"), "{german}");
        assert!(german.contains("« Wer wem schreiben darf »"), "{german}");
        assert!(localize(text, "es").contains("« Probar » / « Más tarde » / « No es para mí »"));
        assert_eq!(localize(text, "fr"), text);
        assert_eq!(localize("«\u{a0}Essayer\u{a0}»", "en"), "«\u{a0}Try it\u{a0}»", "its spaces kept");
        assert_eq!(
            localize("A card « Essayer / Plus tard / Pas pour moi » waits.", "es"),
            "A card « Probar / Más tarde / No es para mí » waits."
        );
    }

    #[test]
    fn tool_lists_and_results() {
        let mut tools = vec![serde_json::json!({"name": "notebook_note", "description": "File it in the Carnet.",
            "inputSchema": {"properties": {"page": {"type": "string", "description": "a Carnet page"}}}})];
        localize_tools(&mut tools, "de");
        assert_eq!(tools[0]["description"], "File it in the Notizbuch.");
        assert_eq!(tools[0]["inputSchema"]["properties"]["page"]["description"], "a Notizbuch page");
        assert_eq!(tools[0]["name"], "notebook_note", "an identifier");
        let mut result = crate::tools_knowledge::tool_text("Nothing found in the Carnet.", false);
        localize_result(&mut result, "es");
        assert_eq!(result["content"][0]["text"], "Nothing found in the Cuaderno.");
        let mut french = crate::tools_knowledge::tool_text("Nothing found in the Carnet.", false);
        localize_result(&mut french, "fr");
        assert_eq!(french["content"][0]["text"], "Nothing found in the Carnet.");
    }

    #[test]
    fn the_rule_names_no_french() {
        assert_eq!(rule("fr"), None);
        for lang in ["en", "de", "es", "nl"] {
            let text = rule(lang).unwrap();
            assert_eq!(french_in(&text), Vec::<&str>::new(), "{text}");
            assert!(text.contains(names(lang).notebook), "{text}");
        }
    }
}
