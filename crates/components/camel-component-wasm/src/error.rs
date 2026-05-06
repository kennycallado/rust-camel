//! WASM component error types.
//!
//! Provides structured error classification for WASM execution failures:
//! - Timeout (epoch deadline expired)
//! - Trap (guest panic, stack overflow, unreachable, etc.)
//! - Out of memory (linear memory exceeded limit)
//! - Unhealthy (consecutive failures, unable to re-instantiate)
//!
//! All variants carry context (plugin name, timeout value, etc.) for debugging
//! and error handler integration (dead letter channel, retry policies).

use camel_api::CamelError;

/// Classification of a WASM trap reason.
///
/// Used to provide structured error information to error handlers
/// instead of a flat string message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrapReason {
    /// Execution exceeded the configured timeout.
    Timeout,
    /// Linear memory exceeded the configured limit.
    OutOfMemory,
    /// `unreachable` instruction executed (guest panic).
    Unreachable,
    /// Call stack exceeded limit.
    StackOverflow,
    /// Other trap with description.
    Other(String),
}

impl std::fmt::Display for TrapReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "execution timeout"),
            Self::OutOfMemory => write!(f, "out of memory"),
            Self::Unreachable => write!(f, "unreachable instruction"),
            Self::StackOverflow => write!(f, "stack overflow"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

/// Errors that can occur during WASM plugin execution.
#[derive(Debug, thiserror::Error)]
pub enum WasmError {
    #[error("WASM module not found: {0}")]
    ModuleNotFound(String),

    #[error("WASM compilation failed: {0}")]
    CompilationFailed(String),

    #[error("WASM instantiation failed: {0}")]
    InstantiationFailed(String),

    #[error("WASM guest panicked (trap): {0}")]
    GuestPanic(String),

    #[error("WASM type conversion failed: {0}")]
    TypeConversion(String),

    #[error("WASM I/O error: {0}")]
    Io(String),

    #[error("WASM configuration error: {0}")]
    Config(String),

    // ── Phase 4: Structured error variants ──────────────────────────────
    #[error("WASM plugin '{plugin}' timed out after {timeout_secs}s")]
    Timeout { plugin: String, timeout_secs: u64 },

    #[error("WASM plugin '{plugin}' trapped: {reason}")]
    Trap { plugin: String, reason: TrapReason },

    #[error("WASM plugin '{plugin}' exceeded memory limit ({max_memory_bytes} bytes)")]
    OutOfMemory {
        plugin: String,
        max_memory_bytes: u64,
    },

    #[error("WASM plugin '{plugin}' is unhealthy: {detail}")]
    Unhealthy { plugin: String, detail: String },
}

impl WasmError {
    /// Classify a wasmtime `Trap` into a `TrapReason`.
    ///
    /// Uses the trap type for well-known cases and falls back to
    /// message matching for epoch-related traps.
    ///
    /// **Note:** `wasmtime::Trap` is `#[non_exhaustive]` — the `other` catch-all
    /// arm is **required** and handles any future trap variants added by wasmtime.
    pub fn classify_trap(trap: &wasmtime::Trap) -> TrapReason {
        match trap {
            wasmtime::Trap::StackOverflow => TrapReason::StackOverflow,
            wasmtime::Trap::MemoryOutOfBounds => TrapReason::OutOfMemory,
            wasmtime::Trap::UnreachableCodeReached => TrapReason::Unreachable,
            wasmtime::Trap::Interrupt => TrapReason::Timeout,
            // #[non_exhaustive] catch-all
            other => {
                let msg = other.to_string();
                if msg.contains("epoch") {
                    TrapReason::Timeout
                } else {
                    TrapReason::Other(msg)
                }
            }
        }
    }

    /// Returns the plugin name associated with this error, if any.
    pub fn plugin_name(&self) -> Option<&str> {
        match self {
            Self::Timeout { plugin, .. } => Some(plugin),
            Self::Trap { plugin, .. } => Some(plugin),
            Self::OutOfMemory { plugin, .. } => Some(plugin),
            Self::Unhealthy { plugin, .. } => Some(plugin),
            _ => None,
        }
    }
}

impl From<WasmError> for CamelError {
    fn from(err: WasmError) -> Self {
        match &err {
            WasmError::GuestPanic(msg) => CamelError::ProcessorError(msg.clone()),
            WasmError::TypeConversion(msg) => CamelError::TypeConversionFailed(msg.clone()),
            WasmError::ModuleNotFound(msg) => CamelError::ComponentNotFound(msg.clone()),
            WasmError::CompilationFailed(msg) => CamelError::EndpointCreationFailed(msg.clone()),
            WasmError::InstantiationFailed(msg) => CamelError::EndpointCreationFailed(msg.clone()),
            WasmError::Io(msg) => CamelError::Io(msg.clone()),
            WasmError::Config(msg) => CamelError::Config(msg.clone()),
            // ── Phase 4 structured variants ──
            WasmError::Timeout {
                plugin,
                timeout_secs,
            } => CamelError::ProcessorError(format!(
                "WASM plugin '{}' timed out after {}s",
                plugin, timeout_secs
            )),
            WasmError::Trap { plugin, reason } => {
                CamelError::ProcessorError(format!("WASM plugin '{}' trapped: {}", plugin, reason))
            }
            WasmError::OutOfMemory {
                plugin,
                max_memory_bytes,
            } => CamelError::ProcessorError(format!(
                "WASM plugin '{}' exceeded memory limit ({} bytes)",
                plugin, max_memory_bytes
            )),
            WasmError::Unhealthy { plugin, detail } => CamelError::ProcessorError(format!(
                "WASM plugin '{}' is unhealthy: {}",
                plugin, detail
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trap_reason_display() {
        assert_eq!(TrapReason::Timeout.to_string(), "execution timeout");
        assert_eq!(TrapReason::OutOfMemory.to_string(), "out of memory");
        assert_eq!(
            TrapReason::Unreachable.to_string(),
            "unreachable instruction"
        );
        assert_eq!(TrapReason::StackOverflow.to_string(), "stack overflow");
        assert_eq!(
            TrapReason::Other("custom".to_string()).to_string(),
            "custom"
        );
    }

    #[test]
    fn test_classify_trap_stack_overflow() {
        let trap = wasmtime::Trap::StackOverflow;
        assert!(matches!(
            WasmError::classify_trap(&trap),
            TrapReason::StackOverflow
        ));
    }

    #[test]
    fn test_classify_trap_memory_out_of_bounds() {
        let trap = wasmtime::Trap::MemoryOutOfBounds;
        assert!(matches!(
            WasmError::classify_trap(&trap),
            TrapReason::OutOfMemory
        ));
    }

    #[test]
    fn test_classify_trap_unreachable() {
        let trap = wasmtime::Trap::UnreachableCodeReached;
        assert!(matches!(
            WasmError::classify_trap(&trap),
            TrapReason::Unreachable
        ));
    }

    #[test]
    fn test_wasm_error_timeout_display() {
        let err = WasmError::Timeout {
            plugin: "my_plugin".to_string(),
            timeout_secs: 30,
        };
        let msg = err.to_string();
        assert!(msg.contains("my_plugin"));
        assert!(msg.contains("30"));
        assert!(msg.contains("timed out"));
    }

    #[test]
    fn test_wasm_error_trap_display() {
        let err = WasmError::Trap {
            plugin: "my_plugin".to_string(),
            reason: TrapReason::StackOverflow,
        };
        let msg = err.to_string();
        assert!(msg.contains("my_plugin"));
        assert!(msg.contains("stack overflow"));
    }

    #[test]
    fn test_wasm_error_out_of_memory_display() {
        let err = WasmError::OutOfMemory {
            plugin: "my_plugin".to_string(),
            max_memory_bytes: 52428800,
        };
        let msg = err.to_string();
        assert!(msg.contains("my_plugin"));
        assert!(msg.contains("52428800"));
    }

    #[test]
    fn test_wasm_error_unhealthy_display() {
        let err = WasmError::Unhealthy {
            plugin: "my_plugin".to_string(),
            detail: "consecutive failures".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("my_plugin"));
        assert!(msg.contains("consecutive failures"));
    }

    #[test]
    fn test_wasm_error_to_camel_error_timeout() {
        let err = WasmError::Timeout {
            plugin: "p".to_string(),
            timeout_secs: 10,
        };
        let camel: CamelError = err.into();
        let msg = camel.to_string();
        assert!(msg.contains("timed out"));
        assert!(msg.contains("10"));
    }

    #[test]
    fn test_wasm_error_to_camel_error_trap() {
        let err = WasmError::Trap {
            plugin: "p".to_string(),
            reason: TrapReason::Unreachable,
        };
        let camel: CamelError = err.into();
        let msg = camel.to_string();
        assert!(msg.contains("unreachable"));
    }

    #[test]
    fn test_wasm_error_to_camel_error_out_of_memory() {
        let err = WasmError::OutOfMemory {
            plugin: "p".to_string(),
            max_memory_bytes: 1024,
        };
        let camel: CamelError = err.into();
        let msg = camel.to_string();
        assert!(msg.contains("memory"));
    }

    #[test]
    fn test_wasm_error_to_camel_error_unhealthy() {
        let err = WasmError::Unhealthy {
            plugin: "p".to_string(),
            detail: "broken".to_string(),
        };
        let camel: CamelError = err.into();
        assert!(matches!(camel, CamelError::ProcessorError(_)));
    }

    #[test]
    fn test_guest_panic_still_maps_to_processor_error() {
        let err = WasmError::GuestPanic("boom".to_string());
        let camel: CamelError = err.into();
        assert!(matches!(camel, CamelError::ProcessorError(_)));
    }

    #[test]
    fn test_plugin_name() {
        let err = WasmError::Timeout {
            plugin: "test".to_string(),
            timeout_secs: 5,
        };
        assert_eq!(err.plugin_name(), Some("test"));

        let err = WasmError::GuestPanic("msg".to_string());
        assert_eq!(err.plugin_name(), None);
    }
}
