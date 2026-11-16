//! Hotfix of 15/11 on production (main 2f755bb74), after the seventh review of card-guards tried its attacks there
//! (test_card_guard_review7_main_chrome.py): `location` reached under another name or in brackets escaped the text
//! rule (MAKES_WAY) and a `javascript:` address was never looked at; a script on the card form itself typed a number
//! and pressed « Payer »; a reader kept in the site's storage came back. Here: `location` in any form but a read of one
//! of its parts, a `javascript:` address however written, the site's storage written, are refused everywhere; where a
//! page shows a card's field, no script runs at all.

use bipbridge::browser_mcp::{
    script_card_refusal, PROVIDER_SCRIPT_REFUSED, SCRIPT_ON_CARDS_REFUSED, SCRIPT_STORAGE_REFUSED, SCRIPT_TEXT_CODE_REFUSED,
};
use bipbridge::script_shape::shape;
use serde_json::json;

fn ordinary() -> serde_json::Value {
    json!({"pw": {}, "known": [], "boxes": [], "cards": {}, "cardtext": false, "frames": 0})
}

/// `javascript:void(0)` in base64, given at run time: no `javascript:` in the script's text.
const U: &str = "atob(document.body.dataset.u)";

fn refused_as(scripts: &[String], text: &str) {
    let passed: Vec<String> = scripts
        .iter()
        .filter(|s| script_card_refusal(s, Some(&ordinary())) != Some(text))
        .map(|s| format!("{s} => {:?}", script_card_refusal(s, Some(&ordinary())).map(|t| &t[..60])))
        .collect();
    assert!(passed.is_empty(), "not refused as expected ({}):\n{}", passed.len(), passed.join("\n"));
}

#[test]
fn location_in_any_form_but_a_read_of_one_of_its_parts_is_refused() {
    let forms = [
        "(location).replace(U)",
        "location.replace(U)",
        "location.assign(U)",
        "location.reload()",
        "location = U",
        "location.href = U",
        "location['href'] = U",
        "[location.href] = [U]",
        "const l = location; l.replace(U)",
        "let l; l = location; l.assign(U)",
        "[location][0].replace(U)",
        "(0, location).replace(U)",
        "window.location.replace(U)",
        "self.location.assign(U)",
        "globalThis.location.href = U",
        "top.location = U",
        "parent.location.replace(U)",
        "document.location.replace(U)",
        "document.defaultView.location.replace(U)",
        "this.location.replace(U)",
        "window['location'].replace(U)",
        "window[`location`].replace(U)",
        "window['loc' + 'ation'].replace(U)",
        "const k = 'loc'; window[k + 'ation'].replace(U)",
        "const {location: l} = window; l.replace(U)",
        "const {location} = window; location.replace(U)",
        "let l; ({location: l} = document); l.assign(U)",
        "var location = U",
        "function go(location) { location.replace(U) } go(document.location)",
        "Object.values(window).find((v) => v && v.ancestorOrigins).replace(U)",
        "Object.values(document).find((v) => v && v.ancestorOrigins).replace(U)",
        "Object.getOwnPropertyDescriptor(document, 'location').get.call(document).replace(U)",
        "Reflect.get(window, 'location').replace(U)",
        "const w = window; w.location.replace(U)",
        "const d = document.defaultView; d.location.replace(U)",
        "const o = {...document}; o.location.replace(U)",
        "with (document) { location.replace(U) }",
        "const f = (x) => x.replace(U); f(document.location)",
        "new Function('return this')().location.replace(U)",
        "(function () { return this })().location.replace(U)",
        "const r = location.replace; r.call(location, U)",
        "navigation.navigate(U)",
        "window.navigation.navigate(U)",
    ];
    let scripts: Vec<String> = forms.iter().map(|f| f.replace('U', U)).collect();
    let passed: Vec<String> = scripts
        .iter()
        .filter(|s| script_card_refusal(s, Some(&ordinary())).is_none())
        .cloned()
        .collect();
    assert!(passed.is_empty(), "not refused ({}):\n{}", passed.len(), passed.join("\n"));
    let not_ways: Vec<&String> =
        scripts.iter().filter(|s| shape(s).parsed && !shape(s).makes_way && !shape(s).code_from_text && !shape(s).hidden_names).collect();
    assert!(not_ways.is_empty(), "not taken for ways out: {not_ways:#?}");
}

#[test]
fn a_javascript_address_however_written_is_refused() {
    let scripts: Vec<String> = [
        "document.querySelector('a').dataset.go = 'javascript:void(0)'",
        "document.querySelector('a').dataset.go = 'JavaScript:void(0)'",
        "document.querySelector('a').dataset.go = ' java\\tscript:void(0)'",
        "document.querySelector('a').dataset.go = '\\x6aavascript:void(0)'",
        "document.querySelector('a').dataset.go = `javascript:void(0)`",
        "document.querySelector('a').dataset.go = 'java' + 'script:void(0)'",
        "const x = document.title; document.querySelector('a').dataset.go = x + 'java' + 'script:' + x",
        "document.querySelector('a').dataset.go = `java${''}script:void(0)`",
        "document.querySelector('a').dataset.go = atob('amF2YXNjcmlwdDp2b2lkKDAp')",
        "document.querySelector('a').dataset.go = atob('amF2YXNjcmlwdDp2b2lkKDAp\\n')",
        "document.querySelector('a').dataset.go = decodeURIComponent('%6Aavascript%3Avoid(0)')",
        "document.querySelector('a').dataset.go = unescape('%6A%61vascript:void(0)')",
        "document.querySelector('a').dataset.go = String.fromCharCode(106, 97, 118, 97, 115, 99, 114, 105, 112, 116, 58)",
        "document.querySelector('a').dataset.go = ['java', 'script:'].join('')",
        "document.querySelector('a').dataset.go = 'java'.concat('script:')",
        "document.querySelector('a').dataset.go = 'jaXvascript:'.replace('X', '')",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    refused_as(&scripts, PROVIDER_SCRIPT_REFUSED);
}

#[test]
fn the_sites_storage_written_is_refused() {
    let scripts: Vec<String> = [
        "localStorage.setItem('k', 'v')",
        "sessionStorage.setItem('wc_fragments', '<img src=x onerror=1>')",
        "window.localStorage.setItem('k', 'v')",
        "localStorage.removeItem('k')",
        "sessionStorage.clear()",
        "localStorage.k = 'v'",
        "localStorage['k'] = 'v'",
        "const s = localStorage; s.setItem('k', 'v')",
        "document.cookie = 'a=b'",
        "const d = document; d.cookie = 'a=b'",
        "window.name = 'x'",
        "self.name = 'x'",
        "name = 'x'",
        "var name = 'x'",
        "history.pushState({}, '', '/x')",
        "history.replaceState({r: 1}, '')",
        "indexedDB.deleteDatabase('x')",
        "caches.delete('x')",
        "cookieStore.set('a', 'b')",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    refused_as(&scripts, SCRIPT_STORAGE_REFUSED);
}

#[test]
fn other_ways_into_the_page_or_out_of_it_are_refused() {
    let ways: Vec<String> = [
        "const w = window; w.open('about:blank')",
        "window.open.bind(window)('about:blank')",
        "document.write('<p>x</p>')",
        "const d = document; d.write('<b>x</b>')",
        "document['writeln']('<b>x</b>')",
        "document.querySelector('iframe').src = U",
        "document.querySelector('object').data = U",
        "Object.assign(document.querySelector('a'), {href: U}); document.querySelector('a').click()",
        "document.querySelector('a').attributes.href.value = U",
        "document.querySelector('a').attributes[0].value = U",
        "document.querySelector('a').getAttributeNode('href').value = U",
        "const a = document.createAttribute('onclick'); a.value = U; document.body.setAttributeNode(a)",
        "document.querySelector('svg a').href.baseVal = U",
        "document.body.insertAdjacentHTML('beforeend', '<img src=x>')",
        "const p = new DOMParser(); p.parseFromString('<p>', 'text/html')",
        "location.reload()",
        "const {sessionStorage: s} = window; s.setItem('k', 'v')",
        "indexedDB.open('x')",
        "caches.open('x')",
    ]
    .iter()
    .map(|s| s.replace('U', U))
    .collect();
    let passed: Vec<&String> = ways.iter().filter(|s| script_card_refusal(s, Some(&ordinary())).is_none()).collect();
    assert!(passed.is_empty(), "not refused: {passed:#?}");
    let script = "const s = document.createElement('script'); s.textContent = document.title; document.head.append(s)";
    assert_eq!(script_card_refusal(script, Some(&ordinary())), Some(SCRIPT_TEXT_CODE_REFUSED));
    let script = "const s = document.createElement('SCRIPT'); s.text = document.title; document.body.appendChild(s)";
    assert_eq!(script_card_refusal(script, Some(&ordinary())), Some(SCRIPT_TEXT_CODE_REFUSED));
}

#[test]
fn reads_of_the_address_and_of_the_page_go_on() {
    let reads = [
        "return location.href",
        "return window.location.pathname",
        "return document.location.hostname.includes('shop')",
        "return new URL(location.href).searchParams.get('q')",
        "return JSON.stringify({url: location.href, title: document.title, text: document.body.innerText.replace(/\\s+/g, ' ').slice(0, 600)})",
        "(()=>{const t=[...document.querySelectorAll('.error,.alert,[role=alert]')].map(e=>e.innerText.trim()).filter(Boolean);return JSON.stringify({url:location.pathname,errs:t});})()",
        "return [...document.querySelectorAll('a')].map((a) => ({text: a.innerText, href: a.href}))",
        "const t = document.title; return t.replace('x', 'y')",
        "const t = document.title.trim(); return t.replace(/x/g, 'y')",
        "return (document.querySelector('h1').textContent || '').replace(/\\s+/g, ' ')",
        "return [...document.querySelectorAll('li')].map((li) => li.textContent.replace(/\\s+/g, ' ').trim())",
        "const norm = (s) => s.replace(/\\s+/g, ' ').trim(); return norm(document.title)",
        "return localStorage.getItem('k')",
        "return localStorage.length",
        "return document.cookie",
        "return typeof window",
        "return window.scrollY",
        "return window.innerWidth > 800",
        "return getComputedStyle(document.body).color",
        "return document.body.getBoundingClientRect().top",
        "const {top, left} = document.body.getBoundingClientRect(); return {top, left}",
        "(function () { return document.title; }).call(this)",
        "return Object.assign({}, {a: 1})",
        "const out = {}; out.src = document.querySelector('img').src; out.data = 'x'; return out",
        "return document.evaluate('//button', document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue?.innerText",
        "return document.URL",
        "return window.name",
        "document.querySelector('#q').value = 'chaussures'; document.querySelector('#go').click()",
    ];
    let refused: Vec<String> = reads
        .iter()
        .filter_map(|s| script_card_refusal(s, Some(&ordinary())).map(|t| format!("{s} => {}", &t[..70])))
        .collect();
    assert!(refused.is_empty(), "refused where no card is ({}):\n{}", refused.len(), refused.join("\n"));
}

#[test]
fn where_a_page_shows_a_cards_field_no_script_runs() {
    let cards = json!({"pw": {}, "known": [], "boxes": [], "cards": {"#cc": "811c9dc5"}, "cardtext": false, "frames": 0});
    let frames = json!({"pw": {}, "known": [], "boxes": [], "cards": {}, "cardtext": false, "frames": 1});
    for script in ["return document.title", "return location.href", "return 1 + 1", "document.querySelector('#go').click()"] {
        assert_eq!(script_card_refusal(script, Some(&cards)), Some(SCRIPT_ON_CARDS_REFUSED), "{script}");
        assert_eq!(script_card_refusal(script, Some(&frames)), Some(SCRIPT_ON_CARDS_REFUSED), "{script}");
        // the page couldn't say: as if it showed one
        assert_eq!(script_card_refusal(script, None), Some(SCRIPT_ON_CARDS_REFUSED), "{script}");
        assert_eq!(script_card_refusal(script, Some(&ordinary())), None, "{script}");
    }
    assert!(SCRIPT_ON_CARDS_REFUSED.contains("get_page_content") && SCRIPT_ON_CARDS_REFUSED.contains("pay_with_card"));
}
