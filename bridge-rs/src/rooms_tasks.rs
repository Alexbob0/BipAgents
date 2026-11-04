//! Tasks in a room and « Réassigner » (Alex, 2026-11-02, after OpenBot's team rooms): each piece of work in a room
//! has a visible owner (the agent working on it) and a state, and the person can hand it to another agent
//! (« c'est plutôt pour Research »), over the router's own pick.
//!
//! **A task** starts with a message of the person (rooms.rs `post`): its first words are its title, the agent picked
//! to answer its owner. A message that doesn't look like a new piece of work continues the open task instead when the
//! person chose its owner, or in team work (a goal). The owner follows who works on it (each turn's agent; in team
//! work the agent handed the turn) until the person chooses one: then it stays, and the agents the person took it
//! from are never handed it again by the relay or the conductor. States: `running` (en cours), `waiting` (en attente:
//! the person's answer, or stopped), `done` (fini: `NEXT: none`, or the person moved on to another task). A plain
//! question (not a piece of work, no goal) is a task too, so any answer can be handed over, but `question` keeps it out
//! of the room's « En cours » line, which the « … écrit » line already says.
//!
//! **Reassigning** (`POST /v1/rooms/{id}/tasks/{task}/reassign {agent}`): the owner changes at once (every device sees
//! it at its next look: `tasks` in the room and in each look at its messages), a notice says it in the room
//! (« Research reprend « Plan du voyage ». », `handover: {task, from, to}`), the old owner's queued turns on it go,
//! and the new owner gets a turn with a short handover note (what was done, what is left):
//! - the old owner is working on it now: its run is stopped and it writes the note itself in one short turn without
//!   tools (rooms.rs `wrap_up`, within its turn's cap), kept as its message; the new owner reads it with the rest;
//! - else one small call writes it (two lines, « Fait : … » / « Reste : … », `[rooms] router_model`, counted in
//!   `small_tokens` and `router_daily_max`), else the old owner's last words on it. The new owner's turn starts with it.
//!
//! Costs and caps stay per turn as before: the new owner's turn has its usual cap. Rooms without the relay
//! (`[rooms] handoff = false`) keep no tasks. No Python counterpart.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::rooms::{Message, Room};
use crate::rooms_handoff::{self as relay, Next};
use crate::tools_knowledge as tk;

/// Tasks kept per room (the oldest go).
pub const TASKS_KEPT: usize = 20;
/// A task's title: its message's first words.
pub const TITLE_CHARS: usize = 60;
/// The handover note kept on the task.
pub const NOTE_CHARS: usize = 400;
/// What the small call reads of each message, and of all of them.
pub const NOTE_MESSAGE_CHARS: usize = 500;
pub const NOTE_INPUT_CHARS: usize = 2400;
pub const NOTE_MAX_TOKENS: i64 = 80;

pub const NOTE_INSTRUCTIONS: &str = "Write the handover note for an agent taking over a task in a group chat: two \
short lines, « Done: … » and « Left: … », a few words each, in the language of the messages. Nothing else.";

pub const RUNNING: &str = "running";
pub const WAITING: &str = "waiting";
pub const DONE: &str = "done";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Handover {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub at: f64,
    /// The notice that says it in the room.
    #[serde(default)]
    pub message: u64,
    /// What was done and what is left (empty until written).
    #[serde(default)]
    pub note: String,
    /// The note is the old owner's own message in the room (the new owner reads it there).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_room: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: u32,
    pub title: String,
    /// The agent working on it.
    pub owner: String,
    /// `running`, `waiting` or `done`.
    pub state: String,
    /// The person's message it started with.
    #[serde(default)]
    pub cause: u64,
    #[serde(default)]
    pub created: f64,
    #[serde(default)]
    pub updated: f64,
    /// A plain question, not a piece of work: no « En cours » line for it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub question: bool,
    /// The person chose its owner: the router sends the follow-ups to it, the relay never changes it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub assigned: bool,
    /// The agents the person took it from: never handed it again by the relay or the conductor.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handover: Option<Handover>,
}

fn now() -> f64 {
    crate::py::time_time()
}

/// A task's title: the message's first line without the @names it starts with, cut at a word.
pub fn title_of(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let words: Vec<&str> = line.split_whitespace().collect();
    let first = words.iter().position(|w| !w.starts_with('@')).unwrap_or(0);
    let line = words[first..].join(" ");
    let line = line.trim_start_matches([',', ':', ' ']);
    let mut chars = line.chars();
    let line = match chars.next() {
        Some(c) if first > 0 => c.to_uppercase().chain(chars).collect::<String>(),
        _ => line.to_string(),
    };
    let line = tk::squeeze(&line);
    if line.chars().count() <= TITLE_CHARS {
        return line;
    }
    let head = tk::head(&line, TITLE_CHARS - 1);
    let head = match head.rfind(' ') {
        Some(at) if at > TITLE_CHARS / 2 => &head[..at],
        _ => head,
    };
    format!("{}…", head.trim_end_matches([',', ';', ':', ' ']))
}

pub fn get(room: &Room, task: u32) -> Option<&Task> {
    room.tasks.iter().find(|t| t.id == task)
}

pub fn get_mut(room: &mut Room, task: u32) -> Option<&mut Task> {
    room.tasks.iter_mut().find(|t| t.id == task)
}

/// The latest task not finished.
pub fn open(room: &Room) -> Option<&Task> {
    room.tasks.iter().rev().find(|t| t.state != DONE)
}

/// The owner the person chose for the open task, still in the room: the person's next message goes to it, unless it
/// names someone or starts a new piece of work.
pub fn assigned_owner(room: &Room, text: &str) -> Option<String> {
    let task = open(room).filter(|t| t.assigned)?;
    if relay::looks_like_task(text) || !room.participants.contains(&task.owner) {
        return None;
    }
    Some(task.owner.clone())
}

/// The task the person's message `cause` belongs to: the open one it continues (its owner chosen by the person, or
/// team work), else a new one, owned by the first agent picked. The person moving on finishes the tasks left waiting.
pub fn begin(room: &mut Room, text: &str, owner: &str, cause: u64) -> u32 {
    let work = relay::looks_like_task(text);
    let team = room.team();
    let at = now();
    if !work {
        if let Some(task) = room.tasks.iter_mut().rev().find(|t| t.state != DONE) {
            if task.assigned || team {
                task.state = RUNNING.into();
                task.updated = at;
                return task.id;
            }
        }
    }
    for task in room.tasks.iter_mut().filter(|t| t.state == WAITING && !t.assigned) {
        task.state = DONE.into();
        task.updated = at;
    }
    let id = room.tasks.iter().map(|t| t.id).max().unwrap_or(0) + 1;
    let title = if team && !work { tk::head(&room.goal, TITLE_CHARS).to_string() } else { title_of(text) };
    room.tasks.push(Task {
        id,
        title,
        owner: owner.into(),
        state: RUNNING.into(),
        cause,
        created: at,
        updated: at,
        question: !work && !team,
        ..Default::default()
    });
    if room.tasks.len() > TASKS_KEPT {
        let extra = room.tasks.len() - TASKS_KEPT;
        room.tasks.drain(..extra);
    }
    id
}

/// A turn on `task` starts: its agent is the owner now, unless the person chose one.
pub fn started(room: &mut Room, task: u32, agent: &str) {
    if let Some(t) = get_mut(room, task) {
        if !t.assigned {
            t.owner = agent.into();
        }
        t.state = RUNNING.into();
        t.updated = now();
    }
}

/// The person took `task` from `agent`: the relay may not hand it back to it.
pub fn dropped(room: &Room, task: Option<u32>, agent: &str) -> bool {
    task.and_then(|t| get(room, t)).is_some_and(|t| t.assigned && t.dropped.iter().any(|d| d == agent))
}

/// Where the task stands after a turn: handed to an agent (its owner unless the person chose one), to the person
/// (waiting), finished, or stopped.
pub fn after(room: &mut Room, task: u32, next: Option<&Next>, others_queued: bool) {
    let Some(t) = get_mut(room, task) else { return };
    t.updated = now();
    match next {
        Some(Next::Agent(x)) => {
            if !t.assigned {
                t.owner = x.clone();
            }
            t.state = RUNNING.into();
        }
        Some(Next::Done) if !others_queued => t.state = DONE.into(),
        Some(_) | None if !others_queued => t.state = WAITING.into(),
        _ => {}
    }
}

/// The person's « Stop »: the tasks going on wait.
pub fn stopped(room: &mut Room) {
    for t in room.tasks.iter_mut().filter(|t| t.state == RUNNING) {
        t.state = WAITING.into();
    }
}

/// A queued turn of `agent` for `cause` carries `task` (set on the turn just queued, or on the one already waiting).
pub fn tag_turn(room: &mut Room, agent: &str, task: Option<u32>) {
    if task.is_none() {
        return;
    }
    if let Some(turn) = room.queue.iter_mut().rev().find(|t| t.agent == agent) {
        if turn.task.is_none() {
            turn.task = task;
        }
    }
}

/// The new owner's turn on `task`: its turn already waiting carries it, else one queued (the person's choice: past the
/// message's cap of turns; the turn keeps its own cap of tokens).
pub fn queue_owner(room: &mut Room, agent: &str, cause: u64, task: u32) {
    if let Some(turn) = room.queue.iter_mut().find(|t| t.agent == agent) {
        turn.task = Some(task);
        return;
    }
    room.queue.push(crate::rooms::Turn { agent: agent.into(), cause, queued: now(), task: Some(task), ..Default::default() });
}

/// What the reassignment changed: the old owner, whether it was working on it now (its run then stopped and its
/// note asked), the notice's id.
#[derive(Debug, Clone, PartialEq)]
pub struct Reassigned {
    pub from: String,
    pub running: bool,
    pub notice: u64,
    pub cause: u64,
}

/// « Research reprend « Plan du voyage ». » in the new owner's language.
pub fn reassigned_text(lang: &str, to_name: &str, title: &str) -> String {
    match lang {
        "fr" => format!("{to_name} reprend « {title} »."),
        "de" => format!("{to_name} übernimmt „{title}“."),
        "es" => format!("{to_name} se encarga de «{title}»."),
        "nl" => format!("{to_name} neemt „{title}” over."),
        _ => format!("{to_name} takes over “{title}”."),
    }
}

/// The task handed to `to` (a participant, not its owner) in `room`: the owner and the person's choice set, the old
/// owner's queued turns on it gone, its turn going on on it told to stop (`Turn.reassigned`), the notice kept. The new
/// owner's turn is queued by the caller once the note is ready (at once when the old owner writes it).
pub fn reassign(room: &mut Room, task: u32, to: &str, notice_text: &str) -> Option<Reassigned> {
    let at = now();
    let cause = room.messages.iter().rev().find(|m| m.kind == "person").map(|m| m.id);
    let t = get_mut(room, task)?;
    let from = std::mem::replace(&mut t.owner, to.to_string());
    t.assigned = true;
    t.dropped.retain(|d| d != to);
    if from != to && !t.dropped.contains(&from) {
        t.dropped.push(from.clone());
    }
    t.state = RUNNING.into();
    t.updated = at;
    let cause = cause.unwrap_or(t.cause);
    let mut running = false;
    if let Some(current) = room.current.as_mut() {
        if current.agent == from && current.task == Some(task) && current.reassigned.is_none() {
            current.reassigned = Some(to.to_string());
            running = true;
        }
    }
    room.queue.retain(|turn| !(turn.agent == from && turn.task == Some(task)));
    let notice = room.push_message(Message {
        at,
        from: to.to_string(),
        kind: "notice".into(),
        text: notice_text.to_string(),
        reply_to: None, // not a turn: never counted among the message's turns
        task: Some(task),
        handover: Some(json!({"task": task, "from": from, "to": to})),
        ..Default::default()
    });
    if let Some(t) = get_mut(room, task) {
        t.handover = Some(Handover { from: from.clone(), to: to.into(), at, message: notice, note: String::new(), in_room: false });
    }
    Some(Reassigned { from, running, notice, cause })
}

/// The small call's prompt: the task, who had it, its messages since it started (the latest kept), names as shown.
pub fn note_prompt(room: &Room, task: &Task, names: &[(String, String)]) -> String {
    let name_of = |id: &str| names.iter().find(|(i, _)| i == id).map(|(_, n)| n.clone()).unwrap_or_else(|| id.into());
    let from = task.handover.as_ref().map(|h| name_of(&h.from)).unwrap_or_default();
    let mut blocks: Vec<String> = Vec::new();
    let mut size = 0;
    for m in room.messages.iter().rev().filter(|m| m.id >= task.cause && m.kind != "notice") {
        let who = if m.kind == "person" { "Person".to_string() } else { name_of(&m.from) };
        let (text, _) = crate::rooms::room_text(&m.text);
        let block = format!("[{who}] {}", tk::head(&tk::squeeze(&text), NOTE_MESSAGE_CHARS));
        if size + block.len() > NOTE_INPUT_CHARS && !blocks.is_empty() {
            break;
        }
        size += block.len();
        blocks.insert(0, block);
    }
    format!("Task: {}\nIt was {from}'s.\nMessages:\n{}", task.title, blocks.join("\n"))
}

/// The small model's note, two short lines at most (None: nothing usable).
pub fn parse_note(answer: &str) -> Option<String> {
    let lines: Vec<String> =
        answer.lines().map(|l| tk::squeeze(l.trim_matches(|c: char| "*`\"".contains(c)))).filter(|l| !l.is_empty()).take(2).collect();
    let note = lines.join("\n");
    (!note.is_empty()).then(|| tk::head(&note, NOTE_CHARS).to_string())
}

/// Without the small model: the old owner's last words on the task.
pub fn fallback_note(room: &Room, task: &Task, from: &str) -> String {
    room.messages
        .iter()
        .rev()
        .filter(|m| m.id >= task.cause && m.kind == "agent" && m.from == from)
        .map(|m| crate::rooms::room_text(&m.text).0)
        .find(|t| !t.is_empty())
        .map(|t| tk::head(&tk::squeeze(&t), NOTE_CHARS / 2).to_string())
        .unwrap_or_default()
}

/// What the old owner, stopped, is asked (out of the person's sight): its note for the new owner, no tool.
pub fn handover_input(to_name: &str) -> String {
    format!(
        "[BipAgents] Your person handed this task to {to_name}; your turn was stopped. Do not call any tool. Write \
the handover note for {to_name} in two short lines, « Done: … » and « Left: … », in your person's language. Nothing \
else, no NEXT line."
    )
}

/// What the new owner's first turn on a task handed to it starts with (empty: nothing handed to it since its last
/// turn).
pub fn handover_context(room: &Room, agent: &str, names: &[(String, String)]) -> String {
    let seen = room.seen.get(agent).copied().unwrap_or(0);
    let Some((task, h)) = room
        .tasks
        .iter()
        .rev()
        .filter_map(|t| t.handover.as_ref().map(|h| (t, h)))
        .find(|(t, h)| h.to == agent && t.owner == agent && h.message > seen)
    else {
        return String::new();
    };
    let from = names.iter().find(|(i, _)| *i == h.from).map(|(_, n)| n.as_str()).unwrap_or(&h.from);
    let mut out = format!("Your person handed you the task « {} » ({from} had it). Go on from where it stands; don't redo what is done.", task.title);
    if h.in_room {
        out.push_str(&format!(" {from}'s handover note is its last message below."));
    } else if !h.note.is_empty() {
        out.push_str(&format!("\nHandover note from {from}:\n{}", h.note));
    }
    out.push('\n');
    out
}

/// The run's instructions' line once a task was handed over in the room (only then: no tokens for rooms that never
/// do it).
pub const REASSIGN_RULE: &str = "Your person may hand a task to another agent: when one is handed to you, go on from \
its handover note; once one is taken from you, leave it.";

pub fn any_handover(room: &Room) -> bool {
    room.tasks.iter().any(|t| t.handover.is_some())
}

/// The tasks as the app shows them.
pub fn view(room: &Room) -> Value {
    json!(room.tasks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::Turn;

    fn room() -> Room {
        Room {
            id: "g000000000001".into(),
            title: "T".into(),
            participants: vec!["bip".into(), "back".into(), "research".into()],
            next_id: 1,
            ..Default::default()
        }
    }

    fn person(room: &mut Room, text: &str) -> u64 {
        room.push_message(Message {
            at: 1.0,
            from: "person".into(),
            kind: "person".into(),
            text: text.into(),
            ..Default::default()
        })
    }

    #[test]
    fn titles_are_first_words() {
        assert_eq!(title_of("  Prépare le plan du voyage\net le reste"), "Prépare le plan du voyage");
        assert_eq!(title_of("@coach @cuisine faites moi un plan"), "Faites moi un plan");
        assert_eq!(title_of("@tous"), "@tous");
        let long = title_of(&"mot ".repeat(40));
        assert!(long.ends_with('…') && long.chars().count() <= TITLE_CHARS);
    }

    #[test]
    fn a_message_starts_a_task_and_a_follow_up_continues_an_assigned_one() {
        let mut r = room();
        let m1 = person(&mut r, "Quelle heure est-il à Tokyo ?");
        let t1 = begin(&mut r, "Quelle heure est-il à Tokyo ?", "bip", m1);
        assert!(get(&r, t1).unwrap().question);
        after(&mut r, t1, Some(&Next::Person), false);
        assert_eq!(get(&r, t1).unwrap().state, WAITING);
        let m2 = person(&mut r, "Prépare moi le plan du voyage à Tokyo en mai");
        let t2 = begin(&mut r, "Prépare moi le plan du voyage à Tokyo en mai", "back", m2);
        assert_ne!(t1, t2);
        assert_eq!(get(&r, t1).unwrap().state, DONE); // the person moved on
        assert!(!get(&r, t2).unwrap().question);
        // not assigned: a follow-up is a new task
        let m3 = person(&mut r, "Et pour juin ?");
        assert_ne!(begin(&mut r, "Et pour juin ?", "bip", m3), t2);
        // assigned: the follow-up continues it, goes to its owner
        let notice = reassigned_text("fr", "Research", "Plan");
        let done = reassign(&mut r, t2, "research", &notice).unwrap();
        assert_eq!((done.from.as_str(), done.running), ("back", false));
        assert_eq!(assigned_owner(&r, "et les hôtels ?"), None); // the open task is the latest one (m3's)
        r.tasks.retain(|t| t.id != 3);
        assert_eq!(assigned_owner(&r, "et les hôtels ?").as_deref(), Some("research"));
        assert_eq!(assigned_owner(&r, "Faites moi aussi un budget complet"), None); // new work: the router
        let m4 = person(&mut r, "et les hôtels ?");
        assert_eq!(begin(&mut r, "et les hôtels ?", "research", m4), t2);
    }

    #[test]
    fn reassigning_stops_the_old_owner_and_keeps_the_new_one() {
        let mut r = room();
        let m1 = person(&mut r, "Faites moi une petite app web de liste");
        let t = begin(&mut r, "Faites moi une petite app web de liste", "back", m1);
        r.current = Some(Turn { agent: "back".into(), cause: m1, task: Some(t), ..Default::default() });
        r.queue.push(Turn { agent: "back".into(), cause: m1, task: Some(t), ..Default::default() });
        r.queue.push(Turn { agent: "bip".into(), cause: m1, ..Default::default() });
        let done = reassign(&mut r, t, "research", "Research reprend « app ».").unwrap();
        assert!(done.running);
        assert_eq!(r.current.as_ref().unwrap().reassigned.as_deref(), Some("research"));
        assert_eq!(r.queue.iter().map(|q| q.agent.as_str()).collect::<Vec<_>>(), vec!["bip"]);
        let task = get(&r, t).unwrap();
        assert!(task.assigned && task.owner == "research" && task.dropped == vec!["back".to_string()]);
        let notice = r.messages.last().unwrap();
        assert_eq!((notice.kind.as_str(), notice.from.as_str(), notice.reply_to), ("notice", "research", None));
        assert_eq!(notice.handover.as_ref().unwrap()["from"], "back");
        // the relay never hands it back to Back, nor changes its owner
        assert!(dropped(&r, Some(t), "back") && !dropped(&r, Some(t), "bip") && !dropped(&r, None, "back"));
        started(&mut r, t, "bip");
        after(&mut r, t, Some(&Next::Agent("bip".into())), false);
        assert_eq!(get(&r, t).unwrap().owner, "research");
        // back to Back by the person: Back is no longer dropped
        reassign(&mut r, t, "back", "x").unwrap();
        assert_eq!(get(&r, t).unwrap().dropped, vec!["research".to_string()]);
    }

    #[test]
    fn the_new_owner_is_told_once() {
        let mut r = room();
        let m1 = person(&mut r, "Faites moi une petite app web de liste");
        let t = begin(&mut r, "Faites moi une petite app web de liste", "back", m1);
        reassign(&mut r, t, "research", "n").unwrap();
        let names = vec![("back".to_string(), "Back".to_string()), ("research".to_string(), "Research".to_string())];
        assert!(handover_context(&r, "research", &names).starts_with("Your person handed you the task « Faites moi"));
        assert!(handover_context(&r, "back", &names).is_empty());
        get_mut(&mut r, t).unwrap().handover.as_mut().unwrap().note = "Done: maquette\nLeft: le code".into();
        assert!(handover_context(&r, "research", &names).contains("Handover note from Back:\nDone: maquette"));
        r.seen.insert("research".into(), r.next_id - 1);
        assert!(handover_context(&r, "research", &names).is_empty());
        assert!(note_prompt(&r, get(&r, t).unwrap(), &names).contains("It was Back's.\nMessages:\n[Person] Faites moi"));
    }

    #[test]
    fn notes_and_texts() {
        assert_eq!(parse_note("**Done: la maquette**\n\nLeft: le code\nautre").as_deref(), Some("Done: la maquette\nLeft: le code"));
        assert_eq!(parse_note("  \n"), None);
        assert_eq!(reassigned_text("fr", "Research", "Plan"), "Research reprend « Plan ».");
        assert_eq!(reassigned_text("en", "Research", "Plan"), "Research takes over “Plan”.");
        assert!(handover_input("Research").contains("no NEXT line"));
    }
}
