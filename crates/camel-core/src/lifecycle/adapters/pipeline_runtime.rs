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
use camel_api::{BoxProcessor, IdentityProcessor, SyncBoxProcessor};
use std::sync::Arc;

/// A reference-counted, atomically-swappable pipeline wrapper.
pub(crate) type SharedPipeline = Arc<ArcSwap<SyncBoxProcessor>>;

/// Create a new shared pipeline from a [`BoxProcessor`].
pub(crate) fn new_shared_pipeline(processor: BoxProcessor) -> SharedPipeline {
    Arc::new(ArcSwap::from_pointee(SyncBoxProcessor::new(processor)))
}

/// Create a new shared pipeline wrapping an [`IdentityProcessor`].
#[allow(dead_code)]
pub(crate) fn new_identity_pipeline() -> SharedPipeline {
    Arc::new(ArcSwap::from_pointee(SyncBoxProcessor::new(
        BoxProcessor::new(IdentityProcessor),
    )))
}

/// Atomically swap the pipeline.
///
/// In-flight requests finish with the old pipeline (kept alive by the Arc).
/// New requests immediately use the new pipeline.
pub(crate) fn swap_pipeline(pipeline: &SharedPipeline, new_processor: BoxProcessor) {
    pipeline.store(Arc::new(SyncBoxProcessor::new(new_processor)));
}

/// Get a clone of the current pipeline.
pub(crate) fn get_pipeline(pipeline: &SharedPipeline) -> BoxProcessor {
    pipeline.load().clone_inner()
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
        swap_pipeline(&shared, new_proc);

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
                    swap_pipeline(&s, new_proc);
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
