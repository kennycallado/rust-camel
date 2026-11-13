use super::{
    body_variant_name, get_i64_header, get_str_header, get_str_vec_header, require_key,
    require_value, resolve_value_bytes, value_to_redis_arg,
};
use crate::HEADER_VALUE;
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
        .map(|(f, v)| (f.clone(), value_to_redis_arg(v)))
        .collect::<Vec<_>>())
}

/// Resolves the `HMSET` field/value pairs as raw bytes.
///
/// `CamelRedis.Values` keeps precedence and encodes exactly as before
/// ([`resolve_hash_values_map`]); when the header is absent the map comes from
/// the exchange body: a JSON object body is used as-is, and `Text`/`Bytes`
/// bodies are parsed as a JSON object. Any other body form is rejected before
/// the command reaches Redis, naming `HMSET` and the rejected variant.
pub(crate) fn resolve_hmset_values(
    exchange: &Exchange,
) -> Result<Vec<(String, Vec<u8>)>, CamelError> {
    if exchange.input.header("CamelRedis.Values").is_some() {
        return Ok(resolve_hash_values_map(exchange)?
            .into_iter()
            .map(|(field, value)| (field, value.into_bytes()))
            .collect());
    }

    Ok(hmset_body_map(&exchange.input.body)?
        .iter()
        .map(|(field, value)| (field.clone(), value_to_redis_arg(value).into_bytes()))
        .collect())
}

/// Reads the `HMSET` field/value map from an exchange body.
fn hmset_body_map(body: &Body) -> Result<serde_json::Map<String, serde_json::Value>, CamelError> {
    match body {
        Body::Json(serde_json::Value::Object(map)) => Ok(map.clone()),
        Body::Text(text) => parse_hmset_body(text),
        Body::Bytes(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => parse_hmset_body(text),
            Err(err) => Err(CamelError::ProcessorError(format!(
                "HMSET value: the Bytes body is not valid UTF-8: {err}"
            ))),
        },
        Body::Json(_) => Err(unencodable_hmset_body("Body::Json")),
        Body::Xml(_) => Err(unencodable_hmset_body("Body::Xml")),
        Body::Empty => Err(unencodable_hmset_body("Body::Empty")),
        Body::Stream(_) => Err(unencodable_hmset_body("Body::Stream")),
        other => Err(unencodable_hmset_body(&body_variant_name(other))),
    }
}

/// Parses a `Text`/`Bytes` body into the `HMSET` field/value map.
fn parse_hmset_body(text: &str) -> Result<serde_json::Map<String, serde_json::Value>, CamelError> {
    serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(text).map_err(|e| {
        CamelError::ProcessorError(format!(
            "HMSET value: the exchange body is not a JSON object: {e}"
        ))
    })
}

/// Named error for a body form that cannot carry `HMSET` field/value pairs.
fn unencodable_hmset_body(variant: &str) -> CamelError {
    CamelError::ProcessorError(format!(
        "HMSET value: the exchange body is {variant}; provide a JSON object body or set CamelRedis.Values"
    ))
}

pub(crate) fn resolve_hash_increment(exchange: &Exchange) -> Result<i64, CamelError> {
    Ok(get_i64_header(exchange, "CamelRedis.Increment")?.unwrap_or(1))
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

#[allow(dead_code)]
pub(crate) fn build_redis_cmd(
    cmd: &RedisCommand,
    exchange: &Exchange,
) -> Result<redis::Cmd, CamelError> {
    if !is_hash_command(cmd) {
        return Err(CamelError::ProcessorError("Not a hash command".into()));
    }

    let redis_cmd = match cmd {
        RedisCommand::Hset => {
            let key = require_key(exchange)?;
            let field = resolve_hash_field(exchange)?;
            let value = require_value(exchange)?;
            let mut c = redis::cmd("HSET");
            c.arg(key).arg(field).arg(value_to_redis_arg(&value));
            c
        }
        RedisCommand::Hget => {
            let (key, field) = resolve_hash_field_operands(exchange)?;
            let mut c = redis::cmd("HGET");
            c.arg(key).arg(field);
            c
        }
        RedisCommand::Hsetnx => {
            let key = require_key(exchange)?;
            let field = resolve_hash_field(exchange)?;
            let value = require_value(exchange)?;
            let mut c = redis::cmd("HSETNX");
            c.arg(key).arg(field).arg(value_to_redis_arg(&value));
            c
        }
        RedisCommand::Hmset => {
            let key = require_key(exchange)?;
            let values = resolve_hash_values_map(exchange)?;
            let mut c = redis::cmd("HMSET");
            c.arg(key);
            for (f, v) in values {
                c.arg(f).arg(v);
            }
            c
        }
        RedisCommand::Hmget => {
            let key = require_key(exchange)?;
            let fields = resolve_hash_fields(exchange)?;
            let mut c = redis::cmd("HMGET");
            c.arg(key);
            for field in fields {
                c.arg(field);
            }
            c
        }
        RedisCommand::Hdel => {
            let (key, field) = resolve_hash_field_operands(exchange)?;
            let mut c = redis::cmd("HDEL");
            c.arg(key).arg(field);
            c
        }
        RedisCommand::Hexists => {
            let (key, field) = resolve_hash_field_operands(exchange)?;
            let mut c = redis::cmd("HEXISTS");
            c.arg(key).arg(field);
            c
        }
        RedisCommand::Hlen => {
            let key = require_key(exchange)?;
            let mut c = redis::cmd("HLEN");
            c.arg(key);
            c
        }
        RedisCommand::Hkeys => {
            let key = require_key(exchange)?;
            let mut c = redis::cmd("HKEYS");
            c.arg(key);
            c
        }
        RedisCommand::Hvals => {
            let key = require_key(exchange)?;
            let mut c = redis::cmd("HVALS");
            c.arg(key);
            c
        }
        RedisCommand::Hgetall => {
            let key = require_key(exchange)?;
            let mut c = redis::cmd("HGETALL");
            c.arg(key);
            c
        }
        RedisCommand::Hincrby => {
            let (key, field) = resolve_hash_field_operands(exchange)?;
            let by = resolve_hash_increment(exchange)?;
            let mut c = redis::cmd("HINCRBY");
            c.arg(key).arg(field).arg(by);
            c
        }
        _ => unreachable!("non-hash commands rejected above"),
    };

    Ok(redis_cmd)
}

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    if !is_hash_command(cmd) {
        return Err(CamelError::ProcessorError("Not a hash command".into()));
    }

    let result: serde_json::Value = match cmd {
        RedisCommand::Hset => {
            let key = require_key(exchange)?;
            let field = resolve_hash_field(exchange)?;
            let value = resolve_value_bytes(exchange, "HSET", HEADER_VALUE)?;
            let n: i64 = conn
                .hset(&key, field, &value)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("HSET", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Hget => {
            let (key, field) = resolve_hash_field_operands(exchange)?;
            let val: Option<String> = conn
                .hget(&key, field)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("HGET", e))?;
            json_from_optional_hash_value(val)
        }
        RedisCommand::Hsetnx => {
            let key = require_key(exchange)?;
            let field = resolve_hash_field(exchange)?;
            let value = resolve_value_bytes(exchange, "HSETNX", HEADER_VALUE)?;
            let ok: bool = conn
                .hset_nx(&key, field, &value)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("HSETNX", e))?;
            serde_json::json!(ok)
        }
        RedisCommand::Hmset => {
            let key = require_key(exchange)?;
            let values = resolve_hmset_values(exchange)?;
            conn.hset_multiple::<_, _, _, ()>(&key, &values)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("HMSET", e))?;
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
                .map_err(|e| crate::transport_error::redis_error_to_camel("HMGET", e))?;
            json_from_optional_hash_values(vals)
        }
        RedisCommand::Hdel => {
            let (key, field) = resolve_hash_field_operands(exchange)?;
            let n: i64 = conn
                .hdel(&key, field)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("HDEL", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Hexists => {
            let (key, field) = resolve_hash_field_operands(exchange)?;
            let ok: bool = conn
                .hexists(&key, field)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("HEXISTS", e))?;
            serde_json::json!(ok)
        }
        RedisCommand::Hlen => {
            let key = require_key(exchange)?;
            let n: i64 = conn
                .hlen(&key)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("HLEN", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Hkeys => {
            let key = require_key(exchange)?;
            let keys: Vec<String> = conn
                .hkeys(&key)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("HKEYS", e))?;
            serde_json::json!(keys)
        }
        RedisCommand::Hvals => {
            let key = require_key(exchange)?;
            let vals: Vec<String> = conn
                .hvals(&key)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("HVALS", e))?;
            serde_json::json!(vals)
        }
        RedisCommand::Hgetall => {
            let key = require_key(exchange)?;
            let map: std::collections::HashMap<String, String> = conn
                .hgetall(&key)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("HGETALL", e))?;
            serde_json::json!(map)
        }
        RedisCommand::Hincrby => {
            let (key, field) = resolve_hash_field_operands(exchange)?;
            let by = resolve_hash_increment(exchange)?;
            let n: i64 = conn
                .hincr(&key, field, by)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("HINCRBY", e))?;
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
        assert!(crate::commands::get_str_header(&ex, "CamelRedis.Field").is_none());
    }

    #[test]
    fn test_hset_has_key_field_value() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Field", serde_json::json!("myfield")),
            (HEADER_VALUE, serde_json::json!("myvalue")),
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
            crate::commands::get_i64_header(&ex, "CamelRedis.Increment").unwrap(),
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
        assert!(values.iter().any(|(k, v)| k == "f1" && v == "v1"));
        assert!(values.iter().any(|(k, v)| k == "f2" && v == "v2"));
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
        assert_eq!(resolve_hash_increment(&ex_default).unwrap(), 1);

        let mut msg = Message::default();
        msg.set_header("CamelRedis.Increment", serde_json::json!(9));
        let ex = Exchange::new(msg);
        assert_eq!(resolve_hash_increment(&ex).unwrap(), 9);

        let mut smsg = Message::default();
        smsg.set_header("CamelRedis.Increment", serde_json::json!(" 9 "));
        let sex = Exchange::new(smsg);
        assert_eq!(resolve_hash_increment(&sex).unwrap(), 9);
    }

    #[test]
    fn test_resolve_hash_increment_garbage_is_named_error() {
        let mut msg = Message::default();
        msg.set_header("CamelRedis.Increment", serde_json::json!("abc"));
        let ex = Exchange::new(msg);
        let err = resolve_hash_increment(&ex).expect_err("garbage increment must fail");
        let text = err.to_string();
        assert!(text.contains("CamelRedis.Increment"), "{text}");
        assert!(text.contains("abc"), "{text}");
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
    fn test_build_redis_cmd_hset() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Field", serde_json::json!("myfield")),
            (HEADER_VALUE, serde_json::json!("myvalue")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Hset, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "HSET");
        assert_eq!(cmd_args(&cmd), vec!["mykey", "myfield", "myvalue"]);
    }

    #[test]
    fn test_build_redis_cmd_hset_missing_key() {
        let ex = ex_with(&[
            ("CamelRedis.Field", serde_json::json!("f")),
            (HEADER_VALUE, serde_json::json!("v")),
        ]);
        assert!(build_redis_cmd(&RedisCommand::Hset, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_hset_missing_field() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("k")),
            (HEADER_VALUE, serde_json::json!("v")),
        ]);
        assert!(build_redis_cmd(&RedisCommand::Hset, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_hset_missing_value() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("k")),
            ("CamelRedis.Field", serde_json::json!("f")),
        ]);
        assert!(build_redis_cmd(&RedisCommand::Hset, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_hget() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Field", serde_json::json!("myfield")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Hget, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "HGET");
        assert_eq!(cmd_args(&cmd), vec!["mykey", "myfield"]);
    }

    #[test]
    fn test_build_redis_cmd_hget_missing_key() {
        let ex = ex_with(&[("CamelRedis.Field", serde_json::json!("f"))]);
        assert!(build_redis_cmd(&RedisCommand::Hget, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_hget_missing_field() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("k"))]);
        assert!(build_redis_cmd(&RedisCommand::Hget, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_hsetnx() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Field", serde_json::json!("myfield")),
            (HEADER_VALUE, serde_json::json!("myvalue")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Hsetnx, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "HSETNX");
        assert_eq!(cmd_args(&cmd), vec!["mykey", "myfield", "myvalue"]);
    }

    #[test]
    fn test_build_redis_cmd_hsetnx_missing_key() {
        let ex = ex_with(&[
            ("CamelRedis.Field", serde_json::json!("f")),
            (HEADER_VALUE, serde_json::json!("v")),
        ]);
        assert!(build_redis_cmd(&RedisCommand::Hsetnx, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_hmset() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            (
                "CamelRedis.Values",
                serde_json::json!({"f1": "v1", "f2": "v2"}),
            ),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Hmset, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "HMSET");
        let args = cmd_args(&cmd);
        assert_eq!(args[0], "mykey");
        assert!(args.contains(&"f1".to_string()));
        assert!(args.contains(&"v1".to_string()));
        assert!(args.contains(&"f2".to_string()));
        assert!(args.contains(&"v2".to_string()));
        assert_eq!(args.len(), 5);
    }

    #[test]
    fn test_build_redis_cmd_hmset_missing_key() {
        let ex = ex_with(&[("CamelRedis.Values", serde_json::json!({"f1": "v1"}))]);
        assert!(build_redis_cmd(&RedisCommand::Hmset, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_hmset_missing_values() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("k"))]);
        assert!(build_redis_cmd(&RedisCommand::Hmset, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_hmget() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Fields", serde_json::json!(["f1", "f2", "f3"])),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Hmget, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "HMGET");
        assert_eq!(cmd_args(&cmd), vec!["mykey", "f1", "f2", "f3"]);
    }

    #[test]
    fn test_build_redis_cmd_hmget_missing_key() {
        let ex = ex_with(&[("CamelRedis.Fields", serde_json::json!(["f1"]))]);
        assert!(build_redis_cmd(&RedisCommand::Hmget, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_hmget_missing_fields() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("k"))]);
        assert!(build_redis_cmd(&RedisCommand::Hmget, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_hdel() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Field", serde_json::json!("myfield")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Hdel, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "HDEL");
        assert_eq!(cmd_args(&cmd), vec!["mykey", "myfield"]);
    }

    #[test]
    fn test_build_redis_cmd_hdel_missing_key() {
        let ex = ex_with(&[("CamelRedis.Field", serde_json::json!("f"))]);
        assert!(build_redis_cmd(&RedisCommand::Hdel, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_hexists() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Field", serde_json::json!("myfield")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Hexists, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "HEXISTS");
        assert_eq!(cmd_args(&cmd), vec!["mykey", "myfield"]);
    }

    #[test]
    fn test_build_redis_cmd_hlen() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
        let cmd = build_redis_cmd(&RedisCommand::Hlen, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "HLEN");
        assert_eq!(cmd_args(&cmd), vec!["mykey"]);
    }

    #[test]
    fn test_build_redis_cmd_hlen_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Hlen, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_hkeys() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
        let cmd = build_redis_cmd(&RedisCommand::Hkeys, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "HKEYS");
        assert_eq!(cmd_args(&cmd), vec!["mykey"]);
    }

    #[test]
    fn test_build_redis_cmd_hvals() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
        let cmd = build_redis_cmd(&RedisCommand::Hvals, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "HVALS");
        assert_eq!(cmd_args(&cmd), vec!["mykey"]);
    }

    #[test]
    fn test_build_redis_cmd_hgetall() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("mykey"))]);
        let cmd = build_redis_cmd(&RedisCommand::Hgetall, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "HGETALL");
        assert_eq!(cmd_args(&cmd), vec!["mykey"]);
    }

    #[test]
    fn test_build_redis_cmd_hincrby() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Field", serde_json::json!("counter")),
            ("CamelRedis.Increment", serde_json::json!(5i64)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Hincrby, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "HINCRBY");
        assert_eq!(cmd_args(&cmd), vec!["mykey", "counter", "5"]);
    }

    #[test]
    fn test_build_redis_cmd_hincrby_default_increment() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Field", serde_json::json!("counter")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Hincrby, &ex).unwrap();
        assert_eq!(cmd_args(&cmd), vec!["mykey", "counter", "1"]);
    }

    #[test]
    fn test_build_redis_cmd_hincrby_garbage_increment_fails_before_construction() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("mykey")),
            ("CamelRedis.Field", serde_json::json!("counter")),
            ("CamelRedis.Increment", serde_json::json!("abc")),
        ]);
        let err = build_redis_cmd(&RedisCommand::Hincrby, &ex)
            .expect_err("garbage increment must fail before HINCRBY construction");
        let msg = err.to_string();
        assert!(msg.contains("CamelRedis.Increment"), "{msg}");
        assert!(msg.contains("abc"), "{msg}");
    }

    #[test]
    fn test_build_redis_cmd_rejects_non_hash() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("k"))]);
        assert!(build_redis_cmd(&RedisCommand::Set, &ex).is_err());
    }
}

#[cfg(test)]
mod resolve_hmset_values_tests {
    use super::*;
    use camel_component_api::{Exchange, Message};

    fn ex_with_body(body: Body, headers: &[(&str, serde_json::Value)]) -> Exchange {
        let mut msg = Message::new(body);
        for (k, v) in headers {
            msg.set_header(*k, v.clone());
        }
        Exchange::new(msg)
    }

    fn processor_message(err: CamelError) -> String {
        match err {
            CamelError::ProcessorError(msg) => msg,
            other => panic!("expected a processor error, got {other:?}"),
        }
    }

    #[test]
    fn hmset_empty_body_names_command_and_variant() {
        let ex = ex_with_body(Body::Empty, &[]);
        let msg = processor_message(
            resolve_hmset_values(&ex).expect_err("an empty body must be rejected"),
        );
        assert!(msg.contains("HMSET"), "{msg}");
        assert!(msg.contains("Body::Empty"), "{msg}");
    }

    #[test]
    fn hmset_non_object_text_body_is_rejected() {
        let ex = ex_with_body(Body::Text("[1,2,3]".to_string()), &[]);
        let msg = processor_message(
            resolve_hmset_values(&ex).expect_err("a JSON array body must be rejected"),
        );
        assert!(msg.contains("HMSET"), "{msg}");
    }

    #[test]
    fn hmset_utf8_bytes_object_body_parses() {
        let ex = ex_with_body(Body::Bytes(br#"{"f":"v"}"#.to_vec().into()), &[]);
        let values = resolve_hmset_values(&ex).expect("a UTF-8 JSON object body must parse");
        assert_eq!(values, vec![("f".to_string(), b"v".to_vec())]);
    }

    #[test]
    fn hmset_header_wins_over_body() {
        let ex = ex_with_body(
            Body::Json(serde_json::json!({"b": "body"})),
            &[("CamelRedis.Values", serde_json::json!({"h": "header"}))],
        );
        let values = resolve_hmset_values(&ex).expect("the header map must resolve");
        assert_eq!(values, vec![("h".to_string(), b"header".to_vec())]);
    }
}
