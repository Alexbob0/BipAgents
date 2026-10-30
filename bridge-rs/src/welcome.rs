//! A new agent's first words (Alex, 2026-10-30, after pairing his first cloud place: « Ça serait bien que l'agent te
//! parle directement et te dise ce qu'il peut faire. Là, il est juste, il dit rien. »).
//!
//! When an agent is made for the person (the claim of a place, the guide's card, by hand), its first conversation
//! opens with a short welcome in the agent's language: who it is, three things it can do (from its kind), an
//! invitation to answer, and two or three suggestions the apps show as chips. Written here from templates, never by
//! the model: no cost, no wait, the same words on every app.
//!
//! Where it lives: NOT in Hermes. Hermes' API has no way to add an assistant message to a session without a turn, its
//! sessions are its own SQLite (writing there would break at its next update), and a history that starts with the
//! assistant is refused by some providers. So the bridge keeps the agents made with a welcome (`guide.json` →
//! `welcome`, with the kind and language they were made with) and `GET /v1/guide` gives each one's text and
//! suggestions (`welcome: {agent: {text, suggestions, session}}`); the apps show it as the agent's first bubble at the
//! top of an empty conversation (with the chips), where they showed their own intro before. The person's first
//! message to that agent ties it to that conversation (`session`): seen by the door's relay when it starts a run
//! (lan.rs), or told by the app (`POST /v1/guide/welcome/{agent}/done {session}`, for a phone that reaches Hermes
//! directly). From then on it stays at the top of that conversation only, on every app, and no other conversation
//! opens with it. Agents that existed before, an imported agent, a team member (it writes its own « Premiers pas »),
//! a project or family agent never get one.
//!
//! No Python counterpart.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::{AgentConfig, Config};

/// An agent made with a welcome: what it was made as, and the conversation of the person's first message to it (None:
/// not written to yet; empty: written to in a conversation the bridge didn't learn).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Waiting {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub lang: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// The languages the welcomes are written in (the apps' languages); any other gets English.
pub const LANGUAGES: [&str; 5] = ["fr", "en", "de", "es", "nl"];
/// The kinds with their own welcome: the guide, the categories (souls::CATEGORIES) and an agent with its own role.
pub const KINDS: [&str; 10] =
    ["guide", "daily", "wellness", "finance", "home", "work", "learning", "creative", "tech", "custom"];

/// One welcome: the line after « I'm <name> », three things it can do, the question, the chips.
struct Text {
    about: &'static str,
    can: [&'static str; 3],
    ask: &'static str,
    chips: [&'static str; 3],
}

/// The words around the template, per language: « Hello, I'm {name}! », « For instance, I can: », the guide's rename line.
struct Frame {
    hello: &'static str,
    can: &'static str,
    rename: &'static str,
}

fn frame(lang: &str) -> Frame {
    match lang {
        "fr" => Frame {
            hello: "Bonjour, je suis {name} !",
            can: "Je peux par exemple :",
            rename: "Tu préfères m'appeler autrement ? Dis-le-moi, je change de nom.",
        },
        "de" => Frame {
            hello: "Hallo, ich bin {name}!",
            can: "Ich kann zum Beispiel:",
            rename: "Lieber ein anderer Name für mich? Sag es mir, dann heiße ich so.",
        },
        "es" => Frame {
            hello: "¡Hola, soy {name}!",
            can: "Por ejemplo, puedo:",
            rename: "¿Prefieres llamarme de otra forma? Dímelo y me cambio el nombre.",
        },
        "nl" => Frame {
            hello: "Hallo, ik ben {name}!",
            can: "Ik kan bijvoorbeeld:",
            rename: "Wil je me liever anders noemen? Zeg het maar, dan verander ik mijn naam.",
        },
        _ => Frame {
            hello: "Hello, I'm {name}!",
            can: "For instance, I can:",
            rename: "Would you rather call me something else? Just tell me and I'll change my name.",
        },
    }
}

fn text(kind: &str, lang: &str) -> Text {
    match (lang, kind) {
        // -- French ---------------------------------------------------------------------------------------------
        ("fr", "guide") => Text {
            about: "Je suis là pour ton quotidien, et pour t'aider à mettre en place les bons agents.",
            can: [
                "chercher une info sur internet et te la résumer",
                "te rappeler un rendez-vous ou une chose à faire",
                "te proposer des agents rien que pour toi : budget, santé, maison…",
            ],
            ask: "Pour commencer, dis-m'en un peu plus : qu'est-ce qui te prend du temps en ce moment ?",
            chips: [
                "Ma famille : les devoirs, les rendez-vous, les courses.",
                "Mon travail : les mails, les réunions, les choses à ne pas oublier.",
                "Mon budget : les dépenses, les abonnements, les factures.",
            ],
        },
        ("fr", "daily") => Text {
            about: "Je m'occupe de ton quotidien.",
            can: [
                "te faire un point le matin : météo, rendez-vous, rappels",
                "chercher une info sur internet et te la résumer",
                "te rappeler une chose à faire au bon moment",
            ],
            ask: "Par quoi veux-tu commencer ?",
            chips: [
                "Fais-moi un point chaque matin à 7 h 30.",
                "Rappelle-moi d'appeler le médecin demain.",
                "Quel temps fera-t-il ce week-end ?",
            ],
        },
        ("fr", "wellness") => Text {
            about: "Je t'accompagne pour ta santé et ton bien-être.",
            can: [
                "te préparer un programme de sport à ton rythme",
                "t'aider à mieux dormir et à suivre tes habitudes",
                "te rappeler tes rendez-vous et tes médicaments",
            ],
            ask: "Qu'est-ce qui compte le plus pour toi en ce moment ?",
            chips: [
                "Je voudrais me remettre au sport, doucement.",
                "Aide-moi à mieux dormir.",
                "Rappelle-moi de boire de l'eau dans la journée.",
            ],
        },
        ("fr", "finance") => Text {
            about: "Je t'aide à garder un œil sur ton argent.",
            can: [
                "faire le point sur tes dépenses et tes abonnements",
                "lire une facture ou un relevé que tu m'envoies",
                "te rappeler une échéance avant qu'elle tombe",
            ],
            ask: "Par quoi veux-tu qu'on commence ?",
            chips: [
                "Aide-moi à faire le point sur mes abonnements.",
                "Je voudrais un budget simple pour le mois.",
                "Rappelle-moi de payer mes factures.",
            ],
        },
        ("fr", "home") => Text {
            about: "Je m'occupe de la maison avec toi.",
            can: [
                "te faire la liste des courses et des repas",
                "t'organiser le ménage et les petites réparations",
                "garder tes papiers et te rappeler ce qui arrive",
            ],
            ask: "Qu'est-ce qui te simplifierait la vie à la maison ?",
            chips: [
                "Propose-moi les repas de la semaine.",
                "Fais-moi la liste des courses.",
                "Aide-moi à organiser le ménage.",
            ],
        },
        ("fr", "work") => Text {
            about: "Je t'aide dans ton travail.",
            can: [
                "préparer une réunion ou un message",
                "lire un document et en tirer l'essentiel",
                "suivre tes tâches et te rappeler les échéances",
            ],
            ask: "Sur quoi travailles-tu en ce moment ?",
            chips: ["Aide-moi à préparer ma semaine.", "Résume-moi un document.", "Aide-moi à écrire un mail délicat."],
        },
        ("fr", "learning") => Text {
            about: "Je t'aide à apprendre, un peu chaque jour.",
            can: [
                "te faire de petites leçons à ton niveau",
                "t'interroger pour voir ce que tu as retenu",
                "te rappeler ta séance au moment que tu choisis",
            ],
            ask: "Qu'aimerais-tu apprendre ?",
            chips: [
                "Je voudrais apprendre l'espagnol.",
                "Explique-moi un sujet simplement.",
                "Fais-moi réviser 10 minutes par jour.",
            ],
        },
        ("fr", "creative") => Text {
            about: "Je suis là pour tes idées et tes projets créatifs.",
            can: ["trouver des idées avec toi", "écrire ou relire un texte", "suivre ton projet pas à pas"],
            ask: "Sur quoi as-tu envie de créer ?",
            chips: [
                "Aide-moi à trouver des idées.",
                "Relis un texte que j'ai écrit.",
                "J'ai un projet, aide-moi à le lancer.",
            ],
        },
        ("fr", "tech") => Text {
            about: "Je t'aide avec l'informatique et tes appareils.",
            can: [
                "t'expliquer un souci et comment le régler",
                "chercher la bonne marche à suivre sur internet",
                "te guider pas à pas dans un réglage",
            ],
            ask: "Qu'est-ce qui te pose problème ?",
            chips: [
                "Mon ordinateur est lent, que faire ?",
                "Aide-moi à mieux protéger mes comptes.",
                "Explique-moi comment sauvegarder mes photos.",
            ],
        },
        ("fr", _) => Text {
            about: "Je suis là pour t'aider.",
            can: [
                "chercher une info sur internet et te la résumer",
                "lire un document ou une photo que tu m'envoies",
                "te rappeler une chose à faire au bon moment",
            ],
            ask: "Dis-moi ce que tu attends de moi, je m'adapte.",
            chips: [
                "Voilà ce que j'aimerais que tu fasses pour moi.",
                "Qu'est-ce que tu sais faire ?",
                "Rappelle-moi quelque chose demain.",
            ],
        },
        // -- German ---------------------------------------------------------------------------------------------
        ("de", "guide") => Text {
            about: "Ich bin für deinen Alltag da und helfe dir, die richtigen Agenten einzurichten.",
            can: [
                "etwas im Internet suchen und es dir zusammenfassen",
                "dich an einen Termin oder eine Aufgabe erinnern",
                "dir Agenten nur für dich vorschlagen: Budget, Gesundheit, Zuhause…",
            ],
            ask: "Erzähl mir zum Anfang ein bisschen: Was kostet dich gerade Zeit?",
            chips: [
                "Meine Familie: Hausaufgaben, Termine, Einkäufe.",
                "Meine Arbeit: Mails, Meetings, Dinge, die ich nicht vergessen darf.",
                "Mein Budget: Ausgaben, Abos, Rechnungen.",
            ],
        },
        ("de", "daily") => Text {
            about: "Ich kümmere mich um deinen Alltag.",
            can: [
                "dir morgens einen Überblick geben: Wetter, Termine, Erinnerungen",
                "etwas im Internet suchen und es dir zusammenfassen",
                "dich im richtigen Moment an etwas erinnern",
            ],
            ask: "Womit möchtest du anfangen?",
            chips: [
                "Gib mir jeden Morgen um 7:30 einen Überblick.",
                "Erinnere mich morgen daran, den Arzt anzurufen.",
                "Wie wird das Wetter am Wochenende?",
            ],
        },
        ("de", "wellness") => Text {
            about: "Ich begleite dich bei Gesundheit und Wohlbefinden.",
            can: [
                "dir ein Sportprogramm in deinem Tempo zusammenstellen",
                "dir helfen, besser zu schlafen und Gewohnheiten zu halten",
                "dich an Termine und Medikamente erinnern",
            ],
            ask: "Was ist dir gerade am wichtigsten?",
            chips: [
                "Ich möchte langsam wieder mit Sport anfangen.",
                "Hilf mir, besser zu schlafen.",
                "Erinnere mich tagsüber ans Trinken.",
            ],
        },
        ("de", "finance") => Text {
            about: "Ich helfe dir, dein Geld im Blick zu behalten.",
            can: [
                "deine Ausgaben und Abos durchgehen",
                "eine Rechnung oder einen Auszug lesen, den du mir schickst",
                "dich vor einer Frist erinnern",
            ],
            ask: "Womit sollen wir anfangen?",
            chips: [
                "Hilf mir, meine Abos durchzugehen.",
                "Ich möchte ein einfaches Monatsbudget.",
                "Erinnere mich, meine Rechnungen zu zahlen.",
            ],
        },
        ("de", "home") => Text {
            about: "Ich kümmere mich mit dir ums Zuhause.",
            can: [
                "Einkaufsliste und Essensplan machen",
                "Putzen und kleine Reparaturen planen",
                "deine Unterlagen ordnen und an Fristen erinnern",
            ],
            ask: "Was würde dir zu Hause das Leben leichter machen?",
            chips: [
                "Schlag mir Gerichte für die Woche vor.",
                "Mach mir eine Einkaufsliste.",
                "Hilf mir, das Putzen zu planen.",
            ],
        },
        ("de", "work") => Text {
            about: "Ich helfe dir bei der Arbeit.",
            can: [
                "ein Meeting oder eine Nachricht vorbereiten",
                "ein Dokument lesen und das Wichtigste herausziehen",
                "deine Aufgaben verfolgen und an Fristen erinnern",
            ],
            ask: "Woran arbeitest du gerade?",
            chips: [
                "Hilf mir, meine Woche zu planen.",
                "Fass mir ein Dokument zusammen.",
                "Hilf mir bei einer heiklen Mail.",
            ],
        },
        ("de", "learning") => Text {
            about: "Ich helfe dir, jeden Tag ein bisschen zu lernen.",
            can: [
                "dir kleine Lektionen auf deinem Niveau geben",
                "dich abfragen, was du behalten hast",
                "dich zu deiner Lernzeit erinnern",
            ],
            ask: "Was möchtest du lernen?",
            chips: [
                "Ich möchte Spanisch lernen.",
                "Erklär mir ein Thema ganz einfach.",
                "Lass mich jeden Tag 10 Minuten wiederholen.",
            ],
        },
        ("de", "creative") => Text {
            about: "Ich bin für deine Ideen und kreativen Projekte da.",
            can: [
                "mit dir Ideen finden",
                "einen Text schreiben oder gegenlesen",
                "dein Projekt Schritt für Schritt begleiten",
            ],
            ask: "Was möchtest du erschaffen?",
            chips: [
                "Hilf mir, Ideen zu finden.",
                "Lies einen Text, den ich geschrieben habe.",
                "Ich habe ein Projekt, hilf mir beim Start.",
            ],
        },
        ("de", "tech") => Text {
            about: "Ich helfe dir mit Computer und Geräten.",
            can: [
                "ein Problem erklären und wie man es löst",
                "die richtige Anleitung im Internet finden",
                "dich Schritt für Schritt durch eine Einstellung führen",
            ],
            ask: "Wo hakt es gerade?",
            chips: [
                "Mein Computer ist langsam, was tun?",
                "Hilf mir, meine Konten besser zu schützen.",
                "Wie sichere ich meine Fotos?",
            ],
        },
        ("de", _) => Text {
            about: "Ich bin da, um dir zu helfen.",
            can: [
                "etwas im Internet suchen und es dir zusammenfassen",
                "ein Dokument oder Foto lesen, das du mir schickst",
                "dich im richtigen Moment an etwas erinnern",
            ],
            ask: "Sag mir, was du von mir erwartest, ich stelle mich darauf ein.",
            chips: ["Das hätte ich gern von dir.", "Was kannst du alles?", "Erinnere mich morgen an etwas."],
        },
        // -- Spanish --------------------------------------------------------------------------------------------
        ("es", "guide") => Text {
            about: "Estoy aquí para tu día a día y para ayudarte a crear los agentes adecuados.",
            can: [
                "buscar algo en internet y resumírtelo",
                "recordarte una cita o algo pendiente",
                "proponerte agentes solo para ti: presupuesto, salud, casa…",
            ],
            ask: "Para empezar, cuéntame un poco: ¿qué te quita tiempo ahora mismo?",
            chips: [
                "Mi familia: los deberes, las citas, la compra.",
                "Mi trabajo: los correos, las reuniones, lo que no debo olvidar.",
                "Mi presupuesto: los gastos, las suscripciones, las facturas.",
            ],
        },
        ("es", "daily") => Text {
            about: "Me ocupo de tu día a día.",
            can: [
                "darte un resumen por la mañana: tiempo, citas, recordatorios",
                "buscar algo en internet y resumírtelo",
                "recordarte algo en el momento justo",
            ],
            ask: "¿Por dónde quieres empezar?",
            chips: [
                "Dame un resumen cada mañana a las 7:30.",
                "Recuérdame llamar al médico mañana.",
                "¿Qué tiempo hará este fin de semana?",
            ],
        },
        ("es", "wellness") => Text {
            about: "Te acompaño en tu salud y bienestar.",
            can: [
                "prepararte un plan de ejercicio a tu ritmo",
                "ayudarte a dormir mejor y a mantener tus hábitos",
                "recordarte tus citas y tus medicamentos",
            ],
            ask: "¿Qué es lo más importante para ti ahora?",
            chips: [
                "Quiero volver a hacer deporte, poco a poco.",
                "Ayúdame a dormir mejor.",
                "Recuérdame beber agua durante el día.",
            ],
        },
        ("es", "finance") => Text {
            about: "Te ayudo a vigilar tu dinero.",
            can: [
                "repasar tus gastos y suscripciones",
                "leer una factura o un extracto que me envíes",
                "recordarte un vencimiento antes de que llegue",
            ],
            ask: "¿Por dónde empezamos?",
            chips: [
                "Ayúdame a repasar mis suscripciones.",
                "Quiero un presupuesto sencillo para el mes.",
                "Recuérdame pagar mis facturas.",
            ],
        },
        ("es", "home") => Text {
            about: "Me ocupo de la casa contigo.",
            can: [
                "hacer la lista de la compra y el menú",
                "organizar la limpieza y las pequeñas reparaciones",
                "ordenar tus papeles y recordarte lo que llega",
            ],
            ask: "¿Qué te haría la vida más fácil en casa?",
            chips: [
                "Proponme el menú de la semana.",
                "Hazme la lista de la compra.",
                "Ayúdame a organizar la limpieza.",
            ],
        },
        ("es", "work") => Text {
            about: "Te ayudo en tu trabajo.",
            can: [
                "preparar una reunión o un mensaje",
                "leer un documento y sacar lo esencial",
                "seguir tus tareas y recordarte los plazos",
            ],
            ask: "¿En qué estás trabajando ahora?",
            chips: [
                "Ayúdame a preparar mi semana.",
                "Resúmeme un documento.",
                "Ayúdame a escribir un correo delicado.",
            ],
        },
        ("es", "learning") => Text {
            about: "Te ayudo a aprender un poco cada día.",
            can: [
                "darte lecciones cortas a tu nivel",
                "preguntarte para ver qué recuerdas",
                "recordarte tu sesión a la hora que elijas",
            ],
            ask: "¿Qué te gustaría aprender?",
            chips: [
                "Quiero aprender inglés.",
                "Explícame un tema de forma sencilla.",
                "Hazme repasar 10 minutos al día.",
            ],
        },
        ("es", "creative") => Text {
            about: "Estoy aquí para tus ideas y proyectos creativos.",
            can: ["encontrar ideas contigo", "escribir o revisar un texto", "seguir tu proyecto paso a paso"],
            ask: "¿Qué te apetece crear?",
            chips: [
                "Ayúdame a encontrar ideas.",
                "Revisa un texto que he escrito.",
                "Tengo un proyecto, ayúdame a empezar.",
            ],
        },
        ("es", "tech") => Text {
            about: "Te ayudo con la informática y tus aparatos.",
            can: [
                "explicarte un problema y cómo resolverlo",
                "buscar los pasos correctos en internet",
                "guiarte paso a paso en un ajuste",
            ],
            ask: "¿Qué problema tienes?",
            chips: [
                "Mi ordenador va lento, ¿qué hago?",
                "Ayúdame a proteger mejor mis cuentas.",
                "¿Cómo hago copia de mis fotos?",
            ],
        },
        ("es", _) => Text {
            about: "Estoy aquí para ayudarte.",
            can: [
                "buscar algo en internet y resumírtelo",
                "leer un documento o una foto que me envíes",
                "recordarte algo en el momento justo",
            ],
            ask: "Dime qué esperas de mí y me adapto.",
            chips: ["Esto es lo que me gustaría que hicieras.", "¿Qué sabes hacer?", "Recuérdame algo mañana."],
        },
        // -- Dutch ----------------------------------------------------------------------------------------------
        ("nl", "guide") => Text {
            about: "Ik ben er voor je dagelijks leven en help je de juiste agenten in te stellen.",
            can: [
                "iets op internet opzoeken en voor je samenvatten",
                "je herinneren aan een afspraak of iets wat moet",
                "agenten voor je voorstellen: budget, gezondheid, huis…",
            ],
            ask: "Vertel eerst eens: waar gaat nu veel van je tijd naartoe?",
            chips: [
                "Mijn gezin: huiswerk, afspraken, boodschappen.",
                "Mijn werk: mails, vergaderingen, dingen die ik niet mag vergeten.",
                "Mijn budget: uitgaven, abonnementen, rekeningen.",
            ],
        },
        ("nl", "daily") => Text {
            about: "Ik zorg voor je dagelijkse dingen.",
            can: [
                "je 's ochtends een overzicht geven: weer, afspraken, herinneringen",
                "iets op internet opzoeken en voor je samenvatten",
                "je op het juiste moment ergens aan herinneren",
            ],
            ask: "Waarmee wil je beginnen?",
            chips: [
                "Geef me elke ochtend om 7.30 uur een overzicht.",
                "Herinner me eraan morgen de dokter te bellen.",
                "Wat voor weer wordt het dit weekend?",
            ],
        },
        ("nl", "wellness") => Text {
            about: "Ik help je met je gezondheid en je welzijn.",
            can: [
                "een sportschema op jouw tempo maken",
                "je helpen beter te slapen en gewoontes vol te houden",
                "je herinneren aan afspraken en medicijnen",
            ],
            ask: "Wat is nu het belangrijkst voor je?",
            chips: [
                "Ik wil rustig weer gaan sporten.",
                "Help me beter te slapen.",
                "Herinner me overdag eraan water te drinken.",
            ],
        },
        ("nl", "finance") => Text {
            about: "Ik help je je geld in de gaten te houden.",
            can: [
                "je uitgaven en abonnementen doorlopen",
                "een rekening of afschrift lezen dat je me stuurt",
                "je herinneren voordat iets betaald moet worden",
            ],
            ask: "Waar zullen we beginnen?",
            chips: [
                "Help me mijn abonnementen door te lopen.",
                "Ik wil een eenvoudig maandbudget.",
                "Herinner me eraan mijn rekeningen te betalen.",
            ],
        },
        ("nl", "home") => Text {
            about: "Ik zorg samen met jou voor het huis.",
            can: [
                "een boodschappenlijst en weekmenu maken",
                "schoonmaken en kleine klussen plannen",
                "je papieren ordenen en je aan termijnen herinneren",
            ],
            ask: "Wat zou het thuis makkelijker maken?",
            chips: [
                "Stel een weekmenu voor me op.",
                "Maak een boodschappenlijst.",
                "Help me het schoonmaken te plannen.",
            ],
        },
        ("nl", "work") => Text {
            about: "Ik help je met je werk.",
            can: [
                "een vergadering of bericht voorbereiden",
                "een document lezen en de kern eruit halen",
                "je taken volgen en je aan deadlines herinneren",
            ],
            ask: "Waar werk je nu aan?",
            chips: [
                "Help me mijn week te plannen.",
                "Vat een document voor me samen.",
                "Help me met een lastige mail.",
            ],
        },
        ("nl", "learning") => Text {
            about: "Ik help je elke dag een beetje te leren.",
            can: [
                "je korte lessen op jouw niveau geven",
                "je overhoren om te zien wat je hebt onthouden",
                "je herinneren aan je oefenmoment",
            ],
            ask: "Wat zou je graag leren?",
            chips: [
                "Ik wil Spaans leren.",
                "Leg me een onderwerp eenvoudig uit.",
                "Laat me elke dag 10 minuten herhalen.",
            ],
        },
        ("nl", "creative") => Text {
            about: "Ik ben er voor je ideeën en creatieve projecten.",
            can: [
                "samen met jou ideeën bedenken",
                "een tekst schrijven of nalezen",
                "je project stap voor stap volgen",
            ],
            ask: "Wat wil je maken?",
            chips: [
                "Help me ideeën te bedenken.",
                "Lees een tekst die ik heb geschreven.",
                "Ik heb een project, help me beginnen.",
            ],
        },
        ("nl", "tech") => Text {
            about: "Ik help je met computers en apparaten.",
            can: [
                "een probleem uitleggen en hoe je het oplost",
                "de juiste stappen op internet opzoeken",
                "je stap voor stap door een instelling leiden",
            ],
            ask: "Waar loop je tegenaan?",
            chips: [
                "Mijn computer is traag, wat nu?",
                "Help me mijn accounts beter te beveiligen.",
                "Hoe maak ik een back-up van mijn foto's?",
            ],
        },
        ("nl", _) => Text {
            about: "Ik ben er om je te helpen.",
            can: [
                "iets op internet opzoeken en voor je samenvatten",
                "een document of foto lezen die je me stuurt",
                "je op het juiste moment ergens aan herinneren",
            ],
            ask: "Vertel me wat je van me verwacht, dan pas ik me aan.",
            chips: ["Dit zou ik graag van je willen.", "Wat kun je allemaal?", "Herinner me morgen ergens aan."],
        },
        // -- English (and any other language) -------------------------------------------------------------------
        (_, "guide") => Text {
            about: "I'm here for your everyday life, and to help you set up the right agents.",
            can: [
                "look something up online and sum it up for you",
                "remind you of an appointment or a thing to do",
                "suggest agents just for you: budget, health, home…",
            ],
            ask: "To start, tell me a bit more: what takes up your time at the moment?",
            chips: [
                "My family: homework, appointments, shopping.",
                "My work: e-mails, meetings, things not to forget.",
                "My budget: spending, subscriptions, bills.",
            ],
        },
        (_, "daily") => Text {
            about: "I take care of your everyday life.",
            can: [
                "give you a morning briefing: weather, appointments, reminders",
                "look something up online and sum it up for you",
                "remind you of a thing to do at the right time",
            ],
            ask: "Where would you like to start?",
            chips: [
                "Give me a briefing every morning at 7:30.",
                "Remind me to call the doctor tomorrow.",
                "What's the weather like this weekend?",
            ],
        },
        (_, "wellness") => Text {
            about: "I'm here for your health and well-being.",
            can: [
                "plan workouts at your own pace",
                "help you sleep better and keep your habits",
                "remind you of appointments and medication",
            ],
            ask: "What matters most to you right now?",
            chips: [
                "I'd like to get back into exercise, gently.",
                "Help me sleep better.",
                "Remind me to drink water during the day.",
            ],
        },
        (_, "finance") => Text {
            about: "I help you keep an eye on your money.",
            can: [
                "go through your spending and subscriptions",
                "read a bill or statement you send me",
                "remind you before a payment is due",
            ],
            ask: "Where shall we start?",
            chips: [
                "Help me go through my subscriptions.",
                "I'd like a simple monthly budget.",
                "Remind me to pay my bills.",
            ],
        },
        (_, "home") => Text {
            about: "I look after the home with you.",
            can: [
                "make the shopping list and plan meals",
                "organise cleaning and small repairs",
                "keep your papers in order and remind you of what's coming",
            ],
            ask: "What would make life at home easier?",
            chips: ["Suggest meals for the week.", "Make me a shopping list.", "Help me organise the cleaning."],
        },
        (_, "work") => Text {
            about: "I help you with your work.",
            can: [
                "prepare a meeting or a message",
                "read a document and pull out what matters",
                "track your tasks and remind you of deadlines",
            ],
            ask: "What are you working on at the moment?",
            chips: ["Help me plan my week.", "Sum up a document for me.", "Help me write a tricky e-mail."],
        },
        (_, "learning") => Text {
            about: "I help you learn a little every day.",
            can: [
                "give you short lessons at your level",
                "quiz you to see what you remember",
                "remind you of your session at the time you choose",
            ],
            ask: "What would you like to learn?",
            chips: ["I'd like to learn Spanish.", "Explain a topic to me simply.", "Have me revise 10 minutes a day."],
        },
        (_, "creative") => Text {
            about: "I'm here for your ideas and creative projects.",
            can: ["come up with ideas with you", "write or proofread a text", "follow your project step by step"],
            ask: "What would you like to create?",
            chips: ["Help me find ideas.", "Read a text I wrote.", "I have a project, help me get it going."],
        },
        (_, "tech") => Text {
            about: "I help you with computers and devices.",
            can: [
                "explain a problem and how to fix it",
                "find the right steps online",
                "walk you through a setting step by step",
            ],
            ask: "What's giving you trouble?",
            chips: [
                "My computer is slow, what can I do?",
                "Help me protect my accounts better.",
                "How do I back up my photos?",
            ],
        },
        (_, _) => Text {
            about: "I'm here to help you.",
            can: [
                "look something up online and sum it up for you",
                "read a document or photo you send me",
                "remind you of a thing to do at the right time",
            ],
            ask: "Tell me what you'd like from me, and I'll adapt.",
            chips: ["Here's what I'd like you to do for me.", "What can you do?", "Remind me of something tomorrow."],
        },
    }
}

/// The welcome's language: one of `LANGUAGES` (« fr-FR » → fr), English otherwise.
pub fn language(code: &str) -> &'static str {
    let code = code.trim().to_lowercase();
    LANGUAGES.iter().find(|l| code.starts_with(**l)).copied().unwrap_or("en")
}

/// The welcome's kind: the guide's own, a category, else « custom ».
pub fn kind_of(kind: &str, guide: bool) -> &'static str {
    if guide {
        return "guide";
    }
    KINDS.iter().find(|k| **k == kind && **k != "guide").copied().unwrap_or("custom")
}

/// The first message of an agent named `name`: its text and the suggestions the apps show as chips.
pub fn message(name: &str, kind: &str, lang: &str, guide: bool) -> (String, Vec<String>) {
    let lang = language(lang);
    let kind = kind_of(kind, guide);
    let words = text(kind, lang);
    let frame = frame(lang);
    let mut out = format!("{} {}\n\n{}\n", frame.hello.replace("{name}", name.trim()), words.about, frame.can);
    for thing in words.can {
        out.push_str(&format!("• {thing}\n"));
    }
    out.push('\n');
    if guide {
        out.push_str(frame.rename);
        out.push('\n');
    }
    out.push_str(words.ask);
    (out, words.chips.iter().map(|c| c.to_string()).collect())
}

/// `GET /v1/guide`'s `welcome`: each agent made with one → {text, suggestions, session (null: not written to yet)}.
pub fn view(config: &Config, state: &crate::guide::GuideState) -> Value {
    let mut out = serde_json::Map::new();
    for (id, waiting) in &state.welcome {
        let Some(agent) = config.agents.get(id) else { continue };
        if waiting.session.as_deref() == Some("") {
            continue; // written to in a conversation the bridge didn't learn: nowhere to show it
        }
        let guide = state.guide.as_deref() == Some(id.as_str());
        let (text, suggestions) = message(&agent.display_name, &waiting.kind, &waiting.lang, guide);
        out.insert(id.clone(), json!({"text": text, "suggestions": suggestions, "session": waiting.session}));
    }
    Value::Object(out)
}

/// `agent` was just made for the person: its first conversation opens with its welcome (once: a welcome already
/// given stays as it is).
pub fn wait_for(config: &Config, agent: &AgentConfig, kind: &str, lang: &str) {
    let waiting = Waiting { kind: kind.to_string(), lang: language(lang).to_string(), session: None };
    let result = crate::guide::change(config, |s| {
        s.welcome.entry(agent.name.clone()).or_insert(waiting);
    });
    if let Err(err) = result {
        tracing::warn!(target: "bipbridge::guide", agent = %agent.name, error = %err, "welcome not kept");
    }
}

/// The person wrote to `agent` for the first time, in `session`: its welcome stays at the top of that conversation
/// only. Later messages change nothing.
pub fn done(config: &Config, agent: &str, session: Option<&str>) {
    let waiting = |s: &crate::guide::GuideState| s.welcome.get(agent).is_some_and(|w| w.session.is_none());
    if !waiting(&crate::guide::load(config)) {
        return;
    }
    let session = session.filter(|s| s.len() <= 200).unwrap_or("").to_string();
    let result = crate::guide::change(config, |s| {
        if let Some(w) = s.welcome.get_mut(agent).filter(|w| w.session.is_none()) {
            w.session = Some(session);
        }
    });
    if let Err(err) = result {
        tracing::warn!(target: "bipbridge::guide", agent = %agent, error = %err, "welcome not ended");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_a_welcome_in_every_language() {
        for lang in LANGUAGES {
            let frame = frame(lang);
            assert!(frame.hello.contains("{name}") && !frame.can.is_empty() && !frame.rename.is_empty(), "{lang}");
            for kind in KINDS {
                let words = text(kind, lang);
                assert!(!words.about.trim().is_empty() && !words.ask.trim().is_empty(), "{kind} {lang}");
                assert!(words.can.iter().all(|c| !c.trim().is_empty()), "{kind} {lang}");
                assert!(words.chips.iter().all(|c| !c.trim().is_empty() && c.chars().count() <= 80), "{kind} {lang}");
                let (text, chips) = message("Bip", kind, lang, kind == "guide");
                assert!(text.starts_with(&frame.hello.replace("{name}", "Bip")), "{kind} {lang}: {text}");
                assert_eq!(text.matches("• ").count(), 3, "{kind} {lang}");
                assert_eq!(chips.len(), 3);
                assert_eq!(text.contains(frame.rename), kind == "guide", "{kind} {lang}");
            }
        }
        // the languages are really their own (not English everywhere)
        let english = text("daily", "en").about;
        for lang in ["fr", "de", "es", "nl"] {
            assert_ne!(text("daily", lang).about, english, "{lang}");
            assert_ne!(text("guide", lang).about, text("guide", "en").about, "{lang}");
        }
    }

    #[test]
    fn languages_and_kinds_fall_back() {
        assert_eq!(language("fr-FR"), "fr");
        assert_eq!(language("nl_BE"), "nl");
        assert_eq!(language("it"), "en");
        assert_eq!(language(""), "en");
        assert_eq!(kind_of("finance", false), "finance");
        assert_eq!(kind_of("finance", true), "guide");
        assert_eq!(kind_of("guide", false), "custom"); // only the guide itself says it proposes agents
        assert_eq!(kind_of("family", false), "custom");
        let (text, chips) = message("Sous", "finance", "fr", false);
        assert!(text.starts_with("Bonjour, je suis Sous ! Je t'aide à garder un œil sur ton argent."), "{text}");
        assert!(text.ends_with("Par quoi veux-tu qu'on commence ?"), "{text}");
        assert_eq!(chips[0], "Aide-moi à faire le point sur mes abonnements.");
        let (text, _) = message("Bip", "daily", "fr-FR", true);
        assert!(text.contains("Tu préfères m'appeler autrement ?"), "{text}");
    }
}
