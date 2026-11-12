//! When a person last used the place (12/11, freed beta places: a beta place nobody uses for 7 days goes back to the
//! reserve, BipAgents-reprise/release-1112/place-idle-contract.md §1). The bridge writes the time of the last person
//! action, epoch seconds as text, at most once a minute; the seat agent (installer/cloud/seat-agent.py) sends it to
//! the guichet in its beat (`"used"`):
//!
//!     ~/.local/state/bipagents/used          1762950000
//!
//! A person action, answered with success: a message to any agent in any conversation (text, voice note, photo, file,
//! share-sheet item: the one sending route of threads/send.rs, a relative's message to a shared or family agent),
//! a Live / Quick Live call started, an approval card answered (yes, no, always; a card payment too). Background
//! reads, sync polls, push registrations, routines, watchers and tasks never touch it. Writing it never fails a
//! request. Self-installs write it too (nobody reads it there).
//!
//! « Garder ma place » (contract v2), the day-5 banner's button: `POST /v1/place/keep` (the bridge key) → 204, a
//! person action written at once, whatever the once-a-minute rule.

use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::LazyLock;

use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;

use crate::app::AppState;

/// `~/.local/state/bipagents/used`, next to sleep.rs' `asleep` marker.
pub const FILE: &str = ".local/state/bipagents/used";
/// At most one write a minute.
pub const EVERY_SECONDS: i64 = 60;
/// « Garder ma place »: the place kept, written whatever the once-a-minute rule.
pub const KEEP_PATH: &str = "/v1/place/keep";

/// Whether `method path` (the full path, as the outer router sees it) is a person's action.
pub fn person_action(method: &Method, path: &str) -> bool {
    if method != Method::POST {
        return false;
    }
    let parts: Vec<&str> = path.trim_end_matches('/').split('/').skip(1).collect();
    matches!(
        parts.as_slice(),
        // a message, any agent, any conversation (rooms, shared agents, the family agent's: threads/mod.rs)
        ["v1", "threads", _, "messages"]
            // a Live or Quick Live call starts (live_brief.rs)
            | ["v1", "live", "calls", "start"]
            // an approval card answered (app.rs `approve`), a card payment approved or refused (card_pay.rs)
            | ["v1", "approve"]
            // « Garder ma place », the day-5 banner's button
            | ["v1", "place", "keep"]
            | ["v1", "vault", "payments", _, "approve" | "deny"]
            // a relative's message or answer to an agent of this place (agent_shares.rs: shared and family agents)
            | ["family", "s", _, "conversations", _, "messages"]
            | ["family", "s", _, "approval"]
    )
}

/// The file and the last time it was written.
pub struct Used {
    pub path: PathBuf,
    last: AtomicI64,
}

impl Used {
    pub fn new(home: PathBuf) -> Self {
        Used { path: home.join(FILE), last: AtomicI64::new(0) }
    }

    /// Notes a person action at `now` (epoch seconds): written unless it was less than a minute ago. Whether it wrote.
    pub fn note_at(&self, now: i64) -> bool {
        self.write(now, false)
    }

    /// The same, written whatever the once-a-minute rule (« Garder ma place »).
    pub fn keep_at(&self, now: i64) -> bool {
        self.write(now, true)
    }

    fn write(&self, now: i64, always: bool) -> bool {
        let last = self.last.load(Ordering::Relaxed);
        if (!always && now - last < EVERY_SECONDS)
            || self.last.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_err()
        {
            return false;
        }
        let written = (|| -> std::io::Result<()> {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // atomically: the seat agent never reads half a number
            let tmp = self.path.with_extension("tmp");
            std::fs::write(&tmp, now.to_string())?;
            std::fs::rename(&tmp, &self.path)
        })();
        if let Err(exc) = written {
            tracing::warn!(error = %exc, "last use not written");
            // tried again at the next action
            self.last.store(last, Ordering::Relaxed);
            return false;
        }
        true
    }
}

static USED: LazyLock<Used> = LazyLock::new(|| {
    let home = std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(crate::py::expanduser("~")));
    Used::new(home)
});

/// The middleware (app.rs, around the routed app): a person's action that succeeded is noted.
pub async fn note(request: Request, next: Next) -> Response {
    let counted = person_action(request.method(), request.uri().path());
    let kept = request.uri().path().trim_end_matches('/') == KEEP_PATH;
    let response = next.run(request).await;
    if counted && response.status().is_success() {
        let now = chrono::Utc::now().timestamp();
        if kept {
            USED.keep_at(now);
        } else {
            USED.note_at(now);
        }
    }
    response
}

/// « Garder ma place »: nothing but the use the middleware above writes.
async fn keep() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new().route("/place/keep", post(keep))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(path: &str) -> bool {
        person_action(&Method::POST, path)
    }

    #[test]
    fn a_message_counts_whatever_the_conversation() {
        // text, voice note, photo, file, share-sheet item: one sending route
        assert!(post("/v1/threads/main/messages"));
        assert!(post("/v1/threads/new/messages"));
        assert!(post("/v1/threads/t_01JBX4K9W3M2Q8R6T5Y7Z0A1B2/messages"));
        assert!(post("/v1/threads/room:gdemo0000001/messages"));
        // a relative's message to a shared agent, or in the family agent's common conversation
        assert!(post("/family/s/cuisine/conversations/family/messages"));
    }

    #[test]
    fn a_live_call_start_counts() {
        assert!(post("/v1/live/calls/start"));
        // the call's end, its brief: not a start
        assert!(!post("/v1/live/calls"));
        assert!(!person_action(&Method::GET, "/v1/live/brief"));
    }

    #[test]
    fn an_approval_answered_counts() {
        assert!(post("/v1/approve"));
        assert!(post("/v1/vault/payments/p1/approve"));
        assert!(post("/v1/vault/payments/p1/deny"));
        assert!(post("/family/s/cuisine/approval"));
    }

    #[test]
    fn keep_my_place_counts_and_is_written_at_once() {
        assert!(post(KEEP_PATH));
        assert!(!person_action(&Method::GET, KEEP_PATH));
        let dir = tempfile::tempdir().unwrap();
        let used = Used::new(dir.path().to_path_buf());
        assert!(used.note_at(1_762_950_000));
        // a message a few seconds before: the button still writes its own time
        assert!(used.keep_at(1_762_950_010));
        assert_eq!(std::fs::read_to_string(&used.path).unwrap(), "1762950010");
        // and the once-a-minute rule goes on from there
        assert!(!used.note_at(1_762_950_030));
    }

    #[test]
    fn background_reads_polls_and_pushes_do_not_count() {
        assert!(!person_action(&Method::GET, "/v1/threads/main/timeline"));
        assert!(!person_action(&Method::GET, "/v1/threads/events"));
        assert!(!person_action(&Method::GET, "/v1/threads/main/sends/C-1"));
        assert!(!person_action(&Method::GET, "/v1/approvals"));
        assert!(!person_action(&Method::GET, "/v1/threads/main/messages"));
        assert!(!post("/v1/devices"));
        assert!(!post("/v1/live-activities"));
        assert!(!post("/v1/files"));
        assert!(!post("/v1/threads/main/stop"));
        assert!(!post("/v1/watch"));
        assert!(!post("/family/fa/said"));
    }

    #[test]
    fn written_atomically_at_most_once_a_minute() {
        let dir = tempfile::tempdir().unwrap();
        let used = Used::new(dir.path().to_path_buf());
        assert!(used.note_at(1_762_950_000));
        assert_eq!(std::fs::read_to_string(&used.path).unwrap(), "1762950000");
        assert!(!used.note_at(1_762_950_059));
        assert_eq!(std::fs::read_to_string(&used.path).unwrap(), "1762950000");
        assert!(used.note_at(1_762_950_060));
        assert_eq!(std::fs::read_to_string(&used.path).unwrap(), "1762950060");
        assert!(!used.path.with_extension("tmp").exists());
    }

    #[test]
    fn a_failed_write_never_fails_and_is_tried_again() {
        let dir = tempfile::tempdir().unwrap();
        // the state folder is a file: nothing can be written there
        std::fs::create_dir_all(dir.path().join(".local/state")).unwrap();
        std::fs::write(dir.path().join(".local/state/bipagents"), "").unwrap();
        let used = Used::new(dir.path().to_path_buf());
        assert!(!used.note_at(1_762_950_000));
        std::fs::remove_file(dir.path().join(".local/state/bipagents")).unwrap();
        assert!(used.note_at(1_762_950_001));
    }
}
