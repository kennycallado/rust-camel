//! ADR-0033 startup-validation phase.
//!
//! `ConfigCheck` impls are registered for Require-Explicit-Choice security
//! defaults. The fail-closed `run_startup_validation()` walks the registry
//! and returns `Err(CamelError::Config)` if any check fails. See ADR-0033 in
//! `docs/adr/0033-…`.

use camel_api::CamelError;

/// A single startup-time configuration check.
///
/// Each `ConfigCheck` is run as part of the fail-closed startup-validation phase
/// established by ADR-0033. The check owns a name (for error reporting), a human
/// description, and a synchronous `run` that returns `Ok(())` if the check passes
/// or `Err(CamelError::Config)` if the check fails and the operator config is
/// invalid.
pub trait ConfigCheck: Send + Sync {
    /// Stable identifier for this check (e.g. `"grpc-tls"`, `"sql-dynamic-query"`).
    /// Used in error messages and `camel doctor` output.
    fn name(&self) -> &'static str;

    /// One-line human description of what this check enforces.
    fn description(&self) -> &'static str;

    /// Run the check. Returns `Err(CamelError::Config(_))` to fail-closed.
    fn run(&self) -> Result<(), CamelError>;
}

/// Aggregated result of running every registered `ConfigCheck`.
///
/// Batch 1 ships the type so later batches can populate `failures`. Skeleton
/// phase always produces a `StartupValidationReport { failures: vec![] }`.
#[derive(Debug, Default, Clone)]
pub struct StartupValidationReport {
    /// Names of every `ConfigCheck` that returned `Err`, in registration order.
    pub failures: Vec<String>,
}

impl StartupValidationReport {
    /// True when no check failed. Caller MUST refuse to start the runtime when
    /// this is `false`.
    pub fn is_ok(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Entry point for the startup-validation phase.
///
/// Walks the supplied `ConfigCheck` registry, collects failures, and fails closed
/// when any check returns `Err`. Established by ADR-0033.
pub fn run_startup_validation(
    checks: Vec<Box<dyn ConfigCheck>>,
) -> Result<StartupValidationReport, CamelError> {
    let mut report = StartupValidationReport::default();
    for check in &checks {
        if let Err(e) = check.run() {
            report.failures.push(format!("{}: {}", check.name(), e));
        }
    }
    if !report.is_ok() {
        return Err(CamelError::Config(report.failures.join("; ")));
    }
    Ok(report)
}

/// ConfigCheck: SQL dynamic-query intent must match capability (H7).
///
/// If `use_message_body_for_sql=true` but `allow_dynamic_query=false`,
/// the operator intent (use body as query) is blocked by the capability
/// gate — this is a misconfiguration that would silently produce empty
/// queries. Fail closed at startup instead.
pub struct SqlDynamicQueryCheck {
    pub use_message_body_for_sql: bool,
    pub allow_dynamic_query: bool,
}

impl ConfigCheck for SqlDynamicQueryCheck {
    fn name(&self) -> &'static str {
        "sql-dynamic-query"
    }
    fn description(&self) -> &'static str {
        "SQL use_message_body_for_sql requires allow_dynamic_query=true"
    }
    fn run(&self) -> Result<(), CamelError> {
        if self.use_message_body_for_sql && !self.allow_dynamic_query {
            return Err(CamelError::Config(
                "SQL endpoint has use_message_body_for_sql=true but allow_dynamic_query=false \
                 — set allow_dynamic_query=true to permit body-sourced queries"
                    .into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test: the empty registry returns `Ok` with an empty failure list.
    #[test]
    fn empty_registry_returns_empty_ok_report() {
        let report = run_startup_validation(vec![]).expect("empty registry must return Ok");
        assert!(report.is_ok());
        assert!(report.failures.is_empty());
    }

    /// Trait smoke test: a trivial `ConfigCheck` impl can be defined by callers
    /// and dispatched dynamically. This guards the trait surface against
    /// accidental breaking changes between batches.
    #[test]
    fn trait_is_object_safe_and_dispatchable() {
        struct AlwaysOk;
        impl ConfigCheck for AlwaysOk {
            fn name(&self) -> &'static str {
                "always-ok"
            }
            fn description(&self) -> &'static str {
                "always-ok skeleton check"
            }
            fn run(&self) -> Result<(), CamelError> {
                Ok(())
            }
        }

        let check: Box<dyn ConfigCheck> = Box::new(AlwaysOk);
        assert_eq!(check.name(), "always-ok");
        assert!(check.run().is_ok());
    }

    /// H7: SQL endpoint with `use_message_body_for_sql=true` and
    /// `allow_dynamic_query=false` is a misconfiguration that would silently
    /// produce empty queries — fail closed at startup.
    #[test]
    fn sql_dynamic_query_check_refuses_startup() {
        let check = SqlDynamicQueryCheck {
            use_message_body_for_sql: true,
            allow_dynamic_query: false,
        };
        let result = run_startup_validation(vec![Box::new(check)]);
        match result {
            Err(CamelError::Config(msg)) => {
                assert!(msg.contains("sql-dynamic-query"));
                assert!(msg.contains("allow_dynamic_query"));
            }
            other => panic!("expected CamelError::Config, got {other:?}"),
        }
    }

    /// H7: explicit opt-in satisfies the check — startup proceeds.
    #[test]
    fn sql_dynamic_query_check_passes_with_opt_in() {
        let check = SqlDynamicQueryCheck {
            use_message_body_for_sql: true,
            allow_dynamic_query: true,
        };
        let report =
            run_startup_validation(vec![Box::new(check)]).expect("opt-in must satisfy the check");
        assert!(report.is_ok());
    }

    /// H7: when `use_message_body_for_sql=false`, the dynamic-query gate
    /// is irrelevant and the check passes regardless of the opt-in flag.
    #[test]
    fn sql_dynamic_query_check_passes_without_body_sql() {
        let check = SqlDynamicQueryCheck {
            use_message_body_for_sql: false,
            allow_dynamic_query: false,
        };
        let report = run_startup_validation(vec![Box::new(check)])
            .expect("no body-sourced queries → no fail-closed condition");
        assert!(report.is_ok());
    }
}
