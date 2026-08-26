//! Pure leadership decision module.
//!
//! Owns the verdict vocabulary produced by lease reconciliation. The verdict
//! refines the design.md §1 sketch: `Renewed` carries `Option<u64>` to
//! preserve today's only-if-`Some` defensive epoch update; the decision-table
//! semantics are unchanged.

use std::future::Future;
use std::time::{Duration, Instant};

use crate::platform_service::KubernetesPlatformConfig;

/// Outcome of one lease reconciliation cycle.
#[derive(Debug)]
pub(crate) enum ReconcileVerdict {
    /// Lease now held by us via create or expiry takeover; `term` is never 0
    /// (fallback 1 applied at the return site).
    Acquired { term: u64 },
    /// Renewal succeeded; `None` means the server stripped the leader-term
    /// annotation (caller keeps the current epoch).
    Renewed { term: Option<u64> },
    /// Server answered; a valid foreign holder owns the lease.
    ForeignHolder,
    /// Optimistic-concurrency 409; proves only a stale generation.
    Conflict,
}

/// Loop-side leadership state mutated by [`decide`].
#[derive(Debug)]
pub(crate) struct LoopState {
    pub(crate) currently_leader: bool,
    pub(crate) last_success: Option<Instant>,
}

/// Remaining renewal budget, measured from the last successful renewal.
/// `None` when not leading (no success to measure from).
pub(crate) fn remaining_budget(
    last_success: Option<Instant>,
    config: &KubernetesPlatformConfig,
    now: Instant,
) -> Option<Duration> {
    last_success.map(|success| config.renew_deadline.saturating_sub(now - success))
}

/// True when the renewal budget is fully spent (remaining is exactly zero).
pub(crate) fn budget_exhausted(
    last_success: Option<Instant>,
    config: &KubernetesPlatformConfig,
    now: Instant,
) -> bool {
    matches!(
        remaining_budget(last_success, config, now),
        Some(remaining) if remaining == Duration::ZERO
    )
}

/// Why the loop gave up leadership.
#[derive(Debug)]
pub(crate) enum StepDownReason {
    BudgetExhausted,
    LostLease,
}

/// Action the leadership loop must perform after one reconciliation cycle.
#[derive(Debug)]
pub(crate) enum CycleAction {
    /// Transition to leader: store the epoch, set `is_leader`, emit
    /// `StartedLeading`, set `last_success`.
    BecomeLeader { term: u64, sleep: Duration },
    /// Keep leading: defensively update the stored epoch when `term` is
    /// `Some`; no leadership event.
    ContinueLeading { term: Option<u64>, sleep: Duration },
    /// Drop leadership: clear `is_leader`, emit `StoppedLeading`, clear
    /// `last_success`.
    StepDown { reason: StepDownReason },
    /// Keep waiting to acquire the lease; no side effects.
    SleepAcquiring { sleep: Duration },
}

/// Result of one bounded reconciliation attempt, as consumed by [`decide`].
#[derive(Debug)]
pub(crate) enum CycleOutcome {
    Acquired { term: u64 },
    Renewed { term: Option<u64> },
    Lost,
    Conflict,
    Failed,
}

/// Pure decision table for the leadership loop.
///
/// Success rows measure the budget from the new success (set before the
/// sleep is computed); failure rows measure it from the pre-update
/// `last_success`, because a failure renews nothing.
pub(crate) fn decide(
    state: &mut LoopState,
    outcome: CycleOutcome,
    config: &KubernetesPlatformConfig,
    retry_sleep: Duration,
    now: Instant,
) -> CycleAction {
    match outcome {
        CycleOutcome::Acquired { term } => {
            if !state.currently_leader {
                state.currently_leader = true;
                state.last_success = Some(now);
                CycleAction::BecomeLeader {
                    term,
                    sleep: retry_sleep,
                }
            } else {
                state.last_success = Some(now);
                // last_success was just set, so the budget is the full
                // renew_deadline; clamp retry_sleep defensively.
                let remaining = config.renew_deadline;
                CycleAction::ContinueLeading {
                    term: Some(term),
                    sleep: retry_sleep.min(remaining),
                }
            }
        }
        CycleOutcome::Renewed { term } => {
            if !state.currently_leader {
                state.currently_leader = true;
                state.last_success = Some(now);
                CycleAction::BecomeLeader {
                    term: term.unwrap_or(1),
                    sleep: retry_sleep,
                }
            } else {
                state.last_success = Some(now);
                // last_success was just set, so the budget is the full
                // renew_deadline; clamp retry_sleep defensively.
                let remaining = config.renew_deadline;
                CycleAction::ContinueLeading {
                    term,
                    sleep: retry_sleep.min(remaining),
                }
            }
        }
        CycleOutcome::Lost => {
            if !state.currently_leader {
                CycleAction::SleepAcquiring { sleep: retry_sleep }
            } else {
                state.currently_leader = false;
                state.last_success = None;
                CycleAction::StepDown {
                    reason: StepDownReason::LostLease,
                }
            }
        }
        CycleOutcome::Conflict | CycleOutcome::Failed => {
            if !state.currently_leader {
                return CycleAction::SleepAcquiring { sleep: retry_sleep };
            }
            if budget_exhausted(state.last_success, config, now) {
                state.currently_leader = false;
                state.last_success = None;
                return CycleAction::StepDown {
                    reason: StepDownReason::BudgetExhausted,
                };
            }
            let remaining =
                remaining_budget(state.last_success, config, now).unwrap_or(Duration::ZERO);
            CycleAction::ContinueLeading {
                term: None,
                sleep: retry_sleep.min(remaining),
            }
        }
    }
}

/// Why a bounded attempt produced no verdict.
#[derive(Debug)]
pub(crate) enum AttemptFailure {
    Transport(kube::Error),
    Deadline,
}

/// Run one reconciliation attempt under a hard time budget.
pub(crate) async fn bound_attempt<F>(
    fut: F,
    budget: Duration,
) -> Result<ReconcileVerdict, AttemptFailure>
where
    F: Future<Output = Result<ReconcileVerdict, kube::Error>>,
{
    match tokio::time::timeout(budget, fut).await {
        Ok(Ok(verdict)) => Ok(verdict),
        Ok(Err(e)) => Err(AttemptFailure::Transport(e)),
        Err(_elapsed) => Err(AttemptFailure::Deadline),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use kube::core::Status;
    use kube::core::response::StatusSummary;

    fn test_config() -> KubernetesPlatformConfig {
        KubernetesPlatformConfig {
            namespace: "default".to_string(),
            lease_name_prefix: "camel-".to_string(),
            lease_duration: Duration::from_secs(15),
            renew_deadline: Duration::from_secs(10),
            retry_period: Duration::from_secs(2),
            jitter_factor: 0.2,
        }
    }

    #[test]
    fn decide_transient_failure_within_budget_keeps_leading() {
        let now = Instant::now();
        let config = test_config();
        let mut state = LoopState {
            currently_leader: true,
            last_success: Some(now - Duration::from_secs(2)),
        };

        let action = decide(
            &mut state,
            CycleOutcome::Failed,
            &config,
            Duration::from_secs(2),
            now,
        );

        match action {
            CycleAction::ContinueLeading { term: None, sleep } => {
                assert_eq!(sleep, Duration::from_secs(2));
            }
            other => panic!("expected ContinueLeading, got {other:?}"),
        }
        assert!(state.currently_leader);
    }

    #[test]
    fn decide_failure_sleep_capped_by_remaining_budget() {
        let now = Instant::now();
        let config = test_config();
        let mut state = LoopState {
            currently_leader: true,
            last_success: Some(now - Duration::from_millis(9500)),
        };

        let action = decide(
            &mut state,
            CycleOutcome::Failed,
            &config,
            Duration::from_secs(2),
            now,
        );

        match action {
            CycleAction::ContinueLeading { term: None, sleep } => {
                assert_eq!(sleep, Duration::from_millis(500));
            }
            other => panic!("expected ContinueLeading, got {other:?}"),
        }
    }

    #[test]
    fn decide_budget_exhaustion_steps_down() {
        let now = Instant::now();
        let config = test_config();
        let mut state = LoopState {
            currently_leader: true,
            last_success: Some(now - Duration::from_secs(10)),
        };

        let action = decide(
            &mut state,
            CycleOutcome::Failed,
            &config,
            Duration::from_secs(2),
            now,
        );

        match action {
            CycleAction::StepDown {
                reason: StepDownReason::BudgetExhausted,
            } => {}
            other => panic!("expected StepDown(BudgetExhausted), got {other:?}"),
        }
        assert!(!state.currently_leader);
        assert_eq!(state.last_success, None);
    }

    #[test]
    fn budget_exhausted_true_at_deadline() {
        let now = Instant::now();
        let config = test_config();

        assert!(budget_exhausted(
            Some(now - Duration::from_secs(10)),
            &config,
            now
        ));
    }

    #[test]
    fn budget_exhausted_false_within_budget() {
        let now = Instant::now();
        let config = test_config();

        assert!(!budget_exhausted(
            Some(now - Duration::from_secs(2)),
            &config,
            now
        ));
    }

    #[test]
    fn budget_exhausted_false_when_never_led() {
        let now = Instant::now();
        let config = test_config();

        assert!(!budget_exhausted(None, &config, now));
    }

    #[test]
    fn decide_conflict_within_budget_keeps_leading() {
        let now = Instant::now();
        let config = test_config();
        let mut state = LoopState {
            currently_leader: true,
            last_success: Some(now - Duration::from_secs(2)),
        };

        let action = decide(
            &mut state,
            CycleOutcome::Conflict,
            &config,
            Duration::from_secs(2),
            now,
        );

        match action {
            CycleAction::ContinueLeading { term: None, .. } => {}
            other => panic!("expected ContinueLeading, got {other:?}"),
        }
        assert!(state.currently_leader);
    }

    #[test]
    fn decide_lost_steps_down_immediately() {
        let now = Instant::now();
        let config = test_config();
        let mut state = LoopState {
            currently_leader: true,
            last_success: Some(now - Duration::from_secs(1)),
        };

        let action = decide(
            &mut state,
            CycleOutcome::Lost,
            &config,
            Duration::from_secs(2),
            now,
        );

        match action {
            CycleAction::StepDown {
                reason: StepDownReason::LostLease,
            } => {}
            other => panic!("expected StepDown(LostLease), got {other:?}"),
        }
    }

    #[test]
    fn decide_acquired_while_not_leading_becomes_leader() {
        let now = Instant::now();
        let config = test_config();
        let mut state = LoopState {
            currently_leader: false,
            last_success: None,
        };

        let action = decide(
            &mut state,
            CycleOutcome::Acquired { term: 3 },
            &config,
            Duration::from_secs(2),
            now,
        );

        match action {
            CycleAction::BecomeLeader { term: 3, sleep } => {
                assert_eq!(sleep, Duration::from_secs(2));
            }
            other => panic!("expected BecomeLeader, got {other:?}"),
        }
        assert!(state.currently_leader);
        assert_eq!(state.last_success, Some(now));
    }

    #[test]
    fn decide_renewed_resets_budget() {
        let now = Instant::now();
        let config = test_config();
        let mut state = LoopState {
            currently_leader: true,
            last_success: Some(now - Duration::from_secs(9)),
        };

        let action = decide(
            &mut state,
            CycleOutcome::Renewed { term: Some(5) },
            &config,
            Duration::from_secs(2),
            now,
        );

        match action {
            CycleAction::ContinueLeading {
                term: Some(5),
                sleep,
            } => {
                assert_eq!(sleep, Duration::from_secs(2));
            }
            other => panic!("expected ContinueLeading, got {other:?}"),
        }
        assert!(!budget_exhausted(
            state.last_success,
            &config,
            now + Duration::from_secs(9)
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn bound_attempt_times_out_at_budget() {
        let start = tokio::time::Instant::now();

        let result = bound_attempt(std::future::pending(), Duration::from_secs(10)).await;

        assert!(matches!(result, Err(AttemptFailure::Deadline)));
        assert_eq!(tokio::time::Instant::now() - start, Duration::from_secs(10));
    }

    #[tokio::test]
    async fn bound_attempt_passes_transport_error() {
        let err = kube::Error::Api(Box::new(Status {
            status: Some(StatusSummary::Failure),
            message: "conflict".to_string(),
            reason: "Conflict".to_string(),
            code: 409,
            metadata: None,
            details: None,
        }));

        let result = bound_attempt(async { Err(err) }, Duration::from_secs(10)).await;

        assert!(matches!(result, Err(AttemptFailure::Transport(_))));
    }
}
