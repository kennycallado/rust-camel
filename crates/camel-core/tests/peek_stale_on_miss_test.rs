//! Integration tests for the `cache_peek_stale` `on_miss` policy
//! (stale-while-revalidate end-to-end).
//!
//! Verifies the full camel-core wiring: a declarative [`RouteDefinition`] with a
//! `cache_peek_stale` step compiles through [`CamelContext`], and an exchange
//! executed against an empty `memory` repository obeys the miss policy:
//! `continue` lets the pipeline proceed, `stop` (default) truncates it.

use std::sync::Arc;

use camel_api::{Exchange, Message, Value};
use camel_component_direct::DirectComponent;
use camel_core::route::{BuilderStep, LanguageExpressionDef, ValueSourceDef};
use camel_core::{CamelContext, RouteDefinition};
use camel_processor::{CAMEL_CACHE_PEEK_HIT, CAMEL_CACHE_PEEK_STALE, PeekStaleMissPolicy};
use tower::ServiceExt;

/// Runtime observability stub for `create_producer`.
fn test_rt() -> Arc<dyn camel_component_api::RuntimeObservability> {
    Arc::new(camel_component_api::NoOpComponentContext)
}

/// A fixed cache key (simple-language string literal).
fn fixed_key() -> LanguageExpressionDef {
    LanguageExpressionDef {
        language: "simple".to_string(),
        source: "swr-key".to_string(),
    }
}

/// A literal `set_body` step.
fn set_body_literal(text: &str) -> BuilderStep {
    BuilderStep::DeclarativeSetBody {
        value: ValueSourceDef::Literal(Value::String(text.to_string())),
    }
}

/// The SWR route: set "orig" → cache_peek_stale (memory, fixed key) → set
/// "PIPELINE-COMPLETE". The trailing marker step only runs when the peek miss
/// does not stop the pipeline.
fn swr_route(on_miss: PeekStaleMissPolicy) -> RouteDefinition {
    RouteDefinition::new(
        "direct:swr",
        vec![
            set_body_literal("orig"),
            BuilderStep::CachePeekStale {
                repository: Some("memory".to_string()),
                key: fixed_key(),
                on_miss,
            },
            set_body_literal("PIPELINE-COMPLETE"),
        ],
    )
    .with_route_id("swr")
}

/// Send an exchange into a `direct:` endpoint and return the pipeline result.
async fn send_to_direct(ctx: &CamelContext, endpoint_uri: &str, exchange: Exchange) -> Exchange {
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
    producer
        .oneshot(exchange)
        .await
        .expect("direct call should succeed")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn swr_route_compiles_and_continues_on_miss() {
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(DirectComponent::new());

    ctx.add_route_definition(swr_route(PeekStaleMissPolicy::Continue))
        .await
        .expect("route must compile");
    ctx.start().await.expect("context start failed");

    let result = send_to_direct(&ctx, "direct:swr", Exchange::new(Message::new("seed"))).await;

    // The MISS did not stop the pipeline: the second set_body ran.
    assert_eq!(result.input.body.as_text(), Some("PIPELINE-COMPLETE"));
    // The peek reported a miss.
    assert_eq!(
        result.property(CAMEL_CACHE_PEEK_HIT),
        Some(&Value::Bool(false))
    );
    // The miss policy did not set peek-stale: `continue` on a miss.
    assert_eq!(
        result.property(CAMEL_CACHE_PEEK_STALE),
        Some(&Value::Bool(false))
    );

    ctx.stop().await.expect("context stop failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn swr_route_default_stop_stops_pipeline() {
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(DirectComponent::new());

    ctx.add_route_definition(swr_route(PeekStaleMissPolicy::Stop))
        .await
        .expect("route must compile");
    ctx.start().await.expect("context start failed");

    let result = send_to_direct(&ctx, "direct:swr", Exchange::new(Message::new("seed"))).await;

    // The default (stop) policy truncated the pipeline before the second
    // set_body; the body remains "orig".
    assert_eq!(result.input.body.as_text(), Some("orig"));

    ctx.stop().await.expect("context stop failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peek_stale_service_receives_policy_from_dsl() {
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(DirectComponent::new());

    let def = swr_route(PeekStaleMissPolicy::Continue);

    // Compile-only (no start, no execution): the declarative CachePeekStale
    // step carries the miss policy threaded from the DSL.
    match &def.steps()[1] {
        BuilderStep::CachePeekStale { on_miss, .. } => {
            assert_eq!(*on_miss, PeekStaleMissPolicy::Continue);
        }
        other => panic!("expected CachePeekStale step, got {other:?}"),
    }

    // Compilation must accept the policy (exercises the CoreCompiler arm).
    ctx.add_route_definition(def)
        .await
        .expect("route must compile");

    ctx.stop().await.expect("context stop failed");
}
