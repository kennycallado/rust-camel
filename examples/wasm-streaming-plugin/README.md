# wasm-streaming-plugin

Fully runnable example: a pre-built streaming byte-counter plugin that reads incoming `stream<u8>` incrementally in 4 KiB chunks without materializing the whole stream in WASM linear memory.

## Return-stream mode

When the input body contains the marker `emit-return-stream`, the guest emits a deterministic streaming return body (10 chunks of `chunk-N\n`, 80 bytes total) via a `spawn_local` writer. This exercises the plugin-specific return-path drain (`call_process` → `spawn_return_drain` → `out.output.body` reattachment). The integration test `plugin_return_path_streams_body` validates this path end-to-end.

(For the bean-path return-stream pattern, see `examples/wasm-bean-example/`'s `emit_stream` method and the component README §"Return-path streaming".)

## Running

```bash
cargo run -p wasm-streaming-plugin
```

No extra setup required. The pre-built plugin is in `fixtures/streaming-plugin.wasm`.

## What it shows

- Registering `WasmComponent` in a `CamelContext` with streaming body support
- Phase 4 URI params: `timeout` and `max-memory`
- Guest-side streaming: `stream<u8>` incremental reads with bounded memory
- Output contract: `streamed {n} bytes` for stream bodies, byte-length for other body types

```
timer:tick?period=1000&repeatCount=3
  → wasm:streaming-plugin.wasm?timeout=5&max-memory=10485760
  → log:info
```

## URI parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `timeout` | 30 | Max seconds per guest call (epoch-based interruption) |
| `max-memory` | 52428800 | Max linear memory in bytes (50 MB) |

Zero or invalid values fall back to defaults.

## Error handling

| `WasmError` variant | Cause |
|--------------------|-------|
| `Timeout { plugin, timeout_secs }` | Guest exceeded epoch deadline |
| `OutOfMemory { plugin, max_memory_bytes }` | Guest exceeded memory cap |
| `Trap { plugin, reason }` | Guest hit unreachable / stack overflow |
| `GuestPanic(msg)` | Guest panicked |

The runtime resets automatically after any of these — no manual restart needed.

## Building your own plugin

Plugins are Rust crates compiled to `wasm32-wasip2` using `wit-bindgen`:

```toml
# Cargo.toml
[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = "0.58"
```

```rust
// src/lib.rs
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

struct ByteCounter;

impl Guest for ByteCounter {
    fn init() -> Result<(), String> { Ok(()) }

    async fn process(mut exchange: WasmExchange) -> Result<WasmExchange, WasmError> {
        let body = core::mem::replace(&mut exchange.input.body, WasmBody::Empty);
        let n: u64 = match body {
            WasmBody::Stream(handle) => count_stream_bytes(handle).await?,
            WasmBody::Bytes(b) => b.len() as u64,
            WasmBody::Text(s) | WasmBody::Json(s) | WasmBody::Xml(s) => s.len() as u64,
            WasmBody::Empty => 0,
        };
        exchange.output = Some(WasmMessage {
            headers: exchange.input.headers,
            body: WasmBody::Text(format!("streamed {n} bytes")),
        });
        Ok(exchange)
    }
}

bindings::export!(ByteCounter with_types_in bindings);
```

```bash
cd guest && cargo build --target wasm32-wasip2 --release
# → guest/target/wasm32-wasip2/release/streaming_plugin_guest.wasm
```

The `fixtures/streaming-plugin.wasm` in this example was built from `guest/` using the same steps.
