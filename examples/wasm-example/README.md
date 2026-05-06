# wasm-example

Fully runnable example: a pre-built echo plugin processed through a rust-camel route with Phase 4 production hardening.

## Running

```bash
cargo run -p wasm-example
```

No extra setup required. The pre-built plugin is in `fixtures/echo.wasm`.

## What it shows

- Registering `WasmComponent` in a `CamelContext`
- Phase 4 URI params: `timeout` and `max-memory`
- Timer → WASM → Log route pattern

```
timer:tick?period=1000&repeatCount=3
  → wasm:echo.wasm?timeout=5&max-memory=10485760
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

Plugins are Rust crates compiled to `wasm32-wasip2` using the `camel-wasm-sdk`:

```toml
# Cargo.toml
[lib]
crate-type = ["cdylib"]

[dependencies]
camel-wasm-sdk = "0.7"
```

```rust
// src/lib.rs
use camel_wasm_sdk::{export, Guest, WasmBody, WasmError, WasmExchange};

struct MyPlugin;

impl Guest for MyPlugin {
    fn init() -> Result<(), String> { Ok(()) }

    fn process(mut exchange: WasmExchange) -> Result<WasmExchange, WasmError> {
        if let WasmBody::Text(s) = &exchange.input.body {
            exchange.input.body = WasmBody::Text(format!("[processed] {s}"));
        }
        Ok(exchange)
    }
}

export!(MyPlugin);
```

```bash
cargo build --target wasm32-wasip2 --release
# → target/wasm32-wasip2/release/my_plugin.wasm
```

The `fixtures/echo.wasm` in this example was built from `guest/` using the same steps.
