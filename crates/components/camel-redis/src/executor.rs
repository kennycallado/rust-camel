use async_trait::async_trait;
use camel_component_api::{CamelError, Exchange, NetworkRetryPolicy};
// retry_async is used in tests (the regression test in this file).
#[cfg(test)]
use camel_component_api::retry_async;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Mutex;

use crate::config::{RedisCommand, is_transient_redis_error};

/// Abstraction over a Redis connection that can execute commands.
///
/// This trait enables testing retry/reconnect behavior without a live Redis
/// by allowing injection of a fake implementation.
#[async_trait]
pub trait RedisCommandExecutor: Send + Sync {
    /// Execute a Redis command against the given exchange.
    async fn execute_command(
        &mut self,
        cmd: &RedisCommand,
        exchange: &mut Exchange,
    ) -> Result<(), CamelError>;

    /// Reconnect the underlying connection.
    async fn reconnect(&mut self) -> Result<(), CamelError>;
}

/// Fake executor for testing retry behavior.
///
/// Configured with a sequence of results to return. Each call to
/// `execute_command` consumes the next result. After exhausting the
/// sequence, returns `Ok(())`.
pub struct FakeExecutor {
    /// Pre-programmed results: each call pops the next one.
    results: Arc<Mutex<Vec<Result<(), FakeError>>>>,
    /// Counter of how many times `execute_command` was called.
    pub call_count: Arc<AtomicUsize>,
    /// Counter of how many times `reconnect` was called.
    pub reconnect_count: Arc<AtomicUsize>,
}

/// Error type for the fake executor, can be transient or non-transient.
#[derive(Debug, Clone)]
pub struct FakeError {
    pub message: String,
    pub is_transient: bool,
}

impl std::fmt::Display for FakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl FakeExecutor {
    /// Creates a new fake executor with the given result sequence.
    pub fn new(results: Vec<Result<(), FakeError>>) -> Self {
        Self {
            results: Arc::new(Mutex::new(results)),
            call_count: Arc::new(AtomicUsize::new(0)),
            reconnect_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Returns a shared reference to the call counter.
    pub fn call_count(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.call_count)
    }

    /// Returns a shared reference to the reconnect counter.
    pub fn reconnect_count(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.reconnect_count)
    }
}

#[async_trait]
impl RedisCommandExecutor for FakeExecutor {
    async fn execute_command(
        &mut self,
        _cmd: &RedisCommand,
        _exchange: &mut Exchange,
    ) -> Result<(), CamelError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);

        let result = {
            let mut results = self.results.lock().await;
            results.pop().unwrap_or(Ok(()))
        };

        match result {
            Ok(()) => Ok(()),
            Err(fake_err) => {
                if fake_err.is_transient {
                    Err(CamelError::ProcessorError(format!(
                        "Connection error: {}",
                        fake_err.message
                    )))
                } else {
                    Err(CamelError::ProcessorError(fake_err.message))
                }
            }
        }
    }

    async fn reconnect(&mut self) -> Result<(), CamelError> {
        self.reconnect_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Executes a command with retry on transient errors, using `NetworkRetryPolicy`.
///
/// This is the core retry logic extracted for testability.
///
/// - `policy`: reconnection policy (max attempts, backoff, jitter, etc.)
/// - `is_idempotent`: whether the command is safe to retry
///
/// # Implementation note
///
/// Uses a manual retry loop calling [`NetworkRetryPolicy::should_retry`] and
/// [`NetworkRetryPolicy::delay_for`] rather than the shared [`retry_async`]
/// helper. Manual loop needed because: (a) `executor.reconnect()` must run
/// before each retry attempt (not the first attempt), so `retry_async`'s
/// "always invoke op" model doesn't fit; (b) `&mut executor` / `&mut exchange`
/// borrows cannot be re-borrowed through `FnMut() -> async move { ... }` —
/// the Future returned by the closure holds the borrow past the closure body,
/// which the borrow checker rejects. `retry_async_cancelable` has the same
/// FnMut constraint so it is also excluded.
pub async fn execute_with_retry<E: RedisCommandExecutor>(
    executor: &mut E,
    cmd: &RedisCommand,
    exchange: &mut Exchange,
    is_idempotent: bool,
    policy: &NetworkRetryPolicy,
) -> Result<(), CamelError> {
    // Non-idempotent commands must not be retried — use a disabled policy.
    let effective_policy = if is_idempotent {
        policy.clone()
    } else {
        NetworkRetryPolicy::disabled()
    };

    let mut attempt: u32 = 0;

    loop {
        // Reconnect before retries (attempt > 0), not on the initial try.
        // This preserves the existing reconnect-before-retry semantics.
        if attempt > 0 {
            executor.reconnect().await?;
        }

        match executor.execute_command(cmd, exchange).await {
            Ok(()) => return Ok(()),
            Err(err) => {
                if !is_transient_redis_error(&err) || !effective_policy.should_retry(attempt + 1) {
                    return Err(err);
                }
                let delay = effective_policy.delay_for(attempt);
                tracing::warn!(
                    attempt,
                    delay_ms = delay.as_millis(),
                    error = %err,
                    "transient error — retrying"
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn transient_err(msg: &str) -> Result<(), FakeError> {
        Err(FakeError {
            message: msg.to_string(),
            is_transient: true,
        })
    }

    fn non_transient_err(msg: &str) -> Result<(), FakeError> {
        Err(FakeError {
            message: msg.to_string(),
            is_transient: false,
        })
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_transient_failures() {
        // 3 transient failures, then success
        // Results are popped from the back, so we reverse: success first in vec = last popped
        let executor = FakeExecutor::new(vec![
            Ok(()),
            transient_err("connection reset"),
            transient_err("connection reset"),
            transient_err("connection reset"),
        ]);
        let call_count = executor.call_count();
        let reconnect_count = executor.reconnect_count();

        let mut exchange = Exchange::default();
        let cmd = RedisCommand::Get; // idempotent

        let policy = NetworkRetryPolicy {
            max_attempts: 10,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            ..NetworkRetryPolicy::default()
        };

        let result = execute_with_retry(
            &mut { executor },
            &cmd,
            &mut exchange,
            true, // is_idempotent
            &policy,
        )
        .await;

        assert!(result.is_ok(), "should succeed after retries: {:?}", result);
        // 4 execute calls total: 1 initial + 3 retries
        assert_eq!(call_count.load(Ordering::SeqCst), 4);
        // 3 reconnects (one per retry attempt)
        assert_eq!(reconnect_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted_after_max_retries() {
        // 10 transient failures (exhausts all retries)
        let mut results = vec![transient_err("connection refused"); 10];
        results.push(transient_err("final failure")); // one more for the initial call
        let executor = FakeExecutor::new(results);
        let call_count = executor.call_count();

        let mut exchange = Exchange::default();
        let cmd = RedisCommand::Get;

        let policy = NetworkRetryPolicy {
            max_attempts: 11, // 1 initial + 10 retries = 11 total
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            ..NetworkRetryPolicy::default()
        };

        let result =
            execute_with_retry(&mut { executor }, &cmd, &mut exchange, true, &policy).await;

        assert!(result.is_err(), "should fail after exhausting retries");
        // 1 initial + 10 retries = 11 calls
        assert_eq!(call_count.load(Ordering::SeqCst), 11);
    }

    #[tokio::test]
    async fn test_non_idempotent_command_not_retried() {
        let executor = FakeExecutor::new(vec![transient_err("connection reset")]);
        let call_count = executor.call_count();

        let mut exchange = Exchange::default();
        let cmd = RedisCommand::Incr; // NOT idempotent

        let policy = NetworkRetryPolicy {
            max_attempts: 10,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            ..NetworkRetryPolicy::default()
        };

        let result = execute_with_retry(
            &mut { executor },
            &cmd,
            &mut exchange,
            false, // NOT idempotent
            &policy,
        )
        .await;

        assert!(result.is_err(), "non-idempotent should not be retried");
        // Only 1 call — no retries
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_non_transient_error_not_retried() {
        let executor = FakeExecutor::new(vec![non_transient_err("WRONGTYPE")]);
        let call_count = executor.call_count();

        let mut exchange = Exchange::default();
        let cmd = RedisCommand::Get; // idempotent, but error is NOT transient

        let policy = NetworkRetryPolicy {
            max_attempts: 10,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            ..NetworkRetryPolicy::default()
        };

        let result = execute_with_retry(
            &mut { executor },
            &cmd,
            &mut exchange,
            true, // is_idempotent
            &policy,
        )
        .await;

        assert!(result.is_err(), "non-transient error should not be retried");
        // Only 1 call — no retries for non-transient
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_idempotent_command_retried_on_transient() {
        let executor = FakeExecutor::new(vec![
            Ok(()),
            transient_err("EOF"),
            transient_err("timed out"),
        ]);
        let call_count = executor.call_count();

        let mut exchange = Exchange::default();
        let cmd = RedisCommand::Set; // idempotent write

        let policy = NetworkRetryPolicy {
            max_attempts: 10,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            ..NetworkRetryPolicy::default()
        };

        let result =
            execute_with_retry(&mut { executor }, &cmd, &mut exchange, true, &policy).await;

        assert!(result.is_ok());
        // 1 initial + 2 retries = 3 calls
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_immediate_success_no_retries() {
        let executor = FakeExecutor::new(vec![Ok(())]);
        let call_count = executor.call_count();
        let reconnect_count = executor.reconnect_count();

        let mut exchange = Exchange::default();
        let cmd = RedisCommand::Get;

        let policy = NetworkRetryPolicy {
            max_attempts: 10,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            ..NetworkRetryPolicy::default()
        };

        let result =
            execute_with_retry(&mut { executor }, &cmd, &mut exchange, true, &policy).await;

        assert!(result.is_ok());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        assert_eq!(reconnect_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn network_retry_policy_retries_on_transient() {
        let policy = NetworkRetryPolicy {
            max_attempts: 2,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            ..NetworkRetryPolicy::default()
        };

        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result = retry_async::<(), _, _, _, CamelError>(
            &policy,
            || {
                let c = attempts_clone.clone();
                async move {
                    let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if n == 0 {
                        Err(CamelError::ProcessorError("transient".into()))
                    } else {
                        Ok(())
                    }
                }
            },
            |_| true,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}
