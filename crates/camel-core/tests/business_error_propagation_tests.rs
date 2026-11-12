//! Async boundary propagation pins for business errors
//! (`openspec/changes/business-error-model`, Task 1.7).
//!
//! Three contracts are pinned:
//!
//! 1. `direct:` propagation — a `CamelError::Business` thrown by a callee
//!    route reaches the caller's doTry catch-by-code clause.
//! 2. `seda:` fire-and-forget (InOnly) — the consumer-side route-level
//!    `on_exceptions` policy owns the failed exchange (`handled_by`), while
//!    the producer exchange completes normally on its own copy, blind to the
//!    consumer-side failure.
//! 3. `seda:` waiting (InOut) — the producer observes the consumer-side
//!    failure because the waiting-delivery contract forwards the pipeline
//!    result back through the reply channel.
//!
//! No production change is expected here: the suite pins existing contracts,
//! so a failure is a bug found, not a feature gap.

use std::sync::Arc;
use std::time::Duration;

use camel_api::error_handler::{ErrorHandlerConfig, ExceptionDisposition};
use camel_api::{
    Body, BoxProcessor, BoxProcessorExt, CamelError, Exchange, Message, OpaqueProcessor,
};
use camel_component_api::RuntimeObservability;
use camel_core::route::{BuilderStep, DoTryCatchClause};
use camel_core::{CamelContext, RouteDefinition};
use camel_processor::CatchMatcher;
use tower::ServiceExt;

const CODE: &str = "ORDER_NOT_FOUND";
const KIND: &str = "order";
const TIMEOUT: Duration = Duration::from_secs(5);

fn rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(camel_component_api::NoOpComponentContext)
}

/// Fresh context with `direct`, `seda` and `mock` registered.
async fn new_context() -> (CamelContext, camel_component_mock::MockComponent) {
    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("context builds");
    ctx.register_component(camel_component_direct::DirectComponent::new());
    ctx.register_component(camel_component_seda::SedaComponent::new());
    let mock = camel_component_mock::MockComponent::new();
    ctx.register_component(mock.clone());
    (ctx, mock)
}

fn direct_not_registered(err: &CamelError) -> bool {
    matches!(
        err,
        CamelError::EndpointCreationFailed(msg)
            if msg.starts_with("direct endpoint '") && msg.ends_with("' not registered")
    )
}

/// Send one exchange to a `direct:` endpoint, retrying while the route
/// consumer's registration task has not yet published the endpoint.
async fn send_direct(
    ctx: &CamelContext,
    uri: &str,
    exchange: Exchange,
) -> Result<Exchange, CamelError> {
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        let component = {
            let registry = ctx.registry();
            registry.get("direct").expect("direct component registered")
        };
        let producer_ctx = ctx.producer_context();
        let endpoint = component
            .create_endpoint(uri, ctx)
            .expect("direct endpoint creation");
        let producer = endpoint
            .create_producer(rt(), &producer_ctx)
            .expect("direct producer creation");

        match producer.oneshot(exchange.clone()).await {
            Err(err) if direct_not_registered(&err) && tokio::time::Instant::now() < deadline => {
                tokio::task::yield_now().await;
            }
            other => return other,
        }
    }
}

/// Raw `seda:` producer for `endpoint_uri` (no intercepted pipeline in play).
fn raw_seda_producer(ctx: &CamelContext, endpoint_uri: &str) -> BoxProcessor {
    let component = {
        let registry = ctx.registry();
        registry.get("seda").expect("seda component registered")
    };
    let producer_ctx = ctx.producer_context();
    let endpoint = component
        .create_endpoint(endpoint_uri, ctx)
        .expect("seda endpoint creation");
    endpoint
        .create_producer(rt(), &producer_ctx)
        .expect("seda producer creation")
}

/// Declarative `throw` raising the canonical business error.
fn throw_order_not_found() -> BuilderStep {
    BuilderStep::DeclarativeThrow {
        code: CODE.to_string(),
        kind: Some(KIND.to_string()),
        message: None,
        data: None,
    }
}

fn set_body(marker: &'static str) -> BuilderStep {
    BuilderStep::Processor(OpaqueProcessor(BoxProcessor::from_fn(
        move |mut ex: Exchange| {
            Box::pin(async move {
                ex.input.body = Body::Text(marker.to_string());
                Ok(ex)
            })
        },
    )))
}

fn catch_by_code(steps: Vec<BuilderStep>) -> DoTryCatchClause {
    DoTryCatchClause {
        matcher: CatchMatcher::ByCode(CODE.to_string()),
        on_when: None,
        disposition: ExceptionDisposition::Handled,
        steps,
    }
}

fn business_code(err: &CamelError) -> Option<&str> {
    match err {
        CamelError::Business { code, .. } => Some(code.as_str()),
        _ => None,
    }
}

/// (1) A callee route's business error crosses the `direct:` boundary and the
/// caller's catch-by-code clause handles it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_propagates_business_error_to_caller() {
    let (mut ctx, _mock) = new_context().await;

    ctx.add_route_definition(
        RouteDefinition::new("direct:inner", vec![throw_order_not_found()])
            .with_route_id("inner-route"),
    )
    .await
    .expect("inner route must compile");

    ctx.add_route_definition(
        RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::DoTry {
                try_steps: vec![BuilderStep::To("direct:inner".into())],
                catches: vec![catch_by_code(vec![set_body("caller-caught")])],
                finally: None,
            }],
        )
        .with_route_id("caller-route"),
    )
    .await
    .expect("caller route must compile");

    ctx.start().await.expect("context start");

    let out = send_direct(
        &ctx,
        "direct:start",
        Exchange::new(Message::new("order-42")),
    )
    .await
    .expect("caller catch clause must handle the callee business error");

    assert_eq!(
        out.input.body.as_text(),
        Some("caller-caught"),
        "caller catch-by-code clause must run; got body {:?}",
        out.input.body
    );
    assert!(
        out.error.is_none(),
        "a handled business error must leave no error on the exchange"
    );

    let _ = ctx.stop().await;
}

/// (2) InOnly `seda:` is fire-and-forget: the consumer route's policy owns the
/// failure and dispatches it to `direct:dlq-shim`; the producer exchange
/// stays intact and successful.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seda_fire_and_forget_producer_blind_consumer_owns() {
    let (mut ctx, mock) = new_context().await;

    let handler = ErrorHandlerConfig::log_only()
        .on_exception(|err| matches!(err, CamelError::Business { code, .. } if code == CODE))
        .handled_by("direct:dlq-shim")
        .handled(true)
        .build();

    ctx.add_route_definition(
        RouteDefinition::new("seda:jobs", vec![throw_order_not_found()])
            .with_route_id("jobs-consumer")
            .with_error_handler(handler),
    )
    .await
    .expect("consumer route must compile");

    ctx.add_route_definition(
        RouteDefinition::new("direct:dlq-shim", vec![BuilderStep::To("mock:dlq".into())])
            .with_route_id("dlq-shim"),
    )
    .await
    .expect("dlq shim route must compile");

    ctx.start().await.expect("context start");

    let producer = raw_seda_producer(&ctx, "seda:jobs");
    let mut producer_ex = producer
        .oneshot(Exchange::new(Message::new("producer-owned")))
        .await
        .expect("InOnly send must complete without the consumer-side error");

    // The producer owns its exchange copy: mutate after the send to prove the
    // consumer-side failure never reaches it.
    producer_ex.input.body = Body::Text("producer-marker".to_string());

    let dlq = mock.get_endpoint("dlq").expect("dlq mock endpoint");
    dlq.await_exchanges(1, TIMEOUT).await;
    dlq.assert_exchange_count(1).await;

    let received = dlq.get_received_exchanges().await;
    let delivered = received.first().expect("dlq shim received one exchange");
    assert_eq!(
        delivered.error.as_ref().and_then(business_code),
        Some(CODE),
        "dlq shim must receive the failed exchange with the business error, got {:?}",
        delivered.error
    );

    assert_eq!(
        producer_ex.input.body.as_text(),
        Some("producer-marker"),
        "producer exchange body must survive the consumer-side failure"
    );
    assert!(
        producer_ex.error.is_none(),
        "fire-and-forget producer exchange must complete without an error"
    );

    let _ = ctx.stop().await;
}

/// (3) InOut `seda:` waits for the consumer: the producer surfaces the
/// consumer-side business failure instead of reporting success.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seda_in_out_delivers_consumer_failure() {
    const INOUT_URI: &str = "seda:jobs-wait?exchangePattern=InOut";

    let (mut ctx, _mock) = new_context().await;

    ctx.add_route_definition(
        RouteDefinition::new(INOUT_URI, vec![throw_order_not_found()])
            .with_route_id("waiting-consumer"),
    )
    .await
    .expect("consumer route must compile");

    ctx.start().await.expect("context start");

    let producer = raw_seda_producer(&ctx, INOUT_URI);
    let err = producer
        .oneshot(Exchange::new(Message::new("order-42")))
        .await
        .expect_err("waiting send must surface the consumer-side failure");

    assert_eq!(
        business_code(&err),
        Some(CODE),
        "producer must observe the consumer-side business error, got {err:?}"
    );

    let _ = ctx.stop().await;
}
