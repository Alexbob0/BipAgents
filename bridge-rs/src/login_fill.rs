//! A password the vault made, typed into the agent's visible browser by the bridge over Chrome's DevTools protocol
//! (card_fill.rs' connection and page): from the bridge's memory to the page's password fields and nowhere else (not
//! the agent, not its Hermes, not a log).
//!
//! Only fields of type « password » are touched: the one the agent focused (in whichever frame it is) and the password
//! field after it on the same form, its confirmation, never one asking for the current password (`assets/login_fill.js`,
//! in an isolated world of each frame). Before each keystroke the bridge checks, in the page, that the focused element
//! is still that password field; else nothing is typed there.

use serde_json::{json, Value};

use crate::card_fill::{FillError, Page};

const SCRIPT: &str = include_str!("../assets/login_fill.js");
const WORLD: &str = "bipagents-login";

/// The password fields found in one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Fields {
    pub session: String,
    pub context: i64,
    pub frame_url: String,
    /// The agent's focus is on the first one.
    pub focused: bool,
    /// Their indexes in the script's list (one or two).
    pub indexes: Vec<u64>,
}

/// Where to type: the frame whose password field has the focus, else the one frame with a sign-up form's password
/// fields. None: no password field to type into (or several forms, none focused).
pub async fn find(page: &mut Page) -> Result<Option<Fields>, FillError> {
    let mut focused = Vec::new();
    let mut fallback = Vec::new();
    for (session, frame_id, frame_url) in page.frames().await? {
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
        let fields = Fields { session, context, frame_url, focused: is_focused, indexes };
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

/// A function run on the field itself, its answer (`true`/`false`); `argument` given as a call argument, never in
/// the expression's text.
async fn on_field(page: &mut Page, fields: &Fields, object: &str, function: &str, argument: Option<&str>) -> bool {
    let mut params = json!({"objectId": object, "functionDeclaration": function, "returnByValue": true});
    if let Some(value) = argument {
        params["arguments"] = json!([{ "value": value }]);
    }
    let Ok(result) = page.cdp.call("Runtime.callFunctionOn", params, Some(&fields.session)).await else { return false };
    result.get("result").and_then(|r| r.get("value")).and_then(Value::as_bool) == Some(true)
}

const IS_PASSWORD: &str = "function () { return this.tagName === 'INPUT' && (this.getAttribute('type') || '').toLowerCase() === 'password'; }";
const FOCUSED_PASSWORD: &str = "function () { return this.tagName === 'INPUT' && (this.getAttribute('type') || '').toLowerCase() === 'password' && this.getRootNode().activeElement === this; }";
const HOLDS: &str = "function (v) { return this.tagName === 'INPUT' && (this.getAttribute('type') || '').toLowerCase() === 'password' && this.value === v; }";
const SET: &str = "function (v) { const p = globalThis.__bipLogin; return p.set(p.els.indexOf(this), v); }";

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
        if !on_field(page, fields, &object, IS_PASSWORD, None).await {
            continue; // never anything but a password field
        }
        let _ = page.cdp.call("DOM.focus", json!({"objectId": object}), Some(&fields.session)).await;
        // Typed as a person would only while that very field has the focus.
        if on_field(page, fields, &object, FOCUSED_PASSWORD, None).await {
            let typed = page.cdp.call("Input.insertText", json!({"text": password}), Some(&fields.session)).await;
            if typed.is_err() && fields.session != page.session {
                let own = page.session.clone();
                if on_field(page, fields, &object, FOCUSED_PASSWORD, None).await {
                    let _ = page.cdp.call("Input.insertText", json!({"text": password}), Some(&own)).await;
                }
            }
        }
        let mut ok = on_field(page, fields, &object, HOLDS, Some(password)).await;
        if !ok {
            // set the way a framework sees it (the script refuses anything but a password field)
            on_field(page, fields, &object, SET, Some(password)).await;
            ok = on_field(page, fields, &object, HOLDS, Some(password)).await;
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
