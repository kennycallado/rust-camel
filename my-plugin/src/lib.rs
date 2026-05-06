use camel_wasm_sdk::{export, Guest, WasmBody, WasmError, WasmExchange};

struct Processor;

impl Guest for Processor {
    fn init() -> Result<(), String> {
        Ok(())
    }

    fn process(mut exchange: WasmExchange) -> Result<WasmExchange, WasmError> {
        exchange.input.body = match &exchange.input.body {
            WasmBody::Text(s) => WasmBody::Text(format!("processed: {s}")),
            other => other.clone(),
        };
        Ok(exchange)
    }
}

export!(Processor);
