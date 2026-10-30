//! souls against the Python's own results (tests/golden/souls.json, made by gen_souls.py).

use bipbridge::souls;
use serde_json::{json, Value};

fn opt(v: &Value) -> Option<&str> {
    v.as_str()
}

#[test]
fn souls_match_python() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("golden/souls.json")).unwrap();
    let mut failures = Vec::new();
    // GOLDEN_SOULS_OUT=<file>: what the Rust gives for each case, to rewrite the golden after a deliberate change
    // (scripts read it back: `json.dumps(cases, indent=1, ensure_ascii=False)`)
    let mut gots = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let input = &case["input"];
        let got: Value = match case["fn"].as_str().unwrap() {
            "agent_language" => json!(souls::agent_language(opt(&input[0]))),
            "language" => json!(souls::language(opt(&input[0]))),
            "language_block" => json!(souls::language_block(opt(&input[0]))),
            "templates" => json!(souls::templates(opt(&input[0]))),
            "split_language" => {
                let (body, lang) = souls::split_language(input[0].as_str().unwrap());
                json!([body, lang])
            }
            // Rust only (the Carnet, 2026-10-22): its cases added by hand
            "with_notebook" => json!(souls::with_notebook(input[0].as_str().unwrap(), input[1].as_bool().unwrap())),
            // Rust only (the archives, 2026-10-23): its cases added by hand
            "with_archives" => json!(souls::with_archives(input[0].as_str().unwrap(), input[1].as_bool().unwrap())),
            // Rust only (the health agent's rule, 2026-10-30: the body stays off « Toi »): its cases added by hand
            "with_health" => json!(souls::with_health(input[0].as_str().unwrap(), input[1].as_bool().unwrap())),
            // Rust only (the circle's common memory, 2026-10-30): its cases added by hand; input[1] null (not in the
            // circle), true (gives) or false (the health agent)
            "with_shared_memory" => json!(souls::with_shared_memory(input[0].as_str().unwrap(), input[1].as_bool())),
            "with_language" => json!(souls::with_language(input[0].as_str().unwrap(), opt(&input[1]))),
            // Rust only (the answer styles, 2026-10-30): its cases added by hand
            "with_style" => json!(souls::with_style(input[0].as_str().unwrap(), opt(&input[1]), opt(&input[2]))),
            "soul_for" => match souls::soul_for(input[0].as_str().unwrap(), input[1].as_str().unwrap(), opt(&input[2]))
            {
                Ok(s) => json!(s),
                Err(e) => json!({"error": e.to_string()}),
            },
            "custom_soul" => json!(souls::custom_soul(
                input[0].as_str().unwrap(),
                input[1].as_str().unwrap(),
                input[2].as_str().unwrap(),
                input[3].as_str().unwrap(),
                opt(&input[4])
            )),
            "profile_name" => {
                let taken: Vec<String> =
                    input[1].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
                json!(souls::profile_name(input[0].as_str().unwrap(), &taken))
            }
            other => panic!("unknown fn {other}"),
        };
        gots.push(got.clone());
        if got != case["output"] {
            failures
                .push(format!("case {i} {}({input}):\n  got      {got}\n  expected {}", case["fn"], case["output"]));
        }
    }
    if let Ok(out) = std::env::var("GOLDEN_SOULS_OUT") {
        std::fs::write(out, serde_json::to_string(&gots).unwrap()).unwrap();
        return;
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
