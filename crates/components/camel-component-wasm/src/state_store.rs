use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;

/// Persistent key-value store scoped to a WASM producer (per route endpoint).
///
/// Each route endpoint using a WASM component gets its own independent state store.
/// If two routes use the same `.wasm` file, they maintain separate state.
/// Owned by `WasmProducer` and passed to `WasmRuntime` when creating host state.
#[derive(Clone)]
pub struct StateStore {
    data: Arc<Mutex<HashMap<String, String>>>,
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
        Self {
            data: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn store(&self, key: &str, value: &str) -> Result<(), String> {
        let mut guard = self
            .data
            .lock()
            .map_err(|e| format!("lock poisoned: {}", e))?;
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
}
