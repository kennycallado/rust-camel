// Accepted-payload cap on the materialized chat loop, plus the streaming
// passthrough surfacing the adapter's cap error as a terminal frame.

use std::sync::Arc;

use camel_api::{Body, CamelError};
use tower::Service;

use crate::LlmEndpointConfig;
use crate::MAX_CHAT_RESPONSE_BYTES;
use crate::config::LlmOperation;
use crate::headers::CAMEL_LLM_TEXT;
use crate::producer::LlmProducer;
use crate::provider::LlmProvider;
use crate::provider::mock::{MockMode, MockProvider};

use super::producer_test_helpers::make_exchange;

/// Materialized chat producer backed by a mock that emits `fixture` as a
/// single `Delta` (mock.rs: one Delta then Finished).
fn make_materialized_producer(fixture: String) -> LlmProducer {
    let provider: Arc<dyn LlmProvider> =
        Arc::new(MockProvider::new("test", MockMode::Fixed(fixture)));
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    LlmProducer::new(config, provider, 32768, "test-route".into()).build()
}

#[cfg(feature = "openai")]
fn make_streaming_producer(provider: Arc<dyn LlmProvider>) -> LlmProducer {
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: true,
        ..Default::default()
    };
    LlmProducer::new(config, provider, 32768, "test-route".into()).build()
}

#[tokio::test]
async fn materialized_under_cap_completes_byte_identical() {
    let fixture = "The quick brown fox ".repeat(256);
    let producer = make_materialized_producer(fixture.clone());
    let mut exchange = make_exchange(Body::Text("prompt".into()));

    producer
        .handle_chat(&mut exchange)
        .await
        .expect("under-cap chat must succeed");

    match &exchange.input.body {
        Body::Text(text) => assert_eq!(text, &fixture, "body must be byte identical"),
        other => panic!("expected Text body, got {other:?}"),
    }
    assert!(
        !exchange.input.headers.contains_key(CAMEL_LLM_TEXT),
        "text-only turn must not set {CAMEL_LLM_TEXT}"
    );
}

#[tokio::test]
async fn materialized_over_cap_fails_without_partial_result() {
    let producer = make_materialized_producer("x".repeat(MAX_CHAT_RESPONSE_BYTES + 1));
    let mut exchange = make_exchange(Body::Text("prompt".into()));

    let err = producer
        .handle_chat(&mut exchange)
        .await
        .expect_err("cap-plus-one must fail");

    let display = err.to_string();
    assert!(
        display.contains("chat_materialized"),
        "missing path: {display}"
    );
    assert!(display.contains("8388608"), "missing cap: {display}");
    assert!(display.contains("8388609"), "missing count: {display}");

    assert!(
        matches!(&exchange.input.body, Body::Text(text) if text == "prompt"),
        "no partial body may be written to the exchange"
    );
    assert!(
        !exchange.input.headers.contains_key(CAMEL_LLM_TEXT),
        "no partial text header may be written"
    );
}

#[tokio::test]
async fn materialized_producer_observes_camel_error() {
    let mut producer = make_materialized_producer("x".repeat(MAX_CHAT_RESPONSE_BYTES + 1));

    let err = producer
        .call(make_exchange(Body::Text("prompt".into())))
        .await
        .expect_err("cap-plus-one must fail through the producer service");

    assert!(
        matches!(err, CamelError::ProcessorError(_)),
        "expected ProcessorError, got {err:?}"
    );
    let text = err.to_string();
    assert!(
        text.contains("response body exceeds"),
        "missing display prefix: {text}"
    );
    assert!(text.contains("chat_materialized"), "missing path: {text}");
    assert!(text.contains("8388608"), "missing cap: {text}");
    assert!(text.contains("8388609"), "missing count: {text}");
}

#[tokio::test]
async fn materialized_exactly_cap_accepted() {
    let fixture = "x".repeat(MAX_CHAT_RESPONSE_BYTES);
    let producer = make_materialized_producer(fixture.clone());
    let mut exchange = make_exchange(Body::Text("prompt".into()));

    producer
        .handle_chat(&mut exchange)
        .await
        .expect("exactly-cap chat must succeed");

    match &exchange.input.body {
        Body::Text(text) => {
            assert_eq!(text.len(), MAX_CHAT_RESPONSE_BYTES);
            assert_eq!(text, &fixture, "body must be byte identical");
        }
        other => panic!("expected Text body, got {other:?}"),
    }
}

/// Streaming passthrough: the genai adapter's cap error crosses the producer
/// seam as a terminal `CamelError::ProcessorError` frame after the earlier
/// deltas have been delivered.
#[cfg(feature = "openai")]
mod streaming {
    use std::time::Duration;

    use camel_api::SsrfPolicy;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::producer_test_helpers::collect_stream_results;

    use super::*;

    const CHUNK_BYTES: usize = 1024;

    /// One OpenAI content delta, framed as an SSE event.
    fn content_event(text: &str) -> String {
        let payload = serde_json::json!({ "choices": [{ "delta": { "content": text } }] });
        format!("data: {payload}\n\n")
    }

    /// Wire body carrying every chunk as a content delta, then stop + `[DONE]`.
    fn sse_body(chunks: &[String]) -> String {
        let mut body = String::new();
        for chunk in chunks {
            body.push_str(&content_event(chunk));
        }
        body.push_str(
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
             data: [DONE]\n\n",
        );
        body
    }

    async fn sse_server(body: String) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
            .mount(&server)
            .await;
        server
    }

    /// Real `build_openai` path: the hardened client is `no_proxy`, so the
    /// loopback mock server is reached directly under `AllowInternal`.
    fn openai_provider(server: &MockServer) -> Arc<dyn LlmProvider> {
        crate::provider::genai_adapter::build_openai(
            "wiremock-openai",
            &crate::config::OpenaiProviderConfig {
                api_key: "sk-test".into(),
                base_url: Some(format!("{}/v1", server.uri())),
                default_model: "gpt-4o".into(),
                timeout_secs: None,
                max_concurrency: None,
                network_retry: None,
                pricing: None,
                cache_ttl_secs: None,
                cache_max_entries: None,
            },
            Duration::from_secs(30),
            SsrfPolicy::AllowInternal,
        )
        .expect("build openai provider")
    }

    #[tokio::test]
    async fn streaming_body_delivers_deltas_then_camel_error() {
        // 8192 chunks fill the cap exactly; the next one breaches it.
        let chunk_count = MAX_CHAT_RESPONSE_BYTES / CHUNK_BYTES + 1;
        let chunks: Vec<String> = (0..chunk_count).map(|_| "x".repeat(CHUNK_BYTES)).collect();
        let counted = chunk_count * CHUNK_BYTES;
        assert!(counted > MAX_CHAT_RESPONSE_BYTES);

        let server = sse_server(sse_body(&chunks)).await;
        let producer = make_streaming_producer(openai_provider(&server));
        let mut exchange = make_exchange(Body::Text("prompt".into()));

        producer
            .handle_chat(&mut exchange)
            .await
            .expect("streaming setup must succeed");
        let results = collect_stream_results(&mut exchange).await;

        assert!(
            results.len() > 1,
            "expected delivered deltas plus a terminal error, got {} frames",
            results.len()
        );
        let (last, earlier) = results.split_last().expect("non-empty frames");
        assert!(
            earlier.iter().all(|frame| frame.is_ok()),
            "deltas must precede the terminal error"
        );
        assert!(
            earlier
                .iter()
                .all(|frame| { frame.as_ref().is_ok_and(|bytes| bytes.len() == CHUNK_BYTES) }),
            "every delivered frame must be an intact delta"
        );

        let CamelError::ProcessorError(text) =
            last.as_ref().expect_err("terminal frame must be Err")
        else {
            panic!("expected ProcessorError, got {last:?}");
        };
        assert!(text.contains("chat_stream"), "missing path: {text}");
        assert!(text.contains("8388608"), "missing cap: {text}");
        assert!(text.contains(&counted.to_string()), "missing count: {text}");
    }
}
