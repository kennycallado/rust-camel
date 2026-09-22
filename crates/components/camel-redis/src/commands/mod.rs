pub mod hash;
pub mod key;
pub mod list;
pub mod other;
pub mod pubsub;
pub mod set;
pub mod string;
pub mod zset;

use camel_component_api::{CamelError, Exchange};

// ── Header extraction helpers ────────────────────────────────────────────────

pub fn get_str_header<'a>(exchange: &'a Exchange, key: &str) -> Option<&'a str> {
    exchange.input.header(key).and_then(|v| v.as_str())
}

pub fn get_u64_header(exchange: &Exchange, key: &str) -> Option<u64> {
    exchange.input.header(key).and_then(|v| v.as_u64())
}

pub fn get_i64_header(exchange: &Exchange, key: &str) -> Option<i64> {
    exchange.input.header(key).and_then(|v| v.as_i64())
}

pub fn get_f64_header(exchange: &Exchange, key: &str) -> Option<f64> {
    exchange.input.header(key).and_then(|v| v.as_f64())
}

pub fn get_bool_header(exchange: &Exchange, key: &str) -> Option<bool> {
    exchange.input.header(key).and_then(|v| v.as_bool())
}

pub fn get_str_vec_header(exchange: &Exchange, key: &str) -> Option<Vec<String>> {
    exchange.input.header(key).and_then(|v| {
        v.as_array().map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
    })
}

pub fn get_value_header(exchange: &Exchange, key: &str) -> Option<serde_json::Value> {
    exchange.input.header(key).cloned()
}

pub fn require_str_header<'a>(exchange: &'a Exchange, key: &str) -> Result<&'a str, CamelError> {
    get_str_header(exchange, key)
        .ok_or_else(|| CamelError::ProcessorError(format!("Missing required header: {}", key)))
}

pub fn require_key(exchange: &Exchange) -> Result<String, CamelError> {
    require_str_header(exchange, "CamelRedis.Key").map(|s| s.to_string())
}

pub fn require_value(exchange: &Exchange) -> Result<serde_json::Value, CamelError> {
    get_value_header(exchange, "CamelRedis.Value").ok_or_else(|| {
        CamelError::ProcessorError("Missing required header: CamelRedis.Value".into())
    })
}

pub(crate) fn value_to_redis_arg(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RedisCommand;
    use crate::transport_error::is_transient_redis_error;
    use camel_component_api::{Exchange, Message};
    use std::error::Error as _;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn make_exchange_with_header(key: &str, val: serde_json::Value) -> Exchange {
        let mut msg = Message::default();
        msg.set_header(key, val);
        Exchange::new(msg)
    }

    #[test]
    fn test_get_str_header_found() {
        let ex =
            make_exchange_with_header("CamelRedis.Key", serde_json::Value::String("mykey".into()));
        assert_eq!(get_str_header(&ex, "CamelRedis.Key"), Some("mykey"));
    }

    #[test]
    fn test_get_str_header_missing() {
        let ex = Exchange::new(Message::default());
        assert_eq!(get_str_header(&ex, "CamelRedis.Key"), None);
    }

    #[test]
    fn test_get_u64_header() {
        let ex = make_exchange_with_header("CamelRedis.Timeout", serde_json::json!(30u64));
        assert_eq!(get_u64_header(&ex, "CamelRedis.Timeout"), Some(30));
    }

    #[test]
    fn test_get_f64_header() {
        let ex = make_exchange_with_header("CamelRedis.Score", serde_json::json!(3.15f64));
        assert_eq!(get_f64_header(&ex, "CamelRedis.Score"), Some(3.15));
    }

    #[test]
    fn test_get_i64_header() {
        let ex = make_exchange_with_header("CamelRedis.Start", serde_json::json!(-1i64));
        assert_eq!(get_i64_header(&ex, "CamelRedis.Start"), Some(-1));
    }

    #[test]
    fn test_get_bool_header() {
        let ex = make_exchange_with_header("CamelRedis.WithScore", serde_json::json!(true));
        assert_eq!(get_bool_header(&ex, "CamelRedis.WithScore"), Some(true));
    }

    #[test]
    fn test_get_str_vec_header() {
        let ex = make_exchange_with_header("CamelRedis.Keys", serde_json::json!(["a", "b", "c"]));
        assert_eq!(
            get_str_vec_header(&ex, "CamelRedis.Keys"),
            Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
        );
    }

    #[test]
    fn test_require_str_header_ok() {
        let ex = make_exchange_with_header("CamelRedis.Key", serde_json::Value::String("k".into()));
        assert_eq!(require_str_header(&ex, "CamelRedis.Key").unwrap(), "k");
    }

    #[test]
    fn test_require_str_header_missing_returns_err() {
        let ex = Exchange::new(Message::default());
        assert!(require_str_header(&ex, "CamelRedis.Key").is_err());
    }

    #[test]
    fn test_value_to_redis_arg_preserves_string_content() {
        assert_eq!(value_to_redis_arg(&serde_json::json!("hello")), "hello");
    }

    #[test]
    fn test_value_to_redis_arg_serializes_non_strings() {
        assert_eq!(
            value_to_redis_arg(&serde_json::json!({"a": 1})),
            r#"{"a":1}"#
        );
        assert_eq!(value_to_redis_arg(&serde_json::json!(42)), "42");
    }

    // ── Command wrap source preservation (rediserr task 2.1) ────────────────

    /// Returns the first argument of the next complete RESP array frame
    /// in `buf`, plus the frame's total byte length. All frames on this
    /// loopback path are plain ASCII (GET and handshake verbs).
    fn next_resp_command(buf: &[u8]) -> Option<(String, usize)> {
        if buf.first() != Some(&b'*') {
            return None;
        }
        let nl = buf.iter().position(|&b| b == b'\n')?;
        let argc = std::str::from_utf8(&buf[1..nl - 1])
            .ok()?
            .parse::<usize>()
            .ok()?;
        let mut pos = nl + 1;
        let mut first_arg = None;
        for i in 0..argc {
            if buf.get(pos) != Some(&b'$') {
                return None;
            }
            let nl = buf[pos..].iter().position(|&b| b == b'\n')? + pos;
            let len = std::str::from_utf8(&buf[pos + 1..nl - 1])
                .ok()?
                .parse::<usize>()
                .ok()?;
            let arg_start = nl + 1;
            let arg_end = arg_start + len;
            if buf.len() < arg_end + 2 {
                return None;
            }
            if i == 0 {
                first_arg = Some(String::from_utf8_lossy(&buf[arg_start..arg_end]).into_owned());
            }
            pos = arg_end + 2;
        }
        first_arg.map(|verb| (verb, pos))
    }

    /// Loopback RESP peer for the command-path wrap test: answers the
    /// redis driver handshake commands (`+OK`), then closes the socket
    /// on the first application command, so that command fails with a
    /// genuine `redis::RedisError` (peer hang-up) instead of a canned
    /// reply. Unit-test scale port of the integration harness
    /// `HandshakeStub` in `tests/common/mod.rs`.
    async fn handshake_then_close_peer() -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback bind must succeed");
        let addr = listener.local_addr().expect("bound listener has an addr");
        tokio::spawn(async move {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 512];
            loop {
                // Answer every complete RESP frame currently buffered
                // (the driver may pipeline its handshake commands).
                while let Some((verb, consumed)) = next_resp_command(&buf) {
                    buf.drain(..consumed);
                    match verb.as_str() {
                        "CLIENT" | "SELECT" | "AUTH" => {
                            if sock.write_all(b"+OK\r\n").await.is_err() {
                                return;
                            }
                        }
                        // First application command: drop the socket so
                        // the driver's read fails.
                        _ => return,
                    }
                }
                let Ok(n) = sock.read(&mut chunk).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
        });
        addr
    }

    #[tokio::test]
    async fn command_wrap_preserves_redis_source() {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let addr = handshake_then_close_peer().await;
            let client =
                redis::Client::open(format!("redis://{addr}/")).expect("loopback URL must parse");
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .expect("stub completes the driver handshake");

            let mut msg = Message::default();
            msg.set_header("CamelRedis.Key", serde_json::Value::String("k".into()));
            let mut exchange = Exchange::new(msg);

            // Normal command path: string::dispatch drives the GET whose
            // wrap site this change converts. The peer closed the socket,
            // so the command fails with a real redis::RedisError.
            let err = string::dispatch(&RedisCommand::Get, &mut conn, &mut exchange)
                .await
                .err()
                .expect("GET against a closed peer must fail");

            assert!(
                is_transient_redis_error(&err),
                "peer-hangup io error must classify transient: {err}"
            );

            // The redis::RedisError must be preserved in the source
            // chain. std wraps the Arc<dyn Error> hop, so unwrap one
            // level like the classifier's walk does before downcasting.
            let src = err
                .source()
                .expect("command wrap must attach the redis error as source");
            let src = src
                .downcast_ref::<Arc<dyn std::error::Error + Send + Sync>>()
                .map(|arc| &**arc as &(dyn std::error::Error + 'static))
                .unwrap_or(src);
            let redis_err = src
                .downcast_ref::<redis::RedisError>()
                .expect("source must downcast to redis::RedisError");

            // Byte-identical wrap text: the legacy site rendered
            // `format!("Redis GET failed: {e}")` inside ProcessorError,
            // whose Display prefixes `Processor error: `.
            assert_eq!(
                err.to_string(),
                format!("Processor error: Redis GET failed: {redis_err}"),
                "wrap text must stay byte-identical to the legacy ProcessorError"
            );
        })
        .await;
        assert!(outcome.is_ok(), "command wrap test timed out: {outcome:?}");
    }
}
