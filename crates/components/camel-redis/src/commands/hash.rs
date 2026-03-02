use redis::AsyncCommands;
use redis::aio::MultiplexedConnection;
use camel_api::{CamelError, Exchange, body::Body};
use crate::config::RedisCommand;
use super::{get_str_header, get_i64_header, get_str_vec_header, require_key, require_value};

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    let result: serde_json::Value = match cmd {
        RedisCommand::Hset => {
            let key = require_key(exchange)?;
            let field = get_str_header(exchange, "CamelRedis.Field")
                .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Field".into()))?;
            let value = require_value(exchange)?;
            let n: i64 = conn.hset(&key, field, value.to_string()).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis HSET failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Hget => {
            let key = require_key(exchange)?;
            let field = get_str_header(exchange, "CamelRedis.Field")
                .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Field".into()))?;
            let val: Option<String> = conn.hget(&key, field).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis HGET failed: {e}")))?;
            val.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null)
        }
        RedisCommand::Hsetnx => {
            let key = require_key(exchange)?;
            let field = get_str_header(exchange, "CamelRedis.Field")
                .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Field".into()))?;
            let value = require_value(exchange)?;
            let ok: bool = conn.hset_nx(&key, field, value.to_string()).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis HSETNX failed: {e}")))?;
            serde_json::json!(ok)
        }
        RedisCommand::Hmset => {
            let key = require_key(exchange)?;
            let values = exchange.input.header("CamelRedis.Values")
                .and_then(|v| v.as_object())
                .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Values".into()))?
                .iter()
                .map(|(f, v)| (f.clone(), v.to_string()))
                .collect::<Vec<_>>();
            conn.hset_multiple::<_, _, _, ()>(&key, &values).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis HMSET failed: {e}")))?;
            serde_json::Value::Null
        }
        RedisCommand::Hmget => {
            let key = require_key(exchange)?;
            let fields = get_str_vec_header(exchange, "CamelRedis.Fields")
                .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Fields".into()))?;
            // Use raw command for HMGET since AsyncCommands::hget only supports single field
            let mut cmd = redis::cmd("HMGET");
            cmd.arg(&key);
            for field in &fields {
                cmd.arg(field);
            }
            let vals: Vec<Option<String>> = cmd.query_async(conn).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis HMGET failed: {e}")))?;
            serde_json::json!(vals.into_iter()
                .map(|v| v.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null))
                .collect::<Vec<_>>())
        }
        RedisCommand::Hdel => {
            let key = require_key(exchange)?;
            let field = get_str_header(exchange, "CamelRedis.Field")
                .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Field".into()))?;
            let n: i64 = conn.hdel(&key, field).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis HDEL failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Hexists => {
            let key = require_key(exchange)?;
            let field = get_str_header(exchange, "CamelRedis.Field")
                .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Field".into()))?;
            let ok: bool = conn.hexists(&key, field).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis HEXISTS failed: {e}")))?;
            serde_json::json!(ok)
        }
        RedisCommand::Hlen => {
            let key = require_key(exchange)?;
            let n: i64 = conn.hlen(&key).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis HLEN failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Hkeys => {
            let key = require_key(exchange)?;
            let keys: Vec<String> = conn.hkeys(&key).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis HKEYS failed: {e}")))?;
            serde_json::json!(keys)
        }
        RedisCommand::Hvals => {
            let key = require_key(exchange)?;
            let vals: Vec<String> = conn.hvals(&key).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis HVALS failed: {e}")))?;
            serde_json::json!(vals)
        }
        RedisCommand::Hgetall => {
            let key = require_key(exchange)?;
            let map: std::collections::HashMap<String, String> = conn.hgetall(&key).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis HGETALL failed: {e}")))?;
            serde_json::json!(map)
        }
        RedisCommand::Hincrby => {
            let key = require_key(exchange)?;
            let field = get_str_header(exchange, "CamelRedis.Field")
                .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Field".into()))?;
            let by = get_i64_header(exchange, "CamelRedis.Increment").unwrap_or(1);
            let n: i64 = conn.hincr(&key, field, by).await
                .map_err(|e| CamelError::ProcessorError(format!("Redis HINCRBY failed: {e}")))?;
            serde_json::json!(n)
        }
        _ => return Err(CamelError::ProcessorError("Not a hash command".into())),
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

    #[test]
    fn test_hset_requires_key() {
        let ex = Exchange::new(Message::default());
        assert!(crate::commands::require_key(&ex).is_err());
    }

    #[test]
    fn test_hset_requires_field() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
        ]);
        // Field is extracted via get_str_header, so we test that it returns None
        assert!(crate::commands::get_str_header(&ex, "CamelRedis.Field").is_none());
    }

    #[test]
    fn test_hset_has_key_field_value() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Field", serde_json::json!("myfield")),
            ("CamelRedis.Value", serde_json::json!("myvalue")),
        ]);
        assert_eq!(crate::commands::require_key(&ex).unwrap(), "mykey");
        assert_eq!(crate::commands::get_str_header(&ex, "CamelRedis.Field"), Some("myfield"));
        assert_eq!(crate::commands::require_value(&ex).unwrap(), serde_json::json!("myvalue"));
    }

    #[test]
    fn test_hmget_fields_extraction() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Fields", serde_json::json!(["f1", "f2", "f3"])),
        ]);
        let fields = crate::commands::get_str_vec_header(&ex, "CamelRedis.Fields");
        assert_eq!(fields, Some(vec!["f1".to_string(), "f2".to_string(), "f3".to_string()]));
    }

    #[test]
    fn test_hincrby_increment_header() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Field", serde_json::json!("counter")),
            ("CamelRedis.Increment", serde_json::json!(5i64)),
        ]);
        assert_eq!(crate::commands::get_i64_header(&ex, "CamelRedis.Increment"), Some(5));
    }

    #[test]
    fn test_hmset_values_extraction() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Values", serde_json::json!({"f1": "v1", "f2": "v2"})),
        ]);
        let values = ex.input.header("CamelRedis.Values").and_then(|v| v.as_object());
        assert!(values.is_some());
        let obj = values.unwrap();
        assert_eq!(obj.len(), 2);
        assert!(obj.contains_key("f1"));
        assert!(obj.contains_key("f2"));
    }
}
