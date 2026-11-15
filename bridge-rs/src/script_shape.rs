//! What an agent's script does, read from its syntax (oxc's JavaScript parser) rather than from its raw text (third
//! review of card-guards, 14/11). Raw text took a word in a string for code (« observer » searched for in a page, a
//! selector `[onclick="…"]`), refused an array of strings joined with `+`, and missed callbacks run later under names
//! no list knows (`scheduler.postTask`, `navigator.locks.request`, a media's `onended`…). Here:
//! - a write or a click (`acts`) on something not the script's own (`o.name = …` on `const o = {}` is its own);
//! - one that may run later (`acts_later`): inside a function given to anything but what calls it at once (an IIFE, the
//!   array methods `forEach`, `map`…, `new Promise`'s executor, `replace`'s and `JSON`'s callbacks), a function stored
//!   or passed by name, a method of the page taken as a value (`btn.click` given to a timer), or after an `await`;
//! - functions that run later at all (`later`), renames of fields or labels (`renames`), names made at run time
//!   (`hidden_names`), code run from text (`code_from_text`), and the script's code without its strings, template
//!   texts, regular expressions and comments (`code`) for the checks that stay on text.
//!
//! A script that doesn't parse never runs in the page either: `parsed` is false and browser_mcp.rs keeps its text rules.

use std::collections::{HashMap, HashSet};

use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_ast_visit::{walk, Visit};
use oxc_parser::{ParseOptions, Parser};
use oxc_span::{GetSpan, SourceType};
use base64::Engine;
use oxc_syntax::operator::{BinaryOperator, UnaryOperator};
use oxc_syntax::scope::ScopeFlags;

/// What a script does (`shape`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Shape {
    /// It parsed; when not, nothing else here is known.
    pub parsed: bool,
    /// The script with its strings, template texts, regular expressions and comments blanked (spaces).
    pub code: String,
    /// It writes into the page or acts on it: a property of something not its own set or deleted, a click, an event
    /// sent, a form sent, an attribute set or removed, an element added, moved or removed.
    pub acts: bool,
    /// One of those may run after the call (see the module's doc).
    pub acts_later: bool,
    /// Some of its code may run after the call: a function given to a timer, a listener, an observer, a promise, a
    /// lock…, stored, or passed by a name; a method of the page given as a value.
    pub later: bool,
    /// It renames fields or labels: an id, a name, a data- attribute, a class, a label's text, a placeholder, a title,
    /// an autocomplete, a type, attributes set or removed, an element removed or replaced.
    pub renames: bool,
    /// What it writes or calls is named at run time (`a['hr' + 'ef'] = …`, `el[k]()` on the page's objects,
    /// `Reflect.set`, `createElement(String.fromCharCode(97))`).
    pub hidden_names: bool,
    /// It runs code from text: eval, Function, a timer given a string, an `on…` attribute set, a tagged template
    /// whose tag isn't a function of the script's (`` setTimeout`…` ``), a timer or eval taken as a value, text given
    /// to a callee that can't be named.
    pub code_from_text: bool,
    /// Too long or too deeply nested to be read safely (`MAX_SCRIPT_BYTES`, `MAX_NESTING`): never parsed.
    pub too_big: bool,
    /// A way out of the page, or into its code by an address (hotfix of 15/11, the seventh review on production: a
    /// `javascript:` address given to `(location).replace` ran an observer that filled the card form drawn later):
    /// the page's `location` reached any way but to read one of its parts (`location.href`), under another name, in
    /// brackets, through the window or the document, destructured; a `javascript:` address, written or decoded from
    /// what the script holds; a window opened, the document written, a link's, a form's, a frame's or an object's
    /// address written, an attribute node changed; the window taken as a value or by a name made at run time;
    /// `replace` on what isn't known to be a string, `assign` on anything but `Object`.
    pub makes_way: bool,
    /// It writes what outlives the page and the page's own code may put back at its next load (the same review: a
    /// shop's stored cart fragment brought a reader back after a reload): localStorage, sessionStorage, IndexedDB, the
    /// caches, cookies, the window's name, the history's state.
    pub stores: bool,
}

/// The longest script read (an agent's are a few kilobytes; fourth review of 15/11: a parser recursing as deep as a
/// script nests must never exhaust a thread's stack, which aborts the whole bridge).
pub const MAX_SCRIPT_BYTES: usize = 32 * 1024;
/// The deepest nesting read: brackets, and runs of prefix operators (`!!!!…`, `- - -…`).
pub const MAX_NESTING: usize = 256;
/// The stack of the thread the script is read on: room for `MAX_SCRIPT_BYTES` of the worst nesting the limits let
/// through, in a debug build too (reserved, not used, by the system).
const STACK_BYTES: usize = 256 * 1024 * 1024;

/// Functions that run text as code, and timers: never taken as values (`const t = setTimeout`, `[f].forEach(setTimeout)`,
/// `setTimeout.call(…)`), only called by their name, and then their arguments read.
const SINKS: &[&str] = &["eval", "Function", "setTimeout", "setInterval", "setImmediate", "execScript", "AsyncFunction",
    "GeneratorFunction", "AsyncGeneratorFunction"];

/// Methods that act on the page, called on something not the script's own.
const ACT_METHODS: &[&str] = &[
    "click", "submit", "requestSubmit", "dispatchEvent", "setAttribute", "setAttributeNS", "removeAttribute",
    "removeAttributeNS", "toggleAttribute", "setAttributeNode", "setNamedItem", "removeNamedItem", "append",
    "appendChild", "prepend", "insertBefore", "insertAdjacentHTML", "insertAdjacentText", "insertAdjacentElement",
    "replaceWith", "replaceChild", "replaceChildren", "remove", "removeChild", "before", "after", "execCommand",
    "setRangeText", "showPicker", "reset", "trigger", "write", "writeln", "postMessage", "add", "toggle",
    "setCustomValidity", "assign", "triggerHandler", "simulate",
];

/// Of those, the ones that rename fields or labels, or take elements out.
const RENAME_METHODS: &[&str] = &[
    "setAttribute", "setAttributeNS", "removeAttribute", "removeAttributeNS", "toggleAttribute", "setAttributeNode",
    "setNamedItem", "removeNamedItem", "replaceWith", "replaceChild", "replaceChildren", "remove", "removeChild",
];

/// Properties whose writing renames a field or a label (or rewrites a label's text).
const RENAME_PROPS: &[&str] = &[
    "id", "name", "htmlFor", "className", "textContent", "innerText", "outerText", "nodeValue", "data", "placeholder",
    "title", "autocomplete", "type", "dataset", "ariaLabel", "ariaLabelledByElements", "labels", "attributes",
];

/// Methods that call the function given at once, before they return (the array's, a string's `replace`, JSON's).
const CALLS_AT_ONCE: &[&str] = &[
    "forEach", "map", "filter", "some", "every", "find", "findIndex", "findLast", "findLastIndex", "reduce",
    "reduceRight", "flatMap", "sort", "toSorted", "replace", "replaceAll", "stringify", "parse", "from",
];

/// What registers a function given by a name or a value to run later (a page's own function, a bound method): only
/// `later`, for pages with card fields (what it would do is unknown; a function written in the script is read).
const REGISTERS: &[&str] = &[
    "addEventListener", "addListener", "setTimeout", "setInterval", "setImmediate", "requestAnimationFrame",
    "requestIdleCallback", "queueMicrotask", "then", "catch", "finally", "postTask", "request",
    "requestVideoFrameCallback", "on", "one", "observe",
];

/// Timers given a string run it as code.
const TIMERS: &[&str] = &["setTimeout", "setInterval", "setImmediate", "execScript"];

/// Objects whose methods act when called (`Reflect.set`), or whose `assign` / `define…` change what they're given.
const META: &[&str] = &["Reflect", "Object"];

/// The window by its names (`makes_way` when taken as a value: `const w = window`).
const WINDOWS: &[&str] = &["window", "self", "globalThis", "top", "parent", "frames", "opener", "content"];
/// Properties that give a window.
const WINDOW_PROPS: &[&str] =
    &["window", "self", "globalThis", "top", "parent", "frames", "opener", "defaultView", "contentWindow"];
/// Properties that give a document.
const DOCUMENT_PROPS: &[&str] = &["document", "ownerDocument", "contentDocument"];
/// The parts of the page's address a script may read (strings).
const LOCATION_PARTS: &[&str] = &["href", "origin", "protocol", "host", "hostname", "port", "pathname", "search", "hash"];
/// Names that open a way out of the page or write its document, whatever has them, called or taken as a value.
const WAY_NAMES: &[&str] = &[
    "location", "open", "write", "writeln", "navigate", "navigation", "reload", "insertAdjacentHTML", "setHTMLUnsafe",
    "createContextualFragment", "parseFromString", "createAttribute", "createAttributeNS", "getAttributeNode",
    "getAttributeNodeNS", "setAttributeNode", "setAttributeNodeNS", "getNamedItem", "getNamedItemNS", "setNamedItem",
    "setNamedItemNS", "createObjectURL",
];
/// Properties whose writing gives a link, a form, a frame, an object or the page an address, or HTML.
const WAY_PROPS: &[&str] =
    &["href", "src", "srcdoc", "data", "action", "formAction", "baseVal", "innerHTML", "outerHTML", "location"];
/// The page's storages written item by item (read with `getItem`, `key`, `length`).
const STORAGES: &[&str] = &["localStorage", "sessionStorage"];
/// What stores beyond the page, however used.
const STORE_NAMES: &[&str] = &["indexedDB", "caches", "cookieStore", "pushState", "replaceState"];
/// Functions that give a string (`replace` read on what they give is a string's).
const STRING_FUNCTIONS: &[&str] =
    &["String", "encodeURIComponent", "encodeURI", "decodeURIComponent", "decodeURI", "escape", "unescape", "atob", "btoa"];
/// Methods that give a string.
const STRING_METHODS: &[&str] = &[
    "toString", "trim", "trimStart", "trimEnd", "toLowerCase", "toUpperCase", "toLocaleLowerCase", "toLocaleUpperCase",
    "substring", "substr", "slice", "padStart", "padEnd", "normalize", "repeat", "replace", "replaceAll", "join",
    "concat", "charAt", "toFixed", "toPrecision", "toISOString", "toDateString", "toTimeString", "toLocaleString",
    "toLocaleDateString", "toLocaleTimeString", "toUTCString", "stringify", "getAttribute", "getPropertyValue",
    "fromCharCode", "fromCodePoint",
];
/// Properties that hold a string.
const STRING_PROPS: &[&str] = &[
    "textContent", "innerText", "outerText", "innerHTML", "outerHTML", "value", "defaultValue", "href", "src", "id",
    "className", "title", "name", "placeholder", "alt", "label", "text", "nodeName", "tagName", "localName",
    "nodeValue", "data", "type", "URL", "documentURI", "baseURI", "origin", "protocol", "host", "hostname", "port",
    "pathname", "search", "hash", "referrer", "cookie", "domain", "lang", "dir", "action", "method", "target",
    "rel", "characterSet", "contentType", "readyState", "visibilityState", "autocomplete", "htmlFor", "cssText",
    "userAgent", "language", "source",
];
/// The longest text put together from what a script holds to look for a `javascript:` address in.
const FOLD_MAX: usize = 4096;

/// How a function is used: called at once (`Now`), or maybe later (`Later`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Use {
    Now,
    Later,
}

/// One function being read: whether it may run later, whether it is async, whether an `await` came already.
struct Frame {
    later: bool,
    awaited: bool,
}

/// The first pass: names declared as the script's own objects, functions bound to names, and the names used
/// otherwise than called at once.
#[derive(Default)]
struct Names {
    own: HashSet<String>,
    /// Every name the script declares (`const`, `let`, `var`, a function's).
    declared: HashSet<String>,
    /// Names given a method of the page bound to its object (`const $ = document.querySelector.bind(document)`).
    bound: HashSet<String>,
    functions: HashMap<u32, String>,
    called: HashSet<u32>,
    passed_at_once: HashSet<u32>,
    escaping: HashSet<String>,
    /// How many times each name is bound by the script (declared, a parameter, a caught error…).
    bindings: HashMap<String, usize>,
}

impl<'a> Visit<'a> for Names {
    fn visit_binding_identifier(&mut self, it: &BindingIdentifier<'a>) {
        *self.bindings.entry(it.name.to_string()).or_default() += 1;
    }

    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let BindingPattern::BindingIdentifier(id) = &it.id {
            self.declared.insert(id.name.to_string());
            if let Some(init) = &it.init {
                let init = init.without_parentheses();
                if is_own_object(init) {
                    self.own.insert(id.name.to_string());
                }
                if let Some(start) = function_start(init) {
                    self.functions.insert(start, id.name.to_string());
                }
                if let Expression::CallExpression(call) = init {
                    let bound = call.callee.without_parentheses().as_member_expression().is_some_and(|m| {
                        m.static_property_name().and_then(|n| n.as_str()) == Some("bind")
                            && m.object().without_parentheses().as_member_expression().is_some_and(|inner| {
                                inner.static_property_name().and_then(|n| n.as_str()).is_some_and(|n| !SINKS.contains(&n))
                            })
                    });
                    if bound {
                        self.bound.insert(id.name.to_string());
                    }
                }
            }
        }
        walk::walk_variable_declarator(self, it);
    }

    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        if let Some(id) = &it.id {
            self.functions.insert(it.span.start, id.name.to_string());
            self.declared.insert(id.name.to_string());
        }
        walk::walk_function(self, it, flags);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Expression::Identifier(id) = resolved(&it.callee) {
            self.called.insert(id.span.start);
        }
        if calls_at_once(&it.callee) {
            for arg in &it.arguments {
                if let Some(Expression::Identifier(id)) = arg.as_expression().map(|e| e.without_parentheses()) {
                    self.passed_at_once.insert(id.span.start);
                }
            }
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if !self.called.contains(&it.span.start) && !self.passed_at_once.contains(&it.span.start) {
            self.escaping.insert(it.name.to_string());
        }
    }
}

/// The second pass: what the script does.
struct Reader<'s> {
    names: &'s Names,
    uses: HashMap<u32, Use>,
    /// Members that are the callee of a call (called, not taken as a value), by their span.
    invoked: HashSet<(u32, u32)>,
    frames: Vec<Frame>,
    blank: Vec<(u32, u32)>,
    shape: Shape,
    /// `location` (or `x.location`) read only for one of its parts (`location.href`): these spans.
    location_reads: HashSet<(u32, u32)>,
    /// localStorage / sessionStorage only read (`getItem`, `key`, `length`): these spans.
    storage_reads: HashSet<(u32, u32)>,
    /// What a member is read on, what `typeof` looks at, `this` given to `call` / `apply` / `bind`: these spans (the
    /// window under these is not taken as a value).
    window_reads: HashSet<(u32, u32)>,
    /// The script's own constants that hold a string (`const t = el.textContent`, bound once).
    strings: HashSet<String>,
    /// Sums already read with the one they are part of: these spans.
    summed: HashSet<(u32, u32)>,
    /// `x.replace(…)` given a regular expression or a plain text first: these callees' spans.
    plain_replaces: HashSet<(u32, u32)>,
}

/// The names a binding pattern binds (`var {a, b: [c]} = …`).
#[derive(Default)]
struct Bound(Vec<String>);

impl<'a> Visit<'a> for Bound {
    fn visit_binding_identifier(&mut self, it: &BindingIdentifier<'a>) {
        self.0.push(it.name.to_string());
    }
}

impl Reader<'_> {
    /// A name the script binds itself (declared, a parameter…): not the page's.
    fn bound(&self, name: &str) -> bool {
        self.names.bindings.contains_key(name)
    }

    /// The window (`window`, `self`, `top`…, `this`, `x.defaultView`…), or with `documents` a document too.
    fn window_like(&self, expression: &Expression, documents: bool) -> bool {
        match resolved(expression) {
            Expression::Identifier(id) => {
                let name = id.name.as_str();
                !self.bound(name) && (WINDOWS.contains(&name) || documents && name == "document")
            }
            Expression::ThisExpression(_) => true,
            other => other.as_member_expression().is_some_and(|m| {
                let name = m.static_property_name().and_then(|n| n.as_str()).unwrap_or("");
                self.gives_window(m.object(), name) || documents && DOCUMENT_PROPS.contains(&name)
            }),
        }
    }

    /// `object.name` is a window: `x.defaultView`, `x.contentWindow`, `x.opener`, or `top`, `parent`… of a window or
    /// a document (`rect.top` is a number).
    fn gives_window(&self, object: &Expression, name: &str) -> bool {
        matches!(name, "defaultView" | "contentWindow" | "opener")
            || WINDOW_PROPS.contains(&name) && self.window_like(object, true)
    }

    /// Known to be a string: a string or a template, a sum with one, a string's method's or a string property's
    /// value, or the script's own constant holding one.
    fn known_string(&self, expression: &Expression) -> bool {
        match resolved(expression) {
            Expression::StringLiteral(_) | Expression::TemplateLiteral(_) => true,
            Expression::BinaryExpression(b) => {
                b.operator == BinaryOperator::Addition && (self.known_string(&b.left) || self.known_string(&b.right))
            }
            Expression::LogicalExpression(l) => self.known_string(&l.left) && self.known_string(&l.right),
            Expression::ConditionalExpression(c) => self.known_string(&c.consequent) && self.known_string(&c.alternate),
            Expression::Identifier(id) => self.strings.contains(id.name.as_str()),
            Expression::CallExpression(call) => match resolved(&call.callee) {
                Expression::Identifier(id) => {
                    STRING_FUNCTIONS.contains(&id.name.as_str()) && !self.bound(id.name.as_str())
                }
                callee => callee
                    .as_member_expression()
                    .and_then(|m| m.static_property_name())
                    .and_then(|n| n.as_str())
                    .is_some_and(|n| STRING_METHODS.contains(&n)),
            },
            other => other
                .as_member_expression()
                .and_then(|m| m.static_property_name())
                .and_then(|n| n.as_str())
                .is_some_and(|n| STRING_PROPS.contains(&n)),
        }
    }

    /// A member named `name` on `object` (`span`: the member's), whether read, called or written (hotfix of 15/11).
    fn member_named(&mut self, object: &Expression, name: &str, span: (u32, u32)) {
        let object = resolved(object);
        let invoked = self.invoked.contains(&span);
        // what a member is read on is not taken as a value
        self.window_reads.insert(span_of(object));
        // the page's address: one of its parts read, nothing else (its methods, the object itself)
        if is_location(object) {
            if LOCATION_PARTS.contains(&name) && !invoked {
                self.location_reads.insert(span_of(object));
            } else {
                self.shape.makes_way = true;
            }
        }
        // (`indexedDB.open`, `caches.open`: storages, `stores`)
        let storage_opened = name == "open" && method_name(object).is_some_and(|n| STORE_NAMES.contains(&n.as_str()));
        if WAY_NAMES.contains(&name) && !(name == "location" && self.location_reads.contains(&span)) && !storage_opened {
            self.shape.makes_way = true;
        }
        match name {
            // the window taken as a value (`const w = document.defaultView`)
            _ if self.gives_window(object, name) && !self.window_reads.contains(&span) => self.shape.makes_way = true,
            // `location.assign`, `location.replace` under any name: only `Object.assign` and a string's `replace`
            "assign" if !matches!(object, Expression::Identifier(id) if id.name == "Object" && !self.bound("Object")) => {
                self.shape.makes_way = true
            }
            "replace" if !self.known_string(object) && !self.plain_replaces.contains(&span) => self.shape.makes_way = true,
            _ => {}
        }
        if is_storage(object) && matches!(name, "getItem" | "key" | "length") {
            self.storage_reads.insert(span_of(object));
        }
        if STORAGES.contains(&name) && !self.storage_reads.contains(&span) || STORE_NAMES.contains(&name) {
            self.shape.stores = true;
        }
    }

    /// A member written, assigned in a pattern, updated (hotfix of 15/11): an address, HTML, an attribute node, the
    /// page's location, a storage's item, a cookie, the window's name.
    fn way_written(&mut self, member: &MemberExpression) {
        if self.own(member.object()) {
            return;
        }
        let object = resolved(member.object());
        let name = member.static_property_name().and_then(|n| n.as_str()).map(str::to_string);
        match name.as_deref() {
            Some(name) => {
                if WAY_PROPS.contains(&name) {
                    self.shape.makes_way = true;
                }
                if name == "cookie" || matches!(name, "name" | "status" | "defaultStatus") && self.window_like(object, false) {
                    self.shape.stores = true;
                }
            }
            // `window[k] = …`: a name made at run time on the window or a document
            None if self.window_like(object, true) => self.shape.makes_way = true,
            None => {}
        }
        if is_location(object) || self.window_like(object, true) && name.as_deref().is_some_and(|n| WAY_NAMES.contains(&n)) {
            self.shape.makes_way = true;
        }
        if is_storage(object) {
            self.shape.stores = true;
        }
        // an attribute node's value (`el.attributes.href.value = …`, `el.attributes[0].nodeValue = …`)
        if chain_has(object, "attributes") {
            self.shape.makes_way = true;
        }
    }

    /// A `javascript:` address (or the name `location`) in a text the script puts together.
    fn looks_at_text(&mut self, text: Option<String>) {
        if let Some(text) = text {
            if names_javascript(&text) || text.trim() == "location" {
                self.shape.makes_way = true;
            }
        }
    }

    /// Pieces of text in a row (a sum's, a template's), those the script holds put together and looked at; one it
    /// doesn't (None) ends a run (`x + 'java' + 'script:'`).
    fn looks_at_runs(&mut self, pieces: Vec<Option<String>>) {
        let mut run = String::new();
        for piece in pieces {
            match piece {
                Some(text) => {
                    run.push_str(&text);
                    if run.len() > 4 * FOLD_MAX {
                        let kept: String = run.chars().rev().take(16).collect::<Vec<_>>().into_iter().rev().collect();
                        self.looks_at_text(Some(std::mem::replace(&mut run, kept)));
                    }
                }
                None => self.looks_at_text(Some(std::mem::take(&mut run))),
            }
        }
        self.looks_at_text(Some(run));
    }

    /// A sum's pieces, its inner sums with it (each read once).
    fn sum_pieces(&mut self, it: &BinaryExpression, pieces: &mut Vec<Option<String>>) {
        self.summed.insert((it.span.start, it.span.end));
        for side in [&it.left, &it.right] {
            match resolved(side) {
                Expression::BinaryExpression(inner) if inner.operator == BinaryOperator::Addition => {
                    self.sum_pieces(inner, pieces)
                }
                other => pieces.push(fold(other, 0)),
            }
        }
    }

    /// A key taken out of something (`const {location: l} = window`, `({open: o} = window)`).
    fn key_taken(&mut self, key: &PropertyKey) {
        match literal_name_key(key) {
            Some(name) => {
                let name = name.as_str();
                if WAY_NAMES.contains(&name)
                    || matches!(name, "window" | "self" | "globalThis" | "frames" | "parent" | "defaultView" | "contentWindow")
                {
                    self.shape.makes_way = true;
                }
                if STORAGES.contains(&name) || STORE_NAMES.contains(&name) {
                    self.shape.stores = true;
                }
            }
            None => self.shape.makes_way = true,
        }
    }

    /// What a call opens, writes or stores (hotfix of 15/11): a `javascript:` address decoded from what the script
    /// holds (`atob('…')`, `String.fromCharCode(…)`); `Object.assign` onto the page's objects with an address, HTML or
    /// a handler, or from what isn't read; `Object.values(document)` (its location among them); `this` given to
    /// `call` / `apply` / `bind` (an IIFE's) is not taken as a value.
    fn ways_called(&mut self, it: &CallExpression, callee: &Expression) {
        self.looks_at_text(fold_call(it, 0));
        let Some(member) = callee.as_member_expression() else { return };
        let method = member.static_property_name().and_then(|n| n.as_str()).unwrap_or("");
        let first = it.arguments.first().and_then(|a| a.as_expression()).map(resolved);
        // `s.replace(/\s+/g, ' ')`: a location's `replace` given a regular expression or a plain text goes nowhere but a
        // path of the same site (a `javascript:` text is refused as such)
        let plain = |e: &Expression| match e {
            Expression::RegExpLiteral(_) | Expression::StringLiteral(_) => true,
            Expression::TemplateLiteral(t) => t.expressions.is_empty(),
            _ => false,
        };
        if method == "replace" && first.is_some_and(plain) {
            self.plain_replaces.insert(span_of(callee));
        }
        if matches!(method, "call" | "apply" | "bind") {
            if let Some(Expression::ThisExpression(this)) = first {
                self.window_reads.insert((this.span.start, this.span.end));
            }
        }
        let on_object =
            matches!(resolved(member.object()), Expression::Identifier(id) if id.name == "Object" && !self.bound("Object"));
        if !on_object {
            return;
        }
        match method {
            "values" | "entries" | "getOwnPropertyDescriptor" | "getOwnPropertyDescriptors" => {
                if first.is_some_and(|e| self.window_like(e, true)) {
                    self.shape.makes_way = true;
                }
            }
            "assign" => {
                let own = first_is_own(&it.arguments, &self.names.own);
                if first.is_some_and(|e| self.window_like(e, true) || is_storage(e)) {
                    self.shape.makes_way = true;
                }
                for source in it.arguments.iter().skip(1) {
                    match source.as_expression().map(resolved) {
                        Some(Expression::ObjectExpression(object)) => {
                            for property in &object.properties {
                                let key = match property {
                                    ObjectPropertyKind::ObjectProperty(p) => literal_name_key(&p.key),
                                    ObjectPropertyKind::SpreadProperty(_) => None,
                                };
                                match key {
                                    _ if own => {}
                                    Some(key) => {
                                        if WAY_PROPS.contains(&key.as_str()) || key.len() > 2 && key.starts_with("on") {
                                            self.shape.makes_way = true;
                                        }
                                        if key == "cookie" {
                                            self.shape.stores = true;
                                        }
                                    }
                                    None => self.shape.makes_way = true,
                                }
                            }
                        }
                        Some(e) if !own || self.window_like(e, true) => self.shape.makes_way = true,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// A timer's first argument that isn't a function: a string, a template, a sum, a call's result, a name the script
    /// gives something else (a page's own name, undeclared here, is left: what it holds is unknown).
    fn timer_runs_text(&self, arguments: &[Argument]) -> bool {
        match arguments.first().and_then(|a| a.as_expression()).map(|e| e.without_parentheses()) {
            None => false,
            Some(Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_)) => false,
            Some(Expression::Identifier(id)) => {
                let name = id.name.as_str();
                self.names.declared.contains(name) && !self.names.functions.values().any(|f| f == name)
            }
            // `f.bind(x)` gives a function (read as one: a method of the page taken as a value acts later)
            Some(Expression::CallExpression(call))
                if call.callee.without_parentheses().as_member_expression().and_then(|m| m.static_property_name()).and_then(|n| n.as_str())
                    == Some("bind") =>
            {
                false
            }
            Some(e) => e.as_member_expression().is_none() || first_is_text(arguments),
        }
    }

    fn act(&mut self) {
        self.shape.acts = true;
        if self.frames.last().is_some_and(|f| f.later || f.awaited) {
            self.shape.acts_later = true;
        }
    }

    fn own(&self, object: &Expression) -> bool {
        matches!(object.without_parentheses(), Expression::Identifier(id) if self.names.own.contains(id.name.as_str()))
    }

    /// A member written (assigned, updated, deleted).
    fn written(&mut self, member: &MemberExpression) {
        if self.own(member.object()) {
            return;
        }
        self.act();
        match member {
            MemberExpression::StaticMemberExpression(m) => {
                let name = m.property.name.as_str();
                if RENAME_PROPS.contains(&name) || under(&m.object, "dataset") || under(&m.object, "attributes") {
                    self.shape.renames = true;
                }
                if name.len() > 2 && name.starts_with("on") {
                    // an event handler set: it runs later
                    self.shape.later = true;
                }
            }
            MemberExpression::ComputedMemberExpression(m) => {
                match literal_name(&m.expression) {
                    Some(name) => {
                        if RENAME_PROPS.contains(&name.as_str()) || under(&m.object, "dataset") {
                            self.shape.renames = true;
                        }
                    }
                    None => self.shape.hidden_names = true,
                }
            }
            _ => {}
        }
    }

    fn enter(&mut self, start: u32, generator: bool) {
        let parent = self.frames.last().map(|f| f.later).unwrap_or(false);
        let used = match self.uses.get(&start) {
            Some(used) => *used,
            None => match self.names.functions.get(&start) {
                Some(name) if !self.names.escaping.contains(name) => Use::Now,
                _ => Use::Later,
            },
        };
        let later = parent || used == Use::Later || generator;
        if later {
            self.shape.later = true;
        }
        self.frames.push(Frame { later, awaited: false });
    }
}

impl<'a> Visit<'a> for Reader<'_> {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        let callee = resolved(&it.callee);
        self.invoked.insert(span_of(callee));
        // a callee that can't be named (a call's result, a choice, a name the script gives a value): text given to it
        // may be run as code (fourth review of 15/11: fail closed)
        let named = match callee {
            Expression::Identifier(id) => {
                let name = id.name.as_str();
                !self.names.declared.contains(name)
                    || self.names.functions.values().any(|f| f == name)
                    || self.names.bound.contains(name)
            }
            Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_) => true,
            other => other.as_member_expression().is_some(),
        };
        if !named && it.arguments.iter().any(text_like) {
            self.shape.code_from_text = true;
        }
        self.ways_called(it, callee);
        // the functions given: at once (an IIFE, the array's methods…) or maybe later
        if let Some(start) = function_start(callee) {
            self.uses.insert(start, Use::Now);
        }
        if let Some(member) = callee.as_member_expression() {
            // `(function () {…}).call(this)`: at once too
            let how = member.static_property_name().and_then(|n| n.as_str());
            if matches!(how, Some("call" | "apply")) {
                let object = resolved(member.object());
                if let Some(start) = function_start(object) {
                    self.uses.insert(start, Use::Now);
                }
                self.invoked.insert(span_of(object));
                // `setTimeout.call(window, 'code', 10)`: a timer or eval called this way, given text (or an array of
                // arguments, unread)
                let sink = match object {
                    Expression::Identifier(id) => SINKS.contains(&id.name.as_str()),
                    other => other
                        .as_member_expression()
                        .and_then(|m| m.static_property_name())
                        .and_then(|n| n.as_str())
                        .is_some_and(|n| SINKS.contains(&n)),
                };
                if sink && (how == Some("apply") || it.arguments.iter().skip(1).any(text_like)) {
                    self.shape.code_from_text = true;
                }
            }
        }
        let at_once = calls_at_once(&it.callee);
        for arg in &it.arguments {
            if let Some(start) = arg.as_expression().and_then(|e| function_start(e.without_parentheses())) {
                self.uses.insert(start, if at_once { Use::Now } else { Use::Later });
            }
        }
        let registers = match callee {
            Expression::Identifier(id) => REGISTERS.contains(&id.name.as_str()),
            other => other
                .as_member_expression()
                .and_then(|m| m.static_property_name())
                .and_then(|n| n.as_str())
                .is_some_and(|n| REGISTERS.contains(&n)),
        };
        if registers && !it.arguments.is_empty() {
            self.shape.later = true;
        }
        // what it calls
        match callee {
            Expression::Identifier(id) => {
                let name = id.name.as_str();
                if name == "eval" || name == "Function" || (TIMERS.contains(&name) && self.timer_runs_text(&it.arguments)) {
                    self.shape.code_from_text = true;
                }
            }
            _ => {
                if let Some(member) = callee.as_member_expression() {
                    let object = member.object().without_parentheses();
                    match member.static_property_name().and_then(|n| n.as_str()) {
                        Some(method) => {
                            let root = root_name(object);
                            // `[].constructor.constructor('…')()`: Function by another way
                            if TIMERS.contains(&method) && self.timer_runs_text(&it.arguments)
                                || method == "eval"
                                || method == "constructor"
                            {
                                self.shape.code_from_text = true;
                            }
                            if matches!(method, "setAttribute" | "setAttributeNS") {
                                match attribute_named(&it.arguments) {
                                    Some(name) if name.to_ascii_lowercase().starts_with("on") => self.shape.code_from_text = true,
                                    Some(_) => {}
                                    None => self.shape.hidden_names = true,
                                }
                            }
                            if root.as_deref().is_some_and(|r| META.contains(&r)) && matches!(object, Expression::Identifier(_))
                            {
                                // Reflect.set / apply / defineProperty…; Object.assign / defineProperty…
                                if root.as_deref() == Some("Reflect") {
                                    self.shape.hidden_names = true;
                                    self.act();
                                } else if matches!(method, "assign" | "defineProperty" | "defineProperties" | "setPrototypeOf")
                                    && !first_is_own(&it.arguments, &self.names.own)
                                {
                                    self.act();
                                    self.shape.renames = true;
                                }
                            } else if matches!(method, "call" | "apply") {
                                // `x.click.call(el)`: the method itself, at once
                                if let Some(inner) = object.as_member_expression() {
                                    if inner.static_property_name().and_then(|n| n.as_str()).is_some_and(|m| ACT_METHODS.contains(&m)) {
                                        self.act();
                                    }
                                }
                            } else if ACT_METHODS.contains(&method) && !self.own(object) {
                                self.act();
                                // an attribute that names nothing (a value, a state) is a write, not a rename
                                let plain = method.ends_with("Attribute") || method.ends_with("AttributeNS");
                                let named = if plain { attribute_named_first(&it.arguments) } else { None };
                                let state = named.as_deref().is_some_and(|n| STATE_ATTRIBUTES.contains(&n.to_ascii_lowercase().as_str()));
                                if RENAME_METHODS.contains(&method) && !state {
                                    self.shape.renames = true;
                                }
                            }
                            if method == "createElement" || method == "createElementNS" {
                                let named = it.arguments.last().and_then(|a| a.as_expression()).and_then(literal_name);
                                if named.is_none() {
                                    self.shape.hidden_names = true;
                                }
                                // a script element: its text runs as code once added (hotfix of 15/11)
                                if named.is_some_and(|n| n.trim().eq_ignore_ascii_case("script")) {
                                    self.shape.code_from_text = true;
                                }
                            }
                        }
                        None => {
                            // `el[k]()`, `el['cli' + 'ck']()` on the page's objects
                            if let MemberExpression::ComputedMemberExpression(m) = member {
                                if literal_name(&m.expression).is_none() && !self.own(object) {
                                    self.shape.hidden_names = true;
                                    self.act();
                                }
                            }
                        }
                    }
                }
            }
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        let promise = matches!(it.callee.without_parentheses(), Expression::Identifier(id) if id.name == "Promise");
        if matches!(it.callee.without_parentheses(), Expression::Identifier(id) if id.name.ends_with("Observer") || id.name.ends_with("Worker"))
        {
            self.shape.later = true;
        }
        if matches!(it.callee.without_parentheses(), Expression::Identifier(id) if id.name == "Function") {
            self.shape.code_from_text = true;
        }
        for arg in &it.arguments {
            if let Some(start) = arg.as_expression().and_then(|e| function_start(e.without_parentheses())) {
                // a promise's executor runs at once; an observer's, a worker's… later
                self.uses.insert(start, if promise { Use::Now } else { Use::Later });
            }
        }
        walk::walk_new_expression(self, it);
    }

    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        self.enter(it.span.start, it.generator);
        walk::walk_function(self, it, flags);
        self.frames.pop();
    }

    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        self.enter(it.span.start, false);
        walk::walk_arrow_function_expression(self, it);
        self.frames.pop();
    }

    fn visit_class(&mut self, it: &Class<'a>) {
        // a class's methods run whenever it is used: later
        self.frames.push(Frame { later: true, awaited: false });
        self.shape.later = true;
        walk::walk_class(self, it);
        self.frames.pop();
    }

    fn visit_await_expression(&mut self, it: &AwaitExpression<'a>) {
        walk::walk_await_expression(self, it);
        if let Some(frame) = self.frames.last_mut() {
            frame.awaited = true;
        }
    }

    fn visit_for_of_statement(&mut self, it: &ForOfStatement<'a>) {
        if it.r#await {
            if let Some(frame) = self.frames.last_mut() {
                frame.awaited = true;
            }
        }
        walk::walk_for_of_statement(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if let Some(member) = it.left.as_member_expression() {
            self.written(member);
            // a function stored in a property (a handler): later
            if let Some(start) = function_start(it.right.without_parentheses()) {
                self.uses.insert(start, Use::Later);
            }
        } else if it.left.as_assignment_target_pattern().is_some() {
            // `[a.b, c.d] = …`: written members inside a pattern, taken as acts
            self.act();
        }
        walk::walk_assignment_expression(self, it);
    }

    fn visit_update_expression(&mut self, it: &UpdateExpression<'a>) {
        if let Some(member) = it.argument.as_member_expression() {
            self.written(member);
        }
        walk::walk_update_expression(self, it);
    }

    fn visit_unary_expression(&mut self, it: &UnaryExpression<'a>) {
        if it.operator == UnaryOperator::Delete {
            if let Some(member) = it.argument.without_parentheses().as_member_expression() {
                self.written(member);
                self.way_written(member);
            }
        }
        if it.operator == UnaryOperator::Typeof {
            self.window_reads.insert(span_of(resolved(&it.argument)));
        }
        walk::walk_unary_expression(self, it);
    }

    fn visit_simple_assignment_target(&mut self, it: &SimpleAssignmentTarget<'a>) {
        match it {
            // `name = …`, `status = …`: the window's own, when the script never bound them
            SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                let name = id.name.as_str();
                if !self.bound(name) && matches!(name, "name" | "status" | "defaultStatus" | "cookie") {
                    self.shape.stores = true;
                }
            }
            other => {
                if let Some(member) = other.as_member_expression() {
                    self.way_written(member);
                }
            }
        }
        walk::walk_simple_assignment_target(self, it);
    }

    fn visit_binding_identifier(&mut self, it: &BindingIdentifier<'a>) {
        // `var location = …` at a script's top level is the window's location set
        if it.name == "location" {
            self.shape.makes_way = true;
        }
    }

    fn visit_variable_declaration(&mut self, it: &VariableDeclaration<'a>) {
        for declarator in &it.declarations {
            let mut bound = Bound::default();
            bound.visit_binding_pattern(&declarator.id);
            // `var name = …` at a script's top level is the window's name set
            if it.kind == VariableDeclarationKind::Var
                && self.frames.len() == 1
                && bound.0.iter().any(|n| matches!(n.as_str(), "name" | "status" | "defaultStatus"))
            {
                self.shape.stores = true;
            }
            if let (VariableDeclarationKind::Const, BindingPattern::BindingIdentifier(id), Some(init)) =
                (it.kind, &declarator.id, &declarator.init)
            {
                if self.names.bindings.get(id.name.as_str()) == Some(&1) && self.known_string(init) {
                    self.strings.insert(id.name.to_string());
                }
            }
            self.visit_variable_declarator(declarator);
        }
    }

    fn visit_binding_property(&mut self, it: &BindingProperty<'a>) {
        // `const {location: l} = window`, `const {defaultView: w} = document`
        self.key_taken(&it.key);
        walk::walk_binding_property(self, it);
    }

    fn visit_assignment_target_property_property(&mut self, it: &AssignmentTargetPropertyProperty<'a>) {
        self.key_taken(&it.name);
        walk::walk_assignment_target_property_property(self, it);
    }

    fn visit_this_expression(&mut self, it: &ThisExpression) {
        // `this` as a value: the window at a script's top level (`const w = this`)
        if !self.window_reads.contains(&(it.span.start, it.span.end)) {
            self.shape.makes_way = true;
        }
    }

    fn visit_spread_element(&mut self, it: &SpreadElement<'a>) {
        // `{...document}`: its location among its properties
        if self.window_like(&it.argument, true) {
            self.shape.makes_way = true;
        }
        walk::walk_spread_element(self, it);
    }

    fn visit_with_statement(&mut self, it: &WithStatement<'a>) {
        // names resolved on an object at run time
        self.shape.hidden_names = true;
        walk::walk_with_statement(self, it);
    }

    fn visit_binary_expression(&mut self, it: &BinaryExpression<'a>) {
        // `'java' + 'script:'`: a sum read once, from its top
        if it.operator == BinaryOperator::Addition && !self.summed.contains(&(it.span.start, it.span.end)) {
            let mut pieces = Vec::new();
            self.sum_pieces(it, &mut pieces);
            self.looks_at_runs(pieces);
        }
        walk::walk_binary_expression(self, it);
    }

    fn visit_template_literal(&mut self, it: &TemplateLiteral<'a>) {
        let mut pieces = Vec::new();
        for (i, quasi) in it.quasis.iter().enumerate() {
            pieces.push(Some(quasi.value.cooked.as_ref().and_then(|c| c.as_str()).unwrap_or(quasi.value.raw.as_str()).to_string()));
            if let Some(expression) = it.expressions.get(i) {
                pieces.push(fold(expression, 0));
            }
        }
        self.looks_at_runs(pieces);
        walk::walk_template_literal(self, it);
    }

    fn visit_tagged_template_expression(&mut self, it: &TaggedTemplateExpression<'a>) {
        // `` setTimeout`…//${1500}` `` calls setTimeout(['…'], 1500): a tag is a call given text (fourth review of
        // 15/11). Only String.raw and the script's own functions read it; any other tag runs it as code, maybe later.
        let tag = resolved(&it.tag);
        self.invoked.insert(span_of(tag));
        let known = match tag {
            Expression::Identifier(id) => self.names.functions.values().any(|f| f == id.name.as_str()),
            other => other.as_member_expression().is_some_and(|m| {
                m.static_property_name().and_then(|n| n.as_str()) == Some("raw")
                    && matches!(m.object().without_parentheses(), Expression::Identifier(id) if id.name == "String")
            }),
        };
        if !known {
            self.shape.code_from_text = true;
        }
        walk::walk_tagged_template_expression(self, it);
    }

    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        // a timer or eval taken as a value (`const t = setTimeout`, `[f].forEach(setTimeout)`): its later call unread
        if SINKS.contains(&it.name.as_str()) && !self.invoked.contains(&(it.span.start, it.span.end)) {
            self.shape.code_from_text = true;
        }
        let name = it.name.as_str();
        let span = (it.span.start, it.span.end);
        // `location` any way but to read one of its parts (`(location).replace`, `const l = location`)
        if name == "location" && !self.location_reads.contains(&span) {
            self.shape.makes_way = true;
        }
        if !self.bound(name) {
            // `open(…)`, `navigation.navigate(…)`: the window's (`location`: above, its parts read)
            if WAY_NAMES.contains(&name) && name != "location" {
                self.shape.makes_way = true;
            }
            // the window taken as a value (`const w = window`, `f(self)`)
            if WINDOWS.contains(&name) && !self.window_reads.contains(&span) {
                self.shape.makes_way = true;
            }
            if STORAGES.contains(&name) && !self.storage_reads.contains(&span) || STORE_NAMES.contains(&name) {
                self.shape.stores = true;
            }
        }
    }

    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        if literal_name(&it.expression).is_some_and(|n| SINKS.contains(&n.as_str()))
            && !self.invoked.contains(&(it.span.start, it.span.end))
        {
            self.shape.code_from_text = true;
        }
        match literal_name(&it.expression) {
            Some(name) => self.member_named(&it.object, &name, (it.span.start, it.span.end)),
            // `window['loc' + 'ation']`, `document[k]`: a name made at run time on the window or a document
            None => {
                if self.window_like(&it.object, true) || is_storage(resolved(&it.object)) {
                    self.shape.makes_way = true;
                }
                self.window_reads.insert(span_of(resolved(&it.object)));
            }
        }
        walk::walk_computed_member_expression(self, it);
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        let name = it.property.name.as_str();
        self.member_named(&it.object, name, (it.span.start, it.span.end));
        // `window.setTimeout` taken as a value; `x.constructor.constructor`, `(() => {}).constructor`: Function
        if SINKS.contains(&name) && !self.invoked.contains(&(it.span.start, it.span.end)) {
            self.shape.code_from_text = true;
        }
        if name == "constructor" {
            let object = it.object.without_parentheses();
            let of_constructor =
                object.as_member_expression().and_then(|m| m.static_property_name()).and_then(|n| n.as_str()) == Some("constructor");
            if of_constructor || function_start(object).is_some() || matches!(object, Expression::ClassExpression(_)) {
                self.shape.code_from_text = true;
            }
        }
        // a method of the page taken as a value (`setTimeout(btn.click.bind(btn))`, `new MutationObserver(f.submit)`)
        if !self.invoked.contains(&(it.span.start, it.span.end)) && ACT_METHODS.contains(&it.property.name.as_str()) && !self.own(&it.object) {
            self.shape.later = true;
            self.shape.acts = true;
            self.shape.acts_later = true;
        }
        walk::walk_static_member_expression(self, it);
    }

    fn visit_import_expression(&mut self, it: &ImportExpression<'a>) {
        self.shape.code_from_text = true;
        walk::walk_import_expression(self, it);
    }

    fn visit_string_literal(&mut self, it: &StringLiteral<'a>) {
        self.looks_at_text(it.value.as_str().map(str::to_string));
        self.blank.push((it.span.start + 1, it.span.end.saturating_sub(1)));
    }

    fn visit_template_element(&mut self, it: &TemplateElement<'a>) {
        self.looks_at_text(Some(it.value.raw.as_str().to_string()));
        self.looks_at_text(it.value.cooked.as_ref().and_then(|c| c.as_str()).map(str::to_string));
        self.blank.push((it.span.start, it.span.end));
    }

    fn visit_reg_exp_literal(&mut self, it: &RegExpLiteral<'a>) {
        self.blank.push((it.span.start + 1, it.span.end.saturating_sub(1)));
    }
}

/// The page's `location`: the bare name, `x.location`, `x['location']`.
fn is_location(expression: &Expression) -> bool {
    match expression.without_parentheses() {
        Expression::Identifier(id) => id.name == "location",
        other => other.as_member_expression().and_then(|m| m.static_property_name()).and_then(|n| n.as_str()) == Some("location"),
    }
}

/// localStorage, sessionStorage (bare, or a window's).
fn is_storage(expression: &Expression) -> bool {
    match expression.without_parentheses() {
        Expression::Identifier(id) => STORAGES.contains(&id.name.as_str()),
        other => other
            .as_member_expression()
            .and_then(|m| m.static_property_name())
            .and_then(|n| n.as_str())
            .is_some_and(|n| STORAGES.contains(&n)),
    }
}

/// `….name…` somewhere along a chain of members (`el.attributes.href`, `el.attributes[0]`).
fn chain_has(expression: &Expression, name: &str) -> bool {
    let mut current = expression.without_parentheses();
    while let Some(member) = current.as_member_expression() {
        if member.static_property_name().and_then(|n| n.as_str()) == Some(name) {
            return true;
        }
        current = member.object().without_parentheses();
    }
    false
}

/// A text holding a `javascript:` address (spaces and controls the browser drops ignored, any case).
fn names_javascript(text: &str) -> bool {
    let plain: String = text.chars().filter(|c| !c.is_whitespace() && !c.is_control()).collect::<String>().to_ascii_lowercase();
    plain.contains("javascript:")
}

/// A property key's name when written plainly (`{a: …}`, `{'a': …}`, `{['a']: …}`).
fn literal_name_key(key: &PropertyKey) -> Option<String> {
    match key {
        PropertyKey::StaticIdentifier(id) => Some(id.name.to_string()),
        other => other.as_expression().and_then(literal_name),
    }
}

/// A method's (or a global's) name as written: `x.m(…)`, `x['m'](…)`, `m(…)`.
fn method_name(callee: &Expression) -> Option<String> {
    match callee.without_parentheses() {
        Expression::Identifier(id) => Some(id.name.to_string()),
        other => other.as_member_expression().and_then(|m| m.static_property_name()).and_then(|n| n.as_str()).map(str::to_string),
    }
}

/// The text an expression gives when the script holds all of it (strings, numbers, sums, templates, `atob`,
/// `decodeURIComponent`, `unescape`, `String.fromCharCode`, `[…].join`, `concat`, `replace` with strings), up to
/// `FOLD_MAX` characters: a `javascript:` address decoded (hotfix of 15/11: `atob` in the review's attack).
fn fold(expression: &Expression, depth: usize) -> Option<String> {
    if depth > 64 {
        return None;
    }
    let text = match resolved(expression) {
        Expression::StringLiteral(s) => s.value.as_str()?.to_string(),
        Expression::NumericLiteral(n) => n.value.to_string(),
        Expression::TemplateLiteral(t) => {
            let mut text = String::new();
            for (i, quasi) in t.quasis.iter().enumerate() {
                text.push_str(quasi.value.cooked.as_ref().and_then(|c| c.as_str()).unwrap_or(quasi.value.raw.as_str()));
                if let Some(expression) = t.expressions.get(i) {
                    text.push_str(&fold(expression, depth + 1)?);
                }
                if text.len() > FOLD_MAX {
                    return None;
                }
            }
            text
        }
        Expression::BinaryExpression(b) if b.operator == BinaryOperator::Addition => {
            fold(&b.left, depth + 1)? + &fold(&b.right, depth + 1)?
        }
        Expression::CallExpression(c) => fold_call(c, depth)?,
        _ => return None,
    };
    (text.len() <= FOLD_MAX).then_some(text)
}

/// `fold` for a call: the decoders, `join`, `concat`, `replace`.
fn fold_call(it: &CallExpression, depth: usize) -> Option<String> {
    if depth > 64 {
        return None;
    }
    let callee = resolved(&it.callee);
    let argument = |i: usize| it.arguments.get(i).and_then(|a| a.as_expression()).and_then(|e| fold(e, depth + 1));
    let text = match method_name(callee)?.as_str() {
        "atob" => {
            let given: String = argument(0)?.chars().filter(|c| !c.is_whitespace() && *c != '=').collect();
            let bytes = base64::engine::general_purpose::STANDARD_NO_PAD.decode(given).ok()?;
            bytes.into_iter().map(char::from).collect::<String>()
        }
        "decodeURIComponent" | "decodeURI" | "unescape" => {
            percent_encoding::percent_decode_str(&argument(0)?).decode_utf8_lossy().into_owned()
        }
        "fromCharCode" | "fromCodePoint" => {
            let mut text = String::new();
            for given in &it.arguments {
                let Some(Expression::NumericLiteral(n)) = given.as_expression().map(resolved) else { return None };
                text.push(char::from_u32(n.value as u32)?);
            }
            text
        }
        "join" => {
            let Expression::ArrayExpression(array) = resolved(callee.as_member_expression()?.object()) else { return None };
            let separator = if it.arguments.is_empty() { ",".to_string() } else { argument(0)? };
            let mut parts = Vec::new();
            for element in &array.elements {
                parts.push(fold(element.as_expression()?, depth + 1)?);
            }
            parts.join(&separator)
        }
        "concat" => {
            let mut text = fold(callee.as_member_expression()?.object(), depth + 1)?;
            for i in 0..it.arguments.len() {
                text.push_str(&argument(i)?);
            }
            text
        }
        "replace" | "replaceAll" => {
            let text = fold(callee.as_member_expression()?.object(), depth + 1)?;
            text.replace(&argument(0)?, &argument(1)?)
        }
        _ => return None,
    };
    (text.len() <= FOLD_MAX).then_some(text)
}

/// `[…]`, `{…}`, `new Map()`… : the script's own object, whose properties aren't the page's.
fn is_own_object(init: &Expression) -> bool {
    match init {
        Expression::ObjectExpression(_) | Expression::ArrayExpression(_) => true,
        Expression::NewExpression(new) => matches!(
            new.callee.without_parentheses(),
            Expression::Identifier(id) if matches!(id.name.as_str(), "Map" | "Set" | "WeakMap" | "WeakSet" | "Object" | "Array" | "URLSearchParams")
        ),
        _ => false,
    }
}

/// Where a function expression starts (an arrow's or a `function`'s), if this is one.
fn function_start(expression: &Expression) -> Option<u32> {
    match expression {
        Expression::ArrowFunctionExpression(f) => Some(f.span.start),
        Expression::FunctionExpression(f) => Some(f.span.start),
        _ => None,
    }
}

/// A callee that calls the functions given at once: `x.forEach(…)`, `Array.from(x, …)`, `s.replace(re, …)`,
/// `JSON.stringify(o, …)`, `Array.prototype.map.call(nodes, …)`.
fn calls_at_once(callee: &Expression) -> bool {
    let Some(member) = callee.without_parentheses().as_member_expression() else { return false };
    match member.static_property_name().and_then(|n| n.as_str()) {
        Some("call" | "apply") => member
            .object()
            .without_parentheses()
            .as_member_expression()
            .and_then(|m| m.static_property_name())
            .and_then(|n| n.as_str())
            .is_some_and(|n| CALLS_AT_ONCE.contains(&n)),
        Some(name) => CALLS_AT_ONCE.contains(&name),
        None => false,
    }
}

/// The name a key spells out when it is written plainly: a string, a number, a template without `${}`.
fn literal_name(key: &Expression) -> Option<String> {
    match key.without_parentheses() {
        Expression::StringLiteral(s) => s.value.as_str().map(str::to_string),
        Expression::NumericLiteral(n) => Some(n.value.to_string()),
        Expression::TemplateLiteral(t) if t.expressions.is_empty() => {
            Some(t.quasis.iter().map(|q| q.value.raw.as_str()).collect::<String>())
        }
        _ => None,
    }
}

/// What a callee or a tag is once its parentheses and a comma's leading values are taken off: `(0, setTimeout)` is
/// setTimeout (fourth review of 15/11).
fn resolved<'b, 'a>(expression: &'b Expression<'a>) -> &'b Expression<'a> {
    match expression.without_parentheses() {
        Expression::SequenceExpression(sequence) => match sequence.expressions.last() {
            Some(last) => resolved(last),
            None => expression,
        },
        other => other,
    }
}

/// An argument that may be text run as code by a callee that can't be named: anything but a function, a number, a
/// boolean, null, an object or array literal or a regular expression.
fn text_like(argument: &Argument) -> bool {
    match argument.as_expression().map(|e| e.without_parentheses()) {
        None => true,
        Some(
            Expression::ArrowFunctionExpression(_)
            | Expression::FunctionExpression(_)
            | Expression::NumericLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::ObjectExpression(_)
            | Expression::ArrayExpression(_)
            | Expression::RegExpLiteral(_),
        ) => false,
        Some(_) => true,
    }
}

/// An expression's place in the script (its start and end).
fn span_of(expression: &Expression) -> (u32, u32) {
    let span = expression.span();
    (span.start, span.end)
}

/// The first name of a member chain (`document` in `document.forms[0].x`).
fn root_name(expression: &Expression) -> Option<String> {
    match expression.without_parentheses() {
        Expression::Identifier(id) => Some(id.name.to_string()),
        other => other.as_member_expression().and_then(|m| root_name(m.object())),
    }
}

/// Whether `expression` is `….name` (`el.dataset`, `el.attributes`).
fn under(expression: &Expression, name: &str) -> bool {
    expression.without_parentheses().as_member_expression().and_then(|m| m.static_property_name()).and_then(|n| n.as_str())
        == Some(name)
}

/// A timer's first argument written as text (a string, a template, a sum of strings), not a function.
fn first_is_text(arguments: &[Argument]) -> bool {
    matches!(
        arguments.first().and_then(|a| a.as_expression()).map(|e| e.without_parentheses()),
        Some(Expression::StringLiteral(_) | Expression::TemplateLiteral(_) | Expression::BinaryExpression(_))
    )
}

/// Attributes that hold a field's value or state, not its name: set or removed, no rename.
const STATE_ATTRIBUTES: &[&str] = &["value", "checked", "selected", "disabled", "readonly", "hidden", "style", "open"];

/// The attribute `setAttribute(name, …)` / `removeAttribute(name)` / `toggleAttribute(name)` names (`…NS(ns, name,
/// …)`: its second), written plainly.
fn attribute_named_first(arguments: &[Argument]) -> Option<String> {
    let first = arguments.first().and_then(|a| a.as_expression()).and_then(literal_name);
    let second = arguments.get(1).and_then(|a| a.as_expression()).and_then(literal_name);
    match (first, second) {
        (Some(ns), Some(name)) if ns.starts_with("http") => Some(name),
        (first, _) => first,
    }
}

/// The attribute `setAttribute(name, value)` / `setAttributeNS(ns, name, value)` sets, written plainly.
fn attribute_named(arguments: &[Argument]) -> Option<String> {
    arguments.iter().rev().nth(1).and_then(|a| a.as_expression()).and_then(literal_name)
}

/// `Object.assign(o, …)` on the script's own `o` (or a new `{}`).
fn first_is_own(arguments: &[Argument], own: &HashSet<String>) -> bool {
    match arguments.first().and_then(|a| a.as_expression()).map(|e| e.without_parentheses()) {
        Some(Expression::ObjectExpression(_) | Expression::ArrayExpression(_)) => true,
        Some(Expression::Identifier(id)) => own.contains(id.name.as_str()),
        _ => false,
    }
}

/// What `script` does (see the module's doc). Parsed as a script whose top level may `return` (the real server runs
/// it so), then as a module (a top-level `await`).
pub fn shape(script: &str) -> Shape {
    if script.len() > MAX_SCRIPT_BYTES || nesting(script) > MAX_NESTING {
        return Shape { too_big: true, ..Shape::default() };
    }
    // on a thread of its own with a stack for the deepest script the limits let through (the caller's may be a
    // runtime worker's 2 MB); one that can't be had, or a read that fails, counts as too big to read: refused
    let on_stack = |bytes: usize| {
        let owned = script.to_string();
        std::thread::Builder::new().name("script-shape".into()).stack_size(bytes).spawn(move || read(&owned))
    };
    // a system that won't reserve that much: a smaller stack, for a short script only
    let reading = match on_stack(STACK_BYTES) {
        Ok(thread) => Ok(thread),
        Err(_) if script.len() <= SHORT_SCRIPT_BYTES => on_stack(SHORT_STACK_BYTES),
        Err(exc) => Err(exc),
    };
    match reading.map(|thread| thread.join()) {
        Ok(Ok(shape)) => shape,
        _ => Shape { too_big: true, ..Shape::default() },
    }
}

/// When `STACK_BYTES` can't be had: scripts this short, on this stack.
const SHORT_SCRIPT_BYTES: usize = 2 * 1024;
const SHORT_STACK_BYTES: usize = 16 * 1024 * 1024;

/// How deep `script` nests, roughly and never less than the parser will recurse for brackets and prefix operators:
/// the deepest bracket (`(`, `[`, `{`, a template's `${`), and the longest run of `!`, `~`, `+`, `-` (spaces between
/// them included). Brackets inside strings count too: a script made only of them is no agent's.
pub fn nesting(script: &str) -> usize {
    let (mut depth, mut deepest, mut run, mut longest) = (0usize, 0usize, 0usize, 0usize);
    for c in script.chars() {
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
        match c {
            '!' | '~' | '+' | '-' => {
                run += 1;
                longest = longest.max(run);
            }
            ' ' | '\t' | '\n' | '\r' => {}
            _ => run = 0,
        }
    }
    deepest.max(longest)
}

fn read(script: &str) -> Shape {
    for source_type in [SourceType::script(), SourceType::mjs()] {
        let allocator = Allocator::default();
        let options = ParseOptions { allow_return_outside_function: true, ..ParseOptions::default() };
        let parsed = Parser::new(&allocator, script, source_type).with_options(options).parse();
        if parsed.diagnostics.has_errors() {
            continue;
        }
        let program = parsed.program;
        let mut names = Names::default();
        names.visit_program(&program);
        let mut reader = Reader {
            names: &names,
            uses: HashMap::new(),
            invoked: HashSet::new(),
            // the script's own top level: at once; in a module, a top-level `await` counts
            frames: vec![Frame { later: false, awaited: false }],
            blank: Vec::new(),
            shape: Shape { parsed: true, ..Shape::default() },
            location_reads: HashSet::new(),
            storage_reads: HashSet::new(),
            window_reads: HashSet::new(),
            strings: HashSet::new(),
            summed: HashSet::new(),
            plain_replaces: HashSet::new(),
        };
        reader.visit_program(&program);
        let mut code = script.as_bytes().to_vec();
        let comments = program.comments.iter().map(|c| (c.span.start, c.span.end));
        for (start, end) in reader.blank.iter().copied().chain(comments) {
            let (start, end) = (start as usize, (end as usize).min(code.len()));
            if start < end {
                code[start..end].iter_mut().for_each(|b| *b = b' ');
            }
        }
        let mut shape = reader.shape;
        shape.code = String::from_utf8(code).unwrap_or_default();
        return shape;
    }
    Shape::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_clicks_are_told_from_reading() {
        let read = shape("return [...document.querySelectorAll('h2')].map((h) => h.innerText)");
        assert!(read.parsed && !read.acts && !read.later);
        let own = shape("const o = {}; o.name = document.querySelector('h1').innerText; return o");
        assert!(!own.acts && !own.renames, "the script's own object");
        let local = shape("(()=>{const r={};['day','month','year'].forEach(k=>{r[k]=1});return JSON.stringify(r);})()");
        assert!(!local.acts && !local.later && !local.hidden_names, "{local:?}");
        assert!(shape("document.querySelector('#go').click()").acts);
        assert!(shape("document.querySelector('#q').value = 'x'").acts);
        assert!(shape("function set(id,v){const s=document.getElementById(id);s.value=v;} set('a','1')").acts);
        assert!(!shape("function set(id,v){const s=document.getElementById(id);s.value=v;} set('a','1')").acts_later);
    }

    #[test]
    fn what_runs_later_is_told_by_where_its_function_goes() {
        for script in [
            "setTimeout(() => { document.querySelector('#cc').value = '1'; }, 10)",
            "const tick = () => { const f = document.querySelector('#f'); if (f) f.value = '1'; else scheduler.postTask(tick, {delay: 100}); }; tick(); 'ok'",
            "const tick = () => navigator.locks.request('x', () => { document.querySelector('b').click(); }); tick();",
            "new MutationObserver(() => { document.querySelector('b').click(); }).observe(document.body, {childList: true})",
            "video.onended = () => { form.submit(); }",
            "Promise.resolve().then(() => f.submit())",
            "(async () => { await x; document.querySelector('#cc').value = v; })()",
            "setTimeout(btn.click.bind(btn), 100)",
            "document.addEventListener('x', {handleEvent() { b.click(); }})",
        ] {
            let s = shape(script);
            assert!(s.parsed && s.acts_later, "{script}: {s:?}");
        }
        for script in [
            "[d, m, y].forEach((e) => e.dispatchEvent(new Event('change', {bubbles: true})))",
            "(async () => { document.querySelector('#q').value = 'x'; document.querySelector('#s').submit(); })()",
            "Array.prototype.forEach.call(document.querySelectorAll('a'), (a) => a.click())",
            "new Promise((resolve) => { b.click(); resolve(); })",
            "const f = () => b.click(); f();",
        ] {
            let s = shape(script);
            assert!(s.parsed && !s.acts_later, "{script}: {s:?}");
        }
        // a function of the script's or the page's own, given to a timer: no text run
        assert!(!shape("function f() { return 1; } setTimeout(f, 10)").code_from_text);
        assert!(!shape("setTimeout(window.refresh, 10)").code_from_text);
        // later but only reading: told apart
        let reads_later = shape("setTimeout(() => console.log(document.title), 10)");
        assert!(reads_later.later && !reads_later.acts_later);
    }

    #[test]
    fn names_made_at_run_time_and_code_from_text() {
        for script in [
            "const a = document.createElement(String.fromCharCode(97)); a['hr' + 'ef'] = u; a.click()",
            "Reflect.set(f, 'act' + 'ion', u); f.submit()",
            "a[`hr${e}`] = u",
            "window['loc' + 'ation'] = u",
            "el['cli' + 'ck']()",
        ] {
            assert!(shape(script).hidden_names, "{script}");
        }
        for script in ["return ['Prix : ' + p, 'Stock : ' + s].join('\\n')", "const out = []; out[i] = x",
                       "const r = {}; r[k] = 1"] {
            assert!(!shape(script).hidden_names, "{script}");
        }
        for script in ["eval(c)", "new Function(c)()", "setTimeout('f.submit()', 10)", "el.setAttribute('onclick', 'pay()')",
                       "const c = 'f.submit()'; setTimeout(c, 10)", "setTimeout(String(x), 10)",
                       "[].constructor.constructor(c)()"] {
            assert!(shape(script).code_from_text, "{script}");
        }
    }

    #[test]
    fn a_tagged_template_a_timer_as_a_value_or_a_callee_without_a_name_runs_text() {
        for script in [
            "setTimeout`document.title = 1 //${1500}`; 'ok'",
            "setInterval`document.title = 1 //${1500}`; 'ok'",
            "window.setTimeout`document.title = 1 //${1500}`; 'ok'",
            "(0, setTimeout)`document.title = 1 //${1500}`; 'ok'",
            "Function`document.title = 1`(); 'ok'",
            "unknownTag`x`",
            "const t = setTimeout; t('b.click()', 10)",
            "const o = {t: window.setTimeout}; o.t('b.click()', 10)",
            "[f].forEach(setTimeout)",
            "setTimeout.call(window, 'b.click()', 10)",
            "(cond ? setTimeout : f)('b.click()', 10)",
            "getTimer()('b.click()', 10)",
            "(async () => {}).constructor('b.click()')()",
            "window['eval'](c)",
        ] {
            assert!(shape(script).code_from_text, "{script}");
        }
        for script in [
            "return String.raw`a\\b`",
            "function tag(s) { return s.join(''); } return tag`a${1}b`",
            "const $ = document.querySelector.bind(document); return $('#a').value",
            "const q = (s) => document.querySelector(s); return q('#a').value",
            "return x.constructor.name",
            "return document.body.innerText.search(/setTimeout/i)",
        ] {
            let s = shape(script);
            assert!(s.parsed && !s.code_from_text, "{script}: {s:?}");
        }
    }

    #[test]
    fn a_script_too_long_or_too_deep_is_refused_and_the_deepest_let_through_never_crashes() {
        assert!(shape(&format!("return {}1{}", "[".repeat(300), "]".repeat(300))).too_big);
        assert!(shape(&format!("return {}1", "!".repeat(300))).too_big);
        assert!(shape(&format!("return {}1", "- ".repeat(300))).too_big);
        assert!(shape(&"x;".repeat(MAX_SCRIPT_BYTES)).too_big);
        assert!(!shape(&format!("return {}1{}", "[".repeat(200), "]".repeat(200))).too_big);
        // the deepest each kind of nesting gets within the limits: read on the thread's stack, never a crash (a stack
        // overflow would abort this test binary)
        let fill = |unit: &str, end: &str| {
            let n = (MAX_SCRIPT_BYTES - 64) / unit.len();
            format!("return {}{end}", unit.repeat(n))
        };
        for script in [
            fill("a=", "1"),
            fill("a?a:", "1"),
            fill("a=>", "1"),
            fill("typeof ", "1"),
            fill("void ", "1"),
            fill("new ", "X"),
            fill("a+", "1"),
            fill("a.", "a"),
            format!("return a{}", "()".repeat((MAX_SCRIPT_BYTES - 64) / 2)),
            format!("{}1", "if(a)".repeat((MAX_SCRIPT_BYTES - 64) / 5)),
            format!("if(a){{}}{}", "else if(a){}".repeat((MAX_SCRIPT_BYTES - 64) / 13)),
        ] {
            let s = shape(&script);
            assert!(s.parsed || s.too_big, "{}…", &script[..40]);
        }
    }

    #[test]
    fn renames_and_the_code_without_its_strings() {
        assert!(shape("document.getElementById('numero').dataset.kind = 'iban'").renames);
        assert!(shape("document.querySelector('label').textContent = 'IBAN'").renames);
        assert!(shape("document.getElementById('numero').id = 'x'").renames);
        assert!(!shape("const x = {}; x.title = document.title; return x").renames);
        let s = shape("return document.body.innerText.indexOf('observer') /* new MutationObserver */ + /onclick=/.source");
        assert!(s.parsed && !s.code.contains("observer") && !s.code.contains("MutationObserver") && !s.code.contains("onclick"));
        assert!(!shape("this is not { javascript").parsed);
        assert!(shape("const r = await fetch(u); return r.status").parsed, "a top-level await");
    }
}
