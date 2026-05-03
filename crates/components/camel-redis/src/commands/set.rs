use super::{get_i64_header, get_str_header, get_str_vec_header, require_key, require_value};
use crate::config::RedisCommand;
use camel_component_api::{Body, CamelError, Exchange};
use redis::AsyncCommands;
use redis::aio::MultiplexedConnection;

pub(crate) fn is_set_command(cmd: &RedisCommand) -> bool {
    matches!(
        cmd,
        RedisCommand::Sadd
            | RedisCommand::Srem
            | RedisCommand::Smembers
            | RedisCommand::Scard
            | RedisCommand::Sismember
            | RedisCommand::Spop
            | RedisCommand::Smove
            | RedisCommand::Sinter
            | RedisCommand::Sunion
            | RedisCommand::Sdiff
            | RedisCommand::Sinterstore
            | RedisCommand::Sunionstore
            | RedisCommand::Sdiffstore
            | RedisCommand::Srandmember
    )
}

pub(crate) fn resolve_set_keys(exchange: &Exchange) -> Result<Vec<String>, CamelError> {
    get_str_vec_header(exchange, "CamelRedis.Keys")
        .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Keys".into()))
}

pub(crate) fn resolve_destination(exchange: &Exchange) -> Result<String, CamelError> {
    get_str_header(exchange, "CamelRedis.Destination")
        .map(|s| s.to_string())
        .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Destination".into()))
}

pub(crate) fn resolve_random_member_count(exchange: &Exchange) -> Option<i64> {
    get_i64_header(exchange, "CamelRedis.Count")
}

pub(crate) fn resolve_store_operands(
    exchange: &Exchange,
) -> Result<(String, Vec<String>), CamelError> {
    Ok((resolve_destination(exchange)?, resolve_set_keys(exchange)?))
}

pub(crate) fn json_from_optional_member(value: Option<String>) -> serde_json::Value {
    value
        .map(serde_json::Value::String)
        .unwrap_or(serde_json::Value::Null)
}

pub(crate) fn resolve_key_value_operands(
    exchange: &Exchange,
) -> Result<(String, serde_json::Value), CamelError> {
    Ok((require_key(exchange)?, require_value(exchange)?))
}

pub(crate) fn resolve_key_destination_value_operands(
    exchange: &Exchange,
) -> Result<(String, String, serde_json::Value), CamelError> {
    Ok((
        require_key(exchange)?,
        resolve_destination(exchange)?,
        require_value(exchange)?,
    ))
}

pub(crate) fn json_from_members(values: Vec<String>) -> serde_json::Value {
    serde_json::json!(values)
}

#[allow(dead_code)]
pub(crate) fn build_redis_cmd(cmd: &RedisCommand, exchange: &Exchange) -> Result<redis::Cmd, CamelError> {
    if !is_set_command(cmd) {
        return Err(CamelError::ProcessorError("Not a set command".into()));
    }

    let redis_cmd = match cmd {
        RedisCommand::Sadd => {
            let (key, value) = resolve_key_value_operands(exchange)?;
            let mut c = redis::cmd("SADD");
            c.arg(key).arg(value.to_string());
            c
        }
        RedisCommand::Srem => {
            let (key, value) = resolve_key_value_operands(exchange)?;
            let mut c = redis::cmd("SREM");
            c.arg(key).arg(value.to_string());
            c
        }
        RedisCommand::Smembers => {
            let key = require_key(exchange)?;
            let mut c = redis::cmd("SMEMBERS");
            c.arg(key);
            c
        }
        RedisCommand::Scard => {
            let key = require_key(exchange)?;
            let mut c = redis::cmd("SCARD");
            c.arg(key);
            c
        }
        RedisCommand::Sismember => {
            let (key, value) = resolve_key_value_operands(exchange)?;
            let mut c = redis::cmd("SISMEMBER");
            c.arg(key).arg(value.to_string());
            c
        }
        RedisCommand::Spop => {
            let key = require_key(exchange)?;
            let mut c = redis::cmd("SPOP");
            c.arg(key);
            c
        }
        RedisCommand::Smove => {
            let (key, dest, value) = resolve_key_destination_value_operands(exchange)?;
            let mut c = redis::cmd("SMOVE");
            c.arg(key).arg(dest).arg(value.to_string());
            c
        }
        RedisCommand::Sinter => {
            let keys = resolve_set_keys(exchange)?;
            let mut c = redis::cmd("SINTER");
            for key in keys {
                c.arg(key);
            }
            c
        }
        RedisCommand::Sunion => {
            let keys = resolve_set_keys(exchange)?;
            let mut c = redis::cmd("SUNION");
            for key in keys {
                c.arg(key);
            }
            c
        }
        RedisCommand::Sdiff => {
            let keys = resolve_set_keys(exchange)?;
            let mut c = redis::cmd("SDIFF");
            for key in keys {
                c.arg(key);
            }
            c
        }
        RedisCommand::Sinterstore => {
            let (dest, keys) = resolve_store_operands(exchange)?;
            let mut c = redis::cmd("SINTERSTORE");
            c.arg(dest);
            for key in keys {
                c.arg(key);
            }
            c
        }
        RedisCommand::Sunionstore => {
            let (dest, keys) = resolve_store_operands(exchange)?;
            let mut c = redis::cmd("SUNIONSTORE");
            c.arg(dest);
            for key in keys {
                c.arg(key);
            }
            c
        }
        RedisCommand::Sdiffstore => {
            let (dest, keys) = resolve_store_operands(exchange)?;
            let mut c = redis::cmd("SDIFFSTORE");
            c.arg(dest);
            for key in keys {
                c.arg(key);
            }
            c
        }
        RedisCommand::Srandmember => {
            let key = require_key(exchange)?;
            let count = resolve_random_member_count(exchange);
            let mut c = redis::cmd("SRANDMEMBER");
            c.arg(key);
            if let Some(cnt) = count {
                c.arg(cnt);
            }
            c
        }
        _ => unreachable!("non-set commands rejected above"),
    };

    Ok(redis_cmd)
}

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    if !is_set_command(cmd) {
        return Err(CamelError::ProcessorError("Not a set command".into()));
    }

    let result: serde_json::Value =
        match cmd {
            RedisCommand::Sadd => {
                let (key, value) = resolve_key_value_operands(exchange)?;
                let n: i64 = conn
                    .sadd(&key, value.to_string())
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis SADD failed: {e}")))?;
                serde_json::json!(n)
            }
            RedisCommand::Srem => {
                let (key, value) = resolve_key_value_operands(exchange)?;
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
                json_from_members(members)
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
                let (key, value) = resolve_key_value_operands(exchange)?;
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
                json_from_optional_member(val)
            }
            RedisCommand::Smove => {
                let (key, dest, value) = resolve_key_destination_value_operands(exchange)?;
                let ok: bool = conn
                    .smove(&key, dest, value.to_string())
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis SMOVE failed: {e}")))?;
                serde_json::json!(ok)
            }
            RedisCommand::Sinter => {
                let keys = resolve_set_keys(exchange)?;
                let members: Vec<String> = conn
                    .sinter(&keys)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis SINTER failed: {e}")))?;
                json_from_members(members)
            }
            RedisCommand::Sunion => {
                let keys = resolve_set_keys(exchange)?;
                let members: Vec<String> = conn
                    .sunion(&keys)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis SUNION failed: {e}")))?;
                json_from_members(members)
            }
            RedisCommand::Sdiff => {
                let keys = resolve_set_keys(exchange)?;
                let members: Vec<String> = conn
                    .sdiff(&keys)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis SDIFF failed: {e}")))?;
                json_from_members(members)
            }
            RedisCommand::Sinterstore => {
                let (dest, keys) = resolve_store_operands(exchange)?;
                let n: i64 = conn.sinterstore(dest, &keys).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis SINTERSTORE failed: {e}"))
                })?;
                serde_json::json!(n)
            }
            RedisCommand::Sunionstore => {
                let (dest, keys) = resolve_store_operands(exchange)?;
                let n: i64 = conn.sunionstore(dest, &keys).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis SUNIONSTORE failed: {e}"))
                })?;
                serde_json::json!(n)
            }
            RedisCommand::Sdiffstore => {
                let (dest, keys) = resolve_store_operands(exchange)?;
                let n: i64 = conn.sdiffstore(dest, &keys).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis SDIFFSTORE failed: {e}"))
                })?;
                serde_json::json!(n)
            }
            RedisCommand::Srandmember => {
                let key = require_key(exchange)?;
                let count = resolve_random_member_count(exchange);
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
                        json_from_optional_member(member)
                    }
                }
            }
            _ => unreachable!("non-set commands rejected above"),
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
        let err = resolve_destination(&ex).expect_err("destination should be required");
        assert!(err.to_string().contains("CamelRedis.Destination"));
    }

    #[test]
    fn test_set_command_classification() {
        assert!(is_set_command(&RedisCommand::Sadd));
        assert!(is_set_command(&RedisCommand::Srandmember));
        assert!(!is_set_command(&RedisCommand::Get));
    }

    #[test]
    fn test_resolve_set_keys_requires_header() {
        let ex = Exchange::new(Message::default());
        let err = resolve_set_keys(&ex).expect_err("keys should be required");
        assert!(err.to_string().contains("CamelRedis.Keys"));
    }

    #[test]
    fn test_resolve_set_keys_returns_values() {
        let ex = ex_with(&[("CamelRedis.Keys", serde_json::json!(["s1", "s2"]))]);
        assert_eq!(resolve_set_keys(&ex).unwrap(), vec!["s1", "s2"]);
    }

    #[test]
    fn test_resolve_random_member_count() {
        let ex_none = Exchange::new(Message::default());
        assert_eq!(resolve_random_member_count(&ex_none), None);

        let ex = ex_with(&[("CamelRedis.Count", serde_json::json!(3))]);
        assert_eq!(resolve_random_member_count(&ex), Some(3));
    }

    #[test]
    fn test_resolve_destination_returns_value() {
        let ex = ex_with(&[("CamelRedis.Destination", serde_json::json!("dest"))]);
        assert_eq!(resolve_destination(&ex).unwrap(), "dest");
    }

    #[test]
    fn test_resolve_store_operands_returns_destination_and_keys() {
        let ex = ex_with(&[
            ("CamelRedis.Destination", serde_json::json!("dest")),
            ("CamelRedis.Keys", serde_json::json!(["k1", "k2"])),
        ]);
        let (dest, keys) = resolve_store_operands(&ex).unwrap();
        assert_eq!(dest, "dest");
        assert_eq!(keys, vec!["k1", "k2"]);
    }

    #[test]
    fn test_resolve_store_operands_requires_destination_first() {
        let ex = ex_with(&[("CamelRedis.Keys", serde_json::json!(["k1"]))]);
        let err = resolve_store_operands(&ex).expect_err("destination should be required");
        assert!(err.to_string().contains("CamelRedis.Destination"));
    }

    #[test]
    fn test_json_from_optional_member_variants() {
        assert_eq!(
            json_from_optional_member(Some("m1".to_string())),
            serde_json::json!("m1")
        );
        assert_eq!(json_from_optional_member(None), serde_json::Value::Null);
    }

    #[test]
    fn test_resolve_key_value_operands_requires_headers() {
        let ex_missing = Exchange::new(Message::default());
        assert!(resolve_key_value_operands(&ex_missing).is_err());

        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("s1")),
            ("CamelRedis.Value", serde_json::json!("v1")),
        ]);
        let (key, value) = resolve_key_value_operands(&ex).unwrap();
        assert_eq!(key, "s1");
        assert_eq!(value, serde_json::json!("v1"));
    }

    #[test]
    fn test_resolve_key_destination_value_operands_requires_destination() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("s1")),
            ("CamelRedis.Value", serde_json::json!("v1")),
        ]);
        let err = resolve_key_destination_value_operands(&ex)
            .expect_err("destination should be required");
        assert!(err.to_string().contains("CamelRedis.Destination"));
    }

    #[test]
    fn test_json_from_members_returns_array() {
        assert_eq!(
            json_from_members(vec!["a".to_string(), "b".to_string()]),
            serde_json::json!(["a", "b"])
        );
    }

    fn cmd_args(cmd: &redis::Cmd) -> Vec<String> {
        cmd.args_iter()
            .skip(1)
            .filter_map(|a| match a {
                redis::Arg::Simple(bytes) => String::from_utf8(bytes.to_vec()).ok(),
                redis::Arg::Cursor => Some("CURSOR".to_string()),
                _ => None,
            })
            .collect()
    }

    fn cmd_name(cmd: &redis::Cmd) -> String {
        cmd.args_iter()
            .next()
            .and_then(|a| match a {
                redis::Arg::Simple(bytes) => String::from_utf8(bytes.to_vec()).ok(),
                _ => None,
            })
            .unwrap_or_default()
    }

    #[test]
    fn test_build_redis_cmd_sadd() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myset")),
            ("CamelRedis.Value", serde_json::json!("member1")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Sadd, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "SADD");
        assert_eq!(cmd_args(&cmd), vec!["myset", "\"member1\""]);
    }

    #[test]
    fn test_build_redis_cmd_sadd_missing_key() {
        let ex = ex_with(&[("CamelRedis.Value", serde_json::json!("m"))]);
        assert!(build_redis_cmd(&RedisCommand::Sadd, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_sadd_missing_value() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("k"))]);
        assert!(build_redis_cmd(&RedisCommand::Sadd, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_srem() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myset")),
            ("CamelRedis.Value", serde_json::json!("member1")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Srem, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "SREM");
        assert_eq!(cmd_args(&cmd), vec!["myset", "\"member1\""]);
    }

    #[test]
    fn test_build_redis_cmd_srem_missing_key() {
        let ex = ex_with(&[("CamelRedis.Value", serde_json::json!("m"))]);
        assert!(build_redis_cmd(&RedisCommand::Srem, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_smembers() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("myset"))]);
        let cmd = build_redis_cmd(&RedisCommand::Smembers, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "SMEMBERS");
        assert_eq!(cmd_args(&cmd), vec!["myset"]);
    }

    #[test]
    fn test_build_redis_cmd_smembers_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Smembers, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_scard() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("myset"))]);
        let cmd = build_redis_cmd(&RedisCommand::Scard, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "SCARD");
        assert_eq!(cmd_args(&cmd), vec!["myset"]);
    }

    #[test]
    fn test_build_redis_cmd_scard_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Scard, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_sismember() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myset")),
            ("CamelRedis.Value", serde_json::json!("member1")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Sismember, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "SISMEMBER");
        assert_eq!(cmd_args(&cmd), vec!["myset", "\"member1\""]);
    }

    #[test]
    fn test_build_redis_cmd_sismember_missing_key() {
        let ex = ex_with(&[("CamelRedis.Value", serde_json::json!("m"))]);
        assert!(build_redis_cmd(&RedisCommand::Sismember, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_spop() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("myset"))]);
        let cmd = build_redis_cmd(&RedisCommand::Spop, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "SPOP");
        assert_eq!(cmd_args(&cmd), vec!["myset"]);
    }

    #[test]
    fn test_build_redis_cmd_spop_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Spop, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_smove() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("set1")),
            ("CamelRedis.Destination", serde_json::json!("set2")),
            ("CamelRedis.Value", serde_json::json!("member1")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Smove, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "SMOVE");
        assert_eq!(cmd_args(&cmd), vec!["set1", "set2", "\"member1\""]);
    }

    #[test]
    fn test_build_redis_cmd_smove_missing_key() {
        let ex = ex_with(&[
            ("CamelRedis.Destination", serde_json::json!("set2")),
            ("CamelRedis.Value", serde_json::json!("m")),
        ]);
        assert!(build_redis_cmd(&RedisCommand::Smove, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_smove_missing_destination() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("set1")),
            ("CamelRedis.Value", serde_json::json!("m")),
        ]);
        assert!(build_redis_cmd(&RedisCommand::Smove, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_sinter() {
        let ex = ex_with(&[("CamelRedis.Keys", serde_json::json!(["s1", "s2", "s3"]))]);
        let cmd = build_redis_cmd(&RedisCommand::Sinter, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "SINTER");
        assert_eq!(cmd_args(&cmd), vec!["s1", "s2", "s3"]);
    }

    #[test]
    fn test_build_redis_cmd_sinter_missing_keys() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Sinter, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_sunion() {
        let ex = ex_with(&[("CamelRedis.Keys", serde_json::json!(["s1", "s2"]))]);
        let cmd = build_redis_cmd(&RedisCommand::Sunion, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "SUNION");
        assert_eq!(cmd_args(&cmd), vec!["s1", "s2"]);
    }

    #[test]
    fn test_build_redis_cmd_sunion_missing_keys() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Sunion, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_sdiff() {
        let ex = ex_with(&[("CamelRedis.Keys", serde_json::json!(["s1", "s2"]))]);
        let cmd = build_redis_cmd(&RedisCommand::Sdiff, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "SDIFF");
        assert_eq!(cmd_args(&cmd), vec!["s1", "s2"]);
    }

    #[test]
    fn test_build_redis_cmd_sdiff_missing_keys() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Sdiff, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_sinterstore() {
        let ex = ex_with(&[
            ("CamelRedis.Destination", serde_json::json!("dest")),
            ("CamelRedis.Keys", serde_json::json!(["s1", "s2"])),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Sinterstore, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "SINTERSTORE");
        assert_eq!(cmd_args(&cmd), vec!["dest", "s1", "s2"]);
    }

    #[test]
    fn test_build_redis_cmd_sinterstore_missing_destination() {
        let ex = ex_with(&[("CamelRedis.Keys", serde_json::json!(["s1"]))]);
        assert!(build_redis_cmd(&RedisCommand::Sinterstore, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_sunionstore() {
        let ex = ex_with(&[
            ("CamelRedis.Destination", serde_json::json!("dest")),
            ("CamelRedis.Keys", serde_json::json!(["s1", "s2"])),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Sunionstore, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "SUNIONSTORE");
        assert_eq!(cmd_args(&cmd), vec!["dest", "s1", "s2"]);
    }

    #[test]
    fn test_build_redis_cmd_sunionstore_missing_destination() {
        let ex = ex_with(&[("CamelRedis.Keys", serde_json::json!(["s1"]))]);
        assert!(build_redis_cmd(&RedisCommand::Sunionstore, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_sdiffstore() {
        let ex = ex_with(&[
            ("CamelRedis.Destination", serde_json::json!("dest")),
            ("CamelRedis.Keys", serde_json::json!(["s1", "s2"])),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Sdiffstore, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "SDIFFSTORE");
        assert_eq!(cmd_args(&cmd), vec!["dest", "s1", "s2"]);
    }

    #[test]
    fn test_build_redis_cmd_sdiffstore_missing_destination() {
        let ex = ex_with(&[("CamelRedis.Keys", serde_json::json!(["s1"]))]);
        assert!(build_redis_cmd(&RedisCommand::Sdiffstore, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_srandmember() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("myset"))]);
        let cmd = build_redis_cmd(&RedisCommand::Srandmember, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "SRANDMEMBER");
        assert_eq!(cmd_args(&cmd), vec!["myset"]);
    }

    #[test]
    fn test_build_redis_cmd_srandmember_with_count() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myset")),
            ("CamelRedis.Count", serde_json::json!(3i64)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Srandmember, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "SRANDMEMBER");
        assert_eq!(cmd_args(&cmd), vec!["myset", "3"]);
    }

    #[test]
    fn test_build_redis_cmd_srandmember_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Srandmember, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_rejects_non_set() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("k"))]);
        assert!(build_redis_cmd(&RedisCommand::Get, &ex).is_err());
    }
}
