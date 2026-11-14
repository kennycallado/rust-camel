use super::{
    body_value_bytes, get_i64_header, get_str_header, get_str_vec_header, require_key,
    require_key_list, require_string_list,
};
use crate::HEADER_VALUE;
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
            | RedisCommand::Smismember
            | RedisCommand::Sintercard
    )
}

/// Strict resolver for `SMISMEMBER`'s `CamelRedis.Members`.
pub(crate) fn resolve_members_list(exchange: &Exchange) -> Result<Vec<String>, CamelError> {
    require_string_list(exchange, "CamelRedis.Members", "SMISMEMBER", "member")
}

/// Optional `SINTERCARD` `CamelRedis.Limit`; a negative value is a named
/// pre-send error because Redis reads `LIMIT` as a result bound.
pub(crate) fn resolve_sintercard_limit(exchange: &Exchange) -> Result<Option<i64>, CamelError> {
    match get_i64_header(exchange, "CamelRedis.Limit")? {
        Some(limit) if limit < 0 => Err(CamelError::ProcessorError(format!(
            "SINTERCARD: invalid CamelRedis.Limit {limit}: LIMIT must be non-negative"
        ))),
        limit => Ok(limit),
    }
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

pub(crate) fn resolve_random_member_count(exchange: &Exchange) -> Result<Option<i64>, CamelError> {
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

pub(crate) fn json_from_members(values: Vec<String>) -> serde_json::Value {
    serde_json::json!(values)
}

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    if !is_set_command(cmd) {
        return Err(CamelError::ProcessorError("Not a set command".into()));
    }

    let result: serde_json::Value = match cmd {
        RedisCommand::Sadd => {
            let key = require_key(exchange)?;
            let value = body_value_bytes(exchange, "SADD", &[HEADER_VALUE])?;
            let n: i64 = conn
                .sadd(&key, &value)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SADD", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Srem => {
            let key = require_key(exchange)?;
            let value = body_value_bytes(exchange, "SREM", &[HEADER_VALUE])?;
            let n: i64 = conn
                .srem(&key, &value)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SREM", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Smembers => {
            let key = require_key(exchange)?;
            let members: Vec<String> = conn
                .smembers(&key)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SMEMBERS", e))?;
            json_from_members(members)
        }
        RedisCommand::Scard => {
            let key = require_key(exchange)?;
            let n: i64 = conn
                .scard(&key)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SCARD", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Sismember => {
            let key = require_key(exchange)?;
            let value = body_value_bytes(exchange, "SISMEMBER", &[HEADER_VALUE])?;
            let ok: bool = conn
                .sismember(&key, &value)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SISMEMBER", e))?;
            serde_json::json!(ok)
        }
        RedisCommand::Spop => {
            let key = require_key(exchange)?;
            let val: Option<String> = conn
                .spop(&key)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SPOP", e))?;
            json_from_optional_member(val)
        }
        RedisCommand::Smove => {
            let key = require_key(exchange)?;
            let dest = resolve_destination(exchange)?;
            let value = body_value_bytes(exchange, "SMOVE", &[HEADER_VALUE])?;
            let ok: bool = conn
                .smove(&key, dest, &value)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SMOVE", e))?;
            serde_json::json!(ok)
        }
        RedisCommand::Sinter => {
            let keys = resolve_set_keys(exchange)?;
            let members: Vec<String> = conn
                .sinter(&keys)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SINTER", e))?;
            json_from_members(members)
        }
        RedisCommand::Sunion => {
            let keys = resolve_set_keys(exchange)?;
            let members: Vec<String> = conn
                .sunion(&keys)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SUNION", e))?;
            json_from_members(members)
        }
        RedisCommand::Sdiff => {
            let keys = resolve_set_keys(exchange)?;
            let members: Vec<String> = conn
                .sdiff(&keys)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SDIFF", e))?;
            json_from_members(members)
        }
        RedisCommand::Sinterstore => {
            let (dest, keys) = resolve_store_operands(exchange)?;
            let n: i64 = conn
                .sinterstore(dest, &keys)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SINTERSTORE", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Sunionstore => {
            let (dest, keys) = resolve_store_operands(exchange)?;
            let n: i64 = conn
                .sunionstore(dest, &keys)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SUNIONSTORE", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Sdiffstore => {
            let (dest, keys) = resolve_store_operands(exchange)?;
            let n: i64 = conn
                .sdiffstore(dest, &keys)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SDIFFSTORE", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Srandmember => {
            let key = require_key(exchange)?;
            let count = resolve_random_member_count(exchange)?;
            match count {
                Some(c) => {
                    let members: Vec<String> = conn
                        .srandmember_multiple(&key, c as isize)
                        .await
                        .map_err(|e| {
                            crate::transport_error::redis_error_to_camel("SRANDMEMBER", e)
                        })?;
                    serde_json::json!(members)
                }
                None => {
                    let member: Option<String> = conn.srandmember(&key).await.map_err(|e| {
                        crate::transport_error::redis_error_to_camel("SRANDMEMBER", e)
                    })?;
                    json_from_optional_member(member)
                }
            }
        }
        RedisCommand::Smismember => {
            let key = require_key(exchange)?;
            let members = resolve_members_list(exchange)?;
            let mut command = redis::cmd("SMISMEMBER");
            command.arg(&key);
            for member in &members {
                command.arg(member);
            }
            let flags: Vec<i64> = command
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SMISMEMBER", e))?;
            serde_json::json!(flags.iter().map(|&flag| flag != 0).collect::<Vec<bool>>())
        }
        RedisCommand::Sintercard => {
            let keys = require_key_list(exchange, "SINTERCARD")?;
            let limit = resolve_sintercard_limit(exchange)?;
            let mut command = redis::cmd("SINTERCARD");
            command.arg(keys.len());
            for key in &keys {
                command.arg(key);
            }
            if let Some(limit) = limit {
                command.arg("LIMIT").arg(limit);
            }
            let n: i64 = command
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SINTERCARD", e))?;
            serde_json::json!(n)
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
    fn test_sadd_has_key_and_body_value() {
        let mut msg = Message::new(Body::Text("member1".into()));
        msg.set_header("CamelRedis.Key", serde_json::json!("myset"));
        let ex = Exchange::new(msg);
        assert_eq!(crate::commands::require_key(&ex).unwrap(), "myset");
        assert_eq!(
            body_value_bytes(&ex, "SADD", &[HEADER_VALUE]).unwrap(),
            b"member1".to_vec()
        );
    }

    #[test]
    fn test_sinter_requires_keys() {
        let ex = Exchange::new(Message::default());
        assert!(crate::commands::get_str_vec_header(&ex, "CamelRedis.Keys").is_none());
    }

    #[test]
    fn test_smove_requires_destination() {
        let mut msg = Message::new(Body::Text("member".into()));
        msg.set_header("CamelRedis.Key", serde_json::json!("set1"));
        let ex = Exchange::new(msg);
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
        assert_eq!(resolve_random_member_count(&ex_none).unwrap(), None);

        let ex = ex_with(&[("CamelRedis.Count", serde_json::json!(3))]);
        assert_eq!(resolve_random_member_count(&ex).unwrap(), Some(3));

        let ex_str = ex_with(&[("CamelRedis.Count", serde_json::json!(" 3 "))]);
        assert_eq!(resolve_random_member_count(&ex_str).unwrap(), Some(3));
    }

    #[test]
    fn test_resolve_random_member_count_garbage_is_named_error() {
        let ex = ex_with(&[("CamelRedis.Count", serde_json::json!("abc"))]);
        let err = resolve_random_member_count(&ex).expect_err("garbage count must fail");
        let text = err.to_string();
        assert!(text.contains("CamelRedis.Count"), "{text}");
        assert!(text.contains("abc"), "{text}");
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
    fn test_json_from_members_returns_array() {
        assert_eq!(
            json_from_members(vec!["a".to_string(), "b".to_string()]),
            serde_json::json!(["a", "b"])
        );
    }

    // ── SMISMEMBER / SINTERCARD resolvers (rediscmds task 2.4) ──────────────

    #[test]
    fn resolve_members_list_strict() {
        let ok = ex_with(&[("CamelRedis.Members", serde_json::json!(["a", "b"]))]);
        assert_eq!(
            resolve_members_list(&ok).expect("a valid member list must resolve"),
            vec!["a".to_string(), "b".to_string()]
        );

        for (label, value) in [
            ("empty array", serde_json::json!([])),
            ("non-array", serde_json::json!("a")),
        ] {
            let ex = ex_with(&[("CamelRedis.Members", value)]);
            let err = resolve_members_list(&ex).expect_err(label);
            let msg = err.to_string();
            assert!(msg.contains("CamelRedis.Members"), "{label}: {msg}");
        }

        let missing = ex_with(&[]);
        let err = resolve_members_list(&missing).expect_err("an absent member list must fail");
        assert!(err.to_string().contains("CamelRedis.Members"), "{err}");

        let malformed = ex_with(&[("CamelRedis.Members", serde_json::json!(["a", 7]))]);
        let err = resolve_members_list(&malformed).expect_err("a non-string entry must fail");
        let msg = err.to_string();
        assert!(msg.contains("CamelRedis.Members"), "{msg}");
        assert!(msg.contains("index 1"), "{msg}");
    }

    #[test]
    fn resolve_sintercard_limit_domain() {
        let ok = ex_with(&[("CamelRedis.Limit", serde_json::json!(10))]);
        assert_eq!(
            resolve_sintercard_limit(&ok).expect("a non-negative limit must resolve"),
            Some(10)
        );

        let negative = ex_with(&[("CamelRedis.Limit", serde_json::json!(-1))]);
        let err = resolve_sintercard_limit(&negative).expect_err("a negative limit must fail");
        let msg = err.to_string();
        assert!(msg.contains("CamelRedis.Limit"), "{msg}");
        assert!(msg.contains("non-negative"), "{msg}");

        let absent = ex_with(&[]);
        assert_eq!(
            resolve_sintercard_limit(&absent).expect("an absent limit is optional"),
            None
        );
    }
}
