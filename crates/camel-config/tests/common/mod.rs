//! Per-binary test coordination helpers for integration tests.
//!
//! Each tests/*.rs binary is a separate process; this module gives each
//! one its own lock and static instance. Mirrors the lib-test
//! ENV_OVERRIDE_LOCK discipline (crates/camel-config/src/config.rs).

static ENV_OVERRIDE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Test-only coordination mutex acquisition. Recovery from poison is
/// safe because every env test restores vars before assertions.
pub fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_OVERRIDE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Install a bare registry as the process-global tracing default, once
/// per test binary. Guards capture tests in this binary against
/// callsite-interest poisoning: `tracing` caches each callsite's
/// `Interest` process-wide from its FIRST macro execution, evaluated
/// against the executing thread's dispatcher. A subscriber-less sibling
/// test that hits a shared `warn!` callsite first caches
/// `Interest::never`, so a later thread-local `set_default` capture
/// silently drops events. The global registry heals prior poison and
/// floors future rebuilds at `sometimes` (fix pattern: c3853198; bd
/// rc-img5; convention: docs/testing/tracing-capture-guards.md).
///
/// Only `tests/cache_repo_config.rs` calls this today; the allow covers
/// the other common-consuming binaries that never reference it.
#[allow(dead_code)]
pub fn ensure_global_tracing_default() {
    static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if INIT.set(()).is_ok() {
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
    }
}
