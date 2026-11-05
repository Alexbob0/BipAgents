//! New e-mail for the routines that start on one (triggers.rs, kind `mail`): one IMAP connection per mailbox in use,
//! waiting with IDLE (RFC 2177) on its inbox — the server tells of new mail as it arrives, nothing is polled — renewed
//! every `idle_minutes`; a server without IDLE is looked at every `poll_minutes`. Each new message (by UID, above the
//! last one seen: a first connection starts from the newest, old mail is never replayed) is read once (its first
//! 256 KB, `BODY.PEEK`: it stays unread for the person), then offered to the mailbox's triggers, whose filters choose.
//!
//! A connection that fails (a server down, a password changed, the network) is tried again after 5 s, then twice as
//! long each time up to 15 min; a mailbox whose settings changed (a new password) gets a new connection; one no
//! trigger needs any more is closed. Each runs in a thread of its own (the IMAP client blocks), stopped within a
//! second or so of being told to.
//!
//! The standing mail rules' mailboxes (mail_rules.rs) are watched the same way: each new message's headers go to their
//! `sort_new` (only a mailbox a routine wants has its messages read whole).
//!
//!     <data>/triggers-mail.json (0600)   {account: {validity, last_uid}}   where each inbox was left
//!
//! No Python counterpart.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc::UnboundedSender;

use crate::app::AppState;
use crate::config::Config;
use crate::mailbox::{self, Connection};
use crate::tools_knowledge::imap::Data;
use crate::tools_knowledge::{self as tk};
use crate::triggers::{self, Event, MAIL};
use crate::{py, Dict};

pub const FOLDER: &str = "INBOX";
/// A message's first bytes read: its text, not its attachments.
pub const FETCH_BYTES: usize = 256 * 1024;
/// New messages read in one look (a mailbox that received thousands at once: the newest wait the next look).
pub const PER_LOOK: usize = 50;
pub const BACKOFF_START: f64 = 5.0;
pub const BACKOFF_MAX: f64 = 900.0;
/// How often the supervisor looks at which mailboxes are needed.
pub const SUPERVISE_SECONDS: f64 = 10.0;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Cursor {
    pub validity: u64,
    pub last_uid: u64,
}

static CURSORS: Mutex<()> = Mutex::new(());

fn cursors_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("triggers-mail.json")
}

fn load_cursors(config: &Config) -> HashMap<String, Cursor> {
    std::fs::read_to_string(cursors_path(config)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn cursor(config: &Config, account: &str) -> Option<Cursor> {
    load_cursors(config).remove(account)
}

fn save_cursor(config: &Config, account: &str, cursor: &Cursor) {
    let _held = CURSORS.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load_cursors(config);
    all.insert(account.to_string(), cursor.clone());
    let file = cursors_path(config);
    if let Ok(body) = serde_json::to_vec(&all) {
        let tmp = file.with_extension("part");
        if py::write_with_mode(&tmp.to_string_lossy(), &body, 0o600).is_ok() {
            let _ = std::fs::rename(&tmp, &file);
        }
    }
}

/// The mailboxes some active e-mail routine needs (id → account, password included), each only for agents it is
/// given to.
pub fn needed(config: &Config) -> HashMap<String, Dict> {
    let wanted: Vec<(String, String)> = triggers::load(config)
        .triggers
        .into_iter()
        .filter(|t| t.enabled && t.kind == MAIL && config.agent(Some(&t.agent)).is_some())
        .filter_map(|t| t.mail.map(|m| (m.account, t.agent)))
        .chain(crate::mail_rules::watched(config)) // the standing mail rules' mailboxes too (mail_rules.rs)
        .collect();
    if wanted.is_empty() {
        return HashMap::new();
    }
    let Ok(accounts) = mailbox::of(config).read() else { return HashMap::new() };
    let mut out = HashMap::new();
    for (id, agent) in wanted {
        if let Some(account) = accounts.iter().find(|a| tk::str_or_empty(a.get("id")) == id) {
            if tk::py_in(&agent, account.get("agents").unwrap_or(&json!([]))) {
                out.insert(id, account.clone());
            }
        }
    }
    out
}

/// What makes a new connection needed: where, who, the password.
fn fingerprint(account: &Dict) -> String {
    let mut h = Sha256::new();
    for k in ["host", "port", "username", "password", "starttls"] {
        h.update(py::py_str(account.get(k).unwrap_or(&Value::Null)).as_bytes());
        h.update([0]);
    }
    hex::encode(h.finalize())
}

fn numbers(data: &[Data]) -> Vec<u64> {
    let line = match data.first() {
        Some(Data::Line(l)) | Some(Data::Literal(l, _)) => l.clone(),
        _ => Vec::new(),
    };
    String::from_utf8_lossy(&line).split_whitespace().filter_map(|w| w.parse().ok()).collect()
}

/// The inbox's highest UID (`UID FETCH * (UID)`), 0 when empty.
fn newest_uid(conn: &mut Connection) -> Result<u64, String> {
    let (status, data) = conn.uid("FETCH", &["*", "(UID)"]).map_err(|e| e.to_string())?;
    if status != "OK" {
        return Ok(0);
    }
    let re = regex::Regex::new(r"UID (\d+)").expect("valid regex");
    let mut best = 0;
    for item in &data {
        if let Data::Line(l) | Data::Literal(l, _) = item {
            if let Some(c) = re.captures(&String::from_utf8_lossy(l)) {
                best = best.max(c[1].parse().unwrap_or(0));
            }
        }
    }
    Ok(best)
}

/// Whether some active e-mail routine of this mailbox wants its new messages.
fn has_routines(config: &Config, account: &str) -> bool {
    triggers::load(config)
        .triggers
        .iter()
        .any(|t| t.enabled && t.kind == MAIL && t.mail.as_ref().is_some_and(|m| m.account == account))
}

/// New messages' headers (mail_rules.rs: sender, list, flags, Message-ID), never their text.
fn headers_of(conn: &mut Connection, uids: &[u64]) -> Result<Vec<crate::mail_tidy::Row>, String> {
    let small: Vec<u32> = uids.iter().filter_map(|u| u32::try_from(*u).ok()).collect();
    let (status, data) = conn
        .uid("FETCH", &[&crate::mail_tidy::uid_set(&small), crate::mail_tidy::FETCH_ITEMS])
        .map_err(|e| e.to_string())?;
    Ok(if status == "OK" { crate::mail_tidy::parse_fetch(&data) } else { Vec::new() })
}

/// UIDs above `last`, in order.
fn new_uids(conn: &mut Connection, last: u64) -> Result<Vec<u64>, String> {
    let range = format!("{}:*", last + 1);
    let (status, data) = conn.uid("SEARCH", &["UID", &range]).map_err(|e| e.to_string())?;
    if status != "OK" {
        return Err("the search was refused".into());
    }
    let mut found: Vec<u64> = numbers(&data).into_iter().filter(|u| *u > last).collect();
    found.sort_unstable();
    found.dedup();
    Ok(found)
}

/// One message read (its first bytes), as an event. None: gone meanwhile.
fn read_one(conn: &mut Connection, account: &str, uid: u64) -> Result<Option<Event>, String> {
    let (status, data) = conn
        .uid("FETCH", &[&uid.to_string(), &format!("(BODY.PEEK[]<0.{FETCH_BYTES}>)")])
        .map_err(|e| e.to_string())?;
    let raw = if status == "OK" { data.iter().find_map(Data::literal).map(<[u8]>::to_vec) } else { None };
    let Some(raw) = raw.filter(|r| !r.is_empty()) else { return Ok(None) };
    let (message, text, files) = mailbox::message_text(&raw);
    let mut text = text;
    if !files.is_empty() {
        text.push_str(&format!("\n\n[attachments: {}]", files.join(", ")));
    }
    Ok(Some(Event {
        kind: MAIL.into(),
        at: py::time_time(),
        from: mailbox::decode(message.get("From").as_deref()),
        subject: mailbox::decode(message.get("Subject").as_deref()),
        date: message.get("Date").unwrap_or_default(),
        message_id: message.get("Message-ID").unwrap_or_default().trim().to_string(),
        mail_ref: format!("{account}|{FOLDER}|{uid}"),
        text,
        ..Event::default()
    }))
}

fn sleep_unless(stop: &AtomicBool, seconds: f64) {
    let until = std::time::Instant::now() + Duration::from_secs_f64(seconds.max(0.0));
    while std::time::Instant::now() < until && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Where a session ended: told to stop, or failed (plain words).
enum Ended {
    Stopped,
    Failed(String),
}

/// One connection's life: log in, catch up, then IDLE (or look every `poll_minutes`) until it fails or is stopped.
fn session(config: &Config, id: &str, account: &Dict, stop: &AtomicBool, tx: &UnboundedSender<(String, Event)>) -> Ended {
    let mut conn = match mailbox::connect(account) {
        Ok(c) => c,
        Err(e) => return Ended::Failed(e.0),
    };
    let ended = (|| -> Result<(), String> {
        let (status, _) = conn.select(&mailbox::quote_box(FOLDER), true).map_err(|e| e.to_string())?;
        if status != "OK" {
            return Err("no inbox".into());
        }
        let validity = conn.uid_validity().unwrap_or(0);
        let mut at = match cursor(config, id) {
            Some(c) if c.validity == validity => c,
            _ => {
                let fresh = Cursor { validity, last_uid: newest_uid(&mut conn)? };
                save_cursor(config, id, &fresh);
                tracing::info!(target: "bipbridge::triggers", account = id, uid = fresh.last_uid, "mailbox watched from its newest message");
                fresh
            }
        };
        let idle = conn.can_idle();
        let settings = config.triggers.clone();
        tracing::info!(target: "bipbridge::triggers", account = id, idle, "mailbox connected for routines");
        loop {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            let uids: Vec<u64> = new_uids(&mut conn, at.last_uid)?.into_iter().take(PER_LOOK).collect();
            // a mailbox watched only for its rules: no message read whole
            let routines = !uids.is_empty() && has_routines(config, id);
            for &uid in &uids {
                if routines {
                    if let Some(event) = read_one(&mut conn, id, uid)? {
                        let _ = tx.send((id.to_string(), event));
                    }
                }
                at.last_uid = uid;
                save_cursor(config, id, &at);
            }
            // then the standing rules file them (their headers only, another connection: mail_rules.rs)
            if !uids.is_empty() && crate::mail_rules::has_rules(config, id) {
                let rows = headers_of(&mut conn, &uids)?;
                crate::mail_rules::sort_new(config, id, &rows, &mailbox::default_connect());
            }
            if idle {
                conn.idle(Duration::from_secs_f64(settings.idle_minutes * 60.0), &|| stop.load(Ordering::Relaxed))
                    .map_err(|e| e.to_string())?;
            } else {
                sleep_unless(stop, settings.poll_minutes * 60.0);
                conn.noop().map_err(|e| e.to_string())?;
            }
        }
    })();
    let _ = conn.logout();
    match ended {
        Ok(()) => Ended::Stopped,
        Err(why) => Ended::Failed(why),
    }
}

/// A mailbox watched until `stop`, reconnecting with backoff.
fn watch(config: crate::config::SharedConfig, id: String, account: Dict, stop: Arc<AtomicBool>, tx: UnboundedSender<(String, Event)>) {
    let mut backoff = BACKOFF_START;
    while !stop.load(Ordering::Relaxed) {
        let started = std::time::Instant::now();
        match session(&config.get(), &id, &account, &stop, &tx) {
            Ended::Stopped => break,
            Ended::Failed(why) => {
                if started.elapsed() > Duration::from_secs(300) {
                    backoff = BACKOFF_START; // it held a while: a short break
                }
                tracing::warn!(target: "bipbridge::triggers", account = %id, why = %why, retry_in = backoff, "mailbox watch failed");
                sleep_unless(&stop, backoff);
                backoff = (backoff * 2.0).min(BACKOFF_MAX);
            }
        }
    }
    tracing::info!(target: "bipbridge::triggers", account = %id, "mailbox no longer watched");
}

struct Worker {
    stop: Arc<AtomicBool>,
    fingerprint: String,
    thread: std::thread::JoinHandle<()>,
}

/// A new message offered to the routines of its mailbox: the ones whose filter it fits get it queued.
pub fn offer(config: &Config, account: &str, event: &Event) -> usize {
    let mut queued = 0;
    for t in triggers::load(config).triggers {
        let Some(filter) = t.mail.as_ref().filter(|m| m.account == account) else { continue };
        if !t.enabled || t.kind != MAIL {
            continue;
        }
        if !triggers::matches(filter, &event.from, &event.subject, &event.text) {
            continue;
        }
        match triggers::enqueue(config, &t.id, event.clone()) {
            Ok(true) => {
                queued += 1;
                tracing::info!(target: "bipbridge::triggers", agent = %t.agent, trigger = %t.id, "e-mail queued for a routine");
            }
            Ok(false) => {}
            Err(why) => tracing::info!(target: "bipbridge::triggers", trigger = %t.id, why = %why, "e-mail not queued"),
        }
    }
    queued
}

/// Keeps one watcher per mailbox needed, and hands their messages to the routines.
pub async fn supervisor(state: AppState) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, Event)>();
    let receiver = state.clone();
    tokio::spawn(async move {
        while let Some((account, event)) = rx.recv().await {
            let config = receiver.config();
            let _ = tokio::task::spawn_blocking(move || offer(&config, &account, &event)).await;
        }
    });
    let mut workers: HashMap<String, Worker> = HashMap::new();
    loop {
        let config = state.config();
        let wanted = if config.triggers.mail {
            let c = config.clone();
            tokio::task::spawn_blocking(move || needed(&c)).await.unwrap_or_default()
        } else {
            HashMap::new()
        };
        workers.retain(|id, w| {
            let keep = !w.thread.is_finished() && wanted.get(id).is_some_and(|a| fingerprint(a) == w.fingerprint);
            if !keep {
                w.stop.store(true, Ordering::Relaxed);
            }
            keep
        });
        for (id, account) in wanted {
            if workers.contains_key(&id) {
                continue;
            }
            let stop = Arc::new(AtomicBool::new(false));
            let (shared, s, t, i, a) = (state.config.clone(), stop.clone(), tx.clone(), id.clone(), account.clone());
            let spawned = std::thread::Builder::new()
                .name(format!("mail-watch-{}", tk::head(&id, 8)))
                .spawn(move || watch(shared, i, a, s, t));
            match spawned {
                Ok(thread) => {
                    workers.insert(id, Worker { stop, fingerprint: fingerprint(&account), thread });
                }
                Err(err) => tracing::warn!(target: "bipbridge::triggers", error = %err, "mailbox watch not started"),
            }
        }
        tokio::time::sleep(Duration::from_secs_f64(SUPERVISE_SECONDS.min(config.triggers.tick_seconds * 2.0).max(0.5)))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    /// A small IMAP server with IDLE: one message at first, a second told while idling.
    fn serve() -> (u16, std::sync::mpsc::Sender<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            sock.write_all(b"* OK [CAPABILITY IMAP4rev1 IDLE] ready\r\n").unwrap();
            let mut reader = BufReader::new(sock.try_clone().unwrap());
            let mut messages: Vec<(u64, String)> = vec![(7, "Message-ID: <a@x>\r\nFrom: a@x\r\nSubject: un\r\n\r\nUn".into())];
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                let tag = line.split(' ').next().unwrap().to_string();
                let rest = line[tag.len()..].trim().to_string();
                let upper = rest.to_uppercase();
                if upper.starts_with("CAPABILITY") {
                    sock.write_all(b"* CAPABILITY IMAP4rev1 IDLE\r\n").unwrap();
                } else if upper.starts_with("EXAMINE") {
                    sock.write_all(b"* 1 EXISTS\r\n* OK [UIDVALIDITY 3] ok\r\n").unwrap();
                } else if upper.starts_with("UID FETCH * (UID)") {
                    sock.write_all(b"* 1 FETCH (UID 7)\r\n").unwrap();
                } else if upper.starts_with("UID SEARCH UID") {
                    let low: u64 = rest.split_whitespace().nth(3).unwrap().split(':').next().unwrap().parse().unwrap();
                    let mut uids: Vec<String> = messages.iter().filter(|m| m.0 >= low).map(|m| m.0.to_string()).collect();
                    if uids.is_empty() {
                        uids.push(messages.last().unwrap().0.to_string()); // `n:*` always gives the last one
                    }
                    sock.write_all(format!("* SEARCH {}\r\n", uids.join(" ")).as_bytes()).unwrap();
                } else if upper.starts_with("UID FETCH") {
                    let uid: u64 = rest.split_whitespace().nth(2).unwrap().parse().unwrap();
                    let raw = &messages.iter().find(|m| m.0 == uid).unwrap().1;
                    sock.write_all(format!("* 1 FETCH (UID {uid} BODY[]<0> {{{}}}\r\n{raw})\r\n", raw.len()).as_bytes()).unwrap();
                } else if upper.starts_with("IDLE") {
                    sock.write_all(b"+ idling\r\n").unwrap();
                    // news when the test says so, DONE whenever the client sends it
                    sock.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
                    let mut done = String::new();
                    loop {
                        if rx.try_recv().is_ok() {
                            messages.push((8, "Message-ID: <b@x>\r\nFrom: Banque <b@banque.fr>\r\nSubject: deux\r\n\r\nDeux".into()));
                            sock.write_all(b"* 2 EXISTS\r\n").unwrap();
                        }
                        match reader.read_line(&mut done) {
                            Ok(0) => return,
                            Ok(_) if done.ends_with('\n') => break,
                            _ => {}
                        }
                    }
                    sock.set_read_timeout(None).unwrap();
                    assert_eq!(done.trim(), "DONE");
                } else if upper.starts_with("LOGOUT") {
                    sock.write_all(b"* BYE\r\n").unwrap();
                    sock.write_all(format!("{tag} OK bye\r\n").as_bytes()).unwrap();
                    return;
                }
                sock.write_all(format!("{tag} OK done\r\n").as_bytes()).unwrap();
            }
        });
        (port, tx)
    }

    #[test]
    fn idle_tells_new_mail_and_only_new_mail() {
        let (port, news) = serve();
        let client = crate::tools_knowledge::imap::Client::connect_local("127.0.0.1", port, Duration::from_secs(5)).unwrap();
        let mut conn: Connection = Box::new(client);
        conn.select("\"INBOX\"", true).unwrap();
        assert_eq!(conn.uid_validity(), Some(3));
        assert_eq!(newest_uid(&mut conn).unwrap(), 7);
        // `8:*` answers the last message (7) when nothing is newer: left out
        assert!(new_uids(&mut conn, 7).unwrap().is_empty());
        assert!(conn.can_idle());
        news.send(()).unwrap();
        assert!(conn.idle(Duration::from_secs(10), &|| false).unwrap());
        assert_eq!(new_uids(&mut conn, 7).unwrap(), vec![8]);
        let event = read_one(&mut conn, "acct", 8).unwrap().unwrap();
        assert_eq!(event.message_id, "<b@x>");
        assert_eq!(event.mail_ref, "acct|INBOX|8");
        assert!(event.from.contains("banque.fr"));
        // nothing new: the IDLE ends at its time, or when told to stop
        let started = std::time::Instant::now();
        assert!(!conn.idle(Duration::from_millis(800), &|| false).unwrap());
        assert!(started.elapsed() < Duration::from_secs(4));
        let stop = AtomicBool::new(true);
        assert!(!conn.idle(Duration::from_secs(60), &|| stop.load(Ordering::Relaxed)).unwrap());
        conn.logout().unwrap();
    }
}
