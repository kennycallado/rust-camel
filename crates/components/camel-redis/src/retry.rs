//! Shared transient-retry step for the consumer reconnect loops.
//!
//! Every bounded reconnect loop in this component (queue, pubsub) repeats the
//! same shape after a failed stage: classify the error, check the
//! [`NetworkRetryPolicy`] budget, `warn!` with an outside-contract
//! log-policy annotation, sleep the backoff, and retry. This module owns that
//! shape once.
//!
//! # ADR-0012 invariant
//!
//! The terminal (budget-exhaustion) error built here contains the word
//! "connection" — exactly once, in [`retry_budget_exhausted`] — because
//! [`is_transient_redis_error`](crate::config::is_transient_redis_error)
//! matches it. That classification routes budget exhaustion to the consumer's
//! `e:redis:message-transient-budget` metric instead of
//! `e:redis:message-non-transient`. Do not reword these messages.

use camel_component_api::{CamelError, NetworkRetryPolicy};
use std::ops::ControlFlow;
use tracing::warn;

use crate::config::is_transient_redis_error;

/// Terminal error when the transient-retry budget is exhausted.
///
/// The word "connection" is load-bearing (see the module docs): it makes
/// `is_transient_redis_error` classify this error as transient so the
/// consumer's Err-branch fires the transient-budget metric (ADR-0012) and
/// Route supervision restarts the route (ADR-0007).
pub(crate) fn retry_budget_exhausted(
    policy: &NetworkRetryPolicy,
    stage: &str,
    cause: &str,
) -> CamelError {
    CamelError::ProcessorError(format!(
        "connection lost while {stage} (retry budget exhausted after {} attempts): {cause}",
        policy.max_attempts
    ))
}

/// One classify → budget → warn → backoff step of a bounded reconnect loop.
///
/// * `attempt` — the loop's shared attempt counter; advanced on every retry.
/// * `stage` — a short phrase naming the failed operation (e.g.
///   `"resolving master for BLPOP"`). Used in the retry `warn!` field and in
///   the budget-exhaustion error.
///
/// Returns:
/// - `ControlFlow::Continue(())` — the error was transient and the budget
///   allows another attempt; the backoff delay has already been slept. The
///   caller retries the stage.
/// - `ControlFlow::Break(err)` — stop retrying: either the error was
///   non-transient (returned unchanged), or the budget is exhausted (wrapped
///   by [`retry_budget_exhausted`], which classifies transient so the
///   consumer's Err-branch routing fires the right metric). The caller
///   returns `Err` so Route supervision fires (ADR-0007).
pub(crate) async fn transient_retry_step(
    policy: &NetworkRetryPolicy,
    attempt: &mut u32,
    err: CamelError,
    stage: &str,
) -> ControlFlow<CamelError> {
    if !is_transient_redis_error(&err) {
        return ControlFlow::Break(err);
    }
    *attempt += 1;
    if !policy.should_retry(*attempt) {
        return ControlFlow::Break(retry_budget_exhausted(policy, stage, &err.to_string()));
    }
    let delay = policy.delay_for(*attempt - 1);
    // log-policy: outside-contract
    warn!(
        error = %err,
        attempt = *attempt,
        delay_ms = delay.as_millis(),
        stage,
        "Transient error, retrying with backoff"
    );
    tokio::time::sleep(delay).await;
    ControlFlow::Continue(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn fast_policy(max_attempts: u32) -> NetworkRetryPolicy {
        NetworkRetryPolicy {
            max_attempts,
            initial_delay: Duration::from_millis(0),
            ..NetworkRetryPolicy::default()
        }
    }

    #[tokio::test]
    async fn continues_within_budget_and_advances_attempt() {
        let mut attempt = 0;
        let flow = transient_retry_step(
            &fast_policy(2),
            &mut attempt,
            CamelError::ProcessorError("connection refused".into()),
            "connecting",
        )
        .await;

        assert!(matches!(flow, ControlFlow::Continue(())));
        assert_eq!(
            attempt, 1,
            "one retry step must advance the attempt counter"
        );
    }

    // ADR-0012: the exhaustion error must contain "connection" so it
    // classifies transient (transient-budget metric path, not non-transient).
    #[tokio::test]
    async fn exhaustion_error_classifies_transient_and_names_stage() {
        let mut attempt = 0;
        let err = match transient_retry_step(
            &fast_policy(1),
            &mut attempt,
            CamelError::ProcessorError("connection refused".into()),
            "connecting for PubSub",
        )
        .await
        {
            ControlFlow::Break(e) => e,
            ControlFlow::Continue(()) => panic!("max_attempts=1 must break on the first failure"),
        };

        assert!(
            is_transient_redis_error(&err),
            "budget-exhaustion error must classify as transient: {err}"
        );
        assert!(
            err.to_string().contains("connecting for PubSub"),
            "error must name the failed stage: {err}"
        );
        assert_eq!(attempt, 1);
    }

    #[tokio::test]
    async fn non_transient_error_breaks_immediately_unchanged() {
        let mut attempt = 0;
        let err = match transient_retry_step(
            &fast_policy(5),
            &mut attempt,
            CamelError::ProcessorError("WRONGTYPE".into()),
            "popping",
        )
        .await
        {
            ControlFlow::Break(e) => e,
            ControlFlow::Continue(()) => panic!("non-transient error must not be retried"),
        };

        assert!(
            err.to_string().contains("WRONGTYPE"),
            "non-transient error must be returned unchanged: {err}"
        );
        assert_eq!(attempt, 0, "non-transient failures do not consume budget");
    }
}
