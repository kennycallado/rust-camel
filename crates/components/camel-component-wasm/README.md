# camel-component-wasm

WASM plugin component for rust-camel — loads and executes WASM modules as route processors using the Component Model.

## Features

- **WASM Component Model**: Loads WASM modules compiled for `wasm32-wasip2` target
- **Wasmtime v31**: Latest runtime with async support and component-model features
- **Host Functions**: `camel_call()`, `get_property()`, `set_property()`, `host_store()`, `host_load()` for guest-host communication
- **URI-Based Routing**: `wasm:path/to/module.wasm` format for easy integration
- **Path Validation**: Prevents directory traversal and escapes from project root
- **Recursion Guard**: Blocks nested WASM calls to prevent infinite loops
- **Tower Service**: Implements `Service<Exchange>` for async processing
- **Exchange Properties**: Per-request properties accessible from WASM via host functions
- **Persistent State**: `host_store`/`host_load` for per-endpoint state that survives across `process()` calls
- **Streaming Bodies**: `Body::Stream` crosses the WASM boundary via WASI 0.3 `stream<u8>` without materialization
- **Production Hardening**: Epoch-based timeouts, memory limits, structured trap classification, automatic recovery

## Installation

Add to `Cargo.toml` (workspace dependency):

```toml
[dependencies]
camel-component-wasm.workspace = true
```

## URI Format

```
wasm:path/to/module.wasm[?timeout=<secs>&max-memory=<bytes>]
```

- Must be relative path (no leading `/`)
- No `..` components allowed
- Resolved against configured base directory

### Query Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `timeout` | `u64` (seconds) | `30` | Max wall-clock time per guest call. Enforced via epoch interruption. |
| `max-memory` | `u64` (bytes) | `52428800` (50 MB) | Max linear memory the guest can allocate. |

Zero or invalid values are silently ignored and the default is used.

### Examples

```
wasm:plugins/transform.wasm
wasm:plugins/transform.wasm?timeout=5
wasm:plugins/transform.wasm?timeout=10&max-memory=10485760
```

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

### `host_store(key: String, value: String) -> Result<()>`

Stores a key-value pair that persists across `process()` calls for this route endpoint.

```rust
use camel_wasm_sdk::state_helpers;

// Store config loaded in init()
state_helpers::store("api-key", "secret-123")?;

// Store structured data as JSON
state_helpers::store_json("config", &my_config)?;
```

### `host_load(key: String) -> Result<Option<String>>`

Loads a previously stored value. Returns `None` if the key has not been stored.

```rust
// Load a string value
let api_key = state_helpers::load("api-key")?;

// Load and deserialize JSON
let config: Option<MyConfig> = state_helpers::load_json("config")?;
```

> **Scope:** State is scoped per route endpoint. Different routes using the same `.wasm` file maintain independent state stores.

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

## Production Configuration

Phase 4 hardening adds epoch-based timeouts, memory limits, and structured trap classification to every plugin call.

### Timeout enforcement

Every guest call (`init` and `process`) sets an epoch deadline before invocation. A background thread (`EpochTicker`) increments the wasmtime engine epoch every 10 ms. If the deadline is exceeded, the call is interrupted and returns `WasmError::Timeout`.

```rust
// 5-second timeout
let uri = "wasm:plugins/slow.wasm?timeout=5";
```

### Memory limits

`StoreLimits` is installed in every `Store`. If the guest exceeds `max-memory`, the next allocation fails and returns `WasmError::OutOfMemory`.

```rust
// 10 MB limit
let uri = "wasm:plugins/heavy.wasm?max-memory=10485760";
```

### Configuring Bean and AuthorizationPolicy plugins via `Camel.toml`

Processor plugins are configured via the `wasm:` URI query string (above). For **Bean** and **AuthorizationPolicy** plugins, the same knobs are exposed through a `[limits]` block in `Camel.toml`:

```toml
# Bean plugin
[default.beans.my-bean]
plugin = "my-bean"

[default.beans.my-bean.limits]
timeout-secs = 600          # optional, defaults to 30
max-memory = 4294967296     # optional, defaults to 52428800 (50 MiB)
max-concurrent-calls = 4    # optional, defaults to 4 (bean: not enforced today)
```

```toml
# AuthorizationPolicy plugin (WASM provider)
[security.permissions.providers.my-policy]
provider = "wasm"
path = "plugins/authz.wasm"

[security.permissions.providers.my-policy.limits]
timeout-secs = 5
max-memory = 10485760
```

All three fields are optional. **`None` means "use the runtime default"** — no silent fallback lie (per ADR-0011). The defaults are applied explicitly in `WasmConfig::from_limits`, the single source of truth.

For **SecurityPolicy**, ADR-0014 documents why no `Camel.toml` path is exposed (no production callers today). The runtime still honours the default 50 MiB cap through the shared `WasmRuntime::create_host_state`.

See [`docs/adr/0014-wasm-plugin-config-unification.md`](../../docs/adr/0014-wasm-plugin-config-unification.md) for the full design rationale.

### Error variants

| Variant | When raised |
|---------|-------------|
| `WasmError::Timeout { plugin, timeout_secs }` | Epoch deadline exceeded |
| `WasmError::OutOfMemory { plugin, max_memory_bytes }` | Guest exceeded memory limit |
| `WasmError::Trap { plugin, reason }` | Guest hit unreachable/stack-overflow/other trap |
| `WasmError::GuestPanic(msg)` | Guest panicked with a message |
| `WasmError::Unhealthy(msg)` | Plugin failed health check |

### Recovery

After a `Timeout`, `Trap`, or `OutOfMemory`, the plugin runtime is automatically reset on the next call. No manual intervention required.

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
│   state_store,  │
│   call_depth)   │
└─────────────────┘
```

- **WasmComponent**: Component trait implementation, URI scheme `wasm:`, path validation
- **WasmEndpoint**: Resolves URI to WASM module path, creates producer
- **WasmProducer**: Tower Service wrapping WasmRuntime, lazy initialization, error handling
- **WasmRuntime**: Wasmtime engine, linker, component instantiation
- **WasmHostState**: Per-request state with registry, properties, call depth guard
- **WasmSourceConsumer**: `Consumer` impl for the `source` world; bridges the guest's blocking run loop to the async pipeline via bounded channels (see [WASM Source Components](#wasm-source-components))

## Host Function Internals

The `WasmHostState` maintains per-invocation state:

```rust
pub struct WasmHostState {
    pub table: ResourceTable,           // Wasmtime resource table
    pub wasi: WasiCtx,                  // WASI system calls
    pub properties: HashMap<String, Value>, // Exchange properties
    pub registry: Arc<Mutex<Registry>>, // Component registry
    pub call_depth: Arc<AtomicUsize>,   // Recursion guard (0 = allowed)
    pub state_store: StateStore,        // Per-endpoint persistent state
    pub limits: StoreLimits,            // Memory allocation limits
    pub capabilities: WasmCapabilities, // Feature flags (streaming, etc.)
}
```

Each request gets a new `WasmHostState` with:
- Exchange properties copied from the incoming `Exchange`
- `call_depth` reset to 0 (`AtomicUsize::new(0)`)
- Fresh WASI context with stderr inheritance

## Streaming Bodies

The WASM component supports `Body::Stream` — streaming bodies cross the WASM boundary via WASI 0.3 `stream<u8>` without materialization in WASM linear memory.

### How it works

1. Host extracts the `BoxStream` from `Body::Stream` before `run_concurrent`
2. Inside `run_concurrent`, `assemble_stream_body` creates a `StreamReader`
3. The guest reads chunks via `StreamReader::read(buf)` in a loop
4. A terminal `future<result<_, wasm-error>>` signals completion or error
5. A no-progress watchdog aborts stalled streams after a configurable timeout

### Resource limits

- `max_bytes`: per-stream byte limit. Overflow → terminal error + stream drop.
- `CancellationToken`: host can abort the stream mid-flight.
- `no_progress_timeout`: watchdog fires if no chunks are produced within the window.

### Guest migration

See `MIGRATION-ASYNC.md` for instructions on migrating sync guests to async exports.

### Example

See `examples/wasm-streaming-plugin/` for a byte-counter guest that reads a streaming body without materializing it.

## Testing

Unit tests verify path validation, recursion guard, host functions, state persistence, hardening (epoch timeout, memory limits, trap recovery), and performance benchmarks:

```bash
cargo test -p camel-component-wasm
# 81 tests: 50 unit + 10 hardening + 14 integration + 6 state + 1 perf
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

## Bean Support

The WASM component also supports **bean plugins** — multi-method WASM components that expose several callable methods from a single module.

### `WasmBean` Adapter

`WasmBean` is the host-side adapter that loads a WASM bean module and dispatches method calls to the correct guest function. It uses the `bean` WIT world (distinct from the `processor` world) to communicate with the guest.

### Configuration

Register beans in `Camel.toml`:

```toml
[beans.auth]
plugin = "my-auth-bean"
```

Each bean entry creates an isolated WASM instance. Methods are invoked by name from YAML DSL or Rust routes:

```yaml
routes:
  - id: "auth-route"
    from: "direct:auth"
    steps:
      - bean:
          name: "auth"
          method: "validate"
```

### Building Bean Plugins

Use the SDK's `BeanPlugin` trait and `export_bean!` macro:

```rust
use camel_wasm_sdk::{export_bean, BeanPlugin, WasmExchange, WasmError};

struct AuthBean;

impl BeanPlugin for AuthBean {
    fn methods() -> Vec<&'static str> {
        vec!["validate", "refresh"]
    }

    fn invoke(method: &str, exchange: WasmExchange) -> Result<WasmExchange, WasmError> {
        match method {
            "validate" => Ok(exchange),
            "refresh" => Ok(exchange),
            _ => Err(WasmError::ProcessorError(format!("unknown method: {method}"))),
        }
    }
}

export_bean!(AuthBean);
```

## Security Policy Support

The WASM component can serve as a security policy backend, delegating authorization decisions to a guest module. Two host types are provided, both backed by the shared `WasmPluginContext`.

### Exchange-Level: `WasmSecurityPolicy`

Implements the `SecurityPolicy` trait. Called during route processing with the full `Exchange` (including `camel.auth.*` properties populated by the authentication layer). The guest module's `evaluate()` function returns:

- `Ok(None)` -- access granted, the exchange continues
- `Ok(Some(reason))` -- access denied with a reason string
- `Err(...)` -- processing error, propagated as `CamelError`

```rust
use camel_component_wasm::security_policy::WasmSecurityPolicy;

let policy = WasmSecurityPolicy::new(
    "plugins/auth-policy.wasm",
    WasmConfig::default(),
    registry,
    init_config,
).await?;
```

### Permission-Level: `WasmAuthorizationPolicyEvaluator`

Implements the `PermissionEvaluator` trait. Called by the `PermissionEvaluatorRegistry` with a `PermissionRequest` (principal, resource, action, scopes). The host builds a synthetic `Exchange` from the request and delegates to the same guest `evaluate()` function. Returns `PermissionDecision::Granted` or `PermissionDecision::Denied { reason }`.

Registered as a permission provider in `Camel.toml`:

```toml
[security.permission-providers.rbac]
provider = "wasm"
path = "plugins/rbac-policy.wasm"

[security.permission-providers.rbac.config]
default-role = "viewer"
```

### Shared Context: `WasmPluginContext`

Both `WasmSecurityPolicy` and `WasmAuthorizationPolicyEvaluator` are thin wrappers around `WasmPluginContext`, which owns:

- `Engine` and `Linker` -- shared WASM runtime
- `Component` -- the compiled guest module
- `Registry` -- for host function callbacks
- `StateStore` -- persistent key-value state
- `EpochTicker` -- epoch-based timeout enforcement

Each call to `evaluate()` creates a fresh `Store` from this shared context, so concurrent evaluations are isolated.

## Authorization Policy WIT World

Guest modules implementing security policies must target the `authorization-policy` world defined in `wit/camel-plugin.wit`. This world imports the same host functions as the `plugin` world and exports two functions:

```wit
world authorization-policy {
    import host;

    use types.{wasm-exchange, wasm-error};

    /// Evaluate the exchange and return an authorization decision.
    /// None = Granted, Some(reason) = Denied.
    export evaluate: func(exchange: wasm-exchange) -> result<option<string>, wasm-error>;

    /// Initialization hook with config from registration.
    export init: func(config: list<tuple<string, string>>) -> result<_, string>;
}
```

Key differences from the `plugin` world:

- `evaluate` replaces `process` -- returns `option<string>` (deny reason) instead of a full exchange
- `init` receives key-value config from the provider registration in `Camel.toml`
- Guest reads auth context via `get-property("camel.auth.roles")`, `get-property("camel.auth.principal")`, etc.

### Route Example

```yaml
routes:
  - id: "secured-api"
    from: "http:0.0.0.0:8080/api"
    security_policy:
      wasm: "corp-auth"
    steps:
      - to: "log:secured"
```

The `security_policy: wasm` form references a `WasmSecurityPolicy` registered in the `SecurityPolicyRegistry`. Policies are registered from `Camel.toml`:

```toml
[security.policies.wasm.corp-auth]
path = "fixtures/role-check.wasm"

[security.policies.wasm.corp-auth.limits]
timeout-secs = 30
max-memory = 52428800

[security.policies.wasm.corp-auth.config]
ldap_url = "ldap://corp"
```

The guest module's `init()` function receives the `[<name>.config]` pairs as sorted `(String, String)` arguments at instantiation time.

> **Migration** — Previously the YAML form accepted a `config:` block per-route, which was silently dropped. As of ADR-0014 §4 closure (bd rc-0te), per-route `config` is rejected with a hard error. Move `config:` keys to `[security.policies.wasm.<name>.config]` in Camel.toml.

## WASM Source Components

A WASM **source** is a 3rd-party component that acts as an inbound source — a webhook receiver or HTTP listener. Unlike a processor (host-driven `process()` calls), the source guest **owns its consumption loop**: it declares what it needs up front, the host grants an `http-listener` resource, and the guest then drains requests and pushes exchanges into the pipeline.

The host adapter is `WasmSourceConsumer`, which implements the `Consumer` trait and bridges the guest's synchronous `run()` loop to the async pipeline via capacity-1 tokio channels.

### URI Format

```
wasm:<module>[?bind=<addr>&path=<path>&timeout=<secs>&max-memory=<bytes>]
```

In addition to the standard processor query parameters, sources accept:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `bind` | socket address | `0.0.0.0:8080` | Address the host-granted HTTP listener binds. Surfaced as a `source-config` key to the guest. |
| `path` | string | (all paths) | URL path filter the listener accepts (e.g. `/webhook`). |

`bind` and `path` are passed into the guest's `configure()` as `(key, value)` pairs; the guest reflects them back inside its `source-plan`. The TCP listener is bound synchronously during `start()` so a bind failure (port in use, permission denied) surfaces as a start error rather than a silent background warning.

```
wasm:wasm-source-webhook-guest.wasm?bind=0.0.0.0:8080&path=/webhook
wasm:plugins/my-source.wasm?bind=127.0.0.1:9090&timeout=30
```

### Lifecycle: resource negotiation

The `source` world uses a negotiate-then-run handshake:

1. **`configure(config) -> source-plan`** — the host passes the URI query params (plus any registered config). The guest returns a `source-plan` declaring the capability it needs and its concurrency model.
2. **Host grants the capability** — the plan must request exactly one `http-listener` capability. The host creates an `http-listener` resource handle and binds the TCP listener.
3. **`run(listener) -> result`** — the host hands the listener to the guest, which owns a blocking loop calling the host imports below until cancelled or stopped.

### Host capabilities (guest imports)

| Function | Signature | Effect |
|----------|-----------|--------|
| `accept-http` | `(listener: borrow<http-listener>) -> result<option<http-request>, wasm-error>` | Blocking; returns the next inbound HTTP request, or `none` when cancelled (channel closed). |
| `submit-exchange` | `(exchange: wasm-exchange) -> result<submit-outcome, wasm-error>` | Pushes an exchange into the pipeline. Blocks on backpressure; returns `stopped` if the host is shutting down. |
| `is-cancelled` | `() -> bool` | True when the host has cancelled the run loop (e.g. route shutdown). |

The `http-listener` resource itself is host-owned and has no guest-callable methods — it is merely the capability handle passed to `accept-http`.

### Concurrency model

`source-plan.concurrency` declares how the guest wants to be drained:

- **`sequential`** — supported. The host drains the guest strictly one request at a time through capacity-1 channels.
- **`concurrent(max)`** — **not implemented**. Rejected at `start()` with an explicit error rather than silently degraded, because degrading `concurrent(N)` to sequential would violate the guest's declared contract.

### Limitations

- **Sequential mode only** — `concurrent(N)` plans are rejected (see above).
- **HTTP transport only** — the single capability granted is `http-listener`; there is no Kafka/queue/gRPC source yet.
- **Synchronous bindings (WASI 0.2)** — the guest targets `wasm32-wasip2` and its host imports block on tokio channels, so the guest `run()` loop runs on a dedicated OS thread (`spawn_blocking`), off the async runtime.

For a working end-to-end example, see [`examples/wasm-source-webhook/`](../../examples/wasm-source-webhook/).

## Documentation

- [API Documentation](https://docs.rs/camel-component-wasm)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

This project is licensed under the same license as rust-camel.

## Contributing

See the [main repository](https://github.com/kennycallado/rust-camel) for contribution guidelines.
