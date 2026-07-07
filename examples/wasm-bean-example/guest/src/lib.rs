use bindings::camel::plugin::types::{StreamBodyHandle, WasmBody, WasmError, WasmExchange};
use bindings::Guest;
use wit_bindgen::StreamResult;

/// Chunk size for incremental stream reads — keeps linear-memory footprint bounded.
const READ_CHUNK: usize = 4096;

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
        vec!["upper".into(), "reverse".into(), "last".into(), "count".into(), "fail".into()]
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
            // Stream body proof: read an incoming `stream<u8>` incrementally
            // in bounded chunks and count bytes WITHOUT materializing the
            // whole body, then surface the count in the input body — bean
            // convention is to transform `input.body` in place (see
            // `upper`/`reverse`), unlike the processor world which uses
            // `output`.
            "count" => {
                let body = core::mem::replace(&mut exchange.input.body, WasmBody::Empty);
                let n: u64 = match body {
                    WasmBody::Stream(handle) => count_stream_bytes(handle).await?,
                    WasmBody::Bytes(b) => b.len() as u64,
                    WasmBody::Text(s) => s.len() as u64,
                    WasmBody::Json(s) => s.len() as u64,
                    WasmBody::Xml(s) => s.len() as u64,
                    WasmBody::Empty => 0,
                };
                exchange.input.body = WasmBody::Text(format!("streamed {n} bytes"));
                Ok(exchange)
            }
            // Error-path probe: return a WasmError immediately so the host
            // 2-layer peel + CamelError mapping can be exercised by tests.
            "fail" => Err(WasmError::ProcessorError("bean requested failure".into())),
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

/// Read a `stream<u8>` in fixed-size chunks, tallying bytes without ever
/// holding the full body. Awaits the terminal future afterwards so a
/// producer error propagates as a `WasmError`.
async fn count_stream_bytes(handle: StreamBodyHandle) -> Result<u64, WasmError> {
    let mut count: u64 = 0;
    let mut reader = handle.r#stream;
    loop {
        let buf = Vec::<u8>::with_capacity(READ_CHUNK);
        let (status, chunk) = reader.read(buf).await;
        count += chunk.len() as u64;
        match status {
            StreamResult::Complete(_) => {}
            StreamResult::Dropped | StreamResult::Cancelled => break,
        }
    }
    handle.terminal.await?;
    Ok(count)
}

bindings::export!(TextUtils with_types_in bindings);
