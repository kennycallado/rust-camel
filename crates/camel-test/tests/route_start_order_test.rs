//! rc-xd7tq: routes tied on `startup_order` (the default, 1000) must start in
//! definition order and stop in the reverse order. Before the fix the route
//! registry sorted a `HashMap`, so ties started in a random order per run.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use camel_api::{BoxProcessor, CamelError};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::{
    Component, ComponentContext, Consumer, ConsumerContext, ConsumerStartupMode, Endpoint,
    ProducerContext, RuntimeObservability,
};
use camel_component_mock::MockComponent;
use camel_core::CamelContext;
use tokio::task::JoinHandle;

type Log = Arc<Mutex<Vec<String>>>;

struct OrderedComponent(Log);

impl Component for OrderedComponent {
    fn scheme(&self) -> &str {
        "ordered"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(OrderedEndpoint {
            uri: uri.to_string(),
            log: Arc::clone(&self.0),
        }))
    }
}

struct OrderedEndpoint {
    uri: String,
    log: Log,
}

impl Endpoint for OrderedEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(OrderedConsumer {
            name: self.uri.trim_start_matches("ordered:").to_string(),
            log: Arc::clone(&self.log),
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

struct OrderedConsumer {
    name: String,
    log: Log,
    handle: Option<JoinHandle<Result<(), CamelError>>>,
}

#[async_trait]
impl Consumer for OrderedConsumer {
    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }

    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        self.log
            .lock()
            .unwrap()
            .push(format!("start:{}", self.name)); // allow-unwrap
        // Explicit mode: the route controller calls `stop()` as soon as the
        // background task ends, so park it until the route is cancelled.
        let route_ctx = ctx.clone();
        self.handle = Some(tokio::spawn(async move {
            route_ctx.cancelled().await;
            Ok(())
        }));
        ctx.mark_ready();
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        self.log.lock().unwrap().push(format!("stop:{}", self.name)); // allow-unwrap
        Ok(())
    }

    fn background_task_handle(&mut self) -> Option<JoinHandle<Result<(), CamelError>>> {
        self.handle.take()
    }
}

/// A fresh context per round: every round gets a fresh `HashMap` hasher seed,
/// so a registry that still iterates a `HashMap` fails some round with
/// near-certainty (12 routes -> 1/12! per round to pass by luck).
#[tokio::test(flavor = "multi_thread")]
async fn tied_routes_start_in_definition_order_and_stop_in_reverse() {
    const ROUTES: usize = 12;
    for _round in 0..3 {
        let log: Log = Arc::default();
        let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
        ctx.register_component(OrderedComponent(Arc::clone(&log)));
        ctx.register_component(MockComponent::new());

        for i in 0..ROUTES {
            let route = RouteBuilder::from(&format!("ordered:r{i:02}"))
                .route_id(format!("r{i:02}"))
                .to("mock:out")
                .build()
                .unwrap(); // allow-unwrap
            ctx.add_route_definition(route).await.unwrap(); // allow-unwrap
        }

        ctx.start().await.unwrap(); // allow-unwrap
        ctx.stop().await.unwrap(); // allow-unwrap

        let expected_starts: Vec<String> = (0..ROUTES).map(|i| format!("start:r{i:02}")).collect();
        let expected_stops: Vec<String> =
            (0..ROUTES).rev().map(|i| format!("stop:r{i:02}")).collect();
        let events = log.lock().unwrap().clone(); // allow-unwrap
        let starts: Vec<String> = events
            .iter()
            .filter(|e| e.starts_with("start:"))
            .cloned()
            .collect();
        let stops: Vec<String> = events
            .iter()
            .filter(|e| e.starts_with("stop:"))
            .cloned()
            .collect();
        assert_eq!(
            starts, expected_starts,
            "start order must follow definition order"
        );
        assert_eq!(
            stops, expected_stops,
            "stop order must be the reverse of start"
        );
    }
}
