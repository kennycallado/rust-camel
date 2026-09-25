# Design: wasmguest

## Context

bd rc-zakf: guest-triggered producer paths run with a no-op observability
context. The host function path (`camel:plugin/host` world) dynamically
creates an endpoint + producer per guest `camel_call` / `camel_poll`; both
were constructed against `NoOpComponentContext` even though the real
component context (registry) is in scope on that exact path.

## Acquisition precedent (do not invent a new one)

`RuntimeObservability` is blanket-implemented for every `T: ComponentContext`
(concrete types; a `dyn ComponentContext` -> `dyn RuntimeObservability`
sidecast does not exist). The sanctioned pattern across camel-core is a
**separate handle threaded from the same owner**:

- `make_endpoint_resolver(component_ctx, rt, producer_ctx)`
  (endpoint_resolver_factory.rs) — two Arcs, one origin.
- `CompilationContext { rt, component_ctx, .. }` (step_compilers).
- `ControllerComponentContext` carries metrics/health/in_flight fields so
  its `Arc<dyn RuntimeObservability>` view reaches `create_producer`.

Inside camel-component-wasm itself, `WasmEndpoint::create_producer(rt, ..)`
already receives the real handle from the pipeline and stores it as
`WasmProducer.observability` (used today only for the producer's own
`wasm:invoke` component-metrics emission). The missing link is
`WasmProducer -> WasmRuntime -> WasmHostState -> host_functions`.

## The seam (constructor injection, tlsseam dcb45b54 pattern)

1. `WasmHostState` gains `observability: Arc<dyn RuntimeObservability>`.
2. `WasmRuntime::{call_init_once, call_process, process_streaming_exchange}`
   take the handle as a parameter; `create_host_state` stores it.
3. `WasmProducer::call` passes `Arc::clone(&self.observability)`.
4. `camel_call_impl` / `camel_poll_impl` snapshot
   `view.get().observability.clone()` next to the existing registry
   snapshot (same `store.with` discipline — no borrow across await).
5. `run_async_call(registry, observability, uri, payload)`:
   - `component.create_endpoint(&uri, &*registry)` — real context
     (mirrors `camel-cli job`: `create_endpoint(uri, ctx)` with the live
     context). Today: `&NoOpComponentContext`.
   - `endpoint.create_producer(observability, &ProducerContext::new())` —
     real handle. Today: `Arc::new(NoOpComponentContext)`.
6. `run_async_poll` analogously: real registry for `create_endpoint`
   (`polling_consumer()` takes no rt; nothing else changes there).

Adding the struct field makes the compiler enumerate every `WasmHostState`
construction site; each site gets either the threaded handle or, in worlds
where `camel_call` is capability-denied (bean, security-policy,
authorization-policy via `WasmCapabilities::denied()` / empty call
allowlist — CONTEXT.md: "an empty allowlist denies every scheme"), a NoOp
handle with a comment stating why that is unreachable-by-construction.
The source world (`SourceHostState`) has no camel host functions and is
out of scope.

## Tests

Direct-call unit tests in `host_functions.rs` (the inherent impls exist so
tests can call them without an `Accessor`):

- `guest_call_threads_observability_to_dynamic_producer`:
  fake `ComponentContext` (recording metrics collector) resolves a fake
  component; fake endpoint's `create_producer` captures the rt and its
  producer emits a counter through `rt.metrics()`. Assert the emitted
  counter/labels landed in the recording collector (bd AC: "test asserts a
  recorded counter/label").
- `guest_call_passes_live_context_to_create_endpoint`: the context received
  by `create_endpoint` is the live registry, not NoOp — proven behaviorally
  (an emission through `ctx.metrics()` inside `create_endpoint` reaches the
  recording collector).
- Same capture for the `run_async_poll` endpoint-creation path.
- Existing producer/runtime tests stay green (constructor updates only);
  `cargo test -p camel-component-wasm --lib` exit 0 (bd AC).

## Sweep classification (production non-test `NoOpComponentContext`)

| Site | Class | Action |
|---|---|---|
| wasm host_functions.rs run_async_call/poll | defect | fix in-mission |
| camel-cli commands/job/mod.rs:2114 | production, not identical-trivial | assess: fix only if concrete ctx Arc in scope; else park-note |
| camel-cli commands/test/runner.rs:265 | production test-harness | same assessment |
| camel-bench benches/direct.rs | intentional (bench noise isolation) | park-note |
| camel-core step_compilers / endpoint_resolver_factory / consumer_management / context_lifecycle | `#[cfg(test)]` (verified except context_lifecycle — worker verifies) | none |

## Risks

- Signature widening on `WasmRuntime` methods is additive; struct literals
  are crate-internal (compiler enumerates). External-crate `WasmHostState`
  literals would break — none exist outside camel-component-wasm except
  tests (verified by grep: only camel-test support touches related types,
  not this struct).
- Passing the live registry into `create_endpoint` changes what nested
  resolution a guest-triggered endpoint can do. This mirrors the CLI and
  core resolver behavior; capability gating (scheme allowlist) still runs
  before any of it.
