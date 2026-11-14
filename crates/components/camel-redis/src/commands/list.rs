use super::{
    body_value_bytes, get_i64_header, get_str_header, get_u64_header, invalid_header_value_preview,
    reply_to_body, require_key, require_key_list, require_str_header,
};
use crate::HEADER_VALUE;
use crate::config::RedisCommand;
use camel_component_api::{Body, CamelError, Exchange};
use redis::AsyncCommands;
use redis::Value;
use redis::aio::MultiplexedConnection;

pub(crate) fn is_list_command(cmd: &RedisCommand) -> bool {
    matches!(
        cmd,
        RedisCommand::Lpush
            | RedisCommand::Rpush
            | RedisCommand::Lpushx
            | RedisCommand::Rpushx
            | RedisCommand::Lpop
            | RedisCommand::Rpop
            | RedisCommand::Blpop
            | RedisCommand::Brpop
            | RedisCommand::Llen
            | RedisCommand::Lrange
            | RedisCommand::Lindex
            | RedisCommand::Linsert
            | RedisCommand::Lset
            | RedisCommand::Lrem
            | RedisCommand::Ltrim
            | RedisCommand::Rpoplpush
            | RedisCommand::Lmpop
            | RedisCommand::Blmpop
            | RedisCommand::Lmove
            | RedisCommand::Blmove
            | RedisCommand::Lpos
    )
}

pub(crate) fn resolve_destination(exchange: &Exchange) -> Result<String, CamelError> {
    get_str_header(exchange, "CamelRedis.Destination")
        .map(|s| s.to_string())
        .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Destination".into()))
}

pub(crate) fn resolve_linsert_mode(exchange: &Exchange) -> &'static str {
    let position = get_str_header(exchange, "CamelRedis.Position").unwrap_or("BEFORE");
    if position.eq_ignore_ascii_case("BEFORE") {
        "BEFORE"
    } else {
        "AFTER"
    }
}

/// Resolves the `BLPOP`/`BRPOP` blocking timeout in seconds.
///
/// A missing or zero `CamelRedis.Timeout` DELIBERATELY means "block
/// forever": `0` is Redis's documented infinite-wait sentinel for
/// `BLPOP`/`BRPOP`. Unlike the EXPIRE family, `0` here does not delete
/// anything, so the default is retained on purpose (Task 354 timeout audit).
pub(crate) fn resolve_blocking_timeout(exchange: &Exchange) -> Result<f64, CamelError> {
    Ok(get_u64_header(exchange, "CamelRedis.Timeout")?.unwrap_or(0) as f64)
}

pub(crate) fn resolve_index(exchange: &Exchange) -> Result<isize, CamelError> {
    Ok(get_i64_header(exchange, "CamelRedis.Index")?.unwrap_or(0) as isize)
}

pub(crate) fn resolve_range_bounds(exchange: &Exchange) -> Result<(isize, isize), CamelError> {
    Ok((
        get_i64_header(exchange, "CamelRedis.Start")?.unwrap_or(0) as isize,
        get_i64_header(exchange, "CamelRedis.End")?.unwrap_or(-1) as isize,
    ))
}

pub(crate) fn resolve_lrem_count(exchange: &Exchange) -> Result<isize, CamelError> {
    Ok(get_i64_header(exchange, "CamelRedis.Count")?.unwrap_or(0) as isize)
}

pub(crate) fn resolve_linsert_operands(
    exchange: &Exchange,
) -> Result<(&'static str, String), CamelError> {
    let mode = resolve_linsert_mode(exchange);
    let pivot = get_str_header(exchange, "CamelRedis.Pivot")
        .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Pivot".into()))?
        .to_string();
    Ok((mode, pivot))
}

/// Resolves a `LEFT`/`RIGHT` direction header case-insensitively.
///
/// Missing and present-but-invalid values are both named errors carrying the
/// header name, so a route typo fails before any command reaches Redis. The
/// rejected value is rendered through the shared untrusted-header preview.
fn resolve_direction_header(exchange: &Exchange, header: &str) -> Result<&'static str, CamelError> {
    let Some(raw) = get_str_header(exchange, header) else {
        return Err(CamelError::ProcessorError(format!(
            "Missing {header}: expected LEFT or RIGHT"
        )));
    };
    if raw.eq_ignore_ascii_case("LEFT") {
        return Ok("LEFT");
    }
    if raw.eq_ignore_ascii_case("RIGHT") {
        return Ok("RIGHT");
    }
    Err(CamelError::ProcessorError(format!(
        "Invalid {header}: expected LEFT or RIGHT, got {}",
        invalid_header_value_preview(&serde_json::Value::String(raw.to_string()))
    )))
}

/// Resolves the `CamelRedis.Where` list end for `LMPOP`/`BLMPOP`.
pub(crate) fn resolve_pop_direction(exchange: &Exchange) -> Result<&'static str, CamelError> {
    resolve_direction_header(exchange, "CamelRedis.Where")
}

/// Resolves the required `CamelRedis.WhereFrom`/`CamelRedis.WhereTo` pair for
/// `LMOVE`/`BLMOVE`.
pub(crate) fn resolve_lmove_directions(
    exchange: &Exchange,
) -> Result<(&'static str, &'static str), CamelError> {
    Ok((
        resolve_direction_header(exchange, "CamelRedis.WhereFrom")?,
        resolve_direction_header(exchange, "CamelRedis.WhereTo")?,
    ))
}

/// Resolves the optional `CamelRedis.Count` for `LMPOP`/`BLMPOP`.
///
/// `COUNT` must be positive on these commands: Redis rejects zero, and a
/// silently defaulted count would change how many elements a pop returns.
pub(crate) fn resolve_pop_count_positive(exchange: &Exchange) -> Result<Option<u64>, CamelError> {
    let Some(raw) = exchange.input.header("CamelRedis.Count") else {
        return Ok(None);
    };
    let invalid = || {
        CamelError::ProcessorError(format!(
            "Invalid CamelRedis.Count: expected a positive integer (> 0), got {}",
            invalid_header_value_preview(raw)
        ))
    };
    let count = get_u64_header(exchange, "CamelRedis.Count").map_err(|_| invalid())?;
    match count {
        Some(count) if count > 0 => Ok(Some(count)),
        _ => Err(invalid()),
    }
}

/// Resolves the optional `CamelRedis.Rank`/`CamelRedis.Count` options for
/// `LPOS`.
///
/// `RANK 0` is rejected (Redis requires a non-zero rank); a negative `COUNT`
/// is rejected, while `COUNT 0` is valid and means "all matches".
pub(crate) fn resolve_lpos_args(
    exchange: &Exchange,
) -> Result<(Option<i64>, Option<i64>), CamelError> {
    let rank = get_i64_header(exchange, "CamelRedis.Rank")?;
    if rank == Some(0) {
        return Err(CamelError::ProcessorError(
            "Invalid CamelRedis.Rank: expected a non-zero integer (any positive or negative rank is valid), got 0".into(),
        ));
    }
    let count = get_i64_header(exchange, "CamelRedis.Count")?;
    if let Some(count) = count
        && count < 0
    {
        return Err(CamelError::ProcessorError(format!(
            "Invalid CamelRedis.Count: expected a non-negative integer (0 returns all matches), got {count}"
        )));
    }
    Ok((rank, count))
}

/// Renders the `LMPOP`/`BLMPOP` arguments in wire order:
/// `<numkeys> <key...> <LEFT|RIGHT> [COUNT n]`.
pub(crate) fn lmpop_args(keys: &[String], direction: &str, count: Option<u64>) -> Vec<String> {
    let mut args = Vec::with_capacity(keys.len() + 4);
    args.push(keys.len().to_string());
    args.extend(keys.iter().cloned());
    args.push(direction.to_string());
    if let Some(count) = count {
        args.push("COUNT".to_string());
        args.push(count.to_string());
    }
    args
}

/// The key and popped elements of a non-nil `LMPOP`/`BLMPOP` reply.
struct LmpopReply {
    key: Vec<u8>,
    elements: Vec<Vec<u8>>,
}

/// Converts the raw `LMPOP`/`BLMPOP` reply (`nil` or `[key, [element...]]`)
/// into its key/element bytes.
fn parse_lmpop_reply(reply: Value, verb: &str) -> Result<Option<LmpopReply>, CamelError> {
    let mut rows = match reply {
        Value::Nil => return Ok(None),
        Value::Array(rows) if rows.len() == 2 => rows,
        other => {
            return Err(CamelError::ProcessorError(format!(
                "{verb}: unexpected reply shape: expected nil or [key, [elements]], got {other:?}"
            )));
        }
    };
    let elements = match rows.pop() {
        Some(Value::Array(items)) => items
            .into_iter()
            .map(|item| reply_bulk_bytes(item, verb))
            .collect::<Result<Vec<_>, _>>()?,
        other => {
            return Err(CamelError::ProcessorError(format!(
                "{verb}: unexpected reply shape: expected an element array, got {other:?}"
            )));
        }
    };
    let key = match rows.pop() {
        Some(Value::BulkString(bytes)) => bytes,
        other => {
            return Err(CamelError::ProcessorError(format!(
                "{verb}: unexpected reply shape: expected a bulk-string key, got {other:?}"
            )));
        }
    };
    Ok(Some(LmpopReply { key, elements }))
}

/// Returns the raw bytes of a bulk-string reply value.
fn reply_bulk_bytes(value: Value, verb: &str) -> Result<Vec<u8>, CamelError> {
    match value {
        Value::BulkString(bytes) => Ok(bytes),
        Value::SimpleString(text) => Ok(text.into_bytes()),
        other => Err(CamelError::ProcessorError(format!(
            "{verb}: unexpected reply element: expected a bulk string, got {other:?}"
        ))),
    }
}

/// Encodes an `LMPOP`/`BLMPOP` reply as a RESP nested array of bulk strings.
///
/// Mirrors the server shape (`*2\r\n$<len>\r\n<key>\r\n*<n>\r\n` per
/// element `$<len>\r\n<bytes>\r\n`) so a binary key or element is preserved
/// instead of degrading to a lossy JSON string.
pub(crate) fn encode_lmpop_reply_bytes(key: &[u8], elements: &[Vec<u8>]) -> Vec<u8> {
    let mut out = format!("*2\r\n${}\r\n", key.len()).into_bytes();
    out.extend_from_slice(key);
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(format!("*{}\r\n", elements.len()).as_bytes());
    for element in elements {
        out.extend_from_slice(format!("${}\r\n", element.len()).as_bytes());
        out.extend_from_slice(element);
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// Maps an `LMPOP`/`BLMPOP` reply to an exchange body.
///
/// A nil reply is JSON null. An all-UTF-8 key and elements keep the JSON
/// array shape `[key, [elements]]`; any binary key or element switches the
/// whole reply to the RESP-encoded `Bytes` body.
fn lmpop_reply_to_body(reply: Option<LmpopReply>) -> Body {
    let Some(LmpopReply { key, elements }) = reply else {
        return Body::Json(serde_json::Value::Null);
    };
    let key_text = String::from_utf8(key.clone()).ok();
    let elements_text: Option<Vec<String>> = elements
        .iter()
        .map(|element| String::from_utf8(element.clone()).ok())
        .collect();
    match (key_text, elements_text) {
        (Some(key_text), Some(elements_text)) => {
            let elements: Vec<serde_json::Value> = elements_text
                .into_iter()
                .map(serde_json::Value::String)
                .collect();
            Body::Json(serde_json::json!([key_text, elements]))
        }
        _ => Body::Bytes(encode_lmpop_reply_bytes(&key, &elements).into()),
    }
}

/// Maps an `LPOS` reply to JSON: an integer index, a nil miss, or an array of
/// integer indexes when `COUNT` was supplied.
fn lpos_reply_json(reply: Value, verb: &str) -> Result<serde_json::Value, CamelError> {
    match reply {
        Value::Nil => Ok(serde_json::Value::Null),
        Value::Int(index) => Ok(serde_json::json!(index)),
        Value::Array(items) => items
            .into_iter()
            .map(|item| match item {
                Value::Int(index) => Ok(serde_json::json!(index)),
                other => Err(CamelError::ProcessorError(format!(
                    "{verb}: unexpected reply element: expected an integer index, got {other:?}"
                ))),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        other => Err(CamelError::ProcessorError(format!(
            "{verb}: unexpected reply shape: expected an integer or integer array, got {other:?}"
        ))),
    }
}

pub(crate) fn json_from_optional_string(value: Option<String>) -> serde_json::Value {
    value
        .map(serde_json::Value::String)
        .unwrap_or(serde_json::Value::Null)
}

pub(crate) fn json_from_optional_pair_value(value: Option<(String, String)>) -> serde_json::Value {
    value
        .map(|(_, v)| serde_json::Value::String(v))
        .unwrap_or(serde_json::Value::Null)
}

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    if !is_list_command(cmd) {
        return Err(CamelError::ProcessorError("Not a list command".into()));
    }

    let result: serde_json::Value = match cmd {
        RedisCommand::Lpush => {
            let key = require_key(exchange)?;
            let value = body_value_bytes(exchange, "LPUSH", &[HEADER_VALUE])?;
            let n: i64 = conn
                .lpush(&key, &value)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("LPUSH", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Rpush => {
            let key = require_key(exchange)?;
            let value = body_value_bytes(exchange, "RPUSH", &[HEADER_VALUE])?;
            let n: i64 = conn
                .rpush(&key, &value)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("RPUSH", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Lpushx => {
            let key = require_key(exchange)?;
            let value = body_value_bytes(exchange, "LPUSHX", &[HEADER_VALUE])?;
            let n: i64 = conn
                .lpush_exists(&key, &value)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("LPUSHX", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Rpushx => {
            let key = require_key(exchange)?;
            let value = body_value_bytes(exchange, "RPUSHX", &[HEADER_VALUE])?;
            let n: i64 = conn
                .rpush_exists(&key, &value)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("RPUSHX", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Lpop => {
            let key = require_key(exchange)?;
            let val: Option<String> = conn
                .lpop(&key, None)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("LPOP", e))?;
            json_from_optional_string(val)
        }
        RedisCommand::Rpop => {
            let key = require_key(exchange)?;
            let val: Option<String> = conn
                .rpop(&key, None)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("RPOP", e))?;
            json_from_optional_string(val)
        }
        RedisCommand::Blpop => {
            let key = require_key(exchange)?;
            let timeout = resolve_blocking_timeout(exchange)?;
            let val: Option<(String, String)> = conn
                .blpop(&key, timeout)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("BLPOP", e))?;
            json_from_optional_pair_value(val)
        }
        RedisCommand::Brpop => {
            let key = require_key(exchange)?;
            let timeout = resolve_blocking_timeout(exchange)?;
            let val: Option<(String, String)> = conn
                .brpop(&key, timeout)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("BRPOP", e))?;
            json_from_optional_pair_value(val)
        }
        RedisCommand::Llen => {
            let key = require_key(exchange)?;
            let n: i64 = conn
                .llen(&key)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("LLEN", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Lrange => {
            let key = require_key(exchange)?;
            let (start, end) = resolve_range_bounds(exchange)?;
            let vals: Vec<String> = conn
                .lrange(&key, start, end)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("LRANGE", e))?;
            serde_json::json!(vals)
        }
        RedisCommand::Lindex => {
            let key = require_key(exchange)?;
            let idx = resolve_index(exchange)?;
            let val: Option<String> = conn
                .lindex(&key, idx)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("LINDEX", e))?;
            json_from_optional_string(val)
        }
        RedisCommand::Linsert => {
            let key = require_key(exchange)?;
            let (mode, pivot) = resolve_linsert_operands(exchange)?;
            let value = body_value_bytes(exchange, "LINSERT", &[HEADER_VALUE])?;
            // Use redis::cmd for LINSERT since AsyncCommands doesn't have a direct method
            let n: i64 = redis::cmd("LINSERT")
                .arg(&key)
                .arg(mode)
                .arg(pivot)
                .arg(&value)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("LINSERT", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Lset => {
            let key = require_key(exchange)?;
            let idx = resolve_index(exchange)?;
            let value = body_value_bytes(exchange, "LSET", &[HEADER_VALUE])?;
            conn.lset::<_, _, ()>(&key, idx, &value)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("LSET", e))?;
            serde_json::Value::Null
        }
        RedisCommand::Lrem => {
            let key = require_key(exchange)?;
            let count = resolve_lrem_count(exchange)?;
            let value = body_value_bytes(exchange, "LREM", &[HEADER_VALUE])?;
            let n: usize = conn
                .lrem(&key, count, &value)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("LREM", e))?;
            serde_json::json!(n as i64)
        }
        RedisCommand::Ltrim => {
            let key = require_key(exchange)?;
            let (start, end) = resolve_range_bounds(exchange)?;
            conn.ltrim::<_, ()>(&key, start, end)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("LTRIM", e))?;
            serde_json::Value::Null
        }
        RedisCommand::Rpoplpush => {
            let key = require_key(exchange)?;
            let dest = resolve_destination(exchange)?;
            let val: Option<String> = conn
                .rpoplpush(&key, dest)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("RPOPLPUSH", e))?;
            json_from_optional_string(val)
        }
        RedisCommand::Lmpop => {
            let keys = require_key_list(exchange, "LMPOP")?;
            let direction = resolve_pop_direction(exchange)?;
            let count = resolve_pop_count_positive(exchange)?;
            let mut command = redis::cmd("LMPOP");
            for arg in lmpop_args(&keys, direction, count) {
                command.arg(arg);
            }
            let reply: Value = command
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("LMPOP", e))?;
            let pair = parse_lmpop_reply(reply, "LMPOP")?;
            // Binary-safe nested reply: the body itself carries the payload,
            // so this arm bypasses the `Body::Json` dispatch tail below.
            exchange.input.body = lmpop_reply_to_body(pair);
            return Ok(());
        }
        RedisCommand::Blmpop => {
            let keys = require_key_list(exchange, "BLMPOP")?;
            let direction = resolve_pop_direction(exchange)?;
            let count = resolve_pop_count_positive(exchange)?;
            let timeout = resolve_blocking_timeout(exchange)?;
            let mut command = redis::cmd("BLMPOP");
            // Integer token per the blessed frames; Redis accepts doubles but ryu would render "5.0".
            command.arg(timeout as u64);
            for arg in lmpop_args(&keys, direction, count) {
                command.arg(arg);
            }
            let reply: Value = command
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("BLMPOP", e))?;
            let pair = parse_lmpop_reply(reply, "BLMPOP")?;
            // Binary-safe nested reply: the body itself carries the payload.
            exchange.input.body = lmpop_reply_to_body(pair);
            return Ok(());
        }
        RedisCommand::Lmove => {
            let key = require_key(exchange)?;
            let dest = require_str_header(exchange, "CamelRedis.Destination")?.to_string();
            let (from, to) = resolve_lmove_directions(exchange)?;
            let val: Option<Vec<u8>> = redis::cmd("LMOVE")
                .arg(&key)
                .arg(&dest)
                .arg(from)
                .arg(to)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("LMOVE", e))?;
            // Single bulk-string reply: the body itself carries the payload.
            exchange.input.body = reply_to_body(val.as_deref());
            return Ok(());
        }
        RedisCommand::Blmove => {
            let key = require_key(exchange)?;
            let dest = require_str_header(exchange, "CamelRedis.Destination")?.to_string();
            let (from, to) = resolve_lmove_directions(exchange)?;
            let timeout = resolve_blocking_timeout(exchange)?;
            // Integer token per the blessed frames; Redis accepts doubles but ryu would render "5.0".
            let val: Option<Vec<u8>> = redis::cmd("BLMOVE")
                .arg(&key)
                .arg(&dest)
                .arg(from)
                .arg(to)
                .arg(timeout as u64)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("BLMOVE", e))?;
            // Single bulk-string reply: the body itself carries the payload.
            exchange.input.body = reply_to_body(val.as_deref());
            return Ok(());
        }
        RedisCommand::Lpos => {
            let key = require_key(exchange)?;
            let element = body_value_bytes(exchange, "LPOS", &[HEADER_VALUE])?;
            let (rank, count) = resolve_lpos_args(exchange)?;
            let mut command = redis::cmd("LPOS");
            command.arg(&key).arg(&element);
            if let Some(rank) = rank {
                command.arg("RANK").arg(rank);
            }
            if let Some(count) = count {
                command.arg("COUNT").arg(count);
            }
            let reply: Value = command
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("LPOS", e))?;
            lpos_reply_json(reply, "LPOS")?
        }
        _ => unreachable!("non-list commands rejected above"),
    };
    exchange.input.body = Body::Json(result);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::{Exchange, Message};

    #[test]
    fn test_lpush_requires_key_and_value() {
        let ex = Exchange::new(Message::default());
        assert!(crate::commands::require_key(&ex).is_err());
    }

    #[test]
    fn test_lrange_has_start_and_end() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Key", serde_json::json!("mylist"));
        msg.set_header("CamelRedis.Start", serde_json::json!(0));
        msg.set_header("CamelRedis.End", serde_json::json!(-1));
        let ex = Exchange::new(msg);
        assert_eq!(get_i64_header(&ex, "CamelRedis.Start").unwrap(), Some(0));
        assert_eq!(get_i64_header(&ex, "CamelRedis.End").unwrap(), Some(-1));
    }

    #[test]
    fn test_linsert_has_pivot_and_position() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Key", serde_json::json!("mylist"));
        msg.set_header("CamelRedis.Pivot", serde_json::json!("pivot_value"));
        msg.set_header("CamelRedis.Position", serde_json::json!("AFTER"));
        let ex = Exchange::new(msg);
        assert_eq!(get_str_header(&ex, "CamelRedis.Pivot"), Some("pivot_value"));
        assert_eq!(get_str_header(&ex, "CamelRedis.Position"), Some("AFTER"));
    }

    #[test]
    fn test_blpop_has_timeout() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Key", serde_json::json!("myqueue"));
        msg.set_header("CamelRedis.Timeout", serde_json::json!(5));
        let ex = Exchange::new(msg);
        assert_eq!(get_u64_header(&ex, "CamelRedis.Timeout").unwrap(), Some(5));
    }

    #[test]
    fn test_rpoplpush_has_destination() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Key", serde_json::json!("src"));
        msg.set_header("CamelRedis.Destination", serde_json::json!("dest"));
        let ex = Exchange::new(msg);
        assert_eq!(resolve_destination(&ex).unwrap(), "dest");
    }

    #[test]
    fn test_list_command_classification() {
        assert!(is_list_command(&RedisCommand::Lpush));
        assert!(is_list_command(&RedisCommand::Rpoplpush));
        assert!(!is_list_command(&RedisCommand::Set));
    }

    #[test]
    fn test_resolve_linsert_mode_defaults_before() {
        let ex = Exchange::new(Message::default());
        assert_eq!(resolve_linsert_mode(&ex), "BEFORE");
    }

    #[test]
    fn test_resolve_linsert_mode_after_for_non_before_values() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Position", serde_json::json!("AFTER"));
        let ex = Exchange::new(msg);
        assert_eq!(resolve_linsert_mode(&ex), "AFTER");
    }

    #[test]
    fn test_resolve_destination_requires_header() {
        let ex = Exchange::new(Message::default());
        let err = resolve_destination(&ex).expect_err("destination should be required");
        assert!(err.to_string().contains("CamelRedis.Destination"));
    }

    #[test]
    fn test_resolve_blocking_timeout_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_blocking_timeout(&ex_default).unwrap(), 0.0);

        let mut msg = Message::default();
        msg.set_header("CamelRedis.Timeout", serde_json::json!(7));
        let ex = Exchange::new(msg);
        assert_eq!(resolve_blocking_timeout(&ex).unwrap(), 7.0);

        let mut smsg = Message::default();
        smsg.set_header("CamelRedis.Timeout", serde_json::json!(" 7 "));
        let sex = Exchange::new(smsg);
        assert_eq!(resolve_blocking_timeout(&sex).unwrap(), 7.0);
    }

    #[test]
    fn test_resolve_blocking_timeout_garbage_is_named_error() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Timeout", serde_json::json!("abc"));
        let ex = Exchange::new(msg);
        let err = resolve_blocking_timeout(&ex).expect_err("garbage timeout must fail");
        let text = err.to_string();
        assert!(text.contains("CamelRedis.Timeout"), "{text}");
        assert!(text.contains("abc"), "{text}");
    }

    #[test]
    fn test_resolve_index_default_and_value() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_index(&ex_default).unwrap(), 0);

        let mut msg = Message::default();
        msg.set_header("CamelRedis.Index", serde_json::json!(4));
        let ex = Exchange::new(msg);
        assert_eq!(resolve_index(&ex).unwrap(), 4);

        let mut smsg = Message::default();
        smsg.set_header("CamelRedis.Index", serde_json::json!(" -1 "));
        let sex = Exchange::new(smsg);
        assert_eq!(resolve_index(&sex).unwrap(), -1);
    }

    #[test]
    fn test_resolve_range_bounds_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_range_bounds(&ex_default).unwrap(), (0, -1));

        let mut msg = Message::default();
        msg.set_header("CamelRedis.Start", serde_json::json!(2));
        msg.set_header("CamelRedis.End", serde_json::json!(5));
        let ex = Exchange::new(msg);
        assert_eq!(resolve_range_bounds(&ex).unwrap(), (2, 5));
    }

    #[test]
    fn test_resolve_lrem_count_default_and_value() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_lrem_count(&ex_default).unwrap(), 0);

        let mut msg = Message::default();
        msg.set_header("CamelRedis.Count", serde_json::json!(3));
        let ex = Exchange::new(msg);
        assert_eq!(resolve_lrem_count(&ex).unwrap(), 3);
    }

    #[test]
    fn test_resolve_linsert_operands_requires_pivot() {
        let ex = Exchange::new(Message::default());
        let err = resolve_linsert_operands(&ex).expect_err("pivot should be required");
        assert!(err.to_string().contains("CamelRedis.Pivot"));
    }

    #[test]
    fn test_json_from_optional_helpers() {
        assert_eq!(
            json_from_optional_string(Some("a".to_string())),
            serde_json::json!("a")
        );
        assert_eq!(json_from_optional_string(None), serde_json::Value::Null);

        assert_eq!(
            json_from_optional_pair_value(Some(("k".to_string(), "v".to_string()))),
            serde_json::json!("v")
        );
        assert_eq!(json_from_optional_pair_value(None), serde_json::Value::Null);
    }

    // ── List command family (rediscmds task 2.3) ────────────────────────────

    fn exchange_with(headers: &[(&str, serde_json::Value)]) -> Exchange {
        let mut msg = Message::default();
        for (key, value) in headers {
            msg.set_header(*key, value.clone());
        }
        Exchange::new(msg)
    }

    #[test]
    fn resolve_pop_direction_domains() {
        for (raw, expected) in [("LEFT", "LEFT"), ("right", "RIGHT")] {
            let ex = exchange_with(&[("CamelRedis.Where", serde_json::json!(raw))]);
            assert_eq!(
                resolve_pop_direction(&ex).expect("a valid direction must resolve"),
                expected,
                "{raw} must map to {expected}"
            );
        }

        let invalid = exchange_with(&[("CamelRedis.Where", serde_json::json!("UP"))]);
        let err = resolve_pop_direction(&invalid).expect_err("UP must be refused");
        let text = err.to_string();
        assert!(text.contains("CamelRedis.Where"), "{text}");
        assert!(text.contains("LEFT"), "{text}");
        assert!(text.contains("RIGHT"), "{text}");

        let missing = Exchange::new(Message::default());
        let err = resolve_pop_direction(&missing).expect_err("a missing direction must fail");
        let text = err.to_string();
        assert!(text.contains("CamelRedis.Where"), "{text}");
        assert!(text.contains("LEFT"), "{text}");
        assert!(text.contains("RIGHT"), "{text}");
    }

    #[test]
    fn resolve_lmove_directions_domains() {
        let valid = exchange_with(&[
            ("CamelRedis.WhereFrom", serde_json::json!("LEFT")),
            ("CamelRedis.WhereTo", serde_json::json!("RIGHT")),
        ]);
        assert_eq!(
            resolve_lmove_directions(&valid).expect("a valid pair must resolve"),
            ("LEFT", "RIGHT")
        );

        let invalid = exchange_with(&[
            ("CamelRedis.WhereFrom", serde_json::json!("CENTER")),
            ("CamelRedis.WhereTo", serde_json::json!("RIGHT")),
        ]);
        let err = resolve_lmove_directions(&invalid).expect_err("CENTER must be refused");
        let text = err.to_string();
        assert!(text.contains("CamelRedis.WhereFrom"), "{text}");
        assert!(text.contains("LEFT"), "{text}");
        assert!(text.contains("RIGHT"), "{text}");

        let missing_to = exchange_with(&[("CamelRedis.WhereFrom", serde_json::json!("LEFT"))]);
        let err = resolve_lmove_directions(&missing_to).expect_err("a missing WhereTo must fail");
        assert!(
            err.to_string().contains("CamelRedis.WhereTo"),
            "error must name the missing header: {err}"
        );
    }

    #[test]
    fn resolve_pop_count_positive_domain() {
        let present = exchange_with(&[("CamelRedis.Count", serde_json::json!(2))]);
        assert_eq!(
            resolve_pop_count_positive(&present).expect("a positive count must resolve"),
            Some(2)
        );

        for raw in [serde_json::json!(0), serde_json::json!(-1)] {
            let ex = exchange_with(&[("CamelRedis.Count", raw)]);
            let err = resolve_pop_count_positive(&ex).expect_err("a non-positive count must fail");
            let text = err.to_string();
            assert!(text.contains("CamelRedis.Count"), "{text}");
            assert!(text.contains("positive"), "{text}");
        }

        let absent = Exchange::new(Message::default());
        assert_eq!(
            resolve_pop_count_positive(&absent).expect("an absent count must resolve to None"),
            None
        );
    }

    #[test]
    fn resolve_lpos_args_domains() {
        let zero_rank = exchange_with(&[("CamelRedis.Rank", serde_json::json!(0))]);
        let err = resolve_lpos_args(&zero_rank).expect_err("rank zero must be refused");
        let text = err.to_string();
        assert!(text.contains("CamelRedis.Rank"), "{text}");
        assert!(text.contains("non-zero"), "{text}");

        let negative_count = exchange_with(&[("CamelRedis.Count", serde_json::json!(-1))]);
        let err = resolve_lpos_args(&negative_count).expect_err("a negative count must be refused");
        let text = err.to_string();
        assert!(text.contains("CamelRedis.Count"), "{text}");
        assert!(text.contains("non-negative"), "{text}");

        let valid = exchange_with(&[
            ("CamelRedis.Rank", serde_json::json!(-2)),
            ("CamelRedis.Count", serde_json::json!(0)),
        ]);
        assert_eq!(
            resolve_lpos_args(&valid).expect("a negative rank and zero count must resolve"),
            (Some(-2), Some(0))
        );

        let absent = Exchange::new(Message::default());
        assert_eq!(
            resolve_lpos_args(&absent).expect("absent options must resolve to None"),
            (None, None)
        );
    }

    #[test]
    fn lpos_body_forms_fail_closed_or_compact() {
        use camel_component_api::StreamBody;
        use std::sync::Arc;

        let rejected = [
            ("Body::Empty", Body::Empty),
            (
                "Body::Stream",
                Body::Stream(StreamBody {
                    stream: Arc::new(tokio::sync::Mutex::new(None)),
                    metadata: Default::default(),
                }),
            ),
            ("Body::Json(null)", Body::Json(serde_json::Value::Null)),
        ];
        for (label, body) in rejected {
            let mut msg = Message::new(body);
            msg.set_header("CamelRedis.Key", serde_json::json!("k"));
            let ex = Exchange::new(msg);
            let err = body_value_bytes(&ex, "LPOS", &[HEADER_VALUE])
                .expect_err("a payload-less body must be refused");
            let text = err.to_string();
            assert!(text.contains("LPOS"), "{label}: {text}");
            assert!(text.contains("body"), "{label}: {text}");
        }

        let mut empty_msg = Message::new(Body::Text(String::new()));
        empty_msg.set_header("CamelRedis.Key", serde_json::json!("k"));
        let empty = Exchange::new(empty_msg);
        assert_eq!(
            body_value_bytes(&empty, "LPOS", &[HEADER_VALUE])
                .expect("an empty text body is an empty bulk argument"),
            Vec::<u8>::new()
        );

        let mut json_msg = Message::new(Body::Json(serde_json::json!({"a": 1})));
        json_msg.set_header("CamelRedis.Key", serde_json::json!("k"));
        let json = Exchange::new(json_msg);
        assert_eq!(
            body_value_bytes(&json, "LPOS", &[HEADER_VALUE])
                .expect("a JSON object resolves through compact serialization"),
            br#"{"a":1}"#.to_vec()
        );
    }

    #[test]
    fn lmpop_wire_args_assembly() {
        let keys = vec!["a".to_string(), "b".to_string()];
        assert_eq!(
            lmpop_args(&keys, "LEFT", Some(2)),
            vec!["2", "a", "b", "LEFT", "COUNT", "2"]
        );
        assert_eq!(lmpop_args(&keys, "LEFT", None), vec!["2", "a", "b", "LEFT"]);
    }
}
