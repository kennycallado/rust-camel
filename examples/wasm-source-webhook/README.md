# wasm-source-webhook

Fully runnable example: a WASM **source** component that turns inbound HTTP requests into pipeline Exchanges. Unlike a processor, the guest **is** the source — it owns an **async** consumption loop over a host-granted HTTP listener. The guest's `run`, `accept-http`, and `submit-exchange` are all async, driven under `Store::run_concurrent` on a tokio task.

## Running

```bash
# build the guest (first time only)
cd guest && cargo build --target wasm32-wasip2 && cd ..
cp guest/target/wasm32-wasip2/debug/wasm_source_webhook_guest.wasm plugins/

# run the route
camel run routes/webhook.yaml
```

Then send a request:

```bash
curl -X POST http://localhost:8080/webhook \
  -H "Content-Type: application/json" \
  -d '{"event": "test"}'
```

## What it shows

- **Guest-as-source pattern** — the WASM component fills the `Consumer` role (inbound), not the processor role.
- **Resource negotiation** — `configure(config) → source-plan` declares the capability the guest needs; the host grants an `http-listener` and hands it to `run(listener)`.
- **HTTP listener via a WIT resource** — the listener is host-owned; the guest borrows it and calls `accept-http`.
- **Async world** — `run`, `accept-http`, and `submit-exchange` are all async; the guest awaits host imports and the event loop drives concurrent progress.
- **Streaming HTTP body ingestion** — `accept-http` returns a `stream-body-handle`; the guest reads the body incrementally without materialization (no 10 MiB cap).
- **Fire-and-return submit-exchange** — with a streaming body, `submit-exchange` returns before the host drains it; an `AccessorTask` drains the stream in the background.
- **Guest `spawn_local` pattern** — for streaming echo bodies, the guest uses `wit_bindgen::spawn_local` to write the stream concurrently and avoid deadlock.
- **Backpressure via bounded channels** — capacity-1 channels propagate backpressure end-to-end: the HTTP client parks until the prior exchange is accepted by the pipeline.
- **Cancellation via channel close + epoch deadline** — `accept-http` returns `none` when the listener task exits, and `is-cancelled()` reflects the route's `CancellationToken`.

## Source component API

The guest implements the `source` world (`wit/camel-source.wit`). Two exports belong to the guest; the rest are host imports the guest calls from its loop:

| Function | Direction | Input | Output | Side effect |
|----------|-----------|-------|--------|-------------|
| `configure` | export (guest) | `list<tuple<string,string>>` config | `result<source-plan, wasm-error>` | Sync; parses `bind`/`path`/`crash`/`echo`; declares the `http-listener` capability |
| `run` | export (guest) | `borrow<http-listener>` | `result<_, wasm-error>` | **Async**; owns the accept→submit loop until cancelled or stopped. Driven under `Store::run_concurrent` |
| `accept-http` | import (host) | `borrow<http-listener>` | `result<option<http-request>, wasm-error>` | **Async**; awaits the next request, yielding to the event loop. Returns a `stream-body-handle` for incremental body reads |
| `submit-exchange` | import (host) | `wasm-exchange` | `result<submit-outcome, wasm-error>` | **Async**; pushes into the pipeline. Returns **before full body drain** (fire-and-return) for streaming bodies; `stopped` on shutdown |
| `is-cancelled` | import (host) | — | `bool` | Sync (must not yield); true when the route has been cancelled |

## source-config parameters

`configure()` receives the URI query params as `(key, value)` pairs. The example guest understands:

| Key | Default | Meaning |
|-----|---------|---------|
| `bind` | `0.0.0.0:8080` | Socket address the host-granted listener binds. |
| `path` | (all paths) | URL path filter accepted by the listener (e.g. `/webhook`). |
| `crash=run` | off | Test toggle — `run()` deliberately traps on its first iteration to exercise host crash/lifecycle handling. |
| `echo` | `bytes` | Echo body mode: `bytes` (materialize then echo), `stream` (re-emit as streaming body via `spawn_local`), `stream-fail` (emit partial stream then terminal error). |

## source-plan variants

`configure()` returns a `source-plan` whose `concurrency` field is one of:

- **`sequential`** — used here and supported by the host (capacity-1 drain).
- **`concurrent(max)`** — **rejected** by the host with an explicit error. Degrading `concurrent(N)` to sequential would violate the guest's declared contract, so the host refuses rather than silently downgrading.

The plan must request exactly one capability; the only capability granted today is `http-listener`.

## Echo streaming mode

When `echo=stream` is passed in the URI, the guest re-emits the request body as a **streaming `WasmBody::Stream`** via `submit-exchange` instead of a materialized `WasmBody::Bytes`. This exercises the guest→host streaming path of `submit-exchange`:

1. The guest reads the entire inbound stream into a buffer (via `StreamReader::read`).
2. It creates a fresh `stream<u8>` + terminal future pair.
3. It `spawn_local`s a writer task that re-emits the buffered body in bounded chunks.
4. It wraps the reader ends in a `WasmBody::Stream` and passes it to `submit-exchange`.
5. The host registers an `AccessorTask` drain (see component README §"Fire-and-return submit-exchange") that reads the stream in the background.

With `echo=stream-fail`, the writer emits only a partial chunk then resolves the terminal future with `Err`, exercising terminal-error propagation through the drain.

> **Note:** The `echo=stream` route does not use `log:info` — the streaming body would be discarded without a downstream consumer that reads it. Instead, a direct pipeline or a `file:` endpoint ensures the body is consumed.

## Guest `spawn_local` pattern

Streaming bodies from the guest to the host (both directions) require the same rendezvous pattern documented in the component README §"Return-path streaming":

- The component-model `stream<u8>` is a **rendezvous channel with no buffer**: `write_all().await` completes only when the host performs a matching read.
- The host registers its drain **after** `submit-exchange` returns — it needs `&Accessor` inside `run_concurrent`.
- Writing inline before `submit-exchange` **deadlocks**: the write waits for the host, the host waits for the call to return.

The guest MUST `spawn_local` the writer and return the reader ends immediately:

```rust
// guest/src/lib.rs (emit_stream_echo)
let (mut writer, reader) = wit_stream::new::<u8>();
let (future_writer, future_reader) = wit_future::new::<Result<(), WasmError>>(|| Ok(()));

wit_bindgen::spawn_local(async move {
    for chunk in data.chunks(4096) {
        writer.write_all(chunk.to_vec()).await.ok();
    }
    drop(writer);                                // EOF — drop writer FIRST
    let _ = future_writer.write(Ok(())).await;   // terminal resolves LAST
});

Ok(WasmBody::Stream(StreamBodyHandle {
    r#stream: reader,
    terminal: future_reader,
    ..
}))
```

Enable the `async-spawn` cargo feature on the guest for `wit_bindgen::spawn_local`:

```toml
# guest/Cargo.toml
wit-bindgen = { version = "0.58", features = ["async-spawn"] }
```

## Expected output

```
source HTTP listener bound addr=0.0.0.0:8080
source HTTP listener started addr=0.0.0.0:8080
```

After the `curl` above, the guest converts the request body into an exchange (`InOnly`) and submits it; the route forwards it to `log:info`:

```
[info] Body: {"event": "test"}
```

The guest also stamps `camel.http.method` and `camel.http.path` as exchange properties.

## Limitations

- **Sequential mode only** — `concurrent(N)` plans are rejected (see above).
- **HTTP transport only** — the single granted capability is `http-listener`; no Kafka/queue/gRPC source yet.
- **Guest `spawn_local` required for streaming submit** — the guest must enable `async-spawn` when using streaming bodies with `submit-exchange`.
- **Debug `.wasm` size** — the unoptimized debug build is ~3.5 MB; build with `--release` for a smaller artifact.

## Building the guest

The guest source is in `guest/`. It uses `wit-bindgen` directly against `../wit` (the example's local copy of `camel-source.wit`) and is excluded from the workspace — it targets `wasm32-wasip2`, not the native host.

The `Cargo.toml` enables the `async-spawn` feature on `wit-bindgen`, required for `wit_bindgen::spawn_local` when streaming echo bodies.

```bash
cd guest
cargo build --target wasm32-wasip2
# artifact: target/wasm32-wasip2/debug/wasm_source_webhook_guest.wasm
```

Copy it where the route can resolve it (the route references `wasm-source-webhook-guest.wasm` from the configured base directory):

```bash
cp target/wasm32-wasip2/debug/wasm_source_webhook_guest.wasm ../plugins/
```

> **Note:** The guest is excluded from the workspace (separate `[workspace]` in its `Cargo.toml`) because it targets `wasm32-wasip2`, not the native host.
