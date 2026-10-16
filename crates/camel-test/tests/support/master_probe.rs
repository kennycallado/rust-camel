//! Observes `master_delegate_lifecycle_total{event=...}` so integration tests
//! can assert a delegate was started/stopped exactly N times (rc-w02ns: no
//! restart churn). Attach BEFORE adding routes.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{CamelError, Lifecycle, MetricsCollector};
use camel_core::CamelContext;

#[derive(Default)]
pub struct DelegateLifecycleProbe {
    events: Mutex<Vec<String>>,
}

impl DelegateLifecycleProbe {
    /// Number of `master_delegate_lifecycle_total` observations for `event`
    /// (`started`, `stopped`, `create_error`, `start_error`).
    pub fn count(&self, event: &str) -> usize {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|e| e.as_str() == event)
            .count()
    }
}

impl MetricsCollector for DelegateLifecycleProbe {
    fn record_exchange_duration(&self, _route_id: &str, _duration: Duration) {}
    fn increment_errors(&self, _route_id: &str, _error_type: &str) {}
    fn increment_exchanges(&self, _route_id: &str) {}
    fn set_queue_depth(&self, _queue: &str, _depth: usize) {}
    fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {}

    fn record_counter(&self, name: &str, _value: f64, labels: &[(&str, &str)]) {
        if name != "master_delegate_lifecycle_total" {
            return;
        }
        if let Some((_, event)) = labels.iter().find(|(k, _)| *k == "event") {
            self.events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((*event).to_string());
        }
    }
}

struct ProbeLifecycle(Arc<DelegateLifecycleProbe>);

#[async_trait]
impl Lifecycle for ProbeLifecycle {
    fn name(&self) -> &str {
        "master-delegate-lifecycle-probe"
    }

    async fn start(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn as_metrics_collector(&self) -> Option<Arc<dyn MetricsCollector>> {
        Some(Arc::clone(&self.0) as Arc<dyn MetricsCollector>)
    }
}

/// Registers the probe as the context's metrics collector.
pub fn attach(ctx: CamelContext) -> (CamelContext, Arc<DelegateLifecycleProbe>) {
    let probe = Arc::new(DelegateLifecycleProbe::default());
    (
        ctx.with_lifecycle(ProbeLifecycle(Arc::clone(&probe))),
        probe,
    )
}
