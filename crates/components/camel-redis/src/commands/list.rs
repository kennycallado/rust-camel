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
}
