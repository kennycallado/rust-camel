// Materialized chat, streaming chat, embed, extract_prompt, build_chat_request,
// set_start_headers, and cost histogram tests.

use std::sync::Arc;

use bytes::Bytes;
use camel_api::{Body, MetricsCollector, StreamBody};
use camel_component_api::{HealthCheckRegistry, NoOpHealthCheckRegistry, RuntimeObservability};
use serde_json::Value;
use tower::Service;

use crate::LlmEndpointConfig;
use crate::config::LlmOperation;
use crate::cost::PricingTable;
use crate::headers::*;
use crate::producer::LlmProducer;
use crate::provider::ChatRole;
use crate::provider::mock::{MockMode, MockProvider};

use super::producer_test_helpers::{
    drain_stream, make_exchange, make_producer, make_producer_with_pricing,
};

// ---- handle_chat (materialized) ----

#[tokio::test]
async fn chat_materialized_returns_text() {
    let producer = make_producer(false, LlmOperation::Chat);
    let mut exchange = make_exchange(Body::Text("hello".into()));
    producer.handle_chat(&mut exchange).await.expect("chat ok");
    match &exchange.input.body {
        Body::Text(s) => assert_eq!(s, "hello"),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[tokio::test]
async fn chat_materialized_sets_usage_available_true() {
    let producer = make_producer(false, LlmOperation::Chat);
    let mut exchange = make_exchange(Body::Text("prompt".into()));
    producer.handle_chat(&mut exchange).await.expect("chat ok");
    assert_eq!(
        exchange.input.headers.get(CAMEL_LLM_USAGE_AVAILABLE),
        Some(&Value::Bool(true))
    );
}

#[tokio::test]
async fn chat_materialized_sets_token_headers() {
    let producer = make_producer(false, LlmOperation::Chat);
    let mut exchange = make_exchange(Body::Text("hello".into()));
    producer.handle_chat(&mut exchange).await.expect("chat ok");
    assert!(exchange.input.headers.contains_key(CAMEL_LLM_TOKENS_IN));
    assert!(exchange.input.headers.contains_key(CAMEL_LLM_TOKENS_OUT));
}

#[tokio::test]
async fn chat_materialized_sets_finish_reason() {
    let producer = make_producer(false, LlmOperation::Chat);
    let mut exchange = make_exchange(Body::Text("hello".into()));
    producer.handle_chat(&mut exchange).await.expect("chat ok");
    assert!(exchange.input.headers.contains_key(CAMEL_LLM_FINISH_REASON));
}

#[tokio::test]
async fn cost_header_set_from_pricing_materialized() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let mut producer = make_producer_with_pricing(provider, 0.0025, 0.01);
    let out = producer
        .call(make_exchange(Body::Text("x".into())))
        .await
        .unwrap();
    assert!(out.input.headers.contains_key(CAMEL_LLM_ESTIMATED_COST_USD));
}

#[tokio::test]
async fn missing_pricing_no_cost_no_failure() {
    let mut producer = make_producer(false, LlmOperation::Chat);
    let out = producer
        .call(make_exchange(Body::Text("x".into())))
        .await
        .unwrap();
    assert!(!out.input.headers.contains_key(CAMEL_LLM_ESTIMATED_COST_USD));
}

// ---- handle_chat (streaming) ----

#[tokio::test]
async fn chat_streaming_sets_stream_body() {
    let producer = make_producer(true, LlmOperation::Chat);
    let mut exchange = make_exchange(Body::Text("hello".into()));
    producer.handle_chat(&mut exchange).await.expect("chat ok");
    assert!(matches!(exchange.input.body, Body::Stream(_)));
}

#[tokio::test]
async fn chat_streaming_sets_stream_header() {
    let producer = make_producer(true, LlmOperation::Chat);
    let mut exchange = make_exchange(Body::Text("hello".into()));
    producer.handle_chat(&mut exchange).await.expect("chat ok");
    assert_eq!(
        exchange.input.headers.get(CAMEL_LLM_STREAM),
        Some(&Value::Bool(true))
    );
}

#[tokio::test]
async fn chat_streaming_sets_usage_available_false() {
    let producer = make_producer(true, LlmOperation::Chat);
    let mut exchange = make_exchange(Body::Text("hello".into()));
    producer.handle_chat(&mut exchange).await.expect("chat ok");
    assert_eq!(
        exchange.input.headers.get(CAMEL_LLM_USAGE_AVAILABLE),
        Some(&Value::Bool(false))
    );
}

#[tokio::test]
async fn chat_streaming_sets_provider_header() {
    let producer = make_producer(true, LlmOperation::Chat);
    let mut exchange = make_exchange(Body::Text("hello".into()));
    producer.handle_chat(&mut exchange).await.expect("chat ok");
    assert_eq!(
        exchange.input.headers.get(CAMEL_LLM_PROVIDER),
        Some(&Value::String("test".into()))
    );
}

// ---- handle_embed ----

#[tokio::test]
async fn embed_returns_json() {
    let producer = make_producer(false, LlmOperation::Embed);
    let mut exchange = make_exchange(Body::Text("hello".into()));
    producer
        .handle_embed(&mut exchange)
        .await
        .expect("embed ok");
    assert!(matches!(exchange.input.body, Body::Json(_)));
}

#[tokio::test]
async fn embed_sets_model_header() {
    let producer = make_producer(false, LlmOperation::Embed);
    let mut exchange = make_exchange(Body::Text("hello".into()));
    producer
        .handle_embed(&mut exchange)
        .await
        .expect("embed ok");
    assert!(exchange.input.headers.contains_key(CAMEL_LLM_MODEL));
}

#[tokio::test]
async fn embed_sets_usage_available() {
    let producer = make_producer(false, LlmOperation::Embed);
    let mut exchange = make_exchange(Body::Text("hello".into()));
    producer
        .handle_embed(&mut exchange)
        .await
        .expect("embed ok");
    assert_eq!(
        exchange.input.headers.get(CAMEL_LLM_USAGE_AVAILABLE),
        Some(&Value::Bool(true))
    );
}

#[tokio::test]
async fn embed_sets_tokens_in_header() {
    let producer = make_producer(false, LlmOperation::Embed);
    let mut exchange = make_exchange(Body::Text("hello".into()));
    producer
        .handle_embed(&mut exchange)
        .await
        .expect("embed ok");
    assert!(exchange.input.headers.contains_key(CAMEL_LLM_TOKENS_IN));
}

// ---- extract_prompt ----

#[test]
fn extract_prompt_from_text() {
    let producer = make_producer(false, LlmOperation::Chat);
    let exchange = make_exchange(Body::Text("hello".into()));
    let prompt = producer.extract_prompt(&exchange).expect("extract ok");
    assert_eq!(prompt, "hello");
}

#[test]
fn extract_prompt_from_bytes() {
    let producer = make_producer(false, LlmOperation::Chat);
    let exchange = make_exchange(Body::Bytes(Bytes::from("hello")));
    let prompt = producer.extract_prompt(&exchange).expect("extract ok");
    assert_eq!(prompt, "hello");
}

#[test]
fn extract_prompt_from_json() {
    let producer = make_producer(false, LlmOperation::Chat);
    let exchange = make_exchange(Body::Json(serde_json::json!("hello")));
    let prompt = producer.extract_prompt(&exchange).expect("extract ok");
    assert_eq!(prompt, "\"hello\"");
}

#[test]
fn extract_prompt_from_empty_errors() {
    let producer = make_producer(false, LlmOperation::Chat);
    let exchange = make_exchange(Body::Empty);
    assert!(producer.extract_prompt(&exchange).is_err());
}

#[test]
fn extract_prompt_from_stream_errors() {
    use tokio::sync::Mutex;

    let producer = make_producer(false, LlmOperation::Chat);
    let stream_body = StreamBody {
        stream: Arc::new(Mutex::new(None)),
        metadata: Default::default(),
    };
    let exchange = make_exchange(Body::Stream(stream_body));
    assert!(producer.extract_prompt(&exchange).is_err());
}

#[test]
fn extract_prompt_enforces_max_bytes() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig::default();
    let producer = LlmProducer::new(config, provider, 3, "route".into()).build();
    let exchange = make_exchange(Body::Text("hello world".into()));
    assert!(producer.extract_prompt(&exchange).is_err());
}

#[test]
fn extract_prompt_allows_at_max_bytes() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig::default();
    let producer = LlmProducer::new(config, provider, 5, "route".into()).build();
    let exchange = make_exchange(Body::Text("hello".into()));
    assert!(producer.extract_prompt(&exchange).is_ok());
}

// ---- build_chat_request ----

#[test]
fn build_chat_request_falls_back_to_provider_model() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig::default();
    let producer = LlmProducer::new(config, provider, 32768, "route".into()).build();
    let exchange = make_exchange(Body::Text("hello".into()));
    let req = producer
        .build_chat_request("hello", &exchange)
        .expect("build ok");
    assert_eq!(req.model, "mock-model");
}

#[test]
fn build_chat_request_uses_config_model() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig {
        model: Some("gpt-4o".into()),
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "route".into()).build();
    let exchange = make_exchange(Body::Text("hello".into()));
    let req = producer
        .build_chat_request("hello", &exchange)
        .expect("build ok");
    assert_eq!(req.model, "gpt-4o");
}

#[test]
fn build_chat_request_uses_header_model() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig::default();
    let producer = LlmProducer::new(config, provider, 32768, "route".into()).build();
    let mut exchange = make_exchange(Body::Text("hello".into()));
    exchange
        .input
        .headers
        .insert(CAMEL_LLM_MODEL.into(), Value::String("header-model".into()));
    let req = producer
        .build_chat_request("hello", &exchange)
        .expect("build ok");
    assert_eq!(req.model, "header-model");
}

#[test]
fn build_chat_request_uses_temperature_from_config() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig {
        temperature: Some(0.5),
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "route".into()).build();
    let exchange = make_exchange(Body::Text("hello".into()));
    let req = producer
        .build_chat_request("hello", &exchange)
        .expect("build ok");
    assert_eq!(req.temperature, Some(0.5));
}

#[test]
fn build_chat_request_uses_max_tokens_from_config() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig {
        max_tokens: Some(100),
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "route".into()).build();
    let exchange = make_exchange(Body::Text("hello".into()));
    let req = producer
        .build_chat_request("hello", &exchange)
        .expect("build ok");
    assert_eq!(req.max_tokens, Some(100));
}

#[test]
fn build_chat_request_respects_system_prompt_from_config() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig {
        system_prompt: Some("be helpful".into()),
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "route".into()).build();
    let exchange = make_exchange(Body::Text("hello".into()));
    let req = producer
        .build_chat_request("hello", &exchange)
        .expect("build ok");
    assert_eq!(req.system_prompt.as_deref(), Some("be helpful"));
}

#[test]
fn build_chat_request_includes_user_message() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig::default();
    let producer = LlmProducer::new(config, provider, 32768, "route".into()).build();
    let exchange = make_exchange(Body::Text("hello".into()));
    let req = producer
        .build_chat_request("hello", &exchange)
        .expect("build ok");
    assert_eq!(req.messages.len(), 1);
    assert_eq!(req.messages[0].content, "hello");
    assert_eq!(req.messages[0].role, ChatRole::User);
}

// ---- set_start_headers ----

#[test]
fn set_start_headers_sets_provider() {
    let producer = make_producer(false, LlmOperation::Chat);
    let mut exchange = make_exchange(Body::Empty);
    producer.set_start_headers(&mut exchange);
    assert_eq!(
        exchange.input.headers.get(CAMEL_LLM_PROVIDER),
        Some(&Value::String("test".into()))
    );
}

#[test]
fn set_start_headers_sets_stream_false() {
    let producer = make_producer(false, LlmOperation::Chat);
    let mut exchange = make_exchange(Body::Empty);
    producer.set_start_headers(&mut exchange);
    assert_eq!(
        exchange.input.headers.get(CAMEL_LLM_STREAM),
        Some(&Value::Bool(false))
    );
}

#[test]
fn set_start_headers_sets_stream_true() {
    let producer = make_producer(true, LlmOperation::Chat);
    let mut exchange = make_exchange(Body::Empty);
    producer.set_start_headers(&mut exchange);
    assert_eq!(
        exchange.input.headers.get(CAMEL_LLM_STREAM),
        Some(&Value::Bool(true))
    );
}

#[test]
fn set_start_headers_usage_available_starts_false() {
    let producer = make_producer(true, LlmOperation::Chat);
    let mut exchange = make_exchange(Body::Empty);
    producer.set_start_headers(&mut exchange);
    assert_eq!(
        exchange.input.headers.get(CAMEL_LLM_USAGE_AVAILABLE),
        Some(&Value::Bool(false))
    );
}

#[test]
fn set_start_headers_skips_model_when_not_configured() {
    let producer = make_producer(false, LlmOperation::Chat);
    let mut exchange = make_exchange(Body::Empty);
    producer.set_start_headers(&mut exchange);
    assert!(!exchange.input.headers.contains_key(CAMEL_LLM_MODEL));
}

#[test]
fn set_start_headers_sets_model_when_configured() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig {
        model: Some("gpt-4o".into()),
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "route".into()).build();
    let mut exchange = make_exchange(Body::Empty);
    producer.set_start_headers(&mut exchange);
    assert_eq!(
        exchange.input.headers.get(CAMEL_LLM_MODEL),
        Some(&Value::String("gpt-4o".into()))
    );
}

// -----------------------------------------------------------------------
// Cost histogram metric (F2)
// -----------------------------------------------------------------------

#[allow(clippy::type_complexity)]
struct RecordingMetrics {
    histograms: std::sync::Mutex<Vec<(String, f64, Vec<(String, String)>)>>,
}

impl MetricsCollector for RecordingMetrics {
    fn record_exchange_duration(&self, _: &str, _: std::time::Duration) {}
    fn increment_errors(&self, _: &str, _: &str) {}
    fn increment_exchanges(&self, _: &str) {}
    fn set_queue_depth(&self, _: &str, _: usize) {}
    fn record_circuit_breaker_change(&self, _: &str, _: &str, _: &str) {}
    fn record_histogram(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        self.histograms.lock().unwrap().push((
            name.to_string(),
            value,
            labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        ));
    }
}

struct RecordingRuntime {
    metrics: Arc<RecordingMetrics>,
}

impl RecordingRuntime {
    fn new() -> Self {
        Self {
            metrics: Arc::new(RecordingMetrics {
                histograms: std::sync::Mutex::new(Vec::new()),
            }),
        }
    }
}

impl HealthCheckRegistry for RecordingRuntime {
    fn force_unhealthy_for_route(&self, _: &str, _: &str, _: &str) {}
}

impl RuntimeObservability for RecordingRuntime {
    fn metrics(&self) -> Arc<dyn MetricsCollector> {
        Arc::clone(&self.metrics) as Arc<dyn MetricsCollector>
    }
    fn health(&self) -> Arc<dyn HealthCheckRegistry> {
        Arc::new(NoOpHealthCheckRegistry)
    }
}

#[tokio::test]
async fn cost_metric_emitted_on_materialized() {
    let rt = Arc::new(RecordingRuntime::new());
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hello".into())));
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let pricing = Arc::new(PricingTable {
        input_per_1k_tokens: 0.0025,
        output_per_1k_tokens: 0.01,
    });
    let mut producer = LlmProducer::new(config, provider, 32768, "test-route".into())
        .with_pricing(Some(pricing))
        .with_observability(Some(rt.clone() as Arc<dyn RuntimeObservability>))
        .build();

    let out = producer
        .call(make_exchange(Body::Text("x".into())))
        .await
        .unwrap();

    // Verify cost header was set (prices > 0, mock returns usage)
    assert!(out.input.headers.contains_key(CAMEL_LLM_ESTIMATED_COST_USD));

    // Verify histogram was recorded
    let hists = rt.metrics.histograms.lock().unwrap();
    assert!(!hists.is_empty(), "expected at least one histogram entry");
    assert_eq!(hists[0].0, "llm_request_cost_usd");
    assert!(hists[0].1 > 0.0, "cost must be positive");
    let labels: std::collections::HashMap<&str, &str> = hists[0]
        .2
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(labels.get("provider"), Some(&"test"));
    assert_eq!(labels.get("route"), Some(&"test-route"));
    assert_eq!(labels.len(), 2);
}

#[tokio::test]
async fn cost_metric_emitted_on_streaming() {
    let rt = Arc::new(RecordingRuntime::new());
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hello".into())));
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: true,
        ..Default::default()
    };
    let pricing = Arc::new(PricingTable {
        input_per_1k_tokens: 0.0025,
        output_per_1k_tokens: 0.01,
    });
    let mut producer = LlmProducer::new(config, provider, 32768, "test-route".into())
        .with_pricing(Some(pricing))
        .with_observability(Some(rt.clone() as Arc<dyn RuntimeObservability>))
        .build();

    let mut out = producer
        .call(make_exchange(Body::Text("x".into())))
        .await
        .unwrap();

    // Drain the stream to completion — Finished event triggers cost metric
    drain_stream(&mut out).await;

    // Verify histogram was recorded
    let hists = rt.metrics.histograms.lock().unwrap();
    assert!(!hists.is_empty(), "expected at least one histogram entry");
    assert_eq!(hists[0].0, "llm_request_cost_usd");
    assert!(hists[0].1 > 0.0, "cost must be positive");
    let labels: std::collections::HashMap<&str, &str> = hists[0]
        .2
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(labels.get("provider"), Some(&"test"));
    assert_eq!(labels.get("route"), Some(&"test-route"));
    assert_eq!(labels.len(), 2);
}
