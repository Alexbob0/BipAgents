//! Second independent review of `page-store` (15/11, after the fixes of 3ee0bdcc8): tests for what is still wrong.
//! Run: `cargo test --locked --test review2_page_store`. Each test FAILS on 3ee0bdcc8 and shows one finding.

mod common;

use bipbridge::{page_store, pages};
use common::{config, Urls};
use serde_json::{json, Value};

const AGENT: &str = "wellness";

fn group_page(config: &bipbridge::config::Config, title: &str) -> String {
    let mark = pages::Mark { group: true, asked_by: None, lang: "fr".into() };
    let made = pages::save_marked(
        config,
        AGENT,
        "<h1>Dépenses</h1><script>Bip.store.onChange(function(){})</script>",
        title,
        None,
        Some(&mark),
    )
    .unwrap();
    made["page"].as_str().unwrap().to_string()
}

fn write(config: &bipbridge::config::Config, page: &str, op: Value, act: &str, by: &str) {
    let (op, _) = page_store::parse_op(op.as_object().unwrap(), &[]).unwrap();
    page_store::apply(config, AGENT, page, &op, Some(act), by).unwrap();
}

fn amounts(store: &Value) -> Vec<(String, i64)> {
    store["keys"]["depenses"]["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|i| (i["value"]["what"].as_str().unwrap_or("").to_string(), i["value"]["cents"].as_i64().unwrap_or(-1)))
                .collect()
        })
        .unwrap_or_default()
}

fn remake(config: &bipbridge::config::Config, page: &str, script: &str) {
    pages::save(config, AGENT, script, "Dépenses", Some(page)).unwrap();
}

/// FINDING R2-2 (the way back destroys what it should keep). `restore(version, current)` first writes the live store
/// over the copy named `current`, THEN reads the copy asked for. When the person asks for the copy `restore` itself just
/// made (the page's current version: « finalement, remets comme avant »), that copy is overwritten by the store it is
/// meant to replace before it is read: the restore answers 200, changes nothing, and what the store held before the
/// first restore (here the line « Taxi » typed after the new version) is gone for good.
#[test]
fn review2_undoing_a_restore_gives_back_the_store_as_it_was_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), &Urls::default());
    let page = group_page(&config, "Dépenses annulées"); // version 1
    write(&config, &page, json!({"op": "add", "key": "depenses", "id": "l1", "value": {"what": "Resto", "cents": 4250}}), "a1", "Léa");
    remake(&config, &page, "<h1>Dépenses v2</h1><script>Bip.store.onChange(function(){})</script>"); // version 2: v1 kept
    // after the new version: its script edits Resto (or a person did), and Léa types a real line
    write(&config, &page, json!({"op": "edit", "key": "depenses", "id": "l1", "value": {"what": "Resto", "cents": 1}}), "a2", "Alex");
    write(&config, &page, json!({"op": "add", "key": "depenses", "id": "l2", "value": {"what": "Taxi", "cents": 1800}}), "a3", "Léa");
    // the person puts version 1's copy back: Resto 42,50 again, Taxi missing; what it replaced is kept « in its turn »
    page_store::restore(&config, AGENT, &page, 1, 2).unwrap();
    assert_eq!(amounts(&page_store::read(&config, AGENT, &page)), vec![("Resto".to_string(), 4250)]);
    let kept: Vec<i64> = page_store::snapshots(&config, AGENT, &page).into_iter().map(|(v, _)| v).collect();
    assert!(kept.contains(&2), "the store replaced by the restore is listed as version 2's copy: {kept:?}");
    // « Taxi » is missing: they put back the copy the restore just made (the newest one, as GET versions lists it)
    page_store::restore(&config, AGENT, &page, 2, 2).unwrap();
    let back = amounts(&page_store::read(&config, AGENT, &page));
    assert!(
        back.contains(&("Taxi".to_string(), 1800)),
        "undoing the restore gave {back:?}: the copy of the store before the restore was overwritten before being read"
    );
}

/// FINDING R2-4 (the way back can be flushed by the very agent it guards against). Copies are made at each new version
/// of the page and only the last `SNAPSHOTS_KEPT` (5) are kept. An agent that remade its page with a script that rewrites
/// the amounts (the review's finding 6) needs only to remake it 5 more times once the script has run in someone's page
/// (one more turn: the friend's line that injected it is still in the store, read again with page_data): every copy
/// then holds the rewritten amounts, the one copy with the real ones is deleted.
#[test]
fn review2_a_few_more_versions_of_the_page_do_not_flush_the_last_good_copy() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), &Urls::default());
    let page = group_page(&config, "Dépenses vidées");
    write(&config, &page, json!({"op": "add", "key": "depenses", "id": "l1", "value": {"what": "Resto", "cents": 4250}}), "a1", "Léa");
    // the injected agent's version: its script edits every line to 1 cent at the next opening (copy v1: the real amounts)
    remake(&config, &page, "<script>fetch('bipstore://store/?op=…')</script>");
    write(&config, &page, json!({"op": "edit", "key": "depenses", "id": "l1", "value": {"what": "Resto", "cents": 1}}), "a2", "Alex");
    // a later turn: five more versions (each keeps the store as it now is, the rewritten one)
    for n in 0..page_store::SNAPSHOTS_KEPT {
        remake(&config, &page, &format!("<h1>v{n}</h1><script>Bip.store.onChange(function(){{}})</script>"));
    }
    let copies: Vec<Vec<(String, i64)>> = page_store::snapshots(&config, AGENT, &page)
        .into_iter()
        .map(|(_, file)| amounts(&page_store::read_file(&file)))
        .collect();
    assert!(
        copies.iter().any(|c| c.contains(&("Resto".to_string(), 4250))),
        "no copy keeps the real amount any more: {copies:?}"
    );
}
