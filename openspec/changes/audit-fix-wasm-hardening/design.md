# Design: audit-fix-wasm-hardening

## Context

ADR-0050 (Accepted) declares the target posture for the WASM sandbox:
selective per-world WASI registration (Option B), no `inherit_stderr`, and
bounded host-side allocations. The code still registers the full WASI p2
surface, inherits stderr in three of four worlds, and does not bound the
`StateStore` HashMap. This change closes those three gaps.

All work is in `camel-component-wasm`.

## Decision 1 — StateStore + properties bounds (rc-cgc8)

### Current code

`StateStore::store` (`state_store.rs`) inserts any `key: &str`, `value: &str`
pair without checking key length, value length, or entry count.
`WasmHostState::set_property_impl` (`host_functions.rs:525`) inserts into the
`properties: HashMap<String, Value>` without bounds. Wasmtime `StoreLimits`
covers guest linear memory, instances, and tables — it does not cover these
host-side HashMaps. A cooperating guest can grow them without limit.

### Fix shape

Introduce three configurable limits in `WasmConfig`:

| Field | Default | Rationale |
|-------|---------|-----------|
| `max_kv_entries: usize` | 256 | Per-route store; 256 keys covers typical plugin state |
| `max_key_bytes: usize` | 1024 | Keys are identifiers; 1 KiB is generous |
| `max_value_bytes: usize` | 65536 | Values may carry small JSON; 64 KiB matches HTTP header budget |

These defaults follow ADR-0033 (fail-closed: tight defaults, operator raises).

`StateStore::new()` becomes `StateStore::with_limits(entries, key_bytes, value_bytes)`.
The existing `StateStore::new()` is kept as a convenience that uses defaults (for
tests and backward compatibility).

`StateStore::store` checks (in order):
1. Key byte length ≤ `max_key_bytes`
2. Value byte length ≤ `max_value_bytes`
3. Entry count < `max_kv_entries` (if key is new — updates to existing keys do
   not increment count)

Each check returns `Err(format!(...))` with a message naming the limit exceeded.

`set_property_impl` checks key byte length and value byte length before
insertion. `WasmHostState` currently has no config or limit fields; two new
fields are added to carry the bounds: `max_property_key_bytes: usize` and
`max_property_value_bytes: usize`. These are populated by `create_host_state`
from the `WasmConfig` values and read by `set_property_impl` via
`self.max_property_key_bytes` / `self.max_property_value_bytes`. All
`WasmHostState` struct literals (runtime.rs production + test, host_functions.rs
test, stream_bridge.rs test × 2) must be updated.

The properties HashMap is a per-invocation ephemeral map cloned from
exchange properties and discarded when the guest call returns. Its growth is
already bounded by the guest's memory and epoch limits within a single call,
so a persistent entry-count cap is unnecessary.

`WasmConfig::from_uri` and `WasmConfig::from_limits` parse the three new fields
from URI query params (`?max-kv-entries=N`, `?max-key-bytes=N`,
`?max-value-bytes=N`) and `Camel.toml` (`[wasm.limits]`). Default values apply
when absent.

### Wiring

`WasmRuntime::new` (`runtime.rs`) receives `WasmConfig` and passes the three
limits to `StateStore::with_limits` when constructing the store. The store is
then passed to `create_host_state`.

`WasmPluginContext::new` (`wasm_plugin_context.rs`) does the same.

`SourceConsumer::start` (`source_consumer.rs`) does not use `StateStore` —
source guests do not receive `host_store`/`set_property`. No wiring needed.

## Decision 2 — Selective WASI registration (rc-466y)

### Current code

Three call sites register the full WASI p2 surface:

1. `runtime.rs:101` — `wasmtime_wasi::p2::add_to_linker_async(&mut linker)`
2. `wasm_plugin_context.rs:75` — same
3. `source_host.rs:459` — `wasmtime_wasi::p2::add_to_linker_async(linker)`

The `WasiCtxBuilder::new()` context already denies filesystem preopens,
environment variables, socket ports, and IP-name lookup. But the linker exposes
the interface surface — guests that import filesystem or sockets will link
successfully and fail only at runtime when the capability is absent. ADR-0050
Option B requires selective registration so guests with disallowed imports fail
at link time.

### Fix shape

Create a helper function in `runtime.rs`:

```rust
use wasmtime_wasi::clocks::WasiClocks;
use wasmtime_wasi::random::WasiRandom;
use wasmtime_wasi::p2::bindings::{clocks, random};

fn register_minimal_wasi<T: wasmtime_wasi::WasiView>(
    linker: &mut wasmtime::component::Linker<T>,
) -> Result<(), wasmtime::Error> {
    clocks::wall_clock::add_to_linker::<T, WasiClocks>(linker, T::clocks)?;
    clocks::monotonic_clock::add_to_linker::<T, WasiClocks>(linker, T::clocks)?;
    random::random::add_to_linker::<T, WasiRandom>(linker, T::random)?;
    random::insecure::add_to_linker::<T, WasiRandom>(linker, T::random)?;
    random::insecure_seed::add_to_linker::<T, WasiRandom>(linker, T::random)?;
    Ok(())
}
```

This registers only `wasi:clocks` (monotonic and wall clocks) and `wasi:random`
(all three interfaces: `random`, `insecure`, `insecure_seed`). Filesystem,
sockets, CLI, environment, and stdio are absent from the linker. Verified
against wasmtime-wasi 46.0.2 source.

Replace all three `add_to_linker_async` call sites with `register_minimal_wasi`.

`SourceHostState` already implements `WasiView`, so the generic constraint
satisfies both `WasmHostState` and `SourceHostState`.

Note: `WasiView` provides `clocks()` and `random()` accessors via the trait.
Both `WasmHostState` and `SourceHostState` already implement `WasiView`.

## Decision 3 — Remove inherit_stderr (rc-dzd7)

### Current code

`WasiCtxBuilder::new().inherit_stderr().build()` appears in:
1. `runtime.rs:154` — `create_host_state` (processor/bean/policy) — production
2. `runtime.rs:540` — test helper `test_wasm_host_state_creation`
3. `host_functions.rs:657` — test helper
4. `stream_bridge.rs:449` — test helper
5. `stream_bridge.rs:569` — test helper

Only site 1 is a production world. Sites 2–5 are `#[cfg(test)]` helpers removed
for consistency so that `CONTEXT.md`'s "no world inherits stdio" claim is
accurate in all build configurations.

`source_consumer.rs:99` — `WasiCtxBuilder::new().build()` (source world, no
inherit_stderr already).

ADR-0050 rule 5: "El host no llamará a `inherit_stderr()`."

### Fix shape

Remove `.inherit_stderr()` from all five sites listed above. The
`WasiCtxBuilder::new()` context already has no stdout, no stderr, no preopens.
After this change, all four worlds have consistent WASI contexts: no stdio
inheritance.

Guests that wrote to stderr (e.g., `eprintln!` in Rust guests) will see the output
silently dropped instead of appearing in the host's stderr. This is the intended
consequence — logging goes through `camel_call("log:...", ...)`.

## Decision 4 — CONTEXT.md update

Update `CONTEXT.md` for `camel-component-wasm`:
- WASI surface section: change from "full p2 surface registered" to "selective
  clocks+random registration per ADR-0050"
- Capability posture: update the "host functions" table to note bounded
  StateStore and properties
- Guest worlds: remove the stderr inconsistency note, state no world inherits
  stdio

## Phases

Single phase; tasks are ordered W1 → W2 → W3 → W4 (W2 consumes W1's WasmConfig
fields for `set_property_impl` bounds).
