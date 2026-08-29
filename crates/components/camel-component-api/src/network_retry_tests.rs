use super::*;
use std::time::Duration;

#[test]
fn delay_for_grows_exponentially() {
    let p = NetworkRetryPolicy::default();
    let d0 = p.delay_for(0);
    let d1 = p.delay_for(1);
    let d2 = p.delay_for(2);
    // jitter makes exact values non-deterministic; verify ordering and bounds
    assert!(d0 >= Duration::from_millis(80)); // 100ms - 20% jitter floor
    assert!(d1 > d0);
    assert!(d2 > d1);
}

#[test]
fn delay_for_is_capped_at_max_delay() {
    let p = NetworkRetryPolicy::default();
    let d_high = p.delay_for(100);
    assert!(d_high <= p.max_delay);
}

#[test]
fn should_retry_false_when_disabled() {
    let p = NetworkRetryPolicy::disabled();
    assert!(!p.should_retry(0));
}

#[test]
fn should_retry_false_when_max_attempts_reached() {
    let p = NetworkRetryPolicy {
        max_attempts: 3,
        ..NetworkRetryPolicy::default()
    };
    assert!(p.should_retry(2));
    assert!(!p.should_retry(3));
}

#[test]
fn disabled_returns_disabled_policy() {
    let p = NetworkRetryPolicy::disabled();
    assert!(!p.enabled);
}

#[tokio::test]
async fn retry_async_succeeds_on_first_attempt() {
    let policy = NetworkRetryPolicy::default();
    let count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_clone = count.clone();
    let result = retry_async::<u32, _, _, _, CamelError>(
        &policy,
        "t",
        "op",
        || {
            let c = count_clone.clone();
            async move {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok::<_, CamelError>(42u32)
            }
        },
        |_| false,
        None,
    )
    .await;
    assert_eq!(result.unwrap(), 42);
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retry_async_retries_on_transient_error() {
    let policy = NetworkRetryPolicy {
        max_attempts: 3,
        ..NetworkRetryPolicy::default()
    };
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let attempts_clone = attempts.clone();
    let result = retry_async::<u32, _, _, _, CamelError>(
        &policy,
        "t",
        "op",
        || {
            let c = attempts_clone.clone();
            async move {
                let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n < 2 {
                    Err(CamelError::ProcessorError("transient".to_string()))
                } else {
                    Ok(99u32)
                }
            }
        },
        |_| true, // all errors transient
        None,
    )
    .await;
    assert_eq!(result.unwrap(), 99);
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[tokio::test]
async fn retry_async_does_not_retry_permanent_error() {
    let policy = NetworkRetryPolicy {
        max_attempts: 5,
        ..NetworkRetryPolicy::default()
    };
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let attempts_clone = attempts.clone();
    let result = retry_async::<u32, _, _, _, CamelError>(
        &policy,
        "t",
        "op",
        || {
            let c = attempts_clone.clone();
            async move {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(CamelError::ProcessorError("permanent".to_string()))
            }
        },
        |_| false, // no errors transient
        None,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retry_async_exhausts_max_attempts() {
    let policy = NetworkRetryPolicy {
        max_attempts: 3,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        ..NetworkRetryPolicy::default()
    };
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let attempts_clone = attempts.clone();
    let result = retry_async::<u32, _, _, _, CamelError>(
        &policy,
        "t",
        "op",
        || {
            let c = attempts_clone.clone();
            async move {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(CamelError::ProcessorError("always fails".to_string()))
            }
        },
        |_| true,
        None,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[test]
fn deserializes_from_toml_with_ms_fields() {
    let toml_str = r#"
            max_attempts = 5
            initial_delay_ms = 200
            max_delay_ms = 60000
            multiplier = 1.5
            jitter_factor = 0.1
        "#;
    let p: NetworkRetryPolicy = toml::from_str(toml_str).unwrap();
    assert_eq!(p.max_attempts, 5);
    assert_eq!(p.initial_delay, Duration::from_millis(200));
    assert_eq!(p.max_delay, Duration::from_millis(60_000));
    assert!((p.multiplier - 1.5).abs() < f64::EPSILON);
    assert!((p.jitter_factor - 0.1).abs() < f64::EPSILON);
}

#[tokio::test]
async fn retry_async_does_not_retry_when_disabled() {
    let policy = NetworkRetryPolicy::disabled();
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let attempts_clone = attempts.clone();
    let result = retry_async::<u32, _, _, _, CamelError>(
        &policy,
        "t",
        "op",
        || {
            let c = attempts_clone.clone();
            async move {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(CamelError::ProcessorError("always fails".to_string()))
            }
        },
        |_| true,
        None,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn should_retry_with_max_attempts_zero_is_unlimited() {
    let p = NetworkRetryPolicy {
        max_attempts: 0,
        ..NetworkRetryPolicy::default()
    };
    assert!(p.should_retry(0));
    assert!(p.should_retry(100));
    assert!(p.should_retry(10_000));
}

#[test]
fn delay_for_with_zero_jitter_is_exact_exponential() {
    let p = NetworkRetryPolicy {
        initial_delay: Duration::from_millis(10),
        multiplier: 2.0,
        max_delay: Duration::from_millis(100),
        jitter_factor: 0.0,
        ..NetworkRetryPolicy::default()
    };
    assert_eq!(p.delay_for(0), Duration::from_millis(10));
    assert_eq!(p.delay_for(1), Duration::from_millis(20));
    assert_eq!(p.delay_for(2), Duration::from_millis(40));
    assert_eq!(p.delay_for(3), Duration::from_millis(80));
    assert_eq!(p.delay_for(4), Duration::from_millis(100));
}

// ── Generic retry_async with custom error type ──────────────────────

#[derive(Debug, PartialEq)]
struct TestError(bool); // bool = is_retryable

impl std::fmt::Display for TestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TestError({})", self.0)
    }
}

#[tokio::test]
async fn retry_async_works_with_custom_error_type() {
    use std::sync::atomic::{AtomicU32, Ordering};

    let policy = NetworkRetryPolicy {
        max_attempts: 2,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        ..NetworkRetryPolicy::default()
    };

    let calls = std::sync::Arc::new(AtomicU32::new(0));
    let calls_clone = calls.clone();
    let result: Result<(), TestError> = retry_async(
        &policy,
        "t",
        "op",
        || {
            let c = calls_clone.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err(TestError(true))
            }
        },
        |e: &TestError| e.0,
        None,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn retry_async_custom_error_permanent_stops_immediately() {
    use std::sync::atomic::{AtomicU32, Ordering};

    let policy = NetworkRetryPolicy {
        max_attempts: 5,
        initial_delay: Duration::from_millis(1),
        ..NetworkRetryPolicy::default()
    };

    let calls = std::sync::Arc::new(AtomicU32::new(0));
    let calls_clone = calls.clone();
    let result: Result<(), TestError> = retry_async(
        &policy,
        "t",
        "op",
        || {
            let c = calls_clone.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err(TestError(false)) // permanent
            }
        },
        |e: &TestError| e.0,
        None,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

// ── max_attempts_absolute cap ────────────────────────────────────────

#[test]
fn should_retry_capped_by_absolute_max() {
    let p = NetworkRetryPolicy {
        max_attempts: 0, // unlimited by max_attempts
        max_attempts_absolute: Some(5),
        ..NetworkRetryPolicy::default()
    };
    assert!(p.should_retry(0));
    assert!(p.should_retry(4));
    assert!(!p.should_retry(5));
    assert!(!p.should_retry(100));
}

#[test]
fn should_retry_absolute_default_none_preserves_unlimited() {
    let p = NetworkRetryPolicy {
        max_attempts: 0,
        max_attempts_absolute: None,
        ..NetworkRetryPolicy::default()
    };
    assert!(p.should_retry(0));
    assert!(p.should_retry(100));
    assert!(p.should_retry(10_000));
}

#[test]
fn should_retry_absolute_cap_respected_even_with_limited_max_attempts() {
    // When both are set, the stricter bound wins
    let p = NetworkRetryPolicy {
        max_attempts: 10,
        max_attempts_absolute: Some(3),
        ..NetworkRetryPolicy::default()
    };
    assert!(p.should_retry(0));
    assert!(p.should_retry(2));
    assert!(!p.should_retry(3)); // absolute cap stops first
}

#[tokio::test]
async fn retry_async_stops_at_absolute_cap() {
    use std::sync::atomic::{AtomicU32, Ordering};

    let policy = NetworkRetryPolicy {
        max_attempts: 0, // unlimited
        max_attempts_absolute: Some(5),
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        ..NetworkRetryPolicy::default()
    };
    let attempts = std::sync::Arc::new(AtomicU32::new(0));
    let attempts_clone = attempts.clone();
    let result = retry_async::<u32, _, _, _, CamelError>(
        &policy,
        "t",
        "op",
        || {
            let c = attempts_clone.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err(CamelError::ProcessorError("always fails".to_string()))
            }
        },
        |_| true,
        None,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(attempts.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn retry_async_absolute_default_none_allows_unlimited_retries_up_to_max_attempts() {
    use std::sync::atomic::{AtomicU32, Ordering};

    let policy = NetworkRetryPolicy {
        max_attempts: 3,
        max_attempts_absolute: None,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        ..NetworkRetryPolicy::default()
    };
    let attempts = std::sync::Arc::new(AtomicU32::new(0));
    let attempts_clone = attempts.clone();
    let result = retry_async::<u32, _, _, _, CamelError>(
        &policy,
        "t",
        "op",
        || {
            let c = attempts_clone.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err(CamelError::ProcessorError("always fails".to_string()))
            }
        },
        |_| true,
        None,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
}

// ── is_retryable_camel_error ────────────────────────────────────────

#[test]
fn is_retryable_camel_error_io_is_retryable() {
    let err = CamelError::Io("connection refused".to_string());
    assert!(is_retryable_camel_error(&err));
}

#[test]
fn is_retryable_camel_error_transient_marker_is_retryable() {
    let err = CamelError::ProcessorError("grpc[TRANSIENT][UNAVAILABLE]: down".to_string());
    assert!(is_retryable_camel_error(&err));
}

#[test]
fn is_retryable_camel_error_config_is_not_retryable() {
    let err = CamelError::Config("bad config".to_string());
    assert!(!is_retryable_camel_error(&err));
}

#[test]
fn is_retryable_camel_error_processor_error_no_marker_is_not_retryable() {
    let err = CamelError::ProcessorError("something went wrong".to_string());
    assert!(!is_retryable_camel_error(&err));
}

#[test]
fn is_retryable_camel_error_processor_with_source_transient_marker_is_retryable() {
    let err = CamelError::ProcessorErrorWithSource(
        "grpc[TRANSIENT][UNAVAILABLE]: down".to_string(),
        std::sync::Arc::new(std::io::Error::other("underlying cause")),
    );
    assert!(is_retryable_camel_error(&err));
}

#[test]
fn is_retryable_camel_error_processor_with_source_no_marker_is_not_retryable() {
    let err = CamelError::ProcessorErrorWithSource(
        "some non-transient error".to_string(),
        std::sync::Arc::new(std::io::Error::other("underlying cause")),
    );
    assert!(!is_retryable_camel_error(&err));
}

// ── is_retryable_camel_error via retry_async ─────────────────────────

#[tokio::test]
async fn retry_async_with_is_retryable_camel_error_retries_on_io_error() {
    use std::sync::atomic::{AtomicU32, Ordering};

    let policy = NetworkRetryPolicy {
        max_attempts: 3,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        ..NetworkRetryPolicy::default()
    };

    let calls = std::sync::Arc::new(AtomicU32::new(0));
    let calls_clone = calls.clone();
    let result = retry_async(
        &policy,
        "t",
        "op",
        || {
            let c = calls_clone.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err::<u32, _>(CamelError::Io("connection refused".to_string()))
            }
        },
        is_retryable_camel_error,
        None,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn retry_async_with_is_retryable_camel_error_stops_on_permanent_camel_error() {
    use std::sync::atomic::{AtomicU32, Ordering};

    let policy = NetworkRetryPolicy {
        max_attempts: 5,
        initial_delay: Duration::from_millis(1),
        ..NetworkRetryPolicy::default()
    };

    let calls = std::sync::Arc::new(AtomicU32::new(0));
    let calls_clone = calls.clone();
    let result = retry_async(
        &policy,
        "t",
        "op",
        || {
            let c = calls_clone.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err::<u32, _>(CamelError::Config("permanent".to_string()))
            }
        },
        is_retryable_camel_error,
        None,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

// ── retry_async_cancelable tests ─────────────────────────────────────

#[tokio::test]
async fn retry_async_cancelable_succeeds_without_cancellation() {
    use std::sync::atomic::{AtomicU32, Ordering};

    let policy = NetworkRetryPolicy {
        max_attempts: 5,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        ..NetworkRetryPolicy::default()
    };
    let cancel = tokio_util::sync::CancellationToken::new();

    let attempts = std::sync::Arc::new(AtomicU32::new(0));
    let attempts_clone = attempts.clone();
    let result: Result<u32, CamelError> = retry_async_cancelable(
        &policy,
        "t",
        "op",
        || {
            let c = attempts_clone.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(CamelError::ProcessorError("transient".to_string()))
                } else {
                    Ok(99u32)
                }
            }
        },
        |_| true,
        &cancel,
        None,
    )
    .await;
    assert_eq!(result.unwrap(), 99);
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn retry_async_cancelable_exhausts_attempts_when_cancel_not_signaled() {
    use std::sync::atomic::{AtomicU32, Ordering};

    let policy = NetworkRetryPolicy {
        max_attempts: 2,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        ..NetworkRetryPolicy::default()
    };
    // Token created but never cancelled — should behave like regular retry_async
    let cancel = tokio_util::sync::CancellationToken::new();

    let calls = std::sync::Arc::new(AtomicU32::new(0));
    let calls_clone = calls.clone();
    let result: Result<u32, CamelError> = retry_async_cancelable(
        &policy,
        "t",
        "op",
        || {
            let c = calls_clone.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err(CamelError::ProcessorError("transient".to_string()))
            }
        },
        |_| true,
        &cancel,
        None,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn retry_async_cancelable_returns_last_error_when_cancelled_during_sleep() {
    use std::sync::atomic::{AtomicU32, Ordering};

    // Long delay so cancel is guaranteed to fire during the sleep,
    // not before op() runs or after it wakes.
    let policy = NetworkRetryPolicy {
        max_attempts: 5,
        initial_delay: Duration::from_secs(60),
        max_delay: Duration::from_secs(60),
        multiplier: 1.0,
        ..NetworkRetryPolicy::default()
    };
    let cancel = tokio_util::sync::CancellationToken::new();

    let calls = std::sync::Arc::new(AtomicU32::new(0));
    let calls_clone = calls.clone();
    let op_ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let op_ran_clone = op_ran.clone();
    let cancel_clone = cancel.clone();

    // Spawn cancel task: poll until op signals it has run (and is about to
    // return Err), THEN cancel. The retry will be in its sleep(60s) at
    // this point. Eliminates the race in the previous spawn+sleep(1ms) approach.
    let cancel_task = tokio::spawn(async move {
        while !op_ran_clone.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        cancel_clone.cancel();
    });

    let result: Result<u32, CamelError> = retry_async_cancelable(
        &policy,
        "t",
        "op",
        || {
            let c = calls_clone.clone();
            let flag = op_ran.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                Err(CamelError::Io("connection refused".to_string()))
            }
        },
        |_| true,
        &cancel,
        None,
    )
    .await;

    cancel_task.await.ok();

    assert!(
        result.is_err(),
        "cancel should cause early return with error"
    );
    // Exactly one op call — cancel fired during first sleep, no second attempt
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retry_async_cancelable_honors_cancel_signalled_during_op_at_next_sleep() {
    // Per docs: cancel is checked during inter-retry sleep, NOT during op itself.
    // If cancel fires while op is in-flight, op completes normally (proving
    // cancel didn't kill it), then the next sleep boundary observes the
    // cancel and aborts with the last operation error.
    use std::sync::atomic::{AtomicU32, Ordering};

    let policy = NetworkRetryPolicy {
        max_attempts: 5,
        initial_delay: Duration::from_secs(60),
        max_delay: Duration::from_secs(60),
        multiplier: 1.0,
        ..NetworkRetryPolicy::default()
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();

    let calls = std::sync::Arc::new(AtomicU32::new(0));
    let calls_clone = calls.clone();

    // Cancel fires during op's 20ms sleep (at ~5ms)
    let cancel_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(5)).await;
        cancel_clone.cancel();
    });

    let result: Result<u32, CamelError> = retry_async_cancelable(
        &policy,
        "t",
        "op",
        || {
            let c = calls_clone.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                // Slow op: sleeps 20ms then returns retryable error.
                // Cancel fires at ~5ms during this sleep but op completes.
                tokio::time::sleep(Duration::from_millis(20)).await;
                Err(CamelError::Io("transient".to_string()))
            }
        },
        |_| true,
        &cancel,
        None,
    )
    .await;

    cancel_task.await.ok();

    // Op was called once and completed its 20ms sleep (cancel didn't kill it);
    // then the next sleep(60s) boundary observed cancel and aborted.
    assert!(
        result.is_err(),
        "cancel during op must abort at next sleep boundary"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "op ran exactly once — cancel honored at sleep, not during op"
    );
}

#[tokio::test]
async fn retry_async_cancelable_does_not_retry_permanent_error() {
    use std::sync::atomic::{AtomicU32, Ordering};

    let policy = NetworkRetryPolicy {
        max_attempts: 5,
        initial_delay: Duration::from_millis(1),
        ..NetworkRetryPolicy::default()
    };
    let cancel = tokio_util::sync::CancellationToken::new();

    let calls = std::sync::Arc::new(AtomicU32::new(0));
    let calls_clone = calls.clone();
    let result: Result<u32, CamelError> = retry_async_cancelable(
        &policy,
        "t",
        "op",
        || {
            let c = calls_clone.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err(CamelError::ProcessorError("permanent".to_string()))
            }
        },
        |_| false, // no errors are retryable
        &cancel,
        None,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

// ── Labeled retry observability tests ───────────────────────────────

use std::fmt::Write as FmtWrite;
use std::sync::{Arc, Mutex};
use tracing::Subscriber;
use tracing_subscriber::layer::SubscriberExt;

/// A tracing `Layer` that captures formatted event field key=value pairs
/// into a shared buffer.
struct CollectingLayer {
    events: Arc<Mutex<Vec<String>>>,
}

impl<S: Subscriber> tracing_subscriber::Layer<S> for CollectingLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut buf = String::new();
        let mut visitor = CollectingVisitor { fields: &mut buf };
        event.record(&mut visitor);
        if let Ok(mut events) = self.events.lock() {
            events.push(buf);
        }
    }
}

struct CollectingVisitor<'a> {
    fields: &'a mut String,
}

impl CollectingVisitor<'_> {
    fn record_field(&mut self, name: &str, value: &str) {
        write!(self.fields, " {name}={value}").ok();
    }
}

impl tracing::field::Visit for CollectingVisitor<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.record_field(field.name(), value);
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.record_field(field.name(), &format!("{value:?}"));
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.record_field(field.name(), &value.to_string());
    }
}

#[tokio::test]
async fn labeled_policy_emits_component_in_log_message() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let layer = CollectingLayer {
        events: events.clone(),
    };
    let subscriber = tracing_subscriber::registry().with(layer);
    let _guard = tracing::subscriber::set_default(subscriber);

    let policy = NetworkRetryPolicy {
        max_attempts: 2,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        ..NetworkRetryPolicy::default()
    };

    let result = retry_async::<u32, _, _, _, CamelError>(
        &policy,
        "ws",
        "connect",
        || async { Err(CamelError::Io("connection refused".to_string())) },
        |_| true,
        None,
    )
    .await;

    assert!(result.is_err());
    let captured = events.lock().unwrap();
    assert!(
        !captured.is_empty(),
        "expected at least one log event, got none"
    );
    // The first (and only) retry should carry the scheme/operation identity
    let first = &captured[0];
    assert!(
        first.contains("scheme=ws"),
        "expected 'scheme=ws' in log event, got: {first}"
    );
    assert!(
        first.contains("operation=connect"),
        "expected 'operation=connect' in log event, got: {first}"
    );
    assert!(
        first.contains("ws/connect: transient error"),
        "expected 'ws/connect: transient error' in log event, got: {first}"
    );
}

#[tokio::test]
async fn labeled_policy_preserves_structured_fields() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let layer = CollectingLayer {
        events: events.clone(),
    };
    let subscriber = tracing_subscriber::registry().with(layer);
    let _guard = tracing::subscriber::set_default(subscriber);

    let policy = NetworkRetryPolicy {
        max_attempts: 2,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        ..NetworkRetryPolicy::default()
    };

    let result = retry_async::<u32, _, _, _, CamelError>(
        &policy,
        "sql",
        "producer",
        || async { Err(CamelError::Io("timeout".to_string())) },
        |_| true,
        None,
    )
    .await;

    assert!(result.is_err());
    let captured = events.lock().unwrap();
    assert!(!captured.is_empty());
    let first = &captured[0];
    // Structured fields must be present
    assert!(
        first.contains("attempt=0"),
        "expected 'attempt=0' field, got: {first}"
    );
    assert!(
        first.contains("scheme=sql"),
        "expected 'scheme=sql' field, got: {first}"
    );
    assert!(
        first.contains("operation=producer"),
        "expected 'operation=producer' field, got: {first}"
    );
    assert!(
        first.contains("error=IO error: timeout"),
        "expected 'error=IO error: timeout' field, got: {first}"
    );
    assert!(
        first.contains("delay_ms="),
        "expected 'delay_ms=' field, got: {first}"
    );
}

#[tokio::test]
async fn unlabeled_policy_omits_component_field() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let layer = CollectingLayer {
        events: events.clone(),
    };
    let subscriber = tracing_subscriber::registry().with(layer);
    let _guard = tracing::subscriber::set_default(subscriber);

    let policy = NetworkRetryPolicy {
        max_attempts: 2,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        ..NetworkRetryPolicy::default()
    };
    // The old `component=` field is gone — identity now travels as
    // scheme/operation fields.
    let result = retry_async::<u32, _, _, _, CamelError>(
        &policy,
        "t",
        "op",
        || async { Err(CamelError::Io("connection refused".to_string())) },
        |_| true,
        None,
    )
    .await;

    assert!(result.is_err());
    let captured = events.lock().unwrap();
    assert!(!captured.is_empty());
    let first = &captured[0];
    assert!(
        !first.contains("component="),
        "expected no 'component=' field in unlabeled path, got: {first}"
    );
    assert!(
        first.contains("transient error — retrying"),
        "expected bare 'transient error — retrying' message, got: {first}"
    );
}

// ── Retry metrics accounting (one error per exhausted sequence) ──────

/// Recording test double: captures `(scheme, operation)` retry-attempt
/// calls and `(route_id, error_type)` error calls.
struct RecordingMetrics {
    attempts: Mutex<Vec<(String, String)>>,
    errors: Mutex<Vec<(String, String)>>,
}

impl RecordingMetrics {
    fn new() -> Self {
        Self {
            attempts: Mutex::new(Vec::new()),
            errors: Mutex::new(Vec::new()),
        }
    }
}

impl camel_api::MetricsCollector for RecordingMetrics {
    fn record_exchange_duration(&self, _route_id: &str, _duration: std::time::Duration) {}
    fn increment_errors(&self, route_id: &str, error_type: &str) {
        self.errors
            .lock()
            .unwrap()
            .push((route_id.to_string(), error_type.to_string()));
    }
    fn increment_exchanges(&self, _route_id: &str) {}
    fn set_queue_depth(&self, _route_id: &str, _depth: usize) {}
    fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {}
    fn increment_retry_attempt(&self, scheme: &str, operation: &str) {
        self.attempts
            .lock()
            .unwrap()
            .push((scheme.to_string(), operation.to_string()));
    }
}

#[tokio::test]
async fn attempts_counted_per_try() {
    use std::sync::atomic::{AtomicU32, Ordering};

    let policy = NetworkRetryPolicy {
        max_attempts: 3,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        ..NetworkRetryPolicy::default()
    };
    let metrics = RecordingMetrics::new();

    let attempts = Arc::new(AtomicU32::new(0));
    let attempts_clone = attempts.clone();
    let result: Result<u32, CamelError> = retry_async(
        &policy,
        "t",
        "op",
        || {
            let c = attempts_clone.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(CamelError::Io("transient".to_string()))
                } else {
                    Ok(1u32)
                }
            }
        },
        |_| true,
        Some(&metrics),
    )
    .await;

    assert_eq!(result.unwrap(), 1);
    assert_eq!(
        *metrics.attempts.lock().unwrap(),
        vec![
            ("t".to_string(), "op".to_string()),
            ("t".to_string(), "op".to_string()),
            ("t".to_string(), "op".to_string()),
        ],
        "every attempt (first included) must be recorded"
    );
    assert!(
        metrics.errors.lock().unwrap().is_empty(),
        "success after retries must record no errors"
    );
}

#[tokio::test]
async fn exhaustion_errors_once() {
    use std::sync::atomic::{AtomicU32, Ordering};

    let policy = NetworkRetryPolicy {
        max_attempts: 5,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        ..NetworkRetryPolicy::default()
    };
    let metrics = RecordingMetrics::new();

    let attempts = Arc::new(AtomicU32::new(0));
    let attempts_clone = attempts.clone();
    let result: Result<u32, CamelError> = retry_async(
        &policy,
        "t",
        "op",
        || {
            let c = attempts_clone.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err(CamelError::Io("always fails".to_string()))
            }
        },
        |_| true,
        Some(&metrics),
    )
    .await;

    assert!(result.is_err());
    assert_eq!(
        metrics.attempts.lock().unwrap().len(),
        5,
        "all five attempts must be recorded"
    );
    assert_eq!(
        *metrics.errors.lock().unwrap(),
        vec![("op".to_string(), "e:t:op".to_string())],
        "exhaustion must record exactly one error with operation label and e:scheme:operation type"
    );
}

#[tokio::test]
async fn non_retryable_errors_once_first_attempt() {
    let policy = NetworkRetryPolicy {
        max_attempts: 5,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        ..NetworkRetryPolicy::default()
    };
    let metrics = RecordingMetrics::new();

    let result: Result<u32, CamelError> = retry_async(
        &policy,
        "t",
        "op",
        || async { Err(CamelError::Config("permanent".to_string())) },
        |_| false,
        Some(&metrics),
    )
    .await;

    assert!(result.is_err());
    assert_eq!(metrics.attempts.lock().unwrap().len(), 1);
    assert_eq!(
        *metrics.errors.lock().unwrap(),
        vec![("op".to_string(), "e:t:op".to_string())],
        "non-retryable first-fail must record exactly one error"
    );
}

#[tokio::test]
async fn cancelled_mid_sequence_no_error() {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    // Long delay so cancel fires during the inter-retry sleep.
    let policy = NetworkRetryPolicy {
        max_attempts: 5,
        initial_delay: Duration::from_secs(60),
        max_delay: Duration::from_secs(60),
        multiplier: 1.0,
        ..NetworkRetryPolicy::default()
    };
    let metrics = RecordingMetrics::new();
    let cancel = tokio_util::sync::CancellationToken::new();

    let calls = Arc::new(AtomicU32::new(0));
    let calls_clone = calls.clone();
    let op_ran = Arc::new(AtomicBool::new(false));
    let op_ran_clone = op_ran.clone();
    let cancel_clone = cancel.clone();

    // Cancel after the first attempt signals it has run (op is pending in
    // the 60s sleep when the token fires).
    let cancel_task = tokio::spawn(async move {
        while !op_ran_clone.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        cancel_clone.cancel();
    });

    let result: Result<u32, CamelError> = retry_async_cancelable(
        &policy,
        "t",
        "op",
        || {
            let c = calls_clone.clone();
            let flag = op_ran.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                flag.store(true, Ordering::SeqCst);
                Err(CamelError::Io("transient".to_string()))
            }
        },
        |_| true,
        &cancel,
        Some(&metrics),
    )
    .await;

    cancel_task.await.ok();

    assert!(result.is_err());
    assert!(
        metrics.errors.lock().unwrap().is_empty(),
        "cancellation is a clean shutdown, not a failure"
    );
    assert_eq!(metrics.attempts.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn none_metrics_is_fine() {
    let policy = NetworkRetryPolicy {
        max_attempts: 2,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        ..NetworkRetryPolicy::default()
    };

    let result: Result<u32, CamelError> = retry_async(
        &policy,
        "t",
        "op",
        || async { Err(CamelError::Io("always fails".to_string())) },
        |_| true,
        None,
    )
    .await;

    assert!(result.is_err(), "error must be returned unchanged");
}
