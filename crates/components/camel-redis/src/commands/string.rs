use redis::AsyncCommands;
use redis::aio::MultiplexedConnection;
use camel_api::{CamelError, Exchange, body::Body};
use crate::config::RedisCommand;
use super::{get_u64_header, get_i64_header, get_str_vec_header, require_key, require_value};

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    let result: serde_json::Value = match cmd {
        RedisCommand::Set => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            conn.set::<_, _, ()>(&key, value.to_string()).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis SET failed: {e}")))?;
            serde_json::Value::Null
        }
        RedisCommand::Get => {
            let key = require_key(exchange)?;
            let val: Option<String> = conn.get(&key).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis GET failed: {e}")))?;
            val.map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null)
        }
        RedisCommand::Getset => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let old: Option<String> = conn.getset(&key, value.to_string()).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis GETSET failed: {e}")))?;
            old.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null)
        }
        RedisCommand::Setnx => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let ok: bool = conn.set_nx(&key, value.to_string()).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis SETNX failed: {e}")))?;
            serde_json::Value::Bool(ok)
        }
        RedisCommand::Setex => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let ttl = get_u64_header(exchange, "CamelRedis.Timeout").unwrap_or(0);
            conn.set_ex::<_, _, ()>(&key, value.to_string(), ttl).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis SETEX failed: {e}")))?;
            serde_json::Value::Null
        }
        RedisCommand::Mget => {
            let keys = get_str_vec_header(exchange, "CamelRedis.Keys")
                .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Keys".into()))?;
            let vals: Vec<Option<String>> = conn.mget(&keys).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis MGET failed: {e}")))?;
            serde_json::Value::Array(vals.into_iter()
                .map(|v| v.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null))
                .collect())
        }
        RedisCommand::Mset => {
            let values = exchange.input.header("CamelRedis.Values")
                .and_then(|v| v.as_object())
                .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Values".into()))?
                .iter()
                .map(|(k, v)| (k.clone(), v.to_string()))
                .collect::<Vec<_>>();
            conn.mset::<_, _, ()>(&values).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis MSET failed: {e}")))?;
            serde_json::Value::Null
        }
        RedisCommand::Incr => {
            let key = require_key(exchange)?;
            let n: i64 = conn.incr(&key, 1i64).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis INCR failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Incrby => {
            let key = require_key(exchange)?;
            let by = get_i64_header(exchange, "CamelRedis.Increment").unwrap_or(1);
            let n: i64 = conn.incr(&key, by).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis INCRBY failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Decr => {
            let key = require_key(exchange)?;
            let n: i64 = conn.decr(&key, 1i64).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis DECR failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Decrby => {
            let key = require_key(exchange)?;
            let by = get_i64_header(exchange, "CamelRedis.Increment").unwrap_or(1);
            let n: i64 = conn.decr(&key, by).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis DECRBY failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Append => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let n: i64 = conn.append(&key, value.to_string()).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis APPEND failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Strlen => {
            let key = require_key(exchange)?;
            let n: i64 = conn.strlen(&key).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis STRLEN failed: {e}")))?;
            serde_json::json!(n)
        }
        _ => return Err(CamelError::ProcessorError("Not a string command".into())),
    };
    exchange.input.body = Body::Json(result);
    Ok(())
}

#[cfg(test)]
mod tests {
    use camel_api::{Exchange, Message};

    fn ex_with(headers: &[(&str, serde_json::Value)]) -> Exchange {
        let mut msg = Message::default();
        for (k, v) in headers {
            msg.set_header(*k, v.clone());
        }
        Exchange::new(msg)
    }

    // Test that dispatch returns Err when key is missing (no real Redis needed)
    #[tokio::test]
    async fn test_set_missing_key_returns_err() {
        // We can't call dispatch without a real connection, but we can test
        // header extraction logic via require_key/require_value directly.
        let ex = Exchange::new(Message::default());
        assert!(crate::commands::require_key(&ex).is_err());
    }

    #[test]
    fn test_set_has_key_and_value() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Value", serde_json::json!("hello")),
        ]);
        assert_eq!(crate::commands::require_key(&ex).unwrap(), "mykey");
        assert_eq!(crate::commands::require_value(&ex).unwrap(), serde_json::json!("hello"));
    }
}
