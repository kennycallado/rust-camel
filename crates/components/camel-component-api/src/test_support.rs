//! Test-only helpers for components + integration tests that need a
//! `RuntimeObservability` stub. Gates behind the `test-support` Cargo feature
//! so it never leaks into production builds.
//!
//! Usage from a downstream component's test mod:
//! ```ignore
//! use camel_component_api::test_support::PanicRuntimeObservability;
//! let rt: Arc<dyn camel_component_api::RuntimeObservability> =
//!     Arc::new(PanicRuntimeObservability);
//! ```

use std::sync::Arc;
use std::time::Duration;

use camel_api::MetricsCollector;

use crate::{HealthCheckRegistry, RuntimeObservability};

/// `RuntimeObservability` stub that panics if any method is invoked.
///
/// Use in test mods that exercise Endpoint trait surface but should NOT
/// actually invoke observability methods. Per Phase A spec line 98:
/// "Test fixtures implement `RuntimeObservability` with a stub that panics
/// on use (only observability tests should invoke it)."
#[derive(Debug, Default, Clone, Copy)]
pub struct PanicRuntimeObservability;

impl MetricsCollector for PanicRuntimeObservability {
    fn record_exchange_duration(&self, _: &str, _: Duration) {
        panic!("PanicRuntimeObservability::record_exchange_duration invoked")
    }
    fn increment_errors(&self, _: &str, _: &str) {
        panic!("PanicRuntimeObservability::increment_errors invoked")
    }
    fn increment_exchanges(&self, _: &str) {
        panic!("PanicRuntimeObservability::increment_exchanges invoked")
    }
    fn set_queue_depth(&self, _: &str, _: usize) {
        panic!("PanicRuntimeObservability::set_queue_depth invoked")
    }
    fn record_circuit_breaker_change(&self, _: &str, _: &str, _: &str) {
        panic!("PanicRuntimeObservability::record_circuit_breaker_change invoked")
    }
}

impl HealthCheckRegistry for PanicRuntimeObservability {
    fn force_unhealthy_for_route(&self, _: &str, _: &str, _: &str) {
        panic!("PanicRuntimeObservability::force_unhealthy_for_route invoked")
    }
}

impl RuntimeObservability for PanicRuntimeObservability {
    fn metrics(&self) -> Arc<dyn MetricsCollector> {
        panic!("PanicRuntimeObservability::metrics invoked")
    }
    fn health(&self) -> Arc<dyn HealthCheckRegistry> {
        panic!("PanicRuntimeObservability::health invoked")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_runtime_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PanicRuntimeObservability>();
    }
}
