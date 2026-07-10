use crate::component::XjBridgeRuntime;
use crate::config::Direction;
use camel_component_api::{Body, CamelError, Exchange};
use camel_xslt::{StylesheetId, XsltBridgeClient};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::Service;

#[derive(Clone)]
pub struct XjProducer {
    stylesheet_id: StylesheetId,
    params: Vec<(String, String)>,
    client: Arc<XsltBridgeClient>,
    runtime: Arc<XjBridgeRuntime>,
    direction: Direction,
    max_payload_bytes: Option<usize>,
    retry_count: u32,
    retry_delay_ms: u64,
}

impl XjProducer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        stylesheet_id: StylesheetId,
        params: Vec<(String, String)>,
        client: Arc<XsltBridgeClient>,
        runtime: Arc<XjBridgeRuntime>,
        direction: Direction,
        max_payload_bytes: Option<usize>,
        retry_count: u32,
        retry_delay_ms: u64,
    ) -> Self {
        Self {
            stylesheet_id,
            params,
            client,
            runtime,
            direction,
            max_payload_bytes,
            retry_count,
            retry_delay_ms,
        }
    }
}

impl Service<Exchange> for XjProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let stylesheet_id = self.stylesheet_id.clone();
        let mut params = self.params.clone();
        let client = Arc::clone(&self.client);
        let runtime = Arc::clone(&self.runtime);
        let direction = self.direction;
        let max_payload_bytes = self.max_payload_bytes;
        let retry_count = self.retry_count;
        let retry_delay_ms = self.retry_delay_ms;

        Box::pin(async move {
            let input_body = std::mem::take(&mut exchange.input.body);
            let raw = input_body
                .materialize()
                .await
                .map_err(|e| CamelError::ProcessorError(format!("XJ input body error: {e}")))?
                .to_vec();

            // Reject oversized payloads before sending to the bridge process.
            if let Some(limit) = max_payload_bytes
                && raw.len() > limit
            {
                return Err(CamelError::ProcessorError(format!(
                    "payload too large: {} bytes (limit: {limit} bytes)",
                    raw.len()
                )));
            }

            // For json2xml the input is JSON, not XML. Saxon cannot parse JSON as a SAXSource.
            // Pass the JSON as the XSLT parameter `jsonInput` and send a minimal XML document.
            let document = if direction == Direction::JsonToXml {
                let json_str = String::from_utf8(raw).map_err(|e| {
                    CamelError::ProcessorError(format!("XJ: input is not valid UTF-8: {e}"))
                })?;
                params.push(("jsonInput".to_string(), json_str));
                b"<root/>".to_vec()
            } else {
                raw
            };

            let transformed = runtime
                .transform_with_retry_configured(
                    &client,
                    &stylesheet_id,
                    document,
                    params,
                    retry_count,
                    retry_delay_ms,
                )
                .await
                .map_err(|e| CamelError::ProcessorError(e.to_string()))?;

            exchange.input.body = Body::from(transformed);
            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{Exchange, Message};
    use camel_component_api::Body;
    use camel_xslt::BridgeState;
    use tokio::sync::watch;
    use tower::ServiceExt;

    fn make_producer(max_payload_bytes: Option<usize>) -> XjProducer {
        // rustls 0.23 requires a process-level CryptoProvider; the bridge client
        // touches TLS on call(), so install ring before constructing anything.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
        let state_rx = Arc::new(state_rx);
        let client = Arc::new(XsltBridgeClient::new(Arc::clone(&state_rx)));
        let runtime = Arc::new(XjBridgeRuntime::new(
            crate::component::XjComponentConfig::default(),
            Arc::new(tokio::sync::Mutex::new(None)),
            state_tx,
            Arc::clone(&state_rx),
        ));
        XjProducer::new(
            "test-stylesheet".to_string(),
            vec![],
            client,
            runtime,
            Direction::XmlToJson,
            max_payload_bytes,
            3,
            500,
        )
    }

    #[tokio::test]
    async fn test_rejects_oversized_payload() {
        let mut producer = make_producer(Some(100));
        let big_body = "x".repeat(200).into_bytes();
        let exchange = Exchange::new(Message::new(Body::from(big_body)));

        let result = producer.ready().await.unwrap().call(exchange).await;
        let err = result.expect_err("should reject oversized payload");
        let msg = err.to_string();
        assert!(
            msg.contains("payload too large"),
            "expected payload-too-large error, got: {msg}"
        );
        assert!(
            msg.contains("200 bytes"),
            "expected actual size in message, got: {msg}"
        );
        assert!(
            msg.contains("limit: 100 bytes"),
            "expected limit in message, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_allows_payload_within_limit() {
        let mut producer = make_producer(Some(200));
        let body = "x".repeat(100).into_bytes();
        let exchange = Exchange::new(Message::new(Body::from(body)));

        // The producer will try to reach the bridge (which isn't running),
        // so we expect a bridge-related error — NOT a payload-too-large error.
        let result = producer.ready().await.unwrap().call(exchange).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            !err_msg.contains("payload too large"),
            "should NOT reject within-limit payload, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn test_no_limit_allows_any_size() {
        let mut producer = make_producer(None);
        let body = "x".repeat(10_000).into_bytes();
        let exchange = Exchange::new(Message::new(Body::from(body)));

        let result = producer.ready().await.unwrap().call(exchange).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            !err_msg.contains("payload too large"),
            "no limit means no payload rejection, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn test_empty_body_materializes_to_zero_bytes() {
        let mut producer = make_producer(None);
        // Body::Empty should materialize to 0 bytes and pass through to the
        // bridge without panicking. The bridge call will fail (no process),
        // but that's expected — the point is that the body handling path works.
        let exchange = Exchange::new(Message::new(Body::Empty));

        let result = producer.ready().await.unwrap().call(exchange).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            !err_msg.contains("payload too large"),
            "empty body should not trigger payload rejection, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn test_json_body_materializes_to_bytes() {
        let mut producer = make_producer(None);
        let json = serde_json::json!({"key": "value"});
        let exchange = Exchange::new(Message::new(Body::Json(json)));

        let result = producer.ready().await.unwrap().call(exchange).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            !err_msg.contains("payload too large"),
            "JSON body should materialize without payload rejection, got: {err_msg}"
        );
    }
}
