use super::{
    body_value_bytes, encode_map_entries, get_i64_header, get_str_header, get_str_vec_header,
    map_body_map, require_key,
};
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

/// Resolves the `HMSET` field/value pairs as raw bytes from the body alone.
///
/// `CamelRedis.Values` was removed in 0.69; a route that still sets it fails
/// with the named tombstone error before the body is inspected, so a stale
/// header can never silently write a different payload. The map comes from
/// the exchange body via [`map_body_map`], and each entry is encoded by the
/// scalar rule of [`encode_map_entries`].
pub(crate) fn resolve_hmset_values(
    exchange: &Exchange,
) -> Result<Vec<(String, Vec<u8>)>, CamelError> {
    if exchange.input.header("CamelRedis.Values").is_some() {
        return Err(CamelError::ProcessorError(
            "CamelRedis.Values was removed in 0.69; put the value in the body".into(),
        ));
    }

    let map = map_body_map(&exchange.input.body, "HMSET")?;
    encode_map_entries(&map, "HMSET")
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
            let value = body_value_bytes(exchange, "HSET", &[crate::HEADER_VALUE])?;
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
            let value = body_value_bytes(exchange, "HSETNX", &[crate::HEADER_VALUE])?;
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
    fn hmset_values_header_tombstones_before_body_encoding() {
        let ex = ex_with_body(
            Body::Empty,
            &[("CamelRedis.Values", serde_json::json!({"h": "header"}))],
        );
        let msg = processor_message(
            resolve_hmset_values(&ex).expect_err("the removed Values header must be rejected"),
        );
        assert!(
            msg.contains("CamelRedis.Values was removed in 0.69"),
            "error must name the tombstoned header: {msg}"
        );
        assert!(
            !msg.contains("Body::Empty"),
            "the tombstone must fire before the body is encoded: {msg}"
        );
    }
}
