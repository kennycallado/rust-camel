use crate::config::RedisCommand;
use camel_component_api::{Body, CamelError, Exchange};
use redis::aio::MultiplexedConnection;

pub(crate) fn is_other_command(cmd: &RedisCommand) -> bool {
    matches!(
        cmd,
        RedisCommand::Ping | RedisCommand::Echo | RedisCommand::Wait
    )
}

/// Header that carries WAIT's replica count.
const NUM_REPLICAS_HEADER: &str = "CamelRedis.NumReplicas";

/// Header that carries WAIT's per-attempt timeout in milliseconds.
///
/// Deliberately distinct from the TTL header `CamelRedis.Timeout`: a WAIT
/// timeout bounds an acknowledgement probe, it does not expire a key.
const WAIT_TIMEOUT_HEADER: &str = "CamelRedis.WaitTimeout";

/// Named error for a missing WAIT operand.
fn missing_required(header: &str) -> CamelError {
    CamelError::ProcessorError(format!("Missing required header: {header}"))
}

/// Named error for a WAIT operand above Redis's signed 64-bit wire range.
///
/// Redis parses `WAIT`'s arguments as signed 64-bit integers, so a `u64`
/// above `i64::MAX` cannot travel on the wire. The parsed value is numeric,
/// so the diagnostic is bounded even though the header is untrusted.
fn wait_wire_range(header: &str, value: u64) -> CamelError {
    CamelError::ProcessorError(format!(
        "Invalid {header}: value {value} exceeds the signed 64-bit Redis wire range (i64::MAX = {}); refusing to send an unparseable WAIT argument",
        i64::MAX
    ))
}

/// Resolves `CamelRedis.NumReplicas` for `WAIT`.
///
/// Missing, a non-`u64` value, and a value above `i64::MAX` all fail closed
/// with a named error before send (ADR-0032).
pub(crate) fn resolve_num_replicas(exchange: &Exchange) -> Result<u64, CamelError> {
    let value = super::get_u64_header(exchange, NUM_REPLICAS_HEADER)?
        .ok_or_else(|| missing_required(NUM_REPLICAS_HEADER))?;
    if i64::try_from(value).is_err() {
        return Err(wait_wire_range(NUM_REPLICAS_HEADER, value));
    }
    Ok(value)
}

/// Resolves `CamelRedis.WaitTimeout`, WAIT's timeout in milliseconds.
///
/// A missing or zero value fails closed before send: Redis `WAIT ... 0`
/// blocks forever. A value above `i64::MAX` is outside the signed wire range
/// and fails with a named error (ADR-0032).
pub(crate) fn resolve_wait_timeout(exchange: &Exchange) -> Result<u64, CamelError> {
    let value = super::get_u64_header(exchange, WAIT_TIMEOUT_HEADER)?
        .ok_or_else(|| missing_required(WAIT_TIMEOUT_HEADER))?;
    if value == 0 {
        return Err(CamelError::ProcessorError(format!(
            "Invalid {WAIT_TIMEOUT_HEADER}: a zero timeout makes Redis WAIT block forever; set a positive millisecond timeout"
        )));
    }
    if i64::try_from(value).is_err() {
        return Err(wait_wire_range(WAIT_TIMEOUT_HEADER, value));
    }
    Ok(value)
}

/// Builds `WAIT numreplicas timeout`.
pub(crate) fn encode_wait(num_replicas: u64, timeout_ms: u64) -> redis::Cmd {
    let mut command = redis::cmd("WAIT");
    command.arg(num_replicas).arg(timeout_ms);
    command
}

pub(crate) fn extract_echo_message(exchange: &mut Exchange) -> String {
    std::mem::replace(&mut exchange.input.body, Body::Empty)
        .try_into_text()
        .ok()
        .and_then(|b| b.as_text().map(|s| s.to_string()))
        .unwrap_or_default()
}

#[allow(dead_code)]
pub(crate) fn build_redis_cmd(
    cmd: &RedisCommand,
    exchange: &Exchange,
) -> Result<redis::Cmd, CamelError> {
    if !is_other_command(cmd) {
        return Err(CamelError::ProcessorError("Not an other command".into()));
    }

    let redis_cmd = match cmd {
        RedisCommand::Ping => redis::cmd("PING"),
        RedisCommand::Echo => {
            let message = match &exchange.input.body {
                Body::Text(s) => s.clone(),
                Body::Json(v) => v.to_string(),
                _ => String::new(),
            };
            let mut c = redis::cmd("ECHO");
            c.arg(message);
            c
        }
        RedisCommand::Wait => {
            let num_replicas = resolve_num_replicas(exchange)?;
            let timeout_ms = resolve_wait_timeout(exchange)?;
            encode_wait(num_replicas, timeout_ms)
        }
        _ => unreachable!("non-other commands rejected above"),
    };

    Ok(redis_cmd)
}

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    if !is_other_command(cmd) {
        return Err(CamelError::ProcessorError("Not an other command".into()));
    }

    let result: serde_json::Value = match cmd {
        RedisCommand::Ping => {
            let response: String = redis::cmd("PING")
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("PING", e))?;

            // REDIS-013: Validate PING response is exactly "PONG"
            if response != "PONG" {
                return Err(CamelError::ProcessorError(format!(
                    "Unexpected PING response: expected 'PONG', got '{}'",
                    response
                )));
            }

            serde_json::Value::String(response)
        }
        RedisCommand::Echo => {
            let message = extract_echo_message(exchange);

            let response: String = redis::cmd("ECHO")
                .arg(message)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("ECHO", e))?;

            serde_json::Value::String(response)
        }
        RedisCommand::Wait => {
            // Resolve both operands before the command reaches the
            // connection: a bad WAIT argument must never be sent (ADR-0032).
            let num_replicas = resolve_num_replicas(exchange)?;
            let timeout_ms = resolve_wait_timeout(exchange)?;
            let acked: i64 = encode_wait(num_replicas, timeout_ms)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("WAIT", e))?;
            serde_json::json!(acked)
        }
        _ => unreachable!("non-other commands rejected above"),
    };

    exchange.input.body = Body::Json(result);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::Message;

    #[test]
    fn test_is_other_command() {
        assert!(is_other_command(&RedisCommand::Ping));
        assert!(is_other_command(&RedisCommand::Echo));
        assert!(is_other_command(&RedisCommand::Wait));
        assert!(!is_other_command(&RedisCommand::Set));
    }

    #[test]
    fn test_extract_echo_message_from_text_body() {
        let mut exchange = Exchange::new(Message::new(Body::Text("hello".to_string())));
        let message = extract_echo_message(&mut exchange);
        assert_eq!(message, "hello");
        assert!(matches!(exchange.input.body, Body::Empty));
    }

    #[test]
    fn test_extract_echo_message_serializes_json_body() {
        let mut exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({"x": 1}))));
        let message = extract_echo_message(&mut exchange);
        assert_eq!(message, "{\"x\":1}");
        assert!(matches!(exchange.input.body, Body::Empty));
    }

    fn ex_with(headers: &[(&str, serde_json::Value)]) -> Exchange {
        let mut msg = Message::default();
        for (k, v) in headers {
            msg.set_header(*k, v.clone());
        }
        Exchange::new(msg)
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

    #[test]
    fn test_build_redis_cmd_ping() {
        let ex = Exchange::new(Message::default());
        let cmd = build_redis_cmd(&RedisCommand::Ping, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "PING");
        assert!(cmd_args(&cmd).is_empty());
    }

    #[test]
    fn test_build_redis_cmd_echo_with_text_body() {
        let ex = Exchange::new(Message::new(Body::Text("hello".to_string())));
        let cmd = build_redis_cmd(&RedisCommand::Echo, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "ECHO");
        assert_eq!(cmd_args(&cmd), vec!["hello"]);
    }

    #[test]
    fn test_build_redis_cmd_echo_with_json_body() {
        let ex = Exchange::new(Message::new(Body::Json(
            serde_json::json!({"key": "value"}),
        )));
        let cmd = build_redis_cmd(&RedisCommand::Echo, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "ECHO");
        assert_eq!(cmd_args(&cmd), vec!["{\"key\":\"value\"}"]);
    }

    #[test]
    fn test_build_redis_cmd_echo_with_empty_body() {
        let ex = Exchange::new(Message::new(Body::Empty));
        let cmd = build_redis_cmd(&RedisCommand::Echo, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "ECHO");
        assert_eq!(cmd_args(&cmd), vec![""]);
    }

    #[test]
    fn test_build_redis_cmd_rejects_non_other() {
        let ex = ex_with(&[("CamelRedis.Key", serde_json::json!("k"))]);
        assert!(build_redis_cmd(&RedisCommand::Set, &ex).is_err());
        assert!(build_redis_cmd(&RedisCommand::Hset, &ex).is_err());
        assert!(build_redis_cmd(&RedisCommand::Sadd, &ex).is_err());
        assert!(build_redis_cmd(&RedisCommand::Lpush, &ex).is_err());
    }

    #[test]
    fn test_is_other_command_rejects_non_other() {
        assert!(!is_other_command(&RedisCommand::Set));
        assert!(!is_other_command(&RedisCommand::Hget));
        assert!(!is_other_command(&RedisCommand::Publish));
        assert!(!is_other_command(&RedisCommand::Del));
    }

    // ── redisbmp task 4.1: WAIT wire frame and pre-send header checks ───────

    #[test]
    fn test_wait_encodes_wire_frame() {
        let command = super::encode_wait(1, 1000);
        assert_eq!(cmd_name(&command), "WAIT");
        assert_eq!(cmd_args(&command), vec!["1", "1000"]);
    }

    #[test]
    fn test_build_redis_cmd_wait_ok() {
        let ex = ex_with(&[
            ("CamelRedis.NumReplicas", serde_json::json!(1u64)),
            ("CamelRedis.WaitTimeout", serde_json::json!(1000u64)),
        ]);

        let command = build_redis_cmd(&RedisCommand::Wait, &ex)
            .expect("valid WAIT headers must encode without hitting the unreachable arm");

        assert_eq!(cmd_name(&command), "WAIT");
        assert_eq!(cmd_args(&command), vec!["1", "1000"]);
    }

    #[test]
    fn test_build_redis_cmd_wait_invalid_headers_no_panic() {
        let zero = ex_with(&[
            ("CamelRedis.NumReplicas", serde_json::json!(1u64)),
            ("CamelRedis.WaitTimeout", serde_json::json!(0u64)),
        ]);
        let err = build_redis_cmd(&RedisCommand::Wait, &zero)
            .expect_err("a zero WaitTimeout must be refused, not reach the unreachable arm");
        assert!(
            err.to_string().contains("CamelRedis.WaitTimeout"),
            "error must name CamelRedis.WaitTimeout: {err}"
        );

        let missing = ex_with(&[("CamelRedis.NumReplicas", serde_json::json!(1u64))]);
        let err = build_redis_cmd(&RedisCommand::Wait, &missing)
            .expect_err("a missing WaitTimeout must be refused, not reach the unreachable arm");
        assert!(
            err.to_string().contains("CamelRedis.WaitTimeout"),
            "error must name CamelRedis.WaitTimeout: {err}"
        );
    }
}
