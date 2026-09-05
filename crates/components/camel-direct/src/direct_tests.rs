//! Tests for the camel-direct component.
//! Sibling file via `#[path]` so the production module stays scannable;
//! still in-crate for private-field access.

use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;

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
use camel_component_api::StartupSignal;
use std::task::RawWakerVTable;
use tokio::sync::Notify;
use tokio::sync::mpsc;
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

/// Drive one producer dispatch to completion: readiness poll, then a
/// single `call` (the tower `ServiceExt` one-shot drive pattern).
/// Generic over the service so both `BoxProcessor` and the bare
/// `DirectProducer` built by `direct_producer` can be driven.
async fn dispatch<S>(mut producer: S, exchange: Exchange) -> Result<Exchange, CamelError>
where
    S: tower::Service<Exchange, Response = Exchange, Error = CamelError>,
{
    producer.ready().await?.call(exchange).await
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
fn rejects_block_param() {
    let result = DirectConfig::from_uri("direct:foo?block=true");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("block is not supported"),
        "expected block rejection"
    );
}

#[test]
fn rejects_exchange_pattern_snake_case() {
    let result = DirectConfig::from_uri("direct:foo?exchange_pattern=InOnly");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("exchange_pattern is not supported"),
        "expected exchange_pattern rejection"
    );
}

#[test]
fn rejects_exchange_pattern_camel_case() {
    let result = DirectConfig::from_uri("direct:foo?exchangePattern=InOnly");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("exchange_pattern is not supported"),
        "expected exchangePattern rejection"
    );
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
    let result = dispatch(producer, exchange).await;
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
    let token_a = tokio_util::sync::CancellationToken::new();
    let ctx_a = ConsumerContext::new(
        route_tx_a,
        token_a.clone(),
        "direct-test-route-a".to_string(),
    );
    let handle_a = tokio::spawn(async move {
        consumer_a.start(ctx_a).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

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

    token_a.cancel();
    handle_a.await.unwrap();
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
    let result = dispatch(producer, exchange).await;

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
    let result = dispatch(producer, exchange).await;
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
    };
    let result = consumer.stop().await;
    assert!(result.is_ok());
}

fn direct_producer(
    name: &str,
    registry: DirectRegistry,
    fail_if_no_consumers: Option<bool>,
    timeout_ms: Option<u64>,
) -> DirectProducer {
    DirectProducer {
        name: name.to_string(),
        registry,
        config: DirectConfig {
            name: name.to_string(),
            timeout_ms,
            fail_if_no_consumers,
        },
        semaphore: Arc::new(Semaphore::new(1)),
        fail_if_no_consumers,
        runtime: rt(),
    }
}

/// Build a `DirectEntry` value for registry seeding in tests: a context
/// backed by its own route channel plus the given liveness flag.
fn test_entry(closed: bool, route_id: &str) -> DirectEntry {
    let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(1);
    DirectEntry {
        ctx: ConsumerContext::new(tx, CancellationToken::new(), route_id.to_string()),
        closed: Arc::new(AtomicBool::new(closed)),
        dispatcher: None,
    }
}

#[test]
fn poll_ready_absent_consumer_fails_fast() {
    let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
    let mut producer = direct_producer("missing", registry, None, None);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let result = producer.poll_ready(&mut cx);
    assert!(matches!(
        result,
        Poll::Ready(Err(CamelError::EndpointCreationFailed(_)))
    ));
}

#[test]
fn poll_ready_live_consumer_ok_without_permit() {
    let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
    registry
        .lock()
        .unwrap()
        .insert("active".to_string(), test_entry(false, "active-route"));
    let mut producer = direct_producer("active", registry, None, None);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let result = producer.poll_ready(&mut cx);
    assert!(matches!(result, Poll::Ready(Ok(()))));
    assert_eq!(
        producer.semaphore.available_permits(),
        1,
        "poll_ready must not consume the call permit"
    );
}

#[test]
fn test_poll_ready_allows_missing_consumer_when_fail_if_no_consumers_false() {
    let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
    let mut producer = direct_producer("missing-ok", registry, Some(false), None);

    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let result = producer.poll_ready(&mut cx);
    assert!(matches!(result, Poll::Ready(Ok(()))));
}

#[tokio::test]
async fn test_direct_crashed_consumer_entry_is_overwritable() {
    let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
    // A closed entry IS the crashed-consumer state (a consumer whose
    // start() exited without cleanup — its CloseGuard set the flag).
    registry
        .lock()
        .unwrap()
        .insert("crashed".to_string(), test_entry(true, "stale-route"));

    let (route_tx, _route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let token = CancellationToken::new();
    let ctx = ConsumerContext::new(route_tx, token.clone(), "replacement-route".to_string());
    let mut consumer = DirectConsumer {
        name: "crashed".to_string(),
        registry: registry.clone(),
        cancel: None,
    };
    let handle = tokio::spawn(async move {
        consumer.start(ctx).await.unwrap();
    });

    // Registration must succeed and REPLACE the stale entry: within 2s
    // the live entry is the replacement consumer's.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let replaced = {
            let reg = registry.lock().unwrap_or_else(|e| e.into_inner());
            reg.get("crashed")
                .is_some_and(|entry| entry.ctx.route_id() == "replacement-route")
        };
        if replaced {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "replacement consumer did not register within 2s"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    token.cancel();
    handle.await.unwrap();
}

/// poll_ready on a registry entry left behind by a crashed consumer must
/// report not-ready with the same error shape as the old closed-sender
/// arm.
#[test]
fn test_direct_poll_ready_reports_stale_entry_not_ready() {
    let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
    registry
        .lock()
        .unwrap()
        .insert("stale".to_string(), test_entry(true, "stale-route"));

    let mut producer = direct_producer("stale", registry, None, None);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    match producer.poll_ready(&mut cx) {
        Poll::Ready(Err(CamelError::EndpointCreationFailed(msg))) => {
            assert!(msg.contains("closed"), "unexpected error message: {msg}");
        }
        other => panic!("expected closed-entry error, got: {other:?}"),
    }
}

#[tokio::test]
async fn call_blocks_on_semaphore_until_release() {
    let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
    let (route_tx, mut route_rx) = mpsc::channel::<ExchangeEnvelope>(4);
    let ctx = ConsumerContext::new(route_tx, CancellationToken::new(), "park-route".to_string());
    registry.lock().unwrap().insert(
        "park".to_string(),
        DirectEntry {
            ctx,
            closed: Arc::new(AtomicBool::new(false)),
            dispatcher: None,
        },
    );

    let mut producer = direct_producer("park", registry, None, None);
    let semaphore = Arc::clone(&producer.semaphore);

    // Route stub: parks the FIRST exchange's reply on the test-controlled
    // release signal; later exchanges reply immediately.
    let parked = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let parked_signal = Arc::clone(&parked);
    let release_signal = Arc::clone(&release);
    tokio::spawn(async move {
        let mut first = true;
        while let Some(envelope) = route_rx.recv().await {
            if first {
                first = false;
                parked_signal.notify_one();
                release_signal.notified().await;
            }
            if let Some(tx) = envelope.reply_tx {
                let _ = tx.send(Ok(envelope.exchange));
            }
        }
    });

    let fut_a = producer.call(Exchange::new(Message::new("a")));
    let fut_b = producer.call(Exchange::new(Message::new("b")));
    let a = tokio::spawn(fut_a);
    let b = tokio::spawn(fut_b);

    tokio::time::timeout(Duration::from_secs(5), parked.notified())
        .await
        .expect("route stub must receive call A's exchange within 5s");
    // Let B run into the semaphore acquisition and park there.
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;

    assert_eq!(
        semaphore.available_permits(),
        0,
        "call A must hold the sole permit while awaiting its reply"
    );
    assert!(
        !b.is_finished(),
        "call B must be blocked on the semaphore while call A is in flight"
    );

    release.notify_one();

    let reply_a = a
        .await
        .expect("call A task must not panic")
        .expect("call A must complete Ok once its reply arrives");
    assert_eq!(reply_a.input.body.as_text(), Some("a"));
    let reply_b = b
        .await
        .expect("call B task must not panic")
        .expect("call B must complete Ok after the permit is released");
    assert_eq!(reply_b.input.body.as_text(), Some("b"));
}

#[tokio::test]
async fn call_pending_when_all_permits_held() {
    let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
    // Keep the route receiver alive (unpolled) so the submission channel
    // stays open and the reply never arrives: the call future must park.
    let (route_tx, _route_rx) = mpsc::channel::<ExchangeEnvelope>(1);
    let ctx = ConsumerContext::new(route_tx, CancellationToken::new(), "held-route".to_string());
    registry.lock().unwrap().insert(
        "held".to_string(),
        DirectEntry {
            ctx,
            closed: Arc::new(AtomicBool::new(false)),
            dispatcher: None,
        },
    );

    let mut producer = direct_producer("held", registry, None, None);
    let permit = Arc::clone(&producer.semaphore)
        .try_acquire_owned()
        .expect("sole permit must be free before call");

    let mut fut = producer.call(Exchange::new(Message::new("x")));
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(
        fut.as_mut().poll(&mut cx).is_pending(),
        "call must pend while the sole permit is held elsewhere"
    );

    drop(permit);
    // Next poll completes acquisition and proceeds into the send/reply
    // round-trip (which pends: nobody replies). Holding the permit proves
    // the future got past acquisition.
    assert!(fut.as_mut().poll(&mut cx).is_pending());
    assert_eq!(
        producer.semaphore.available_permits(),
        0,
        "call future must now hold the permit (past acquisition)"
    );
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

    // start() runs the consumer loop inline on the managed consumer task.
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

    // Drain envelopes but hold them so the producer never gets a reply
    tokio::spawn(async move {
        let mut held: Vec<ExchangeEnvelope> = Vec::new();
        while let Some(envelope) = route_rx.recv().await {
            held.push(envelope);
        }
        drop(held);
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Create producer with a short timeout
    let _ = test_producer_ctx();
    let _producer_endpoint = component
        .create_endpoint("direct:timeout-test", &NoOpComponentContext)
        .unwrap();
    let producer = direct_producer(
        "timeout-test",
        Arc::clone(&component.registry),
        None,
        Some(100), // 100ms timeout
    );

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
    let mut consumer = endpoint.create_consumer(rt()).unwrap();

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

    // Send an exchange via the producer so its dispatch's send_and_wait
    // returns Err, triggering the metrics call.
    let ctx = test_producer_ctx();
    let producer_endpoint = component
        .create_endpoint("direct:metrics-error", &NoOpComponentContext)
        .unwrap();
    // The producer owns the b-prime emission: it is the participant that
    // observes the send_and_wait Err.
    let producer = producer_endpoint.create_producer(runtime, &ctx).unwrap();

    let exchange = Exchange::new(Message::new("test"));
    // The dispatch helper wraps the call — it returns the Err from
    // send_and_wait back to us.
    let _result = dispatch(producer, exchange).await;

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

#[test]
fn test_direct_consumer_startup_mode_is_explicit() {
    let component = DirectComponent::new();
    let endpoint = component
        .create_endpoint("direct:ready-check", &NoOpComponentContext)
        .unwrap();
    let consumer = endpoint.create_consumer(rt()).unwrap();
    assert_eq!(
        consumer.startup_mode(),
        ConsumerStartupMode::Explicit,
        "DirectConsumer must opt into Explicit startup"
    );
}

#[tokio::test]
async fn test_direct_consumer_marks_ready_after_registration() {
    let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));

    let mut consumer = DirectConsumer {
        name: "ready-probe-direct".into(),
        registry: registry.clone(),
        cancel: None,
    };

    let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, token.clone(), "ready-probe-route".to_string());

    let (signal, startup_rx) = StartupSignal::pair();
    let ctx = ctx.with_startup(signal);

    tokio::spawn(async move {
        let _ = consumer.start(ctx).await;
    });

    let result = tokio::time::timeout(Duration::from_secs(2), startup_rx.await_ready())
        .await
        .expect("DirectConsumer must call ctx.mark_ready() after registration");
    assert!(
        result.is_ok(),
        "mark_ready must resolve Ok after registration"
    );

    {
        let reg = registry.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            reg.contains_key("ready-probe-direct"),
            "registry must contain consumer name after mark_ready resolves"
        );
    }

    token.cancel();
}

/// No-op fake inline dispatcher: resolves with the exchange unchanged.
/// Mirrors the camel-component-api test fakes but is defined locally so
/// test-only fakes stay out of the cross-crate dependency graph.
struct FakeInlineDispatcher;

impl InlineRouteDispatcher for FakeInlineDispatcher {
    fn dispatch(
        &self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send + 'static>> {
        Box::pin(async move { Ok(exchange) })
    }
}

/// The registry entry must carry whatever capability the core runtime
/// published on the consumer context: `Some` when a dispatcher was set
/// before startup, `None` otherwise (producer fallback to `send_and_wait`).
#[tokio::test]
async fn test_direct_registry_entry_carries_dispatcher_option() {
    // With a dispatcher set before startup: the entry carries Some.
    let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
    let mut consumer = DirectConsumer {
        name: "dispatcher-probe-direct".into(),
        registry: registry.clone(),
        cancel: None,
    };

    let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, token.clone(), "dispatcher-probe-route".to_string());
    ctx.set_inline_dispatcher(Arc::new(FakeInlineDispatcher));

    let (signal, startup_rx) = StartupSignal::pair();
    let ctx = ctx.with_startup(signal);

    tokio::spawn(async move {
        let _ = consumer.start(ctx).await;
    });

    let result = tokio::time::timeout(Duration::from_secs(2), startup_rx.await_ready())
        .await
        .expect("consumer must reach mark_ready within 2s");
    assert!(
        result.is_ok(),
        "mark_ready must resolve Ok after registration"
    );

    {
        let reg = registry.lock().unwrap_or_else(|e| e.into_inner());
        let entry = reg
            .get("dispatcher-probe-direct")
            .expect("entry must exist after mark_ready resolves");
        assert!(
            entry.dispatcher.is_some(),
            "entry must carry the capability published on the context"
        );
    }

    token.cancel();

    // Without a dispatcher: the entry carries None (send_and_wait fallback).
    let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
    let mut consumer = DirectConsumer {
        name: "no-dispatcher-probe-direct".into(),
        registry: registry.clone(),
        cancel: None,
    };

    let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, token.clone(), "no-dispatcher-probe-route".to_string());

    let (signal, startup_rx) = StartupSignal::pair();
    let ctx = ctx.with_startup(signal);

    tokio::spawn(async move {
        let _ = consumer.start(ctx).await;
    });

    let result = tokio::time::timeout(Duration::from_secs(2), startup_rx.await_ready())
        .await
        .expect("consumer must reach mark_ready within 2s");
    assert!(
        result.is_ok(),
        "mark_ready must resolve Ok after registration"
    );

    {
        let reg = registry.lock().unwrap_or_else(|e| e.into_inner());
        let entry = reg
            .get("no-dispatcher-probe-direct")
            .expect("entry must exist after mark_ready resolves");
        assert!(
            entry.dispatcher.is_none(),
            "entry must carry None when the context has no capability"
        );
    }

    token.cancel();
}

// -----------------------------------------------------------------------
// Cycle regression: direct:a -> direct:b -> direct:a
// -----------------------------------------------------------------------

/// Pipeline stand-in for the cycle fixture: the "route body" is one
/// forwarding producer call to the OTHER endpoint (what a
/// `to(direct:...)` step would do), executed inline by the dispatcher
/// on the caller's task — the shape the live camel-core runtime wires
/// for Sequential routes.
struct ForwardingDispatcher {
    forward: BoxProcessor,
}

impl InlineRouteDispatcher for ForwardingDispatcher {
    fn dispatch(
        &self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send + 'static>> {
        let forward = self.forward.clone();
        Box::pin(async move { dispatch(forward, exchange).await })
    }
}

/// Two direct endpoints sharing one registry, wired into a call cycle
/// the way the live inline runtime wires Sequential routes: each
/// consumer's context publishes an inline dispatcher whose pipeline is
/// the forwarding producer call. Both endpoints use `timeout_ms=500`
/// so every producer `call()` bounds itself instead of hanging on the
/// 30s default.
fn cycle_routes_ctx() -> (DirectComponent, ProducerContext) {
    let component = DirectComponent::new();
    let producer_ctx = ProducerContext::new();

    let endpoint_a = component
        .create_endpoint("direct:a?timeout_ms=500", &NoOpComponentContext)
        .unwrap();
    let endpoint_b = component
        .create_endpoint("direct:b?timeout_ms=500", &NoOpComponentContext)
        .unwrap();

    let mut consumer_a = endpoint_a.create_consumer(rt()).unwrap();
    let mut consumer_b = endpoint_b.create_consumer(rt()).unwrap();

    // Producers used inside the pipelines to forward to the OTHER
    // endpoint of the cycle.
    let producer_to_b = endpoint_b.create_producer(rt(), &producer_ctx).unwrap();
    let producer_to_a = endpoint_a.create_producer(rt(), &producer_ctx).unwrap();

    let (tx_a, _rx_a) = mpsc::channel::<ExchangeEnvelope>(16);
    let (tx_b, _rx_b) = mpsc::channel::<ExchangeEnvelope>(16);

    let ctx_a = ConsumerContext::new(tx_a, CancellationToken::new(), "cycle-route-a".to_string());
    ctx_a.set_inline_dispatcher(Arc::new(ForwardingDispatcher {
        forward: producer_to_b,
    }));
    let ctx_b = ConsumerContext::new(tx_b, CancellationToken::new(), "cycle-route-b".to_string());
    ctx_b.set_inline_dispatcher(Arc::new(ForwardingDispatcher {
        forward: producer_to_a,
    }));

    tokio::spawn(async move {
        consumer_a.start(ctx_a).await.unwrap();
    });
    tokio::spawn(async move {
        consumer_b.start(ctx_b).await.unwrap();
    });

    (component, producer_ctx)
}

/// Pins current cycle semantics: a `direct:a -> direct:b -> direct:a`
/// call cycle must terminate with a dispatch error — never `Ok`, never
/// a panic, never the external deadline elapsing silently. With the
/// inline path live, the task-local cycle guard rejects the re-entry
/// immediately: the error arrives well under the 500ms producer
/// timeout (which is only a backstop now) and carries the cycle-guard
/// prefix.
#[tokio::test]
async fn test_direct_cycle_never_succeeds_or_hangs() {
    let (component, producer_ctx) = cycle_routes_ctx();

    // Wait until both cycle consumers are registered so the dispatch
    // exercises the real cycle instead of failing fast with
    // "not registered".
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let registered = {
            let reg = component.registry.lock().unwrap_or_else(|e| e.into_inner());
            reg.contains_key("a") && reg.contains_key("b")
        };
        if registered {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "cycle consumers did not register within 2s"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let producer = component
        .create_endpoint("direct:a?timeout_ms=500", &NoOpComponentContext)
        .unwrap()
        .create_producer(rt(), &producer_ctx)
        .unwrap();

    let exchange = Exchange::new(Message::new("cycle"));

    // External deadline: the cycle guard rejects immediately, long
    // before these 5s elapse.
    let start = std::time::Instant::now();
    let outcome = tokio::time::timeout(Duration::from_secs(5), dispatch(producer, exchange))
        .await
        .expect("cycle dispatch must resolve before the 5s external deadline");
    let elapsed = start.elapsed();

    let err =
        outcome.expect_err("cyclic direct dispatch must never succeed — cycle semantics regressed");
    assert!(
        matches!(&err, CamelError::ProcessorError(msg)
            if msg.starts_with(inline_guard::CYCLE_ERROR_PREFIX)),
        "expected cycle-guard rejection, got: {err:?}"
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "cycle guard must reject immediately (timeout_ms=500 is only a \
         backstop): took {elapsed:?}"
    );
}

// -----------------------------------------------------------------------
// Inline fast path (Phase 3): producer path selection, guards, and
// timeout parity. Fakes implement the InlineRouteDispatcher contract
// per the camel-component-api test pattern; admission-style fakes own
// their tokio Mutex so a test helper can hold it externally.
// -----------------------------------------------------------------------

/// Seed one live registry entry: a real consumer context backed by its
/// own route channel (the test retains the receiver) plus the given
/// inline capability.
fn live_entry(
    route_tx: mpsc::Sender<ExchangeEnvelope>,
    dispatcher: Option<Arc<dyn InlineRouteDispatcher>>,
) -> DirectEntry {
    DirectEntry {
        ctx: ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "inline-test-route".to_string(),
        ),
        closed: Arc::new(AtomicBool::new(false)),
        dispatcher,
    }
}

/// Fake dispatcher that marks the exchange so tests can prove the
/// pipeline ran inline and the reply came back on the same task.
struct MarkerDispatcher;

impl InlineRouteDispatcher for MarkerDispatcher {
    fn dispatch(
        &self,
        mut exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send + 'static>> {
        Box::pin(async move {
            exchange.set_property("inline-marker", "ran");
            Ok(exchange)
        })
    }
}

/// Fake dispatcher whose dispatch never resolves (parks forever).
struct ParkingDispatcher;

impl InlineRouteDispatcher for ParkingDispatcher {
    fn dispatch(
        &self,
        _exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send + 'static>> {
        Box::pin(std::future::pending())
    }
}

/// Fake dispatcher mirroring the real adapter's admission protocol:
/// it locks its own FIFO mutex, signals `entered`, then parks while
/// holding the lock.
struct AdmissionParkingDispatcher {
    admission: Arc<tokio::sync::Mutex<()>>,
    entered: Arc<Notify>,
}

impl InlineRouteDispatcher for AdmissionParkingDispatcher {
    fn dispatch(
        &self,
        _exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send + 'static>> {
        let admission = Arc::clone(&self.admission);
        let entered = Arc::clone(&self.entered);
        Box::pin(async move {
            let _guard = admission.lock().await;
            entered.notify_one();
            std::future::pending().await
        })
    }
}

/// Fake dispatcher that re-dispatches into `direct:<same name>` through
/// a nested producer, forming a self-cycle through the producer path.
struct CycleDispatcher {
    name: String,
    registry: DirectRegistry,
}

impl InlineRouteDispatcher for CycleDispatcher {
    fn dispatch(
        &self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send + 'static>> {
        let mut producer = direct_producer(&self.name, Arc::clone(&self.registry), None, Some(500));
        Box::pin(async move { producer.call(exchange).await })
    }
}

/// Fake dispatcher that serializes on its own admission mutex and
/// records begin/end events under an internal delay, so interleaving
/// would be visible in the event log.
struct FifoRecorderDispatcher {
    admission: Arc<tokio::sync::Mutex<()>>,
    events: Arc<Mutex<Vec<String>>>,
}

impl InlineRouteDispatcher for FifoRecorderDispatcher {
    fn dispatch(
        &self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send + 'static>> {
        let admission = Arc::clone(&self.admission);
        let events = Arc::clone(&self.events);
        Box::pin(async move {
            let id = exchange
                .input
                .body
                .as_text()
                .unwrap_or_default()
                .to_string();
            let _guard = admission.lock().await;
            events.lock().unwrap().push(format!("begin:{id}"));
            tokio::time::sleep(Duration::from_millis(10)).await;
            events.lock().unwrap().push(format!("end:{id}"));
            Ok(exchange)
        })
    }
}

#[tokio::test]
async fn inline_dispatch_roundtrip_same_task() {
    let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
    let (route_tx, mut route_rx) = mpsc::channel::<ExchangeEnvelope>(1);
    registry.lock().unwrap().insert(
        "roundtrip".to_string(),
        live_entry(route_tx, Some(Arc::new(MarkerDispatcher))),
    );

    let producer = direct_producer("roundtrip", registry, None, Some(500));
    let reply = dispatch(producer, Exchange::new(Message::new("inline")))
        .await
        .expect("inline dispatch must complete Ok");

    assert_eq!(
        reply.property("inline-marker").and_then(|v| v.as_str()),
        Some("ran"),
        "the fake dispatcher's pipeline must have run and its marker returned"
    );
    assert!(
        route_rx.try_recv().is_err(),
        "inline path must not touch the consumer-context submission channel"
    );
}

#[tokio::test]
async fn inline_falls_back_when_capability_absent() {
    let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
    let (route_tx, mut route_rx) = mpsc::channel::<ExchangeEnvelope>(4);
    registry
        .lock()
        .unwrap()
        .insert("fallback".to_string(), live_entry(route_tx, None));

    let submitted = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&submitted);
    tokio::spawn(async move {
        while let Some(envelope) = route_rx.recv().await {
            counter.fetch_add(1, Ordering::SeqCst);
            if let Some(tx) = envelope.reply_tx {
                let _ = tx.send(Ok(envelope.exchange));
            }
        }
    });

    let producer = direct_producer("fallback", registry, None, None);
    let reply = dispatch(producer, Exchange::new(Message::new("via-channel")))
        .await
        .expect("channel fallback must complete Ok");

    assert_eq!(reply.input.body.as_text(), Some("via-channel"));
    assert_eq!(
        submitted.load(Ordering::SeqCst),
        1,
        "exactly one envelope must have gone through the submission channel"
    );
}

#[tokio::test]
async fn inline_timeout_error_text_matches_channel() {
    // Inline side: a parking dispatcher under the shared endpoint name.
    let inline_registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
    let (tx_inline, _rx_inline) = mpsc::channel::<ExchangeEnvelope>(1);
    inline_registry.lock().unwrap().insert(
        "tmo-parity".to_string(),
        live_entry(tx_inline, Some(Arc::new(ParkingDispatcher))),
    );
    let inline_producer = direct_producer("tmo-parity", inline_registry, None, Some(200));

    // Channel side: real ctx with a parking consumer — the envelope is
    // buffered but no one ever recv's it, so no reply ever arrives.
    let channel_registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
    let (tx_channel, _rx_channel) = mpsc::channel::<ExchangeEnvelope>(1);
    channel_registry
        .lock()
        .unwrap()
        .insert("tmo-parity".to_string(), live_entry(tx_channel, None));
    let channel_producer = direct_producer("tmo-parity", channel_registry, None, Some(200));

    let inline_err = dispatch(inline_producer, Exchange::new(Message::new("x")))
        .await
        .expect_err("parking inline dispatch must time out")
        .to_string();
    let channel_err = dispatch(channel_producer, Exchange::new(Message::new("y")))
        .await
        .expect_err("parking channel dispatch must time out")
        .to_string();

    assert_eq!(
        inline_err, channel_err,
        "inline and channel timeout texts must be identical"
    );
    assert_eq!(inline_err, dispatch_timeout_error("tmo-parity").to_string());
}

#[tokio::test]
async fn inline_timeout_covers_admission_wait() {
    let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
    let (route_tx, _route_rx) = mpsc::channel::<ExchangeEnvelope>(1);
    let dispatcher = AdmissionParkingDispatcher {
        admission: Arc::new(tokio::sync::Mutex::new(())),
        entered: Arc::new(Notify::new()),
    };
    let entered = Arc::clone(&dispatcher.entered);
    registry.lock().unwrap().insert(
        "adm-wait".to_string(),
        live_entry(route_tx, Some(Arc::new(dispatcher))),
    );

    // First dispatch: parks inside dispatch() while HOLDING the
    // admission mutex; its generous budget keeps it parked for the
    // whole test.
    let parked_producer = direct_producer("adm-wait", Arc::clone(&registry), None, Some(10_000));
    let parked = tokio::spawn(async move {
        let mut svc = parked_producer;
        let _ = svc.call(Exchange::new(Message::new("first"))).await;
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .expect("first dispatch must acquire admission within 2s");

    // Second concurrent producer (distinct instance → distinct
    // endpoint semaphore): it must time out while WAITING on
    // admission — the wait is inside the timeout boundary — not hang
    // until the parked dispatch releases it.
    let waiting_producer = direct_producer("adm-wait", registry, None, Some(200));
    let started = std::time::Instant::now();
    let err = dispatch(waiting_producer, Exchange::new(Message::new("second")))
        .await
        .expect_err("second dispatch must time out on the admission wait");

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "admission wait must be inside the timeout boundary (elapsed {:?})",
        started.elapsed()
    );
    assert_eq!(
        err.to_string(),
        dispatch_timeout_error("adm-wait").to_string()
    );

    parked.abort();
}

#[test]
fn inline_default_timeout_is_30s() {
    // One shared construction site serves both paths: no endpoint
    // timeout_ms → the 30s default, identical on the channel path.
    assert_eq!(
        effective_dispatch_timeout(None),
        Duration::from_millis(30_000)
    );
    assert_eq!(
        effective_dispatch_timeout(Some(200)),
        Duration::from_millis(200)
    );
    assert_eq!(
        DirectConfig::from_uri("direct:no-tmo").unwrap().timeout_ms,
        None
    );
}

#[tokio::test]
async fn inline_cycle_rejected_without_fallback() {
    let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
    let (route_tx, mut route_rx) = mpsc::channel::<ExchangeEnvelope>(1);
    registry.lock().unwrap().insert(
        "cycle".to_string(),
        live_entry(
            route_tx,
            Some(Arc::new(CycleDispatcher {
                name: "cycle".to_string(),
                registry: Arc::clone(&registry),
            })),
        ),
    );

    let producer = direct_producer("cycle", Arc::clone(&registry), None, Some(2_000));
    let err = dispatch(producer, Exchange::new(Message::new("cycle")))
        .await
        .expect_err("cyclic inline dispatch must fail");

    assert!(
        matches!(&err, CamelError::ProcessorError(msg)
            if msg.starts_with(inline_guard::CYCLE_ERROR_PREFIX)),
        "expected cycle rejection, got: {err:?}"
    );
    assert!(
        route_rx.try_recv().is_err(),
        "cycle rejection must not fall back to the channel path"
    );
}

#[tokio::test]
async fn concurrent_producers_serialized_fifo() {
    let registry: DirectRegistry = Arc::new(Mutex::new(HashMap::new()));
    let (route_tx, _route_rx) = mpsc::channel::<ExchangeEnvelope>(1);
    let dispatcher = FifoRecorderDispatcher {
        admission: Arc::new(tokio::sync::Mutex::new(())),
        events: Arc::new(Mutex::new(Vec::new())),
    };
    let events = Arc::clone(&dispatcher.events);
    registry.lock().unwrap().insert(
        "fifo".to_string(),
        live_entry(route_tx, Some(Arc::new(dispatcher))),
    );

    let producer = direct_producer("fifo", registry, None, Some(2_000));
    let mut tasks = Vec::new();
    for i in 0..4 {
        let mut svc = producer.clone();
        let events_clone = Arc::clone(&events);
        tasks.push(tokio::spawn(async move {
            if svc
                .call(Exchange::new(Message::new(format!("p{i}"))))
                .await
                .is_err()
            {
                events_clone.lock().unwrap().push(format!("error:{i}"));
            }
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }

    let log = events.lock().unwrap().clone();
    assert_eq!(log.len(), 8, "four begin/end pairs, got: {log:?}");
    let mut seen: Vec<&str> = Vec::new();
    for pair in log.chunks(2) {
        let begin = pair[0].strip_prefix("begin:").expect("begin event");
        let end = pair[1]
            .strip_prefix("end:")
            .expect("end event, no interleaving");
        assert_eq!(
            begin, end,
            "each begin must be immediately followed by its end: {log:?}"
        );
        assert!(!seen.contains(&begin), "duplicate execution: {log:?}");
        seen.push(begin);
    }
    assert_eq!(seen.len(), 4, "all four producers must have executed");
}
