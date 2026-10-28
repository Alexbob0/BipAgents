//! A password the vault made, typed into the agent's visible browser by the bridge over Chrome's DevTools protocol
//! (card_fill.rs' connection and page): from the bridge's memory to the page's password fields and nowhere else (not
//! the agent, not its Hermes, not a log).
//!
//! Only fields of type « password » are touched: the one the agent focused (in whichever frame it is) and the password
//! field after it on the same form, its confirmation, never one asking for the current password (`assets/login_fill.js`,
//! in an isolated world of each frame). Before each keystroke the bridge checks, in the page, that the focused element
//! is still that password field; else nothing is typed there.
//!
//! Only on the site approved (2026-10-28, anti-phishing): a frame is looked at only when its own address (Chrome's
//! frame tree, which a page cannot fake) has exactly the approved origin, so a password field in a frame of another
//! site (an ad, a widget, a look-alike) is never typed into, even when the agent focused it on the right page. The
//! same origin is checked again on the field itself, in its frame, right before each keystroke and each fallback
//! write.

use serde_json::{json, Value};

use crate::card_fill::{FillError, Page};

const SCRIPT: &str = include_str!("../assets/login_fill.js");
const WORLD: &str = "bipagents-login";

/// The password fields found in one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Fields {
    /// The origin approved (`https://host[:port]`), checked again in the frame before each write.
    pub origin: String,
    pub session: String,
    pub context: i64,
    pub frame_url: String,
    /// The agent's focus is on the first one.
    pub focused: bool,
    /// Their indexes in the script's list (one or two).
    pub indexes: Vec<u64>,
}

/// A frame whose address (Chrome's own) is exactly on `origin`: scheme, host and port; never a subdomain, a parent
/// domain, plain http for https, nor a frame with no web address of its own (about:blank, srcdoc, data:).
pub fn frame_on(frame_url: &str, origin: &str) -> bool {
    frame_url.starts_with("http") && crate::vault::normalize_origin(frame_url).is_ok_and(|o| o == origin)
}

/// Where to type on `origin` (the site approved): the frame of that origin whose password field has the focus, else
/// the one such frame with a sign-up form's password fields. Frames of any other origin are left alone. None: no
/// password field to type into (or several forms, none focused).
pub async fn find(page: &mut Page, origin: &str) -> Result<Option<Fields>, FillError> {
    let mut focused = Vec::new();
    let mut fallback = Vec::new();
    for (session, frame_id, frame_url) in page.frames().await? {
        if !frame_on(&frame_url, origin) {
            continue; // another site's frame: never looked at, never typed into
        }
        let world = json!({"frameId": frame_id, "worldName": WORLD, "grantUniveralAccess": false});
        let Ok(created) = page.cdp.call("Page.createIsolatedWorld", world, Some(&session)).await else { continue };
        let Some(context) = created.get("executionContextId").and_then(Value::as_i64) else { continue };
        let expression = format!("{SCRIPT}\n;globalThis.__bipLogin.detect()");
        let Ok(found) = page.cdp.evaluate(&session, context, &expression).await else { continue };
        let indexes: Vec<u64> = found
            .get("fields")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter(|f| f.get("type").and_then(Value::as_str) == Some("password"))
                    .filter_map(|f| f.get("i").and_then(Value::as_u64))
                    .take(2)
                    .collect()
            })
            .unwrap_or_default();
        if indexes.is_empty() {
            continue;
        }
        let is_focused = found.get("focused").and_then(Value::as_bool) == Some(true);
        let fields = Fields { origin: origin.to_string(), session, context, frame_url, focused: is_focused, indexes };
        if is_focused {
            focused.push(fields);
        } else if !fallback.iter().any(|f: &Fields| f.frame_url == fields.frame_url && f.session == fields.session) {
            fallback.push(fields);
        }
    }
    if let Some(first) = focused.into_iter().next() {
        return Ok(Some(first));
    }
    Ok((fallback.len() == 1).then(|| fallback.remove(0)))
}

async fn object(page: &mut Page, fields: &Fields, index: u64) -> Option<String> {
    let result = page
        .cdp
        .call(
            "Runtime.evaluate",
            json!({"expression": format!("globalThis.__bipLogin.el({index})"), "contextId": fields.context}),
            Some(&fields.session),
        )
        .await
        .ok()?;
    result.get("result").and_then(|r| r.get("objectId")).and_then(Value::as_str).map(str::to_string)
}

/// A function run on the field itself, its answer (`true`/`false`); `arguments` given as call arguments, never in
/// the expression's text.
async fn on_field(page: &mut Page, fields: &Fields, object: &str, function: &str, arguments: &[&str]) -> bool {
    let mut params = json!({"objectId": object, "functionDeclaration": function, "returnByValue": true});
    if !arguments.is_empty() {
        params["arguments"] = Value::Array(arguments.iter().map(|v| json!({ "value": v })).collect());
    }
    let Ok(result) = page.cdp.call("Runtime.callFunctionOn", params, Some(&fields.session)).await else { return false };
    result.get("result").and_then(|r| r.get("value")).and_then(Value::as_bool) == Some(true)
}

const IS_PASSWORD: &str = "function () { return this.tagName === 'INPUT' && (this.getAttribute('type') || '').toLowerCase() === 'password'; }";
/// Focused, and its frame still on the approved origin (`o`): checked right before each keystroke.
const FOCUSED_PASSWORD: &str = "function (o) { return this.tagName === 'INPUT' && (this.getAttribute('type') || '').toLowerCase() === 'password' && this.getRootNode().activeElement === this && this.ownerDocument.location.origin === o; }";
const HOLDS: &str = "function (v) { return this.tagName === 'INPUT' && (this.getAttribute('type') || '').toLowerCase() === 'password' && this.value === v; }";
const SET: &str = "function (v, o) { if (this.ownerDocument.location.origin !== o) return false; const p = globalThis.__bipLogin; return p.set(p.els.indexOf(this), v); }";

/// The password typed into each field (the first, then its confirmation): how many hold it.
pub async fn fill(page: &mut Page, fields: &Fields, password: &str) -> Result<usize, FillError> {
    let mut filled = 0;
    for &index in &fields.indexes {
        let prepared = page
            .cdp
            .evaluate(&fields.session, fields.context, &format!("globalThis.__bipLogin.prepare({index})"))
            .await?;
        if prepared != json!(true) {
            continue;
        }
        let Some(object) = object(page, fields, index).await else { continue };
        if !on_field(page, fields, &object, IS_PASSWORD, &[]).await {
            continue; // never anything but a password field
        }
        let _ = page.cdp.call("DOM.focus", json!({"objectId": object}), Some(&fields.session)).await;
        // Typed as a person would only while that very field has the focus.
        if on_field(page, fields, &object, FOCUSED_PASSWORD, &[&fields.origin]).await {
            let typed = page.cdp.call("Input.insertText", json!({"text": password}), Some(&fields.session)).await;
            if typed.is_err() && fields.session != page.session {
                let own = page.session.clone();
                if on_field(page, fields, &object, FOCUSED_PASSWORD, &[&fields.origin]).await {
                    let _ = page.cdp.call("Input.insertText", json!({"text": password}), Some(&own)).await;
                }
            }
        }
        let mut ok = on_field(page, fields, &object, HOLDS, &[password]).await;
        if !ok {
            // set the way a framework sees it (the script refuses anything but a password field)
            on_field(page, fields, &object, SET, &[password, &fields.origin]).await;
            ok = on_field(page, fields, &object, HOLDS, &[password]).await;
        }
        let _ =
            page.cdp.evaluate(&fields.session, fields.context, &format!("globalThis.__bipLogin.blur({index})")).await;
        if ok {
            filled += 1;
        }
    }
    Ok(filled)
}

/// The fields emptied (typing went wrong): nothing of the password stays in the page.
pub async fn clear(page: &mut Page, fields: &Fields) {
    for &index in &fields.indexes {
        let _ = page
            .cdp
            .evaluate(&fields.session, fields.context, &format!("globalThis.__bipLogin.set({index}, '')"))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::frame_on;

    #[test]
    fn only_frames_of_the_approved_origin_are_typed_into() {
        let origin = "https://shop.example";
        assert!(frame_on("https://shop.example/signup?x=1#y", origin));
        assert!(frame_on("https://SHOP.example:443/signup", origin));
        for other in [
            "https://www.shop.example/signup",       // a subdomain
            "https://example/signup",                // a parent domain
            "http://shop.example/signup",            // plain http
            "https://shop.example:8443/signup",      // another port
            "https://shop.example.evil.test/signup", // a look-alike
            "https://shop-example.test/signup",
            "https://sh0p.example/signup",
            "https://ads.thirdparty.test/frame",     // another site's frame
            "about:blank",
            "about:srcdoc",
            "data:text/html,<input type=password>",
            "",
        ] {
            assert!(!frame_on(other, origin), "{other}");
        }
    }
}
