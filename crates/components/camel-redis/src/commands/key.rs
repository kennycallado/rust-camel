use super::{get_str_header, get_str_vec_header, get_u64_header, require_key};
use crate::config::RedisCommand;
use camel_component_api::{Body, CamelError, Exchange};
use redis::AsyncCommands;
use redis::aio::MultiplexedConnection;

pub(crate) fn is_key_command(cmd: &RedisCommand) -> bool {
    matches!(
        cmd,
        RedisCommand::Exists
            | RedisCommand::Del
            | RedisCommand::Expire
            | RedisCommand::Expireat
            | RedisCommand::Pexpire
            | RedisCommand::Pexpireat
            | RedisCommand::Ttl
            | RedisCommand::Keys
            | RedisCommand::Rename
            | RedisCommand::Renamenx
            | RedisCommand::Type
            | RedisCommand::Persist
            | RedisCommand::Move
            | RedisCommand::Sort
    )
}

pub(crate) fn resolve_del_keys(exchange: &Exchange) -> Result<Vec<String>, CamelError> {
    get_str_vec_header(exchange, "CamelRedis.Keys")
        .or_else(|| require_key(exchange).ok().map(|k| vec![k]))
        .ok_or_else(|| {
            CamelError::ProcessorError("Missing CamelRedis.Key or CamelRedis.Keys".into())
        })
}

pub(crate) fn resolve_destination(exchange: &Exchange) -> Result<String, CamelError> {
    get_str_header(exchange, "CamelRedis.Destination")
        .map(|s| s.to_string())
        .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Destination".into()))
}

pub(crate) fn resolve_move_db(exchange: &Exchange) -> i64 {
    exchange
        .input
        .header("CamelRedis.Db")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as i64
}

pub(crate) fn resolve_expire_timeout(exchange: &Exchange) -> i64 {
    get_u64_header(exchange, "CamelRedis.Timeout").unwrap_or(0) as i64
}

pub(crate) fn resolve_expire_timestamp(exchange: &Exchange) -> i64 {
    get_u64_header(exchange, "CamelRedis.Timestamp").unwrap_or(0) as i64
}

pub(crate) fn resolve_keys_pattern(exchange: &Exchange) -> String {
    get_str_header(exchange, "CamelRedis.Pattern")
        .unwrap_or("*")
        .to_string()
}

pub(crate) fn resolve_rename_operands(exchange: &Exchange) -> Result<(String, String), CamelError> {
    Ok((require_key(exchange)?, resolve_destination(exchange)?))
}

pub(crate) fn resolve_move_operands(exchange: &Exchange) -> Result<(String, i64), CamelError> {
    Ok((require_key(exchange)?, resolve_move_db(exchange)))
}

pub(crate) fn json_from_move_result(result: i64) -> serde_json::Value {
    serde_json::json!(result == 1)
}

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    if !is_key_command(cmd) {
        return Err(CamelError::ProcessorError("Not a key command".into()));
    }

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
                let keys = resolve_del_keys(exchange)?;
                let n: i64 = conn
                    .del(&keys)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis DEL failed: {e}")))?;
                serde_json::json!(n)
            }
            RedisCommand::Expire => {
                let key = require_key(exchange)?;
                let secs = resolve_expire_timeout(exchange);
                let ok: bool = conn
                    .expire(&key, secs)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis EXPIRE failed: {e}")))?;
                serde_json::json!(ok)
            }
            RedisCommand::Expireat => {
                let key = require_key(exchange)?;
                let ts = resolve_expire_timestamp(exchange);
                let ok: bool = conn.expire_at(&key, ts).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis EXPIREAT failed: {e}"))
                })?;
                serde_json::json!(ok)
            }
            RedisCommand::Pexpire => {
                let key = require_key(exchange)?;
                let ms = resolve_expire_timeout(exchange);
                let ok: bool = conn.pexpire(&key, ms).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis PEXPIRE failed: {e}"))
                })?;
                serde_json::json!(ok)
            }
            RedisCommand::Pexpireat => {
                let key = require_key(exchange)?;
                let ts = resolve_expire_timestamp(exchange);
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
                let pattern = resolve_keys_pattern(exchange);
                let keys: Vec<String> = conn
                    .keys(pattern)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis KEYS failed: {e}")))?;
                serde_json::json!(keys)
            }
            RedisCommand::Rename => {
                let (key, dest) = resolve_rename_operands(exchange)?;
                conn.rename::<_, _, ()>(&key, dest)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis RENAME failed: {e}")))?;
                serde_json::Value::Null
            }
            RedisCommand::Renamenx => {
                let (key, dest) = resolve_rename_operands(exchange)?;
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
                let (key, db) = resolve_move_operands(exchange)?;
                // MOVE is not directly exposed in AsyncCommands, use raw command
                let ok: i64 = redis::cmd("MOVE")
                    .arg(&key)
                    .arg(db)
                    .query_async(conn)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis MOVE failed: {e}")))?;
                json_from_move_result(ok)
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
            _ => unreachable!("non-key commands rejected above"),
        };
    exchange.input.body = Body::Json(result);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::{Exchange, Message};

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
        assert_eq!(resolve_move_db(&ex), 3);
    }

    #[test]
    fn test_is_key_command_classification() {
        assert!(is_key_command(&RedisCommand::Del));
        assert!(is_key_command(&RedisCommand::Sort));
        assert!(!is_key_command(&RedisCommand::Set));
    }

    #[test]
    fn test_resolve_del_keys_prefers_keys_header() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Keys", serde_json::json!(["a", "b"]));
        msg.set_header("CamelRedis.Key", serde_json::json!("single"));
        let ex = Exchange::new(msg);
        assert_eq!(resolve_del_keys(&ex).unwrap(), vec!["a", "b"]);
    }

    #[test]
    fn test_resolve_del_keys_falls_back_to_single_key() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Key", serde_json::json!("single"));
        let ex = Exchange::new(msg);
        assert_eq!(resolve_del_keys(&ex).unwrap(), vec!["single"]);
    }

    #[test]
    fn test_resolve_destination_requires_header() {
        let ex = Exchange::new(Message::default());
        let err = resolve_destination(&ex).expect_err("destination should be required");
        assert!(err.to_string().contains("CamelRedis.Destination"));
    }

    #[test]
    fn test_resolve_expire_timeout_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_expire_timeout(&ex_default), 0);

        let mut msg = Message::default();
        msg.set_header("CamelRedis.Timeout", serde_json::json!(9));
        let ex = Exchange::new(msg);
        assert_eq!(resolve_expire_timeout(&ex), 9);
    }

    #[test]
    fn test_resolve_expire_timestamp_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_expire_timestamp(&ex_default), 0);

        let mut msg = Message::default();
        msg.set_header("CamelRedis.Timestamp", serde_json::json!(123));
        let ex = Exchange::new(msg);
        assert_eq!(resolve_expire_timestamp(&ex), 123);
    }

    #[test]
    fn test_resolve_keys_pattern_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_keys_pattern(&ex_default), "*");

        let mut msg = Message::default();
        msg.set_header("CamelRedis.Pattern", serde_json::json!("user:*"));
        let ex = Exchange::new(msg);
        assert_eq!(resolve_keys_pattern(&ex), "user:*");
    }

    #[test]
    fn test_resolve_rename_operands_requires_destination() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Key", serde_json::json!("k1"));
        let ex = Exchange::new(msg);
        let err = resolve_rename_operands(&ex).expect_err("destination should be required");
        assert!(err.to_string().contains("CamelRedis.Destination"));
    }

    #[test]
    fn test_resolve_move_operands_uses_default_db() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Key", serde_json::json!("k1"));
        let ex = Exchange::new(msg);
        assert_eq!(resolve_move_operands(&ex).unwrap(), ("k1".to_string(), 0));
    }

    #[test]
    fn test_json_from_move_result_variants() {
        assert_eq!(json_from_move_result(1), serde_json::json!(true));
        assert_eq!(json_from_move_result(0), serde_json::json!(false));
    }
}
