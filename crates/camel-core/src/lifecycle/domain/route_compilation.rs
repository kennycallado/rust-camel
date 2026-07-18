//! Route compilation contract types — the value objects that route compilation
//! produces and that downstream ports/use-cases consume.
//!
//! `CompiledPipeline` is a pure contract value type: no runtime handles, no
//! I/O, no framework coupling. Its fields reference the [`camel_api`] contract
//! crate (a `BoxProcessor` and the `StepLifecycle` trait), so it can safely
//! live in the domain ring and be referenced by application-ring ports without
//! violating the dependency rule (`domain ← application ← adapters`).
//!
//! (`PreparedRoute` — the companion type one might expect here — is NOT
//! relocatable to domain: its `managed: ManagedRoute` field bundles
//! adapter-internal state. It stays in `lifecycle::adapters` and the port
//! references it under a documented charter §4 exception. See
//! `port_traits_do_not_import_from_adapter_ring` test doc + ADR-0045 §4.)
//!
//! ADR-0045 §1 rings — domain is the innermost ring; ports (application ring)
//! depend on domain types, never on adapter types.

use std::sync::Arc;

use camel_api::{BoxProcessor, StepLifecycle};

/// A compiled pipeline bundle carrying both the processor and its lifecycle
/// handles. Returned by route compilation so callers (especially the hot-reload
/// Restart path) can thread lifecycle into the raw pipeline swap path.
#[derive(Debug)]
pub(crate) struct CompiledPipeline {
    pub(crate) processor: BoxProcessor,
    pub(crate) lifecycle: Vec<Arc<dyn StepLifecycle>>,
}
