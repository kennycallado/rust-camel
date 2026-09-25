# Proposal: wasmguest — thread real observability through guest-initiated producers

## Why

Guest-initiated wasm producers hard-code `NoOpComponentContext` on the
host-function path (`host_functions.rs`: `run_async_call` /
`run_async_poll` construct it for both `create_endpoint` and the
`create_producer` observability handle). In production the host already
holds the real `Arc<dyn RuntimeObservability>` — `WasmEndpoint::create_producer`
receives it from the route pipeline and stores it on `WasmProducer` — but
the value never reaches `WasmHostState`. Result: every producer dynamically
created to serve a guest `camel_call` / `camel_poll` runs with a no-op
context (spans/telemetry/context services missing on that path). bd rc-zakf.

## What Changes

- `WasmHostState` gains an `observability: Arc<dyn RuntimeObservability>`
  field, threaded through `WasmRuntime::create_host_state`,
  `call_init_once`, `call_process`, and `process_streaming_exchange`
  (constructor injection — the dual-handle seam pattern of tlsseam,
  dcb45b54). `WasmProducer::call` supplies its existing
  `observability` field. No new acquisition mechanism is invented.
- `camel_call_impl` / `camel_poll_impl` snapshot `state.observability`
  (alongside the existing registry snapshot) and pass it to
  `run_async_call` / `run_async_poll`.
- `run_async_call` / `run_async_poll` use the real registry as the
  `create_endpoint` context (`&*registry`, mirroring the CLI precedent)
  and the threaded handle as the `create_producer` rt.
- Worlds where `camel_call` is capability-denied (bean / security-policy /
  authorization-policy plugin contexts) keep a documented NoOp handle —
  the call path is unreachable there by construction.
- Tests: guest-initiated producer observes the real handle — a recording
  observability plus fake component/endpoint prove (a) the producer rt is
  the threaded handle and (b) a counter emitted on that path lands in the
  recording collector.
- Sweep: remaining hard-coded `NoOpComponentContext` in non-test code is
  classified; identical-trivial sites are fixed in-mission, the rest are
  noted with file:line in the park report.

## Impact

- crates/camel-component-wasm (source + tests + CONTEXT.md)
- No spec delta: `wasm-sandbox-hardening` has no host-function-context
  requirement to modify (`skip_specs: true`).
- No public API break: `WasmEndpoint::new` / `WasmProducer::new` /
  `WasmRuntime` method signatures are crate-internal or additive
  (verified during implementation; struct-literal constructors are
  compile-enumerated).
