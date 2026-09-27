//! Shared test contexts for cancellation-binding tests.
//!
//! Declared `#[cfg(test)]` in `lib.rs`; both the producer and bean test
//! suites use this stub to simulate bound (never swapped) and rebooting
//! `CamelContext`s without pulling in camel-core. No `unwrap`/`expect`
//! here: this file is not lexically inside a test scope for
//! `cargo xtask lint-unwrap`.

use std::sync::Arc;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use camel_component_api::ComponentContext;

/// Stub context whose shutdown token can be swapped mid-test to simulate a
/// `CamelContext` stop/start cycle: each start installs a fresh token under
/// the SAME context object (camel-core `start_context` replaces its
/// shutdown token on every start while the route registry persists).
pub(crate) struct SwappableContext {
    token: Mutex<CancellationToken>,
}

impl SwappableContext {
    pub(crate) fn new(token: CancellationToken) -> Self {
        Self {
            token: Mutex::new(token),
        }
    }

    /// Simulate a context reboot: the next `shutdown_token()` resolution
    /// observes `token` (test helper, not a production API).
    pub(crate) fn swap(&self, token: CancellationToken) {
        *self
            .token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = token;
    }
}

impl ComponentContext for SwappableContext {
    fn resolve_component(&self, _scheme: &str) -> Option<Arc<dyn camel_component_api::Component>> {
        None
    }

    fn resolve_language(&self, _name: &str) -> Option<Arc<dyn camel_language_api::Language>> {
        None
    }

    fn metrics(&self) -> Arc<dyn camel_api::MetricsCollector> {
        Arc::new(camel_api::NoOpMetrics)
    }

    fn platform_service(&self) -> Arc<dyn camel_api::PlatformService> {
        Arc::new(camel_api::NoopPlatformService::default())
    }

    fn register_route_health_check(
        &self,
        _route_id: &str,
        _check: Arc<dyn camel_api::AsyncHealthCheck>,
    ) {
    }

    fn unregister_route_health_check(&self, _route_id: &str) {}

    fn shutdown_token(&self) -> Option<CancellationToken> {
        Some(
            self.token
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )
    }
}
