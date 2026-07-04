use bindings::camel::plugin::types::{WasmBody, WasmError, WasmExchange, WasmMessage};
use bindings::Guest;

mod bindings {
    wit_bindgen::generate!({
        world: "plugin",
        path: "wit",
    });
}

struct Echo;

impl Guest for Echo {
    fn init() -> Result<(), String> {
        Ok(())
    }

    async fn process(mut exchange: WasmExchange) -> Result<WasmExchange, WasmError> {
        // Echo: move input → output so the route can observe the round-trip.
        if exchange.output.is_none() {
            exchange.output = Some(WasmMessage {
                headers: exchange.input.headers.clone(),
                body: WasmBody::Text(text_body(&exchange.input.body)),
            });
        }
        // Also normalise empty bodies to text so callers can rely on a text payload.
        match exchange.input.body {
            WasmBody::Empty => {
                exchange.input.body = WasmBody::Text(String::new());
            }
            _ => {}
        }
        Ok(exchange)
    }
}

fn text_body(body: &WasmBody) -> String {
    match body {
        WasmBody::Text(s) => s.clone(),
        WasmBody::Json(s) => s.clone(),
        WasmBody::Xml(s) => s.clone(),
        WasmBody::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        WasmBody::Empty => String::new(),
        WasmBody::Stream(_) => String::new(),
    }
}

bindings::export!(Echo with_types_in bindings);
