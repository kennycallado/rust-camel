//! WASM component error types.

use camel_api::CamelError;

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
}

impl From<WasmError> for CamelError {
    fn from(err: WasmError) -> Self {
        match &err {
            WasmError::GuestPanic(msg) => CamelError::ProcessorError(msg.clone()),
            WasmError::TypeConversion(msg) => CamelError::TypeConversionFailed(msg.clone()),
            WasmError::ModuleNotFound(msg) => CamelError::ComponentNotFound(msg.clone()),
            _ => CamelError::ProcessorError(err.to_string()),
        }
    }
}
