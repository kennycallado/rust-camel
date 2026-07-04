use bindings::camel::plugin::types::{WasmBody, WasmError, WasmExchange};
use bindings::Guest;

#[allow(clippy::too_many_arguments)]
mod bindings {
    wit_bindgen::generate!({
        world: "bean",
        path: "../wit",
    });
}

struct TextUtils;

impl Guest for TextUtils {
    fn init(config: Vec<(String, String)>) -> Result<(), String> {
        for (key, value) in &config {
            let _ = bindings::camel::plugin::host::host_store(
                &format!("cfg:{key}"),
                value,
            );
        }
        Ok(())
    }

    fn methods() -> Vec<String> {
        vec!["upper".into(), "reverse".into(), "last".into()]
    }

    async fn invoke(method: String, mut exchange: WasmExchange) -> Result<WasmExchange, WasmError> {
        match method.as_str() {
            "upper" => {
                let text = extract_text(&exchange.input.body);
                let result = text.to_uppercase();
                bindings::camel::plugin::host::host_store("last_result", &result)
                    .map_err(|e| WasmError::Io(e.to_string()))?;
                exchange.input.body = WasmBody::Text(result);
                Ok(exchange)
            }
            "reverse" => {
                let text = extract_text(&exchange.input.body);
                let result = text.chars().rev().collect::<String>();
                bindings::camel::plugin::host::host_store("last_result", &result)
                    .map_err(|e| WasmError::Io(e.to_string()))?;
                exchange.input.body = WasmBody::Text(result);
                Ok(exchange)
            }
            "last" => {
                let last = bindings::camel::plugin::host::host_load("last_result")
                    .map_err(|e| WasmError::Io(e.to_string()))?;
                match last {
                    Some(text) => exchange.input.body = WasmBody::Text(text),
                    None => exchange.input.body = WasmBody::Text("(no transform yet)".into()),
                }
                Ok(exchange)
            }
            _ => Err(WasmError::ProcessorError(format!(
                "unknown method: {method}"
            ))),
        }
    }
}

fn extract_text(body: &WasmBody) -> String {
    match body {
        WasmBody::Text(s) => s.clone(),
        WasmBody::Json(s) => s.clone(),
        _ => String::new(),
    }
}

bindings::export!(TextUtils with_types_in bindings);
