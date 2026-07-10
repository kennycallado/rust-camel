use std::future::Future;

use bindings::camel::plugin::types::{
    StreamBodyHandle, WasmBody, WasmError, WasmExchange, WasmMessage, WasmPattern,
};
use bindings::camel::plugin::source_host::{
    self, CapabilityRequest, ConcurrencyModel, HttpListener, HttpListenerSpec, HttpRequest,
    SourcePlan, SubmitOutcome,
};
use wit_bindgen::StreamResult;

mod bindings {
    wit_bindgen::generate!({
        world: "source",
        path: "../wit",
    });
}

/// Chunk size for incremental body reads — keeps linear-memory footprint bounded.
const READ_CHUNK: usize = 4096;

/// Chunk size for streaming echo writes.
const WRITE_CHUNK: usize = 4096;

struct WebhookSource;

/// Guest-side crash toggle. When `configure()` is called with `crash=run`,
/// `run()` deliberately traps (via `unreachable`) on its first iteration so
/// the host's crash/lifecycle handling can be exercised end-to-end. The value
/// survives between `configure()` and `run()` because the component instance is
/// long-lived for the source world.
static mut CRASH_IN_RUN: bool = false;

/// Echo mode selected at configure() time. Survives into run() because the
/// component instance is long-lived for the source world.
///
/// - `Bytes` (default): read the full body, submit it as `WasmBody::Bytes`.
/// - `Stream`: read the full body, then re-emit it as a streaming
///   `WasmBody::Stream` via a `spawn_local` writer — exercises the
///   submit-exchange guest→host streaming path.
/// - `StreamFail`: like `Stream` but the writer resolves the terminal future
///   with `Err` after emitting partial data — exercises terminal-error
///   propagation through the drain.
#[derive(Clone, Copy)]
enum EchoMode {
    Bytes,
    Stream,
    StreamFail,
}

static mut ECHO_MODE: EchoMode = EchoMode::Bytes;

impl bindings::Guest for WebhookSource {
    fn configure(config: Vec<(String, String)>) -> Result<SourcePlan, WasmError> {
        let mut bind = String::from("0.0.0.0:8080");
        let mut path: Option<String> = None;

        for (key, value) in &config {
            match key.as_str() {
                "bind" => bind = value.clone(),
                "path" => path = Some(value.clone()),
                "crash" if value == "run" => {
                    // SAFETY: the source component is single-threaded; configure()
                    // and run() never execute concurrently.
                    unsafe { CRASH_IN_RUN = true };
                }
                "echo" => {
                    // SAFETY: single-threaded component; configure() and run()
                    // never execute concurrently.
                    unsafe {
                        ECHO_MODE = match value.as_str() {
                            "stream" => EchoMode::Stream,
                            "stream-fail" => EchoMode::StreamFail,
                            _ => EchoMode::Bytes,
                        };
                    }
                }
                _ => {}
            }
        }

        Ok(SourcePlan {
            capabilities: vec![CapabilityRequest::HttpListener(HttpListenerSpec {
                bind,
                path,
            })],
            concurrency: ConcurrencyModel::Sequential,
        })
    }

    fn run(listener: &HttpListener) -> impl Future<Output = Result<(), WasmError>> {
        async move {
            // SAFETY: single-threaded component; see CRASH_IN_RUN docs.
            if unsafe { CRASH_IN_RUN } {
                // Intentional trap to exercise host crash handling (Q3 of the spike).
                unreachable!("crash-guest: deliberate trap in run()");
            }
            loop {
                if source_host::is_cancelled() {
                    return Ok(());
                }

                let req = source_host::accept_http(listener).await?;
                let Some(req) = req else {
                    // Cancelled — no more requests
                    return Ok(());
                };

                let exchange = http_request_to_exchange(req).await?;

                match source_host::submit_exchange(exchange).await? {
                    SubmitOutcome::Accepted => {}
                    SubmitOutcome::Stopped => return Ok(()),
                }
            }
        }
    }
}

/// Convert an HTTP request into a WasmExchange.
///
/// The body is a `stream-body-handle` — read it incrementally into a buffer
/// first. Depending on the configured [`EchoMode`], the echoed body is then
/// submitted either materialized (`Bytes`) or as a fresh streaming body
/// (`Stream` / `StreamFail`) that the host drains in the background.
async fn http_request_to_exchange(req: HttpRequest) -> Result<WasmExchange, WasmError> {
    let body = read_stream_body(req.body).await?;

    // SAFETY: single-threaded component; run() is the only reader of ECHO_MODE.
    // Read by value (Copy) — no shared reference to the mutable static.
    let mode = unsafe { ECHO_MODE };
    let wasm_body = match mode {
        EchoMode::Bytes => {
            if body.is_empty() {
                WasmBody::Empty
            } else {
                WasmBody::Bytes(body)
            }
        }
        EchoMode::Stream => emit_stream_echo(&body, /* fail */ false).await?,
        EchoMode::StreamFail => emit_stream_echo(&body, /* fail */ true).await?,
    };

    Ok(WasmExchange {
        input: WasmMessage {
            headers: req.headers,
            body: wasm_body,
        },
        output: None,
        properties: vec![
            ("camel.http.method".to_string(), req.method),
            ("camel.http.path".to_string(), req.path),
        ],
        pattern: WasmPattern::InOnly,
        correlation_id: String::new(),
        route_id: None,
        message_id: None,
    })
}

/// Read a `stream-body-handle` incrementally, buffering all chunks.
///
/// Mirrors the plugin input-streaming guest pattern
/// (`wasm-streaming-plugin/guest/src/lib.rs`). Awaits the terminal future
/// afterwards so a producer error propagates as a `WasmError`.
async fn read_stream_body(
    handle: bindings::camel::plugin::types::StreamBodyHandle,
) -> Result<Vec<u8>, WasmError> {
    let mut buf = Vec::new();
    let mut reader = handle.r#stream;
    loop {
        let chunk_buf = Vec::<u8>::with_capacity(READ_CHUNK);
        let (status, chunk) = reader.read(chunk_buf).await;
        buf.extend_from_slice(&chunk);
        match status {
            StreamResult::Complete(_) => {}
            StreamResult::Dropped | StreamResult::Cancelled => break,
        }
    }
    handle.terminal.await?;
    Ok(buf)
}

/// Re-emit `data` as a streaming body via a `spawn_local` writer.
///
/// Mirrors the bean `emit_stream_body` rendezvous pattern: create the
/// stream+future pair, `spawn_local` the writer BEFORE returning, and drop the
/// writer for EOF before resolving the terminal future. The component-model
/// `stream<u8>` is a rendezvous (no internal buffer): the writer's `write_all`
/// only completes once the host performs a matching read, which happens after
/// `submit-exchange` returns (the host registers its drain via `Accessor::spawn`
/// on the same event loop). Ordering is load-bearing: the terminal future MUST
/// resolve only AFTER the writer is dropped (EOF).
///
/// When `fail` is true, the writer resolves the terminal with `Err` after
/// emitting partial data, exercising terminal-error propagation.
async fn emit_stream_echo(data: &[u8], fail: bool) -> Result<WasmBody, WasmError> {
    use bindings::wit_future;
    use bindings::wit_stream;

    let (mut stream_writer, stream_reader) = wit_stream::new::<u8>();
    let (future_writer, future_reader) = wit_future::new::<Result<(), WasmError>>(|| Ok(()));

    let size_hint = data.len() as u64;
    let data = data.to_vec();
    wit_bindgen::spawn_local(async move {
        if fail {
            // Emit partial data (first chunk only), then fail.
            let partial = &data[..data.len().min(WRITE_CHUNK)];
            let _ = stream_writer.write_all(partial.to_vec()).await;
            drop(stream_writer);
            let _ = future_writer
                .write(Err(WasmError::ProcessorError(
                    "echo stream: requested failure after partial emit".into(),
                )))
                .await;
            return;
        }

        // Write the full body in bounded chunks.
        for chunk in data.chunks(WRITE_CHUNK) {
            let remaining = stream_writer.write_all(chunk.to_vec()).await;
            if !remaining.is_empty() {
                // Host dropped the stream early — stop writing.
                break;
            }
        }

        // Drop the writer to signal EOF. The reader observes the complete
        // byte sequence followed by end-of-stream.
        drop(stream_writer);

        // Only now resolve the terminal future (clean completion). Awaiting
        // the writes ensures the host has drained all bytes before it sees the
        // terminal result.
        let _ = future_writer.write(Ok(())).await;
    });

    Ok(WasmBody::Stream(StreamBodyHandle {
        r#stream: stream_reader,
        terminal: future_reader,
        size_hint: Some(size_hint),
        content_type: Some("application/octet-stream".into()),
        origin: Some("source://echo_stream".into()),
    }))
}

bindings::export!(WebhookSource with_types_in bindings);
