use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;

/// Default maximum number of `StateStore` entries per producer.
const DEFAULT_MAX_ENTRIES: usize = crate::config::DEFAULT_MAX_KV_ENTRIES;

/// Default maximum byte length of a `StateStore` key.
const DEFAULT_MAX_KEY_BYTES: usize = crate::config::DEFAULT_MAX_KEY_BYTES;

/// Default maximum byte length of a `StateStore` value (64 KiB).
const DEFAULT_MAX_VALUE_BYTES: usize = crate::config::DEFAULT_MAX_VALUE_BYTES;

/// Persistent key-value store scoped to a WASM producer (per route endpoint).
///
/// Each route endpoint using a WASM component gets its own independent state store.
/// If two routes use the same `.wasm` file, they maintain separate state.
/// Owned by `WasmProducer` and passed to `WasmRuntime` when creating host state.
///
/// ADR-0051 credential boundary: manual-redaction
#[derive(Clone)]
pub struct StateStore {
    data: Arc<Mutex<HashMap<String, String>>>,
    max_entries: usize,
    max_key_bytes: usize,
    max_value_bytes: usize,
}

impl fmt::Debug for StateStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateStore")
            .field("data", &"[REDACTED]")
            .finish()
    }
}

impl StateStore {
    pub fn new() -> Self {
        Self::with_limits(
            DEFAULT_MAX_ENTRIES,
            DEFAULT_MAX_KEY_BYTES,
            DEFAULT_MAX_VALUE_BYTES,
        )
    }

    /// Build a `StateStore` with explicit bounds on entry count, key length, and
    /// value length. `store` rejects writes that exceed any of these limits.
    pub fn with_limits(max_entries: usize, max_key_bytes: usize, max_value_bytes: usize) -> Self {
        Self {
            data: Arc::new(Mutex::new(HashMap::new())),
            max_entries,
            max_key_bytes,
            max_value_bytes,
        }
    }

    pub fn store(&self, key: &str, value: &str) -> Result<(), String> {
        if key.len() > self.max_key_bytes {
            return Err(format!(
                "key exceeds max_key_bytes limit ({})",
                self.max_key_bytes
            ));
        }
        if value.len() > self.max_value_bytes {
            return Err(format!(
                "value exceeds max_value_bytes limit ({})",
                self.max_value_bytes
            ));
        }
        let mut guard = self
            .data
            .lock()
            .map_err(|e| format!("lock poisoned: {}", e))?;
        if !guard.contains_key(key) && guard.len() >= self.max_entries {
            return Err(format!("kv entry limit exceeded ({})", self.max_entries));
        }
        guard.insert(key.to_string(), value.to_string());
        Ok(())
    }

    pub fn load(&self, key: &str) -> Result<Option<String>, String> {
        let guard = self
            .data
            .lock()
            .map_err(|e| format!("lock poisoned: {}", e))?;
        Ok(guard.get(key).cloned())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        self.data.lock().map(|g| g.len()).unwrap_or(0)
    }

    pub(crate) fn max_key_bytes(&self) -> usize {
        self.max_key_bytes
    }

    pub(crate) fn max_value_bytes(&self) -> usize {
        self.max_value_bytes
    }
}

impl Default for StateStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_secrets() {
        let store = StateStore::new();
        store.store("api-key", "SENTINEL-GUEST-SECRET").unwrap();
        let debug_output = format!("{:?}", store);
        assert!(
            !debug_output.contains("SENTINEL-GUEST-SECRET"),
            "Debug output must not contain secret values: {}",
            debug_output
        );
    }

    #[test]
    fn test_store_rejects_oversized_key() {
        let store = StateStore::with_limits(256, 10, 65536);
        // 22 ASCII chars > 10-byte max_key_bytes cap.
        let oversized = "a-key-much-longer-than-ten";
        let err = store.store(oversized, "v").unwrap_err();
        assert!(
            err.contains("max_key_bytes"),
            "expected error to mention max_key_bytes, got: {err}"
        );
    }

    #[test]
    fn test_store_rejects_oversized_value() {
        let store = StateStore::with_limits(256, 1024, 10);
        // 30 ASCII chars > 10-byte max_value_bytes cap.
        let oversized = "this value is far too long!!";
        let err = store.store("k", oversized).unwrap_err();
        assert!(
            err.contains("max_value_bytes"),
            "expected error to mention max_value_bytes, got: {err}"
        );
    }

    #[test]
    fn test_store_rejects_entry_count_overflow() {
        let store = StateStore::with_limits(2, 1024, 65536);
        store.store("k1", "v1").unwrap();
        store.store("k2", "v2").unwrap();
        let err = store.store("k3", "v3").unwrap_err();
        assert!(
            err.contains("kv entry limit"),
            "expected error to mention kv entry limit, got: {err}"
        );
    }

    #[test]
    fn test_store_allows_update_within_bounds() {
        let store = StateStore::with_limits(2, 1024, 65536);
        store.store("k1", "v1").unwrap();
        store.store("k2", "v2").unwrap();
        // Updating an existing key must not count against max_entries.
        store.store("k1", "v1-updated").unwrap();
        let loaded = store.load("k1").unwrap();
        assert_eq!(loaded.as_deref(), Some("v1-updated"));
        assert_eq!(store.len(), 2);
    }
}
