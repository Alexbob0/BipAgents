//! FastAPI-compatible plumbing, so every route answers as the Python bridge does:
//!
//! - [`HttpError`]: `raise HTTPException(status_code, detail, headers)` → `{"detail": <detail>}`;
//! - [`Json`]: a pydantic body model → the value, or FastAPI's 422 `{"detail": [{"type", "loc", "msg", "input"}]}`;
//! - [`Validate`] + [`check_max_len`]…: pydantic's `Field(max_length=…, min_length=…, ge=…, le=…)`;
//! - [`QueryMap`] + [`query_int`]…: `Query(...)` parameters with their 422s;
//! - [`lax`]: pydantic's lax coercions (`"true"` for a bool, `"5"` for an int) for model fields;
//! - [`json_response`] / [`detail`]: `JSONResponse(content, status_code, headers)`.
//!
//! Not a Python module: shared plumbing (core group).

use std::collections::HashMap;

use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

/// `HTTPException(status_code, detail, headers)`.
#[derive(Debug, Clone)]
pub struct HttpError {
    pub status: StatusCode,
    pub detail: Value,
    pub headers: Vec<(HeaderName, HeaderValue)>,
}

pub type ApiResult<T> = Result<T, HttpError>;

impl HttpError {
    /// `HTTPException(status_code=status, detail=text)`.
    pub fn new(status: u16, detail: impl Into<String>) -> Self {
        HttpError {
            status: StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            detail: Value::String(detail.into()),
            headers: Vec::new(),
        }
    }

    /// `HTTPException(status_code=status, detail={...})` (a dict or list detail).
    pub fn json(status: u16, detail: Value) -> Self {
        HttpError {
            status: StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            detail,
            headers: Vec::new(),
        }
    }

    pub fn with_header(mut self, name: &'static str, value: &str) -> Self {
        if let Ok(v) = HeaderValue::from_str(value) {
            self.headers.push((HeaderName::from_static(name), v));
        }
        self
    }

    pub fn not_found() -> Self {
        Self::new(404, "not found")
    }

    /// An uncaught exception in a Python route: Starlette's plain-text 500.
    pub fn internal() -> Self {
        HttpError { status: StatusCode::INTERNAL_SERVER_ERROR, detail: Value::Null, headers: Vec::new() }
    }

    /// FastAPI's request validation error (422) with pydantic's error list.
    pub fn validation(errors: Vec<FieldError>) -> Self {
        Self::json(422, Value::Array(errors.into_iter().map(FieldError::into_value).collect()))
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {}: {}", self.status.as_u16(), self.detail)
    }
}

impl std::error::Error for HttpError {}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        if self.status == StatusCode::INTERNAL_SERVER_ERROR && self.detail.is_null() {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                [("content-type", "text/plain; charset=utf-8")],
                "Internal Server Error",
            )
                .into_response();
        }
        let mut response = json_response(self.status.as_u16(), &json!({"detail": self.detail}));
        for (name, value) in self.headers {
            response.headers_mut().insert(name, value);
        }
        response
    }
}

/// Any internal error (Python: an exception the route does not catch) → 500, logged.
impl From<anyhow::Error> for HttpError {
    fn from(err: anyhow::Error) -> Self {
        tracing::error!(exc = %format!("{err:#}"), "Exception in ASGI application");
        HttpError::internal()
    }
}

/// `JSONResponse(content, status_code)`: compact JSON, UTF-8 (FastAPI's rendering).
pub fn json_response(status: u16, content: &Value) -> Response {
    let body = serde_json::to_vec(content).unwrap_or_else(|_| b"null".to_vec());
    let mut response = Response::new(axum::body::Body::from(body));
    *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
    response.headers_mut().insert("content-type", HeaderValue::from_static("application/json"));
    response
}

/// A route's plain `return {...}`: 200 with the JSON.
pub fn ok(content: Value) -> Response {
    json_response(200, &content)
}

/// `Response(status_code=204)`.
pub fn no_content() -> Response {
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    response
}

// -- validation errors -----------------------------------------------------------------------------

/// One entry of FastAPI's 422 `detail` list (pydantic v2's error).
#[derive(Debug, Clone)]
pub struct FieldError {
    pub kind: String,
    pub loc: Vec<Value>,
    pub msg: String,
    pub input: Value,
    pub ctx: Option<Value>,
}

impl FieldError {
    pub fn new(kind: &str, loc: Vec<Value>, msg: impl Into<String>, input: Value) -> Self {
        FieldError { kind: kind.into(), loc, msg: msg.into(), input, ctx: None }
    }

    pub fn with_ctx(mut self, ctx: Value) -> Self {
        self.ctx = Some(ctx);
        self
    }

    /// `{"type": "missing", "loc": [...], "msg": "Field required", "input": ...}`.
    pub fn missing(loc: Vec<Value>, input: Value) -> Self {
        Self::new("missing", loc, "Field required", input)
    }

    pub fn into_value(self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("type".into(), Value::String(self.kind));
        map.insert("loc".into(), Value::Array(self.loc));
        map.insert("msg".into(), Value::String(self.msg));
        map.insert("input".into(), self.input);
        if let Some(ctx) = self.ctx {
            map.insert("ctx".into(), ctx);
        }
        Value::Object(map)
    }
}

/// `["body", "field"]`.
pub fn body_loc(field: &str) -> Vec<Value> {
    vec![json!("body"), json!(field)]
}

/// pydantic's `Field(...)` constraints, checked after the body parsed. Implement it for each body model (an empty
/// `impl Validate for X {}` when there is nothing to check).
pub trait Validate {
    fn validate(&self, _errors: &mut Vec<FieldError>) {}
}

/// `Field(max_length=n)` on a str (counted in characters, as Python does).
pub fn check_max_len(errors: &mut Vec<FieldError>, field: &str, value: Option<&str>, max: usize) {
    if let Some(v) = value {
        if v.chars().count() > max {
            errors.push(
                FieldError::new(
                    "string_too_long",
                    body_loc(field),
                    format!("String should have at most {max} character{}", plural(max)),
                    json!(v),
                )
                .with_ctx(json!({"max_length": max})),
            );
        }
    }
}

/// `Field(min_length=n)` on a str.
pub fn check_min_len(errors: &mut Vec<FieldError>, field: &str, value: Option<&str>, min: usize) {
    if let Some(v) = value {
        if v.chars().count() < min {
            errors.push(
                FieldError::new(
                    "string_too_short",
                    body_loc(field),
                    format!("String should have at least {min} character{}", plural(min)),
                    json!(v),
                )
                .with_ctx(json!({"min_length": min})),
            );
        }
    }
}

/// `Field(max_length=n)` on a list.
pub fn check_max_items<T: serde::Serialize>(
    errors: &mut Vec<FieldError>,
    field: &str,
    value: Option<&[T]>,
    max: usize,
) {
    if let Some(v) = value {
        if v.len() > max {
            errors.push(
                FieldError::new(
                    "too_long",
                    body_loc(field),
                    format!("List should have at most {max} item{} after validation, not {}", plural(max), v.len()),
                    serde_json::to_value(v).unwrap_or(Value::Null),
                )
                .with_ctx(json!({"field_type": "List", "max_length": max, "actual_length": v.len()})),
            );
        }
    }
}

/// `Field(ge=…, le=…)` on a number.
pub fn check_range(
    errors: &mut Vec<FieldError>,
    loc: Vec<Value>,
    value: f64,
    ge: Option<f64>,
    le: Option<f64>,
    input: Value,
) {
    if let Some(min) = ge {
        if value < min {
            errors.push(
                FieldError::new(
                    "greater_than_equal",
                    loc.clone(),
                    format!("Input should be greater than or equal to {}", num(min)),
                    input.clone(),
                )
                .with_ctx(json!({"ge": num_value(min)})),
            );
            return;
        }
    }
    if let Some(max) = le {
        if value > max {
            errors.push(
                FieldError::new(
                    "less_than_equal",
                    loc,
                    format!("Input should be less than or equal to {}", num(max)),
                    input,
                )
                .with_ctx(json!({"le": num_value(max)})),
            );
        }
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn num(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        crate::py::float_repr(f)
    }
}

fn num_value(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        json!(f as i64)
    } else {
        json!(f)
    }
}

// -- JSON bodies -----------------------------------------------------------------------------------

/// A pydantic body model: the JSON body deserialized into `T`, then [`Validate`]d. Failures are FastAPI's 422s.
/// Put it last among a handler's extractors (it reads the body).
pub struct Json<T>(pub T);

/// The body as FastAPI reads it for a model: JSON when the content type says so (or says nothing), else raw bytes.
pub enum BodyValue {
    Missing,
    Json(Value),
    Bytes(Vec<u8>),
    Invalid(Vec<FieldError>),
}

fn is_json_content_type(headers: &HeaderMap) -> Option<bool> {
    let value = headers.get("content-type")?.to_str().ok()?.to_ascii_lowercase();
    let main = value.split(';').next().unwrap_or("").trim().to_string();
    let (maintype, subtype) = main.split_once('/').unwrap_or((main.as_str(), ""));
    Some(maintype == "application" && (subtype == "json" || subtype.ends_with("+json")))
}

/// Reads the request body the way FastAPI does before validating a model.
#[allow(clippy::result_large_err)]
pub async fn read_body(headers: &HeaderMap, body: axum::body::Body) -> Result<BodyValue, Response> {
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|_| HttpError::new(400, "There was an error parsing the body").into_response())?;
    if bytes.is_empty() {
        return Ok(BodyValue::Missing);
    }
    match is_json_content_type(headers) {
        Some(false) => Ok(BodyValue::Bytes(bytes.to_vec())),
        _ => match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) => Ok(BodyValue::Json(v)),
            Err(e) => {
                let mut pos = byte_position(&bytes, e.line(), e.column(), e.is_eof());
                if e.to_string().starts_with("invalid escape") {
                    pos = pos.saturating_sub(1); // Python points at the backslash
                }
                if e.to_string().starts_with("trailing comma") {
                    let at = String::from_utf8_lossy(&bytes).chars().nth(pos).unwrap_or(' ');
                    let kind = if at == '}' { "object" } else { "list" };
                    let text = if kind == "object" {
                        "Expecting property name enclosed in double quotes"
                    } else {
                        "Expecting value"
                    };
                    return Ok(BodyValue::Invalid(vec![FieldError::new(
                        "json_invalid",
                        vec![json!("body"), json!(pos)],
                        "JSON decode error",
                        json!({}),
                    )
                    .with_ctx(json!({"error": text}))]));
                }
                Ok(BodyValue::Invalid(vec![FieldError::new(
                    "json_invalid",
                    vec![json!("body"), json!(pos)],
                    "JSON decode error",
                    json!({}),
                )
                .with_ctx(json!({"error": json_error_text(&e)}))]))
            }
        },
    }
}

fn byte_position(bytes: &[u8], line: usize, column: usize, eof: bool) -> usize {
    // serde's line/column (1-based, the last character read) → Python's character offset (0-based; at the end of
    // the input for an EOF).
    let text = String::from_utf8_lossy(bytes);
    let mut offset = 0;
    for (i, l) in text.split('\n').enumerate() {
        if i + 1 == line {
            return offset + if eof { column } else { column.saturating_sub(1) };
        }
        offset += l.chars().count() + 1;
    }
    offset
}

/// Python's `json` error text (what FastAPI puts in `ctx.error`) for serde's.
fn json_error_text(e: &serde_json::Error) -> String {
    let text = e.to_string();
    let starts = |p: &str| text.starts_with(p);
    if starts("EOF while parsing an object") || starts("EOF while parsing a list") || starts("expected `,` or") {
        "Expecting ',' delimiter".into()
    } else if starts("expected `:`") {
        "Expecting ':' delimiter".into()
    } else if starts("key must be a string") || starts("EOF while parsing an object key") {
        "Expecting property name enclosed in double quotes".into()
    } else if starts("EOF while parsing a string") {
        "Unterminated string starting at".into()
    } else if starts("trailing comma") {
        // `{"a": 1,}` / `[1,]`: Python expects another key / value there
        if text.contains("object") {
            "Expecting property name enclosed in double quotes".into()
        } else {
            "Expecting value".into()
        }
    } else if starts("trailing characters") {
        "Extra data".into()
    } else if starts("control character") {
        "Invalid control character at".into()
    } else if starts("invalid escape") || starts("invalid unicode code point") {
        "Invalid \\escape".into()
    } else {
        "Expecting value".into()
    }
}

/// Deserializes a model from a JSON value, mapping serde's error to pydantic's.
pub fn from_value<T: DeserializeOwned + Validate>(value: Value) -> Result<T, Vec<FieldError>> {
    if !value.is_object() {
        return Err(vec![FieldError::new(
            "model_attributes_type",
            vec![json!("body")],
            "Input should be a valid dictionary or object to extract fields from",
            value,
        )]);
    }
    let original = value.clone();
    let parsed: Result<T, _> = serde_path_to_error::deserialize(value);
    match parsed {
        Ok(model) => {
            let mut errors = Vec::new();
            model.validate(&mut errors);
            if errors.is_empty() {
                Ok(model)
            } else {
                Err(errors)
            }
        }
        Err(err) => Err(vec![serde_to_field_error(&err, &original)]),
    }
}

fn serde_to_field_error(err: &serde_path_to_error::Error<serde_json::Error>, original: &Value) -> FieldError {
    let mut loc = vec![json!("body")];
    let mut input = original.clone();
    for seg in err.path().iter() {
        match seg {
            serde_path_to_error::Segment::Map { key } => {
                loc.push(json!(key));
                input = input.get(key).cloned().unwrap_or(Value::Null);
            }
            serde_path_to_error::Segment::Seq { index } => {
                loc.push(json!(index));
                input = input.get(*index).cloned().unwrap_or(Value::Null);
            }
            _ => {}
        }
    }
    let text = err.inner().to_string();
    if let Some(rest) = text.strip_prefix("missing field `") {
        let field = rest.split('`').next().unwrap_or("");
        let parent = input.clone();
        loc.push(json!(field));
        return FieldError::missing(loc, parent);
    }
    if let Some(rest) = text.strip_prefix("pydantic:") {
        // `pydantic:<type>:<message>` from the lax helpers
        let (kind, msg) = rest.split_once(':').unwrap_or((rest, ""));
        let msg = msg.split(" at line ").next().unwrap_or(msg);
        return FieldError::new(kind, loc, msg, input);
    }
    let (kind, msg) = if text.contains("expected a string") {
        ("string_type", "Input should be a valid string")
    } else if text.contains("expected a boolean") {
        ("bool_type", "Input should be a valid boolean")
    } else if text.contains("expected i") || text.contains("expected u") || text.contains("integer") {
        ("int_type", "Input should be a valid integer")
    } else if text.contains("expected f") {
        ("float_type", "Input should be a valid number")
    } else if text.contains("expected a sequence") {
        ("list_type", "Input should be a valid list")
    } else if text.contains("expected a map") || text.contains("expected struct") {
        ("model_type", "Input should be a valid dictionary")
    } else {
        ("value_error", "Value error")
    };
    FieldError::new(kind, loc, msg, input)
}

impl<S, T> FromRequest<S> for Json<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Validate + Send,
{
    type Rejection = Response;

    async fn from_request(req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let (parts, body) = req.into_parts();
        let value = match read_body(&parts.headers, body).await? {
            // FastAPI: an empty body, or `null`, is a missing body
            BodyValue::Missing | BodyValue::Json(Value::Null) => {
                return Err(
                    HttpError::validation(vec![FieldError::missing(vec![json!("body")], Value::Null)]).into_response()
                )
            }
            BodyValue::Invalid(errors) => return Err(HttpError::validation(errors).into_response()),
            BodyValue::Bytes(bytes) => Value::String(String::from_utf8_lossy(&bytes).into_owned()),
            BodyValue::Json(v) => v,
        };
        from_value::<T>(value).map(Json).map_err(|errors| HttpError::validation(errors).into_response())
    }
}

/// An optional JSON body (`body: Optional[Model] = None`): None when the request has none.
pub struct OptionalJson<T>(pub Option<T>);

impl<S, T> FromRequest<S> for OptionalJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Validate + Send,
{
    type Rejection = Response;

    async fn from_request(req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let (parts, body) = req.into_parts();
        let value = match read_body(&parts.headers, body).await? {
            BodyValue::Missing => return Ok(OptionalJson(None)),
            BodyValue::Invalid(errors) => return Err(HttpError::validation(errors).into_response()),
            BodyValue::Bytes(bytes) => Value::String(String::from_utf8_lossy(&bytes).into_owned()),
            BodyValue::Json(Value::Null) => return Ok(OptionalJson(None)),
            BodyValue::Json(v) => v,
        };
        from_value::<T>(value)
            .map(|m| OptionalJson(Some(m)))
            .map_err(|errors| HttpError::validation(errors).into_response())
    }
}

/// A free-form JSON body (`body: Dict[str, Any]`): must be an object.
pub struct JsonObject(pub serde_json::Map<String, Value>);

impl<S: Send + Sync> FromRequest<S> for JsonObject {
    type Rejection = Response;

    async fn from_request(req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let (parts, body) = req.into_parts();
        match read_body(&parts.headers, body).await? {
            BodyValue::Missing => {
                Err(HttpError::validation(vec![FieldError::missing(vec![json!("body")], Value::Null)]).into_response())
            }
            BodyValue::Invalid(errors) => Err(HttpError::validation(errors).into_response()),
            BodyValue::Json(Value::Object(map)) => Ok(JsonObject(map)),
            BodyValue::Json(other) => Err(HttpError::validation(vec![FieldError::new(
                "dict_type",
                vec![json!("body")],
                "Input should be a valid dictionary",
                other,
            )])
            .into_response()),
            BodyValue::Bytes(bytes) => Err(HttpError::validation(vec![FieldError::new(
                "dict_type",
                vec![json!("body")],
                "Input should be a valid dictionary",
                Value::String(String::from_utf8_lossy(&bytes).into_owned()),
            )])
            .into_response()),
        }
    }
}

// -- query parameters ------------------------------------------------------------------------------

/// The query string as Starlette's `request.query_params`: every pair in order; `get` gives the last value of a
/// key (as FastAPI reads a scalar parameter).
#[derive(Debug, Clone, Default)]
pub struct QueryMap {
    pub pairs: Vec<(String, String)>,
}

impl QueryMap {
    pub fn parse(query: Option<&str>) -> Self {
        let pairs = query
            .map(|q| url::form_urlencoded::parse(q.as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect())
            .unwrap_or_default();
        QueryMap { pairs }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.pairs.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn get_all(&self, key: &str) -> Vec<&str> {
        self.pairs.iter().filter(|(k, _)| k == key).map(|(_, v)| v.as_str()).collect()
    }

    pub fn to_map(&self) -> HashMap<String, String> {
        self.pairs.iter().cloned().collect()
    }
}

impl<S: Send + Sync> FromRequestParts<S> for QueryMap {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(QueryMap::parse(parts.uri.query()))
    }
}

fn query_loc(name: &str) -> Vec<Value> {
    vec![json!("query"), json!(name)]
}

/// `name: str = Query(...)`: required.
pub fn query_required(q: &QueryMap, name: &str) -> ApiResult<String> {
    q.get(name)
        .map(str::to_string)
        .ok_or_else(|| HttpError::validation(vec![FieldError::missing(query_loc(name), Value::Null)]))
}

/// `name: str = Query(..., max_length=n)` (or `Optional[str] = Query(None, max_length=n)` with `required=false`).
pub fn query_str(q: &QueryMap, name: &str, required: bool, max_length: Option<usize>) -> ApiResult<Option<String>> {
    let Some(value) = q.get(name) else {
        return if required {
            Err(HttpError::validation(vec![FieldError::missing(query_loc(name), Value::Null)]))
        } else {
            Ok(None)
        };
    };
    if let Some(max) = max_length {
        if value.chars().count() > max {
            return Err(HttpError::validation(vec![FieldError::new(
                "string_too_long",
                query_loc(name),
                format!("String should have at most {max} character{}", plural(max)),
                json!(value),
            )
            .with_ctx(json!({"max_length": max}))]));
        }
    }
    Ok(Some(value.to_string()))
}

/// `name: int = Query(default, ge=…, le=…)`.
pub fn query_int(
    q: &QueryMap,
    name: &str,
    default: Option<i64>,
    ge: Option<i64>,
    le: Option<i64>,
) -> ApiResult<Option<i64>> {
    let Some(raw) = q.get(name) else { return Ok(default) };
    let parsed = raw.trim().parse::<i64>().ok().or_else(|| {
        let f = raw.trim().parse::<f64>().ok()?;
        (f.fract() == 0.0).then_some(f as i64)
    });
    let Some(value) = parsed else {
        return Err(HttpError::validation(vec![FieldError::new(
            "int_parsing",
            query_loc(name),
            "Input should be a valid integer, unable to parse string as an integer",
            json!(raw),
        )]));
    };
    let mut errors = Vec::new();
    check_range(&mut errors, query_loc(name), value as f64, ge.map(|v| v as f64), le.map(|v| v as f64), json!(raw));
    if errors.is_empty() {
        Ok(Some(value))
    } else {
        Err(HttpError::validation(errors))
    }
}

/// `name: bool = Query(default)`: pydantic's accepted spellings.
pub fn query_bool(q: &QueryMap, name: &str, default: bool) -> ApiResult<bool> {
    let Some(raw) = q.get(name) else { return Ok(default) };
    lax::parse_bool_str(raw).ok_or_else(|| {
        HttpError::validation(vec![FieldError::new(
            "bool_parsing",
            query_loc(name),
            "Input should be a valid boolean, unable to interpret input",
            json!(raw),
        )])
    })
}

// -- pydantic's lax mode ---------------------------------------------------------------------------

/// `#[serde(deserialize_with = "web::lax::bool")]` and friends: what pydantic accepts in lax mode.
pub mod lax {
    use serde::de::{Deserialize, Deserializer, Error};
    use serde_json::Value;

    pub fn parse_bool_str(s: &str) -> Option<bool> {
        match s.trim().to_ascii_lowercase().as_str() {
            "1" | "on" | "t" | "true" | "y" | "yes" => Some(true),
            "0" | "off" | "f" | "false" | "n" | "no" => Some(false),
            _ => None,
        }
    }

    const BOOL_PARSING: &str = "pydantic:bool_parsing:Input should be a valid boolean, unable to interpret input";
    const BOOL_TYPE: &str = "pydantic:bool_type:Input should be a valid boolean";
    const INT_PARSING: &str =
        "pydantic:int_parsing:Input should be a valid integer, unable to parse string as an integer";
    const INT_FROM_FLOAT: &str =
        "pydantic:int_from_float:Input should be a valid integer, got a number with a fractional part";
    const INT_TYPE: &str = "pydantic:int_type:Input should be a valid integer";
    const FLOAT_PARSING: &str =
        "pydantic:float_parsing:Input should be a valid number, unable to parse string as a number";
    const FLOAT_TYPE: &str = "pydantic:float_type:Input should be a valid number";

    fn to_bool(v: &Value) -> Result<bool, &'static str> {
        match v {
            Value::Bool(b) => Ok(*b),
            Value::Number(n) => match n.as_f64() {
                Some(0.0) => Ok(false),
                Some(1.0) => Ok(true),
                _ => Err(BOOL_PARSING),
            },
            Value::String(s) => parse_bool_str(s).ok_or(BOOL_PARSING),
            _ => Err(BOOL_TYPE),
        }
    }

    fn to_int(v: &Value) -> Result<i64, &'static str> {
        match v {
            Value::Number(n) => match n.as_i64() {
                Some(i) => Ok(i),
                None => match n.as_f64() {
                    Some(f) if f.fract() == 0.0 => Ok(f as i64),
                    Some(_) => Err(INT_FROM_FLOAT),
                    None => Err(INT_TYPE),
                },
            },
            Value::String(s) => s.trim().parse::<i64>().map_err(|_| INT_PARSING),
            Value::Bool(b) => Ok(*b as i64),
            _ => Err(INT_TYPE),
        }
    }

    fn to_float(v: &Value) -> Result<f64, &'static str> {
        match v {
            Value::Number(n) => n.as_f64().ok_or(FLOAT_TYPE),
            Value::String(s) => s.trim().parse::<f64>().map_err(|_| FLOAT_PARSING),
            Value::Bool(b) => Ok(*b as i64 as f64),
            _ => Err(FLOAT_TYPE),
        }
    }

    pub fn bool<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
        let v = Value::deserialize(d)?;
        to_bool(&v).map_err(D::Error::custom)
    }

    pub fn opt_bool<'de, D: Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
        let v = Value::deserialize(d)?;
        if v.is_null() {
            return Ok(None);
        }
        to_bool(&v).map(Some).map_err(D::Error::custom)
    }

    pub fn int<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
        let v = Value::deserialize(d)?;
        to_int(&v).map_err(D::Error::custom)
    }

    pub fn opt_int<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
        let v = Value::deserialize(d)?;
        if v.is_null() {
            return Ok(None);
        }
        to_int(&v).map(Some).map_err(D::Error::custom)
    }

    pub fn float<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        let v = Value::deserialize(d)?;
        to_float(&v).map_err(D::Error::custom)
    }

    pub fn opt_float<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
        let v = Value::deserialize(d)?;
        if v.is_null() {
            return Ok(None);
        }
        to_float(&v).map(Some).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize, Debug)]
    #[allow(dead_code)]
    struct Model {
        token: String,
        #[serde(default)]
        agent_ids: Vec<String>,
        #[serde(default, deserialize_with = "lax::opt_bool")]
        flag: Option<bool>,
    }
    impl Validate for Model {
        fn validate(&self, errors: &mut Vec<FieldError>) {
            check_max_len(errors, "token", Some(&self.token), 5);
        }
    }

    #[test]
    fn pydantic_like_errors() {
        let err = from_value::<Model>(json!({"agent_ids": []})).unwrap_err();
        let v = err[0].clone().into_value();
        assert_eq!(v["type"], "missing");
        assert_eq!(v["loc"], json!(["body", "token"]));
        assert_eq!(v["msg"], "Field required");
        let err = from_value::<Model>(json!({"token": 5})).unwrap_err();
        assert_eq!(err[0].kind, "string_type");
        assert_eq!(err[0].loc, vec![json!("body"), json!("token")]);
        let err = from_value::<Model>(json!({"token": "toolong"})).unwrap_err();
        assert_eq!(err[0].kind, "string_too_long");
        let ok = from_value::<Model>(json!({"token": "abc", "flag": "yes", "extra": 1})).unwrap();
        assert_eq!(ok.flag, Some(true));
        assert!(from_value::<Model>(json!([1])).is_err());
    }

    #[test]
    fn query_values() {
        let q = QueryMap::parse(Some("limit=5&limit=7&agent=a%20b"));
        assert_eq!(q.get("limit"), Some("7"));
        assert_eq!(q.get("agent"), Some("a b"));
        assert_eq!(query_int(&q, "limit", Some(100), Some(1), Some(500)).unwrap(), Some(7));
        let bad = query_int(&QueryMap::parse(Some("limit=0")), "limit", Some(100), Some(1), Some(500)).unwrap_err();
        assert_eq!(bad.status.as_u16(), 422);
        assert_eq!(bad.detail[0]["type"], "greater_than_equal");
        assert!(query_required(&q, "missing").is_err());
    }
}

// -- files -----------------------------------------------------------------------------------------

/// Starlette's `FileResponse(path, media_type=…, filename=…, headers=…)`: content-length, last-modified, its
/// etag (`md5("<st_mtime>-<st_size>")`), `content-disposition: attachment` when a filename is given, and single
/// byte ranges (`Range: bytes=a-b` → 206). The file is read when the response is made.
pub async fn file_response(
    path: &str,
    media_type: &str,
    filename: Option<&str>,
    extra: &[(&str, String)],
    request_headers: &HeaderMap,
) -> Response {
    use md5::{Digest, Md5};
    let meta = match tokio::fs::metadata(path).await {
        Ok(m) if m.is_file() => m,
        _ => return HttpError::internal().into_response(),
    };
    let size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as f64 + d.subsec_nanos() as f64 * 1e-9)
        .unwrap_or(0.0);
    let etag =
        format!("\"{}\"", hex::encode(Md5::digest(format!("{}-{}", crate::py::float_repr(mtime), size).as_bytes())));
    let last_modified = httpdate(mtime as i64);
    let data = match tokio::fs::read(path).await {
        Ok(d) => d,
        Err(_) => return HttpError::internal().into_response(),
    };
    let mut status = StatusCode::OK;
    let mut body = data;
    let mut content_range = None;
    if let Some(range) = request_headers.get("range").and_then(|v| v.to_str().ok()) {
        if let Some((start, end)) = parse_single_range(range, size) {
            content_range = Some(format!("bytes {start}-{end}/{size}"));
            body = body[start as usize..=end as usize].to_vec();
            status = StatusCode::PARTIAL_CONTENT;
        } else if range.trim().starts_with("bytes=") && size > 0 {
            let mut r = Response::new(axum::body::Body::from("Range Not Satisfiable"));
            *r.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
            r.headers_mut().insert(
                "content-range",
                HeaderValue::from_str(&format!("*/{size}")).unwrap_or(HeaderValue::from_static("*/0")),
            );
            return r;
        }
    }
    let length = body.len();
    let mut response = Response::new(axum::body::Body::from(body));
    *response.status_mut() = status;
    let h = response.headers_mut();
    let content_type = if media_type.starts_with("text/") && !media_type.contains("charset") {
        format!("{media_type}; charset=utf-8")
    } else {
        media_type.to_string()
    };
    for (name, value) in extra {
        if let (Ok(n), Ok(v)) = (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(value)) {
            h.insert(n, v);
        }
    }
    if let Ok(v) = HeaderValue::from_str(&content_type) {
        h.insert("content-type", v);
    }
    if let Some(name) = filename {
        let quoted = percent_encoding::utf8_percent_encode(name, QUOTE_SET).to_string();
        let disposition = if quoted != name {
            format!("attachment; filename*=utf-8''{quoted}")
        } else {
            format!("attachment; filename=\"{name}\"")
        };
        if let Ok(v) = HeaderValue::from_str(&disposition) {
            h.insert("content-disposition", v);
        }
    }
    h.insert("content-length", HeaderValue::from(length));
    if let Ok(v) = HeaderValue::from_str(&last_modified) {
        h.insert("last-modified", v);
    }
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert("etag", v);
    }
    h.insert("accept-ranges", HeaderValue::from_static("bytes"));
    if let Some(cr) = content_range {
        if let Ok(v) = HeaderValue::from_str(&cr) {
            h.insert("content-range", v);
        }
    }
    response
}

/// Python's `urllib.parse.quote` (safe "/"): what Starlette uses for a filename.
const QUOTE_SET: &percent_encoding::AsciiSet =
    &percent_encoding::NON_ALPHANUMERIC.remove(b'_').remove(b'.').remove(b'-').remove(b'~').remove(b'/');

fn parse_single_range(header: &str, size: u64) -> Option<(u64, u64)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || size == 0 {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        let n = n.min(size);
        if n == 0 {
            return None;
        }
        return Some((size - n, size - 1));
    }
    let start: u64 = a.parse().ok()?;
    let end: u64 = if b.is_empty() { size - 1 } else { b.parse::<u64>().ok()?.min(size - 1) };
    if start > end || start >= size {
        return None;
    }
    Some((start, end))
}

/// An HTTP date (`Tue, 21 Oct 2026 07:48:49 GMT`).
pub fn httpdate(unix_seconds: i64) -> String {
    use chrono::TimeZone;
    chrono::Utc
        .timestamp_opt(unix_seconds, 0)
        .single()
        .map(|d| d.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod json_error_tests {
    use super::*;

    /// Python's `json.loads` errors (position, message), as FastAPI reports them.
    #[tokio::test]
    async fn invalid_json_like_python() {
        let cases: Vec<(&str, usize, &str)> = vec![
            ("{\"x\": ", 6, "Expecting value"),
            ("{\"x\":", 5, "Expecting value"),
            ("{nope", 1, "Expecting property name enclosed in double quotes"),
            ("{\"a\": 1", 7, "Expecting ',' delimiter"),
            ("[1", 2, "Expecting ',' delimiter"),
            ("{\"a\" 1}", 5, "Expecting ':' delimiter"),
            ("{\"a\": 1 \"b\"}", 8, "Expecting ',' delimiter"),
            ("{\"a\": 1} x", 9, "Extra data"),
            (" ", 1, "Expecting value"),
            ("{\"a\": \"\\q\"}", 7, "Invalid \\escape"),
            ("{\"a\": 1,}", 8, "Expecting property name enclosed in double quotes"),
        ];
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let mut wrong = Vec::new();
        for (doc, pos, msg) in cases {
            match read_body(&headers, axum::body::Body::from(doc)).await {
                Ok(BodyValue::Invalid(errors)) => {
                    let v = errors[0].clone().into_value();
                    if v["loc"][1] != json!(pos) || v["ctx"]["error"] != json!(msg) {
                        wrong.push(format!("{doc:?}: got {} {}, expected {pos} {msg}", v["loc"][1], v["ctx"]["error"]));
                    }
                }
                _ => wrong.push(format!("{doc:?}: not invalid")),
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}
