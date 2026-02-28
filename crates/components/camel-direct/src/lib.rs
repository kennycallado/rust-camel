use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use tokio::sync::{Mutex, mpsc, oneshot};
use tower::Service;

use camel_api::{BoxProcessor, CamelError, Exchange};
use camel_component::{Component, Consumer, ConsumerContext, Endpoint};
use camel_endpoint::parse_uri;

// ---------------------------------------------------------------------------
// Shared state: maps endpoint names to senders that deliver exchanges to the
// consumer side.  Each entry holds a sender of `(Exchange, oneshot::Sender)`
// so the producer can wait for the consumer's pipeline to finish processing
// and receive the (possibly transformed) exchange back.
// ---------------------------------------------------------------------------

type DirectSender = mpsc::Sender<(Exchange, oneshot::Sender<Result<Exchange, CamelError>>)>;
type DirectRegistry = Arc<Mutex<HashMap<String, DirectSender>>>;

// ---------------------------------------------------------------------------
// DirectComponent
// ---------------------------------------------------------------------------

/// The Direct component provides in-memory synchronous communication between
/// routes.
///
/// URI format: `direct:name`
///
/// A producer sending to `direct:foo` will block until the consumer on
/// `direct:foo` has finished processing the exchange.
pub struct DirectComponent {
    registry: DirectRegistry,
}

impl DirectComponent {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for DirectComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for DirectComponent {
    fn scheme(&self) -> &str {
        "direct"
    }

    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
        let parts = parse_uri(uri)?;
        if parts.scheme != "direct" {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme 'direct', got '{}'",
                parts.scheme
            )));
        }

        Ok(Box::new(DirectEndpoint {
            uri: uri.to_string(),
            name: parts.path,
            registry: Arc::clone(&self.registry),
        }))
    }
}

// ---------------------------------------------------------------------------
// DirectEndpoint
// ---------------------------------------------------------------------------

struct DirectEndpoint {
    uri: String,
    name: String,
    registry: DirectRegistry,
}

impl Endpoint for DirectEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(DirectConsumer {
            name: self.name.clone(),
            registry: Arc::clone(&self.registry),
        }))
    }

    fn create_producer(&self) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(DirectProducer {
            name: self.name.clone(),
            registry: Arc::clone(&self.registry),
        }))
    }
}

// ---------------------------------------------------------------------------
// DirectConsumer
// ---------------------------------------------------------------------------

/// The Direct consumer registers itself in the shared registry and forwards
/// incoming exchanges to the route pipeline via `ConsumerContext`.
struct DirectConsumer {
    name: String,
    registry: DirectRegistry,
}

#[async_trait]
impl Consumer for DirectConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        // Create a channel for producers to send exchanges to this consumer.
        let (tx, mut rx) =
            mpsc::channel::<(Exchange, oneshot::Sender<Result<Exchange, CamelError>>)>(32);

        // Register ourselves so producers can find us.
        {
            let mut reg = self.registry.lock().await;
            reg.insert(self.name.clone(), tx);
        }

        // Process incoming exchanges.
        while let Some((exchange, reply_tx)) = rx.recv().await {
            // Send the exchange into the route pipeline and wait for the result.
            // This gives us the pipeline result (Ok or Err) to relay back to the producer.
            let result = context.send_and_wait(exchange).await;
            let _ = reply_tx.send(result);
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        // Remove from registry so no new producers can send to us.
        let mut reg = self.registry.lock().await;
        reg.remove(&self.name);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// DirectProducer
// ---------------------------------------------------------------------------

/// The Direct producer sends an exchange to the named direct endpoint and
/// waits for a reply (synchronous in-memory call).
#[derive(Clone)]
struct DirectProducer {
    name: String,
    registry: DirectRegistry,
}

impl Service<Exchange> for DirectProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let name = self.name.clone();
        let registry = Arc::clone(&self.registry);

        Box::pin(async move {
            let reg = registry.lock().await;
            let sender = reg.get(&name).ok_or_else(|| {
                CamelError::EndpointCreationFailed(format!(
                    "no consumer registered for direct:{name}"
                ))
            })?;

            let (reply_tx, reply_rx) = oneshot::channel();
            sender
                .send((exchange, reply_tx))
                .await
                .map_err(|_| CamelError::ChannelClosed)?;

            // Drop the lock before awaiting the reply to avoid deadlocks.
            drop(reg);

            // Propagate Ok or Err from the subroute pipeline.
            reply_rx.await.map_err(|_| CamelError::ChannelClosed)?
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::Message;
    use camel_component::ExchangeEnvelope;
    use tower::ServiceExt;

    #[test]
    fn test_direct_component_scheme() {
        let component = DirectComponent::new();
        assert_eq!(component.scheme(), "direct");
    }

    #[test]
    fn test_direct_creates_endpoint() {
        let component = DirectComponent::new();
        let endpoint = component.create_endpoint("direct:foo");
        assert!(endpoint.is_ok());
    }

    #[test]
    fn test_direct_wrong_scheme() {
        let component = DirectComponent::new();
        let result = component.create_endpoint("timer:tick");
        assert!(result.is_err());
    }

    #[test]
    fn test_direct_endpoint_creates_consumer() {
        let component = DirectComponent::new();
        let endpoint = component.create_endpoint("direct:foo").unwrap();
        assert!(endpoint.create_consumer().is_ok());
    }

    #[test]
    fn test_direct_endpoint_creates_producer() {
        let component = DirectComponent::new();
        let endpoint = component.create_endpoint("direct:foo").unwrap();
        assert!(endpoint.create_producer().is_ok());
    }

    #[tokio::test]
    async fn test_direct_producer_no_consumer_registered() {
        let component = DirectComponent::new();
        let endpoint = component.create_endpoint("direct:missing").unwrap();
        let producer = endpoint.create_producer().unwrap();

        let exchange = Exchange::new(Message::new("test"));
        let result = producer.oneshot(exchange).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_direct_producer_consumer_roundtrip() {
        let component = DirectComponent::new();

        // Create consumer endpoint and start it
        let consumer_endpoint = component.create_endpoint("direct:test").unwrap();
        let mut consumer = consumer_endpoint.create_consumer().unwrap();

        // The route channel now carries ExchangeEnvelope (request-reply support).
        let (route_tx, mut route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(route_tx);

        // Start the consumer in a background task
        tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        // Give the consumer a moment to register
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Spawn a pipeline simulator that reads envelopes and replies Ok.
        tokio::spawn(async move {
            while let Some(envelope) = route_rx.recv().await {
                let ExchangeEnvelope { exchange, reply_tx } = envelope;
                if let Some(tx) = reply_tx {
                    let _ = tx.send(Ok(exchange));
                }
            }
        });

        // Now send an exchange via the producer
        let producer_endpoint = component.create_endpoint("direct:test").unwrap();
        let producer = producer_endpoint.create_producer().unwrap();

        let exchange = Exchange::new(Message::new("hello direct"));
        let result = producer.oneshot(exchange).await;

        assert!(result.is_ok());
        let reply = result.unwrap();
        assert_eq!(reply.input.body.as_text(), Some("hello direct"));
    }

    #[tokio::test]
    async fn test_direct_propagates_error_when_no_handler() {
        let component = DirectComponent::new();

        let consumer_endpoint = component.create_endpoint("direct:err-test").unwrap();
        let mut consumer = consumer_endpoint.create_consumer().unwrap();

        let (route_tx, mut route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(route_tx);

        tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Pipeline simulator that replies with Err (simulates no error handler).
        tokio::spawn(async move {
            while let Some(envelope) = route_rx.recv().await {
                if let Some(tx) = envelope.reply_tx {
                    let _ = tx.send(Err(CamelError::ProcessorError("subroute failed".into())));
                }
            }
        });

        let producer_endpoint = component.create_endpoint("direct:err-test").unwrap();
        let producer = producer_endpoint.create_producer().unwrap();

        let exchange = Exchange::new(Message::new("test"));
        let result = producer.oneshot(exchange).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CamelError::ProcessorError(_)));
    }

    #[tokio::test]
    async fn test_direct_consumer_stop_unregisters() {
        let component = DirectComponent::new();
        let endpoint = component.create_endpoint("direct:cleanup").unwrap();

        // We need a consumer to register
        let mut consumer = endpoint.create_consumer().unwrap();

        let (route_tx, _route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(route_tx);

        // Start consumer in background
        let handle = tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Verify the name is registered
        {
            let reg = component.registry.lock().await;
            assert!(reg.contains_key("cleanup"));
        }

        // Create a new consumer just to call stop (stop removes from registry)
        let mut stop_consumer = DirectConsumer {
            name: "cleanup".to_string(),
            registry: Arc::clone(&component.registry),
        };
        stop_consumer.stop().await.unwrap();

        // Verify removed from registry
        {
            let reg = component.registry.lock().await;
            assert!(!reg.contains_key("cleanup"));
        }

        handle.abort();
    }
}
