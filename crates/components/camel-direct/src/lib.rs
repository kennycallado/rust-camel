//! In-memory direct component for rust-camel — synchronous point-to-point
//! channel between routes sharing the same context with no serialization overhead.
//!
//! Main types: `DirectComponent`, `DirectEndpoint`, `DirectConsumer`, `DirectProducer`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};
use tower::Service;

use camel_component_api::UriConfig;
use camel_component_api::{BoxProcessor, CamelError, Exchange};
use camel_component_api::{Component, Consumer, ConsumerContext, Endpoint, ProducerContext};
use tracing::{debug, error, info};

// ---------------------------------------------------------------------------
// Shared state: maps endpoint names to senders that deliver exchanges to the
// consumer side.  Each entry holds a sender of `(Exchange, oneshot::Sender)`
// so the producer can wait for the consumer's pipeline to finish processing
// and receive the (possibly transformed) exchange back.
// ---------------------------------------------------------------------------

type DirectSender = mpsc::Sender<(Exchange, oneshot::Sender<Result<Exchange, CamelError>>)>;
type DirectRegistry = Arc<Mutex<HashMap<String, DirectSender>>>;

// ---------------------------------------------------------------------------
// DirectConfig
// ---------------------------------------------------------------------------

/// Configuration for Direct endpoints parsed from URIs.
///
/// URI format: `direct:name`
///
/// Example: `direct:foo` creates an endpoint named "foo"
#[derive(Debug, Clone, UriConfig)]
#[uri_scheme = "direct"]
#[uri_config(crate = "camel_component_api")]
pub struct DirectConfig {
    /// Endpoint name (path portion).
    pub name: String,
}

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

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let config = DirectConfig::from_uri(uri)?;
        if config.name.trim().is_empty() {
            return Err(CamelError::InvalidUri(
                "direct: endpoint name must not be empty".to_string(),
            ));
        }
        let name = config.name.clone();
        debug!(endpoint_name = %name, "direct endpoint created");
        Ok(Box::new(DirectEndpoint {
            uri: uri.to_string(),
            name: config.name,
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

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
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
            let mut reg = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            reg.insert(self.name.clone(), tx);
        }

        info!(endpoint_name = %self.name, "direct consumer started");

        // Process incoming exchanges with cooperative cancellation.
        loop {
            tokio::select! {
                _ = context.cancelled() => {
                    debug!(endpoint_name = %self.name, "direct consumer received cancellation");
                    break;
                }
                msg = rx.recv() => {
                    match msg {
                        Some((exchange, reply_tx)) => {
                            let result = context.send_and_wait(exchange).await;
                            let _ = reply_tx.send(result);
                        }
                        None => break,
                    }
                }
            }
        }

        // Cleanup: remove from registry on exit
        {
            let mut reg = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            reg.remove(&self.name);
        }

        debug!(endpoint_name = %self.name, "direct consumer stopped");

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        // Remove from registry so no new producers can send to us.
        let mut reg = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        reg.remove(&self.name);
        debug!(endpoint_name = %self.name, "direct consumer stopped");
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
        let reg = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        match reg.get(&self.name) {
            None => Poll::Ready(Err(CamelError::EndpointCreationFailed(format!(
                "direct endpoint '{}' not registered",
                self.name
            )))),
            Some(sender) if sender.is_closed() => {
                Poll::Ready(Err(CamelError::EndpointCreationFailed(format!(
                    "direct endpoint '{}' channel closed",
                    self.name
                ))))
            }
            Some(_) => Poll::Ready(Ok(())),
        }
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let name = self.name.clone();
        let registry = Arc::clone(&self.registry);

        Box::pin(async move {
            let sender = {
                let reg = registry.lock().unwrap_or_else(|e| e.into_inner());
                reg.get(&name)
                    .ok_or_else(|| {
                        let err = CamelError::EndpointCreationFailed(format!(
                            "no consumer registered for direct:{name}"
                        ));
                        error!(endpoint_name = %name, error = %err, "direct send failed");
                        err
                    })?
                    .clone()
            };

            let (reply_tx, reply_rx) = oneshot::channel();
            sender.send((exchange, reply_tx)).await.map_err(|err| {
                error!(endpoint_name = %name, error = %err, "direct send failed");
                CamelError::ChannelClosed
            })?;

            let result = reply_rx.await.map_err(|err| {
                error!(endpoint_name = %name, error = %err, "direct send failed");
                CamelError::ChannelClosed
            })?;

            debug!(endpoint_name = %name, "direct message sent");
            result
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::ExchangeEnvelope;
    use camel_component_api::Message;
    use camel_component_api::NoOpComponentContext;
    use std::task::RawWakerVTable;
    use tower::ServiceExt;

    fn noop_waker() -> std::task::Waker {
        const VTABLE: RawWakerVTable = RawWakerVTable::new(|_| RAW, |_| {}, |_| {}, |_| {});
        const RAW: std::task::RawWaker = std::task::RawWaker::new(std::ptr::null(), &VTABLE);
        unsafe { std::task::Waker::from_raw(RAW) }
    }

    fn test_producer_ctx() -> ProducerContext {
        ProducerContext::new()
    }

    #[test]
    fn test_direct_component_scheme() {
        let component = DirectComponent::new();
        assert_eq!(component.scheme(), "direct");
    }

    #[test]
    fn test_direct_component_default() {
        let component = DirectComponent::default();
        assert_eq!(component.scheme(), "direct");
    }

    #[test]
    fn test_direct_config_from_uri() {
        let config = DirectConfig::from_uri("direct:orders").unwrap();
        assert_eq!(config.name, "orders");
    }

    #[test]
    fn test_direct_endpoint_uri() {
        let component = DirectComponent::new();
        let endpoint = component
            .create_endpoint("direct:uri-check", &NoOpComponentContext)
            .unwrap();
        assert_eq!(endpoint.uri(), "direct:uri-check");
    }

    #[test]
    fn test_direct_creates_endpoint() {
        let component = DirectComponent::new();
        let endpoint = component.create_endpoint("direct:foo", &NoOpComponentContext);
        assert!(endpoint.is_ok());
    }

    #[test]
    fn test_direct_wrong_scheme() {
        let component = DirectComponent::new();
        let result = component.create_endpoint("timer:tick", &NoOpComponentContext);
        assert!(result.is_err());
    }

    #[test]
    fn test_direct_endpoint_creates_consumer() {
        let component = DirectComponent::new();
        let endpoint = component
            .create_endpoint("direct:foo", &NoOpComponentContext)
            .unwrap();
        assert!(endpoint.create_consumer().is_ok());
    }

    #[test]
    fn test_direct_endpoint_creates_producer() {
        let ctx = test_producer_ctx();
        let component = DirectComponent::new();
        let endpoint = component
            .create_endpoint("direct:foo", &NoOpComponentContext)
            .unwrap();
        assert!(endpoint.create_producer(&ctx).is_ok());
    }

    #[test]
    fn test_direct_empty_name_rejected() {
        let component = DirectComponent::new();
        match component.create_endpoint("direct:", &NoOpComponentContext) {
            Err(e) => assert!(
                e.to_string().contains("must not be empty"),
                "unexpected error: {e}"
            ),
            Ok(_) => panic!("expected error for empty name"),
        }
    }

    #[tokio::test]
    async fn test_direct_producer_no_consumer_registered() {
        let ctx = test_producer_ctx();
        let component = DirectComponent::new();
        let endpoint = component
            .create_endpoint("direct:missing", &NoOpComponentContext)
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let exchange = Exchange::new(Message::new("test"));
        let result = producer.oneshot(exchange).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_direct_producer_consumer_roundtrip() {
        let component = DirectComponent::new();

        // Create consumer endpoint and start it
        let consumer_endpoint = component
            .create_endpoint("direct:test", &NoOpComponentContext)
            .unwrap();
        let mut consumer = consumer_endpoint.create_consumer().unwrap();

        // The route channel now carries ExchangeEnvelope (request-reply support).
        let (route_tx, mut route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(route_tx, tokio_util::sync::CancellationToken::new());

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
        let ctx = test_producer_ctx();
        let producer_endpoint = component
            .create_endpoint("direct:test", &NoOpComponentContext)
            .unwrap();
        let producer = producer_endpoint.create_producer(&ctx).unwrap();

        let exchange = Exchange::new(Message::new("hello direct"));
        let result = producer.oneshot(exchange).await;

        assert!(result.is_ok());
        let reply = result.unwrap();
        assert_eq!(reply.input.body.as_text(), Some("hello direct"));
    }

    #[tokio::test]
    async fn test_direct_propagates_error_when_no_handler() {
        let component = DirectComponent::new();

        let consumer_endpoint = component
            .create_endpoint("direct:err-test", &NoOpComponentContext)
            .unwrap();
        let mut consumer = consumer_endpoint.create_consumer().unwrap();

        let (route_tx, mut route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(route_tx, tokio_util::sync::CancellationToken::new());

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

        let ctx = test_producer_ctx();
        let producer_endpoint = component
            .create_endpoint("direct:err-test", &NoOpComponentContext)
            .unwrap();
        let producer = producer_endpoint.create_producer(&ctx).unwrap();

        let exchange = Exchange::new(Message::new("test"));
        let result = producer.oneshot(exchange).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CamelError::ProcessorError(_)));
    }

    #[tokio::test]
    async fn test_direct_consumer_stop_unregisters() {
        let component = DirectComponent::new();
        let endpoint = component
            .create_endpoint("direct:cleanup", &NoOpComponentContext)
            .unwrap();

        // We need a consumer to register
        let mut consumer = endpoint.create_consumer().unwrap();

        let (route_tx, _route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(route_tx, tokio_util::sync::CancellationToken::new());

        // Start consumer in background
        let handle = tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Verify the name is registered
        {
            let reg = component.registry.lock().unwrap_or_else(|e| e.into_inner());
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
            let reg = component.registry.lock().unwrap_or_else(|e| e.into_inner());
            assert!(!reg.contains_key("cleanup"));
        }

        handle.abort();
    }

    #[tokio::test]
    async fn test_direct_consumer_respects_cancellation() {
        use tokio_util::sync::CancellationToken;

        let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
        let token = CancellationToken::new();
        let (tx, _rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(tx, token.clone());

        let mut consumer = DirectConsumer {
            name: "cancel-test".to_string(),
            registry: registry.clone(),
        };

        let handle = tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            registry
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key("cancel-test")
        );

        token.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), handle).await;
        assert!(
            result.is_ok(),
            "Consumer should have stopped after cancellation"
        );

        // After cancellation, the consumer should have cleaned up the registry
        assert!(
            !registry
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key("cancel-test")
        );
    }

    #[tokio::test]
    async fn test_direct_consumer_stop_missing_entry_is_ok() {
        let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
        let mut consumer = DirectConsumer {
            name: "never-registered".to_string(),
            registry,
        };
        let result = consumer.stop().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_poll_ready_endpoint_not_registered() {
        let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
        let producer = DirectProducer {
            name: "missing".to_string(),
            registry,
        };
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut producer = producer;
        let result = producer.poll_ready(&mut cx);
        assert!(matches!(
            result,
            Poll::Ready(Err(CamelError::EndpointCreationFailed(_)))
        ));
    }

    #[test]
    fn test_poll_ready_endpoint_registered() {
        let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
        let (tx, _rx) =
            mpsc::channel::<(Exchange, oneshot::Sender<Result<Exchange, CamelError>>)>(1);
        registry.lock().unwrap().insert("active".to_string(), tx);
        let producer = DirectProducer {
            name: "active".to_string(),
            registry,
        };
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut producer = producer;
        let result = producer.poll_ready(&mut cx);
        assert!(matches!(result, Poll::Ready(Ok(()))));
    }

    #[test]
    fn test_poll_ready_channel_closed() {
        let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) =
            mpsc::channel::<(Exchange, oneshot::Sender<Result<Exchange, CamelError>>)>(1);
        drop(rx);
        registry.lock().unwrap().insert("closed".to_string(), tx);
        let producer = DirectProducer {
            name: "closed".to_string(),
            registry,
        };
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut producer = producer;
        let result = producer.poll_ready(&mut cx);
        assert!(matches!(
            result,
            Poll::Ready(Err(CamelError::EndpointCreationFailed(_)))
        ));
    }
}
