# camel-component-wasm

WASM plugin component for rust-camel — loads and executes WASM modules as route processors using the Component Model.

## Features

- **WASM Component Model**: Loads WASM modules compiled for `wasm32-wasip2` target
- **Wasmtime v31**: Latest runtime with async support and component-model features
- **Host Functions**: `camel_call()`, `get_property()`, `set_property()` for guest-host communication
- **URI-Based Routing**: `wasm:path/to/module.wasm` format for easy integration
- **Path Validation**: Prevents directory traversal and escapes from project root
- **Recursion Guard**: Blocks nested WASM calls to prevent infinite loops
- **Tower Service**: Implements `Service<Exchange>` for async processing
- **Exchange Properties**: Per-request properties accessible from WASM via host functions

## Installation

Add to `Cargo.toml` (workspace dependency):

```toml
[dependencies]
camel-component-wasm.workspace = true
```

## URI Format

```
wasm:path/to/module.wasm
```

- Must be relative path (no leading `/`)
- No `..` components allowed
- Resolved against configured base directory
- Example: `wasm:plugins/transform.wasm`

## Host Functions

WASM plugins can call these host functions from guest code:

### `camel_call(uri: String, payload: String) -> Result<String>`

Calls another endpoint from within WASM.

```rust
// Guest-side (WASM)
use camel_wasm_sdk::Host;

async fn process(exchange: &mut Exchange) -> Result<()> {
    let response = Host::camel_call("log:info", "Processing data".to_string()).await?;
    Ok(())
}
```

### `get_property(key: String) -> Option<String>`

Retrieves an exchange property by key.

```rust
let user_id = Host::get_property("userId".to_string()).await?;
```

### `set_property(key: String, value: String)`

Sets an exchange property. Value can be JSON string for structured data.

```rust
Host::set_property("timestamp".to_string(), "2024-01-01T00:00:00Z".to_string()).await;
Host::set_property("metadata".to_string(), r#"{"source":"wasm"}"#.to_string()).await;
```

## Usage

### Registration

```rust
use camel_component_wasm::WasmBundle;
use camel_core::CamelContext;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ctx = CamelContext::new();
    let registry = ctx.registry().clone();

    // Register WASM component with base directory
    let bundle = WasmBundle::new(registry, PathBuf::from("./wasm-plugins"));
    bundle.register_all(&mut ctx);

    ctx.start().await?;
    Ok(())
}
```

### Route with WASM Processor

```rust
use camel_builder::RouteBuilder;

// Route that processes data through a WASM module
ctx.add_route(
    RouteBuilder::from("http://0.0.0.0:8080/process")
        .to("wasm:plugins/transform.wasm")
        .to("log:info")
        .build()?
).await?;
```

### WASM Plugin Example

Build your plugin with `wasm32-wasip2` target:

```rust
// src/main.rs (guest plugin)
use camel_wasm_sdk::{Exchange, Host, plugin};

#[plugin]
async fn init() -> Result<()> {
    println!("Plugin initialized");
    Ok(())
}

#[plugin]
async fn process(mut exchange: Exchange) -> Result<Exchange> {
    let body = exchange.body.as_text().unwrap_or("");
    let processed = format!("[WASM] {}", body);
    exchange.body = camel_wasm_sdk::Body::Text(processed);

    // Log via host function
    let _ = Host::camel_call("log:debug", format!("Processed: {}", body).into()).await;

    Ok(exchange)
}
```

Build:

```bash
cargo build --target wasm32-wasip2 --release
cp target/wasm32-wasip2/release/plugin.wasm wasm-plugins/
```

### Chaining WASM Plugins

```rust
RouteBuilder::from("direct:chain")
    .to("wasm:plugins/validate.wasm")
    .to("wasm:plugins/transform.wasm")
    .to("wasm:plugins/enrich.wasm")
    .to("log:info")
    .build()?;
```

## Security

### Path Validation

- Absolute paths are rejected
- Paths containing `..` are rejected
- Canonical path must start with base directory
- Prevents directory traversal attacks

### Recursion Guard

- WASM plugins cannot call other WASM plugins via `camel_call()`
- Prevents infinite recursion and stack overflow
- Returns error: `recursive wasm calls not supported`

## Architecture

```
┌─────────────────┐
│  WasmComponent  │
│  (scheme: wasm) │
└────────┬────────┘
         │ creates_endpoint()
         ▼
┌─────────────────┐
│  WasmEndpoint   │
│  (URI resolver) │
└────────┬────────┘
         │ create_producer()
         ▼
┌─────────────────┐
│  WasmProducer   │
│  (Tower Service)│
└────────┬────────┘
         │ poll_ready() -> call()
         ▼
┌─────────────────┐
│  WasmRuntime    │
│  (Wasmtime)     │
└─────────────────┘
         │ call_init_once() / call_process()
         ▼
┌─────────────────┐
│  WasmHostState  │
│  (registry,     │
│   properties,   │
│   call_depth)   │
└─────────────────┘
```

- **WasmComponent**: Component trait implementation, URI scheme `wasm:`, path validation
- **WasmEndpoint**: Resolves URI to WASM module path, creates producer
- **WasmProducer**: Tower Service wrapping WasmRuntime, lazy initialization, error handling
- **WasmRuntime**: Wasmtime engine, linker, component instantiation
- **WasmHostState**: Per-request state with registry, properties, call depth guard

## Host Function Internals

The `WasmHostState` maintains per-invocation state:

```rust
pub struct WasmHostState {
    pub table: ResourceTable,           // Wasmtime resource table
    pub wasi: WasiCtx,                  // WASI system calls
    pub properties: HashMap<String, Value>, // Exchange properties
    pub registry: Arc<Mutex<Registry>>, // Component registry
    pub call_depth: u32,                // Recursion guard (0 = allowed)
}
```

Each request gets a new `WasmHostState` with:
- Exchange properties copied from the incoming `Exchange`
- `call_depth` reset to 0
- Fresh WASI context with stderr inheritance

## Testing

Unit tests verify path validation, recursion guard, and host functions:

```bash
cargo test -p camel-component-wasm
```

Integration tests require a compiled WASM module:

```bash
# Build test plugin
cargo build --target wasm32-wasip2 --release -p example-wasm-plugin
cp target/wasm32-wasip2/release/example_wasm_plugin.wasm tests/fixtures/

# Run integration tests
cargo test -p camel-component-wasm --test integration
```

## Guest SDK

See `crates/camel-wasm-sdk/README.md` for plugin development:

- `#[plugin]` macro for exported functions
- `Exchange` and `Body` types for data access
- `Host` trait for calling host functions

## Documentation

- [API Documentation](https://docs.rs/camel-component-wasm)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

This project is licensed under the same license as rust-camel.

## Contributing

See the [main repository](https://github.com/kennycallado/rust-camel) for contribution guidelines.
