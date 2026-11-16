//! vault-no-readback (Alex, 16/11): what bip-browser-mcp decides without a page. A selector that tests part of a field's
//! « value » attribute is refused in every tool (a React page mirrors the password there: one character at a time, the
//! agent would read it all), CSS escapes and XPath included; a selector naming the attribute whole, or another
//! attribute, goes on. The scripts that hide a password field's value from the agent's script and put it back carry
//! what the proxy remembers of the page (fields seen as passwords, fingerprints), never a value. The Chrome tests are
//! bridge/tests_blackbox/test_vault_no_readback_chrome.py.

use bipbridge::browser_mcp::{
    conceal_script, guard_before_in, hide_script, selector_tests_value, shows_password_with, Secrets, RESTORE_SCRIPT,
    SELECTOR_VALUE_REFUSED,
};
use serde_json::json;

#[test]
fn a_selector_testing_part_of_the_value_attribute_is_refused() {
    for selector in [
        r#"input[value^="Zq7"]"#,
        r#"input[value*='4471']"#,
        r#"input[ value $= "1!" ]"#,
        r#"input[value~="a"]"#,
        r#"input[value|="a"]"#,
        r#"[VALUE^="Zq7"]"#,
        r#"input[*|value^="Z"]"#,
        r#"input[valu\65 ^="Zq"]"#,
        r#"input[\76 alue^="Zq"]"#,
        r#"input[\value^="Zq"]"#,
        r#"input[v\61 lue^="Zq"]"#,
        r#"input[value/**/^="Zq"]"#,
        r#"form:has(input[value^="Z"]) button"#,
        r#"input:not([value^="A"])"#,
        r#"input[value^="Z" i]"#,
        "//input[starts-with(@value,'Zq7')]",
        "//input[contains(@value,'4471')]",
        "//input[substring(@value,1,1)='Z']",
        "//input[string-length(@value)>10]",
        "//input[@value>5000]",
        "//*[@*[starts-with(.,'Zq7')]]",
        "(//input[contains(attribute::value,'Z')])[1]",
        "//input[translate(@value,'Z','z')='zq']",
    ] {
        assert!(selector_tests_value(selector), "not refused: {selector}");
    }
}

#[test]
fn ordinary_selectors_go_on() {
    for selector in [
        "#mdp",
        "input[type=password]",
        "input[name=login]",
        r#"input[value="Rechercher"]"#,
        r#"input[type=radio][value="2"]"#,
        "input[value]",
        r#"a[href^="https://"]"#,
        r#"a[href*="login"]"#,
        r#"[data-value^="M"]"#,
        r#"[aria-valuenow="3"]"#,
        "button.value-add",
        "#values > li",
        "//input[@value='OK']",
        "//button[contains(text(),'Se connecter')]",
        "//a[starts-with(@href,'/compte')]",
        "//input[@type='submit' and not(@disabled)]",
        "",
    ] {
        assert!(!selector_tests_value(selector), "refused: {selector}");
    }
    assert!(SELECTOR_VALUE_REFUSED.starts_with("Not done") && SELECTOR_VALUE_REFUSED.contains("Nothing was"));
}

#[test]
fn what_is_remembered_of_a_page_goes_to_its_scripts_never_a_value() {
    let mut secrets = Secrets::default();
    // a look found two password fields and a value's fingerprint, on one origin
    secrets
        .learn(&json!({"origin": "https://agences.example", "sk": ["#mdp", "INPUT|code|0"], "sh": ["1a2b3c"]}), 100.0);
    secrets.learn(&json!({"origin": "https://agences.example", "sk": ["#mdp"], "sh": []}), 200.0);
    secrets.learn(&json!({"origin": "https://autre.example", "sk": ["#pass"], "sh": ["ffff"]}), 200.0);
    let here = secrets.memory("https://agences.example", 300.0);
    assert_eq!(here, json!({"k": ["#mdp", "INPUT|code|0"], "h": ["1a2b3c"]}));
    // another origin's are not given; an unknown origin gets nothing
    assert_eq!(secrets.memory("https://autre.example", 300.0), json!({"k": ["#pass"], "h": ["ffff"]}));
    assert_eq!(secrets.memory("https://nulle-part.example", 300.0), json!({"k": [], "h": []}));
    // forgotten after a while
    assert_eq!(secrets.memory("https://agences.example", 100.0 + 4.0 * 3600.0), json!({"k": [], "h": []}));
    // bounded, and garbage in a look's answer ignored
    let mut many = Secrets::default();
    for i in 0..500 {
        many.learn(
            &json!({"origin": "https://a.example", "sk": [format!("#f{i}"), 3, null], "sh": [format!("{i:x}")]}),
            1.0,
        );
    }
    let kept = many.memory("https://a.example", 2.0);
    assert!(kept["k"].as_array().unwrap().len() <= 64 && kept["h"].as_array().unwrap().len() <= 64);
    assert!(kept["k"].as_array().unwrap().iter().all(|k| k.is_string()));
    many.learn(&json!("not an object"), 3.0);

    // the scripts get every origin's, and pick their page's own
    let all = secrets.all(300.0);
    assert_eq!(all["https://agences.example"], here);
    for script in [
        hide_script(&all),
        conceal_script(&all),
        shows_password_with("answer", &all),
        guard_before_in(&json!([]), &all),
    ] {
        assert!(script.contains("\"#mdp\"") && script.contains("\"1a2b3c\""), "{script}");
        assert!(script.starts_with("/* bipagents guard"), "{script}");
        assert!(script.contains("secMM[location.origin]"));
        // nothing of a value leaves the page: answers are counts, keys and fingerprints
        assert!(!script.contains("return e.value") && !script.contains("sv: "));
    }
    for script in [
        hide_script(&all),
        conceal_script(&all),
        shows_password_with("answer", &all),
        guard_before_in(&json!([]), &all),
        RESTORE_SCRIPT.to_string(),
    ] {
        let allocator = oxc_allocator::Allocator::default();
        let parsed = oxc_parser::Parser::new(&allocator, &script, oxc_span::SourceType::script()).parse();
        assert!(!parsed.diagnostics.has_errors(), "a guard script that doesn't parse: {}", &script[..60]);
    }
    assert!(RESTORE_SCRIPT.starts_with("/* bipagents guard"));
}
