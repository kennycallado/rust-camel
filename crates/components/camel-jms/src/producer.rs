use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use camel_component_api::{Body, CamelError, Exchange, Value};
use tokio::sync::Semaphore;
use tonic::transport::Channel;
use tower::Service;
use tracing::debug;

use crate::component::BRIDGE_TRANSPORT_ERROR_PREFIX;
use crate::config::{DestinationType, JmsEndpointConfig};
use crate::headers::extract_send_headers;
use crate::proto::{SendRequest, bridge_service_client::BridgeServiceClient};

/// Default concurrency limit for JMS producer backpressure.
const DEFAULT_CONCURRENCY_LIMIT: usize = 128;

// JMS-004 [resource-leak]: Exchange is NOT cloned. It is passed by value (moved)
// through LazyJmsProducer::call → JmsProducer::call → async block → returned.
// No clone site exists in this crate.

// Clone shares the semaphore Arc so every clone observes one backpressure
// budget; remaining fields clone by value.
pub struct JmsProducer {
    channel: Channel,
    endpoint_config: JmsEndpointConfig,
    /// Semaphore bounding concurrent in-flight sends.
    semaphore: Arc<Semaphore>,
}

impl Clone for JmsProducer {
    fn clone(&self) -> Self {
        Self {
            channel: self.channel.clone(),
            endpoint_config: self.endpoint_config.clone(),
            semaphore: Arc::clone(&self.semaphore),
        }
    }
}

impl JmsProducer {
    pub fn new(channel: Channel, endpoint_config: JmsEndpointConfig) -> Self {
        Self::with_concurrency(channel, endpoint_config, DEFAULT_CONCURRENCY_LIMIT)
    }

    /// Create a producer with an explicit concurrency limit for backpressure.
    pub fn with_concurrency(
        channel: Channel,
        endpoint_config: JmsEndpointConfig,
        concurrency_limit: usize,
    ) -> Self {
        Self {
            channel,
            endpoint_config,
            semaphore: Arc::new(Semaphore::new(concurrency_limit)),
        }
    }

    fn body_to_bytes(body: &Body) -> Result<Vec<u8>, CamelError> {
        match body {
            Body::Text(s) => Ok(s.as_bytes().to_vec()),
            Body::Xml(s) => Ok(s.as_bytes().to_vec()),
            Body::Bytes(b) => Ok(b.to_vec()),
            Body::Json(v) => serde_json::to_vec(v)
                .map_err(|e| CamelError::ProcessorError(format!("JSON error: {e}"))),
            Body::Empty => Ok(vec![]),
            Body::Stream(_) => Err(CamelError::ProcessorError(
                "Body::Stream must be materialized before sending to JMS".to_string(),
            )),
            _ => Err(CamelError::ProcessorError(
                "unsupported body type for JMS producer".to_string(),
            )),
        }
    }

    fn content_type(exchange: &Exchange) -> String {
        // Explicit header always wins
        if let Some(ct) = exchange
            .input
            .header("Content-Type")
            .and_then(|v| v.as_str().map(str::to_string))
        {
            return ct;
        }
        // Infer from body type so Java creates the right message type
        match &exchange.input.body {
            Body::Text(_) => "text/plain".to_string(),
            Body::Xml(_) => "text/xml".to_string(),
            Body::Json(_) => "application/json".to_string(),
            _ => String::new(),
        }
    }

    fn destination(endpoint_config: &JmsEndpointConfig) -> String {
        format!(
            "{}:{}",
            match endpoint_config.destination_type {
                DestinationType::Queue => "queue",
                DestinationType::Topic => "topic",
            },
            endpoint_config.destination_name
        )
    }
}

impl Service<Exchange> for JmsProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Permits are NOT acquired here: a permit reserved in poll_ready is
        // held across the poll_ready/call boundary, wedging the semaphore
        // when a wrapping Service re-readies a clone (tower contract:
        // call() may only reserve resources for its own future).
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let channel = self.channel.clone();
        let destination = Self::destination(&self.endpoint_config);
        let semaphore = Arc::clone(&self.semaphore);

        // JMS-018: gate header extraction behind config flag
        let map_headers = self.endpoint_config.map_jms_headers;

        // JMS-013: QoS options
        let time_to_live = self.endpoint_config.time_to_live;
        let priority = self.endpoint_config.priority;
        let persistent_delivery = self.endpoint_config.persistent_delivery;

        Box::pin(async move {
            // Acquire the send permit inside call()'s future and hold it for
            // the duration of the send. A closed semaphore surfaces
            // ConsumerStopping here rather than in poll_ready.
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_| CamelError::ConsumerStopping)?;

            let body = Self::body_to_bytes(&exchange.input.body)?;
            let headers = if map_headers {
                extract_send_headers(&exchange)
            } else {
                Default::default()
            };
            let content_type = Self::content_type(&exchange);

            // JMS-013: apply QoS headers when configured
            // TODO(JMS-013): pass TTL, priority, and delivery mode to bridge SendRequest
            // once the proto supports them. For now, set them as exchange metadata.
            if let Some(ttl) = time_to_live {
                exchange
                    .input
                    .set_header("JMSExpiration", Value::String(ttl.to_string()));
            }
            if let Some(p) = priority {
                exchange
                    .input
                    .set_header("JMSPriority", Value::String(p.to_string()));
            }
            exchange.input.set_header(
                "JMSDeliveryMode",
                Value::String(
                    if persistent_delivery {
                        "PERSISTENT"
                    } else {
                        "NON_PERSISTENT"
                    }
                    .to_string(),
                ),
            );

            let mut client = BridgeServiceClient::new(channel);
            let request = SendRequest {
                destination,
                body,
                headers,
                content_type,
            };

            let response = client
                .send(request)
                .await
                .map_err(|s| {
                    CamelError::ProcessorError(format!(
                        "{BRIDGE_TRANSPORT_ERROR_PREFIX}send error: {s}"
                    ))
                })?
                .into_inner();

            debug!(message_id = %response.message_id, "JMS message sent");
            if map_headers {
                exchange
                    .input
                    .set_header("JMSMessageID", Value::String(response.message_id));
            }
            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::StreamBody;
    use futures::stream;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn body_text_to_bytes() {
        let b = JmsProducer::body_to_bytes(&Body::Text("hello".to_string())).unwrap();
        assert_eq!(b, b"hello");
    }

    #[test]
    fn body_empty_to_bytes() {
        let b = JmsProducer::body_to_bytes(&Body::Empty).unwrap();
        assert!(b.is_empty());
    }

    #[test]
    fn body_stream_returns_error() {
        let stream = stream::empty::<Result<bytes::Bytes, camel_component_api::CamelError>>();
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: Default::default(),
        });
        let err = JmsProducer::body_to_bytes(&body).unwrap_err();
        assert!(err.to_string().contains("Stream"));
    }

    #[test]
    fn content_type_inferred_from_text_body() {
        // When body is Body::Text and no Content-Type header is set,
        // the content type should be inferred as "text/plain" so Java
        // creates a TextMessage instead of a BytesMessage.
        let mut ex = Exchange::default();
        ex.input.body = Body::Text("hello".to_string());
        assert_eq!(JmsProducer::content_type(&ex), "text/plain");
    }

    #[test]
    fn content_type_header_overrides_inferred() {
        // Explicit Content-Type header should always take precedence
        let mut ex = Exchange::default();
        ex.input.body = Body::Text("hello".to_string());
        ex.input
            .set_header("Content-Type", Value::String("text/xml".to_string()));
        assert_eq!(JmsProducer::content_type(&ex), "text/xml");
    }

    #[test]
    fn content_type_inferred_from_xml_body() {
        let mut ex = Exchange::default();
        ex.input.body = Body::Xml("<root/>".to_string());
        assert_eq!(JmsProducer::content_type(&ex), "text/xml");
    }

    #[test]
    fn content_type_inferred_from_json_body() {
        let mut ex = Exchange::default();
        ex.input.body = Body::Json(serde_json::json!({"key": "val"}));
        assert_eq!(JmsProducer::content_type(&ex), "application/json");
    }

    #[test]
    fn content_type_empty_for_bytes_without_header() {
        // Body::Bytes without a header → no inference, stays empty
        let mut ex = Exchange::default();
        ex.input.body = Body::from(b"raw".to_vec());
        assert_eq!(JmsProducer::content_type(&ex), "");
    }

    #[test]
    fn destination_queue_format() {
        let endpoint_config = JmsEndpointConfig::from_uri("jms:queue:orders").unwrap();
        assert_eq!(JmsProducer::destination(&endpoint_config), "queue:orders");
    }

    #[test]
    fn destination_topic_format() {
        let endpoint_config = JmsEndpointConfig::from_uri("jms:topic:events").unwrap();
        assert_eq!(JmsProducer::destination(&endpoint_config), "topic:events");
    }

    #[tokio::test]
    async fn poll_ready_returns_ok_unconditionally() {
        use futures::task::noop_waker_ref;
        use std::task::{Context, Poll};

        let config = JmsEndpointConfig::from_uri("jms:queue:orders").unwrap();
        let channel: tonic::transport::Channel =
            tonic::transport::Endpoint::from_static("http://127.0.0.1:1").connect_lazy();
        let mut producer = JmsProducer::with_concurrency(channel, config, 1);

        let mut cx = Context::from_waker(noop_waker_ref());
        assert!(
            matches!(producer.poll_ready(&mut cx), Poll::Ready(Ok(()))),
            "fresh producer with an open semaphore must be ready"
        );
        assert_eq!(
            producer.semaphore.available_permits(),
            1,
            "poll_ready must not consume a call permit"
        );
    }

    #[tokio::test]
    async fn call_closed_semaphore_returns_error() {
        let config = JmsEndpointConfig::from_uri("jms:queue:orders").unwrap();
        let channel: tonic::transport::Channel =
            tonic::transport::Endpoint::from_static("http://127.0.0.1:1").connect_lazy();
        let mut producer = JmsProducer::with_concurrency(channel, config, 1);
        producer.semaphore.close();

        let result = producer.call(Exchange::default()).await;
        assert!(
            matches!(result, Err(CamelError::ConsumerStopping)),
            "closed semaphore must surface ConsumerStopping from call"
        );
    }

    #[tokio::test]
    async fn call_blocks_on_semaphore_until_release() {
        use futures::task::noop_waker_ref;
        use std::task::Context;

        let config = JmsEndpointConfig::from_uri("jms:queue:orders").unwrap();
        let channel: tonic::transport::Channel =
            tonic::transport::Endpoint::from_static("http://127.0.0.1:1").connect_lazy();
        let mut producer = JmsProducer::with_concurrency(channel, config, 1);

        // Hold the sole permit externally: the call future must pend on
        // acquisition until it is released.
        let held = Arc::clone(&producer.semaphore)
            .try_acquire_owned()
            .expect("sole permit must be free initially");

        let mut fut = producer.call(Exchange::default());
        let mut cx = Context::from_waker(noop_waker_ref());
        assert!(
            fut.as_mut().poll(&mut cx).is_pending(),
            "call must pend while the sole permit is held elsewhere"
        );

        drop(held);
        // The next poll completes acquisition and proceeds into header
        // setup and the bridge send (which pends on the absent broker).
        // The permit count dropping back to zero proves the future got
        // past acquisition; the send result itself is not under test.
        assert!(fut.as_mut().poll(&mut cx).is_pending());
        assert_eq!(
            producer.semaphore.available_permits(),
            0,
            "call future must now hold the permit (past acquisition)"
        );
    }
}
