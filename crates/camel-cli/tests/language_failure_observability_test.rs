//! Pins the label contract of `language_eval_failures_total` through the real
//! camel-cli boot path: YAML route → `parse_routes_with_env` →
//! `add_route_definition` → direct producer delivery.
//!
//! The metric itself is emitted by `camel-core`; this suite is the integration
//! proof that Simple, Rhai, JsonPath and XPath failures surface the expected
//! `{language, route, verb, class}` labels and never leak exchange data.
//!
//! Rhai/JsonPath/XPath tests are feature-gated so lang-free builds still
//! compile.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use camel_api::Exchange;
use camel_api::Message;
use camel_api::MetricsCollector;
use camel_component_api::NoOpComponentContext;
use camel_component_direct::DirectComponent;
use camel_component_mock::MockComponent;
use camel_core::CamelContext;
use tower::ServiceExt;

const METRIC: &str = "language_eval_failures_total";
const ROUTE_ID: &str = "metric-route";

/// One captured `record_counter` call.
#[derive(Clone, Debug, PartialEq)]
struct CounterCall {
    name: String,
    value: f64,
    labels: Vec<(String, String)>,
}

/// Test double that captures every `record_counter` call and ignores the rest
/// of the `MetricsCollector` surface.
#[derive(Clone, Default)]
struct RecordingMetrics {
    counters: Arc<Mutex<Vec<CounterCall>>>,
}

impl RecordingMetrics {
    fn calls(&self) -> Vec<CounterCall> {
        self.counters
            .lock()
            .expect("metrics recorder mutex poisoned")
            .clone()
    }
}

impl MetricsCollector for RecordingMetrics {
    fn record_exchange_duration(&self, _route_id: &str, _duration: Duration) {}

    fn increment_errors(&self, _route_id: &str, _error_type: &str) {}

    fn increment_exchanges(&self, _route_id: &str) {}

    fn set_queue_depth(&self, _queue: &str, _depth: usize) {}

    fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {}

    fn record_counter(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        self.counters
            .lock()
            .expect("metrics recorder mutex poisoned")
            .push(CounterCall {
                name: name.to_string(),
                value,
                labels: labels
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect(),
            });
    }
}

/// Build a one-route context (`metric-route` from `direct:start`), run one
/// exchange through `step_yaml`, stop the context and return all captured
/// counter calls.
async fn run_route(step_yaml: &str, body: &str) -> Vec<CounterCall> {
    let recorder = RecordingMetrics::default();
    let mut ctx = CamelContext::builder()
        .metrics(Arc::new(recorder.clone()) as Arc<dyn MetricsCollector>)
        .build()
        .await
        .expect("context builds");
    ctx.register_component(DirectComponent::new());
    ctx.register_component(MockComponent::new());

    let yaml = format!(
        r#"routes:
  - id: metric-route
    from: "direct:start"
    steps:
      - {step_yaml}
"#
    );
    let mut defs =
        camel_dsl::parse_routes_with_env(&yaml, &|_name| None).expect("route YAML parses");
    assert_eq!(defs.len(), 1, "exactly one route expected");
    let def = defs.swap_remove(0);
    ctx.add_route_definition(def)
        .await
        .expect("route definition compiles");
    ctx.start().await.expect("context starts");

    deliver(&ctx, body).await;
    ctx.stop().await.expect("context stops");

    recorder.calls()
}

/// Deliver one exchange to `direct:start`. A failing route is the expected
/// outcome here, so only the startup race (direct consumer not registered yet)
/// is retried, bounded by a deadline with no sleep.
async fn deliver(ctx: &CamelContext, body: &str) {
    let exchange = Exchange::new(Message::new(body));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let producer = {
            let registry = ctx.registry();
            let component = registry.get("direct").expect("direct component registered");
            let endpoint = component
                .create_endpoint("direct:start", ctx)
                .expect("direct endpoint creation");
            endpoint
                .create_producer(Arc::new(NoOpComponentContext), &ctx.producer_context())
                .expect("direct producer creation")
        };

        match producer.oneshot(exchange.clone()).await {
            Err(err) if direct_not_registered(&err) && tokio::time::Instant::now() < deadline => {
                tokio::task::yield_now().await;
            }
            // Failing routes are expected; a success is irrelevant to labels.
            _ => return,
        }
    }
}

fn direct_not_registered(err: &camel_api::CamelError) -> bool {
    matches!(
        err,
        camel_api::CamelError::EndpointCreationFailed(msg)
            if msg.starts_with("direct endpoint '") && msg.ends_with("' not registered")
    )
}

/// Assert exactly one `language_eval_failures_total` call and that its labels
/// (key and value) match `expected` exactly.
fn assert_single_metric(calls: &[CounterCall], expected: &[(&str, &str)]) {
    let matching: Vec<&CounterCall> = calls.iter().filter(|call| call.name == METRIC).collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one {METRIC} call, got {calls:?}"
    );
    let call = matching[0];
    assert_eq!(call.value, 1.0, "counter value must be 1.0: {call:?}");

    let mut actual = call.labels.clone();
    actual.sort();
    let mut want: Vec<(String, String)> = expected
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    want.sort();
    assert_eq!(actual, want, "label set mismatch: {call:?}");
}

#[tokio::test]
async fn metric_labels_for_simple_evaluation_failure() {
    let calls = run_route(r#"set_body: { simple: "${lang:nope:body}" }"#, "x").await;
    assert_single_metric(
        &calls,
        &[
            ("language", "simple"),
            ("route", ROUTE_ID),
            ("verb", "set_body"),
            ("class", "runtime"),
        ],
    );
}

#[cfg(feature = "lang-rhai")]
#[tokio::test]
async fn metric_labels_for_rhai_evaluation_failure() {
    let calls = run_route(r#"set_body: { rhai: "nonexistent_fn()" }"#, "x").await;
    assert_single_metric(
        &calls,
        &[
            ("language", "rhai"),
            ("route", ROUTE_ID),
            ("verb", "set_body"),
            ("class", "function-not-found"),
        ],
    );
}

#[cfg(feature = "lang-jsonpath")]
#[tokio::test]
async fn metric_labels_for_jsonpath_evaluation_failure() {
    let calls = run_route(r#"set_body: { jsonpath: "$.x" }"#, "not json").await;
    assert_single_metric(
        &calls,
        &[
            ("language", "jsonpath"),
            ("route", ROUTE_ID),
            ("verb", "set_body"),
            ("class", "conversion"),
        ],
    );
}

#[cfg(feature = "lang-xpath")]
#[tokio::test]
async fn metric_labels_for_xpath_evaluation_failure() {
    let calls = run_route(r#"set_body: { xpath: "/a" }"#, "not xml").await;
    assert_single_metric(
        &calls,
        &[
            ("language", "xpath"),
            ("route", ROUTE_ID),
            ("verb", "set_body"),
            ("class", "conversion"),
        ],
    );
}

#[cfg(feature = "lang-rhai")]
#[tokio::test]
async fn metric_labels_exclude_exchange_data() {
    let calls = run_route(
        r#"set_body: { rhai: "nonexistent_fn()" }"#,
        "SECRET-SENTINEL",
    )
    .await;
    assert_single_metric(
        &calls,
        &[
            ("language", "rhai"),
            ("route", ROUTE_ID),
            ("verb", "set_body"),
            ("class", "function-not-found"),
        ],
    );

    for call in &calls {
        assert!(
            !call.name.contains("SECRET-SENTINEL"),
            "metric name leaked exchange data: {call:?}"
        );
        for (key, value) in &call.labels {
            assert!(
                !key.contains("SECRET-SENTINEL"),
                "label key leaked exchange data: {key:?}"
            );
            assert!(
                !value.contains("SECRET-SENTINEL"),
                "label value leaked exchange data: {value:?}"
            );
            assert!(
                !value.contains("Processor error"),
                "label value leaked cause text: {value:?}"
            );
        }
    }
}
