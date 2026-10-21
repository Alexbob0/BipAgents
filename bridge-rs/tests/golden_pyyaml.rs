//! agents_admin::pyyaml against PyYAML's own results (tests/golden/pyyaml.json, made by gen_pyyaml.py): safe_load's
//! values and safe_dump(allow_unicode=True, sort_keys=False)'s text, byte for byte.

use bipbridge::agents_admin::pyyaml::{dump, dump_json, load, to_json};
use serde_json::Value;

#[test]
fn load_and_dump_match_pyyaml() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("golden/pyyaml.json")).unwrap();
    let mut failures = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let expected = case["output"].as_str().unwrap();
        if case["kind"] == "text" {
            let text = case["input"].as_str().unwrap();
            let data = match load(text) {
                Ok(data) => data,
                Err(e) => {
                    failures.push(format!("case {i}: load failed: {e}"));
                    continue;
                }
            };
            if to_json(&data) != case["json"] {
                failures.push(format!("case {i}: loaded\n  got      {}\n  expected {}", to_json(&data), case["json"]));
            }
            let got = dump(&data);
            if got != expected {
                failures.push(format!("case {i}: dump\n  got      {got:?}\n  expected {expected:?}"));
            }
        } else {
            let got = dump_json(&case["input"]);
            if got != expected {
                failures.push(format!("case {i}: dump\n  got      {got:?}\n  expected {expected:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
