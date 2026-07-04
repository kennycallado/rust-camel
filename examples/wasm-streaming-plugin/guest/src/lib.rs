//! Streaming byte-counter WASM guest plugin.
//!
//! Demonstrates the streaming-body feature: reads an incoming `stream<u8>`
//! incrementally in 4 KiB chunks and counts bytes **without** materializing
//! the whole stream in WASM linear memory, then awaits the terminal future
//! to surface any producer-side error.

use bindings::Guest;
use bindings::camel::plugin::types::{
    StreamBodyHandle, WasmBody, WasmError, WasmExchange, WasmMessage,
};
use wit_bindgen::StreamResult;

mod bindings {
    wit_bindgen::generate!({
        world: "plugin",
        path: "wit",
    });
}

/// Chunk size for incremental reads — keeps linear-memory footprint bounded.
const READ_CHUNK: usize = 4096;

struct ByteCounter;

impl Guest for ByteCounter {
    fn init() -> Result<(), String> {
        Ok(())
    }

    async fn process(mut exchange: WasmExchange) -> Result<WasmExchange, WasmError> {
        // Take the body by value (streams are not Clone); leave Empty behind so
        // the exchange stays fully owned for the return.
        let body = core::mem::replace(&mut exchange.input.body, WasmBody::Empty);

        let n: u64 = match body {
            WasmBody::Stream(handle) => count_stream_bytes(handle).await?,
            WasmBody::Bytes(b) => b.len() as u64,
            WasmBody::Text(s) => s.len() as u64,
            WasmBody::Json(s) => s.len() as u64,
            WasmBody::Xml(s) => s.len() as u64,
            WasmBody::Empty => 0,
        };

        exchange.output = Some(WasmMessage {
            headers: exchange.input.headers.clone(),
            body: WasmBody::Text(format!("streamed {n} bytes")),
        });
        Ok(exchange)
    }
}

/// Read the `stream<u8>` in fixed-size chunks, tallying bytes without ever
/// holding the full body. Awaits the terminal future afterwards so a
/// producer error propagates as a `WasmError`.
async fn count_stream_bytes(handle: StreamBodyHandle) -> Result<u64, WasmError> {
    let mut count: u64 = 0;
    // `read` fills the *spare capacity* of the supplied buffer, so pass an
    // empty Vec with capacity — it comes back populated with the chunk.
    let mut reader = handle.r#stream;
    loop {
        let buf = Vec::<u8>::with_capacity(READ_CHUNK);
        let (status, chunk) = reader.read(buf).await;
        count += chunk.len() as u64;
        match status {
            // Stream still open and yielded `Complete(n)` — keep reading.
            StreamResult::Complete(_) => {}
            // Producer dropped / cancelled its write handle → stream finished.
            StreamResult::Dropped | StreamResult::Cancelled => break,
        }
    }
    // Surface any error the producer attached to the terminal future.
    handle.terminal.await?;
    Ok(count)
}

bindings::export!(ByteCounter with_types_in bindings);
