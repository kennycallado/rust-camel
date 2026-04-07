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
        build_context(self.registrations, self.mock)
    }
}

impl CamelTestContextBuilder<WithTimeControl> {
    /// Build the harness with time control.
    ///
    /// Calls `tokio::time::pause()` before returning. Use the returned
    /// [`TimeController`] to advance the clock inside the test.
    pub async fn build(self) -> (CamelTestContext, TimeController) {
        tokio::time::pause();
        let ctx = build_context(self.registrations, self.mock);
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

fn build_context(registrations: Vec<Registration>, mock: MockComponent) -> CamelTestContext {
    let mut ctx = CamelContext::new();

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
        ctx.start().await.expect("CamelTestContext: start failed");
    }

    /// Stop all routes explicitly. Safe to call before the harness is dropped —
    /// subsequent drop is a no-op.
    pub async fn stop(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return; // already stopped
        }
        let mut ctx = self.ctx.lock().await;
        ctx.stop().await.expect("CamelTestContext: stop failed");
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
}
