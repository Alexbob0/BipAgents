//! Fourth review of card-guards (523e46524): code run from text through a tagged template (`` setTimeout`…` `` calls
//! setTimeout(['…'], …): its text runs later, unread by the guard; on Chrome 157 it filled the card form drawn next and
//! pressed « Payer », test_rz_card_guards_chrome.py). Each must be refused like eval, Function or a timer given text.
//! Never pushed.

use bipbridge::browser_mcp::script_card_refusal;
use serde_json::json;

#[test]
fn rz8_code_from_text_through_a_tagged_template_is_refused() {
    let ordinary = json!({"pw": {}, "known": [], "boxes": [], "cards": {}, "cardtext": false, "frames": 0});
    let mut passed = Vec::new();
    for script in [
        "setTimeout`document.title = 1 //${1500}`; 'ok'",
        "setInterval`document.title = 1 //${1500}`; 'ok'",
        "window.setTimeout`document.title = 1 //${1500}`; 'ok'",
        "(0, setTimeout)`document.title = 1 //${1500}`; 'ok'",
        "Function`document.title = 1`(); 'ok'",
        "Function`scheduler.postTask(() => { document.querySelector('#later button').click(); }, {delay: 1500})`(); 'ok'",
    ] {
        if script_card_refusal(script, Some(&ordinary)).is_none() {
            passed.push(script);
        }
    }
    assert!(passed.is_empty(), "not refused: {passed:#?}");
}
