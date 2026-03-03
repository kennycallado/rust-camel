use super::{get_i64_header, get_str_header, get_str_vec_header, require_key, require_value};
use crate::config::RedisCommand;
use camel_api::{CamelError, Exchange, body::Body};
use redis::AsyncCommands;
use redis::aio::MultiplexedConnection;

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    let result: serde_json::Value =
        match cmd {
            RedisCommand::Sadd => {
                let key = require_key(exchange)?;
                let value = require_value(exchange)?;
                let n: i64 = conn
                    .sadd(&key, value.to_string())
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis SADD failed: {e}")))?;
                serde_json::json!(n)
            }
            RedisCommand::Srem => {
                let key = require_key(exchange)?;
                let value = require_value(exchange)?;
                let n: i64 = conn
                    .srem(&key, value.to_string())
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis SREM failed: {e}")))?;
                serde_json::json!(n)
            }
            RedisCommand::Smembers => {
                let key = require_key(exchange)?;
                let members: Vec<String> = conn.smembers(&key).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis SMEMBERS failed: {e}"))
                })?;
                serde_json::json!(members)
            }
            RedisCommand::Scard => {
                let key = require_key(exchange)?;
                let n: i64 = conn
                    .scard(&key)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis SCARD failed: {e}")))?;
                serde_json::json!(n)
            }
            RedisCommand::Sismember => {
                let key = require_key(exchange)?;
                let value = require_value(exchange)?;
                let ok: bool = conn.sismember(&key, value.to_string()).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis SISMEMBER failed: {e}"))
                })?;
                serde_json::json!(ok)
            }
            RedisCommand::Spop => {
                let key = require_key(exchange)?;
                let val: Option<String> = conn
                    .spop(&key)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis SPOP failed: {e}")))?;
                val.map(serde_json::Value::String)
                    .unwrap_or(serde_json::Value::Null)
            }
            RedisCommand::Smove => {
                let key = require_key(exchange)?;
                let dest = get_str_header(exchange, "CamelRedis.Destination").ok_or_else(|| {
                    CamelError::ProcessorError("Missing CamelRedis.Destination".into())
                })?;
                let value = require_value(exchange)?;
                let ok: bool = conn
                    .smove(&key, dest, value.to_string())
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis SMOVE failed: {e}")))?;
                serde_json::json!(ok)
            }
            RedisCommand::Sinter => {
                let keys = get_str_vec_header(exchange, "CamelRedis.Keys")
                    .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Keys".into()))?;
                let members: Vec<String> = conn
                    .sinter(&keys)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis SINTER failed: {e}")))?;
                serde_json::json!(members)
            }
            RedisCommand::Sunion => {
                let keys = get_str_vec_header(exchange, "CamelRedis.Keys")
                    .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Keys".into()))?;
                let members: Vec<String> = conn
                    .sunion(&keys)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis SUNION failed: {e}")))?;
                serde_json::json!(members)
            }
            RedisCommand::Sdiff => {
                let keys = get_str_vec_header(exchange, "CamelRedis.Keys")
                    .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Keys".into()))?;
                let members: Vec<String> = conn
                    .sdiff(&keys)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis SDIFF failed: {e}")))?;
                serde_json::json!(members)
            }
            RedisCommand::Sinterstore => {
                let dest = get_str_header(exchange, "CamelRedis.Destination").ok_or_else(|| {
                    CamelError::ProcessorError("Missing CamelRedis.Destination".into())
                })?;
                let keys = get_str_vec_header(exchange, "CamelRedis.Keys")
                    .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Keys".into()))?;
                let n: i64 = conn.sinterstore(dest, &keys).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis SINTERSTORE failed: {e}"))
                })?;
                serde_json::json!(n)
            }
            RedisCommand::Sunionstore => {
                let dest = get_str_header(exchange, "CamelRedis.Destination").ok_or_else(|| {
                    CamelError::ProcessorError("Missing CamelRedis.Destination".into())
                })?;
                let keys = get_str_vec_header(exchange, "CamelRedis.Keys")
                    .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Keys".into()))?;
                let n: i64 = conn.sunionstore(dest, &keys).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis SUNIONSTORE failed: {e}"))
                })?;
                serde_json::json!(n)
            }
            RedisCommand::Sdiffstore => {
                let dest = get_str_header(exchange, "CamelRedis.Destination").ok_or_else(|| {
                    CamelError::ProcessorError("Missing CamelRedis.Destination".into())
                })?;
                let keys = get_str_vec_header(exchange, "CamelRedis.Keys")
                    .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Keys".into()))?;
                let n: i64 = conn.sdiffstore(dest, &keys).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis SDIFFSTORE failed: {e}"))
                })?;
                serde_json::json!(n)
            }
            RedisCommand::Srandmember => {
                let key = require_key(exchange)?;
                let count = get_i64_header(exchange, "CamelRedis.Count");
                match count {
                    Some(c) => {
                        let members: Vec<String> = conn
                            .srandmember_multiple(&key, c as isize)
                            .await
                            .map_err(|e| {
                                CamelError::ProcessorError(format!("Redis SRANDMEMBER failed: {e}"))
                            })?;
                        serde_json::json!(members)
                    }
                    None => {
                        let member: Option<String> = conn.srandmember(&key).await.map_err(|e| {
                            CamelError::ProcessorError(format!("Redis SRANDMEMBER failed: {e}"))
                        })?;
                        member
                            .map(serde_json::Value::String)
                            .unwrap_or(serde_json::Value::Null)
                    }
                }
            }
            _ => return Err(CamelError::ProcessorError("Not a set command".into())),
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
    fn test_sadd_requires_key() {
        let ex = Exchange::new(Message::default());
        assert!(crate::commands::require_key(&ex).is_err());
    }

    #[test]
    fn test_sadd_has_key_and_value() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myset")),
            ("CamelRedis.Value", serde_json::json!("member1")),
        ]);
        assert_eq!(crate::commands::require_key(&ex).unwrap(), "myset");
        assert_eq!(
            crate::commands::require_value(&ex).unwrap(),
            serde_json::json!("member1")
        );
    }

    #[test]
    fn test_sinter_requires_keys() {
        let ex = Exchange::new(Message::default());
        assert!(crate::commands::get_str_vec_header(&ex, "CamelRedis.Keys").is_none());
    }

    #[test]
    fn test_smove_requires_destination() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("set1")),
            ("CamelRedis.Value", serde_json::json!("member")),
        ]);
        // Destination is missing
        assert!(crate::commands::get_str_header(&ex, "CamelRedis.Destination").is_none());
    }
}
