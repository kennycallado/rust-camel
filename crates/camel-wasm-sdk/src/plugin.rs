use crate::bindings::Guest;
use crate::{WasmError, WasmExchange};

pub trait ProcessorPlugin: Sized {
    fn process(exchange: WasmExchange) -> Result<WasmExchange, WasmError>;

    fn init() -> Result<(), String> {
        Ok(())
    }
}

impl<T: ProcessorPlugin> Guest for T {
    fn process(exchange: WasmExchange) -> Result<WasmExchange, WasmError> {
        <T as ProcessorPlugin>::process(exchange)
    }

    fn init() -> Result<(), String> {
        <T as ProcessorPlugin>::init()
    }
}
