#![cfg(feature = "decisions")]

//! HTTP codec, status mapping, and fail-fast tests for `llm:decide`.
//!
//! Every provider call goes to a wiremock server bound on loopback. No sleeps:
//! each producer or provider await is bounded by `tokio::time::timeout`.

use std::sync::Arc;
use std::time::Duration;

use camel_api::{Body, CamelError, Exchange, Message};
use camel_component_api::{
    Component, NoOpComponentContext, NoopRuntimeObservability, RuntimeObservability,
};
use camel_component_llm::{
    DecisionAnswer, DecisionQuestion, DecisionRequest, DecisionState, LlmComponent, LlmError,
    LlmGlobalConfig,
};
use serde_json::{Value, json};
use tower::Service;
use wiremock::matchers::method;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

use common::decision::OPENAI_TRIAGE_RESPONSE;
use common::decision::SYSTEM_ONE_TRIAGE_RESPONSE;

const TIMEOUT: Duration = Duration::from_secs(10);

/// Builds a component from provider tables plus the triage question set.
/// `allow_internal` must precede every table, so it is emitted first.
fn component(providers_toml: &str) -> LlmComponent {
    let toml_str = format!(
        "allow_internal = true\n{providers_toml}{}",
        common::decision::TRIAGE_TOML
    );
    let config: LlmGlobalConfig = toml::from_str(&toml_str).expect("config parses");
    LlmComponent::new(config).expect("component builds")
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
    tokio::time::timeout(TIMEOUT, producer.call(exchange))
        .await
        .expect("producer call timed out")
}

/// The triage question set, as the component resolved it from config.
fn triage_questions(component: &LlmComponent) -> Arc<[DecisionQuestion]> {
    component
        .config()
        .decision_question_sets
        .get("triage")
        .expect("triage set")
        .to_questions()
}

fn json_body(exchange: &Exchange) -> Value {
    match &exchange.input.body {
        Body::Json(value) => value.clone(),
        other => panic!("expected a JSON answers body, got {other:?}"),
    }
}

fn openai_toml(name: &str, base_url: &str) -> String {
    format!(
        "[decision_providers.{name}]\ntype = \"openai\"\napi_key = \"sk-test\"\n\
         default_model = \"gpt-6-luna\"\nbase_url = \"{base_url}\"\n"
    )
}

fn typesafe_toml(name: &str, base_url: &str) -> String {
    format!(
        "[decision_providers.{name}]\ntype = \"typesafe\"\napi_key = \"sk-test\"\n\
         default_model = \"tev1:0.8b\"\nbase_url = \"{base_url}\"\n"
    )
}

fn ollama_toml(name: &str, base_url: &str) -> String {
    format!(
        "[decision_providers.{name}]\ntype = \"ollama\"\n\
         default_model = \"tev1:0.8b\"\nbase_url = \"{base_url}\"\n"
    )
}

/// Mounts a POST responder for `route` on `server`.
async fn mount(server: &MockServer, route: &str, template: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path(route))
        .respond_with(template)
        .mount(server)
        .await;
}

/// Mounts a 200 JSON responder for `route` on `server`.
async fn mount_json(server: &MockServer, route: &str, body: &str) {
    mount(
        server,
        route,
        ResponseTemplate::new(200).set_body_raw(body, "application/json"),
    )
    .await;
}

#[tokio::test]
async fn same_set_on_every_http_codec() {
    let server_a = MockServer::start().await;
    let server_b = MockServer::start().await;
    mount_json(&server_a, "/decisions", OPENAI_TRIAGE_RESPONSE).await;
    mount_json(&server_b, "/v1/systemone", SYSTEM_ONE_TRIAGE_RESPONSE).await;

    let providers = format!(
        "{}{}{}",
        openai_toml("oai", &server_a.uri()),
        typesafe_toml("ts", &server_b.uri()),
        ollama_toml("local", &server_b.uri()),
    );
    let component = component(&providers);

    for provider in ["oai", "ts", "local"] {
        let uri = format!("llm:decide?provider={provider}&questions=triage");
        let exchange = run(&component, &uri, Body::Text("hello".into()))
            .await
            .expect("decide succeeds");
        let body = json_body(&exchange);
        assert_eq!(
            body["answers"]["is_billing"]["type"], "predicate",
            "provider {provider}"
        );
        assert_eq!(
            body["answers"]["dept"]["choice"], "billing",
            "provider {provider}"
        );
        assert_eq!(
            body["answers"]["urgency"]["type"], "score",
            "provider {provider}"
        );
    }
}

#[tokio::test]
async fn openai_refusal_per_question() {
    let server = MockServer::start().await;
    let mut response: Value = serde_json::from_str(OPENAI_TRIAGE_RESPONSE).expect("fixture parses");
    // answers[1] is the `dept` answer in the fixture.
    response["answers"][1] = json!({"type": "refusal", "name": "dept"});
    mount_json(&server, "/decisions", &response.to_string()).await;

    let component = component(&openai_toml("oai", &server.uri()));
    let exchange = run(
        &component,
        "llm:decide?provider=oai&questions=triage",
        Body::Text("hello".into()),
    )
    .await
    .expect("decide succeeds");

    let body = json_body(&exchange);
    assert_eq!(body["answers"]["dept"], json!({"type": "refusal"}));
    assert_eq!(body["answers"]["is_billing"]["type"], "predicate");
    assert_eq!(body["answers"]["urgency"]["type"], "score");
}

#[tokio::test]
async fn system_one_json_string_body_is_text_state() {
    let server = MockServer::start().await;
    mount_json(&server, "/v1/systemone", SYSTEM_ONE_TRIAGE_RESPONSE).await;

    let component = component(&typesafe_toml("ts", &server.uri()));
    run(
        &component,
        "llm:decide?provider=ts&questions=triage",
        Body::Json(json!("hello")),
    )
    .await
    .expect("decide succeeds");

    let received = server.received_requests().await.unwrap_or_default();
    assert_eq!(received.len(), 1, "exactly one provider request");
    let request: Value = serde_json::from_slice(&received[0].body).expect("request is JSON");
    assert_eq!(request["state"], "hello");
}

#[tokio::test]
async fn openai_json_scalar_array_no_io() {
    let server = MockServer::start().await;
    let component = component(&openai_toml("oai", &server.uri()));

    let result = run(
        &component,
        "llm:decide?provider=oai&questions=triage",
        Body::Json(json!([1, 2])),
    )
    .await;

    assert!(result.is_err(), "scalar array state must be rejected");
    let received = server.received_requests().await.unwrap_or_default();
    assert!(received.is_empty(), "no request may reach the provider");
}

#[tokio::test]
async fn status_table() {
    type Check = fn(&LlmError) -> bool;
    let rows: [(u16, &str, Check); 11] = [
        (401, "", |e| matches!(e, LlmError::AuthFailed { .. })),
        (403, "", |e| matches!(e, LlmError::AuthFailed { .. })),
        (404, "", |e| matches!(e, LlmError::ModelNotFound(_))),
        (400, "", |e| matches!(e, LlmError::InvalidRequest(_))),
        (413, "", |e| matches!(e, LlmError::InvalidRequest(_))),
        (415, "", |e| matches!(e, LlmError::InvalidRequest(_))),
        (422, "", |e| matches!(e, LlmError::InvalidRequest(_))),
        (429, r#"{"error":{"code":"insufficient_quota"}}"#, |e| {
            matches!(e, LlmError::QuotaExceeded { .. })
        }),
        (429, "", |e| matches!(e, LlmError::RateLimit { .. })),
        (500, "", |e| matches!(e, LlmError::ProviderUnavailable(_))),
        (529, "", |e| matches!(e, LlmError::ProviderUnavailable(_))),
    ];

    for (status, body, check) in rows {
        let server = MockServer::start().await;
        mount(
            &server,
            "/v1/systemone",
            ResponseTemplate::new(status).set_body_raw(body, "application/json"),
        )
        .await;

        let component = component(&typesafe_toml("ts", &server.uri()));
        let provider = component
            .decision_providers()
            .get("ts")
            .expect("ts provider");
        let request = DecisionRequest {
            model: "m".into(),
            state: DecisionState::Text("x".into()),
            questions: triage_questions(&component),
        };
        let err = tokio::time::timeout(TIMEOUT, provider.decide(request))
            .await
            .expect("decide timed out")
            .expect_err("error status must fail");

        assert!(
            check(&err),
            "status {status} with body {body:?}: got {err:?}"
        );
    }
}

#[tokio::test]
async fn auth_failure_redacted() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/decisions",
        ResponseTemplate::new(401).set_body_string(
            r#"{"error":{"message":"Incorrect API key provided: sk-secret-123"}}"#, // allow-secret: provider echoes the key
        ),
    )
    .await;

    let providers = format!(
        "[decision_providers.oai]\ntype = \"openai\"\napi_key = \"sk-secret-123\"\n\
         default_model = \"gpt-6-luna\"\nbase_url = \"{}\"\n",
        server.uri()
    );
    let component = component(&providers);

    let err = match run(
        &component,
        "llm:decide?provider=oai&questions=triage",
        Body::Text("hello".into()),
    )
    .await
    {
        Ok(_) => panic!("401 must fail"),
        Err(e) => e,
    };

    assert!(matches!(err, CamelError::Unauthenticated(_)), "got {err:?}");
    assert!(
        !err.to_string().contains("sk-secret-123"),
        "error text leaks the key: {err}"
    );
    assert!(
        !format!("{:?}", component.config().decision_providers).contains("sk-secret-123"),
        "config Debug leaks the key"
    );
}

#[tokio::test]
async fn redirect_not_followed() {
    let server_a = MockServer::start().await;
    let server_b = MockServer::start().await;
    mount(
        &server_a,
        "/v1/systemone",
        ResponseTemplate::new(302)
            .insert_header("Location", format!("{}/v1/systemone", server_b.uri())),
    )
    .await;
    mount_json(&server_b, "/v1/systemone", SYSTEM_ONE_TRIAGE_RESPONSE).await;

    let component = component(&typesafe_toml("ts", &server_a.uri()));
    let result = run(
        &component,
        "llm:decide?provider=ts&questions=triage",
        Body::Text("hello".into()),
    )
    .await;

    assert!(result.is_err(), "a 302 must not produce answers");
    let sent = server_a.received_requests().await.unwrap_or_default();
    assert_eq!(
        sent.len(),
        1,
        "the request must reach the redirecting server"
    );
    let received = server_b.received_requests().await.unwrap_or_default();
    assert!(received.is_empty(), "redirect target must not be contacted");
}

#[tokio::test]
async fn decide_returns_variants_in_question_order() {
    let server_a = MockServer::start().await;
    let server_b = MockServer::start().await;
    mount_json(&server_a, "/decisions", OPENAI_TRIAGE_RESPONSE).await;
    mount_json(&server_b, "/v1/systemone", SYSTEM_ONE_TRIAGE_RESPONSE).await;

    let providers = format!(
        "{}{}{}",
        openai_toml("oai", &server_a.uri()),
        typesafe_toml("ts", &server_b.uri()),
        ollama_toml("local", &server_b.uri()),
    );
    let component = component(&providers);

    for name in ["oai", "ts", "local"] {
        let provider = component.decision_providers().get(name).expect("provider");
        let request = DecisionRequest {
            model: "m".into(),
            state: DecisionState::Text("hello".into()),
            questions: triage_questions(&component),
        };
        let response = tokio::time::timeout(TIMEOUT, provider.decide(request))
            .await
            .expect("decide timed out")
            .expect("decide succeeds");

        let names: Vec<&str> = response.answers.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["is_billing", "dept", "urgency"], "provider {name}");
        assert!(
            matches!(response.answers[0].answer, DecisionAnswer::Predicate { .. }),
            "provider {name}: is_billing"
        );
        assert!(
            matches!(response.answers[1].answer, DecisionAnswer::Choice { .. }),
            "provider {name}: dept"
        );
        assert!(
            matches!(response.answers[2].answer, DecisionAnswer::Score { .. }),
            "provider {name}: urgency"
        );
    }
}

#[tokio::test]
async fn ollama_cap_exceeded_no_io() {
    let server = MockServer::start().await;

    let big: String = (0..65)
        .map(|i| {
            format!(
                "[[decision_question_sets.big.questions]]\n\
                 name = \"q{i}\"\ntype = \"predicate\"\ninstructions = \"Question {i}?\"\n"
            )
        })
        .collect();
    let providers = format!("{}{big}", ollama_toml("local", &server.uri()));
    let component = component(&providers);

    let err = match component.create_endpoint(
        "llm:decide?provider=local&questions=big",
        &NoOpComponentContext,
    ) {
        Ok(_) => panic!("65 questions must exceed the Ollama cap"),
        Err(e) => e,
    };

    assert!(err.to_string().contains("64"), "error was: {err}");
    let received = server.received_requests().await.unwrap_or_default();
    assert!(received.is_empty(), "no request may reach the provider");
}
