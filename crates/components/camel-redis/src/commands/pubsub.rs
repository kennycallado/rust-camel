use super::require_str_header;
use crate::config::RedisCommand;
use camel_component_api::{Body, CamelError, Exchange};
use redis::AsyncCommands;
use redis::aio::MultiplexedConnection;

pub(crate) fn is_pubsub_producer_command(cmd: &RedisCommand) -> bool {
    matches!(cmd, RedisCommand::Publish)
}

pub(crate) fn is_pubsub_consumer_only_command(cmd: &RedisCommand) -> bool {
    matches!(cmd, RedisCommand::Subscribe | RedisCommand::Psubscribe)
}

pub(crate) fn extract_publish_channel(exchange: &Exchange) -> Result<String, CamelError> {
    require_str_header(exchange, "CamelRedis.Channel")
        .map(|s| s.to_string())
        .map_err(|_| {
            CamelError::ProcessorError("Missing required header: CamelRedis.Channel".into())
        })
}

pub(crate) fn extract_publish_message(exchange: &mut Exchange) -> Result<String, CamelError> {
    let body = std::mem::replace(&mut exchange.input.body, Body::Empty)
        .try_into_text()
        .map_err(|e| {
            CamelError::ProcessorError(format!(
                "Message body must be convertible to Text for PUBLISH: {}",
                e
            ))
        })?;
    Ok(body
        .as_text()
        .expect("try_into_text guarantees Text")
        .to_string())
}

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    if is_pubsub_consumer_only_command(cmd) {
        return Err(CamelError::ProcessorError(
            "SUBSCRIBE/PSUBSCRIBE are consumer-only commands. Use from() instead of to()".into(),
        ));
    }
    if !is_pubsub_producer_command(cmd) {
        return Err(CamelError::ProcessorError("Not a pub/sub command".into()));
    }

    let result: serde_json::Value = match cmd {
        RedisCommand::Publish => {
            let channel = extract_publish_channel(exchange)?;
            let message = extract_publish_message(exchange)?;

            let receivers: i64 = conn
                .publish(&channel, &message)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis PUBLISH failed: {}", e)))?;

            serde_json::json!(receivers)
        }
        _ => unreachable!("non-producer pubsub commands rejected above"),
    };

    exchange.input.body = Body::Json(result);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::Message;

    #[test]
    fn test_pubsub_command_classification() {
        assert!(is_pubsub_producer_command(&RedisCommand::Publish));
        assert!(is_pubsub_consumer_only_command(&RedisCommand::Subscribe));
        assert!(is_pubsub_consumer_only_command(&RedisCommand::Psubscribe));
        assert!(!is_pubsub_producer_command(&RedisCommand::Set));
    }

    #[test]
    fn test_extract_publish_channel_requires_header() {
        let exchange = Exchange::new(Message::default());
        let err = extract_publish_channel(&exchange).expect_err("channel header is required");
        assert!(err.to_string().contains("CamelRedis.Channel"));
    }

    #[test]
    fn test_extract_publish_message_from_text_body() {
        let mut exchange = Exchange::new(Message::new(Body::Text("event-1".to_string())));
        let message = extract_publish_message(&mut exchange).expect("text body should be accepted");
        assert_eq!(message, "event-1");
        assert!(matches!(exchange.input.body, Body::Empty));
    }

    #[test]
    fn test_extract_publish_message_serializes_json_body() {
        let mut exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({"a": 1}))));
        let message = extract_publish_message(&mut exchange).expect("json body should serialize");
        assert_eq!(message, "{\"a\":1}");
    }
}
