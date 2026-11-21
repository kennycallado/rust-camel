pub mod batch;
pub(crate) mod bitmap;
pub mod geo;
pub mod hash;
pub(crate) mod hll;
pub mod key;
pub mod list;
pub mod other;
pub mod pubsub;
pub mod scan;
pub mod set;
pub mod stream;
pub mod string;
pub mod zset;

use camel_component_api::Body;
use camel_component_api::{CamelError, Exchange};

// ── Header extraction helpers ────────────────────────────────────────────────

pub fn get_str_header<'a>(exchange: &'a Exchange, key: &str) -> Option<&'a str> {
    exchange.input.header(key).and_then(|v| v.as_str())
}

/// Longest rendered value echoed by [`invalid_numeric_header`].
///
/// Header values are untrusted (ADR-0032) and attacker-sized, so the
/// diagnostic keeps a bounded preview instead of echoing an unbounded
/// payload into logs or the DLQ.
const MAX_INVALID_HEADER_VALUE_CHARS: usize = 64;

/// Renders a header value for an error message, truncated at a UTF-8 scalar
/// boundary to [`MAX_INVALID_HEADER_VALUE_CHARS`] characters with a trailing
/// ellipsis only when truncation occurred. Short values stay intact.
pub(crate) fn invalid_header_value_preview(value: &serde_json::Value) -> String {
    let rendered = value.to_string();
    match rendered.char_indices().nth(MAX_INVALID_HEADER_VALUE_CHARS) {
        Some((boundary, _)) => format!("{}…", &rendered[..boundary]),
        None => rendered,
    }
}

/// Named error for a present-but-invalid numeric header. Names both the
/// header and the received value so operators can fix the route config
/// without guessing (ADR-0032: exchange data is untrusted; never silently
/// default a misconfigured numeric header into a Redis-side effect).
fn invalid_numeric_header(key: &str, expected: &str, value: &serde_json::Value) -> CamelError {
    CamelError::ProcessorError(format!(
        "Invalid value for header {key}: expected {expected}, got {}",
        invalid_header_value_preview(value)
    ))
}

/// Rejects a missing or zero TTL/expiry value.
///
/// `CamelRedis.Timeout` and `CamelRedis.Timestamp` are NOT
/// default-to-zero safe for the expiration family: Redis reads a zero
/// `EXPIRE`/`PEXPIRE`/`EXPIREAT`/`PEXPIREAT` argument as an immediate key
/// deletion, and `SETEX` rejects it outright. A header that silently
/// defaulted to `0` (the natural `${env:TTL:-0}` config path, where env
/// interpolation always yields a string) was therefore a data-loss trap.
///
/// This helper makes the misconfiguration fail closed locally, before any
/// command reaches Redis, naming the header and pointing at the explicit
/// `DEL` command (or a positive value).
pub(crate) fn require_positive(
    value: Option<u64>,
    header: &str,
    guidance: &str,
) -> Result<u64, CamelError> {
    match value {
        Some(v) if v > 0 => Ok(v),
        _ => Err(CamelError::ProcessorError(format!(
            "Invalid {header}: {guidance}"
        ))),
    }
}

/// Converts a `u64` command argument to the `i64` Redis expects, rejecting
/// values that would wrap negative.
///
/// A negative expiration is read by Redis as an already-past timestamp, so
/// the `as i64` wrap would silently turn an oversized TTL/timestamp into an
/// immediate key deletion. Values above `i64::MAX` are rejected instead.
pub(crate) fn u64_to_i64_checked(value: u64, header: &str) -> Result<i64, CamelError> {
    i64::try_from(value).map_err(|_| {
        CamelError::ProcessorError(format!(
            "Invalid {header}: value {value} exceeds i64::MAX ({}); refusing to wrap into a negative value",
            i64::MAX
        ))
    })
}

/// Reads a `u64` header. Accepts a JSON integer or a trimmed numeric string
/// (`" 45 "`). Returns `Ok(None)` when the header is absent; a present value
/// that is not a non-negative integer within `u64` range is a named error.
pub fn get_u64_header(exchange: &Exchange, key: &str) -> Result<Option<u64>, CamelError> {
    let Some(value) = exchange.input.header(key) else {
        return Ok(None);
    };
    let expected = "a non-negative integer (u64)";
    match value {
        serde_json::Value::Number(n) => n
            .as_u64()
            .map(Some)
            .ok_or_else(|| invalid_numeric_header(key, expected, value)),
        serde_json::Value::String(s) => s
            .trim()
            .parse::<u64>()
            .map(Some)
            .map_err(|_| invalid_numeric_header(key, expected, value)),
        _ => Err(invalid_numeric_header(key, expected, value)),
    }
}

/// Reads an `i64` header. Accepts a JSON integer or a trimmed numeric string
/// (`" -3 "`). Returns `Ok(None)` when the header is absent; a present value
/// that is not an integer within `i64` range is a named error.
pub fn get_i64_header(exchange: &Exchange, key: &str) -> Result<Option<i64>, CamelError> {
    let Some(value) = exchange.input.header(key) else {
        return Ok(None);
    };
    let expected = "an integer (i64)";
    match value {
        serde_json::Value::Number(n) => n
            .as_i64()
            .map(Some)
            .ok_or_else(|| invalid_numeric_header(key, expected, value)),
        serde_json::Value::String(s) => s
            .trim()
            .parse::<i64>()
            .map(Some)
            .map_err(|_| invalid_numeric_header(key, expected, value)),
        _ => Err(invalid_numeric_header(key, expected, value)),
    }
}

/// Reads an `f64` header. Accepts a JSON number or a trimmed numeric string
/// (`" 1.5 "`). Returns `Ok(None)` when the header is absent; non-finite
/// values (`NaN`, `inf`, overflow like `1e999`) and non-numeric values are
/// named errors rather than silent lossy conversions.
pub fn get_f64_header(exchange: &Exchange, key: &str) -> Result<Option<f64>, CamelError> {
    let Some(value) = exchange.input.header(key) else {
        return Ok(None);
    };
    let expected = "a finite number (f64)";
    let parsed = match value {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    };
    match parsed {
        Some(v) if v.is_finite() => Ok(Some(v)),
        _ => Err(invalid_numeric_header(key, expected, value)),
    }
}

pub fn get_bool_header(exchange: &Exchange, key: &str) -> Option<bool> {
    exchange.input.header(key).and_then(|v| v.as_bool())
}

/// Strict boolean header resolver for option flags.
///
/// [`get_bool_header`] maps a present-but-malformed value to `None`, which the
/// callers cannot tell apart from an absent header: a route that sets a typo
/// would silently run without the option. This helper keeps the two cases
/// distinct — absent is `Ok(None)`, malformed is a named error that echoes a
/// bounded value preview (ADR-0032: fail closed, never default untrusted
/// exchange data into a Redis-side effect).
pub(crate) fn require_bool_header(
    exchange: &Exchange,
    key: &str,
) -> Result<Option<bool>, CamelError> {
    let Some(value) = exchange.input.header(key) else {
        return Ok(None);
    };
    match value {
        serde_json::Value::Bool(b) => Ok(Some(*b)),
        other => Err(CamelError::ProcessorError(format!(
            "Invalid value for header {key}: expected a boolean, got {}",
            invalid_header_value_preview(other)
        ))),
    }
}

pub fn get_str_vec_header(exchange: &Exchange, key: &str) -> Option<Vec<String>> {
    exchange.input.header(key).and_then(|v| {
        v.as_array().map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
    })
}

/// Strict resolver for a non-empty JSON array-of-strings header.
///
/// [`get_str_vec_header`] maps a malformed header to `None` and silently drops
/// non-string entries, so a route with a typo would send a *shorter* list to
/// Redis than configured — a wrong-result or data-loss trap. This resolver
/// accepts only a non-empty JSON array of strings and fails closed, naming the
/// header, the command, and (for a bad entry) its index plus a bounded value
/// preview (ADR-0032: untrusted exchange data never silently becomes a
/// Redis-side effect). `noun` names the list element in the empty-list
/// diagnostic ("key", "member", ...).
pub(crate) fn require_string_list(
    exchange: &Exchange,
    header: &str,
    verb: &str,
    noun: &str,
) -> Result<Vec<String>, CamelError> {
    let Some(value) = exchange.input.header(header) else {
        return Err(CamelError::ProcessorError(format!(
            "{verb}: missing required header {header}: expected a non-empty JSON array of strings"
        )));
    };
    let Some(entries) = value.as_array() else {
        return Err(CamelError::ProcessorError(format!(
            "{verb}: invalid {header}: expected a non-empty JSON array of strings, got {}",
            invalid_header_value_preview(value)
        )));
    };
    if entries.is_empty() {
        return Err(CamelError::ProcessorError(format!(
            "{verb}: invalid {header}: the {noun} list must not be empty"
        )));
    }
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| match entry {
            serde_json::Value::String(item) => Ok(item.clone()),
            other => Err(CamelError::ProcessorError(format!(
                "{verb}: invalid {header}: entry at index {index} is not a string: {}",
                invalid_header_value_preview(other)
            ))),
        })
        .collect()
}

/// Strict resolver for the multi-key commands' `CamelRedis.Keys`.
///
/// The key-list form of [`require_string_list`]; the `MGET` resolver is
/// grandfathered and keeps [`get_str_vec_header`].
pub(crate) fn require_key_list(exchange: &Exchange, verb: &str) -> Result<Vec<String>, CamelError> {
    require_string_list(exchange, "CamelRedis.Keys", verb, "key")
}

/// Reads a raw JSON header value by name.
///
/// Generic message infrastructure, not a payload path: the value-payload
/// contract is body-only (ADR-0098), so nothing in the dispatch reads
/// `CamelRedis.Value` / `CamelRedis.Values` / the GEOADD `CamelRedis.Member`
/// through this helper — those headers are tombstoned before send.
pub fn get_value_header(exchange: &Exchange, key: &str) -> Option<serde_json::Value> {
    exchange.input.header(key).cloned()
}

pub fn require_str_header<'a>(exchange: &'a Exchange, key: &str) -> Result<&'a str, CamelError> {
    get_str_header(exchange, key)
        .ok_or_else(|| CamelError::ProcessorError(format!("Missing required header: {}", key)))
}

pub fn require_key(exchange: &Exchange) -> Result<String, CamelError> {
    require_str_header(exchange, "CamelRedis.Key").map(|s| s.to_string())
}

// ── Value payload helpers ────────────────────────────────────────────────────

/// Named error for a body form that cannot carry a Redis value payload.
///
/// The diagnostic names the command (`verb`) and the rejected variant so the
/// operator can fix the route without guessing; used by
/// [`crate::commands::stream::resolve_xadd_body_bytes`].
fn unencodable_body(verb: &str, variant: &str) -> CamelError {
    CamelError::ProcessorError(format!(
        "{verb} value: the exchange body is {variant}; provide a Text, JSON, or Bytes body"
    ))
}

/// Renders the variant name of a [`Body`] without its payload.
///
/// Only reached by the catch-all arm: `Body` is `#[non_exhaustive]`, so future
/// variants must still be named in the error rather than reported as unknown.
fn body_variant_name(body: &Body) -> String {
    let rendered = format!("{body:?}");
    let end = rendered.find(['(', ' ', '{']).unwrap_or(rendered.len());
    rendered[..end].to_string()
}

/// Reads a JSON object map from an exchange body for map payload commands
/// (`HMSET`, `MSET`).
///
/// A `Body::Json` object is used as-is; `Text` and UTF-8 `Bytes` bodies are
/// parsed as a JSON object. Every other body form is rejected before any
/// command reaches Redis, with an error naming `verb` and the rejected
/// variant. Binary map values are a documented limit: a map body is JSON or
/// UTF-8 text, so binary payloads cannot travel in a map entry.
pub(crate) fn map_body_map(
    body: &Body,
    verb: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, CamelError> {
    match body {
        Body::Json(serde_json::Value::Object(map)) => Ok(map.clone()),
        Body::Text(text) => parse_map_body(text, verb),
        Body::Bytes(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => parse_map_body(text, verb),
            Err(err) => Err(CamelError::ProcessorError(format!(
                "{verb} value: the Bytes body is not valid UTF-8: {err}"
            ))),
        },
        Body::Json(_) => Err(unencodable_map_body(verb, "Body::Json")),
        Body::Xml(_) => Err(unencodable_map_body(verb, "Body::Xml")),
        Body::Empty => Err(unencodable_map_body(verb, "Body::Empty")),
        Body::Stream(_) => Err(unencodable_map_body(verb, "Body::Stream")),
        other => Err(unencodable_map_body(verb, &body_variant_name(other))),
    }
}

/// Parses a `Text`/`Bytes` body into the map payload.
fn parse_map_body(
    text: &str,
    verb: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, CamelError> {
    serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(text).map_err(|e| {
        CamelError::ProcessorError(format!(
            "{verb} value: the exchange body is not a JSON object: {e}"
        ))
    })
}

/// Named error for a body form that cannot carry a map payload.
fn unencodable_map_body(verb: &str, variant: &str) -> CamelError {
    CamelError::ProcessorError(format!(
        "{verb} value: the exchange body is {variant}; provide a JSON object body"
    ))
}

/// Encodes a JSON object map into the key/value pairs a map payload command
/// (`HMSET`, `MSET`) sends to Redis.
///
/// This is the single home of the scalar rule for map entries: a JSON string
/// contributes its raw characters, any other JSON value its compact
/// serialization, and a `null` entry is a named `verb` pre-send error naming
/// the rejected entry, so a null never reaches Redis as a payload.
pub(crate) fn encode_map_entries(
    map: &serde_json::Map<String, serde_json::Value>,
    verb: &str,
) -> Result<Vec<(String, Vec<u8>)>, CamelError> {
    map.iter()
        .map(|(key, value)| {
            let encoded = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => {
                    return Err(CamelError::ProcessorError(format!(
                        "{verb} value: the JSON object entry {key:?} is null; a JSON null is not a valid payload"
                    )));
                }
                other => serde_json::to_string(other).map_err(|e| {
                    CamelError::ProcessorError(format!(
                        "{verb} value: the JSON object entry {key:?} could not be serialized: {e}"
                    ))
                })?,
            };
            Ok((key.clone(), encoded.into_bytes()))
        })
        .collect()
}

/// Named error for a `Body::Json(null)` payload.
///
/// A nil `GET`/`MGET` reply becomes `Body::Json(null)`; without this rejection
/// a route that forwards such a body to a write would store the literal bytes
/// `null` where the caller meant "no value". JSON null carries no payload, so
/// it is refused before the command reaches Redis.
fn json_null_body(verb: &str) -> CamelError {
    CamelError::ProcessorError(format!(
        "{verb} value: the exchange body is Body::Json(null); a JSON null body is not a valid payload"
    ))
}

/// Encodes the exchange body into the raw bytes sent to Redis as a value.
///
/// Codec table: `Text`/`Xml` contribute their UTF-8 bytes; `Json` holding a
/// string contributes the string's raw characters (no JSON quoting), and any
/// other `Json` value its compact serialization; `Bytes` pass through verbatim
/// (binary-safe). `Empty`, `Stream`, `Json(null)`, and any future variant are
/// rejected before any command reaches Redis, with an error naming `verb` and
/// the rejected variant.
pub(crate) fn encode_body_value(body: &Body, verb: &str) -> Result<Vec<u8>, CamelError> {
    match body {
        Body::Text(s) => Ok(s.as_bytes().to_vec()),
        Body::Json(serde_json::Value::String(s)) => Ok(s.as_bytes().to_vec()),
        Body::Json(serde_json::Value::Null) => Err(json_null_body(verb)),
        Body::Json(v) => serde_json::to_string(v)
            .map(String::into_bytes)
            .map_err(|e| {
                CamelError::ProcessorError(format!(
                    "{verb} value: the JSON body could not be serialized: {e}"
                ))
            }),
        Body::Xml(s) => Ok(s.as_bytes().to_vec()),
        Body::Bytes(b) => Ok(b.to_vec()),
        Body::Empty => Err(unencodable_body(verb, "Body::Empty")),
        Body::Stream(_) => Err(unencodable_body(verb, "Body::Stream")),
        other => Err(unencodable_body(verb, &body_variant_name(other))),
    }
}

/// Resolves the value payload bytes for `verb` from the exchange body alone.
///
/// `tombstones` names the headers removed in 0.69 that are still illegal for
/// this command. A present tombstone header fails with a named error before
/// the body is encoded, so a route that still sets the header never writes a
/// silently different payload. The body is the only payload source, so the
/// error never suggests a header as a fallback.
pub(crate) fn body_value_bytes(
    exchange: &Exchange,
    verb: &str,
    tombstones: &[&str],
) -> Result<Vec<u8>, CamelError> {
    for name in tombstones {
        if exchange.input.header(name).is_some() {
            return Err(CamelError::ProcessorError(format!(
                "{name} was removed in 0.69; put the value in the body"
            )));
        }
    }
    encode_body_value(&exchange.input.body, verb)
}

/// Maps a Redis reply payload to an exchange body.
///
/// A nil reply becomes JSON `null`; a byte reply becomes `Text` when it is
/// valid UTF-8 and `Bytes` otherwise, so binary payloads round-trip losslessly.
pub(crate) fn reply_to_body(reply: Option<&[u8]>) -> Body {
    match reply {
        None => Body::Json(serde_json::Value::Null),
        Some(bytes) => match String::from_utf8(bytes.to_vec()) {
            Ok(text) => Body::Text(text),
            Err(_) => Body::Bytes(bytes.to_vec().into()),
        },
    }
}

// ── Script operands and rich replies (redisfc 2.1) ──────────────────────────

/// The documented body shape for script operands, echoed by every
/// body-shape error so the operator sees the expected input.
const SCRIPT_OPERAND_SHAPE: &str = r#"{"keys": [...], "args": [...]}"#;

fn script_operand_shape_error(detail: impl std::fmt::Display) -> CamelError {
    CamelError::ProcessorError(format!(
        "script operands must be a JSON body object {SCRIPT_OPERAND_SHAPE} with string keys and string-or-number args: {detail}"
    ))
}

/// Resolves `EVAL`/`FCALL` keys and args from the exchange body.
///
/// Scripts declare their own multi-key operand layout, which the single-key
/// `CamelRedis.Key` header cannot express, so this is the documented scoped
/// exception to header-sourced key addressing (see the `redis-values` spec).
/// The body MUST be a JSON object `{"keys": [<string>...], "args":
/// [<string|number>...]}`: BOTH arrays are required (a body missing either
/// fails before send), `keys` elements are strings ONLY, and `args` elements
/// are strings or numbers. Every violation fails before send with an error
/// naming the shape. Numeric args are encoded as their decimal string form,
/// which is what the wire protocol carries.
pub fn script_operands(exchange: &Exchange) -> Result<(Vec<String>, Vec<String>), CamelError> {
    let Body::Json(serde_json::Value::Object(body)) = &exchange.input.body else {
        return Err(script_operand_shape_error(format!(
            "got a {} body",
            script_body_shape(&exchange.input.body)
        )));
    };
    let keys = script_operand_array(body.get("keys"), "keys", OperandKind::Key)?;
    let args = script_operand_array(body.get("args"), "args", OperandKind::Arg)?;
    Ok((keys, args))
}

/// Which operand array is being resolved.
///
/// The two arrays share their container rule but not their element rule:
/// `keys` carry key names (strings only) while `args` may carry numbers too.
#[derive(Clone, Copy)]
enum OperandKind {
    /// `keys`: key names, strings only.
    Key,
    /// `args`: strings or numbers (numbers encode as their decimal string).
    Arg,
}

/// Renders the body variant for a body-shape error.
fn script_body_shape(body: &Body) -> &'static str {
    match body {
        Body::Empty => "empty",
        Body::Bytes(_) => "binary",
        Body::Text(_) => "text",
        Body::Json(_) => "non-object JSON",
        Body::Xml(_) => "XML",
        Body::Stream(_) => "streaming",
        _ => "unexpected",
    }
}

/// Maps one operand array of the script body.
///
/// The field is REQUIRED: a body that omits `keys` or `args` fails here
/// rather than silently sending an empty operand list. Non-array values and
/// elements outside the field's element rule fail closed.
fn script_operand_array(
    value: Option<&serde_json::Value>,
    name: &str,
    kind: OperandKind,
) -> Result<Vec<String>, CamelError> {
    let Some(value) = value else {
        return Err(script_operand_shape_error(format!(
            "'{name}' is required: both operand arrays must be present"
        )));
    };
    let serde_json::Value::Array(elements) = value else {
        return Err(script_operand_shape_error(format!(
            "'{name}' must be an array, got {}",
            value
        )));
    };
    elements
        .iter()
        .enumerate()
        .map(|(index, element)| script_operand_element(element, name, index, kind))
        .collect()
}

/// Maps one operand element under its field's element rule.
fn script_operand_element(
    element: &serde_json::Value,
    name: &str,
    index: usize,
    kind: OperandKind,
) -> Result<String, CamelError> {
    match (kind, element) {
        (_, serde_json::Value::String(text)) => Ok(text.clone()),
        // The wire form of a script arg is its decimal string.
        (OperandKind::Arg, serde_json::Value::Number(number)) => Ok(number.to_string()),
        (OperandKind::Key, other) => Err(script_operand_shape_error(format!(
            "'{name}[{index}]' must be a string key, got {other}"
        ))),
        (OperandKind::Arg, other) => Err(script_operand_shape_error(format!(
            "'{name}[{index}]' must be a string or a number, got {other}"
        ))),
    }
}

/// Renders a Redis server error the way Redis reports it (`CODE detail`).
///
/// redis-rs's `Display` for a known-kind server error prints the enum variant
/// name, so the code and its detail are reassembled from the structural
/// accessors instead.
pub(crate) fn server_error_text(err: &redis::ServerError) -> String {
    match err.details() {
        Some(detail) => format!("{}: {detail}", err.code()),
        None => err.code().to_string(),
    }
}

fn server_error_body_error(err: &redis::ServerError) -> CamelError {
    CamelError::ProcessorError(format!(
        "a Redis server error reply ({}) cannot be represented as a body",
        server_error_text(err)
    ))
}

/// Maps a rich `redis::Value` reply to an exchange body.
///
/// The script and batch paths use this instead of `reply_to_body` because
/// they must represent nested replies. The mapping:
///
/// - `Nil` → JSON `null`; `Int` → JSON number; `Double` → JSON number;
/// - `Boolean` → JSON bool; `Okay`/`SimpleString` → text;
/// - `BulkString` → text when UTF-8, bytes at the top level when not (the
///   byte-body contract); nested non-UTF-8 bulk strings become
///   `{"bytes": "<base64>"}` because JSON arrays cannot carry raw bytes;
/// - `Array`/`Set` → JSON array (recursive element mapping); `Map` → JSON
///   object (keys rendered to strings);
/// - `VerbatimString` → its text content; `BigNumber` → its decimal digits as
///   a JSON string;
/// - `Attribute` → the underlying data value (RESP3 attributes are metadata
///   and are dropped); `Push` → JSON `null` (defensive: not expected here);
/// - `ServerError` → an error, at the top level and for any nested entry
///   (batch dispatch extracts per-entry errors with the entry index before
///   conversion; script dispatch treats it as a terminal/business path).
pub fn redis_value_to_body(value: &redis::Value) -> Result<Body, CamelError> {
    match value {
        redis::Value::Nil => Ok(Body::Json(serde_json::Value::Null)),
        redis::Value::Int(number) => Ok(Body::Json(serde_json::Value::from(*number))),
        redis::Value::Okay => Ok(Body::Text("OK".to_string())),
        redis::Value::SimpleString(text) => Ok(Body::Text(text.clone())),
        redis::Value::BulkString(bytes) => Ok(match String::from_utf8(bytes.clone()) {
            Ok(text) => Body::Text(text),
            Err(_) => Body::Bytes(bytes.clone().into()),
        }),
        redis::Value::VerbatimString { text, .. } => Ok(Body::Text(text.clone())),
        redis::Value::Array(elements) | redis::Value::Set(elements) => {
            Ok(Body::Json(serde_json::Value::Array(
                elements
                    .iter()
                    .map(reply_value_to_json)
                    .collect::<Result<Vec<_>, _>>()?,
            )))
        }
        redis::Value::Map(entries) => Ok(Body::Json(map_reply_to_json(entries)?)),
        redis::Value::Double(number) => Ok(Body::Json(serde_json::Value::from(*number))),
        redis::Value::Boolean(flag) => Ok(Body::Json(serde_json::Value::Bool(*flag))),
        redis::Value::BigNumber(number) => {
            Ok(Body::Json(serde_json::Value::String(number.to_string())))
        }
        redis::Value::Attribute { data, .. } => redis_value_to_body(data),
        redis::Value::ServerError(err) => Err(server_error_body_error(err)),
        // Push messages are not expected on the script/batch reply paths; the
        // defensive mapping is JSON null, and the wildcard keeps the
        // `#[non_exhaustive]` match total. Neither arm can panic.
        _ => Ok(Body::Json(serde_json::Value::Null)),
    }
}

/// Element mapping for the nested positions of [`redis_value_to_body`].
///
/// Batch dispatch calls this for each entry's `EXEC`/pipeline reply so the
/// per-entry results reuse the exact element contract (`Nil` → `null`,
/// `Int` → number, non-UTF-8 bulk → `{"bytes": "<base64>"}`, and so on).
pub(crate) fn reply_value_to_json(value: &redis::Value) -> Result<serde_json::Value, CamelError> {
    use base64::Engine;
    let encoded = match value {
        redis::Value::Nil => serde_json::Value::Null,
        redis::Value::Int(number) => serde_json::Value::from(*number),
        redis::Value::Okay => serde_json::Value::String("OK".to_string()),
        redis::Value::SimpleString(text) => serde_json::Value::String(text.clone()),
        redis::Value::BulkString(bytes) => match String::from_utf8(bytes.clone()) {
            Ok(text) => serde_json::Value::String(text),
            // JSON arrays cannot carry raw bytes: the object form is the
            // explicit nested-binary representation.
            Err(_) => serde_json::json!({
                "bytes": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
        },
        redis::Value::VerbatimString { text, .. } => serde_json::Value::String(text.clone()),
        redis::Value::Array(elements) | redis::Value::Set(elements) => serde_json::Value::Array(
            elements
                .iter()
                .map(reply_value_to_json)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        redis::Value::Map(entries) => map_reply_to_json(entries)?,
        redis::Value::Double(number) => serde_json::Value::from(*number),
        redis::Value::Boolean(flag) => serde_json::Value::Bool(*flag),
        redis::Value::BigNumber(number) => serde_json::Value::String(number.to_string()),
        redis::Value::Attribute { data, .. } => reply_value_to_json(data)?,
        redis::Value::ServerError(err) => return Err(server_error_body_error(err)),
        // Defence in depth for the not-expected Push/unknown variants: a
        // nested position renders JSON null and never panics.
        _ => serde_json::Value::Null,
    };
    Ok(encoded)
}

/// A `Map` reply as a JSON object: keys rendered to strings, values via the
/// element mapping.
fn map_reply_to_json(
    entries: &[(redis::Value, redis::Value)],
) -> Result<serde_json::Value, CamelError> {
    let mut object = serde_json::Map::with_capacity(entries.len());
    for (key, value) in entries {
        object.insert(map_key_to_string(key)?, reply_value_to_json(value)?);
    }
    Ok(serde_json::Value::Object(object))
}

/// Renders a `Map` key as the JSON object key it becomes.
///
/// UTF-8 bulk/simple text is used verbatim, integers use their decimal form,
/// and every other kind uses its element-mapped JSON rendered to a string.
fn map_key_to_string(key: &redis::Value) -> Result<String, CamelError> {
    match key {
        redis::Value::BulkString(bytes) => Ok(String::from_utf8_lossy(bytes).into_owned()),
        redis::Value::SimpleString(text) => Ok(text.clone()),
        redis::Value::Okay => Ok("OK".to_string()),
        redis::Value::Int(number) => Ok(number.to_string()),
        other => match reply_value_to_json(other)? {
            serde_json::Value::String(text) => Ok(text),
            rendered => Ok(rendered.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RedisCommand;
    use crate::transport_error::is_transient_redis_error;
    use camel_component_api::{Exchange, Message};
    use std::error::Error as _;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn make_exchange_with_header(key: &str, val: serde_json::Value) -> Exchange {
        let mut msg = Message::default();
        msg.set_header(key, val);
        Exchange::new(msg)
    }

    #[test]
    fn test_get_str_header_found() {
        let ex =
            make_exchange_with_header("CamelRedis.Key", serde_json::Value::String("mykey".into()));
        assert_eq!(get_str_header(&ex, "CamelRedis.Key"), Some("mykey"));
    }

    #[test]
    fn test_get_str_header_missing() {
        let ex = Exchange::new(Message::default());
        assert_eq!(get_str_header(&ex, "CamelRedis.Key"), None);
    }

    #[test]
    fn test_get_u64_header() {
        let ex = make_exchange_with_header("CamelRedis.Timeout", serde_json::json!(30u64));
        assert_eq!(get_u64_header(&ex, "CamelRedis.Timeout").unwrap(), Some(30));
    }

    #[test]
    fn test_get_f64_header() {
        let ex = make_exchange_with_header("CamelRedis.Score", serde_json::json!(3.15f64));
        assert_eq!(get_f64_header(&ex, "CamelRedis.Score").unwrap(), Some(3.15));
    }

    #[test]
    fn test_get_i64_header() {
        let ex = make_exchange_with_header("CamelRedis.Start", serde_json::json!(-1i64));
        assert_eq!(get_i64_header(&ex, "CamelRedis.Start").unwrap(), Some(-1));
    }

    // ── 354 task 1: numeric helper strictness ───────────────────────────────

    #[test]
    fn numeric_helpers_accept_trimmed_numeric_strings() {
        let u = make_exchange_with_header("CamelRedis.Timeout", serde_json::json!(" 45 "));
        assert_eq!(get_u64_header(&u, "CamelRedis.Timeout").unwrap(), Some(45));

        let i = make_exchange_with_header("CamelRedis.Start", serde_json::json!(" -3 "));
        assert_eq!(get_i64_header(&i, "CamelRedis.Start").unwrap(), Some(-3));

        let f = make_exchange_with_header("CamelRedis.Score", serde_json::json!(" 1.5 "));
        assert_eq!(get_f64_header(&f, "CamelRedis.Score").unwrap(), Some(1.5));
    }

    #[test]
    fn numeric_helpers_still_accept_json_numbers() {
        let u = make_exchange_with_header("CamelRedis.Timeout", serde_json::json!(45u64));
        assert_eq!(get_u64_header(&u, "CamelRedis.Timeout").unwrap(), Some(45));

        let i = make_exchange_with_header("CamelRedis.Start", serde_json::json!(-3i64));
        assert_eq!(get_i64_header(&i, "CamelRedis.Start").unwrap(), Some(-3));

        let f = make_exchange_with_header("CamelRedis.Score", serde_json::json!(1.5f64));
        assert_eq!(get_f64_header(&f, "CamelRedis.Score").unwrap(), Some(1.5));
    }

    #[test]
    fn numeric_helpers_absent_header_returns_none() {
        let ex = Exchange::new(Message::default());
        assert_eq!(get_u64_header(&ex, "CamelRedis.Timeout").unwrap(), None);
        assert_eq!(get_i64_header(&ex, "CamelRedis.Start").unwrap(), None);
        assert_eq!(get_f64_header(&ex, "CamelRedis.Score").unwrap(), None);
    }

    fn assert_named_error<T: std::fmt::Debug>(
        result: Result<Option<T>, CamelError>,
        header: &str,
        value_fragment: &str,
    ) {
        let err = result.expect_err("present-but-invalid header must be refused");
        let msg = err.to_string();
        assert!(
            msg.contains(header),
            "error must name header {header}: {msg}"
        );
        assert!(
            msg.contains(value_fragment),
            "error must carry received value {value_fragment}: {msg}"
        );
    }

    #[test]
    fn numeric_helpers_reject_garbage_with_named_error() {
        let ex = make_exchange_with_header("CamelRedis.Timeout", serde_json::json!("abc"));
        assert_named_error(
            get_u64_header(&ex, "CamelRedis.Timeout"),
            "CamelRedis.Timeout",
            "abc",
        );
        assert_named_error(
            get_i64_header(&ex, "CamelRedis.Timeout"),
            "CamelRedis.Timeout",
            "abc",
        );
        assert_named_error(
            get_f64_header(&ex, "CamelRedis.Timeout"),
            "CamelRedis.Timeout",
            "abc",
        );
    }

    #[test]
    fn numeric_helpers_reject_non_numeric_json_types_with_named_error() {
        for value in [
            serde_json::json!(true),
            serde_json::Value::Null,
            serde_json::json!([1, 2]),
            serde_json::json!({"n": 1}),
        ] {
            let ex = make_exchange_with_header("CamelRedis.Timeout", value.clone());
            assert_named_error(
                get_u64_header(&ex, "CamelRedis.Timeout"),
                "CamelRedis.Timeout",
                &value.to_string(),
            );
            assert_named_error(
                get_i64_header(&ex, "CamelRedis.Timeout"),
                "CamelRedis.Timeout",
                &value.to_string(),
            );
            assert_named_error(
                get_f64_header(&ex, "CamelRedis.Timeout"),
                "CamelRedis.Timeout",
                &value.to_string(),
            );
        }
    }

    #[test]
    fn numeric_helpers_reject_u64_negative_and_overflow() {
        for raw in ["-3", "18446744073709551616"] {
            let ex = make_exchange_with_header("CamelRedis.Timeout", serde_json::json!(raw));
            assert_named_error(
                get_u64_header(&ex, "CamelRedis.Timeout"),
                "CamelRedis.Timeout",
                raw,
            );
        }
    }

    #[test]
    fn numeric_helpers_reject_i64_overflow() {
        for raw in ["9223372036854775808", "-9223372036854775809"] {
            let ex = make_exchange_with_header("CamelRedis.Start", serde_json::json!(raw));
            assert_named_error(
                get_i64_header(&ex, "CamelRedis.Start"),
                "CamelRedis.Start",
                raw,
            );
        }
    }

    #[test]
    fn numeric_helpers_reject_nonfinite_f64() {
        for raw in ["NaN", "inf", "-inf", "infinity", "1e999"] {
            let ex = make_exchange_with_header("CamelRedis.Score", serde_json::json!(raw));
            assert_named_error(
                get_f64_header(&ex, "CamelRedis.Score"),
                "CamelRedis.Score",
                raw,
            );
        }
    }

    #[test]
    fn invalid_numeric_header_bounds_long_unicode_value_preview() {
        let long = "é".repeat(100);
        let ex = make_exchange_with_header(
            "CamelRedis.Timeout",
            serde_json::Value::String(long.clone()),
        );
        let err = get_u64_header(&ex, "CamelRedis.Timeout")
            .expect_err("present-but-invalid header must be refused");
        let msg = err.to_string();

        assert!(
            msg.contains("CamelRedis.Timeout"),
            "error must name the header: {msg}"
        );

        // The full untruncated JSON-rendered payload must never be echoed.
        let full = serde_json::Value::String(long).to_string();
        assert!(
            !msg.contains(&full),
            "full value must not appear in the diagnostic: {msg}"
        );

        // The rendered preview is capped at 64 Unicode scalars plus one
        // ellipsis. Truncating on a byte boundary instead would panic or
        // mojibake the multi-byte é payload.
        let preview = msg
            .split_once("got ")
            .map(|(_, p)| p)
            .expect("named error renders the received value after 'got '");
        let expected_head: String = full.chars().take(64).collect();
        let mut rest = preview.chars();
        let head: String = rest.by_ref().take(64).collect();
        assert_eq!(head, expected_head, "preview must keep the first 64 chars");
        assert_eq!(
            rest.next(),
            Some('…'),
            "truncated preview must end in an ellipsis"
        );
        assert_eq!(rest.next(), None, "nothing may follow the ellipsis");
    }

    #[test]
    fn test_get_bool_header() {
        let ex = make_exchange_with_header("CamelRedis.WithScore", serde_json::json!(true));
        assert_eq!(get_bool_header(&ex, "CamelRedis.WithScore"), Some(true));
    }

    #[test]
    fn test_get_str_vec_header() {
        let ex = make_exchange_with_header("CamelRedis.Keys", serde_json::json!(["a", "b", "c"]));
        assert_eq!(
            get_str_vec_header(&ex, "CamelRedis.Keys"),
            Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
        );
    }

    // ── Strict multi-key resolver (rediscmds task 2.2) ──────────────────────

    #[test]
    fn require_key_list_strict_domains() {
        let ok = make_exchange_with_header("CamelRedis.Keys", serde_json::json!(["a", "b"]));
        assert_eq!(
            require_key_list(&ok, "LMPOP").expect("a valid key list must resolve"),
            vec!["a".to_string(), "b".to_string()]
        );

        let rejected = [
            ("empty array", serde_json::json!([])),
            ("non-array", serde_json::json!("a")),
            ("malformed entry", serde_json::json!(["a", 42])),
            ("null entry", serde_json::json!(["a", null])),
        ];
        for (label, value) in rejected {
            let ex = make_exchange_with_header("CamelRedis.Keys", value);
            let err = require_key_list(&ex, "LMPOP").expect_err(label);
            let msg = err.to_string();
            assert!(msg.contains("CamelRedis.Keys"), "{label}: {msg}");
            assert!(msg.contains("LMPOP"), "{label}: {msg}");
        }

        let no_header = Exchange::new(Message::default());
        let err = require_key_list(&no_header, "LMPOP").expect_err("absent header must fail");
        let msg = err.to_string();
        assert!(msg.contains("CamelRedis.Keys"), "{msg}");
        assert!(msg.contains("LMPOP"), "{msg}");

        for (bad_entry, rendered) in [
            (serde_json::json!(42), "42"),
            (serde_json::Value::Null, "null"),
        ] {
            let ex =
                make_exchange_with_header("CamelRedis.Keys", serde_json::json!(["a", bad_entry]));
            let err = require_key_list(&ex, "LMPOP").expect_err("a non-string entry must fail");
            let msg = err.to_string();
            assert!(msg.contains("index 1"), "{msg}");
            assert!(msg.contains(rendered), "{msg}");
        }
    }

    #[test]
    fn test_require_str_header_ok() {
        let ex = make_exchange_with_header("CamelRedis.Key", serde_json::Value::String("k".into()));
        assert_eq!(require_str_header(&ex, "CamelRedis.Key").unwrap(), "k");
    }

    #[test]
    fn test_require_str_header_missing_returns_err() {
        let ex = Exchange::new(Message::default());
        assert!(require_str_header(&ex, "CamelRedis.Key").is_err());
    }

    // ── Command wrap source preservation (rediserr task 2.1) ────────────────

    /// Returns the first argument of the next complete RESP array frame
    /// in `buf`, plus the frame's total byte length. All frames on this
    /// loopback path are plain ASCII (GET and handshake verbs).
    fn next_resp_command(buf: &[u8]) -> Option<(String, usize)> {
        if buf.first() != Some(&b'*') {
            return None;
        }
        let nl = buf.iter().position(|&b| b == b'\n')?;
        let argc = std::str::from_utf8(&buf[1..nl - 1])
            .ok()?
            .parse::<usize>()
            .ok()?;
        let mut pos = nl + 1;
        let mut first_arg = None;
        for i in 0..argc {
            if buf.get(pos) != Some(&b'$') {
                return None;
            }
            let nl = buf[pos..].iter().position(|&b| b == b'\n')? + pos;
            let len = std::str::from_utf8(&buf[pos + 1..nl - 1])
                .ok()?
                .parse::<usize>()
                .ok()?;
            let arg_start = nl + 1;
            let arg_end = arg_start + len;
            if buf.len() < arg_end + 2 {
                return None;
            }
            if i == 0 {
                first_arg = Some(String::from_utf8_lossy(&buf[arg_start..arg_end]).into_owned());
            }
            pos = arg_end + 2;
        }
        first_arg.map(|verb| (verb, pos))
    }

    /// Loopback RESP peer for the command-path wrap test: answers the
    /// redis driver handshake commands (`+OK`), then closes the socket
    /// on the first application command, so that command fails with a
    /// genuine `redis::RedisError` (peer hang-up) instead of a canned
    /// reply. Unit-test scale port of the integration harness
    /// `HandshakeStub` in `tests/common/mod.rs`.
    async fn handshake_then_close_peer() -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback bind must succeed");
        let addr = listener.local_addr().expect("bound listener has an addr");
        tokio::spawn(async move {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 512];
            loop {
                // Answer every complete RESP frame currently buffered
                // (the driver may pipeline its handshake commands).
                while let Some((verb, consumed)) = next_resp_command(&buf) {
                    buf.drain(..consumed);
                    match verb.as_str() {
                        "CLIENT" | "SELECT" | "AUTH" => {
                            if sock.write_all(b"+OK\r\n").await.is_err() {
                                return;
                            }
                        }
                        // First application command: drop the socket so
                        // the driver's read fails.
                        _ => return,
                    }
                }
                let Ok(n) = sock.read(&mut chunk).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
        });
        addr
    }

    #[tokio::test]
    async fn command_wrap_preserves_redis_source() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let addr = handshake_then_close_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            let mut msg = Message::default();
            msg.set_header("CamelRedis.Key", serde_json::Value::String("k".into()));
            let mut exchange = Exchange::new(msg);

            // Normal command path: string::dispatch drives the GET whose
            // wrap site this change converts. The peer closed the socket,
            // so the command fails with a real redis::RedisError.
            let err = string::dispatch(&RedisCommand::Get, &mut conn, &mut exchange)
                .await
                .expect_err("GET against a closed peer must fail");

            assert!(
                is_transient_redis_error(&err),
                "peer-hangup io error must classify transient: {err}"
            );

            // The redis::RedisError must be preserved in the source
            // chain. std wraps the Arc<dyn Error> hop, so unwrap one
            // level like the classifier's walk does before downcasting.
            let src = err
                .source()
                .expect("command wrap must attach the redis error as source");
            let src = src
                .downcast_ref::<Arc<dyn std::error::Error + Send + Sync>>()
                .map(|arc| &**arc as &(dyn std::error::Error + 'static))
                .unwrap_or(src);
            let redis_err = src
                .downcast_ref::<redis::RedisError>()
                .expect("source must downcast to redis::RedisError");

            // Byte-identical wrap text: the legacy site rendered
            // `format!("Redis GET failed: {e}")` inside ProcessorError,
            // whose Display prefixes `Processor error: `.
            assert_eq!(
                err.to_string(),
                format!("Processor error: Redis GET failed: {redis_err}"),
                "wrap text must stay byte-identical to the legacy ProcessorError"
            );
        })
        .await;
        assert!(outcome.is_ok(), "command wrap test timed out: {outcome:?}");
    }

    // ── 354 task 4: invalid TTL/timestamp never reaches Redis ───────────────

    /// Loopback RESP peer that answers the redis driver handshake commands
    /// (`CLIENT`/`SELECT`/`AUTH` -> `+OK`) and records every application
    /// command verb it sees until the peer disconnects. The receiver yields
    /// the recorded verbs once the socket reaches EOF.
    ///
    /// Used by the TTL fail-closed dispatch test: invalid exchanges must be
    /// refused BEFORE any command bytes are written, so the recorded list
    /// must stay empty after the connection is dropped.
    async fn handshake_then_record_peer() -> (
        std::net::SocketAddr,
        tokio::sync::oneshot::Receiver<Vec<String>>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback bind must succeed");
        let addr = listener.local_addr().expect("bound listener has an addr");
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let mut recorded = Vec::new();
            let Ok((mut sock, _)) = listener.accept().await else {
                let _ = tx.send(recorded);
                return;
            };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 512];
            loop {
                while let Some((verb, consumed)) = next_resp_command(&buf) {
                    buf.drain(..consumed);
                    match verb.as_str() {
                        "CLIENT" | "SELECT" | "AUTH" => {
                            if sock.write_all(b"+OK\r\n").await.is_err() {
                                let _ = tx.send(recorded);
                                return;
                            }
                        }
                        other => {
                            recorded.push(other.to_string());
                            // Answer so a regression that DOES reach Redis
                            // terminates instead of hanging the test.
                            if sock.write_all(b"+OK\r\n").await.is_err() {
                                let _ = tx.send(recorded);
                                return;
                            }
                        }
                    }
                }
                match sock.read(&mut chunk).await {
                    Ok(0) | Err(_) => {
                        let _ = tx.send(recorded);
                        return;
                    }
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
        });
        (addr, rx)
    }

    /// Task 354: `EXPIRE`/`PEXPIRE`/`SETEX` with a missing, zero, or garbage
    /// TTL, and `EXPIREAT`/`PEXPIREAT` with a missing, zero, or overflowing
    /// timestamp, must be refused locally — before the driver writes a single
    /// command byte.
    ///
    /// The scripted peer completes the handshake, then records any
    /// application command. Every exchange below is invalid, so `dispatch`
    /// must return a named error and the peer's recorded list must stay
    /// empty: this proves the fail-closed path cannot delete a key and never
    /// incurs a Redis roundtrip (no data loss).
    #[tokio::test]
    async fn dispatch_invalid_ttl_and_timestamp_fails_before_redis_roundtrip() {
        struct Case {
            cmd: RedisCommand,
            headers: Vec<(&'static str, serde_json::Value)>,
            expected_header: &'static str,
        }

        fn key(k: &str) -> Vec<(&'static str, serde_json::Value)> {
            vec![("CamelRedis.Key", serde_json::json!(k))]
        }

        let mut cases: Vec<Case> = Vec::new();
        for cmd in [RedisCommand::Expire, RedisCommand::Pexpire] {
            cases.push(Case {
                cmd: cmd.clone(),
                headers: key("k"),
                expected_header: "CamelRedis.Timeout",
            });
            cases.push(Case {
                cmd: cmd.clone(),
                headers: {
                    let mut h = key("k");
                    h.push(("CamelRedis.Timeout", serde_json::json!(0u64)));
                    h
                },
                expected_header: "CamelRedis.Timeout",
            });
            cases.push(Case {
                cmd: cmd.clone(),
                headers: {
                    let mut h = key("k");
                    h.push(("CamelRedis.Timeout", serde_json::json!("0")));
                    h
                },
                expected_header: "CamelRedis.Timeout",
            });
            cases.push(Case {
                cmd,
                headers: {
                    let mut h = key("k");
                    h.push(("CamelRedis.Timeout", serde_json::json!("abc")));
                    h
                },
                expected_header: "CamelRedis.Timeout",
            });
        }
        for cmd in [RedisCommand::Expireat, RedisCommand::Pexpireat] {
            for timestamp in [
                None,
                Some(serde_json::json!(0u64)),
                Some(serde_json::json!("0")),
                Some(serde_json::json!("abc")),
                Some(serde_json::json!(9223372036854775808u64)),
                Some(serde_json::json!("9223372036854775808")),
            ] {
                let mut headers = key("k");
                if let Some(value) = &timestamp {
                    headers.push(("CamelRedis.Timestamp", value.clone()));
                }
                cases.push(Case {
                    cmd: cmd.clone(),
                    headers,
                    expected_header: "CamelRedis.Timestamp",
                });
            }
        }
        for timeout in [
            None,
            Some(serde_json::json!(0u64)),
            Some(serde_json::json!("0")),
        ] {
            let mut headers = key("k");
            if let Some(value) = &timeout {
                headers.push(("CamelRedis.Timeout", value.clone()));
            }
            cases.push(Case {
                cmd: RedisCommand::Setex,
                headers,
                expected_header: "CamelRedis.Timeout",
            });
        }

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            for case in cases {
                let mut msg = if matches!(&case.cmd, RedisCommand::Setex) {
                    Message::new(Body::Text("v".into()))
                } else {
                    Message::default()
                };
                for (k, v) in case.headers {
                    msg.set_header(k, v);
                }
                let mut exchange = Exchange::new(msg);

                let result = if matches!(case.cmd, RedisCommand::Setex) {
                    string::dispatch(&case.cmd, &mut conn, &mut exchange).await
                } else {
                    key::dispatch(&case.cmd, &mut conn, &mut exchange).await
                };
                let err = result.expect_err(
                    "an invalid TTL/timestamp must be refused locally before any Redis command",
                );
                let msg = err.to_string();
                assert!(
                    msg.contains(case.expected_header),
                    "{:?} error must name {}: {msg}",
                    case.cmd,
                    case.expected_header
                );
                assert!(
                    msg.contains("positive") || msg.contains("expected") || msg.contains("exceeds"),
                    "{:?} error must be actionable (explain the expected value): {msg}",
                    case.cmd
                );
            }

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("dispatch fail-closed test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent for an invalid TTL/timestamp: {recorded:?}"
        );
    }

    // ── redisbmp task 1.1: bit access fails closed before send ──────────────

    /// `SETBIT` with a non-binary `CamelRedis.BitValue` must be refused
    /// locally, naming the header, before the driver writes a command byte.
    #[tokio::test]
    async fn test_setbit_rejects_non_binary_bit_value() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            let mut msg = Message::default();
            msg.set_header("CamelRedis.Key", serde_json::json!("k"));
            msg.set_header("CamelRedis.BitOffset", serde_json::json!(7u64));
            msg.set_header("CamelRedis.BitValue", serde_json::json!(2u64));
            let mut exchange = Exchange::new(msg);

            let err = bitmap::dispatch(&RedisCommand::Setbit, &mut conn, &mut exchange)
                .await
                .expect_err("SETBIT with a non-binary bit value must be refused locally");
            let msg = err.to_string();
            assert!(msg.contains("CamelRedis.BitValue"), "{msg}");

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("SETBIT bit-value rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent for a non-binary bit value: {recorded:?}"
        );
    }

    /// `SETBIT` without `CamelRedis.BitOffset` must be refused locally: the
    /// offset has no safe default, so it is a named missing-header error.
    #[tokio::test]
    async fn test_setbit_rejects_missing_bit_offset() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            let mut msg = Message::default();
            msg.set_header("CamelRedis.Key", serde_json::json!("k"));
            msg.set_header("CamelRedis.BitValue", serde_json::json!(1u64));
            let mut exchange = Exchange::new(msg);

            let err = bitmap::dispatch(&RedisCommand::Setbit, &mut conn, &mut exchange)
                .await
                .expect_err("SETBIT without a bit offset must be refused locally");
            let msg = err.to_string();
            assert!(msg.contains("CamelRedis.BitOffset"), "{msg}");

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("SETBIT missing-offset rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent without a bit offset: {recorded:?}"
        );
    }

    /// A bit offset above the 512 MB bitstring cap (`4294967295`) must be
    /// refused locally, naming `CamelRedis.BitOffset`.
    #[tokio::test]
    async fn test_bit_offset_out_of_range_rejected() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            let mut msg = Message::default();
            msg.set_header("CamelRedis.Key", serde_json::json!("k"));
            msg.set_header("CamelRedis.BitOffset", serde_json::json!(4294967296u64));
            msg.set_header("CamelRedis.BitValue", serde_json::json!(1u64));
            let mut exchange = Exchange::new(msg);

            let err = bitmap::dispatch(&RedisCommand::Setbit, &mut conn, &mut exchange)
                .await
                .expect_err("an offset above the 512 MB bitstring cap must be refused locally");
            let msg = err.to_string();
            assert!(msg.contains("CamelRedis.BitOffset"), "{msg}");

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("bit-offset range rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent for an out-of-range bit offset: {recorded:?}"
        );
    }

    // ── redisbmp task 1.2: BITCOUNT / BITPOS ranges fail closed ─────────────

    /// `BITCOUNT` needs `CamelRedis.Start` and `CamelRedis.End` together, so
    /// either endpoint on its own must be refused locally with an error that
    /// names both headers and the both-endpoints rule.
    #[tokio::test]
    async fn test_bitcount_rejects_incomplete_range() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            for only in ["CamelRedis.Start", "CamelRedis.End"] {
                let mut msg = Message::default();
                msg.set_header("CamelRedis.Key", serde_json::json!("k"));
                msg.set_header(only, serde_json::json!(0i64));
                let mut exchange = Exchange::new(msg);

                let err = bitmap::dispatch(&RedisCommand::Bitcount, &mut conn, &mut exchange)
                    .await
                    .expect_err("BITCOUNT with only one endpoint must be refused locally");
                let msg = err.to_string();
                assert!(
                    msg.contains("CamelRedis.Start"),
                    "{only} rejection must name CamelRedis.Start: {msg}"
                );
                assert!(
                    msg.contains("CamelRedis.End"),
                    "{only} rejection must name CamelRedis.End: {msg}"
                );
                assert!(
                    msg.contains("both"),
                    "{only} rejection must state the both-endpoints rule: {msg}"
                );
            }

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("BITCOUNT incomplete-range rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent for an incomplete range: {recorded:?}"
        );
    }

    /// A `CamelRedis.BitRangeUnit` token is meaningful only for a complete
    /// range: `BITPOS` with a bare start and a unit must fail locally, naming
    /// `CamelRedis.End` and the unit-requires-both-endpoints rule.
    #[tokio::test]
    async fn test_bitpos_unit_requires_both_endpoints() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            let mut msg = Message::default();
            msg.set_header("CamelRedis.Key", serde_json::json!("k"));
            msg.set_header("CamelRedis.BitValue", serde_json::json!(1u64));
            msg.set_header("CamelRedis.Start", serde_json::json!(0i64));
            msg.set_header("CamelRedis.BitRangeUnit", serde_json::json!("BIT"));
            let mut exchange = Exchange::new(msg);

            let err = bitmap::dispatch(&RedisCommand::Bitpos, &mut conn, &mut exchange)
                .await
                .expect_err("a unit without a complete range must be refused locally");
            let msg = err.to_string();
            assert!(
                msg.contains("CamelRedis.End"),
                "error must name CamelRedis.End: {msg}"
            );
            assert!(
                msg.contains("both endpoints"),
                "error must state the unit-requires-both-endpoints rule: {msg}"
            );

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("BITPOS unit-without-range rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent for a unit without a full range: {recorded:?}"
        );
    }

    /// A present-but-malformed `CamelRedis.BitRangeUnit` must fail closed: a
    /// non-string (`42`) and an unknown token (`NIBBLE`) are named errors, not
    /// a silent fallback to the documented `BYTE` default.
    #[tokio::test]
    async fn test_bitrange_unit_rejects_present_malformed_values() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            for unit in [serde_json::json!(42), serde_json::json!("NIBBLE")] {
                let mut msg = Message::default();
                msg.set_header("CamelRedis.Key", serde_json::json!("k"));
                msg.set_header("CamelRedis.Start", serde_json::json!(0i64));
                msg.set_header("CamelRedis.End", serde_json::json!(2i64));
                msg.set_header("CamelRedis.BitRangeUnit", unit.clone());
                let mut exchange = Exchange::new(msg);

                let err = bitmap::dispatch(&RedisCommand::Bitcount, &mut conn, &mut exchange)
                    .await
                    .expect_err("a malformed range unit must be refused locally");
                let msg = err.to_string();
                assert!(
                    msg.contains("CamelRedis.BitRangeUnit"),
                    "{unit} rejection must name CamelRedis.BitRangeUnit: {msg}"
                );
            }

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("range-unit strictness test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent for a malformed range unit: {recorded:?}"
        );
    }

    // ── redisbmp task 1.3: BITOP fails closed before send ───────────────────

    /// `BITOP NOT` accepts exactly one source key, so a two-key `CamelRedis.Keys`
    /// list must be refused locally with an error that names the header and the
    /// single-source rule.
    #[tokio::test]
    async fn test_bitop_not_rejects_multiple_sources() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            let mut msg = Message::default();
            msg.set_header("CamelRedis.BitOperation", serde_json::json!("NOT"));
            msg.set_header("CamelRedis.Destination", serde_json::json!("d"));
            msg.set_header("CamelRedis.Keys", serde_json::json!(["a", "b"]));
            let mut exchange = Exchange::new(msg);

            let err = bitmap::dispatch(&RedisCommand::Bitop, &mut conn, &mut exchange)
                .await
                .expect_err("BITOP NOT with two sources must be refused locally");
            let msg = err.to_string();
            assert!(
                msg.contains("CamelRedis.Keys"),
                "error must name CamelRedis.Keys: {msg}"
            );
            assert!(
                msg.contains("exactly one"),
                "error must state the NOT single-source rule: {msg}"
            );

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("BITOP NOT multi-source rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent for BITOP NOT with two sources: {recorded:?}"
        );
    }

    /// `BITOP AND` requires at least one source key, so an empty
    /// `CamelRedis.Keys` array must be refused locally.
    #[tokio::test]
    async fn test_bitop_and_rejects_empty_sources() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            let mut msg = Message::default();
            msg.set_header("CamelRedis.BitOperation", serde_json::json!("AND"));
            msg.set_header("CamelRedis.Destination", serde_json::json!("d"));
            msg.set_header("CamelRedis.Keys", serde_json::json!([]));
            let mut exchange = Exchange::new(msg);

            let err = bitmap::dispatch(&RedisCommand::Bitop, &mut conn, &mut exchange)
                .await
                .expect_err("BITOP AND with no sources must be refused locally");
            let msg = err.to_string();
            assert!(
                msg.contains("CamelRedis.Keys"),
                "error must name CamelRedis.Keys: {msg}"
            );

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("BITOP AND empty-source rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent for BITOP AND with no sources: {recorded:?}"
        );
    }

    /// A non-string entry in `CamelRedis.Keys` must fail closed: the strict list
    /// resolver never drops it and runs the command against fewer sources.
    #[tokio::test]
    async fn test_bitop_rejects_non_string_key_entries() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            let mut msg = Message::default();
            msg.set_header("CamelRedis.BitOperation", serde_json::json!("OR"));
            msg.set_header("CamelRedis.Destination", serde_json::json!("d"));
            msg.set_header("CamelRedis.Keys", serde_json::json!(["a", 42]));
            let mut exchange = Exchange::new(msg);

            let err = bitmap::dispatch(&RedisCommand::Bitop, &mut conn, &mut exchange)
                .await
                .expect_err("a non-string source key must be refused locally");
            let msg = err.to_string();
            assert!(
                msg.contains("CamelRedis.Keys"),
                "error must name CamelRedis.Keys: {msg}"
            );

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("BITOP non-string-source rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent for a non-string source key: {recorded:?}"
        );
    }

    /// `CamelRedis.BitOperation` accepts only `AND`, `OR`, `XOR`, and `NOT`; an
    /// unknown token or a non-string value is a named error listing the valid
    /// operations.
    #[tokio::test]
    async fn test_bitop_rejects_unknown_operation() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            for operation in [serde_json::json!("NAND"), serde_json::json!(42)] {
                let mut msg = Message::default();
                msg.set_header("CamelRedis.BitOperation", operation.clone());
                msg.set_header("CamelRedis.Destination", serde_json::json!("d"));
                msg.set_header("CamelRedis.Keys", serde_json::json!(["a"]));
                let mut exchange = Exchange::new(msg);

                let err = bitmap::dispatch(&RedisCommand::Bitop, &mut conn, &mut exchange)
                    .await
                    .expect_err("an unknown bit operation must be refused locally");
                let msg = err.to_string();
                assert!(
                    msg.contains("CamelRedis.BitOperation"),
                    "{operation} rejection must name CamelRedis.BitOperation: {msg}"
                );
                for valid in ["AND", "OR", "XOR", "NOT"] {
                    assert!(
                        msg.contains(valid),
                        "{operation} rejection must list {valid}: {msg}"
                    );
                }
            }

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("BITOP unknown-operation rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent for an unknown bit operation: {recorded:?}"
        );
    }

    /// `BITOP` without `CamelRedis.Destination` must be refused locally with a
    /// missing-header error that names the header and differs from the invalid
    /// type error.
    #[tokio::test]
    async fn test_bitop_rejects_missing_destination() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            let mut msg = Message::default();
            msg.set_header("CamelRedis.BitOperation", serde_json::json!("OR"));
            msg.set_header("CamelRedis.Keys", serde_json::json!(["a"]));
            let mut exchange = Exchange::new(msg);

            let err = bitmap::dispatch(&RedisCommand::Bitop, &mut conn, &mut exchange)
                .await
                .expect_err("BITOP without a destination must be refused locally");
            let msg = err.to_string();
            assert!(
                msg.contains("CamelRedis.Destination"),
                "error must name CamelRedis.Destination: {msg}"
            );
            assert!(
                msg.contains("Missing required header"),
                "error must state that the destination is missing: {msg}"
            );
            assert!(
                !msg.contains("expected a string"),
                "a missing destination must not report an invalid-type cause: {msg}"
            );

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("BITOP missing-destination rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent without a destination: {recorded:?}"
        );
    }

    /// A non-string `CamelRedis.Destination` is present-but-invalid, not
    /// missing: the strict resolver must name the header and the expected
    /// string type instead of reporting an absent header.
    #[tokio::test]
    async fn test_bitop_rejects_non_string_destination() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            let mut msg = Message::default();
            msg.set_header("CamelRedis.BitOperation", serde_json::json!("OR"));
            msg.set_header("CamelRedis.Destination", serde_json::json!(42));
            msg.set_header("CamelRedis.Keys", serde_json::json!(["a"]));
            let mut exchange = Exchange::new(msg);

            let err = bitmap::dispatch(&RedisCommand::Bitop, &mut conn, &mut exchange)
                .await
                .expect_err("a non-string destination must be refused locally");
            let msg = err.to_string();
            assert!(
                msg.contains("CamelRedis.Destination"),
                "error must name CamelRedis.Destination: {msg}"
            );
            assert!(
                msg.contains("expected a string"),
                "error must state the expected string type: {msg}"
            );
            assert!(
                !msg.contains("Missing required header"),
                "a present non-string destination must not be reported as missing: {msg}"
            );

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("BITOP non-string-destination rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent for a non-string destination: {recorded:?}"
        );
    }

    // ── redisbmp task 2.1: BITFIELD fails closed before send ────────────────

    /// A mutating `BITFIELD` entry with no effective overflow mode must be
    /// refused locally: the server default `WRAP` must never apply by accident.
    #[tokio::test]
    async fn test_bitfield_dispatch_rejects_missing_overflow_before_send() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            let mut msg = Message::default();
            msg.set_header("CamelRedis.Key", serde_json::json!("k"));
            msg.set_header(
                "CamelRedis.BitFieldOps",
                serde_json::json!(["INCRBY u8 #0 1"]),
            );
            let mut exchange = Exchange::new(msg);

            let err = bitmap::dispatch(&RedisCommand::Bitfield, &mut conn, &mut exchange)
                .await
                .expect_err("BITFIELD without an overflow mode must be refused locally");
            let msg = err.to_string();
            assert!(
                msg.contains("CamelRedis.BitFieldOverflow"),
                "error must name CamelRedis.BitFieldOverflow: {msg}"
            );

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("BITFIELD missing-overflow rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent without an overflow mode: {recorded:?}"
        );
    }

    /// A present but malformed `CamelRedis.BitFieldOverflow` must never be
    /// treated as absent, even when an inline `OVERFLOW` entry supplies a mode.
    #[tokio::test]
    async fn test_bitfield_overflow_header_rejects_present_malformed_values() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            for value in [serde_json::json!(42), serde_json::json!("SOMETIMES")] {
                let mut msg = Message::default();
                msg.set_header("CamelRedis.Key", serde_json::json!("k"));
                msg.set_header("CamelRedis.BitFieldOverflow", value);
                msg.set_header(
                    "CamelRedis.BitFieldOps",
                    serde_json::json!(["OVERFLOW SAT", "INCRBY u8 #0 1"]),
                );
                let mut exchange = Exchange::new(msg);

                let err = bitmap::dispatch(&RedisCommand::Bitfield, &mut conn, &mut exchange)
                    .await
                    .expect_err("a present but malformed overflow header must be refused locally");
                let msg = err.to_string();
                assert!(
                    msg.contains("CamelRedis.BitFieldOverflow"),
                    "error must name CamelRedis.BitFieldOverflow: {msg}"
                );
            }

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded =
            outcome.expect("BITFIELD malformed-overflow-header rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent for a malformed overflow header: {recorded:?}"
        );
    }

    // ── redisbmp task 3.1: HyperLogLog operands fail closed before send ─────

    /// `PFADD` takes its element from the body (ADR-0098), so a route that
    /// still sets the removed `CamelRedis.Value` header must fail locally with
    /// the tombstone error before the driver writes a command byte.
    #[tokio::test]
    async fn test_pfadd_rejects_tombstoned_value_header() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            let mut msg = Message::new(Body::Text("café".into()));
            msg.set_header("CamelRedis.Key", serde_json::json!("k"));
            msg.set_header(crate::HEADER_VALUE, serde_json::json!("café"));
            let mut exchange = Exchange::new(msg);

            let err = hll::dispatch(&RedisCommand::Pfadd, &mut conn, &mut exchange)
                .await
                .expect_err("a tombstoned value header must be refused locally");
            let msg = err.to_string();
            assert!(
                msg.contains("CamelRedis.Value was removed in 0.69"),
                "error must name the tombstoned header: {msg}"
            );
            assert!(
                msg.contains("put the value in the body"),
                "error must point at the body contract: {msg}"
            );

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("PFADD tombstoned-header rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent for a tombstoned value header: {recorded:?}"
        );
    }

    /// `PFCOUNT` requires one or more keys: an empty list and a missing header
    /// must both fail locally with a named error stating the non-empty rule.
    #[tokio::test]
    async fn test_pfcount_rejects_empty_keys() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            for keys in [None, Some(serde_json::json!([]))] {
                let mut msg = Message::default();
                if let Some(value) = keys {
                    msg.set_header("CamelRedis.Keys", value);
                }
                let mut exchange = Exchange::new(msg);

                let err = hll::dispatch(&RedisCommand::Pfcount, &mut conn, &mut exchange)
                    .await
                    .expect_err("PFCOUNT without a source key must be refused locally");
                let msg = err.to_string();
                assert!(
                    msg.contains("CamelRedis.Keys"),
                    "error must name CamelRedis.Keys: {msg}"
                );
                assert!(
                    msg.contains("empty"),
                    "error must state the non-empty (>= 1 key) rule: {msg}"
                );
            }

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("PFCOUNT empty-key rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent without a source key: {recorded:?}"
        );
    }

    /// `CamelRedis.Keys` is parsed strictly: a non-string entry must fail
    /// locally instead of being silently dropped from the wire frame.
    #[tokio::test]
    async fn test_pfcount_rejects_non_string_key_entries() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            let mut msg = Message::default();
            msg.set_header("CamelRedis.Keys", serde_json::json!(["h1", 42]));
            let mut exchange = Exchange::new(msg);

            let err = hll::dispatch(&RedisCommand::Pfcount, &mut conn, &mut exchange)
                .await
                .expect_err("a non-string key entry must be refused locally");
            let msg = err.to_string();
            assert!(
                msg.contains("CamelRedis.Keys"),
                "error must name CamelRedis.Keys: {msg}"
            );
            assert!(
                msg.contains("index 1"),
                "error must name the offending entry: {msg}"
            );

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("PFCOUNT non-string-key rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent for a non-string key entry: {recorded:?}"
        );
    }

    /// `PFMERGE` writes to `CamelRedis.Destination`: a missing destination is
    /// a named error before send, never a merge into an unintended key.
    #[tokio::test]
    async fn test_pfmerge_rejects_missing_destination() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            let mut msg = Message::default();
            msg.set_header("CamelRedis.Keys", serde_json::json!(["h1"]));
            let mut exchange = Exchange::new(msg);

            let err = hll::dispatch(&RedisCommand::Pfmerge, &mut conn, &mut exchange)
                .await
                .expect_err("PFMERGE without a destination must be refused locally");
            let msg = err.to_string();
            assert!(
                msg.contains("CamelRedis.Destination"),
                "error must name CamelRedis.Destination: {msg}"
            );

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("PFMERGE missing-destination rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent without a destination: {recorded:?}"
        );
    }

    // ── redisbmp task 4.1: WAIT arguments fail closed before send ───────────

    /// `CamelRedis.WaitTimeout=0` makes Redis `WAIT ... 0` block forever, so
    /// the zero value must be refused locally, naming the header, before the
    /// driver writes a command byte.
    #[tokio::test]
    async fn test_wait_rejects_zero_timeout() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            let mut msg = Message::default();
            msg.set_header("CamelRedis.NumReplicas", serde_json::json!(1u64));
            msg.set_header("CamelRedis.WaitTimeout", serde_json::json!(0u64));
            let mut exchange = Exchange::new(msg);

            let err = other::dispatch(&RedisCommand::Wait, &mut conn, &mut exchange)
                .await
                .expect_err("a zero WaitTimeout would block forever and must be refused locally");
            assert!(
                err.to_string().contains("CamelRedis.WaitTimeout"),
                "error must name CamelRedis.WaitTimeout: {err}"
            );

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("WAIT zero-timeout rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent for a zero WaitTimeout: {recorded:?}"
        );
    }

    /// Both `WAIT` operands are required: a missing `CamelRedis.NumReplicas`
    /// or `CamelRedis.WaitTimeout` must fail locally with a named error.
    #[tokio::test]
    async fn test_wait_rejects_missing_headers() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            let cases: [(&str, Vec<(&str, serde_json::Value)>); 2] = [
                (
                    "CamelRedis.NumReplicas",
                    vec![("CamelRedis.WaitTimeout", serde_json::json!(1000u64))],
                ),
                (
                    "CamelRedis.WaitTimeout",
                    vec![("CamelRedis.NumReplicas", serde_json::json!(1u64))],
                ),
            ];
            for (missing, headers) in cases {
                let mut msg = Message::default();
                for (key, value) in headers {
                    msg.set_header(key, value);
                }
                let mut exchange = Exchange::new(msg);

                let err = other::dispatch(&RedisCommand::Wait, &mut conn, &mut exchange)
                    .await
                    .expect_err("both WAIT operands are required");
                assert!(
                    err.to_string().contains(missing),
                    "error must name the missing header {missing}: {err}"
                );
            }

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("WAIT missing-header rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent without both WAIT operands: {recorded:?}"
        );
    }

    /// Redis parses `WAIT`'s arguments as signed 64-bit, so a `u64` above
    /// `i64::MAX` cannot travel on the wire. Both operands must be checked
    /// before send, naming the header and the signed wire range.
    #[tokio::test]
    async fn test_wait_rejects_values_above_wire_range() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (addr, recorded_rx) = handshake_then_record_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            let cases: [(&str, Vec<(&str, serde_json::Value)>); 2] = [
                (
                    "CamelRedis.WaitTimeout",
                    vec![
                        ("CamelRedis.NumReplicas", serde_json::json!(1u64)),
                        ("CamelRedis.WaitTimeout", serde_json::json!(u64::MAX)),
                    ],
                ),
                (
                    "CamelRedis.NumReplicas",
                    vec![
                        ("CamelRedis.NumReplicas", serde_json::json!(u64::MAX)),
                        ("CamelRedis.WaitTimeout", serde_json::json!(1000u64)),
                    ],
                ),
            ];
            for (header, headers) in cases {
                let mut msg = Message::default();
                for (key, value) in headers {
                    msg.set_header(key, value);
                }
                let mut exchange = Exchange::new(msg);

                let err = other::dispatch(&RedisCommand::Wait, &mut conn, &mut exchange)
                    .await
                    .expect_err("values above the signed 64-bit wire range must be refused");
                let msg = err.to_string();
                assert!(msg.contains(header), "error must name {header}: {msg}");
                assert!(
                    msg.contains("signed 64-bit") || msg.contains("i64::MAX"),
                    "error must name the signed 64-bit wire range: {msg}"
                );
            }

            drop(conn);
            recorded_rx
                .await
                .expect("peer must report recorded verbs on EOF")
        })
        .await;

        let recorded = outcome.expect("WAIT wire-range rejection test timed out");
        assert!(
            recorded.is_empty(),
            "no Redis command may be sent for an out-of-range WAIT operand: {recorded:?}"
        );
    }
}

#[cfg(test)]
mod value_helper_tests {
    use super::*;
    use camel_component_api::{Message, StreamBody};
    use std::sync::Arc;

    fn ex_with(body: Body, headers: &[(&str, serde_json::Value)]) -> Exchange {
        let mut msg = Message::new(body);
        for (k, v) in headers {
            msg.set_header(*k, v.clone());
        }
        Exchange::new(msg)
    }

    fn processor_message(err: CamelError) -> String {
        match err {
            CamelError::ProcessorError(msg) => msg,
            other => panic!("expected ProcessorError, got {other:?}"),
        }
    }

    #[test]
    fn encode_body_value_text_yields_utf8_bytes() {
        let encoded =
            encode_body_value(&Body::Text("café".into()), "SET").expect("text body must encode");
        assert_eq!(encoded, b"caf\xc3\xa9".to_vec());
    }

    #[test]
    fn encode_body_value_json_is_compact() {
        let encoded = encode_body_value(&Body::Json(serde_json::json!({"a": 1})), "SET")
            .expect("json body must encode");
        assert_eq!(encoded, br#"{"a":1}"#.to_vec());
    }

    #[test]
    fn encode_body_value_bytes_is_verbatim() {
        let payload = vec![0xFF, 0x00, 0xC3, 0x28];
        let encoded = encode_body_value(&Body::Bytes(payload.clone().into()), "SET")
            .expect("bytes body must encode");
        assert_eq!(encoded, payload);
    }

    #[test]
    fn encode_body_value_xml_yields_the_xml_text() {
        let encoded =
            encode_body_value(&Body::Xml("<a/>".into()), "SET").expect("xml body must encode");
        assert_eq!(encoded, b"<a/>".to_vec());
    }

    // ── Body-only codec and tombstone resolver (redisbodyonly 1.1) ─────────

    #[test]
    fn codec_json_string_yields_raw_characters() {
        let ex = ex_with(Body::Json(serde_json::Value::String("abc".into())), &[]);
        let encoded =
            body_value_bytes(&ex, "SET", &[]).expect("a JSON string body must encode raw");
        assert_eq!(encoded, b"abc".to_vec());
        assert_eq!(
            encoded.len(),
            3,
            "no JSON quote bytes may be added to a string body"
        );
    }

    #[test]
    fn codec_rejects_json_null_body() {
        let ex = ex_with(Body::Json(serde_json::Value::Null), &[]);
        let msg = processor_message(
            body_value_bytes(&ex, "SET", &[]).expect_err("a JSON null body must be rejected"),
        );
        assert!(
            msg.contains("Body::Json(null)"),
            "error must name the rejected body form: {msg}"
        );
        assert!(msg.contains("SET"), "error must name the verb: {msg}");
    }

    #[test]
    fn codec_accepts_empty_string_rejects_empty_body() {
        let text = ex_with(Body::Text(String::new()), &[]);
        assert_eq!(
            body_value_bytes(&text, "SET", &[]).expect("an empty text body is a valid payload"),
            Vec::<u8>::new()
        );

        let json_string = ex_with(Body::Json(serde_json::Value::String(String::new())), &[]);
        assert_eq!(
            body_value_bytes(&json_string, "SET", &[])
                .expect("an empty JSON string body is a valid payload"),
            Vec::<u8>::new()
        );

        let empty = ex_with(Body::Empty, &[]);
        let msg = processor_message(
            body_value_bytes(&empty, "SET", &[]).expect_err("an empty body must be rejected"),
        );
        assert!(
            msg.contains("Body::Empty"),
            "error must name the rejected variant: {msg}"
        );
    }

    #[test]
    fn tombstone_header_fails_before_encoding() {
        let ex = ex_with(
            Body::Text("body".into()),
            &[(crate::HEADER_VALUE, serde_json::json!("v"))],
        );
        let msg = processor_message(
            body_value_bytes(&ex, "SET", &[crate::HEADER_VALUE])
                .expect_err("the removed value header must be rejected"),
        );
        assert!(
            msg.contains("CamelRedis.Value was removed in 0.69"),
            "error must name the tombstoned header: {msg}"
        );
    }

    #[test]
    fn encode_body_value_empty_and_stream_name_command_and_variant() {
        let stream = Body::Stream(StreamBody {
            stream: Arc::new(tokio::sync::Mutex::new(None)),
            metadata: Default::default(),
        });
        for (variant, body) in [("Body::Empty", Body::Empty), ("Body::Stream", stream)] {
            let msg = processor_message(
                encode_body_value(&body, "SET").expect_err("unencodable body must be rejected"),
            );
            assert!(
                msg.contains("SET"),
                "{variant} error must name the verb: {msg}"
            );
            assert!(
                msg.contains(variant),
                "error must name the rejected variant {variant}: {msg}"
            );
        }
    }

    #[test]
    fn reply_to_body_rule() {
        assert_eq!(reply_to_body(None), Body::Json(serde_json::Value::Null));
        assert_eq!(
            reply_to_body(Some("café".as_bytes())),
            Body::Text("café".into())
        );
        assert_eq!(
            reply_to_body(Some(&[0xC3, 0x28])),
            Body::Bytes(vec![0xC3, 0x28].into())
        );
    }
}

#[cfg(test)]
mod script_operand_tests {
    use super::*;
    use camel_component_api::Message;

    fn body_exchange(body: Body) -> Exchange {
        Exchange::new(Message::new(body))
    }

    #[test]
    fn script_keys_and_args_bind_from_body() {
        let exchange = body_exchange(Body::Json(serde_json::json!({
            "keys": ["a", "b"],
            "args": ["x", 7],
        })));

        let (keys, args) =
            script_operands(&exchange).expect("a well-formed operand body must bind");

        assert_eq!(keys, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            args,
            vec!["x".to_string(), "7".to_string()],
            "numeric args encode as their decimal string form"
        );
    }

    #[test]
    fn script_operands_require_both_arrays_and_typed_args() {
        let missing_args = body_exchange(Body::Json(serde_json::json!({"keys": []})));
        let err = script_operands(&missing_args).expect_err("a missing args field must fail");
        assert!(
            err.to_string().contains("'args' is required"),
            "the error must name the missing field, got: {err}"
        );

        let mistyped_arg = body_exchange(Body::Json(serde_json::json!({
            "keys": [],
            "args": [true],
        })));
        let err = script_operands(&mistyped_arg).expect_err("a boolean arg must fail");
        assert!(
            err.to_string()
                .contains("'args[0]' must be a string or a number"),
            "the error must state the args element rule, got: {err}"
        );
    }
}

#[cfg(test)]
mod script_reply_tests {
    use super::*;

    #[test]
    fn redis_value_to_body_maps_nested_replies() {
        let nested = redis::Value::Array(vec![
            redis::Value::Int(1),
            redis::Value::BulkString("ütf8".as_bytes().to_vec()),
            redis::Value::BulkString(vec![0xFFu8]),
            redis::Value::Nil,
            redis::Value::Okay,
        ]);

        assert_eq!(
            redis_value_to_body(&nested).expect("nested array replies must map"),
            Body::Json(serde_json::json!([1, "ütf8", {"bytes": "/w=="}, null, "OK"])),
            "nested binary bulk strings use the explicit {{\"bytes\": base64}} object"
        );

        let standalone = redis::Value::BulkString(vec![0xFFu8]);
        assert_eq!(
            redis_value_to_body(&standalone).expect("a binary bulk string maps to bytes"),
            Body::Bytes(vec![0xFFu8].into()),
            "top-level non-UTF-8 bulk strings keep the byte-body contract"
        );
    }

    #[test]
    fn redis_value_to_body_maps_map_bignumber_and_attribute() {
        let map = redis::Value::Map(vec![
            (
                redis::Value::Int(1),
                redis::Value::BulkString(b"one".to_vec()),
            ),
            (
                redis::Value::BulkString(b"k".to_vec()),
                redis::Value::Int(2),
            ),
        ]);
        assert_eq!(
            redis_value_to_body(&map).expect("map replies must map"),
            Body::Json(serde_json::json!({"1": "one", "k": 2})),
            "map keys render to strings; values use the element mapping"
        );

        let big = redis::Value::BigNumber(
            "12345678901234567890"
                .parse()
                .expect("the decimal literal parses into the driver's number type"),
        );
        assert_eq!(
            redis_value_to_body(&big).expect("bignumber replies must map"),
            Body::Json(serde_json::json!("12345678901234567890")),
            "a bignumber maps to its decimal digits as a JSON string"
        );

        let attribute = redis::Value::Attribute {
            data: Box::new(redis::Value::Int(5)),
            attributes: vec![(
                redis::Value::BulkString(b"ttl".to_vec()),
                redis::Value::Int(1),
            )],
        };
        assert_eq!(
            redis_value_to_body(&attribute).expect("attribute replies must map"),
            Body::Json(serde_json::json!(5)),
            "RESP3 attributes are dropped and the underlying data value is mapped"
        );
    }

    #[test]
    fn redis_value_to_body_server_error_is_err() {
        let top = redis::parse_redis_value(
            b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n",
        )
        .expect("valid RESP error reply");
        let err = redis_value_to_body(&top).expect_err("a server error has no body form");
        assert!(
            err.to_string().contains("WRONGTYPE"),
            "the error must name the server error text, got: {err}"
        );

        let nested_error =
            redis::parse_redis_value(b"-ERR nested boom\r\n").expect("valid RESP error reply");
        let nested = redis::Value::Array(vec![redis::Value::Int(1), nested_error]);
        let err = redis_value_to_body(&nested).expect_err("a nested server error has no body form");
        let msg = err.to_string();
        assert!(
            msg.contains("ERR") && msg.contains("nested boom"),
            "the error must name the nested server error text, got: {msg}"
        );
    }
}
