# Design: shared-log-capture-harness

## Approach

**Host: `camel-component-api::test_support::log_capture`** (not camel-api).
Rationale: (a) it is the established test-support precedent surface —
`NoopRuntimeObservability` lives there, gated `#[cfg(any(test, feature =
"test-support"))]`; (b) three of five consumers (camel-core, camel-config,
camel-component-wasm) already dev-depend on it with `test-support` enabled,
and camel-processor already has a normal `camel-component-api` edge;
(c) camel-api has no `tracing`/`tracing-subscriber` dependencies and would
need a duplicate gating mechanism. Module layout follows the existing
children pattern: `src/test_support/log_capture.rs` +
`log_capture_tests.rs` (`#[cfg(test)] #[path = ...]`), declared in
`src/test_support.rs` as `pub mod log_capture;`.

**Verbatim-move rule.** Every capture family moves byte-identical
(logic, rendering, level predicates, return shapes). Behavior of every
consumer test must be identical; no unification of formats across
families.

**Shared surface** (all `pub`, doc-commented):

- `CaptureWriter` (`Arc<Mutex<Vec<u8>>>` + `MakeWriter`), `capture_logs`
  (sync, `with_default`, returns `String`) and `capture_logs_async`
  (`set_default` held across `.await`, returns `(F::Output, String)`).
  Sites 3/4 already share the identical writer/format contract; two entry
  points, one copy, per mission. `capture_logs_async` is documented for
  current-thread execution only: the thread-local default is installed on
  the polling thread, spawned tasks' events are not captured, and no
  `Send`/migration-safety guarantee is made (same restriction as the
  wasm site it replaces).
- `capture_warns` — camel-config's hand-rolled `Recorder` Subscriber,
  verbatim.
- `capture_debugs_with_span_records`, `captured_field`, `record_field` —
  camel-processor's Layer-based registry capture, verbatim.
- `traced::{lock_log_capture, clear_captured_logs,
  captured_records_containing}` — the `#[traced_test]` global-buffer
  family, behind `#[cfg(feature = "traced-test-capture")]`. This is a
  deliberately different mechanism (global recording subscriber installed
  by `#[traced_test]`; the k8s leadership loop logs from spawned tasks, so
  a thread-local capture would miss events). It is promoted so all five
  sites use the shared import, not converted to another shape.

**Guard-colocation contract (hard constraint, rc-img5 / rc-6jarb).** The
shared capture fns install only thread-local subscribers and never touch
`set_global_default`. Each consumer keeps its process-global
`ensure_global_tracing_default` / `ensure_global_registry` body exactly
where it is today, invoked by a thin local wrapper that then delegates to
the shared fn: `camel-config/src/config.rs` `log_capture` (lib binary),
`camel-config/tests/common/mod.rs` (integration binary, untouched),
`camel-core/src/lifecycle/adapters/route_controller_trait_tests.rs`
(inline), `camel-component-wasm/tests/common/mod.rs`,
`camel-processor/src/test_log_capture.rs` (`ensure_global_registry`).
The kubernetes site has no tracing-default guard (tracing-test installs
its own); only the local fns swap to shared imports.

**Feature wiring** (ADR-0055-clean: component-api's normal deps never
reach back to any holder; the new dev-deps are the sanctioned
`test-support` shape):

- `camel-component-api [dependencies]`: `tracing-subscriber = {
  workspace = true, optional = true }`, `tracing-test = { workspace =
  true, optional = true, features = ["no-env-filter"] }` (currently
  tracing-subscriber is dev-only, which downstream feature consumers
  would not see).
- `[features]`: `test-support = ["dep:rcgen", "dep:tracing-subscriber"]`,
  `traced-test-capture = ["test-support", "dep:tracing-test"]` (it
  implies `test-support` because the parent `test_support` module gate
  requires it); self dev-dep adds `traced-test-capture` so in-crate tests
  cover all families.
- `[dev-dependencies]`: camel-processor adds `camel-component-api =
  { workspace = true, features = ["test-support"] }`;
  camel-platform-kubernetes adds `camel-component-api = { workspace =
  true, features = ["test-support", "traced-test-capture"] }`.

**Level-Ord ruling (pre-flight).** tracing-core 0.1.36 orders `Level`
reversed (ERROR lowest; `metadata.rs:945-950`). camel-processor's
`<= Level::DEBUG` ("DEBUG-and-more-severe") is correct; camel-config's
`>= Level::WARN` therefore matches WARN/INFO/DEBUG/TRACE (everything
except `error!`), not "WARN-level" as its old doc claims. Ruling: move
verbatim, document the real semantics honestly in the shared module,
file a bd follow-up for the predicate fix + config-test audit. Changing
the predicate here would alter captured output — out of bounds.

**No-unwrap adaptation.** The shared module is feature-gated, not
`#[cfg(test)]`-scoped, so `lint-unwrap` applies in full. Failure
semantics stay identical to the sources: the fmt-family writer and
readback use `.lock().expect("capture buffer lock poisoned")` (panics on
poison exactly like the `.unwrap()` it replaces, lint-clean); the
processor/config families keep their existing verbatim poison-tolerant
handling (`if let Ok(mut slot)`, `unwrap_or_default`) unchanged.

**camel-cli:** no log-capture consumers found; no change (lane 192).

## Affected crates

- `camel-component-api`: new `test_support::log_capture` module + feature/dep wiring.
- `camel-processor`: dev-dep added; `test_log_capture.rs` becomes guard + wrappers + re-exports (call sites in `error_handler.rs`, `do_try_segment.rs` keep their import paths).
- `camel-config`: `log_capture` keeps its guard; `capture_warns` delegates to shared; config_tests imports unchanged.
- `camel-core`: `route_controller_trait_tests.rs` keeps its guard; `capture_logs` delegates; inline writer deleted.
- `camel-component-wasm`: `source_bind_gate.rs` keeps `common::ensure_global_tracing_default`; async wrapper delegates; no Cargo change.
- `camel-platform-kubernetes`: dev-dep added; platform_service trio swaps to shared imports.

## Architecture boundaries

Test-support surface lives in the ports crate (`camel-component-api`),
consumed via dev-deps by adapters/platforms — no control-plane or
data-plane code changes, no production behavior change (ADR-0045 charter;
ADR-0055 publish topology; ADR-0091 style on the new module). Convention
context: `docs/testing/tracing-capture-guards.md` (guard inventory +
promotion rationale; a follow-up bd issue will point its "Shared home"
section at the new module).
