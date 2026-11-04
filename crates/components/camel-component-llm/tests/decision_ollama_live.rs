#![cfg(feature = "decisions")]

//! Live `llm:decide` tests against a local Ollama instance.
//!
//! Requirements:
//!   - Ollama running at http://127.0.0.1:11434
//!   - Model: `tev1:0.8b`
//!
//! Run with:
//!   cargo test -p camel-component-llm --features decisions --test decision_ollama_live -- --ignored
//!
//! Tests:
//!   - `ollama_tev1_decides_triage`: producer call on the triage set; checks
//!     the answer shapes and probability ranges.
//!   - `ollama_tev1_decide_route_threshold`: timer -> process -> llm:decide ->
//!     mock; checks that `meets_threshold` is a boolean.
//!   - `ollama_tev1_decides_json_state`: JSON object state; checks that all
//!     three answer types decode.
//!   - `ollama_tev1_usage_headers`: token, provider, model and usage headers
//!     are set, and no cost header is emitted without pricing.
//!   - `ollama_tev1_min_confidence`: `meets_min_confidence` on choice and score
//!     answers matches the returned confidence against a 0.5 minimum.
//!   - `ollama_unknown_model_is_model_not_found`: an unknown model fails with a
//!     "model not found" error.
//!
//! The tests check the answer shapes, ranges and structure only. They do not
//! assert which answer is correct.

use std::sync::Arc;
use std::time::Duration;

use camel_api::{Body, CamelError, Exchange, Message};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::{
    Component, ComponentBundle, NoOpComponentContext, NoopRuntimeObservability,
    RuntimeObservability,
};
use camel_component_llm::headers::{
    CAMEL_LLM_ESTIMATED_COST_USD, CAMEL_LLM_MODEL, CAMEL_LLM_PROVIDER, CAMEL_LLM_TOKENS_IN,
    CAMEL_LLM_USAGE_AVAILABLE,
};
use camel_component_llm::{LlmBundle, LlmComponent, LlmGlobalConfig};
use camel_component_mock::MockComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;
use serde_json::{Value, json};
use tower::Service;

mod common;

use common::decision::TRIAGE_TOML;

const OLLAMA_BASE_URL: &str = "http://127.0.0.1:11434";
const MODEL: &str = "tev1:0.8b";
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
const INVOICE_TEXT: &str = "My invoice was charged twice, please refund";

/// Extra question set: a choice and a score question, both with a 0.5 minimum confidence.
const CONF_TOML: &str = r#"
[[decision_question_sets.conf.questions]]
name = "dept"
type = "choice"
instructions = "Which department?"
choices = [{ value = "billing", description = "payments" }, { value = "tech" }]
min_confidence = 0.5

[[decision_question_sets.conf.questions]]
name = "urgency"
type = "score"
instructions = "How urgent?"
levels = [{ label = "low" }, { label = "medium" }, { label = "high", description = "outage" }]
min_confidence = 0.5
"#;

/// Full TOML for the local Ollama decision provider, followed by `question_sets`.
/// `allow_internal` must precede the tables, so it is emitted first.
fn config_toml(question_sets: &str) -> String {
    format!(
        "allow_internal = true\n\
         [decision_providers.local]\n\
         type = \"ollama\"\n\
         base_url = \"{OLLAMA_BASE_URL}\"\n\
         default_model = \"{MODEL}\"\n\
         {question_sets}"
    )
}

/// Builds the live config with one Ollama decision provider and `question_sets`.
fn make_component_with(question_sets: &str) -> LlmComponent {
    let config: LlmGlobalConfig =
        toml::from_str(&config_toml(question_sets)).expect("config parses");
    LlmComponent::new(config).expect("component builds")
}

/// Builds the live config: one Ollama decision provider plus the triage set.
fn make_component() -> LlmComponent {
    make_component_with(TRIAGE_TOML)
}

/// Creates an endpoint and producer from `uri`, then calls it with `body`.
async fn run(component: &LlmComponent, uri: &str, body: Body) -> Result<Exchange, CamelError> {
    let endpoint = component
        .create_endpoint(uri, &NoOpComponentContext)
        .expect("endpoint");
    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
    let mut producer = endpoint
        .create_producer(rt, &common::make_producer_ctx())
        .expect("producer");
    let exchange = Exchange::new(Message::new(body));
    tokio::time::timeout(CALL_TIMEOUT, producer.call(exchange))
        .await
        .expect("producer call timed out")
}

fn json_body(exchange: &Exchange) -> Value {
    match &exchange.input.body {
        Body::Json(value) => value.clone(),
        other => panic!("expected a JSON answers body, got {other:?}"),
    }
}

/// Asserts that `value` is a number in `[0, 1]`.
fn assert_unit_interval(value: &Value, what: &str) {
    let n = value
        .as_f64()
        .unwrap_or_else(|| panic!("{what} is not a number: {value:?}"));
    assert!((0.0..=1.0).contains(&n), "{what} is outside [0, 1]: {n}");
}

/// Asserts the shape of the triage answers: predicate, choice and score.
fn assert_triage_answers(answers: &Value) {
    let is_billing = &answers["is_billing"];
    assert_eq!(is_billing["type"], "predicate", "is_billing: {is_billing}");
    assert_unit_interval(&is_billing["probability"], "is_billing.probability");

    let dept = &answers["dept"];
    assert_eq!(dept["type"], "choice", "dept: {dept}");
    assert_unit_interval(&dept["confidence"], "dept.confidence");
    for (value, p) in dept["probabilities"]
        .as_object()
        .expect("dept.probabilities is an object")
    {
        assert_unit_interval(p, &format!("dept.probabilities.{value}"));
    }

    let urgency = &answers["urgency"];
    assert_eq!(urgency["type"], "score", "urgency: {urgency}");
    assert_unit_interval(&urgency["confidence"], "urgency.confidence");
    for entry in urgency["probabilities"]
        .as_array()
        .expect("urgency.probabilities is an array")
    {
        assert_unit_interval(&entry["probability"], "urgency.probabilities[].probability");
    }
}

/// Registers the Ollama triage config as a bundle on `ctx`.
fn register(ctx: &mut CamelContext) {
    let value: toml::Value = toml::from_str(&config_toml(TRIAGE_TOML)).expect("parse toml");
    let bundle = LlmBundle::from_toml(value).expect("bundle");
    bundle.register_all(ctx);
}

#[tokio::test]
#[ignore = "slow test: requires local Ollama with tev1:0.8b"]
async fn ollama_tev1_decides_triage() {
    let component = make_component();
    let exchange = run(
        &component,
        "llm:decide?provider=local&questions=triage",
        Body::Text(INVOICE_TEXT.into()),
    )
    .await
    .expect("decide succeeds");

    let body = json_body(&exchange);
    assert_triage_answers(&body["answers"]);
}

#[tokio::test]
#[ignore = "slow test: requires local Ollama with tev1:0.8b"]
async fn ollama_tev1_decide_route_threshold() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::builder().build().await.expect("context");
    register(&mut ctx);
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("timer:llm-decide-live?period=10&repeatCount=1")
        .route_id("llm-decide-live-threshold-route")
        .process(|mut ex: Exchange| async move {
            ex.input.body = Body::Text(INVOICE_TEXT.into());
            Ok(ex)
        })
        .to("llm:decide?provider=local&questions=triage")
        .to("mock:out")
        .build()
        .expect("route");

    ctx.add_route_definition(route).await.expect("add route");
    ctx.start().await.expect("start");

    let out = mock.get_endpoint("out").expect("out endpoint exists");
    out.await_exchanges(1, CALL_TIMEOUT).await;
    ctx.stop().await.expect("stop");

    let exchanges = out.get_received_exchanges().await;
    assert_eq!(
        exchanges.len(),
        1,
        "mock:out should receive exactly 1 exchange"
    );
    let body = json_body(&exchanges[0]);
    let is_billing = &body["answers"]["is_billing"];
    assert!(
        is_billing["meets_threshold"].is_boolean(),
        "meets_threshold must be a JSON boolean: {is_billing}"
    );
    assert_unit_interval(&is_billing["probability"], "is_billing.probability");
}

#[tokio::test]
#[ignore = "slow test: requires local Ollama with tev1:0.8b"]
async fn ollama_tev1_decides_json_state() {
    let component = make_component();
    let state = json!({"ticket": "Invoice charged twice", "customer": "acme"});
    let exchange = run(
        &component,
        "llm:decide?provider=local&questions=triage",
        Body::Json(state),
    )
    .await
    .expect("decide succeeds");

    let body = json_body(&exchange);
    assert_triage_answers(&body["answers"]);
}

#[tokio::test]
#[ignore = "slow test: requires local Ollama with tev1:0.8b"]
async fn ollama_tev1_usage_headers() {
    let component = make_component();
    let exchange = run(
        &component,
        "llm:decide?provider=local&questions=triage",
        Body::Text(INVOICE_TEXT.into()),
    )
    .await
    .expect("decide succeeds");

    let headers = &exchange.input.headers;
    let tokens_in = headers
        .get(CAMEL_LLM_TOKENS_IN)
        .and_then(Value::as_u64)
        .expect("CamelLlmTokensIn must be a number");
    assert!(
        tokens_in > 0,
        "CamelLlmTokensIn must be > 0, got {tokens_in}"
    );
    assert_eq!(
        headers.get(CAMEL_LLM_PROVIDER),
        Some(&json!("local")),
        "provider header"
    );
    let model = headers
        .get(CAMEL_LLM_MODEL)
        .and_then(Value::as_str)
        .expect("CamelLlmModel must be a string");
    assert!(!model.is_empty(), "CamelLlmModel must not be empty");
    assert_eq!(
        headers.get(CAMEL_LLM_USAGE_AVAILABLE),
        Some(&json!(true)),
        "usage available"
    );
    assert!(
        !headers.contains_key(CAMEL_LLM_ESTIMATED_COST_USD),
        "no cost header without pricing configured"
    );
}

#[tokio::test]
#[ignore = "slow test: requires local Ollama with tev1:0.8b"]
async fn ollama_tev1_min_confidence() {
    let component = make_component_with(&format!("{TRIAGE_TOML}{CONF_TOML}"));
    let exchange = run(
        &component,
        "llm:decide?provider=local&questions=conf",
        Body::Text(INVOICE_TEXT.into()),
    )
    .await
    .expect("decide succeeds");

    let body = json_body(&exchange);
    let answers = &body["answers"];

    let dept = &answers["dept"];
    let dept_confidence = dept["confidence"].as_f64().expect("dept.confidence");
    assert_unit_interval(&dept["confidence"], "dept.confidence");
    let dept_meets = dept["meets_min_confidence"]
        .as_bool()
        .expect("dept.meets_min_confidence must be a JSON boolean");
    assert_eq!(
        dept_meets,
        dept_confidence >= 0.5,
        "dept.meets_min_confidence must match confidence {dept_confidence} >= 0.5"
    );

    let urgency = &answers["urgency"];
    let urgency_confidence = urgency["confidence"].as_f64().expect("urgency.confidence");
    assert_unit_interval(&urgency["confidence"], "urgency.confidence");
    let urgency_meets = urgency["meets_min_confidence"]
        .as_bool()
        .expect("urgency.meets_min_confidence must be a JSON boolean");
    assert_eq!(
        urgency_meets,
        urgency_confidence >= 0.5,
        "urgency.meets_min_confidence must match confidence {urgency_confidence} >= 0.5"
    );
}

#[tokio::test]
#[ignore = "slow test: requires local Ollama with tev1:0.8b"]
async fn ollama_unknown_model_is_model_not_found() {
    let component = make_component();
    let err = match run(
        &component,
        "llm:decide?provider=local&questions=triage&model=no-such-model-xyz",
        Body::Text(INVOICE_TEXT.into()),
    )
    .await
    {
        Ok(_) => panic!("unknown model must fail"),
        Err(e) => e,
    };
    let text = err.to_string();
    assert!(
        text.contains("model not found"),
        "error must mention 'model not found', got: {text}"
    );
}
