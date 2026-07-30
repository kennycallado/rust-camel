//! `template:` scheme Endpoint for the external template component
//! (ADR-0047 Stage 2, Phase 4 / Task 4.3).
//!
//! [`TemplateEndpoint`] is the producer-only endpoint returned by
//! [`TemplateComponent::create_endpoint`]. It holds:
//!
//! - the operator-resolved [`ResolvedExternalTemplateLimits`] (fail-closed
//!   on zero, see [`crate::config`]),
//! - the per-route render limits ([`MinijinjaLimitsConfig`]) applied at
//!   render time,
//! - the shared compiled-set [`SharedTemplates`] cell (seeded empty and
//!   filled by the lifecycle `start()` in Task 4.4),
//! - the `Arc<dyn RuntimeObservability>` stashed at `create_producer` so
//!   the lifecycle handle can later reach metrics/health surfaces.
//!
//! `create_consumer` is rejected: the `template:` scheme is producer-only.
//! The lifecycle handle is wired in [`Endpoint::lifecycle`] and is a stub
//! `StepLifecycle` until Task 4.4 implements the real `start()`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use camel_api::{BodyType, BoxProcessor, CamelError, StepLifecycle};
use camel_component_api::{Consumer, Endpoint, ProducerContext, RuntimeObservability};
use camel_language_api::MinijinjaLimitsConfig;
use camel_language_minijinja::ResolvedLimits;

use crate::config::ResolvedExternalTemplateLimits;
use crate::lifecycle::StartupBuildHandle;
use crate::producer::TemplateProducer;
use crate::template_set::{SharedTemplates, TemplateSet};

/// Producer-only Endpoint for the `template:` scheme.
#[allow(dead_code)] // fields seeded for Phase 4/5 wiring; consumed via impls below.
pub(crate) struct TemplateEndpoint {
    uri: String,
    entry_abs_path: PathBuf,
    limits: ResolvedExternalTemplateLimits,
    render_limits: MinijinjaLimitsConfig,
    route_id: String,
    shared: SharedTemplates,
    rt: Mutex<Option<Arc<dyn RuntimeObservability>>>,
}

impl TemplateEndpoint {
    /// Construct an endpoint from already-resolved limits and the parsed
    /// entry path. `shared` is seeded with an empty [`TemplateSet`] —
    /// `start()` (Task 4.4) replaces it with a compiled set.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        uri: String,
        entry_abs_path: PathBuf,
        limits: ResolvedExternalTemplateLimits,
        render_limits: MinijinjaLimitsConfig,
        route_id: String,
    ) -> Self {
        Self {
            uri,
            entry_abs_path,
            limits,
            render_limits,
            route_id,
            shared: Arc::new(ArcSwap::from_pointee(TemplateSet::empty())),
            rt: Mutex::new(None),
        }
    }
}

impl Endpoint for TemplateEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn body_contract(&self) -> Option<BodyType> {
        None
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "template is producer-only".to_string(),
        ))
    }

    fn create_producer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        // Stash the runtime observability handle so the lifecycle (Task 4.4)
        // can read metrics/health on its own thread.
        *self.rt.lock().expect("rt cell poisoned") = Some(Arc::clone(&rt)); // allow-unwrap
        Ok(BoxProcessor::new(TemplateProducer::new(
            Arc::clone(&self.shared),
            ResolvedLimits::from_config(&self.render_limits),
            Some(rt),
            self.route_id.clone(),
        )))
    }

    /// Expose a `StepLifecycle` for the runtime to start/shut down. The
    /// returned handle is a stub until Task 4.4 implements the real
    /// `start()` (open root → build snapshot → compile → seed `shared`).
    fn lifecycle(&self) -> Option<Arc<dyn StepLifecycle>> {
        let rt = self
            .rt
            .lock()
            .expect("rt cell poisoned") // allow-unwrap
            .as_ref()
            .map(Arc::clone);
        Some(Arc::new(StartupBuildHandle {
            shared: Arc::clone(&self.shared),
            entry_abs_path: self.entry_abs_path.clone(),
            render_limits: self.render_limits.clone(),
            limits: self.limits,
            rt,
            route_id: self.route_id.clone(),
            handler: Mutex::new(None),
            guard: Mutex::new(None),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::test_support::PanicRuntimeObservability;
    use camel_language_api::MinijinjaLimitsConfig;

    /// Build a `TemplateEndpoint` with reasonable defaults for the
    /// endpoint-only tests. The constructor is `pub(crate)` and the
    /// `ResolvedExternalTemplateLimits` field is otherwise tedious to
    /// spell out at every call site.
    fn make_endpoint() -> TemplateEndpoint {
        TemplateEndpoint::new(
            "template:file:///srv/t/page.html".to_string(),
            PathBuf::from("/srv/t/page.html"),
            ResolvedExternalTemplateLimits {
                max_total_source_bytes: 16 * 1024 * 1024,
                max_include_count: 64,
                max_include_depth: 16,
                max_template_size: 1024 * 1024,
                reload_timeout_ms: 5000,
            },
            MinijinjaLimitsConfig::default(),
            "test-route".to_string(),
        )
    }

    /// `template:` is producer-only; `create_consumer` MUST return
    /// `EndpointCreationFailed`. Pin the variant + message: a bare
    /// `is_err()` would pass if a future refactor returned the wrong
    /// kind. `Ok(_)` is bound without formatting to sidestep the
    /// `Box<dyn Consumer>: !Debug` constraint.
    #[test]
    fn endpoint_create_consumer_errors() {
        let endpoint = make_endpoint();
        let rt: Arc<dyn RuntimeObservability> = Arc::new(PanicRuntimeObservability);
        match endpoint.create_consumer(rt) {
            Err(CamelError::EndpointCreationFailed(msg)) => {
                assert!(msg.contains("producer-only"), "wrong message: {msg}")
            }
            Err(e) => panic!("wrong error variant: {e:?}"),
            Ok(_) => panic!("producer-only endpoint must reject create_consumer"),
        }
    }

    /// `lifecycle()` must return `Some(Arc<dyn StepLifecycle>)` so the
    /// runtime can drive the startup build. A `None` return would leave
    /// the endpoint with an empty `SharedTemplates` and every render
    /// would fail with "template lookup".
    #[test]
    fn endpoint_lifecycle_returns_handle() {
        let endpoint = make_endpoint();
        let handle = endpoint.lifecycle();
        assert!(
            handle.is_some(),
            "lifecycle() must return Some(StartupBuildHandle)"
        );
    }
}
