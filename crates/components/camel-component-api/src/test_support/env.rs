//! Serialized, restoring mutation of process environment variables for tests.
//!
//! `std::env::set_var`/`remove_var` are `unsafe` because concurrent reads from
//! other threads are UB. This module gives every Rust-side participant a single
//! process-wide lock (`ENV_LOCK`) and an RAII [`EnvGuard`] that records prior
//! values and restores them on drop.
//!
//! # Safety posture
//!
//! All Rust-side participants that opt into this module are serialized by
//! `ENV_LOCK`, so their reads and writes never overlap. Non-Rust (libc/FFI)
//! environment readers cannot be excluded in a multithreaded test process:
//! that is an accepted residual, recorded by rc-99d5 R4 as amended — the guard
//! "narrows but is not hermetic".
//!
//! Mitigation hierarchy, in order of preference:
//!
//! 1. Inject configuration instead of reading the process environment.
//! 2. Exercise env-reading behavior out of process (spawn a binary/test child).
//! 3. Guarded in-process mutation via [`EnvGuard`] — last resort.
//!
//! # Lock ordering
//!
//! The lock is non-reentrant. Never construct a second [`EnvGuard`] (or call
//! [`env_lock`]) while another guard is alive on the same thread; use the
//! `and_set`/`and_remove` chaining mutators instead.

#[cfg(test)]
#[path = "env_tests.rs"]
mod env_tests;

use std::env;
use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard, PoisonError, TryLockError};

/// Process-wide serialization point for all guarded env mutation and reads.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Acquire the process-wide environment lock, resuming from poisoning.
///
/// Poisoning means a prior holder panicked while mutating env variables; the
/// guard's `Drop` still restored its recorded values during unwinding, so the
/// environment is consistent and the lock is safe to reuse.
#[must_use = "dropping the guard immediately releases the environment lock"]
pub fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Try to acquire the environment lock without blocking.
///
/// Returns `Some` when uncontended (or merely poisoned — resumed), `None` when
/// another thread (or a guard on this thread) holds it.
#[must_use = "dropping the probe immediately releases the environment lock"]
pub fn env_try_lock() -> Option<MutexGuard<'static, ()>> {
    match ENV_LOCK.try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::Poisoned(e)) => Some(e.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

/// RAII environment guard: records prior values and restores them on drop.
///
/// The `_lock` field is declared last so it is dropped last: struct fields
/// drop in declaration order after `Drop::drop` returns, so restoration runs
/// while the environment lock is still held.
#[must_use = "the guard restores environment variables only while it is alive"]
pub struct EnvGuard {
    undo: Vec<(String, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// Acquire the environment lock without mutating anything yet.
    pub fn acquire() -> Self {
        Self {
            undo: Vec::new(),
            _lock: env_lock(),
        }
    }

    /// Acquire the lock, then set `key` to `value` until the guard drops.
    pub fn set(key: impl Into<String>, value: impl Into<OsString>) -> Self {
        let mut guard = Self::acquire();
        guard.and_set(key, value);
        guard
    }

    /// Acquire the lock, then remove `key` until the guard drops.
    pub fn remove(key: impl Into<String>) -> Self {
        let mut guard = Self::acquire();
        guard.and_remove(key);
        guard
    }

    /// Set `key` to `value`, recording the prior value for restoration.
    pub fn and_set(&mut self, key: impl Into<String>, value: impl Into<OsString>) -> &mut Self {
        let key = key.into();
        let value = value.into();
        self.undo.push((key.clone(), env::var_os(&key)));
        // SAFETY: every Rust-side env participant is serialized by ENV_LOCK,
        // which this guard holds for the whole mutation window.
        unsafe { env::set_var(&key, &value) };
        self
    }

    /// Remove `key`, recording the prior value for restoration.
    pub fn and_remove(&mut self, key: impl Into<String>) -> &mut Self {
        let key = key.into();
        self.undo.push((key.clone(), env::var_os(&key)));
        // SAFETY: every Rust-side env participant is serialized by ENV_LOCK,
        // which this guard holds for the whole mutation window.
        unsafe { env::remove_var(&key) };
        self
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // Reverse order so repeated mutations of one key unwind to the original.
        while let Some((key, previous)) = self.undo.pop() {
            match previous {
                Some(previous) => {
                    // SAFETY: ENV_LOCK is still held (`_lock` drops last), so no
                    // Rust-side participant can observe this write concurrently.
                    unsafe { env::set_var(&key, previous) };
                }
                None => {
                    // SAFETY: ENV_LOCK is still held (`_lock` drops last), so no
                    // Rust-side participant can observe this removal concurrently.
                    unsafe { env::remove_var(&key) };
                }
            }
        }
    }
}

/// Test fixture that installs environment pre-state and restores the ambient
/// value on drop. It holds the lock only for each mutation, never across the
/// test body, because [`EnvGuard`]'s lock is non-reentrant.
#[cfg(test)]
struct Fixture {
    key: &'static str,
    ambient: Option<OsString>,
}

#[cfg(test)]
impl Fixture {
    fn install(key: &'static str, value: Option<&str>) -> Self {
        let _lock = env_lock();
        let ambient = env::var_os(key);
        match value {
            // SAFETY: ENV_LOCK is held for this mutation, serializing it
            // against every other guarded Rust-side env reader/writer.
            Some(value) => unsafe { env::set_var(key, value) },
            // SAFETY: as above; the lock is held for this removal.
            None => unsafe { env::remove_var(key) },
        }
        Self { key, ambient }
    }
}

#[cfg(test)]
impl Drop for Fixture {
    fn drop(&mut self) {
        let _lock = env_lock();
        match &self.ambient {
            // SAFETY: ENV_LOCK is held; this restores the ambient value.
            Some(value) => unsafe { env::set_var(self.key, value) },
            // SAFETY: ENV_LOCK is held; this restores the ambient absence.
            None => unsafe { env::remove_var(self.key) },
        }
    }
}
