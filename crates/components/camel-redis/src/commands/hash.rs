use super::{get_i64_header, get_str_header, get_str_vec_header, require_key, require_value};
use crate::config::RedisCommand;
use camel_component_api::{Body, CamelError, Exchange};
use redis::AsyncCommands;
use redis::aio::MultiplexedConnection;

pub(crate) fn is_hash_command(cmd: &RedisCommand) -> bool {
    matches!(
        cmd,
        RedisCommand::Hset
            | RedisCommand::Hget
            | RedisCommand::Hsetnx
            | RedisCommand::Hmset
            | RedisCommand::Hmget
            | RedisCommand::Hdel
            | RedisCommand::Hexists
            | RedisCommand::Hlen
            | RedisCommand::Hkeys
            | RedisCommand::Hvals
            | RedisCommand::Hgetall
            | RedisCommand::Hincrby
    )
}

pub(crate) fn resolve_hash_field(exchange: &Exchange) -> Result<String, CamelError> {
    get_str_header(exchange, "CamelRedis.Field")
        .map(|s| s.to_string())
        .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Field".into()))
}

pub(crate) fn resolve_hash_fields(exchange: &Exchange) -> Result<Vec<String>, CamelError> {
    get_str_vec_header(exchange, "CamelRedis.Fields")
        .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Fields".into()))
}

pub(crate) fn resolve_hash_values_map(
    exchange: &Exchange,
) -> Result<Vec<(String, String)>, CamelError> {
    let values = exchange
        .input
        .header("CamelRedis.Values")
        .and_then(|v| v.as_object())
        .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Values".into()))?;

    Ok(values
        .iter()
        .map(|(f, v)| (f.clone(), v.to_string()))
        .collect::<Vec<_>>())
}

pub(crate) fn resolve_hash_increment(exchange: &Exchange) -> i64 {
    get_i64_header(exchange, "CamelRedis.Increment").unwrap_or(1)
}

pub(crate) fn resolve_hash_field_operands(
    exchange: &Exchange,
) -> Result<(String, String), CamelError> {
    Ok((require_key(exchange)?, resolve_hash_field(exchange)?))
}

pub(crate) fn json_from_optional_hash_value(value: Option<String>) -> serde_json::Value {
    value
        .map(serde_json::Value::String)
        .unwrap_or(serde_json::Value::Null)
}

pub(crate) fn json_from_optional_hash_values(values: Vec<Option<String>>) -> serde_json::Value {
    serde_json::json!(
        values
            .into_iter()
            .map(json_from_optional_hash_value)
            .collect::<Vec<_>>()
    )
}

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    if !is_hash_command(cmd) {
        return Err(CamelError::ProcessorError("Not a hash command".into()));
    }

    let result: serde_json::Value =
        match cmd {
            RedisCommand::Hset => {
                let key = require_key(exchange)?;
                let field = resolve_hash_field(exchange)?;
                let value = require_value(exchange)?;
                let n: i64 = conn
                    .hset(&key, field, value.to_string())
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis HSET failed: {e}")))?;
                serde_json::json!(n)
            }
            RedisCommand::Hget => {
                let (key, field) = resolve_hash_field_operands(exchange)?;
                let val: Option<String> = conn
                    .hget(&key, field)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis HGET failed: {e}")))?;
                json_from_optional_hash_value(val)
            }
            RedisCommand::Hsetnx => {
                let key = require_key(exchange)?;
                let field = resolve_hash_field(exchange)?;
                let value = require_value(exchange)?;
                let ok: bool = conn
                    .hset_nx(&key, field, value.to_string())
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis HSETNX failed: {e}")))?;
                serde_json::json!(ok)
            }
            RedisCommand::Hmset => {
                let key = require_key(exchange)?;
                let values = resolve_hash_values_map(exchange)?;
                conn.hset_multiple::<_, _, _, ()>(&key, &values)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis HMSET failed: {e}")))?;
                serde_json::Value::Null
            }
            RedisCommand::Hmget => {
                let key = require_key(exchange)?;
                let fields = resolve_hash_fields(exchange)?;
                // Use raw command for HMGET since AsyncCommands::hget only supports single field
                let mut cmd = redis::cmd("HMGET");
                cmd.arg(&key);
                for field in &fields {
                    cmd.arg(field);
                }
                let vals: Vec<Option<String>> = cmd
                    .query_async(conn)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis HMGET failed: {e}")))?;
                json_from_optional_hash_values(vals)
            }
            RedisCommand::Hdel => {
                let (key, field) = resolve_hash_field_operands(exchange)?;
                let n: i64 = conn
                    .hdel(&key, field)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis HDEL failed: {e}")))?;
                serde_json::json!(n)
            }
            RedisCommand::Hexists => {
                let (key, field) = resolve_hash_field_operands(exchange)?;
                let ok: bool = conn.hexists(&key, field).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis HEXISTS failed: {e}"))
                })?;
                serde_json::json!(ok)
            }
            RedisCommand::Hlen => {
                let key = require_key(exchange)?;
                let n: i64 = conn
                    .hlen(&key)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis HLEN failed: {e}")))?;
                serde_json::json!(n)
            }
            RedisCommand::Hkeys => {
                let key = require_key(exchange)?;
                let keys: Vec<String> = conn
                    .hkeys(&key)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis HKEYS failed: {e}")))?;
                serde_json::json!(keys)
            }
            RedisCommand::Hvals => {
                let key = require_key(exchange)?;
                let vals: Vec<String> = conn
                    .hvals(&key)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis HVALS failed: {e}")))?;
                serde_json::json!(vals)
            }
            RedisCommand::Hgetall => {
                let key = require_key(exchange)?;
                let map: std::collections::HashMap<String, String> =
                    conn.hgetall(&key).await.map_err(|e| {
                        CamelError::ProcessorError(format!("Redis HGETALL failed: {e}"))
                    })?;
                serde_json::json!(map)
            }
            RedisCommand::Hincrby => {
                let (key, field) = resolve_hash_field_operands(exchange)?;
                let by = resolve_hash_increment(exchange);
                let n: i64 = conn.hincr(&key, field, by).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis HINCRBY failed: {e}"))
                })?;
                serde_json::json!(n)
            }
            _ => unreachable!("non-hash commands rejected above"),
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
    fn test_hset_requires_key() {
        let ex = Exchange::new(Message::default());
        assert!(crate::commands::require_key(&ex).is_err());
    }

    #[test]
    fn test_hset_requires_field() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
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
        assert_eq!(
            crate::commands::get_str_header(&ex, "CamelRedis.Field"),
            Some("myfield")
        );
        assert_eq!(
            crate::commands::require_value(&ex).unwrap(),
            serde_json::json!("myvalue")
        );
    }

    #[test]
    fn test_hmget_fields_extraction() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Fields", serde_json::json!(["f1", "f2", "f3"])),
        ]);
        let fields = crate::commands::get_str_vec_header(&ex, "CamelRedis.Fields");
        assert_eq!(
            fields,
            Some(vec!["f1".to_string(), "f2".to_string(), "f3".to_string()])
        );
    }

    #[test]
    fn test_hincrby_increment_header() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Field", serde_json::json!("counter")),
            ("CamelRedis.Increment", serde_json::json!(5i64)),
        ]);
        assert_eq!(
            crate::commands::get_i64_header(&ex, "CamelRedis.Increment"),
            Some(5)
        );
    }

    #[test]
    fn test_hmset_values_extraction() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            (
                "CamelRedis.Values",
                serde_json::json!({"f1": "v1", "f2": "v2"}),
            ),
        ]);
        let values = resolve_hash_values_map(&ex).expect("values should be present");
        assert_eq!(values.len(), 2);
        assert!(values.iter().any(|(k, _)| k == "f1"));
        assert!(values.iter().any(|(k, _)| k == "f2"));
    }

    #[test]
    fn test_hash_command_classification() {
        assert!(is_hash_command(&RedisCommand::Hset));
        assert!(is_hash_command(&RedisCommand::Hincrby));
        assert!(!is_hash_command(&RedisCommand::Set));
    }

    #[test]
    fn test_resolve_hash_field_requires_header() {
        let ex = Exchange::new(Message::default());
        let err = resolve_hash_field(&ex).expect_err("field should be required");
        assert!(err.to_string().contains("CamelRedis.Field"));
    }

    #[test]
    fn test_resolve_hash_fields_requires_header() {
        let ex = Exchange::new(Message::default());
        let err = resolve_hash_fields(&ex).expect_err("fields should be required");
        assert!(err.to_string().contains("CamelRedis.Fields"));
    }

    #[test]
    fn test_resolve_hash_increment_default_and_value() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_hash_increment(&ex_default), 1);

        let mut msg = Message::default();
        msg.set_header("CamelRedis.Increment", serde_json::json!(9));
        let ex = Exchange::new(msg);
        assert_eq!(resolve_hash_increment(&ex), 9);
    }

    #[test]
    fn test_resolve_hash_field_operands_requires_both() {
        let ex_missing = Exchange::new(Message::default());
        assert!(resolve_hash_field_operands(&ex_missing).is_err());

        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("hk")),
            ("CamelRedis.Field", serde_json::json!("f1")),
        ]);
        assert_eq!(
            resolve_hash_field_operands(&ex).unwrap(),
            ("hk".to_string(), "f1".to_string())
        );
    }

    #[test]
    fn test_json_from_optional_hash_helpers() {
        assert_eq!(
            json_from_optional_hash_value(Some("v1".to_string())),
            serde_json::json!("v1")
        );
        assert_eq!(json_from_optional_hash_value(None), serde_json::Value::Null);

        let vals = json_from_optional_hash_values(vec![Some("a".to_string()), None]);
        assert_eq!(vals, serde_json::json!(["a", null]));
    }
}
