//! Lifecycle test for the circuit breaker fallback sub-pipeline.
//!
//! Pins the R2-C2 fix: fallback steps compiled into
//! `CircuitBreakerConfig.fallback` are live participants of the route —
//! BOTH their in-flight exchanges (drain) AND their `StepLifecycle`
//! handles participate in route start/stop. The fallback sidecar carries a
//! `To("blocker:tap")` step backed by a test component whose endpoint
//! exposes a custom `StepLifecycle`, so the test observes:
//!
//! 1. `start()` is invoked for the fallback step at route start (the
//!    fallback lifecycle handles are merged into the route's lifecycle vec
//!    at compile time — `attach_cb_fallback` + `lifecycle.extend`).
//! 2. A shutdown started while a fallback exchange is blocked inside the
//!    fallback producer does NOT complete until the exchange drains.
//! 3. Shutdown does NOT complete until the fallback step's own
//!    `StepLifecycle::shutdown` completes: remove the lifecycle merge from
//!    the compile sites and assertions 1 and 3 fail (start/shutdown never
//!    invoked, stop finishes early).
//!
//! Pattern: direct `RouteDefinition::new` + `CamelContext` (no YAML), mirroring
//! `peek_stale_on_miss_test.rs`. Runs the layer branch (`eh_config = None` →
//! Tower `CircuitBreakerService`) — the gate branch is covered by
//! `circuit_breaker_fallback_gate_path_serves_body` in camel-test.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::circuit_breaker::CircuitBreakerConfig;
use camel_api::{
    BoxProcessor, BoxProcessorExt, CamelError, Exchange, Message, OpaqueProcessor, StepLifecycle,
    StepShutdownReason,
};
use camel_component_api::{Component, Consumer, Endpoint};
use camel_component_direct::DirectComponent;
use camel_core::route::BuilderStep;
use camel_core::{CamelContext, RouteDefinition};
use tower::ServiceExt;

/// Runtime observability stub for `create_producer`.
fn test_rt() -> Arc<dyn camel_component_api::RuntimeObservability> {
    Arc::new(camel_component_api::NoOpComponentContext)
}

/// A processor that always fails with the given message.
fn failing_step(msg: &'static str) -> BoxProcessor {
    BoxProcessor::from_fn(move |_ex| {
        Box::pin(async move { Err(CamelError::ProcessorError(msg.into())) })
    })
}

/// Shared observable state for the `blocker:` test component.
#[derive(Debug)]
struct BlockerState {
    /// The fallback producer was invoked (fallback is executing).
    entered: AtomicBool,
    /// The fallback producer completed — set only AFTER `producer_release`.
    delivered: AtomicBool,
    /// `StepLifecycle::start` was invoked for the fallback step.
    started: AtomicBool,
    /// `StepLifecycle::shutdown` was invoked for the fallback step.
    shutdown_entered: AtomicBool,
    /// `StepLifecycle::shutdown` completed — set only AFTER
    /// `lifecycle_release`.
    shutdown_done: AtomicBool,
    /// Releases the blocked fallback producer (in-flight drain hold).
    producer_release: tokio::sync::Notify,
    /// Releases the blocked fallback `StepLifecycle::shutdown`.
    lifecycle_release: tokio::sync::Notify,
}

impl BlockerState {
    fn new() -> Self {
        Self {
            entered: AtomicBool::new(false),
            delivered: AtomicBool::new(false),
            started: AtomicBool::new(false),
            shutdown_entered: AtomicBool::new(false),
            shutdown_done: AtomicBool::new(false),
            producer_release: tokio::sync::Notify::new(),
            lifecycle_release: tokio::sync::Notify::new(),
        }
    }
}

/// `StepLifecycle` handle surfaced by `BlockingEndpoint::lifecycle()`.
#[derive(Debug)]
struct BlockerLifecycle(Arc<BlockerState>);

#[async_trait]
impl StepLifecycle for BlockerLifecycle {
    fn name(&self) -> &'static str {
        "blocker"
    }

    async fn start(&self) -> Result<(), CamelError> {
        self.0.started.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn shutdown(&self, _reason: StepShutdownReason) -> Result<(), CamelError> {
        self.0.shutdown_entered.store(true, Ordering::SeqCst);
        self.0.lifecycle_release.notified().await;
        self.0.shutdown_done.store(true, Ordering::SeqCst);
        Ok(())
    }
}

/// Endpoint vends a producer that blocks until `producer_release`, plus the
/// [`BlockerLifecycle`] handle harvested by the `To` step compiler.
struct BlockingEndpoint {
    state: Arc<BlockerState>,
}

impl Endpoint for BlockingEndpoint {
    fn uri(&self) -> &str {
        "blocker:tap"
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::RouteError("blocker has no consumer".into()))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &camel_api::ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        let state = Arc::clone(&self.state);
        Ok(BoxProcessor::from_fn(move |ex| {
            let state = Arc::clone(&state);
            Box::pin(async move {
                // Signal entry so the test knows the fallback is executing.
                state.entered.store(true, Ordering::SeqCst);
                // Block until the test releases the producer (drain hold).
                state.producer_release.notified().await;
                // Record the delivery only after the release.
                state.delivered.store(true, Ordering::SeqCst);
                Ok(ex)
            })
        }))
    }

    fn lifecycle(&self) -> Option<Arc<dyn StepLifecycle>> {
        Some(Arc::new(BlockerLifecycle(Arc::clone(&self.state))))
    }
}

/// Component serving the `blocker:` scheme for the fallback sidecar.
struct BlockingComponent {
    state: Arc<BlockerState>,
}

impl Component for BlockingComponent {
    fn scheme(&self) -> &str {
        "blocker"
    }

    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(BlockingEndpoint {
            state: Arc::clone(&self.state),
        }))
    }
}

/// Asserts that fallback steps are live lifecycle participants:
///
/// - `StepLifecycle::start` runs for the fallback step at route start
///   (requires the compile-site lifecycle merge).
/// - Shutdown started while a fallback exchange is blocked inside the
///   fallback producer does NOT complete until the exchange drains.
/// - `StepLifecycle::shutdown` runs for the fallback step and stop does NOT
///   complete until it finishes (requires the compile-site lifecycle merge).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stateful_fallback_step_lifecycle_invoked() {
    let state = Arc::new(BlockerState::new());

    let def = RouteDefinition::new(
        "direct:cb",
        vec![BuilderStep::Processor(OpaqueProcessor(failing_step(
            "cb-lifecycle upstream failure",
        )))],
    )
    .with_route_id("cb-fallback-lifecycle")
    .with_circuit_breaker(
        CircuitBreakerConfig::new()
            .failure_threshold(1)
            .open_duration(Duration::from_secs(60)),
    )
    .with_circuit_breaker_fallback(vec![BuilderStep::To("blocker:tap".into())]);

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(DirectComponent::new());
    ctx.register_component(BlockingComponent {
        state: Arc::clone(&state),
    });

    ctx.add_route_definition(def)
        .await
        .expect("route must compile");
    ctx.start().await.expect("context start failed");

    // The fallback step's lifecycle handle must be merged into the route's
    // lifecycle vec at compile time, so `start_route` invokes `start()`.
    assert!(
        state.started.load(Ordering::SeqCst),
        "fallback StepLifecycle::start was never invoked — \
         fallback lifecycle handles were not merged into the route lifecycle"
    );

    // Exchange 1: circuit closed → body fails → failure counted → circuit
    // OPENS (failure_threshold = 1). With no error handler the failure
    // propagates as a reply `Err`.
    {
        let component = ctx.registry().get("direct").expect("direct registered");
        let producer_ctx = ctx.producer_context();
        let endpoint = component
            .create_endpoint("direct:cb", &ctx)
            .expect("create endpoint");
        let producer = endpoint
            .create_producer(test_rt(), &producer_ctx)
            .expect("create producer");
        match producer.oneshot(Exchange::new(Message::new("first"))).await {
            Err(e) => assert!(
                !matches!(e, CamelError::CircuitOpen(_)),
                "first exchange must fail upstream, not on an open circuit: {e}"
            ),
            Ok(_) => panic!("first exchange must fail and open the circuit"),
        }
    }

    // Exchange 2: circuit open → fallback runs → the producer BLOCKS. Spawn
    // the send so the test can begin shutdown while the fallback is in
    // flight.
    let producer = {
        let component = ctx.registry().get("direct").expect("direct registered");
        let producer_ctx = ctx.producer_context();
        let endpoint = component
            .create_endpoint("direct:cb", &ctx)
            .expect("create endpoint");
        endpoint
            .create_producer(test_rt(), &producer_ctx)
            .expect("create producer")
    };
    let in_flight = tokio::spawn(async move {
        let _ = producer
            .oneshot(Exchange::new(Message::new("second")))
            .await;
    });

    // Wait until the fallback producer has actually entered (bounded).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while !state.entered.load(Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "fallback producer never entered — fallback was not compiled/executed"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        !state.delivered.load(Ordering::SeqCst),
        "delivery must not be recorded while the producer is blocked"
    );

    // BEGIN shutdown while the fallback exchange is in flight. The stop
    // future must NOT complete: the blocked fallback exchange keeps the
    // route's drain counter above zero (grace period is 5s by default —
    // far above this window).
    let stopper = tokio::spawn(async move {
        ctx.stop().await.expect("context stop failed");
    });
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        !stopper.is_finished(),
        "shutdown completed while a fallback exchange was still in flight — \
         fallback drain was skipped"
    );

    // Release the producer: the fallback exchange completes, the delivery is
    // recorded, drain finishes, and stop proceeds to step-lifecycle
    // shutdown. The fallback step's `StepLifecycle::shutdown` must be
    // invoked — without the compile-site lifecycle merge it never runs.
    state.producer_release.notify_one();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while !state.shutdown_entered.load(Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "fallback StepLifecycle::shutdown was never invoked — \
             fallback lifecycle handles were not merged into the route lifecycle"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // Shutdown is now blocked inside the fallback step's lifecycle
    // shutdown. Stop must NOT complete until that shutdown finishes.
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        !stopper.is_finished(),
        "shutdown completed while the fallback StepLifecycle::shutdown was \
         still blocked — stop does not await fallback step shutdown"
    );

    // Release the lifecycle shutdown: it completes and stop now finishes.
    state.lifecycle_release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), stopper)
        .await
        .expect("shutdown must complete after the fallback lifecycle drains")
        .expect("stop task must not panic");

    assert!(
        state.delivered.load(Ordering::SeqCst),
        "fallback delivery must be recorded after release"
    );
    assert!(
        state.shutdown_done.load(Ordering::SeqCst),
        "fallback StepLifecycle::shutdown must complete after release"
    );
    let _ = in_flight.await;
}

/// Fail-closed: a non-empty fallback sidecar WITHOUT a circuit breaker is
/// programmatic misuse (the DSL cannot produce it, but the builder setter
/// can). `attach_cb_fallback` must reject it instead of silently discarding
/// the sidecar.
#[tokio::test]
async fn fallback_without_circuit_breaker_fails_closed() {
    let def = RouteDefinition::new(
        "direct:cb",
        vec![BuilderStep::Processor(OpaqueProcessor(failing_step(
            "cb-no-config upstream",
        )))],
    )
    .with_route_id("cb-fallback-no-config")
    .with_circuit_breaker_fallback(vec![BuilderStep::To("blocker:tap".into())]);

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(DirectComponent::new());
    ctx.register_component(BlockingComponent {
        state: Arc::new(BlockerState::new()),
    });

    let err = ctx
        .add_route_definition(def)
        .await
        .expect_err("fallback sidecar without circuit_breaker must fail closed");
    let msg = err.to_string();
    assert!(
        msg.contains("circuit_breaker_fallback"),
        "error must name circuit_breaker_fallback: {msg}"
    );
}
