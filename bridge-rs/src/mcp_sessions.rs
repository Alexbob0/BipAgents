//! The agents' MCP sessions (`/v1/agent-tools/mcp`, agent_tools.rs): an agent sees a new tool as soon as the person
//! gives it one (a mailbox's calendar, the phone's calendar, the contacts, the weather…), without anybody restarting
//! it (Alex, 2026-10-28: « Personne ne va redémarrer ses agents soi-même »).
//!
//! `initialize` hands out an `Mcp-Session-Id`, kept in memory with the agent it belongs to and the tool set
//! (`version_of(tools_for(…))`) that session last listed. The client's GET opens the session's server stream; when the
//! agent's tool set differs from what the session listed, `notifications/tools/list_changed` goes down that stream
//! (and, as a fallback, before the result of a `tools/call` answered as SSE). Hermes' MCP client re-lists on it and
//! refreshes the live agent's tools (tools/mcp_tool_health.py). The tool list a model sees never changes unless the
//! tools really did (the prompt cache): nothing is sent while the fingerprint is the same, and once per new set.
//!
//! A Hermes that never re-lists (an old self-install, a client without the stream) sees its new tools at its next
//! restart (the nightly restart of idle agents).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use crate::config::{AgentConfig, Config};

/// How often an open stream checks its agent's tools when nothing pokes it.
pub const CHECK_SECONDS: f64 = 5.0;
/// A comment line on an idle stream, so proxies and the client keep it open.
pub const KEEPALIVE_SECONDS: f64 = 15.0;
/// Sessions kept at most (the least recently used dropped first): a client that never says DELETE.
const MAX_SESSIONS: usize = 2000;

pub const LIST_CHANGED: &str = r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#;

/// The tool set's fingerprint for `agent` now.
pub fn fingerprint(config: &Config, agent: &AgentConfig) -> String {
    crate::agent_tools::version_of(&crate::agent_tools::tools_for(config, agent))
}

#[derive(Debug)]
struct Session {
    agent: String,
    /// What this session last listed (None: not yet, nothing to tell it).
    listed: Option<String>,
    /// When it listed (Unix time).
    listed_at: f64,
    /// A set other than the one it listed, and since when it was seen (Unix time).
    differs: Option<(String, f64)>,
    /// The set it was last told about (once per set).
    notified: Option<String>,
    /// Each GET stream opened gets the next number; an older stream of the session ends.
    stream: u64,
    used: Instant,
}

impl Session {
    /// `fingerprint` (not the listed set) seen at `now`: the time kept from its first sight.
    fn note_difference(&mut self, fingerprint: &str, now: f64) {
        if self.differs.as_ref().is_none_or(|(set, _)| set != fingerprint) {
            self.differs = Some((fingerprint.to_string(), now));
        }
    }
}

#[derive(Default)]
pub struct Sessions {
    sessions: Mutex<HashMap<String, Session>>,
    changed: Notify,
}

pub static SESSIONS: LazyLock<Sessions> = LazyLock::new(Sessions::default);

/// Something that may change an agent's tools happened (a config change, a mailbox, a registration): the open
/// streams check now instead of at their next tick.
pub fn poke() {
    SESSIONS.changed.notify_waiters();
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Where a session id stands for an agent's request.
#[derive(Debug, PartialEq, Eq)]
pub enum Known {
    Yes,
    /// Not known (a bridge restarted since): taken on for this agent.
    Adopted,
    /// Another agent's.
    Other,
}

impl Sessions {
    /// A new session for `agent` (at `initialize`).
    pub fn open(&self, agent: &str) -> String {
        let id = crate::py::token_urlsafe(24);
        let mut sessions = lock(&self.sessions);
        if sessions.len() >= MAX_SESSIONS {
            if let Some(oldest) = sessions.iter().min_by_key(|(_, s)| s.used).map(|(k, _)| k.clone()) {
                sessions.remove(&oldest);
            }
        }
        sessions.insert(
            id.clone(),
            Session {
                agent: agent.to_string(),
                listed: None,
                listed_at: 0.0,
                differs: None,
                notified: None,
                stream: 0,
                used: Instant::now(),
            },
        );
        id
    }

    /// The session `id` for a request of `agent`. An unknown one (this bridge restarted while the client kept its
    /// session) is taken on, with `baseline` as what it listed: the `?v=` of its URL, the tools' version when its
    /// entry was written.
    pub fn check(&self, id: &str, agent: &str, baseline: Option<&str>) -> Known {
        let mut sessions = lock(&self.sessions);
        if let Some(session) = sessions.get_mut(id) {
            if session.agent != agent {
                return Known::Other;
            }
            session.used = Instant::now();
            return Known::Yes;
        }
        sessions.insert(
            id.to_string(),
            Session {
                agent: agent.to_string(),
                listed: baseline.filter(|v| !v.is_empty()).map(str::to_string),
                listed_at: crate::py::time_time(),
                differs: None,
                notified: None,
                stream: 0,
                used: Instant::now(),
            },
        );
        Known::Adopted
    }

    /// Session `id` listed `fingerprint`.
    pub fn listed(&self, id: &str, fingerprint: &str) {
        if let Some(session) = lock(&self.sessions).get_mut(id) {
            session.listed = Some(fingerprint.to_string());
            session.listed_at = crate::py::time_time();
            session.differs = None;
            session.notified = None;
        }
    }

    /// Whether session `id` should be told its tools changed (`fingerprint`: the set now). True once per new set,
    /// and never for a session that hasn't listed yet or listed this very set.
    pub fn due(&self, id: &str, fingerprint: &str) -> bool {
        let mut sessions = lock(&self.sessions);
        let Some(session) = sessions.get_mut(id) else { return false };
        let Some(listed) = session.listed.as_deref() else { return false };
        let same = listed == fingerprint;
        if !same {
            session.note_difference(fingerprint, crate::py::time_time());
        }
        if same || session.notified.as_deref() == Some(fingerprint) {
            return false;
        }
        session.notified = Some(fingerprint.to_string());
        true
    }

    /// A GET stream opens for session `id`: its number (an older stream of the session ends).
    pub fn new_stream(&self, id: &str) -> Option<u64> {
        let mut sessions = lock(&self.sessions);
        let session = sessions.get_mut(id)?;
        session.stream += 1;
        Some(session.stream)
    }

    /// Whether stream number `stream` of session `id` is still the one to serve.
    pub fn serves(&self, id: &str, stream: u64) -> bool {
        lock(&self.sessions).get(id).is_some_and(|s| s.stream == stream)
    }

    /// DELETE: the session ends (only the agent's own).
    pub fn end(&self, id: &str, agent: &str) -> bool {
        let mut sessions = lock(&self.sessions);
        if sessions.get(id).is_some_and(|s| s.agent == agent) {
            sessions.remove(id);
            true
        } else {
            false
        }
    }

    /// The agents whose tools now (`current(agent)`, None: no such agent any more) differ from what their latest
    /// listing session listed, each with since when (Unix time: the first time this bridge saw the difference, at
    /// the latest `now`). An agent with no session that listed isn't stale.
    pub fn stale(&self, current: impl Fn(&str) -> Option<String>, now: f64) -> Vec<(String, f64)> {
        let mut sessions = lock(&self.sessions);
        let mut latest: HashMap<String, &mut Session> = HashMap::new();
        for session in sessions.values_mut().filter(|s| s.listed.is_some()) {
            match latest.get(&session.agent) {
                Some(kept) if kept.listed_at >= session.listed_at => {}
                _ => {
                    latest.insert(session.agent.clone(), session);
                }
            }
        }
        let mut out = Vec::new();
        for (agent, session) in latest {
            let Some(print) = current(&agent) else { continue };
            if session.listed.as_deref() == Some(print.as_str()) {
                session.differs = None;
                continue;
            }
            session.note_difference(&print, now);
            out.push((agent, session.differs.as_ref().map(|(_, since)| *since).unwrap_or(now)));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Waits for a poke or `seconds`.
    pub async fn wait(&self, seconds: f64) {
        let notified = self.changed.notified();
        tokio::select! {
            _ = notified => {}
            _ = tokio::time::sleep(Duration::from_secs_f64(seconds)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_is_told_once_per_new_tool_set() {
        let s = Sessions::default();
        let id = s.open("vie");
        assert!(!s.due(&id, "aaa"), "not listed yet: it lists anyway");
        s.listed(&id, "aaa");
        assert!(!s.due(&id, "aaa"), "the same set: nothing (the prompt cache)");
        assert!(s.due(&id, "bbb"));
        assert!(!s.due(&id, "bbb"), "once");
        assert!(s.due(&id, "ccc"), "another change before it re-listed");
        s.listed(&id, "ccc");
        assert!(!s.due(&id, "ccc"));
        assert!(!s.due("nope", "ccc"));
    }

    #[test]
    fn stale_agents_are_those_whose_latest_session_listed_another_set() {
        let s = Sessions::default();
        let tools: HashMap<&str, &str> = [("vie", "bbb"), ("coach", "kkk"), ("guide", "ggg")].into_iter().collect();
        let current = |a: &str| tools.get(a).map(|v| v.to_string());
        assert!(s.stale(current, 100.0).is_empty(), "no session: not stale");
        let old = s.open("vie");
        s.listed(&old, "aaa");
        let id = s.open("vie");
        assert!(s.stale(current, 100.0).len() == 1, "the session that listed counts, not the one that didn't yet");
        s.listed(&id, "aaa");
        let coach = s.open("coach");
        s.listed(&coach, "kkk");
        s.open("guide"); // never listed
        assert_eq!(s.stale(current, 200.0), vec![("vie".to_string(), 200.0)], "its latest listing session's");
        assert_eq!(s.stale(current, 300.0), vec![("vie".to_string(), 200.0)], "since its first sight");
        s.listed(&id, "bbb");
        assert!(s.stale(current, 400.0).is_empty(), "its latest session listed the new set");
        let gone = |a: &str| if a == "vie" { None } else { current(a) };
        s.listed(&id, "zzz");
        assert!(s.stale(gone, 500.0).is_empty(), "an agent removed: not stale");
    }

    #[test]
    fn sessions_belong_to_their_agent() {
        let s = Sessions::default();
        let id = s.open("vie");
        assert_eq!(s.check(&id, "vie", None), Known::Yes);
        assert_eq!(s.check(&id, "coach", None), Known::Other);
        assert!(!s.end(&id, "coach"));
        assert!(s.end(&id, "vie"));
        // a bridge restarted: the client's session taken on, its URL's version as what it listed
        assert_eq!(s.check(&id, "vie", Some("aaa")), Known::Adopted);
        assert!(!s.due(&id, "aaa") && s.due(&id, "bbb"));
        assert_eq!(s.check("other", "vie", Some("")), Known::Adopted);
        assert!(!s.due("other", "bbb"), "no version: nothing known, nothing told");
    }

    #[test]
    fn a_newer_stream_replaces_the_older() {
        let s = Sessions::default();
        let id = s.open("vie");
        let first = s.new_stream(&id).unwrap();
        assert!(s.serves(&id, first));
        let second = s.new_stream(&id).unwrap();
        assert!(!s.serves(&id, first) && s.serves(&id, second));
        s.end(&id, "vie");
        assert!(!s.serves(&id, second) && s.new_stream(&id).is_none());
    }
}
