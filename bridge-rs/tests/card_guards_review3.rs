//! Third review of card-guards (68729945f), 14/11. Never pushed. rx1/rx2 measure false refusals on scripts real
//! agents wrote (145 distinct execute_script calls of the fix-signup-hold and select-option-direct benches, GLM-5.3
//! Flash and DeepSeek on sign-up, delivery and checkout forms) and on ordinary reading scripts; rx3 is a way to change
//! the page's code that TAMPERS doesn't know (it matters only when the guard looks through the page's own world).

use bipbridge::browser_mcp::{blind_refusal, script_card_refusal};
use serde_json::{json, Value};

const BENCH: &str = include_str!("rx_agent_scripts.json");

fn ordinary() -> Value {
    json!({"pw": {}, "known": [], "boxes": [], "cards": {}, "cardtext": false, "frames": 0})
}
fn card_page() -> Value {
    json!({"pw": {}, "known": [], "boxes": [], "cards": {"#cc": "811c9dc5"}, "cardtext": false, "frames": 0})
}

/// The benches' scripts, on a page without card fields (a sign-up, a delivery address): none refused by the card
/// guard. On a page with card fields (a checkout): listed, for the report.
#[test]
fn rx1_real_agents_scripts_are_not_refused_where_no_card_is() {
    let scripts: Vec<String> = serde_json::from_str(BENCH).unwrap();
    let mut refused = Vec::new();
    let mut on_cards = Vec::new();
    for s in &scripts {
        // fourth review of 15/11: a script that doesn't parse is refused (it runs nowhere either); 44 of these were cut
        // at 300 characters when collected, one has a parenthesis too many: left out here
        if !bipbridge::script_shape::shape(s).parsed {
            continue;
        }
        if let Some(why) = script_card_refusal(s, Some(&ordinary())) {
            refused.push(format!("{} => {}", s.chars().take(160).collect::<String>().replace('\n', " "), &why[..60]));
        }
        if script_card_refusal(s, Some(&card_page())).is_some() || blind_refusal(s, Some(&card_page())).is_some() {
            on_cards.push(s.chars().take(100).collect::<String>().replace('\n', " "));
        }
    }
    println!("{} bench scripts; on a page with card fields {} refused (card rules or blind)", scripts.len(), on_cards.len());
    assert!(refused.is_empty(), "{} of {} refused where no card is:\n{}", refused.len(), scripts.len(), refused.join("\n"));
}

/// Ordinary scripts an agent writes on ordinary pages (no card field): reading, searching a word, a cookie banner,
/// « load more », a search form. Each should go on.
#[test]
fn rx2_ordinary_scripts_go_on_where_no_card_is() {
    let around = |word: &str| bipbridge::browser_mcp::AROUND_A_WORD.replace("word", word);
    let scripts = [
        around("observer"),                       // the skill's own script, searching « The Observer »
        around("postMessage"),                    // a developer docs page
        "return ['Prix : ' + document.querySelector('.price').innerText, 'Stock : ' + document.querySelector('.stock').innerText].join('\\n')".to_string(),
        "return document.querySelector('[onclick=\"addToCart(42)\"]').innerText".to_string(),
        "return JSON.parse(atob(document.getElementById('product-data').textContent)).price".to_string(),
        "(async () => { document.querySelector('#q').value = 'chaussures'; document.querySelector('#search').submit(); })()".to_string(),
        "document.querySelector('#onetrust-accept-btn-handler').click()".to_string(),
        "window.scrollTo(0, document.body.scrollHeight); document.querySelector('.load-more').click()".to_string(),
    ];
    let refused: Vec<String> = scripts
        .iter()
        .filter_map(|s| script_card_refusal(s, Some(&ordinary())).map(|why| format!("{} => {}", s.chars().take(120).collect::<String>(), &why[..70])))
        .collect();
    assert!(refused.is_empty(), "refused on an ordinary page:\n{}", refused.join("\n"));
}

/// On a page with card fields, reading scripts that build their answer in an object (`o.name = …`) or copy a title:
/// read as reads (no other refusal), and since the hotfix of 15/11 refused there as any script (SCRIPT_ON_CARDS_REFUSED).
#[test]
fn rx2b_reading_scripts_go_on_on_a_checkout() {
    let scripts = [
        "const o = {}; o.name = document.querySelector('h1').innerText; o.total = document.querySelector('.total').innerText; return o",
        "const r = []; for (const li of document.querySelectorAll('.line')) { const x = {}; x.title = li.innerText; r.push(x); } return r",
    ];
    let refused: Vec<String> = scripts
        .iter()
        .filter_map(|s| {
            script_card_refusal(s, Some(&card_page()))
                .filter(|why| *why != bipbridge::browser_mcp::SCRIPT_ON_CARDS_REFUSED)
                .map(|why| format!("{s} => {}", &why[..70]))
        })
        .collect();
    assert!(refused.is_empty(), "refused on a checkout page for something else:\n{}", refused.join("\n"));
    for s in scripts {
        assert_eq!(script_card_refusal(s, Some(&card_page())), Some(bipbridge::browser_mcp::SCRIPT_ON_CARDS_REFUSED), "{s}");
    }
}

/// Changing the page's own code without the words TAMPERS knows: `Object.defineProperties`, `Object.assign` onto a
/// prototype. Refused like `defineProperty` (it matters when the guard looks through the page's world: blind).
#[test]
fn rx3_every_way_to_change_the_pages_code_is_refused() {
    for script in [
        "Object.defineProperties(HTMLInputElement.prototype, {name: {get() { return 'iban'; }}}); 'ok'",
        "Object.assign(String.prototype, {match() { return null; }}); 'ok'",
    ] {
        assert!(script_card_refusal(script, Some(&ordinary())).is_some(), "not refused: {script}");
    }
}
