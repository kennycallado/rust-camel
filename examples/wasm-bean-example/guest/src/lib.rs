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
        vec![
            "upper".into(),
            "reverse".into(),
            "last".into(),
            "count".into(),
            "fail".into(),
            "emit_stream".into(),
            "emit_stream_fail".into(),
            "emit_stream_slow".into(),
        ]
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
            // Stream-emission probe: construct a `stream<u8>` writer, write a
            // deterministic byte pattern, drop the writer for EOF, and return
            // the exchange with a `WasmBody::Stream` so the host-side return-
            // path drain can be exercised end-to-end.
            "emit_stream" => {
                let body = emit_stream_body().await?;
                exchange.input.body = body;
                Ok(exchange)
            }
            // Emit-then-fail probe: writes some bytes, then resolves the
            // terminal future with Err. Tests terminal error propagation
            // (Task 12).
            "emit_stream_fail" => {
                let body = emit_stream_fail_body().await?;
                exchange.input.body = body;
                Ok(exchange)
            }
            // Slow-emit probe: emits many small chunks with yields between
            // them. Tests backpressure under slow consumer (Task 13).
            "emit_stream_slow" => {
                let body = emit_stream_slow_body().await?;
                exchange.input.body = body;
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

/// Construct a `WasmBody::Stream` with a deterministic byte pattern.
///
/// The component-model `stream<u8>` is a **rendezvous** with no internal
/// buffer: `write_all().await` blocks until the host performs a matching
/// `stream.read`. The host only registers its stream consumer AFTER `invoke`
/// returns (it needs `&mut Store` inside `run_concurrent`). Writing inline
/// here would therefore deadlock — `write_all` can't complete until the host
/// reads, the host can't read until `invoke` returns, `invoke` can't return
/// until the write completes.
///
/// The fix mirrors the host-side `BoxStreamProducer` (which spawns a writer
/// inside `run_concurrent`): we `spawn_local` a task that writes the pattern
/// and resolves the terminal future, then return the reader ends immediately.
/// The spawned task is a component-model-async task polled *after* `invoke`
/// returns, so its writes rendezvous with the host's reads. This is the
/// blessed wasmtime pattern (see wasmtime `async_cross_instance_source`
/// test program).
///
/// Ordering is load-bearing: the terminal future MUST resolve only AFTER the
/// writer is dropped (EOF). The host's drain awaits the terminal future and
/// then closes its channel, so resolving early could truncate the byte stream.
async fn emit_stream_body() -> Result<WasmBody, WasmError> {
    use bindings::wit_future;
    use bindings::wit_stream;

    // Create stream and terminal-future pairs. The `wit_future` default
    // closure is only invoked if the writer is dropped without an explicit
    // write; we always write, so `Ok(())` is a benign fallback.
    let (mut stream_writer, stream_reader) = wit_stream::new::<u8>();
    let (future_writer, future_reader) = wit_future::new::<Result<(), WasmError>>(|| Ok(()));

    // Write concurrently with the return. This task is polled by the
    // component-model-async executor after `invoke` returns, at which point
    // the host has registered its stream consumer and reads can rendezvous
    // with these writes.
    wit_bindgen::spawn_local(async move {
        // Write a deterministic pattern: 5 chunks of "data\n" (25 bytes).
        // The integration test asserts on this exact byte sequence.
        let pattern = b"data\n";
        for _ in 0..5 {
            let remaining = stream_writer.write_all(pattern.to_vec()).await;
            if !remaining.is_empty() {
                // Host dropped the stream early — stop writing.
                break;
            }
        }

        // Drop the writer to signal EOF. The reader observes the complete
        // byte sequence followed by end-of-stream.
        drop(stream_writer);

        // Only now resolve the terminal future (clean completion). Awaiting
        // the write ensures the host has drained all bytes before it sees the
        // terminal result and closes its drain channel.
        let _ = future_writer.write(Ok(())).await;
    });

    Ok(WasmBody::Stream(StreamBodyHandle {
        r#stream: stream_reader,
        terminal: future_reader,
        size_hint: Some(25),
        content_type: Some("application/octet-stream".into()),
        origin: Some("bean://emit_stream".into()),
    }))
}

/// Emit-then-fail variant: writes some bytes, then resolves terminal with Err.
/// Tests terminal error propagation (Task 12).
async fn emit_stream_fail_body() -> Result<WasmBody, WasmError> {
    use bindings::wit_future;
    use bindings::wit_stream;

    let (mut stream_writer, stream_reader) = wit_stream::new::<u8>();
    let (future_writer, future_reader) = wit_future::new::<Result<(), WasmError>>(|| Ok(()));

    wit_bindgen::spawn_local(async move {
        // Write some bytes first (partial data before failure).
        let pattern = b"partial";
        let remaining = stream_writer.write_all(pattern.to_vec()).await;
        if !remaining.is_empty() {
            // Host dropped early — stop.
            drop(stream_writer);
            let _ = future_writer.write(Ok(())).await;
            return;
        }

        // Drop writer for EOF, then resolve terminal with error.
        drop(stream_writer);
        let _ = future_writer
            .write(Err(WasmError::ProcessorError(
                "bean requested failure after partial emit".into(),
            )))
            .await;
    });

    Ok(WasmBody::Stream(StreamBodyHandle {
        r#stream: stream_reader,
        terminal: future_reader,
        size_hint: Some(7),
        content_type: Some("application/octet-stream".into()),
        origin: Some("bean://emit_stream_fail".into()),
    }))
}

/// Slow-emit variant: emits many small chunks with yields between them.
/// Tests backpressure under slow consumer (Task 13).
async fn emit_stream_slow_body() -> Result<WasmBody, WasmError> {
    use bindings::wit_future;
    use bindings::wit_stream;

    let (mut stream_writer, stream_reader) = wit_stream::new::<u8>();
    let (future_writer, future_reader) = wit_future::new::<Result<(), WasmError>>(|| Ok(()));

    wit_bindgen::spawn_local(async move {
        // Emit 20 small chunks with yields between them.
        for i in 0..20u8 {
            let chunk = vec![i; 4]; // 4 bytes per chunk
            let remaining = stream_writer.write_all(chunk).await;
            if !remaining.is_empty() {
                // Host dropped early — stop.
                break;
            }
            // Yield to allow host to read and test backpressure.
            wit_bindgen::yield_async().await;
        }

        drop(stream_writer);
        let _ = future_writer.write(Ok(())).await;
    });

    Ok(WasmBody::Stream(StreamBodyHandle {
        r#stream: stream_reader,
        terminal: future_reader,
        size_hint: Some(80), // 20 chunks × 4 bytes
        content_type: Some("application/octet-stream".into()),
        origin: Some("bean://emit_stream_slow".into()),
    }))
}

bindings::export!(TextUtils with_types_in bindings);
