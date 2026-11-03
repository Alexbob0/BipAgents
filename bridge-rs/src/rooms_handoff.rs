//! The relay in a group discussion (rooms.rs; Alex, 2026-10-31: « qu'ils se taguent en fonction de l'objectif », not a
//! round of the table). The pure parts: what an agent's reply says about who acts next, the conductor's prompt and
//! answer, a message that looks like a task, the room's goal and where it stands.
//!
//! **Two ways a room works**:
//! - **Discussion** (no goal, the default): for each message of the person, ONE agent answers (the most concerned);
//!   the others speak when tagged, or when the one answering really needs a colleague (`NEXT: @id`, up to
//!   `[rooms] max_hops` hops). Then the hand goes back to the person. No conductor call after a turn.
//! - **Team work** (a goal the person accepted): after each turn the agent named next acts, else the conductor (one
//!   small model call) picks who moves the goal forward, or gives the hand back to the person; at most
//!   `[rooms] team_turns` turns and `team_tokens` tokens per message of the person; no back-and-forth past 3 turns
//!   between two agents.
//!
//! **The relay mark**: the run's instructions ask each agent to end its reply with one line
//! `NEXT: @<id> — <why>` (or `NEXT: person — <the decision needed>`, or `NEXT: none`), in team work also
//! `DONE: <what it just did>`, and to reply only `PASS` when it has nothing useful to add. The bridge reads them
//! leniently (case, bold, brackets, arrows, a name instead of an id, French/German/Spanish words) from the reply's last
//! lines, takes them out of the text the person and the others read, and keeps the relay on the message (`handoff`:
//! {to: <id>|"person"|"none", why, by: "agent"|"conductor"|"budget"}).
//!
//! **The goal** is never imposed: a message that looks like a task (`looks_like_task`, then a small model call that
//! may say « none ») gets a goal *proposed* (`goal_proposed`); the person accepts it (team work starts), edits it or
//! ignores it. Where it stands (`progress`: done, next, blocked) follows the relays, a few lines.
//!
//! No Python counterpart.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tools_knowledge as tk;

/// A goal, at most (characters).
pub const GOAL_CHARS: usize = 160;
/// A relay's why, a progress line, at most.
pub const WHY_CHARS: usize = 120;
/// The « done » lines kept.
pub const DONE_KEPT: usize = 5;
/// Turns two agents may take one after the other (A, B, A) before a third is asked.
pub const PING_PONG: usize = 3;
/// An agent who spoke this many times in the round is not handed the turn again without the conductor's view.
pub const SPOKE_MAX: usize = 2;
pub const CONDUCTOR_MAX_TOKENS: i64 = 60;
pub const GOAL_MAX_TOKENS: i64 = 50;
/// What the conductor reads of the last reply and of the person's message.
pub const CONDUCTOR_REPLY_CHARS: usize = 1200;
pub const CONDUCTOR_MESSAGE_CHARS: usize = 400;

pub const CONDUCTOR_INSTRUCTIONS: &str = "You coordinate a team of agents working toward a goal in a group chat. \
Choose who acts next. Answer one line only: `<agent id> — <why, under 12 words>`; or `person — <the decision or \
information needed>` when the person must decide or answer; or `none — <why>` when the person's last request is done \
or nobody has something useful to add. Pick the agent whose skill moves the work forward most at this step; never \
one with nothing new to bring; avoid back-and-forth between the same two agents. Write the why in the language of \
the discussion.";

pub const ROUTER_ONE_INSTRUCTIONS: &str = "You route a message of a group chat to the agent who should answer it. \
Reply with only the id of the one most concerned agent. A piece of work to do (build, write, plan, organise…) \
goes to one agent only, who will hand over to colleagues itself. Two ids, comma-separated, only if the message asks \
two separate questions for two different skills.";

pub const GOAL_INSTRUCTIONS: &str = "Decide whether this message asks a team for a real piece of work with a result \
to deliver (an app, a plan, a document, an event to organise…). If it does, write its goal in one short line (at \
most 12 words), in the message's language, no quotes. If it is a question, a chat or a small request, answer \
exactly: none";

/// Where a team's work stands, a few lines (team work only).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    /// « Design : maquette des 2 écrans », the latest last.
    #[serde(default)]
    pub done: Vec<String>,
    /// « Front : coder l'écran de liste ».
    #[serde(default)]
    pub next: String,
    /// What the person must decide.
    #[serde(default)]
    pub blocked: String,
}

impl Progress {
    pub fn is_empty(&self) -> bool {
        self.done.is_empty() && self.next.is_empty() && self.blocked.is_empty()
    }
}

/// A goal the bridge proposes, from the person's message `message`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GoalProposal {
    pub text: String,
    #[serde(default)]
    pub message: u64,
}

/// Who acts next, as an agent's reply or the conductor says it.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    Agent(String),
    Person,
    Done,
    /// A name the bridge doesn't know (kept for the conductor's prompt).
    Unknown(String),
}

/// What a reply's relay lines said.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Marks {
    /// The reply without its NEXT/DONE lines (empty, or `pass`: it had nothing to add).
    pub text: String,
    pub next: Option<(Next, String)>,
    pub done: Option<String>,
    pub pass: bool,
}

static NEXT_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^[\s>*_`•\-\[(]*(next|suivant|handoff|hand off|n[äa]chste[rs]?|siguiente)[\s*_`]*(?:[:：=]|->|→|=>|—|–)+[\s*_`]*(.*?)[\s*_`\])]*$",
    )
    .expect("valid regex")
});
static DONE_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[\s>*_`•\-\[(]*(done|fait|erledigt|hecho)[\s*_`]*(?:[:：=]|->|→|—|–)+[\s*_`]*(.*?)[\s*_`\])]*$")
        .expect("valid regex")
});
static PASS_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[\s*_`\[(]*(pass|passe|je passe|rien à ajouter|rien a ajouter|nothing to add)[\s.!*_`\])]*$")
        .expect("valid regex")
});
static WHY_SPLIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s+(?:—|–|-|:|\||because|car|parce que|pour|to|weil|porque)\s+|\s*[—–|:(,]\s*").expect("valid regex")
});

const PERSON_WORDS: &[&str] = &[
    "person",
    "personne",
    "the person",
    "la personne",
    "user",
    "utilisateur",
    "you",
    "toi",
    "vous",
    "human",
    "humain",
    "your person",
    "ta personne",
    "person",
    "persona",
    "mensch",
    "nutzer",
    "usuario",
];
const NONE_WORDS: &[&str] = &[
    "none", "nobody", "no one", "noone", "rien", "aucun", "aucune", "done", "fin", "finished", "termine", "fini",
    "nichts", "niemand", "nada", "nadie", "ninguno", "end", "stop", "-",
];

fn fold(text: &str) -> String {
    crate::rooms::fold_text(text)
}

fn handle(text: &str) -> String {
    fold(text).chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// `who` (an id, a name, `person`, `none`…) among `names` (id, name).
pub fn target(who: &str, names: &[(String, String)]) -> Next {
    let raw = who.trim().trim_start_matches('@').trim_matches(|c: char| "*_`'\"«».,;!?[]()".contains(c)).trim();
    let folded = fold(raw).split_whitespace().collect::<Vec<_>>().join(" ");
    if folded.is_empty() || NONE_WORDS.contains(&folded.as_str()) {
        return Next::Done;
    }
    if PERSON_WORDS.contains(&folded.as_str()) {
        return Next::Person;
    }
    let key = handle(raw);
    let found = names.iter().find(|(id, name)| {
        let first = name.split_whitespace().next().map(handle).unwrap_or_default();
        handle(id) == key || handle(name) == key || (!first.is_empty() && first == key)
    });
    match found {
        Some((id, _)) => Next::Agent(id.clone()),
        None => Next::Unknown(tk::head(raw, 40).to_string()),
    }
}

/// A NEXT line's rest: who, then why. The who may be several words (« Chef de projet ») before the separator.
pub fn split_next(rest: &str, names: &[(String, String)]) -> (Next, String) {
    let rest = rest.trim();
    // the longest beginning that names someone, else the first word
    let words: Vec<&str> = rest.split_whitespace().collect();
    for n in (1..=words.len().min(4)).rev() {
        let who = words[..n].join(" ");
        let who = who.trim_end_matches(|c: char| "—–-:|,(".contains(c));
        let next = target(who, names);
        if !matches!(next, Next::Unknown(_)) {
            return (next, clean_why(&words[n..].join(" ")));
        }
    }
    let mut parts = WHY_SPLIT.splitn(rest, 2);
    let who = parts.next().unwrap_or("");
    let why = parts.next().unwrap_or("");
    (target(who, names), clean_why(why))
}

fn clean_why(why: &str) -> String {
    let why = why.trim().trim_start_matches(|c: char| "—–-:|,(".contains(c) || c.is_whitespace());
    let why = why.trim_end_matches(|c: char| "*_`)]".contains(c)).trim();
    tk::head(why, WHY_CHARS).to_string()
}

/// The reply's relay lines read (from its last lines) and taken out of its text.
pub fn read_marks(reply: &str, names: &[(String, String)]) -> Marks {
    let mut lines: Vec<&str> = reply.lines().collect();
    let mut marks = Marks::default();
    // the marks are at the end: look at the last few non-empty lines only (a « Next: » in the body stays)
    let mut looked = 0;
    let mut i = lines.len();
    while i > 0 && looked < 6 {
        i -= 1;
        let line = lines[i].trim();
        if line.is_empty() {
            continue;
        }
        looked += 1;
        if let Some(caps) = NEXT_LINE.captures(line) {
            let next = split_next(&caps[2], names);
            // « Next: we deploy » in the body is no mark; « NEXT: @bob » (nobody here) is one
            if matches!(next.0, Next::Unknown(_)) && !caps[2].trim_start().starts_with('@') {
                continue;
            }
            if marks.next.is_none() {
                marks.next = Some(next);
            }
            lines.remove(i);
        } else if let Some(caps) = DONE_LINE.captures(line) {
            if marks.done.is_none() {
                let done = clean_why(&caps[2]);
                if !done.is_empty() {
                    marks.done = Some(done);
                }
            }
            lines.remove(i);
        }
    }
    let text = tk::strip(&lines.join("\n")).to_string();
    marks.pass = text.is_empty() || PASS_TEXT.is_match(&text);
    marks.text = if marks.pass { String::new() } else { text };
    marks
}

/// The relay as kept on a message.
pub fn handoff_json(next: &Next, why: &str, by: &str) -> Value {
    let to = match next {
        Next::Agent(id) => id.clone(),
        Next::Person => "person".into(),
        Next::Done | Next::Unknown(_) => "none".into(),
    };
    json!({"to": to, "why": tk::head(why.trim(), WHY_CHARS), "by": by})
}

/// Two agents passing the turn back and forth: `speakers` (this round's, in order) end with `me, next, me` already.
pub fn ping_pong(speakers: &[String], me: &str, next: &str) -> bool {
    let n = speakers.len();
    if n < PING_PONG {
        return false;
    }
    // with `next` added, the last PING_PONG + 1 turns would only be these two
    let tail = &speakers[n - PING_PONG..];
    tail.iter().all(|s| s == me || s == next)
        && tail.last().map(String::as_str) == Some(me)
        && tail.iter().any(|s| s == next)
}

/// The conductor's prompt (user part).
#[allow(clippy::too_many_arguments)]
pub fn conductor_prompt(
    goal: &str,
    progress: &Progress,
    lines: &[(String, String)],
    spoke: &[String],
    person: &str,
    last: Option<(&str, &str, &str)>,
    refused: Option<&str>,
    turns_left: usize,
) -> String {
    let mut out = format!("Goal: {}\n", if goal.is_empty() { "(none)" } else { goal });
    if !progress.is_empty() {
        out.push_str("Where it stands:");
        if !progress.done.is_empty() {
            out.push_str(&format!(" done: {};", progress.done.join("; ")));
        }
        if !progress.next.is_empty() {
            out.push_str(&format!(" next: {};", progress.next));
        }
        if !progress.blocked.is_empty() {
            out.push_str(&format!(" waiting for the person: {};", progress.blocked));
        }
        out.push('\n');
    }
    out.push_str("\nAgents:\n");
    for (id, line) in lines {
        let n = spoke.iter().filter(|s| *s == id).count();
        let said = if n == 0 { String::new() } else { format!(" [spoke {n}× since the person's message]") };
        out.push_str(&format!("- {id}: {}{said}\n", tk::head(line, crate::rooms::ROLE_CHARS)));
    }
    out.push_str(&format!("\nThe person's last message:\n{}\n", tk::head(tk::strip(person), CONDUCTOR_MESSAGE_CHARS)));
    if let Some((id, name, text)) = last {
        out.push_str(&format!(
            "\nLast reply, by {name} (@{id}):\n{}\n",
            tk::head(tk::strip(text), CONDUCTOR_REPLY_CHARS)
        ));
    }
    if let Some(refused) = refused {
        out.push_str(&format!("\n({refused})\n"));
    }
    out.push_str(&format!("\nTurns left before the person gets the hand back: {turns_left}."));
    out
}

/// The conductor's answer: who and why (an agent it names must be one of `names`; anything else: none).
pub fn parse_conductor(answer: &str, names: &[(String, String)]) -> (Next, String) {
    let line = answer.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line = NEXT_LINE.captures(line).map(|c| c[2].to_string()).unwrap_or_else(|| line.to_string());
    match split_next(line.trim_matches('`'), names) {
        (Next::Unknown(_), why) => (Next::Done, why),
        other => other,
    }
}

/// Words that make a message look like work for a team (folded). The small model then says yes (a goal) or none.
const TASK_WORDS: &[&str] = &[
    "faites",
    "fais moi",
    "faites moi",
    "fais nous",
    "faites nous",
    "creez",
    "cree moi",
    "creer",
    "construisez",
    "construis",
    "developpez",
    "developpe",
    "on doit",
    "il faut",
    "on va faire",
    "preparez",
    "prepare moi",
    "realisez",
    "organisez",
    "organise",
    "concevez",
    "codez",
    "redigez",
    "ecrivez",
    "livrer",
    "mettez en place",
    "lancez",
    "montez",
    "fabriquez",
    "planifiez",
    "build",
    "make me",
    "make us",
    "create",
    "develop",
    "we need to",
    "we must",
    "let s make",
    "let s build",
    "implement",
    "prepare",
    "organize",
    "organise",
    "ship",
    "deliver",
    "set up",
    "design",
    "write",
    "baut",
    "erstellt",
    "macht mir",
    "wir mussen",
    "entwickelt",
    "plant",
    "haced",
    "cread",
    "hazme",
    "hacednos",
    "tenemos que",
    "desarrollad",
    "preparad",
    "organizad",
];

/// A message that may ask for a piece of work (a few words at least, a task word): worth asking the small model.
pub fn looks_like_task(text: &str) -> bool {
    let folded = fold(text);
    let words: Vec<&str> = folded.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    if words.len() < 4 {
        return false;
    }
    let spaced = format!(" {} ", words.join(" "));
    TASK_WORDS.iter().any(|w| spaced.contains(&format!(" {w} ")))
}

/// The goal the small model wrote, or None (« none », empty, too long to be one line).
pub fn parse_goal(answer: &str) -> Option<String> {
    let line = answer.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line.trim_matches(|c: char| "\"'«»`*. ".contains(c)).trim();
    let line = line.strip_prefix("Goal:").or_else(|| line.strip_prefix("Objectif :")).unwrap_or(line).trim();
    let folded = fold(line);
    if line.is_empty() || folded == "none" || folded.starts_with("none ") || folded == "aucun" || folded == "rien" {
        return None;
    }
    // asked for 12 words at most: a long line or a question is the model thinking aloud, not a goal
    if line.split_whitespace().count() > 20 || line.contains('?') {
        return None;
    }
    Some(tk::head(line, GOAL_CHARS).to_string())
}

/// The progress after a turn: what it did, who's next (or what the person must decide).
pub fn advance(progress: &mut Progress, name: &str, done: Option<&str>, next: &Next, why: &str, next_name: &str) {
    if let Some(done) = done.filter(|d| !d.is_empty()) {
        progress.done.push(tk::head(&format!("{name} : {done}"), WHY_CHARS + 30).to_string());
        if progress.done.len() > DONE_KEPT {
            let extra = progress.done.len() - DONE_KEPT;
            progress.done.drain(..extra);
        }
    }
    match next {
        Next::Agent(_) => {
            progress.next = tk::head(
                &if why.is_empty() { next_name.to_string() } else { format!("{next_name} : {why}") },
                WHY_CHARS + 30,
            )
            .to_string();
            progress.blocked.clear();
        }
        Next::Person => {
            progress.next.clear();
            progress.blocked = tk::head(why, WHY_CHARS).to_string();
        }
        Next::Done | Next::Unknown(_) => {
            progress.next.clear();
            progress.blocked.clear();
        }
    }
}

/// The run's instructions' relay part: discussion (no goal) or team work.
pub fn relay_rules(team: bool) -> &'static str {
    if team {
        "This discussion has a goal your person accepted: work toward it as a team, each from their own role. Do \
your part of the next step now (concretely, not a plan of what you will do), briefly. If you have nothing useful to \
add, reply only PASS. End your reply with two lines: `DONE: <what you just did, a few words>` and \
`NEXT: @<id> — <why, a few words>` naming the one agent best placed for the next step, or `NEXT: person — <the \
decision or information you need>` when your person must decide, or `NEXT: none` when the step asked is finished."
    } else {
        "Your person usually wants one answer, from the agent most concerned: answer, then end with the line \
`NEXT: person`. Only if you really need a colleague's skill to answer, end with `NEXT: @<id> — <why, a few words>` \
instead (it then gets one turn). If you have nothing useful to add, reply only PASS; if the message is rather for a \
colleague (it continues what they just said, or is about their field), reply PASS and end with `NEXT: @<id>` naming \
them. Never call everyone."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn team() -> Vec<(String, String)> {
        [("pm", "Chef de projet"), ("design", "Design"), ("front", "Front"), ("back", "Back"), ("qa", "Tests")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn the_next_line_is_read_leniently_and_taken_out() {
        let n = team();
        let m =
            read_marks("Voici la maquette.\n\nDONE: maquette des 2 écrans\nNEXT: @front — coder l'écran de liste", &n);
        assert_eq!(m.text, "Voici la maquette.");
        assert_eq!(m.done.as_deref(), Some("maquette des 2 écrans"));
        assert_eq!(m.next, Some((Next::Agent("front".into()), "coder l'écran de liste".into())));
        assert!(!m.pass);
        for line in [
            "**NEXT:** @front - coder l'écran",
            "Next → Front: coder l'écran",
            "[NEXT: front | coder l'écran]",
            "- next: @Front (coder l'écran)",
            "Suivant : @front — coder l'écran",
            "NEXT:@front coder l'écran",
        ] {
            let m = read_marks(&format!("Texte.\n{line}"), &n);
            assert_eq!(m.text, "Texte.", "{line}");
            assert!(
                matches!(&m.next, Some((Next::Agent(a), why)) if a == "front" && why.contains("coder")),
                "{line}: {:?}",
                m.next
            );
        }
        let m = read_marks("Ok.\nNEXT: @Chef de projet — valider le périmètre", &n);
        assert_eq!(m.next, Some((Next::Agent("pm".into()), "valider le périmètre".into())));
        let m = read_marks("Question.\nNEXT: person — choisir web ou mobile", &n);
        assert_eq!(m.next, Some((Next::Person, "choisir web ou mobile".into())));
        let m = read_marks("Fini.\nNEXT: none", &n);
        assert_eq!(m.next, Some((Next::Done, String::new())));
        let m = read_marks("Fini.\nNEXT: @bob — x", &n);
        assert!(matches!(m.next, Some((Next::Unknown(_), _))));
        // a « next: » in the middle of the body stays
        let body = "Plan:\nnext: the API\nthen tests\nmore\nlines\nhere\nand here\nend";
        assert_eq!(read_marks(body, &n).text, body);
        assert!(read_marks(body, &n).next.is_none());
        // no mark at all
        assert_eq!(
            read_marks("Juste une réponse.", &n),
            Marks { text: "Juste une réponse.".into(), ..Default::default() }
        );
    }

    #[test]
    fn pass_is_silence() {
        let n = team();
        for reply in ["PASS", "pass.", "**PASS**\nNEXT: @qa — tester", "Rien à ajouter.", "NEXT: none"] {
            let m = read_marks(reply, &n);
            assert!(m.pass && m.text.is_empty(), "{reply}");
        }
        assert!(!read_marks("Je passe à l'étape 2 : voici le code.", &n).pass);
    }

    #[test]
    fn ping_pong_stops_at_the_fourth_turn() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(!ping_pong(&s(&["pm"]), "pm", "design"));
        assert!(!ping_pong(&s(&["design", "pm"]), "pm", "design")); // design pm design: 3 turns, fine
        assert!(ping_pong(&s(&["pm", "design", "pm"]), "pm", "design")); // a 4th between the same two
        assert!(!ping_pong(&s(&["pm", "front", "pm"]), "pm", "design"));
        assert!(!ping_pong(&s(&["front", "design", "pm"]), "pm", "design"));
    }

    #[test]
    fn the_conductor_prompt_and_answer() {
        let n = team();
        let lines: Vec<(String, String)> = n.iter().map(|(id, name)| (id.clone(), format!("{name} (work)"))).collect();
        let progress =
            Progress { done: vec!["Design : maquette".into()], next: "Front : coder".into(), blocked: String::new() };
        let p = conductor_prompt(
            "App web de liste de courses",
            &progress,
            &lines,
            &["pm".into(), "design".into(), "pm".into()],
            "faites-moi une app",
            Some(("design", "Design", "Voici la maquette")),
            Some("Design proposed @bob, who isn't here"),
            4,
        );
        assert!(p.starts_with(
            "Goal: App web de liste de courses\nWhere it stands: done: Design : maquette; next: Front : coder;"
        ));
        assert!(p.contains("- pm: Chef de projet (work) [spoke 2× since the person's message]"));
        assert!(p.contains("- front: Front (work)\n"));
        assert!(p.contains("Last reply, by Design (@design):\nVoici la maquette"));
        assert!(p.ends_with("Turns left before the person gets the hand back: 4."));
        assert_eq!(parse_conductor("front — coder l'écran", &n), (Next::Agent("front".into()), "coder l'écran".into()));
        assert_eq!(parse_conductor("`qa` - tester\nblah", &n), (Next::Agent("qa".into()), "tester".into()));
        assert_eq!(
            parse_conductor("NEXT: person — choisir les couleurs", &n),
            (Next::Person, "choisir les couleurs".into())
        );
        assert_eq!(parse_conductor("none — c'est livré", &n).0, Next::Done);
        assert_eq!(parse_conductor("someone else", &n).0, Next::Done);
        assert_eq!(parse_conductor("", &n).0, Next::Done);
    }

    #[test]
    fn tasks_and_goals() {
        assert!(looks_like_task("Faites-moi une petite app web de liste de courses"));
        assert!(looks_like_task("On doit livrer le site du club vendredi"));
        assert!(looks_like_task("Build me a small landing page for the bakery"));
        assert!(!looks_like_task("Quel temps fera-t-il demain ?"));
        assert!(!looks_like_task("Faites-moi rire"));
        assert!(!looks_like_task("Salut tout le monde"));
        assert_eq!(
            parse_goal("App web de liste de courses partagée"),
            Some("App web de liste de courses partagée".into())
        );
        assert_eq!(parse_goal("\"Livrer le site du club.\""), Some("Livrer le site du club".into()));
        assert_eq!(parse_goal("none"), None);
        // a thinking model's thoughts are no goal (DeepSeek on the lab, 2026-10-31)
        assert_eq!(parse_goal("We need answer in French? The user asks: « Faites-moi une petite app web »"), None);
        assert_eq!(parse_goal("None."), None);
        assert_eq!(parse_goal(""), None);
    }

    #[test]
    fn progress_follows_the_relays() {
        let mut p = Progress::default();
        advance(&mut p, "Design", Some("maquette"), &Next::Agent("front".into()), "coder l'écran", "Front");
        assert_eq!(
            p,
            Progress {
                done: vec!["Design : maquette".into()],
                next: "Front : coder l'écran".into(),
                blocked: String::new()
            }
        );
        advance(&mut p, "Front", None, &Next::Person, "choisir les couleurs", "");
        assert_eq!((p.next.as_str(), p.blocked.as_str()), ("", "choisir les couleurs"));
        for i in 0..7 {
            advance(&mut p, "Back", Some(&format!("api {i}")), &Next::Done, "", "");
        }
        assert_eq!(p.done.len(), DONE_KEPT);
        assert_eq!(p.done.last().unwrap(), "Back : api 6");
        assert!(p.next.is_empty() && p.blocked.is_empty());
    }
}
