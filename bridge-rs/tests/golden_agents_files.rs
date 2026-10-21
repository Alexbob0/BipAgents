//! The files the agents group writes (profile.yaml, config.yaml, the bridge's config) against the Python's own output
//! (tests/golden/agents_files.json, made by gen_agents_files.py), byte for byte.

use bipbridge::agents_admin::{mark_as_bot, save_agent, set_model, set_thinking};
use bipbridge::config::{parse_config, AgentConfig, SharedConfig};
use indexmap::IndexMap;
use serde_json::{json, Value};

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

fn agent_of(arg: &Value) -> AgentConfig {
    let opt = |k: &str| text(&arg[k]);
    AgentConfig {
        name: opt("name").unwrap(),
        display_name: opt("display_name").unwrap(),
        hermes_url: opt("hermes_url").unwrap(),
        hermes_key: opt("hermes_key").unwrap(),
        public_url: opt("public_url"),
        upload_dir_host: opt("upload_dir_host"),
        upload_dir_container: opt("upload_dir_container"),
        isolated: arg["isolated"].as_bool().unwrap(),
        sandboxed: arg["sandboxed"].as_bool().unwrap(),
        python: arg["python"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect(),
        hermes_home: opt("hermes_home"),
        hermes_root: opt("hermes_root"),
        space: opt("space"),
        desktop_url: opt("desktop_url"),
        desktop_vnc: opt("desktop_vnc"),
        desktop_vnc_password_file: opt("desktop_vnc_password_file"),
        shares: opt("shares").unwrap_or_default(),
        ..AgentConfig::default()
    }
}

#[test]
fn written_files_match_the_python() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("golden/agents_files.json")).unwrap();
    let mut failures = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path();
        let input = text(&case["input"]);
        let expected = text(&case["output"]);
        let got: Option<String> = match case["fn"].as_str().unwrap() {
            "mark_as_bot" => {
                if let Some(t) = &input {
                    std::fs::write(tmp.join("profile.yaml"), t).unwrap();
                }
                mark_as_bot(tmp).ok().map(|_| std::fs::read_to_string(tmp.join("profile.yaml")).unwrap())
            }
            "set_thinking" => {
                let home = tmp.join("home");
                std::fs::create_dir_all(home.join("profiles/vie")).unwrap();
                std::fs::write(home.join("profiles/vie/config.yaml"), input.clone().unwrap()).unwrap();
                let config = parse_config(
                    &json!({"bridge_key": "b".repeat(40), "pocket": {"url": "http://p"},
                            "agents": {"vie": {"hermes_url": "http://h", "hermes_key": "k".repeat(32), "host_home": home.to_string_lossy()}}}),
                    None,
                )
                .unwrap();
                set_thinking(&config, config.agent(Some("vie")).unwrap(), case["arg"].as_str().unwrap()).unwrap();
                Some(std::fs::read_to_string(home.join("profiles/vie/config.yaml")).unwrap())
            }
            "set_model" => {
                let home = tmp.join("home");
                std::fs::create_dir_all(home.join("profiles/vie")).unwrap();
                std::fs::write(home.join("profiles/vie/config.yaml"), input.clone().unwrap()).unwrap();
                let config = parse_config(&json!({"bridge_key": "b".repeat(40), "pocket": {"url": "http://p"}, "hermes": {"home": home.to_string_lossy()}}), None).unwrap();
                let agent = AgentConfig {
                    name: "vie".into(),
                    display_name: "Vie".into(),
                    hermes_url: "http://h".into(),
                    hermes_key: "k".repeat(32),
                    ..AgentConfig::default()
                };
                set_model(&config, &agent, case["arg"][0].as_str().unwrap(), case["arg"][1].as_bool()).unwrap();
                Some(std::fs::read_to_string(home.join("profiles/vie/config.yaml")).unwrap())
            }
            "set_enabled" => {
                for name in ["plant-care", "monthly-budget"] {
                    std::fs::create_dir_all(tmp.join("skills").join(name)).unwrap();
                    std::fs::write(
                        tmp.join("skills").join(name).join("SKILL.md"),
                        format!("---\nname: {name}\ndescription: x\n---\n"),
                    )
                    .unwrap();
                }
                std::fs::write(tmp.join("config.yaml"), input.clone().unwrap()).unwrap();
                let changes: IndexMap<String, bool> =
                    case["arg"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_bool().unwrap())).collect();
                bipbridge::own_skills::set_enabled(tmp, &changes).unwrap();
                Some(std::fs::read_to_string(tmp.join("config.yaml")).unwrap())
            }
            "save_agent" => {
                let path = tmp.join("bridge.toml");
                std::fs::write(&path, input.clone().unwrap()).unwrap();
                let config = SharedConfig::new(
                    parse_config(
                        &json!({"bridge_key": "b".repeat(40), "pocket": {"url": "http://p"}}),
                        Some(&path.to_string_lossy()),
                    )
                    .unwrap(),
                );
                save_agent(&config, &agent_of(&case["arg"])).unwrap();
                Some(std::fs::read_to_string(&path).unwrap())
            }
            other => panic!("unknown fn {other}"),
        };
        if got != expected {
            failures.push(format!("case {i} ({}): got {got:?}\n  expected {expected:?}", case["fn"]));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
