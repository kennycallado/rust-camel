use super::require_str_header;
use crate::config::RedisCommand;
use camel_api::{CamelError, Exchange, body::Body};
use redis::AsyncCommands;
use redis::aio::MultiplexedConnection;

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    let result: serde_json::Value = match cmd {
        RedisCommand::Publish => {
            let channel = require_str_header(exchange, "CamelRedis.Channel")
                .map(|s| s.to_string())
                .map_err(|_| {
                    CamelError::ProcessorError("Missing required header: CamelRedis.Channel".into())
                })?;

            let body = std::mem::replace(&mut exchange.input.body, Body::Empty)
                .try_into_text()
                .map_err(|e| {
                    CamelError::ProcessorError(format!(
                        "Message body must be convertible to Text for PUBLISH: {}",
                        e
                    ))
                })?;
            let message = body.as_text().expect("try_into_text guarantees Text");

            let receivers: i64 = conn
                .publish(&channel, message)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis PUBLISH failed: {}", e)))?;

            serde_json::json!(receivers)
        }
        RedisCommand::Subscribe | RedisCommand::Psubscribe => {
            return Err(CamelError::ProcessorError(
                "SUBSCRIBE/PSUBSCRIBE are consumer-only commands. Use from() instead of to()"
                    .into(),
            ));
        }
        _ => return Err(CamelError::ProcessorError("Not a pub/sub command".into())),
    };

    exchange.input.body = Body::Json(result);
    Ok(())
}
