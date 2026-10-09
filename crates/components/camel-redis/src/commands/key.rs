use super::{
    get_str_header, get_str_vec_header, get_u64_header, require_key, require_positive,
    u64_to_i64_checked,
};
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

/// Resolves the `MOVE` destination database index.
///
/// A missing `CamelRedis.Db` deliberately defaults to `0` (the default
/// database is a valid MOVE target, not a deletion trap). The `u64 -> i64`
/// conversion IS guarded: `MOVE key >i64::MAX` must not wrap into a
/// negative db index (Task 354).
pub(crate) fn resolve_move_db(exchange: &Exchange) -> Result<i64, CamelError> {
    const HEADER: &str = "CamelRedis.Db";
    let db = get_u64_header(exchange, HEADER)?.unwrap_or(0);
    u64_to_i64_checked(db, HEADER)
}

/// Resolves the `EXPIRE`/`PEXPIRE` TTL.
///
/// `CamelRedis.Timeout` is REQUIRED and must be positive: a zero TTL makes
/// Redis delete the key immediately. Deletion is an explicit `DEL`, never a
/// misconfigured zero. The `u64 -> i64` conversion is guarded so an
/// oversized TTL cannot wrap into a negative (already-past) expiration.
pub(crate) fn resolve_expire_timeout(exchange: &Exchange) -> Result<i64, CamelError> {
    const HEADER: &str = "CamelRedis.Timeout";
    let timeout = require_positive(
        get_u64_header(exchange, HEADER)?,
        HEADER,
        "a positive TTL is required for EXPIRE/PEXPIRE; set a positive value or use DEL to delete a key",
    )?;
    u64_to_i64_checked(timeout, HEADER)
}

/// Resolves the `EXPIREAT`/`PEXPIREAT` absolute expiry timestamp.
///
/// `CamelRedis.Timestamp` is REQUIRED and must be positive: a zero
/// timestamp is in the past, so Redis deletes the key immediately. Explicit
/// positive past timestamps keep Redis semantics (an intentional expiry).
/// The `u64 -> i64` conversion is guarded so an oversized timestamp cannot
/// wrap into a negative value.
pub(crate) fn resolve_expire_timestamp(exchange: &Exchange) -> Result<i64, CamelError> {
    const HEADER: &str = "CamelRedis.Timestamp";
    let timestamp = require_positive(
        get_u64_header(exchange, HEADER)?,
        HEADER,
        "a positive future Unix timestamp is required for EXPIREAT/PEXPIREAT",
    )?;
    u64_to_i64_checked(timestamp, HEADER)
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
    Ok((require_key(exchange)?, resolve_move_db(exchange)?))
}

pub(crate) fn json_from_move_result(result: i64) -> serde_json::Value {
    serde_json::json!(result == 1)
}

#[allow(dead_code)]
pub(crate) fn build_redis_cmd(
    cmd: &RedisCommand,
    exchange: &Exchange,
) -> Result<redis::Cmd, CamelError> {
    if !is_key_command(cmd) {
        return Err(CamelError::ProcessorError("Not a key command".into()));
    }

    let redis_cmd = match cmd {
        RedisCommand::Del => {
            let keys = resolve_del_keys(exchange)?;
            let mut c = redis::cmd("DEL");
            for key in keys {
                c.arg(key);
            }
            c
        }
        RedisCommand::Exists => {
            let key = require_key(exchange)?;
            let mut c = redis::cmd("EXISTS");
            c.arg(key);
            c
        }
        RedisCommand::Expire => {
            let key = require_key(exchange)?;
            let secs = resolve_expire_timeout(exchange)?;
            let mut c = redis::cmd("EXPIRE");
            c.arg(key).arg(secs);
            c
        }
        RedisCommand::Expireat => {
            let key = require_key(exchange)?;
            let ts = resolve_expire_timestamp(exchange)?;
            let mut c = redis::cmd("EXPIREAT");
            c.arg(key).arg(ts);
            c
        }
        RedisCommand::Pexpire => {
            let key = require_key(exchange)?;
            let ms = resolve_expire_timeout(exchange)?;
            let mut c = redis::cmd("PEXPIRE");
            c.arg(key).arg(ms);
            c
        }
        RedisCommand::Pexpireat => {
            let key = require_key(exchange)?;
            let ts = resolve_expire_timestamp(exchange)?;
            let mut c = redis::cmd("PEXPIREAT");
            c.arg(key).arg(ts);
            c
        }
        RedisCommand::Ttl => {
            let key = require_key(exchange)?;
            let mut c = redis::cmd("TTL");
            c.arg(key);
            c
        }
        RedisCommand::Keys => {
            let pattern = resolve_keys_pattern(exchange);
            let mut c = redis::cmd("KEYS");
            c.arg(pattern);
            c
        }
        RedisCommand::Persist => {
            let key = require_key(exchange)?;
            let mut c = redis::cmd("PERSIST");
            c.arg(key);
            c
        }
        RedisCommand::Rename => {
            let (key, dest) = resolve_rename_operands(exchange)?;
            let mut c = redis::cmd("RENAME");
            c.arg(key).arg(dest);
            c
        }
        RedisCommand::Renamenx => {
            let (key, dest) = resolve_rename_operands(exchange)?;
            let mut c = redis::cmd("RENAMENX");
            c.arg(key).arg(dest);
            c
        }
        RedisCommand::Type => {
            let key = require_key(exchange)?;
            let mut c = redis::cmd("TYPE");
            c.arg(key);
            c
        }
        RedisCommand::Move => {
            let (key, db) = resolve_move_operands(exchange)?;
            let mut c = redis::cmd("MOVE");
            c.arg(key).arg(db);
            c
        }
        RedisCommand::Sort => {
            let key = require_key(exchange)?;
            let mut c = redis::cmd("SORT");
            c.arg(key);
            c
        }
        _ => unreachable!("non-key commands rejected above"),
    };

    Ok(redis_cmd)
}

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    if !is_key_command(cmd) {
        return Err(CamelError::ProcessorError("Not a key command".into()));
    }

    let result: serde_json::Value = match cmd {
        RedisCommand::Exists => {
            let key = require_key(exchange)?;
            let n: bool = conn
                .exists(&key)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("EXISTS", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Del => {
            let keys = resolve_del_keys(exchange)?;
            let n: i64 = conn
                .del(&keys)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("DEL", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Expire => {
            let key = require_key(exchange)?;
            let secs = resolve_expire_timeout(exchange)?;
            let ok: bool = conn
                .expire(&key, secs)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("EXPIRE", e))?;
            serde_json::json!(ok)
        }
        RedisCommand::Expireat => {
            let key = require_key(exchange)?;
            let ts = resolve_expire_timestamp(exchange)?;
            let ok: bool = conn
                .expire_at(&key, ts)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("EXPIREAT", e))?;
            serde_json::json!(ok)
        }
        RedisCommand::Pexpire => {
            let key = require_key(exchange)?;
            let ms = resolve_expire_timeout(exchange)?;
            let ok: bool = conn
                .pexpire(&key, ms)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("PEXPIRE", e))?;
            serde_json::json!(ok)
        }
        RedisCommand::Pexpireat => {
            let key = require_key(exchange)?;
            let ts = resolve_expire_timestamp(exchange)?;
            let ok: bool = conn
                .pexpire_at(&key, ts)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("PEXPIREAT", e))?;
            serde_json::json!(ok)
        }
        RedisCommand::Ttl => {
            let key = require_key(exchange)?;
            let n: i64 = conn
                .ttl(&key)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("TTL", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Keys => {
            let pattern = resolve_keys_pattern(exchange);
            let keys: Vec<String> = conn
                .keys(pattern)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("KEYS", e))?;
            serde_json::json!(keys)
        }
        RedisCommand::Rename => {
            let (key, dest) = resolve_rename_operands(exchange)?;
            conn.rename::<_, _, ()>(&key, dest)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("RENAME", e))?;
            serde_json::Value::Null
        }
        RedisCommand::Renamenx => {
            let (key, dest) = resolve_rename_operands(exchange)?;
            let ok: bool = conn
                .rename_nx(&key, dest)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("RENAMENX", e))?;
            serde_json::json!(ok)
        }
        RedisCommand::Type => {
            let key = require_key(exchange)?;
            // redis-rs returns a String for TYPE
            let t: String = redis::cmd("TYPE")
                .arg(&key)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("TYPE", e))?;
            serde_json::Value::String(t)
        }
        RedisCommand::Persist => {
            let key = require_key(exchange)?;
            let ok: bool = conn
                .persist(&key)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("PERSIST", e))?;
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
                .map_err(|e| crate::transport_error::redis_error_to_camel("MOVE", e))?;
            json_from_move_result(ok)
        }
        RedisCommand::Sort => {
            let key = require_key(exchange)?;
            // Basic SORT — returns sorted list using raw command
            let vals: Vec<String> = redis::cmd("SORT")
                .arg(&key)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SORT", e))?;
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

    fn ex_with(headers: &[(&str, serde_json::Value)]) -> Exchange {
        let mut msg = Message::default();
        for (k, v) in headers {
            msg.set_header(*k, v.clone());
        }
        Exchange::new(msg)
    }

    /// Task 354: a missing or zero `CamelRedis.Timeout` deletes a key under
    /// `EXPIRE`/`PEXPIRE` semantics, so it must fail closed locally naming
    /// the header and pointing at the explicit `DEL` command.
    fn assert_ttl_error(err: &CamelError) {
        let msg = err.to_string();
        assert!(msg.contains("CamelRedis.Timeout"), "{msg}");
        assert!(msg.contains("positive"), "{msg}");
        assert!(msg.contains("DEL"), "{msg}");
    }

    /// Task 354: a missing or zero `CamelRedis.Timestamp` deletes a key under
    /// `EXPIREAT`/`PEXPIREAT` semantics (a zero timestamp is in the past), so
    /// it must fail closed locally with guidance for a positive timestamp.
    fn assert_timestamp_error(err: &CamelError) {
        let msg = err.to_string();
        assert!(msg.contains("CamelRedis.Timestamp"), "{msg}");
        assert!(msg.contains("positive"), "{msg}");
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
        assert_eq!(resolve_move_db(&ex).unwrap(), 3);
    }

    #[test]
    fn test_move_db_garbage_is_named_error() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Db", serde_json::json!("abc"));
        let ex = Exchange::new(msg);
        let err = resolve_move_db(&ex).expect_err("garbage db must fail");
        let text = err.to_string();
        assert!(text.contains("CamelRedis.Db"), "{text}");
        assert!(text.contains("abc"), "{text}");
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
    fn test_resolve_expire_timeout_rejects_missing_and_zero() {
        for ex in [
            Exchange::new(Message::default()),
            ex_with(&[("CamelRedis.Timeout", serde_json::json!(0u64))]),
            ex_with(&[("CamelRedis.Timeout", serde_json::json!("0"))]),
        ] {
            let err = resolve_expire_timeout(&ex).expect_err("missing/zero TTL must fail closed");
            assert_ttl_error(&err);
        }
    }

    #[test]
    fn test_resolve_expire_timeout_values() {
        let ex = ex_with(&[("CamelRedis.Timeout", serde_json::json!(9))]);
        assert_eq!(resolve_expire_timeout(&ex).unwrap(), 9);

        let sex = ex_with(&[("CamelRedis.Timeout", serde_json::json!(" 9 "))]);
        assert_eq!(resolve_expire_timeout(&sex).unwrap(), 9);
    }

    /// Task 354: a `u64` TTL above `i64::MAX` must be rejected, never cast
    /// with `as i64` (which wraps to a negative value Redis reads as an
    /// already-past, immediate-deletion expiration).
    #[test]
    fn test_resolve_expire_timeout_rejects_above_i64_max() {
        for raw in [
            serde_json::json!(9223372036854775808u64),
            serde_json::json!("9223372036854775808"),
        ] {
            let ex = ex_with(&[("CamelRedis.Timeout", raw.clone())]);
            let err = resolve_expire_timeout(&ex)
                .expect_err("u64 TTL above i64::MAX must not wrap into a negative expiration");
            let msg = err.to_string();
            assert!(msg.contains("CamelRedis.Timeout"), "{msg}");
            assert!(msg.contains("i64"), "{msg}");
        }
    }

    #[test]
    fn test_resolve_expire_timeout_garbage_is_named_error() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Timeout", serde_json::json!("abc"));
        let ex = Exchange::new(msg);
        let err = resolve_expire_timeout(&ex).expect_err("garbage timeout must fail");
        let text = err.to_string();
        assert!(text.contains("CamelRedis.Timeout"), "{text}");
        assert!(text.contains("abc"), "{text}");
    }

    #[test]
    fn test_resolve_expire_timestamp_rejects_missing_and_zero() {
        for ex in [
            Exchange::new(Message::default()),
            ex_with(&[("CamelRedis.Timestamp", serde_json::json!(0u64))]),
            ex_with(&[("CamelRedis.Timestamp", serde_json::json!("0"))]),
        ] {
            let err =
                resolve_expire_timestamp(&ex).expect_err("missing/zero timestamp must fail closed");
            assert_timestamp_error(&err);
        }
    }

    #[test]
    fn test_resolve_expire_timestamp_values() {
        let ex = ex_with(&[("CamelRedis.Timestamp", serde_json::json!(123))]);
        assert_eq!(resolve_expire_timestamp(&ex).unwrap(), 123);

        let sex = ex_with(&[("CamelRedis.Timestamp", serde_json::json!(" 123 "))]);
        assert_eq!(resolve_expire_timestamp(&sex).unwrap(), 123);
    }

    /// Task 354: a `u64` timestamp above `i64::MAX` must be rejected, never
    /// cast with `as i64` (the wrap goes negative, which Redis reads as an
    /// already-past timestamp and deletes the key).
    #[test]
    fn test_resolve_expire_timestamp_rejects_above_i64_max() {
        for raw in [
            serde_json::json!(9223372036854775808u64),
            serde_json::json!("9223372036854775808"),
        ] {
            let ex = ex_with(&[("CamelRedis.Timestamp", raw.clone())]);
            let err = resolve_expire_timestamp(&ex)
                .expect_err("u64 timestamp above i64::MAX must not wrap negative");
            let msg = err.to_string();
            assert!(msg.contains("CamelRedis.Timestamp"), "{msg}");
            assert!(msg.contains("i64"), "{msg}");
        }
    }

    #[test]
    fn test_resolve_expire_timestamp_garbage_is_named_error() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Timestamp", serde_json::json!("soon"));
        let ex = Exchange::new(msg);
        let err = resolve_expire_timestamp(&ex).expect_err("garbage timestamp must fail");
        let text = err.to_string();
        assert!(text.contains("CamelRedis.Timestamp"), "{text}");
        assert!(text.contains("soon"), "{text}");
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

    // --- build_redis_cmd tests ---

    #[test]
    fn test_build_redis_cmd_del_with_keys_header() {
        let ex = ex_with(&[("CamelRedis.Keys", serde_json::json!(["k1", "k2", "k3"]))]);
        let cmd = build_redis_cmd(&RedisCommand::Del, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "DEL");
        assert_eq!(cmd_args(&cmd), vec!["k1", "k2", "k3"]);
    }

    #[test]
    fn test_build_redis_cmd_del_with_single_key() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
        let cmd = build_redis_cmd(&RedisCommand::Del, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "DEL");
        assert_eq!(cmd_args(&cmd), vec!["mykey"]);
    }

    #[test]
    fn test_build_redis_cmd_del_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Del, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_exists() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
        let cmd = build_redis_cmd(&RedisCommand::Exists, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "EXISTS");
        assert_eq!(cmd_args(&cmd), vec!["mykey"]);
    }

    #[test]
    fn test_build_redis_cmd_exists_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Exists, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_expire() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Timeout", serde_json::json!(60u64)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Expire, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "EXPIRE");
        assert_eq!(cmd_args(&cmd), vec!["mykey", "60"]);
    }

    #[test]
    fn test_build_redis_cmd_expire_missing_timeout_fails_before_construction() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
        let err = build_redis_cmd(&RedisCommand::Expire, &ex)
            .expect_err("EXPIRE with a missing TTL must fail before construction");
        assert_ttl_error(&err);
    }

    // ── 354 task 2: EXPIRE/PEXPIRE reject absent/zero TTL ───────────────────

    #[test]
    fn test_build_redis_cmd_expire_family_rejects_absent_zero_timeout() {
        for cmd in [RedisCommand::Expire, RedisCommand::Pexpire] {
            for timeout in [
                None,
                Some(serde_json::json!(0u64)),
                Some(serde_json::json!("0")),
            ] {
                let mut headers = vec![("CamelRedis.Key", serde_json::json!("mykey"))];
                if let Some(value) = &timeout {
                    headers.push(("CamelRedis.Timeout", value.clone()));
                }
                let ex = ex_with(&headers);
                let err = build_redis_cmd(&cmd, &ex).expect_err(
                    "EXPIRE/PEXPIRE with absent/zero TTL must fail before construction",
                );
                assert_ttl_error(&err);
            }
        }
    }

    #[test]
    fn test_build_redis_cmd_expire_family_numeric_string_timeout_encodes_ttl() {
        for cmd in [RedisCommand::Expire, RedisCommand::Pexpire] {
            let ex = ex_with(&[
                ("CamelRedis.Key", serde_json::json!("mykey")),
                ("CamelRedis.Timeout", serde_json::json!(" 45 ")),
            ]);
            let built =
                build_redis_cmd(&cmd, &ex).expect("trimmed numeric string TTL must be accepted");
            let expected = match cmd {
                RedisCommand::Expire => "EXPIRE",
                RedisCommand::Pexpire => "PEXPIRE",
                other => panic!("unexpected command in table: {other:?}"),
            };
            assert_eq!(cmd_name(&built), expected);
            assert_eq!(cmd_args(&built), vec!["mykey", "45"]);
        }
    }

    #[test]
    fn test_build_redis_cmd_expire_family_rejects_above_i64_max() {
        for cmd in [RedisCommand::Expire, RedisCommand::Pexpire] {
            for raw in [
                serde_json::json!(9223372036854775808u64),
                serde_json::json!("9223372036854775808"),
            ] {
                let ex = ex_with(&[
                    ("CamelRedis.Key", serde_json::json!("mykey")),
                    ("CamelRedis.Timeout", raw.clone()),
                ]);
                let err = build_redis_cmd(&cmd, &ex)
                    .expect_err("TTL above i64::MAX must not wrap into a negative expiration");
                let msg = err.to_string();
                assert!(msg.contains("CamelRedis.Timeout"), "{msg}");
                assert!(msg.contains("i64"), "{msg}");
            }
        }
    }

    // ── 354 task 2: EXPIREAT/PEXPIREAT reject absent/zero/overflowing ts ────

    #[test]
    fn test_build_redis_cmd_expireat_family_rejects_absent_zero_timestamp() {
        for cmd in [RedisCommand::Expireat, RedisCommand::Pexpireat] {
            for timestamp in [
                None,
                Some(serde_json::json!(0u64)),
                Some(serde_json::json!("0")),
            ] {
                let mut headers = vec![("CamelRedis.Key", serde_json::json!("mykey"))];
                if let Some(value) = &timestamp {
                    headers.push(("CamelRedis.Timestamp", value.clone()));
                }
                let ex = ex_with(&headers);
                let err = build_redis_cmd(&cmd, &ex).expect_err(
                    "EXPIREAT/PEXPIREAT with absent/zero timestamp must fail before construction",
                );
                assert_timestamp_error(&err);
            }
        }
    }

    #[test]
    fn test_build_redis_cmd_expireat_family_rejects_timestamp_above_i64_max() {
        for cmd in [RedisCommand::Expireat, RedisCommand::Pexpireat] {
            for raw in [
                serde_json::json!(9223372036854775808u64),
                serde_json::json!("9223372036854775808"),
            ] {
                let ex = ex_with(&[
                    ("CamelRedis.Key", serde_json::json!("mykey")),
                    ("CamelRedis.Timestamp", raw.clone()),
                ]);
                let err = build_redis_cmd(&cmd, &ex)
                    .expect_err("timestamp above i64::MAX must not wrap negative");
                let msg = err.to_string();
                assert!(msg.contains("CamelRedis.Timestamp"), "{msg}");
                assert!(msg.contains("i64"), "{msg}");
            }
        }
    }

    #[test]
    fn test_build_redis_cmd_expireat_family_valid_positive_timestamp_succeeds() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Timestamp", serde_json::json!(1700000000u64)),
        ]);
        let built = build_redis_cmd(&RedisCommand::Expireat, &ex)
            .expect("positive EXPIREAT timestamp must be accepted");
        assert_eq!(cmd_name(&built), "EXPIREAT");
        assert_eq!(cmd_args(&built), vec!["mykey", "1700000000"]);

        let sex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Timestamp", serde_json::json!("1700000000000")),
        ]);
        let sbuilt = build_redis_cmd(&RedisCommand::Pexpireat, &sex)
            .expect("positive PEXPIREAT numeric-string timestamp must be accepted");
        assert_eq!(cmd_name(&sbuilt), "PEXPIREAT");
        assert_eq!(cmd_args(&sbuilt), vec!["mykey", "1700000000000"]);
    }

    // ── 354 task 2: MOVE Db u64 -> i64 overflow ─────────────────────────────

    #[test]
    fn test_resolve_move_db_rejects_above_i64_max() {
        let ex = ex_with(&[("CamelRedis.Db", serde_json::json!(9223372036854775808u64))]);
        let err = resolve_move_db(&ex).expect_err("Db above i64::MAX must not wrap negative");
        let msg = err.to_string();
        assert!(msg.contains("CamelRedis.Db"), "{msg}");
        assert!(msg.contains("i64"), "{msg}");
    }

    #[test]
    fn test_build_redis_cmd_move_rejects_db_above_i64_max() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Db", serde_json::json!("9223372036854775808")),
        ]);
        let err = build_redis_cmd(&RedisCommand::Move, &ex)
            .expect_err("MOVE with Db above i64::MAX must fail before construction");
        let msg = err.to_string();
        assert!(msg.contains("CamelRedis.Db"), "{msg}");
        assert!(msg.contains("i64"), "{msg}");
    }

    #[test]
    fn test_build_redis_cmd_expire_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Expire, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_expire_garbage_timeout_fails_before_construction() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Timeout", serde_json::json!("abc")),
        ]);
        let err = build_redis_cmd(&RedisCommand::Expire, &ex)
            .expect_err("garbage timeout must fail before EXPIRE construction");
        let msg = err.to_string();
        assert!(msg.contains("CamelRedis.Timeout"), "{msg}");
        assert!(msg.contains("abc"), "{msg}");
    }

    #[test]
    fn test_build_redis_cmd_expireat() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Timestamp", serde_json::json!(1700000000u64)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Expireat, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "EXPIREAT");
        assert_eq!(cmd_args(&cmd), vec!["mykey", "1700000000"]);
    }

    #[test]
    fn test_build_redis_cmd_expireat_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Expireat, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_pexpire() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Timeout", serde_json::json!(5000u64)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Pexpire, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "PEXPIRE");
        assert_eq!(cmd_args(&cmd), vec!["mykey", "5000"]);
    }

    #[test]
    fn test_build_redis_cmd_pexpire_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Pexpire, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_pexpireat() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Timestamp", serde_json::json!(1700000000000u64)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Pexpireat, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "PEXPIREAT");
        assert_eq!(cmd_args(&cmd), vec!["mykey", "1700000000000"]);
    }

    #[test]
    fn test_build_redis_cmd_pexpireat_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Pexpireat, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_ttl() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
        let cmd = build_redis_cmd(&RedisCommand::Ttl, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "TTL");
        assert_eq!(cmd_args(&cmd), vec!["mykey"]);
    }

    #[test]
    fn test_build_redis_cmd_ttl_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Ttl, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_keys_default_pattern() {
        let ex = Exchange::new(Message::default());
        let cmd = build_redis_cmd(&RedisCommand::Keys, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "KEYS");
        assert_eq!(cmd_args(&cmd), vec!["*"]);
    }

    #[test]
    fn test_build_redis_cmd_keys_custom_pattern() {
        let ex = ex_with(&[("CamelRedis.Pattern", serde_json::json!("user:*"))]);
        let cmd = build_redis_cmd(&RedisCommand::Keys, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "KEYS");
        assert_eq!(cmd_args(&cmd), vec!["user:*"]);
    }

    #[test]
    fn test_build_redis_cmd_persist() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
        let cmd = build_redis_cmd(&RedisCommand::Persist, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "PERSIST");
        assert_eq!(cmd_args(&cmd), vec!["mykey"]);
    }

    #[test]
    fn test_build_redis_cmd_persist_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Persist, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_rename() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("oldkey")),
            ("CamelRedis.Destination", serde_json::json!("newkey")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Rename, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "RENAME");
        assert_eq!(cmd_args(&cmd), vec!["oldkey", "newkey"]);
    }

    #[test]
    fn test_build_redis_cmd_rename_missing_key() {
        let ex = ex_with(&[("CamelRedis.Destination", serde_json::json!("newkey"))]);
        assert!(build_redis_cmd(&RedisCommand::Rename, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_rename_missing_destination() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("oldkey"))]);
        assert!(build_redis_cmd(&RedisCommand::Rename, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_renamenx() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("oldkey")),
            ("CamelRedis.Destination", serde_json::json!("newkey")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Renamenx, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "RENAMENX");
        assert_eq!(cmd_args(&cmd), vec!["oldkey", "newkey"]);
    }

    #[test]
    fn test_build_redis_cmd_renamenx_missing_key() {
        let ex = ex_with(&[("CamelRedis.Destination", serde_json::json!("newkey"))]);
        assert!(build_redis_cmd(&RedisCommand::Renamenx, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_type() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
        let cmd = build_redis_cmd(&RedisCommand::Type, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "TYPE");
        assert_eq!(cmd_args(&cmd), vec!["mykey"]);
    }

    #[test]
    fn test_build_redis_cmd_type_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Type, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_move() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Db", serde_json::json!(3u64)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Move, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "MOVE");
        assert_eq!(cmd_args(&cmd), vec!["mykey", "3"]);
    }

    #[test]
    fn test_build_redis_cmd_move_default_db() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
        let cmd = build_redis_cmd(&RedisCommand::Move, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "MOVE");
        assert_eq!(cmd_args(&cmd), vec!["mykey", "0"]);
    }

    #[test]
    fn test_build_redis_cmd_move_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Move, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_sort() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mylist"))]);
        let cmd = build_redis_cmd(&RedisCommand::Sort, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "SORT");
        assert_eq!(cmd_args(&cmd), vec!["mylist"]);
    }

    #[test]
    fn test_build_redis_cmd_sort_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Sort, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_rejects_non_key() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("k"))]);
        assert!(build_redis_cmd(&RedisCommand::Set, &ex).is_err());
    }
}
