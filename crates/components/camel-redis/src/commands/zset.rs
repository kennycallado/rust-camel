use super::list::{resolve_blocking_timeout, resolve_pop_count_positive};
use super::string::encode_mget_reply_bytes;
use super::{
    body_value_bytes, get_bool_header, get_f64_header, get_i64_header, get_str_header,
    get_str_vec_header, invalid_header_value_preview, reply_to_body, require_key, require_key_list,
    require_str_header,
};
use crate::HEADER_VALUE;
use crate::config::RedisCommand;
use camel_component_api::{Body, CamelError, Exchange};
use redis::AsyncCommands;
use redis::Value;
use redis::aio::MultiplexedConnection;

pub(crate) fn is_zset_command(cmd: &RedisCommand) -> bool {
    matches!(
        cmd,
        RedisCommand::Zadd
            | RedisCommand::Zrem
            | RedisCommand::Zrange
            | RedisCommand::Zrevrange
            | RedisCommand::Zrank
            | RedisCommand::Zrevrank
            | RedisCommand::Zscore
            | RedisCommand::Zcard
            | RedisCommand::Zincrby
            | RedisCommand::Zcount
            | RedisCommand::Zrangebyscore
            | RedisCommand::Zrevrangebyscore
            | RedisCommand::Zremrangebyrank
            | RedisCommand::Zremrangebyscore
            | RedisCommand::Zunionstore
            | RedisCommand::Zinterstore
            | RedisCommand::Zmpop
            | RedisCommand::Bzmpop
            | RedisCommand::Zrandmember
            | RedisCommand::Zdiff
            | RedisCommand::Zinter
            | RedisCommand::Zunion
            | RedisCommand::Zrangebylex
    )
}

pub(crate) fn resolve_destination(exchange: &Exchange) -> Result<String, CamelError> {
    get_str_header(exchange, "CamelRedis.Destination")
        .map(|s| s.to_string())
        .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Destination".into()))
}

pub(crate) fn resolve_zstore_keys(exchange: &Exchange) -> Result<Vec<String>, CamelError> {
    if let Some(keys) = get_str_vec_header(exchange, "CamelRedis.Keys") {
        Ok(keys)
    } else if let Ok(key) = require_key(exchange) {
        Ok(vec![key])
    } else {
        Err(CamelError::ProcessorError(
            "ZUNIONSTORE/ZINTERSTORE requires either CamelRedis.Keys (array) or CamelRedis.Key header".into(),
        ))
    }
}

pub(crate) fn resolve_range_bounds(exchange: &Exchange) -> Result<(isize, isize), CamelError> {
    Ok((
        get_i64_header(exchange, "CamelRedis.Start")?.unwrap_or(0) as isize,
        get_i64_header(exchange, "CamelRedis.End")?.unwrap_or(-1) as isize,
    ))
}

pub(crate) fn resolve_score_bounds(exchange: &Exchange) -> Result<(f64, f64), CamelError> {
    Ok((
        get_f64_header(exchange, "CamelRedis.Min")?.unwrap_or(f64::NEG_INFINITY),
        get_f64_header(exchange, "CamelRedis.Max")?.unwrap_or(f64::INFINITY),
    ))
}

pub(crate) fn resolve_with_scores(exchange: &Exchange) -> bool {
    get_bool_header(exchange, "CamelRedis.WithScore").unwrap_or(false)
}

pub(crate) fn resolve_zadd_score(exchange: &Exchange) -> Result<f64, CamelError> {
    Ok(get_f64_header(exchange, "CamelRedis.Score")?.unwrap_or(0.0))
}

pub(crate) fn resolve_zincr_increment(exchange: &Exchange) -> Result<f64, CamelError> {
    Ok(get_f64_header(exchange, "CamelRedis.Increment")?.unwrap_or(1.0))
}

pub(crate) fn resolve_zremrange_rank_bounds(
    exchange: &Exchange,
) -> Result<(isize, isize), CamelError> {
    resolve_range_bounds(exchange)
}

pub(crate) fn resolve_revrange_score_bounds(exchange: &Exchange) -> Result<(f64, f64), CamelError> {
    let (min, max) = resolve_score_bounds(exchange)?;
    Ok((max, min))
}

pub(crate) fn resolve_zstore_operands(
    exchange: &Exchange,
) -> Result<(String, Vec<String>), CamelError> {
    Ok((
        resolve_destination(exchange)?,
        resolve_zstore_keys(exchange)?,
    ))
}

/// Resolves the `MIN`/`MAX` score order for `ZMPOP`/`BZMPOP`.
///
/// Sorted-set pops order by score, not by list end, so the header is
/// required: a missing or invalid value is a named pre-send error instead of
/// a silently chosen default.
pub(crate) fn resolve_order_by(exchange: &Exchange) -> Result<&'static str, CamelError> {
    let Some(raw) = get_str_header(exchange, "CamelRedis.OrderBy") else {
        return Err(CamelError::ProcessorError(
            "Missing CamelRedis.OrderBy: expected MIN or MAX (sorted-set pops order by score, not by list end)".into(),
        ));
    };
    if raw.eq_ignore_ascii_case("MIN") {
        return Ok("MIN");
    }
    if raw.eq_ignore_ascii_case("MAX") {
        return Ok("MAX");
    }
    Err(CamelError::ProcessorError(format!(
        "Invalid CamelRedis.OrderBy: expected MIN or MAX (sorted-set pops order by score, not by list end), got {}",
        invalid_header_value_preview(&serde_json::Value::String(raw.to_string()))
    )))
}

/// Resolves the optional `CamelRedis.Count` for `ZRANDMEMBER`.
///
/// Any `i64` is accepted: a negative count is valid Redis semantics (the
/// returned members may repeat). Garbage is already a named error from
/// [`get_i64_header`].
pub(crate) fn resolve_optional_count(exchange: &Exchange) -> Result<Option<i64>, CamelError> {
    get_i64_header(exchange, "CamelRedis.Count")
}

/// Resolves the required lexicographic bounds for `ZRANGEBYLEX`.
///
/// Both bounds are mandatory; the error names the missing header.
pub(crate) fn resolve_lex_bounds(exchange: &Exchange) -> Result<(String, String), CamelError> {
    Ok((
        require_str_header(exchange, "CamelRedis.Min")?.to_string(),
        require_str_header(exchange, "CamelRedis.Max")?.to_string(),
    ))
}

pub(crate) fn json_from_optional_rank(value: Option<i64>) -> serde_json::Value {
    value.map_or(serde_json::Value::Null, |v| serde_json::json!(v))
}

pub(crate) fn json_from_optional_score(value: Option<f64>) -> serde_json::Value {
    value.map_or(serde_json::Value::Null, |v| serde_json::json!(v))
}

pub(crate) fn json_from_scored_members(values: Vec<(String, f64)>) -> serde_json::Value {
    serde_json::json!(
        values
            .into_iter()
            .map(|(member, score)| serde_json::json!({"member": member, "score": score}))
            .collect::<Vec<_>>()
    )
}

/// Assembles the `ZMPOP`/`BZMPOP` argument list after the verb.
///
/// `timeout` is `Some` for `BZMPOP` (the leading blocking timeout) and `None`
/// for `ZMPOP`. Both commands carry `<numkeys> <keys...> <MIN|MAX>`, then an
/// optional `COUNT n`.
pub(crate) fn zmpop_args(
    timeout: Option<u64>,
    keys: &[String],
    order: &str,
    count: Option<u64>,
) -> Vec<String> {
    let mut args = Vec::with_capacity(keys.len() + 6);
    if let Some(timeout) = timeout {
        args.push(timeout.to_string());
    }
    args.push(keys.len().to_string());
    args.extend(keys.iter().cloned());
    args.push(order.to_string());
    if let Some(count) = count {
        args.push("COUNT".to_string());
        args.push(count.to_string());
    }
    args
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

/// Parses a member-array reply (`ZDIFF`/`ZINTER`/`ZUNION`/`ZRANGEBYLEX`/
/// `ZRANDMEMBER COUNT`) into raw member bytes.
fn parse_member_array(reply: Value, verb: &str) -> Result<Vec<Vec<u8>>, CamelError> {
    match reply {
        Value::Array(items) => items
            .into_iter()
            .map(|item| reply_bulk_bytes(item, verb))
            .collect(),
        Value::Nil => Ok(Vec::new()),
        other => Err(CamelError::ProcessorError(format!(
            "{verb}: unexpected reply shape: expected a member array, got {other:?}"
        ))),
    }
}

/// Maps a member-array reply to an exchange body per the `MGET` binary rule.
///
/// An all-UTF-8 array maps to a JSON string array; any binary member switches
/// the whole reply to the RESP-encoded `Bytes` body so no member is mangled.
fn member_array_reply_to_body(members: Vec<Vec<u8>>) -> Body {
    let values: Vec<Option<Vec<u8>>> = members.into_iter().map(Some).collect();
    let text: Option<Vec<String>> = values
        .iter()
        .map(|value| {
            value
                .as_ref()
                .and_then(|bytes| String::from_utf8(bytes.clone()).ok())
        })
        .collect();
    match text {
        Some(text) => Body::Json(serde_json::json!(text)),
        None => Body::Bytes(encode_mget_reply_bytes(&values).into()),
    }
}

/// The key and scored members of a non-nil `ZMPOP`/`BZMPOP` reply.
struct ZmpopReply {
    key: Vec<u8>,
    members: Vec<(Vec<u8>, f64)>,
}

/// Converts the raw `ZMPOP`/`BZMPOP` reply (`nil` or
/// `[key, [[member, score], ...]]`) into its key/member bytes and scores.
fn parse_zmpop_reply(reply: Value, verb: &str) -> Result<Option<ZmpopReply>, CamelError> {
    let mut rows = match reply {
        Value::Nil => return Ok(None),
        Value::Array(rows) if rows.len() == 2 => rows,
        other => {
            return Err(CamelError::ProcessorError(format!(
                "{verb}: unexpected reply shape: expected nil or [key, [[member, score], ...]], got {other:?}"
            )));
        }
    };
    let entries = match rows.pop() {
        Some(Value::Array(entries)) => entries,
        other => {
            return Err(CamelError::ProcessorError(format!(
                "{verb}: unexpected reply shape: expected a scored-member array, got {other:?}"
            )));
        }
    };
    let members = entries
        .into_iter()
        .map(|entry| parse_scored_member(entry, verb))
        .collect::<Result<Vec<_>, _>>()?;
    let key = match rows.pop() {
        Some(Value::BulkString(bytes)) => bytes,
        Some(Value::SimpleString(text)) => text.into_bytes(),
        other => {
            return Err(CamelError::ProcessorError(format!(
                "{verb}: unexpected reply shape: expected a bulk-string key, got {other:?}"
            )));
        }
    };
    Ok(Some(ZmpopReply { key, members }))
}

/// Parses one `[member, score]` pair of a `ZMPOP`/`BZMPOP` reply.
fn parse_scored_member(entry: Value, verb: &str) -> Result<(Vec<u8>, f64), CamelError> {
    let mut pair = match entry {
        Value::Array(pair) if pair.len() == 2 => pair,
        other => {
            return Err(CamelError::ProcessorError(format!(
                "{verb}: unexpected reply element: expected a [member, score] pair, got {other:?}"
            )));
        }
    };
    let Some(score_value) = pair.pop() else {
        return Err(CamelError::ProcessorError(format!(
            "{verb}: unexpected reply element: expected a [member, score] pair"
        )));
    };
    let Some(member_value) = pair.pop() else {
        return Err(CamelError::ProcessorError(format!(
            "{verb}: unexpected reply element: expected a [member, score] pair"
        )));
    };
    let member = reply_bulk_bytes(member_value, verb)?;
    let score = match score_value {
        Value::BulkString(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            text.parse::<f64>().map_err(|_| {
                CamelError::ProcessorError(format!(
                    "{verb}: unexpected reply score: expected a numeric bulk string, got {text:?}"
                ))
            })?
        }
        Value::Double(score) => score,
        Value::Int(score) => score as f64,
        other => {
            return Err(CamelError::ProcessorError(format!(
                "{verb}: unexpected reply score: expected a bulk string or double, got {other:?}"
            )));
        }
    };
    Ok((member, score))
}

/// Encodes a `ZMPOP`/`BZMPOP` reply as a RESP nested array.
///
/// Mirrors `[key, [[member, score], ...]]` so a binary key or member is
/// preserved instead of degrading to a lossy JSON string; scores render as
/// bulk strings, as the server does in RESP2.
pub(crate) fn encode_zmpop_reply_bytes(key: &[u8], members: &[(Vec<u8>, f64)]) -> Vec<u8> {
    let mut out = format!("*2\r\n${}\r\n", key.len()).into_bytes();
    out.extend_from_slice(key);
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(format!("*{}\r\n", members.len()).as_bytes());
    for (member, score) in members {
        out.extend_from_slice(b"*2\r\n");
        out.extend_from_slice(format!("${}\r\n", member.len()).as_bytes());
        out.extend_from_slice(member);
        out.extend_from_slice(b"\r\n");
        let score = score.to_string();
        out.extend_from_slice(format!("${}\r\n", score.len()).as_bytes());
        out.extend_from_slice(score.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// Maps a `ZMPOP`/`BZMPOP` reply to an exchange body.
///
/// A nil reply is JSON null. An all-UTF-8 key and members keep the JSON array
/// shape `[key, [[member, score], ...]]`; any binary key or member switches
/// the whole reply to the RESP-encoded `Bytes` body.
fn zmpop_reply_to_body(reply: Option<ZmpopReply>) -> Body {
    let Some(ZmpopReply { key, members }) = reply else {
        return Body::Json(serde_json::Value::Null);
    };
    let key_text = String::from_utf8(key.clone()).ok();
    let members_text: Option<Vec<String>> = members
        .iter()
        .map(|(member, _)| String::from_utf8(member.clone()).ok())
        .collect();
    match (key_text, members_text) {
        (Some(key_text), Some(members_text)) => {
            let pairs: Vec<serde_json::Value> = members_text
                .into_iter()
                .zip(members.iter())
                .map(|(member, (_, score))| serde_json::json!([member, score]))
                .collect();
            Body::Json(serde_json::json!([key_text, pairs]))
        }
        _ => Body::Bytes(encode_zmpop_reply_bytes(&key, &members).into()),
    }
}

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    if !is_zset_command(cmd) {
        return Err(CamelError::ProcessorError(
            "Not a sorted set command".into(),
        ));
    }

    let result: serde_json::Value = match cmd {
        RedisCommand::Zadd => {
            let key = require_key(exchange)?;
            let score = resolve_zadd_score(exchange)?;
            let member = body_value_bytes(exchange, "ZADD", &[HEADER_VALUE])?;
            let n: i64 = conn
                .zadd(&key, &member, score)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZADD", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Zrem => {
            let key = require_key(exchange)?;
            let member = body_value_bytes(exchange, "ZREM", &[HEADER_VALUE])?;
            let n: i64 = conn
                .zrem(&key, &member)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZREM", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Zrange => {
            let key = require_key(exchange)?;
            let (start, end) = resolve_range_bounds(exchange)?;
            let with_scores = resolve_with_scores(exchange);
            if with_scores {
                let vals: Vec<(String, f64)> = conn
                    .zrange_withscores(&key, start, end)
                    .await
                    .map_err(|e| crate::transport_error::redis_error_to_camel("ZRANGE", e))?;
                json_from_scored_members(vals)
            } else {
                let vals: Vec<String> = conn
                    .zrange(&key, start, end)
                    .await
                    .map_err(|e| crate::transport_error::redis_error_to_camel("ZRANGE", e))?;
                serde_json::json!(vals)
            }
        }
        RedisCommand::Zrevrange => {
            let key = require_key(exchange)?;
            let (start, end) = resolve_range_bounds(exchange)?;
            let with_scores = resolve_with_scores(exchange);
            if with_scores {
                let vals: Vec<(String, f64)> = conn
                    .zrevrange_withscores(&key, start, end)
                    .await
                    .map_err(|e| crate::transport_error::redis_error_to_camel("ZREVRANGE", e))?;
                json_from_scored_members(vals)
            } else {
                let vals: Vec<String> = conn
                    .zrevrange(&key, start, end)
                    .await
                    .map_err(|e| crate::transport_error::redis_error_to_camel("ZREVRANGE", e))?;
                serde_json::json!(vals)
            }
        }
        RedisCommand::Zrank => {
            let key = require_key(exchange)?;
            let member = body_value_bytes(exchange, "ZRANK", &[HEADER_VALUE])?;
            let rank: Option<i64> = conn
                .zrank(&key, &member)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZRANK", e))?;
            json_from_optional_rank(rank)
        }
        RedisCommand::Zrevrank => {
            let key = require_key(exchange)?;
            let member = body_value_bytes(exchange, "ZREVRANK", &[HEADER_VALUE])?;
            let rank: Option<i64> = conn
                .zrevrank(&key, &member)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZREVRANK", e))?;
            json_from_optional_rank(rank)
        }
        RedisCommand::Zscore => {
            let key = require_key(exchange)?;
            let member = body_value_bytes(exchange, "ZSCORE", &[HEADER_VALUE])?;
            let score: Option<f64> = conn
                .zscore(&key, &member)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZSCORE", e))?;
            json_from_optional_score(score)
        }
        RedisCommand::Zcard => {
            let key = require_key(exchange)?;
            let n: i64 = conn
                .zcard(&key)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZCARD", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Zincrby => {
            let key = require_key(exchange)?;
            let increment = resolve_zincr_increment(exchange)?;
            let member = body_value_bytes(exchange, "ZINCRBY", &[HEADER_VALUE])?;
            let new_score: f64 = conn
                .zincr(&key, &member, increment)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZINCRBY", e))?;
            serde_json::json!(new_score)
        }
        RedisCommand::Zcount => {
            let key = require_key(exchange)?;
            let (min, max) = resolve_score_bounds(exchange)?;
            let n: i64 = conn
                .zcount(&key, min, max)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZCOUNT", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Zrangebyscore => {
            let key = require_key(exchange)?;
            let (min, max) = resolve_score_bounds(exchange)?;
            let with_scores = resolve_with_scores(exchange);
            if with_scores {
                let vals: Vec<(String, f64)> = conn
                    .zrangebyscore_withscores(&key, min, max)
                    .await
                    .map_err(|e| {
                        crate::transport_error::redis_error_to_camel("ZRANGEBYSCORE", e)
                    })?;
                json_from_scored_members(vals)
            } else {
                let vals: Vec<String> = conn.zrangebyscore(&key, min, max).await.map_err(|e| {
                    crate::transport_error::redis_error_to_camel("ZRANGEBYSCORE", e)
                })?;
                serde_json::json!(vals)
            }
        }
        RedisCommand::Zrevrangebyscore => {
            let key = require_key(exchange)?;
            let (max, min) = resolve_revrange_score_bounds(exchange)?;
            let with_scores = resolve_with_scores(exchange);
            if with_scores {
                let vals: Vec<(String, f64)> = conn
                    .zrevrangebyscore_withscores(&key, max, min)
                    .await
                    .map_err(|e| {
                        crate::transport_error::redis_error_to_camel("ZREVRANGEBYSCORE", e)
                    })?;
                json_from_scored_members(vals)
            } else {
                let vals: Vec<String> =
                    conn.zrevrangebyscore(&key, max, min).await.map_err(|e| {
                        crate::transport_error::redis_error_to_camel("ZREVRANGEBYSCORE", e)
                    })?;
                serde_json::json!(vals)
            }
        }
        RedisCommand::Zremrangebyrank => {
            let key = require_key(exchange)?;
            let (start, end) = resolve_zremrange_rank_bounds(exchange)?;
            let n: usize = conn
                .zremrangebyrank(&key, start, end)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZREMRANGEBYRANK", e))?;
            serde_json::json!(n as i64)
        }
        RedisCommand::Zremrangebyscore => {
            let key = require_key(exchange)?;
            let (min, max) = resolve_score_bounds(exchange)?;
            // Use raw command since method doesn't exist in redis-rs
            let n: i64 = redis::cmd("ZREMRANGEBYSCORE")
                .arg(&key)
                .arg(min)
                .arg(max)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZREMRANGEBYSCORE", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Zunionstore => {
            let (dest, keys) = resolve_zstore_operands(exchange)?;
            let n: i64 = conn
                .zunionstore(dest, &keys)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZUNIONSTORE", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Zinterstore => {
            let (dest, keys) = resolve_zstore_operands(exchange)?;
            let n: i64 = conn
                .zinterstore(dest, &keys)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZINTERSTORE", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Zmpop => {
            let keys = require_key_list(exchange, "ZMPOP")?;
            let order = resolve_order_by(exchange)?;
            let count = resolve_pop_count_positive(exchange)?;
            let mut command = redis::cmd("ZMPOP");
            for arg in zmpop_args(None, &keys, order, count) {
                command.arg(arg);
            }
            let reply: Value = command
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZMPOP", e))?;
            let popped = parse_zmpop_reply(reply, "ZMPOP")?;
            // Binary-safe nested reply: the body itself carries the payload,
            // so this arm bypasses the `Body::Json` dispatch tail below.
            exchange.input.body = zmpop_reply_to_body(popped);
            return Ok(());
        }
        RedisCommand::Bzmpop => {
            let keys = require_key_list(exchange, "BZMPOP")?;
            let order = resolve_order_by(exchange)?;
            // An absent Count is sent as COUNT 1: the blessed BZMPOP frame
            // always carries an explicit count.
            let count = resolve_pop_count_positive(exchange)?.unwrap_or(1);
            let timeout = resolve_blocking_timeout(exchange)?;
            let mut command = redis::cmd("BZMPOP");
            // Integer token per the blessed frames; Redis accepts doubles but ryu would render "5.0".
            for arg in zmpop_args(Some(timeout as u64), &keys, order, Some(count)) {
                command.arg(arg);
            }
            let reply: Value = command
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("BZMPOP", e))?;
            let popped = parse_zmpop_reply(reply, "BZMPOP")?;
            // Binary-safe nested reply: the body itself carries the payload.
            exchange.input.body = zmpop_reply_to_body(popped);
            return Ok(());
        }
        RedisCommand::Zrandmember => {
            let key = require_key(exchange)?;
            match resolve_optional_count(exchange)? {
                Some(count) => {
                    let reply: Value = redis::cmd("ZRANDMEMBER")
                        .arg(&key)
                        .arg(count)
                        .query_async(conn)
                        .await
                        .map_err(|e| {
                            crate::transport_error::redis_error_to_camel("ZRANDMEMBER", e)
                        })?;
                    let members = parse_member_array(reply, "ZRANDMEMBER")?;
                    // Member-array reply: the body itself carries the payload.
                    exchange.input.body = member_array_reply_to_body(members);
                }
                None => {
                    let member: Option<Vec<u8>> = redis::cmd("ZRANDMEMBER")
                        .arg(&key)
                        .query_async(conn)
                        .await
                        .map_err(|e| {
                            crate::transport_error::redis_error_to_camel("ZRANDMEMBER", e)
                        })?;
                    // Single bulk-string reply: the body itself carries the payload.
                    exchange.input.body = reply_to_body(member.as_deref());
                }
            }
            return Ok(());
        }
        RedisCommand::Zdiff | RedisCommand::Zinter | RedisCommand::Zunion => {
            let (verb, keys) = match cmd {
                RedisCommand::Zdiff => ("ZDIFF", require_key_list(exchange, "ZDIFF")?),
                RedisCommand::Zinter => ("ZINTER", require_key_list(exchange, "ZINTER")?),
                _ => ("ZUNION", require_key_list(exchange, "ZUNION")?),
            };
            let mut command = redis::cmd(verb);
            command.arg(keys.len());
            for key in &keys {
                command.arg(key);
            }
            let reply: Value = command
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel(verb, e))?;
            let members = parse_member_array(reply, verb)?;
            // Member-array reply: the body itself carries the payload.
            exchange.input.body = member_array_reply_to_body(members);
            return Ok(());
        }
        RedisCommand::Zrangebylex => {
            let key = require_key(exchange)?;
            let (min, max) = resolve_lex_bounds(exchange)?;
            let reply: Value = redis::cmd("ZRANGEBYLEX")
                .arg(&key)
                .arg(&min)
                .arg(&max)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZRANGEBYLEX", e))?;
            let members = parse_member_array(reply, "ZRANGEBYLEX")?;
            // Member-array reply: the body itself carries the payload.
            exchange.input.body = member_array_reply_to_body(members);
            return Ok(());
        }
        _ => unreachable!("non-zset commands rejected above"),
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

    #[test]
    fn test_zadd_requires_key() {
        let ex = Exchange::new(Message::default());
        assert!(crate::commands::require_key(&ex).is_err());
    }

    #[test]
    fn test_zadd_has_key_score_and_body_member() {
        let mut msg = Message::new(Body::Text("member1".into()));
        msg.set_header("CamelRedis.Key", serde_json::json!("myzset"));
        msg.set_header("CamelRedis.Score", serde_json::json!(10.5));
        let ex = Exchange::new(msg);
        assert_eq!(crate::commands::require_key(&ex).unwrap(), "myzset");
        assert_eq!(
            crate::commands::get_f64_header(&ex, "CamelRedis.Score").unwrap(),
            Some(10.5)
        );
        assert_eq!(
            body_value_bytes(&ex, "ZADD", &[HEADER_VALUE]).unwrap(),
            b"member1".to_vec()
        );
    }

    #[test]
    fn test_zrange_with_score_header() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.Start", serde_json::json!(0)),
            ("CamelRedis.End", serde_json::json!(-1)),
            ("CamelRedis.WithScore", serde_json::json!(true)),
        ]);
        assert_eq!(
            crate::commands::get_bool_header(&ex, "CamelRedis.WithScore"),
            Some(true)
        );
        assert_eq!(
            crate::commands::get_i64_header(&ex, "CamelRedis.Start").unwrap(),
            Some(0)
        );
        assert_eq!(
            crate::commands::get_i64_header(&ex, "CamelRedis.End").unwrap(),
            Some(-1)
        );
    }

    #[test]
    fn test_zcount_with_min_max() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.Min", serde_json::json!(0.0)),
            ("CamelRedis.Max", serde_json::json!(100.0)),
        ]);
        assert_eq!(
            crate::commands::get_f64_header(&ex, "CamelRedis.Min").unwrap(),
            Some(0.0)
        );
        assert_eq!(
            crate::commands::get_f64_header(&ex, "CamelRedis.Max").unwrap(),
            Some(100.0)
        );
    }

    #[test]
    fn test_zunionstore_with_destination_and_keys() {
        let ex = ex_with(&[
            ("CamelRedis.Destination", serde_json::json!("dest")),
            ("CamelRedis.Keys", serde_json::json!(["zset1", "zset2"])),
        ]);
        assert_eq!(resolve_destination(&ex).unwrap(), "dest");
        assert_eq!(resolve_zstore_keys(&ex).unwrap(), vec!["zset1", "zset2"]);
    }

    #[test]
    fn test_zset_command_classification() {
        assert!(is_zset_command(&RedisCommand::Zadd));
        assert!(is_zset_command(&RedisCommand::Zinterstore));
        assert!(!is_zset_command(&RedisCommand::Set));
    }

    #[test]
    fn test_resolve_destination_requires_header() {
        let ex = Exchange::new(Message::default());
        let err = resolve_destination(&ex).expect_err("destination should be required");
        assert!(err.to_string().contains("CamelRedis.Destination"));
    }

    // REDIS-005: resolve_zstore_keys returns error when neither keys nor key header exists
    #[test]
    fn test_resolve_zstore_keys_falls_back_to_single_key_or_error() {
        // With Keys header
        let ex = ex_with(&[
            ("CamelRedis.Destination", serde_json::json!("dest")),
            ("CamelRedis.Keys", serde_json::json!(["zset1", "zset2"])),
        ]);
        assert_eq!(resolve_zstore_keys(&ex).unwrap(), vec!["zset1", "zset2"]);

        // With Key header (fallback)
        let ex_with_key = ex_with(&[
            ("CamelRedis.Destination", serde_json::json!("dest")),
            ("CamelRedis.Key", serde_json::json!("k1")),
        ]);
        assert_eq!(resolve_zstore_keys(&ex_with_key).unwrap(), vec!["k1"]);

        // REDIS-005: Missing both keys and key returns error
        let ex_missing = ex_with(&[("CamelRedis.Destination", serde_json::json!("dest"))]);
        let err = resolve_zstore_keys(&ex_missing).expect_err("missing keys should return error");
        assert!(err.to_string().contains("CamelRedis.Keys"));
        assert!(err.to_string().contains("CamelRedis.Key"));
    }

    #[test]
    fn test_resolve_range_bounds_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_range_bounds(&ex_default).unwrap(), (0, -1));

        let ex = ex_with(&[
            ("CamelRedis.Start", serde_json::json!(2)),
            ("CamelRedis.End", serde_json::json!(8)),
        ]);
        assert_eq!(resolve_range_bounds(&ex).unwrap(), (2, 8));

        let ex_str = ex_with(&[
            ("CamelRedis.Start", serde_json::json!(" 2 ")),
            ("CamelRedis.End", serde_json::json!(" 8 ")),
        ]);
        assert_eq!(resolve_range_bounds(&ex_str).unwrap(), (2, 8));
    }

    #[test]
    fn test_resolve_score_bounds_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        let (min, max) = resolve_score_bounds(&ex_default).unwrap();
        assert_eq!(min, f64::NEG_INFINITY);
        assert_eq!(max, f64::INFINITY);

        let ex = ex_with(&[
            ("CamelRedis.Min", serde_json::json!(1.5)),
            ("CamelRedis.Max", serde_json::json!(9.5)),
        ]);
        assert_eq!(resolve_score_bounds(&ex).unwrap(), (1.5, 9.5));

        let ex_str = ex_with(&[
            ("CamelRedis.Min", serde_json::json!(" 1.5 ")),
            ("CamelRedis.Max", serde_json::json!(" 9.5 ")),
        ]);
        assert_eq!(resolve_score_bounds(&ex_str).unwrap(), (1.5, 9.5));
    }

    #[test]
    fn test_resolve_score_bounds_nonfinite_is_named_error() {
        let ex = ex_with(&[("CamelRedis.Min", serde_json::json!("NaN"))]);
        let err = resolve_score_bounds(&ex).expect_err("NaN score bound must fail");
        let text = err.to_string();
        assert!(text.contains("CamelRedis.Min"), "{text}");
        assert!(text.contains("NaN"), "{text}");
    }

    #[test]
    fn test_resolve_with_scores_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        assert!(!resolve_with_scores(&ex_default));

        let ex = ex_with(&[("CamelRedis.WithScore", serde_json::json!(true))]);
        assert!(resolve_with_scores(&ex));
    }

    #[test]
    fn test_resolve_zadd_score_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_zadd_score(&ex_default).unwrap(), 0.0);

        let ex = ex_with(&[("CamelRedis.Score", serde_json::json!(3.25))]);
        assert_eq!(resolve_zadd_score(&ex).unwrap(), 3.25);

        let ex_str = ex_with(&[("CamelRedis.Score", serde_json::json!(" 3.25 "))]);
        assert_eq!(resolve_zadd_score(&ex_str).unwrap(), 3.25);
    }

    #[test]
    fn test_resolve_zincr_increment_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_zincr_increment(&ex_default).unwrap(), 1.0);

        let ex = ex_with(&[("CamelRedis.Increment", serde_json::json!(2.5))]);
        assert_eq!(resolve_zincr_increment(&ex).unwrap(), 2.5);
    }

    #[test]
    fn test_resolve_zremrange_rank_bounds_uses_range_defaults() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_zremrange_rank_bounds(&ex_default).unwrap(), (0, -1));

        let ex = ex_with(&[
            ("CamelRedis.Start", serde_json::json!(4)),
            ("CamelRedis.End", serde_json::json!(9)),
        ]);
        assert_eq!(resolve_zremrange_rank_bounds(&ex).unwrap(), (4, 9));
    }

    #[test]
    fn test_resolve_revrange_score_bounds_swaps_order() {
        let ex = ex_with(&[
            ("CamelRedis.Min", serde_json::json!(1.0)),
            ("CamelRedis.Max", serde_json::json!(8.0)),
        ]);
        assert_eq!(resolve_revrange_score_bounds(&ex).unwrap(), (8.0, 1.0));
    }

    #[test]
    fn test_resolve_zstore_operands_requires_destination() {
        let ex = ex_with(&[("CamelRedis.Keys", serde_json::json!(["k1", "k2"]))]);
        let err = resolve_zstore_operands(&ex).expect_err("destination should be required");
        assert!(err.to_string().contains("CamelRedis.Destination"));
    }

    #[test]
    fn test_json_from_optional_rank_and_score() {
        assert_eq!(json_from_optional_rank(Some(2)), serde_json::json!(2));
        assert_eq!(json_from_optional_rank(None), serde_json::Value::Null);
        assert_eq!(json_from_optional_score(Some(1.5)), serde_json::json!(1.5));
        assert_eq!(json_from_optional_score(None), serde_json::Value::Null);
    }

    #[test]
    fn test_json_from_scored_members_shape() {
        let json = json_from_scored_members(vec![("m1".to_string(), 1.0)]);
        assert_eq!(json, serde_json::json!([{"member": "m1", "score": 1.0}]));
    }

    #[test]
    fn resolve_order_by_domains() {
        let min = ex_with(&[("CamelRedis.OrderBy", serde_json::json!("MIN"))]);
        assert_eq!(resolve_order_by(&min).expect("MIN must resolve"), "MIN");

        let max = ex_with(&[("CamelRedis.OrderBy", serde_json::json!("max"))]);
        assert_eq!(
            resolve_order_by(&max).expect("lowercase max must resolve"),
            "MAX"
        );

        let invalid = ex_with(&[("CamelRedis.OrderBy", serde_json::json!("LEFT"))]);
        let err = resolve_order_by(&invalid).expect_err("LEFT is not a score order");
        let text = err.to_string();
        assert!(text.contains("CamelRedis.OrderBy"), "{text}");
        assert!(text.contains("MIN") && text.contains("MAX"), "{text}");

        let missing = Exchange::new(Message::default());
        let err = resolve_order_by(&missing).expect_err("missing order must fail");
        let text = err.to_string();
        assert!(text.contains("CamelRedis.OrderBy"), "{text}");
        assert!(text.contains("MIN") && text.contains("MAX"), "{text}");
    }

    #[test]
    fn zmpop_wire_args_assembly() {
        let keys = vec!["a".to_string(), "b".to_string()];
        assert_eq!(
            zmpop_args(None, &keys, "MAX", Some(2)),
            vec!["2", "a", "b", "MAX", "COUNT", "2"]
        );
        assert_eq!(
            zmpop_args(None, &keys, "MAX", None),
            vec!["2", "a", "b", "MAX"]
        );
        assert_eq!(
            zmpop_args(Some(0), &keys, "MIN", Some(1)),
            vec!["0", "2", "a", "b", "MIN", "COUNT", "1"]
        );
    }

    #[test]
    fn zset_multi_key_requires_key_list() {
        let ex = ex_with(&[("CamelRedis.Keys", serde_json::json!([]))]);
        let err = require_key_list(&ex, "ZMPOP").expect_err("an empty key list must be refused");
        let msg = err.to_string();
        assert!(msg.contains("CamelRedis.Keys"), "{msg}");
        assert!(msg.contains("ZMPOP"), "{msg}");
    }

    #[test]
    fn zrangebylex_requires_both_bounds() {
        let min_only = ex_with(&[("CamelRedis.Min", serde_json::json!("[a"))]);
        let err = resolve_lex_bounds(&min_only).expect_err("min alone must fail");
        assert!(err.to_string().contains("CamelRedis.Max"), "{err}");

        let max_only = ex_with(&[("CamelRedis.Max", serde_json::json!("+"))]);
        let err = resolve_lex_bounds(&max_only).expect_err("max alone must fail");
        assert!(err.to_string().contains("CamelRedis.Min"), "{err}");

        let both = ex_with(&[
            ("CamelRedis.Min", serde_json::json!("[a")),
            ("CamelRedis.Max", serde_json::json!("+")),
        ]);
        assert_eq!(
            resolve_lex_bounds(&both).expect("both bounds must resolve"),
            ("[a".to_string(), "+".to_string())
        );
    }
}
