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
        max_attempts_absolute: None,
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
        max_attempts_absolute: None,
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
        max_attempts_absolute: None,
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
        max_attempts_absolute: None,
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
        max_attempts_absolute: None,
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
/// We assert on the **relative ordering of `chat_stream` invocations**
/// (recorded by `with_start_times_tracker`) rather than absolute wall-clock
/// time. CI runners (especially macOS) have high scheduling variance, which
/// makes absolute thresholds like `elapsed < 180ms` flaky — the gap between
/// the with-release (~160ms) and without-release (~200ms) scenarios is too
/// small for a robust absolute threshold.
///
/// Timeline with per-attempt permit release (max_concurrency=1):
///   - t=0:     call 1 attempt 1 acquires permit → chat_stream (delay)
///   - t=delay: call 1 errors, drops permit, enters backoff
///   - t=delay: call 2 acquires permit → chat_stream (delay)
///   - t=2*delay+backoff: call 1 retry → chat_stream (success)
///   → 2nd invocation happens at ~delay after the 1st.
///
/// Without release (permit held across retries):
///   - call 1 retries before call 2 starts.
///   → 2nd invocation happens at ~delay+backoff after the 1st.
///
/// Asserting `gap < delay + backoff/2` discriminates deterministically:
/// CI slowdown scales both sides equally, but the threshold is a fixed
/// midpoint between `delay` and `delay + backoff`.
#[tokio::test]
async fn permit_released_during_retry_backoff() {
    // max_concurrency=1, fail_after=1 (first call fails, retry succeeds).
    // First call acquires permit, fails, releases permit during backoff.
    // Second concurrent call can then acquire the permit while first is sleeping.
    let delay_ms = 40;
    let backoff_ms = 80;
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_delay(Duration::from_millis(delay_ms))
            .with_fail_after(1, LlmError::Network("boom".into()))
            .with_start_times_tracker(),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let policy = NetworkRetryPolicy {
        max_attempts: 3,
        initial_delay: Duration::from_millis(backoff_ms), // long backoff = window for 2nd call
        multiplier: 1.0,
        max_delay: Duration::from_millis(200),
        jitter_factor: 0.0,
        enabled: true,
        max_attempts_absolute: None,
    };
    let producer = make_producer_with_concurrency_and_retry(provider, 1, Some(policy));
    let p = Arc::new(producer);

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

    let starts = mock.start_times();
    // 3 invocations: call1 attempt1, call2, call1 attempt2 (release case), or
    // call1 attempt1, call1 attempt2, call2 (no-release case).
    assert_eq!(
        starts.len(),
        3,
        "expected 3 chat_stream invocations (2 from retrying call 1 + 1 from call 2), got {}",
        starts.len()
    );
    let gap = starts[1].duration_since(starts[0]);
    // With release: gap ≈ delay (call 2 starts right after call 1 drops permit).
    // Without release: gap ≈ delay + backoff (call 1 retries before call 2).
    // Threshold at delay + backoff/2 sits squarely between the two scenarios.
    let threshold = Duration::from_millis(delay_ms + backoff_ms / 2);
    assert!(
        gap < threshold,
        "permit must be released during backoff — 2nd chat_stream started {gap:?} after the 1st \
         (threshold {threshold:?}); this means call 1 retried before call 2 could start, i.e. the \
         permit was held across the backoff",
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
        max_attempts_absolute: None,
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
        max_attempts_absolute: None,
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
