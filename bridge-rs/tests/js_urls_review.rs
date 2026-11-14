//! Independent review of refuse-javascript-urls (88f6faaff), 14/11, kept as written. rj2 failed on 88f6faaff
//! (blob:, filesystem:); rj1 and rj3 passed.

use bipbridge::browser_mcp::refused_address;
use serde_json::{json, Map, Value};

fn args(url: &str) -> Map<String, Value> {
    json!({"instance_id": "i", "url": url}).as_object().cloned().unwrap()
}

/// Every spelling Chrome's URL parser turns into a javascript: / data: address (it trims C0 controls and spaces at
/// both ends, drops tabs and newlines anywhere, ignores the scheme's case).
#[test]
fn rj1_every_spelling_chrome_runs_is_refused() {
    for url in [
        "\n\tjavascript:void(0)",
        "\r\njavascript:void(0)",
        "javascript\t:void(0)",
        "JAVASCRIPT:void(0)",
        "\u{0}javascript:void(0)",
        "\u{1f}\u{b}javascript:void(0)",
        "jav\nascript:void(0)",
        "j\ra\tvascript:void(0)",
        " VbScRiPt:msgbox(1)",
        "\tdata:,x",
    ] {
        for tool in ["navigate", "new_tab"] {
            assert!(refused_address(tool, &args(url)).is_some(), "{tool} {url:?} not refused");
        }
    }
}

/// Addresses that open a document of the page's own origin made by a script (`URL.createObjectURL`,
/// `webkitRequestFileSystem`): its scripts run at load, with none of the guard's checks (navigate is no guarded tool).
#[test]
fn rj2_an_address_to_a_document_a_script_made_is_refused() {
    for url in [
        "blob:http://127.0.0.1:8123/0b7e3a52-49a5-4a39-9d1c-6c1b8d0f4c11",
        " BLOB:https://shop.example/0b7e3a52-49a5-4a39-9d1c-6c1b8d0f4c11",
        "filesystem:https://shop.example/temporary/p.html",
    ] {
        for tool in ["navigate", "new_tab"] {
            assert!(refused_address(tool, &args(url)).is_some(), "{tool} {url:?} not refused");
        }
    }
}

/// Web addresses go on, whatever words they hold.
#[test]
fn rj3_web_addresses_still_open() {
    for url in [
        "https://shop.example/javascript:void(0)",
        "https://javascript.info/",
        "https://www.data.gouv.fr/fr/datasets/",
        "http://localhost:8080/",
        "about:blank",
        "shop.example/data:x",
    ] {
        assert!(refused_address("navigate", &args(url)).is_none(), "{url:?} refused");
    }
}
