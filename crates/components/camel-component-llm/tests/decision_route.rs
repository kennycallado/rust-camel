//! Route tests for `llm:decide`.
//!
//! Pipeline: timer → process → llm:decide → (validate | choice) → mock sinks.
//! Waits use `MockEndpointInner::await_exchanges`; no sleeps in test bodies.

use std::time::Duration;

use camel_api::Exchange;
use camel_api::Value;
use camel_api::body::Body;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::ComponentBundle;
use camel_component_llm::LlmBundle;
use camel_component_mock::MockComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;

mod common;

use common::decision::TRIAGE_TOML;

fn register(ctx: &mut CamelContext) {
    let toml_str = format!(
        "default_decision_provider = \"fake\"\n\
         [decision_providers.fake]\n\
         type = \"mock\"\n\
         predicate_probability = 1.0\n\
         {TRIAGE_TOML}"
    );
    let value: toml::Value = toml::from_str(&toml_str).expect("parse toml");
    let bundle = LlmBundle::from_toml(value).expect("bundle");
    bundle.register_all(ctx);
}

#[tokio::test]
async fn decide_simple_threshold_route() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::builder().build().await.expect("context");
    register(&mut ctx);
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:llm-decide-1?period=10&repeatCount=1")
        .route_id("llm-decide-threshold-route")
        .process(|mut ex: Exchange| async move {
            ex.input.body = Body::Text("My invoice was charged twice".into());
            Ok(ex)
        })
        .to("llm:decide?provider=fake&questions=triage")
        .validate("${body.answers.is_billing.meets_threshold} == true")
        .to("mock:billing")
        .build()
        .expect("route");

    ctx.add_route_definition(route).await.expect("add route");
    ctx.start().await.expect("start");

    let billing = mock
        .get_endpoint("billing")
        .expect("billing endpoint exists");
    billing.await_exchanges(1, Duration::from_secs(5)).await;
    ctx.stop().await.expect("stop");

    assert_eq!(
        billing.received_count().await,
        1,
        "billing should receive exactly 1 exchange"
    );
}

#[tokio::test]
async fn decide_choice_route() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::builder().build().await.expect("context");
    register(&mut ctx);
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:llm-decide-2?period=10&repeatCount=1")
        .route_id("llm-decide-choice-route")
        .process(|mut ex: Exchange| async move {
            ex.input.body = Body::Text("My invoice was charged twice".into());
            Ok(ex)
        })
        .to("llm:decide?questions=triage")
        .choice()
        .when(|ex: &Exchange| match &ex.input.body {
            Body::Json(v) => {
                v.pointer("/answers/dept/choice") == Some(&serde_json::json!("billing"))
            }
            _ => false,
        })
        .to("mock:billing")
        .end_when()
        .otherwise()
        .to("mock:other")
        .end_otherwise()
        .end_choice()
        .build()
        .expect("route");

    ctx.add_route_definition(route).await.expect("add route");
    ctx.start().await.expect("start");

    let billing = mock
        .get_endpoint("billing")
        .expect("billing endpoint exists");
    billing.await_exchanges(1, Duration::from_secs(5)).await;
    ctx.stop().await.expect("stop");

    assert_eq!(
        billing.received_count().await,
        1,
        "billing should receive 1 exchange"
    );
    let other = mock.get_endpoint("other").expect("other endpoint exists");
    assert_eq!(
        other.received_count().await,
        0,
        "other should receive 0 exchanges"
    );
}

#[tokio::test]
async fn decide_route_sets_headers() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::builder().build().await.expect("context");
    register(&mut ctx);
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:llm-decide-3?period=10&repeatCount=1")
        .route_id("llm-decide-headers-route")
        .process(|mut ex: Exchange| async move {
            ex.input.body = Body::Text("My invoice was charged twice".into());
            Ok(ex)
        })
        .to("llm:decide?provider=fake&questions=triage")
        .validate("${body.answers.is_billing.meets_threshold} == true")
        .to("mock:billing")
        .build()
        .expect("route");

    ctx.add_route_definition(route).await.expect("add route");
    ctx.start().await.expect("start");

    let billing = mock
        .get_endpoint("billing")
        .expect("billing endpoint exists");
    billing.await_exchanges(1, Duration::from_secs(5)).await;
    ctx.stop().await.expect("stop");

    let exchanges = billing.get_received_exchanges().await;
    assert_eq!(
        exchanges.len(),
        1,
        "billing should receive exactly 1 exchange"
    );
    let headers = &exchanges[0].input.headers;
    assert_eq!(
        headers.get("CamelLlmProvider"),
        Some(&Value::String("fake".into())),
        "provider header should be set to the decision provider name"
    );
    assert_eq!(
        headers.get("CamelLlmUsageAvailable"),
        Some(&Value::Bool(true)),
        "usage available should be true for the mock decision provider"
    );
}
