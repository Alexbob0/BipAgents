//! Seventh review of card-guards, 15/11: new ways against the production hotfix (hotfix-card-guards-main 23785ed96),
//! its rules 1 (location in any form, a javascript: address written or decoded) and 2 (the site's storage). Each
//! script must be refused (`script_card_refusal`, a page without card fields); red = it goes through.
//! h1/h2 are proven on Chrome 157 by test_card_guard_review7_hotfix_chrome.py.

use bipbridge::browser_mcp::script_card_refusal;
use serde_json::json;

fn ordinary() -> serde_json::Value {
    json!({"pw": {}, "known": [], "boxes": [], "cards": {}, "cardtext": false, "frames": 0})
}

/// The document without its name, its location from its only own property, a javascript: address in pieces the
/// script can't put together before it runs.
const DOC: &str = "const d = document.body.getRootNode(); \
const L = Object.values(Object.getOwnPropertyDescriptors(d)).find((x) => x.get).get.call(d); ";
const ADDRESS: &str = "'jav' + ['a'].pop() + 'script:' + encodeURIComponent(c)";

#[test]
fn h_new_ways_to_location_javascript_and_storage_are_refused() {
    let scripts = [
        // the coordinator's: Reflect.get, globalThis[…], with, indirect eval
        "Reflect.get(window, 'location').replace(u)".to_string(),
        "globalThis['loc' + 'ation'].replace(u)".to_string(),
        "with (document) { location.replace(u) }".to_string(),
        "(0, eval)(s)".to_string(),
        // h1: Location from the document's descriptors, its replace on what the script calls a string (toString)
        format!("{DOC}const o = {{toString() {{ return L; }}}}; o.toString().replace({ADDRESS}); 'ok'"),
        // h2: Location's own href setter, called with .call
        format!("{DOC}Object.values(Object.getOwnPropertyDescriptors(L)).find((x) => x.set).set.call(L, {ADDRESS}); 'ok'"),
        // h3: a cookie written through Document.prototype's setter
        "Object.getOwnPropertyDescriptor(Document.prototype, 'cookie').set.call(document, 'r7=1; path=/'); 'ok'".to_string(),
        // h4: sessionStorage written through Storage.prototype
        "Storage.prototype.setItem.call(sessionStorage, 'wc_fragments_r7', s); 'ok'".to_string(),
    ];
    let mut passed = Vec::new();
    for s in &scripts {
        let r = script_card_refusal(s, Some(&ordinary()));
        println!("{} => {:?}", s.chars().take(110).collect::<String>(), r.map(|w| &w[..60]));
        if r.is_none() {
            passed.push(s.chars().take(140).collect::<String>());
        }
    }
    assert!(passed.is_empty(), "not refused ({}): {passed:#?}", passed.len());
}
