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
// TODO(WASM-005): WasmError may conflict with WIT-generated types in bindings.
// If WIT adds an error resource named WasmError, rename this to CamelWasmError.
#[derive(Debug, Clone, thiserror::Error)]
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
            WasmError::GuestPanic(msg) => CamelError::ProcessorError(format!("wasm trap: {msg}")),
            WasmError::TypeConversion(msg) => CamelError::TypeConversionFailed(msg.clone()),
            WasmError::ModuleNotFound(msg) => CamelError::ComponentNotFound(msg.clone()),
            WasmError::CompilationFailed(msg) => {
                CamelError::Config(format!("wasm compilation failed: {msg}"))
            }
            WasmError::InstantiationFailed(msg) => {
                CamelError::Config(format!("wasm instantiation failed: {msg}"))
            }
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
                CamelError::ProcessorError(format!("wasm trap: plugin '{}': {}", plugin, reason))
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

impl From<crate::bindings::camel::plugin::types::WasmError> for CamelError {
    fn from(err: crate::bindings::camel::plugin::types::WasmError) -> Self {
        map_plugin_error(err).into()
    }
}

impl From<crate::bean_bindings::camel::plugin::types::WasmError> for CamelError {
    fn from(err: crate::bean_bindings::camel::plugin::types::WasmError) -> Self {
        map_bean_error(err).into()
    }
}

/// Map a WIT bindings [`plugin::WasmError`] to the canonical crate-level [`WasmError`].
///
/// Both the `call_process` and `process_streaming_exchange` paths encounter
/// the WIT-level error type after [`peel_concurrent`] strips the wasmtime
/// error layers. This remaps the four WIT variants to the crate's own
/// error variants, collapsing `Timeout` into `GuestPanic` (the guest timed
/// out — not a host-level epoch deadline).
pub fn map_plugin_error(wasm_err: crate::bindings::camel::plugin::types::WasmError) -> WasmError {
    use crate::bindings::camel::plugin::types::WasmError as PluginWasmError;
    match wasm_err {
        PluginWasmError::ProcessorError(s) => WasmError::GuestPanic(s),
        PluginWasmError::TypeConversion(s) => WasmError::TypeConversion(s),
        PluginWasmError::Io(s) => WasmError::Io(s),
        PluginWasmError::Timeout => WasmError::GuestPanic("guest timeout".into()),
    }
}

/// Map a bean-world WIT `WasmError` to the canonical crate-level [`WasmError`].
///
/// Mirrors [`map_plugin_error`] for the bean binding namespace.
pub fn map_bean_error(
    wasm_err: crate::bean_bindings::camel::plugin::types::WasmError,
) -> WasmError {
    use crate::bean_bindings::camel::plugin::types::WasmError as BeanWasmError;
    match wasm_err {
        BeanWasmError::ProcessorError(s) => WasmError::GuestPanic(s),
        BeanWasmError::TypeConversion(s) => WasmError::TypeConversion(s),
        BeanWasmError::Io(s) => WasmError::Io(s),
        BeanWasmError::Timeout => WasmError::GuestPanic("guest timeout".into()),
    }
}

/// `?` ergonomics for the bindgen trappable layer: when a generated
/// `call_*(...)` returns `Result<_, wasmtime::Error>` (the trappable wrapper
/// around a WIT `result<_, _>`), this impl lets `?` convert the trap to
/// the canonical `WasmError::GuestPanic` (further classified later via
/// `classify_error` if more precision is needed).
impl From<wasmtime::Error> for WasmError {
    fn from(err: wasmtime::Error) -> Self {
        WasmError::GuestPanic(err.to_string())
    }
}

/// Peel the layers of a `run_concurrent` result.
///
/// The bindgen-generated `async | store` shape with a WIT `result<_, _>`
/// produces a 2-layer result from `Store::run_concurrent`:
///
/// ```text
///     Result<Result<T, Inner>, wasmtime::Error>
///      ─────────────────┬─────────────────────  outer wasmtime::Error:
///      │              │                       infrastructure trap, epoch
///      │              │                       interrupt, store access
///      │              │                       failure
///      │              └────────────────────  inner `Inner`:
///      │                                      WIT result, guest trap,
///      │                                      or domain error
///      └─────────────────────  success: T
/// ```
///
/// Both layers carry errors that the caller wants to collapse into a
/// single target error type (`E`). The bindgen shapes vary across
/// per-world modules (plugin, bean, security-policy) and the inner
/// type varies too (String, plugin::WasmError, bean::WasmError,
/// security_policy::WasmError), so the helper takes two closures —
/// one for each layer — and is fully generic over `T`, `Inner`, and `E`.
///
/// This replaces the copy-pasted 6+ `match outer { ... }` / `match
/// middle { ... }` chain at the call sites in `runtime.rs`,
/// `wasm_plugin_context.rs`, `bean.rs`, `authorization_policy.rs`, and
/// `security_policy.rs`.
pub fn peel_concurrent<T, Inner, E>(
    result: Result<Result<T, Inner>, wasmtime::Error>,
    map_outer: impl FnOnce(wasmtime::Error) -> E,
    map_inner: impl FnOnce(Inner) -> E,
) -> Result<T, E> {
    match result {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(inner)) => Err(map_inner(inner)),
        Err(outer) => Err(map_outer(outer)),
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
        assert!(camel.to_string().contains("wasm trap"));
    }

    #[test]
    fn test_instantiation_maps_to_config_error() {
        let err = WasmError::InstantiationFailed("bad import".to_string());
        let camel: CamelError = err.into();
        assert!(matches!(camel, CamelError::Config(_)));
        assert!(camel.to_string().contains("instantiation failed"));
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

    // ── peel_concurrent helper (I1) ──────────────────────────────────────

    #[test]
    fn peel_concurrent_ok_outer_ok_inner_returns_t() {
        let r: Result<Result<i32, String>, wasmtime::Error> = Ok(Ok(42));
        let result = peel_concurrent(r, |_| 0, |_| -1);
        assert_eq!(result, Ok(42));
    }

    #[test]
    fn peel_concurrent_ok_outer_err_inner_runs_map_inner() {
        let r: Result<Result<i32, String>, wasmtime::Error> = Ok(Err("inner".to_string()));
        let result = peel_concurrent(r, |_| "outer".to_string(), |s| s.to_uppercase());
        assert_eq!(result, Err("INNER".to_string()));
    }

    #[test]
    fn peel_concurrent_err_outer_runs_map_outer() {
        // Construct a fake wasmtime::Error via the Into function.
        let wt_err = wasmtime::Error::msg("outer trap");
        let r: Result<Result<i32, String>, wasmtime::Error> = Err(wt_err);
        let result = peel_concurrent(r, |_| "outer-mapped".to_string(), |_| "inner".to_string());
        assert_eq!(result, Err("outer-mapped".to_string()));
    }
}
