//! WASM plugin configuration for Processor URI query params and `Camel.toml`
//! limits blocks used by Bean/AuthorizationPolicy/SecurityPolicy.
//!
//! `max_memory_bytes` is enforced at runtime via
//! `wasmtime::StoreLimitsBuilder::memory_size` in `WasmRuntime::create_host_state`.
//! The default (50 MiB) is intentionally tight; raise it through `Camel.toml`
//! (`[default.beans.<name>.limits]` or `[permissions.providers.<name>.limits]`)
//! or via the `wasm:` URI query string (`?max-memory=N`) for Processor plugins.
//! Timeout uses epoch interruption.

use std::path::Path;
use std::time::Duration;

use camel_api::body::DEFAULT_MATERIALIZE_LIMIT;

/// Default execution timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Default maximum linear memory in bytes (50 MB).
const DEFAULT_MAX_MEMORY_BYTES: u64 = 50 * 1024 * 1024;

/// Default maximum concurrent `call_process` executions per producer.
const DEFAULT_MAX_CONCURRENT_CALLS: usize = 4;

/// Default maximum .wasm file size in bytes (10 MB).
const DEFAULT_MAX_WASM_SIZE_BYTES: u64 = 10 * 1024 * 1024;

/// Default maximum bytes for the streaming body bridge.
pub(crate) const DEFAULT_MAX_STREAM_BYTES: u64 = DEFAULT_MATERIALIZE_LIMIT as u64;

/// Epoch tick interval in milliseconds (same as Surrealism).
const EPOCH_INTERVAL_MILLIS: u64 = 10;

/// Configuration for a WASM plugin instance.
///
/// Parsed from URI query parameters or Camel.toml.
/// Example URI: `wasm:plugin.wasm?timeout=10&max-memory=52428800`
#[derive(Debug, Clone)]
pub struct WasmConfig {
    /// Maximum execution time per guest call, in seconds.
    pub timeout_secs: u64,

    /// Maximum linear memory the guest can allocate, in bytes.
    /// Enforced via `wasmtime::StoreLimitsBuilder::memory_size`.
    pub max_memory_bytes: u64,

    /// Maximum concurrent `call_process` executions per producer.
    pub max_concurrent_calls: usize,

    /// Maximum .wasm file size in bytes. Files exceeding this are rejected
    /// before compilation to prevent DoS via pathologically large modules.
    /// Default: 10 MB.
    pub max_wasm_size_bytes: u64,

    /// Comma-separated URI schemes the guest may call via camel_call/camel_poll.
    /// Empty string = deny all (fail-closed). Example: "log,direct,file".
    /// Ignored for AuthorizationPolicy/SecurityPolicy worlds (always denied).
    pub allow_call_schemes: String,

    /// Maximum bytes for the streaming body bridge.
    pub max_stream_bytes: u64,
}

impl Default for WasmConfig {
    fn default() -> Self {
        Self {
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            max_memory_bytes: DEFAULT_MAX_MEMORY_BYTES,
            max_concurrent_calls: DEFAULT_MAX_CONCURRENT_CALLS,
            max_wasm_size_bytes: DEFAULT_MAX_WASM_SIZE_BYTES,
            allow_call_schemes: String::new(),
            max_stream_bytes: DEFAULT_MAX_STREAM_BYTES,
        }
    }
}

impl WasmConfig {
    /// Build concrete runtime config from optional `Camel.toml` WASM limits.
    ///
    /// `None` values use runtime defaults matching `WasmConfig::default()`.
    /// This constructor is the single source of truth for `WasmConfig` defaults
    /// sourced from `Camel.toml` — no silent fallback lie elsewhere (ADR-0011).
    pub fn from_limits(limits: &camel_config::WasmLimitsConfig) -> WasmConfig {
        WasmConfig {
            timeout_secs: limits.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
            max_memory_bytes: limits.max_memory.unwrap_or(DEFAULT_MAX_MEMORY_BYTES),
            max_concurrent_calls: limits
                .max_concurrent_calls
                .unwrap_or(DEFAULT_MAX_CONCURRENT_CALLS),
            max_wasm_size_bytes: limits.max_wasm_size.unwrap_or(DEFAULT_MAX_WASM_SIZE_BYTES),
            allow_call_schemes: limits.allow_call_schemes.clone().unwrap_or_default(),
            max_stream_bytes: limits.max_stream_bytes.unwrap_or(DEFAULT_MAX_STREAM_BYTES),
        }
    }

    /// Parse `WasmConfig` from the query portion of a WASM URI.
    ///
    /// `uri_without_scheme` is everything after `wasm:`, e.g.
    /// `plugins/my_processor.wasm?timeout=10&max-memory=52428800`.
    ///
    /// Returns `(path, config)` where path has no query string.
    pub fn from_uri(uri_without_scheme: &str) -> (String, WasmConfig) {
        let (path, query) = match uri_without_scheme.find('?') {
            Some(i) => (&uri_without_scheme[..i], Some(&uri_without_scheme[i + 1..])),
            None => (uri_without_scheme, None),
        };

        let mut config = WasmConfig::default();

        if let Some(q) = query {
            for pair in q.split('&') {
                if let Some((key, value)) = pair.split_once('=') {
                    match key {
                        "timeout" => {
                            if let Ok(secs) = value.parse::<u64>()
                                && secs > 0
                            {
                                config.timeout_secs = secs;
                            }
                        }
                        "max-memory" => {
                            if let Ok(bytes) = value.parse::<u64>()
                                && bytes > 0
                            {
                                config.max_memory_bytes = bytes;
                            }
                        }
                        "max-concurrent-calls" => {
                            if let Ok(max) = value.parse::<usize>()
                                && max > 0
                            {
                                config.max_concurrent_calls = max;
                            }
                        }
                        "max-wasm-size" => {
                            if let Ok(bytes) = value.parse::<u64>()
                                && bytes > 0
                            {
                                config.max_wasm_size_bytes = bytes;
                            }
                        }
                        "allow-call" => {
                            config.allow_call_schemes = value.to_string();
                        }
                        "max-stream-bytes" => {
                            if let Ok(bytes) = value.parse::<u64>()
                                && bytes > 0
                            {
                                config.max_stream_bytes = bytes;
                            }
                        }
                        _ => {} // ignore unknown params
                    }
                }
            }
        }

        (path.to_string(), config)
    }

    /// Convert the wall-clock timeout to an epoch deadline (number of ticks).
    ///
    /// At 10ms per tick: deadline = timeout_secs * 100
    pub fn epoch_deadline(&self) -> u64 {
        self.timeout_secs * (1000 / EPOCH_INTERVAL_MILLIS)
    }

    /// The interval at which the epoch ticker thread increments the epoch.
    pub fn epoch_interval(&self) -> Duration {
        Duration::from_millis(EPOCH_INTERVAL_MILLIS)
    }

    /// Returns the configured epoch interval in milliseconds.
    pub fn epoch_interval_millis(&self) -> u64 {
        EPOCH_INTERVAL_MILLIS
    }

    pub fn classify_error(
        &self,
        plugin_path: &Path,
        e: wasmtime::Error,
    ) -> crate::error::WasmError {
        classify_error(self, plugin_path, e)
    }
}

/// Hoisted free-function form of [`WasmConfig::classify_error`] so spawned
/// tasks (which cannot borrow `&self`) can classify wasmtime errors.
///
/// Captures `config` + `plugin_path` by value/clone at spawn site; the
/// wasmtime error is consumed.
pub fn classify_error(
    config: &WasmConfig,
    plugin_path: &Path,
    e: wasmtime::Error,
) -> crate::error::WasmError {
    use crate::error::{TrapReason, WasmError};
    let name = plugin_path.display().to_string();
    if let Some(trap) = e.downcast_ref::<wasmtime::Trap>() {
        match WasmError::classify_trap(trap) {
            TrapReason::Timeout => WasmError::Timeout {
                plugin: name,
                timeout_secs: config.timeout_secs,
            },
            TrapReason::OutOfMemory => WasmError::OutOfMemory {
                plugin: name,
                max_memory_bytes: config.max_memory_bytes,
            },
            other => WasmError::Trap {
                plugin: name,
                reason: other,
            },
        }
    } else {
        WasmError::GuestPanic(e.to_string())
    }
}

pub fn validate_wasm_size(path: &std::path::Path, max_bytes: u64) -> Result<(), String> {
    let metadata = std::fs::metadata(path)
        .map_err(|e| format!("cannot stat wasm module {}: {}", path.display(), e))?;
    let size = metadata.len();
    if size > max_bytes {
        return Err(format!(
            "wasm module {} is {} bytes ({} KiB), exceeds cap of {} bytes ({} KiB)",
            path.display(),
            size,
            size / 1024,
            max_bytes,
            max_bytes / 1024,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = WasmConfig::default();
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.max_memory_bytes, 50 * 1024 * 1024);
        assert_eq!(config.max_concurrent_calls, 4);
    }

    #[test]
    fn test_from_uri_no_params() {
        let (path, config) = WasmConfig::from_uri("plugins/test.wasm");
        assert_eq!(path, "plugins/test.wasm");
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.max_memory_bytes, 50 * 1024 * 1024);
        assert_eq!(config.max_concurrent_calls, 4);
    }

    #[test]
    fn test_from_uri_with_timeout() {
        let (path, config) = WasmConfig::from_uri("plugins/test.wasm?timeout=10");
        assert_eq!(path, "plugins/test.wasm");
        assert_eq!(config.timeout_secs, 10);
        assert_eq!(config.max_memory_bytes, 50 * 1024 * 1024);
        assert_eq!(config.max_concurrent_calls, 4);
    }

    #[test]
    fn test_from_uri_with_max_memory() {
        let (path, config) = WasmConfig::from_uri("plugins/test.wasm?max-memory=10485760");
        assert_eq!(path, "plugins/test.wasm");
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.max_memory_bytes, 10_485_760);
        assert_eq!(config.max_concurrent_calls, 4);
    }

    #[test]
    fn test_from_uri_with_both_params() {
        let (path, config) = WasmConfig::from_uri("plugins/test.wasm?timeout=5&max-memory=1048576");
        assert_eq!(path, "plugins/test.wasm");
        assert_eq!(config.timeout_secs, 5);
        assert_eq!(config.max_memory_bytes, 1_048_576);
        assert_eq!(config.max_concurrent_calls, 4);
    }

    #[test]
    fn test_from_uri_with_max_concurrent_calls() {
        let (path, config) = WasmConfig::from_uri("plugins/test.wasm?max-concurrent-calls=8");
        assert_eq!(path, "plugins/test.wasm");
        assert_eq!(config.max_concurrent_calls, 8);
    }

    #[test]
    fn test_from_uri_ignores_unknown_params() {
        let (path, config) = WasmConfig::from_uri("plugins/test.wasm?foo=bar&timeout=60");
        assert_eq!(path, "plugins/test.wasm");
        assert_eq!(config.timeout_secs, 60);
    }

    #[test]
    fn test_from_uri_ignores_invalid_values() {
        let (_path, config) = WasmConfig::from_uri("plugins/test.wasm?timeout=abc");
        assert_eq!(config.timeout_secs, 30); // stays default
    }

    #[test]
    fn test_from_uri_ignores_zero_values() {
        let (_path, config) = WasmConfig::from_uri("plugins/test.wasm?timeout=0&max-memory=0");
        assert_eq!(config.timeout_secs, 30); // stays default
        assert_eq!(config.max_memory_bytes, 50 * 1024 * 1024); // stays default
        assert_eq!(config.max_concurrent_calls, 4);
    }

    #[test]
    fn test_epoch_deadline() {
        let config = WasmConfig {
            timeout_secs: 30,
            max_memory_bytes: 0,
            max_concurrent_calls: 4,
            ..WasmConfig::default()
        };
        assert_eq!(config.epoch_deadline(), 3000); // 30s * 100 ticks/s
    }

    #[test]
    fn test_epoch_deadline_custom_timeout() {
        let config = WasmConfig {
            timeout_secs: 5,
            max_memory_bytes: 0,
            max_concurrent_calls: 4,
            ..WasmConfig::default()
        };
        assert_eq!(config.epoch_deadline(), 500);
    }

    #[test]
    fn test_epoch_interval() {
        let config = WasmConfig::default();
        assert_eq!(config.epoch_interval(), Duration::from_millis(10));
    }

    #[test]
    fn from_limits_applies_provided_values() {
        let limits = camel_config::WasmLimitsConfig {
            timeout_secs: Some(90),
            max_memory: Some(128 * 1024 * 1024),
            max_concurrent_calls: Some(2),
            ..camel_config::WasmLimitsConfig::default()
        };

        let config = WasmConfig::from_limits(&limits);

        assert_eq!(config.timeout_secs, 90);
        assert_eq!(config.max_memory_bytes, 128 * 1024 * 1024);
        assert_eq!(config.max_concurrent_calls, 2);
    }

    #[test]
    fn from_limits_falls_back_to_runtime_defaults_when_none() {
        let limits = camel_config::WasmLimitsConfig::default();

        let config = WasmConfig::from_limits(&limits);

        assert_eq!(config.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert_eq!(config.max_memory_bytes, DEFAULT_MAX_MEMORY_BYTES);
        assert_eq!(config.max_concurrent_calls, 4);
    }

    #[test]
    fn from_limits_mixed_some_and_none() {
        let limits = camel_config::WasmLimitsConfig {
            timeout_secs: Some(15),
            max_memory: None,
            max_concurrent_calls: Some(1),
            ..camel_config::WasmLimitsConfig::default()
        };

        let config = WasmConfig::from_limits(&limits);

        assert_eq!(config.timeout_secs, 15);
        assert_eq!(config.max_memory_bytes, DEFAULT_MAX_MEMORY_BYTES);
        assert_eq!(config.max_concurrent_calls, 1);
    }

    #[test]
    fn test_validate_wasm_size_rejects_oversized() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.wasm");
        std::fs::write(&path, vec![0u8; 100]).unwrap();
        let err = validate_wasm_size(&path, 50).unwrap_err();
        assert!(err.contains("exceeds cap"), "got: {err}");
    }

    #[test]
    fn test_validate_wasm_size_allows_within_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ok.wasm");
        std::fs::write(&path, vec![0u8; 100]).unwrap();
        validate_wasm_size(&path, 200).expect("100 bytes within 200 cap");
    }

    #[test]
    fn test_default_max_wasm_size_bytes() {
        let config = WasmConfig::default();
        assert_eq!(config.max_wasm_size_bytes, 10 * 1024 * 1024);
    }

    #[test]
    fn test_from_uri_max_wasm_size() {
        let (_path, config) = WasmConfig::from_uri("p.wasm?max-wasm-size=1048576");
        assert_eq!(config.max_wasm_size_bytes, 1_048_576);
    }

    #[test]
    fn test_validate_wasm_size_errors_on_missing_file() {
        let err = validate_wasm_size(std::path::Path::new("/nonexistent.wasm"), 1000).unwrap_err();
        assert!(err.contains("cannot stat"));
    }

    #[test]
    fn wasm_config_default_max_stream_bytes() {
        let cfg = WasmConfig::default();
        assert_eq!(cfg.max_stream_bytes, DEFAULT_MATERIALIZE_LIMIT as u64);
    }

    #[test]
    fn wasm_config_from_uri_max_stream_bytes() {
        let (_, cfg) = WasmConfig::from_uri("test.wasm?max-stream-bytes=52428800");
        assert_eq!(cfg.max_stream_bytes, 52_428_800);
    }

    #[test]
    fn wasm_config_from_uri_max_stream_bytes_ignores_zero() {
        let (_, cfg) = WasmConfig::from_uri("test.wasm?max-stream-bytes=0");
        assert_eq!(cfg.max_stream_bytes, DEFAULT_MAX_STREAM_BYTES);
    }

    #[test]
    fn wasm_config_from_limits_max_stream_bytes() {
        let limits = camel_config::WasmLimitsConfig {
            max_stream_bytes: Some(52_428_800),
            ..camel_config::WasmLimitsConfig::default()
        };
        let cfg = WasmConfig::from_limits(&limits);
        assert_eq!(cfg.max_stream_bytes, 52_428_800);
    }

    #[test]
    fn wasm_config_from_limits_max_stream_bytes_default() {
        let limits = camel_config::WasmLimitsConfig::default();
        let cfg = WasmConfig::from_limits(&limits);
        assert_eq!(cfg.max_stream_bytes, DEFAULT_MAX_STREAM_BYTES);
    }
}
