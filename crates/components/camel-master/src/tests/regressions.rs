//! regression tests (rc-f9k): slow stopping, failing start. Split from tests.rs by concern (bd rc-ubk1v).
//! Shared mocks/helpers live in the tests module root (tests.rs).

use super::*;

// ── Existing regression tests (rc-f9k) ──────────────────────────────

#[tokio::test]
async fn stops_retrying_delegate_start_after_max_attempts() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));

    let mut master = MasterConsumer::new(
        "lock-a".to_string(),
        "failing:delegate".to_string(),
        Arc::new(FailingDelegateComponent {
            create_endpoint_calls: Arc::clone(&create_endpoint_calls),
        }),
        Arc::new(NoOpMetrics),
        platform_service,
        Duration::from_millis(500),
        NetworkRetryPolicy {
            max_attempts: 1,
            ..NetworkRetryPolicy::default()
        },
        Arc::new(PanicRuntimeObservability) as Arc<dyn camel_component_api::RuntimeObservability>,
    );

    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "master-test-route".to_string());

    master.start(ctx).await.unwrap();
    sleep(Duration::from_millis(750)).await;

    // With error classification (rc-i1z), EndpointCreationFailed is
    // permanent → fail-fast after exactly 1 invocation. Previously
    // (pre-rc-i1z) this would have been 2 calls (initial + retry via
    // budget exhaustion).
    assert_eq!(create_endpoint_calls.load(Ordering::SeqCst), 1);

    cancel.cancel();
    let _ = master.stop().await;
}

/// Regression test for MST-002: stop() must abort the leadership JoinHandle
/// instead of just dropping it when the task is slow to drain.
/// Without the fix, stop() blocks for the full drain_timeout (~500 ms)
/// because the leadership task is stuck in stop_delegate awaiting a
/// slow delegate. With abort-first, stop() returns almost instantly.
#[tokio::test]
async fn stop_completes_quickly_when_leadership_task_is_slow() {
    // Delegate consumer that ignores its cancellation token and blocks.
    struct SlowStoppingConsumer;

    #[async_trait]
    impl Consumer for SlowStoppingConsumer {
        async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
            ctx.send(Exchange::new(Message::new("slow-start")))
                .await
                .ok();
            // Ignore cancellation — sleep far beyond the drain timeout.
            sleep(Duration::from_secs(60)).await;
            Ok(())
        }

        async fn stop(&mut self) -> Result<(), CamelError> {
            Ok(())
        }
    }

    struct SlowStoppingComponent;

    impl Component for SlowStoppingComponent {
        fn scheme(&self) -> &str {
            "slow"
        }

        fn create_endpoint(
            &self,
            _uri: &str,
            _ctx: &dyn ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            Ok(Box::new(SlowStoppingEndpoint))
        }
    }

    struct SlowStoppingEndpoint;

    impl Endpoint for SlowStoppingEndpoint {
        fn uri(&self) -> &str {
            "slow:delegate"
        }

        fn create_consumer(
            &self,
            _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
        ) -> Result<Box<dyn Consumer>, CamelError> {
            Ok(Box::new(SlowStoppingConsumer))
        }

        fn create_producer(
            &self,
            _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
            _ctx: &ProducerContext,
        ) -> Result<BoxProcessor, CamelError> {
            Err(CamelError::EndpointCreationFailed("not used".into()))
        }
    }

    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));

    let mut master = MasterConsumer::new(
        "lock-slow".into(),
        "slow:delegate".into(),
        Arc::new(SlowStoppingComponent),
        Arc::new(NoOpMetrics),
        platform_service,
        Duration::from_millis(500), // drain_timeout
        NetworkRetryPolicy {
            max_attempts: 30,
            ..NetworkRetryPolicy::default()
        },
        Arc::new(PanicRuntimeObservability) as Arc<dyn camel_component_api::RuntimeObservability>,
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "master-test-route".to_string());

    master.start(ctx).await.unwrap();

    // Wait for the delegate to actually start.
    let msg = timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(msg.exchange.input.body.as_text(), Some("slow-start"));

    // stop() must complete quickly because the leadership task is aborted,
    // not just timed-out and leaked.
    let start = Instant::now();
    master.stop().await.unwrap();
    let elapsed = start.elapsed();

    // With abort-first: ~0 ms. Without the fix: ~drain_timeout (500 ms).
    // Assert < 250 ms to reliably distinguish the two behaviours.
    assert!(
        elapsed < Duration::from_millis(250),
        "stop() took {:?}, expected < 250 ms (abort should be near-instant)",
        elapsed,
    );

    cancel.cancel();
}

#[tokio::test]
async fn stop_propagates_delegate_start_error() {
    struct FailingStartConsumer;

    #[async_trait]
    impl Consumer for FailingStartConsumer {
        async fn start(&mut self, _ctx: ConsumerContext) -> Result<(), CamelError> {
            Err(CamelError::ProcessorError(
                "delegate start failed".to_string(),
            ))
        }

        async fn stop(&mut self) -> Result<(), CamelError> {
            Ok(())
        }
    }

    struct FailingStartComponent;

    impl Component for FailingStartComponent {
        fn scheme(&self) -> &str {
            "failstart"
        }

        fn create_endpoint(
            &self,
            _uri: &str,
            _ctx: &dyn ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            Ok(Box::new(FailingStartEndpoint))
        }
    }

    struct FailingStartEndpoint;

    impl Endpoint for FailingStartEndpoint {
        fn uri(&self) -> &str {
            "failstart:delegate"
        }

        fn create_consumer(
            &self,
            _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
        ) -> Result<Box<dyn Consumer>, CamelError> {
            Ok(Box::new(FailingStartConsumer))
        }

        fn create_producer(
            &self,
            _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
            _ctx: &ProducerContext,
        ) -> Result<BoxProcessor, CamelError> {
            Err(CamelError::EndpointCreationFailed("not used".into()))
        }
    }

    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));

    let mut master = MasterConsumer::new(
        "lock-error".into(),
        "failstart:delegate".into(),
        Arc::new(FailingStartComponent),
        Arc::new(NoOpMetrics),
        platform_service,
        Duration::from_millis(500),
        NetworkRetryPolicy {
            max_attempts: 30,
            ..NetworkRetryPolicy::default()
        },
        Arc::new(PanicRuntimeObservability) as Arc<dyn camel_component_api::RuntimeObservability>,
    );

    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "master-test-route".to_string());

    master.start(ctx).await.unwrap();
    sleep(Duration::from_millis(250)).await;
    assert!(
        master
            .leadership_task
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished),
        "leadership task should finish after delegate error"
    );
    let err = master.stop().await.expect_err("expected delegate error");
    assert!(err.to_string().contains("delegate start failed"));

    cancel.cancel();
}
