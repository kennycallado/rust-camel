# wasm-bean-example

Fully runnable example: a WASM bean plugin (`text-utils`) with text-transform methods, persistent state across two concurrent routes, and **streaming return bodies** (guest → host `Body::Stream`).

## Running

```bash
cargo run -p wasm-bean-example
```

No extra setup required — the pre-built plugin is in `fixtures/text_utils.wasm`.

## What it shows

- **Multi-method WASM bean** with `upper`, `reverse`, and `last` methods
- **`host_store` / `host_load`** — state persists across invocations within the same bean
- **Two concurrent routes** sharing one bean instance via `BeanRegistry`
- **JS language step** producing random input text into the exchange body

## Routes

```
Route 1 (transform-route):
  timer:tick?period=150&repeatCount=200
    → script(js): picks random text from ["hello","world","camel","wasm","bean","rust"]
    → bean:text-utils.upper

Route 2 (last-route):
  timer:tick?period=300&repeatCount=100
    → bean:text-utils.last
    → log:info
```

**Route 1** sets a random text in the body via JS, then calls `bean:text-utils.upper` which uppercases it and saves the result in `host_store("last_result")`.

**Route 2** calls `bean:text-utils.last` which reads `host_store("last_result")` and returns the saved value — demonstrating that state persists across calls.

## Expected output

```
bean methods: ["upper", "reverse", "last"]

=== WASM Bean Example ===
Route 1 (transform): timer → js(set random text) → bean:upper → log
Route 2 (last):      timer → bean:last → log
Running for ~10s…

[info] Body: (no transform yet)    ← last-route fires before any transform
[info] Body: BEAN                  ← transform-route
[info] Body: RUST                  ← last-route reads last saved transform
[info] Body: HELLO                 ← transform-route
...
```

The interlacing depends on timer timing, but `last` always shows whatever `upper` saved most recently.

## Limitation: static method binding

The bean method name is fixed at route definition time. `.bean("text-utils", "upper")` always calls `upper` — the method cannot be resolved dynamically from exchange properties at runtime.

This means each method that needs a different bean invocation requires its own route:

```rust
// One route per method
.bean("text-utils", "upper")
.bean("text-utils", "reverse")  // needs a separate route
```

Dynamic method resolution (e.g., from a header or property) is a future enhancement.

## Building the guest plugin

The guest source is in `guest/`. It uses `wit-bindgen` directly (not the SDK's re-exported macro) because `wit_bindgen::generate!` with `pub_export_macro` requires the bindings to be generated in the guest crate itself.

```bash
cd guest
cargo build --target wasm32-wasip2 --release
cp target/wasm32-wasip2/release/text_utils_bean.wasm ../fixtures/text_utils.wasm
```

> **Note:** The guest is excluded from the workspace (separate `[workspace]` in its Cargo.toml) because it targets `wasm32-wasip2`, not the native host.

## Bean plugin API

The guest implements these methods:

| Method | Input | Output | Side effect |
|--------|-------|--------|-------------|
| `upper` | Text body | Uppercased text | Saves to `host_store("last_result")` |
| `reverse` | Text body | Reversed text | Saves to `host_store("last_result")` |
| `last` | (ignored) | Last saved result from store | None |
| `emit_stream` | (ignored) | **Streaming body** (`data\n` × 5) | Demonstrates return-path streaming |
| `emit_stream_fail` | (ignored) | Partial stream (`partial`) + terminal error | Error propagation through a stream |
| `emit_stream_slow` | (ignored) | 20 small chunks (slow emit) | Backpressure / cancel-mid-stream behavior |

Only `upper` is used in the demo routes, but the others are available — notably
the `emit_stream*` methods demonstrate **return-path streaming** (a guest that
returns a `Body::Stream`).

## Streaming return bodies

The `emit_stream*` methods show how a WASM guest returns a **streaming body**
(the host drains it lazily into the pipeline instead of materializing it).
Because the component-model `stream<u8>` is a rendezvous channel with no buffer,
the guest must write concurrently via `wit_bindgen::spawn_local` and return the
reader ends immediately — writing inline before returning **deadlocks**. See the
component README §"Return-path streaming" for the full pattern + rationale, and
`guest/src/lib.rs` (`emit_stream_body`) for the canonical reference
implementation, including the load-bearing ordering (drop the writer for EOF
*before* resolving the terminal future).
