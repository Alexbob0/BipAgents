//! Structured JSON logging. Never pass secrets or message bodies as fields at info level.
//!
//! One JSON object per line on stdout, as the Python bridge writes them:
//! `{"ts": "2026-10-04T08:15:00.123Z", "level": "info", "logger": "bipbridge.app", "msg": "...", <fields>}`
//! (Python's `json.dumps` defaults: `", "` and `": "` separators, UTF-8 kept). The logger name is the tracing
//! target with `::` turned into `.`: a module's own events are `bipbridge.<module>`, as Python's
//! `logging.getLogger("bipbridge.<module>")`. Fields are tracing fields:
//! `tracing::info!(agent = %name, removed = n, "uploads purged")`; a JSON value (a list, a dict) goes through
//! [`J`]: `tracing::info!(agents = ?logs::J(json!(names)), "bridge started")`. Python's `log.exception` is
//! `tracing::error!(exc = %err, "...")`; its `warning` level is written `"warning"`.

use std::fmt;
use std::sync::OnceLock;

use serde_json::{Map, Value};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt;

use crate::py;

const JSON_MARK: &str = "\u{0}json:";

/// A JSON value as a log field (`agents = ?J(json!([...]))`): written as JSON, not as a string.
pub struct J<T: serde::Serialize>(pub T);

impl<T: serde::Serialize> fmt::Debug for J<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = serde_json::to_string(&self.0).unwrap_or_else(|_| "null".into());
        write!(f, "{JSON_MARK}{text}")
    }
}

#[derive(Default)]
struct Fields {
    msg: String,
    values: Map<String, Value>,
    exc: Option<String>,
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let text = format!("{value:?}");
        let value = match text.strip_prefix(JSON_MARK) {
            Some(json) => serde_json::from_str(json).unwrap_or(Value::Null),
            None => Value::String(text),
        };
        self.put(field, value);
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.put(field, Value::String(value.to_string()));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.put(field, Value::from(value));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.put(field, Value::from(value));
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.put(field, serde_json::Number::from_f64(value).map(Value::Number).unwrap_or(Value::Null));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.put(field, Value::Bool(value));
    }
}

impl Fields {
    fn put(&mut self, field: &Field, value: Value) {
        match field.name() {
            "message" => self.msg = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string()),
            "exc" => self.exc = Some(value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string())),
            name => {
                // `r#type` style raw identifiers come through as "r#type".
                let name = name.strip_prefix("r#").unwrap_or(name);
                self.values.insert(name.to_string(), value);
            }
        }
    }
}

/// Python's logging level name, lowercased.
pub fn level_name(level: &Level) -> &'static str {
    match *level {
        Level::ERROR => "error",
        Level::WARN => "warning",
        Level::INFO => "info",
        Level::DEBUG | Level::TRACE => "debug",
    }
}

/// `ts` as Python writes it: UTC, milliseconds, `Z`.
pub fn timestamp(now: chrono::DateTime<chrono::Utc>) -> String {
    now.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// One log line (without its newline), for the formatter and the tests.
pub fn format_line(level: &Level, target: &str, msg: &str, fields: &Map<String, Value>, exc: Option<&str>) -> String {
    let mut payload = Map::new();
    payload.insert("ts".into(), Value::String(timestamp(chrono::Utc::now())));
    payload.insert("level".into(), Value::String(level_name(level).into()));
    payload.insert("logger".into(), Value::String(target.replace("::", ".")));
    payload.insert("msg".into(), Value::String(msg.to_string()));
    for (k, v) in fields {
        payload.insert(k.clone(), v.clone());
    }
    if let Some(exc) = exc {
        payload.insert("exc".into(), Value::String(exc.to_string()));
    }
    py::Dumps::new().unicode().dumps(&Value::Object(payload))
}

struct JsonFormatter;

impl<S, N> FormatEvent<S, N> for JsonFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(&self, _ctx: &FmtContext<'_, S, N>, mut writer: Writer<'_>, event: &Event<'_>) -> fmt::Result {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let meta = event.metadata();
        let line = format_line(meta.level(), meta.target(), &fields.msg, &fields.values, fields.exc.as_deref());
        writeln!(writer, "{line}")
    }
}

type ReloadHandle = reload::Handle<EnvFilter, tracing_subscriber::Registry>;
static RELOAD: OnceLock<ReloadHandle> = OnceLock::new();

fn filter_for(level: &str) -> EnvFilter {
    let lvl = match level.to_ascii_lowercase().as_str() {
        "debug" => "debug",
        "warning" | "warn" => "warn",
        "error" | "critical" => "error",
        _ => "info",
    };
    // httpx/httpcore/hpack at WARNING in Python: their Rust counterparts here.
    let quiet = if lvl == "error" { "error" } else { "warn" };
    let spec = format!(
        "{lvl},hyper={quiet},hyper_util={quiet},reqwest={quiet},h2={quiet},rustls={quiet},tower={quiet},axum={quiet},tungstenite={quiet},tokio_tungstenite={quiet}"
    );
    EnvFilter::new(spec)
}

/// JSON logs on stdout at `level` (`debug`, `info`, `warning`, `error`; anything else: info). Called again, it
/// changes the level (Python calls it once with "info", then with the config's level).
pub fn setup_logging(level: &str) {
    if let Some(handle) = RELOAD.get() {
        let _ = handle.reload(filter_for(level));
        return;
    }
    let (filter, handle) = reload::Layer::new(filter_for(level));
    let fmt_layer = tracing_subscriber::fmt::layer().event_format(JsonFormatter).with_writer(std::io::stdout);
    if tracing_subscriber::registry().with(filter).with(fmt_layer).try_init().is_ok() {
        let _ = RELOAD.set(handle);
    }
}

/// Short, non-reversible hint for identifiers such as device tokens.
pub fn redact(value: Option<&str>, keep: usize) -> String {
    match value {
        None | Some("") => String::new(),
        Some(v) => {
            let chars: Vec<char> = v.chars().collect();
            if chars.len() > keep {
                format!("{}…", chars[..keep].iter().collect::<String>())
            } else {
                "…".into()
            }
        }
    }
}

/// `redact(value)` with Python's default `keep=6`.
pub fn redact6(value: &str) -> String {
    redact(Some(value), 6)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redact_like_python() {
        assert_eq!(redact(None, 6), "");
        assert_eq!(redact(Some("abcdefgh"), 6), "abcdef…");
        assert_eq!(redact(Some("abc"), 6), "…");
        assert_eq!(redact(Some("abcdef"), 6), "…");
    }

    #[test]
    fn line_layout() {
        let mut fields = Map::new();
        fields.insert("agent".into(), json!("lea"));
        fields.insert("count".into(), json!(3));
        let line = format_line(&Level::WARN, "bipbridge::guard", "wrong key", &fields, None);
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["level"], "warning");
        assert_eq!(value["logger"], "bipbridge.guard");
        assert_eq!(value["msg"], "wrong key");
        assert_eq!(value["count"], 3);
        assert!(line.starts_with("{\"ts\": \""), "{line}");
        assert!(value["ts"].as_str().unwrap().ends_with('Z'));
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["ts", "level", "logger", "msg", "agent", "count"]);
    }

    #[test]
    fn json_fields() {
        let mut f = Fields::default();
        let text = format!("{:?}", J(json!(["a", "b"])));
        assert!(text.starts_with(JSON_MARK));
        let parsed: Value = serde_json::from_str(text.strip_prefix(JSON_MARK).unwrap()).unwrap();
        assert_eq!(parsed, json!(["a", "b"]));
        f.values.insert("x".into(), parsed);
        assert_eq!(f.values["x"][1], "b");
    }
}
