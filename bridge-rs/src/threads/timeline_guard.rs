//! The guard of a conversation's pages: reading a conversation never fails, whatever the cursor and whatever state the
//! conversation is in. On 7/11 a catch-up with nothing new (`after=` the thread's last item) while a turn ran was a 500:
//! thin-apps-7's `layout.open` indexed the page's first item on an empty page (fixed in `open_on_page`). The iPhone
//! catches up that way on every hint; it lost the secret card placed with no item and the « Arrêter » of the answer
//! going on (release journeys J13, J19). Neither the unit tests nor the black-box suite had asked for that page.
//!
//! Here small threads, every sequence of a few item kinds and every window of longer conversations, and a few
//! threads at the bridge's own page sizes, are read every way a request can say (`Want`: the latest page, `before=`,
//! `after=`, `around=`, sizes chosen by the bridge or given), with a turn going on or not, through the route's own
//! `conversation_page` and `Page::answer`. Each page must come without a panic, and well-formed (`check`). The
//! expectations are worked out from the thread's letters and the request, never from the page itself.
//! `bridge/tests_blackbox/test_timeline_every_read.py` does the same over HTTP with real conversations.

use std::collections::HashSet;
use std::ops::Range;
use std::panic::{catch_unwind, AssertUnwindSafe};

use serde_json::{json, Value};

use super::conversation_page;
use super::layout;
use super::timeline::{self, cursor_of, cursor_pos, Item, Want, CATCH_UP_PAGE, LATEST_PAGE, OLDER_PAGE};
use super::LIMIT_MAX;

/// The item kinds the exhaustive part combines: a person's message starting a turn (`P`), the agent's reasoning (`R`),
/// a tool (`T`), the agent's reply (`Y`), a card's tool (`C`, the secret card's call).
const KINDS: &str = "PRTYC";
/// Every sequence of `KINDS` up to this length is a thread (each also with a message being sent at its end).
const EXHAUSTIVE: usize = 4;
/// The longest small thread.
const SMALL: usize = 8;
/// Longer conversations, several turns each; every window of up to `SMALL` items is a thread too. Besides `KINDS`:
/// a message added to the answer in progress (`A`), another agent's (`E`), Hermes' line closing a stopped turn (`I`),
/// a Boîte report (`B`), an answered approval (`V`), a message that draws nothing (`N`), a batch of calls (`G`), a
/// message to another agent (`M`), a message being sent and not stored yet (`S`, only at the end, as the bridge puts
/// them).
const CONVERSATIONS: [&str; 6] = [
    "PYPRTYPTTYAYPCY",
    "PRTCYPAYTYPYSS",
    "EYPTVYBPYNPRY",
    "NRTYPPYITPYPGTY",
    "PMTYEYPYBPTCYS",
    "BPYEYVPTTTYPRRY",
];

/// A made-up thread: one item per letter, in reading order, over two segments and several days.
fn thread(letters: &str) -> Vec<Item> {
    let now = chrono::Utc::now().timestamp() as f64;
    let n = letters.chars().count();
    letters
        .chars()
        .enumerate()
        .map(|(k, letter)| {
            let tool = |name: &str, arguments: &str, result: &str| {
                json!({"kind": "tool", "tool": {"name": name, "arguments": arguments, "result": result, "status": "done"}})
            };
            let (id, body) = match letter {
                'P' => (format!("m_{k}"), json!({"kind": "message", "role": "person", "text": "Raconte-moi",
                                                  "client_id": format!("c{k}")})),
                'A' => (format!("m_{k}"), json!({"kind": "message", "role": "person", "text": "Et aussi ?",
                                                  "added_during_run": true})),
                'R' => (format!("m_{k}"), json!({"kind": "message", "role": "agent", "text": "",
                                                  "reasoning": "Je réfléchis."})),
                'Y' => (format!("m_{k}"), json!({"kind": "message", "role": "agent", "text": "Voilà, c'est fait."})),
                'I' => (format!("m_{k}"), json!({"kind": "message", "role": "agent",
                                                  "text": "Operation interrupted: handling API error (timeout)."})),
                'N' => (format!("m_{k}"), json!({"kind": "message", "role": "agent", "text": ""})),
                'E' => (format!("m_{k}"), json!({"kind": "message", "role": "peer", "from": "coach",
                                                  "from_name": "Coach", "text": "Tu peux m'aider ?"})),
                'T' => (format!("m_{k}.t0"), tool("web_search", r#"{"query": "piscine"}"#, "3 résultats")),
                'C' => (format!("m_{k}.t0"), tool("mcp_bipagents_connectors_ask_secret", r#"{"name": "exemple"}"#,
                                                  "Carte affichée")),
                'G' => (format!("m_{k}.t0"), tool("tool_call", r#"{"calls": [{"name": "memory"}, {"name": "web_search"}]}"#,
                                                  "ok")),
                'M' => (format!("m_{k}.t0"), tool("message_agent", r#"{"to": "coach"}"#, "envoyé à coach")),
                'B' => (format!("o_{k}"), json!({"kind": "boite", "text": "Le point du matin", "title": "Le point",
                                                  "attachments": [], "boite": {"outbox_id": k, "audio": false}})),
                'V' => (format!("ap_{k}"), json!({"kind": "approval", "approval": {"run_id": "r", "request_id": "q",
                                                   "command": "rm -rf ~/brouillons", "choice": "once",
                                                   "state": "answered"}})),
                'S' => (format!("s_c{k}"), json!({"kind": "message", "role": "person", "text": "Hello ?",
                                                   "client_id": format!("c{k}"), "sending": true})),
                other => panic!("no item kind {other}"),
            };
            // ten hours apart, the last stored an hour ago; one being sent a minute ago (« Pas envoyé » if nothing runs)
            let at = if letter == 'S' { now - 60.0 } else { now - 3600.0 - (n - k) as f64 * 36_000.0 };
            Item {
                turn: id.clone(),
                id,
                pos: (u32::from(k >= 3), 10 + k as i64, (k % 2) as u32),
                at,
                segment: format!("s{}", u32::from(k >= 3)),
                row: k as i64,
                body: body.as_object().cloned().unwrap_or_default(),
            }
        })
        .collect()
}

/// Where a turn would go on in it (`layout.open` of the whole thread, a turn running): its last message starting one
/// (the person's, another agent's, one being sent), else the first item drawing a line, else the first.
fn turn_going_on(letters: &[char]) -> usize {
    letters
        .iter()
        .rposition(|l| matches!(l, 'P' | 'E' | 'S'))
        .or_else(|| letters.iter().position(|l| *l != 'N'))
        .unwrap_or(0)
}

/// What a request asks: as `want_of` makes it, the bridge's size (`chosen`) or a given `limit`.
fn want(size: Option<usize>) -> Want {
    Want {
        chosen: size.is_none(),
        limit: size.unwrap_or(super::LIMIT_DEFAULT).clamp(1, LIMIT_MAX),
        before_count: 20,
        ..Default::default()
    }
}

/// The sizes a request may give (none: the bridge's).
const SIZES: [Option<usize>; 5] = [None, Some(1), Some(2), Some(3), Some(LIMIT_MAX)];

/// Every request for a thread: the latest page; `before=` and `after=` each item's cursor (`at`: only these items'),
/// one where an item was (gone), one before every item, one past the end, an unreadable one; `around=` each id, each
/// id's prefix (`m_3` for `m_3.t0`), an unknown one, with every `before_count` / `after_count`.
fn requests(items: &[Item], at: &[usize]) -> Vec<(String, Want)> {
    let mut cursors: Vec<(String, String)> = Vec::new();
    for &k in at {
        let item = &items[k];
        cursors.push((item.id.clone(), item.cursor()));
        cursors.push((format!("gone after {}", item.id), cursor_of((item.pos.0, item.pos.1, item.pos.2 + 50))));
    }
    cursors.push(("before all".into(), cursor_of((0, 0, 0))));
    cursors.push(("past the end".into(), cursor_of((9, 999_999, 0))));
    cursors.push(("unreadable".into(), "zz".into()));
    let mut ids: Vec<String> = at.iter().map(|&k| items[k].id.clone()).collect();
    ids.extend(at.iter().filter_map(|&k| items[k].id.split_once('.').map(|(p, _)| p.to_string())));
    ids.push("m_unknown".into());

    let mut out = Vec::new();
    for size in SIZES {
        let named = |what: String| format!("{what}, {}", size.map_or("bridge's size".into(), |s| format!("limit {s}")));
        out.push((named("latest".into()), want(size)));
        for (name, cursor) in &cursors {
            out.push((named(format!("before={name}")), Want { before: Some(cursor.clone()), ..want(size) }));
            out.push((named(format!("after={name}")), Want { after: Some(cursor.clone()), ..want(size) }));
        }
    }
    for id in &ids {
        for before_count in [0, 1, 20] {
            for after_count in [None, Some(0), Some(2)] {
                let w = Want { around: Some(id.clone()), before_count, after_count, ..want(None) };
                out.push((format!("around={id} before_count {before_count} after_count {after_count:?}"), w));
            }
        }
    }
    // an unknown item with a given size: the latest page of that size
    out.push(("around=m_unknown, limit 2".into(), Want { around: Some("m_unknown".into()), ..want(Some(2)) }));
    out
}

/// What the page must be, from the thread and the request alone: its range, whether it is the window (`reset`), its
/// anchor, the cursor it joins.
struct Expected {
    range: Range<usize>,
    reset: bool,
    anchor: Option<String>,
    joins: Option<String>,
}

fn expected(items: &[Item], w: &Want) -> Expected {
    let n = items.len();
    let latest = |size: usize| Expected { range: n.saturating_sub(size)..n, reset: true, anchor: None, joins: None };
    let size = |chosen: usize| if w.chosen { chosen } else { w.limit };
    // the first item at or after a cursor's place, and whether it is the cursor's own
    let place = |cursor: &str| {
        let pos = cursor_pos(cursor).unwrap_or((0, 0, 0));
        let k = items.iter().take_while(|i| i.pos < pos).count();
        (k, items.get(k).is_some_and(|i| i.pos == pos))
    };
    if let Some(around) = &w.around {
        let prefix = format!("{around}.");
        match items.iter().position(|i| &i.id == around || i.id.starts_with(&prefix)) {
            Some(k) => Expected {
                range: k.saturating_sub(w.before_count)..w.after_count.map_or(n, |a| (k + 1 + a).min(n)),
                reset: true,
                anchor: Some(items[k].id.clone()),
                joins: None,
            },
            None => latest(size(LATEST_PAGE)),
        }
    } else if let Some(after) = &w.after {
        let (k, found) = place(after);
        let first = if found { k + 1 } else { k };
        if w.chosen && (!found || n - first > CATCH_UP_PAGE) {
            return latest(LATEST_PAGE);
        }
        // the messages being sent come again in every catch-up (always at the end)
        let first = items.iter().position(|i| i.body.contains_key("sending")).map_or(first, |s| s.min(first));
        let range = first..(first + size(CATCH_UP_PAGE)).min(n);
        Expected { range, reset: false, anchor: None, joins: found.then(|| after.clone()) }
    } else if let Some(before) = &w.before {
        let (k, found) = place(before);
        if w.chosen && !found {
            return latest(LATEST_PAGE);
        }
        Expected { range: k.saturating_sub(size(OLDER_PAGE))..k, reset: false, anchor: None, joins: found.then(|| before.clone()) }
    } else {
        latest(size(LATEST_PAGE))
    }
}

type Days = Vec<Option<(chrono::NaiveDate, bool)>>;

/// Reads one page as the route does (`days`: the thread's, where the person is), and says what is wrong with it.
fn check(letters: &[char], items: &[Item], days: &Days, w: &Want, running: bool) -> Result<(), String> {
    let n = items.len();
    let read = catch_unwind(AssertUnwindSafe(|| {
        let (page, open) = conversation_page(items.to_vec(), w, days, running);
        let answer = page.answer(json!({"id": "t_guard"}), 1, json!({"run": null}), false);
        (page, open, answer)
    }));
    let (page, open, answer) = read.map_err(|e| {
        let why = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()));
        format!("panicked: {}", why.unwrap_or_default())
    })?;
    let want = expected(items, w);
    let range = page.start..page.start + page.items.len();

    // the items: a contiguous slice of the thread, the one asked for
    let ids = |of: &[Item]| of.iter().map(|i| i.id.clone()).collect::<Vec<_>>();
    if range.end > n || ids(&page.items) != ids(&items[range.start.min(n)..range.end.min(n)]) {
        return Err(format!("items {:?} at {} are not the thread's from there", ids(&page.items), page.start));
    }
    if range != want.range {
        return Err(format!("items {range:?}, expected {:?}", want.range));
    }
    if (page.reset, &page.anchor, &page.joins) != (want.reset, &want.anchor, &want.joins) {
        return Err(format!("reset/anchor/joins {:?}, expected {:?}",
                           (page.reset, &page.anchor, &page.joins), (want.reset, &want.anchor, &want.joins)));
    }
    if let Some(anchor) = &page.anchor {
        if !page.items.iter().any(|i| &i.id == anchor) {
            return Err(format!("anchor {anchor} is not on the page"));
        }
    }
    // more before / after: what the thread has outside the page
    if (page.more_before, page.more_after) != (range.start > 0, range.end < n) {
        return Err(format!("more_before/more_after {:?} for items {range:?} of {n}", (page.more_before, page.more_after)));
    }

    // `layout.open`: none, or the page's item where the turn going on is (its first, or the page's first after it)
    let going_on = turn_going_on(letters);
    let open_expected = (running && !range.is_empty() && going_on < range.end)
        .then(|| items[going_on.max(range.start)].id.clone());
    if open != open_expected {
        return Err(format!("layout.open {open:?}, expected {open_expected:?}"));
    }
    if let Some(open) = &open {
        if !page.items.iter().any(|i| &i.id == open) {
            return Err(format!("layout.open {open} is not on the page"));
        }
    }
    // `layout` fields: only for the page's items, never for the turn going on (undecided)
    let decided = layout::layout(items, range.clone(), running);
    if let Some(n) = decided.fields.keys().find(|k| !range.contains(k) || (running && **k >= going_on)) {
        return Err(format!("layout fields for item {n}, outside the page {range:?} or in the turn going on"));
    }
    for (k, item) in page.items.iter().enumerate() {
        let at = range.start + k;
        if item.body.contains_key("layout") != decided.fields.contains_key(&at) {
            return Err(format!("item {} carries layout: {}", item.id, item.body.contains_key("layout")));
        }
        // its day where the person is, when it draws something (the first of its day: a divider)
        if item.body.contains_key("day") != days[at].is_some()
            || item.body.contains_key("new_day") != days[at].is_some_and(|(_, new)| new)
        {
            return Err(format!("item {} day {:?} new_day {:?}", item.id, item.body.get("day"), item.body.get("new_day")));
        }
        // a message being sent: « Pas envoyé » once nothing runs
        if letters[at] == 'S' && (item.body.contains_key("not_sent") == running || item.body.contains_key("sending") != running) {
            return Err(format!("item {} being sent, running {running}: {:?}", item.id, item.body));
        }
    }

    // as served: its items with their cursors, `first` and `last` the page's own
    let served = answer["items"].as_array().cloned().unwrap_or_default();
    if served.len() != page.items.len() {
        return Err(format!("{} items served of {}", served.len(), page.items.len()));
    }
    for (s, item) in served.iter().zip(&page.items) {
        let cursor = s["cursor"].as_str().unwrap_or("");
        if s["id"] != json!(item.id) || cursor_pos(cursor) != Some(item.pos) {
            return Err(format!("served {} with cursor {cursor:?}", s["id"]));
        }
    }
    let edge = |i: Option<&Item>| i.map_or(Value::Null, |i| json!(i.cursor()));
    if answer["first"] != edge(page.items.first()) || answer["last"] != edge(page.items.last()) {
        return Err(format!("first/last {} {}", answer["first"], answer["last"]));
    }
    if answer["more_before"] != json!(page.more_before) || answer["more_after"] != json!(page.more_after) {
        return Err("more_before/more_after served differently".into());
    }
    Ok(())
}

/// What reading threads every way found: the pages read, how many were the catch-up of 7/11 (after the last item, a
/// turn going on, nothing new), what failed.
#[derive(Default)]
struct Reads {
    pages: usize,
    caught_up_empty: usize,
    /// What failed: the read (thread, state, request), and why.
    failures: Vec<(String, String)>,
}

/// Reads `letters`' thread with every request (`at`: the items whose cursors and ids are asked for, None: all), a
/// turn going on and not.
fn read_every_way(letters: &str, at: Option<Vec<usize>>, out: &mut Reads) {
    let chars: Vec<char> = letters.chars().collect();
    let items = thread(letters);
    let zone = crate::dates::parse_zone("Europe/Paris").expect("a zone");
    let days = timeline::days(&items, zone);
    let at = at.unwrap_or_else(|| (0..items.len()).collect());
    let last = items.last().map(Item::cursor);
    for running in [false, true] {
        for (name, w) in requests(&items, &at) {
            out.pages += 1;
            if running && last.is_some() && w.after == last {
                out.caught_up_empty += 1;
            }
            if let Err(why) = check(&chars, &items, &days, &w, running) {
                out.failures.push((format!("thread {letters:?} ({} items), running {running}, {name}", items.len()), why));
            }
        }
    }
}

/// Reads every thread every way, on every core.
fn read_all(threads: Vec<(String, Option<Vec<usize>>)>) -> Reads {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
    let chunks: Vec<Vec<(String, Option<Vec<usize>>)>> = (0..cores)
        .map(|c| threads.iter().skip(c).step_by(cores).cloned().collect())
        .collect();
    std::thread::scope(|scope| {
        let readers: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                scope.spawn(move || {
                    let mut out = Reads::default();
                    for (letters, at) in chunk {
                        read_every_way(&letters, at, &mut out);
                    }
                    out
                })
            })
            .collect();
        readers.into_iter().fold(Reads::default(), |mut all, reader| {
            let r = reader.join().expect("a reader");
            all.pages += r.pages;
            all.caught_up_empty += r.caught_up_empty;
            all.failures.extend(r.failures);
            all
        })
    })
}

#[test]
fn every_conversation_reads_every_way() {
    // the small threads: every sequence of KINDS up to EXHAUSTIVE items, each also with a message being sent after
    // it; every window of up to SMALL items of the longer conversations
    let mut small: Vec<String> = vec![String::new()];
    let mut last = vec![String::new()];
    for _ in 0..EXHAUSTIVE {
        last = last.iter().flat_map(|s| KINDS.chars().map(move |k| format!("{s}{k}"))).collect();
        small.extend(last.iter().cloned());
    }
    small.extend(small.clone().into_iter().map(|s| format!("{s}S")));
    for conversation in CONVERSATIONS {
        let c: Vec<char> = conversation.chars().collect();
        for start in 0..c.len() {
            for end in start..=(start + SMALL).min(c.len()) {
                small.push(c[start..end].iter().collect());
            }
        }
    }
    let mut seen = HashSet::new();
    small.retain(|s| seen.insert(s.clone()));
    let sizes: HashSet<usize> = small.iter().map(String::len).collect();
    assert_eq!(sizes, (0..=SMALL).collect::<HashSet<_>>(), "a size of thread missing");

    let mut threads: Vec<(String, Option<Vec<usize>>)> = small.iter().map(|s| (s.clone(), None)).collect();
    // threads at the bridge's own sizes (latest page, catch-up, older page): the cursors around their edges
    let mut large = 0;
    for n in [LATEST_PAGE - 1, LATEST_PAGE, LATEST_PAGE + 1, CATCH_UP_PAGE, CATCH_UP_PAGE + 1, CATCH_UP_PAGE + 2] {
        for tail in ["", "S"] {
            let letters: String = CONVERSATIONS[0].chars().filter(|c| *c != 'S').cycle().take(n).collect::<String>() + tail;
            let len = letters.len();
            let mut at: Vec<usize> = [0, 1, 2, len / 2, len - 1, len.saturating_sub(2)]
                .into_iter()
                .chain([CATCH_UP_PAGE, LATEST_PAGE, OLDER_PAGE].iter().flat_map(|p| {
                    [len.saturating_sub(*p + 2), len.saturating_sub(*p + 1), len.saturating_sub(*p)]
                }))
                .filter(|k| *k < len)
                .collect();
            at.sort_unstable();
            at.dedup();
            threads.push((letters, Some(at)));
            large += 1;
        }
    }
    let Reads { pages, caught_up_empty, failures } = read_all(threads);
    println!(
        "timeline guard: {} small threads (0..={SMALL} items) and {large} at the page sizes, {pages} pages read, \
         {caught_up_empty} of them the 7/11 catch-up (after the last item, a turn going on)",
        small.len()
    );
    // the guard is not empty: it read the page of 7/11 in every small thread that has an item
    assert!(caught_up_empty >= small.len() - 1, "the catch-up of 7/11 read {caught_up_empty} times");
    // what failed, by kind (its words without numbers), then the first reads
    let mut kinds = std::collections::BTreeMap::<String, usize>::new();
    for (_, why) in &failures {
        *kinds.entry(why.chars().filter(|c| !c.is_ascii_digit()).collect()).or_default() += 1;
    }
    assert!(
        failures.is_empty(),
        "{} of {pages} pages failed; by kind:\n{}\nthe first ones:\n{}",
        failures.len(),
        kinds.iter().map(|(why, n)| format!("{n} × {why}")).collect::<Vec<_>>().join("\n"),
        failures.iter().take(12).map(|(read, why)| format!("{read}: {why}")).collect::<Vec<_>>().join("\n")
    );
}
