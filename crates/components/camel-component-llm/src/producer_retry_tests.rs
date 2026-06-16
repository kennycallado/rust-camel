// Retry + composition tests: retry after transient failure,
// retry-after honoring, no retry in streaming, no retry after
// content-start, timeout+retry composition, semaphore+retry
// composition, and embed retry.

use std::sync::Arc;
use std::time::Duration;

use camel_api::Body;
use camel_component_api::NetworkRetryPolicy;
use tower::Service;

use crate::LlmEndpointConfig;
use crate::config::LlmOperation;
use crate::error::LlmError;
use crate::producer::LlmProducer;
use crate::provider::LlmProvider;
use crate::provider::mock::{MockMode, MockProvider};

use super::producer_test_helpers::{
    make_exchange, make_producer_with_concurrency_and_retry, make_producer_with_retry,
    make_producer_with_timeout_and_retry,
};

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
// Composition: timeout fires during retry backoff
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
// Composition: permit released during retry backoff
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

// -----------------------------------------------------------------------
// Retry exhaustion test
// -----------------------------------------------------------------------

/// Verifies that when retry policy is exhausted, the last error (not a
/// generic "retry exhausted" error) is surfaced, and the expected number
/// of attempts were made.
#[tokio::test]
async fn retry_exhaustion_surfaces_last_error() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into())).with_rate_limit(None), // always RateLimit { retry_after: None }
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
    let result = producer.call(make_exchange(Body::Text("x".into()))).await;

    assert!(result.is_err(), "retry exhaustion must produce an error");
    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("rate limited"),
        "surfaced error must mention rate limit, got: {msg}"
    );
    assert_eq!(
        mock.call_count(),
        3,
        "provider must be called exactly max_attempts times"
    );
}

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
    let mut producer = LlmProducer::new(config, provider, 32768, "test-route".into())
        .with_retry(Some(policy))
        .build();
    let _ = producer.call(make_exchange(Body::Text("x".into()))).await;
    assert_eq!(mock.call_count(), 2, "embed must retry transient failures");
}
