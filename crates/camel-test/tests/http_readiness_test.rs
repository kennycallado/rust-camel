//! rc-hcica: readiness must not depend on a producer's remote target.
//!
//! Before the fix every `http:` endpoint (including `to("http://remote")`)
//! registered a TCP probe on its URI host:port, so one dead downstream
//! service turned `/readyz` unhealthy and a Kubernetes pod left the Service.

use std::time::Duration;

use camel_api::{HealthSource, HealthStatus};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_direct::DirectComponent;
use camel_component_http::HttpComponent;
use camel_component_mock::MockComponent;
use camel_core::CamelContext;

/// A loopback port nothing listens on.
async fn closed_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // allow-unwrap
    l.local_addr().unwrap().port() // allow-unwrap
}

async fn context() -> CamelContext {
    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
    ctx.register_component(DirectComponent::new());
    ctx.register_component(HttpComponent::new());
    ctx.register_component(MockComponent::new());
    ctx
}

async fn readiness_settles(ctx: &CamelContext, want: HealthStatus) -> HealthStatus {
    // Bounded poll: the consumer's bind completes asynchronously.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let status = ctx.readiness().await;
        if status == want || tokio::time::Instant::now() >= deadline {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn readiness_ignores_dead_http_producer_target() {
    let dead = closed_port().await;
    let mut ctx = context().await;
    let route = RouteBuilder::from("direct:in")
        .route_id("calls-dead-service")
        .to(format!("http://127.0.0.1:{dead}/x?allowInternal=true"))
        .build()
        .unwrap(); // allow-unwrap
    ctx.add_route_definition(route).await.unwrap(); // allow-unwrap
    ctx.start().await.unwrap(); // allow-unwrap

    assert_eq!(
        readiness_settles(&ctx, HealthStatus::Healthy).await,
        HealthStatus::Healthy,
        "a dead producer target must not fail readiness"
    );
    ctx.stop().await.unwrap(); // allow-unwrap
}

#[tokio::test(flavor = "multi_thread")]
async fn readiness_is_healthy_with_own_listener_up_and_dead_producer_target() {
    let dead = closed_port().await;
    let own = closed_port().await;
    let mut ctx = context().await;
    let producer = RouteBuilder::from("direct:in")
        .route_id("calls-dead-service")
        .to(format!("http://127.0.0.1:{dead}/x?allowInternal=true"))
        .build()
        .unwrap(); // allow-unwrap
    let consumer = RouteBuilder::from(&format!("http://127.0.0.1:{own}/ok"))
        .route_id("serves")
        .to("mock:out")
        .build()
        .unwrap(); // allow-unwrap
    ctx.add_route_definition(producer).await.unwrap(); // allow-unwrap
    ctx.add_route_definition(consumer).await.unwrap(); // allow-unwrap
    ctx.start().await.unwrap(); // allow-unwrap

    assert_eq!(
        readiness_settles(&ctx, HealthStatus::Healthy).await,
        HealthStatus::Healthy,
        "own listener is up, dead producer target is ignored"
    );

    ctx.stop().await.unwrap(); // allow-unwrap
}
