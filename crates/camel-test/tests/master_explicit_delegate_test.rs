//! rc-w02ns: framework-level contract test for `master:` over an
//! Explicit-mode delegate consumer (`start()` returns after spawning a
//! background task exposed through `background_task_handle()`), as camel-jms
//! and camel-kafka do. No broker: the delegate is an in-test component, so the
//! test pins the framework contract rather than any concrete component.
//!
//! Before the fix, master called the delegate's `stop()` right after
//! `start()` returned, so the delegate never ran and leadership churned.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{BoxProcessor, CamelError, Exchange, Message};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::{
    Component, ComponentBundle, ComponentContext, Consumer, ConsumerContext, ConsumerStartupMode,
    Endpoint, ProducerContext, RuntimeObservability,
};
use camel_component_mock::MockComponent;
use camel_core::CamelContext;
use camel_master::MasterBundle;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Probe {
    starts: AtomicUsize,
    stops: AtomicUsize,
}

struct ExplicitComponent(Arc<Probe>);

impl Component for ExplicitComponent {
    fn scheme(&self) -> &str {
        "explicit"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(ExplicitEndpoint {
            uri: uri.to_string(),
            probe: Arc::clone(&self.0),
        }))
    }
}

struct ExplicitEndpoint {
    uri: String,
    probe: Arc<Probe>,
}

impl Endpoint for ExplicitEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(ExplicitConsumer {
            probe: Arc::clone(&self.probe),
            own_cancel: CancellationToken::new(),
            handle: None,
        }))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::EndpointCreationFailed("not used".to_string()))
    }
}

struct ExplicitConsumer {
    probe: Arc<Probe>,
    own_cancel: CancellationToken,
    handle: Option<JoinHandle<Result<(), CamelError>>>,
}

#[async_trait]
impl Consumer for ExplicitConsumer {
    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }

    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        self.probe.starts.fetch_add(1, Ordering::SeqCst);
        let own_cancel = self.own_cancel.clone();
        // Return at once; work lives in a supervised background task.
        self.handle = Some(tokio::spawn({
            let ctx = ctx.clone();
            async move {
                loop {
                    tokio::select! {
                        _ = ctx.cancelled() => return Ok(()),
                        _ = own_cancel.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(Duration::from_millis(10)) => {
                            ctx.send(Exchange::new(Message::new("tick"))).await?;
                        }
                    }
                }
            }
        }));
        ctx.mark_ready();
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        self.probe.stops.fetch_add(1, Ordering::SeqCst);
        self.own_cancel.cancel();
        Ok(())
    }

    fn background_task_handle(&mut self) -> Option<JoinHandle<Result<(), CamelError>>> {
        self.handle.take()
    }
}

/// Noop platform (always leader): the Explicit delegate must be started once
/// and keep delivering until the context stops; `stop()` runs exactly once,
/// on shutdown.
#[tokio::test(flavor = "multi_thread")]
async fn master_keeps_explicit_delegate_running_until_shutdown() {
    let probe = Arc::new(Probe::default());
    let mock = MockComponent::new();

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
    MasterBundle::from_toml(toml::Value::Table(toml::map::Map::new()))
        .unwrap() // allow-unwrap
        .register_all(&mut ctx);
    ctx.register_component(ExplicitComponent(Arc::clone(&probe)));
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("master:explicit-lock:explicit:feed")
        .route_id("master-explicit")
        .to("mock:out")
        .build()
        .unwrap(); // allow-unwrap
    ctx.add_route_definition(route).await.unwrap(); // allow-unwrap
    ctx.start().await.unwrap(); // allow-unwrap

    // Bounded wait: panics on timeout, so a delegate that was stopped right
    // after start() (the bug) fails here instead of hanging.
    let out = mock.get_endpoint("out").unwrap(); // allow-unwrap
    out.await_exchanges(10, Duration::from_secs(10)).await;

    assert_eq!(probe.starts.load(Ordering::SeqCst), 1, "no restart churn");
    assert_eq!(probe.stops.load(Ordering::SeqCst), 0, "no premature stop");

    ctx.stop().await.unwrap(); // allow-unwrap

    // The delegate's own task runs stop() after cancellation; wait for it
    // (bounded) instead of asserting right after ctx.stop() returns.
    tokio::time::timeout(Duration::from_secs(5), async {
        while probe.stops.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("delegate stop() must run on shutdown");
    assert_eq!(
        probe.stops.load(Ordering::SeqCst),
        1,
        "delegate stopped exactly once on shutdown"
    );
}
