# ADR-0014: Unify WASM plugin runtime configuration across all plugin types

- **Status:** Accepted
- **Date:** 2026-06-05
- **Tracking:** bd `rc-zdi`
- **Supersedes:** none
- **Related:** ADR-0001 (data-plane / control-plane split), ADR-0011 (no silent default surprises)

## Context

Until this ADR, the `WasmConfig` struct exposed three knobs — `timeout_secs`,
`max_memory_bytes`, `max_concurrent_calls` — but:

1. **Only the `wasm:` URI scheme (Processor) parsed them from configuration.**
   Bean, AuthorizationPolicy, and SecurityPolicy were all instantiated with
   `WasmConfig::default()` hardcoded at the call site (`camel-cli/main.rs:305`,
   `authorization_policy.rs::build_permission_registry`, all SecurityPolicy
   callers).
2. **`max_memory_bytes` was never enforced for any plugin type**, including
   Processor. The 50 MiB default in `config.rs:14` was documentation only;
   every `WasmHostState` started with `StoreLimits::default()`, which lets a
   guest grow its linear memory to wasmtime's 4 GiB address-space ceiling.

This was discovered during the OSM PBF ingest spike (`rust-camel_GEO-69f`)
when a bean parsing a 180 MB PBF was killed at the 30-second timeout.

## Decision

1. **Introduce a shared `WasmLimitsConfig` type in `camel-config`** with
   `Option<T>` fields for `timeout_secs`, `max_memory`, and
   `max_concurrent_calls`. `None` means "use the runtime default"; the
   defaults are explicitly applied in `WasmConfig::from_limits`, the single
   source of truth.
2. **Embed `WasmLimitsConfig` in `BeanConfig` and `PermissionProviderConfig`.**
   Users tune plugins from `Camel.toml`:
   ```toml
   [default.beans.<name>.limits]
   timeout-secs = 600
   max-memory = 4294967296
   ```
3. **Keep the `wasm:` URI parser for Processor.** Processor endpoints continue
   to accept `?timeout=X&max-memory=Y&max-concurrent-calls=Z`. Internally the
   URI parser produces the same `WasmConfig`; the URI form is the data-plane
   surface, `Camel.toml` is the control-plane surface.
4. **Enforce `max_memory_bytes` via `wasmtime::StoreLimitsBuilder::memory_size`
   in `WasmRuntime::create_host_state`.** Every consumer of `create_host_state`
   (Producer, Bean, AuthorizationPolicy, SecurityPolicy) inherits the limiter.

## Alternatives considered

### Synthesise a fake `wasm:plugin.wasm?timeout=X&max-memory=Y` URI in `camel-cli`

Rejected. **ADR-0001** separates data-plane endpoint concerns (URI parsers,
producers, consumers) from control-plane lifecycle configuration
(`Camel.toml`, plugin manifests). Reusing the URI parser for bean
configuration would breach that boundary and create a confusing dual
interpretation of the same syntax.

### Defer memory enforcement

Rejected by the maintainer. There are no existing clients of the WASM plugin
system today, so the cost of the breaking change (effective ceiling drops
from unbounded to 50 MiB enforced unless raised) is zero now and grows
monotonically with adoption. Shipping a documented safety net that does not
exist is worse than fixing it.

### Add `fuel` and other wasmtime knobs

Out of scope. We can add them later as new `Option<T>` fields on
`WasmLimitsConfig` without breaking anyone.

## Consequences

- **Breaking change for any workload relying on the implicit 4 GiB ceiling.**
  None exist today; documented in the `rc-zdi` body and in release notes.
- **`WasmConfig::default()` still exists** for tests and as the source of
  truth for the defaults, but production callers go through `from_limits` or
  `from_uri`.
- **`WasmHostState::create_host_state` now requires `max_memory_bytes`.** All
  call sites must be updated when adding new WASM consumers.
- **`WasmSecurityPolicy` has no production callers today**; this ADR fixes
  its potential path without adding one.

## Drive-by fix: EpochTicker migrated to a dedicated OS thread

The new behavioral test `timeout_kills_infinite_loop_guest` (added under
Task 9 of the implementation plan) needs the wasmtime epoch to advance
while a malicious guest is spinning inside `call_async`. The pre-existing
`EpochTicker::start` was implemented with `tokio::task::spawn` +
`tokio::time::sleep`, which cannot make progress when a tight CPU loop
inside `call_async` is starving the same tokio runtime — most notably
under single-worker or `current_thread` configurations.

To make the timeout enforced end-to-end (and not just in the test's
re-implementation), `EpochTicker::start` now spawns a dedicated OS thread
with `std::thread::spawn` + `std::thread::sleep`, matching the
"Surrealism approach" already cited in `epoch.rs`. `Drop` was changed
from `handle.abort()` to `handle.join()` so we synchronously know the
thread is no longer touching the engine before `WasmRuntime::Drop`
drops the `Engine` itself.

This change has no public-API impact (`EpochTicker` is internal to
`camel-component-wasm`) but is recorded here because it was discovered
through the new behavioral test for `timeout_secs`.
