//! Fourth review of card-guards (523e46524), 14/11. Never pushed.
//! rz3: what oxc can't parse among lines V8 runs (each falls to the text rules), printed; rz4: the skill's own scripts
//! and the reviewer's ordinary scripts are not refused; rz5: parse time of a large script.

use bipbridge::browser_mcp::script_card_refusal;
use bipbridge::script_shape::shape;
use serde_json::json;

const POLL: &str = "const tick = () => { const f = document.querySelector('#later input'), b = document.querySelector('#later button'); if (f && b && !f.value) { f.value = ['4242', '4242', '4242', '4242'].join(''); b.click(); } else scheduler.postTask(tick, {delay: 100}); }; tick(); 'ok'";

#[test]
fn rz3_a_script_v8_runs_is_never_judged_by_the_weaker_text_rules() {
    let ordinary = json!({"pw": {}, "known": [], "boxes": [], "cards": {}, "cardtext": false, "frames": 0});
    let mut weak = Vec::new();
    for prefix in ["<!-- a note\n", "/* */ --> a note\n", "if (0) function f1() {}\n", "lbl: function f2() {}\n",
                   "with ({}) {}\n", "010;\n", "var yield = 1;\n", "x = /(?i:a)/;\n"] {
        let script = format!("{prefix}{POLL}");
        let parsed = shape(&script).parsed;
        let refused = script_card_refusal(&script, Some(&ordinary));
        println!("{prefix:?}: parsed={parsed} refused={refused:?}");
        if refused.is_none() {
            weak.push(format!("{prefix:?} (parsed: {parsed})"));
        }
    }
    assert!(weak.is_empty(), "a poll that fills and clicks later, not refused after: {weak:?}");
}

#[test]
fn rz4_reading_scripts_of_the_skill_and_ordinary_ones_go_on() {
    let ordinary = json!({"pw": {}, "known": [], "boxes": [], "cards": {}, "cardtext": false, "frames": 0});
    let around = |word: &str| bipbridge::browser_mcp::AROUND_A_WORD.replace("word", word);
    let mut scripts = vec![around("observer"), around("postMessage"), around("setTimeout"), around("eval")];
    scripts.extend([
        "return ['Prix : ' + document.querySelector('.price').innerText, 'Stock : ' + document.querySelector('.stock').innerText].join('\\n')",
        "return document.querySelector('[onclick=\"addToCart(42)\"]').innerText",
        "return JSON.parse(atob(document.getElementById('product-data').textContent)).price",
        "document.querySelector('#onetrust-accept-btn-handler').click()",
        "window.scrollTo(0, document.body.scrollHeight); document.querySelector('.load-more').click()",
        "return (async () => { const r = await fetch('/api/stock?id=42'); return (await r.json()).count; })()",
        "return [...document.querySelectorAll('tr')].map((r) => [...r.cells].map((c) => c.innerText))",
    ].map(String::from));
    let refused: Vec<String> = scripts.iter()
        .filter_map(|s| script_card_refusal(s, Some(&ordinary)).map(|why| format!("{} => {}", s.chars().take(110).collect::<String>(), &why[..60])))
        .collect();
    assert!(refused.is_empty(), "refused on an ordinary page:\n{}", refused.join("\n"));
}

#[test]
fn rz5_a_large_script_is_read_quickly() {
    let one = "document.querySelectorAll('.item').forEach((e) => { const t = e.innerText; if (t.length > 3) console.log(t); });\n";
    for size in [10_000usize, 1_000_000] {
        let script = one.repeat(size / one.len() + 1);
        let started = std::time::Instant::now();
        let _ = script_card_refusal(&script, None);
        let took = started.elapsed();
        println!("{} bytes: {:?}", script.len(), took);
        assert!(took.as_secs_f64() < 2.0, "{} bytes took {took:?}", script.len());
    }
}
