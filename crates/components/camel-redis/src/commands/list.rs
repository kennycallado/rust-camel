use redis::AsyncCommands;
use redis::aio::MultiplexedConnection;
use camel_api::{CamelError, Exchange, body::Body};
use crate::config::RedisCommand;
use super::{get_str_header, get_u64_header, get_i64_header, require_key, require_value};

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    let result: serde_json::Value = match cmd {
        RedisCommand::Lpush => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let n: i64 = conn.lpush(&key, value.to_string()).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LPUSH failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Rpush => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let n: i64 = conn.rpush(&key, value.to_string()).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis RPUSH failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Lpushx => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let n: i64 = conn.lpush_exists(&key, value.to_string()).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LPUSHX failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Rpushx => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let n: i64 = conn.rpush_exists(&key, value.to_string()).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis RPUSHX failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Lpop => {
            let key = require_key(exchange)?;
            let val: Option<String> = conn.lpop(&key, None).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LPOP failed: {e}")))?;
            val.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null)
        }
        RedisCommand::Rpop => {
            let key = require_key(exchange)?;
            let val: Option<String> = conn.rpop(&key, None).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis RPOP failed: {e}")))?;
            val.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null)
        }
        RedisCommand::Blpop => {
            let key = require_key(exchange)?;
            let timeout = get_u64_header(exchange, "CamelRedis.Timeout").unwrap_or(0) as f64;
            let val: Option<(String, String)> = conn.blpop(&key, timeout).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis BLPOP failed: {e}")))?;
            val.map(|(_, v)| serde_json::Value::String(v)).unwrap_or(serde_json::Value::Null)
        }
        RedisCommand::Brpop => {
            let key = require_key(exchange)?;
            let timeout = get_u64_header(exchange, "CamelRedis.Timeout").unwrap_or(0) as f64;
            let val: Option<(String, String)> = conn.brpop(&key, timeout).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis BRPOP failed: {e}")))?;
            val.map(|(_, v)| serde_json::Value::String(v)).unwrap_or(serde_json::Value::Null)
        }
        RedisCommand::Llen => {
            let key = require_key(exchange)?;
            let n: i64 = conn.llen(&key).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LLEN failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Lrange => {
            let key = require_key(exchange)?;
            let start = get_i64_header(exchange, "CamelRedis.Start").unwrap_or(0) as isize;
            let end = get_i64_header(exchange, "CamelRedis.End").unwrap_or(-1) as isize;
            let vals: Vec<String> = conn.lrange(&key, start, end).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LRANGE failed: {e}")))?;
            serde_json::json!(vals)
        }
        RedisCommand::Lindex => {
            let key = require_key(exchange)?;
            let idx = get_i64_header(exchange, "CamelRedis.Index").unwrap_or(0) as isize;
            let val: Option<String> = conn.lindex(&key, idx).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LINDEX failed: {e}")))?;
            val.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null)
        }
        RedisCommand::Linsert => {
            let key = require_key(exchange)?;
            let position = get_str_header(exchange, "CamelRedis.Position").unwrap_or("BEFORE");
            let pivot = get_str_header(exchange, "CamelRedis.Pivot")
                .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Pivot".into()))?;
            let value = require_value(exchange)?;
            // Use redis::cmd for LINSERT since AsyncCommands doesn't have a direct method
            let n: i64 = if position.eq_ignore_ascii_case("BEFORE") {
                redis::cmd("LINSERT")
                    .arg(&key)
                    .arg("BEFORE")
                    .arg(pivot)
                    .arg(value.to_string())
                    .query_async(conn)
                    .await
            } else {
                redis::cmd("LINSERT")
                    .arg(&key)
                    .arg("AFTER")
                    .arg(pivot)
                    .arg(value.to_string())
                    .query_async(conn)
                    .await
            }.map_err(|e| CamelError::ProcessorError(format!("Redis LINSERT failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Lset => {
            let key = require_key(exchange)?;
            let idx = get_i64_header(exchange, "CamelRedis.Index").unwrap_or(0) as isize;
            let value = require_value(exchange)?;
            conn.lset::<_, _, ()>(&key, idx, value.to_string()).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LSET failed: {e}")))?;
            serde_json::Value::Null
        }
        RedisCommand::Lrem => {
            let key = require_key(exchange)?;
            let count = get_i64_header(exchange, "CamelRedis.Count").unwrap_or(0) as isize;
            let value = require_value(exchange)?;
            let n: usize = conn.lrem(&key, count, value.to_string()).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LREM failed: {e}")))?;
            serde_json::json!(n as i64)
        }
        RedisCommand::Ltrim => {
            let key = require_key(exchange)?;
            let start = get_i64_header(exchange, "CamelRedis.Start").unwrap_or(0) as isize;
            let end = get_i64_header(exchange, "CamelRedis.End").unwrap_or(-1) as isize;
            conn.ltrim::<_, ()>(&key, start, end).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis LTRIM failed: {e}")))?;
            serde_json::Value::Null
        }
        RedisCommand::Rpoplpush => {
            let key = require_key(exchange)?;
            let dest = get_str_header(exchange, "CamelRedis.Destination")
                .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Destination".into()))?;
            let val: Option<String> = conn.rpoplpush(&key, dest).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis RPOPLPUSH failed: {e}")))?;
            val.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null)
        }
        _ => return Err(CamelError::ProcessorError("Not a list command".into())),
    };
    exchange.input.body = Body::Json(result);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{Exchange, Message};

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
        assert_eq!(crate::commands::require_value(&ex).unwrap(), serde_json::json!("hello"));
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
        assert_eq!(get_str_header(&ex, "CamelRedis.Destination"), Some("dest"));
    }
}
