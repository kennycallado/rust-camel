# Log-level policy

Per ADR-0012, this component's `error!` / `warn!` sites are categorized as:

## Producer

- **(a) handler-owned** (producer.rs L228): WASM transform execution inside the pipeline (`warn!`). Route
  ErrorHandler owns the ERROR. Downgraded to `warn!` with
  `// log-policy: handler-owned`. No metric call.

## Capability posture

This crate runs operator-installed WASM guests in a Wasmtime sandbox. Operators
trust the plugin they install. The sandbox limits damage from guest defects. It
is not a security boundary for intentionally malicious plugins.

### Camel host functions

Six host functions cross the guest/host boundary in `wit/camel-plugin.wit`.
Their grants depend on the guest world:

| Function | Gate |
|---|---|
| `camel_call`, `camel_poll` | `WasmCapabilities.call_schemes`; an empty allowlist denies every scheme |
| `host_store`, `host_load` | `WasmCapabilities.host_kv`; enabled for processor and bean guests, denied for policy guests |
| `get_property`, `set_property` | Available when the Camel host interface is linked |

Authorization-policy and security-policy guests use
`WasmCapabilities::denied()`. Processor and bean guests use
`WasmCapabilities::from_scheme_list()`. Source guests do not receive Camel host
functions. The host gives them only the `http-listener` resource defined by the
source WIT world.

`StateStore` enforces configurable limits on key count (default 256),
key byte length (default 1024), and value byte length (default 65536).
`set_property_impl` enforces key and value byte limits. Over-limit calls are
rejected.

### WASI surface

The linker registers only `wasi:clocks` and `wasi:random` per ADR-0050.
Filesystem, sockets, CLI, environment, and stdio are absent from the
linker. No world inherits host stdio. Guests use `camel_call` for logging
output per ADR-0050.

## Resource and lifecycle limits

- `validate_wasm_size` rejects an oversized module before compilation.
- `StoreLimitsBuilder` enforces memory, instance, table, and table-element
  limits from ADR-0014.
- Epoch interruption uses a dedicated `EpochTicker` thread. Each guest call
  sets a new epoch deadline.
- `DepthGuard` uses RAII to release the recursion count on return, error, or
  cancellation. It prevents `camel_call` from re-entering the same Store.

## Guest worlds

| World | Lifecycle | Camel capability source |
|---|---|---|
| `plugin` | Host calls `process()` for each Exchange | `from_scheme_list()` |
| `bean` | Host dispatches a selected method | `from_scheme_list()` |
| `authorization-policy` | Host calls `evaluate(exchange)` before pipeline execution | `denied()` |
| `source` | Guest owns the `run(listener)` loop from ADR-0031 | No Camel host functions; host grants `http-listener` |

## Trust direction

ADR-0032 classifies Exchange bodies, headers, and properties as untrusted.
WASM guests receive that data through `get_property`; the sandbox contains the
guest while it processes the data. By contrast, `init_config` sends trusted
operator configuration to the guest. Operators must not place secrets there
unless the guest requires them. Debug output for `init_config` or `StateStore`
must not expose secret values.

## Dependency boundary

The component uses Wasmtime types directly across its host implementation.
Generated component-model traits and Store state make a project-owned adapter
impractical. This differs from the provider adapter in ADR-0020. The accepted
cost is broad source churn when Wasmtime changes its component-model API.
