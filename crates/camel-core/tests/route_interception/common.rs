//! Shared helpers for the route-interception suite.

use std::sync::Arc;

use camel_api::{CamelError, Exchange};
use camel_component_direct::DirectComponent;
use camel_component_mock::MockComponent;
use camel_component_seda::SedaComponent;
use camel_core::intercept::{InterceptAction, InterceptRule, InterceptRules};
use camel_core::route::BuilderStep;
use camel_core::{CamelContext, RouteDefinition};
use tower::ServiceExt;

/// Runtime observability stub for `create_producer`.
pub(crate) fn test_rt() -> Arc<dyn camel_component_api::RuntimeObservability> {
    Arc::new(camel_component_api::NoOpComponentContext)
}

/// One valid rule used by every freeze test: `seda:out` skips to `mock:z`.
pub(crate) fn skip_to_mock_z() -> InterceptRules {
    InterceptRules::new(vec![InterceptRule {
        uri: "seda:out".into(),
        action: InterceptAction::SkipTo {
            uri: "mock:z".into(),
        },
    }])
    .expect("valid mock targets")
}

/// Boot a context with the mock/direct/seda components registered and
/// optional interception rules installed at build time. Returns the context
/// plus the mock component handle (clone sharing recorded endpoint state)
/// used for delivery assertions.
pub(crate) async fn boot_context_with_intercept(
    rules: Option<InterceptRules>,
) -> (CamelContext, MockComponent) {
    let mut builder = CamelContext::builder();
    if let Some(rules) = rules {
        builder = builder.with_intercept_rules(rules);
    }
    let mut ctx = builder.build().await.expect("build context");
    let mock = MockComponent::new();
    ctx.register_component(mock.clone());
    ctx.register_component(DirectComponent::new());
    ctx.register_component(SedaComponent::new());
    (ctx, mock)
}

/// Boot a context with no interception configuration at all.
pub(crate) async fn boot_context() -> (CamelContext, MockComponent) {
    boot_context_with_intercept(None).await
}

/// Await budget for the notify-aware mock arrival primitives.
pub(crate) const TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Send an exchange into a `direct:` endpoint and return the raw pipeline
/// result (final exchange or error) — used by tests that assert on error
/// outcomes verbatim.
pub(crate) async fn send_to_direct_result(
    ctx: &CamelContext,
    endpoint_uri: &str,
    exchange: Exchange,
) -> Result<Exchange, CamelError> {
    let component = ctx
        .registry()
        .get("direct")
        .expect("direct component not registered");
    let producer_ctx = ctx.producer_context();
    let endpoint = component
        .create_endpoint(endpoint_uri, ctx)
        .expect("failed to create direct endpoint");
    let producer = endpoint
        .create_producer(test_rt(), &producer_ctx)
        .expect("failed to create direct producer");
    producer.oneshot(exchange).await
}

/// Send an exchange into a `direct:` endpoint and return the pipeline result.
pub(crate) async fn send_to_direct(
    ctx: &CamelContext,
    endpoint_uri: &str,
    exchange: Exchange,
) -> Exchange {
    send_to_direct_result(ctx, endpoint_uri, exchange)
        .await
        .expect("direct call should succeed")
}

pub(crate) fn direct_to_mock_route() -> RouteDefinition {
    RouteDefinition::new("direct:in", vec![BuilderStep::To("mock:out".into())])
        .with_route_id("freeze-after-add")
}
