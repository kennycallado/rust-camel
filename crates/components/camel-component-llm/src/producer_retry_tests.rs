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
        max_delay: Duration::from_millis(200),
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
// Composition: Retry-After sleeps are capped
// -----------------------------------------------------------------------

/// A header far above `max_delay` is capped, retried until attempts exhaust,
/// and surfaced honestly naming both values.
///
/// 3 attempts, 2 capped sleeps of 60s: inside the retry policy alone the
/// virtual clock advances 120s, never the 86400s the header asked for.
#[tokio::test(start_paused = true)]
async fn capped_retry_after_sleeps_cap_and_names_both_values() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_rate_limit(Some(Duration::from_secs(86_400))),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let policy = NetworkRetryPolicy {
        enabled: true,
        max_attempts: 3,
        initial_delay: Duration::from_millis(1),
        multiplier: 1.0,
        max_delay: Duration::from_secs(60),
        jitter_factor: 0.0,
        max_attempts_absolute: None,
    };
    let mut producer = make_producer_with_retry(Arc::clone(&provider), false, Some(policy));
    let start = tokio::time::Instant::now();
    let result = producer.call(make_exchange(Body::Text("x".into()))).await;
    let elapsed = start.elapsed();
    let msg = match result {
        Ok(_) => panic!("exhausted retries must error"),
        Err(e) => e.to_string(),
    };
    assert!(msg.contains("86400s"), "missing requested value: {msg}");
    assert!(msg.contains("60s"), "missing cap value: {msg}");
    assert!(
        elapsed >= Duration::from_secs(120),
        "two 60s capped sleeps must elapse, got {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(200),
        "sleeps must use the 60s cap, not the 86400s header, got {elapsed:?}"
    );
    assert_eq!(mock.call_count(), 3);
}

/// A header below the cap is honoured as-is and the error is not annotated.
#[tokio::test(start_paused = true)]
async fn retry_after_under_cap_is_honoured_unchanged() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_rate_limit(Some(Duration::from_secs(3))),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let policy = NetworkRetryPolicy {
        enabled: true,
        max_attempts: 3,
        initial_delay: Duration::from_millis(1),
        multiplier: 1.0,
        max_delay: Duration::from_secs(60),
        jitter_factor: 0.0,
        max_attempts_absolute: None,
    };
    let mut producer = make_producer_with_retry(Arc::clone(&provider), false, Some(policy));
    let start = tokio::time::Instant::now();
    let result = producer.call(make_exchange(Body::Text("x".into()))).await;
    let elapsed = start.elapsed();
    let msg = match result {
        Ok(_) => panic!("exhausted retries must error"),
        Err(e) => e.to_string(),
    };
    assert!(
        elapsed >= Duration::from_secs(6),
        "two 3s header sleeps must elapse, got {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "header sleeps must stay uncapped, got {elapsed:?}"
    );
    assert!(msg.contains("rate limited"), "wrong error: {msg}");
    assert!(!msg.contains("exceeds the"), "must not be annotated: {msg}");
    assert_eq!(mock.call_count(), 3);
}

/// A header exactly equal to the cap is honoured, not annotated.
#[tokio::test(start_paused = true)]
async fn retry_after_equal_to_cap_is_honoured_not_annotated() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_rate_limit(Some(Duration::from_secs(3))),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let policy = NetworkRetryPolicy {
        enabled: true,
        max_attempts: 3,
        initial_delay: Duration::from_millis(1),
        multiplier: 1.0,
        max_delay: Duration::from_secs(3),
        jitter_factor: 0.0,
        max_attempts_absolute: None,
    };
    let mut producer = make_producer_with_retry(Arc::clone(&provider), false, Some(policy));
    let start = tokio::time::Instant::now();
    let result = producer.call(make_exchange(Body::Text("x".into()))).await;
    let elapsed = start.elapsed();
    let msg = match result {
        Ok(_) => panic!("exhausted retries must error"),
        Err(e) => e.to_string(),
    };
    assert!(
        elapsed >= Duration::from_secs(6),
        "two 3s sleeps must elapse, got {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "sleeps must stay at the header value, got {elapsed:?}"
    );
    assert!(!msg.contains("exceeds the"), "must not be annotated: {msg}");
    assert_eq!(mock.call_count(), 3);
}

/// A rate limit WITHOUT a header keeps the exponential backoff schedule.
#[tokio::test(start_paused = true)]
async fn rate_limit_without_retry_after_keeps_exponential_backoff() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_fail_after(1, LlmError::RateLimit { retry_after: None }),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let policy = NetworkRetryPolicy {
        enabled: true,
        max_attempts: 3,
        initial_delay: Duration::from_millis(100),
        multiplier: 2.0,
        max_delay: Duration::from_secs(60),
        jitter_factor: 0.0,
        max_attempts_absolute: None,
    };
    let mut producer = make_producer_with_retry(Arc::clone(&provider), false, Some(policy));
    let start = tokio::time::Instant::now();
    let result = producer.call(make_exchange(Body::Text("x".into()))).await;
    let elapsed = start.elapsed();
    let out = match result {
        Ok(out) => out,
        Err(e) => panic!("retry must succeed, got: {e}"),
    };
    assert!(matches!(out.input.body, Body::Text(_)));
    assert_eq!(mock.call_count(), 2);
    assert!(
        elapsed >= Duration::from_millis(100),
        "exponential backoff must still elapse, got {elapsed:?}"
    );
}

/// The remaining total deadline bounds the Retry-After sleep: a header above
/// the remaining budget sleeps only to the deadline, then surfaces `Timeout`
/// without launching another request.
#[tokio::test(start_paused = true)]
async fn retry_after_sleep_capped_at_remaining_deadline() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_rate_limit(Some(Duration::from_secs(86_400))),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let policy = NetworkRetryPolicy {
        enabled: true,
        max_attempts: 3,
        initial_delay: Duration::from_millis(1),
        multiplier: 1.0,
        max_delay: Duration::from_secs(60),
        jitter_factor: 0.0,
        max_attempts_absolute: None,
    };
    let mut producer = make_producer_with_timeout_and_retry(
        provider,
        /*stream=*/ false,
        Duration::from_secs(10),
        Some(policy),
    );
    let result = producer.call(make_exchange(Body::Text("x".into()))).await;
    let msg = match result {
        Ok(_) => panic!("remaining deadline must surface a timeout"),
        Err(e) => e.to_string(),
    };
    assert!(msg.contains("timeout"), "wrong error: {msg}");
    assert!(msg.contains("10s"), "must name the deadline: {msg}");
    assert_eq!(
        mock.call_count(),
        1,
        "no request may launch at the deadline"
    );
}

/// A positive header under a zero cap returns immediately with the annotated
/// error — no sleep, no further attempt (guards against the unlimited-policy
/// hang the old code had: `max_attempts == 0` means unlimited).
#[tokio::test(start_paused = true)]
async fn zero_max_delay_positive_header_returns_immediately() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_rate_limit(Some(Duration::from_secs(86_400))),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let policy = NetworkRetryPolicy {
        enabled: true,
        max_attempts: 0, // unlimited
        initial_delay: Duration::from_millis(1),
        multiplier: 1.0,
        max_delay: Duration::ZERO,
        jitter_factor: 0.0,
        max_attempts_absolute: None,
    };
    let mut producer = make_producer_with_retry(Arc::clone(&provider), false, Some(policy));
    let result = match tokio::time::timeout(
        Duration::from_secs(1),
        producer.call(make_exchange(Body::Text("x".into()))),
    )
    .await
    {
        Ok(r) => r,
        Err(_elapsed) => panic!("call hung: zero cap did not short-circuit"),
    };
    let msg = match result {
        Ok(_) => panic!("positive header under a zero cap must error"),
        Err(e) => e.to_string(),
    };
    assert!(msg.contains("86400s"), "missing requested value: {msg}");
    assert!(msg.contains("0s"), "missing cap value: {msg}");
    assert_eq!(mock.call_count(), 1);
}

/// A zero header under a zero cap is retried normally: the zero-cap
/// short-circuit only applies to a POSITIVE header (rule ORDER guard).
#[tokio::test(start_paused = true)]
async fn zero_retry_after_under_zero_cap_keeps_raw_error() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into())).with_rate_limit(Some(Duration::ZERO)),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let policy = NetworkRetryPolicy {
        enabled: true,
        max_attempts: 2,
        initial_delay: Duration::from_millis(1),
        multiplier: 1.0,
        max_delay: Duration::ZERO,
        jitter_factor: 0.0,
        max_attempts_absolute: None,
    };
    let mut producer = make_producer_with_retry(Arc::clone(&provider), false, Some(policy));
    let result = producer.call(make_exchange(Body::Text("x".into()))).await;
    let msg = match result {
        Ok(_) => panic!("exhausted retries must error"),
        Err(e) => e.to_string(),
    };
    assert!(msg.contains("rate limited"), "wrong error: {msg}");
    assert!(!msg.contains("exceeds the"), "must not be annotated: {msg}");
    assert_eq!(mock.call_count(), 2);
}

/// The embed path goes through the same retry loop, so the cap applies there
/// too.
#[tokio::test(start_paused = true)]
async fn embed_retry_after_cap_applies() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_rate_limit(Some(Duration::from_secs(86_400))),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let policy = NetworkRetryPolicy {
        enabled: true,
        max_attempts: 3,
        initial_delay: Duration::from_millis(1),
        multiplier: 1.0,
        max_delay: Duration::from_secs(60),
        jitter_factor: 0.0,
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
    let start = tokio::time::Instant::now();
    let result = producer.call(make_exchange(Body::Text("x".into()))).await;
    let elapsed = start.elapsed();
    let msg = match result {
        Ok(_) => panic!("exhausted retries must error"),
        Err(e) => e.to_string(),
    };
    assert!(msg.contains("86400s"), "missing requested value: {msg}");
    assert!(msg.contains("60s"), "missing cap value: {msg}");
    assert!(
        elapsed >= Duration::from_secs(120),
        "two 60s capped sleeps must elapse, got {elapsed:?}"
    );
    assert_eq!(mock.call_count(), 3);
}

// -----------------------------------------------------------------------
// Composition: outer deadline still cuts a backoff sleep short
// -----------------------------------------------------------------------

/// Regression guard for the retired premise: without a Retry-After header the
/// outer total deadline still cuts a BACKOFF sleep short (behavior via
/// `apply_timeout`, unchanged by the cap).
#[tokio::test(start_paused = true)]
async fn timeout_still_cuts_backoff_sleep() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_fail_after(1, LlmError::Network("boom".into())),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let policy = NetworkRetryPolicy {
        enabled: true,
        max_attempts: 3,
        initial_delay: Duration::from_millis(200),
        multiplier: 1.0,
        max_delay: Duration::from_millis(200),
        jitter_factor: 0.0,
        max_attempts_absolute: None,
    };
    let mut producer = make_producer_with_timeout_and_retry(
        provider,
        /*stream=*/ false,
        Duration::from_millis(50),
        Some(policy),
    );
    let result = producer.call(make_exchange(Body::Text("x".into()))).await;
    let msg = match result {
        Ok(_) => panic!("deadline must cut the 200ms backoff short"),
        Err(e) => e.to_string(),
    };
    assert!(msg.contains("timeout"), "wrong error: {msg}");
    assert!(msg.contains("50ms"), "must name the deadline: {msg}");
    assert!(
        mock.call_count() <= 2,
        "deadline must prevent excessive retries, got: {}",
        mock.call_count()
    );
}

/// 10 attempts x 5ms capped sleeps = 45ms virtual, all inside a 50ms
/// deadline: attempts exhaust before the deadline and the error names both
/// values (the old premise — the deadline cutting a 200ms header sleep — is
/// retired by the cap).
#[tokio::test(start_paused = true)]
async fn capped_retry_after_exhausts_attempts_within_deadline() {
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
    let start = tokio::time::Instant::now();
    let result = producer.call(make_exchange(Body::Text("x".into()))).await;
    let elapsed = start.elapsed();
    let msg = match result {
        Ok(_) => panic!("exhausted retries must error"),
        Err(e) => e.to_string(),
    };
    assert!(msg.contains("retry after"), "wrong error: {msg}");
    assert!(msg.contains("exceeds the"), "wrong error: {msg}");
    assert_eq!(
        mock.call_count(),
        10,
        "all attempts fit inside the deadline"
    );
    assert!(
        elapsed >= Duration::from_millis(45),
        "9 capped sleeps of 5ms must elapse, got {elapsed:?}"
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
