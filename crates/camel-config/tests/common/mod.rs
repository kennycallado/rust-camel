//! Per-binary env coordination for integration tests.
//!
//! Each tests/*.rs binary is a separate process; this module gives each
//! one its own lock instance. Mirrors the lib-test ENV_OVERRIDE_LOCK
//! discipline (crates/camel-config/src/config.rs).

static ENV_OVERRIDE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Test-only coordination mutex acquisition. Recovery from poison is
/// safe because every env test restores vars before assertions.
pub fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_OVERRIDE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
