use crate::config::RedisCommand;
use camel_api::{CamelError, Exchange, body::Body};
use redis::aio::MultiplexedConnection;

pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    let result: serde_json::Value = match cmd {
        RedisCommand::Ping => {
            let response: String = redis::cmd("PING")
                .query_async(conn)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis PING failed: {}", e)))?;

            serde_json::Value::String(response)
        }
        RedisCommand::Echo => {
            let message = std::mem::replace(&mut exchange.input.body, Body::Empty)
                .try_into_text()
                .ok()
                .and_then(|b| b.as_text().map(|s| s.to_string()))
                .unwrap_or_default();

            let response: String = redis::cmd("ECHO")
                .arg(message)
                .query_async(conn)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("Redis ECHO failed: {}", e)))?;

            serde_json::Value::String(response)
        }
        _ => return Err(CamelError::ProcessorError("Not an other command".into())),
    };

    exchange.input.body = Body::Json(result);
    Ok(())
}
