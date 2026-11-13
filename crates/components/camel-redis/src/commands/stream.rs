//! Stream producer commands: `XADD` and `XLEN`.
//!
//! `XADD` appends exactly one entry to the stream named by
//! `CamelRedis.Key` (falling back to the URI `key=`), with exactly one field
//! named `body` whose value is the exchange body encoded as raw bytes. The entry
//! ID is always `*` (server-assigned) and the exchange body is kept unchanged;
//! the returned entry id is exposed as `CamelRedis.StreamId`.
//!
//! `XADD` is deliberately NOT idempotent (at-least-once): it is never retried
//! by the component and duplicates under route redelivery are accepted.
//! A transport failure during the command attempt has an unknowable outcome
//! (the entry may or may not exist), so it is reported as an unknown-outcome
//! processor error that never claims the request was sent. A server error
//! reply (WRONGTYPE, NOPERM, OOM, …) is definitive instead: the entry was not
//! appended, so that outcome is reported as a plain failure.
//! `XLEN` is a read and is classified as idempotent.

use super::require_key;
use crate::config::RedisCommand;
use camel_component_api::{Body, CamelError, Exchange};
use redis::aio::MultiplexedConnection;
use std::sync::Arc;

pub(crate) fn is_stream_producer_command(cmd: &RedisCommand) -> bool {
    matches!(cmd, RedisCommand::Xadd | RedisCommand::Xlen)
}

/// Resolves the single `body` field value for `XADD` from the exchange body.
///
/// [`encode_body_value`] accepts text, XML, structured bodies (compact JSON),
/// and raw bytes (sent verbatim, binary-safe). `Body::Empty` and `Body::Stream`
/// are rejected here, before any command reaches Redis, with an error naming
/// `XADD` and the rejected body variant.
pub(crate) fn resolve_xadd_body_bytes(exchange: &Exchange) -> Result<Vec<u8>, CamelError> {
    super::encode_body_value(&exchange.input.body, "XADD")
}

async fn execute_xlen(
    exchange: &mut Exchange,
    conn: &mut MultiplexedConnection,
) -> Result<(), CamelError> {
    let key = require_key(exchange)?;
    let count: u64 = redis::cmd("XLEN")
        .arg(&key)
        .query_async(conn)
        .await
        .map_err(|e| crate::transport_error::redis_error_to_camel("XLEN", e))?;
    exchange.input.body = Body::Json(serde_json::json!(count));
    Ok(())
}

/// Processor error text for an `XADD` whose transport attempt failed.
///
/// `query_async` exposes no write-completion status, so "was it written?" is
/// unknowable: the entry may or may not exist. The text must not claim the
/// request was sent, and the component never retries to resolve the doubt.
const XADD_OUTCOME_UNKNOWN: &str = "XADD outcome unknown: the command attempt \
     failed during transport; the entry may or may not exist";

/// Processor error text for an `XADD` the server answered with an error reply.
///
/// A RESP error reply is definitive: `XADD` was received and rejected, so the
/// entry was not appended. The `RedisError` stays in the source chain.
const XADD_SERVER_REJECTED: &str = "XADD rejected by the server: the entry was \
     not appended";

/// Appends one entry (`*` id, one `body` field) and exposes the entry id.
///
/// The exchange body is never replaced: it stays exactly what the route sent.
/// `CamelRedis.StreamId` carries the server-assigned entry id and
/// `CamelRedis.Stream` the resolved stream key; both overwrite upstream
/// values.
///
/// Setup failures (key resolution, body conversion) keep their plain error and
/// never reach the wire. A server error reply proves the entry was not
/// appended; an error without a server code is a transport failure with an
/// unknown outcome, so its text names the ambiguity instead of claiming a send
/// or a failure.
async fn execute_xadd(
    exchange: &mut Exchange,
    conn: &mut MultiplexedConnection,
) -> Result<(), CamelError> {
    let key = require_key(exchange)?;
    let body = resolve_xadd_body_bytes(exchange)?;
    let entry_id: String = redis::cmd("XADD")
        .arg(&key)
        .arg("*")
        .arg("body")
        .arg(&body)
        .query_async(conn)
        .await
        .map_err(|e| {
            let source: Arc<dyn std::error::Error + Send + Sync> = Arc::new(e);
            // A server-coded reply is definitive; only an uncoded transport
            // failure leaves the outcome unknown. Probe the preserved source
            // error itself so the decision reads the RedisError code, never
            // the rendered message text.
            let server_coded =
                crate::transport_error::redis_error_code_in_source(Some(&*source)).is_some();
            let text = if server_coded {
                XADD_SERVER_REJECTED
            } else {
                XADD_OUTCOME_UNKNOWN
            };
            CamelError::ProcessorErrorWithSource(text.to_string(), source)
        })?;
    exchange
        .input
        .set_header("CamelRedis.StreamId", entry_id.as_str());
    exchange.input.set_header("CamelRedis.Stream", key.as_str());
    Ok(())
}

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    if !is_stream_producer_command(cmd) {
        return Err(CamelError::ProcessorError(
            "Not a stream producer command".into(),
        ));
    }

    match cmd {
        RedisCommand::Xadd => execute_xadd(exchange, conn).await,
        RedisCommand::Xlen => execute_xlen(exchange, conn).await,
        _ => unreachable!("non-stream producer commands rejected above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::{Message, StreamBody, StreamMetadata};
    use serde_json::json;
    use std::sync::Arc;

    fn ex_with_body(body: Body) -> Exchange {
        Exchange::new(Message::new(body))
    }

    #[test]
    fn test_is_stream_producer_command_matches_only_xadd_xlen() {
        assert!(is_stream_producer_command(&RedisCommand::Xadd));
        assert!(is_stream_producer_command(&RedisCommand::Xlen));
        assert!(!is_stream_producer_command(&RedisCommand::Xreadgroup));
        assert!(!is_stream_producer_command(&RedisCommand::Get));
    }

    #[test]
    fn test_resolve_xadd_body_bytes_text_kept_as_is() {
        let exchange = ex_with_body(Body::Text("María & café ☕".to_string()));
        assert_eq!(
            resolve_xadd_body_bytes(&exchange).expect("text body must be accepted"),
            "María & café ☕".as_bytes()
        );
    }

    #[test]
    fn test_resolve_xadd_body_bytes_json_is_compact() {
        let exchange = ex_with_body(Body::Json(json!({"a": 1})));
        assert_eq!(
            resolve_xadd_body_bytes(&exchange).expect("json body must be accepted"),
            br#"{"a":1}"#.as_slice()
        );
    }

    #[test]
    fn test_resolve_xadd_body_bytes_utf8_bytes_kept_verbatim() {
        let exchange = ex_with_body(Body::from("hello".as_bytes().to_vec()));
        assert_eq!(
            resolve_xadd_body_bytes(&exchange).expect("UTF-8 bytes must be accepted"),
            b"hello".as_slice()
        );
    }

    #[test]
    fn test_resolve_xadd_body_bytes_empty_fails_naming_xadd() {
        let err = resolve_xadd_body_bytes(&ex_with_body(Body::Empty))
            .expect_err("empty body must be rejected");
        let text = err.to_string();
        assert!(text.contains("XADD"), "error must name XADD: {text}");
        assert!(
            text.contains("Empty"),
            "error must name the rejected body form: {text}"
        );
    }

    #[test]
    fn test_resolve_xadd_body_bytes_stream_fails() {
        let exchange = ex_with_body(Body::Stream(StreamBody {
            stream: Arc::new(tokio::sync::Mutex::new(None)),
            metadata: StreamMetadata::default(),
        }));
        let err = resolve_xadd_body_bytes(&exchange).expect_err("stream body must be rejected");
        let text = err.to_string();
        assert!(text.contains("XADD"), "error must name XADD: {text}");
        assert!(
            text.contains("Stream"),
            "error must name the rejected body form: {text}"
        );
    }

    #[test]
    fn test_resolve_xadd_body_bytes_non_utf8_bytes_kept_verbatim() {
        let exchange = ex_with_body(Body::from(vec![0xff, 0xfe]));
        assert_eq!(
            resolve_xadd_body_bytes(&exchange).expect("byte bodies must be accepted"),
            vec![0xff, 0xfe]
        );
    }
}
