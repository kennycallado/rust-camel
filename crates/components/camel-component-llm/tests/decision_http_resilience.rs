#![cfg(feature = "decisions")]

//! HTTP retry, timeout, and cancellation tests for `llm:decide`.
//!
//! Every provider call goes to a wiremock server bound on loopback. No sleeps:
//! each producer await is bounded by `tokio::time::timeout`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use camel_api::{Body, CamelError, Exchange, Message};
use camel_component_api::{
    Component, NoOpComponentContext, NoopRuntimeObservability, RuntimeObservability,
};
use camel_component_llm::{LlmComponent, LlmGlobalConfig};
use tower::Service;
use wiremock::matchers::method;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

use common::decision::SYSTEM_ONE_TRIAGE_RESPONSE;

const TIMEOUT: Duration = Duration::from_secs(10);

/// Retry policy shared by the retry and fail-fast tests.
const RETRY_TOML: &str = "network_retry = { enabled = true, max_attempts = 3, initial_delay_ms = 10, max_delay_ms = 50 }\n";

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

/// Mounts a POST responder for `route` on `server`.
async fn mount(server: &MockServer, route: &str, template: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path(route))
        .respond_with(template)
        .mount(server)
        .await;
}

/// Mounts a POST responder for `route` that answers exactly one request.
/// Mounted before a fallback responder, it is used once and then skipped.
async fn mount_once(server: &MockServer, route: &str, template: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path(route))
        .respond_with(template)
        .up_to_n_times(1)
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
async fn retry_after_then_success() {
    let server = MockServer::start().await;
    mount_once(
        &server,
        "/v1/systemone",
        ResponseTemplate::new(429).insert_header("Retry-After", "1"),
    )
    .await;
    mount_json(&server, "/v1/systemone", SYSTEM_ONE_TRIAGE_RESPONSE).await;

    let providers = format!("{}{RETRY_TOML}", typesafe_toml("ts", &server.uri()));
    let component = component(&providers);

    let t0 = Instant::now();
    run(
        &component,
        "llm:decide?provider=ts&questions=triage",
        Body::Text("hello".into()),
    )
    .await
    .expect("throttled call succeeds after retry");

    assert!(
        t0.elapsed() >= Duration::from_secs(1),
        "Retry-After: 1 must delay the retry, elapsed {:?}",
        t0.elapsed()
    );
    let received = server
        .received_requests()
        .await
        .expect("request recording enabled");
    assert_eq!(received.len(), 2, "one throttled attempt, one success");
}

#[tokio::test]
async fn quota_exhaustion_no_retry() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/decisions",
        ResponseTemplate::new(429).set_body_raw(
            r#"{"error":{"code":"insufficient_quota"}}"#,
            "application/json",
        ),
    )
    .await;

    let providers = format!("{}{RETRY_TOML}", openai_toml("oai", &server.uri()));
    let component = component(&providers);

    let err = match run(
        &component,
        "llm:decide?provider=oai&questions=triage",
        Body::Text("hello".into()),
    )
    .await
    {
        Ok(_) => panic!("quota exhaustion must fail"),
        Err(e) => e,
    };

    assert!(
        err.to_string().contains("quota exceeded"),
        "error was: {err}"
    );
    let received = server
        .received_requests()
        .await
        .expect("request recording enabled");
    assert_eq!(received.len(), 1, "quota exhaustion must not be retried");
}

#[tokio::test]
async fn timeout_fires() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/v1/systemone",
        ResponseTemplate::new(200)
            .set_body_raw(SYSTEM_ONE_TRIAGE_RESPONSE, "application/json")
            .set_delay(Duration::from_secs(5)),
    )
    .await;

    let providers = format!("{}timeout_secs = 1\n", typesafe_toml("ts", &server.uri()));
    let component = component(&providers);

    let t0 = Instant::now();
    let err = match run(
        &component,
        "llm:decide?provider=ts&questions=triage",
        Body::Text("hello".into()),
    )
    .await
    {
        Ok(_) => panic!("a 5s response must miss a 1s timeout"),
        Err(e) => e,
    };

    assert!(err.to_string().contains("timeout"), "error was: {err}");
    assert!(
        t0.elapsed() < Duration::from_secs(4),
        "timeout must fire near 1s, elapsed {:?}",
        t0.elapsed()
    );
}

#[tokio::test]
async fn cancellation_releases_permit() {
    let server = MockServer::start().await;
    mount_once(
        &server,
        "/v1/systemone",
        ResponseTemplate::new(200)
            .set_body_raw(SYSTEM_ONE_TRIAGE_RESPONSE, "application/json")
            .set_delay(Duration::from_secs(10)),
    )
    .await;
    mount_json(&server, "/v1/systemone", SYSTEM_ONE_TRIAGE_RESPONSE).await;

    let providers = format!(
        "{}max_concurrency = 1\n",
        typesafe_toml("ts", &server.uri())
    );
    let component = component(&providers);

    let endpoint = component
        .create_endpoint(
            "llm:decide?provider=ts&questions=triage",
            &NoOpComponentContext,
        )
        .expect("endpoint");
    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
    let mut p1 = endpoint
        .create_producer(rt, &common::make_producer_ctx())
        .expect("producer");
    let mut p2 = p1.clone();

    let ex1 = Exchange::new(Message::new(Body::Text("first".into())));
    let ex2 = Exchange::new(Message::new(Body::Text("second".into())));

    // The first call holds the only permit while its response is delayed.
    // Wait (bounded) until its request is in flight at the server, so the
    // permit is provably held, then abort it: dropping the call must release
    // the permit.
    let first = tokio::spawn(async move { p1.call(ex1).await });
    let mut ticks = tokio::time::interval(Duration::from_millis(10));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            ticks.tick().await;
            let received = server
                .received_requests()
                .await
                .expect("request recording enabled");
            if !received.is_empty() {
                break;
            }
        }
    })
    .await
    .expect("first request must reach the server within 5s");
    first.abort();
    let joined = tokio::time::timeout(Duration::from_secs(5), first)
        .await
        .expect("aborted call must finish within 5s");
    assert!(
        joined.is_err_and(|e| e.is_cancelled()),
        "first call must be cancelled, not completed"
    );

    let received = server
        .received_requests()
        .await
        .expect("request recording enabled");
    assert_eq!(received.len(), 1, "exactly the first request arrived");

    let result = tokio::time::timeout(Duration::from_secs(3), p2.call(ex2)).await;
    assert!(
        matches!(result, Ok(Ok(_))),
        "second call must acquire the released permit, got {result:?}"
    );
}
