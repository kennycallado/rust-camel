//! Shared helpers for the route-interception suite.

use std::sync::Arc;

use camel_api::{BoxProcessor, CamelError, Exchange, Message};
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

/// True for the SEDA producer's startup-race rejection. `ctx.start()`
/// returns before Immediate-mode consumers mark themselves active, so a
/// send racing that activation observes this error. The gate fires before
/// enqueue, so a retried attempt cannot duplicate the exchange.
fn is_seda_no_active_consumers(err: &CamelError) -> bool {
    matches!(err, CamelError::EndpointCreationFailed(msg) if msg.contains("has no active consumers"))
}

/// Drive `attempt` to completion, retrying only while it fails with the
/// SEDA startup-race rejection, bounded by [`TEST_TIMEOUT`]. The Immediate
/// startup contract never promised consumer readiness at `start()` return,
/// so waiting out the activation race is the correct synchronization here.
pub(crate) async fn send_awaiting_consumers<F, Fut>(what: &str, attempt: F) -> Exchange
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<Exchange, CamelError>>,
{
    let deadline = tokio::time::Instant::now() + TEST_TIMEOUT;
    loop {
        match attempt().await {
            Err(err) if is_seda_no_active_consumers(&err) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "SEDA consumers did not activate within {TEST_TIMEOUT:?}: {err}"
                );
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            result => {
                return result.unwrap_or_else(|err| panic!("{what} should succeed: {err}"));
            }
        }
    }
}

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

/// Create a raw producer for a seda endpoint, bypassing any intercepted
/// pipeline (and therefore any divert copies).
pub(crate) fn raw_seda_producer(ctx: &CamelContext, endpoint_uri: &str) -> BoxProcessor {
    let component = ctx
        .registry()
        .get("seda")
        .expect("seda component not registered");
    let producer_ctx = ctx.producer_context();
    let endpoint = component
        .create_endpoint(endpoint_uri, ctx)
        .expect("failed to create seda endpoint");
    endpoint
        .create_producer(test_rt(), &producer_ctx)
        .expect("failed to create seda producer")
}

/// Establish that a seda endpoint's consumer is active by enqueueing a
/// probe exchange with a distinguishable body through a raw seda producer,
/// retrying only on the startup-race rejection. The probe bypasses the
/// intercepted pipeline, so it triggers no divert copies; its only side
/// effect is the probe exchange itself arriving downstream (FIFO before
/// any payload sent afterwards). Use before sends whose pipeline has side
/// effects preceding the seda enqueue — those sends must not retry.
pub(crate) async fn probe_seda_until_active(
    ctx: &CamelContext,
    endpoint_uri: &str,
    probe_body: &str,
) {
    let producer = raw_seda_producer(ctx, endpoint_uri);
    send_awaiting_consumers("seda readiness probe", || {
        producer
            .clone()
            .oneshot(Exchange::new(Message::new(probe_body.to_string())))
    })
    .await;
}

pub(crate) fn direct_to_mock_route() -> RouteDefinition {
    RouteDefinition::new("direct:in", vec![BuilderStep::To("mock:out".into())])
        .with_route_id("freeze-after-add")
}
