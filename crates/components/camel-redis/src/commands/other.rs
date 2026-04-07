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
}
