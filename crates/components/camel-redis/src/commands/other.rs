use crate::config::RedisCommand;
use camel_component_api::{Body, CamelError, Exchange};
use redis::aio::MultiplexedConnection;

pub(crate) fn is_other_command(cmd: &RedisCommand) -> bool {
    matches!(cmd, RedisCommand::Ping | RedisCommand::Echo)
}

pub(crate) fn extract_echo_message(exchange: &mut Exchange) -> String {
    std::mem::replace(&mut exchange.input.body, Body::Empty)
        .try_into_text()
        .ok()
        .and_then(|b| b.as_text().map(|s| s.to_string()))
        .unwrap_or_default()
}

#[allow(dead_code)]
pub(crate) fn build_redis_cmd(cmd: &RedisCommand, exchange: &Exchange) -> Result<redis::Cmd, CamelError> {
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
                .map_err(|e| CamelError::ProcessorError(format!("Redis PING failed: {}", e)))?;

            serde_json::Value::String(response)
        }
        RedisCommand::Echo => {
            let message = extract_echo_message(exchange);

            let response: String = redis::cmd("ECHO")
                .arg(message)
                .query_async(conn)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis ECHO failed: {}", e)))?;

            serde_json::Value::String(response)
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
        let ex = Exchange::new(Message::new(Body::Json(serde_json::json!({"key": "value"}))));
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
}
