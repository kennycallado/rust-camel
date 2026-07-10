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

### Return-path streaming (guest → host)

A guest can also **return** a `Body::Stream` in its response — the host drains it
lazily into the pipeline (a spawned drain task owns the moved `Store`, drives the
guest's `StreamReader` via `pipe`, and backpressures through a bounded channel
with cancel-on-drop + a no-progress watchdog). No host configuration is needed;
returning a `WasmBody::Stream` from an exported `process`/`invoke` is sufficient.

> **Critical guest pattern — `spawn_local` (avoid deadlock).** The component-model
> `stream<u8>` is a **rendezvous channel with no buffer**: `write_all().await`
> completes only when the host reads. The host registers its stream consumer
> *after* your exported function returns (it needs `&mut Store` inside
> `run_concurrent`). Writing inline before returning **deadlocks** — the write
> waits for the host to read, the host waits for the function to return, the
> function waits for the write. You MUST spawn the writer concurrently and return
> the reader immediately. Enable the `async-spawn` cargo feature on the guest
> crate.

```rust
async fn my_handler(exchange: WasmExchange) -> Result<WasmExchange, WasmError> {
    let (mut writer, reader) = bindings::wit_stream::new::<u8>();
    let (future_writer, future_reader) =
        bindings::wit_future::new::<Result<(), WasmError>>(|| Ok(()));

    wit_bindgen::spawn_local(async move {
        writer.write_all(my_bytes().to_vec()).await.ok();
        drop(writer);                                 // EOF — drop writer FIRST
        let _ = future_writer.write(Ok(())).await;    // terminal resolves LAST (else truncation)
    });

    // Return the reader ends now; the spawned task's writes rendezvous with the
    // host's reads after this function returns.
    Ok(exchange_with_stream_body(reader, future_reader))
}
```

Ordering is load-bearing: resolve the terminal future **after** dropping the
writer, or the host may truncate the stream. To signal a terminal error instead
of clean EOF, write `Err(WasmError::...)` to the future writer after the bytes.

See `examples/wasm-bean-example/guest/src/lib.rs` (`emit_stream_body`) for the
canonical reference.

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

## WASM Source Components (async)

A WASM **source** is a 3rd-party component that acts as an inbound source — a webhook receiver or HTTP listener. Unlike a processor (host-driven `process()` calls), the source guest **owns its consumption loop**: it declares what it needs up front, the host grants an `http-listener` resource, and the guest then drains requests and pushes exchanges into the pipeline.

The host adapter is `WasmSourceConsumer`, which implements the `Consumer` trait. The guest's async `run()` export is driven under `Store::run_concurrent` on a tokio task; its async host imports (`accept-http`, `submit-exchange`) receive an `&Accessor` and use async channel ops. No dedicated OS thread or `spawn_blocking` is needed — the source runs fully on the async runtime.

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
3. **`run(listener) -> result`** — async export. The host hands the listener to the guest, which owns an async loop awaiting `accept-http` / `submit-exchange` until cancelled or stopped. Driven under `Store::run_concurrent` on a tokio task.

### Host capabilities (guest imports)

| Function | Signature | Effect |
|----------|-----------|--------|
| `accept-http` | `async (listener: borrow<http-listener>) -> result<option<http-request>, wasm-error>` | Async; awaits the next inbound HTTP request, yielding back to the host event loop until one arrives (or `none` on cancel). The body is a `stream-body-handle` — the guest reads incrementally via `stream<u8>.read`, removing the old 10 MiB materialization cap. |
| `submit-exchange` | `async (exchange: wasm-exchange) -> result<submit-outcome, wasm-error>` | Async; pushes an exchange into the pipeline. Returns **before full body drain** (fire-and-return) when the body carries a `stream<u8>`. Returns `stopped` if the host is shutting down. See below. |
| `is-cancelled` | `() -> bool` | Sync — a quick peek that must not yield (called in tight guest loops). True when the host has cancelled the run loop. |

The `http-listener` resource itself is host-owned and has no guest-callable methods — it is merely the capability handle passed to `accept-http`.

### Fire-and-return `submit-exchange`

When the body is a `WasmBody::Stream`, `submit-exchange` does NOT wait for the host to drain it before returning:

1. The import extracts the live `StreamReader`/`FutureReader` from the guest's exchange.
2. It builds the native `Exchange` with an empty body placeholder.
3. It registers an `AccessorTask` (via `Accessor::spawn`) on the **same** event loop that drives `run`. This task drains the guest's stream into a bounded chunk channel concurrently with the guest fiber.
4. It attaches a lazy `Body::Stream` (backed by the chunk channel) to the native exchange, sends it into the pipeline, and returns immediately.

The pipeline receives the exchange right away; the body stream drains in the background as the downstream reads. Backpressure applies through the bounded chunk channel. If the guest's stream terminates with an error, the downstream `Body::Stream` reader observes it as a stream error.

> **Why `Accessor::spawn` and not `tokio::spawn`?** The plugin/bean return path uses `spawn_return_drain` which moves the store and opens a fresh `run_concurrent`. The source world's `run` is already inside `run_concurrent` — opening a second one would panic (`check_recursive_run`). `Accessor::spawn` registers the drain on the same event loop, where it progresses alongside the guest fiber.

### Guest `spawn_local` rendezvous contract

The component-model `stream<u8>` is a **rendezvous channel with no buffer**: `write_all().await` completes only when the host performs a matching read. The host registers its drain (via `Accessor::spawn`) *after* `submit-exchange` returns, and the drain only progresses while `run_concurrent` polls the event loop. This creates a required guest pattern:

- The guest MUST `spawn_local` the stream writer **before** calling `submit-exchange`, passing only the reader ends.
- The guest MUST keep `run` alive while the stream drains — the event loop must keep polling so the spawned drain progresses.
- The writer MUST drop the stream writer (EOF) **before** resolving the terminal future (same load-bearing ordering as §"Return-path streaming").

Writing the stream inline and then calling `submit-exchange` **deadlocks**: the write waits for the host to read, the host waits for `submit-exchange` to return, `submit-exchange` waits for the write.

```rust
// Guest: emit streaming body, then submit-exchange
let (mut writer, reader) = wit_stream::new::<u8>();
let (future_writer, future_reader) = wit_future::new::<Result<(), WasmError>>(|| Ok(()));

wit_bindgen::spawn_local(async move {
    writer.write_all(data.to_vec()).await.ok();
    drop(writer);                                // EOF first
    let _ = future_writer.write(Ok(())).await;   // terminal last
});

source_host::submit_exchange(WasmExchange {
    input: WasmMessage {
        body: WasmBody::Stream(StreamBodyHandle {
            r#stream: reader,
            terminal: future_reader, .. }),
        .. },
    .. }).await?;
```

### Concurrency model

`source-plan.concurrency` declares how the guest wants to be drained:

- **`sequential`** — supported. The host drains the guest strictly one request at a time through capacity-1 channels.
- **`concurrent(max)`** — **not implemented**. Rejected at `start()` with an explicit error rather than silently degraded, because degrading `concurrent(N)` to sequential would violate the guest's declared contract.

### No no-progress watchdog

The source `run()` loop legitimately parks in `accept-http` for arbitrary durations between requests. A fixed no-progress timeout would kill idle webhook sources. Existing safeguards cover all failure modes:

- **Epoch interruption** — deadline `1` is a stop tripwire; `stop()` calls `engine.increment_epoch()` which traps a CPU-bound guest loop.
- **Cancel-token select!** — each async import races against the cancel token, unblocking park-on-stop promptly.
- **Route-level supervision** — catches "guest hangs forever" at the route manager level.

### Limitations

- **Sequential mode only** — `concurrent(N)` plans are rejected (see above).
- **HTTP transport only** — the single capability granted is `http-listener`; no Kafka/queue/gRPC source yet.
- **Guest `spawn_local` required for streaming exchanges** — the guest must enable the `async-spawn` feature and use `wit_bindgen::spawn_local` when submitting streaming bodies.

### Architecture: source vs plugin return-path streaming

Both the source world and the plugin/bean return path stream data guest→host, but the mechanics differ:

| Aspect | Plugin/bean return path | Source world |
|--------|------------------------|--------------|
| Drain model | `spawn_return_drain` — moves store, opens fresh `run_concurrent` | `Accessor::spawn` — registers on the same event loop |
| Guest export | Sync `process()` / `invoke()` returning a `WasmBody::Stream` | Async `run()` that stays alive while the stream drains |
| No-progress watchdog | Active (configurable timeout via `no_progress_timeout`) | None (idle sources survive indefinitely) |
| Termination | Watchdog or cancel-token | Cancel-token + epoch tripwire |

For a working end-to-end example, see [`examples/wasm-source-webhook/`](../../examples/wasm-source-webhook/).

## Documentation

- [API Documentation](https://docs.rs/camel-component-wasm)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

This project is licensed under the same license as rust-camel.

## Contributing

See the [main repository](https://github.com/kennycallado/rust-camel) for contribution guidelines.
