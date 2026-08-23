# wasm-sandbox-hardening Specification

## Purpose
TBD - created by archiving change audit-fix-wasm-hardening. Update Purpose after archive.
## Requirements
### Requirement: Host-side key-value store MUST enforce bounded allocations

The WASM host-side `StateStore` and `WasmHostState::properties` MUST enforce
configurable limits on key count, key byte length, and value byte length. These
limits are independent of Wasmtime `StoreLimits`, which covers guest linear
memory, instances, and tables but not host-side HashMap allocations.

#### Scenario: StateStore rejects oversized key

- **WHEN** a guest calls `host_store` with a key exceeding the configured
  `max_key_bytes` limit
- **THEN** `StateStore::store` returns `Err` with a message naming the limit
- **AND** the key-value pair is not inserted into the HashMap

#### Scenario: StateStore rejects oversized value

- **WHEN** a guest calls `host_store` with a value exceeding the configured
  `max_value_bytes` limit
- **THEN** `StateStore::store` returns `Err` with a message naming the limit
- **AND** the key-value pair is not inserted into the HashMap

#### Scenario: StateStore rejects entry count overflow

- **WHEN** a guest calls `host_store` with a new key (not already present) and
  the HashMap already contains `max_kv_entries` entries
- **THEN** `StateStore::store` returns `Err` with a message naming the limit
- **AND** the key-value pair is not inserted

#### Scenario: StateStore allows update within bounds

- **WHEN** a guest calls `host_store` with an existing key (already present)
  and a value within the `max_value_bytes` limit
- **THEN** the value is updated in place
- **AND** the entry count does not increase

#### Scenario: set_property_impl rejects oversized key or value

- **WHEN** a guest calls `set_property` with a key exceeding `max_key_bytes`
  or a value exceeding `max_value_bytes`
- **THEN** `set_property_impl` does not insert the pair
- **AND** returns early without modifying `self.properties`

### Requirement: WASM linker MUST register only clocks and random WASI interfaces

The WASM linker MUST NOT register the full WASI p2 interface surface. Only
`wasi:clocks` and `wasi:random` MAY be registered. Filesystem, sockets, CLI,
environment, and stdio interfaces MUST be absent from the linker. A guest that
imports a disallowed WASI interface MUST fail at instantiation, not at runtime.

#### Scenario: Selective WASI registration for processor/bean/policy worlds

- **WHEN** `WasmRuntime::new` or `WasmPluginContext::new` constructs a linker
- **THEN** the linker contains only `wasi:clocks` and `wasi:random` interface
  registrations
- **AND** no call to `wasmtime_wasi::p2::add_to_linker_async` exists

#### Scenario: Selective WASI registration for source world

- **WHEN** `source_host::add_to_linker` constructs a linker
- **THEN** the linker contains only `wasi:clocks` and `wasi:random` interface
  registrations
- **AND** no call to `wasmtime_wasi::p2::add_to_linker_async` exists

### Requirement: WASM host MUST NOT inherit host stderr in any world

No WASM world (processor, bean, policy, or source) MUST inherit the host's
stderr via `WasiCtxBuilder::inherit_stderr()`. Guests use `camel_call` for
logging output.

#### Scenario: No inherit_stderr in create_host_state

- **WHEN** `WasmRuntime::create_host_state` constructs a `WasiCtx`
- **THEN** the `WasiCtxBuilder` chain does not call `.inherit_stderr()`
- **AND** all four worlds have identical (no-stdio) WASI contexts

#### Scenario: No inherit_stderr in source world

- **WHEN** `SourceConsumer` constructs a `WasiCtx` for the source world
- **THEN** the `WasiCtxBuilder` chain does not call `.inherit_stderr()`
- **AND** the source world WASI context matches the other three worlds

### Requirement: WASM source listener bind governance

The host SHALL treat the listener bind as operator-authoritative.
When the operator declares a `bind` URI query parameter and the guest
declares a matching `listener_spec.bind` (equal after normalization),
the host SHALL bind exactly that address. When the two differ, the
consumer SHALL fail during startup — after the guest reveals its
listener spec and before `TcpListener::bind` — with an error naming
both addresses; the guest-declared bind SHALL NOT be silently
overridden. When the operator declares no bind, the guest-declared
bind SHALL be used but SHALL pass through the same per-bind exposure
gate as any other transport bind before the TCP listener is created.
A `bind` forwarded from the route URI through guest config SHALL NOT
bypass the gate.

#### Scenario: matching binds produce one listener

- **GIVEN** a `wasm:` source route with `?bind=127.0.0.1:8080` and a
  guest declaring `listener_spec.bind = "127.0.0.1:8080"`
- **WHEN** the source consumer starts
- **THEN** the host binds `127.0.0.1:8080` and exactly one listener
  exists

#### Scenario: conflicting binds fail before the socket is bound

- **GIVEN** a `wasm:` source route with an operator-declared bind and
  a guest-declared `listener_spec.bind` that differs after
  normalization
- **WHEN** the source consumer starts
- **THEN** startup fails with an error naming the route, the operator
  bind, and the guest bind, and no TCP listener is bound

#### Scenario: guest-declared non-loopback bind is gated

- **GIVEN** a `wasm:` source route with no operator bind whose guest
  declares `listener_spec.bind = "0.0.0.0:8080"` and a `Public` plan
- **WHEN** the source consumer starts
- **THEN** startup fails unless the operator acknowledged that bind
  with `allow_public_exposure = true`

