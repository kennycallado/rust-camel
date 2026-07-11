//! Streaming byte-counter WASM guest plugin.
//!
//! Demonstrates the streaming-body feature: reads an incoming `stream<u8>`
//! incrementally in 4 KiB chunks and counts bytes **without** materializing
//! the whole stream in WASM linear memory, then awaits the terminal future
//! to surface any producer-side error.
//!
//! ## Return-stream mode
//!
//! When the input body contains the marker `emit-return-stream`, the guest
//! emits a deterministic streaming return body (via `spawn_local` writer)
//! instead of the usual byte-count text. This exercises the plugin-specific
//! return-path drain (`call_process` → `spawn_return_drain` → `out.output.body`
//! reattachment).

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

/// Input body marker that triggers return-stream emission.
const RETURN_STREAM_MARKER: &str = "emit-return-stream";

struct ByteCounter;

impl Guest for ByteCounter {
    fn init() -> Result<(), String> {
        Ok(())
    }

    async fn process(mut exchange: WasmExchange) -> Result<WasmExchange, WasmError> {
        // Take the body by value (streams are not Clone); leave Empty behind so
        // the exchange stays fully owned for the return.
        let body = core::mem::replace(&mut exchange.input.body, WasmBody::Empty);

        // Check for return-stream trigger before consuming the body.
        let emit_return_stream = match &body {
            WasmBody::Text(s) => s.contains(RETURN_STREAM_MARKER),
            WasmBody::Bytes(b) => {
                core::str::from_utf8(b)
                    .map(|s| s.contains(RETURN_STREAM_MARKER))
                    .unwrap_or(false)
            }
            _ => false,
        };

        if emit_return_stream {
            // Return-stream mode: emit a deterministic streaming body via
            // spawn_local writer. The host's return-path drain reattaches
            // it as Body::Stream on the output exchange.
            let stream_body = emit_return_stream_body().await?;
            exchange.output = Some(WasmMessage {
                headers: exchange.input.headers.clone(),
                body: stream_body,
            });
            return Ok(exchange);
        }

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

/// Construct a `WasmBody::Stream` with a deterministic byte pattern for the
/// return-stream mode. Mirrors the bean example's `emit_stream_body` pattern:
/// `spawn_local` a writer task that feeds bytes into the stream, then return
/// the reader ends immediately. The spawned task is polled after `process`
/// returns, at which point the host has registered its stream consumer.
///
/// Emits 10 chunks of `b"chunk-N\n"` (80 bytes total) — large enough to
/// exceed the rendezvous buffer and prove multi-chunk drain without deadlock.
async fn emit_return_stream_body() -> Result<WasmBody, WasmError> {
    use bindings::wit_future;
    use bindings::wit_stream;

    let (mut stream_writer, stream_reader) = wit_stream::new::<u8>();
    let (future_writer, future_reader) = wit_future::new::<Result<(), WasmError>>(|| Ok(()));

    wit_bindgen::spawn_local(async move {
        // Write 10 deterministic chunks. Each write rendezvous with the host's
        // read; the spawn_local ensures we return from `process` first so the
        // host can register its consumer.
        for i in 0..10u8 {
            let chunk = format!("chunk-{i}\n").into_bytes();
            let remaining = stream_writer.write_all(chunk).await;
            if !remaining.is_empty() {
                // Host dropped the stream early — stop writing.
                break;
            }
        }

        // Drop writer to signal EOF, then resolve terminal with clean completion.
        drop(stream_writer);
        let _ = future_writer.write(Ok(())).await;
    });

    Ok(WasmBody::Stream(StreamBodyHandle {
        r#stream: stream_reader,
        terminal: future_reader,
        size_hint: Some(80), // 10 chunks × 8 bytes each
        content_type: Some("application/octet-stream".into()),
        origin: Some("plugin://emit_return_stream".into()),
    }))
}

bindings::export!(ByteCounter with_types_in bindings);
