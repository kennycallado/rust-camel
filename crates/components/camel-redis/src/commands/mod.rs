pub mod geo;
pub mod hash;
pub mod key;
pub mod list;
pub mod other;
pub mod pubsub;
pub mod set;
pub mod stream;
pub mod string;
pub mod zset;

use crate::HEADER_VALUE;
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
fn invalid_header_value_preview(value: &serde_json::Value) -> String {
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

pub fn get_str_vec_header(exchange: &Exchange, key: &str) -> Option<Vec<String>> {
    exchange.input.header(key).and_then(|v| {
        v.as_array().map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
    })
}

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

pub fn require_value(exchange: &Exchange) -> Result<serde_json::Value, CamelError> {
    get_value_header(exchange, HEADER_VALUE).ok_or_else(|| {
        CamelError::ProcessorError(format!("Missing required header: {HEADER_VALUE}"))
    })
}

pub(crate) fn value_to_redis_arg(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
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

/// Encodes the exchange body into the raw bytes sent to Redis as a value.
///
/// Codec table: `Text`/`Xml` contribute their UTF-8 bytes, `Json` its compact
/// serialization, and `Bytes` pass through verbatim (binary-safe). `Empty`,
/// `Stream`, and any future variant are rejected before any command reaches
/// Redis, with an error naming `verb` and the rejected variant.
pub(crate) fn encode_body_value(body: &Body, verb: &str) -> Result<Vec<u8>, CamelError> {
    match body {
        Body::Text(s) => Ok(s.as_bytes().to_vec()),
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

/// Resolves the value payload bytes for `verb` from the exchange.
///
/// Precedence: the `value_header` wins when present and is encoded exactly as
/// before ([`value_to_redis_arg`] byte-identically: JSON strings stay raw
/// characters, everything else compact JSON); otherwise the exchange body is
/// used through [`encode_body_value`]. The body-path error appends a hint to
/// set the header, which body-only callers (for example XADD) never see.
pub(crate) fn resolve_value_bytes(
    exchange: &Exchange,
    verb: &str,
    value_header: &str,
) -> Result<Vec<u8>, CamelError> {
    match get_value_header(exchange, value_header) {
        Some(value) => Ok(value_to_redis_arg(&value).into_bytes()),
        None => encode_body_value(&exchange.input.body, verb).map_err(|e| match e {
            CamelError::ProcessorError(msg) => {
                CamelError::ProcessorError(format!("{msg}; or set {value_header}"))
            }
            other => other,
        }),
    }
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

    #[test]
    fn test_value_to_redis_arg_preserves_string_content() {
        assert_eq!(value_to_redis_arg(&serde_json::json!("hello")), "hello");
    }

    #[test]
    fn test_value_to_redis_arg_serializes_non_strings() {
        assert_eq!(
            value_to_redis_arg(&serde_json::json!({"a": 1})),
            r#"{"a":1}"#
        );
        assert_eq!(value_to_redis_arg(&serde_json::json!(42)), "42");
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
            headers.push((crate::HEADER_VALUE, serde_json::json!("v")));
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
                let mut msg = Message::default();
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
    fn resolve_value_bytes_header_string_is_raw_chars() {
        let ex = ex_with(
            Body::Text("body".into()),
            &[(HEADER_VALUE, serde_json::json!("hello"))],
        );
        assert_eq!(
            resolve_value_bytes(&ex, "SET", HEADER_VALUE).expect("header must resolve"),
            b"hello".to_vec()
        );
    }

    #[test]
    fn resolve_value_bytes_header_number_is_compact_json() {
        let ex = ex_with(Body::Empty, &[(HEADER_VALUE, serde_json::json!(42))]);
        assert_eq!(
            resolve_value_bytes(&ex, "SET", HEADER_VALUE).expect("numeric header must resolve"),
            b"42".to_vec()
        );
    }

    #[test]
    fn resolve_value_bytes_body_when_header_absent() {
        let ex = ex_with(Body::Text("hello".into()), &[]);
        assert_eq!(
            resolve_value_bytes(&ex, "SET", HEADER_VALUE).expect("body must resolve"),
            b"hello".to_vec()
        );
    }

    #[test]
    fn resolve_value_bytes_error_hints_the_header() {
        let ex = ex_with(Body::Empty, &[]);
        let msg = processor_message(
            resolve_value_bytes(&ex, "SET", HEADER_VALUE).expect_err("empty body must be rejected"),
        );
        assert!(msg.contains("SET"), "error must name the verb: {msg}");
        assert!(
            msg.contains("Body::Empty"),
            "error must name the rejected variant: {msg}"
        );
        assert!(
            msg.contains(HEADER_VALUE),
            "error must hint the value header: {msg}"
        );
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
