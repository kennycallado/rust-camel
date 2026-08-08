# Proposal: audit-fix-wasm-hardening

## Why

Three audit findings converge on a single unmet policy: ADR-0050 (WASM sandbox
capability posture) declared selective WASI registration and no `inherit_stderr`
as the target, but the code still registers the full WASI p2 surface and inherits
host stderr in processor/bean/policy worlds. Separately, the host-side
`StateStore` and properties HashMap accept unbounded key+value sizes and counts
(rc-cgc8, P1) — Wasmtime `StoreLimits` does not cover these host allocations,
leaving a DoS vector that requires only guest cooperation.

## What Changes

**Included** (all in `camel-component-wasm`):

1. **rc-cgc8 — StateStore + properties bounds**: configurable limits on key
   count, key byte length, and value byte length. Defaults: 256 entries, 1024
   byte keys, 64 KiB values. Enforced in `StateStore::store` and
   `set_property_impl`. Over-limit returns `WasmError::ProcessorError`.
2. **rc-466y — Selective WASI registration**: replace
   `wasmtime_wasi::p2::add_to_linker_async` (full surface) with selective
   registration of `wasi:clocks` and `wasi:random` only, at all three call sites
   (`runtime.rs`, `wasm_plugin_context.rs`, `source_host.rs`).
3. **rc-dzd7 — Remove `inherit_stderr`**: delete `.inherit_stderr()` from
   `create_host_state`, `source_consumer`, and test helpers. No world inherits
   host stderr. Guests use `camel_call` for logging per ADR-0050 rule 5.

**Excluded**: ADR-0050 itself (already Accepted), new WIT interface changes,
`WasmCapabilities` restructuring.

## Acceptance criteria

- `StateStore::store` rejects keys exceeding the configured byte limit, values
  exceeding the configured byte limit, and entry counts exceeding the configured
  maximum. All three paths return `Err`.
- `set_property_impl` rejects oversized keys and values before insertion.
- No call site uses `wasmtime_wasi::p2::add_to_linker_async`; all three sites
  register only clocks and random via individual interface functions.
- No call site uses `inherit_stderr()`.
- `cargo test -p camel-component-wasm --lib` passes with new unit tests for every
  bound check and existing tests unchanged.
- `cargo clippy -p camel-component-wasm -- -D warnings` passes.

## Risk budget

Guests that import filesystem, sockets, CLI, environment, or stdio WASI
interfaces will fail to instantiate after this change. This is the intended
consequence of ADR-0050 Option B. The risk is that an operator's existing
.wasm plugin depends on stderr output — mitigated by the `camel_call` logging
path which remains available to processor/bean/policy worlds.

bd: rc-cgc8 (P1), rc-466y, rc-dzd7
