//! Test-only log capture shim for camel-processor test modules.
//!
//! `#[cfg(test)]`-gated and `pub(crate)`. The capture families
//! (`capture_debugs_with_span_records`, `captured_field`, `record_field`)
//! live in `camel_component_api::test_support::log_capture`; this module
//! delegates to them so the existing `crate::test_log_capture::*` imports
//! keep working (promotion: bd rc-421es).
//!
//! The process-global guard [`ensure_global_registry`] STAYS LOCAL: it is
//! the colocated guard of this crate's lib test binary, and the shared
//! module deliberately installs thread-local subscribers only (rc-img5 /
//! rc-6jarb; see `docs/testing/tracing-capture-guards.md`). Every test in
//! this binary that triggers the shared error-handler `debug!`/`error!`
//! callsites must run the guard first.
//!
//! The rendering contract (unquoted `message` first, then `field="value"`
//! pairs) is documented on the shared module's functions.

/// OnceLock-gated global registry install: heals/prevents callsite-
/// interest poisoning of the shared error-handler `debug!`/`error!`
/// callsites (`error_handler.rs:280/370` and the system-broken
/// `error!` sites in `send_to_handler`), which subscriber-less
/// sibling error-handler tests in this binary can hit first (fix
/// pattern: c3853198; bd rc-img5). Every test that triggers those
/// callsites must call this BEFORE the first evaluation, otherwise
/// the callsite interest is cached as `never` process-wide.
pub(crate) fn ensure_global_registry() {
    static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if INIT.set(()).is_ok() {
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
    }
}

/// Runs `f` with capturing layers installed and returns `(f's result,
/// captured event field strings, captured span record strings)`.
///
/// Thin wrapper: the implementation lives in
/// `camel_component_api::test_support::log_capture` (promotion: bd
/// rc-421es); the guard stays per-binary because `tracing` caches callsite
/// interest process-wide.
pub(crate) fn capture_debugs_with_span_records<T>(
    f: impl FnOnce() -> T,
) -> (T, Vec<String>, Vec<String>) {
    ensure_global_registry();
    camel_component_api::test_support::log_capture::capture_debugs_with_span_records(f)
}

pub(crate) use camel_component_api::test_support::log_capture::{captured_field, record_field};
