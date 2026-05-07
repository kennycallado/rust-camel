use crate::bean_bindings::Guest;
use crate::bean_bindings::camel::plugin::types::{
    WasmBody as BeanWasmBody, WasmError as BeanWasmError, WasmExchange as BeanWasmExchange,
    WasmMessage as BeanWasmMessage, WasmPattern as BeanWasmPattern,
};
use crate::bindings::camel::plugin::types::{
    WasmBody, WasmError, WasmExchange, WasmMessage, WasmPattern,
};

// --- From bean_bindings types → bindings types ---

impl From<BeanWasmBody> for WasmBody {
    fn from(v: BeanWasmBody) -> Self {
        match v {
            BeanWasmBody::Empty => WasmBody::Empty,
            BeanWasmBody::Text(s) => WasmBody::Text(s),
            BeanWasmBody::Bytes(b) => WasmBody::Bytes(b),
            BeanWasmBody::Json(s) => WasmBody::Json(s),
            BeanWasmBody::Xml(s) => WasmBody::Xml(s),
        }
    }
}

impl From<BeanWasmPattern> for WasmPattern {
    fn from(v: BeanWasmPattern) -> Self {
        match v {
            BeanWasmPattern::InOnly => WasmPattern::InOnly,
            BeanWasmPattern::InOut => WasmPattern::InOut,
        }
    }
}

impl From<BeanWasmMessage> for WasmMessage {
    fn from(v: BeanWasmMessage) -> Self {
        WasmMessage {
            headers: v.headers,
            body: v.body.into(),
        }
    }
}

impl From<BeanWasmExchange> for WasmExchange {
    fn from(v: BeanWasmExchange) -> Self {
        WasmExchange {
            input: v.input.into(),
            output: v.output.map(Into::into),
            properties: v.properties,
            pattern: v.pattern.into(),
            correlation_id: v.correlation_id,
        }
    }
}

impl From<BeanWasmError> for WasmError {
    fn from(v: BeanWasmError) -> Self {
        match v {
            BeanWasmError::ProcessorError(s) => WasmError::ProcessorError(s),
            BeanWasmError::TypeConversion(s) => WasmError::TypeConversion(s),
            BeanWasmError::Io(s) => WasmError::Io(s),
            BeanWasmError::Timeout => WasmError::Timeout,
        }
    }
}

// --- From bindings types → bean_bindings types ---

impl From<WasmBody> for BeanWasmBody {
    fn from(v: WasmBody) -> Self {
        match v {
            WasmBody::Empty => BeanWasmBody::Empty,
            WasmBody::Text(s) => BeanWasmBody::Text(s),
            WasmBody::Bytes(b) => BeanWasmBody::Bytes(b),
            WasmBody::Json(s) => BeanWasmBody::Json(s),
            WasmBody::Xml(s) => BeanWasmBody::Xml(s),
        }
    }
}

impl From<WasmPattern> for BeanWasmPattern {
    fn from(v: WasmPattern) -> Self {
        match v {
            WasmPattern::InOnly => BeanWasmPattern::InOnly,
            WasmPattern::InOut => BeanWasmPattern::InOut,
        }
    }
}

impl From<WasmMessage> for BeanWasmMessage {
    fn from(v: WasmMessage) -> Self {
        BeanWasmMessage {
            headers: v.headers,
            body: v.body.into(),
        }
    }
}

impl From<WasmExchange> for BeanWasmExchange {
    fn from(v: WasmExchange) -> Self {
        BeanWasmExchange {
            input: v.input.into(),
            output: v.output.map(Into::into),
            properties: v.properties,
            pattern: v.pattern.into(),
            correlation_id: v.correlation_id,
        }
    }
}

impl From<WasmError> for BeanWasmError {
    fn from(v: WasmError) -> Self {
        match v {
            WasmError::ProcessorError(s) => BeanWasmError::ProcessorError(s),
            WasmError::TypeConversion(s) => BeanWasmError::TypeConversion(s),
            WasmError::Io(s) => BeanWasmError::Io(s),
            WasmError::Timeout => BeanWasmError::Timeout,
        }
    }
}

/// Trait for bean-style plugins with multi-method dispatch.
///
/// Beans expose multiple named methods via `methods()` and handle
/// invocations via `invoke(method, exchange)`. The host calls `init()`
/// once at registration and `invoke()` for each route step targeting
/// a specific method on this bean.
pub trait BeanPlugin: Sized {
    /// Returns the list of method names this bean exposes.
    fn methods() -> Vec<&'static str>;

    /// Invokes a specific method with the given exchange.
    fn invoke(method: &str, exchange: WasmExchange) -> Result<WasmExchange, WasmError>;

    /// Optional initialization hook. Called once when the WASM module is instantiated.
    fn init() -> Result<(), String> {
        Ok(())
    }
}

impl<T: BeanPlugin> Guest for T {
    fn invoke(
        method: String,
        exchange: BeanWasmExchange,
    ) -> Result<BeanWasmExchange, BeanWasmError> {
        <T as BeanPlugin>::invoke(&method, exchange.into())
            .map(Into::into)
            .map_err(Into::into)
    }

    fn methods() -> Vec<String> {
        <T as BeanPlugin>::methods()
            .into_iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn init() -> Result<(), String> {
        <T as BeanPlugin>::init()
    }
}
