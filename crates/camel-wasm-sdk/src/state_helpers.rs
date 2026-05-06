//! Convenience helpers for persistent plugin state.
//!
//! Plugins can use [`store`] and [`load`] to persist key-value data
//! across `process()` calls. State is scoped per route endpoint —
//! different routes using the same WASM component get independent state stores.

/// Store a string value that persists across process() calls.
pub fn store(key: &str, value: &str) -> Result<(), String> {
    crate::bindings::camel::plugin::host::host_store(key, value)
        .map_err(|e| format!("host_store failed: {:?}", e))
}

/// Load a previously stored string value.
pub fn load(key: &str) -> Result<Option<String>, String> {
    crate::bindings::camel::plugin::host::host_load(key)
        .map_err(|e| format!("host_load failed: {:?}", e))
}

/// Store a serializable value as JSON.
pub fn store_json<T: serde::Serialize>(key: &str, value: &T) -> Result<(), String> {
    let json = serde_json::to_string(value).map_err(|e| e.to_string())?;
    store(key, &json)
}

/// Load and deserialize a JSON value.
pub fn load_json<T: serde::de::DeserializeOwned>(key: &str) -> Result<Option<T>, String> {
    match load(key)? {
        Some(json) => Ok(Some(
            serde_json::from_str(&json).map_err(|e| e.to_string())?,
        )),
        None => Ok(None),
    }
}
