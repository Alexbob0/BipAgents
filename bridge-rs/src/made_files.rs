//! The files the bridge made for an agent's turn (audio, pages, maps), so the reply that ends that turn hands them
//! over whatever the model wrote (2026-10-28, aibox: a 07:50 routine called create_podcast, the bridge made today's
//! file, and the reply carried yesterday's `MEDIA:` path, written from memory: the agent reads its last two days'
//! outputs not to repeat itself, so old `MEDIA:` lines sit in its context; the person got the text without today's
//! podcast). The bridge doesn't depend on the model copying a path.
//!
//! The tools that make a file (`create_audio`, `create_podcast`, `create_page`, `map`: agent_tools.rs, the `MEDIA:`
//! line first in their result) note it here, with the turn it was made in: the run the hub follows
//! (`RunHub::turn_of`), or none (a routine: Hermes' cron, which the bridge only watches). When the reply that ends the
//! turn is delivered (runs.rs `hand_over_files` for the runs the hub follows, cronwatch.rs for a routine's output: the
//! files made by that agent with no run, between the routine's start and its output), `fix_reply`:
//!
//! - replaces a `MEDIA:` line pointing at another file of the same kind and agent (`/bipagents/audio/<agent>/…`,
//!   `/bipagents/pages/<agent>/…`, `/bipagents/maps/<agent>/…`) by a file of that kind made in the turn that the
//!   reply doesn't carry;
//! - adds a `MEDIA:` line, at the end, for an audio file or a page made in the turn that the reply still doesn't
//!   carry (not a map's picture: a map is mostly pasted into a page, its picture alone is optional);
//! - never touches any other `MEDIA:` line (photos, documents, another agent's files), nor anything when no file was
//!   made in the turn (an older podcast the person asked for again stays);
//! - keeps only the last version of an audio file remade in the turn (2026-10-30 bench, DeepSeek: « a 2-minute
//!   podcast » came out at 1 min 21, the agent made it again, twice, and left all three `MEDIA:` lines, so the person
//!   got three files). A version is an audio file of the turn with the same title as a later one (its name without
//!   the time stamp: `20261030-101503-mars.m4a` and `20261030-101722-mars.m4a`): the earlier one's line goes. Two
//!   podcasts with different titles, pages, documents: all kept.
//!
//! In memory only: a restart forgets files made before it (the model's own lines are then kept as they are).
//!
//! No Python counterpart (owner group: push-runs).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use serde_json::Value;

/// The kinds of files the bridge makes, by `MEDIA:` prefix, and whether a reply must carry one made in its turn.
const KINDS: [(&str, bool); 3] = [
    (crate::agent_tools::MEDIA_PREFIX, true),
    (crate::pages::PREFIX, true),
    (crate::maps::PREFIX, false),
];

/// The tools whose result hands a file the bridge made.
pub fn makes_files(tool: &str) -> bool {
    [
        crate::agent_tools::CREATE_AUDIO,
        crate::agent_tools::CREATE_PODCAST,
        crate::agent_tools::CREATE_PAGE,
        crate::maps::TOOL_NAME,
    ]
    .contains(&tool)
}

/// The kind of `path` for `agent` (its prefix, and whether a reply must carry it), if one the bridge makes.
fn kind_of(path: &str, agent: &str) -> Option<(&'static str, bool)> {
    KINDS.iter().copied().find(|(prefix, _)| path.starts_with(&format!("{prefix}/{agent}/")))
}

/// One file made: its `MEDIA:` path, when, and the run it was made in (None: a turn no run carries).
#[derive(Debug, Clone, PartialEq)]
struct Made {
    path: String,
    at: f64,
    run_id: Option<String>,
}

#[derive(Debug, Default)]
pub struct MadeFiles {
    by_agent: Mutex<HashMap<String, Vec<Made>>>,
}

pub static MADE: LazyLock<MadeFiles> = LazyLock::new(MadeFiles::default);

/// A day: a turn's reply comes long before.
const KEEP_SECONDS: f64 = 86_400.0;

impl MadeFiles {
    /// `agent` got `path` (its `MEDIA:` path) at `at`, in the run `run_id` (None: a routine's turn).
    pub fn note(&self, agent: &str, path: &str, run_id: Option<&str>, at: f64) {
        let mut all = self.by_agent.lock().unwrap_or_else(|e| e.into_inner());
        let files = all.entry(agent.to_string()).or_default();
        files.retain(|m| at - m.at < KEEP_SECONDS && !(m.path == path && m.run_id.as_deref() == run_id));
        files.push(Made { path: path.to_string(), at, run_id: run_id.map(str::to_string) });
    }

    /// A file-making tool's result that went well (`makes_files`): its file noted (the `MEDIA:` line it gives).
    pub fn note_result(&self, agent: &str, result: &crate::Dict, run_id: Option<&str>, at: f64) {
        if result.get("isError").and_then(Value::as_bool).unwrap_or(false) {
            return;
        }
        let text = result
            .get("content")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if let Some(path) = MEDIA_LINE.captures_iter(text).map(|c| c[1].to_string()).find(|p| kind_of(p, agent).is_some())
        {
            tracing::info!(agent = %agent, path = %path, run_id = ?run_id, "file made in the turn noted");
            self.note(agent, &path, run_id, at);
        }
    }

    /// The files made in the run `run_id`, oldest first.
    pub fn in_run(&self, agent: &str, run_id: &str) -> Vec<String> {
        let all = self.by_agent.lock().unwrap_or_else(|e| e.into_inner());
        all.get(agent)
            .map(|files| files.iter().filter(|m| m.run_id.as_deref() == Some(run_id)).map(|m| m.path.clone()).collect())
            .unwrap_or_default()
    }

    /// The files made by `agent` in a turn no run carries (a routine), between `from` and `to` (Unix seconds).
    pub fn in_routine(&self, agent: &str, from: f64, to: f64) -> Vec<String> {
        let all = self.by_agent.lock().unwrap_or_else(|e| e.into_inner());
        all.get(agent)
            .map(|files| {
                files
                    .iter()
                    .filter(|m| m.run_id.is_none() && m.at >= from && m.at <= to)
                    .map(|m| m.path.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// An audio file's title as its name says it (`/bipagents/audio/<agent>/20261030-101503-mars.m4a` → `mars`): the
/// key of its versions in one turn. None for any other file.
fn audio_title(path: &str) -> Option<&str> {
    let file = path.strip_prefix(crate::agent_tools::MEDIA_PREFIX)?.rsplit('/').next()?;
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    // the time stamp `%Y%m%d-%H%M%S-` the maker puts first (agent_tools.rs `save`)
    let (stamp, title) = (stem.get(..16)?, stem.get(16..)?);
    let stamped = stamp.bytes().enumerate().all(|(i, b)| if i == 8 || i == 15 { b == b'-' } else { b.is_ascii_digit() });
    (stamped && !title.is_empty()).then_some(title)
}

/// The files of `made` (oldest first) that a later one of the turn replaces: an audio file remade with the same title.
pub fn superseded(made: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (index, path) in made.iter().enumerate() {
        let Some(title) = audio_title(path) else { continue };
        if made[index + 1..].iter().any(|later| later != path && audio_title(later) == Some(title)) && !out.contains(path) {
            out.push(path.clone());
        }
    }
    out
}

/// The first `MEDIA:` path a tool's result gives (None for an error or no line).
pub fn media_path_of(result: &crate::Dict) -> Option<String> {
    if result.get("isError").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let text = result.get("content")?.get(0)?.get("text")?.as_str()?;
    MEDIA_LINE.captures(text).map(|c| c[1].to_string())
}

/// `MEDIA:<path>` on a line of its own (spaces around allowed).
static MEDIA_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[ \t]*MEDIA:[ \t]*(\S+)[ \t]*$").expect("valid regex"));

/// The files made in the turn that `reply` doesn't carry and must (audio, pages), for the screen's last delta.
pub fn missing_from(reply: &str, agent: &str, made: &[String]) -> Vec<String> {
    let carried: Vec<String> = MEDIA_LINE.captures_iter(reply).map(|c| c[1].to_string()).collect();
    let replaced = superseded(made);
    let mut out: Vec<String> = Vec::new();
    for path in made {
        if !carried.contains(path)
            && !out.contains(path)
            && !replaced.contains(path)
            && kind_of(path, agent).is_some_and(|(_, required)| required)
        {
            out.push(path.clone());
        }
    }
    out
}

/// `reply` handing over the files `made` in its turn by `agent` (see the module's rules); None when it already does,
/// or when nothing was made.
pub fn fix_reply(reply: &str, agent: &str, made: &[String]) -> Option<String> {
    if made.is_empty() {
        return None;
    }
    // the earlier versions of a file remade in the turn: their lines dropped, never added back
    let input = reply;
    let replaced = superseded(made);
    let without_old;
    let reply = if replaced.is_empty() {
        reply
    } else {
        without_old = drop_lines(reply, &replaced);
        without_old.as_str()
    };
    let made: Vec<String> = made.iter().filter(|m| !replaced.contains(m)).cloned().collect();
    let made = made.as_slice();
    let marks = missing_marks(reply, made);
    let carried: Vec<String> = MEDIA_LINE.captures_iter(reply).map(|c| c[1].to_string()).collect();
    let mut missing: Vec<String> = Vec::new();
    for path in made {
        if !carried.contains(path) && !missing.contains(path) && kind_of(path, agent).is_some() {
            missing.push(path.clone());
        }
    }
    if missing.is_empty() {
        if marks.is_empty() {
            return (reply != input).then(|| reply.to_string());
        }
        let mut fixed = reply.trim_end().to_string();
        for mark in marks {
            if !fixed.is_empty() {
                fixed.push('\n');
            }
            fixed.push_str(&mark);
        }
        return Some(fixed);
    }
    let mut fixed = MEDIA_LINE
        .replace_all(reply, |caps: &regex::Captures| {
            let path = &caps[1];
            if let Some((prefix, _)) = kind_of(path, agent).filter(|_| !made.iter().any(|m| m == path)) {
                // a stale file of this kind: the first new one of the same kind instead
                if let Some(at) = missing.iter().position(|m| m.starts_with(&format!("{prefix}/"))) {
                    return format!("MEDIA:{}", missing.remove(at));
                }
            }
            caps[0].to_string()
        })
        .into_owned();
    let rest: Vec<&String> =
        missing.iter().filter(|p| kind_of(p, agent).is_some_and(|(_, required)| required)).collect();
    if !rest.is_empty() {
        let end = fixed.trim_end().len();
        fixed.truncate(end);
        for path in rest {
            if !fixed.is_empty() {
                fixed.push('\n');
            }
            fixed.push_str(&format!("MEDIA:{path}"));
        }
    }
    for mark in marks {
        let end = fixed.trim_end().len();
        fixed.truncate(end);
        if !fixed.is_empty() {
            fixed.push('\n');
        }
        fixed.push_str(&mark);
    }
    (fixed != input).then_some(fixed)
}

/// `reply` without the `MEDIA:` lines of `paths` (a blank line they leave behind squeezed with its neighbour).
fn drop_lines(reply: &str, paths: &[String]) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in reply.split('\n') {
        let dropped = MEDIA_LINE.captures(line).is_some_and(|c| paths.iter().any(|p| p == &c[1]));
        if dropped {
            continue;
        }
        if line.trim().is_empty() && out.last().is_none_or(|l| l.trim().is_empty()) {
            continue;
        }
        out.push(line);
    }
    let joined = out.join("\n");
    joined.trim_end().to_string()
}

/// Whether `made` holds a vault entry's mark (vault_contacts.rs: the reply's chip) rather than a file's path.
pub fn is_mark(made: &str) -> bool {
    made.starts_with(crate::vault_contacts::MARK_OPEN) || made.starts_with(crate::guide::RENAMED_OPEN)
}

/// The line a reply carries for `made`: a file's `MEDIA:` line, or a vault entry's mark as it is.
pub fn line_of(made: &str) -> String {
    if is_mark(made) {
        made.to_string()
    } else {
        format!("MEDIA:{made}")
    }
}

/// The vault entries' marks noted in the turn that `reply` doesn't carry yet (by their entry's id: the model may have
/// copied the label differently), each once.
fn missing_marks(reply: &str, made: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // a rename (guide.rs `rename_yourself`): its mark as it is, the last one only (renamed twice in a turn)
    if let Some(mark) = made.iter().rev().find(|m| m.starts_with(crate::guide::RENAMED_OPEN)) {
        if !reply.contains(mark.as_str()) {
            out.push(mark.clone());
        }
    }
    for mark in made.iter().filter(|m| is_mark(m)) {
        let Some(id) = crate::vault_contacts::mark_id(mark) else { continue };
        let carried = reply.contains(&format!("{}{id}", crate::vault_contacts::MARK_OPEN));
        if !carried && !out.iter().any(|m| crate::vault_contacts::mark_id(m) == Some(id)) {
            out.push(mark.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: &str = "/bipagents/audio/vie/20261028-075253-podcast-du-28-octobre-2026.m4a";
    const YESTERDAY: &str = "/bipagents/audio/vie/20261027-095308-podcast-du-mardi-27-octobre.m4a";
    const PAGE: &str = "/bipagents/pages/vie/budget-novembre.html";
    const OLD_PAGE: &str = "/bipagents/pages/vie/budget-octobre.html";
    const MAP: &str = "/bipagents/maps/vie/m-0123456789.png";
    const OLD_MAP: &str = "/bipagents/maps/vie/m-9876543210.png";

    #[test]
    fn a_missing_line_is_added() {
        let reply = "🎙️ Ton podcast du jour est prêt : météo, agenda et une idée de sortie.";
        assert_eq!(fix_reply(reply, "vie", &[TODAY.into()]).unwrap(), format!("{reply}\nMEDIA:{TODAY}"));
        // trailing blank lines kept tidy
        assert_eq!(fix_reply("Voilà.\n\n", "vie", &[TODAY.into()]).unwrap(), format!("Voilà.\nMEDIA:{TODAY}"));
        // a page too
        assert_eq!(fix_reply("Ton budget.", "vie", &[PAGE.into()]).unwrap(), format!("Ton budget.\nMEDIA:{PAGE}"));
        assert_eq!(missing_from("Ton budget.", "vie", &[PAGE.into(), MAP.into()]), vec![PAGE.to_string()]);
    }

    #[test]
    fn a_map_picture_is_never_added_only_replaced() {
        // a map pasted into a page: its picture alone is optional
        assert_eq!(fix_reply(&format!("Le plan.\nMEDIA:{PAGE}"), "vie", &[MAP.into(), PAGE.into()]), None);
        let stale = format!("La carte.\nMEDIA:{OLD_MAP}");
        assert_eq!(fix_reply(&stale, "vie", &[MAP.into()]).unwrap(), format!("La carte.\nMEDIA:{MAP}"));
    }

    #[test]
    fn a_stale_line_is_replaced_by_one_of_its_kind() {
        // the aibox case: yesterday's path written from memory
        let reply = format!("🎙️ Podcast du 28 octobre prêt.\n\nMEDIA:{YESTERDAY}");
        assert_eq!(fix_reply(&reply, "vie", &[TODAY.into()]).unwrap(), format!("🎙️ Podcast du 28 octobre prêt.\n\nMEDIA:{TODAY}"));
        // an old page and an old podcast, a new page and a new podcast: each by its own kind
        let both = format!("Page :\nMEDIA:{OLD_PAGE}\nAudio :\nMEDIA:{YESTERDAY}");
        assert_eq!(
            fix_reply(&both, "vie", &[TODAY.into(), PAGE.into()]).unwrap(),
            format!("Page :\nMEDIA:{PAGE}\nAudio :\nMEDIA:{TODAY}")
        );
        // a new podcast only: the old page stays
        let page_kept = format!("MEDIA:{OLD_PAGE}\nMEDIA:{YESTERDAY}");
        assert_eq!(fix_reply(&page_kept, "vie", &[TODAY.into()]).unwrap(), format!("MEDIA:{OLD_PAGE}\nMEDIA:{TODAY}"));
    }

    #[test]
    fn a_right_line_and_other_files_are_left_alone() {
        let right = format!("Prêt.\nMEDIA:{TODAY}");
        assert_eq!(fix_reply(&right, "vie", &[TODAY.into()]), None);
        // a photo, a document, another agent's audio: never touched; the new file added
        let others = "Voici la photo.\nMEDIA:/home/hermes/.hermes/media/photo.jpg\nMEDIA:/envoi/devis.pdf\n\
MEDIA:/bipagents/audio/coach/20261027-seance.m4a";
        assert_eq!(fix_reply(others, "vie", &[TODAY.into()]).unwrap(), format!("{others}\nMEDIA:{TODAY}"));
    }

    #[test]
    fn a_vault_entry_used_in_the_turn_gets_its_chip() {
        let mark = crate::vault_contacts::mark("item_0123456789ab", "Kerfy Studio (pro)");
        let reply = "J'ai enregistré tes coordonnées pro.";
        assert_eq!(fix_reply(reply, "vie", std::slice::from_ref(&mark)).unwrap(), format!("{reply}\n{mark}"));
        // carried already, even with another label: nothing added
        assert_eq!(fix_reply(&format!("{reply}\n{mark}"), "vie", std::slice::from_ref(&mark)), None);
        let other_label = format!("{reply}\n<!--bip:vault item_0123456789ab Kerfy-->");
        assert_eq!(fix_reply(&other_label, "vie", std::slice::from_ref(&mark)), None);
        // with a page made in the same turn: both, the mark last, each once
        let fixed = fix_reply(reply, "vie", &[PAGE.into(), mark.clone(), mark.clone()]).unwrap();
        assert_eq!(fixed, format!("{reply}\nMEDIA:{PAGE}\n{mark}"));
        assert_eq!(line_of(&mark), mark);
        assert_eq!(line_of(PAGE), format!("MEDIA:{PAGE}"));
    }

    #[test]
    fn a_remade_podcast_keeps_only_its_last_version() {
        // the 2026-10-30 bench (DeepSeek): 1 min 21, remade 1 min 43, remade 2 min 06, all three lines in the reply
        let first = "/bipagents/audio/vie/20261030-101503-mars-en-2-minutes.m4a";
        let second = "/bipagents/audio/vie/20261030-101722-mars-en-2-minutes.m4a";
        let third = "/bipagents/audio/vie/20261030-101950-mars-en-2-minutes.m4a";
        let made: Vec<String> = vec![first.into(), second.into(), third.into()];
        assert_eq!(superseded(&made), vec![first.to_string(), second.to_string()]);
        let reply = format!("MEDIA:{first}\n\nJ'ajoute une trentaine de secondes.\n\nMEDIA:{second}\n\nVoilà !\n\nMEDIA:{third}");
        assert_eq!(
            fix_reply(&reply, "vie", &made).unwrap(),
            format!("J'ajoute une trentaine de secondes.\n\nVoilà !\n\nMEDIA:{third}")
        );
        // only the first one written: the last version instead of it
        assert_eq!(fix_reply(&format!("Prêt.\nMEDIA:{first}"), "vie", &made).unwrap(), format!("Prêt.\nMEDIA:{third}"));
        // none written: the last one added, not the others
        assert_eq!(fix_reply("Prêt.", "vie", &made).unwrap(), format!("Prêt.\nMEDIA:{third}"));
        assert_eq!(missing_from("Prêt.", "vie", &made), vec![third.to_string()]);
        // the last one alone already: nothing to change
        assert_eq!(fix_reply(&format!("Prêt.\nMEDIA:{third}"), "vie", &made), None);
    }

    #[test]
    fn distinct_files_of_a_turn_are_all_kept() {
        // two podcasts with their own titles, a page, a document: no version of another
        let mars = "/bipagents/audio/vie/20261030-101503-mars.m4a";
        let venus = "/bipagents/audio/vie/20261030-101722-venus.m4a";
        let made: Vec<String> = vec![mars.into(), venus.into(), PAGE.into()];
        assert!(superseded(&made).is_empty());
        let reply = format!("MEDIA:{mars}\nMEDIA:{venus}\nMEDIA:{PAGE}\nMEDIA:/envoi/devis.pdf");
        assert_eq!(fix_reply(&reply, "vie", &made), None);
        // the same file noted twice is not a version of itself
        assert!(superseded(&[mars.into(), mars.into()]).is_empty());
        assert_eq!(audio_title(mars), Some("mars"));
        assert_eq!(audio_title(PAGE), None);
        assert_eq!(audio_title("/bipagents/audio/vie/chat.m4a"), None);
    }

    #[test]
    fn a_rename_reaches_the_apps_with_its_mark() {
        let first = crate::guide::renamed_mark("bip", "Max");
        let last = crate::guide::renamed_mark("bip", "Léon");
        let reply = "Je m'appelle maintenant Léon.";
        // the last name of the turn only
        assert_eq!(fix_reply(reply, "bip", &[first.clone(), last.clone()]).unwrap(), format!("{reply}\n{last}"));
        assert_eq!(fix_reply(&format!("{reply}\n{last}"), "bip", std::slice::from_ref(&last)), None);
        assert!(is_mark(&last));
        assert_eq!(line_of(&last), last);
    }

    #[test]
    fn nothing_made_nothing_changed() {
        // the person asked for yesterday's podcast again: nothing new made in the turn
        let reply = format!("Le voici.\nMEDIA:{YESTERDAY}");
        assert_eq!(fix_reply(&reply, "vie", &[]), None);
    }

    #[test]
    fn files_by_turn() {
        let made = MadeFiles::default();
        made.note("vie", YESTERDAY, None, 1000.0);
        made.note("vie", TODAY, None, 5000.0);
        made.note("vie", "/bipagents/audio/vie/chat.m4a", Some("run_1"), 5001.0);
        made.note("coach", "/bipagents/audio/coach/x.m4a", None, 5002.0);
        assert_eq!(made.in_routine("vie", 4000.0, 6000.0), vec![TODAY.to_string()]);
        assert_eq!(made.in_run("vie", "run_1"), vec!["/bipagents/audio/vie/chat.m4a".to_string()]);
        assert!(made.in_run("vie", "run_2").is_empty());
        assert!(made.in_routine("lea", 0.0, 1e12).is_empty());
        // from a tool's result: the MEDIA line of the agent's own kind of file
        let result = |text: &str| crate::tools_knowledge::tool_text(text, false);
        made.note_result("vie", &result(&format!("MEDIA:{PAGE}\nPage ready: id budget-novembre.")), Some("run_3"), 6000.0);
        made.note_result("vie", &result("MEDIA:/tmp/x.png\nnot ours"), Some("run_3"), 6000.0);
        made.note_result("vie", &crate::tools_knowledge::tool_text(&format!("MEDIA:{MAP}"), true), Some("run_3"), 6000.0);
        assert_eq!(made.in_run("vie", "run_3"), vec![PAGE.to_string()]);
        assert!(makes_files("create_page") && makes_files("create_podcast") && !makes_files("list_pages"));
    }
}
