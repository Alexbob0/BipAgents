//! Review of vault-no-readback (16/11): what the selector rule lets through. A hidden field's « value » attribute is
//! its value (the guard can't empty it before a tool, as it does a mirrored attribute), and click_element's
//! `text_match` is a DevTools search (DOM.performSearch: text, CSS selector and XPath) the real server runs as is. The
//! Chrome tests are bridge/tests_blackbox/test_review_vault_no_readback_chrome.py.

use bipbridge::browser_mcp::{call_selectors, selector_tests_value};
use serde_json::json;

#[test]
fn an_xpath_doing_sums_on_the_value_attribute_is_refused() {
    // a numeric code (a bank's, a PIN) one digit per call: no starts-with, no comparison, no function the rule knows
    for selector in [
        "//input[floor(@value div 100000)=4]",
        "//input[@value mod 10 = 5]",
        "//input[round(@value div 1000) = 483]",
        "//input[ceiling(@value div 100000)=5]",
        "//input[floor(attribute::value div 10) mod 10 = 1]",
    ] {
        assert!(selector_tests_value(selector), "not refused: {selector}");
    }
}

#[test]
fn click_elements_text_match_is_checked_as_a_selector() {
    // nodriver's tab.find(text_match): DOM.performSearch takes it as a CSS selector and as an XPath too
    for text_match in [r#"body:has(input[value^="WnE3"]) h1"#, "//h1[//input[starts-with(@value,'WnE3')]]"] {
        let args = json!({"instance_id": "i", "selector": "h1", "text_match": text_match});
        let args = args.as_object().unwrap();
        assert!(call_selectors(args).iter().any(|s| selector_tests_value(s)), "not checked: {text_match}");
    }
}
