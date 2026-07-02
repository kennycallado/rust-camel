//! ADR-0033 startup-validation phase (skeleton).
//!
//! Batch 1 ships the *skeleton* — the public type/trait surface and a stub
//! `run_startup_validation()` that returns `Ok(())`. Later batches (Batch 5+)
//! register `ConfigCheck` impls for Require-Explicit-Choice members and wire
//! `CamelContext::start()` to call the phase. See ADR-0033 in `docs/adr/0033-…`.

use camel_api::CamelError;

/// A single startup-time configuration check.
///
/// Each `ConfigCheck` is run as part of the fail-closed startup-validation phase
/// established by ADR-0033. The check owns a name (for error reporting), a human
/// description, and a synchronous `run` that returns `Ok(())` if the check passes
/// or `Err(CamelError::Config)` if the check fails and the operator config is
/// invalid. Batch 1 ships the trait; later batches register impls.
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
/// **Batch 1 (skeleton):** returns `Ok(StartupValidationReport::default())`.
/// No `ConfigCheck` impls are registered yet — the registry is empty by design.
///
/// **Later batches:** register `ConfigCheck` impls (gRPC TLS, SQL `allow_dynamic_query`,
/// aggregator completion bound, …) and convert this function to a registry walk
/// that fails closed on the first error.
pub fn run_startup_validation() -> Result<StartupValidationReport, CamelError> {
    Ok(StartupValidationReport::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test: the skeleton compiles, exports its public surface, and the
    /// entry point returns `Ok` with an empty failure list. Later batches will
    /// replace this with a registry walk; the test will then grow.
    #[test]
    fn skeleton_run_returns_empty_ok_report() {
        let report = run_startup_validation().expect("skeleton must return Ok");
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
}
