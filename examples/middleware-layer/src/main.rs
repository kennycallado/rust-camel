//! # middleware-layer
//!
//! Demonstrates the three Rust-camel middleware hooks:
//!
//! - `to_layered` wraps the producer resolved for one URI (`TimeoutLayer`).
//! - `layered` wraps a route section (`ConcurrencyLimitLayer`).
//! - `route_layer` wraps the whole route (a custom `ProcessorLayer` built from
//!   `RateLimit` + `Buffer`).
//!
//! A timer route feeds three `direct:` routes, one per granularity, and the
//! program prints how many exchanges each mock endpoint received.

use std::sync::Arc;
use std::time::Duration;

use camel_api::{BoxProcessor, CamelError, CamelErrorAdapter, ProcessorLayer, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_direct::DirectComponent;
use camel_component_mock::{MockComponent, MockEndpointInner};
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_core::route::RouteDefinition;
use tower::buffer::Buffer;
use tower::limit::rate::Rate;
use tower::limit::{ConcurrencyLimitLayer, RateLimit};
use tower::timeout::TimeoutLayer;

/// Whole-route middleware built by hand.
///
/// `tower::limit::RateLimit` is not `Clone`, so it cannot be wrapped by the
/// blanket `ProcessorLayer` impl. `Buffer` is placed OUTSIDE `RateLimit`: its
/// handle is cloneable, and every clone shares the single worker that owns the
/// rate state. `CamelErrorAdapter` maps the buffer's `BoxError` back to
/// `CamelError`.
// ANCHOR: route-layer
#[derive(Clone)]
struct RateLimitMiddleware {
    rate: u64,
    period: Duration,
}

impl ProcessorLayer for RateLimitMiddleware {
    fn wrap(&self, inner: BoxProcessor) -> Result<BoxProcessor, CamelError> {
        let limited = RateLimit::new(inner, Rate::new(self.rate, self.period));
        let buffered = Buffer::new(limited, 16);
        Ok(BoxProcessor::new(CamelErrorAdapter::new(buffered)))
    }
}

fn rate_limited_route() -> Result<RouteDefinition, CamelError> {
    RouteBuilder::from("direct:rate")
        .route_id("rate-limited")
        .to("mock:rate")
        .route_layer(RateLimitMiddleware {
            rate: 2,
            period: Duration::from_millis(100),
        })
        .build()
}
// ANCHOR_END: route-layer

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await?;
    let mock = MockComponent::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(DirectComponent::new());
    ctx.register_component(mock.clone());

    // One timer exchange visits the three routes below in sequence.
    let trigger = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
        .route_id("middleware-trigger")
        .set_body("tick")
        .to("direct:producer")
        .to("direct:section")
        .to("direct:rate")
        .build()?;

    // ANCHOR: layered-to
    let producer_route = RouteBuilder::from("direct:producer")
        .route_id("layered-producer")
        .to_layered(
            "mock:producer",
            TimeoutLayer::new(Duration::from_millis(250)),
        )
        .build()?;
    // ANCHOR_END: layered-to

    // ANCHOR: section
    let section_route = RouteBuilder::from("direct:section")
        .route_id("layered-section")
        .layered(ConcurrencyLimitLayer::new(2), |section| {
            section
                .to("mock:section-a")
                .set_header("stage", Value::String("section".into()))
                .to("mock:section-b")
        })
        .build()?;
    // ANCHOR_END: section

    let rate_route = rate_limited_route()?;

    ctx.add_route_definition(trigger).await?;
    ctx.add_route_definition(producer_route).await?;
    ctx.add_route_definition(section_route).await?;
    ctx.add_route_definition(rate_route).await?;
    ctx.start().await?;

    for name in ["producer", "section-a", "section-b", "rate"] {
        let endpoint = mock_endpoint(&mock, name)?;
        endpoint.await_exchanges(3, Duration::from_secs(5)).await;
        println!(
            "mock:{name} received {} exchanges",
            endpoint.received_count().await
        );
    }

    ctx.stop().await?;
    Ok(())
}

fn mock_endpoint(mock: &MockComponent, name: &str) -> Result<Arc<MockEndpointInner>, CamelError> {
    mock.get_endpoint(name).ok_or_else(|| {
        CamelError::ProcessorError(format!("mock endpoint '{name}' was not created"))
    })
}
