//! Independent review of `page-store-agent-writes` (16/11, c08e88793): the agent writes its own pages' stores
//! (`page_data` with `ops`). Run: `cargo test --locked --test review_page_store_agent_writes`.
//! Each test FAILS on c08e88793 and shows one finding (R1…R7 in passation-pro/briefs/review-page-store-agent-writes.md).

mod common;

use bipbridge::config::{AgentConfig, Config};
use bipbridge::loop_brake::{Settings, Turn, Turns};
use bipbridge::{page_store, pages, Dict};
use common::{auth, config, TestApp, Urls};
use serde_json::{json, Value};

const AGENT: &str = "wellness";

fn setup() -> (tempfile::TempDir, Config, AgentConfig) {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), &Urls::default());
    let agent = config.agent(Some(AGENT)).cloned().unwrap();
    (dir, config, agent)
}

/// A group page (made in a shared conversation: friends and family see its store). Titles differ per test: the
/// agent's budget (`<page>#agent`) is the process's.
fn group_page(config: &Config, title: &str) -> String {
    let mark = pages::Mark { group: true, asked_by: None, lang: "fr".into() };
    let made = pages::save_marked(
        config,
        AGENT,
        "<h1>Feuille</h1><p id=\"t\"></p><script>Bip.store.onChange(function(){Bip.set(\"#t\", \"ok\")})</script>",
        title,
        None,
        Some(&mark),
    )
    .unwrap();
    made["page"].as_str().unwrap().to_string()
}

/// A person's write (the app's door, a relative, a friend): by their first name.
fn typed(config: &Config, page: &str, op: Value, by: &str) {
    let (op, _) = page_store::parse_op(op.as_object().unwrap(), &[]).unwrap();
    page_store::apply(config, AGENT, page, &op, None, by).unwrap();
}

fn items(store: &Value, key: &str) -> Vec<Value> {
    store["keys"][key]["items"].as_array().cloned().unwrap_or_default()
}

fn args(v: Value) -> Dict {
    v.as_object().cloned().unwrap()
}

fn text_of(result: &Dict) -> String {
    result["content"][0]["text"].as_str().unwrap_or("").to_string()
}

/// The line's time moved back by `seconds` in the store's file (as if it had been added that long ago).
fn age_lines(config: &Config, page: &str, key: &str, seconds: f64) {
    let file = page_store::path(config, AGENT, page);
    let mut store: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    for line in store["keys"][key]["items"].as_array_mut().unwrap() {
        let at = line["at"].as_f64().unwrap();
        line["at"] = json!(at - seconds);
    }
    std::fs::write(&file, serde_json::to_string(&store).unwrap()).unwrap();
}

// -- R1: an `add` without an id is merged with ANY earlier line of the same key and value --------------------------

/// FINDING R1. `line_id` derives an add's id from its key and value only. A tracker fed by a routine (the tool's own
/// pitch: « a dashboard, prices, scores, a tracker ») adds the same reading again two hours (or a day) later: the
/// second reading is never written, and the agent is told « Nothing to change in page …: it already holds that ».
/// Two coffees in one call make one line. A line a person edited is « already there » when the agent adds its first
/// value again. Only a real resend (the same call again within minutes, the line untouched) should count once.
#[test]
fn review_r1_the_same_reading_later_or_twice_in_a_call_is_a_new_line_only_a_resend_counts_once() {
    let (_dir, config, agent) = setup();
    let page = group_page(&config, "Relevés du jardin R1");
    let reading = json!([{"op": "add", "key": "releves", "value": {"temp": 17}}]);
    page_store::agent_write(&config, &agent, &page, &reading).unwrap();
    // a resend right away (Hermes or the model sent it twice): still one line — the author's guarantee, kept
    let again = page_store::agent_write(&config, &agent, &page, &reading).unwrap();
    assert_eq!(again["changed"], json!(false));
    assert_eq!(items(&page_store::read(&config, AGENT, &page), "releves").len(), 1);
    // two hours later the routine runs again and reads 17 °C again: a second reading
    age_lines(&config, &page, "releves", 7200.0);
    let said = page_store::call(&config, &agent, &args(json!({"page": page, "ops": reading})));
    assert!(!text_of(&said).starts_with("Nothing to change"), "the 2nd reading was dropped: {}", text_of(&said));
    assert_eq!(items(&page_store::read(&config, AGENT, &page), "releves").len(), 2, "R1: the same reading 2 h later is merged");

    // two coffees in one call: two lines (and the same call resent at once: still two)
    let coffees = json!([{"op": "add", "key": "depenses", "value": {"what": "Café", "cents": 250}},
                         {"op": "add", "key": "depenses", "value": {"what": "Café", "cents": 250}}]);
    let first = page_store::agent_write(&config, &agent, &page, &coffees).unwrap();
    page_store::agent_write(&config, &agent, &page, &coffees).unwrap();
    let ids: Vec<&str> = first["ids"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_ne!(ids[0], ids[1], "R1: two lines in one call got one id");
    assert_eq!(items(&page_store::read(&config, AGENT, &page), "depenses").len(), 2, "R1: two coffees, one line");

    // a person edits the agent's line; the agent adds its first words again: a new line, not « already there »
    let bread = json!([{"op": "add", "key": "courses", "value": "Pain"}]);
    let made = page_store::agent_write(&config, &agent, &page, &bread).unwrap();
    let id = made["ids"][0].as_str().unwrap().to_string();
    typed(&config, &page, json!({"op": "edit", "key": "courses", "id": id, "value": "Pain complet"}), "Léa");
    page_store::agent_write(&config, &agent, &page, &bread).unwrap();
    let values: Vec<Value> = items(&page_store::read(&config, AGENT, &page), "courses").iter().map(|l| l["value"].clone()).collect();
    assert_eq!(values, vec![json!("Pain complet"), json!("Pain")], "R1: the agent's « Pain » is lost");
}

// -- R2: a drop of a line that isn't there is « it already holds that » -------------------------------------------

/// FINDING R2. A `drop` whose line isn't there (gone — or an id the model got wrong, as models do with `l3fa9…`) is
/// done in silence: the agent hears « Nothing to change …: it already holds that », says « c'est retiré », and the
/// line is still in the page.
#[test]
fn review_r2_a_drop_of_a_line_that_is_not_there_is_said_not_hidden() {
    let (_dir, config, agent) = setup();
    let page = group_page(&config, "Courses R2");
    typed(&config, &page, json!({"op": "add", "key": "courses", "id": "l1", "value": "Lait"}), "Léa");
    let said = page_store::call(&config, &agent, &args(json!({"page": page, "ops": [{"op": "drop", "key": "courses", "id": "l1x"}]})));
    let text = text_of(&said);
    assert_eq!(items(&page_store::read(&config, AGENT, &page), "courses").len(), 1);
    assert!(text.contains("l1x") && !text.contains("already holds"), "R2: the agent is told it's done: {text}");
}

// -- R3: the agent can flush the copy that keeps what people typed --------------------------------------------------

/// FINDING R3 (security: the « copy before touching people's data » is the replacement for « no write tool »). Each
/// agent call that touches a person's line keeps a new copy, and copies are capped at 30: an agent misled by a line it
/// read (« retire les lignes une par une ») drops 30 of Léa's lines one per call — one routine run allows 40 writes —
/// and the copy made before its first drop is pruned (the oldest copy, kept forever, predates Léa's lines). Léa's
/// first line is then nowhere: not in the page, not in any copy support could put back.
#[test]
fn review_r3_an_agent_dropping_people_s_lines_one_by_one_can_t_flush_the_copy_that_holds_them() {
    let (_dir, config, agent) = setup();
    let page = group_page(&config, "Dépenses du week-end R3");
    // the page as first made, then made again once (a copy of its store then: the oldest, kept forever)
    typed(&config, &page, json!({"op": "set", "key": "titre", "value": "Week-end"}), "Alex");
    pages::save(&config, AGENT, "<h1>Feuille v2</h1><script>Bip.store.onChange(function(){})</script>", "Dépenses du week-end R3", Some(&page))
        .unwrap();
    assert_eq!(page_store::snapshots(&config, AGENT, &page).len(), 1);
    // Léa types the weekend's 35 expenses
    for n in 1..=35 {
        typed(&config, &page, json!({"op": "add", "key": "depenses", "id": format!("l{n}"), "value": {"what": format!("Ligne {n}"), "cents": 100 * n}}), "Léa");
    }
    // the misled agent drops them one per call (within its page budget and a routine's 40 writes)
    for n in 1..=30 {
        if page_store::agent_write(&config, &agent, &page, &json!([{"op": "drop", "key": "depenses", "id": format!("l{n}")}])).is_err() {
            break; // refused: fine, as long as nothing is lost
        }
    }
    let mut held: Vec<Value> = vec![page_store::read(&config, AGENT, &page)];
    held.extend(page_store::snapshots(&config, AGENT, &page).iter().map(|(_, f)| page_store::read_file(f)));
    for n in 1..=35 {
        let id = format!("l{n}");
        let somewhere = held.iter().any(|s| items(s, "depenses").iter().any(|l| l["id"] == json!(id) && l["by"] == json!("Léa")));
        assert!(somewhere, "R3: Léa's line {id} is in no copy any more ({} copies)", held.len() - 1);
    }
}

// -- R4: a routine's reads of page_data are refused after 10 calls of it, writes included -------------------------

/// FINDING R4. In a routine (no followed run), the brake counts `page_data` calls under one key, reads and writes
/// together, and holds a READ at the conversation's `same_tool` (10): a routine that wrote its dashboard in 10 calls
/// (one op each, as small models do) and reads it back is told « Stop calling tools now ».
#[test]
fn review_r4_a_routine_that_wrote_its_page_ten_times_may_still_read_it() {
    let turns = Turns::default();
    let settings = Settings::default();
    let write = |n: i64| args(json!({"page": "jardin", "ops": [{"op": "set", "key": format!("k{n}"), "value": n}]}));
    let written = args(json!({"content": [{"type": "text", "text": "Written in page jardin."}], "isError": false}));
    for n in 0..10 {
        assert!(turns.check(&settings, "vie", None, "page_data", &write(n), n as f64).is_none(), "{n}");
        turns.record_answer("vie", None, "page_data", &write(n), &written);
    }
    let read = turns.check(&settings, "vie", None, "page_data", &args(json!({"page": "jardin"})), 11.0);
    assert!(read.is_none(), "R4: the routine's reading refused: {read:?}");
}

// -- R5: support can't tell which copy is the one before the agent's write ----------------------------------------

/// FINDING R5. `GET /v1/page-store/versions` gives counts only (rev, keys, lines). When the agent overwrote amounts
/// (an edit, a set), every copy has the same counts, and none says when it was kept nor why (the page made again, the
/// agent's write, a restore): support, who must not read what was typed, can't find « the copy before 14:02 ».
#[tokio::test]
async fn review_r5_the_copies_say_when_and_why_they_were_kept() {
    let (dir, config, agent) = setup();
    let page = group_page(&config, "Budget R5");
    typed(&config, &page, json!({"op": "set", "key": "budget", "value": 30000}), "Alex");
    page_store::agent_write(&config, &agent, &page, &json!([{"op": "set", "key": "budget", "value": 1}])).unwrap();
    let app = TestApp::new(config);
    let (k, v) = auth();
    let path = format!("/bipagents/pages/{AGENT}/{page}.html");
    let encoded: String = url::form_urlencoded::byte_serialize(path.as_bytes()).collect();
    let got = app.get(&format!("/v1/page-store/versions?path={encoded}"), &[(k, v.as_str())]).await;
    assert_eq!(got.status, 200, "{}", got.text());
    let versions = got.json()["versions"].clone();
    assert_eq!(versions.as_array().unwrap().len(), 1, "{versions}");
    let copy = &versions[0];
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    assert!(copy["at"].as_f64().is_some_and(|at| (now - at).abs() < 300.0), "R5: no time on the copy: {copy}");
    assert_eq!(copy["why"], json!("agent"), "R5: no cause on the copy: {copy}");
    app.shutdown().await;
    drop(dir);
}

// -- R6: « the agent's » lines are told by its display name ----------------------------------------------------------

/// FINDING R6. Whose line it is goes by name: `touches_theirs` and the reading's « Lines by « X » are yours » compare
/// `by` with the agent's display name. A person whose first name is the agent's name (an agent called « Léa », a
/// friend Léa) has her lines taken for the agent's: dropped with no copy, and read back as the agent's own.
#[test]
fn review_r6_a_person_named_like_the_agent_still_gets_her_copy() {
    let (_dir, config, mut agent) = setup();
    agent.display_name = "Léa".into();
    let page = group_page(&config, "Courses de Léa R6");
    typed(&config, &page, json!({"op": "add", "key": "courses", "id": "l1", "value": "Pain"}), "Léa");
    page_store::agent_write(&config, &agent, &page, &json!([{"op": "drop", "key": "courses", "id": "l1"}])).unwrap();
    assert_eq!(page_store::snapshots(&config, AGENT, &page).len(), 1, "R6: Léa's line dropped with no copy");
}

// -- R7: A, B, then A again in one turn is « Already done » -------------------------------------------------------

/// FINDING R7. The brake's « Already done in this turn » (meant for a write tool asked twice) applies to `page_data`
/// with `ops`: a task feeding a live page sets « état » to « Recherche… », then « 3 trouvés », then « Recherche… »
/// for the next item — the third is refused as already done, with the first answer (« Written … »), while the page
/// shows « 3 trouvés ». The store is idempotent by itself (R1 kept): the brake needn't dedup its writes.
#[test]
fn review_r7_a_value_set_back_in_the_same_turn_is_written() {
    let turns = Turns::default();
    let settings = Settings::default();
    let turn = Some(Turn { run_id: "run_r7".into(), background: true, banned: None });
    let state = |v: &str| args(json!({"page": "veille", "ops": [{"op": "set", "key": "etat", "value": v}]}));
    let written = args(json!({"content": [{"type": "text", "text": "Written in page veille."}], "isError": false}));
    for (n, v) in ["Recherche…", "3 trouvés"].iter().enumerate() {
        assert!(turns.check(&settings, "vie", turn.as_ref(), "page_data", &state(v), n as f64).is_none());
        turns.record_answer("vie", turn.as_ref(), "page_data", &state(v), &written);
    }
    let back = turns.check(&settings, "vie", turn.as_ref(), "page_data", &state("Recherche…"), 3.0);
    assert!(back.is_none(), "R7: « Recherche… » set back is refused: {back:?}");
}
