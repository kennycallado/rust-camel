use crate::bindings::Guest;
use crate::{WasmError, WasmExchange};

pub trait Plugin: Sized {
    fn process(exchange: WasmExchange) -> Result<WasmExchange, WasmError>;

    fn init() -> Result<(), String> {
        Ok(())
    }
}

impl<T: Plugin> Guest for T {
    fn process(exchange: WasmExchange) -> Result<WasmExchange, WasmError> {
        <T as Plugin>::process(exchange)
    }

    fn init() -> Result<(), String> {
        <T as Plugin>::init()
    }
}
