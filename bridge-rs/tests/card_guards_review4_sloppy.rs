//! Fourth review of card-guards (523e46524): more lines V8 runs in a classic script (sloppy mode, Annex B) that oxc
//! might not parse; each before rx4's poll, which must be refused whatever happens. Never pushed.

use bipbridge::browser_mcp::script_card_refusal;
use bipbridge::script_shape::shape;
use serde_json::json;

const POLL: &str = "const tick = () => { const f = document.querySelector('#later input'), b = document.querySelector('#later button'); if (f && b && !f.value) { f.value = ['4242', '4242', '4242', '4242'].join(''); b.click(); } else scheduler.postTask(tick, {delay: 100}); }; tick(); 'ok'";

#[test]
fn rz7_more_sloppy_lines_never_weaken_the_rules() {
    let ordinary = json!({"pw": {}, "known": [], "boxes": [], "cards": {}, "cardtext": false, "frames": 0});
    let mut weak = Vec::new();
    for prefix in ["for (var i0 = 0 in {});\n", "x1 = /]/;\n", "x2 = /{/;\n", "x3 = /\\c/;\n", "x4 = /a{1/;\n",
                   "x5 = '\\8';\n", "arguments = 1;\n", "eval = 1;\n", "function f3(a, a) {}\n", "delete x6;\n",
                   "x7 = 0o7 + 07;\n", "x8 = /(?<n>a)|(?<n>b)/;\n", "x9 = /[\\d-a]/;\n", "x10 = /\\p/;\n",
                   "label1: label2: function f4() {}\n", "x11 = 1 <!-- 2\n", "x12 = `\\unicode`;\n"] {
        let script = format!("{prefix}{POLL}");
        let parsed = shape(&script).parsed;
        let refused = script_card_refusal(&script, Some(&ordinary));
        println!("{prefix:?}: parsed={parsed} refused={}", refused.is_some());
        if refused.is_none() {
            weak.push(format!("{prefix:?} (parsed: {parsed})"));
        }
    }
    assert!(weak.is_empty(), "a poll that fills and clicks later, not refused after: {weak:?}");
}
