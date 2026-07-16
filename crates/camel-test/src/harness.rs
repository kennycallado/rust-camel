// crates/camel-test/src/harness.rs

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use camel_api::CamelError;
use camel_component_direct::DirectComponent;
use camel_component_log::LogComponent;
use camel_component_mock::MockComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;
use camel_core::route::RouteDefinition;
use tokio::sync::Mutex;

use crate::time::TimeController;

// ---------------------------------------------------------------------------
// Typestates
// ---------------------------------------------------------------------------

pub struct NoTimeControl;
pub struct WithTimeControl;

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

type Registration = Box<dyn FnOnce(&mut CamelContext) + Send>;

/// Builder for [`CamelTestContext`].
///
/// Use [`CamelTestContext::builder()`] to obtain one.
pub struct CamelTestContextBuilder<S = NoTimeControl> {
    registrations: Vec<Registration>,
    mock: MockComponent,
    _state: std::marker::PhantomData<S>,
}

impl CamelTestContextBuilder<NoTimeControl> {
    pub(crate) fn new() -> Self {
        Self {
            registrations: Vec::new(),
            mock: MockComponent::new(),
            _state: std::marker::PhantomData,
        }
    }

    /// Activate tokio mock-time. `build()` will call `tokio::time::pause()`
    /// and return a [`TimeController`] alongside the harness.
    pub fn with_time_control(self) -> CamelTestContextBuilder<WithTimeControl> {
        CamelTestContextBuilder {
            registrations: self.registrations,
            mock: self.mock,
            _state: std::marker::PhantomData,
        }
    }

    /// Build the harness without time control.
    pub async fn build(self) -> CamelTestContext {
        build_context(self.registrations, self.mock).await
    }
}

impl CamelTestContextBuilder<WithTimeControl> {
    /// Build the harness with time control.
    ///
    /// Calls `tokio::time::pause()` before returning. Use the returned
    /// [`TimeController`] to advance the clock inside the test.
    pub async fn build(self) -> (CamelTestContext, TimeController) {
        tokio::time::pause();
        let ctx = build_context(self.registrations, self.mock).await;
        (ctx, TimeController)
    }
}

macro_rules! impl_builder_methods {
    ($S:ty) => {
        impl CamelTestContextBuilder<$S> {
            /// Include `MockComponent` explicitly (always registered; this is a
            /// documentation signal for call sites).
            pub fn with_mock(self) -> Self {
                self
            }

            /// Register `TimerComponent`.
            pub fn with_timer(mut self) -> Self {
                self.registrations.push(Box::new(|ctx: &mut CamelContext| {
                    ctx.register_component(TimerComponent::new());
                }));
                self
            }

            /// Register `LogComponent`.
            pub fn with_log(mut self) -> Self {
                self.registrations.push(Box::new(|ctx: &mut CamelContext| {
                    ctx.register_component(LogComponent::new());
                }));
                self
            }

            /// Register `DirectComponent`.
            pub fn with_direct(mut self) -> Self {
                self.registrations.push(Box::new(|ctx: &mut CamelContext| {
                    ctx.register_component(DirectComponent::new());
                }));
                self
            }

            /// Register `SedaComponent`.
            pub fn with_seda(mut self) -> Self {
                self.registrations.push(Box::new(|ctx: &mut CamelContext| {
                    ctx.register_component(camel_component_seda::SedaComponent::new());
                }));
                self
            }

            /// Register any component that implements the `Component` trait.
            pub fn with_component<C>(mut self, component: C) -> Self
            where
                C: camel_component_api::Component + 'static,
            {
                self.registrations
                    .push(Box::new(move |ctx: &mut CamelContext| {
                        ctx.register_component(component);
                    }));
                self
            }
        }
    };
}

impl_builder_methods!(NoTimeControl);
impl_builder_methods!(WithTimeControl);

// ---------------------------------------------------------------------------
// Internal build helper
// ---------------------------------------------------------------------------

async fn build_context(registrations: Vec<Registration>, mock: MockComponent) -> CamelTestContext {
    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap

    // MockComponent is always registered.
    ctx.register_component(mock.clone());

    // Run caller-declared registrations.
    for register in registrations {
        register(&mut ctx);
    }

    let ctx = Arc::new(Mutex::new(ctx));
    let stopped = Arc::new(AtomicBool::new(false));

    CamelTestContext {
        ctx: ctx.clone(),
        mock,
        stopped: stopped.clone(),
        _guard: TestGuard { ctx, stopped },
    }
}

// ---------------------------------------------------------------------------
// TestGuard — automatic stop on drop
// ---------------------------------------------------------------------------

pub(crate) struct TestGuard {
    ctx: Arc<Mutex<CamelContext>>,
    stopped: Arc<AtomicBool>,
}

impl Drop for TestGuard {
    fn drop(&mut self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            // Already stopped explicitly — nothing to do.
            return;
        }
        let ctx = self.ctx.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            match handle.runtime_flavor() {
                tokio::runtime::RuntimeFlavor::MultiThread => {
                    // Deterministic cleanup when blocking is supported.
                    tokio::task::block_in_place(|| {
                        handle.block_on(async move {
                            let mut ctx = ctx.lock().await;
                            let _ = ctx.stop().await;
                        });
                    });
                }
                tokio::runtime::RuntimeFlavor::CurrentThread => {
                    // Best effort fallback for current-thread runtimes where
                    // blocking in Drop is not possible.
                    handle.spawn(async move {
                        let mut ctx = ctx.lock().await;
                        let _ = ctx.stop().await;
                    });
                }
                _ => {
                    handle.spawn(async move {
                        let mut ctx = ctx.lock().await;
                        let _ = ctx.stop().await;
                    });
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// CamelTestContext
// ---------------------------------------------------------------------------

/// Test harness that wraps [`CamelContext`] with teardown helpers,
/// pre-registered components, and a shared [`MockComponent`] accessor.
///
/// # Example
///
/// ```no_run
/// # use camel_test::CamelTestContext;
/// # use std::time::Duration;
/// #[tokio::test]
/// async fn my_test() {
///     let h = CamelTestContext::builder()
///         .with_timer()
///         .with_mock()
///         .build()
///         .await;
///
///     // add routes, start, assert…
///     h.stop().await; // deterministic teardown
///     // Drop also performs best-effort cleanup if omitted
/// }
/// ```
pub struct CamelTestContext {
    ctx: Arc<Mutex<CamelContext>>,
    mock: MockComponent,
    stopped: Arc<AtomicBool>,
    _guard: TestGuard,
}

impl CamelTestContext {
    /// Obtain a builder.
    pub fn builder() -> CamelTestContextBuilder<NoTimeControl> {
        CamelTestContextBuilder::new()
    }

    /// Add a route definition to the context.
    pub async fn add_route(&self, route: RouteDefinition) -> Result<(), CamelError> {
        let ctx = self.ctx.lock().await;
        ctx.add_route_definition(route).await
    }

    /// Start all routes.
    pub async fn start(&self) {
        let mut ctx = self.ctx.lock().await;
        ctx.start().await.expect("CamelTestContext: start failed"); // allow-unwrap
    }

    /// Stop all routes explicitly. Safe to call before the harness is dropped —
    /// subsequent drop is a no-op.
    pub async fn stop(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return; // already stopped
        }
        let mut ctx = self.ctx.lock().await;
        ctx.stop().await.expect("CamelTestContext: stop failed"); // allow-unwrap
    }

    /// Consume the harness and stop routes deterministically.
    pub async fn shutdown(self) {
        self.stop().await;
    }

    /// Access the shared mock component for assertions.
    pub fn mock(&self) -> &MockComponent {
        &self.mock
    }

    /// Escape hatch: access the underlying [`CamelContext`] directly.
    pub fn ctx(&self) -> &Arc<Mutex<CamelContext>> {
        &self.ctx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_builder::{RouteBuilder, StepAccumulator};
    use std::time::Duration;

    #[tokio::test]
    async fn builder_without_time_control_builds_context() {
        let harness = CamelTestContext::builder()
            .with_mock()
            .with_timer()
            .with_log()
            .build()
            .await;

        assert!(harness.mock().get_endpoint("result").is_none());
        let guard = harness.ctx().lock().await;
        let _ = &*guard;
    }

    #[tokio::test]
    async fn builder_with_time_control_builds_and_advances_clock() {
        let (_harness, time) = CamelTestContext::builder()
            .with_mock()
            .with_timer()
            .with_time_control()
            .build()
            .await;

        time.advance(Duration::from_millis(1)).await;
        time.resume();
    }

    #[tokio::test]
    async fn stop_is_idempotent_and_shutdown_is_safe() {
        let harness = CamelTestContext::builder().with_mock().build().await;
        harness.stop().await;
        harness.stop().await;
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn add_route_returns_error_for_invalid_step_uri() {
        let harness = CamelTestContext::builder().with_mock().build().await;

        let route = RouteBuilder::from("direct:start")
            .route_id("bad-route")
            .to("not-a-uri")
            .build()
            .unwrap();

        let err = harness.add_route(route).await.expect_err("must fail");
        assert!(err.to_string().contains("Invalid") || err.to_string().contains("invalid"));
    }

    #[tokio::test]
    async fn with_component_registers_custom_component() {
        let harness = CamelTestContext::builder()
            .with_component(camel_component_direct::DirectComponent::new())
            .with_mock()
            .build()
            .await;

        let route = RouteBuilder::from("direct:start")
            .route_id("direct-route")
            .to("mock:out")
            .build()
            .unwrap();

        harness.add_route(route).await.unwrap();
        harness.start().await;
        harness.stop().await;

        // Harness context remains accessible after lifecycle.
        let _guard = harness.ctx().lock().await;
    }

    // ── TST-004: Route lifecycle (start/stop/restart) ─────────────────────────

    #[tokio::test]
    async fn tst004_route_lifecycle_start_stop_restart() {
        let harness = CamelTestContext::builder()
            .with_direct()
            .with_mock()
            .build()
            .await;

        let route = RouteBuilder::from("direct:lifecycle")
            .route_id("lifecycle-route")
            .to("mock:lifecycle-out")
            .build()
            .unwrap();

        harness.add_route(route).await.unwrap();

        // Start the context — routes transition to Started.
        harness.start().await;

        // Stop the context — routes transition to Stopped.
        harness.stop().await;

        // Restart via the underlying CamelContext to verify start-after-stop works.
        {
            let mut ctx = harness.ctx().lock().await;
            ctx.start().await.expect("restart should succeed");
            ctx.stop().await.expect("stop after restart should succeed");
        }
    }

    // ── TST-005: Concurrent exchange processing ───────────────────────────────

    #[tokio::test]
    async fn tst005_concurrent_exchange_processing() {
        use camel_api::{BoxProcessor, BoxProcessorExt, Exchange, Message};
        use camel_core::route::{CompiledStep, PipelineRuntimeCtx, compose_pipeline};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};
        use tower::ServiceExt;

        let counter = Arc::new(AtomicU32::new(0));
        let processor: BoxProcessor = {
            let c = Arc::clone(&counter);
            BoxProcessor::from_fn(move |ex: Exchange| {
                let c = Arc::clone(&c);
                Box::pin(async move {
                    c.fetch_add(1, Ordering::Relaxed);
                    tokio::task::yield_now().await;
                    Ok(ex)
                })
            })
        };

        let pipeline = compose_pipeline(
            vec![CompiledStep::Process {
                processor,
                body_contract: None,
                lifecycle: None,
            }],
            PipelineRuntimeCtx::compile_time(),
        );

        let concurrency: u32 = 10;
        let mut handles = Vec::with_capacity(concurrency as usize);
        for i in 0..concurrency {
            let p = pipeline.clone();
            handles.push(tokio::spawn(async move {
                let ex = Exchange::new(Message::new(format!("msg-{i}")));
                p.oneshot(ex).await.unwrap()
            }));
        }

        for h in handles {
            let _ = h.await.unwrap();
        }

        assert_eq!(counter.load(Ordering::Relaxed), concurrency);
    }

    // ── TST-006: Error handler invocation ─────────────────────────────────────

    #[tokio::test]
    #[allow(deprecated)]
    async fn tst006_error_handler_invoked_on_failure() {
        use camel_api::error_handler::ExceptionPolicy;
        use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, Exchange, Message};
        use camel_processor::ErrorHandlerService;
        use std::sync::Arc;
        use tower::ServiceExt;

        let error_received = Arc::new(std::sync::Mutex::new(false));
        let error_received_clone = Arc::clone(&error_received);

        // Handler processor that records it was called.
        let handler: BoxProcessor = BoxProcessor::from_fn(move |ex: Exchange| {
            let r = Arc::clone(&error_received_clone);
            Box::pin(async move {
                *r.lock().unwrap() = true;
                Ok(ex)
            })
        });

        // Inner processor that always fails.
        let failing: BoxProcessor = BoxProcessor::from_fn(|_| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        });

        let policy = ExceptionPolicy::new(|_| true);
        let svc = ErrorHandlerService::new(failing, Some(handler), vec![(policy, None)]);
        let ex = Exchange::new(Message::new("test"));
        let result = svc.oneshot(ex).await;

        assert!(result.is_ok(), "error handler should absorb the error");
        assert!(
            result.unwrap().has_error(),
            "exchange should have error set"
        );
        assert!(
            *error_received.lock().unwrap(),
            "error handler processor should have been invoked"
        );
    }

    // ── TST-007: Dead letter channel ──────────────────────────────────────────

    #[tokio::test]
    #[allow(deprecated)]
    async fn tst007_dead_letter_channel_receives_failed_exchange() {
        use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, Exchange, Message};
        use camel_processor::ErrorHandlerService;
        use std::sync::Arc;
        use tower::ServiceExt;

        let dlc_received = Arc::new(std::sync::Mutex::new(Vec::<Exchange>::new()));
        let dlc_received_clone = Arc::clone(&dlc_received);

        // DLC processor that captures the exchange.
        let dlc: BoxProcessor = BoxProcessor::from_fn(move |ex: Exchange| {
            let r = Arc::clone(&dlc_received_clone);
            Box::pin(async move {
                r.lock().unwrap().push(ex.clone());
                Ok(ex)
            })
        });

        let failing: BoxProcessor = BoxProcessor::from_fn(|_| {
            Box::pin(async { Err(CamelError::ProcessorError("fail".into())) })
        });

        let svc = ErrorHandlerService::new(failing, Some(dlc), vec![]);
        let ex = Exchange::new(Message::new("dlc-test"));
        let result = svc.oneshot(ex).await;

        assert!(result.is_ok());
        let exchanges = dlc_received.lock().unwrap();
        assert_eq!(
            exchanges.len(),
            1,
            "DLC should have received exactly one exchange"
        );
        assert!(exchanges[0].has_error());
    }

    // ── TST-008: Header propagation across processors ─────────────────────────

    #[tokio::test]
    async fn tst008_header_propagation_across_processors() {
        use camel_api::{Body, BoxProcessor, BoxProcessorExt, Exchange, Message, Value};
        use camel_core::route::{CompiledStep, PipelineRuntimeCtx, compose_pipeline};
        use tower::ServiceExt;

        let step1: BoxProcessor = BoxProcessor::from_fn(|mut ex: Exchange| {
            Box::pin(async move {
                ex.input
                    .set_header("trace-id", Value::String("abc-123".into()));
                Ok(ex)
            })
        });

        let step2: BoxProcessor = BoxProcessor::from_fn(|mut ex: Exchange| {
            Box::pin(async move {
                // Modify body but leave headers intact.
                ex.input.body = Body::Text("processed".to_string());
                Ok(ex)
            })
        });

        let pipeline = compose_pipeline(
            vec![
                CompiledStep::Process {
                    processor: step1,
                    body_contract: None,
                    lifecycle: None,
                },
                CompiledStep::Process {
                    processor: step2,
                    body_contract: None,
                    lifecycle: None,
                },
            ],
            PipelineRuntimeCtx::compile_time(),
        );
        let ex = Exchange::new(Message::new("input"));
        let result = pipeline.oneshot(ex).await.unwrap();

        assert_eq!(
            result.input.header("trace-id"),
            Some(&Value::String("abc-123".into())),
            "header should survive across processors"
        );
        assert_eq!(result.input.body.as_text(), Some("processed"));
    }

    // ── TST-009: Exchange body type conversion ────────────────────────────────

    #[tokio::test]
    async fn tst009_exchange_body_type_conversion() {
        use camel_api::body::Body;
        use camel_api::body_converter::{BodyType, convert};

        // String → Bytes
        let text_body = Body::Text("hello".to_string());
        let bytes_body = convert(text_body, BodyType::Bytes).unwrap();
        assert!(matches!(bytes_body, Body::Bytes(_)));
        if let Body::Bytes(ref b) = bytes_body {
            assert_eq!(b.as_ref(), b"hello");
        }

        // Bytes → String
        let text_body_back = convert(bytes_body, BodyType::Text).unwrap();
        assert!(matches!(text_body_back, Body::Text(_)));
        assert_eq!(text_body_back.as_text(), Some("hello"));
    }

    // ── TST-010: Multicast EIP ────────────────────────────────────────────────

    #[tokio::test]
    async fn tst010_multicast_delivers_to_multiple_endpoints() {
        use camel_api::multicast::{MulticastConfig, MulticastStrategy};
        use camel_api::{BoxProcessor, BoxProcessorExt, Exchange, Message};
        use camel_processor::MulticastService;
        use std::sync::Arc;
        use tower::ServiceExt;

        let received_a = Arc::new(std::sync::Mutex::new(Vec::<Exchange>::new()));
        let received_b = Arc::new(std::sync::Mutex::new(Vec::<Exchange>::new()));

        let endpoint_a: BoxProcessor = {
            let r = Arc::clone(&received_a);
            BoxProcessor::from_fn(move |ex: Exchange| {
                let r = Arc::clone(&r);
                Box::pin(async move {
                    r.lock().unwrap().push(ex.clone());
                    Ok(ex)
                })
            })
        };

        let endpoint_b: BoxProcessor = {
            let r = Arc::clone(&received_b);
            BoxProcessor::from_fn(move |ex: Exchange| {
                let r = Arc::clone(&r);
                Box::pin(async move {
                    r.lock().unwrap().push(ex.clone());
                    Ok(ex)
                })
            })
        };

        let config = MulticastConfig::new().aggregation(MulticastStrategy::LastWins);

        let svc = MulticastService::new(vec![endpoint_a, endpoint_b], config)
            .expect("multicast service creation should succeed");
        let ex = Exchange::new(Message::new("multicast-test"));
        let _result = svc.oneshot(ex).await.unwrap();

        assert_eq!(
            received_a.lock().unwrap().len(),
            1,
            "endpoint A should receive exactly one exchange"
        );
        assert_eq!(
            received_b.lock().unwrap().len(),
            1,
            "endpoint B should receive exactly one exchange"
        );
    }
}
