//! In-memory direct component for rust-camel — synchronous point-to-point
//! channel between routes sharing the same context with no serialization overhead.
//!
//! Main types: `DirectComponent`, `DirectEndpoint`, `DirectConsumer`, `DirectProducer`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::Service;

use camel_component_api::parse_uri;
use camel_component_api::{BoxProcessor, CamelError, Exchange};
use camel_component_api::{Component, Consumer, ConsumerContext, Endpoint, ProducerContext};
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Shared state: maps endpoint names to senders that deliver exchanges to the
// consumer side.  Each entry holds a sender of `(Exchange, oneshot::Sender)`
// so the producer can wait for the consumer's pipeline to finish processing
// and receive the (possibly transformed) exchange back.
// ---------------------------------------------------------------------------

type DirectSender = mpsc::Sender<(Exchange, oneshot::Sender<Result<Exchange, CamelError>>)>;
type DirectRegistry = Arc<Mutex<HashMap<String, DirectSender>>>;
type AcquirePermitFut =
    Pin<Box<dyn Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send>>;

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

/// Validate the direct endpoint name (the part after `direct:`).
fn validate_name(name: &str) -> Result<(), CamelError> {
    if name.trim().is_empty() {
        return Err(CamelError::InvalidUri(
            "direct: endpoint name must not be empty".to_string(),
        ));
    }
    if name.contains(char::is_whitespace) {
        return Err(CamelError::InvalidUri(
            "direct: endpoint name must not contain whitespace".to_string(),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// DirectConfig
// ---------------------------------------------------------------------------

/// Configuration for Direct endpoints parsed from URIs.
///
/// URI format: `direct:name[?timeout_ms=30000]`
///
/// Example: `direct:foo` creates an endpoint named "foo"
#[derive(Debug, Clone)]
pub struct DirectConfig {
    /// Endpoint name (path portion).
    pub name: String,
    /// Timeout in milliseconds for producer `call()`. Defaults to 30 000 ms.
    pub timeout_ms: Option<u64>,
    /// When false, the producer returns immediately if no consumer is registered.
    /// TODO(DIR-001): implement non-blocking send
    pub block: Option<bool>,
    /// When false, skip readiness error if no consumer registered.
    pub fail_if_no_consumers: Option<bool>,
    /// TODO(DIR-005): implement exchangePattern override
    pub exchange_pattern: Option<String>,
}

impl DirectConfig {
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;
        if parts.scheme != "direct" {
            return Err(CamelError::InvalidUri(format!(
                "invalid scheme '{}', expected 'direct'",
                parts.scheme
            )));
        }

        let parse_bool = |name: &str, value: &str| -> Result<bool, CamelError> {
            match value.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => Ok(true),
                "false" | "0" | "no" => Ok(false),
                _ => Err(CamelError::InvalidUri(format!(
                    "invalid value for {}: invalid boolean value: '{}'",
                    name, value
                ))),
            }
        };

        let timeout_ms = parts
            .params
            .get("timeout_ms")
            .map(|v| {
                v.parse::<u64>().map_err(|e| {
                    CamelError::InvalidUri(format!("invalid value for timeout_ms: {}", e))
                })
            })
            .transpose()?;

        let block = parts
            .params
            .get("block")
            .map(|v| parse_bool("block", v))
            .transpose()?;

        let fail_if_no_consumers = parts
            .params
            .get("fail_if_no_consumers")
            .or_else(|| parts.params.get("failIfNoConsumers"))
            .map(|v| parse_bool("fail_if_no_consumers", v))
            .transpose()?;

        let exchange_pattern = parts
            .params
            .get("exchange_pattern")
            .or_else(|| parts.params.get("exchangePattern"))
            .cloned();

        Ok(Self {
            name: parts.path,
            timeout_ms,
            block,
            fail_if_no_consumers,
            exchange_pattern,
        })
    }
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
        validate_name(&config.name)?;
        let name = config.name.clone();
        debug!(endpoint_name = %name, "direct endpoint created");
        Ok(Box::new(DirectEndpoint {
            uri: uri.to_string(),
            config,
            registry: Arc::clone(&self.registry),
        }))
    }
}

// ---------------------------------------------------------------------------
// DirectEndpoint
// ---------------------------------------------------------------------------

struct DirectEndpoint {
    uri: String,
    config: DirectConfig,
    registry: DirectRegistry,
}

impl Endpoint for DirectEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        rt: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(DirectConsumer::new(
            self.config.name.clone(),
            Arc::clone(&self.registry),
            rt,
        )))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(DirectProducer {
            name: self.config.name.clone(),
            registry: Arc::clone(&self.registry),
            config: self.config.clone(),
            semaphore: Arc::new(Semaphore::new(1)),
            pending_permit: None,
            acquire_fut: None,
            fail_if_no_consumers: self.config.fail_if_no_consumers,
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
    cancel: Option<CancellationToken>,
    handle: Option<JoinHandle<Result<(), CamelError>>>,
    runtime: Arc<dyn camel_component_api::RuntimeObservability>,
}

impl DirectConsumer {
    fn new(
        name: String,
        registry: DirectRegistry,
        runtime: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Self {
        Self {
            name,
            registry,
            cancel: None,
            handle: None,
            runtime,
        }
    }
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
            if let Some(existing) = reg.get(&self.name)
                && !existing.is_closed()
            {
                return Err(CamelError::EndpointCreationFailed(format!(
                    "direct endpoint '{}' already has a registered consumer",
                    self.name
                )));
            }
            reg.insert(self.name.clone(), tx);
        }

        let name = self.name.clone();
        let registry = Arc::clone(&self.registry);
        let cancel = context.cancel_token();
        let cancel_clone = cancel.clone();
        let route_id = context.route_id().to_owned();
        let runtime = Arc::clone(&self.runtime);

        info!(endpoint_name = %self.name, "direct consumer started");

        // Spawn the consumer loop so start() returns immediately.
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel_clone.cancelled() => {
                        debug!(endpoint_name = %name, "direct consumer received cancellation");
                        break;
                    }
                    msg = rx.recv() => {
                        match msg {
                            Some((exchange, reply_tx)) => {
                                debug!(
                                    endpoint_name = %name,
                                    exchange_id = %exchange.correlation_id,
                                    "direct consumer received exchange"
                                );
                                let result = context.send_and_wait(exchange).await;
                                if let Err(ref err) = result {
                                    // (category b′: send_and_wait returned Err on a normal-data send,
                                    // meaning the route handler did NOT absorb the failure —
                                    // see ADR-0012 "b-bridged discriminator". This emitter is the
                                    // only ERROR signal for the unhandled failure; must stay loud.)
                                    runtime
                                        .metrics()
                                        .increment_errors(&route_id, "b-prime:direct:send-and-wait");
                                    // log-policy: outside-contract
                                    error!(
                                        endpoint_name = %name,
                                        error = %err,
                                        "direct consumer pipeline error"
                                    );
                                }
                                let _ = reply_tx.send(result);
                            }
                            None => break,
                        }
                    }
                }
            }

            // Cleanup: remove from registry on exit
            {
                let mut reg = registry.lock().unwrap_or_else(|e| e.into_inner());
                reg.remove(&name);
            }

            debug!(endpoint_name = %name, "direct consumer stopped");
            Ok(())
        });

        self.cancel = Some(cancel);
        self.handle = Some(handle);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        // Cancel the consumer loop if we have a cancellation token.
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }

        // Wait for the spawned task to finish (with a 5s timeout).
        if let Some(mut h) = self.handle.take() {
            if tokio::time::timeout(Duration::from_secs(5), &mut h)
                .await
                .is_err()
            {
                h.abort();
                let _ = h.await;
                warn!(endpoint_name = %self.name, "consumer task did not stop in 5s; aborted");
                // Aborted task cannot clean up its own registry entry — do it here.
                let mut reg = self.registry.lock().unwrap_or_else(|e| e.into_inner());
                reg.remove(&self.name);
            }
        } else {
            // No join handle — just remove from registry.
            let mut reg = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            reg.remove(&self.name);
        }

        debug!(endpoint_name = %self.name, "direct consumer stopped");
        Ok(())
    }

    fn background_task_handle(&mut self) -> Option<JoinHandle<Result<(), CamelError>>> {
        // Take the handle so spawn_consumer_task can monitor it for unexpected
        // panics. The consumer loop exits cleanly via the CancellationToken, so
        // stop() drives shutdown through cancel.take(); the task removes itself
        // from the registry on exit.
        self.handle.take()
    }
}

// ---------------------------------------------------------------------------
// DirectProducer
// ---------------------------------------------------------------------------

/// The Direct producer sends an exchange to the named direct endpoint and
/// waits for a reply (synchronous in-memory call).
struct DirectProducer {
    name: String,
    registry: DirectRegistry,
    config: DirectConfig,
    semaphore: Arc<Semaphore>,
    pending_permit: Option<OwnedSemaphorePermit>,
    acquire_fut: Option<AcquirePermitFut>,
    fail_if_no_consumers: Option<bool>,
}

impl Clone for DirectProducer {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            registry: self.registry.clone(),
            config: self.config.clone(),
            semaphore: self.semaphore.clone(),
            pending_permit: None,
            acquire_fut: None,
            fail_if_no_consumers: self.fail_if_no_consumers,
        }
    }
}

impl Service<Exchange> for DirectProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // If we already hold a permit we are ready.
        if self.pending_permit.is_some() {
            return Poll::Ready(Ok(()));
        }

        // Check that the endpoint is registered.
        {
            let reg = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            match reg.get(&self.name) {
                None => {
                    if self.fail_if_no_consumers != Some(false) {
                        return Poll::Ready(Err(CamelError::EndpointCreationFailed(format!(
                            "direct endpoint '{}' not registered",
                            self.name
                        ))));
                    }
                }
                Some(sender) if sender.is_closed() => {
                    return Poll::Ready(Err(CamelError::EndpointCreationFailed(format!(
                        "direct endpoint '{}' channel closed",
                        self.name
                    ))));
                }
                Some(_) => {}
            }
        }

        // Acquire a semaphore permit (bounded concurrency).
        let fut = self
            .acquire_fut
            .get_or_insert_with(|| Box::pin(Arc::clone(&self.semaphore).acquire_owned()));
        match fut.as_mut().poll(cx) {
            Poll::Ready(Ok(permit)) => {
                self.acquire_fut = None;
                self.pending_permit = Some(permit);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(_)) => Poll::Ready(Err(CamelError::ChannelClosed)),
            Poll::Pending => Poll::Pending,
        }
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let _permit = match self.pending_permit.take() {
            Some(p) => p,
            None => {
                return Box::pin(async {
                    Err(CamelError::ProcessorError(
                        "call() invoked without poll_ready()".into(),
                    ))
                });
            }
        };

        let name = self.name.clone();
        let registry = Arc::clone(&self.registry);
        let timeout = Duration::from_millis(self.config.timeout_ms.unwrap_or(30_000));
        let exchange_id = exchange.correlation_id.clone();

        debug!(
            endpoint_name = %name,
            exchange_id = %exchange_id,
            "direct producer call entry"
        );

        Box::pin(async move {
            tokio::time::timeout(timeout, async {
                let sender = {
                    let reg = registry.lock().unwrap_or_else(|e| e.into_inner());
                    reg.get(&name)
                        .ok_or_else(|| {
                            let err = CamelError::EndpointCreationFailed(format!(
                                "no consumer registered for direct:{name}"
                            ));
                            // log-policy: handler-owned
                            // (category a: producer send failure inside the route pipeline)
                            warn!(endpoint_name = %name, error = %err, "direct send failed");
                            err
                        })?
                        .clone()
                };

                let (reply_tx, reply_rx) = oneshot::channel();
                sender.send((exchange, reply_tx)).await.map_err(|err| {
                    // log-policy: handler-owned
                    // (category a: producer send failure inside the route pipeline)
                    warn!(endpoint_name = %name, error = %err, "direct send failed");
                    CamelError::ChannelClosed
                })?;

                let result = reply_rx.await.map_err(|err| {
                    // log-policy: handler-owned
                    // (category a: producer send failure inside the route pipeline)
                    warn!(endpoint_name = %name, error = %err, "direct send failed");
                    CamelError::ChannelClosed
                })?;

                debug!(endpoint_name = %name, "direct message sent");
                result
            })
            .await
            .map_err(|_| CamelError::ProcessorError(format!("direct:{name} call timed out")))?
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use camel_api::MetricsCollector;
    use camel_component_api::HealthCheckRegistry;
    use std::time::Duration;
    fn rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        // NoOpComponentContext implements RuntimeObservability via blanket
        // impl and returns a no-op metrics collector — avoids panicking now
        // that direct consumer calls runtime.metrics() on send_and_wait errors.
        std::sync::Arc::new(NoOpComponentContext)
    }

    use super::*;
    use camel_component_api::ExchangeEnvelope;
    use camel_component_api::Message;
    use camel_component_api::NoOpComponentContext;
    use camel_component_api::RuntimeObservability;
    use std::task::RawWakerVTable;
    use tower::ServiceExt;

    // -----------------------------------------------------------------------
    // Recording metrics collector for testing increment_errors calls
    // -----------------------------------------------------------------------

    struct RecordingMetrics {
        errors: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl MetricsCollector for RecordingMetrics {
        fn record_exchange_duration(&self, _: &str, _: Duration) {}
        fn increment_errors(&self, route_id: &str, error_type: &str) {
            self.errors
                .lock()
                .unwrap()
                .push((route_id.to_string(), error_type.to_string()));
        }
        fn increment_exchanges(&self, _: &str) {}
        fn set_queue_depth(&self, _: &str, _: usize) {}
        fn record_circuit_breaker_change(&self, _: &str, _: &str, _: &str) {}
    }

    struct RecordingRuntime {
        metrics_collector: Arc<RecordingMetrics>,
    }

    impl RecordingRuntime {
        fn new(errors: Arc<Mutex<Vec<(String, String)>>>) -> Self {
            Self {
                metrics_collector: Arc::new(RecordingMetrics { errors }),
            }
        }
    }

    impl RuntimeObservability for RecordingRuntime {
        fn metrics(&self) -> Arc<dyn MetricsCollector> {
            self.metrics_collector.clone() as Arc<dyn MetricsCollector>
        }
        fn health(&self) -> Arc<dyn HealthCheckRegistry> {
            panic!("RecordingRuntime::health not used in this test")
        }
    }

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
        assert!(endpoint.create_consumer(rt()).is_ok());
    }

    #[test]
    fn test_direct_endpoint_creates_producer() {
        let ctx = test_producer_ctx();
        let component = DirectComponent::new();
        let endpoint = component
            .create_endpoint("direct:foo", &NoOpComponentContext)
            .unwrap();
        assert!(endpoint.create_producer(rt(), &ctx).is_ok());
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
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::new("test"));
        let result = producer.oneshot(exchange).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_direct_duplicate_consumer_returns_error() {
        let component = DirectComponent::new();
        let endpoint = component
            .create_endpoint("direct:dup", &NoOpComponentContext)
            .unwrap();

        let mut consumer_a = endpoint.create_consumer(rt()).unwrap();
        let mut consumer_b = endpoint.create_consumer(rt()).unwrap();

        let (route_tx_a, _route_rx_a) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_a = ConsumerContext::new(
            route_tx_a,
            tokio_util::sync::CancellationToken::new(),
            "direct-test-route-a".to_string(),
        );
        consumer_a.start(ctx_a).await.unwrap();

        let (route_tx_b, _route_rx_b) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx_b = ConsumerContext::new(
            route_tx_b,
            tokio_util::sync::CancellationToken::new(),
            "direct-test-route-b".to_string(),
        );
        let result = consumer_b.start(ctx_b).await;

        assert!(matches!(
            result,
            Err(CamelError::EndpointCreationFailed(msg))
                if msg.contains("already has a registered consumer")
        ));

        consumer_a.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_direct_producer_consumer_roundtrip() {
        let component = DirectComponent::new();

        // Create consumer endpoint and start it
        let consumer_endpoint = component
            .create_endpoint("direct:test", &NoOpComponentContext)
            .unwrap();
        let mut consumer = consumer_endpoint.create_consumer(rt()).unwrap();

        // The route channel now carries ExchangeEnvelope (request-reply support).
        let (route_tx, mut route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(
            route_tx,
            tokio_util::sync::CancellationToken::new(),
            "direct-test-route".to_string(),
        );

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
        let producer = producer_endpoint.create_producer(rt(), &ctx).unwrap();

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
        let mut consumer = consumer_endpoint.create_consumer(rt()).unwrap();

        let (route_tx, mut route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(
            route_tx,
            tokio_util::sync::CancellationToken::new(),
            "direct-test-route".to_string(),
        );

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
        let producer = producer_endpoint.create_producer(rt(), &ctx).unwrap();

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
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let (route_tx, _route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(
            route_tx,
            tokio_util::sync::CancellationToken::new(),
            "direct-test-route".to_string(),
        );

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
            cancel: None,
            handle: None,
            runtime: rt(),
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
        let ctx = ConsumerContext::new(tx, token.clone(), "direct-test-route".to_string());

        let mut consumer = DirectConsumer {
            name: "cancel-test".to_string(),
            registry: registry.clone(),
            cancel: None,
            handle: None,
            runtime: rt(),
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
        // start() now spawns the loop internally and returns immediately,
        // so the outer handle completes right away. Give the inner task time
        // to react to the cancellation and clean up the registry.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // After cancellation, the consumer should have cleaned up the registry
        assert!(
            !registry
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key("cancel-test")
        );

        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_direct_consumer_stop_missing_entry_is_ok() {
        let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
        let mut consumer = DirectConsumer {
            name: "never-registered".to_string(),
            registry,
            cancel: None,
            handle: None,
            runtime: rt(),
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
            config: DirectConfig {
                name: "missing".to_string(),
                timeout_ms: None,
                block: None,
                fail_if_no_consumers: None,
                exchange_pattern: None,
            },
            semaphore: Arc::new(Semaphore::new(1)),
            pending_permit: None,
            acquire_fut: None,
            fail_if_no_consumers: None,
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
            config: DirectConfig {
                name: "active".to_string(),
                timeout_ms: None,
                block: None,
                fail_if_no_consumers: None,
                exchange_pattern: None,
            },
            semaphore: Arc::new(Semaphore::new(1)),
            pending_permit: None,
            acquire_fut: None,
            fail_if_no_consumers: None,
        };
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut producer = producer;
        let result = producer.poll_ready(&mut cx);
        assert!(matches!(result, Poll::Ready(Ok(()))));
    }

    #[test]
    fn test_poll_ready_allows_missing_consumer_when_fail_if_no_consumers_false() {
        let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
        let producer = DirectProducer {
            name: "missing-ok".to_string(),
            registry,
            config: DirectConfig {
                name: "missing-ok".to_string(),
                timeout_ms: None,
                block: None,
                fail_if_no_consumers: Some(false),
                exchange_pattern: None,
            },
            semaphore: Arc::new(Semaphore::new(1)),
            pending_permit: None,
            acquire_fut: None,
            fail_if_no_consumers: Some(false),
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
            config: DirectConfig {
                name: "closed".to_string(),
                timeout_ms: None,
                block: None,
                fail_if_no_consumers: None,
                exchange_pattern: None,
            },
            semaphore: Arc::new(Semaphore::new(1)),
            pending_permit: None,
            acquire_fut: None,
            fail_if_no_consumers: None,
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

    #[tokio::test]
    async fn test_direct_stop_cancels_loop() {
        use tokio_util::sync::CancellationToken;

        let component = DirectComponent::new();
        let endpoint = component
            .create_endpoint("direct:stop-test", &NoOpComponentContext)
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let token = CancellationToken::new();
        let (route_tx, _route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(route_tx, token.clone(), "direct-test-route".to_string());

        // start() blocks — it should run the consumer loop on a JoinHandle
        // and return immediately. But currently start() IS the loop.
        // We test stop() cancels the loop.
        let handle = tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            component
                .registry
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key("stop-test")
        );

        // Create a new consumer just for stop
        let mut stop_consumer = DirectConsumer {
            name: "stop-test".to_string(),
            registry: Arc::clone(&component.registry),
            cancel: Some(token.clone()),
            handle: None,
            runtime: rt(),
        };
        stop_consumer.stop().await.unwrap();

        // The consumer loop should finish within 2s after stop
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "Consumer loop did not stop within 2s");

        // Registry should be cleaned up
        assert!(
            !component
                .registry
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key("stop-test")
        );
    }

    #[tokio::test]
    async fn test_direct_producer_timeout() {
        let component = DirectComponent::new();
        let endpoint = component
            .create_endpoint("direct:timeout-test", &NoOpComponentContext)
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        // Consumer that never replies (simulates a stuck pipeline)
        let (route_tx, mut route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let token = tokio_util::sync::CancellationToken::new();
        let ctx = ConsumerContext::new(route_tx, token.clone(), "direct-test-route".to_string());
        tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        // Drain envelopes but hold onto reply_tx so the producer never gets a reply
        tokio::spawn(async move {
            let mut held_reply: Vec<oneshot::Sender<Result<Exchange, CamelError>>> = Vec::new();
            while let Some(envelope) = route_rx.recv().await {
                held_reply.push(envelope.reply_tx.unwrap());
            }
            drop(held_reply);
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Create producer with a short timeout
        let _ = test_producer_ctx();
        let _producer_endpoint = component
            .create_endpoint("direct:timeout-test", &NoOpComponentContext)
            .unwrap();
        let producer = DirectProducer {
            name: "timeout-test".to_string(),
            registry: Arc::clone(&component.registry),
            config: DirectConfig {
                name: "timeout-test".to_string(),
                timeout_ms: Some(100), // 100ms timeout
                block: None,
                fail_if_no_consumers: None,
                exchange_pattern: None,
            },
            semaphore: Arc::new(Semaphore::new(1)),
            pending_permit: None,
            acquire_fut: None,
            fail_if_no_consumers: None,
        };

        let exchange = Exchange::new(Message::new("test"));
        let mut svc = producer;
        let _ = svc.poll_ready(&mut Context::from_waker(&noop_waker()));
        let result = svc.call(exchange).await;
        assert!(result.is_err(), "Expected timeout error");
        assert!(
            result.unwrap_err().to_string().contains("timed out"),
            "Expected timeout message"
        );

        token.cancel();
    }

    #[tokio::test]
    async fn test_send_and_wait_error_increments_errors_metric() {
        let errors: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let runtime = Arc::new(RecordingRuntime::new(Arc::clone(&errors)));

        let component = DirectComponent::new();
        let endpoint = component
            .create_endpoint("direct:metrics-error", &NoOpComponentContext)
            .unwrap();
        let mut consumer = endpoint.create_consumer(runtime).unwrap();

        // Route that returns Err — simulates unhandled pipeline failure
        let cancel = tokio_util::sync::CancellationToken::new();
        let (route_tx, mut route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(route_tx, cancel.clone(), "test-route-id".to_string());

        tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Route pipeline: reply with Err for every incoming exchange
        tokio::spawn(async move {
            while let Some(envelope) = route_rx.recv().await {
                if let Some(tx) = envelope.reply_tx {
                    let _ = tx.send(Err(CamelError::ProcessorError(
                        "pipeline failure".to_string(),
                    )));
                }
            }
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send an exchange via the producer so the consumer's send_and_wait
        // returns Err, triggering the metrics call.
        let ctx = test_producer_ctx();
        let producer_endpoint = component
            .create_endpoint("direct:metrics-error", &NoOpComponentContext)
            .unwrap();
        let producer = producer_endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::new("test"));
        // The producer oneshot wraps the call — it returns the Err from
        // send_and_wait back to us.
        let _result = producer.oneshot(exchange).await;

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        cancel.cancel();

        // Verify MetricsCollector::increment_errors was called
        let recorded = errors.lock().unwrap();
        assert_eq!(
            recorded.len(),
            1,
            "expected 1 increment_errors call, got {}: {:?}",
            recorded.len(),
            *recorded
        );
        assert_eq!(recorded[0].0, "test-route-id");
        assert_eq!(recorded[0].1, "b-prime:direct:send-and-wait");
    }

    #[test]
    fn test_empty_endpoint_name_rejected() {
        let result = DirectConfig::from_uri("direct:");
        // from_uri may parse empty name — validate catches it
        if let Ok(config) = result {
            assert!(
                validate_name(&config.name).is_err(),
                "expected validation error for empty name"
            );
        }
        // Also verify via Component (the main entry point)
        let component = DirectComponent::new();
        let result = component.create_endpoint("direct:", &NoOpComponentContext);
        assert!(result.is_err(), "empty endpoint name must be rejected");
    }

    #[test]
    fn test_whitespace_endpoint_name_rejected() {
        let result = DirectConfig::from_uri("direct:my endpoint");
        if let Ok(config) = result {
            assert!(
                validate_name(&config.name).is_err(),
                "expected validation error for whitespace in name"
            );
        }
        let component = DirectComponent::new();
        let result = component.create_endpoint("direct:my endpoint", &NoOpComponentContext);
        assert!(result.is_err(), "whitespace endpoint name must be rejected");
    }

    #[test]
    fn test_valid_endpoint_name_accepted() {
        let component = DirectComponent::new();
        let result = component.create_endpoint("direct:my-endpoint", &NoOpComponentContext);
        assert!(result.is_ok(), "valid endpoint name should be accepted");
    }
}
