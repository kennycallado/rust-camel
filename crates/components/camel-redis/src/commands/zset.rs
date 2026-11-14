use super::{
    body_value_bytes, get_bool_header, get_f64_header, get_i64_header, get_str_header,
    get_str_vec_header, require_key,
};
use crate::HEADER_VALUE;
use crate::config::RedisCommand;
use camel_component_api::{Body, CamelError, Exchange};
use redis::AsyncCommands;
use redis::aio::MultiplexedConnection;

pub(crate) fn is_zset_command(cmd: &RedisCommand) -> bool {
    matches!(
        cmd,
        RedisCommand::Zadd
            | RedisCommand::Zrem
            | RedisCommand::Zrange
            | RedisCommand::Zrevrange
            | RedisCommand::Zrank
            | RedisCommand::Zrevrank
            | RedisCommand::Zscore
            | RedisCommand::Zcard
            | RedisCommand::Zincrby
            | RedisCommand::Zcount
            | RedisCommand::Zrangebyscore
            | RedisCommand::Zrevrangebyscore
            | RedisCommand::Zremrangebyrank
            | RedisCommand::Zremrangebyscore
            | RedisCommand::Zunionstore
            | RedisCommand::Zinterstore
    )
}

pub(crate) fn resolve_destination(exchange: &Exchange) -> Result<String, CamelError> {
    get_str_header(exchange, "CamelRedis.Destination")
        .map(|s| s.to_string())
        .ok_or_else(|| CamelError::ProcessorError("Missing CamelRedis.Destination".into()))
}

pub(crate) fn resolve_zstore_keys(exchange: &Exchange) -> Result<Vec<String>, CamelError> {
    if let Some(keys) = get_str_vec_header(exchange, "CamelRedis.Keys") {
        Ok(keys)
    } else if let Ok(key) = require_key(exchange) {
        Ok(vec![key])
    } else {
        Err(CamelError::ProcessorError(
            "ZUNIONSTORE/ZINTERSTORE requires either CamelRedis.Keys (array) or CamelRedis.Key header".into(),
        ))
    }
}

pub(crate) fn resolve_range_bounds(exchange: &Exchange) -> Result<(isize, isize), CamelError> {
    Ok((
        get_i64_header(exchange, "CamelRedis.Start")?.unwrap_or(0) as isize,
        get_i64_header(exchange, "CamelRedis.End")?.unwrap_or(-1) as isize,
    ))
}

pub(crate) fn resolve_score_bounds(exchange: &Exchange) -> Result<(f64, f64), CamelError> {
    Ok((
        get_f64_header(exchange, "CamelRedis.Min")?.unwrap_or(f64::NEG_INFINITY),
        get_f64_header(exchange, "CamelRedis.Max")?.unwrap_or(f64::INFINITY),
    ))
}

pub(crate) fn resolve_with_scores(exchange: &Exchange) -> bool {
    get_bool_header(exchange, "CamelRedis.WithScore").unwrap_or(false)
}

pub(crate) fn resolve_zadd_score(exchange: &Exchange) -> Result<f64, CamelError> {
    Ok(get_f64_header(exchange, "CamelRedis.Score")?.unwrap_or(0.0))
}

pub(crate) fn resolve_zincr_increment(exchange: &Exchange) -> Result<f64, CamelError> {
    Ok(get_f64_header(exchange, "CamelRedis.Increment")?.unwrap_or(1.0))
}

pub(crate) fn resolve_zremrange_rank_bounds(
    exchange: &Exchange,
) -> Result<(isize, isize), CamelError> {
    resolve_range_bounds(exchange)
}

pub(crate) fn resolve_revrange_score_bounds(exchange: &Exchange) -> Result<(f64, f64), CamelError> {
    let (min, max) = resolve_score_bounds(exchange)?;
    Ok((max, min))
}

pub(crate) fn resolve_zstore_operands(
    exchange: &Exchange,
) -> Result<(String, Vec<String>), CamelError> {
    Ok((
        resolve_destination(exchange)?,
        resolve_zstore_keys(exchange)?,
    ))
}

pub(crate) fn json_from_optional_rank(value: Option<i64>) -> serde_json::Value {
    value.map_or(serde_json::Value::Null, |v| serde_json::json!(v))
}

pub(crate) fn json_from_optional_score(value: Option<f64>) -> serde_json::Value {
    value.map_or(serde_json::Value::Null, |v| serde_json::json!(v))
}

pub(crate) fn json_from_scored_members(values: Vec<(String, f64)>) -> serde_json::Value {
    serde_json::json!(
        values
            .into_iter()
            .map(|(member, score)| serde_json::json!({"member": member, "score": score}))
            .collect::<Vec<_>>()
    )
}

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    if !is_zset_command(cmd) {
        return Err(CamelError::ProcessorError(
            "Not a sorted set command".into(),
        ));
    }

    let result: serde_json::Value = match cmd {
        RedisCommand::Zadd => {
            let key = require_key(exchange)?;
            let score = resolve_zadd_score(exchange)?;
            let member = body_value_bytes(exchange, "ZADD", &[HEADER_VALUE])?;
            let n: i64 = conn
                .zadd(&key, &member, score)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZADD", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Zrem => {
            let key = require_key(exchange)?;
            let member = body_value_bytes(exchange, "ZREM", &[HEADER_VALUE])?;
            let n: i64 = conn
                .zrem(&key, &member)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZREM", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Zrange => {
            let key = require_key(exchange)?;
            let (start, end) = resolve_range_bounds(exchange)?;
            let with_scores = resolve_with_scores(exchange);
            if with_scores {
                let vals: Vec<(String, f64)> = conn
                    .zrange_withscores(&key, start, end)
                    .await
                    .map_err(|e| crate::transport_error::redis_error_to_camel("ZRANGE", e))?;
                json_from_scored_members(vals)
            } else {
                let vals: Vec<String> = conn
                    .zrange(&key, start, end)
                    .await
                    .map_err(|e| crate::transport_error::redis_error_to_camel("ZRANGE", e))?;
                serde_json::json!(vals)
            }
        }
        RedisCommand::Zrevrange => {
            let key = require_key(exchange)?;
            let (start, end) = resolve_range_bounds(exchange)?;
            let with_scores = resolve_with_scores(exchange);
            if with_scores {
                let vals: Vec<(String, f64)> = conn
                    .zrevrange_withscores(&key, start, end)
                    .await
                    .map_err(|e| crate::transport_error::redis_error_to_camel("ZREVRANGE", e))?;
                json_from_scored_members(vals)
            } else {
                let vals: Vec<String> = conn
                    .zrevrange(&key, start, end)
                    .await
                    .map_err(|e| crate::transport_error::redis_error_to_camel("ZREVRANGE", e))?;
                serde_json::json!(vals)
            }
        }
        RedisCommand::Zrank => {
            let key = require_key(exchange)?;
            let member = body_value_bytes(exchange, "ZRANK", &[HEADER_VALUE])?;
            let rank: Option<i64> = conn
                .zrank(&key, &member)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZRANK", e))?;
            json_from_optional_rank(rank)
        }
        RedisCommand::Zrevrank => {
            let key = require_key(exchange)?;
            let member = body_value_bytes(exchange, "ZREVRANK", &[HEADER_VALUE])?;
            let rank: Option<i64> = conn
                .zrevrank(&key, &member)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZREVRANK", e))?;
            json_from_optional_rank(rank)
        }
        RedisCommand::Zscore => {
            let key = require_key(exchange)?;
            let member = body_value_bytes(exchange, "ZSCORE", &[HEADER_VALUE])?;
            let score: Option<f64> = conn
                .zscore(&key, &member)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZSCORE", e))?;
            json_from_optional_score(score)
        }
        RedisCommand::Zcard => {
            let key = require_key(exchange)?;
            let n: i64 = conn
                .zcard(&key)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZCARD", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Zincrby => {
            let key = require_key(exchange)?;
            let increment = resolve_zincr_increment(exchange)?;
            let member = body_value_bytes(exchange, "ZINCRBY", &[HEADER_VALUE])?;
            let new_score: f64 = conn
                .zincr(&key, &member, increment)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZINCRBY", e))?;
            serde_json::json!(new_score)
        }
        RedisCommand::Zcount => {
            let key = require_key(exchange)?;
            let (min, max) = resolve_score_bounds(exchange)?;
            let n: i64 = conn
                .zcount(&key, min, max)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZCOUNT", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Zrangebyscore => {
            let key = require_key(exchange)?;
            let (min, max) = resolve_score_bounds(exchange)?;
            let with_scores = resolve_with_scores(exchange);
            if with_scores {
                let vals: Vec<(String, f64)> = conn
                    .zrangebyscore_withscores(&key, min, max)
                    .await
                    .map_err(|e| {
                        crate::transport_error::redis_error_to_camel("ZRANGEBYSCORE", e)
                    })?;
                json_from_scored_members(vals)
            } else {
                let vals: Vec<String> = conn.zrangebyscore(&key, min, max).await.map_err(|e| {
                    crate::transport_error::redis_error_to_camel("ZRANGEBYSCORE", e)
                })?;
                serde_json::json!(vals)
            }
        }
        RedisCommand::Zrevrangebyscore => {
            let key = require_key(exchange)?;
            let (max, min) = resolve_revrange_score_bounds(exchange)?;
            let with_scores = resolve_with_scores(exchange);
            if with_scores {
                let vals: Vec<(String, f64)> = conn
                    .zrevrangebyscore_withscores(&key, max, min)
                    .await
                    .map_err(|e| {
                        crate::transport_error::redis_error_to_camel("ZREVRANGEBYSCORE", e)
                    })?;
                json_from_scored_members(vals)
            } else {
                let vals: Vec<String> =
                    conn.zrevrangebyscore(&key, max, min).await.map_err(|e| {
                        crate::transport_error::redis_error_to_camel("ZREVRANGEBYSCORE", e)
                    })?;
                serde_json::json!(vals)
            }
        }
        RedisCommand::Zremrangebyrank => {
            let key = require_key(exchange)?;
            let (start, end) = resolve_zremrange_rank_bounds(exchange)?;
            let n: usize = conn
                .zremrangebyrank(&key, start, end)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZREMRANGEBYRANK", e))?;
            serde_json::json!(n as i64)
        }
        RedisCommand::Zremrangebyscore => {
            let key = require_key(exchange)?;
            let (min, max) = resolve_score_bounds(exchange)?;
            // Use raw command since method doesn't exist in redis-rs
            let n: i64 = redis::cmd("ZREMRANGEBYSCORE")
                .arg(&key)
                .arg(min)
                .arg(max)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZREMRANGEBYSCORE", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Zunionstore => {
            let (dest, keys) = resolve_zstore_operands(exchange)?;
            let n: i64 = conn
                .zunionstore(dest, &keys)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZUNIONSTORE", e))?;
            serde_json::json!(n)
        }
        RedisCommand::Zinterstore => {
            let (dest, keys) = resolve_zstore_operands(exchange)?;
            let n: i64 = conn
                .zinterstore(dest, &keys)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ZINTERSTORE", e))?;
            serde_json::json!(n)
        }
        _ => unreachable!("non-zset commands rejected above"),
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
    fn test_zadd_requires_key() {
        let ex = Exchange::new(Message::default());
        assert!(crate::commands::require_key(&ex).is_err());
    }

    #[test]
    fn test_zadd_has_key_score_and_body_member() {
        let mut msg = Message::new(Body::Text("member1".into()));
        msg.set_header("CamelRedis.Key", serde_json::json!("myzset"));
        msg.set_header("CamelRedis.Score", serde_json::json!(10.5));
        let ex = Exchange::new(msg);
        assert_eq!(crate::commands::require_key(&ex).unwrap(), "myzset");
        assert_eq!(
            crate::commands::get_f64_header(&ex, "CamelRedis.Score").unwrap(),
            Some(10.5)
        );
        assert_eq!(
            body_value_bytes(&ex, "ZADD", &[HEADER_VALUE]).unwrap(),
            b"member1".to_vec()
        );
    }

    #[test]
    fn test_zrange_with_score_header() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.Start", serde_json::json!(0)),
            ("CamelRedis.End", serde_json::json!(-1)),
            ("CamelRedis.WithScore", serde_json::json!(true)),
        ]);
        assert_eq!(
            crate::commands::get_bool_header(&ex, "CamelRedis.WithScore"),
            Some(true)
        );
        assert_eq!(
            crate::commands::get_i64_header(&ex, "CamelRedis.Start").unwrap(),
            Some(0)
        );
        assert_eq!(
            crate::commands::get_i64_header(&ex, "CamelRedis.End").unwrap(),
            Some(-1)
        );
    }

    #[test]
    fn test_zcount_with_min_max() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.Min", serde_json::json!(0.0)),
            ("CamelRedis.Max", serde_json::json!(100.0)),
        ]);
        assert_eq!(
            crate::commands::get_f64_header(&ex, "CamelRedis.Min").unwrap(),
            Some(0.0)
        );
        assert_eq!(
            crate::commands::get_f64_header(&ex, "CamelRedis.Max").unwrap(),
            Some(100.0)
        );
    }

    #[test]
    fn test_zunionstore_with_destination_and_keys() {
        let ex = ex_with(&[
            ("CamelRedis.Destination", serde_json::json!("dest")),
            ("CamelRedis.Keys", serde_json::json!(["zset1", "zset2"])),
        ]);
        assert_eq!(resolve_destination(&ex).unwrap(), "dest");
        assert_eq!(resolve_zstore_keys(&ex).unwrap(), vec!["zset1", "zset2"]);
    }

    #[test]
    fn test_zset_command_classification() {
        assert!(is_zset_command(&RedisCommand::Zadd));
        assert!(is_zset_command(&RedisCommand::Zinterstore));
        assert!(!is_zset_command(&RedisCommand::Set));
    }

    #[test]
    fn test_resolve_destination_requires_header() {
        let ex = Exchange::new(Message::default());
        let err = resolve_destination(&ex).expect_err("destination should be required");
        assert!(err.to_string().contains("CamelRedis.Destination"));
    }

    // REDIS-005: resolve_zstore_keys returns error when neither keys nor key header exists
    #[test]
    fn test_resolve_zstore_keys_falls_back_to_single_key_or_error() {
        // With Keys header
        let ex = ex_with(&[
            ("CamelRedis.Destination", serde_json::json!("dest")),
            ("CamelRedis.Keys", serde_json::json!(["zset1", "zset2"])),
        ]);
        assert_eq!(resolve_zstore_keys(&ex).unwrap(), vec!["zset1", "zset2"]);

        // With Key header (fallback)
        let ex_with_key = ex_with(&[
            ("CamelRedis.Destination", serde_json::json!("dest")),
            ("CamelRedis.Key", serde_json::json!("k1")),
        ]);
        assert_eq!(resolve_zstore_keys(&ex_with_key).unwrap(), vec!["k1"]);

        // REDIS-005: Missing both keys and key returns error
        let ex_missing = ex_with(&[("CamelRedis.Destination", serde_json::json!("dest"))]);
        let err = resolve_zstore_keys(&ex_missing).expect_err("missing keys should return error");
        assert!(err.to_string().contains("CamelRedis.Keys"));
        assert!(err.to_string().contains("CamelRedis.Key"));
    }

    #[test]
    fn test_resolve_range_bounds_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_range_bounds(&ex_default).unwrap(), (0, -1));

        let ex = ex_with(&[
            ("CamelRedis.Start", serde_json::json!(2)),
            ("CamelRedis.End", serde_json::json!(8)),
        ]);
        assert_eq!(resolve_range_bounds(&ex).unwrap(), (2, 8));

        let ex_str = ex_with(&[
            ("CamelRedis.Start", serde_json::json!(" 2 ")),
            ("CamelRedis.End", serde_json::json!(" 8 ")),
        ]);
        assert_eq!(resolve_range_bounds(&ex_str).unwrap(), (2, 8));
    }

    #[test]
    fn test_resolve_score_bounds_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        let (min, max) = resolve_score_bounds(&ex_default).unwrap();
        assert_eq!(min, f64::NEG_INFINITY);
        assert_eq!(max, f64::INFINITY);

        let ex = ex_with(&[
            ("CamelRedis.Min", serde_json::json!(1.5)),
            ("CamelRedis.Max", serde_json::json!(9.5)),
        ]);
        assert_eq!(resolve_score_bounds(&ex).unwrap(), (1.5, 9.5));

        let ex_str = ex_with(&[
            ("CamelRedis.Min", serde_json::json!(" 1.5 ")),
            ("CamelRedis.Max", serde_json::json!(" 9.5 ")),
        ]);
        assert_eq!(resolve_score_bounds(&ex_str).unwrap(), (1.5, 9.5));
    }

    #[test]
    fn test_resolve_score_bounds_nonfinite_is_named_error() {
        let ex = ex_with(&[("CamelRedis.Min", serde_json::json!("NaN"))]);
        let err = resolve_score_bounds(&ex).expect_err("NaN score bound must fail");
        let text = err.to_string();
        assert!(text.contains("CamelRedis.Min"), "{text}");
        assert!(text.contains("NaN"), "{text}");
    }

    #[test]
    fn test_resolve_with_scores_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        assert!(!resolve_with_scores(&ex_default));

        let ex = ex_with(&[("CamelRedis.WithScore", serde_json::json!(true))]);
        assert!(resolve_with_scores(&ex));
    }

    #[test]
    fn test_resolve_zadd_score_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_zadd_score(&ex_default).unwrap(), 0.0);

        let ex = ex_with(&[("CamelRedis.Score", serde_json::json!(3.25))]);
        assert_eq!(resolve_zadd_score(&ex).unwrap(), 3.25);

        let ex_str = ex_with(&[("CamelRedis.Score", serde_json::json!(" 3.25 "))]);
        assert_eq!(resolve_zadd_score(&ex_str).unwrap(), 3.25);
    }

    #[test]
    fn test_resolve_zincr_increment_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_zincr_increment(&ex_default).unwrap(), 1.0);

        let ex = ex_with(&[("CamelRedis.Increment", serde_json::json!(2.5))]);
        assert_eq!(resolve_zincr_increment(&ex).unwrap(), 2.5);
    }

    #[test]
    fn test_resolve_zremrange_rank_bounds_uses_range_defaults() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_zremrange_rank_bounds(&ex_default).unwrap(), (0, -1));

        let ex = ex_with(&[
            ("CamelRedis.Start", serde_json::json!(4)),
            ("CamelRedis.End", serde_json::json!(9)),
        ]);
        assert_eq!(resolve_zremrange_rank_bounds(&ex).unwrap(), (4, 9));
    }

    #[test]
    fn test_resolve_revrange_score_bounds_swaps_order() {
        let ex = ex_with(&[
            ("CamelRedis.Min", serde_json::json!(1.0)),
            ("CamelRedis.Max", serde_json::json!(8.0)),
        ]);
        assert_eq!(resolve_revrange_score_bounds(&ex).unwrap(), (8.0, 1.0));
    }

    #[test]
    fn test_resolve_zstore_operands_requires_destination() {
        let ex = ex_with(&[("CamelRedis.Keys", serde_json::json!(["k1", "k2"]))]);
        let err = resolve_zstore_operands(&ex).expect_err("destination should be required");
        assert!(err.to_string().contains("CamelRedis.Destination"));
    }

    #[test]
    fn test_json_from_optional_rank_and_score() {
        assert_eq!(json_from_optional_rank(Some(2)), serde_json::json!(2));
        assert_eq!(json_from_optional_rank(None), serde_json::Value::Null);
        assert_eq!(json_from_optional_score(Some(1.5)), serde_json::json!(1.5));
        assert_eq!(json_from_optional_score(None), serde_json::Value::Null);
    }

    #[test]
    fn test_json_from_scored_members_shape() {
        let json = json_from_scored_members(vec![("m1".to_string(), 1.0)]);
        assert_eq!(json, serde_json::json!([{"member": "m1", "score": 1.0}]));
    }
}
