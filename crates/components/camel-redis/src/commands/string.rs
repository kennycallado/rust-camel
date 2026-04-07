use super::{get_i64_header, get_str_vec_header, get_u64_header, require_key, require_value};
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

pub(crate) fn resolve_timeout_seconds(exchange: &Exchange) -> u64 {
    get_u64_header(exchange, "CamelRedis.Timeout").unwrap_or(0)
}

pub(crate) fn resolve_increment(exchange: &Exchange) -> i64 {
    get_i64_header(exchange, "CamelRedis.Increment").unwrap_or(1)
}

pub(crate) fn resolve_mget_keys(exchange: &Exchange) -> Result<Vec<String>, CamelError> {
    get_str_vec_header(exchange, "CamelRedis.Keys")
        .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Keys".into()))
}

pub(crate) fn resolve_mset_values(
    exchange: &Exchange,
) -> Result<Vec<(String, String)>, CamelError> {
    let values = exchange
        .input
        .header("CamelRedis.Values")
        .and_then(|v| v.as_object())
        .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Values".into()))?;

    Ok(values
        .iter()
        .map(|(k, v)| (k.clone(), v.to_string()))
        .collect::<Vec<_>>())
}

pub(crate) fn json_from_optional_string(value: Option<String>) -> serde_json::Value {
    value
        .map(serde_json::Value::String)
        .unwrap_or(serde_json::Value::Null)
}

pub(crate) fn json_array_from_optional_strings(values: Vec<Option<String>>) -> serde_json::Value {
    serde_json::Value::Array(values.into_iter().map(json_from_optional_string).collect())
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
            let value = require_value(exchange)?;
            conn.set::<_, _, ()>(&key, value.to_string())
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis SET failed: {e}")))?;
            serde_json::Value::Null
        }
        RedisCommand::Get => {
            let key = require_key(exchange)?;
            let val: Option<String> = conn
                .get(&key)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis GET failed: {e}")))?;
            json_from_optional_string(val)
        }
        RedisCommand::Getset => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let old: Option<String> = conn
                .getset(&key, value.to_string())
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis GETSET failed: {e}")))?;
            json_from_optional_string(old)
        }
        RedisCommand::Setnx => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let ok: bool = conn
                .set_nx(&key, value.to_string())
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis SETNX failed: {e}")))?;
            serde_json::Value::Bool(ok)
        }
        RedisCommand::Setex => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let ttl = resolve_timeout_seconds(exchange);
            conn.set_ex::<_, _, ()>(&key, value.to_string(), ttl)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis SETEX failed: {e}")))?;
            serde_json::Value::Null
        }
        RedisCommand::Mget => {
            let keys = resolve_mget_keys(exchange)?;
            let vals: Vec<Option<String>> = conn
                .mget(&keys)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis MGET failed: {e}")))?;
            json_array_from_optional_strings(vals)
        }
        RedisCommand::Mset => {
            let values = resolve_mset_values(exchange)?;
            conn.mset::<_, _, ()>(&values)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis MSET failed: {e}")))?;
            serde_json::Value::Null
        }
        RedisCommand::Incr => {
            let key = require_key(exchange)?;
            let n: i64 = conn
                .incr(&key, 1i64)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis INCR failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Incrby => {
            let key = require_key(exchange)?;
            let by = resolve_increment(exchange);
            let n: i64 = conn
                .incr(&key, by)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis INCRBY failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Decr => {
            let key = require_key(exchange)?;
            let n: i64 = conn
                .decr(&key, 1i64)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis DECR failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Decrby => {
            let key = require_key(exchange)?;
            let by = resolve_increment(exchange);
            let n: i64 = conn
                .decr(&key, by)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis DECRBY failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Append => {
            let key = require_key(exchange)?;
            let value = require_value(exchange)?;
            let n: i64 = conn
                .append(&key, value.to_string())
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis APPEND failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Strlen => {
            let key = require_key(exchange)?;
            let n: i64 = conn
                .strlen(&key)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis STRLEN failed: {e}")))?;
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
        assert_eq!(
            crate::commands::require_value(&ex).unwrap(),
            serde_json::json!("hello")
        );
    }

    #[test]
    fn test_string_command_classification() {
        assert!(is_string_command(&RedisCommand::Set));
        assert!(is_string_command(&RedisCommand::Append));
        assert!(!is_string_command(&RedisCommand::Sadd));
    }

    #[test]
    fn test_resolve_timeout_seconds_defaults_to_zero() {
        let ex = Exchange::new(Message::default());
        assert_eq!(resolve_timeout_seconds(&ex), 0);
    }

    #[test]
    fn test_resolve_timeout_seconds_from_header() {
        let ex = ex_with(&[("CamelRedis.Timeout", serde_json::json!(15))]);
        assert_eq!(resolve_timeout_seconds(&ex), 15);
    }

    #[test]
    fn test_resolve_mget_keys_requires_header() {
        let ex = Exchange::new(Message::default());
        let err = resolve_mget_keys(&ex).expect_err("keys should be required");
        assert!(err.to_string().contains("CamelRedis.Keys"));
    }

    #[test]
    fn test_resolve_mset_values_extracts_pairs() {
        let ex = ex_with(&[("CamelRedis.Values", serde_json::json!({"a": 1, "b": "x"}))]);
        let values = resolve_mset_values(&ex).expect("values should be present");
        assert_eq!(values.len(), 2);
        assert!(values.iter().any(|(k, _)| k == "a"));
        assert!(values.iter().any(|(k, _)| k == "b"));
    }

    #[test]
    fn test_resolve_increment_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_increment(&ex_default), 1);

        let ex = ex_with(&[("CamelRedis.Increment", serde_json::json!(7))]);
        assert_eq!(resolve_increment(&ex), 7);
    }

    #[test]
    fn test_resolve_mget_keys_returns_values() {
        let ex = ex_with(&[("CamelRedis.Keys", serde_json::json!(["a", "b"]))]);
        assert_eq!(resolve_mget_keys(&ex).unwrap(), vec!["a", "b"]);
    }

    #[test]
    fn test_resolve_mset_values_requires_header() {
        let ex = Exchange::new(Message::default());
        let err = resolve_mset_values(&ex).expect_err("values should be required");
        assert!(err.to_string().contains("CamelRedis.Values"));
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
