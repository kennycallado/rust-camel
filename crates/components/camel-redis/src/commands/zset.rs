use super::{
    get_bool_header, get_f64_header, get_i64_header, get_str_header, get_str_vec_header,
    require_key, require_value,
};
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
            RedisCommand::Zadd => {
                let key = require_key(exchange)?;
                let score = get_f64_header(exchange, "CamelRedis.Score").unwrap_or(0.0);
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
                let start = get_i64_header(exchange, "CamelRedis.Start").unwrap_or(0) as isize;
                let end = get_i64_header(exchange, "CamelRedis.End").unwrap_or(-1) as isize;
                let with_scores =
                    get_bool_header(exchange, "CamelRedis.WithScore").unwrap_or(false);
                if with_scores {
                    let vals: Vec<(String, f64)> = conn
                        .zrange_withscores(&key, start, end)
                        .await
                        .map_err(|e| {
                        CamelError::ProcessorError(format!("Redis ZRANGE failed: {e}"))
                    })?;
                    serde_json::json!(vals.into_iter()
                    .map(|(member, score)| serde_json::json!({"member": member, "score": score}))
                    .collect::<Vec<_>>())
                } else {
                    let vals: Vec<String> = conn.zrange(&key, start, end).await.map_err(|e| {
                        CamelError::ProcessorError(format!("Redis ZRANGE failed: {e}"))
                    })?;
                    serde_json::json!(vals)
                }
            }
            RedisCommand::Zrevrange => {
                let key = require_key(exchange)?;
                let start = get_i64_header(exchange, "CamelRedis.Start").unwrap_or(0) as isize;
                let end = get_i64_header(exchange, "CamelRedis.End").unwrap_or(-1) as isize;
                let with_scores =
                    get_bool_header(exchange, "CamelRedis.WithScore").unwrap_or(false);
                if with_scores {
                    let vals: Vec<(String, f64)> = conn
                        .zrevrange_withscores(&key, start, end)
                        .await
                        .map_err(|e| {
                            CamelError::ProcessorError(format!("Redis ZREVRANGE failed: {e}"))
                        })?;
                    serde_json::json!(vals.into_iter()
                    .map(|(member, score)| serde_json::json!({"member": member, "score": score}))
                    .collect::<Vec<_>>())
                } else {
                    let vals: Vec<String> =
                        conn.zrevrange(&key, start, end).await.map_err(|e| {
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
                rank.map(|r| serde_json::json!(r))
                    .unwrap_or(serde_json::Value::Null)
            }
            RedisCommand::Zrevrank => {
                let key = require_key(exchange)?;
                let member = require_value(exchange)?;
                let rank: Option<i64> =
                    conn.zrevrank(&key, member.to_string()).await.map_err(|e| {
                        CamelError::ProcessorError(format!("Redis ZREVRANK failed: {e}"))
                    })?;
                rank.map(|r| serde_json::json!(r))
                    .unwrap_or(serde_json::Value::Null)
            }
            RedisCommand::Zscore => {
                let key = require_key(exchange)?;
                let member = require_value(exchange)?;
                let score: Option<f64> = conn
                    .zscore(&key, member.to_string())
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis ZSCORE failed: {e}")))?;
                score
                    .map(|s| serde_json::json!(s))
                    .unwrap_or(serde_json::Value::Null)
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
                let increment = get_f64_header(exchange, "CamelRedis.Increment").unwrap_or(1.0);
                let member = require_value(exchange)?;
                let new_score: f64 = conn
                    .zincr(&key, member.to_string(), increment)
                    .await
                    .map_err(|e| {
                        CamelError::ProcessorError(format!("Redis ZINCRBY failed: {e}"))
                    })?;
                serde_json::json!(new_score)
            }
            RedisCommand::Zcount => {
                let key = require_key(exchange)?;
                let min = get_f64_header(exchange, "CamelRedis.Min").unwrap_or(f64::NEG_INFINITY);
                let max = get_f64_header(exchange, "CamelRedis.Max").unwrap_or(f64::INFINITY);
                let n: i64 = conn
                    .zcount(&key, min, max)
                    .await
                    .map_err(|e| CamelError::ProcessorError(format!("Redis ZCOUNT failed: {e}")))?;
                serde_json::json!(n)
            }
            RedisCommand::Zrangebyscore => {
                let key = require_key(exchange)?;
                let min = get_f64_header(exchange, "CamelRedis.Min").unwrap_or(f64::NEG_INFINITY);
                let max = get_f64_header(exchange, "CamelRedis.Max").unwrap_or(f64::INFINITY);
                let with_scores =
                    get_bool_header(exchange, "CamelRedis.WithScore").unwrap_or(false);
                if with_scores {
                    let vals: Vec<(String, f64)> = conn
                        .zrangebyscore_withscores(&key, min, max)
                        .await
                        .map_err(|e| {
                            CamelError::ProcessorError(format!("Redis ZRANGEBYSCORE failed: {e}"))
                        })?;
                    serde_json::json!(vals.into_iter()
                    .map(|(member, score)| serde_json::json!({"member": member, "score": score}))
                    .collect::<Vec<_>>())
                } else {
                    let vals: Vec<String> =
                        conn.zrangebyscore(&key, min, max).await.map_err(|e| {
                            CamelError::ProcessorError(format!("Redis ZRANGEBYSCORE failed: {e}"))
                        })?;
                    serde_json::json!(vals)
                }
            }
            RedisCommand::Zrevrangebyscore => {
                let key = require_key(exchange)?;
                let max = get_f64_header(exchange, "CamelRedis.Max").unwrap_or(f64::INFINITY);
                let min = get_f64_header(exchange, "CamelRedis.Min").unwrap_or(f64::NEG_INFINITY);
                let with_scores =
                    get_bool_header(exchange, "CamelRedis.WithScore").unwrap_or(false);
                if with_scores {
                    let vals: Vec<(String, f64)> = conn
                        .zrevrangebyscore_withscores(&key, max, min)
                        .await
                        .map_err(|e| {
                            CamelError::ProcessorError(format!(
                                "Redis ZREVRANGEBYSCORE failed: {e}"
                            ))
                        })?;
                    serde_json::json!(vals.into_iter()
                    .map(|(member, score)| serde_json::json!({"member": member, "score": score}))
                    .collect::<Vec<_>>())
                } else {
                    let vals: Vec<String> =
                        conn.zrevrangebyscore(&key, max, min).await.map_err(|e| {
                            CamelError::ProcessorError(format!(
                                "Redis ZREVRANGEBYSCORE failed: {e}"
                            ))
                        })?;
                    serde_json::json!(vals)
                }
            }
            RedisCommand::Zremrangebyrank => {
                let key = require_key(exchange)?;
                let start = get_i64_header(exchange, "CamelRedis.Start").unwrap_or(0) as isize;
                let end = get_i64_header(exchange, "CamelRedis.End").unwrap_or(-1) as isize;
                let n: usize = conn.zremrangebyrank(&key, start, end).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis ZREMRANGEBYRANK failed: {e}"))
                })?;
                serde_json::json!(n as i64)
            }
            RedisCommand::Zremrangebyscore => {
                let key = require_key(exchange)?;
                let min = get_f64_header(exchange, "CamelRedis.Min").unwrap_or(f64::NEG_INFINITY);
                let max = get_f64_header(exchange, "CamelRedis.Max").unwrap_or(f64::INFINITY);
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
                let dest = get_str_header(exchange, "CamelRedis.Destination").ok_or_else(|| {
                    CamelError::ProcessorError("Missing CamelRedis.Destination".into())
                })?;
                let keys = get_str_vec_header(exchange, "CamelRedis.Keys")
                    .unwrap_or_else(|| vec![require_key(exchange).unwrap_or_default()]);
                let n: i64 = conn.zunionstore(dest, &keys).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis ZUNIONSTORE failed: {e}"))
                })?;
                serde_json::json!(n)
            }
            RedisCommand::Zinterstore => {
                let dest = get_str_header(exchange, "CamelRedis.Destination").ok_or_else(|| {
                    CamelError::ProcessorError("Missing CamelRedis.Destination".into())
                })?;
                let keys = get_str_vec_header(exchange, "CamelRedis.Keys")
                    .unwrap_or_else(|| vec![require_key(exchange).unwrap_or_default()]);
                let n: i64 = conn.zinterstore(dest, &keys).await.map_err(|e| {
                    CamelError::ProcessorError(format!("Redis ZINTERSTORE failed: {e}"))
                })?;
                serde_json::json!(n)
            }
            _ => {
                return Err(CamelError::ProcessorError(
                    "Not a sorted set command".into(),
                ));
            }
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
        assert_eq!(
            crate::commands::get_str_header(&ex, "CamelRedis.Destination"),
            Some("dest")
        );
        assert_eq!(
            crate::commands::get_str_vec_header(&ex, "CamelRedis.Keys"),
            Some(vec!["zset1".to_string(), "zset2".to_string()])
        );
    }
}
