//! PyYAML's `yaml.safe_load` and `yaml.safe_dump(data, allow_unicode=True, sort_keys=False)`, for the files the bridge
//! reads and rewrites in Hermes' profiles (config.yaml, profile.yaml): Hermes reads them with PyYAML, and the Python
//! bridge wrote them with it, so the Rust bridge writes the same bytes and reads the same values.
//!
//! - loading follows YAML 1.1 as PyYAML does (`yes`/`no`/`on`/`off` are booleans, `0755` is octal, `1:30` is
//!   sexagesimal, dates and timestamps, merge keys `<<`), on yaml-rust2's events (which keep each scalar's style);
//! - dumping is PyYAML's emitter, block style (`default_flow_style=False`): its quoting choices, its 80-column folding
//!   of long scalars, `[]`/`{}` for empty collections, indentless sequences in mappings.
//!
//! Not reproduced: anchors and aliases on dump (PyYAML writes `&id001` for a list or dict met twice, which only a
//! file with aliases gives), `!!binary`, `!!set`, `!!omap` and `!!pairs` (refused when loading, as an unknown tag).
//!
//! Helper of the agents group (agents_admin, own_skills, services, shares); other modules may use it.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;
use yaml_rust2::parser::{Event, Parser, Tag};
use yaml_rust2::scanner::TScalarStyle;

/// A value as PyYAML's safe loader makes it (Python's dict keeps its order, its keys need not be strings).
#[derive(Debug, Clone, PartialEq)]
pub enum Yaml {
    Null,
    Bool(bool),
    Int(i128),
    Float(f64),
    Str(String),
    /// A date or a datetime, held as PyYAML writes it back (`date.isoformat()`, `datetime.isoformat(' ')`).
    Timestamp(String),
    Seq(Vec<Yaml>),
    Map(Vec<(Yaml, Yaml)>),
}

/// `yaml.YAMLError` (and the `ValueError` of a bad explicit `!!int`…): the text says what was wrong.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct YamlError(pub String);

// -- Python dict semantics ------------------------------------------------------------------------------------------

/// Python's `==` between two dict keys (1 == 1.0 == True).
fn key_eq(a: &Yaml, b: &Yaml) -> bool {
    fn number(v: &Yaml) -> Option<f64> {
        match v {
            Yaml::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Yaml::Int(i) => Some(*i as f64),
            Yaml::Float(f) => Some(*f),
            _ => None,
        }
    }
    match (a, b) {
        (Yaml::Int(x), Yaml::Int(y)) => x == y,
        _ => match (number(a), number(b)) {
            (Some(x), Some(y)) => x == y,
            _ => a == b,
        },
    }
}

impl Yaml {
    pub fn str(text: impl Into<String>) -> Yaml {
        Yaml::Str(text.into())
    }

    /// An empty dict.
    pub fn map() -> Yaml {
        Yaml::Map(Vec::new())
    }

    pub fn is_map(&self) -> bool {
        matches!(self, Yaml::Map(_))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Yaml::Str(s) => Some(s),
            _ => None,
        }
    }

    /// `d.get(key)` for a string key (None: not a dict, or no such key).
    pub fn get(&self, key: &str) -> Option<&Yaml> {
        match self {
            Yaml::Map(pairs) => pairs.iter().find(|(k, _)| matches!(k, Yaml::Str(s) if s == key)).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Yaml> {
        match self {
            Yaml::Map(pairs) => pairs.iter_mut().find(|(k, _)| matches!(k, Yaml::Str(s) if s == key)).map(|(_, v)| v),
            _ => None,
        }
    }

    /// `d[key] = value` (an existing key keeps its place). Does nothing on a non-dict.
    pub fn insert(&mut self, key: &str, value: Yaml) {
        if let Yaml::Map(pairs) = self {
            insert_pair(pairs, Yaml::str(key), value);
        }
    }

    /// `d.pop(key, None)`.
    pub fn remove(&mut self, key: &str) -> Option<Yaml> {
        match self {
            Yaml::Map(pairs) => {
                let at = pairs.iter().position(|(k, _)| matches!(k, Yaml::Str(s) if s == key))?;
                Some(pairs.remove(at).1)
            }
            _ => None,
        }
    }

    /// `d.setdefault(key, {})` → the value (made a dict when missing).
    pub fn entry_map(&mut self, key: &str) -> Option<&mut Yaml> {
        if self.get(key).is_none() {
            self.insert(key, Yaml::map());
        }
        self.get_mut(key)
    }

    /// Python's truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            Yaml::Null => false,
            Yaml::Bool(b) => *b,
            Yaml::Int(i) => *i != 0,
            Yaml::Float(f) => *f != 0.0,
            Yaml::Str(s) => !s.is_empty(),
            Yaml::Timestamp(_) => true,
            Yaml::Seq(items) => !items.is_empty(),
            Yaml::Map(pairs) => !pairs.is_empty(),
        }
    }

    /// Python's `str(value)` of a scalar (a list or dict: its JSON form, which no caller relies on).
    pub fn py_str(&self) -> String {
        match self {
            Yaml::Null => "None".into(),
            Yaml::Bool(true) => "True".into(),
            Yaml::Bool(false) => "False".into(),
            Yaml::Int(i) => i.to_string(),
            Yaml::Float(f) => crate::py::float_repr(*f),
            Yaml::Str(s) | Yaml::Timestamp(s) => s.clone(),
            other => crate::py::py_str(&to_json(other)),
        }
    }
}

fn insert_pair(pairs: &mut Vec<(Yaml, Yaml)>, key: Yaml, value: Yaml) {
    if let Some(slot) = pairs.iter_mut().find(|(k, _)| key_eq(k, &key)) {
        slot.1 = value;
    } else {
        pairs.push((key, value));
    }
}

// -- JSON views -----------------------------------------------------------------------------------------------------

/// The value as JSON, for code that reads it like a Python dict: keys as Python's `str(key)`, dates as their text.
pub fn to_json(value: &Yaml) -> Value {
    match value {
        Yaml::Null => Value::Null,
        Yaml::Bool(b) => Value::Bool(*b),
        Yaml::Int(i) => {
            if let Ok(v) = i64::try_from(*i) {
                Value::from(v)
            } else if let Ok(v) = u64::try_from(*i) {
                Value::from(v)
            } else {
                serde_json::Number::from_f64(*i as f64).map(Value::Number).unwrap_or(Value::Null)
            }
        }
        Yaml::Float(f) => serde_json::Number::from_f64(*f).map(Value::Number).unwrap_or(Value::Null),
        Yaml::Str(s) | Yaml::Timestamp(s) => Value::String(s.clone()),
        Yaml::Seq(items) => Value::Array(items.iter().map(to_json).collect()),
        Yaml::Map(pairs) => {
            let mut map = serde_json::Map::new();
            for (k, v) in pairs {
                map.insert(k.py_str(), to_json(v));
            }
            Value::Object(map)
        }
    }
}

/// A JSON value as the Python object `json.loads` gives (ints stay ints, floats floats).
pub fn from_json(value: &Value) -> Yaml {
    match value {
        Value::Null => Yaml::Null,
        Value::Bool(b) => Yaml::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Yaml::Int(i as i128)
            } else if let Some(u) = n.as_u64() {
                Yaml::Int(u as i128)
            } else {
                Yaml::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => Yaml::Str(s.clone()),
        Value::Array(items) => Yaml::Seq(items.iter().map(from_json).collect()),
        Value::Object(map) => Yaml::Map(map.iter().map(|(k, v)| (Yaml::Str(k.clone()), from_json(v))).collect()),
    }
}

// -- the resolver (YAML 1.1's implicit types, as PyYAML's) --------------------------------------------------------

const TAG_PREFIX: &str = "tag:yaml.org,2002:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Bool,
    Float,
    Int,
    Merge,
    Null,
    Timestamp,
    Value,
    Yaml,
    Str,
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid regex")
}

static BOOL: LazyLock<Regex> =
    LazyLock::new(|| re(r"^(?:yes|Yes|YES|no|No|NO|true|True|TRUE|false|False|FALSE|on|On|ON|off|Off|OFF)\z"));
static FLOAT: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"^(?:[-+]?(?:[0-9][0-9_]*)\.[0-9_]*(?:[eE][-+][0-9]+)?|\.[0-9][0-9_]*(?:[eE][-+][0-9]+)?|[-+]?[0-9][0-9_]*(?::[0-5]?[0-9])+\.[0-9_]*|[-+]?\.(?:inf|Inf|INF)|\.(?:nan|NaN|NAN))\z",
    )
});
static INT: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"^(?:[-+]?0b[0-1_]+|[-+]?0[0-7_]+|[-+]?(?:0|[1-9][0-9_]*)|[-+]?0x[0-9a-fA-F_]+|[-+]?[1-9][0-9_]*(?::[0-5]?[0-9])+)\z",
    )
});
static MERGE: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:<<)\z"));
static NULL: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:~|null|Null|NULL|)\z"));
static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"^(?:[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]|[0-9][0-9][0-9][0-9]-[0-9][0-9]?-[0-9][0-9]?(?:[Tt]|[ \t]+)[0-9][0-9]?:[0-9][0-9]:[0-9][0-9](?:\.[0-9]*)?(?:[ \t]*(?:Z|[-+][0-9][0-9]?(?::[0-9][0-9])?))?)\z",
    )
});
static VALUE: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:=)\z"));
static YAML_TAG: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:!|&|\*)\z"));

/// Python's `re.match(pattern_ending_in_$, text)`: `$` also matches before a final newline.
fn py_match(regex: &Regex, text: &str) -> bool {
    regex.is_match(text) || text.strip_suffix('\n').map(|t| regex.is_match(t)).unwrap_or(false)
}

/// The tag a plain scalar resolves to (PyYAML's `resolve(ScalarNode, value, (True, False))`).
fn resolve_plain(value: &str) -> Kind {
    let first = value.chars().next();
    let candidates: &[(Kind, &LazyLock<Regex>)] = match first {
        None => &[(Kind::Null, &NULL)],
        Some(c) if "yYtTfFoO".contains(c) => &[(Kind::Bool, &BOOL)],
        Some(c) if "nN".contains(c) => &[(Kind::Bool, &BOOL), (Kind::Null, &NULL)],
        Some(c) if c.is_ascii_digit() => &[(Kind::Float, &FLOAT), (Kind::Int, &INT), (Kind::Timestamp, &TIMESTAMP)],
        Some('-') | Some('+') => &[(Kind::Float, &FLOAT), (Kind::Int, &INT)],
        Some('.') => &[(Kind::Float, &FLOAT)],
        Some('<') => &[(Kind::Merge, &MERGE)],
        Some('~') => &[(Kind::Null, &NULL)],
        Some('=') => &[(Kind::Value, &VALUE)],
        Some('!') | Some('&') | Some('*') => &[(Kind::Yaml, &YAML_TAG)],
        _ => &[],
    };
    for (kind, regex) in candidates {
        if py_match(regex, value) {
            return *kind;
        }
    }
    Kind::Str
}

// -- loading --------------------------------------------------------------------------------------------------------

/// `yaml.safe_load(text)`: None (`Yaml::Null`) for an empty document.
pub fn load(text: &str) -> Result<Yaml, YamlError> {
    let mut parser = Parser::new_from_str(text);
    let mut loader = Loader { anchors: HashMap::new() };
    let mut document: Option<Yaml> = None;
    loop {
        let (event, mark) = parser.next_token().map_err(|e| YamlError(e.to_string()))?;
        match event {
            Event::StreamStart | Event::DocumentEnd | Event::Nothing => {}
            Event::StreamEnd => break,
            Event::DocumentStart => {
                if document.is_some() {
                    return Err(YamlError(format!(
                        "expected a single document in the stream, but found another document (line {}, column {})",
                        mark.line(),
                        mark.col() + 1
                    )));
                }
                let (event, _) = parser.next_token().map_err(|e| YamlError(e.to_string()))?;
                document = Some(loader.node(&mut parser, event)?);
            }
            other => {
                // A document without an explicit start event (yaml-rust2 always sends one; kept for safety).
                if document.is_some() {
                    return Err(YamlError("expected a single document in the stream".into()));
                }
                document = Some(loader.node(&mut parser, other)?);
            }
        }
    }
    Ok(document.unwrap_or(Yaml::Null))
}

struct Loader {
    anchors: HashMap<usize, Yaml>,
}

impl Loader {
    fn next(&mut self, parser: &mut Parser<std::str::Chars<'_>>) -> Result<Event, YamlError> {
        parser.next_token().map(|(e, _)| e).map_err(|e| YamlError(e.to_string()))
    }

    fn remember(&mut self, anchor: usize, value: &Yaml) {
        if anchor > 0 {
            self.anchors.insert(anchor, value.clone());
        }
    }

    fn node(&mut self, parser: &mut Parser<std::str::Chars<'_>>, event: Event) -> Result<Yaml, YamlError> {
        match event {
            Event::Alias(id) => {
                self.anchors.get(&id).cloned().ok_or_else(|| YamlError(format!("found undefined alias {id}")))
            }
            Event::Scalar(value, style, anchor, tag) => {
                let built = scalar(&value, style, tag.as_ref())?;
                self.remember(anchor, &built);
                Ok(built)
            }
            Event::SequenceStart(anchor, tag) => {
                check_collection_tag(tag.as_ref(), "seq")?;
                let mut items = Vec::new();
                loop {
                    match self.next(parser)? {
                        Event::SequenceEnd => break,
                        event => items.push(self.node(parser, event)?),
                    }
                }
                let built = Yaml::Seq(items);
                self.remember(anchor, &built);
                Ok(built)
            }
            Event::MappingStart(anchor, tag) => {
                check_collection_tag(tag.as_ref(), "map")?;
                let mut merged: Vec<(Yaml, Yaml)> = Vec::new();
                let mut own: Vec<(Yaml, Yaml)> = Vec::new();
                loop {
                    let key_event = match self.next(parser)? {
                        Event::MappingEnd => break,
                        event => event,
                    };
                    let is_merge = matches!(&key_event, Event::Scalar(v, TScalarStyle::Plain, _, None) if resolve_plain(v) == Kind::Merge)
                        || matches!(&key_event, Event::Scalar(_, _, _, Some(t)) if t.handle == TAG_PREFIX && t.suffix == "merge");
                    let key = if is_merge { Yaml::Null } else { self.node(parser, key_event)? };
                    let value_event = self.next(parser)?;
                    let value = self.node(parser, value_event)?;
                    if is_merge {
                        // PyYAML's flatten_mapping: the merged pairs first, the mapping's own after (they win).
                        match value {
                            Yaml::Map(pairs) => merged.extend(pairs),
                            Yaml::Seq(maps) => {
                                let mut parts = Vec::new();
                                for item in maps {
                                    match item {
                                        Yaml::Map(pairs) => parts.push(pairs),
                                        _ => {
                                            return Err(YamlError(
                                                "while constructing a mapping: expected a mapping for merging".into(),
                                            ))
                                        }
                                    }
                                }
                                for pairs in parts.into_iter().rev() {
                                    merged.extend(pairs);
                                }
                            }
                            _ => {
                                return Err(YamlError(
                                    "while constructing a mapping: expected a mapping or list of mappings for merging"
                                        .into(),
                                ))
                            }
                        }
                        continue;
                    }
                    if matches!(key, Yaml::Seq(_) | Yaml::Map(_)) {
                        return Err(YamlError("while constructing a mapping: found unhashable key".into()));
                    }
                    own.push((key, value));
                }
                let mut pairs = Vec::new();
                for (k, v) in merged.into_iter().chain(own) {
                    insert_pair(&mut pairs, k, v);
                }
                let built = Yaml::Map(pairs);
                self.remember(anchor, &built);
                Ok(built)
            }
            other => Err(YamlError(format!("unexpected event {other:?}"))),
        }
    }
}

fn check_collection_tag(tag: Option<&Tag>, expected: &str) -> Result<(), YamlError> {
    match tag {
        None => Ok(()),
        Some(t) if t.handle == TAG_PREFIX && t.suffix == expected => Ok(()),
        Some(t) if t.handle.is_empty() && t.suffix == "!" => Ok(()),
        Some(t) => Err(YamlError(format!("could not determine a constructor for the tag '{}{}'", t.handle, t.suffix))),
    }
}

fn scalar(value: &str, style: TScalarStyle, tag: Option<&Tag>) -> Result<Yaml, YamlError> {
    let kind = match tag {
        None if style == TScalarStyle::Plain => resolve_plain(value),
        None => Kind::Str,
        Some(t) if t.handle.is_empty() && t.suffix == "!" => Kind::Str,
        Some(t) if t.handle == TAG_PREFIX => match t.suffix.as_str() {
            "str" => Kind::Str,
            "int" => Kind::Int,
            "float" => Kind::Float,
            "bool" => Kind::Bool,
            "null" => Kind::Null,
            "timestamp" => Kind::Timestamp,
            "value" => Kind::Str,
            other => {
                return Err(YamlError(format!("could not determine a constructor for the tag '{TAG_PREFIX}{other}'")))
            }
        },
        Some(t) => {
            return Err(YamlError(format!("could not determine a constructor for the tag '{}{}'", t.handle, t.suffix)))
        }
    };
    match kind {
        Kind::Null => Ok(Yaml::Null),
        Kind::Bool => match value.to_lowercase().as_str() {
            "yes" | "true" | "on" => Ok(Yaml::Bool(true)),
            "no" | "false" | "off" => Ok(Yaml::Bool(false)),
            _ => Err(YamlError(format!("invalid boolean {value:?}"))),
        },
        Kind::Int => construct_int(value).map(Yaml::Int),
        Kind::Float => construct_float(value).map(Yaml::Float),
        Kind::Timestamp => construct_timestamp(value).map(Yaml::Timestamp),
        // `=` alone is the « value » key; `!`, `&`, `*` cannot be plain scalars: strings, as PyYAML builds them.
        Kind::Str | Kind::Value | Kind::Yaml | Kind::Merge => Ok(Yaml::Str(value.to_string())),
    }
}

fn int_error(value: &str) -> YamlError {
    YamlError(format!("invalid literal for int(): {value:?}"))
}

/// PyYAML's `construct_yaml_int`.
fn construct_int(text: &str) -> Result<i128, YamlError> {
    let mut value: String = text.replace('_', "");
    let mut sign: i128 = 1;
    if value.starts_with('-') {
        sign = -1;
    }
    if value.starts_with('-') || value.starts_with('+') {
        value = value[1..].to_string();
    }
    let parse = |digits: &str, radix: u32| -> Result<i128, YamlError> {
        if digits.is_empty() {
            return Err(int_error(text));
        }
        i128::from_str_radix(digits, radix).map_err(|_| int_error(text))
    };
    if value == "0" {
        return Ok(0);
    }
    if let Some(rest) = value.strip_prefix("0b") {
        return Ok(sign * parse(rest, 2)?);
    }
    if let Some(rest) = value.strip_prefix("0x") {
        return Ok(sign * parse(rest, 16)?);
    }
    if value.starts_with('0') {
        return Ok(sign * parse(&value, 8)?);
    }
    if value.contains(':') {
        let mut total: i128 = 0;
        let mut base: i128 = 1;
        for part in value.split(':').rev() {
            total += parse(part, 10)? * base;
            base *= 60;
        }
        return Ok(sign * total);
    }
    Ok(sign * parse(&value, 10)?)
}

/// PyYAML's `construct_yaml_float`.
fn construct_float(text: &str) -> Result<f64, YamlError> {
    let mut value = text.replace('_', "").to_lowercase();
    let mut sign = 1.0;
    if value.starts_with('-') {
        sign = -1.0;
    }
    if value.starts_with('-') || value.starts_with('+') {
        value = value[1..].to_string();
    }
    let parse = |digits: &str| -> Result<f64, YamlError> {
        digits.parse::<f64>().map_err(|_| YamlError(format!("could not convert string to float: {text:?}")))
    };
    if value == ".inf" {
        return Ok(sign * f64::INFINITY);
    }
    if value == ".nan" {
        return Ok(f64::NAN);
    }
    if value.contains(':') {
        let mut total = 0.0;
        let mut base = 1.0;
        for part in value.split(':').rev() {
            total += parse(part)? * base;
            base *= 60.0;
        }
        return Ok(sign * total);
    }
    Ok(sign * parse(&value)?)
}

static TIMESTAMP_PARTS: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"^(?P<year>[0-9][0-9][0-9][0-9])-(?P<month>[0-9][0-9]?)-(?P<day>[0-9][0-9]?)(?:(?:[Tt]|[ \t]+)(?P<hour>[0-9][0-9]?):(?P<minute>[0-9][0-9]):(?P<second>[0-9][0-9])(?:\.(?P<fraction>[0-9]*))?(?:[ \t]*(?P<tz>Z|(?P<tz_sign>[-+])(?P<tz_hour>[0-9][0-9]?)(?::(?P<tz_minute>[0-9][0-9]))?))?)?\n?\z",
    )
});

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        _ => 28,
    }
}

/// PyYAML's `construct_yaml_timestamp`, as the text its representer writes back.
fn construct_timestamp(text: &str) -> Result<String, YamlError> {
    let caps = TIMESTAMP_PARTS.captures(text).ok_or_else(|| YamlError(format!("invalid timestamp {text:?}")))?;
    let num = |name: &str| caps.name(name).map(|m| m.as_str().parse::<i64>().unwrap_or(0));
    let (year, month, day) = (num("year").unwrap_or(0), num("month").unwrap_or(0), num("day").unwrap_or(0));
    let bad = || YamlError(format!("invalid date in {text:?}"));
    if year < 1 || !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return Err(bad());
    }
    let date = format!("{year:04}-{month:02}-{day:02}");
    let Some(hour) = num("hour") else { return Ok(date) };
    let (minute, second) = (num("minute").unwrap_or(0), num("second").unwrap_or(0));
    if hour > 23 || minute > 59 || second > 59 {
        return Err(bad());
    }
    let mut out = format!("{date} {hour:02}:{minute:02}:{second:02}");
    if let Some(fraction) = caps.name("fraction").map(|m| m.as_str()).filter(|f| !f.is_empty()) {
        let mut digits: String = fraction.chars().take(6).collect();
        while digits.len() < 6 {
            digits.push('0');
        }
        if digits.parse::<u32>().unwrap_or(0) != 0 {
            out.push('.');
            out.push_str(&digits);
        }
    }
    if let Some(sign) = caps.name("tz_sign").map(|m| m.as_str()) {
        let (h, m) = (num("tz_hour").unwrap_or(0), num("tz_minute").unwrap_or(0));
        let minutes = h * 60 + m;
        if minutes >= 24 * 60 {
            return Err(bad());
        }
        let negative = sign == "-" && minutes != 0;
        out.push_str(&format!("{}{:02}:{:02}", if negative { '-' } else { '+' }, minutes / 60, minutes % 60));
    } else if caps.name("tz").is_some() {
        out.push_str("+00:00");
    }
    Ok(out)
}

// -- dumping (PyYAML's Emitter) -------------------------------------------------------------------------------------

/// `yaml.safe_dump(data, allow_unicode=True, sort_keys=False)`.
pub fn dump(data: &Yaml) -> String {
    let mut emitter = Emitter::new();
    emitter.document(data);
    emitter.out
}

/// `yaml.safe_dump(json_value, allow_unicode=True, sort_keys=False)`.
pub fn dump_json(data: &Value) -> String {
    dump(&from_json(data))
}

const BREAKS: [char; 4] = ['\n', '\u{85}', '\u{2028}', '\u{2029}'];

fn is_break(c: char) -> bool {
    BREAKS.contains(&c)
}

/// Whitespace for the analysis (`'\0 \t\r\n\x85\u2028\u2029'`).
fn is_space_or_break(c: char) -> bool {
    matches!(c, '\0' | ' ' | '\t' | '\r' | '\n' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

struct Analysis {
    empty: bool,
    multiline: bool,
    allow_flow_plain: bool,
    allow_block_plain: bool,
    allow_single_quoted: bool,
}

/// PyYAML's `Emitter.analyze_scalar` (allow_unicode=True).
fn analyze_scalar(scalar: &[char]) -> Analysis {
    if scalar.is_empty() {
        return Analysis {
            empty: true,
            multiline: false,
            allow_flow_plain: false,
            allow_block_plain: true,
            allow_single_quoted: true,
        };
    }
    let mut block_indicators = false;
    let mut flow_indicators = false;
    let mut line_breaks = false;
    let mut special_characters = false;
    let mut leading_space = false;
    let mut leading_break = false;
    let mut trailing_space = false;
    let mut trailing_break = false;
    let mut break_space = false;
    let mut space_break = false;
    let starts = |p: &str| scalar.len() >= 3 && scalar[..3].iter().collect::<String>() == p;
    if starts("---") || starts("...") {
        block_indicators = true;
        flow_indicators = true;
    }
    let mut preceded_by_whitespace = true;
    let mut followed_by_whitespace = scalar.len() == 1 || is_space_or_break(scalar[1]);
    let mut previous_space = false;
    let mut previous_break = false;
    let len = scalar.len();
    let mut index = 0;
    while index < len {
        let ch = scalar[index];
        if index == 0 {
            if "#,[]{}&*!|>'\"%@`".contains(ch) {
                flow_indicators = true;
                block_indicators = true;
            }
            if ch == '?' || ch == ':' {
                flow_indicators = true;
                if followed_by_whitespace {
                    block_indicators = true;
                }
            }
            if ch == '-' && followed_by_whitespace {
                flow_indicators = true;
                block_indicators = true;
            }
        } else {
            if ",?[]{}".contains(ch) {
                flow_indicators = true;
            }
            if ch == ':' {
                flow_indicators = true;
                if followed_by_whitespace {
                    block_indicators = true;
                }
            }
            if ch == '#' && preceded_by_whitespace {
                flow_indicators = true;
                block_indicators = true;
            }
        }
        if is_break(ch) {
            line_breaks = true;
        }
        if !(ch == '\n' || ('\x20'..='\x7E').contains(&ch)) {
            let unicode = (ch == '\u{85}'
                || ('\u{A0}'..='\u{D7FF}').contains(&ch)
                || ('\u{E000}'..='\u{FFFD}').contains(&ch)
                || ('\u{10000}'..'\u{10FFFF}').contains(&ch))
                && ch != '\u{FEFF}';
            if !unicode {
                special_characters = true;
            }
        }
        if ch == ' ' {
            if index == 0 {
                leading_space = true;
            }
            if index == len - 1 {
                trailing_space = true;
            }
            if previous_break {
                break_space = true;
            }
            previous_space = true;
            previous_break = false;
        } else if is_break(ch) {
            if index == 0 {
                leading_break = true;
            }
            if index == len - 1 {
                trailing_break = true;
            }
            if previous_space {
                space_break = true;
            }
            previous_space = false;
            previous_break = true;
        } else {
            previous_space = false;
            previous_break = false;
        }
        index += 1;
        preceded_by_whitespace = is_space_or_break(ch);
        followed_by_whitespace = index + 1 >= len || is_space_or_break(scalar[index + 1]);
    }
    let mut allow_flow_plain = true;
    let mut allow_block_plain = true;
    let mut allow_single_quoted = true;
    if leading_space || leading_break || trailing_space || trailing_break {
        allow_flow_plain = false;
        allow_block_plain = false;
    }
    if break_space {
        allow_flow_plain = false;
        allow_block_plain = false;
        allow_single_quoted = false;
    }
    if space_break || special_characters {
        allow_flow_plain = false;
        allow_block_plain = false;
        allow_single_quoted = false;
    }
    if line_breaks {
        allow_flow_plain = false;
        allow_block_plain = false;
    }
    if flow_indicators {
        allow_flow_plain = false;
    }
    if block_indicators {
        allow_block_plain = false;
    }
    Analysis { empty: false, multiline: line_breaks, allow_flow_plain, allow_block_plain, allow_single_quoted }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Plain,
    Single,
    Double,
}

/// A scalar as the representer makes it: its text, whether it is a string, and its tag's short form.
struct ScalarNode {
    text: String,
    is_str: bool,
    tag: &'static str,
}

fn float_text(f: f64) -> String {
    if f.is_nan() {
        return ".nan".into();
    }
    if f == f64::INFINITY {
        return ".inf".into();
    }
    if f == f64::NEG_INFINITY {
        return "-.inf".into();
    }
    let value = crate::py::float_repr(f).to_lowercase();
    if !value.contains('.') && value.contains('e') {
        value.replacen('e', ".0e", 1)
    } else {
        value
    }
}

fn represent_scalar(value: &Yaml) -> Option<ScalarNode> {
    let (text, is_str, tag) = match value {
        Yaml::Null => ("null".to_string(), false, "!!null"),
        Yaml::Bool(b) => ((if *b { "true" } else { "false" }).to_string(), false, "!!bool"),
        Yaml::Int(i) => (i.to_string(), false, "!!int"),
        Yaml::Float(f) => (float_text(*f), false, "!!float"),
        Yaml::Str(s) => (s.clone(), true, "!!str"),
        Yaml::Timestamp(s) => (s.clone(), false, "!!timestamp"),
        _ => return None,
    };
    Some(ScalarNode { text, is_str, tag })
}

struct Emitter {
    out: String,
    indents: Vec<Option<usize>>,
    indent: Option<usize>,
    flow_level: usize,
    root_context: bool,
    mapping_context: bool,
    simple_key_context: bool,
    column: usize,
    whitespace: bool,
    indention: bool,
    open_ended: bool,
    best_indent: usize,
    best_width: usize,
}

impl Emitter {
    fn new() -> Self {
        Emitter {
            out: String::new(),
            indents: Vec::new(),
            indent: None,
            flow_level: 0,
            root_context: false,
            mapping_context: false,
            simple_key_context: false,
            column: 0,
            whitespace: true,
            indention: true,
            open_ended: false,
            best_indent: 2,
            best_width: 80,
        }
    }

    fn document(&mut self, data: &Yaml) {
        self.node_in(data, true, false, false);
        self.write_indent();
        if self.open_ended {
            self.write_indicator("...", true, false, false);
            self.write_indent();
        }
    }

    fn increase_indent(&mut self, flow: bool, indentless: bool) {
        self.indents.push(self.indent);
        match self.indent {
            None => self.indent = Some(if flow { self.best_indent } else { 0 }),
            Some(i) if !indentless => self.indent = Some(i + self.best_indent),
            Some(_) => {}
        }
    }

    fn pop_indent(&mut self) {
        self.indent = self.indents.pop().unwrap_or(None);
    }

    fn node_in(&mut self, data: &Yaml, root: bool, mapping: bool, simple_key: bool) {
        self.root_context = root;
        self.mapping_context = mapping;
        self.simple_key_context = simple_key;
        match data {
            Yaml::Seq(items) => {
                if self.flow_level > 0 || items.is_empty() {
                    self.write_indicator("[", true, true, false);
                    self.flow_level += 1;
                    self.increase_indent(true, false);
                    // Only empty sequences are written in flow style here.
                    self.pop_indent();
                    self.flow_level -= 1;
                    self.write_indicator("]", false, false, false);
                } else {
                    let indentless = self.mapping_context && !self.indention;
                    self.increase_indent(false, indentless);
                    for item in items {
                        self.write_indent();
                        self.write_indicator("-", true, false, true);
                        self.node_in(item, false, false, false);
                    }
                    self.pop_indent();
                }
            }
            Yaml::Map(pairs) => {
                if self.flow_level > 0 || pairs.is_empty() {
                    self.write_indicator("{", true, true, false);
                    self.flow_level += 1;
                    self.increase_indent(true, false);
                    self.pop_indent();
                    self.flow_level -= 1;
                    self.write_indicator("}", false, false, false);
                } else {
                    self.increase_indent(false, false);
                    for (key, value) in pairs {
                        self.write_indent();
                        if check_simple_key(key) {
                            self.node_in(key, false, true, true);
                            self.write_indicator(":", false, false, false);
                            self.node_in(value, false, true, false);
                        } else {
                            self.write_indicator("?", true, false, true);
                            self.node_in(key, false, true, false);
                            self.write_indent();
                            self.write_indicator(":", true, false, true);
                            self.node_in(value, false, true, false);
                        }
                    }
                    self.pop_indent();
                }
            }
            scalar => {
                let node = represent_scalar(scalar).expect("a scalar");
                self.increase_indent(true, false);
                self.process_scalar(&node);
                self.pop_indent();
            }
        }
    }

    fn choose_scalar_style(&self, node: &ScalarNode, analysis: &Analysis) -> Style {
        // implicit[0]: the plain text reads back as this value's type (a string: as a string).
        let plain_reads_back = if node.is_str { resolve_plain(&node.text) == Kind::Str } else { true };
        if plain_reads_back
            && !(self.simple_key_context && (analysis.empty || analysis.multiline))
            && ((self.flow_level > 0 && analysis.allow_flow_plain)
                || (self.flow_level == 0 && analysis.allow_block_plain))
        {
            return Style::Plain;
        }
        if analysis.allow_single_quoted && !(self.simple_key_context && analysis.multiline) {
            return Style::Single;
        }
        Style::Double
    }

    fn process_scalar(&mut self, node: &ScalarNode) {
        let chars: Vec<char> = node.text.chars().collect();
        let analysis = analyze_scalar(&chars);
        let style = self.choose_scalar_style(node, &analysis);
        let split = !self.simple_key_context;
        match style {
            Style::Double => self.write_double_quoted(&chars, split),
            Style::Single => self.write_single_quoted(&chars, split),
            Style::Plain => self.write_plain(&chars, split),
        }
    }

    // -- writers ---------------------------------------------------------------------------------------------------

    fn write(&mut self, data: &str) {
        self.out.push_str(data);
    }

    fn write_chars(&mut self, data: &[char]) {
        self.column += data.len();
        self.out.extend(data.iter());
    }

    fn write_indicator(&mut self, indicator: &str, need_whitespace: bool, whitespace: bool, indention: bool) {
        let data = if self.whitespace || !need_whitespace { indicator.to_string() } else { format!(" {indicator}") };
        self.whitespace = whitespace;
        self.indention = self.indention && indention;
        self.column += data.chars().count();
        self.open_ended = false;
        self.write(&data);
    }

    fn write_indent(&mut self) {
        let indent = self.indent.unwrap_or(0);
        if !self.indention || self.column > indent || (self.column == indent && !self.whitespace) {
            self.write_line_break(None);
        }
        if self.column < indent {
            self.whitespace = true;
            let pad = " ".repeat(indent - self.column);
            self.column = indent;
            self.write(&pad);
        }
    }

    fn write_line_break(&mut self, data: Option<char>) {
        self.whitespace = true;
        self.indention = true;
        self.column = 0;
        self.out.push(data.unwrap_or('\n'));
    }

    fn write_plain(&mut self, text: &[char], split: bool) {
        if self.root_context {
            self.open_ended = true;
        }
        if text.is_empty() {
            return;
        }
        if !self.whitespace {
            self.column += 1;
            self.write(" ");
        }
        self.whitespace = false;
        self.indention = false;
        let (mut spaces, mut breaks) = (false, false);
        let (mut start, mut end) = (0usize, 0usize);
        while end <= text.len() {
            let ch = text.get(end).copied();
            if spaces {
                if ch != Some(' ') {
                    if start + 1 == end && self.column > self.best_width && split {
                        self.write_indent();
                        self.whitespace = false;
                        self.indention = false;
                    } else {
                        self.write_chars(&text[start..end]);
                    }
                    start = end;
                }
            } else if breaks {
                if !ch.map(is_break).unwrap_or(false) {
                    if text[start] == '\n' {
                        self.write_line_break(None);
                    }
                    for &br in &text[start..end] {
                        self.write_line_break(if br == '\n' { None } else { Some(br) });
                    }
                    self.write_indent();
                    self.whitespace = false;
                    self.indention = false;
                    start = end;
                }
            } else if ch.map(|c| c == ' ' || is_break(c)).unwrap_or(true) {
                self.write_chars(&text[start..end]);
                start = end;
            }
            if let Some(c) = ch {
                spaces = c == ' ';
                breaks = is_break(c);
            }
            end += 1;
        }
    }

    fn write_single_quoted(&mut self, text: &[char], split: bool) {
        self.write_indicator("'", true, false, false);
        let (mut spaces, mut breaks) = (false, false);
        let (mut start, mut end) = (0usize, 0usize);
        while end <= text.len() {
            let ch = text.get(end).copied();
            if spaces {
                if ch != Some(' ') {
                    if start + 1 == end && self.column > self.best_width && split && start != 0 && end != text.len() {
                        self.write_indent();
                    } else {
                        self.write_chars(&text[start..end]);
                    }
                    start = end;
                }
            } else if breaks {
                if !ch.map(is_break).unwrap_or(false) {
                    if text[start] == '\n' {
                        self.write_line_break(None);
                    }
                    for &br in &text[start..end] {
                        self.write_line_break(if br == '\n' { None } else { Some(br) });
                    }
                    self.write_indent();
                    start = end;
                }
            } else if ch.map(|c| c == ' ' || is_break(c) || c == '\'').unwrap_or(true) && start < end {
                self.write_chars(&text[start..end]);
                start = end;
            }
            if ch == Some('\'') {
                self.column += 2;
                self.write("''");
                start = end + 1;
            }
            if let Some(c) = ch {
                spaces = c == ' ';
                breaks = is_break(c);
            }
            end += 1;
        }
        self.write_indicator("'", false, false, false);
    }

    fn write_double_quoted(&mut self, text: &[char], split: bool) {
        self.write_indicator("\"", true, false, false);
        let (mut start, mut end) = (0usize, 0usize);
        while end <= text.len() {
            let ch = text.get(end).copied();
            let escape = match ch {
                None => true,
                Some(c) => {
                    "\"\\\u{85}\u{2028}\u{2029}\u{FEFF}".contains(c)
                        || !(('\x20'..='\x7E').contains(&c)
                            || ('\u{A0}'..='\u{D7FF}').contains(&c)
                            || ('\u{E000}'..='\u{FFFD}').contains(&c))
                }
            };
            if escape {
                if start < end {
                    self.write_chars(&text[start..end]);
                    start = end;
                }
                if let Some(c) = ch {
                    let data = match c {
                        '\0' => "\\0".to_string(),
                        '\x07' => "\\a".to_string(),
                        '\x08' => "\\b".to_string(),
                        '\x09' => "\\t".to_string(),
                        '\x0A' => "\\n".to_string(),
                        '\x0B' => "\\v".to_string(),
                        '\x0C' => "\\f".to_string(),
                        '\x0D' => "\\r".to_string(),
                        '\x1B' => "\\e".to_string(),
                        '"' => "\\\"".to_string(),
                        '\\' => "\\\\".to_string(),
                        '\u{85}' => "\\N".to_string(),
                        '\u{A0}' => "\\_".to_string(),
                        '\u{2028}' => "\\L".to_string(),
                        '\u{2029}' => "\\P".to_string(),
                        c if (c as u32) <= 0xFF => format!("\\x{:02X}", c as u32),
                        c if (c as u32) <= 0xFFFF => format!("\\u{:04X}", c as u32),
                        c => format!("\\U{:08X}", c as u32),
                    };
                    self.column += data.chars().count();
                    self.write(&data);
                    start = end + 1;
                }
            }
            if 0 < end
                && end + 1 < text.len()
                && (ch == Some(' ') || start >= end)
                && self.column as isize + end as isize - start as isize > self.best_width as isize
                && split
            {
                let mut data: String = text[start.min(end)..end].iter().collect();
                data.push('\\');
                if start < end {
                    start = end;
                }
                self.column += data.chars().count();
                self.write(&data);
                self.write_indent();
                self.whitespace = false;
                self.indention = false;
                if text.get(start) == Some(&' ') {
                    self.column += 1;
                    self.write("\\");
                }
            }
            end += 1;
        }
        self.write_indicator("\"", false, false, false);
    }
}

/// PyYAML's `check_simple_key` for a mapping's key.
fn check_simple_key(key: &Yaml) -> bool {
    match key {
        Yaml::Seq(items) => items.is_empty(),
        Yaml::Map(pairs) => pairs.is_empty(),
        scalar => {
            let node = represent_scalar(scalar).expect("a scalar");
            let chars: Vec<char> = node.text.chars().collect();
            let analysis = analyze_scalar(&chars);
            let length = node.tag.chars().count() + chars.len();
            length < 128 && !analysis.empty && !analysis.multiline
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(text: &str) -> String {
        dump(&load(text).unwrap())
    }

    #[test]
    fn yaml_1_1_types() {
        let data = load(
            "a: yes\nb: off\nc: 0755\nd: 1:30\ne: 1_000\nf: 2026-10-21\ng: ~\nh: '0755'\ni: 1.5\nj: .inf\nk: 0x1F\n",
        )
        .unwrap();
        assert_eq!(data.get("a"), Some(&Yaml::Bool(true)));
        assert_eq!(data.get("b"), Some(&Yaml::Bool(false)));
        assert_eq!(data.get("c"), Some(&Yaml::Int(493)));
        assert_eq!(data.get("d"), Some(&Yaml::Int(90)));
        assert_eq!(data.get("e"), Some(&Yaml::Int(1000)));
        assert_eq!(data.get("f"), Some(&Yaml::Timestamp("2026-10-21".into())));
        assert_eq!(data.get("g"), Some(&Yaml::Null));
        assert_eq!(data.get("h"), Some(&Yaml::str("0755")));
        assert_eq!(data.get("i"), Some(&Yaml::Float(1.5)));
        assert_eq!(data.get("j"), Some(&Yaml::Float(f64::INFINITY)));
        assert_eq!(data.get("k"), Some(&Yaml::Int(31)));
    }

    #[test]
    fn dumps_as_pyyaml() {
        assert_eq!(
            round("a: 1\nb:\n  c: [x, 'y']\n  d: {}\ne: []\n"),
            "a: 1\nb:\n  c:\n  - x\n  - y\n  d: {}\ne: []\n"
        );
        assert_eq!(dump(&Yaml::Map(vec![(Yaml::str("k"), Yaml::str("yes"))])), "k: 'yes'\n");
        assert_eq!(dump(&Yaml::Map(vec![(Yaml::str("k"), Yaml::str(""))])), "k: ''\n");
        assert_eq!(dump(&Yaml::Map(vec![(Yaml::str("k"), Yaml::str("a: b"))])), "k: 'a: b'\n");
        assert_eq!(dump(&Yaml::Map(vec![(Yaml::str("k"), Yaml::str("tab\there"))])), "k: \"tab\\there\"\n");
        assert_eq!(
            dump(&Yaml::Map(vec![(Yaml::str("ui_meta"), Yaml::Map(vec![(Yaml::str("hermes-bots"), Yaml::map())]))])),
            "ui_meta:\n  hermes-bots: {}\n"
        );
    }
}
