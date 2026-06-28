//! Atomic pipeline swap utilities.
//!
//! Provides a [`SharedPipeline`] type alias and helpers for atomic pipeline
//! replacement, used by [`DefaultRouteController`](super::route_controller::DefaultRouteController)
//! to support hot-reload of route pipelines with zero-downtime semantics.
//!
//! # Layering
//!
//! `swap` exists at three levels. Only the lowest (this module) moves:
//! 1. **DefaultRouteController** inherent pipeline-swap/get on `SharedPipeline` → **here**
//! 2. `RouteControllerHandle::swap_pipeline` (controller_actor.rs) → **stays in controller_actor**
//! 3. `RuntimeExecutionHandle::swap_route_pipeline` (context.rs) → **stays in context.rs**

use arc_swap::ArcSwap;
use camel_api::{BoxProcessor, IdentityProcessor, StepLifecycle, SyncBoxProcessor};
use std::sync::Arc;

/// An atomically-swapped pipeline snapshot: the processor plus the stateful
/// lifecycle handles that must be drained when this snapshot is retired.
pub(crate) struct PipelineAssembly {
    pub(crate) processor: SyncBoxProcessor,
    pub(crate) lifecycle: Vec<Arc<dyn StepLifecycle>>,
}

impl PipelineAssembly {
    pub(crate) fn new(processor: SyncBoxProcessor, lifecycle: Vec<Arc<dyn StepLifecycle>>) -> Self {
        Self {
            processor,
            lifecycle,
        }
    }
}

/// A reference-counted, atomically-swappable pipeline wrapper.
pub(crate) type SharedPipeline = Arc<ArcSwap<PipelineAssembly>>;

/// Create a new shared pipeline from a [`BoxProcessor`].
pub(crate) fn new_shared_pipeline(processor: BoxProcessor) -> SharedPipeline {
    Arc::new(ArcSwap::from_pointee(PipelineAssembly::new(
        SyncBoxProcessor::new(processor),
        vec![],
    )))
}

/// Create a new shared pipeline with lifecycle handles.
///
/// Use this in route compilation when steps carry stateful lifecycle handles
/// that must be drained on shutdown. The lifecycle vec may be empty (same as
/// [`new_shared_pipeline`]).
pub(crate) fn new_shared_pipeline_with_lifecycle(
    processor: BoxProcessor,
    lifecycle: Vec<Arc<dyn StepLifecycle>>,
) -> SharedPipeline {
    Arc::new(ArcSwap::from_pointee(PipelineAssembly::new(
        SyncBoxProcessor::new(processor),
        lifecycle,
    )))
}

/// Create a new shared pipeline wrapping an [`IdentityProcessor`].
#[allow(dead_code)]
pub(crate) fn new_identity_pipeline() -> SharedPipeline {
    Arc::new(ArcSwap::from_pointee(PipelineAssembly::new(
        SyncBoxProcessor::new(BoxProcessor::new(IdentityProcessor)),
        vec![],
    )))
}

/// Raw swap — no lifecycle drain. Only for non-lifecycle routes.
///
/// Accepts `lifecycle` so that the new pipeline assembly records the
/// lifecycle handles from the compiled steps.  When the route is subsequently
/// stopped, these handles are drained.
pub(crate) fn swap_pipeline_raw(
    pipeline: &SharedPipeline,
    new_processor: BoxProcessor,
    lifecycle: Vec<Arc<dyn StepLifecycle>>,
) {
    pipeline.store(Arc::new(PipelineAssembly::new(
        SyncBoxProcessor::new(new_processor),
        lifecycle,
    )));
}

/// Get a clone of the current pipeline.
pub(crate) fn get_pipeline(pipeline: &SharedPipeline) -> BoxProcessor {
    pipeline.load().processor.clone_inner()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    #[tokio::test]
    async fn swap_and_get_pipeline() {
        let shared = new_identity_pipeline();
        let mut p1 = get_pipeline(&shared);
        assert!(p1.ready().await.is_ok());

        let new_proc = BoxProcessor::new(IdentityProcessor);
        swap_pipeline_raw(&shared, new_proc, vec![]);

        let mut p2 = get_pipeline(&shared);
        assert!(p2.ready().await.is_ok());
    }

    #[tokio::test]
    async fn concurrent_access() {
        let shared = new_identity_pipeline();
        let counter = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();

        // 4 concurrent tasks: 2 readers, 2 writers
        for i in 0..4 {
            let s = Arc::clone(&shared);
            let c = Arc::clone(&counter);
            handles.push(tokio::spawn(async move {
                if i % 2 == 0 {
                    // writer
                    let new_proc = BoxProcessor::new(IdentityProcessor);
                    swap_pipeline_raw(&s, new_proc, vec![]);
                    c.fetch_add(1, Ordering::Relaxed);
                } else {
                    // reader
                    let _pipe = get_pipeline(&s);
                    c.fetch_add(1, Ordering::Relaxed);
                }
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(counter.load(Ordering::Relaxed), 4);
    }
}
