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

pub(crate) fn resolve_publish_message(exchange: &Exchange) -> Result<String, CamelError> {
    match &exchange.input.body {
        Body::Text(t) => Ok(t.to_string()),
        Body::Json(v) => Ok(v.to_string()),
        _ => Err(CamelError::ProcessorError(
            "Message body must be convertible to Text for PUBLISH".into(),
        )),
    }
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

#[allow(dead_code)]
pub(crate) fn build_redis_cmd(
    cmd: &RedisCommand,
    exchange: &Exchange,
) -> Result<redis::Cmd, CamelError> {
    if !is_pubsub_producer_command(cmd) {
        return Err(CamelError::ProcessorError(
            "Not a pubsub producer command".into(),
        ));
    }

    let redis_cmd = match cmd {
        RedisCommand::Publish => {
            let channel = extract_publish_channel(exchange)?;
            let message = resolve_publish_message(exchange)?;
            let mut c = redis::cmd("PUBLISH");
            c.arg(channel).arg(message);
            c
        }
        _ => unreachable!("non-producer pubsub commands rejected above"),
    };

    Ok(redis_cmd)
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

    fn ex_with(headers: &[(&str, serde_json::Value)]) -> Exchange {
        let mut msg = Message::default();
        for (k, v) in headers {
            msg.set_header(*k, v.clone());
        }
        Exchange::new(msg)
    }

    fn ex_with_body(headers: &[(&str, serde_json::Value)], body: Body) -> Exchange {
        let mut msg = Message::new(body);
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

    // --- build_redis_cmd tests ---

    #[test]
    fn test_build_redis_cmd_publish_text_body() {
        let ex = ex_with_body(
            &[("CamelRedis.Channel", serde_json::json!("mychannel"))],
            Body::Text("hello world".to_string()),
        );
        let cmd = build_redis_cmd(&RedisCommand::Publish, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "PUBLISH");
        assert_eq!(cmd_args(&cmd), vec!["mychannel", "hello world"]);
    }

    #[test]
    fn test_build_redis_cmd_publish_json_body() {
        let ex = ex_with_body(
            &[("CamelRedis.Channel", serde_json::json!("events"))],
            Body::Json(serde_json::json!({"type": "click"})),
        );
        let cmd = build_redis_cmd(&RedisCommand::Publish, &ex).unwrap();
        assert_eq!(cmd_name(&cmd), "PUBLISH");
        assert_eq!(cmd_args(&cmd), vec!["events", "{\"type\":\"click\"}"]);
    }

    #[test]
    fn test_build_redis_cmd_publish_missing_channel() {
        let ex = ex_with_body(&[], Body::Text("msg".to_string()));
        assert!(build_redis_cmd(&RedisCommand::Publish, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_publish_missing_body() {
        let ex = ex_with(&[("CamelRedis.Channel", serde_json::json!("ch"))]);
        assert!(build_redis_cmd(&RedisCommand::Publish, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_publish_empty_body() {
        let ex = ex_with_body(
            &[("CamelRedis.Channel", serde_json::json!("ch"))],
            Body::Empty,
        );
        assert!(build_redis_cmd(&RedisCommand::Publish, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_rejects_non_pubsub() {
        let ex = ex_with_body(
            &[("CamelRedis.Channel", serde_json::json!("ch"))],
            Body::Text("msg".to_string()),
        );
        assert!(build_redis_cmd(&RedisCommand::Set, &ex).is_err());
    }

    #[test]
    fn test_build_redis_cmd_rejects_consumer_only() {
        let ex = ex_with_body(
            &[("CamelRedis.Channel", serde_json::json!("ch"))],
            Body::Text("msg".to_string()),
        );
        assert!(build_redis_cmd(&RedisCommand::Subscribe, &ex).is_err());
        assert!(build_redis_cmd(&RedisCommand::Psubscribe, &ex).is_err());
    }

    #[test]
    fn test_resolve_publish_message_text() {
        let ex = ex_with_body(&[], Body::Text("payload".to_string()));
        assert_eq!(resolve_publish_message(&ex).unwrap(), "payload");
    }

    #[test]
    fn test_resolve_publish_message_json() {
        let ex = ex_with_body(&[], Body::Json(serde_json::json!([1, 2, 3])));
        assert_eq!(resolve_publish_message(&ex).unwrap(), "[1,2,3]");
    }

    #[test]
    fn test_resolve_publish_message_empty_body_errors() {
        let ex = ex_with_body(&[], Body::Empty);
        assert!(resolve_publish_message(&ex).is_err());
    }
}
