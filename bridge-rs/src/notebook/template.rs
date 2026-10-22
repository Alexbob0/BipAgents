//! The pages' fixed outlines (the owner, 2026-10-22: « chaque sujet fait fouillis d'informations diverses »): every
//! page of a kind has the same sections in the same order, so the person learns it once. A person's page: « En bref »,
//! then « Qui c'est », « Dates », « Famille et proches », « Travail », « Goûts et préférences », « Idées cadeaux »,
//! « Ce que vous avez convenu », « Derniers échanges », « À ne pas oublier »; a subject's: « Où on en est »,
//! « Décisions », « Prochaines étapes », « Personnes », « Chiffres et budget », « Documents », « Historique »; an
//! area's: « Ce qui compte », « Habitudes et règles », « Contacts utiles », « En cours ». What fits nowhere goes to
//! « Autres », last, which the nightly pass sorts a little each night.
//!
//! The agents name a section by its id (`dates`, `decisions`…, the tool's enum); a near name, an old one (« Infos »,
//! « Échanges récents ») or another kind's id is mapped here, a generic or unknown one sends each fact to the section
//! its words point to (`guess`, no model), else to « Autres ». `normalize` puts a page's sections in their order, its
//! facts where they belong, and drops the empty ones; `trim_logs` keeps the latest lines of the dated logs (« Derniers
//! échanges », « Historique »), the older ones staying in the page's history.

use std::sync::LazyLock;

use regex::Regex;

use super::{fold, split_attribution};
use crate::tools_knowledge as tk;

/// One section of an outline: its id (what the agents and the apps use), its title in fr/en/de/es, and other names it
/// is known by (folded; any language).
pub struct Section {
    pub id: &'static str,
    pub titles: [&'static str; 4],
    pub aliases: &'static [&'static str],
}

const OTHER: Section = Section {
    id: "other",
    titles: ["Autres", "Other", "Sonstiges", "Otros"],
    aliases: &["autre", "others", "divers", "misc", "sonstige", "otro", "varios"],
};

pub const PERSON: [Section; 10] = [
    Section {
        id: "who",
        titles: ["Qui c'est", "Who they are", "Wer das ist", "Quién es"],
        aliases: &[
            "qui c est",
            "qui",
            "coordonnees",
            "contact",
            "contacts",
            "identite",
            "relation",
            "lien",
            "rencontre",
            "who",
            "about",
            "contact details",
            "how we met",
            "wer",
            "kontakt",
            "kontaktdaten",
            "beziehung",
            "quien",
            "contacto",
            "datos de contacto",
            "relacion",
        ],
    },
    Section {
        id: "dates",
        titles: ["Dates", "Dates", "Wichtige Daten", "Fechas"],
        aliases: &[
            "date",
            "dates importantes",
            "anniversaire",
            "anniversaires",
            "fetes",
            "birthday",
            "birthdays",
            "important dates",
            "daten",
            "geburtstag",
            "geburtstage",
            "jahrestage",
            "fecha",
            "fechas importantes",
            "cumpleanos",
        ],
    },
    Section {
        id: "family",
        titles: ["Famille et proches", "Family and close ones", "Familie und Nahestehende", "Familia y allegados"],
        aliases: &[
            "famille",
            "proches",
            "entourage",
            "enfants",
            "family",
            "relatives",
            "close ones",
            "kids",
            "children",
            "familie",
            "kinder",
            "angehorige",
            "familia",
            "allegados",
            "hijos",
        ],
    },
    Section {
        id: "work",
        titles: ["Travail", "Work", "Arbeit", "Trabajo"],
        aliases: &[
            "pro",
            "professionnel",
            "metier",
            "boulot",
            "emploi",
            "carriere",
            "job",
            "career",
            "professional",
            "beruf",
            "arbeitsplatz",
            "trabajo",
            "profesion",
            "empleo",
        ],
    },
    Section {
        id: "tastes",
        titles: ["Goûts et préférences", "Likes and preferences", "Vorlieben", "Gustos y preferencias"],
        aliases: &[
            "gouts",
            "preferences",
            "aime",
            "aime pas",
            "alimentation",
            "cuisine",
            "likes",
            "tastes",
            "food",
            "likes and dislikes",
            "vorlieben und abneigungen",
            "geschmack",
            "essen",
            "gustos",
            "preferencias",
            "comida",
        ],
    },
    Section {
        id: "gifts",
        titles: ["Idées cadeaux", "Gift ideas", "Geschenkideen", "Ideas de regalo"],
        aliases: &[
            "cadeaux",
            "cadeau",
            "idees de cadeaux",
            "gifts",
            "gift",
            "geschenke",
            "geschenk",
            "regalos",
            "regalo",
        ],
    },
    Section {
        id: "agreed",
        titles: ["Ce que vous avez convenu", "What you agreed", "Was ihr vereinbart habt", "Lo que habéis acordado"],
        aliases: &[
            "accords",
            "accord",
            "convenu",
            "promesses",
            "prets",
            "rendez vous",
            "engagements",
            "agreements",
            "agreed",
            "promises",
            "loans",
            "appointments",
            "vereinbarungen",
            "vereinbart",
            "versprechen",
            "acuerdos",
            "acordado",
            "promesas",
            "prestamos",
            "citas",
        ],
    },
    Section {
        id: "exchanges",
        titles: ["Derniers échanges", "Latest exchanges", "Letzte Kontakte", "Últimos contactos"],
        aliases: &[
            "echanges",
            "echanges recents",
            "historique",
            "conversations",
            "nouvelles",
            "recent exchanges",
            "exchanges",
            "history",
            "news",
            "conversations",
            "austausch",
            "letzter austausch",
            "verlauf",
            "neuigkeiten",
            "intercambios",
            "conversaciones",
            "historial",
            "novedades",
            "ultimos intercambios",
        ],
    },
    Section {
        id: "remember",
        titles: ["À ne pas oublier", "Don't forget", "Nicht vergessen", "No olvidar"],
        aliases: &[
            "a ne pas oublier",
            "a retenir",
            "important",
            "rappels",
            "attention",
            "remember",
            "dont forget",
            "to remember",
            "wichtig",
            "merken",
            "no olvidar",
            "recordar",
            "importante",
        ],
    },
    OTHER,
];

pub const SUBJECT: [Section; 8] = [
    Section {
        id: "status",
        titles: ["Où on en est", "Where it stands", "Stand der Dinge", "Cómo va"],
        aliases: &[
            "ou on en est",
            "statut",
            "etat",
            "avancement",
            "situation",
            "status",
            "progress",
            "state",
            "where it stands",
            "stand",
            "fortschritt",
            "estado",
            "como va",
            "situacion",
        ],
    },
    Section {
        id: "decisions",
        titles: ["Décisions", "Decisions", "Entscheidungen", "Decisiones"],
        aliases: &[
            "decision",
            "choix",
            "accords",
            "decided",
            "choices",
            "entscheidung",
            "beschlusse",
            "decision",
            "acuerdos",
        ],
    },
    Section {
        id: "next",
        titles: ["Prochaines étapes", "Next steps", "Nächste Schritte", "Próximos pasos"],
        aliases: &[
            "prochaines etapes",
            "etapes",
            "a faire",
            "todo",
            "suite",
            "next steps",
            "next",
            "to do",
            "steps",
            "nachste schritte",
            "zu tun",
            "proximos pasos",
            "pendiente",
            "por hacer",
        ],
    },
    Section {
        id: "people",
        titles: ["Personnes", "People", "Personen", "Personas"],
        aliases: &[
            "personnes",
            "intervenants",
            "contacts",
            "qui",
            "equipe",
            "participants",
            "people",
            "contacts",
            "who",
            "team",
            "personen",
            "beteiligte",
            "kontakte",
            "personas",
            "contactos",
            "equipo",
        ],
    },
    Section {
        id: "budget",
        titles: ["Chiffres et budget", "Figures and budget", "Zahlen und Budget", "Cifras y presupuesto"],
        aliases: &[
            "chiffres",
            "budget",
            "couts",
            "cout",
            "prix",
            "argent",
            "devis",
            "depenses",
            "figures",
            "costs",
            "money",
            "price",
            "quotes",
            "zahlen",
            "kosten",
            "geld",
            "preise",
            "cifras",
            "presupuesto",
            "costes",
            "dinero",
            "precios",
        ],
    },
    Section {
        id: "documents",
        titles: ["Documents", "Documents", "Dokumente", "Documentos"],
        aliases: &[
            "document",
            "fichiers",
            "liens",
            "pieces",
            "files",
            "links",
            "dokumente",
            "dateien",
            "documentos",
            "archivos",
            "enlaces",
        ],
    },
    Section {
        id: "history",
        titles: ["Historique", "History", "Verlauf", "Historial"],
        aliases: &[
            "historique",
            "journal",
            "chronologie",
            "echanges",
            "echanges recents",
            "history",
            "log",
            "timeline",
            "verlauf",
            "chronik",
            "historial",
            "cronologia",
        ],
    },
    OTHER,
];

pub const AREA: [Section; 5] = [
    Section {
        id: "matters",
        titles: ["Ce qui compte", "What matters", "Was zählt", "Lo que importa"],
        aliases: &[
            "ce qui compte",
            "essentiel",
            "important",
            "priorites",
            "what matters",
            "priorities",
            "wichtig",
            "was zahlt",
            "lo que importa",
            "prioridades",
        ],
    },
    Section {
        id: "habits",
        titles: ["Habitudes et règles", "Habits and rules", "Gewohnheiten und Regeln", "Hábitos y reglas"],
        aliases: &[
            "habitudes",
            "regles",
            "routines",
            "preferences",
            "organisation",
            "habits",
            "rules",
            "routine",
            "preferences",
            "gewohnheiten",
            "regeln",
            "vorlieben",
            "habitos",
            "reglas",
            "rutinas",
            "preferencias",
        ],
    },
    Section {
        id: "contacts",
        titles: ["Contacts utiles", "Useful contacts", "Nützliche Kontakte", "Contactos útiles"],
        aliases: &[
            "contacts",
            "contacts utiles",
            "numeros",
            "personnes",
            "contacts",
            "people",
            "kontakte",
            "personen",
            "contactos",
            "personas",
        ],
    },
    Section {
        id: "ongoing",
        titles: ["En cours", "In progress", "Laufend", "En curso"],
        aliases: &[
            "en cours",
            "projets",
            "a faire",
            "suivi",
            "in progress",
            "ongoing",
            "projects",
            "laufend",
            "projekte",
            "en curso",
            "proyectos",
        ],
    },
    OTHER,
];

/// The names that say nothing about where a fact goes (an agent's « Infos », a hand-made « Notes »): each fact is
/// placed by its own words.
const GENERIC: [&str; 14] = [
    "",
    "infos",
    "info",
    "informations",
    "notes",
    "note",
    "general",
    "notizen",
    "allgemein",
    "notas",
    "informacion",
    "details",
    "misc",
    "faits",
];

/// The lines a dated log keeps (the older ones are in the page's history).
pub fn keep_lines(id: &str) -> Option<usize> {
    match id {
        "exchanges" => Some(6),
        "history" => Some(10),
        _ => None,
    }
}

pub fn outline(kind: &str) -> &'static [Section] {
    match kind {
        "person" => &PERSON,
        "subject" => &SUBJECT,
        "area" => &AREA,
        _ => &[],
    }
}

/// The section ids of a kind, in order (« other » last).
pub fn ids(kind: &str) -> Vec<&'static str> {
    outline(kind).iter().map(|s| s.id).collect()
}

fn lang_index(lang: &str) -> usize {
    match lang {
        "fr" => 0,
        "de" => 2,
        "es" => 3,
        _ => 1,
    }
}

/// A section's title in `lang` (the id itself when the kind has no such section).
pub fn title(kind: &str, id: &str, lang: &str) -> String {
    outline(kind)
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.titles[lang_index(lang)].to_string())
        .unwrap_or_else(|| id.to_string())
}

/// Another kind's section on this kind's page: where it goes.
fn cross(kind: &str, id: &str) -> Option<&'static str> {
    let pairs: &[(&str, &str)] = match kind {
        "person" => &[
            ("people", "who"),
            ("contacts", "who"),
            ("decisions", "agreed"),
            ("history", "exchanges"),
            ("matters", "remember"),
            ("status", "remember"),
        ],
        "subject" => &[
            ("who", "people"),
            ("family", "people"),
            ("contacts", "people"),
            ("agreed", "decisions"),
            ("exchanges", "history"),
            ("ongoing", "status"),
            ("matters", "status"),
            ("remember", "status"),
        ],
        "area" => &[
            ("who", "contacts"),
            ("people", "contacts"),
            ("family", "contacts"),
            ("status", "ongoing"),
            ("next", "ongoing"),
            ("decisions", "habits"),
            ("agreed", "habits"),
            ("tastes", "habits"),
            ("remember", "matters"),
        ],
        _ => &[],
    };
    pairs.iter().find(|(from, _)| *from == id).map(|(_, to)| *to)
}

/// Whether a section name says nothing about where its facts go.
pub fn generic(name: &str) -> bool {
    GENERIC.contains(&fold(name).as_str())
}

/// The section `name` means on a page of `kind`: its id, a title in any language, a near or old name, or another
/// kind's section. None for a generic or unknown name.
pub fn resolve(kind: &str, name: &str) -> Option<&'static str> {
    let wanted = fold(name);
    if wanted.is_empty() || generic(name) {
        return None;
    }
    let matches = |s: &Section| {
        fold(s.id) == wanted || s.titles.iter().any(|t| fold(t) == wanted) || s.aliases.contains(&wanted.as_str())
    };
    if let Some(s) = outline(kind).iter().find(|s| matches(s)) {
        return Some(s.id);
    }
    for other in ["person", "subject", "area"] {
        if other == kind {
            continue;
        }
        if let Some(s) = outline(other).iter().find(|s| matches(s)) {
            if let Some(to) = cross(kind, s.id) {
                return Some(to);
            }
        }
    }
    None
}

// -- where a fact goes by its words ------------------------------------------------------------------------------------

static STARTS_DATED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:\d{4}-\d{2}-\d{2}|\d{1,2}[/.]\d{1,2}(?:[/.]\d{2,4})?)\b").expect("valid regex")
});

/// Keywords per section (folded words; « x* »: a word starting with x; several words: that phrase), in the order they
/// are tried: the first that matches wins (« Idée cadeau : il aime le jazz » is a gift, not a taste).
fn keywords(kind: &str) -> &'static [(&'static str, &'static [&'static str])] {
    match kind {
        "person" => &[
            (
                "gifts",
                &[
                    "cadeau*",
                    "offrir",
                    "lui offrir",
                    "gift*",
                    "present for",
                    "geschenk*",
                    "schenken",
                    "regalo*",
                    "regalar",
                ],
            ),
            (
                "dates",
                &[
                    "anniv*",
                    "ne le",
                    "nee le",
                    "fete",
                    "birthday*",
                    "born",
                    "name day",
                    "geburtstag*",
                    "geboren",
                    "namenstag",
                    "cumpleanos",
                    "nacio",
                    "nacida",
                    "nacido",
                    "santo",
                ],
            ),
            (
                "agreed",
                &[
                    "promis*",
                    "prete",
                    "emprunte*",
                    "convenu*",
                    "on a dit",
                    "rendez vous",
                    "rdv",
                    "rembourse*",
                    "promised",
                    "lent him",
                    "lent her",
                    "lent them",
                    "borrowed",
                    "owes",
                    "agreed",
                    "appointment",
                    "versprochen",
                    "geliehen",
                    "vereinbart",
                    "schuldet",
                    "prometi*",
                    "prometio",
                    "prestado",
                    "acordado",
                    "acordamos",
                    "debe",
                ],
            ),
            (
                "family",
                &[
                    "frere*",
                    "soeur*",
                    "femme",
                    "mari",
                    "epouse",
                    "epoux",
                    "fils",
                    "fille",
                    "filles",
                    "enfant*",
                    "mere",
                    "pere",
                    "maman",
                    "papa",
                    "parents",
                    "conjoint*",
                    "compagne",
                    "compagnon",
                    "cousin*",
                    "oncle",
                    "tante",
                    "neveu*",
                    "niece*",
                    "petit fils",
                    "petite fille",
                    "grand mere",
                    "grand pere",
                    "marie avec",
                    "mariee",
                    "maries",
                    "brother*",
                    "sister*",
                    "wife",
                    "husband",
                    "my son",
                    "his son",
                    "her son",
                    "their son",
                    "daughter*",
                    "child*",
                    "kids",
                    "mother",
                    "father",
                    "mum",
                    "mom",
                    "dad",
                    "partner",
                    "married",
                    "grandson",
                    "granddaughter",
                    "bruder",
                    "schwester",
                    "ehefrau",
                    "ehemann",
                    "sohn",
                    "tochter",
                    "kinder",
                    "mutter",
                    "vater",
                    "verheiratet",
                    "enkel*",
                    "hermano*",
                    "hermana*",
                    "esposa",
                    "esposo",
                    "marido",
                    "hijo*",
                    "hija*",
                    "madre",
                    "padre",
                    "casado",
                    "casada",
                    "nieto*",
                    "nieta*",
                ],
            ),
            (
                "work",
                &[
                    "travaille",
                    "travail",
                    "boulot",
                    "metier",
                    "poste",
                    "entreprise",
                    "collegue*",
                    "patron",
                    "bureau",
                    "job",
                    "works",
                    "company",
                    "colleague*",
                    "boss",
                    "office",
                    "arbeitet",
                    "firma",
                    "kollege*",
                    "kollegin",
                    "beruf",
                    "trabaja",
                    "empresa",
                    "companero*",
                    "oficina",
                    "jefe",
                ],
            ),
            (
                "tastes",
                &[
                    "aime",
                    "adore",
                    "deteste",
                    "prefere",
                    "allergi*",
                    "vegetarien*",
                    "vegan*",
                    "mange pas",
                    "boit pas",
                    "fan de",
                    "likes",
                    "loves",
                    "hates",
                    "prefers",
                    "allergic",
                    "vegetarian",
                    "favourite",
                    "favorite",
                    "mag",
                    "liebt",
                    "hasst",
                    "bevorzugt",
                    "allergisch",
                    "lieblings*",
                    "le gusta",
                    "le encanta",
                    "odia",
                    "prefiere",
                    "alergic*",
                    "favorit*",
                ],
            ),
            (
                "who",
                &[
                    "telephone",
                    "tel",
                    "portable",
                    "email",
                    "mail",
                    "adresse",
                    "habite",
                    "vit a",
                    "rencontre*",
                    "connu*",
                    "ami",
                    "amie",
                    "voisin*",
                    "phone",
                    "address",
                    "lives",
                    "met",
                    "friend",
                    "neighbo*",
                    "wohnt",
                    "freund",
                    "freundin",
                    "nachbar*",
                    "kennengelernt",
                    "telefono",
                    "direccion",
                    "vive",
                    "amigo",
                    "amiga",
                    "vecin*",
                    "conocimos",
                ],
            ),
        ],
        "subject" => &[
            (
                "decisions",
                &[
                    "decid*",
                    "decision",
                    "choisi*",
                    "valide*",
                    "accepte*",
                    "retenu*",
                    "opte",
                    "decided",
                    "chosen",
                    "approved",
                    "accepted",
                    "entschieden",
                    "beschlossen",
                    "gewahlt",
                    "angenommen",
                    "decidido",
                    "elegido",
                    "aprobado",
                    "aceptado",
                ],
            ),
            (
                "next",
                &[
                    "prochaine etape",
                    "a faire",
                    "il faut",
                    "reste a",
                    "next step",
                    "to do",
                    "need to",
                    "next",
                    "nachster schritt",
                    "muss noch",
                    "zu tun",
                    "proximo paso",
                    "hay que",
                    "falta",
                    "pendiente",
                ],
            ),
            (
                "budget",
                &[
                    "eur",
                    "euro*",
                    "prix",
                    "cout*",
                    "budget",
                    "devis",
                    "montant",
                    "facture*",
                    "paye",
                    "price",
                    "cost*",
                    "quote",
                    "invoice",
                    "paid",
                    "preis",
                    "kosten",
                    "angebot",
                    "rechnung",
                    "bezahlt",
                    "precio",
                    "coste*",
                    "presupuesto",
                    "factura",
                    "pagado",
                ],
            ),
            (
                "documents",
                &[
                    "pdf",
                    "document*",
                    "fichier*",
                    "lien",
                    "http*",
                    "contrat",
                    "scan*",
                    "file",
                    "link",
                    "contract",
                    "dokument*",
                    "datei",
                    "vertrag",
                    "archivo",
                    "enlace",
                    "contrato",
                ],
            ),
            (
                "people",
                &[
                    "plombier",
                    "electricien",
                    "architecte",
                    "artisan*",
                    "notaire",
                    "avocat*",
                    "entrepreneur",
                    "plumber",
                    "electrician",
                    "architect",
                    "contractor",
                    "lawyer",
                    "notary",
                    "handwerker",
                    "klempner",
                    "architekt*",
                    "anwalt",
                    "notar",
                    "fontanero",
                    "electricista",
                    "arquitect*",
                    "abogad*",
                ],
            ),
            (
                "status",
                &[
                    "en cours",
                    "termine*",
                    "fini*",
                    "commence*",
                    "en attente",
                    "bloque*",
                    "avance",
                    "in progress",
                    "done",
                    "finished",
                    "started",
                    "waiting",
                    "blocked",
                    "laufend",
                    "fertig",
                    "begonnen",
                    "wartet",
                    "en curso",
                    "terminad*",
                    "empezad*",
                    "esperando",
                    "bloquead*",
                ],
            ),
        ],
        "area" => &[
            (
                "contacts",
                &[
                    "telephone",
                    "tel",
                    "numero",
                    "plombier",
                    "electricien",
                    "syndic",
                    "gardien*",
                    "assureur",
                    "banquier",
                    "phone",
                    "number",
                    "plumber",
                    "electrician",
                    "landlord",
                    "insurer",
                    "telefon",
                    "nummer",
                    "klempner",
                    "vermieter",
                    "versicherung",
                    "telefono",
                    "numero",
                    "fontanero",
                    "casero",
                ],
            ),
            (
                "habits",
                &[
                    "toujours",
                    "jamais",
                    "chaque",
                    "tous les",
                    "toutes les",
                    "habitude*",
                    "regle*",
                    "always",
                    "never",
                    "every",
                    "rule*",
                    "usually",
                    "immer",
                    "nie",
                    "niemals",
                    "jeden",
                    "jede",
                    "regel*",
                    "siempre",
                    "nunca",
                    "cada",
                    "regla*",
                    "costumbre",
                ],
            ),
            (
                "ongoing",
                &[
                    "en cours",
                    "a faire",
                    "prevu*",
                    "projet*",
                    "in progress",
                    "planned",
                    "project*",
                    "to do",
                    "laufend",
                    "geplant",
                    "projekt*",
                    "en curso",
                    "previsto",
                    "proyecto*",
                ],
            ),
            ("matters", &["important*", "priorit*", "essentiel*", "wichtig*", "importante*", "prioridad*"]),
        ],
        _ => &[],
    }
}

fn hits(words: &[&str], phrase: &str, keyword: &str) -> bool {
    if keyword.contains(' ') {
        return format!(" {phrase} ").contains(&format!(" {keyword} "));
    }
    match keyword.strip_suffix('*') {
        Some(stem) => words.iter().any(|w| w.starts_with(stem)),
        None => words.contains(&keyword),
    }
}

/// The section a fact's words point to on a page of `kind` (no model), if any.
pub fn guess(kind: &str, fact: &str) -> Option<&'static str> {
    let (text, _) = split_attribution(fact.trim().trim_start_matches("- "));
    if STARTS_DATED.is_match(&text) {
        match kind {
            "person" => return Some("exchanges"),
            "subject" => return Some("history"),
            _ => {}
        }
    }
    if kind == "subject" && (text.contains('€') || text.contains('$') || text.contains('£')) {
        let folded = fold(&text);
        let words: Vec<&str> = folded.split(' ').collect();
        if !keywords(kind)[0].1.iter().any(|k| hits(&words, &folded, k)) {
            return Some("budget");
        }
    }
    let folded = fold(&text);
    let words: Vec<&str> = folded.split(' ').collect();
    keywords(kind).iter().find(|(_, list)| list.iter().any(|k| hits(&words, &folded, k))).map(|(id, _)| *id)
}

/// Where a fact goes on a page of `kind` when the agent named `section`: that section when it means one, else where
/// the fact's words point, else « other ».
pub fn place(kind: &str, section: &str, fact: &str) -> &'static str {
    resolve(kind, section).or_else(|| guess(kind, fact)).unwrap_or("other")
}

// -- one fact a line ---------------------------------------------------------------------------------------------------

/// The longest a fact may be (a short line, not a paragraph).
pub const FACT_CHARS: usize = 300;

static SENTENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([.!?])\s+(\p{Lu})").expect("valid regex"));

/// A fact that holds several split into short ones: on « ; », « · », line breaks, and sentences when each has a few
/// words (« M. Dupont » stays whole).
pub fn split_fact(fact: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for piece in fact.split(['\n', ';']).flat_map(|p| p.split(" · ")) {
        let piece = tk::squeeze(piece.trim().trim_start_matches("- ").trim());
        if piece.is_empty() {
            continue;
        }
        // sentences: cut where the text before and after both have three words or more
        let mut start = 0;
        let mut parts: Vec<String> = Vec::new();
        for c in SENTENCE.captures_iter(&piece) {
            let (Some(stop), Some(next)) = (c.get(1), c.get(2)) else { continue };
            let before = &piece[start..stop.end()];
            let after = &piece[next.start()..];
            let last = before.split_whitespace().last().unwrap_or("").trim_end_matches(['.', '!', '?']);
            let initial = tk::chars(last) <= 2 && !last.chars().all(|c| c.is_ascii_digit());
            if !initial && before.split_whitespace().count() >= 3 && after.split_whitespace().count() >= 3 {
                parts.push(before.trim().trim_end_matches('.').trim().to_string());
                start = next.start();
            }
        }
        parts.push(piece[start..].trim().trim_end_matches('.').trim().to_string());
        out.extend(parts.into_iter().filter(|p| !p.is_empty()));
    }
    out.into_iter().map(|f| tk::head(&f, FACT_CHARS).to_string()).collect()
}

// -- a page in its outline ---------------------------------------------------------------------------------------------

/// What `normalize` did: the body in its outline, and how many facts changed section.
#[derive(Debug, Clone, PartialEq)]
pub struct Normalized {
    pub body: String,
    pub moved: usize,
}

fn is_brief(name: &str, brief: &str) -> bool {
    let n = fold(name);
    n == fold(brief) || super::WORDS.iter().any(|w| fold(w.brief) == n)
}

/// A page's body in its outline: « En bref » first, then the kind's sections in their order with their titles in
/// `lang`, the empty ones left out; each fact under a heading that means a section stays there, the others go where
/// their words point (a fact in « Autres » too), else to « Autres » (prefixed with their old heading when it said
/// something: « Voyages : Japon en 2025 »). Attributions and dates are kept as they are. A page of another kind (the
/// tasks, « À ranger »…) is returned as it was.
pub fn normalize(kind: &str, lang: &str, brief: &str, body: &str) -> Normalized {
    let sections = outline(kind);
    if sections.is_empty() {
        return Normalized { body: body.to_string(), moved: 0 };
    }
    let mut brief_lines: Vec<String> = Vec::new();
    let mut buckets: Vec<Vec<String>> = vec![Vec::new(); sections.len()];
    let index = |id: &str| sections.iter().position(|s| s.id == id).unwrap_or(sections.len() - 1);
    let mut moved = 0;
    let mut heading: Option<String> = None;
    for line in body.lines() {
        if let Some(name) = line.strip_prefix("## ") {
            heading = Some(name.trim().to_string());
            continue;
        }
        if line.starts_with("# ") || line.trim().is_empty() {
            continue;
        }
        let name = heading.clone().unwrap_or_default();
        if heading.is_some() && is_brief(&name, brief) {
            brief_lines.push(line.trim_end().to_string());
            continue;
        }
        let resolved = resolve(kind, &name).filter(|id| *id != "other");
        let line = line.trim_end().to_string();
        match resolved {
            Some(id) => buckets[index(id)].push(line),
            None => {
                let trimmed = line.trim();
                let is_item = trimmed.starts_with("- ") || trimmed.starts_with("* ");
                let item = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")).unwrap_or(trimmed);
                let was_other = resolve(kind, &name) == Some("other");
                match guess(kind, item) {
                    Some(id) => {
                        moved += 1;
                        buckets[index(id)].push(line);
                    }
                    None if was_other => buckets[index("other")].push(line),
                    None => {
                        moved += 1;
                        let (text, _) = split_attribution(item);
                        let label = tk::squeeze(&name);
                        let keep_label =
                            !label.is_empty() && !generic(&label) && !fold(&text).starts_with(&fold(&label));
                        if keep_label && is_item {
                            buckets[index("other")].push(format!("- {label} : {item}"));
                        } else {
                            buckets[index("other")].push(line);
                        }
                    }
                }
            }
        }
    }
    let mut out: Vec<String> = Vec::new();
    while brief_lines.last().is_some_and(|l| l.trim().is_empty()) {
        brief_lines.pop();
    }
    if !brief_lines.is_empty() {
        out.push(format!("## {brief}"));
        out.extend(brief_lines);
    }
    for (section, lines) in sections.iter().zip(buckets) {
        if lines.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(String::new());
        }
        out.push(format!("## {}", section.titles[lang_index(lang)]));
        out.extend(lines);
    }
    let body = if out.is_empty() { String::new() } else { out.join("\n") + "\n" };
    Normalized { body, moved }
}

/// The dated logs (« Derniers échanges », « Historique ») cut to their latest lines. Returns the body and the lines
/// taken out (they stay in the page's history).
pub fn trim_logs(kind: &str, lang: &str, body: &str) -> (String, Vec<String>) {
    let mut removed: Vec<String> = Vec::new();
    let mut lines: Vec<String> = body.lines().map(str::to_string).collect();
    for section in outline(kind) {
        let Some(keep) = keep_lines(section.id) else { continue };
        let title = fold(section.titles[lang_index(lang)]);
        let Some(start) = lines.iter().position(|l| l.strip_prefix("## ").map(|n| fold(n) == title).unwrap_or(false))
        else {
            continue;
        };
        let end =
            lines[start + 1..].iter().position(|l| l.starts_with("## ")).map(|i| start + 1 + i).unwrap_or(lines.len());
        let items: Vec<usize> = (start + 1..end).filter(|i| lines[*i].trim().starts_with("- ")).collect();
        if items.len() <= keep {
            continue;
        }
        let drop: Vec<usize> = items[..items.len() - keep].to_vec();
        for i in drop.iter().rev() {
            removed.insert(0, lines.remove(*i));
        }
    }
    if removed.is_empty() {
        return (body.to_string(), removed);
    }
    (lines.join("\n").trim_matches('\n').to_string() + "\n", removed)
}

/// The tool's description of the outlines (the ids an agent gives as `section`).
pub fn described() -> String {
    let list = |kind: &str| ids(kind).iter().filter(|id| **id != "other").copied().collect::<Vec<_>>().join(", ");
    format!(
        "the page's section, by its id. person: {} (who: how they relate to the person, how they met, contact details \
if useful; dates: birthday, name day, important dates; tastes: likes, dislikes, food allergies as everyday facts; \
agreed: promises, loans, appointments; exchanges: short dated lines, the latest few). subject: {}. area: {}. other \
only when nothing fits.",
        list("person"),
        list("subject"),
        list("area")
    )
}

/// Every section id, for the tool's enum.
pub fn all_ids() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for kind in ["person", "subject", "area"] {
        for id in ids(kind) {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_map_to_the_outline() {
        assert_eq!(resolve("person", "dates"), Some("dates"));
        assert_eq!(resolve("person", "Famille"), Some("family"));
        assert_eq!(resolve("person", "Échanges récents"), Some("exchanges"));
        assert_eq!(resolve("person", "Goûts et préférences"), Some("tastes"));
        assert_eq!(resolve("person", "Préférences"), Some("tastes"));
        assert_eq!(resolve("area", "Préférences"), Some("habits"));
        assert_eq!(resolve("subject", "Statut"), Some("status"));
        assert_eq!(resolve("subject", "Échanges récents"), Some("history"));
        assert_eq!(resolve("subject", "family"), Some("people"));
        assert_eq!(resolve("person", "Infos"), None);
        assert_eq!(resolve("person", "Voyages"), None);
        assert_eq!(resolve("person", "Nächste Schritte"), None);
        assert_eq!(resolve("subject", "Nächste Schritte"), Some("next"));
        assert_eq!(title("person", "agreed", "fr"), "Ce que vous avez convenu");
        assert_eq!(title("subject", "status", "de"), "Stand der Dinge");
    }

    #[test]
    fn facts_go_where_their_words_point() {
        assert_eq!(guess("person", "Anniversaire : 14 novembre"), Some("dates"));
        assert_eq!(guess("person", "Sa fille Zoé a 8 ans"), Some("family"));
        assert_eq!(guess("person", "Aime le jazz — *Léa, 2026-10-22*"), Some("tastes"));
        assert_eq!(guess("person", "Idée cadeau : il aime le jazz"), Some("gifts"));
        assert_eq!(guess("person", "Allergique aux noix"), Some("tastes"));
        assert_eq!(guess("person", "Travaille chez Airbus"), Some("work"));
        assert_eq!(guess("person", "2026-10-20 : appel, il déménage"), Some("exchanges"));
        assert_eq!(guess("person", "Lui a prêté la perceuse"), Some("agreed"));
        assert_eq!(guess("person", "Grand, cheveux bruns"), None);
        assert_eq!(guess("subject", "Devis plombier accepté : 480 €"), Some("decisions"));
        assert_eq!(guess("subject", "Carrelage : 1 200 €"), Some("budget"));
        assert_eq!(guess("area", "Les poubelles sortent toujours le mardi"), Some("habits"));
        assert_eq!(place("person", "Infos", "Anniversaire : 12 mars"), "dates");
        assert_eq!(place("person", "Infos", "Grand, cheveux bruns"), "other");
        assert_eq!(place("person", "remember", "Grand"), "remember");
    }

    #[test]
    fn a_line_holds_one_fact() {
        assert_eq!(
            split_fact("Aime le jazz; déteste les huîtres · Fille : Zoé"),
            vec!["Aime le jazz", "déteste les huîtres", "Fille : Zoé"]
        );
        assert_eq!(
            split_fact("Il travaille chez Airbus depuis 2019. Sa femme Léa est médecin."),
            vec!["Il travaille chez Airbus depuis 2019", "Sa femme Léa est médecin"]
        );
        assert_eq!(split_fact("Voisin de M. Dupont"), vec!["Voisin de M. Dupont"]);
    }

    #[test]
    fn pages_are_put_in_their_outline() {
        let body = "## En bref\nAmi d'enfance.\n\n## Infos\n- Aime le jazz — *Léa, 2026-10-20*\n\
- Anniversaire : 14 novembre — *Léa, 2026-10-21*\n- Grand — *Léa, 2026-10-21*\n\n## Voyages\n- Japon en 2025 — *Léa, 2026-10-21*\n\n\
## Famille\n- Fille : Zoé — *Léa, 2026-10-22*\n";
        let n = normalize("person", "fr", "En bref", body);
        assert_eq!(
            n.body,
            "## En bref\nAmi d'enfance.\n\n## Dates\n- Anniversaire : 14 novembre — *Léa, 2026-10-21*\n\n\
## Famille et proches\n- Fille : Zoé — *Léa, 2026-10-22*\n\n## Goûts et préférences\n- Aime le jazz — *Léa, 2026-10-20*\n\n\
## Autres\n- Grand — *Léa, 2026-10-21*\n- Voyages : Japon en 2025 — *Léa, 2026-10-21*\n"
        );
        assert_eq!(n.moved, 4);
        let again = normalize("person", "fr", "En bref", &n.body);
        assert_eq!(again.body, n.body);
        assert_eq!(again.moved, 0);
        assert_eq!(normalize("inbox", "fr", "En bref", "## Léa\n- x\n").body, "## Léa\n- x\n");
    }

    #[test]
    fn the_logs_keep_their_latest_lines() {
        let mut body = "## Derniers échanges\n".to_string();
        for n in 1..=8 {
            body.push_str(&format!("- 2026-10-0{n} : appel {n}\n"));
        }
        body.push_str("\n## À ne pas oublier\n- x\n");
        let (kept, removed) = trim_logs("person", "fr", &body);
        assert_eq!(removed, vec!["- 2026-10-01 : appel 1", "- 2026-10-02 : appel 2"]);
        assert!(kept.starts_with("## Derniers échanges\n- 2026-10-03 : appel 3\n"));
        assert!(kept.ends_with("- 2026-10-08 : appel 8\n\n## À ne pas oublier\n- x\n"));
    }
}
