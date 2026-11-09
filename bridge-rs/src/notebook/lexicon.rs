//! The Carnet's small lexicon (Carnet v2, socle §0 of internal/notes/carnet-v2/spec.md), in the four languages, all
//! folded (`fold`: lower case, no accents): what a « Libellé : valeur » fact's label means (`LABELS`, used at the
//! write to reshape « Chausse du 36 » into « Pointure : 36 » and to replace a label said again, and at the search to
//! answer « Léa chausse du combien ? »), the words a search drops (`STOP`), and the words that make a fact a health
//! fact (`is_health`: never in the shared part) or a money one (`is_money`). Plus the checks of what is never kept (ID
//! numbers) and the person's contact forms (e-mail, phone in E.164).

use super::fold;

/// A label of a short fact, its name in fr, en, de, es (the Carnet's language picks one), and the phrases that mean
/// it (folded). `joins`: phrases that start a fact and lead straight to its value (« chausse du 36 »); a label without
/// any is never written in its form by the bridge (`reshape`), only recognised.
pub struct Label {
    pub id: &'static str,
    pub names: [&'static str; 4],
    pub words: &'static [&'static str],
    pub joins: &'static [&'static str],
    /// A health label (its facts go to the private health part).
    pub health: bool,
}

const fn label(
    id: &'static str,
    names: [&'static str; 4],
    words: &'static [&'static str],
    joins: &'static [&'static str],
) -> Label {
    Label { id, names, words, joins, health: false }
}

const fn health_label(
    id: &'static str,
    names: [&'static str; 4],
    words: &'static [&'static str],
    joins: &'static [&'static str],
) -> Label {
    Label { id, names, words, joins, health: true }
}

/// About thirty labels, those people restate (one value each: a new one replaces the old).
pub const LABELS: &[Label] = &[
    label(
        "shoe_size",
        ["Pointure", "Shoe size", "Schuhgröße", "Talla de zapatos"],
        &["pointure", "chausse", "taille de chaussures", "shoe size", "schuhgrosse", "talla de zapatos", "calza"],
        &["chausse du", "chausse", "taille de chaussures", "wears size", "shoe size", "schuhgrosse", "calza el", "calza un", "calza"],
    ),
    label(
        "birthday",
        ["Anniversaire", "Birthday", "Geburtstag", "Cumpleaños"],
        &["anniversaire", "anniv", "date de naissance", "ne le", "nee le", "birthday", "date of birth", "born on",
          "geburtstag", "geboren am", "cumpleanos", "fecha de nacimiento", "nacio el", "nacida el", "nacido el"],
        &["ne le", "nee le", "date de naissance", "born on", "date of birth", "geboren am", "nacio el", "nacida el", "nacido el"],
    ),
    label(
        "height",
        ["Taille", "Height", "Größe", "Altura"],
        &["taille", "mesure", "height", "tall", "grosse", "altura", "mide"],
        &["mesure", "is tall", "ist gross"],
    ),
    label(
        "clothing_size",
        ["Taille de vêtements", "Clothing size", "Kleidergröße", "Talla de ropa"],
        &["taille de vetements", "taille vetements", "clothing size", "clothes size", "kleidergrosse", "konfektionsgrosse", "talla de ropa"],
        &["taille de vetements", "clothing size", "kleidergrosse", "talla de ropa"],
    ),
    label(
        "ring_size",
        ["Tour de doigt", "Ring size", "Ringgröße", "Talla de anillo"],
        &["tour de doigt", "taille de bague", "ring size", "ringgrosse", "talla de anillo"],
        &["tour de doigt", "taille de bague", "ring size", "ringgrosse", "talla de anillo"],
    ),
    label(
        "name_day",
        ["Fête", "Name day", "Namenstag", "Santo"],
        &["fete", "name day", "namenstag", "santo", "onomastica"],
        &["fete le", "fetee le", "name day on", "namenstag am", "su santo es el"],
    ),
    label(
        "wedding",
        ["Anniversaire de mariage", "Wedding anniversary", "Hochzeitstag", "Aniversario de boda"],
        &["anniversaire de mariage", "mariage", "marie le", "maries le", "wedding anniversary", "wedding", "married on",
          "hochzeitstag", "hochzeit", "aniversario de boda", "boda", "casados el"],
        &["maries le", "marie le", "mariee le", "married on", "verheiratet seit", "casados el"],
    ),
    label(
        "phone",
        ["Téléphone", "Phone", "Telefon", "Teléfono"],
        &["telephone", "tel", "portable", "phone", "mobile", "cell", "telefon", "handy", "handynummer", "telefono", "movil", "celular"],
        &[],
    ),
    label(
        "email",
        ["E-mail", "E-mail", "E-Mail", "Correo"],
        &["e mail", "email", "mail", "courriel", "adresse mail", "adresse e mail", "correo", "correo electronico"],
        &[],
    ),
    label(
        "address",
        ["Adresse", "Address", "Adresse", "Dirección"],
        &["adresse", "address", "anschrift", "direccion", "domicilio"],
        &[],
    ),
    label(
        "city",
        ["Ville", "City", "Stadt", "Ciudad"],
        &["ville", "city", "stadt", "wohnort", "ciudad"],
        &[],
    ),
    label(
        "job",
        ["Métier", "Job", "Beruf", "Profesión"],
        &["metier", "profession", "job", "occupation", "beruf", "profesion", "oficio", "trabajo"],
        &[],
    ),
    label(
        "employer",
        ["Employeur", "Employer", "Arbeitgeber", "Empresa"],
        &["employeur", "employer", "arbeitgeber", "empresa"],
        &[],
    ),
    label(
        "partner",
        ["Conjoint", "Partner", "Partner", "Pareja"],
        &["conjoint", "conjointe", "compagnon", "compagne", "epoux", "epouse", "partner", "husband", "wife", "spouse",
          "ehemann", "ehefrau", "partnerin", "pareja", "marido", "esposa", "esposo"],
        &[],
    ),
    label(
        "children",
        ["Enfants", "Children", "Kinder", "Hijos"],
        &["enfants", "children", "kids", "kinder", "hijos"],
        &[],
    ),
    label(
        "pet",
        ["Animal", "Pet", "Haustier", "Mascota"],
        &["animal", "animal de compagnie", "pet", "haustier", "mascota"],
        &[],
    ),
    label(
        "favourite_colour",
        ["Couleur préférée", "Favourite colour", "Lieblingsfarbe", "Color favorito"],
        &["couleur preferee", "favourite colour", "favorite color", "lieblingsfarbe", "color favorito", "color preferido"],
        &["couleur preferee", "favourite colour", "favorite color", "lieblingsfarbe", "color favorito"],
    ),
    label(
        "favourite_food",
        ["Plat préféré", "Favourite food", "Lieblingsessen", "Comida favorita"],
        &["plat prefere", "favourite food", "favorite food", "favorite dish", "lieblingsessen", "lieblingsgericht", "comida favorita", "plato favorito"],
        &["plat prefere", "favourite food", "favorite food", "lieblingsessen", "comida favorita"],
    ),
    label(
        "favourite_drink",
        ["Boisson préférée", "Favourite drink", "Lieblingsgetränk", "Bebida favorita"],
        &["boisson preferee", "favourite drink", "favorite drink", "lieblingsgetrank", "bebida favorita"],
        &["boisson preferee", "favourite drink", "favorite drink", "lieblingsgetrank", "bebida favorita"],
    ),
    label(
        "diet",
        ["Régime alimentaire", "Diet", "Ernährung", "Dieta"],
        &["regime alimentaire", "regime", "diet", "ernahrung", "dieta"],
        &[],
    ),
    label(
        "school",
        ["École", "School", "Schule", "Colegio"],
        &["ecole", "college", "lycee", "school", "schule", "colegio", "escuela"],
        &[],
    ),
    label(
        "class",
        ["Classe", "School year", "Klasse", "Curso"],
        &["classe", "school year", "grade", "klasse", "curso"],
        &[],
    ),
    label(
        "car",
        ["Voiture", "Car", "Auto", "Coche"],
        &["voiture", "car", "auto", "coche"],
        &[],
    ),
    label(
        "language",
        ["Langue", "Language", "Sprache", "Idioma"],
        &["langue", "language", "sprache", "idioma"],
        &[],
    ),
    label(
        "nickname",
        ["Surnom", "Nickname", "Spitzname", "Apodo"],
        &["surnom", "nickname", "spitzname", "apodo"],
        &[],
    ),
    health_label(
        "allergies",
        ["Allergies", "Allergies", "Allergien", "Alergias"],
        &["allergie", "allergies", "allergique", "allergy", "allergies", "allergic", "allergien", "alergia", "alergias"],
        &[],
    ),
    health_label(
        "blood_type",
        ["Groupe sanguin", "Blood type", "Blutgruppe", "Grupo sanguíneo"],
        &["groupe sanguin", "blood type", "blood group", "blutgruppe", "grupo sanguineo"],
        &["groupe sanguin", "blood type", "blutgruppe", "grupo sanguineo"],
    ),
    health_label(
        "doctor",
        ["Médecin", "Doctor", "Arzt", "Médico"],
        &["medecin", "medecin traitant", "docteur", "doctor", "gp", "arzt", "hausarzt", "medico"],
        &[],
    ),
];

/// Words a search drops (any of the four languages: a question may mix them).
pub const STOP: &[&str] = &[
    // fr
    "de", "du", "la", "le", "les", "des", "d", "l", "un", "une", "au", "aux", "et", "a", "en", "pour", "sur", "ma", "mon",
    "mes", "ta", "ton", "tes", "sa", "son", "ses", "quel", "quelle", "quels", "quelles", "est", "c", "qu", "que", "qui",
    "quoi", "combien", "elle", "il",
    // en
    "the", "of", "an", "and", "to", "is", "what", "which", "who", "how", "many", "much", "my", "your", "his", "her",
    "does",
    // de
    "der", "die", "das", "den", "dem", "von", "ein", "eine", "und", "ist", "was", "wie", "welche",
    // es
    "el", "los", "las", "del", "y", "es", "cual", "cuanto",
];

/// Health words (folded): a word that starts with one of them, or a phrase; the stems end with `*`.
const HEALTH: &[&str] = &[
    // fr
    "allerg*", "medicament*", "traitement*", "maladi*", "malade*", "diagnost*", "symptom*", "ordonnance*", "medecin*",
    "hopital", "hopitaux", "chirurg*", "opere", "operee", "grossesse*", "enceinte", "diabet*", "asthm*", "cancer*",
    "depressi*", "therap*", "psychiatr*", "psycholog*", "vaccin*", "blessure*", "blesse", "blessee", "douleur*",
    "tension arterielle", "cholesterol", "migraine*", "epileps*", "insomni*", "kine", "kinesitherap*", "dentist*",
    "gyneco*", "sante", "handicap*", "groupe sanguin", "intoleran*", "coeliaq*", "perte de poids", "prise de poids",
    // en
    "medication*", "medicine*", "treatment*", "disease*", "illness*", "sick", "diagnos*", "prescription*", "doctor*",
    "hospital*", "surgery", "pregnan*", "asthma", "depress*", "injur*", "blood pressure", "migraines", "epilep*",
    "insomnia", "physio*", "health", "disabilit*", "blood type", "celiac", "weight loss",
    // de
    "medikament*", "behandlung*", "krankheit*", "krank", "arzt", "arztin", "krankenhaus*", "klinik*", "schwanger*",
    "krebs", "impf*", "verletz*", "schmerz*", "blutdruck", "cholesterin", "migran*", "schlaflos*", "zahnarzt*",
    "gesundheit*", "behinder*", "blutgruppe", "unvertraglich*",
    // es
    "alerg*", "tratamiento*", "enfermedad*", "enfermo", "enferma", "medico", "cirug*", "embaraz*", "asma", "vacun*",
    "dolor*", "salud",
];

/// Money words (folded), the same way; with `news::mentions_money` (an amount next to a currency).
const MONEY: &[&str] = &[
    "argent", "budget*", "salaire*", "loyer*", "credit*", "pret", "prets", "impot*", "banque*", "epargne*", "dette*",
    "facture*", "money", "salary", "rent", "loan*", "tax", "taxes", "bank*", "savings", "debt*", "invoice*",
    "mortgage*", "geld", "gehalt*", "miete*", "kredit*", "steuer*", "ersparn*", "schulden", "rechnung*", "dinero",
    "presupuesto*", "sueldo*", "salario*", "alquiler*", "prestamo*", "impuesto*", "banco*", "ahorro*", "deuda*",
    "factura*", "hipoteca*", "finance*",
];

/// Whether folded `text` holds one of `list`'s entries (a stem `x*` at a word's start, else the word or phrase).
fn holds(text: &str, list: &[&str]) -> bool {
    let padded = format!(" {} ", fold(text));
    list.iter().any(|entry| match entry.strip_suffix('*') {
        Some(stem) => padded.contains(&format!(" {stem}")),
        None => padded.contains(&format!(" {entry} ")),
    })
}

/// A health fact (body, care, treatments, illnesses, pregnancy…): wherever it would be written, it goes to the
/// private health part (or « À ranger » without a health agent), never to the shared part.
pub fn is_health(text: &str) -> bool {
    holds(text, HEALTH) || split_label(text).and_then(|(l, _)| label_of(&l)).is_some_and(|l| l.health)
}

/// A money fact (budget, salary, rent, loans…, or an amount).
pub fn is_money(text: &str) -> bool {
    holds(text, MONEY) || super::news::mentions_money(text)
}

/// The label a (folded or not) label text names: one of `LABELS` by its names in any language or its words.
pub fn label_of(text: &str) -> Option<&'static Label> {
    let folded = fold(text);
    if folded.is_empty() {
        return None;
    }
    LABELS.iter().find(|l| l.names.iter().any(|n| fold(n) == folded) || l.words.contains(&folded.as_str()))
}

/// A label's name in the Carnet's language.
pub fn name_of(label: &Label, lang: &str) -> &'static str {
    label.names[super::lang_index(lang).min(3)]
}

/// « Pointure : 36 » → (« Pointure », « 36 »): a short label (4 words, 30 characters at most, no digit) before the
/// first colon, a value after it.
pub fn split_label(fact: &str) -> Option<(String, String)> {
    let (label, value) = fact.split_once(':')?;
    let (label, value) = (label.trim(), value.trim());
    let fine = !label.is_empty()
        && !value.is_empty()
        && label.chars().count() <= 30
        && label.split_whitespace().count() <= 4
        && !label.chars().any(|c| c.is_ascii_digit())
        && !label.contains("[[")
        && !label.contains("http");
    fine.then(|| (label.to_string(), value.to_string()))
}

/// The key a fact's label is compared by: the table's id when it is one of `LABELS`, else the folded label.
pub fn label_key(label: &str) -> String {
    label_of(label).map(|l| l.id.to_string()).unwrap_or_else(|| fold(label))
}

/// A fact put in the « Libellé : valeur » form when it says a label of the table in words: « Chausse du 36 » →
/// « Pointure : 36 », « Pointure 36 » → « Pointure : 36 ». Anything else as it was.
pub fn reshape(fact: &str, lang: &str) -> String {
    let fact = fact.trim();
    if split_label(fact).is_some() {
        return fact.to_string();
    }
    let words: Vec<&str> = fact.split_whitespace().collect();
    if words.len() > 8 {
        return fact.to_string();
    }
    // only the labels of a measure or a date (« Pointure », « Anniversaire »…): a job or a city said in words stays as
    // it is said, where the outline files it
    for l in LABELS.iter().filter(|l| !l.joins.is_empty()) {
        let mut starts: Vec<String> = l.joins.iter().map(|j| j.to_string()).collect();
        starts.extend(l.names.iter().map(|n| fold(n)));
        starts.sort_by_key(|s| std::cmp::Reverse(s.split(' ').count()));
        for start in &starts {
            let n = start.split(' ').count();
            if words.len() <= n || fold(&words[..n].join(" ")) != *start {
                continue;
            }
            let value = words[n..].join(" ");
            let value = value.trim_start_matches([':', '-', '–']).trim();
            // a value, not a sentence going on (« Taille la haie samedi » is not a height)
            let short = value.split_whitespace().count() <= 4;
            let measured = value.chars().any(|c| c.is_ascii_digit()) || l.joins.contains(&start.as_str());
            if !value.is_empty() && short && measured {
                return format!("{} : {value}", name_of(l, lang));
            }
        }
    }
    fact.to_string()
}

/// The words of a question to look for (folded, the stop words left out), and the labels it asks about (« chausse
/// du combien » → the shoe size).
pub fn query(text: &str) -> (Vec<String>, Vec<&'static str>) {
    let mut folded = format!(" {} ", fold(text));
    let mut labels: Vec<&'static str> = Vec::new();
    // the longest phrases first: « taille de chaussures » before « taille »
    let mut phrases: Vec<(&'static str, &'static str)> = Vec::new();
    for l in LABELS {
        for w in l.words.iter().chain(l.joins.iter()) {
            phrases.push((w, l.id));
        }
    }
    phrases.sort_by_key(|(w, _)| std::cmp::Reverse(w.len()));
    for (phrase, id) in phrases {
        let padded = format!(" {phrase} ");
        if folded.contains(&padded) {
            folded = folded.replace(&padded, " ");
            if !labels.contains(&id) {
                labels.push(id);
            }
        }
    }
    let words = folded.split_whitespace().filter(|w| !STOP.contains(w)).map(str::to_string).take(8).collect();
    (words, labels)
}

/// An ID number in a fact: never kept (passports, ID cards, social security, bank cards, IBAN). Phone numbers and
/// dates pass.
pub fn id_number(text: &str) -> bool {
    const KEYS: &[&str] = &[
        "passeport", "passport", "reisepass", "pasaporte", "carte d identite", "carte identite", "cni", "id card",
        "identity card", "personalausweis", "ausweis", "dni", "nie", "permis de conduire", "driving licence",
        "driver s license", "fuhrerschein", "carnet de conducir", "securite sociale", "secu", "social security", "ssn",
        "sozialversicherung", "seguridad social", "nir", "iban", "carte bancaire", "card number", "kreditkarte",
        "tarjeta", "numero fiscal", "tax id", "steuer id", "steuernummer", "nif", "mrz",
    ];
    // a long run of digits (spaces between groups): a card, a social security number, an account
    let mut digits = 0;
    let mut longest = 0;
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if c.is_ascii_digit() {
            digits += 1;
            longest = longest.max(digits);
        } else if *c == ' ' && i > 0 && chars[i - 1].is_ascii_digit() && chars.get(i + 1).is_some_and(char::is_ascii_digit) {
            continue;
        } else {
            digits = 0;
        }
    }
    if longest >= 13 {
        return true;
    }
    // an IBAN: two letters, two digits, then 11 or more letters and digits
    let squeezed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let up = squeezed.to_uppercase();
    let b = up.as_bytes();
    for i in 0..b.len().saturating_sub(15) {
        let head = b[i].is_ascii_uppercase() && b[i + 1].is_ascii_uppercase() && b[i + 2].is_ascii_digit() && b[i + 3].is_ascii_digit();
        if head && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) && b[i + 4..].iter().take_while(|c| c.is_ascii_alphanumeric()).count() >= 11 {
            return true;
        }
    }
    // with an ID word: a code of 6 characters or more with 4 digits or more (« 12AB34567 », « 12 AB 34567 »)
    if !holds(text, KEYS) {
        return false;
    }
    let mut run = String::new();
    let mut runs: Vec<String> = Vec::new();
    for c in text.chars().chain(std::iter::once('.')) {
        if c.is_ascii_alphanumeric() || (c == ' ' && !run.is_empty()) {
            run.push(c);
        } else {
            runs.push(std::mem::take(&mut run));
        }
    }
    runs.iter().flat_map(|r| {
        // a run of words: its codes are the stretches of short groups that hold digits
        let mut out: Vec<String> = Vec::new();
        let mut code = String::new();
        for group in r.split(' ') {
            let coded = group.chars().any(|c| c.is_ascii_digit()) || (group.len() <= 3 && group.chars().all(|c| c.is_ascii_uppercase()) && !group.is_empty());
            if coded {
                code.push_str(group);
            } else {
                out.push(std::mem::take(&mut code));
            }
        }
        out.push(code);
        out
    })
    .any(|code| code.len() >= 6 && code.chars().filter(char::is_ascii_digit).count() >= 4 && !looks_like_date(&code))
}

fn looks_like_date(code: &str) -> bool {
    code.len() == 8 && code.chars().all(|c| c.is_ascii_digit()) && (code.starts_with("19") || code.starts_with("20"))
}

/// An e-mail address, lower case (None: not one).
pub fn email(text: &str) -> Option<String> {
    let t = text.trim().trim_matches(['<', '>']).to_lowercase();
    let (user, domain) = t.split_once('@')?;
    let fine = !user.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !t.chars().any(char::is_whitespace);
    fine.then_some(t)
}

/// A phone number in E.164 (« +33612345678 »), national numbers by the Carnet's language (fr +33, de +49, es +34, en
/// +44). None: not a phone number.
pub fn phone(text: &str, lang: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit() || " +.-()/".contains(c)) {
        return None;
    }
    let digits: String = t.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < 8 || digits.len() > 15 {
        return None;
    }
    if t.starts_with('+') {
        return Some(format!("+{digits}"));
    }
    if let Some(rest) = digits.strip_prefix("00") {
        return Some(format!("+{rest}"));
    }
    let national = |code: &str, len: std::ops::RangeInclusive<usize>| {
        (digits.starts_with('0') && len.contains(&digits.len())).then(|| format!("+{code}{}", &digits[1..]))
    };
    match lang {
        "fr" => national("33", 10..=10),
        "de" => national("49", 10..=12),
        "en" => national("44", 11..=11),
        "es" => (digits.len() == 9 && digits.starts_with(['6', '7', '8', '9'])).then(|| format!("+34{digits}")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_facts_take_the_label_form() {
        assert_eq!(reshape("Chausse du 36", "fr"), "Pointure : 36");
        assert_eq!(reshape("Pointure 36", "fr"), "Pointure : 36");
        assert_eq!(reshape("Pointure : 37", "fr"), "Pointure : 37");
        assert_eq!(reshape("Née le 12 mars 1990", "fr"), "Anniversaire : 12 mars 1990");
        assert_eq!(reshape("Wears size 9", "en"), "Shoe size : 9");
        assert_eq!(reshape("Aime le jazz", "fr"), "Aime le jazz");
        assert_eq!(reshape("Taille la haie samedi avec son voisin", "fr"), "Taille la haie samedi avec son voisin");
        assert_eq!(label_key("Pointure"), "shoe_size");
        assert_eq!(label_key("Taille de chaussures"), "shoe_size");
        assert_eq!(label_key("Couleur du vélo"), "couleur du velo");
        assert!(split_label("Devis plombier accepté le 22/10 : 480 €").is_none());
        assert!(split_label("[[Léa]] : sœur").is_none());
    }

    #[test]
    fn a_question_gives_its_words_and_labels() {
        let (words, labels) = query("Léa chausse du combien ?");
        assert_eq!(words, ["lea"]);
        assert_eq!(labels, ["shoe_size"]);
        let (words, labels) = query("What is the shoe size of Léa");
        assert_eq!((words, labels), (vec!["lea".to_string()], vec!["shoe_size"]));
        assert_eq!(query("anniversaire de Marc").1, ["birthday"]);
    }

    #[test]
    fn health_and_money_words() {
        for fact in ["Allergique à la pénicilline", "Allergies : arachides", "Sous traitement pour la thyroïde",
                     "Enceinte de 4 mois", "Allergic to peanuts", "Hat Diabetes", "Alergia al polen", "Médecin : Dr Roux"] {
            assert!(is_health(fact), "{fact}");
        }
        for fact in ["Aime le jazz", "Pointure : 36", "Chat : Minou", "Pain au chocolat le dimanche", "Joue au tennis"] {
            assert!(!is_health(fact), "{fact}");
        }
        assert!(is_money("Loyer : 950 €") && is_money("Budget vacances 2 000 €") && is_money("Rent is due"));
        assert!(!is_money("Rentrée le 2 septembre"));
    }

    #[test]
    fn id_numbers_are_caught_phones_and_dates_pass() {
        for fact in ["Passeport : 12AB34567", "Numéro de passeport 12 AB 34567", "IBAN FR76 3000 6000 0112 3456 7890 189",
                     "Carte 4970 1012 3456 7890", "Sécu : 1 85 05 78 006 084 36"] {
            assert!(id_number(fact), "{fact}");
        }
        for fact in ["Téléphone : 06 12 34 56 78", "Passeport valable jusqu'au 2031-03-12", "Passeport expire le 12/03/2031",
                     "Anniversaire : 14 novembre", "Code postal 69003", "+33 6 12 34 56 78"] {
            assert!(!id_number(fact), "{fact}");
        }
    }

    #[test]
    fn contact_forms() {
        assert_eq!(email(" Lea@Example.org ").as_deref(), Some("lea@example.org"));
        assert_eq!(email("léa chez gmail"), None);
        assert_eq!(phone("06 12 34 56 78", "fr").as_deref(), Some("+33612345678"));
        assert_eq!(phone("+33 6 12 34 56 78", "en").as_deref(), Some("+33612345678"));
        assert_eq!(phone("0033612345678", "de").as_deref(), Some("+33612345678"));
        assert_eq!(phone("612 345 678", "es").as_deref(), Some("+34612345678"));
        assert_eq!(phone("C-1", "fr"), None);
        assert_eq!(phone("12", "fr"), None);
    }
}
