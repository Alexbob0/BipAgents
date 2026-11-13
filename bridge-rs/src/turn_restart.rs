//! A message sent while the agent answers, before it did anything (Alex 12/11, reported twice: « il finit de
//! répondre à la première question et seulement après il traite la deuxième »).
//!
//! Hermes' steer (`POST /v1/runs/{id}/steer`, threads/send.rs) is delivered after the turn's current tool calls, as
//! its own user row before the next model call. A turn that only thinks and writes (one model call, no tool) has no
//! next call: Hermes hands the words back at its end (`pending_steer`, agent/turn_finalizer.py) and they became the
//! next turn (msg_queue.rs), after a whole answer that ignored them.
//!
//! So, in a conversation's turn the person started:
//! - nothing done yet (it only thought, wrote, or called tools that change nothing: `read_only`): the turn is stopped
//!   and started again at once with the new words (`Plan`), told out of sight (`NOTE`) that its answer was cut by
//!   them and to answer both in one. The run hub chains the new run into the stopped run's stream (runs.rs
//!   `restart`): the apps following it see one turn going on, its stop goes to the new run, and its end is the new
//!   run's end. The stopped answer's words leave the conversation (threads/timeline.rs drops the agent's messages and
//!   Hermes' « not processed » line between the first message and the one marked `NOTE`);
//! - something done (a message sent, a file written, a payment, a card asked…): the words go into the turn at its
//!   next step (Hermes' steer, as before), never cutting an action in the middle;
//! - « Après »: the queue, as before.

use serde_json::Value;

/// The note before the words of the restarted turn (the timeline reads it as « this turn replaces the one cut »).
pub const NOTE_OPEN: &str = "<!--bip:restarted";
pub const NOTE: &str = "<!--bip:restarted\nNot a new subject: your person wrote this while you were answering their \
message just before, and your answer was stopped before you did anything. Answer that message again now with this \
taken into account: ONE answer for both, as if they had sent them together. Never mention the stop.\n-->";

/// What the stopped turn starts again with (threads/send.rs): the new words and what goes with them.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// The words as the agent reads them (a family's framing aside: never planned in its conversation).
    pub said: String,
    /// What waited for the message (the time, a call's recap…) and the app's notes.
    pub notes: String,
    /// The app's id for the message (the queue's item, should Hermes refuse the new turn).
    pub client_id: Option<String>,
    pub phone: Option<String>,
    pub instructions: Option<String>,
    pub options: Option<Value>,
}

impl Plan {
    /// The new turn's input: `cut`, the turn before was stopped for it (else it ended by itself meanwhile, and the
    /// words are only the next message).
    pub fn input(&self, cut: bool) -> Value {
        let notes = [self.notes.trim(), if cut { NOTE } else { "" }].into_iter().filter(|n| !n.is_empty());
        crate::threads::send::input_of(Some(&notes.collect::<Vec<_>>().join("\n")), &self.said, &[])
    }
}

/// Whether a user row is the words a turn was restarted with.
pub fn restarted(raw: &str) -> bool {
    raw.contains(NOTE_OPEN)
}

/// Kinds of calls that change nothing (threads/layout.rs `kind_of`): reading, searching, looking up. Any other call
/// (an unknown one too) counts as the turn having acted.
const READ_ONLY_KINDS: &[&str] = &[
    "notebookSearch", "notebookRead", "weather", "route", "contactsRead", "calendarRead", "remindersRead", "health",
    "mailSearch", "mailRead", "mailOverview", "voices", "webSearch", "webRead", "readFile", "sessionSearch",
    "findTool", "todo", "archiveSearch", "archiveRead", "upcomingDates", "findDate", "familyMembers",
    "familyNotebookRead", "roomHistory", "findTemplate", "xSearch", "vision", "video", "sitesList", "spaceEstimate",
    "contactDetailsRead", "map",
];

/// A tool call that changes nothing: the turn may still be cut and started again.
pub fn read_only(tool: &str) -> bool {
    let kind = crate::threads::layout::kind_of(tool);
    if kind == "skills" {
        // skills_list, skill_view; never skill_manage (it writes)
        return !tool.ends_with("skill_manage");
    }
    READ_ONLY_KINDS.contains(&kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_and_searching_change_nothing_the_rest_acts() {
        for tool in ["web_search", "web_extract", "read_file", "mcp__bipagents__weather", "mcp_bipagents_notebook_search",
                     "calendar_list", "mail_search", "todo", "tool_search", "skill_view", "session_search"] {
            assert!(read_only(tool), "{tool}");
        }
        for tool in ["mail_send", "write_file", "terminal", "send_message", "pay_with_card", "calendar_add", "memory",
                     "browser_navigate", "delegate_task", "skill_manage", "notebook_write", "create_page", "clarify",
                     "some_unknown_tool", "message_agent", "contacts_add"] {
            assert!(!read_only(tool), "{tool}");
        }
    }

    #[test]
    fn the_restarted_turn_is_told_it_replaces_the_cut_one() {
        let plan = Plan { said: "Seulement les publiques".into(), notes: "<!--bip:now 9 h-->".into(), ..Plan::default() };
        let cut = plan.input(true);
        let cut = cut.as_str().unwrap();
        assert!(cut.starts_with("<!--bip:now 9 h-->\n<!--bip:restarted\n") && cut.ends_with("-->\nSeulement les publiques"));
        assert!(restarted(cut));
        // the turn ended by itself before the stop: the words are only the next message
        assert_eq!(plan.input(false), Value::String("<!--bip:now 9 h-->\nSeulement les publiques".into()));
        assert_eq!(Plan { said: "Et en bio".into(), ..Plan::default() }.input(false), Value::String("Et en bio".into()));
    }
}
