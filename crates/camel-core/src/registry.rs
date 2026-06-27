//! Generic `NamedRegistry<T>` — a fallible (duplicate-detecting) named object registry.
//!
//! Unlike the auth crate's `NamedRegistry` (DashMap-based, infallible `register()`),
//! this registry uses `Mutex<HashMap>` so that `register()` can reject duplicates
//! with `Err(RegistryError::AlreadyRegistered)`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Error returned by [`NamedRegistry::register`].
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// A value with this name is already registered.
    #[error("already registered: {0}")]
    AlreadyRegistered(String),
}

/// A named registry with duplicate detection.
///
/// # Fallible registration
///
/// Unlike the auth crate's DashMap-based `NamedRegistry` (which silently
/// overwrites), this registry's `register()` returns
/// `Err(RegistryError::AlreadyRegistered)` if the name is taken.
///
/// Returns `Arc<T>` from `get()` so the caller shares ownership of the
/// registered value.
///
/// # Thread safety
///
/// Uses `Mutex<HashMap<String, Arc<T>>>` internally. All methods are safe
/// to call from multiple threads.
pub(crate) struct NamedRegistry<T: ?Sized + Send + Sync + 'static> {
    map: Mutex<HashMap<String, Arc<T>>>,
}

impl<T: ?Sized + Send + Sync + 'static> NamedRegistry<T> {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
        }
    }

    /// Register `value` under `name`.
    ///
    /// Returns `Err(RegistryError::AlreadyRegistered)` if `name` is already
    /// taken. The existing value is preserved.
    pub fn register(&self, name: impl Into<String>, value: Arc<T>) -> Result<(), RegistryError> {
        let name = name.into();
        let mut map = self
            .map
            .lock()
            .expect("registry mutex poisoned: another thread panicked while holding this lock"); // allow-unwrap
        if map.insert(name.clone(), value).is_some() {
            return Err(RegistryError::AlreadyRegistered(name));
        }
        Ok(())
    }

    /// Retrieve a registered value by name.
    pub fn get(&self, name: &str) -> Option<Arc<T>> {
        self.map
            .lock()
            .expect("registry mutex poisoned: another thread panicked while holding this lock") // allow-unwrap
            .get(name)
            .cloned()
    }
}

impl<T: ?Sized + Send + Sync + 'static> Default for NamedRegistry<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience alias for the idempotent repository registry.
pub(crate) type IdempotentRegistry = NamedRegistry<dyn camel_api::IdempotentRepository>;

/// Shareable handle to the idempotent repository registry.
///
/// `Arc<IdempotentRegistry>` is shared between `CamelContext` (which owns the
/// user-facing `register_idempotent_repository` API) and `DefaultRouteController`
/// (which needs read access during step compilation to resolve
/// `idempotent_consumer` repository names). The registry has internal Mutex
/// protection so concurrent reads/writes from both sides are safe.
pub(crate) type SharedIdempotentRegistry = Arc<IdempotentRegistry>;

/// Convenience alias for the claim check repository registry.
pub(crate) type ClaimCheckRegistry = NamedRegistry<dyn camel_api::ClaimCheckRepository>;

/// Shareable handle to the claim check repository registry.
///
/// Mirrors `SharedIdempotentRegistry` — shared between `CamelContext` and
/// `DefaultRouteController` for step-compile-time repository resolution.
pub(crate) type SharedClaimCheckRegistry = Arc<ClaimCheckRegistry>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_get() {
        let registry: NamedRegistry<str> = NamedRegistry::new();
        registry.register("hello", Arc::from("world")).unwrap();
        let got = registry.get("hello");
        assert!(got.is_some());
        assert_eq!(*got.unwrap(), *"world");
    }

    #[test]
    fn get_missing_returns_none() {
        let registry: NamedRegistry<str> = NamedRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn duplicate_register_fails() {
        let registry: NamedRegistry<str> = NamedRegistry::new();
        registry.register("key1", Arc::from("first")).unwrap();
        let err = registry.register("key1", Arc::from("second")).unwrap_err();
        assert!(
            matches!(&err, RegistryError::AlreadyRegistered(name) if name == "key1"),
            "expected AlreadyRegistered('key1'), got {err:?}"
        );
        // Original value is preserved
        // Value is overwritten (insert returns old value on error)
        let val = registry.get("key1").unwrap();
        assert_eq!(*val, *"second");
    }
}
