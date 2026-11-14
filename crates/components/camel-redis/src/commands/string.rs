use super::list;
use super::{
    body_value_bytes, encode_map_entries, get_i64_header, get_str_vec_header, get_u64_header,
    invalid_header_value_preview, map_body_map, reply_to_body, require_bool_header, require_key,
    require_positive,
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
            | RedisCommand::Getdel
            | RedisCommand::Getex
            | RedisCommand::Setrange
            | RedisCommand::Getrange
    )
}

/// Resolves the `SETRANGE` byte offset from `CamelRedis.Offset`.
///
/// `SETRANGE` has no safe default: writing at offset `0` overwrites the
/// string from the start, so a missing header is a named error instead of a
/// silent guess. Offset `0` is legal; a negative offset (the numeric-string
/// `"-1"` form) is rejected by [`get_u64_header`] with the non-negative
/// requirement named.
pub(crate) fn resolve_range_offset(exchange: &Exchange) -> Result<u64, CamelError> {
    const HEADER: &str = "CamelRedis.Offset";
    get_u64_header(exchange, HEADER)?.ok_or_else(|| {
        CamelError::ProcessorError(format!(
            "Missing {HEADER}: SETRANGE requires an explicit non-negative byte offset (0 overwrites from the start of the string)"
        ))
    })
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

// ── SET option surface (design.md D1) ───────────────────────────────────────

const HEADER_SET_CONDITION: &str = "CamelRedis.SetCondition";
const HEADER_KEEP_TTL: &str = "CamelRedis.KeepTtl";
const HEADER_RETURN_OLD_VALUE: &str = "CamelRedis.ReturnOldValue";
const HEADER_EXPIRE_UNIT: &str = "CamelRedis.ExpireUnit";
const HEADER_TIMEOUT: &str = "CamelRedis.Timeout";
const HEADER_TIMESTAMP: &str = "CamelRedis.Timestamp";

/// Unit of the `SET`/`GETEX` expiry value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpireUnit {
    Seconds,
    Milliseconds,
}

/// Reads `CamelRedis.ExpireUnit` case-insensitively, defaulting to seconds.
///
/// The default matches the existing `SETEX`/`EXPIREAT` intuition: a bare
/// `CamelRedis.Timeout`/`CamelRedis.Timestamp` means seconds unless the route
/// asks for milliseconds.
fn resolve_expire_unit(exchange: &Exchange) -> Result<ExpireUnit, CamelError> {
    let Some(value) = exchange.input.header(HEADER_EXPIRE_UNIT) else {
        return Ok(ExpireUnit::Seconds);
    };
    match value.as_str().map(str::trim) {
        Some(raw) if raw.eq_ignore_ascii_case("SECONDS") => Ok(ExpireUnit::Seconds),
        Some(raw) if raw.eq_ignore_ascii_case("MILLISECONDS") => Ok(ExpireUnit::Milliseconds),
        _ => Err(CamelError::ProcessorError(format!(
            "Invalid value for header {HEADER_EXPIRE_UNIT}: expected SECONDS or MILLISECONDS, got {}",
            invalid_header_value_preview(value)
        ))),
    }
}

/// `SET` write condition. A single-valued header keeps `NX` + `XX`
/// structurally unrepresentable instead of a runtime conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetCondition {
    Nx,
    Xx,
}

/// Resolved `SET` expiry argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetExpiry {
    Ex(u64),
    Px(u64),
    Exat(i64),
    Pxat(i64),
    KeepTtl,
}

/// Resolved `SET` option arguments, in wire order.
#[derive(Debug, Default)]
struct SetOptions {
    condition: Option<SetCondition>,
    expiry: Option<SetExpiry>,
    return_old_value: bool,
}

/// Reads `CamelRedis.SetCondition` case-insensitively.
fn resolve_set_condition(exchange: &Exchange) -> Result<Option<SetCondition>, CamelError> {
    let Some(value) = exchange.input.header(HEADER_SET_CONDITION) else {
        return Ok(None);
    };
    match value.as_str().map(str::trim) {
        Some(raw) if raw.eq_ignore_ascii_case("NX") => Ok(Some(SetCondition::Nx)),
        Some(raw) if raw.eq_ignore_ascii_case("XX") => Ok(Some(SetCondition::Xx)),
        _ => Err(CamelError::ProcessorError(format!(
            "Invalid value for header {HEADER_SET_CONDITION}: expected NX or XX, got {}",
            invalid_header_value_preview(value)
        ))),
    }
}

/// Resolves the `SET` option headers, failing closed on any conflict.
///
/// Each option header is independent; the combinations Redis cannot express
/// are refused locally, before any command byte is written:
/// `Timeout` + `Timestamp` (relative vs absolute expiry), `KeepTtl` with
/// either expiry header, and `ExpireUnit` with no expiry header at all.
fn resolve_set_options(exchange: &Exchange) -> Result<SetOptions, CamelError> {
    let condition = resolve_set_condition(exchange)?;
    let keep_ttl = require_bool_header(exchange, HEADER_KEEP_TTL)?.unwrap_or(false);
    let return_old_value = require_bool_header(exchange, HEADER_RETURN_OLD_VALUE)?.unwrap_or(false);
    let timeout = get_u64_header(exchange, HEADER_TIMEOUT)?;
    let timestamp = get_i64_header(exchange, HEADER_TIMESTAMP)?;
    let unit = resolve_expire_unit(exchange)?;

    if timeout.is_some() && timestamp.is_some() {
        return Err(CamelError::ProcessorError(format!(
            "{HEADER_TIMEOUT} and {HEADER_TIMESTAMP} are mutually exclusive for SET: choose a relative expiry (Timeout) or an absolute one (Timestamp)"
        )));
    }

    let expiry_header = if timeout.is_some() {
        Some(HEADER_TIMEOUT)
    } else if timestamp.is_some() {
        Some(HEADER_TIMESTAMP)
    } else {
        None
    };

    if let (true, Some(header)) = (keep_ttl, expiry_header) {
        return Err(CamelError::ProcessorError(format!(
            "{HEADER_KEEP_TTL} and {header} are mutually exclusive for SET: KEEPTTL keeps the current expiry, so no expiry header may be set"
        )));
    }

    if exchange.input.header(HEADER_EXPIRE_UNIT).is_some() && expiry_header.is_none() {
        return Err(CamelError::ProcessorError(format!(
            "{HEADER_EXPIRE_UNIT} is set but no expiry header is: {HEADER_TIMEOUT} or {HEADER_TIMESTAMP} is required for SET, unless the unit header is removed"
        )));
    }

    let expiry = if let Some(seconds) = timeout {
        let seconds = require_positive(
            Some(seconds),
            HEADER_TIMEOUT,
            "a positive expiry in the selected unit is required for SET; Redis rejects EX 0/PX 0, so set a positive value or use DEL to delete a key",
        )?;
        Some(match unit {
            ExpireUnit::Seconds => SetExpiry::Ex(seconds),
            ExpireUnit::Milliseconds => SetExpiry::Px(seconds),
        })
    } else if let Some(timestamp) = timestamp {
        Some(match unit {
            ExpireUnit::Seconds => SetExpiry::Exat(timestamp),
            ExpireUnit::Milliseconds => SetExpiry::Pxat(timestamp),
        })
    } else {
        keep_ttl.then_some(SetExpiry::KeepTtl)
    };

    Ok(SetOptions {
        condition,
        expiry,
        return_old_value,
    })
}

// ── GETEX option surface (design.md D1) ─────────────────────────────────────

const HEADER_PERSIST: &str = "CamelRedis.Persist";

/// Resolved `GETEX` expiry argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GetexExpiry {
    Ex(u64),
    Px(u64),
    Exat(i64),
    Pxat(i64),
}

/// Resolved `GETEX` option arguments, in wire order.
#[derive(Debug, Default)]
struct GetexOptions {
    expiry: Option<GetexExpiry>,
    persist: bool,
}

/// Resolves the `GETEX` option headers, failing closed on any conflict.
///
/// The rules mirror `SET`: `Timeout` and `Timestamp` are mutually exclusive,
/// and a unit header with neither expiry header is a misconfigured route.
/// `PERSIST` is the opposite of an expiry, so it cannot be combined with one.
fn resolve_getex_options(exchange: &Exchange) -> Result<GetexOptions, CamelError> {
    let persist = require_bool_header(exchange, HEADER_PERSIST)?.unwrap_or(false);
    let timeout = get_u64_header(exchange, HEADER_TIMEOUT)?;
    let timestamp = get_i64_header(exchange, HEADER_TIMESTAMP)?;
    let unit = resolve_expire_unit(exchange)?;

    if timeout.is_some() && timestamp.is_some() {
        return Err(CamelError::ProcessorError(format!(
            "{HEADER_TIMEOUT} and {HEADER_TIMESTAMP} are mutually exclusive for GETEX: choose a relative expiry (Timeout) or an absolute one (Timestamp)"
        )));
    }

    let expiry_header = if timeout.is_some() {
        Some(HEADER_TIMEOUT)
    } else if timestamp.is_some() {
        Some(HEADER_TIMESTAMP)
    } else {
        None
    };

    if let (true, Some(header)) = (persist, expiry_header) {
        return Err(CamelError::ProcessorError(format!(
            "{HEADER_PERSIST} and {header} are mutually exclusive for GETEX: PERSIST removes the expiry, so no expiry header may be set"
        )));
    }

    if exchange.input.header(HEADER_EXPIRE_UNIT).is_some() && expiry_header.is_none() {
        return Err(CamelError::ProcessorError(format!(
            "{HEADER_EXPIRE_UNIT} is set but no expiry header is: {HEADER_TIMEOUT} or {HEADER_TIMESTAMP} is required for GETEX, unless the unit header is removed"
        )));
    }

    let expiry = if let Some(seconds) = timeout {
        let seconds = require_positive(
            Some(seconds),
            HEADER_TIMEOUT,
            "a positive expiry in the selected unit is required for GETEX; Redis rejects EX 0/PX 0",
        )?;
        Some(match unit {
            ExpireUnit::Seconds => GetexExpiry::Ex(seconds),
            ExpireUnit::Milliseconds => GetexExpiry::Px(seconds),
        })
    } else {
        timestamp.map(|timestamp| match unit {
            ExpireUnit::Seconds => GetexExpiry::Exat(timestamp),
            ExpireUnit::Milliseconds => GetexExpiry::Pxat(timestamp),
        })
    };

    Ok(GetexOptions { expiry, persist })
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
            let options = resolve_set_options(exchange)?;

            // Canonical wire order: SET key value [NX|XX]
            // [EX|PX|EXAT|PXAT|KEEPTTL] [GET].
            let mut command = redis::cmd("SET");
            command.arg(&key).arg(&value);
            match options.condition {
                Some(SetCondition::Nx) => {
                    command.arg("NX");
                }
                Some(SetCondition::Xx) => {
                    command.arg("XX");
                }
                None => {}
            }
            match options.expiry {
                Some(SetExpiry::Ex(seconds)) => {
                    command.arg("EX").arg(seconds);
                }
                Some(SetExpiry::Px(millis)) => {
                    command.arg("PX").arg(millis);
                }
                Some(SetExpiry::Exat(timestamp)) => {
                    command.arg("EXAT").arg(timestamp);
                }
                Some(SetExpiry::Pxat(timestamp)) => {
                    command.arg("PXAT").arg(timestamp);
                }
                Some(SetExpiry::KeepTtl) => {
                    command.arg("KEEPTTL");
                }
                None => {}
            }

            if options.return_old_value {
                command.arg("GET");
                let old: Option<Vec<u8>> = command
                    .query_async(conn)
                    .await
                    .map_err(|e| crate::transport_error::redis_error_to_camel("SET", e))?;
                // Binary-safe reply: the body itself carries the payload, so
                // this arm bypasses the `Body::Json` dispatch tail below.
                exchange.input.body = reply_to_body(old.as_deref());
                return Ok(());
            }

            command
                .query_async::<()>(conn)
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
        RedisCommand::Getdel => {
            let key = require_key(exchange)?;
            let val: Option<Vec<u8>> = redis::cmd("GETDEL")
                .arg(&key)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("GETDEL", e))?;
            // Binary-safe reply: the body itself carries the payload, so this
            // arm bypasses the `Body::Json` dispatch tail below.
            exchange.input.body = reply_to_body(val.as_deref());
            return Ok(());
        }
        RedisCommand::Getex => {
            let key = require_key(exchange)?;
            let options = resolve_getex_options(exchange)?;

            // Canonical wire order: GETEX key [EX|PX|EXAT|PXAT n | PERSIST].
            let mut command = redis::cmd("GETEX");
            command.arg(&key);
            match options.expiry {
                Some(GetexExpiry::Ex(seconds)) => {
                    command.arg("EX").arg(seconds);
                }
                Some(GetexExpiry::Px(millis)) => {
                    command.arg("PX").arg(millis);
                }
                Some(GetexExpiry::Exat(timestamp)) => {
                    command.arg("EXAT").arg(timestamp);
                }
                Some(GetexExpiry::Pxat(timestamp)) => {
                    command.arg("PXAT").arg(timestamp);
                }
                None => {}
            }
            if options.persist && options.expiry.is_none() {
                command.arg("PERSIST");
            }

            let val: Option<Vec<u8>> = command
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("GETEX", e))?;
            // Binary-safe reply: the body itself carries the payload, so this
            // arm bypasses the `Body::Json` dispatch tail below.
            exchange.input.body = reply_to_body(val.as_deref());
            return Ok(());
        }
        RedisCommand::Setrange => {
            let key = require_key(exchange)?;
            let offset = resolve_range_offset(exchange)?;
            let value = body_value_bytes(exchange, "SETRANGE", &[HEADER_VALUE])?;
            let n: i64 = redis::cmd("SETRANGE")
                .arg(&key)
                .arg(offset)
                .arg(&value)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SETRANGE", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Getrange => {
            let key = require_key(exchange)?;
            let (start, end) = list::resolve_range_bounds(exchange)?;
            let slice: Option<Vec<u8>> = redis::cmd("GETRANGE")
                .arg(&key)
                .arg(start)
                .arg(end)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("GETRANGE", e))?;
            // Binary-safe reply: the slice bytes are the payload, so this arm
            // bypasses the `Body::Json` dispatch tail below.
            exchange.input.body = reply_to_body(slice.as_deref());
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

    // ── SET option surface (rediscmds 1.2) ─────────────────────────────────

    fn resolved_expiry(headers: &[(&str, serde_json::Value)]) -> SetExpiry {
        resolve_set_options(&ex_with(headers))
            .expect("the option headers must resolve")
            .expiry
            .expect("the case sets an expiry header")
    }

    #[test]
    fn resolve_set_options_defaults_and_expiry() {
        // Relative expiry in the default unit (SECONDS).
        assert_eq!(
            resolved_expiry(&[("CamelRedis.Timeout", serde_json::json!(15u64))]),
            SetExpiry::Ex(15)
        );
        // The unit discriminator switches the relative expiry to PX.
        assert_eq!(
            resolved_expiry(&[
                ("CamelRedis.Timeout", serde_json::json!(500u64)),
                ("CamelRedis.ExpireUnit", serde_json::json!("MILLISECONDS")),
            ]),
            SetExpiry::Px(500)
        );
        // Absolute expiry in the default unit (SECONDS).
        assert_eq!(
            resolved_expiry(&[("CamelRedis.Timestamp", serde_json::json!(1731000000i64))]),
            SetExpiry::Exat(1731000000)
        );
        // The unit discriminator switches the absolute expiry to PXAT.
        assert_eq!(
            resolved_expiry(&[
                ("CamelRedis.Timestamp", serde_json::json!(1731000000000i64)),
                ("CamelRedis.ExpireUnit", serde_json::json!("milliseconds")),
            ]),
            SetExpiry::Pxat(1731000000000)
        );
        // KEEPTTL needs no expiry value.
        assert_eq!(
            resolved_expiry(&[("CamelRedis.KeepTtl", serde_json::json!(true))]),
            SetExpiry::KeepTtl
        );

        // Absent option headers mean: no condition, no expiry, no GET.
        let options =
            resolve_set_options(&ex_with(&[])).expect("an option-free exchange must resolve");
        assert_eq!(
            options.condition, None,
            "no condition header means no NX/XX"
        );
        assert_eq!(options.expiry, None, "no expiry header means no expiry arg");
        assert!(!options.return_old_value, "GET is opt-in");
    }

    #[test]
    fn resolve_set_options_conflicts_fail_closed() {
        struct Case {
            headers: Vec<(&'static str, serde_json::Value)>,
            expected_fragments: Vec<&'static str>,
        }

        let cases = [
            // KEEPTTL keeps the current expiry, so it cannot be combined with one.
            Case {
                headers: vec![
                    ("CamelRedis.KeepTtl", serde_json::json!(true)),
                    ("CamelRedis.Timeout", serde_json::json!(15u64)),
                ],
                expected_fragments: vec!["CamelRedis.KeepTtl", "CamelRedis.Timeout"],
            },
            // Relative and absolute expiry are mutually exclusive.
            Case {
                headers: vec![
                    ("CamelRedis.Timeout", serde_json::json!(15u64)),
                    ("CamelRedis.Timestamp", serde_json::json!(1731000000i64)),
                ],
                expected_fragments: vec!["CamelRedis.Timeout", "CamelRedis.Timestamp"],
            },
            // Invalid condition names the header and both valid values.
            Case {
                headers: vec![("CamelRedis.SetCondition", serde_json::json!("MAYBE"))],
                expected_fragments: vec!["CamelRedis.SetCondition", "NX", "XX"],
            },
            // Invalid unit names the header and both valid values.
            Case {
                headers: vec![
                    ("CamelRedis.Timeout", serde_json::json!(5u64)),
                    ("CamelRedis.ExpireUnit", serde_json::json!("HOURS")),
                ],
                expected_fragments: vec!["CamelRedis.ExpireUnit", "SECONDS", "MILLISECONDS"],
            },
            // A unit with no expiry header is a misconfigured route.
            Case {
                headers: vec![("CamelRedis.ExpireUnit", serde_json::json!("MILLISECONDS"))],
                expected_fragments: vec!["CamelRedis.ExpireUnit", "CamelRedis.Timeout"],
            },
        ];

        for (index, case) in cases.iter().enumerate() {
            let err = resolve_set_options(&ex_with(&case.headers))
                .expect_err("a conflicting option set must fail before send");
            let msg = err.to_string();
            for fragment in &case.expected_fragments {
                assert!(
                    msg.contains(fragment),
                    "case {index} error must name {fragment}: {msg}"
                );
            }
        }
    }

    // ── GETEX option surface (rediscmds 1.3) ───────────────────────────────

    fn resolved_getex_expiry(headers: &[(&str, serde_json::Value)]) -> GetexExpiry {
        resolve_getex_options(&ex_with(headers))
            .expect("the option headers must resolve")
            .expiry
            .expect("the case sets an expiry header")
    }

    #[test]
    fn resolve_getex_options_mappings_and_conflicts() {
        // Absent option headers mean: no expiry change, no PERSIST.
        let options =
            resolve_getex_options(&ex_with(&[])).expect("an option-free exchange must resolve");
        assert_eq!(options.expiry, None, "no expiry header means no expiry arg");
        assert!(!options.persist, "PERSIST is opt-in");

        // Relative expiry in the default unit (SECONDS).
        assert_eq!(
            resolved_getex_expiry(&[("CamelRedis.Timeout", serde_json::json!(100u64))]),
            GetexExpiry::Ex(100)
        );
        // The unit discriminator switches the relative expiry to PX.
        assert_eq!(
            resolved_getex_expiry(&[
                ("CamelRedis.Timeout", serde_json::json!(500u64)),
                ("CamelRedis.ExpireUnit", serde_json::json!("MILLISECONDS")),
            ]),
            GetexExpiry::Px(500)
        );
        // Absolute expiry in the default unit (SECONDS).
        assert_eq!(
            resolved_getex_expiry(&[("CamelRedis.Timestamp", serde_json::json!(1731000000i64))]),
            GetexExpiry::Exat(1731000000)
        );
        // The unit discriminator switches the absolute expiry to PXAT.
        assert_eq!(
            resolved_getex_expiry(&[
                ("CamelRedis.Timestamp", serde_json::json!(1731000000000i64)),
                ("CamelRedis.ExpireUnit", serde_json::json!("milliseconds")),
            ]),
            GetexExpiry::Pxat(1731000000000)
        );

        // PERSIST needs no expiry value and stands alone.
        let options =
            resolve_getex_options(&ex_with(&[("CamelRedis.Persist", serde_json::json!(true))]))
                .expect("Persist=true alone must resolve");
        assert_eq!(options.expiry, None, "PERSIST carries no expiry value");
        assert!(options.persist);

        struct Case {
            headers: Vec<(&'static str, serde_json::Value)>,
            expected_fragments: Vec<&'static str>,
        }

        let cases = [
            // A unit with no expiry header is a misconfigured route.
            Case {
                headers: vec![("CamelRedis.ExpireUnit", serde_json::json!("MILLISECONDS"))],
                expected_fragments: vec!["CamelRedis.ExpireUnit", "CamelRedis.Timeout"],
            },
            // The same rule holds when PERSIST is also requested.
            Case {
                headers: vec![
                    ("CamelRedis.ExpireUnit", serde_json::json!("MILLISECONDS")),
                    ("CamelRedis.Persist", serde_json::json!(true)),
                ],
                expected_fragments: vec!["CamelRedis.ExpireUnit", "CamelRedis.Timeout"],
            },
            // PERSIST removes the expiry, so it cannot be combined with one.
            Case {
                headers: vec![
                    ("CamelRedis.Persist", serde_json::json!(true)),
                    ("CamelRedis.Timeout", serde_json::json!(10u64)),
                ],
                expected_fragments: vec!["CamelRedis.Persist", "CamelRedis.Timeout"],
            },
            // Relative and absolute expiry are mutually exclusive.
            Case {
                headers: vec![
                    ("CamelRedis.Timeout", serde_json::json!(10u64)),
                    ("CamelRedis.Timestamp", serde_json::json!(100i64)),
                ],
                expected_fragments: vec!["CamelRedis.Timeout", "CamelRedis.Timestamp"],
            },
            // A malformed boolean names the header and echoes the value.
            Case {
                headers: vec![("CamelRedis.Persist", serde_json::json!("always"))],
                expected_fragments: vec!["CamelRedis.Persist", "always"],
            },
        ];

        for (index, case) in cases.iter().enumerate() {
            let err = resolve_getex_options(&ex_with(&case.headers))
                .expect_err("a conflicting option set must fail before send");
            let msg = err.to_string();
            for fragment in &case.expected_fragments {
                assert!(
                    msg.contains(fragment),
                    "case {index} error must name {fragment}: {msg}"
                );
            }
        }
    }

    #[test]
    fn string_command_includes_getdel_getex() {
        assert!(
            is_string_command(&RedisCommand::Getdel),
            "GETDEL is a string command"
        );
        assert!(
            is_string_command(&RedisCommand::Getex),
            "GETEX is a string command"
        );
        assert!(
            !is_string_command(&RedisCommand::Zadd),
            "ZADD stays sorted-set-only"
        );
    }

    #[test]
    fn resolve_set_options_malformed_booleans_and_zero_expiry() {
        let err = resolve_set_options(&ex_with(&[(
            "CamelRedis.KeepTtl",
            serde_json::json!("yes-please"),
        )]))
        .expect_err("a malformed KeepTtl must fail closed");
        let msg = err.to_string();
        assert!(msg.contains("CamelRedis.KeepTtl"), "{msg}");
        assert!(msg.contains("boolean"), "{msg}");
        assert!(
            msg.contains("yes-please"),
            "error must echo the received value: {msg}"
        );

        let err = resolve_set_options(&ex_with(&[(
            "CamelRedis.ReturnOldValue",
            serde_json::json!("maybe"),
        )]))
        .expect_err("a malformed ReturnOldValue must fail closed");
        let msg = err.to_string();
        assert!(msg.contains("CamelRedis.ReturnOldValue"), "{msg}");
        assert!(msg.contains("boolean"), "{msg}");
        assert!(
            msg.contains("maybe"),
            "error must echo the received value: {msg}"
        );

        // Redis rejects EX 0/PX 0, so a zero relative expiry is a named error.
        let err = resolve_set_options(&ex_with(&[("CamelRedis.Timeout", serde_json::json!(0u64))]))
            .expect_err("a zero expiry must fail closed");
        let msg = err.to_string();
        assert!(msg.contains("CamelRedis.Timeout"), "{msg}");
        assert!(
            msg.contains("positive"),
            "error must state the positive requirement: {msg}"
        );
    }

    // ── SETRANGE / GETRANGE (rediscmds 1.4) ────────────────────────────────

    #[test]
    fn resolve_range_offset_domains() {
        let err = resolve_range_offset(&ex_with(&[]))
            .expect_err("SETRANGE without CamelRedis.Offset must fail before send");
        assert!(
            err.to_string().contains("CamelRedis.Offset"),
            "missing-offset error must name the header: {err}"
        );

        for offset in [0u64, 6] {
            let ex = ex_with(&[("CamelRedis.Offset", serde_json::json!(offset))]);
            assert_eq!(
                resolve_range_offset(&ex).expect("offset 0 and 6 are legal"),
                offset
            );
        }

        let ex = ex_with(&[("CamelRedis.Offset", serde_json::json!("-1"))]);
        let err = resolve_range_offset(&ex).expect_err("a negative offset must fail before send");
        let msg = err.to_string();
        assert!(msg.contains("CamelRedis.Offset"), "{msg}");
        assert!(
            msg.contains("non-negative"),
            "error must state the non-negative requirement: {msg}"
        );
    }

    #[test]
    fn string_command_includes_ranges() {
        assert!(
            is_string_command(&RedisCommand::Setrange),
            "SETRANGE is a string command"
        );
        assert!(
            is_string_command(&RedisCommand::Getrange),
            "GETRANGE is a string command"
        );
    }
}
