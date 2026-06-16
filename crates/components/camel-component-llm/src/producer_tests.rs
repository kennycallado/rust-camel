use super::*;
use crate::provider::mock::{MockMode, MockProvider};
use camel_api::Message;
use std::time::Duration;

fn make_producer_with_retry(
    provider: Arc<dyn LlmProvider>,
    stream: bool,
    retry: Option<NetworkRetryPolicy>,
) -> LlmProducer {
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream,
        ..Default::default()
    };
    LlmProducer::new(
        config,
        provider,
        32768,
        "test-route".into(),
        None,
        None,
        retry,
    )
}

/// Helper: create a producer with both timeout and retry configured.
fn make_producer_with_timeout_and_retry(
    provider: Arc<dyn LlmProvider>,
    stream: bool,
    timeout: Duration,
    retry: Option<NetworkRetryPolicy>,
) -> LlmProducer {
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream,
        ..Default::default()
    };
    LlmProducer::new(
        config,
        provider,
        32768,
        "test-route".into(),
        None,
        Some(timeout),
        retry,
    )
}

/// Helper: create a producer with both semaphore and retry configured.
fn make_producer_with_concurrency_and_retry(
    provider: Arc<dyn LlmProvider>,
    max_concurrency: usize,
    retry: Option<NetworkRetryPolicy>,
) -> LlmProducer {
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let semaphore = Some(Arc::new(Semaphore::new(max_concurrency)));
    LlmProducer::new(
        config,
        provider,
        32768,
        "test-route".into(),
        semaphore,
        None,
        retry,
    )
}

fn make_producer(stream: bool, operation: LlmOperation) -> LlmProducer {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hello".into())));
    let config = LlmEndpointConfig {
        operation,
        stream,
        ..Default::default()
    };
    LlmProducer::new(
        config,
        provider,
        32768,
        "test-route".into(),
        None,
        None,
        None,
    )
}

fn make_exchange(body: Body) -> Exchange {
    Exchange::new(Message::new(body))
}

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
    let producer = make_producer(false, LlmOperation::Chat);
    let stream_body = StreamBody {
        stream: Arc::new(Mutex::new(None)),
        metadata: StreamMetadata::default(),
    };
    let exchange = make_exchange(Body::Stream(stream_body));
    assert!(producer.extract_prompt(&exchange).is_err());
}

#[test]
fn extract_prompt_enforces_max_bytes() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig::default();
    let producer = LlmProducer::new(config, provider, 3, "route".into(), None, None, None);
    let exchange = make_exchange(Body::Text("hello world".into()));
    assert!(producer.extract_prompt(&exchange).is_err());
}

#[test]
fn extract_prompt_allows_at_max_bytes() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig::default();
    let producer = LlmProducer::new(config, provider, 5, "route".into(), None, None, None);
    let exchange = make_exchange(Body::Text("hello".into()));
    assert!(producer.extract_prompt(&exchange).is_ok());
}

// ---- build_chat_request ----

#[test]
fn build_chat_request_falls_back_to_provider_model() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig::default();
    let producer = LlmProducer::new(config, provider, 32768, "route".into(), None, None, None);
    let exchange = make_exchange(Body::Text("hello".into()));
    let req = producer.build_chat_request("hello", &exchange);
    assert_eq!(req.model, "mock-model");
}

#[test]
fn build_chat_request_uses_config_model() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig {
        model: Some("gpt-4o".into()),
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "route".into(), None, None, None);
    let exchange = make_exchange(Body::Text("hello".into()));
    let req = producer.build_chat_request("hello", &exchange);
    assert_eq!(req.model, "gpt-4o");
}

#[test]
fn build_chat_request_uses_header_model() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig::default();
    let producer = LlmProducer::new(config, provider, 32768, "route".into(), None, None, None);
    let mut exchange = make_exchange(Body::Text("hello".into()));
    exchange
        .input
        .headers
        .insert(CAMEL_LLM_MODEL.into(), Value::String("header-model".into()));
    let req = producer.build_chat_request("hello", &exchange);
    assert_eq!(req.model, "header-model");
}

#[test]
fn build_chat_request_uses_temperature_from_config() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig {
        temperature: Some(0.5),
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "route".into(), None, None, None);
    let exchange = make_exchange(Body::Text("hello".into()));
    let req = producer.build_chat_request("hello", &exchange);
    assert_eq!(req.temperature, Some(0.5));
}

#[test]
fn build_chat_request_uses_max_tokens_from_config() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig {
        max_tokens: Some(100),
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "route".into(), None, None, None);
    let exchange = make_exchange(Body::Text("hello".into()));
    let req = producer.build_chat_request("hello", &exchange);
    assert_eq!(req.max_tokens, Some(100));
}

#[test]
fn build_chat_request_respects_system_prompt_from_config() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig {
        system_prompt: Some("be helpful".into()),
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "route".into(), None, None, None);
    let exchange = make_exchange(Body::Text("hello".into()));
    let req = producer.build_chat_request("hello", &exchange);
    assert_eq!(req.system_prompt.as_deref(), Some("be helpful"));
}

#[test]
fn build_chat_request_includes_user_message() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
    let config = LlmEndpointConfig::default();
    let producer = LlmProducer::new(config, provider, 32768, "route".into(), None, None, None);
    let exchange = make_exchange(Body::Text("hello".into()));
    let req = producer.build_chat_request("hello", &exchange);
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
    let producer = LlmProducer::new(config, provider, 32768, "route".into(), None, None, None);
    let mut exchange = make_exchange(Body::Empty);
    producer.set_start_headers(&mut exchange);
    assert_eq!(
        exchange.input.headers.get(CAMEL_LLM_MODEL),
        Some(&Value::String("gpt-4o".into()))
    );
}

// -----------------------------------------------------------------------
// Semaphore concurrency tests
// -----------------------------------------------------------------------

/// A helper that creates a producer with a semaphore for concurrency tests.
fn make_producer_with_semaphore(
    provider: Arc<dyn LlmProvider>,
    stream: bool,
    max_concurrency: usize,
) -> LlmProducer {
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream,
        ..Default::default()
    };
    let semaphore = Some(Arc::new(Semaphore::new(max_concurrency)));
    LlmProducer::new(
        config,
        provider,
        32768,
        "test-route".into(),
        semaphore,
        None,
        None,
    )
}

/// Drain a streaming body to completion (consume entire stream) and
/// clear the inner stream option so the `PermitStream` (and its
/// semaphore permit) is dropped.
async fn drain_stream(out: &mut Exchange) {
    if let Body::Stream(sb) = &out.input.body {
        let mut guard = sb.stream.lock().await;
        if let Some(stream) = guard.as_mut() {
            while stream.next().await.is_some() {}
        }
        // Clear the stream — this drops the PermitStream and releases
        // the semaphore permit (permit lives in the stream, see ADR-0021).
        *guard = None;
    }
}

#[tokio::test]
async fn semaphore_bounds_inflight_materialized() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_delay(Duration::from_millis(80))
            .with_concurrent_tracker(),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let producer = make_producer_with_semaphore(provider, false, 2);
    let mut handles = vec![];
    for _ in 0..8 {
        let mut p = producer.clone();
        handles.push(tokio::spawn(async move {
            p.call(make_exchange(Body::Text("x".into()))).await
        }));
    }
    for h in handles {
        let _ = h.await;
    }
    assert_eq!(
        mock.max_concurrent(),
        2,
        "with 8 concurrent 80ms calls at max_concurrency=2, peak must be exactly 2"
    );
}

/// Pull exactly one event from a streaming body (to start it) without
/// draining it fully.
async fn drain_one_chunk(out: &mut Exchange) {
    if let Body::Stream(sb) = &out.input.body {
        let mut guard = sb.stream.lock().await;
        if let Some(stream) = guard.as_mut() {
            let _ = stream.next().await;
        }
    }
}

#[tokio::test]
async fn dropping_stream_body_drops_upstream_stream() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_delay(Duration::from_millis(100))
            .with_cancellation_tracking(),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let mut producer = make_producer_with_semaphore(provider, true, 1);
    let mut out = producer
        .call(make_exchange(Body::Text("x".into())))
        .await
        .unwrap();
    drain_one_chunk(&mut out).await; // start the stream
    drop(out); // drops Body::Stream -> PermitStream -> inner stream
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        mock.was_cancelled(),
        "dropping Body::Stream must drop the upstream provider stream, firing the Mock's CancelGuard"
    );
}

#[tokio::test]
async fn streaming_permit_held_until_stream_consumed() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_delay(Duration::from_millis(30))
            .with_concurrent_tracker(),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let producer = make_producer_with_semaphore(provider, true, 1);

    // First call: take the stream but do NOT consume it yet.
    let mut p1 = producer.clone();
    let ex1 = make_exchange(Body::Text("a".into()));
    let mut out1 = p1.call(ex1).await.expect("first call ok");

    // body is now a Stream — permit still held. A second call must block.
    let mut p2 = producer.clone();
    let second = tokio::spawn(async move { p2.call(make_exchange(Body::Text("b".into()))).await });

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !second.is_finished(),
        "second call must wait while first stream is unconsumed"
    );

    // now drain the first stream (consuming it releases the permit)
    drain_stream(&mut out1).await;

    // second can now proceed
    let _ = second.await;
}

// -----------------------------------------------------------------------
// Timeout enforcement tests
// -----------------------------------------------------------------------

/// Helper: create a producer with a given timeout.
fn make_producer_with_timeout(
    provider: Arc<dyn LlmProvider>,
    stream: bool,
    timeout: Duration,
) -> LlmProducer {
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream,
        ..Default::default()
    };
    LlmProducer::new(
        config,
        provider,
        32768,
        "test-route".into(),
        None,
        Some(timeout),
        None,
    )
}

#[tokio::test]
async fn materialized_total_timeout_fires() {
    let provider = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into())).with_delay(Duration::from_millis(200)),
    );
    let mut producer = make_producer_with_timeout(provider, false, Duration::from_millis(50));
    let res = producer.call(make_exchange(Body::Text("x".into()))).await;
    let err = res.unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("timeout"),
        "got: {err}"
    );
}

#[tokio::test]
async fn streaming_activity_timeout_fires() {
    let provider = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into())).with_delay(Duration::from_millis(200)),
    );
    let mut producer = make_producer_with_timeout(provider, true, Duration::from_millis(50));
    let mut out = producer
        .call(make_exchange(Body::Text("x".into())))
        .await
        .unwrap();
    let body = std::mem::replace(&mut out.input.body, Body::Empty);
    if let Body::Stream(sb) = body {
        let mut guard = sb.stream.lock().await;
        if let Some(stream) = guard.as_mut() {
            let res = stream.next().await.unwrap();
            assert!(
                res.is_err(),
                "streaming activity timeout should produce an error"
            );
            let err = res.unwrap_err();
            assert!(
                err.to_string().to_lowercase().contains("timeout"),
                "streaming timeout error should mention 'timeout', got: {err}"
            );
        }
    }
}

#[tokio::test]
async fn materialized_no_timeout_does_not_fire() {
    // With no timeout configured, a slow provider should complete normally.
    let provider = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into())).with_delay(Duration::from_millis(20)),
    );
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let producer = LlmProducer::new(
        config,
        provider,
        32768,
        "test-route".into(),
        None,
        None,
        None,
    );
    let mut exchange = make_exchange(Body::Text("x".into()));
    producer.handle_chat(&mut exchange).await.expect("chat ok");
    match &exchange.input.body {
        Body::Text(s) => assert_eq!(s, "ok"),
        other => panic!("expected Text, got {other:?}"),
    }
}

// -----------------------------------------------------------------------
// Retry enforcement tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn retry_succeeds_after_transient_failure() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_fail_after(1, LlmError::Network("boom".into())),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let policy = NetworkRetryPolicy {
        enabled: true,
        max_attempts: 3,
        initial_delay: Duration::from_millis(1),
        multiplier: 1.0,
        max_delay: Duration::from_millis(5),
        jitter_factor: 0.0,
    };
    let mut producer = make_producer_with_retry(Arc::clone(&provider), false, Some(policy));
    let out = producer
        .call(make_exchange(Body::Text("x".into())))
        .await
        .unwrap();
    assert!(matches!(out.input.body, Body::Text(_)));
    assert_eq!(mock.call_count(), 2);
}

#[tokio::test]
async fn retry_honors_retry_after_over_backoff() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_rate_limit(Some(Duration::from_millis(60))),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let policy = NetworkRetryPolicy {
        enabled: true,
        max_attempts: 2,
        initial_delay: Duration::from_millis(1),
        multiplier: 1.0,
        max_delay: Duration::from_millis(5),
        jitter_factor: 0.0,
    };
    let mut producer = make_producer_with_retry(Arc::clone(&provider), false, Some(policy));
    let start = std::time::Instant::now();
    let _ = producer.call(make_exchange(Body::Text("x".into()))).await;
    // retry_after (60ms) >> backoff (1ms), so elapsed reflects retry_after
    assert!(start.elapsed() >= Duration::from_millis(55));
}

#[tokio::test]
async fn no_retry_in_streaming_mode() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_fail_after(1, LlmError::Network("boom".into())),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let policy = NetworkRetryPolicy {
        enabled: true,
        max_attempts: 3,
        initial_delay: Duration::from_millis(1),
        multiplier: 1.0,
        max_delay: Duration::from_millis(5),
        jitter_factor: 0.0,
    };
    let mut producer = make_producer_with_retry(Arc::clone(&provider), true, Some(policy));
    let _ = producer.call(make_exchange(Body::Text("x".into()))).await;
    assert_eq!(mock.call_count(), 1, "streaming must not retry");
}

#[tokio::test]
async fn no_retry_after_content_start() {
    // MockMode::Error emits a Delta THEN errors — content-started, must not retry.
    let mock = Arc::new(MockProvider::new(
        "t",
        MockMode::Error(LlmError::Network("boom".into())),
    ));
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let policy = NetworkRetryPolicy {
        enabled: true,
        max_attempts: 3,
        initial_delay: Duration::from_millis(1),
        multiplier: 1.0,
        max_delay: Duration::from_millis(5),
        jitter_factor: 0.0,
    };
    let mut producer = make_producer_with_retry(Arc::clone(&provider), false, Some(policy));
    let _ = producer.call(make_exchange(Body::Text("x".into()))).await;
    assert_eq!(mock.call_count(), 1, "must not retry after content-started");
}

// -----------------------------------------------------------------------
// Composition tests: timeout fires during retry backoff
// -----------------------------------------------------------------------

/// Verifies that the total deadline cuts a retry backoff sleep short.
///
/// The provider always rate-limits with retry_after=200ms. The retry policy
/// allows 10 attempts, but the total deadline is 50ms. The deadline must
/// fire DURING the first backoff sleep (200ms), NOT after all retries.
#[tokio::test]
async fn total_timeout_fires_during_retry_backoff() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_rate_limit(Some(Duration::from_millis(200))),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let policy = NetworkRetryPolicy {
        max_attempts: 10,
        initial_delay: Duration::from_millis(1),
        multiplier: 1.0,
        max_delay: Duration::from_millis(5),
        jitter_factor: 0.0,
        enabled: true,
    };
    let mut producer = make_producer_with_timeout_and_retry(
        provider,
        /*stream=*/ false,
        Duration::from_millis(50), // total timeout
        Some(policy),
    );
    let start = std::time::Instant::now();
    let result = producer.call(make_exchange(Body::Text("x".into()))).await;
    let elapsed = start.elapsed();
    // Must timeout (not succeed, not retry 10 times)
    assert!(result.is_err(), "must error with timeout");
    // Must fire around 50ms (the total deadline), NOT 200ms+ (the retry_after)
    assert!(
        elapsed < Duration::from_millis(150),
        "total deadline must cut backoff short, elapsed: {elapsed:?}"
    );
    // Must NOT have retried many times (only 1-2 attempts before timeout)
    assert!(
        mock.call_count() <= 2,
        "total deadline must prevent excessive retries, got: {}",
        mock.call_count()
    );
}

// -----------------------------------------------------------------------
// Composition tests: permit released during retry backoff
// -----------------------------------------------------------------------

/// Verifies the semaphore slot frees up during backoff so a second concurrent
/// call can proceed while the first sleeps.
///
/// We use a timing-based assertion rather than `max_concurrent()` because
/// `max_concurrent()` tracks concurrent `chat_stream()` calls, which run
/// *inside* the per-attempt semaphore window — the first call's stream is
/// always exhausted before the permit is dropped. Timing proves the permit
/// was released without changing production code:
///
/// With per-attempt permit release:
///   - Call 1: acquire → chat_stream(30ms + error) → drop permit → sleep 50ms → reacquire → chat_stream(30ms + success). Total: ~110ms
///   - Call 2: waits ~30ms for permit → chat_stream(30ms + success). Total: ~60ms
///   - Wall clock: ~110ms
///
/// Without permit release (permit held across retries):
///   - Call 2 waits 110ms for call 1 to finish → 30ms → ~140ms total
#[tokio::test]
async fn permit_released_during_retry_backoff() {
    // max_concurrency=1, fail_after=1 (first call fails, retry succeeds).
    // First call acquires permit, fails, releases permit during backoff.
    // Second concurrent call can then acquire the permit while first is sleeping.
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_delay(Duration::from_millis(40))
            .with_fail_after(1, LlmError::Network("boom".into()))
            .with_concurrent_tracker(),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let policy = NetworkRetryPolicy {
        max_attempts: 3,
        initial_delay: Duration::from_millis(80), // long backoff = window for 2nd call
        multiplier: 1.0,
        max_delay: Duration::from_millis(200),
        jitter_factor: 0.0,
        enabled: true,
    };
    let producer = make_producer_with_concurrency_and_retry(provider, 1, Some(policy));
    let p = Arc::new(producer);

    let start = std::time::Instant::now();
    let mut handles = vec![];
    for _ in 0..2 {
        let p = p.clone();
        handles.push(tokio::spawn(async move {
            let mut prod = (*p).clone();
            prod.call(make_exchange(Body::Text("x".into()))).await
        }));
    }
    for h in handles {
        let _ = h.await;
    }
    let elapsed = start.elapsed();

    // If permit were NOT released during backoff, total wall-clock time
    // would be ~200ms (call2 waits ~160ms for call1 then runs ~40ms).
    // With release, wall clock is ~160ms (call1's 2-attempt sequence
    // dominates; call2 overlaps during backoff). A 180ms threshold
    // provides ~20ms margin on each side for CI variance.
    assert!(
        elapsed < Duration::from_millis(180),
        "permit must be released during backoff — total elapsed {elapsed:?} suggests sequential execution",
    );
}

// -----------------------------------------------------------------------
// Embed retry test
// -----------------------------------------------------------------------

/// Verifies that embed retries on transient failure.
#[tokio::test]
async fn embed_retries_on_transient_failure() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_fail_after(1, LlmError::Network("boom".into())),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let policy = NetworkRetryPolicy {
        max_attempts: 3,
        initial_delay: Duration::from_millis(1),
        multiplier: 1.0,
        max_delay: Duration::from_millis(5),
        jitter_factor: 0.0,
        enabled: true,
    };
    let config = LlmEndpointConfig {
        operation: LlmOperation::Embed,
        stream: false,
        ..Default::default()
    };
    let mut producer = LlmProducer::new(
        config,
        provider,
        32768,
        "test-route".into(),
        None,
        None,
        Some(policy),
    );
    let _ = producer.call(make_exchange(Body::Text("x".into()))).await;
    assert_eq!(mock.call_count(), 2, "embed must retry transient failures");
}
