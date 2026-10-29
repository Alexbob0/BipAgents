//! The current time, at the top of each turn the bridge starts for an agent (Alex, 2026-10-29: « C'est super
//! important »).
//!
//! Hermes' system prompt only had the conversation's start date and, once rebuilt on a later day, an « as of the last
//! rebuild » one: conversations last days and the prompt cache keeps it, so it went stale; and the prompt sent the
//! model to the terminal (`date`) for the time, on servers that run in UTC. Now ONE hidden line, computed at send
//! time in the person's zone and the agent's language, starts the new user message (never the system prompt: the
//! cache holds): `<!--bip:now jeudi 29 octobre 2026, 08:40, Europe/Paris (UTC+01:00)-->`. Hermes patch 0015 says in
//! the prompt to trust it, and puts the same line, in the routines' zone, at the top of each routine run.
//!
//! Where: the apps' messages (`POST /v1/live/recaps/take` answers it first, the apps put it before the message), a
//! Live call's brief (`GET /v1/live/brief`), and every turn the bridge starts itself (watchers, handbacks, background
//! tasks, rooms, shared agents, the brake's rescue, messages between agents). The apps hide every `<!--bip:…-->`
//! note, and shared conversations leave them out (read_shares.rs).

use chrono::{DateTime, Offset, Utc};
use chrono_tz::Tz;
use serde_json::Value;

use crate::config::{AgentConfig, Config};

/// How the line starts (the apps' and Hermes' readers look for it).
pub const OPEN: &str = "<!--bip:now ";

/// `<!--bip:now jeudi 29 octobre 2026, 08:40, Europe/Paris (UTC+01:00)-->` for this instant in `zone`.
pub fn line_at(at: DateTime<Utc>, zone: Tz, lang: &str) -> String {
    let local = at.with_timezone(&zone);
    let seconds = local.offset().fix().local_minus_utc();
    let sign = if seconds < 0 { '-' } else { '+' };
    let (h, m) = (seconds.abs() / 3600, seconds.abs() % 3600 / 60);
    let when = super::words::now_text(local.naive_local(), super::words::lang(lang));
    format!("{OPEN}{when}, {} (UTC{sign}{h:02}:{m:02})-->", zone.name())
}

/// The person's zone (dates/mod.rs), else the one the agent's profile says, else the server's.
pub fn zone_of(config: &Config, agent: &AgentConfig) -> Tz {
    let person = super::load(config);
    if let Some(zone) = super::parse_zone(&person.time_zone) {
        return zone;
    }
    let profile = crate::agents_admin::profile_folder(config, agent).join("config.yaml");
    let named = crate::agents_admin::load_or_empty(&profile)
        .ok()
        .and_then(|data| data.get("timezone").and_then(|z| z.as_str().map(str::to_string)));
    named.as_deref().and_then(super::parse_zone).unwrap_or_else(|| person.zone())
}

/// The agent's line now, in its language.
pub fn line(config: &Config, agent: &AgentConfig) -> String {
    line_at(Utc::now(), zone_of(config, agent), &crate::handback::language_of(config, agent))
}

/// What starts a turn of the agent: its time line, then what waits for it in the Carnet (notebook/news.rs, taken
/// here: given once).
pub fn head(config: &Config, agent: &AgentConfig) -> String {
    let mut out = line(config, agent);
    if let Some(news) = crate::notebook::news::take_now(config, &agent.name) {
        out = format!("{out}\n{news}");
    }
    // the friends' votes and ticks on a project agent's cards (friends_cards.rs), as information
    if let Some(taps) = crate::friends_cards::take_note(config, &agent.name) {
        out = format!("{out}\n{taps}");
    }
    out
}

/// `text` with the agent's line (and its Carnet news) at its top (as it is when it already starts with one).
pub fn stamped(config: &Config, agent: &AgentConfig, text: &str) -> String {
    if text.trim_start().starts_with(OPEN) {
        return text.to_string();
    }
    format!("{}\n{text}", head(config, agent))
}

/// Whether a Hermes request's JSON body has a text without the line (`stamped_body` would change it).
pub fn wants_line(body: &[u8]) -> bool {
    let Ok(Value::Object(data)) = serde_json::from_slice::<Value>(body) else { return false };
    ["input", "message"]
        .iter()
        .any(|f| matches!(data.get(*f), Some(Value::String(text)) if !text.trim_start().starts_with(OPEN)))
}

/// `stamped_body` with the agent's `head` (its Carnet news taken only when the body gets it).
pub fn stamped_body_for(config: &Config, agent: &AgentConfig, body: &[u8]) -> Vec<u8> {
    if !wants_line(body) {
        return body.to_vec();
    }
    stamped_body(body, &head(config, agent))
}

/// A Hermes request's JSON body (`/v1/runs`' `input`, a session chat's `message`) with the line at the top of its
/// text; anything else as it is.
pub fn stamped_body(body: &[u8], line: &str) -> Vec<u8> {
    let Ok(Value::Object(mut data)) = serde_json::from_slice::<Value>(body) else { return body.to_vec() };
    let mut changed = false;
    for field in ["input", "message"] {
        if let Some(Value::String(text)) = data.get(field) {
            if !text.trim_start().starts_with(OPEN) {
                let text = format!("{line}\n{text}");
                data.insert(field.to_string(), Value::String(text));
                changed = true;
            }
        }
    }
    if !changed {
        return body.to_vec();
    }
    crate::py::dumps(&Value::Object(data)).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn the_line_in_each_language() {
        let at = utc(2026, 10, 29, 7, 40);
        let paris: Tz = "Europe/Paris".parse().unwrap();
        assert_eq!(line_at(at, paris, "fr"), "<!--bip:now jeudi 29 octobre 2026, 08:40, Europe/Paris (UTC+01:00)-->");
        assert_eq!(
            line_at(at, paris, "en"),
            "<!--bip:now Thursday, October 29, 2026, 08:40, Europe/Paris (UTC+01:00)-->"
        );
        assert_eq!(
            line_at(at, paris, "de-DE"),
            "<!--bip:now Donnerstag, 29. Oktober 2026, 08:40, Europe/Paris (UTC+01:00)-->"
        );
        assert_eq!(
            line_at(at, paris, "es"),
            "<!--bip:now jueves 29 de octubre de 2026, 08:40, Europe/Paris (UTC+01:00)-->"
        );
        // a language without its own words: English; the first of the month in French
        assert!(line_at(at, paris, "nl").starts_with("<!--bip:now Thursday, October 29, 2026"));
        assert!(
            line_at(utc(2026, 11, 1, 9, 5), paris, "fr").starts_with("<!--bip:now dimanche 1er novembre 2026, 10:05")
        );
    }

    #[test]
    fn clock_changes_and_odd_offsets() {
        let paris: Tz = "Europe/Paris".parse().unwrap();
        // 2026-10-25: 03:00 CEST becomes 02:00 CET, the 02:xx hour twice
        assert!(line_at(utc(2026, 10, 25, 0, 30), paris, "fr").ends_with("02:30, Europe/Paris (UTC+02:00)-->"));
        assert!(line_at(utc(2026, 10, 25, 1, 30), paris, "fr").ends_with("02:30, Europe/Paris (UTC+01:00)-->"));
        // 2026-03-08 in New York: 02:00 EST jumps to 03:00 EDT
        let new_york: Tz = "America/New_York".parse().unwrap();
        let before = line_at(utc(2026, 3, 8, 6, 30), new_york, "en");
        assert_eq!(before, "<!--bip:now Sunday, March 8, 2026, 01:30, America/New_York (UTC-05:00)-->");
        assert!(line_at(utc(2026, 3, 8, 7, 30), new_york, "en").ends_with("03:30, America/New_York (UTC-04:00)-->"));
        // late evening in UTC is already tomorrow in Tokyo; half and quarter hours
        let tokyo: Tz = "Asia/Tokyo".parse().unwrap();
        assert!(
            line_at(utc(2026, 10, 29, 22, 0), tokyo, "fr").starts_with("<!--bip:now vendredi 30 octobre 2026, 07:00")
        );
        assert!(line_at(utc(2026, 1, 1, 0, 0), "Asia/Kathmandu".parse().unwrap(), "en").ends_with("(UTC+05:45)-->"));
        assert!(line_at(utc(2026, 1, 1, 0, 0), "America/St_Johns".parse().unwrap(), "en").ends_with("(UTC-03:30)-->"));
        assert!(line_at(utc(2026, 1, 1, 0, 0), Tz::UTC, "en").ends_with(", UTC (UTC+00:00)-->"));
    }

    #[test]
    fn the_zone_is_the_person_s_else_the_profile_s_else_the_server_s() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k".repeat(40));
        config.outbox.db_path = dir.path().join("data/outbox.db").to_string_lossy().into_owned();
        config.hermes.home = dir.path().join("hermes").to_string_lossy().into_owned();
        let agent = AgentConfig { name: "vie".into(), ..Default::default() };
        // nothing known: the server's
        assert_eq!(zone_of(&config, &agent).name(), super::super::server_zone());
        // the profile's
        let folder = crate::agents_admin::profile_folder(&config, &agent);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("config.yaml"), "timezone: Asia/Tokyo\n").unwrap();
        assert_eq!(zone_of(&config, &agent).name(), "Asia/Tokyo");
        // the person's wins
        super::super::change(&config, |p| p.time_zone = "America/New_York".into()).unwrap();
        assert_eq!(zone_of(&config, &agent).name(), "America/New_York");
        // a zone that isn't one: the profile's again
        super::super::change(&config, |p| p.time_zone = "Mars/Olympus".into()).unwrap();
        assert_eq!(zone_of(&config, &agent).name(), "Asia/Tokyo");
        let text = stamped(&config, &agent, "Salut");
        assert!(text.starts_with(OPEN) && text.contains(", Asia/Tokyo (UTC+09:00)-->\nSalut"), "{text}");
        assert_eq!(stamped(&config, &agent, &text), text, "never twice");
    }

    #[test]
    fn a_request_body_gets_the_line_once() {
        let line = "<!--bip:now jeudi 29 octobre 2026, 08:40, Europe/Paris (UTC+01:00)-->";
        let out: Value =
            serde_json::from_slice(&stamped_body(br#"{"input": "salut", "session_id": "s"}"#, line)).unwrap();
        assert_eq!(out["input"], format!("{line}\nsalut"));
        assert_eq!(out["session_id"], "s");
        let again = stamped_body(&serde_json::to_vec(&out).unwrap(), line);
        assert_eq!(serde_json::from_slice::<Value>(&again).unwrap()["input"], format!("{line}\nsalut"));
        let chat: Value = serde_json::from_slice(&stamped_body(br#"{"message": "ok"}"#, line)).unwrap();
        assert_eq!(chat["message"], format!("{line}\nok"));
        // parts (pictures), not JSON: as they are
        let parts = br#"{"input": [{"type": "text", "text": "x"}]}"#;
        assert_eq!(stamped_body(parts, line), parts.to_vec());
        assert_eq!(stamped_body(b"not json", line), b"not json".to_vec());
        // the apps' and shares' reader leaves it out
        assert_eq!(crate::live_brief::visible(&format!("{line}\nsalut")), "salut");
    }
}
