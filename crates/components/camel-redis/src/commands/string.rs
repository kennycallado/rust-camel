use super::{
    body_value_bytes, encode_map_entries, get_i64_header, get_str_vec_header, get_u64_header,
    map_body_map, reply_to_body, require_key,
};
use crate::HEADER_VALUE;
use crate::config::RedisCommand;
use camel_component_api::{Body, CamelError, Exchange};
use redis::AsyncCommands;
use redis::aio::MultiplexedConnection;

pub(crate) fn is_string_command(cmd: &RedisCommand) -> bool {
    matches!(
        cmd,
        RedisCommand::Set
            | RedisCommand::Get
            | RedisCommand::Getset
            | RedisCommand::Setnx
            | RedisCommand::Setex
            | RedisCommand::Mget
            | RedisCommand::Mset
            | RedisCommand::Incr
            | RedisCommand::Incrby
            | RedisCommand::Decr
            | RedisCommand::Decrby
            | RedisCommand::Append
            | RedisCommand::Strlen
    )
}

/// Resolves the `SETEX` TTL in seconds.
///
/// `CamelRedis.Timeout` is REQUIRED and must be positive: Redis rejects
/// `SETEX` with a zero TTL, so defaulting a missing header to `0` produced
/// a confusing server-side error instead of a local, actionable one. Use
/// `DEL` to delete a key. (The `SETEX` TTL is a `u64` end to end, so no
/// `u64 -> i64` narrowing happens here.)
pub(crate) fn resolve_timeout_seconds(exchange: &Exchange) -> Result<u64, CamelError> {
    const HEADER: &str = "CamelRedis.Timeout";
    super::require_positive(
        get_u64_header(exchange, HEADER)?,
        HEADER,
        "a positive TTL in seconds is required for SETEX; set a positive value or use DEL to delete a key",
    )
}

pub(crate) fn resolve_increment(exchange: &Exchange) -> Result<i64, CamelError> {
    Ok(get_i64_header(exchange, "CamelRedis.Increment")?.unwrap_or(1))
}

pub(crate) fn resolve_mget_keys(exchange: &Exchange) -> Result<Vec<String>, CamelError> {
    get_str_vec_header(exchange, "CamelRedis.Keys")
        .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Keys".into()))
}

/// Resolves the `MSET` key/value pairs from the exchange body.
///
/// The body must be a JSON object (or `Text`/UTF-8 `Bytes` that parses as
/// one) via [`map_body_map`], and each entry is encoded by the scalar rule of
/// [`encode_map_entries`]. `CamelRedis.Values` is a removed payload header,
/// so a present one fails with the named tombstone error before the body is
/// read.
pub(crate) fn resolve_mset_values(
    exchange: &Exchange,
) -> Result<Vec<(String, Vec<u8>)>, CamelError> {
    if exchange.input.header("CamelRedis.Values").is_some() {
        return Err(CamelError::ProcessorError(
            "CamelRedis.Values was removed in 0.69; put the value in the body".into(),
        ));
    }

    let map = map_body_map(&exchange.input.body, "MSET")?;
    encode_map_entries(&map, "MSET")
}

pub(crate) fn json_from_optional_string(value: Option<String>) -> serde_json::Value {
    value
        .map(serde_json::Value::String)
        .unwrap_or(serde_json::Value::Null)
}

pub(crate) fn json_array_from_optional_strings(values: Vec<Option<String>>) -> serde_json::Value {
    serde_json::Value::Array(values.into_iter().map(json_from_optional_string).collect())
}

/// Encodes an `MGET` reply as a RESP array of bulk strings.
///
/// The shape mirrors the server's own reply (`*N\r\n`, per element
/// `$<len>\r\n<bytes>\r\n`, nil as `$-1\r\n`), so a binary `MGET` reply keeps
/// every element, its order, and its nils intact instead of degrading to a
/// lossy JSON string.
pub(crate) fn encode_mget_reply_bytes(values: &[Option<Vec<u8>>]) -> Vec<u8> {
    let mut out = format!("*{}\r\n", values.len()).into_bytes();
    for value in values {
        match value {
            None => out.extend_from_slice(b"$-1\r\n"),
            Some(bytes) => {
                out.extend_from_slice(format!("${}\r\n", bytes.len()).as_bytes());
                out.extend_from_slice(bytes);
                out.extend_from_slice(b"\r\n");
            }
        }
    }
    out
}

/// Maps an `MGET` reply to an exchange body.
///
/// A reply whose elements are all nils or valid UTF-8 keeps the JSON array
/// shape (`["a", null, "c"]`) unchanged. A reply with any binary element is
/// encoded with [`encode_mget_reply_bytes`] so no element is mangled.
fn mget_reply_to_body(values: Vec<Option<Vec<u8>>>) -> Body {
    let text: Option<Vec<Option<String>>> = values
        .iter()
        .map(|value| {
            value.as_ref().map_or(Some(None), |bytes| {
                String::from_utf8(bytes.clone()).ok().map(Some)
            })
        })
        .collect();
    match text {
        Some(values) => Body::Json(json_array_from_optional_strings(values)),
        None => Body::Bytes(encode_mget_reply_bytes(&values).into()),
    }
}

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    if !is_string_command(cmd) {
        return Err(CamelError::ProcessorError("Not a string command".into()));
    }

    let result: serde_json::Value = match cmd {
        RedisCommand::Set => {
            let key = require_key(exchange)?;
            let value = body_value_bytes(exchange, "SET", &[HEADER_VALUE])?;
            conn.set::<_, _, ()>(&key, &value)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SET", e))?;
            serde_json::Value::Null
        }
        RedisCommand::Get => {
            let key = require_key(exchange)?;
            let val: Option<Vec<u8>> = redis::cmd("GET")
                .arg(&key)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("GET", e))?;
            // Binary-safe reply: the body itself carries the payload, so this
            // arm bypasses the `Body::Json` dispatch tail below.
            exchange.input.body = reply_to_body(val.as_deref());
            return Ok(());
        }
        RedisCommand::Getset => {
            let key = require_key(exchange)?;
            let value = body_value_bytes(exchange, "GETSET", &[HEADER_VALUE])?;
            let old: Option<String> = redis::cmd("GETSET")
                .arg(&key)
                .arg(&value)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("GETSET", e))?;
            json_from_optional_string(old)
        }
        RedisCommand::Setnx => {
            let key = require_key(exchange)?;
            let value = body_value_bytes(exchange, "SETNX", &[HEADER_VALUE])?;
            let ok: bool = conn
                .set_nx(&key, &value)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SETNX", e))?;
            serde_json::Value::Bool(ok)
        }
        RedisCommand::Setex => {
            let key = require_key(exchange)?;
            let value = body_value_bytes(exchange, "SETEX", &[HEADER_VALUE])?;
            let ttl = resolve_timeout_seconds(exchange)?;
            conn.set_ex::<_, _, ()>(&key, &value, ttl)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SETEX", e))?;
            serde_json::Value::Null
        }
        RedisCommand::Mget => {
            let keys = resolve_mget_keys(exchange)?;
            let vals: Vec<Option<Vec<u8>>> = redis::cmd("MGET")
                .arg(&keys)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("MGET", e))?;
            // Text replies keep the JSON array; binary replies carry the
            // encoded RESP array in the body and bypass the dispatch tail.
            exchange.input.body = mget_reply_to_body(vals);
            return Ok(());
        }
        RedisCommand::Mset => {
            let values = resolve_mset_values(exchange)?;
            conn.mset::<_, _, ()>(&values)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("MSET", e))?;
            serde_json::Value::Null
        }
        RedisCommand::Incr => {
            let key = require_key(exchange)?;
            let n: i64 = conn
                .incr(&key, 1i64)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("INCR", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Incrby => {
            let key = require_key(exchange)?;
            let by = resolve_increment(exchange)?;
            let n: i64 = conn
                .incr(&key, by)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("INCRBY", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Decr => {
            let key = require_key(exchange)?;
            let n: i64 = conn
                .decr(&key, 1i64)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("DECR", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Decrby => {
            let key = require_key(exchange)?;
            let by = resolve_increment(exchange)?;
            let n: i64 = conn
                .decr(&key, by)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("DECRBY", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Append => {
            let key = require_key(exchange)?;
            let value = body_value_bytes(exchange, "APPEND", &[HEADER_VALUE])?;
            let n: i64 = conn
                .append(&key, &value)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("APPEND", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Strlen => {
            let key = require_key(exchange)?;
            let n: i64 = conn
                .strlen(&key)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("STRLEN", e))?;
            serde_json::json!(n)
        }
        _ => unreachable!("non-string commands rejected above"),
    };
    exchange.input.body = Body::Json(result);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RedisCommand;
    use camel_component_api::{Exchange, Message};

    fn ex_with(headers: &[(&str, serde_json::Value)]) -> Exchange {
        let mut msg = Message::default();
        for (k, v) in headers {
            msg.set_header(*k, v.clone());
        }
        Exchange::new(msg)
    }

    fn ex_with_body(body: Body) -> Exchange {
        Exchange::new(Message::new(body))
    }

    // Test that dispatch returns Err when key is missing (no real Redis needed)
    #[tokio::test]
    async fn test_set_missing_key_returns_err() {
        // We can't call dispatch without a real connection, but we can test
        // the key header resolution via require_key directly.
        let ex = Exchange::new(Message::default());
        assert!(crate::commands::require_key(&ex).is_err());
    }

    #[test]
    fn test_string_command_classification() {
        assert!(is_string_command(&RedisCommand::Set));
        assert!(is_string_command(&RedisCommand::Append));
        assert!(!is_string_command(&RedisCommand::Sadd));
    }

    /// Task 354: a missing or zero `CamelRedis.Timeout` is a data-loss trap
    /// (Redis rejects `SETEX` with a zero TTL; `EXPIRE 0` deletes the key).
    /// Both cases must fail closed locally with an actionable named error.
    fn assert_ttl_error(err: &CamelError) {
        let msg = err.to_string();
        assert!(msg.contains("CamelRedis.Timeout"), "{msg}");
        assert!(msg.contains("positive"), "{msg}");
        assert!(msg.contains("DEL"), "{msg}");
    }

    #[test]
    fn test_resolve_timeout_seconds_rejects_missing_and_zero() {
        for ex in [
            Exchange::new(Message::default()),
            ex_with(&[("CamelRedis.Timeout", serde_json::json!(0u64))]),
            ex_with(&[("CamelRedis.Timeout", serde_json::json!("0"))]),
        ] {
            let err = resolve_timeout_seconds(&ex).expect_err("missing/zero TTL must fail closed");
            assert_ttl_error(&err);
        }
    }

    #[test]
    fn test_resolve_timeout_seconds_from_header() {
        let ex = ex_with(&[("CamelRedis.Timeout", serde_json::json!(15))]);
        assert_eq!(resolve_timeout_seconds(&ex).unwrap(), 15);
    }

    #[test]
    fn test_resolve_timeout_seconds_from_numeric_string_header() {
        let ex = ex_with(&[("CamelRedis.Timeout", serde_json::json!(" 15 "))]);
        assert_eq!(resolve_timeout_seconds(&ex).unwrap(), 15);
    }

    #[test]
    fn test_resolve_timeout_seconds_garbage_is_named_error() {
        let ex = ex_with(&[("CamelRedis.Timeout", serde_json::json!("abc"))]);
        let err = resolve_timeout_seconds(&ex).expect_err("garbage must fail");
        let msg = err.to_string();
        assert!(msg.contains("CamelRedis.Timeout"), "{msg}");
        assert!(msg.contains("abc"), "{msg}");
    }

    #[test]
    fn test_resolve_mget_keys_requires_header() {
        let ex = Exchange::new(Message::default());
        let err = resolve_mget_keys(&ex).expect_err("keys should be required");
        assert!(err.to_string().contains("CamelRedis.Keys"));
    }

    #[test]
    fn test_resolve_mset_values_extracts_pairs_from_body() {
        let ex = ex_with_body(Body::Json(serde_json::json!({"a": 1, "b": "x"})));
        let values = resolve_mset_values(&ex).expect("a JSON object body must resolve");
        assert_eq!(values.len(), 2);
        assert!(values.iter().any(|(k, v)| k == "a" && v.as_slice() == b"1"));
        assert!(values.iter().any(|(k, v)| k == "b" && v.as_slice() == b"x"));
    }

    #[test]
    fn test_resolve_increment_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_increment(&ex_default).unwrap(), 1);

        let ex = ex_with(&[("CamelRedis.Increment", serde_json::json!(7))]);
        assert_eq!(resolve_increment(&ex).unwrap(), 7);

        let ex_str = ex_with(&[("CamelRedis.Increment", serde_json::json!(" -2 "))]);
        assert_eq!(resolve_increment(&ex_str).unwrap(), -2);
    }

    #[test]
    fn test_resolve_increment_garbage_is_named_error() {
        let ex = ex_with(&[("CamelRedis.Increment", serde_json::json!("nope"))]);
        let err = resolve_increment(&ex).expect_err("garbage must fail");
        let msg = err.to_string();
        assert!(msg.contains("CamelRedis.Increment"), "{msg}");
        assert!(msg.contains("nope"), "{msg}");
    }

    #[test]
    fn test_resolve_mget_keys_returns_values() {
        let ex = ex_with(&[("CamelRedis.Keys", serde_json::json!(["a", "b"]))]);
        assert_eq!(resolve_mget_keys(&ex).unwrap(), vec!["a", "b"]);
    }

    #[test]
    fn test_resolve_mset_values_rejects_null_entry_from_body() {
        let ex = ex_with_body(Body::Json(serde_json::json!({"bad": null})));
        let err = resolve_mset_values(&ex).expect_err("a null map entry must fail before send");
        let msg = err.to_string();
        assert!(msg.contains("MSET"), "error must name the verb: {msg}");
        assert!(
            msg.contains("null"),
            "error must name the null entry: {msg}"
        );
    }

    #[test]
    fn test_json_from_optional_string_variants() {
        assert_eq!(
            json_from_optional_string(Some("x".to_string())),
            serde_json::json!("x")
        );
        assert_eq!(json_from_optional_string(None), serde_json::Value::Null);
    }

    #[test]
    fn test_json_array_from_optional_strings_mixed() {
        let json = json_array_from_optional_strings(vec![Some("a".to_string()), None]);
        assert_eq!(json, serde_json::json!(["a", null]));
    }
}
