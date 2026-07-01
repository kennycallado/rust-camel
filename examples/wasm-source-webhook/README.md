# wasm-source-webhook

Fully runnable example: a WASM **source** component that turns inbound HTTP requests into pipeline Exchanges. Unlike a processor, the guest **is** the source — it owns the consumption loop over a host-granted HTTP listener.

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
- **Backpressure via a bounded channel** — capacity-1 channels propagate backpressure end-to-end: the HTTP client parks until the prior exchange is accepted by the pipeline.
- **Cancellation via channel close + epoch deadline** — `accept-http` returns `none` when the listener task exits, and `is-cancelled()` reflects the route's `CancellationToken`.

## Source component API

The guest implements the `source` world (`wit/camel-source.wit`). Two exports belong to the guest; the rest are host imports the guest calls from its loop:

| Function | Direction | Input | Output | Side effect |
|----------|-----------|-------|--------|-------------|
| `configure` | export (guest) | `list<tuple<string,string>>` config | `result<source-plan, wasm-error>` | Parses `bind`/`path`/`crash`; declares the `http-listener` capability |
| `run` | export (guest) | `borrow<http-listener>` | `result<_, wasm-error>` | Owns the blocking accept→submit loop until cancelled or stopped |
| `accept-http` | import (host) | `borrow<http-listener>` | `result<option<http-request>, wasm-error>` | Blocking; returns the next request or `none` on cancel |
| `submit-exchange` | import (host) | `wasm-exchange` | `result<submit-outcome, wasm-error>` | Pushes into the pipeline; blocks on backpressure; `stopped` on shutdown |
| `is-cancelled` | import (host) | — | `bool` | True when the route has been cancelled |

## source-config parameters

`configure()` receives the URI query params as `(key, value)` pairs. The example guest understands:

| Key | Default | Meaning |
|-----|---------|---------|
| `bind` | `0.0.0.0:8080` | Socket address the host-granted listener binds. |
| `path` | (all paths) | URL path filter accepted by the listener (e.g. `/webhook`). |
| `crash=run` | off | Test toggle — `run()` deliberately traps on its first iteration to exercise host crash/lifecycle handling. |

## source-plan variants

`configure()` returns a `source-plan` whose `concurrency` field is one of:

- **`sequential`** — used here and supported by the host (capacity-1 drain).
- **`concurrent(max)`** — **rejected** by the host with an explicit error. Degrading `concurrent(N)` to sequential would violate the guest's declared contract, so the host refuses rather than silently downgrading.

The plan must request exactly one capability; the only capability granted today is `http-listener`.

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

- **Synchronous bindings (WASI 0.2)** — the guest targets `wasm32-wasip2` and its host imports block on tokio channels, so `run()` executes on a dedicated OS thread via `spawn_blocking` (off the async runtime).
- **HTTP transport only** — the single granted capability is `http-listener`; no Kafka/queue/gRPC source yet.
- **Sequential mode only** — `concurrent(N)` plans are rejected (see above).
- **Debug `.wasm` size** — the unoptimized debug build is ~3.5 MB; build with `--release` for a smaller artifact.

## Building the guest

The guest source is in `guest/`. It uses `wit-bindgen` directly against `../wit` (the example's local copy of `camel-source.wit`) and is excluded from the workspace — it targets `wasm32-wasip2`, not the native host.

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
