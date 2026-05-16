use async_trait::async_trait;
use camel_component_api::{CamelError, Exchange};
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

/// Executes a command with bounded exponential backoff retry on transient errors.
///
/// This is the core retry logic extracted for testability. It mirrors the
/// retry behavior in `RedisProducer::call()`.
///
/// - `max_retries`: maximum number of retry attempts (default 10)
/// - `base_ms`: base delay in milliseconds (default 100)
/// - `max_backoff`: maximum delay cap (default 30s)
/// - `is_idempotent`: whether the command is safe to retry
pub async fn execute_with_retry<E: RedisCommandExecutor>(
    executor: &mut E,
    cmd: &RedisCommand,
    exchange: &mut Exchange,
    is_idempotent: bool,
    max_retries: u32,
    base_ms: u64,
    max_backoff: std::time::Duration,
) -> Result<(), CamelError> {
    use crate::config::backoff_delay;
    use tracing::{debug, warn};

    let result = executor.execute_command(cmd, exchange).await;

    if let Err(ref e) = result
        && is_transient_redis_error(e)
        && is_idempotent
    {
        warn!(
            command = ?cmd,
            error = %e,
            "Transient error on idempotent command, retrying with backoff"
        );

        let mut last_err = e.clone();

        for attempt in 0..max_retries {
            executor.reconnect().await?;

            let delay = backoff_delay(attempt, base_ms, max_backoff);
            debug!(
                command = ?cmd,
                attempt = attempt + 1,
                delay_ms = delay.as_millis(),
                "Waiting before retry"
            );
            tokio::time::sleep(delay).await;

            match executor.execute_command(cmd, exchange).await {
                Ok(()) => return Ok(()),
                Err(retry_err) => {
                    if is_transient_redis_error(&retry_err) {
                        warn!(
                            command = ?cmd,
                            attempt = attempt + 1,
                            error = %retry_err,
                            "Retry failed with transient error"
                        );
                        last_err = retry_err;
                        continue;
                    } else {
                        return Err(retry_err);
                    }
                }
            }
        }

        return Err(CamelError::ProcessorError(format!(
            "Command {:?} failed after {} retries: {}",
            cmd, max_retries, last_err
        )));
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

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

        let result = execute_with_retry(
            &mut { executor },
            &cmd,
            &mut exchange,
            true, // is_idempotent
            10,
            1, // 1ms base for fast tests
            Duration::from_millis(10),
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

        let result = execute_with_retry(
            &mut { executor },
            &cmd,
            &mut exchange,
            true,
            10,
            1,
            Duration::from_millis(10),
        )
        .await;

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

        let result = execute_with_retry(
            &mut { executor },
            &cmd,
            &mut exchange,
            false, // NOT idempotent
            10,
            1,
            Duration::from_millis(10),
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

        let result = execute_with_retry(
            &mut { executor },
            &cmd,
            &mut exchange,
            true, // is_idempotent
            10,
            1,
            Duration::from_millis(10),
        )
        .await;

        assert!(result.is_err(), "non-transient error should not be retried");
        // Only 1 call — no retries for non-transient
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_backoff_delay_increases_between_attempts() {
        // Use a fake executor that records timing between calls
        let executor = FakeExecutor::new(vec![
            Ok(()),
            transient_err("fail 1"),
            transient_err("fail 2"),
            transient_err("fail 3"),
        ]);

        let mut exchange = Exchange::default();
        let cmd = RedisCommand::Get;

        let start = Instant::now();
        let result = execute_with_retry(
            &mut { executor },
            &cmd,
            &mut exchange,
            true,
            10,
            50, // 50ms base
            Duration::from_millis(500),
        )
        .await;
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        // Delays: 50ms (attempt 0) + 100ms (attempt 1) + 200ms (attempt 2) = 350ms minimum
        // Allow some tolerance for scheduling
        assert!(
            elapsed >= Duration::from_millis(300),
            "elapsed {:?} should be >= 300ms (backoff delays)",
            elapsed
        );
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

        let result = execute_with_retry(
            &mut { executor },
            &cmd,
            &mut exchange,
            true,
            10,
            1,
            Duration::from_millis(10),
        )
        .await;

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

        let result = execute_with_retry(
            &mut { executor },
            &cmd,
            &mut exchange,
            true,
            10,
            1,
            Duration::from_millis(10),
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        assert_eq!(reconnect_count.load(Ordering::SeqCst), 0);
    }
}
