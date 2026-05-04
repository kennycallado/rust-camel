use super::{get_i64_header, get_str_header, get_u64_header, require_key, require_value};
use crate::config::RedisCommand;
use camel_component_api::{Body, CamelError, Exchange};
use redis::AsyncCommands;
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

pub(crate) fn resolve_blocking_timeout(exchange: &Exchange) -> f64 {
    get_u64_header(exchange, "CamelRedis.Timeout").unwrap_or(0) as f64
}

pub(crate) fn resolve_index(exchange: &Exchange) -> isize {
    get_i64_header(exchange, "CamelRedis.Index").unwrap_or(0) as isize
}

pub(crate) fn resolve_range_bounds(exchange: &Exchange) -> (isize, isize) {
    (
        get_i64_header(exchange, "CamelRedis.Start").unwrap_or(0) as isize,
        get_i64_header(exchange, "CamelRedis.End").unwrap_or(-1) as isize,
    )
}

pub(crate) fn resolve_lrem_count(exchange: &Exchange) -> isize {
    get_i64_header(exchange, "CamelRedis.Count").unwrap_or(0) as isize
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

#[allow(dead_code)]
pub(crate) fn build_redis_cmd(
    cmd: &RedisCommand,
    exchange: &Exchange,
) -> Result<redis::Cmd, CamelError> {
    if !is_list_command(cmd) {
        return Err(CamelError::ProcessorError("Not a list command".into()));
    }

    match cmd {
        RedisCommand::Lpush => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let mut c = redis::cmd("LPUSH");
            c.arg(key).arg(value.to_string());
            Ok(c)
        }
        RedisCommand::Rpush => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let mut c = redis::cmd("RPUSH");
            c.arg(key).arg(value.to_string());
            Ok(c)
        }
        RedisCommand::Lpushx => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let mut c = redis::cmd("LPUSHX");
            c.arg(key).arg(value.to_string());
            Ok(c)
        }
        RedisCommand::Rpushx => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let mut c = redis::cmd("RPUSHX");
            c.arg(key).arg(value.to_string());
            Ok(c)
        }
        RedisCommand::Lpop => {
            let key = require_key(exchange)?;
            let mut c = redis::cmd("LPOP");
            c.arg(key);
            Ok(c)
        }
        RedisCommand::Rpop => {
            let key = require_key(exchange)?;
            let mut c = redis::cmd("RPOP");
            c.arg(key);
            Ok(c)
        }
        RedisCommand::Blpop => {
            let key = require_key(exchange)?;
            let timeout = resolve_blocking_timeout(exchange);
            let mut c = redis::cmd("BLPOP");
            c.arg(key).arg(timeout);
            Ok(c)
        }
        RedisCommand::Brpop => {
            let key = require_key(exchange)?;
            let timeout = resolve_blocking_timeout(exchange);
            let mut c = redis::cmd("BRPOP");
            c.arg(key).arg(timeout);
            Ok(c)
        }
        RedisCommand::Llen => {
            let key = require_key(exchange)?;
            let mut c = redis::cmd("LLEN");
            c.arg(key);
            Ok(c)
        }
        RedisCommand::Lrange => {
            let key = require_key(exchange)?;
            let (start, end) = resolve_range_bounds(exchange);
            let mut c = redis::cmd("LRANGE");
            c.arg(key).arg(start).arg(end);
            Ok(c)
        }
        RedisCommand::Lindex => {
            let key = require_key(exchange)?;
            let idx = resolve_index(exchange);
            let mut c = redis::cmd("LINDEX");
            c.arg(key).arg(idx);
            Ok(c)
        }
        RedisCommand::Linsert => {
            let key = require_key(exchange)?;
            let (mode, pivot) = resolve_linsert_operands(exchange)?;
            let value = require_value(exchange)?;
            let mut c = redis::cmd("LINSERT");
            c.arg(key).arg(mode).arg(pivot).arg(value.to_string());
            Ok(c)
        }
        RedisCommand::Lset => {
            let key = require_key(exchange)?;
            let idx = resolve_index(exchange);
            let value = require_value(exchange)?;
            let mut c = redis::cmd("LSET");
            c.arg(key).arg(idx).arg(value.to_string());
            Ok(c)
        }
        RedisCommand::Lrem => {
            let key = require_key(exchange)?;
            let count = resolve_lrem_count(exchange);
            let value = require_value(exchange)?;
            let mut c = redis::cmd("LREM");
            c.arg(key).arg(count).arg(value.to_string());
            Ok(c)
        }
        RedisCommand::Ltrim => {
            let key = require_key(exchange)?;
            let (start, end) = resolve_range_bounds(exchange);
            let mut c = redis::cmd("LTRIM");
            c.arg(key).arg(start).arg(end);
            Ok(c)
        }
        RedisCommand::Rpoplpush => {
            let key = require_key(exchange)?;
            let dest = resolve_destination(exchange)?;
            let mut c = redis::cmd("RPOPLPUSH");
            c.arg(key).arg(dest);
            Ok(c)
        }
        _ => unreachable!("non-list commands rejected above"),
    }
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
            let value = require_value(exchange)?;
            let n: i64 = conn
                .lpush(&key, value.to_string())
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LPUSH failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Rpush => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let n: i64 = conn
                .rpush(&key, value.to_string())
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis RPUSH failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Lpushx => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let n: i64 = conn
                .lpush_exists(&key, value.to_string())
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LPUSHX failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Rpushx => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let n: i64 = conn
                .rpush_exists(&key, value.to_string())
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis RPUSHX failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Lpop => {
            let key = require_key(exchange)?;
            let val: Option<String> = conn
                .lpop(&key, None)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LPOP failed: {e}")))?;
            json_from_optional_string(val)
        }
        RedisCommand::Rpop => {
            let key = require_key(exchange)?;
            let val: Option<String> = conn
                .rpop(&key, None)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis RPOP failed: {e}")))?;
            json_from_optional_string(val)
        }
        RedisCommand::Blpop => {
            let key = require_key(exchange)?;
            let timeout = resolve_blocking_timeout(exchange);
            let val: Option<(String, String)> = conn
                .blpop(&key, timeout)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis BLPOP failed: {e}")))?;
            json_from_optional_pair_value(val)
        }
        RedisCommand::Brpop => {
            let key = require_key(exchange)?;
            let timeout = resolve_blocking_timeout(exchange);
            let val: Option<(String, String)> = conn
                .brpop(&key, timeout)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis BRPOP failed: {e}")))?;
            json_from_optional_pair_value(val)
        }
        RedisCommand::Llen => {
            let key = require_key(exchange)?;
            let n: i64 = conn
                .llen(&key)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LLEN failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Lrange => {
            let key = require_key(exchange)?;
            let (start, end) = resolve_range_bounds(exchange);
            let vals: Vec<String> = conn
                .lrange(&key, start, end)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LRANGE failed: {e}")))?;
            serde_json::json!(vals)
        }
        RedisCommand::Lindex => {
            let key = require_key(exchange)?;
            let idx = resolve_index(exchange);
            let val: Option<String> = conn
                .lindex(&key, idx)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LINDEX failed: {e}")))?;
            json_from_optional_string(val)
        }
        RedisCommand::Linsert => {
            let key = require_key(exchange)?;
            let (mode, pivot) = resolve_linsert_operands(exchange)?;
            let value = require_value(exchange)?;
            // Use redis::cmd for LINSERT since AsyncCommands doesn't have a direct method
            let n: i64 = redis::cmd("LINSERT")
                .arg(&key)
                .arg(mode)
                .arg(pivot)
                .arg(value.to_string())
                .query_async(conn)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LINSERT failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Lset => {
            let key = require_key(exchange)?;
            let idx = resolve_index(exchange);
            let value = require_value(exchange)?;
            conn.lset::<_, _, ()>(&key, idx, value.to_string())
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LSET failed: {e}")))?;
            serde_json::Value::Null
        }
        RedisCommand::Lrem => {
            let key = require_key(exchange)?;
            let count = resolve_lrem_count(exchange);
            let value = require_value(exchange)?;
            let n: usize = conn
                .lrem(&key, count, value.to_string())
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LREM failed: {e}")))?;
            serde_json::json!(n as i64)
        }
        RedisCommand::Ltrim => {
            let key = require_key(exchange)?;
            let (start, end) = resolve_range_bounds(exchange);
            conn.ltrim::<_, ()>(&key, start, end)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LTRIM failed: {e}")))?;
            serde_json::Value::Null
        }
        RedisCommand::Rpoplpush => {
            let key = require_key(exchange)?;
            let dest = resolve_destination(exchange)?;
            let val: Option<String> = conn
                .rpoplpush(&key, dest)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis RPOPLPUSH failed: {e}")))?;
            json_from_optional_string(val)
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
    fn test_lpush_has_key_and_value() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Key", serde_json::json!("mykey"));
        msg.set_header("CamelRedis.Value", serde_json::json!("hello"));
        let ex = Exchange::new(msg);
        assert_eq!(crate::commands::require_key(&ex).unwrap(), "mykey");
        assert_eq!(
            crate::commands::require_value(&ex).unwrap(),
            serde_json::json!("hello")
        );
    }

    #[test]
    fn test_lrange_has_start_and_end() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Key", serde_json::json!("mylist"));
        msg.set_header("CamelRedis.Start", serde_json::json!(0));
        msg.set_header("CamelRedis.End", serde_json::json!(-1));
        let ex = Exchange::new(msg);
        assert_eq!(get_i64_header(&ex, "CamelRedis.Start"), Some(0));
        assert_eq!(get_i64_header(&ex, "CamelRedis.End"), Some(-1));
    }

    #[test]
    fn test_linsert_has_pivot_and_position() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Key", serde_json::json!("mylist"));
        msg.set_header("CamelRedis.Pivot", serde_json::json!("pivot_value"));
        msg.set_header("CamelRedis.Position", serde_json::json!("AFTER"));
        msg.set_header("CamelRedis.Value", serde_json::json!("new_value"));
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
        assert_eq!(get_u64_header(&ex, "CamelRedis.Timeout"), Some(5));
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
        assert_eq!(resolve_blocking_timeout(&ex_default), 0.0);

        let mut msg = Message::default();
        msg.set_header("CamelRedis.Timeout", serde_json::json!(7));
        let ex = Exchange::new(msg);
        assert_eq!(resolve_blocking_timeout(&ex), 7.0);
    }

    #[test]
    fn test_resolve_index_default_and_value() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_index(&ex_default), 0);

        let mut msg = Message::default();
        msg.set_header("CamelRedis.Index", serde_json::json!(4));
        let ex = Exchange::new(msg);
        assert_eq!(resolve_index(&ex), 4);
    }

    #[test]
    fn test_resolve_range_bounds_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_range_bounds(&ex_default), (0, -1));

        let mut msg = Message::default();
        msg.set_header("CamelRedis.Start", serde_json::json!(2));
        msg.set_header("CamelRedis.End", serde_json::json!(5));
        let ex = Exchange::new(msg);
        assert_eq!(resolve_range_bounds(&ex), (2, 5));
    }

    #[test]
    fn test_resolve_lrem_count_default_and_value() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_lrem_count(&ex_default), 0);

        let mut msg = Message::default();
        msg.set_header("CamelRedis.Count", serde_json::json!(3));
        let ex = Exchange::new(msg);
        assert_eq!(resolve_lrem_count(&ex), 3);
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

    fn ex_with(headers: &[(&str, serde_json::Value)]) -> Exchange {
        let mut msg = Message::default();
        for (k, v) in headers {
            msg.set_header(*k, v.clone());
        }
        Exchange::new(msg)
    }

    fn cmd_args(cmd: &redis::Cmd) -> Vec<String> {
        cmd.args_iter()
            .map(|arg| match arg {
                redis::Arg::Simple(bytes) => String::from_utf8(bytes.to_vec()).unwrap(),
                redis::Arg::Cursor => "CURSOR".to_string(),
                _ => unreachable!(),
            })
            .collect()
    }

    #[test]
    fn test_build_redis_cmd_lpush() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Value", serde_json::json!("hello")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Lpush, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "LPUSH");
        assert_eq!(args[1], "mykey");
        assert_eq!(args[2], serde_json::json!("hello").to_string());
    }

    #[test]
    fn test_build_redis_cmd_rpush() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Value", serde_json::json!("world")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Rpush, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "RPUSH");
        assert_eq!(args[1], "mykey");
        assert_eq!(args[2], serde_json::json!("world").to_string());
    }

    #[test]
    fn test_build_redis_cmd_lpushx() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Value", serde_json::json!("val")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Lpushx, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "LPUSHX");
        assert_eq!(args[1], "mykey");
        assert_eq!(args[2], serde_json::json!("val").to_string());
    }

    #[test]
    fn test_build_redis_cmd_rpushx() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Value", serde_json::json!("val")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Rpushx, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "RPUSHX");
        assert_eq!(args[1], "mykey");
        assert_eq!(args[2], serde_json::json!("val").to_string());
    }

    #[test]
    fn test_build_redis_cmd_lpop() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
        let cmd = build_redis_cmd(&RedisCommand::Lpop, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "LPOP");
        assert_eq!(args[1], "mykey");
    }

    #[test]
    fn test_build_redis_cmd_rpop() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
        let cmd = build_redis_cmd(&RedisCommand::Rpop, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "RPOP");
        assert_eq!(args[1], "mykey");
    }

    #[test]
    fn test_build_redis_cmd_blpop() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Timeout", serde_json::json!(5)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Blpop, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "BLPOP");
        assert_eq!(args[1], "mykey");
        assert_eq!(args[2], "5.0");
    }

    #[test]
    fn test_build_redis_cmd_brpop() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Timeout", serde_json::json!(10)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Brpop, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "BRPOP");
        assert_eq!(args[1], "mykey");
        assert_eq!(args[2], "10.0");
    }

    #[test]
    fn test_build_redis_cmd_llen() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
        let cmd = build_redis_cmd(&RedisCommand::Llen, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "LLEN");
        assert_eq!(args[1], "mykey");
    }

    #[test]
    fn test_build_redis_cmd_lrange() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Start", serde_json::json!(0)),
            ("CamelRedis.End", serde_json::json!(-1)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Lrange, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "LRANGE");
        assert_eq!(args[1], "mykey");
        assert_eq!(args[2], "0");
        assert_eq!(args[3], "-1");
    }

    #[test]
    fn test_build_redis_cmd_lrange_defaults() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
        let cmd = build_redis_cmd(&RedisCommand::Lrange, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "LRANGE");
        assert_eq!(args[1], "mykey");
        assert_eq!(args[2], "0");
        assert_eq!(args[3], "-1");
    }

    #[test]
    fn test_build_redis_cmd_lindex() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Index", serde_json::json!(3)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Lindex, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "LINDEX");
        assert_eq!(args[1], "mykey");
        assert_eq!(args[2], "3");
    }

    #[test]
    fn test_build_redis_cmd_lindex_defaults() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
        let cmd = build_redis_cmd(&RedisCommand::Lindex, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "LINDEX");
        assert_eq!(args[1], "mykey");
        assert_eq!(args[2], "0");
    }

    #[test]
    fn test_build_redis_cmd_linsert() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Pivot", serde_json::json!("pivot")),
            ("CamelRedis.Position", serde_json::json!("AFTER")),
            ("CamelRedis.Value", serde_json::json!("new_val")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Linsert, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "LINSERT");
        assert_eq!(args[1], "mykey");
        assert_eq!(args[2], "AFTER");
        assert_eq!(args[3], "pivot");
        assert_eq!(args[4], serde_json::json!("new_val").to_string());
    }

    #[test]
    fn test_build_redis_cmd_linsert_defaults() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Pivot", serde_json::json!("pivot")),
            ("CamelRedis.Value", serde_json::json!("new_val")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Linsert, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "LINSERT");
        assert_eq!(args[1], "mykey");
        assert_eq!(args[2], "BEFORE");
        assert_eq!(args[3], "pivot");
        assert_eq!(args[4], serde_json::json!("new_val").to_string());
    }

    #[test]
    fn test_build_redis_cmd_lset() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Index", serde_json::json!(2)),
            ("CamelRedis.Value", serde_json::json!("replacement")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Lset, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "LSET");
        assert_eq!(args[1], "mykey");
        assert_eq!(args[2], "2");
        assert_eq!(args[3], serde_json::json!("replacement").to_string());
    }

    #[test]
    fn test_build_redis_cmd_lrem() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Count", serde_json::json!(3)),
            ("CamelRedis.Value", serde_json::json!("to_remove")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Lrem, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "LREM");
        assert_eq!(args[1], "mykey");
        assert_eq!(args[2], "3");
        assert_eq!(args[3], serde_json::json!("to_remove").to_string());
    }

    #[test]
    fn test_build_redis_cmd_lrem_defaults() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Value", serde_json::json!("to_remove")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Lrem, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "LREM");
        assert_eq!(args[1], "mykey");
        assert_eq!(args[2], "0");
        assert_eq!(args[3], serde_json::json!("to_remove").to_string());
    }

    #[test]
    fn test_build_redis_cmd_ltrim() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Start", serde_json::json!(1)),
            ("CamelRedis.End", serde_json::json!(5)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Ltrim, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "LTRIM");
        assert_eq!(args[1], "mykey");
        assert_eq!(args[2], "1");
        assert_eq!(args[3], "5");
    }

    #[test]
    fn test_build_redis_cmd_ltrim_defaults() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
        let cmd = build_redis_cmd(&RedisCommand::Ltrim, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "LTRIM");
        assert_eq!(args[1], "mykey");
        assert_eq!(args[2], "0");
        assert_eq!(args[3], "-1");
    }

    #[test]
    fn test_build_redis_cmd_rpoplpush() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("src")),
            ("CamelRedis.Destination", serde_json::json!("dest")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Rpoplpush, &ex).unwrap();
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "RPOPLPUSH");
        assert_eq!(args[1], "src");
        assert_eq!(args[2], "dest");
    }

    #[test]
    fn test_build_redis_cmd_not_a_list_command() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Value", serde_json::json!("hello")),
        ]);
        let err = build_redis_cmd(&RedisCommand::Set, &ex).unwrap_err();
        assert!(err.to_string().contains("Not a list command"));
    }

    #[test]
    fn test_build_redis_cmd_missing_key() {
        let ex = ex_with(&[("CamelRedis.Value", serde_json::json!("hello"))]);
        let err = build_redis_cmd(&RedisCommand::Lpush, &ex).unwrap_err();
        assert!(err.to_string().contains("CamelRedis.Key"));
    }

    #[test]
    fn test_build_redis_cmd_missing_value() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
        let err = build_redis_cmd(&RedisCommand::Lpush, &ex).unwrap_err();
        assert!(err.to_string().contains("CamelRedis.Value"));
    }

    #[test]
    fn test_build_redis_cmd_missing_destination() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("src"))]);
        let err = build_redis_cmd(&RedisCommand::Rpoplpush, &ex).unwrap_err();
        assert!(err.to_string().contains("CamelRedis.Destination"));
    }

    #[test]
    fn test_build_redis_cmd_missing_pivot() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Value", serde_json::json!("new_val")),
        ]);
        let err = build_redis_cmd(&RedisCommand::Linsert, &ex).unwrap_err();
        assert!(err.to_string().contains("CamelRedis.Pivot"));
    }
}
