//! End-to-end DSL route test for camel-component-llm.
//!
//! Verifies the full pipeline: RouteBuilder → CamelContext → LlmBundle →
//! LlmComponent → LlmEndpoint → LlmProducer → MockComponent (assertion sink).

use std::time::Duration;

use camel_api::Value;
use camel_api::body::Body;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::ComponentBundle;
use camel_component_llm::LlmBundle;
use camel_component_mock::MockComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;

fn register_llm_bundle(ctx: &mut CamelContext) {
    let toml_str = r#"
[providers.mock]
type = "mock"
response = "echo"
"#;
    let value: toml::Value = toml::from_str(toml_str).expect("parse toml");
    let bundle = LlmBundle::from_toml(value).expect("bundle");
    bundle.register_all(ctx);
}

// ---------------------------------------------------------------------------
// Test 1: Chat materialized — timer → process → llm:chat → mock:result
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dsl_route_chat_materialized_echo_body() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::builder().build().await.expect("context");
    register_llm_bundle(&mut ctx);
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:llm-dsl-1?period=50&repeatCount=1")
        .route_id("llm-dsl-chat-route")
        .process(|mut ex: camel_api::Exchange| async move {
            ex.input.body = Body::Text("Hola LLM".into());
            Ok(ex)
        })
        .to("llm:chat?provider=mock&stream=false")
        .to("mock:result")
        .build()
        .expect("route");

    ctx.add_route_definition(route).await.expect("add route");
    ctx.start().await.expect("start");
    tokio::time::sleep(Duration::from_millis(200)).await;
    ctx.stop().await.expect("stop");

    let endpoint = mock.get_endpoint("result").expect("endpoint exists");
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(
        exchanges[0].input.body.as_text(),
        Some("Hola LLM"),
        "echo provider should return the prompt"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Chat materialized — verify CamelLlm* headers on captured exchange
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dsl_route_chat_sets_llm_headers() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::builder().build().await.expect("context");
    register_llm_bundle(&mut ctx);
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:llm-dsl-2?period=50&repeatCount=1")
        .route_id("llm-dsl-headers-route")
        .process(|mut ex: camel_api::Exchange| async move {
            ex.input.body = Body::Text("test prompt".into());
            Ok(ex)
        })
        .to("llm:chat?provider=mock&stream=false")
        .to("mock:result")
        .build()
        .expect("route");

    ctx.add_route_definition(route).await.expect("add route");
    ctx.start().await.expect("start");
    tokio::time::sleep(Duration::from_millis(200)).await;
    ctx.stop().await.expect("stop");

    let endpoint = mock.get_endpoint("result").expect("endpoint exists");
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    let headers = &exchanges[0].input.headers;

    assert_eq!(
        headers.get("CamelLlmProvider"),
        Some(&Value::String("mock".into())),
        "provider header should be set"
    );
    assert_eq!(
        headers.get("CamelLlmUsageAvailable"),
        Some(&Value::Bool(true)),
        "usage available should be true for materialized mode"
    );
    assert_eq!(
        headers.get("CamelLlmStream"),
        Some(&Value::Bool(false)),
        "stream header should be false"
    );
    assert!(
        headers.contains_key("CamelLlmTokensIn"),
        "token-in header should be set"
    );
    assert!(
        headers.contains_key("CamelLlmTokensOut"),
        "token-out header should be set"
    );
    assert!(
        headers.contains_key("CamelLlmFinishReason"),
        "finish-reason header should be set"
    );
}

// ---------------------------------------------------------------------------
// Test 3: Embed — timer → process → llm:embed → mock:result
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dsl_route_embed_returns_json() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::builder().build().await.expect("context");
    register_llm_bundle(&mut ctx);
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:llm-dsl-3?period=50&repeatCount=1")
        .route_id("llm-dsl-embed-route")
        .process(|mut ex: camel_api::Exchange| async move {
            ex.input.body = Body::Text("embed this".into());
            Ok(ex)
        })
        .to("llm:embed?provider=mock")
        .to("mock:result")
        .build()
        .expect("route");

    ctx.add_route_definition(route).await.expect("add route");
    ctx.start().await.expect("start");
    tokio::time::sleep(Duration::from_millis(200)).await;
    ctx.stop().await.expect("stop");

    let endpoint = mock.get_endpoint("result").expect("endpoint exists");
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    match &exchanges[0].input.body {
        Body::Json(v) => {
            let arr = v.as_array().expect("embeddings should be a JSON array");
            assert_eq!(arr.len(), 1, "should have 1 embedding");
            assert_eq!(
                arr[0].as_array().expect("embedding should be array").len(),
                16,
                "mock embedding should be 16-dimensional"
            );
        }
        other => panic!("expected Json body, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Test 4: Chat streaming — timer → process → llm:chat?stream=true → mock:result
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dsl_route_chat_streaming_sets_stream_body() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::builder().build().await.expect("context");
    register_llm_bundle(&mut ctx);
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:llm-dsl-4?period=50&repeatCount=1")
        .route_id("llm-dsl-stream-route")
        .process(|mut ex: camel_api::Exchange| async move {
            ex.input.body = Body::Text("stream me".into());
            Ok(ex)
        })
        .to("llm:chat?provider=mock&stream=true")
        .to("mock:result")
        .build()
        .expect("route");

    ctx.add_route_definition(route).await.expect("add route");
    ctx.start().await.expect("start");
    tokio::time::sleep(Duration::from_millis(200)).await;
    ctx.stop().await.expect("stop");

    let endpoint = mock.get_endpoint("result").expect("endpoint exists");
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    assert!(
        matches!(exchanges[0].input.body, Body::Stream(_)),
        "streaming mode should produce a Stream body"
    );
    assert_eq!(
        exchanges[0].input.headers.get("CamelLlmStream"),
        Some(&Value::Bool(true)),
        "stream header should be true"
    );
    assert_eq!(
        exchanges[0].input.headers.get("CamelLlmUsageAvailable"),
        Some(&Value::Bool(false)),
        "usage available should be false for streaming mode"
    );
}
