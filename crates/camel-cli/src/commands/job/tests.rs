//! Unit tests for the `camel job` report, exit-code, and
//! shutdown-budget contracts, split out of `mod.rs` for file-size
//! hygiene. Behavior and test names are unchanged.

mod exit_code_tests {
    use crate::commands::job::{JobReport, exit_code_for};

    /// An `Interrupted` report maps to exit code 2 (apparatus class,
    /// same as load/boot/timeout/shutdown errors).
    #[test]
    fn job_exit_code_interrupted() {
        let report = JobReport {
            document: "doc".to_string(),
            mode: "one-shot".to_string(),
            outcome: "Interrupted",
            terminated_early: false,
            duration_ms: 1,
            reply: None,
            error: Some("interrupted by signal (SIGINT/SIGTERM)".to_string()),
            shutdown_error: None,
        };
        assert_eq!(exit_code_for(report.outcome), 2);
    }
}

mod report_tests {
    use crate::commands::job::{
        JobReport, MIN_SHUTDOWN_BUDGET, exit_code_for, record_shutdown_failure,
    };

    /// An `Interrupted` report with a shutdown detail serializes both:
    /// `outcome` stays `Interrupted` and `shutdown_error` is present.
    #[test]
    fn job_report_interrupted_serializes() {
        let report = JobReport {
            document: "doc".to_string(),
            mode: "one-shot".to_string(),
            outcome: "Interrupted",
            terminated_early: false,
            duration_ms: 1,
            reply: None,
            error: Some("interrupted by signal (SIGINT/SIGTERM)".to_string()),
            shutdown_error: Some("shutdown failure: x".to_string()),
        };
        let json = serde_json::to_value(&report).expect("report must serialize");
        assert_eq!(
            json["outcome"],
            serde_json::json!("Interrupted"),
            "outcome must stay Interrupted: {json}"
        );
        assert!(
            json["shutdown_error"].is_string(),
            "shutdown detail must serialize: {json}"
        );
    }

    /// A shutdown failure after an interruption finalizes without
    /// replacing the verdict: `shutdown_error` is recorded for a
    /// non-zero budget, the outcome stays `Interrupted`, and the exit
    /// code is 2.
    #[test]
    fn job_interrupted_shutdown_failure_preserves_verdict() {
        let mut report = JobReport {
            document: "doc".to_string(),
            mode: "one-shot".to_string(),
            outcome: "Interrupted",
            terminated_early: false,
            duration_ms: 1,
            reply: None,
            error: Some("interrupted by signal (SIGINT/SIGTERM)".to_string()),
            shutdown_error: None,
        };
        record_shutdown_failure(
            &mut report,
            "shutdown failure: x".to_string(),
            MIN_SHUTDOWN_BUDGET,
        );
        assert_eq!(report.outcome, "Interrupted");
        assert_eq!(
            report.shutdown_error.as_deref(),
            Some("shutdown failure: x"),
            "non-zero-budget teardown detail must be recorded"
        );
        assert_eq!(exit_code_for(report.outcome), 2);
    }

    /// A shutdown failure after a recorded verdict serializes alongside
    /// the verdict error: `error` keeps the pipeline/timeout detail and
    /// `shutdown_error` carries the teardown detail.
    #[test]
    fn shutdown_error_serializes_alongside_error() {
        let report = JobReport {
            document: "doc".to_string(),
            mode: "one-shot".to_string(),
            outcome: "Failed",
            terminated_early: false,
            duration_ms: 1,
            reply: None,
            error: Some("pipeline failed".to_string()),
            shutdown_error: Some("shutdown failure: x".to_string()),
        };
        let json = serde_json::to_string(&report).expect("report must serialize");
        assert!(
            json.contains("pipeline failed"),
            "verdict error must serialize: {json}"
        );
        assert!(
            json.contains("shutdown failure: x"),
            "shutdown detail must serialize: {json}"
        );
    }

    /// Without a shutdown failure the `shutdown_error` key is omitted
    /// from the JSON report.
    #[test]
    fn shutdown_error_omitted_when_absent() {
        let report = JobReport {
            document: "doc".to_string(),
            mode: "one-shot".to_string(),
            outcome: "Failed",
            terminated_early: false,
            duration_ms: 1,
            reply: None,
            error: Some("pipeline failed".to_string()),
            shutdown_error: None,
        };
        let json = serde_json::to_string(&report).expect("report must serialize");
        assert!(
            !json.contains("shutdown_error"),
            "absent shutdown_error must be omitted: {json}"
        );
    }
}

mod shutdown_budget_tests {
    use std::time::{Duration, Instant};

    use crate::commands::job::document::JobMode;
    use crate::commands::job::{MIN_SHUTDOWN_BUDGET, shutdown_budget};

    /// Batch: the budget is the remaining wall clock, uncapped below.
    #[test]
    fn shutdown_budget_batch_is_remaining() {
        let deadline = Instant::now() + Duration::from_secs(3);
        let budget = shutdown_budget(JobMode::Batch, deadline);
        assert!(
            budget <= Duration::from_secs(3) && budget > Duration::from_secs(2),
            "expected ~3s remaining, got {budget:?}"
        );
    }

    /// Batch with a spent deadline: zero budget, no floor.
    #[test]
    fn shutdown_budget_batch_zero_when_past() {
        let deadline = Instant::now() - Duration::from_secs(1);
        assert_eq!(shutdown_budget(JobMode::Batch, deadline), Duration::ZERO);
    }

    /// One-shot with a spent deadline: floored to MIN_SHUTDOWN_BUDGET.
    #[test]
    fn shutdown_budget_one_shot_floored() {
        let deadline = Instant::now() - Duration::from_secs(1);
        assert_eq!(
            shutdown_budget(JobMode::OneShot, deadline),
            MIN_SHUTDOWN_BUDGET
        );
    }

    /// One-shot with ample remaining time: the remaining clock wins over
    /// the floor.
    #[test]
    fn shutdown_budget_one_shot_is_remaining_when_large() {
        let deadline = Instant::now() + Duration::from_secs(10);
        let budget = shutdown_budget(JobMode::OneShot, deadline);
        assert!(
            budget <= Duration::from_secs(10) && budget > MIN_SHUTDOWN_BUDGET,
            "expected ~10s remaining, got {budget:?}"
        );
    }

    /// Interruption teardown budget by mode with matching deadlines:
    /// an interrupted one-shot gets at least `MIN_SHUTDOWN_BUDGET`
    /// (the floor lifts a spent or short deadline), while an
    /// interrupted batch gets only the remaining deadline with no
    /// floor (zero once the deadline is spent).
    #[test]
    fn job_interrupted_shutdown_budget_by_mode() {
        // Spent deadline: one-shot floored, batch zero.
        let spent = Instant::now() - Duration::from_secs(1);
        assert!(
            shutdown_budget(JobMode::OneShot, spent) >= MIN_SHUTDOWN_BUDGET,
            "interrupted one-shot teardown keeps the floor"
        );
        assert_eq!(
            shutdown_budget(JobMode::Batch, spent),
            Duration::ZERO,
            "interrupted batch teardown has no floor"
        );
        // Live deadline below the floor: one-shot is lifted to the
        // floor, batch keeps the raw remaining clock.
        let soon = Instant::now() + Duration::from_secs(3);
        assert_eq!(shutdown_budget(JobMode::OneShot, soon), MIN_SHUTDOWN_BUDGET);
        let batch = shutdown_budget(JobMode::Batch, soon);
        assert!(
            batch <= Duration::from_secs(3) && batch > Duration::from_secs(2),
            "interrupted batch teardown gets the remaining deadline, got {batch:?}"
        );
    }
}
