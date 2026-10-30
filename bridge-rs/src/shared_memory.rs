//! One common « what my agents know about me » (Alex, 2026-10-29: « Ok pour la mémoire commune »). Each agent keeps
//! its own Hermes memory: `memories/USER.md` is what it knows of its person (entries separated by « \n§\n », about
//! 1375 characters by default, memory_edit.rs). Told « je déménage à Nantes », Budget kept it in its USER.md and Coach
//! never learnt it. The bridge now keeps USER.md common to the person's own agents, without any model call:
//!
//! - **the circle**: the agents this install manages (agents_admin::managed; one set up by hand with its own
//!   `host_home` is left alone, as notebook::seed does), with the Carnet on (notebook::enabled), never an agent other
//!   people talk to (vault::others_facing_agents: the family's agent, project agents, agents shared with relatives or
//!   linked to another household): those neither give nor receive. An agent the person gave their health to
//!   (notebook::health_agent) receives the common entries, but what it learns itself is never copied out (it may be
//!   health data): it stays in its own USER.md only;
//! - **the diff**: for each agent, the entries the bridge last saw in its USER.md (`last`). An entry there now and not
//!   before was added (Hermes' memory tool `add`), one gone was removed (`remove`), a `replace` is both. Entries are
//!   « the same » by their folded text (notebook::fold). An added entry joins the common list, a removed one leaves it
//!   (and is removed from every agent that has it); the other agents get the change under Hermes' own lock
//!   (memory_edit::update_entries), and the bridge's own writes are recorded as seen, so a write never comes back as a
//!   change (no ping-pong). Each agent also keeps the keys it has already been given (`seen`): an entry it dropped
//!   itself, or one left out for the limit, is not given again;
//! - **the first run** merges what every giver already knew, once (deduplicated), without mirroring it: notebook::seed
//!   already copied those memories to the Carnet. An agent joining the circle later (a new agent, the Carnet switched
//!   back on) gets the common list the same way, silently: no news of what was learnt before it took part (a new
//!   agent gets none before its first turn, notebook/news.rs `start`), and what it brings is not news for the others;
//! - **the limit**: when an agent's USER.md would pass its limit (memory_edit::limit, its profile's
//!   `user_char_limit`) less a fifth kept free (`HEADROOM`: Hermes refuses an add past the limit, and the agent would
//!   spend calls making room, its removals then removed everywhere), the oldest common entries are left out of it (the
//!   common list keeps them, and so does the « Toi » page, which has no such limit); entries that are not common (a
//!   health agent's own) are never dropped. An agent that still removes an entry to make room removes it for all: the
//!   « Toi » page keeps it;
//! - **the Carnet**: each entry learnt after the first run is filed on the person's « Toi » page (notebook::note, kind
//!   self: the section guessed by the outline's words, no model), attributed to the agent that learnt it (to the
//!   person for an edit in the app), and queued as « Nouvelles du Carnet » (notebook/news.rs) for the other agents:
//!   Hermes takes a USER.md change only into its next conversation, the news line reaches the ongoing ones. A removal
//!   is not mirrored (the page keeps its history; the person corrects it there);
//! - **« Toi » → USER.md** (the real-day bench, 2026-10-29: told « je déménage à Nantes », GLM and DeepSeek filed it in
//!   the Carnet with notebook_note, never with Hermes' memory tool, so the common memory never saw it): a fact that
//!   appears on the person's « Toi » page (the main part only: the private health part is never read) joins the
//!   common list too, and so every agent's USER.md, under the same limit and headroom. Only the sections of short,
//!   lasting facts about the person count (`TOI_SECTIONS`: « Qui tu es », « Famille », « Travail », « Goûts et
//!   habitudes », « Objectifs », « Ce que tes agents doivent savoir »): not « En bref » (a summary rewritten as facts
//!   change, it would replace entries all the time), not « Autres » (what the bridge could not place: the nightly
//!   pass sorts it, and once sorted it counts), never a paragraph, nor a line over `TOI_FACT_MAX` characters. « New »
//!   means a key not on the page at the last look (`toi`): a fact the bridge itself mirrored to « Toi » is recorded
//!   there when it is mirrored, and a fact already in a common entry (the same folded words, or contained in one: the
//!   Carnet splits « a ; b » into two facts) is not added again, so nothing goes round. An entry removed from USER.md
//!   stays out even though the page keeps it; written on the page again later, it counts again. The first look takes
//!   the page as it is, once (what was removed stays out). Not queued as news: notebook_note already did that;
//! - **one version of a fact** (the night bench, 2026-10-29: with DeepSeek the old version of a fact stayed next to
//!   the new one and « (noté le 29/10/2026) » ended most entries, so USER.md was full and the oldest facts fell out),
//!   still without a model: date and attribution stamps are left out of the key and of what is given
//!   (`strip_stamps`); a fact edited or replaced on « Toi » takes its old version out of every USER.md (`edited`);
//!   a new entry that clearly updates an older one of the same section replaces it (`updates`: 70 % of their
//!   significant words in both, 4 words each at least; a false merge is worse than a duplicate);
//!
//! When: `run_forever` looks every `INTERVAL_SECONDS` at the circle's files (their size and time only, nothing read
//! when nothing changed) and syncs on a change; the app's memory editor (PUT /v1/agents/{id}/memory/user) syncs at
//! once, so an edit of « what it knows about you » applies to all the person's own agents.
//!
//! `<data>/notebook/shared-user.json` (0600): `{"merged", "common": [{text, by, at}], "removed": [[key, at]],
//! "agents": {agent: {"last": [entry], "seen": [key]}}, "toi": [key], "toi_read", "toi_page": [fact]}`.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::config::Config;
use crate::memory_edit::{self, DELIMITER};
use crate::notebook::{self, Page, Part, Who};
use crate::{py, tools_knowledge as tk};

/// How often the circle's files are looked at.
pub const INTERVAL_SECONDS: u64 = 15;
/// Common entries kept at most (the oldest dropped from the list; the « Toi » page keeps them).
pub const COMMON_MAX: usize = 300;
/// When entries are given, `limit / HEADROOM` characters of the file stay free for the agent's own adds.
pub const HEADROOM: usize = 5;
/// Removed keys remembered at most.
pub const REMOVED_MAX: usize = 300;
/// The « Toi » sections whose facts join the common memory (module doc).
pub const TOI_SECTIONS: [&str; 6] = ["about", "family", "work", "habits", "goals", "agents"];
/// A « Toi » line longer than this is not a short fact: it stays on the page only.
pub const TOI_FACT_MAX: usize = 200;
/// `by` of a common entry taken from the « Toi » page.
pub const TOI_BY: &str = "toi";

static LOCK: Mutex<()> = Mutex::new(());
/// One agent's file as looked at: (agent, gives, size, time in nanoseconds).
type Print = (String, bool, u64, u128);
/// One agent's USER.md to change: (agent, entries before, entries after, keys newly given).
pub type Write = (String, Vec<String>, Vec<String>, Vec<String>);
/// The circle's files as last seen: nothing is read while it stays the same.
static FINGERPRINT: Mutex<Option<Vec<Print>>> = Mutex::new(None);

/// One entry of the common list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Common {
    pub text: String,
    /// The agent that learnt it (its id), or « you » for the app's editor.
    pub by: String,
    pub at: f64,
}

/// What the bridge knows of one agent's USER.md.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Seen {
    /// Its entries when the bridge last read or wrote them.
    #[serde(default)]
    pub last: Vec<String>,
    /// The keys it has had or been given (never given again).
    #[serde(default)]
    pub seen: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Stored {
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub common: Vec<Common>,
    #[serde(default)]
    pub removed: Vec<(String, f64)>,
    #[serde(default)]
    pub agents: BTreeMap<String, Seen>,
    /// The keys on the « Toi » page at the last look, and those the bridge mirrored there.
    #[serde(default)]
    pub toi: Vec<String>,
    /// Whether the « Toi » page was looked at once.
    #[serde(default)]
    pub toi_read: bool,
    /// The facts on the « Toi » page at the last look (without stamps), in the page's order (to tell an edited fact:
    /// `edited`).
    #[serde(default)]
    pub toi_page: Vec<String>,
}

/// An agent of the circle.
#[derive(Debug, Clone)]
pub struct Member {
    pub name: String,
    pub folder: PathBuf,
    /// false for a health agent: it receives, never gives.
    pub gives: bool,
    /// Its USER.md's limit, in characters.
    pub limit: usize,
}

/// What one round decided.
#[derive(Debug, Default, PartialEq)]
pub struct Round {
    /// The agents whose USER.md changes: (agent, entries before, entries after, keys newly given).
    pub writes: Vec<Write>,
    /// Entries learnt by an agent after the first run: (agent, entry), for the Carnet.
    pub learnt: Vec<(String, String)>,
}

/// What a date stamp says before its date (« noté le », « annoncé le », « added on »…), folded or not.
const STAMP_VERB: &str = r"(?:not[ée]e?s?|annonc[ée]e?s?|dite?s?|ajout[ée]e?s?|mise? à jour|mise? a jour|maj|enregistr[ée]e?s?|apprise?s?|signal[ée]e?s?|pr[ée]cis[ée]e?s?|confirm[ée]e?s?|indiqu[ée]e?s?|mentionn[ée]e?s?|rapport[ée]e?s?|[ée]crite?s?|communiqu[ée]e?s?|partag[ée]e?s?|noted|added|said|saved|told|mentioned|recorded|announced|updated|shared|reported)";
const STAMP_MONTH: &str = r"(?:janvier|f[ée]vrier|mars|avril|mai|juin|juillet|ao[ûu]t|septembre|octobre|novembre|d[ée]cembre|january|february|march|april|may|june|july|august|september|october|november|december)";

/// A date stamp: « noté le 29/10/2026 », « annoncé le 29 octobre », « added on 2026-10-29 by Coach ».
static STAMP: LazyLock<String> = LazyLock::new(|| {
    let date = format!(
        r"(?:\d{{4}}-\d{{1,2}}-\d{{1,2}}|\d{{1,2}}[/.]\d{{1,2}}(?:[/.]\d{{2,4}})?|\d{{1,2}}(?:er)?\s+{STAMP_MONTH}(?:\s+\d{{4}})?|{STAMP_MONTH}\s+\d{{1,2}}(?:st|nd|rd|th)?(?:,?\s+\d{{4}})?)"
    );
    format!(r"{STAMP_VERB}\s+(?:(?:le|la|du|au|en date du|on|the)\s+)?{date}(?:\s+(?:par|by)\s+[\p{{L}}'’-]+)?")
});

/// The stamps' places in an entry, each with what it leaves (module doc of `strip_stamps`).
static STAMPS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let s = STAMP.as_str();
    [
        // « (noté le 29/10/2026) »
        (format!(r"(?i)\s*\(\s*{s}\s*\)"), ""),
        // « (reporté depuis mars, annoncé le 29/10/2026) »
        (format!(r"(?i)\s*[,;]\s*{s}\s*\)"), ")"),
        // « (annoncé le 29/10/2026, reporté depuis mars) »
        (format!(r"(?i)\(\s*{s}\s*[,;]\s*"), "("),
        // « … — annoncé le 29/10/2026 »
        (format!(r"(?i)\s*[—–]\s*{s}"), ""),
        // « …, noté le 29/10/2026. » at the end
        (format!(r"(?i)\s*(?:[,;]|\s-)\s*{s}\s*\.?\s*$"), ""),
        // « Noté le 29/10/2026 : … » at the start
        (format!(r"(?i)^\s*{s}\s*[:,—–-]\s*"), ""),
    ]
    .into_iter()
    .map(|(pattern, with)| (Regex::new(&pattern).expect("valid regex"), with))
    .collect()
});

/// An entry without the date or attribution stamps a model adds to it (the night bench, 2026-10-29: DeepSeek ended
/// most entries with « (noté le 29/10/2026) », « — annoncé le 29/10/2026 », « (dit le …) »): they say nothing about
/// the person, cost room in every USER.md, and made two copies of one fact look different. A stamp needs its verb
/// (« noté », « annoncé », « dit », « ajouté », « mis à jour »…, `STAMP_VERB`) and a date: « (née le 3 juin) » or
/// « (3 juin) » stay. Neither in the key nor in what is given.
pub fn strip_stamps(entry: &str) -> String {
    let mut text = entry.to_string();
    for (re, with) in STAMPS.iter() {
        text = re.replace_all(&text, *with).into_owned();
    }
    let text = tk::squeeze(&text);
    let text = text.trim().trim_end_matches([',', ';', ':', '—', '–', '-', ' ']).trim_end();
    if text.is_empty() {
        entry.trim().to_string()
    } else {
        text.to_string()
    }
}

/// « The same entry »: its folded text, stamps left out.
pub fn key(entry: &str) -> String {
    notebook::fold(&strip_stamps(entry))
}

/// Words that say nothing of what a fact is about (folded, 3 letters and more: shorter ones are skipped anyway).
const SMALL_WORDS: &[&str] = &[
    "les", "des", "une", "est", "pour", "avec", "dans", "sur", "pas", "que", "qui", "par", "son", "ses", "mes", "mon",
    "aux", "elle", "ils", "elles", "nous", "vous", "leur", "leurs", "sont", "ont", "sans", "tout", "tous", "toute",
    "plus", "tres", "aussi", "mais", "donc", "depuis", "chez", "cette", "ces", "quand", "lors", "the", "and", "for",
    "with", "his", "her", "its", "are", "was", "has", "have", "this", "that", "from", "but", "not", "their", "they",
    "since", "when", "will", "also", "who",
];
/// An entry updates an older one when, of their significant words together, this share at least is in both
/// (`UPDATE_SHARE_NUM / UPDATE_SHARE_DEN`, Jaccard on folded words of 3 letters and more, small words left out)…
pub const UPDATE_SHARE_NUM: usize = 7;
pub const UPDATE_SHARE_DEN: usize = 10;
/// …and each has this many significant words at least (two short facts of the same kind look alike: « Chat :
/// Biscotte » and « Chat : Moka » are two cats).
pub const UPDATE_MIN_WORDS: usize = 4;

/// A fact's significant words (`SMALL_WORDS` and words under 3 letters left out; stamps too).
fn significant(entry: &str) -> HashSet<String> {
    key(entry).split(' ').filter(|w| w.chars().count() >= 3 && !SMALL_WORDS.contains(w)).map(str::to_string).collect()
}

/// The words written with a capital in an entry (names: « Tiago », « Annecy »), folded. Its first word too: « Tiago
/// fait du judo » starts with a name; a plain first word that differs only makes the entries less alike (a duplicate,
/// never a lost fact).
fn names(entry: &str) -> HashSet<String> {
    strip_stamps(entry)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .filter(|w| w.chars().next().is_some_and(char::is_uppercase))
        .map(notebook::fold)
        .collect()
}

/// Whether `new` clearly updates `old` (the night bench: « Déménagement prévu à Annecy en mars 2027, avec son
/// compagnon Ilyès » then « … en mai 2027 (repoussé depuis mars) … »; « À faire garder … (déménagement à Annecy en mars
/// 2027…) » then « … mai 2027 … »): not the same key, the same section of « Toi » as the outline guesses it
/// (template::guess, None for both counts as the same), and at least 70 % of their significant words together in both,
/// with 4 significant words each at least. Conservative on purpose (Alex, 2026-10-29): a false merge loses a fact, a
/// missed one leaves a duplicate. Changed words count against it: « prévu en mars » / « repoussé à mai » alone share too
/// little and stay two entries (the « Toi » page's edit path catches the person's or an agent's correction there); a
/// name that differs (a word with a capital: « Tiago » / « Maëlle ») makes it another fact.
pub fn updates(new: &str, old: &str) -> bool {
    let (a, b) = (significant(new), significant(old));
    if a.len() < UPDATE_MIN_WORDS || b.len() < UPDATE_MIN_WORDS || key(new) == key(old) {
        return false;
    }
    let both = a.intersection(&b).count();
    let any = a.union(&b).count();
    if both * UPDATE_SHARE_DEN < any * UPDATE_SHARE_NUM {
        return false;
    }
    // a name that differs makes it another fact (« Tiago fait du judo le mercredi » / « Maëlle … »): a date, a number
    // or a plain word may change in an update, a person or a place does not
    let named: HashSet<String> = names(new).into_iter().chain(names(old)).collect();
    if a.symmetric_difference(&b).any(|w| named.contains(w)) {
        return false;
    }
    notebook::template::guess("self", &strip_stamps(new)) == notebook::template::guess("self", &strip_stamps(old))
}

/// The common entries `entry` updates go (recorded as removed, so every USER.md drops them). Returns how many.
fn replace_older(stored: &mut Stored, entry: &str, now: f64) -> usize {
    let older: Vec<String> = stored.common.iter().filter(|c| updates(entry, &c.text)).map(|c| key(&c.text)).collect();
    for k in &older {
        drop_common(stored, k, now);
    }
    older.len()
}

/// A key out of the common list, recorded as removed (every USER.md that has it drops it).
fn drop_common(stored: &mut Stored, k: &str, now: f64) {
    stored.common.retain(|c| key(&c.text) != k);
    stored.removed.retain(|(r, _)| r != k);
    stored.removed.push((k.to_string(), now));
}

fn path(config: &Config) -> PathBuf {
    notebook::root(config).join("shared-user.json")
}

fn load(config: &Config) -> Stored {
    std::fs::read_to_string(path(config)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(config: &Config, stored: &Stored) -> std::io::Result<()> {
    let target = path(config);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = target.with_extension("tmp");
    let text = serde_json::to_string(stored).unwrap_or_else(|_| "{}".into());
    py::write_with_mode(&tmp.to_string_lossy(), text.as_bytes(), 0o600)?;
    std::fs::rename(&tmp, &target)
}

/// The circle, in the config's order (module doc).
pub fn circle(config: &Config) -> Vec<Member> {
    let facing = crate::vault::others_facing_agents(config);
    config
        .agents
        .values()
        .filter(|a| a.host_home.as_deref().is_none_or(str::is_empty))
        .filter(|a| crate::agents_admin::managed(config, &a.name).is_ok())
        .filter(|a| !facing.contains(&a.name.to_lowercase()))
        .filter(|a| notebook::enabled(config, &a.name))
        .map(|a| {
            let folder = crate::agents_admin::profile_folder(config, a);
            let limit = memory_edit::limit(&folder, "user");
            Member { name: a.name.clone(), gives: !notebook::health_agent(config, &a.name), limit, folder }
        })
        .filter(|m| m.folder.is_dir())
        .collect()
}

fn chars_of(entries: &[String]) -> usize {
    tk::chars(&entries.join(DELIMITER))
}

/// One round, on the entries read now (`current`: agent -> its USER.md's entries; an agent missing from it is
/// skipped this round). Changes `stored` (the common list, `last` and `seen` as if every write succeeds: `undo`
/// puts an agent back when its write did not happen).
pub fn plan(stored: &mut Stored, members: &[Member], current: &BTreeMap<String, Vec<String>>, now: f64) -> Round {
    let first = !stored.merged;
    let mut round = Round::default();
    let members: Vec<&Member> = members.iter().filter(|m| current.contains_key(&m.name)).collect();
    // agents gone from the circle: forgotten (back later, they are merged again as new)
    let names: HashSet<&str> = members.iter().map(|m| m.name.as_str()).collect();
    let all: HashSet<String> = stored.agents.keys().cloned().collect();
    for name in all {
        if !names.contains(name.as_str()) && !current.contains_key(&name) {
            stored.agents.remove(&name);
        }
    }
    // 1. what each giver changed since the last round
    for m in &members {
        let now_entries = &current[&m.name];
        let now_keys: HashSet<String> = now_entries.iter().map(|e| key(e)).collect();
        let known = stored.agents.get(&m.name).cloned();
        if m.gives {
            match &known {
                Some(seen) => {
                    let last_keys: HashSet<String> = seen.last.iter().map(|e| key(e)).collect();
                    for gone in last_keys.difference(&now_keys) {
                        stored.common.retain(|c| key(&c.text) != *gone);
                        stored.removed.retain(|(k, _)| k != gone);
                        stored.removed.push((gone.clone(), now));
                    }
                    for entry in now_entries {
                        let k = key(entry);
                        if k.is_empty() || last_keys.contains(&k) {
                            continue;
                        }
                        // said again after a removal: it counts again
                        stored.removed.retain(|(r, _)| *r != k);
                        if !stored.common.iter().any(|c| key(&c.text) == k) {
                            let text = strip_stamps(entry);
                            // a clear update of an older entry replaces it (`updates`)
                            replace_older(stored, &text, now);
                            stored.common.push(Common { text: text.clone(), by: m.name.clone(), at: now });
                            if !first {
                                round.learnt.push((m.name.clone(), text));
                                // mirrored to « Toi »: never taken back from there as new
                                if !stored.toi.contains(&k) {
                                    stored.toi.push(k.clone());
                                }
                            }
                        }
                    }
                }
                None => {
                    // the first run, or an agent new to the circle: what it knew joins, once, not mirrored
                    for entry in now_entries {
                        let k = key(entry);
                        if k.is_empty()
                            || stored.removed.iter().any(|(r, _)| *r == k)
                            || stored.common.iter().any(|c| key(&c.text) == k)
                        {
                            continue;
                        }
                        stored.common.push(Common { text: strip_stamps(entry), by: m.name.clone(), at: now });
                    }
                }
            }
        }
        let seen = stored.agents.entry(m.name.clone()).or_default();
        for k in now_keys {
            if !seen.seen.contains(&k) {
                seen.seen.push(k);
            }
        }
    }
    // one entry a key (a list kept before stamps were left out of the key may hold two)
    let mut keys_seen: HashSet<String> = HashSet::new();
    stored.common.retain(|c| keys_seen.insert(key(&c.text)));
    let extra = stored.common.len().saturating_sub(COMMON_MAX);
    stored.common.drain(..extra);
    let extra = stored.removed.len().saturating_sub(REMOVED_MAX);
    stored.removed.drain(..extra);
    // 2. each agent's USER.md brought to the common list
    let rank: BTreeMap<String, usize> = stored.common.iter().enumerate().map(|(i, c)| (key(&c.text), i)).collect();
    for m in &members {
        let before = current[&m.name].clone();
        let seen = stored.agents.get(&m.name).cloned().unwrap_or_default();
        let last_keys: HashSet<String> = seen.last.iter().map(|e| key(e)).collect();
        let mut after: Vec<String> = before
            .iter()
            .filter(|e| {
                let k = key(e);
                // removed elsewhere: gone here too (an entry this agent has just added itself is not touched)
                !(last_keys.contains(&k) && stored.removed.iter().any(|(r, _)| *r == k))
            })
            .cloned()
            .collect();
        // two copies of a common entry (the same once stamps are left out): the first stays
        let mut keys_here: HashSet<String> = HashSet::new();
        after.retain(|e| {
            let k = key(e);
            !rank.contains_key(&k) || keys_here.insert(k)
        });
        let mut given: Vec<String> = Vec::new();
        for c in &stored.common {
            let k = key(&c.text);
            if seen.seen.contains(&k) || after.iter().any(|e| key(e) == k) {
                continue;
            }
            // given without its stamps
            after.push(strip_stamps(&c.text));
            given.push(k);
        }
        // the limit: the oldest common entries left out first, never an entry that is not common; when entries are
        // given, a fifth of the file stays free (Hermes refuses an add past the limit: the agent would spend calls
        // making room, and what it removes to make room would be removed everywhere)
        let room = if given.is_empty() { m.limit } else { m.limit - m.limit / HEADROOM };
        while chars_of(&after) > room {
            let oldest =
                after.iter().enumerate().filter_map(|(i, e)| rank.get(&key(e)).map(|r| (*r, i))).min().map(|(_, i)| i);
            match oldest {
                Some(i) => {
                    after.remove(i);
                }
                None => break,
            }
        }
        let seen = stored.agents.entry(m.name.clone()).or_default();
        for k in &given {
            seen.seen.push(k.clone());
        }
        seen.last = after.clone();
        let keep: HashSet<String> =
            rank.keys().cloned().chain(after.iter().map(|e| key(e))).chain(before.iter().map(|e| key(e))).collect();
        seen.seen.retain(|k| keep.contains(k));
        if after != before {
            round.writes.push((m.name.clone(), before, after, given));
        }
    }
    stored.merged = true;
    round
}

/// The facts of a « Toi » page that may join the common memory (module doc): the list lines of `TOI_SECTIONS`, short.
pub fn toi_facts(page: &Page) -> Vec<String> {
    let mut out = Vec::new();
    for (title, facts) in page.sections() {
        let Some(id) = notebook::template::resolve("self", &title) else { continue };
        if !TOI_SECTIONS.contains(&id) {
            continue;
        }
        for f in facts {
            let text = tk::squeeze(f.text.trim());
            if f.item && !text.is_empty() && tk::chars(&text) <= TOI_FACT_MAX {
                out.push(text);
            }
        }
    }
    out
}

/// Whether `k` is a common entry's key, or its words are inside one.
fn in_common(stored: &Stored, k: &str) -> bool {
    let padded = format!(" {k} ");
    stored.common.iter().any(|c| format!(" {} ", key(&c.text)).contains(&padded))
}

/// The keys gone from the « Toi » page since the last look that were edited or replaced there (notebook_edit's
/// correction, the person's edit in the app, a section rewritten): a new fact took their place (the same kept fact
/// before both, in the page's order) or clearly updates them (`updates`). A fact only taken off the page is not one:
/// the memory keeps it, as before.
pub fn edited(before: &[String], after: &[String]) -> Vec<String> {
    let before_keys: Vec<String> = before.iter().map(|t| key(t)).collect();
    let after_keys: Vec<String> = after.iter().map(|t| key(t)).collect();
    let kept: HashSet<&String> = before_keys.iter().filter(|k| after_keys.contains(k)).collect();
    // each fact not kept (its key, its text), with the kept key just before it (None at the top)
    let slots = |texts: &[String], keys: &[String], other: &[String]| -> Vec<(String, String, Option<String>)> {
        let mut anchor: Option<String> = None;
        let mut out = Vec::new();
        for (text, k) in texts.iter().zip(keys) {
            if kept.contains(k) {
                anchor = Some(k.clone());
            } else if !other.contains(k) {
                out.push((k.clone(), text.clone(), anchor.clone()));
            }
        }
        out
    };
    let gone = slots(before, &before_keys, &after_keys);
    let new = slots(after, &after_keys, &before_keys);
    gone.into_iter()
        .filter(|(_, g, at)| new.iter().any(|(_, n, there)| there == at || updates(n, g)))
        .map(|(k, _, _)| k)
        .collect()
}

/// The facts on « Toi » now: those new since the last look join the common list (not mirrored: they are on the page
/// already), a clear update of an older common entry replacing it; a fact edited or replaced on the page takes its old
/// version out of the common list (`edited`). Returns how many joined.
pub fn take_toi(stored: &mut Stored, facts: &[String], now: f64) -> usize {
    let first = !stored.toi_read;
    // the page now, one fact a key, in its order, without stamps
    let mut page: Vec<(String, String)> = Vec::new();
    for fact in facts {
        let k = key(fact);
        if !k.is_empty() && !page.iter().any(|(p, _)| *p == k) {
            page.push((k, strip_stamps(fact)));
        }
    }
    let keys: Vec<String> = page.iter().map(|(k, _)| k.clone()).collect();
    let texts: Vec<String> = page.iter().map(|(_, t)| t.clone()).collect();
    if !first {
        for gone in edited(&stored.toi_page, &texts) {
            drop_common(stored, &gone, now);
        }
    }
    let mut added = 0;
    for (k, text) in &page {
        if stored.toi.contains(k) {
            continue;
        }
        if first {
            // the page as it is: what was removed from the memory stays out
            if stored.removed.iter().any(|(r, _)| r == k) {
                continue;
            }
        } else {
            // written on the page again after a removal: it counts again
            stored.removed.retain(|(r, _)| r != k);
        }
        if !in_common(stored, k) {
            if !first {
                replace_older(stored, text, now);
            }
            stored.common.push(Common { text: text.clone(), by: TOI_BY.into(), at: now });
            added += 1;
        }
    }
    stored.toi = keys.clone();
    stored.toi_page = texts;
    stored.toi_read = true;
    added
}

/// The person's « Toi » page's facts that may join the common memory (none when there is no page).
fn read_toi(config: &Config) -> Vec<String> {
    let w = notebook::words(&notebook::language(config));
    match notebook::read_page_text(config, Part::Main, &w.self_path()) {
        Some(text) => toi_facts(&Page::parse(&text, w.me)),
        None => Vec::new(),
    }
}

/// An agent whose write did not happen (Hermes wrote in between): as if the bridge had only read it.
pub fn undo(stored: &mut Stored, agent: &str, before: &[String], given: &[String]) {
    if let Some(seen) = stored.agents.get_mut(agent) {
        seen.last = before.to_vec();
        seen.seen.retain(|k| !given.contains(k));
    }
}

/// One round now: the circle read, the changes written under Hermes' lock, the new entries filed on « Toi ».
/// `edited`: the agent whose USER.md the person just edited in the app (its new entries are the person's). Best
/// effort: an error is logged. Returns how many agents' files were written.
pub fn sync(config: &Config, now: f64, edited: Option<&str>) -> usize {
    let (written, learnt) = {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let members = circle(config);
        if members.is_empty() {
            return 0;
        }
        let mut current = BTreeMap::new();
        for m in &members {
            if let Some(entries) = memory_edit::read_locked(&m.folder, "user") {
                current.insert(m.name.clone(), entries);
            }
        }
        let mut stored = load(config);
        take_toi(&mut stored, &read_toi(config), now);
        let round = plan(&mut stored, &members, &current, now);
        let mut written = 0;
        for (agent, before, after, given) in &round.writes {
            let Some(m) = members.iter().find(|m| &m.name == agent) else { continue };
            let done = memory_edit::update_entries(&m.folder, "user", |on_disk| {
                // Hermes wrote since the read: left for the next round, which sees both
                if on_disk != before.as_slice() {
                    None
                } else {
                    Some(after.clone())
                }
            });
            match done {
                Ok(true) => written += 1,
                Ok(false) => undo(&mut stored, agent, before, given),
                Err(err) => {
                    tracing::warn!(agent = %agent, error = %err, "shared memory not written");
                    undo(&mut stored, agent, before, given);
                }
            }
        }
        if let Err(err) = save(config, &stored) {
            tracing::warn!(error = %err, "shared memory state not saved");
            return written;
        }
        (written, round.learnt)
    };
    for (agent, entry) in learnt {
        mirror(config, &agent, &entry, edited == Some(agent.as_str()), now);
    }
    if written > 0 {
        tracing::info!(agents = written, "shared memory synced");
    }
    written
}

/// An entry learnt by `agent`, on the person's « Toi » page and in the other agents' news.
fn mirror(config: &Config, agent: &str, entry: &str, by_person: bool, now: f64) {
    let who = if by_person { Who::you(config) } else { Who::agent(config, agent) };
    let me = notebook::words(&notebook::language(config)).me;
    let fact = tk::squeeze(&entry.replace('\n', " "));
    match notebook::note(config, Part::Main, "self", me, "", &[fact], &[], "", "", &who) {
        Ok(written) if !written.added.is_empty() => {
            notebook::news::queue(
                config,
                agent,
                Part::Main,
                &written.path,
                &written.title,
                "",
                &written.added.join(" ; "),
                now,
            );
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(agent = %agent, error = %err, "shared memory entry not filed in the Carnet"),
    }
}

/// A file's size and time in nanoseconds (0 when missing).
fn stamp(file: &std::path::Path) -> (u64, u128) {
    let meta = std::fs::metadata(file).ok();
    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let time = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    (size, time)
}

/// The circle's files and the « Toi » page, by size and time (no read).
fn fingerprint(config: &Config) -> Vec<Print> {
    let mut print: Vec<Print> = circle(config)
        .into_iter()
        .map(|m| {
            let (size, time) = stamp(&m.folder.join("memories").join("USER.md"));
            (m.name, m.gives, size, time)
        })
        .collect();
    if !print.is_empty() {
        let w = notebook::words(&notebook::language(config));
        let (size, time) = stamp(&notebook::part_root(config, Part::Main).join(w.self_path()));
        print.push((String::new(), false, size, time));
    }
    print
}

/// A round when a file of the circle (or the circle itself) changed since the last look.
pub fn sync_if_changed(config: &Config) -> usize {
    let print = fingerprint(config);
    {
        let last = FINGERPRINT.lock().unwrap_or_else(|e| e.into_inner());
        if last.as_ref() == Some(&print) {
            return 0;
        }
    }
    let written = sync(config, py::time_time(), None);
    *FINGERPRINT.lock().unwrap_or_else(|e| e.into_inner()) = Some(fingerprint(config));
    written
}

/// The background look (app.rs start_background).
pub async fn run_forever(state: AppState) {
    tokio::time::sleep(Duration::from_secs(20)).await;
    loop {
        let config = state.config();
        if let Err(err) = tokio::task::spawn_blocking(move || sync_if_changed(&config)).await {
            tracing::warn!(error = %err, "shared memory round failed");
        }
        tokio::time::sleep(Duration::from_secs(INTERVAL_SECONDS)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentConfig;

    fn member(name: &str, gives: bool, limit: usize) -> Member {
        Member { name: name.into(), folder: PathBuf::new(), gives, limit }
    }

    fn files(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        pairs.iter().map(|(n, e)| (n.to_string(), e.iter().map(|s| s.to_string()).collect())).collect()
    }

    /// The writes applied to `current`, as the files would be.
    fn apply(current: &mut BTreeMap<String, Vec<String>>, round: &Round) {
        for (agent, _, after, _) in &round.writes {
            current.insert(agent.clone(), after.clone());
        }
    }

    #[test]
    fn the_first_run_merges_once_without_mirroring() {
        let members = [member("budget", true, 1375), member("coach", true, 1375)];
        let mut stored = Stored::default();
        let mut now = files(&[("budget", &["Alex habite Lyon.", "Alex a un chat."]), ("coach", &["alex a un chat"])]);
        let round = plan(&mut stored, &members, &now, 1.0);
        assert!(round.learnt.is_empty(), "nothing filed at the merge");
        assert_eq!(stored.common.len(), 2, "deduplicated by folded text: {:?}", stored.common);
        apply(&mut now, &round);
        assert_eq!(now["coach"], ["alex a un chat", "Alex habite Lyon."]);
        assert_eq!(now["budget"], ["Alex habite Lyon.", "Alex a un chat."]);
        // nothing changed: nothing written (no ping-pong)
        let again = plan(&mut stored, &members, &now, 2.0);
        assert!(again.writes.is_empty() && again.learnt.is_empty(), "{again:?}");
    }

    #[test]
    fn an_add_a_replace_and_a_remove_go_to_the_others() {
        let members = [member("budget", true, 1375), member("coach", true, 1375), member("prof", true, 1375)];
        let mut stored = Stored::default();
        let mut now = files(&[("budget", &["Alex habite Lyon."]), ("coach", &[]), ("prof", &[])]);
        let r = plan(&mut stored, &members, &now, 1.0);
        apply(&mut now, &r);
        // add (Budget's memory tool)
        now.get_mut("budget").unwrap().push("Alex déménage à Nantes en janvier.".into());
        let r = plan(&mut stored, &members, &now, 2.0);
        assert_eq!(r.learnt, [("budget".to_string(), "Alex déménage à Nantes en janvier.".to_string())]);
        apply(&mut now, &r);
        assert_eq!(now["coach"], ["Alex habite Lyon.", "Alex déménage à Nantes en janvier."]);
        assert!(plan(&mut stored, &members, &now, 3.0).writes.is_empty(), "the bridge's writes are not changes");
        // replace (Coach's memory tool: old out, new in)
        let coach = now.get_mut("coach").unwrap();
        coach[0] = "Alex habite Nantes depuis janvier.".into();
        let r = plan(&mut stored, &members, &now, 4.0);
        assert_eq!(r.learnt, [("coach".to_string(), "Alex habite Nantes depuis janvier.".to_string())]);
        apply(&mut now, &r);
        for agent in ["budget", "prof"] {
            assert_eq!(now[agent], ["Alex déménage à Nantes en janvier.", "Alex habite Nantes depuis janvier."]);
        }
        // remove (Prof)
        now.get_mut("prof").unwrap().remove(0);
        let r = plan(&mut stored, &members, &now, 5.0);
        assert!(r.learnt.is_empty());
        apply(&mut now, &r);
        for agent in ["budget", "coach", "prof"] {
            assert_eq!(now[agent], ["Alex habite Nantes depuis janvier."], "{agent}");
        }
        assert!(plan(&mut stored, &members, &now, 6.0).writes.is_empty());
        // said again later: it counts again
        now.get_mut("coach").unwrap().push("Alex déménage à Nantes en janvier.".into());
        let r = plan(&mut stored, &members, &now, 7.0);
        assert_eq!(r.learnt.len(), 1);
        apply(&mut now, &r);
        assert_eq!(now["prof"].len(), 2);
    }

    #[test]
    fn a_health_agent_receives_but_never_gives() {
        let members = [member("budget", true, 1375), member("sante", false, 1375)];
        let mut stored = Stored::default();
        let mut now = files(&[("budget", &["Alex a 41 ans."]), ("sante", &["Alex prend un traitement."])]);
        let r = plan(&mut stored, &members, &now, 1.0);
        apply(&mut now, &r);
        assert_eq!(now["budget"], ["Alex a 41 ans."]);
        assert_eq!(now["sante"], ["Alex prend un traitement.", "Alex a 41 ans."]);
        now.get_mut("sante").unwrap().push("Alex est allergique.".into());
        now.get_mut("sante").unwrap().retain(|e| e != "Alex a 41 ans.");
        let r = plan(&mut stored, &members, &now, 2.0);
        assert!(r.learnt.is_empty() && r.writes.is_empty(), "{r:?}");
        assert!(stored.common.iter().all(|c| c.by == "budget"));
        // and what it dropped is not given back
        assert!(plan(&mut stored, &members, &now, 3.0).writes.is_empty());
    }

    #[test]
    fn the_limit_keeps_the_newest_and_does_not_give_back() {
        let members = [member("budget", true, 1375), member("coach", true, 40)];
        let mut stored = Stored::default();
        let mut now = files(&[("budget", &[]), ("coach", &[])]);
        let r = plan(&mut stored, &members, &now, 1.0);
        apply(&mut now, &r);
        for (i, fact) in ["Premier fait ancien.", "Deuxième fait.", "Troisième fait."].iter().enumerate() {
            now.get_mut("budget").unwrap().push(fact.to_string());
            let r = plan(&mut stored, &members, &now, 2.0 + i as f64);
            apply(&mut now, &r);
        }
        assert!(chars_of(&now["coach"]) <= 32, "{:?}", now["coach"]);
        assert_eq!(now["coach"], ["Deuxième fait.", "Troisième fait."]);
        assert_eq!(stored.common.len(), 3, "the common list keeps them all");
        assert!(plan(&mut stored, &members, &now, 9.0).writes.is_empty(), "the left-out one is not given again");
        // nor taken for a removal
        assert_eq!(now["budget"].len(), 3);
    }

    #[test]
    fn a_write_that_did_not_happen_is_given_again() {
        let members = [member("budget", true, 1375), member("coach", true, 1375)];
        let mut stored = Stored::default();
        let mut now = files(&[("budget", &["A."]), ("coach", &[])]);
        let r = plan(&mut stored, &members, &now, 1.0);
        let (agent, before, _, given) = &r.writes[0];
        undo(&mut stored, agent, before, given);
        // Hermes added to coach meanwhile
        now.get_mut("coach").unwrap().push("B.".into());
        let r = plan(&mut stored, &members, &now, 2.0);
        apply(&mut now, &r);
        assert_eq!(now["coach"], ["B.", "A."]);
        assert_eq!(now["budget"], ["A.", "B."]);
    }

    fn toi(stored: &mut Stored, facts: &[&str], now: f64) -> usize {
        take_toi(stored, &facts.iter().map(|f| f.to_string()).collect::<Vec<_>>(), now)
    }

    #[test]
    fn a_fact_on_toi_joins_the_common_memory_once() {
        let members = [member("budget", true, 1375), member("coach", true, 1375)];
        let mut stored = Stored::default();
        let mut now = files(&[("budget", &[]), ("coach", &[])]);
        assert_eq!(toi(&mut stored, &["Travaille de nuit"], 1.0), 1, "the first look takes the page");
        let r = plan(&mut stored, &members, &now, 1.0);
        apply(&mut now, &r);
        assert!(r.learnt.is_empty(), "not mirrored back to « Toi »");
        // an agent files a move on « Toi »: every USER.md gets it, nothing goes back to the page
        assert_eq!(toi(&mut stored, &["Travaille de nuit", "Déménage à Nantes en janvier"], 2.0), 1);
        let r = plan(&mut stored, &members, &now, 2.0);
        assert!(r.learnt.is_empty(), "{r:?}");
        apply(&mut now, &r);
        for agent in ["budget", "coach"] {
            assert_eq!(now[agent], ["Travaille de nuit", "Déménage à Nantes en janvier"], "{agent}");
        }
        assert_eq!(toi(&mut stored, &["Travaille de nuit", "Déménage à Nantes en janvier"], 3.0), 0);
        assert!(plan(&mut stored, &members, &now, 3.0).writes.is_empty(), "no ping-pong");
        // an entry an agent learns is mirrored to « Toi » (split by the Carnet): it does not come back as new
        now.get_mut("coach").unwrap().push("Alex court le dimanche ; Alex aime le jazz".into());
        let r = plan(&mut stored, &members, &now, 4.0);
        assert_eq!(r.learnt.len(), 1);
        apply(&mut now, &r);
        let page = ["Travaille de nuit", "Déménage à Nantes en janvier", "Alex court le dimanche", "Alex aime le jazz"];
        assert_eq!(toi(&mut stored, &page, 5.0), 0);
        assert!(plan(&mut stored, &members, &now, 5.0).writes.is_empty());
        // removed from the memory: stays out although the page keeps it
        now.get_mut("budget").unwrap().retain(|e| e != "Travaille de nuit");
        let r = plan(&mut stored, &members, &now, 6.0);
        apply(&mut now, &r);
        assert!(now.values().all(|e| !e.contains(&"Travaille de nuit".to_string())));
        assert_eq!(toi(&mut stored, &page, 7.0), 0);
        assert!(plan(&mut stored, &members, &now, 7.0).writes.is_empty());
        // taken off the page and written there again: it counts again
        toi(&mut stored, &page[1..], 8.0);
        assert_eq!(toi(&mut stored, &page, 9.0), 1);
        let r = plan(&mut stored, &members, &now, 9.0);
        apply(&mut now, &r);
        assert!(now["coach"].contains(&"Travaille de nuit".to_string()));
    }

    #[test]
    fn only_short_facts_of_the_lasting_sections_count() {
        let page = Page::parse(
            "---\ntype: self\n---\n# Toi\n\n## En bref\nAlex, 41 ans, Lyon.\n\n## Qui tu es\n- Habite Lyon — *Budget, \
             2026-10-29*\n\n## Travail\nUn paragraphe.\n- Travaille de nuit\n\n## Ce que tes agents doivent savoir\n\
             - Tutoie-moi\n\n## Autres\n- Pas encore rangé\n",
            "Toi",
        );
        let facts = toi_facts(&page);
        assert_eq!(facts, ["Habite Lyon", "Travaille de nuit", "Tutoie-moi"]);
        let long = format!("---\ntype: self\n---\n# Toi\n\n## Objectifs\n- {}\n", "mot ".repeat(60));
        assert!(toi_facts(&Page::parse(&long, "Toi")).is_empty(), "a long line stays on the page");
    }

    #[test]
    fn stamps_are_left_out_of_the_key_and_of_what_is_given() {
        // the night bench's DeepSeek entries (2026-10-29)
        let run = "Court le dimanche matin, en général 10 km";
        let part = "Part de Villeurbanne pour Annecy en mai 2027 (reporté depuis mars), avec son compagnon Ilyès";
        let cases = [
            ("Court le dimanche matin, en général 10 km (noté le 29/10/2026)", run),
            ("Objectif : semi-marathon de Lyon, avril 2027 (annoncé le 29/10/2026)", "Objectif : semi-marathon de Lyon, avril 2027"),
            ("Se fait appeler Cam par tout le monde (dit le 29/10/2026)", "Se fait appeler Cam par tout le monde"),
            (
                "Part de Villeurbanne pour Annecy en mai 2027 (reporté depuis mars), avec son compagnon Ilyès — annoncé le \
                 29/10/2026",
                part,
            ),
            (
                "Part de Villeurbanne pour Annecy en mai 2027 (reporté depuis mars, annoncé le 29/10/2026), avec son \
                 compagnon Ilyès",
                part,
            ),
            ("Noté le 29 octobre 2026 : végétarienne.", "végétarienne."),
            ("Runs on Sundays, added on 2026-10-29 by Coach", "Runs on Sundays"),
            // not stamps: a date that is the fact itself
            ("Deux enfants : Maëlle (née le 3 juin) et Tiago (9 ans)", "Deux enfants : Maëlle (née le 3 juin) et Tiago (9 ans)"),
            ("Mère : Odile, anniversaire le 17 novembre", "Mère : Odile, anniversaire le 17 novembre"),
            ("Mariage (12/06/2015)", "Mariage (12/06/2015)"),
            ("Yoga le mardi soir — pas de course ce soir-là", "Yoga le mardi soir — pas de course ce soir-là"),
        ];
        for (entry, want) in cases {
            assert_eq!(strip_stamps(entry), want, "{entry}");
        }
        assert_eq!(key(cases[3].0), key(cases[4].0), "one fact, two stamps");
        // given without the stamp, not given back to the agent that wrote it with one
        let members = [member("budget", true, 1375), member("coach", true, 1375)];
        let mut stored = Stored::default();
        let mut now = files(&[("budget", &[]), ("coach", &[])]);
        let r = plan(&mut stored, &members, &now, 1.0);
        apply(&mut now, &r);
        now.get_mut("coach").unwrap().push(cases[0].0.into());
        let r = plan(&mut stored, &members, &now, 2.0);
        assert_eq!(r.learnt, [("coach".to_string(), run.to_string())], "filed on « Toi » without it");
        apply(&mut now, &r);
        assert_eq!(now["budget"], [run]);
        assert_eq!(now["coach"], [cases[0].0], "the agent's own line is not rewritten");
        // the same fact written again with another stamp: nothing new, the second copy goes
        now.get_mut("budget").unwrap().push(format!("{run} (précisé le 30/10/2026)"));
        let r = plan(&mut stored, &members, &now, 3.0);
        assert!(r.learnt.is_empty(), "{r:?}");
        apply(&mut now, &r);
        assert_eq!(now["budget"], [run]);
        assert!(plan(&mut stored, &members, &now, 4.0).writes.is_empty(), "no ping-pong");
    }

    #[test]
    fn a_clear_update_replaces_the_older_entry() {
        // the night bench's pairs (GLM at 2000, DeepSeek at 1375)
        let moved = "Déménagement prévu à Annecy en mars 2027, avec son compagnon Ilyès";
        let moved_again = "Déménagement prévu à Annecy en mai 2027 (repoussé depuis mars), avec son compagnon Ilyès";
        let cat =
            "À faire garder quand ils partent (déménagement à Annecy en mars 2027, voyages) — à prévoir à l'avance";
        let cat_again =
            "À faire garder quand ils partent (déménagement à Annecy en mai 2027, voyages) — à prévoir à l'avance";
        assert!(updates(moved_again, moved));
        assert!(updates(cat_again, cat));
        // conservative: different facts that look alike stay two
        for (a, b) in [
            ("Chat : Biscotte", "Chat : Moka"),
            ("Enfant : Maëlle, 6 ans", "Enfant : Tiago, 9 ans"),
            ("Court le dimanche matin", "Court le mardi soir"),
            ("Se lève à 6 h 15 en semaine", "Se lève à 9 h le week-end"),
            ("Rendez-vous chez le dentiste le 12 novembre", "Rendez-vous chez le dentiste le 3 décembre"),
            ("Tiago fait du judo le mercredi après-midi", "Maëlle fait du judo le mercredi après-midi"),
            (
                "Tiago fait du judo le mercredi après-midi au gymnase de Villeurbanne",
                "Maëlle fait du judo le mercredi après-midi au gymnase de Villeurbanne",
            ),
            (moved_again, "Chat : Biscotte, à faire garder lors du déménagement à Annecy (mars 2027)"),
            (moved_again, cat),
            // too few shared words without a model: a duplicate is better than a lost fact
            ("Déménagement repoussé à mai 2027", "Déménagement prévu en mars 2027"),
        ] {
            assert!(!updates(a, b) && !updates(b, a), "{a} / {b}");
        }
        let members = [member("budget", true, 1375), member("coach", true, 1375)];
        let mut stored = Stored::default();
        let mut now = files(&[("budget", &[moved, cat]), ("coach", &[])]);
        let r = plan(&mut stored, &members, &now, 1.0);
        apply(&mut now, &r);
        assert_eq!(now["coach"], [moved, cat]);
        // DeepSeek adds the new versions without removing the old ones
        now.get_mut("budget").unwrap().extend([moved_again.to_string(), cat_again.to_string()]);
        let r = plan(&mut stored, &members, &now, 2.0);
        assert_eq!(r.learnt.len(), 2);
        apply(&mut now, &r);
        for agent in ["budget", "coach"] {
            assert_eq!(now[agent], [moved_again, cat_again], "{agent}");
        }
        assert_eq!(stored.common.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), [moved_again, cat_again]);
        assert!(plan(&mut stored, &members, &now, 3.0).writes.is_empty(), "no ping-pong");
    }

    #[test]
    fn a_fact_edited_on_toi_takes_its_old_version_out() {
        let members = [member("budget", true, 1375), member("coach", true, 1375)];
        let mut stored = Stored::default();
        let mut now = files(&[("budget", &[]), ("coach", &[])]);
        let page = ["Habite à Villeurbanne", "Déménagement prévu en mars 2027", "Chat : Biscotte"];
        toi(&mut stored, &page, 1.0);
        let r = plan(&mut stored, &members, &now, 1.0);
        apply(&mut now, &r);
        assert_eq!(now["coach"], page);
        // corrected in place (notebook_edit, or the person in the app): too different for `updates`, same place
        let edited_page = ["Habite à Villeurbanne", "Déménagement repoussé à mai 2027", "Chat : Biscotte"];
        assert_eq!(toi(&mut stored, &edited_page, 2.0), 1);
        let r = plan(&mut stored, &members, &now, 2.0);
        apply(&mut now, &r);
        for agent in ["budget", "coach"] {
            assert_eq!(now[agent], ["Habite à Villeurbanne", "Chat : Biscotte", "Déménagement repoussé à mai 2027"]);
        }
        assert!(plan(&mut stored, &members, &now, 3.0).writes.is_empty());
        // only taken off the page: the memory keeps it (as before)
        assert_eq!(toi(&mut stored, &["Habite à Villeurbanne", "Déménagement repoussé à mai 2027"], 4.0), 0);
        assert!(plan(&mut stored, &members, &now, 4.0).writes.is_empty());
        assert!(now["coach"].contains(&"Chat : Biscotte".to_string()));
        // an agent's entry mirrored to « Toi », then corrected there: the old one leaves every USER.md
        now.get_mut("coach").unwrap().push("Travaille dans un atelier de lutherie (noté le 29/10/2026)".into());
        let r = plan(&mut stored, &members, &now, 5.0);
        apply(&mut now, &r);
        let mirrored =
            ["Habite à Villeurbanne", "Déménagement repoussé à mai 2027", "Travaille dans un atelier de lutherie"];
        assert_eq!(toi(&mut stored, &mirrored, 6.0), 0);
        let corrected = ["Habite à Villeurbanne", "Déménagement repoussé à mai 2027", "Luthière à son compte"];
        assert_eq!(toi(&mut stored, &corrected, 7.0), 1);
        let r = plan(&mut stored, &members, &now, 7.0);
        apply(&mut now, &r);
        for agent in ["budget", "coach"] {
            assert!(now[agent].iter().all(|e| !e.contains("atelier")), "{agent}: {:?}", now[agent]);
            assert!(now[agent].contains(&"Luthière à son compte".to_string()), "{agent}");
        }
        assert!(edited(&["A".into(), "B".into()], &["A".into()]).is_empty(), "a removal is no edit");
    }

    fn place() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k".repeat(40));
        config.outbox.db_path = dir.path().join("data/outbox.db").to_string_lossy().into_owned();
        config.hermes.bin = "hermes".into();
        config.hermes.hub_url = "http://127.0.0.1:9".into();
        for (name, shown) in [("quotidien", "Quotidien"), ("budget", "Budget"), ("coach", "Coach"), ("sante", "Santé")]
        {
            let space = dir.path().join(format!("spaces/{name}"));
            std::fs::create_dir_all(space.join(format!("profiles/{name}/memories"))).unwrap();
            config.agents.insert(
                name.into(),
                AgentConfig {
                    name: name.into(),
                    display_name: shown.into(),
                    space: Some(space.to_string_lossy().into_owned()),
                    ..Default::default()
                },
            );
        }
        (dir, config)
    }

    fn user_md(config: &Config, agent: &str) -> PathBuf {
        let a = config.agent(Some(agent)).unwrap();
        crate::agents_admin::profile_folder(config, a).join("memories/USER.md")
    }

    #[test]
    fn the_circle_follows_the_rules() {
        let (dir, mut config) = place();
        let names = |c: &Config| circle(c).iter().map(|m| (m.name.clone(), m.gives)).collect::<Vec<_>>();
        let all = names(&config);
        assert_eq!(all.len(), 4, "{all:?}");
        crate::health::set_enabled(&config, "sante", true).unwrap();
        assert!(names(&config).contains(&("sante".to_string(), false)));
        notebook::set_enabled(&config, "coach", false).unwrap();
        assert!(!names(&config).iter().any(|(n, _)| n == "coach"), "Carnet off");
        // the family's agent: out
        let data = PathBuf::from(config.data_dir());
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("family-agent.json"), r#"{"agent": "quotidien", "created": 1.0, "lang": "fr"}"#)
            .unwrap();
        assert!(!names(&config).iter().any(|(n, _)| n == "quotidien"), "others-facing");
        // set up by hand with its own HERMES_HOME: out
        config.agents.get_mut("budget").unwrap().host_home = Some(dir.path().to_string_lossy().into_owned());
        assert!(!names(&config).iter().any(|(n, _)| n == "budget"), "host_home");
    }

    #[test]
    fn a_new_fact_reaches_the_other_agents_and_the_toi_page() {
        let (_dir, config) = place();
        std::fs::write(user_md(&config, "budget"), "Alex habite Lyon.").unwrap();
        assert_eq!(sync(&config, 1.0, None), 3);
        assert_eq!(std::fs::read_to_string(user_md(&config, "coach")).unwrap(), "Alex habite Lyon.");
        assert_eq!(sync(&config, 2.0, None), 0, "no ping-pong");
        // Budget's memory tool adds an entry
        std::fs::write(user_md(&config, "budget"), "Alex habite Lyon.\n§\nAlex déménage à Nantes en janvier.").unwrap();
        assert_eq!(sync(&config, 3.0, None), 3);
        assert_eq!(
            std::fs::read_to_string(user_md(&config, "coach")).unwrap(),
            "Alex habite Lyon.\n§\nAlex déménage à Nantes en janvier."
        );
        let w = notebook::words(&notebook::language(&config));
        let toi = notebook::read_page_text(&config, Part::Main, &w.self_path()).unwrap();
        assert!(toi.contains("Nantes") && !toi.contains("Lyon"), "only what was learnt after the merge: {toi}");
        assert!(toi.contains("Budget"), "attributed: {toi}");
        let news = notebook::news::take(&config, "coach", 4.0).unwrap();
        assert!(news.contains("Nantes"), "{news}");
        assert!(notebook::news::take(&config, "budget", 4.0).is_none(), "not to the one that learnt it");
        assert_eq!(sync(&config, 5.0, None), 0, "the mirrored entry does not come back from « Toi »");
    }

    #[test]
    fn a_new_agent_gets_the_common_memory_silently() {
        let (dir, mut config) = place();
        std::fs::write(user_md(&config, "budget"), "Alex habite Lyon.").unwrap();
        sync(&config, 1.0, None);
        std::fs::write(user_md(&config, "budget"), "Alex habite Lyon.\n§\nAlex a dit « je t'aime » à Budget.").unwrap();
        sync(&config, 2.0, None);
        assert!(notebook::news::pending(&config, "coach", 2.0) > 0, "learnt after the others existed: news");
        // a team agent is made later (agents_admin::create_agent marks it before its profile exists)
        let space = dir.path().join("spaces/qa");
        std::fs::create_dir_all(space.join("profiles/qa/memories")).unwrap();
        notebook::news::start(&config, "qa", 3.0);
        config.agents.insert(
            "qa".into(),
            AgentConfig {
                name: "qa".into(),
                display_name: "QA Testeur".into(),
                space: Some(space.to_string_lossy().into_owned()),
                ..Default::default()
            },
        );
        for agent in ["quotidien", "coach", "sante"] {
            notebook::news::take(&config, agent, 3.5);
        }
        sync(&config, 4.0, None);
        // the whole common list in its USER.md…
        assert_eq!(
            std::fs::read_to_string(user_md(&config, "qa")).unwrap(),
            "Alex habite Lyon.\n§\nAlex a dit « je t'aime » à Budget."
        );
        // …and no backlog as news at its first turn, nothing new for the others either
        assert_eq!(notebook::news::take(&config, "qa", 5.0), None);
        for agent in ["quotidien", "budget", "coach", "sante"] {
            assert_eq!(notebook::news::take(&config, agent, 5.0), None, "{agent}");
        }
        assert_eq!(sync(&config, 6.0, None), 0);
    }

    #[test]
    fn a_fact_filed_on_toi_reaches_every_user_md() {
        let (_dir, config) = place();
        assert_eq!(sync(&config, 1.0, None), 0);
        // what the real-day bench's agents do: notebook_note, not the memory tool
        let who = Who::agent(&config, "budget");
        let fact = vec!["Déménage à Nantes en janvier".to_string()];
        notebook::note(&config, Part::Main, "self", "Toi", "", &fact, &[], "", "", &who).unwrap();
        assert_eq!(sync(&config, 2.0, None), 4);
        for agent in ["quotidien", "budget", "coach", "sante"] {
            assert_eq!(std::fs::read_to_string(user_md(&config, agent)).unwrap(), "Déménage à Nantes en janvier");
        }
        assert_eq!(sync(&config, 3.0, None), 0, "no ping-pong");
        // the private health part is never read
        crate::health::set_enabled(&config, "sante", true).unwrap();
        let who = Who::agent(&config, "sante");
        notebook::note(
            &config,
            Part::Health,
            "self",
            "Toi",
            "",
            &["Allergique aux noix".to_string()],
            &[],
            "",
            "",
            &who,
        )
        .unwrap();
        sync(&config, 4.0, None);
        assert!(!std::fs::read_to_string(user_md(&config, "coach")).unwrap().contains("noix"));
    }

    #[test]
    fn the_app_s_editor_applies_to_all() {
        let (_dir, config) = place();
        assert_eq!(sync(&config, 1.0, None), 0);
        let folder = crate::agents_admin::profile_folder(&config, config.agent(Some("coach")).unwrap());
        memory_edit::write_entries(&folder, "user", &["Alex préfère le tutoiement.".to_string()]).unwrap();
        sync(&config, 2.0, Some("coach"));
        for agent in ["quotidien", "budget", "sante"] {
            assert_eq!(std::fs::read_to_string(user_md(&config, agent)).unwrap(), "Alex préfère le tutoiement.");
        }
        memory_edit::write_entries(&folder, "user", &[]).unwrap();
        sync(&config, 3.0, Some("coach"));
        assert_eq!(std::fs::read_to_string(user_md(&config, "budget")).unwrap(), "");
    }
}
