//! The 145 scripts real agents wrote in the sign-up and drop-down list benches (tests/rx_agent_scripts.json, the third
//! review of card-guards, 14/11; 44 cut at 300 characters when collected), as the card guard reads them: none refused where no card is (rx1); on a checkout,
//! only those that write fields and send the form or press a button, or name a card's field, each refusal saying
//! what to do instead.

use bipbridge::browser_mcp::{script_card_refusal, SCRIPT_CARD_REFUSED, SCRIPT_FILLS_ON_CARDS_REFUSED};
use bipbridge::script_shape::shape;
use serde_json::json;

const BENCH: &str = include_str!("rx_agent_scripts.json");

#[test]
fn the_bench_scripts_parse_and_on_a_checkout_only_filling_and_sending_is_refused() {
    let scripts: Vec<String> = serde_json::from_str(BENCH).unwrap();
    let checkout = json!({"pw": {}, "known": [], "boxes": [], "cards": {"#cc": "811c9dc5"}, "cardtext": false, "frames": 0});
    let mut unparsed = Vec::new();
    let mut other = Vec::new();
    let mut filled = 0;
    for script in &scripts {
        // 44 of them were cut at 300 characters when collected: those can't parse (the text rules decide for them)
        if !shape(script).parsed && script.chars().count() < 300 {
            unparsed.push(script.chars().take(100).collect::<String>());
        }
        match script_card_refusal(script, Some(&checkout)) {
            None => {}
            Some(text) if text == SCRIPT_FILLS_ON_CARDS_REFUSED || text == SCRIPT_CARD_REFUSED => filled += 1,
            Some(text) => other.push(format!("{} => {}", script.chars().take(120).collect::<String>(), &text[..70])),
        }
    }
    println!("{} scripts; on a checkout {filled} refused for filling and sending (or a card field)", scripts.len());
    // one of the agents' own scripts has a parenthesis too many: it ran nowhere either
    assert!(unparsed.len() <= 1, "not parsed:\n{}", unparsed.join("\n"));
    assert!(other.is_empty(), "refused on a checkout for something else:\n{}", other.join("\n"));
    assert!(SCRIPT_FILLS_ON_CARDS_REFUSED.contains("type_text") && SCRIPT_FILLS_ON_CARDS_REFUSED.contains("click_element"));
}
