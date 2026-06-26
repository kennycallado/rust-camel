use crate::CamelError;
use async_trait::async_trait;

/// Why a stateful pipeline step is being shut down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepShutdownReason {
    /// The route is stopping (`stop_route`).
    RouteStop,
    /// The pipeline is being replaced via hot reload (Restart path).
    HotSwap,
}

/// Lifecycle hook for **stateful** pipeline steps that own background work
/// (timers, buckets, gap-detectors, queues) beyond a single `process()` call.
///
/// Stateless processors do NOT implement this trait. The runtime collects
/// `Arc<dyn StepLifecycle>` at compile time and drains them in route order
/// during `stop_route` and hot-swap. See ADR-0022.
///
/// **Why `&self`, not `&mut self`?** `Lifecycle` uses `&mut self` for exclusive
/// start/stop of services. `StepLifecycle` is dispatched through
/// `Arc<dyn StepLifecycle>` carried inside `ArcSwap` pipeline snapshots, so it
/// MUST be `&self` (shared-reference, interior-mutability) for `Arc` cloning and
/// concurrent snapshots to work. See ADR-0022.
///
/// `shutdown` MUST be idempotent. By the time it is called, intake is cancelled
/// and the pipeline task has been joined, so no `process()` is in flight.
/// `Err` is best-effort: the runtime logs and continues (it does NOT fail
/// `stop_route`), mirroring `CamelContext::stop` service handling.
#[async_trait]
pub trait StepLifecycle: std::fmt::Debug + Send + Sync + 'static {
    /// Stable name for logging/diagnostics.
    fn name(&self) -> &'static str;

    async fn shutdown(&self, reason: StepShutdownReason) -> Result<(), CamelError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Debug)]
    struct FakeStep {
        shutdowns: Mutex<Vec<StepShutdownReason>>,
    }

    #[async_trait]
    impl StepLifecycle for FakeStep {
        fn name(&self) -> &'static str {
            "fake"
        }
        async fn shutdown(&self, reason: StepShutdownReason) -> Result<(), CamelError> {
            self.shutdowns.lock().unwrap().push(reason);
            Ok(())
        }
    }

    #[tokio::test]
    async fn dyn_dispatch_works() {
        // Keep concrete handle for assertion, dispatch through Arc<dyn StepLifecycle>
        // (the ArcSwap-snapshot shape).
        let inner = Arc::new(FakeStep {
            shutdowns: Mutex::new(vec![]),
        });
        let step: Arc<dyn StepLifecycle> = inner.clone();
        step.shutdown(StepShutdownReason::RouteStop).await.unwrap();
        step.shutdown(StepShutdownReason::HotSwap).await.unwrap();
        assert_eq!(
            *inner.shutdowns.lock().unwrap(),
            vec![StepShutdownReason::RouteStop, StepShutdownReason::HotSwap,]
        );
    }

    #[tokio::test]
    async fn shutdown_err_is_not_fatal() {
        #[derive(Debug)]
        struct FailingStep;
        #[async_trait]
        impl StepLifecycle for FailingStep {
            fn name(&self) -> &'static str {
                "fail"
            }
            async fn shutdown(&self, _: StepShutdownReason) -> Result<(), CamelError> {
                Err(CamelError::ProcessorError("boom".into()))
            }
        }
        let step: Arc<dyn StepLifecycle> = Arc::new(FailingStep);
        let result = step.shutdown(StepShutdownReason::RouteStop).await;
        assert!(result.is_err());
        // But typical drain loop continues (log + skip), see Task 5.
    }
}
