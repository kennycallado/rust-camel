use super::{
    get_bool_header, get_f64_header, get_i64_header, get_str_header, get_str_vec_header,
    require_key, require_value,
};
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

pub(crate) fn resolve_zstore_keys(exchange: &Exchange) -> Vec<String> {
    get_str_vec_header(exchange, "CamelRedis.Keys")
        .unwrap_or_else(|| vec![require_key(exchange).unwrap_or_default()])
}

pub(crate) fn resolve_range_bounds(exchange: &Exchange) -> (isize, isize) {
    (
        get_i64_header(exchange, "CamelRedis.Start").unwrap_or(0) as isize,
        get_i64_header(exchange, "CamelRedis.End").unwrap_or(-1) as isize,
    )
}

pub(crate) fn resolve_score_bounds(exchange: &Exchange) -> (f64, f64) {
    (
        get_f64_header(exchange, "CamelRedis.Min").unwrap_or(f64::NEG_INFINITY),
        get_f64_header(exchange, "CamelRedis.Max").unwrap_or(f64::INFINITY),
    )
}

pub(crate) fn resolve_with_scores(exchange: &Exchange) -> bool {
    get_bool_header(exchange, "CamelRedis.WithScore").unwrap_or(false)
}

pub(crate) fn resolve_zadd_score(exchange: &Exchange) -> f64 {
    get_f64_header(exchange, "CamelRedis.Score").unwrap_or(0.0)
}

pub(crate) fn resolve_zincr_increment(exchange: &Exchange) -> f64 {
    get_f64_header(exchange, "CamelRedis.Increment").unwrap_or(1.0)
}

pub(crate) fn resolve_zremrange_rank_bounds(exchange: &Exchange) -> (isize, isize) {
    resolve_range_bounds(exchange)
}

pub(crate) fn resolve_revrange_score_bounds(exchange: &Exchange) -> (f64, f64) {
    let (min, max) = resolve_score_bounds(exchange);
    (max, min)
}

pub(crate) fn resolve_zstore_operands(
    exchange: &Exchange,
) -> Result<(String, Vec<String>), CamelError> {
    Ok((
        resolve_destination(exchange)?,
        resolve_zstore_keys(exchange),
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

#[allow(dead_code)]
pub(crate) fn build_redis_cmd(cmd: &RedisCommand, exchange: &Exchange) -> Result<redis::Cmd, CamelError> {
    if !is_zset_command(cmd) {
        return Err(CamelError::ProcessorError("Not a sorted set command".into()));
    }

    match cmd {
        RedisCommand::Zadd => {
            let key = require_key(exchange)?;
            let score = resolve_zadd_score(exchange);
            let member = require_value(exchange)?;
            let mut c = redis::Cmd::new();
            c.arg("ZADD").arg(key).arg(score).arg(member.to_string());
            Ok(c)
        }
        RedisCommand::Zrem => {
            let key = require_key(exchange)?;
            let member = require_value(exchange)?;
            let mut c = redis::Cmd::new();
            c.arg("ZREM").arg(key).arg(member.to_string());
            Ok(c)
        }
        RedisCommand::Zrange => {
            let key = require_key(exchange)?;
            let (start, end) = resolve_range_bounds(exchange);
            let with_scores = resolve_with_scores(exchange);
            let mut c = redis::Cmd::new();
            c.arg("ZRANGE").arg(key).arg(start).arg(end);
            if with_scores {
                c.arg("WITHSCORES");
            }
            Ok(c)
        }
        RedisCommand::Zrevrange => {
            let key = require_key(exchange)?;
            let (start, end) = resolve_range_bounds(exchange);
            let with_scores = resolve_with_scores(exchange);
            let mut c = redis::Cmd::new();
            c.arg("ZREVRANGE").arg(key).arg(start).arg(end);
            if with_scores {
                c.arg("WITHSCORES");
            }
            Ok(c)
        }
        RedisCommand::Zrank => {
            let key = require_key(exchange)?;
            let member = require_value(exchange)?;
            let mut c = redis::Cmd::new();
            c.arg("ZRANK").arg(key).arg(member.to_string());
            Ok(c)
        }
        RedisCommand::Zrevrank => {
            let key = require_key(exchange)?;
            let member = require_value(exchange)?;
            let mut c = redis::Cmd::new();
            c.arg("ZREVRANK").arg(key).arg(member.to_string());
            Ok(c)
        }
        RedisCommand::Zscore => {
            let key = require_key(exchange)?;
            let member = require_value(exchange)?;
            let mut c = redis::Cmd::new();
            c.arg("ZSCORE").arg(key).arg(member.to_string());
            Ok(c)
        }
        RedisCommand::Zcard => {
            let key = require_key(exchange)?;
            let mut c = redis::Cmd::new();
            c.arg("ZCARD").arg(key);
            Ok(c)
        }
        RedisCommand::Zincrby => {
            let key = require_key(exchange)?;
            let increment = resolve_zincr_increment(exchange);
            let member = require_value(exchange)?;
            let mut c = redis::Cmd::new();
            c.arg("ZINCRBY").arg(key).arg(increment).arg(member.to_string());
            Ok(c)
        }
        RedisCommand::Zcount => {
            let key = require_key(exchange)?;
            let (min, max) = resolve_score_bounds(exchange);
            let mut c = redis::Cmd::new();
            c.arg("ZCOUNT").arg(key).arg(min).arg(max);
            Ok(c)
        }
        RedisCommand::Zrangebyscore => {
            let key = require_key(exchange)?;
            let (min, max) = resolve_score_bounds(exchange);
            let with_scores = resolve_with_scores(exchange);
            let mut c = redis::Cmd::new();
            c.arg("ZRANGEBYSCORE").arg(key).arg(min).arg(max);
            if with_scores {
                c.arg("WITHSCORES");
            }
            Ok(c)
        }
        RedisCommand::Zrevrangebyscore => {
            let key = require_key(exchange)?;
            let (max, min) = resolve_revrange_score_bounds(exchange);
            let with_scores = resolve_with_scores(exchange);
            let mut c = redis::Cmd::new();
            c.arg("ZREVRANGEBYSCORE").arg(key).arg(max).arg(min);
            if with_scores {
                c.arg("WITHSCORES");
            }
            Ok(c)
        }
        RedisCommand::Zremrangebyrank => {
            let key = require_key(exchange)?;
            let (start, end) = resolve_zremrange_rank_bounds(exchange);
            let mut c = redis::Cmd::new();
            c.arg("ZREMRANGEBYRANK").arg(key).arg(start).arg(end);
            Ok(c)
        }
        RedisCommand::Zremrangebyscore => {
            let key = require_key(exchange)?;
            let (min, max) = resolve_score_bounds(exchange);
            let mut c = redis::Cmd::new();
            c.arg("ZREMRANGEBYSCORE").arg(key).arg(min).arg(max);
            Ok(c)
        }
        RedisCommand::Zunionstore => {
            let (dest, keys) = resolve_zstore_operands(exchange)?;
            let mut c = redis::Cmd::new();
            c.arg("ZUNIONSTORE").arg(dest).arg(keys.len() as i64).arg(&keys);
            Ok(c)
        }
        RedisCommand::Zinterstore => {
            let (dest, keys) = resolve_zstore_operands(exchange)?;
            let mut c = redis::Cmd::new();
            c.arg("ZINTERSTORE").arg(dest).arg(keys.len() as i64).arg(&keys);
            Ok(c)
        }
        _ => unreachable!("non-zset commands rejected above"),
    }
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
            let score = resolve_zadd_score(exchange);
            let member = require_value(exchange)?;
            let n: i64 = conn
                .zadd(&key, member.to_string(), score)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis ZADD failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Zrem => {
            let key = require_key(exchange)?;
            let member = require_value(exchange)?;
            let n: i64 = conn
                .zrem(&key, member.to_string())
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis ZREM failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Zrange => {
            let key = require_key(exchange)?;
            let (start, end) = resolve_range_bounds(exchange);
            let with_scores = resolve_with_scores(exchange);
            if with_scores {
                let vals: Vec<(String, f64)> = conn
                    .zrange_withscores(&key, start, end)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis ZRANGE failed: {e}")))?;
                json_from_scored_members(vals)
            } else {
                let vals: Vec<String> = conn
                    .zrange(&key, start, end)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis ZRANGE failed: {e}")))?;
                serde_json::json!(vals)
            }
        }
        RedisCommand::Zrevrange => {
            let key = require_key(exchange)?;
            let (start, end) = resolve_range_bounds(exchange);
            let with_scores = resolve_with_scores(exchange);
            if with_scores {
                let vals: Vec<(String, f64)> = conn
                    .zrevrange_withscores(&key, start, end)
                    .await
                    .map_err(|e| {
                        CamelError::ProcessorError(format!("Redis ZREVRANGE failed: {e}"))
                    })?;
                json_from_scored_members(vals)
            } else {
                let vals: Vec<String> = conn.zrevrange(&key, start, end).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis ZREVRANGE failed: {e}"))
                })?;
                serde_json::json!(vals)
            }
        }
        RedisCommand::Zrank => {
            let key = require_key(exchange)?;
            let member = require_value(exchange)?;
            let rank: Option<i64> = conn
                .zrank(&key, member.to_string())
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis ZRANK failed: {e}")))?;
            json_from_optional_rank(rank)
        }
        RedisCommand::Zrevrank => {
            let key = require_key(exchange)?;
            let member = require_value(exchange)?;
            let rank: Option<i64> = conn
                .zrevrank(&key, member.to_string())
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis ZREVRANK failed: {e}")))?;
            json_from_optional_rank(rank)
        }
        RedisCommand::Zscore => {
            let key = require_key(exchange)?;
            let member = require_value(exchange)?;
            let score: Option<f64> = conn
                .zscore(&key, member.to_string())
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis ZSCORE failed: {e}")))?;
            json_from_optional_score(score)
        }
        RedisCommand::Zcard => {
            let key = require_key(exchange)?;
            let n: i64 = conn
                .zcard(&key)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis ZCARD failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Zincrby => {
            let key = require_key(exchange)?;
            let increment = resolve_zincr_increment(exchange);
            let member = require_value(exchange)?;
            let new_score: f64 = conn
                .zincr(&key, member.to_string(), increment)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis ZINCRBY failed: {e}")))?;
            serde_json::json!(new_score)
        }
        RedisCommand::Zcount => {
            let key = require_key(exchange)?;
            let (min, max) = resolve_score_bounds(exchange);
            let n: i64 = conn
                .zcount(&key, min, max)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis ZCOUNT failed: {e}")))?;
            serde_json::json!(n)
        }
        RedisCommand::Zrangebyscore => {
            let key = require_key(exchange)?;
            let (min, max) = resolve_score_bounds(exchange);
            let with_scores = resolve_with_scores(exchange);
            if with_scores {
                let vals: Vec<(String, f64)> = conn
                    .zrangebyscore_withscores(&key, min, max)
                    .await
                    .map_err(|e| {
                        CamelError::ProcessorError(format!("Redis ZRANGEBYSCORE failed: {e}"))
                    })?;
                json_from_scored_members(vals)
            } else {
                let vals: Vec<String> = conn.zrangebyscore(&key, min, max).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis ZRANGEBYSCORE failed: {e}"))
                })?;
                serde_json::json!(vals)
            }
        }
        RedisCommand::Zrevrangebyscore => {
            let key = require_key(exchange)?;
            let (max, min) = resolve_revrange_score_bounds(exchange);
            let with_scores = resolve_with_scores(exchange);
            if with_scores {
                let vals: Vec<(String, f64)> = conn
                    .zrevrangebyscore_withscores(&key, max, min)
                    .await
                    .map_err(|e| {
                        CamelError::ProcessorError(format!("Redis ZREVRANGEBYSCORE failed: {e}"))
                    })?;
                json_from_scored_members(vals)
            } else {
                let vals: Vec<String> =
                    conn.zrevrangebyscore(&key, max, min).await.map_err(|e| {
                        CamelError::ProcessorError(format!("Redis ZREVRANGEBYSCORE failed: {e}"))
                    })?;
                serde_json::json!(vals)
            }
        }
        RedisCommand::Zremrangebyrank => {
            let key = require_key(exchange)?;
            let (start, end) = resolve_zremrange_rank_bounds(exchange);
            let n: usize = conn.zremrangebyrank(&key, start, end).await.map_err(|e| {
                CamelError::ProcessorError(format!("Redis ZREMRANGEBYRANK failed: {e}"))
            })?;
            serde_json::json!(n as i64)
        }
        RedisCommand::Zremrangebyscore => {
            let key = require_key(exchange)?;
            let (min, max) = resolve_score_bounds(exchange);
            // Use raw command since method doesn't exist in redis-rs
            let n: i64 = redis::cmd("ZREMRANGEBYSCORE")
                .arg(&key)
                .arg(min)
                .arg(max)
                .query_async(conn)
                .await
                .map_err(|e| {
                    CamelError::ProcessorError(format!("Redis ZREMRANGEBYSCORE failed: {e}"))
                })?;
            serde_json::json!(n)
        }
        RedisCommand::Zunionstore => {
            let (dest, keys) = resolve_zstore_operands(exchange)?;
            let n: i64 = conn.zunionstore(dest, &keys).await.map_err(|e| {
                CamelError::ProcessorError(format!("Redis ZUNIONSTORE failed: {e}"))
            })?;
            serde_json::json!(n)
        }
        RedisCommand::Zinterstore => {
            let (dest, keys) = resolve_zstore_operands(exchange)?;
            let n: i64 = conn.zinterstore(dest, &keys).await.map_err(|e| {
                CamelError::ProcessorError(format!("Redis ZINTERSTORE failed: {e}"))
            })?;
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
    fn test_zadd_has_key_score_and_member() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.Score", serde_json::json!(10.5)),
            ("CamelRedis.Value", serde_json::json!("member1")),
        ]);
        assert_eq!(crate::commands::require_key(&ex).unwrap(), "myzset");
        assert_eq!(
            crate::commands::get_f64_header(&ex, "CamelRedis.Score"),
            Some(10.5)
        );
        assert_eq!(
            crate::commands::require_value(&ex).unwrap(),
            serde_json::json!("member1")
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
            crate::commands::get_i64_header(&ex, "CamelRedis.Start"),
            Some(0)
        );
        assert_eq!(
            crate::commands::get_i64_header(&ex, "CamelRedis.End"),
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
            crate::commands::get_f64_header(&ex, "CamelRedis.Min"),
            Some(0.0)
        );
        assert_eq!(
            crate::commands::get_f64_header(&ex, "CamelRedis.Max"),
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
        assert_eq!(resolve_zstore_keys(&ex), vec!["zset1", "zset2"]);
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

    #[test]
    fn test_resolve_zstore_keys_falls_back_to_single_key_or_empty() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("k1"))]);
        assert_eq!(resolve_zstore_keys(&ex), vec!["k1"]);

        let ex_missing = Exchange::new(Message::default());
        assert_eq!(resolve_zstore_keys(&ex_missing), vec![""]);
    }

    #[test]
    fn test_resolve_range_bounds_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_range_bounds(&ex_default), (0, -1));

        let ex = ex_with(&[
            ("CamelRedis.Start", serde_json::json!(2)),
            ("CamelRedis.End", serde_json::json!(8)),
        ]);
        assert_eq!(resolve_range_bounds(&ex), (2, 8));
    }

    #[test]
    fn test_resolve_score_bounds_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        let (min, max) = resolve_score_bounds(&ex_default);
        assert_eq!(min, f64::NEG_INFINITY);
        assert_eq!(max, f64::INFINITY);

        let ex = ex_with(&[
            ("CamelRedis.Min", serde_json::json!(1.5)),
            ("CamelRedis.Max", serde_json::json!(9.5)),
        ]);
        assert_eq!(resolve_score_bounds(&ex), (1.5, 9.5));
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
        assert_eq!(resolve_zadd_score(&ex_default), 0.0);

        let ex = ex_with(&[("CamelRedis.Score", serde_json::json!(3.25))]);
        assert_eq!(resolve_zadd_score(&ex), 3.25);
    }

    #[test]
    fn test_resolve_zincr_increment_defaults_and_values() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_zincr_increment(&ex_default), 1.0);

        let ex = ex_with(&[("CamelRedis.Increment", serde_json::json!(2.5))]);
        assert_eq!(resolve_zincr_increment(&ex), 2.5);
    }

    #[test]
    fn test_resolve_zremrange_rank_bounds_uses_range_defaults() {
        let ex_default = Exchange::new(Message::default());
        assert_eq!(resolve_zremrange_rank_bounds(&ex_default), (0, -1));

        let ex = ex_with(&[
            ("CamelRedis.Start", serde_json::json!(4)),
            ("CamelRedis.End", serde_json::json!(9)),
        ]);
        assert_eq!(resolve_zremrange_rank_bounds(&ex), (4, 9));
    }

    #[test]
    fn test_resolve_revrange_score_bounds_swaps_order() {
        let ex = ex_with(&[
            ("CamelRedis.Min", serde_json::json!(1.0)),
            ("CamelRedis.Max", serde_json::json!(8.0)),
        ]);
        assert_eq!(resolve_revrange_score_bounds(&ex), (8.0, 1.0));
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

    fn assert_cmd_name(cmd: &redis::Cmd, expected: &str) {
        let packed = cmd.get_packed_command();
        let s = String::from_utf8_lossy(&packed);
        assert!(s.contains(expected), "expected '{expected}' in packed cmd");
    }

    fn packed_contains(cmd: &redis::Cmd, needle: &str) -> bool {
        let packed = cmd.get_packed_command();
        String::from_utf8_lossy(&packed).contains(needle)
    }

    #[test]
    fn test_build_redis_cmd_zadd() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.Score", serde_json::json!(10.5)),
            ("CamelRedis.Value", serde_json::json!("member1")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zadd, &ex).unwrap();
        assert_cmd_name(&cmd, "ZADD");
        assert!(packed_contains(&cmd, "myzset"));
        assert!(packed_contains(&cmd, "10.5"));
    }

    #[test]
    fn test_build_redis_cmd_zadd_missing_key() {
        let ex = ex_with(&[("CamelRedis.Value", serde_json::json!("m1"))]);
        assert!(build_redis_cmd(&RedisCommand::Zadd, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zadd_missing_member() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("myzset"))]);
        assert!(build_redis_cmd(&RedisCommand::Zadd, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zrem() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.Value", serde_json::json!("member1")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zrem, &ex).unwrap();
        assert_cmd_name(&cmd, "ZREM");
        assert!(packed_contains(&cmd, "myzset"));
    }

    #[test]
    fn test_build_redis_cmd_zrem_missing_key() {
        let ex = ex_with(&[("CamelRedis.Value", serde_json::json!("m1"))]);
        assert!(build_redis_cmd(&RedisCommand::Zrem, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zrem_missing_member() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("myzset"))]);
        assert!(build_redis_cmd(&RedisCommand::Zrem, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zrange() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.Start", serde_json::json!(0)),
            ("CamelRedis.End", serde_json::json!(-1)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zrange, &ex).unwrap();
        assert_cmd_name(&cmd, "ZRANGE");
        assert!(packed_contains(&cmd, "myzset"));
        assert!(!packed_contains(&cmd, "WITHSCORES"));
    }

    #[test]
    fn test_build_redis_cmd_zrange_with_scores() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.WithScore", serde_json::json!(true)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zrange, &ex).unwrap();
        assert_cmd_name(&cmd, "ZRANGE");
        assert!(packed_contains(&cmd, "WITHSCORES"));
    }

    #[test]
    fn test_build_redis_cmd_zrange_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Zrange, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zrevrange() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.Start", serde_json::json!(0)),
            ("CamelRedis.End", serde_json::json!(5)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zrevrange, &ex).unwrap();
        assert_cmd_name(&cmd, "ZREVRANGE");
        assert!(packed_contains(&cmd, "myzset"));
        assert!(!packed_contains(&cmd, "WITHSCORES"));
    }

    #[test]
    fn test_build_redis_cmd_zrevrange_with_scores() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.WithScore", serde_json::json!(true)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zrevrange, &ex).unwrap();
        assert_cmd_name(&cmd, "ZREVRANGE");
        assert!(packed_contains(&cmd, "WITHSCORES"));
    }

    #[test]
    fn test_build_redis_cmd_zrevrange_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Zrevrange, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zrank() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.Value", serde_json::json!("member1")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zrank, &ex).unwrap();
        assert_cmd_name(&cmd, "ZRANK");
        assert!(packed_contains(&cmd, "myzset"));
    }

    #[test]
    fn test_build_redis_cmd_zrank_missing_key() {
        let ex = ex_with(&[("CamelRedis.Value", serde_json::json!("m1"))]);
        assert!(build_redis_cmd(&RedisCommand::Zrank, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zrank_missing_member() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("myzset"))]);
        assert!(build_redis_cmd(&RedisCommand::Zrank, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zrevrank() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.Value", serde_json::json!("member1")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zrevrank, &ex).unwrap();
        assert_cmd_name(&cmd, "ZREVRANK");
        assert!(packed_contains(&cmd, "myzset"));
    }

    #[test]
    fn test_build_redis_cmd_zrevrank_missing_key() {
        let ex = ex_with(&[("CamelRedis.Value", serde_json::json!("m1"))]);
        assert!(build_redis_cmd(&RedisCommand::Zrevrank, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zrevrank_missing_member() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("myzset"))]);
        assert!(build_redis_cmd(&RedisCommand::Zrevrank, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zscore() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.Value", serde_json::json!("member1")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zscore, &ex).unwrap();
        assert_cmd_name(&cmd, "ZSCORE");
        assert!(packed_contains(&cmd, "myzset"));
    }

    #[test]
    fn test_build_redis_cmd_zscore_missing_key() {
        let ex = ex_with(&[("CamelRedis.Value", serde_json::json!("m1"))]);
        assert!(build_redis_cmd(&RedisCommand::Zscore, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zscore_missing_member() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("myzset"))]);
        assert!(build_redis_cmd(&RedisCommand::Zscore, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zcard() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("myzset"))]);
        let cmd = build_redis_cmd(&RedisCommand::Zcard, &ex).unwrap();
        assert_cmd_name(&cmd, "ZCARD");
        assert!(packed_contains(&cmd, "myzset"));
    }

    #[test]
    fn test_build_redis_cmd_zcard_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Zcard, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zincrby() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.Increment", serde_json::json!(2.5)),
            ("CamelRedis.Value", serde_json::json!("member1")),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zincrby, &ex).unwrap();
        assert_cmd_name(&cmd, "ZINCRBY");
        assert!(packed_contains(&cmd, "myzset"));
        assert!(packed_contains(&cmd, "2.5"));
    }

    #[test]
    fn test_build_redis_cmd_zincrby_missing_key() {
        let ex = ex_with(&[("CamelRedis.Value", serde_json::json!("m1"))]);
        assert!(build_redis_cmd(&RedisCommand::Zincrby, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zincrby_missing_member() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("myzset"))]);
        assert!(build_redis_cmd(&RedisCommand::Zincrby, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zcount() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.Min", serde_json::json!(0.0)),
            ("CamelRedis.Max", serde_json::json!(100.0)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zcount, &ex).unwrap();
        assert_cmd_name(&cmd, "ZCOUNT");
        assert!(packed_contains(&cmd, "myzset"));
        assert!(packed_contains(&cmd, "100"));
    }

    #[test]
    fn test_build_redis_cmd_zcount_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Zcount, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zrangebyscore() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.Min", serde_json::json!(0.0)),
            ("CamelRedis.Max", serde_json::json!(50.0)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zrangebyscore, &ex).unwrap();
        assert_cmd_name(&cmd, "ZRANGEBYSCORE");
        assert!(packed_contains(&cmd, "myzset"));
        assert!(!packed_contains(&cmd, "WITHSCORES"));
    }

    #[test]
    fn test_build_redis_cmd_zrangebyscore_with_scores() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.WithScore", serde_json::json!(true)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zrangebyscore, &ex).unwrap();
        assert_cmd_name(&cmd, "ZRANGEBYSCORE");
        assert!(packed_contains(&cmd, "WITHSCORES"));
    }

    #[test]
    fn test_build_redis_cmd_zrangebyscore_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Zrangebyscore, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zrevrangebyscore() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.Min", serde_json::json!(0.0)),
            ("CamelRedis.Max", serde_json::json!(50.0)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zrevrangebyscore, &ex).unwrap();
        assert_cmd_name(&cmd, "ZREVRANGEBYSCORE");
        assert!(packed_contains(&cmd, "myzset"));
    }

    #[test]
    fn test_build_redis_cmd_zrevrangebyscore_with_scores() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.WithScore", serde_json::json!(true)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zrevrangebyscore, &ex).unwrap();
        assert_cmd_name(&cmd, "ZREVRANGEBYSCORE");
        assert!(packed_contains(&cmd, "WITHSCORES"));
    }

    #[test]
    fn test_build_redis_cmd_zrevrangebyscore_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Zrevrangebyscore, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zremrangebyrank() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.Start", serde_json::json!(0)),
            ("CamelRedis.End", serde_json::json!(3)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zremrangebyrank, &ex).unwrap();
        assert_cmd_name(&cmd, "ZREMRANGEBYRANK");
        assert!(packed_contains(&cmd, "myzset"));
    }

    #[test]
    fn test_build_redis_cmd_zremrangebyrank_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Zremrangebyrank, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zremrangebyscore() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("myzset")),
            ("CamelRedis.Min", serde_json::json!(0.0)),
            ("CamelRedis.Max", serde_json::json!(50.0)),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zremrangebyscore, &ex).unwrap();
        assert_cmd_name(&cmd, "ZREMRANGEBYSCORE");
        assert!(packed_contains(&cmd, "myzset"));
    }

    #[test]
    fn test_build_redis_cmd_zremrangebyscore_missing_key() {
        let ex = Exchange::new(Message::default());
        assert!(build_redis_cmd(&RedisCommand::Zremrangebyscore, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zunionstore() {
        let ex = ex_with(&[
            ("CamelRedis.Destination", serde_json::json!("dest")),
            ("CamelRedis.Keys", serde_json::json!(["zset1", "zset2"])),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zunionstore, &ex).unwrap();
        assert_cmd_name(&cmd, "ZUNIONSTORE");
        assert!(packed_contains(&cmd, "dest"));
        assert!(packed_contains(&cmd, "zset1"));
        assert!(packed_contains(&cmd, "zset2"));
    }

    #[test]
    fn test_build_redis_cmd_zunionstore_missing_destination() {
        let ex = ex_with(&[("CamelRedis.Keys", serde_json::json!(["k1"]))]);
        assert!(build_redis_cmd(&RedisCommand::Zunionstore, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_zinterstore() {
        let ex = ex_with(&[
            ("CamelRedis.Destination", serde_json::json!("dest")),
            ("CamelRedis.Keys", serde_json::json!(["zset1", "zset2"])),
        ]);
        let cmd = build_redis_cmd(&RedisCommand::Zinterstore, &ex).unwrap();
        assert_cmd_name(&cmd, "ZINTERSTORE");
        assert!(packed_contains(&cmd, "dest"));
        assert!(packed_contains(&cmd, "zset1"));
        assert!(packed_contains(&cmd, "zset2"));
    }

    #[test]
    fn test_build_redis_cmd_zinterstore_missing_destination() {
        let ex = ex_with(&[("CamelRedis.Keys", serde_json::json!(["k1"]))]);
        assert!(build_redis_cmd(&RedisCommand::Zinterstore, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_rejects_non_zset() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("myzset"))]);
        let err = build_redis_cmd(&RedisCommand::Set, &ex).expect_err("non-zset should fail");
        assert!(err.to_string().contains("Not a sorted set command"));
    }
}
