# Tasks: wasmguest

## Task 1 — Thread observability through the wasm host-function producer path

- [x] 1.1

**Files**: `crates/components/camel-component-wasm/src/{runtime.rs, producer.rs, host_functions.rs, wasm_plugin_context.rs, bean.rs, authorization_policy.rs, security_policy.rs, stream_bridge.rs}` (all `WasmHostState` construction sites; compiler-enumerated after the field is added), `crates/components/camel-component-wasm/CONTEXT.md`.

**Steps**:
1. Add `pub observability: Arc<dyn camel_component_api::RuntimeObservability>` to `WasmHostState` (runtime.rs), threaded via `create_host_state` and the `call_init_once` / `call_process` / `process_streaming_exchange` signatures.
2. `WasmProducer::call` passes `Arc::clone(&self.observability)` into the runtime calls (it already snapshots `observability` per call for its own component-metrics emission).
3. Update every other `WasmHostState` construction site: worlds with `camel_call` capability-denied (bean / security-policy / authorization-policy plugin contexts, test modules) get a NoOp handle (`camel_component_api::NoOpComponentContext` via the blanket impl, or `test_support::NoopRuntimeObservability` in tests — match sibling style in producer.rs tests) with a one-line comment stating why the path is unreachable/denied there.
4. `camel_call_impl` / `camel_poll_impl`: snapshot `observability` next to the existing registry snapshot (same `store.with` closure discipline).
5. `run_async_call` / `run_async_poll` signatures take the handle; use `&*registry` for `create_endpoint` (mirrors camel-cli job precedent) and the handle for `create_producer`.
6. CONTEXT.md "Camel host functions" section: one sentence documenting that `camel_call` / `camel_poll` thread the producer's `RuntimeObservability` into dynamically created endpoints/producers.

**Tests** (write first, verify they fail against the NoOp hard-code, then implement):
- `guest_call_threads_observability_to_dynamic_producer`
  - Arrange: fake `ComponentContext` (metrics -> `CountingMetrics`-style recording collector, pattern: `RecMetrics` context_tests.rs / endpoint_resolver_factory.rs tests); fake `Component` resolves under scheme `fake`; fake `Endpoint` whose `create_producer` captures the rt Arc and returns a producer that emits `record_counter("wasmguest:test:invoke", [("route","test")])` through `rt.metrics()` on `oneshot`.
  - Act: `run_async_call(registry, observability, "fake:dest".into(), "{\"ok\":true}".into()).await`.
  - Assert: `Ok` body echoes payload; captured rt is ptr-eq to the passed `observability` (`Arc::ptr_eq`); recording collector holds the emitted counter with the exact family + label pair.
- `guest_call_passes_live_context_to_create_endpoint`
  - Arrange: same registry; fake endpoint's `create_endpoint` emits `record_counter("wasmguest:test:ctx", [])` through the received `&dyn ComponentContext`'s `metrics()`.
  - Act: `run_async_call(...)` as above.
  - Assert: the emission landed in the recording collector (proves live context, not `NoOpMetrics`).
- `guest_poll_passes_live_context_to_create_endpoint`
  - Arrange/Act: same, via `run_async_poll` (fake endpoint exposes `polling_consumer`).
  - Assert: live-context emission recorded; poll result shape unchanged.

**Acceptance**:
- No `NoOpComponentContext` remains on the `run_async_call` / `run_async_poll` paths.
- `cargo test -p camel-component-wasm --lib` exit 0 (bd AC).
- `cargo fmt --check` clean; `cargo clippy -p camel-component-wasm --all-targets -- -D warnings` clean.
- Deny-worlds carry the explanatory comment at their NoOp handle.

## Task 2 — Production NoOp sweep assessment

- [x] 2.1

**Files**: none modified unless the trivial-fix condition holds; findings go to the park report.

**Steps**:
1. Grep `NoOpComponentContext` across `crates/` (exclude `#[cfg(test)]` modules, `tests/` dirs, `camel-integration-test`, benches).
2. Verify `crates/camel-core/src/lifecycle/application/context_lifecycle.rs:464,468,501` are inside a test module (the other camel-core hits are verified test-module sites).
3. `crates/camel-cli/src/commands/job/mod.rs:2114` and `crates/camel-cli/src/commands/test/runner.rs:265`: check whether a concrete `Arc<CamelContext>` (or equivalent owning Arc) is in scope at the site. If yes, the identical-trivial dual-cast fix (`Arc::clone(&ctx)` coerced to `Arc<dyn RuntimeObservability>` via the blanket impl) applies — apply it and run `cargo test -p camel-cli --lib`. If no, leave untouched.
4. Record every remaining production site with file:line + one-line reason for the park report (deferrals ledger).

**Tests**: no new tests; existing suites must stay green (camel-cli lib tests if touched).

**Acceptance**:
- Sweep table (site / class / action) complete in the park report.
- Any in-mission CLI fix passes `cargo clippy -p camel-cli -- -D warnings` + `cargo test -p camel-cli --lib`.
