# camel-wasm-sdk

WASM SDK for building Camel guest plugins. Provides types and utilities to write processors that run inside the Camel WASM runtime.

## Installation

```toml
[dependencies]
camel-wasm-sdk = "0.7"
```

## Quick Start

```rust
use camel_wasm_sdk::{export, Guest, WasmExchange, WasmBody, WasmError};

struct MyProcessor;

impl Guest for MyProcessor {
    fn process(mut exchange: WasmExchange) -> Result<WasmExchange, WasmError> {
        if let WasmBody::Text(ref mut text) = exchange.input.body {
            *text = text.to_uppercase();
        }
        Ok(exchange)
    }
}

export!(MyProcessor);
```

## Plugin with Init Hook

```rust
use camel_wasm_sdk::{export, Guest, WasmExchange, WasmError};

struct MyProcessor;

impl Guest for MyProcessor {
    fn init() -> Result<(), String> {
        Ok(())
    }

    fn process(exchange: WasmExchange) -> Result<WasmExchange, WasmError> {
        Ok(exchange)
    }
}

export!(MyProcessor);
```

## Plugin with State

```rust
use camel_wasm_sdk::{export, Guest, GuestState, WasmExchange, WasmBody, WasmError};

struct Config {
    prefix: String,
}

static STATE: GuestState<Config> = GuestState::new();

struct StatefulProcessor;

impl Guest for StatefulProcessor {
    fn process(mut exchange: WasmExchange) -> Result<WasmExchange, WasmError> {
        let config = STATE.get_or_init(|| Config {
            prefix: "[camel] ".to_string(),
        });
        if let WasmBody::Text(ref mut text) = exchange.input.body {
            *text = format!("{}{}", config.prefix, text);
        }
        Ok(exchange)
    }
}

export!(StatefulProcessor);
```

## CLI Workflow

```bash
camel plugin new my-plugin
cd my-plugin
camel plugin build
```

## Core Types

| Type | Description |
|------|-------------|
| `WasmExchange` | Simplified exchange with input/output messages |
| `WasmBody` | Body variants: `Empty`, `Text`, `Bytes`, `Json`, `Xml` |
| `WasmError` | Error variants: `ProcessorError`, `TypeConversion`, `Io`, `Timeout` |
| `WasmMessage` | Message with headers (`Vec<(String, String)>`) and body |
| `Guest` | Trait to implement: `init()` (optional) + `process()` (required) |
| `GuestState<T>` | Once-cell state persisted across `process()` calls |

## Host Functions

Available inside `process()` via the WASM interface:

- `camel_call(uri, payload)` — invoke any Camel endpoint
- `get_property(key)` — read exchange property
- `set_property(key, value)` — write exchange property

## Build Requirements

Plugins compile to `wasm32-wasip2`:

```bash
rustup target add wasm32-wasip2
cargo build --target wasm32-wasip2 --release
```

## License

Apache-2.0
