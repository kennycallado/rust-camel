use super::{get_str_header, get_str_vec_header, get_u64_header, require_key};
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
            RedisCommand::Exists => {
                let key = require_key(exchange)?;
                let n: bool = conn
                    .exists(&key)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis EXISTS failed: {e}")))?;
                serde_json::json!(n)
            }
            RedisCommand::Del => {
                let keys = get_str_vec_header(exchange, "CamelRedis.Keys")
                    .or_else(|| require_key(exchange).ok().map(|k| vec![k]))
                    .ok_or_else(|| {
                        CamelError::ProcessorError(
                            "Missing CamelRedis.Key or CamelRedis.Keys".into(),
                        )
                    })?;
                let n: i64 = conn
                    .del(&keys)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis DEL failed: {e}")))?;
                serde_json::json!(n)
            }
            RedisCommand::Expire => {
                let key = require_key(exchange)?;
                let secs = get_u64_header(exchange, "CamelRedis.Timeout").unwrap_or(0) as i64;
                let ok: bool = conn
                    .expire(&key, secs)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis EXPIRE failed: {e}")))?;
                serde_json::json!(ok)
            }
            RedisCommand::Expireat => {
                let key = require_key(exchange)?;
                let ts = get_u64_header(exchange, "CamelRedis.Timestamp").unwrap_or(0) as i64;
                let ok: bool = conn.expire_at(&key, ts).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis EXPIREAT failed: {e}"))
                })?;
                serde_json::json!(ok)
            }
            RedisCommand::Pexpire => {
                let key = require_key(exchange)?;
                let ms = get_u64_header(exchange, "CamelRedis.Timeout").unwrap_or(0) as i64;
                let ok: bool = conn.pexpire(&key, ms).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis PEXPIRE failed: {e}"))
                })?;
                serde_json::json!(ok)
            }
            RedisCommand::Pexpireat => {
                let key = require_key(exchange)?;
                let ts = get_u64_header(exchange, "CamelRedis.Timestamp").unwrap_or(0) as i64;
                let ok: bool = conn.pexpire_at(&key, ts).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis PEXPIREAT failed: {e}"))
                })?;
                serde_json::json!(ok)
            }
            RedisCommand::Ttl => {
                let key = require_key(exchange)?;
                let n: i64 = conn
                    .ttl(&key)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis TTL failed: {e}")))?;
                serde_json::json!(n)
            }
            RedisCommand::Keys => {
                let pattern = get_str_header(exchange, "CamelRedis.Pattern").unwrap_or("*");
                let keys: Vec<String> = conn
                    .keys(pattern)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis KEYS failed: {e}")))?;
                serde_json::json!(keys)
            }
            RedisCommand::Rename => {
                let key = require_key(exchange)?;
                let dest = get_str_header(exchange, "CamelRedis.Destination").ok_or_else(|| {
                    CamelError::ProcessorError("Missing CamelRedis.Destination".into())
                })?;
                conn.rename::<_, _, ()>(&key, dest)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis RENAME failed: {e}")))?;
                serde_json::Value::Null
            }
            RedisCommand::Renamenx => {
                let key = require_key(exchange)?;
                let dest = get_str_header(exchange, "CamelRedis.Destination").ok_or_else(|| {
                    CamelError::ProcessorError("Missing CamelRedis.Destination".into())
                })?;
                let ok: bool = conn.rename_nx(&key, dest).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis RENAMENX failed: {e}"))
                })?;
                serde_json::json!(ok)
            }
            RedisCommand::Type => {
                let key = require_key(exchange)?;
                // redis-rs returns a String for TYPE
                let t: String = redis::cmd("TYPE")
                    .arg(&key)
                    .query_async(conn)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis TYPE failed: {e}")))?;
                serde_json::Value::String(t)
            }
            RedisCommand::Persist => {
                let key = require_key(exchange)?;
                let ok: bool = conn.persist(&key).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis PERSIST failed: {e}"))
                })?;
                serde_json::json!(ok)
            }
            RedisCommand::Move => {
                let key = require_key(exchange)?;
                let db = exchange
                    .input
                    .header("CamelRedis.Db")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as i64;
                // MOVE is not directly exposed in AsyncCommands, use raw command
                let ok: i64 = redis::cmd("MOVE")
                    .arg(&key)
                    .arg(db)
                    .query_async(conn)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis MOVE failed: {e}")))?;
                serde_json::json!(ok == 1)
            }
            RedisCommand::Sort => {
                let key = require_key(exchange)?;
                // Basic SORT — returns sorted list using raw command
                let vals: Vec<String> = redis::cmd("SORT")
                    .arg(&key)
                    .query_async(conn)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis SORT failed: {e}")))?;
                serde_json::json!(vals)
            }
            _ => return Err(CamelError::ProcessorError("Not a key command".into())),
        };
    exchange.input.body = Body::Json(result);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{Exchange, Message};

    #[test]
    fn test_expire_requires_key() {
        let ex = Exchange::new(Message::default());
        assert!(crate::commands::require_key(&ex).is_err());
    }

    #[test]
    fn test_del_with_keys_header() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Keys", serde_json::json!(["key1", "key2"]));
        let ex = Exchange::new(msg);
        assert_eq!(
            crate::commands::get_str_vec_header(&ex, "CamelRedis.Keys"),
            Some(vec!["key1".to_string(), "key2".to_string()])
        );
    }

    #[test]
    fn test_keys_pattern_default() {
        let ex = Exchange::new(Message::default());
        assert_eq!(get_str_header(&ex, "CamelRedis.Pattern"), None);
    }

    #[test]
    fn test_move_db_header() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Db", serde_json::json!(3u64));
        let ex = Exchange::new(msg);
        assert_eq!(
            ex.input.header("CamelRedis.Db").and_then(|v| v.as_u64()),
            Some(3)
        );
    }
}
