use async_trait::async_trait;
use camel_api::{CamelError, StepLifecycle, StepShutdownReason};
use std::sync::Arc;

/// Composes multiple [`StepLifecycle`] handles into one.
///
/// `start()` runs children in forward order; on failure it rolls back
/// already-started children in reverse order. `shutdown()` runs children in
/// reverse order and is best-effort — every child is called even if some
/// error, and all errors are aggregated into a single
/// `CamelError::ProcessorError`.
#[derive(Debug)]
pub(crate) struct CompositeStepLifecycle {
    children: Vec<Arc<dyn StepLifecycle>>,
}

impl CompositeStepLifecycle {
    /// Creates a new composite from the given children.
    ///
    /// # Panics
    ///
    /// Panics if `children` is empty — an empty composite is a programmer
    /// error.
    pub(crate) fn new(children: Vec<Arc<dyn StepLifecycle>>) -> Self {
        assert!(
            !children.is_empty(),
            "CompositeStepLifecycle requires at least one child"
        );
        Self { children }
    }
}

#[async_trait]
impl StepLifecycle for CompositeStepLifecycle {
    fn name(&self) -> &'static str {
        "composite"
    }

    async fn start(&self) -> Result<(), CamelError> {
        for (i, child) in self.children.iter().enumerate() {
            match child.start().await {
                Ok(()) => {}
                Err(e) => {
                    // Rollback: shutdown already-started children in reverse
                    // order.
                    for j in (0..i).rev() {
                        if let Err(e) = self.children[j]
                            .shutdown(StepShutdownReason::RouteStop)
                            .await
                        {
                            tracing::warn!(
                                child = self.children[j].name(),
                                error = %e,
                                "composite start-rollback shutdown failed"
                            );
                        }
                    }
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    async fn shutdown(&self, reason: StepShutdownReason) -> Result<(), CamelError> {
        let mut errors: Vec<(&'static str, CamelError)> = Vec::new();
        for child in self.children.iter().rev() {
            match child.shutdown(reason).await {
                Ok(()) => {}
                Err(e) => {
                    errors.push((child.name(), e));
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            let msg = errors
                .into_iter()
                .map(|(name, err)| format!("child '{name}': {err}"))
                .collect::<Vec<_>>()
                .join("; ");
            Err(CamelError::ProcessorError(msg))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::StepShutdownReason;
    use std::sync::Mutex;

    #[derive(Debug)]
    struct FakeStep {
        name: &'static str,
        order: Arc<Mutex<Vec<&'static str>>>,
        start_result: Result<(), CamelError>,
        shutdown_result: Result<(), CamelError>,
    }

    impl FakeStep {
        fn new(name: &'static str, order: Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self {
                name,
                order,
                start_result: Ok(()),
                shutdown_result: Ok(()),
            }
        }

        fn with_start_fail(mut self, err: CamelError) -> Self {
            self.start_result = Err(err);
            self
        }

        fn with_shutdown_fail(mut self, err: CamelError) -> Self {
            self.shutdown_result = Err(err);
            self
        }
    }

    #[async_trait]
    impl StepLifecycle for FakeStep {
        fn name(&self) -> &'static str {
            self.name
        }

        async fn start(&self) -> Result<(), CamelError> {
            if self.start_result.is_ok() {
                self.order.lock().unwrap().push(self.name);
            }
            self.start_result.clone()
        }

        async fn shutdown(&self, _reason: StepShutdownReason) -> Result<(), CamelError> {
            self.order.lock().unwrap().push(self.name);
            self.shutdown_result.clone()
        }
    }

    #[tokio::test]
    async fn test_composite_start_runs_forward() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let a: Arc<dyn StepLifecycle> = Arc::new(FakeStep::new("a", order.clone()));
        let b: Arc<dyn StepLifecycle> = Arc::new(FakeStep::new("b", order.clone()));
        let composite = CompositeStepLifecycle::new(vec![a, b]);

        composite.start().await.unwrap();

        let guard = order.lock().unwrap();
        assert_eq!(*guard, vec!["a", "b"]);
    }

    #[tokio::test]
    async fn test_composite_shutdown_runs_reverse() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let a: Arc<dyn StepLifecycle> = Arc::new(FakeStep::new("a", order.clone()));
        let b: Arc<dyn StepLifecycle> = Arc::new(FakeStep::new("b", order.clone()));
        let composite = CompositeStepLifecycle::new(vec![a, b]);

        composite
            .shutdown(StepShutdownReason::RouteStop)
            .await
            .unwrap();

        let guard = order.lock().unwrap();
        assert_eq!(*guard, vec!["b", "a"]);
    }

    #[tokio::test]
    async fn test_composite_start_failure_rollbacks_started() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let a: Arc<dyn StepLifecycle> = Arc::new(FakeStep::new("a", order.clone()));
        let b: Arc<dyn StepLifecycle> = Arc::new(
            FakeStep::new("b", order.clone())
                .with_start_fail(CamelError::ProcessorError("b failed".into())),
        );
        let c: Arc<dyn StepLifecycle> = Arc::new(FakeStep::new("c", order.clone()));
        let composite = CompositeStepLifecycle::new(vec![a, b, c]);

        let result = composite.start().await;

        assert!(result.is_err());
        let guard = order.lock().unwrap();
        // "a" started Ok, then got shutdown in rollback.
        assert_eq!(*guard, vec!["a", "a"]);
    }

    #[tokio::test]
    async fn test_composite_start_failure_rollback_is_reverse_order() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let a: Arc<dyn StepLifecycle> = Arc::new(FakeStep::new("a", order.clone()));
        let b: Arc<dyn StepLifecycle> = Arc::new(FakeStep::new("b", order.clone()));
        let c: Arc<dyn StepLifecycle> = Arc::new(
            FakeStep::new("c", order.clone())
                .with_start_fail(CamelError::ProcessorError("c failed".into())),
        );
        let d: Arc<dyn StepLifecycle> = Arc::new(FakeStep::new("d", order.clone()));
        let composite = CompositeStepLifecycle::new(vec![a, b, c, d]);

        let result = composite.start().await;

        assert!(result.is_err());
        let guard = order.lock().unwrap();
        // "a" and "b" started Ok; rollback shuts them down in reverse: b then a.
        // "c" failed at start, so it never started — no shutdown for c or d.
        assert_eq!(*guard, vec!["a", "b", "b", "a"]);
    }

    #[tokio::test]
    async fn test_composite_shutdown_best_effort_all_called() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let a: Arc<dyn StepLifecycle> = Arc::new(FakeStep::new("a", order.clone()));
        let b: Arc<dyn StepLifecycle> = Arc::new(
            FakeStep::new("b", order.clone())
                .with_shutdown_fail(CamelError::ProcessorError("b failed".into())),
        );
        let c: Arc<dyn StepLifecycle> = Arc::new(FakeStep::new("c", order.clone()));
        let composite = CompositeStepLifecycle::new(vec![a, b, c]);

        let result = composite.shutdown(StepShutdownReason::RouteStop).await;

        assert!(result.is_err());
        let guard = order.lock().unwrap();
        // All three called despite b's failure; reverse order.
        assert_eq!(*guard, vec!["c", "b", "a"]);
    }

    #[tokio::test]
    async fn test_composite_shutdown_aggregates_multiple_errors() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let a: Arc<dyn StepLifecycle> = Arc::new(
            FakeStep::new("a", order.clone())
                .with_shutdown_fail(CamelError::ProcessorError("alpha error".into())),
        );
        let b: Arc<dyn StepLifecycle> = Arc::new(FakeStep::new("b", order.clone()));
        let c: Arc<dyn StepLifecycle> = Arc::new(
            FakeStep::new("c", order.clone())
                .with_shutdown_fail(CamelError::ProcessorError("gamma error".into())),
        );
        let composite = CompositeStepLifecycle::new(vec![a, b, c]);

        let result = composite.shutdown(StepShutdownReason::RouteStop).await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("alpha error"),
            "expected 'alpha error' in: {err_msg}"
        );
        assert!(
            err_msg.contains("gamma error"),
            "expected 'gamma error' in: {err_msg}"
        );
    }
}
