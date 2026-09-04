//! Inline route dispatcher — the camel-core side of the
//! [`InlineRouteDispatcher`] capability (direct-inline-dispatch Task 2.2).
//!
//! [`RouteInlineDispatcher`] mirrors the envelope drain path in
//! [`route_controller_trait`](super::route_controller_trait) stage for stage:
//! one pipeline snapshot per dispatch (ADR-0004 atomic-swap discipline), the
//! startup-cohort gate, `ready_with_backoff`, the `CANCEL_TOKEN` scope, and
//! `DrainGuard` accounting — except the Exchange is handed over in-process
//! instead of through the consumer channel.
//!
//! Logging: the dispatcher emits nothing on normal operation. The producer
//! that invokes it (camel-direct) owns the b′ emission for dispatch results.
//! Error paths resolve with `CamelError` values only.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use camel_api::{CamelError, Exchange};
use camel_component_api::InlineRouteDispatcher;
use tokio_util::sync::CancellationToken;
use tower::Service;

use crate::lifecycle::adapters::pipeline_runtime::SharedPipeline;
use crate::lifecycle::adapters::route_compiler::CANCEL_TOKEN;
use crate::lifecycle::adapters::route_helpers::{DrainGuard, ready_with_backoff};
use crate::lifecycle::cohort_activation::CohortActivationGate;

/// Completed hops between fairness yields.
const HOP_YIELD_INTERVAL: u32 = 32;

/// Interior-mutable dispatcher state, shared by every dispatch future.
struct DispatcherState {
    route_id: String,
    /// Pipeline swap source — the same `Arc<ArcSwap<..>>` the envelope
    /// drain path loads its snapshot from.
    pipeline: SharedPipeline,
    /// The route's `pipeline_cancel` child token for this boot.
    cancel: CancellationToken,
    /// Drain counter shared with the envelope path; `DrainGuard` decrements
    /// exactly once on every dispatch exit path.
    drain_in_flight: Arc<AtomicU64>,
    /// FIFO admission permit serializing concurrent producers through the
    /// pipeline.
    admission: Arc<tokio::sync::Mutex<()>>,
    /// Startup-cohort barrier — the same gate the envelope drain sites park
    /// on, so the barrier covers the inline topology too.
    cohort: Arc<CohortActivationGate>,
    /// Fairness yield counter, cumulative across ALL dispatches through this
    /// dispatcher.
    hop_budget: AtomicU32,
    /// Test-only count of times the `yield_now` fairness site fired.
    #[cfg(test)]
    yields: AtomicU32,
}

/// Capability published on the [`ConsumerContext`](camel_component_api::ConsumerContext)
/// for non-Concurrent route topologies: runs an Exchange straight through the
/// route pipeline (request-reply) without a channel round-trip.
///
/// Constructed once per boot at the publication site in
/// [`route_controller_trait`](super::route_controller_trait), before the
/// consumer spawns (and therefore before any `mark_ready`).
pub(crate) struct RouteInlineDispatcher {
    state: Arc<DispatcherState>,
}

impl std::fmt::Debug for RouteInlineDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouteInlineDispatcher")
            .field("route_id", &self.state.route_id)
            .finish()
    }
}

impl RouteInlineDispatcher {
    pub(crate) fn new(
        route_id: String,
        pipeline: SharedPipeline,
        cancel: CancellationToken,
        drain_in_flight: Arc<AtomicU64>,
        cohort: Arc<CohortActivationGate>,
    ) -> Self {
        Self {
            state: Arc::new(DispatcherState {
                route_id,
                pipeline,
                cancel,
                drain_in_flight,
                admission: Arc::new(tokio::sync::Mutex::new(())),
                cohort,
                hop_budget: AtomicU32::new(0),
                #[cfg(test)]
                yields: AtomicU32::new(0),
            }),
        }
    }

    #[cfg(test)]
    fn hop_budget_for_test(&self) -> u32 {
        self.state.hop_budget.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn yields_for_test(&self) -> u32 {
        self.state.yields.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn admission_for_test(&self) -> &Arc<tokio::sync::Mutex<()>> {
        &self.state.admission
    }
}

impl InlineRouteDispatcher for RouteInlineDispatcher {
    fn dispatch(
        &self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send + 'static>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            // Drain accounting starts BEFORE the operation: the guard's Drop
            // runs exactly once on every exit path — producer cancellation
            // (future drop), consumer cancellation, success, or error.
            let _drain_guard = DrainGuard::new(Arc::clone(&state.drain_in_flight));
            // ONE snapshot for the whole call (ADR-0004 atomic-swap
            // discipline; mirrors the envelope drain path).
            let mut pipe = state.pipeline.load().processor.clone_inner();

            let admission = Arc::clone(&state.admission);
            let cohort = Arc::clone(&state.cohort);
            let operation_cancel = state.cancel.clone();

            // Operation, strictly ordered: admission → cohort gate →
            // readiness → scoped pipeline call. Dropping this future on the
            // cancel arm below drops every stage and releases the admission
            // permit.
            let operation = async move {
                // FIFO serialization of concurrent producers.
                let _admission = admission.lock().await;
                // Park until the startup cohort opens (level-triggered, same
                // mechanism as the envelope drain sites).
                let mut cohort_rx = cohort.subscribe();
                let _ = cohort_rx.wait_for(|open| *open).await;
                ready_with_backoff(&mut pipe, &operation_cancel).await?;
                CANCEL_TOKEN
                    .scope(operation_cancel, async move { pipe.call(exchange).await })
                    .await
            };

            // Consumer-cancel wins ties: the biased select polls the cancel
            // arm first.
            let result = tokio::select! {
                biased;
                _ = state.cancel.cancelled() => Err(CamelError::ConsumerStopping),
                result = operation => result,
            };

            if result.is_ok() {
                let prev = state.hop_budget.fetch_add(1, Ordering::Relaxed);
                if (prev + 1).is_multiple_of(HOP_YIELD_INTERVAL) {
                    #[cfg(test)]
                    state.yields.fetch_add(1, Ordering::Relaxed);
                    tokio::task::yield_now().await;
                }
            }
            result
        })
    }
}

#[cfg(test)]
#[path = "inline_dispatcher_tests.rs"]
mod tests;
