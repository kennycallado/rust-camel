//! Bridge generation tracking (spec §4.6 + §4.11, brief 2f/2i).
//!
//! Bridge subprocess PID is recorded at round START and round END
//! (after warmup, before measurement; at round completion). If the PID
//! differs, the round is INVALIDATED and re-run.
//!
//! Per-message `/proc` reads on the hot path are PROHIBITED (e_opus spec
//! blessing: measurement-integrity concern). The bridge subprocess
//! writes its PID once at spawn to a tmpfs file
//! `/tmp/v3-bridge-pid-<cell>.txt`.
//!
//! Bridge-health timeout (e_gpt plan blessing pin): bridge must produce
//! its first invocation output within 30 seconds of warmup start. If no
//! output within 30s, bridge is unhealthy → cell marked
//! `✗ v3 — failed-bridge-unhealthy`.
//!
//! **Bridge health operational definition** (brief 2i): the harness
//! observes condition (3) of spec §4.6 — first successful route
//! completion — as proxy for conditions (1) bridge spawned AND (2)
//! schema/stylesheet loaded. Successful first invocation transitively
//! proves spawn + schema load.
//!
//! Retry semantics (brief 2f):
//! - `failed-stability` — NO retry (deterministic failure mode).
//! - `failed-bridge-restart`, `failed-bridge-unhealthy`, harness-side
//!   transient errors — max 3 retries per cell.

use std::time::Duration;

/// Bridge health timeout (brief 2f, spec §4.6).
pub const BRIDGE_HEALTH_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum retries per cell for retryable failure modes.
pub const MAX_RETRIES_PER_CELL: u32 = 3;

/// Result of comparing the bridge PID at round start vs round end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PidMismatch {
    /// PIDs match — round is valid.
    Consistent { pid: u32 },
    /// PIDs differ — round is INVALIDATED (brief 2f: re-run, retryable).
    Changed {
        /// Bridge PID at round start.
        start_pid: u32,
        /// Bridge PID at round end (different).
        end_pid: u32,
    },
    /// Bridge PID file is missing or unreadable at one of the boundaries.
    /// Treated as retryable (`failed-bridge-restart`).
    Unreadable { stage: PidStage },
}

/// Stage at which a PID was sampled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidStage {
    Start,
    End,
}

/// Result of the bridge first-output health check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeHealth {
    /// First invocation output observed within the timeout window.
    Healthy {
        /// Wall-clock nanoseconds from warmup start to first successful
        /// route completion.
        first_output_latency_ns: u64,
    },
    /// No output observed within 30 seconds of warmup start.
    Unhealthy,
}

/// Compare bridge PIDs recorded at round-start and round-end.
///
/// Inputs are string-typed to admit "missing" (None) as a distinct state
/// from "unreadable" (Some(buf) but parse failed).
pub fn check_bridge_pid(start: Option<&str>, end: Option<&str>) -> PidMismatch {
    let start_pid = match start.and_then(|s| s.trim().parse::<u32>().ok()) {
        Some(p) => p,
        None => {
            return PidMismatch::Unreadable {
                stage: PidStage::Start,
            }
        }
    };
    let end_pid = match end.and_then(|s| s.trim().parse::<u32>().ok()) {
        Some(p) => p,
        None => {
            return PidMismatch::Unreadable {
                stage: PidStage::End,
            }
        }
    };
    if start_pid == end_pid {
        PidMismatch::Consistent { pid: start_pid }
    } else {
        PidMismatch::Changed { start_pid, end_pid }
    }
}

/// Decide bridge health from the observed first-output latency.
///
/// Returns `Healthy` if the first successful route completion arrived
/// within [`BRIDGE_HEALTH_TIMEOUT`] of warmup start; `Unhealthy` otherwise.
pub fn check_bridge_health(first_output_latency: Duration) -> BridgeHealth {
    if first_output_latency <= BRIDGE_HEALTH_TIMEOUT {
        BridgeHealth::Healthy {
            first_output_latency_ns: first_output_latency.as_nanos() as u64,
        }
    } else {
        BridgeHealth::Unhealthy
    }
}

/// Cell failure category (structured label for COVERAGE.md per brief 2f).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellFailureCategory {
    /// `failed-stability` — warmup didn't reach stability criterion.
    /// **NO retry** (deterministic failure mode).
    FailedStability,
    /// `failed-bridge-restart` — PID changed mid-round. Retryable.
    FailedBridgeRestart,
    /// `failed-bridge-unhealthy` — no first-output within 30s. Retryable.
    FailedBridgeUnhealthy,
}

impl CellFailureCategory {
    /// COVERAGE.md label (e.g., `✗ v3 — failed-bridge-restart`).
    pub fn label(&self) -> &'static str {
        match self {
            Self::FailedStability => "failed-stability",
            Self::FailedBridgeRestart => "failed-bridge-restart",
            Self::FailedBridgeUnhealthy => "failed-bridge-unhealthy",
        }
    }

    /// Whether this category allows retries (brief 2f).
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::FailedStability => false,
            Self::FailedBridgeRestart | Self::FailedBridgeUnhealthy => true,
        }
    }
}

/// Decide a cell's failure category from the observed outcomes.
pub fn classify_failure(
    warmup_stable: bool,
    bridge_health: Option<&BridgeHealth>,
    pid_mismatch: Option<&PidMismatch>,
) -> Option<CellFailureCategory> {
    if !warmup_stable {
        return Some(CellFailureCategory::FailedStability);
    }
    if let Some(BridgeHealth::Unhealthy) = bridge_health {
        return Some(CellFailureCategory::FailedBridgeUnhealthy);
    }
    if let Some(PidMismatch::Changed { .. } | PidMismatch::Unreadable { .. }) = pid_mismatch {
        return Some(CellFailureCategory::FailedBridgeRestart);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_consistent_when_start_equals_end() {
        match check_bridge_pid(Some("12345"), Some("12345")) {
            PidMismatch::Consistent { pid } => assert_eq!(pid, 12345),
            other => panic!("expected Consistent, got {other:?}"),
        }
    }

    #[test]
    fn pid_consistent_trims_whitespace() {
        match check_bridge_pid(Some("  12345\n"), Some("12345 ")) {
            PidMismatch::Consistent { pid } => assert_eq!(pid, 12345),
            other => panic!("expected Consistent, got {other:?}"),
        }
    }

    #[test]
    fn pid_changed_when_pids_differ() {
        match check_bridge_pid(Some("12345"), Some("99999")) {
            PidMismatch::Changed { start_pid, end_pid } => {
                assert_eq!(start_pid, 12345);
                assert_eq!(end_pid, 99999);
            }
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    #[test]
    fn pid_unreadable_when_start_missing() {
        match check_bridge_pid(None, Some("12345")) {
            PidMismatch::Unreadable {
                stage: PidStage::Start,
            } => {}
            other => panic!("expected Unreadable/Start, got {other:?}"),
        }
    }

    #[test]
    fn pid_unreadable_when_end_missing() {
        match check_bridge_pid(Some("12345"), None) {
            PidMismatch::Unreadable {
                stage: PidStage::End,
            } => {}
            other => panic!("expected Unreadable/End, got {other:?}"),
        }
    }

    #[test]
    fn pid_unreadable_when_start_non_numeric() {
        match check_bridge_pid(Some("abc"), Some("12345")) {
            PidMismatch::Unreadable {
                stage: PidStage::Start,
            } => {}
            other => panic!("expected Unreadable/Start, got {other:?}"),
        }
    }

    #[test]
    fn bridge_healthy_when_first_output_under_30s() {
        match check_bridge_health(Duration::from_secs(5)) {
            BridgeHealth::Healthy {
                first_output_latency_ns,
            } => {
                assert_eq!(first_output_latency_ns, 5_000_000_000);
            }
            _ => panic!("expected Healthy"),
        }
    }

    #[test]
    fn bridge_healthy_at_exact_30s_boundary() {
        // "<=" — exactly 30s is still healthy.
        assert!(matches!(
            check_bridge_health(Duration::from_secs(30)),
            BridgeHealth::Healthy { .. }
        ));
    }

    #[test]
    fn bridge_unhealthy_when_first_output_exceeds_30s() {
        assert!(matches!(
            check_bridge_health(Duration::from_secs(31)),
            BridgeHealth::Unhealthy
        ));
    }

    #[test]
    fn classify_no_failure_when_all_healthy() {
        let health = BridgeHealth::Healthy {
            first_output_latency_ns: 1_000_000,
        };
        let pid = PidMismatch::Consistent { pid: 12345 };
        assert_eq!(classify_failure(true, Some(&health), Some(&pid)), None);
    }

    #[test]
    fn classify_failed_stability_when_warmup_unstable() {
        let cat = classify_failure(false, None, None).unwrap();
        assert_eq!(cat, CellFailureCategory::FailedStability);
        assert!(!cat.is_retryable());
    }

    #[test]
    fn classify_failed_bridge_unhealthy_when_no_output() {
        let cat = classify_failure(true, Some(&BridgeHealth::Unhealthy), None).unwrap();
        assert_eq!(cat, CellFailureCategory::FailedBridgeUnhealthy);
        assert!(cat.is_retryable());
    }

    #[test]
    fn classify_failed_bridge_restart_when_pid_changed() {
        let pid = PidMismatch::Changed {
            start_pid: 1,
            end_pid: 2,
        };
        let cat = classify_failure(true, None, Some(&pid)).unwrap();
        assert_eq!(cat, CellFailureCategory::FailedBridgeRestart);
        assert!(cat.is_retryable());
    }

    #[test]
    fn classify_failed_bridge_restart_when_pid_unreadable() {
        let pid = PidMismatch::Unreadable {
            stage: PidStage::End,
        };
        let cat = classify_failure(true, None, Some(&pid)).unwrap();
        assert_eq!(cat, CellFailureCategory::FailedBridgeRestart);
    }

    #[test]
    fn classify_warmup_failure_takes_precedence_over_bridge_failure() {
        // Both warmup unstable AND bridge unhealthy → warmup failure wins
        // (and is non-retryable).
        let cat = classify_failure(false, Some(&BridgeHealth::Unhealthy), None).unwrap();
        assert_eq!(cat, CellFailureCategory::FailedStability);
        assert!(!cat.is_retryable());
    }

    #[test]
    fn coverage_label_matches_convention() {
        assert_eq!(
            CellFailureCategory::FailedStability.label(),
            "failed-stability"
        );
        assert_eq!(
            CellFailureCategory::FailedBridgeRestart.label(),
            "failed-bridge-restart"
        );
        assert_eq!(
            CellFailureCategory::FailedBridgeUnhealthy.label(),
            "failed-bridge-unhealthy"
        );
    }
}
