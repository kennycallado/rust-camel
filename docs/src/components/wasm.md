# WASM

The WASM component runs operator-installed WebAssembly plugins in a Wasmtime sandbox. One crate covers the `plugin`, `bean`, `authorization-policy`, and `source` worlds. Each world receives a different capability grant. The sandbox limits damage from guest defects. The sandbox is not a security boundary for intentionally malicious plugins.

The wasm-example wires a timer-driven producer that calls an echo plugin, then logs the result:

```rust,ignore
{{#include ../../../examples/wasm-example/src/main.rs:wasm-producer-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: wasm-example
    from: "timer:tick?period=1000&repeatCount=3"
    steps:
      - to: "wasm:echo.wasm?timeout=5&max-memory=10485760"
      - to: "log:info"
```

The Rust example resolves `echo.wasm` from the example `fixtures/` directory. The YAML form takes the plugin path from the URI. Substitute your own base directory at component registration. The same `wasm:` scheme serves a `source` Endpoint that runs an inbound guest loop driven by an `http-listener` capability. The `examples/wasm-source-webhook/` example shows that direction.

</details>

## URI

```
wasm:<path/to/module.wasm>[?<param>=<value>...]
```

The path must be relative. Absolute paths and `..` segments are rejected. The path resolves against the base directory passed to `WasmComponent::new`.

| Parameter | Type | Default | Description |
| --- | --- | --- | --- |
| `timeout` | seconds | 30 | Per-call wall-clock deadline. Enforced by epoch interruption. |
| `max-memory` | bytes | 52428800 | Maximum linear memory the guest can allocate. |
| `max-concurrent-calls` | integer | 4 | Maximum concurrent `call_process` executions per producer. |
| `max-wasm-size` | bytes | 10485760 | Reject modules larger than this at load time. |
| `allow-call` | string | empty | Comma-separated URI schemes the guest may call via `camel_call` or `camel_poll`. |
| `max-stream-bytes` | bytes | materializer default | Per-stream byte cap for the streaming body bridge. |
| `max-instances` | integer | 10000 | Maximum core instances per store. |
| `max-tables` | integer | 10000 | Maximum tables per store. |
| `max-table-elements` | integer | unlimited | Maximum table elements when set. |
| `bind` | address | `0.0.0.0:8080` | Source world only. Bind address for the granted HTTP listener. |
| `path` | string | all paths | Source world only. URL path filter the listener accepts. |

Zero or invalid values fall back to the runtime default. The component does not hide defaults. ADR-0011 names the rule.

## Worlds

The component ships four WIT worlds. Each world grants a different capability set.

| World | Host calls | Capability source | Camel host functions |
| --- | --- | --- | --- |
| `plugin` | `process()` per Exchange | `from_scheme_list()` | `camel_call`, `camel_poll`, `host_store`, `host_load`, `get_property`, `set_property` |
| `bean` | `invoke()` for a chosen method | `from_scheme_list()` | Same as `plugin` |
| `authorization-policy` | `evaluate(exchange)` before pipeline | `denied()` | `get_property`, `set_property` only |
| `source` | guest owns `run(listener)` loop | none (host grants `http-listener`) | none |

The `authorization-policy` world also backs the `SecurityPolicy` host type. The capability grant stays `denied()` for both roles.

The `source` world grants no Camel host functions. The host binds the TCP listener and hands the guest an `http-listener` resource handle. The guest then drives `accept-http` and `submit-exchange` on its own loop under `Store::run_concurrent`. ADR-0031 defines the source lifecycle.

## Capability model

ADR-0050 defines the target capability posture for the WASI surface. The current linker registers the full WASI 0.2 surface. The runtime context denies filesystem preopens, environment variables, sockets, and IP-name lookup. Clocks and random remain usable. Processor, bean, and policy guests inherit host stderr. Source guests do not.

ADR-0050 selects per-world selective WASI registration as the target. The migration is in progress. The runtime grants today match the target grants. The linker surface is broader until the migration lands. A Wasmtime upgrade or a `WasiCtxBuilder` change must not widen capabilities by accident (ADR-0050).

The Camel host function grants depend on the world. The `WasmCapabilities` struct carries two fields.

| Field | Meaning |
| --- | --- |
| `call_schemes` | URI schemes the guest may call. Empty set denies all schemes. |
| `host_kv` | Whether `host_store` and `host_load` are available. |

Authorization and security policy guests use `WasmCapabilities::denied()`. Processor and bean guests use `WasmCapabilities::from_scheme_list(schemes)`. `from_scheme_list` sets `host_kv` to true. Policy guests do not get host storage.

The `allow-call` URI parameter and the `allow-call-schemes` Camel.toml field both flow into `call_schemes`. An empty list denies every scheme. The component fails closed (ADR-0033).

## Configuration via Camel.toml

Processor plugins read limits from the URI query string. Bean, authorization-policy, and security-policy plugins read limits from a `[limits]` block in `Camel.toml`. The block type is `WasmLimitsConfig`. ADR-0014 unifies these knobs across plugin kinds.

```toml
[default.beans.my-bean]
plugin = "my-bean"

[default.beans.my-bean.limits]
timeout-secs = 600
max-memory = 4294967296
max-concurrent-calls = 1
```

```toml
[security.permissions.providers.my-policy]
provider = "wasm"
path = "plugins/authz.wasm"

[security.permissions.providers.my-policy.limits]
timeout-secs = 5
max-memory = 10485760
```

| Field | Default | Description |
| --- | --- | --- |
| `timeout-secs` | 30 | Per-call wall-clock deadline. |
| `max-memory` | 52428800 | Maximum linear memory. |
| `max-concurrent-calls` | 4 | Maximum concurrent calls. |
| `max-wasm-size` | 10485760 | Maximum module size. |
| `allow-call-schemes` | empty | Comma-separated schemes for `camel_call`. |
| `max-stream-bytes` | materializer default | Per-stream byte cap. |
| `max-instances` | 10000 | Maximum core instances per store. |
| `max-tables` | 10000 | Maximum tables per store. |
| `max-table-elements` | unlimited | Maximum table elements. |

All fields are optional. `None` means use the runtime default. `WasmConfig::from_limits` is the single source of truth for defaults. No silent fallback lie exists elsewhere (ADR-0011).

## Security

The component trusts the plugin the operator installs. The sandbox limits damage from guest defects. The sandbox is not a security boundary for intentionally malicious plugins. ADR-0032 classifies Exchange data as untrusted. The sandbox contains the guest while it processes that data.

Path validation rejects absolute paths. It rejects `..` segments. The canonical path must start with the base directory. The `DepthGuard` prevents `camel_call` from re-entering the same Store. The guard releases the recursion count on return, error, or cancellation.

`init_config` sends trusted operator configuration to the guest. Operators must not place secrets there unless the guest needs them. Debug output for `init_config` or `StateStore` must not expose secret values.

A known gap remains: `set_property` and host-side `StateStore` allocations do not have independent size limits. Wasmtime store limits do not account for these host allocations. The finding is `F-camel-component-wasm-I4`.

## Error handling

WASM transform execution inside the pipeline logs at `warn!`. The route `ErrorHandler` owns the error. The producer downgrades the level to `warn!` with the `// log-policy: handler-owned` marker. ADR-0012 classifies this as category (a).

| Variant | When raised |
| --- | --- |
| `WasmError::Timeout` | Epoch deadline exceeded. |
| `WasmError::OutOfMemory` | Guest exceeded memory limit. |
| `WasmError::Trap` | Guest hit unreachable, stack overflow, or other trap. |
| `WasmError::GuestPanic` | Guest panicked with a message. |
| `WasmError::Unhealthy` | Plugin failed health check. |

After a `Timeout`, `Trap`, or `OutOfMemory`, the plugin runtime resets on the next call. The route does not need manual intervention.

**Reference**: [WASM crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-wasm/CONTEXT.md) — [ADR-0050: WASM sandbox capability posture](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0050-wasm-sandbox-capability-posture.md). Example source: [`examples/wasm-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/wasm-example).
