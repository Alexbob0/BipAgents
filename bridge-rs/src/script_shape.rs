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
use oxc_syntax::operator::UnaryOperator;
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
    /// It runs code from text: eval, Function, a timer given a string, an `on…` attribute set.
    pub code_from_text: bool,
}

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
    functions: HashMap<u32, String>,
    called: HashSet<u32>,
    passed_at_once: HashSet<u32>,
    escaping: HashSet<String>,
}

impl<'a> Visit<'a> for Names {
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
        if let Expression::Identifier(id) = it.callee.without_parentheses() {
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
}

impl Reader<'_> {
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
        let callee = it.callee.without_parentheses();
        self.invoked.insert(span_of(callee));
        // the functions given: at once (an IIFE, the array's methods…) or maybe later
        if let Some(start) = function_start(callee) {
            self.uses.insert(start, Use::Now);
        }
        if let Some(member) = callee.as_member_expression() {
            // `(function () {…}).call(this)`: at once too
            if matches!(member.static_property_name().and_then(|n| n.as_str()), Some("call" | "apply")) {
                if let Some(start) = function_start(member.object().without_parentheses()) {
                    self.uses.insert(start, Use::Now);
                }
                self.invoked.insert(span_of(member.object().without_parentheses()));
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
            }
        }
        walk::walk_unary_expression(self, it);
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
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
        self.blank.push((it.span.start + 1, it.span.end.saturating_sub(1)));
    }

    fn visit_template_element(&mut self, it: &TemplateElement<'a>) {
        self.blank.push((it.span.start, it.span.end));
    }

    fn visit_reg_exp_literal(&mut self, it: &RegExpLiteral<'a>) {
        self.blank.push((it.span.start + 1, it.span.end.saturating_sub(1)));
    }
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
