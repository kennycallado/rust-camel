# Tasks: wasm-registry-metrics

## Task 1: RegistryComponentContext carries the wired collector and lever

**Files**:
- crates/camel-core/src/shared/components/domain/registry.rs (modified)

**Steps** (TDD order — tests land first, red state recorded, then implementation):
1. Add a local recording double `RecordingMetrics` to the existing `mod tests` in registry.rs: struct holding `Arc<std::sync::Mutex<RecordingState>>` where `RecordingState` owns `errors: Vec<(String, String)>` (pairs from `increment_errors(route_id, error_type)`), `component_ops: Vec<(String, String, String)>` (triples from `record_component_operation(component, operation, outcome)`, the trait method the `ComponentMetrics` facade actually calls — camel-api component_metrics.rs), and `counters: Vec<(String, f64)>` (from `record_counter`); accessors `recorded_errors()`, `recorded_component_operations()`, `recorded_counters()`; all other trait methods empty. Owned `String`s, not `&'static str` (the facade passes formatted labels).
2. Add the seven tests below to the same module.
3. Run each test's filter command and record the red state — the tests fail to compile against the one-arg constructor (arity mismatch is the intended first failure). Record which filters were run and their failure output in the task report.
4. Add two fields to `RegistryComponentContext`: `metrics: Arc<dyn camel_api::MetricsCollector>` and `components_enabled: bool`.
5. Replace the constructor `new(registry)` with `new(registry: Arc<std::sync::Mutex<Registry>>, metrics: Option<Arc<dyn camel_api::MetricsCollector>>, components_enabled: bool)`. Resolve the collector ONCE at construction: `metrics.unwrap_or_else(|| Arc::new(camel_api::NoOpMetrics))`. This removes today's per-call `Arc::new(NoOpMetrics)` allocation in `metrics()`.
6. Change the `ComponentContext` impl: `fn metrics(&self)` returns `self.metrics.clone()`; add an override `fn component_metrics_enabled(&self) -> bool { self.components_enabled }` (trait method from camel-component-api `component_context.rs`, default false).
7. Update the struct doc comment: it no longer "no-ops" metrics — state that the collector is threaded from the composition root (camel-cli) and is the ADR-0066 late-bound handle, not a backend snapshot; the lever snapshot gates only the component-operations family (error family is never lever-gated). Rerun all seven filters — green.

**Tests**:
- name: `metrics_returns_wired_collector_and_is_stable`
  setup: a `RecordingMetrics` double wrapped in `Arc`; a `RegistryComponentContext::new(Arc::new(Mutex::new(Registry::new())), Some(collector.clone()), false)`
  action: call `ComponentContext::metrics(&ctx)` twice
  assert: both returned `Arc<dyn MetricsCollector>` are pointer-equal to the wired collector — keep a trait-object-typed clone for the comparison: `let wired_dyn: Arc<dyn MetricsCollector> = collector.clone();` then `Arc::ptr_eq(&returned, &wired_dyn)` (spec scenario "registry context returns the wired collector")
  command: `cargo test -p camel-core --lib metrics_returns_wired_collector`
  expected: fails before implementation (constructor arity mismatch; today returns fresh NoOp each call)
- name: `component_metrics_enabled_reflects_constructor_lever`
  setup: two contexts over the same registry — one `new(reg, None, true)`, one `new(reg, None, false)`
  action: query `ComponentContext::component_metrics_enabled(&ctx)` on each
  assert: first returns `true`, second returns `false` (today both return the default `false`)
  command: `cargo test -p camel-core --lib component_metrics_enabled_reflects`
  expected: fails before implementation
- name: `facade_error_family_reaches_wired_collector_with_lever_off`
  setup: `RecordingMetrics` double; context `new(reg, Some(collector.clone()), false)` (lever OFF)
  action: build the facade via `RuntimeObservability::component_metrics(&ctx)` (blanket impl, camel-component-api `runtime_observability.rs`) and call `facade.observe("wasm", "invoke", true)` (failure)
  assert: exactly one `increment_errors` pair recorded on the double with `route_id == "wasm"` — the error family flows despite the lever being off (spec scenario "error family flows through the facade")
  command: `cargo test -p camel-core --lib facade_error_family_reaches`
  expected: fails before implementation (facade gets NoOp + lever false, double never wired)
- name: `facade_component_family_gated_by_lever`
  setup: two contexts with the same `RecordingMetrics` double — lever ON and lever OFF
  action: on each, `RuntimeObservability::component_metrics(&ctx)` then `facade.observe("wasm", "invoke", false)` (success)
  assert: lever ON records exactly one `record_component_operation` triple `("wasm", "invoke", "success")` on the double; lever OFF records no component-operation triple, no counter, and no error (spec scenario "registry context honors the components lever", suppressed half)
  command: `cargo test -p camel-core --lib facade_component_family_gated`
  expected: fails before implementation
- name: `late_registered_collector_reaches_registry_context`
  setup: `camel_api::MetricsHandle::new()` (camel-api/src/metrics.rs:121) as `Arc<MetricsHandle>`; typed binding `let handle_dyn: Arc<dyn MetricsCollector> = handle_arc.clone();` (no `as` cast); context `new(reg, Some(handle_dyn), false)`
  action: `handle_arc.register(recording_double)` AFTER construction (public API, metrics.rs:132 — not `store`), then `ComponentContext::metrics(&ctx).increment_errors("wasm", "e:wasm:invoke")`
  assert: the double recorded the pair — proves the threaded Arc is the late-bound handle, registration after construction still reaches registry consumers (ADR-0066; proposal acceptance criterion)
  command: `cargo test -p camel-core --lib late_registered_collector`
  expected: fails before implementation (constructor does not accept a collector)
- name: `none_falls_back_to_noop_semantics`
  setup: context `new(reg, None, false)`
  action: call `ComponentContext::metrics(&ctx)` and `RuntimeObservability::component_metrics(&ctx)`, run one `facade.observe("wasm", "invoke", true)`
  assert: no panic; `component_metrics_enabled` is `false`; error flows into the fallback NoOp silently (spec scenario "no collector wired keeps NoOp semantics")
  command: `cargo test -p camel-core --lib none_falls_back`
  expected: fails before implementation (constructor arity mismatch)
- name: `resolve_component_unaffected_by_observability_params`
  setup: registry with `TimerComponent::new()` registered; context `new(reg_arc.clone(), None, false)`
  action: `ComponentContext::resolve_component(&ctx, "timer")`
  assert: resolves to `Some` — lookup semantics regression-free
  command: `cargo test -p camel-core --lib resolve_component_unaffected`
  expected: fails before implementation (constructor arity mismatch)

**Acceptance**:
- `cargo test -p camel-core --lib` exits 0
- `cargo clippy -p camel-core -- -D warnings` exits 0
- `cargo fmt --check -p camel-core` exits 0
- All three scenarios of the ADDED requirement "Registry-resolved components deliver errors to the wired collector" and the new MODIFIED scenario "registry context honors the components lever" are exercised by tests above
- Registry lookup behavior unchanged (regression test present)

**Scenario traceability** (all six scenarios of the two delta specs):
- "registry context returns the wired collector" → `metrics_returns_wired_collector_and_is_stable`
- "error family flows through the facade" → `facade_error_family_reaches_wired_collector_with_lever_off`
- "no collector wired keeps NoOp semantics" → `none_falls_back_to_noop_semantics`
- "registry context honors the components lever" → `component_metrics_enabled_reflects_constructor_lever` + `facade_component_family_gated_by_lever`
- "healthy route observable" and "dead-observability component now emits errors" (inherited canon scenarios, unchanged behavior) → already covered and green via `cargo test -p camel-test --test component_emission_test dead_components_now_emit` (crates/camel-test/tests/component_emission_test.rs:388); Task 2 acceptance reruns it as regression guard, no new work authored

- [x] 1.1

## Task 2: camel-cli composition-root sweep (explicit wiring at all three sites)

**Files**:
- crates/camel-cli/src/commands/run.rs (modified)
- crates/camel-cli/src/security.rs (modified)

**Steps**:
1. run.rs WasmBean site (~:235, inside the bean-loading block after `configure_context_with_beans`): `RegistryComponentContext::new(component_registry.clone(), Some(ctx.metrics()), camel_component_api::ComponentContext::component_metrics_enabled(&ctx))`. `ctx.metrics()` (camel-core context.rs:658) returns the MetricsHandle as `Arc<dyn MetricsCollector>`. Import the `ComponentContext` trait in scope as needed.
2. run.rs WasmBundle site (~:490, `WasmBundle::new`): same threading — `Some(ctx.metrics())` + `ComponentContext::component_metrics_enabled(&ctx)` in place of `ctx.registry_arc()` only-arg construction.
3. security.rs:412 (`build_security_compile_context_from_config`): `RegistryComponentContext::new(registry, None, false)` with a one-line comment: compile-time policy scan, no route runtime, no wired collector (rc-66he design decision).
4. Sweep check (crates/-scoped; the repo-wide acceptance sweep below covers examples/): `rg -n "RegistryComponentContext::new" crates/ -g '*.rs' -g '!crates/camel-core/src/shared/components/domain/registry.rs'` lists exactly 3 matches (the camel-cli sites), all passing explicit observability args — no silent-NoOp construction path remains outside the registry.rs test module (Task 1 adds several test constructions there; they are expected and not counted). Aggregate count: pipe the same `rg -n` through `wc -l` and assert 3.
5. File the deferred-defect bd issue from the proposal's Excluded section, from the REPO ROOT (not the worktree), capturing the id: `ISSUE_ID=$(bd create "Guest-initiated wasm producers hard-code NoOpComponentContext as rt (host_functions.rs:112-113): observability dead path" -t bug -p 3 --deps discovered-from:rc-66he --json | python3 -c "import json,sys; print(json.load(sys.stdin)['id'])")` — record `"$ISSUE_ID"` in the task report.

**Tests** (wiring-only task; behavior is covered by Task 1 units — these are machine-checkable compile/gate verifications):
- name: `camel-cli compiles with wasm sites wired`
  setup: Task 1 landed; the three sites edited
  action: `cargo clippy -p camel-cli -- -D warnings` (wasm is in camel-cli default features, Cargo.toml:107)
  assert: exit 0, zero warnings
  command: `cargo clippy -p camel-cli -- -D warnings`
  expected: fails before the sweep (constructor arity mismatch at 3 sites)
- name: `no implicit constructor remains`
  setup: sweep complete
  action: `rg -n "RegistryComponentContext::new" crates/ -g '*.rs' -g '!crates/camel-core/src/shared/components/domain/registry.rs' | wc -l` (aggregate; the unpiped `rg -n` output is inspected separately for the 3-arg arity at each site)
  assert: aggregate count is exactly 3 (camel-cli/src/commands/run.rs ×2, camel-cli/src/security.rs ×1), each with three arguments; registry.rs test-module constructions excluded from the count
  command: `rg -n "RegistryComponentContext::new" crates/ -g '*.rs' -g '!crates/camel-core/src/shared/components/domain/registry.rs' | wc -l`
  expected: 3 before and after Task 1; arity changes from 1 arg to 3
- name: `deferred defect tracked`
  setup: bd issue created in step 5, id captured in `ISSUE_ID`
  action: `bd show "$ISSUE_ID" --json`
  assert: issue exists, type bug, priority 3, `discovered-from:rc-66he` dependency
  command: `bd show "$ISSUE_ID" --json`
  expected: passes after step 5

**Acceptance**:
- `cargo clippy -p camel-cli -- -D warnings` exits 0
- `cargo fmt --check -p camel-cli` exits 0
- `rg -n "RegistryComponentContext::new" -g '*.rs' -g '!crates/camel-core/src/shared/components/domain/registry.rs' -g '!target' | wc -l` outputs exactly 7 (camel-cli ×3 + examples ×4), all explicit — the negation glob must come after `-g '*.rs'` (rg: last glob wins, so a leading negation is overridden and the registry.rs test module leaks into the count)
- examples/ sites swept: wasm-example and wasm-streaming-plugin thread the context handle + lever; wasm-bean-example and security-wasm-policy pass `None, false` (context built after bean/policy load)
- `cargo test -p camel-test --test component_emission_test dead_components_now_emit` exits 0 (inherited-scenario regression guard)
- bd issue for the host_functions NoOpComponentContext dead path exists with `discovered-from:rc-66he`; id recorded
- No test files added to camel-cli (run paths have no harness; behavior owned by Task 1)

- [x] 2.1

## Change-wide gates (run after both tasks)

- `cargo fmt --check --all` exits 0
- `cargo check -p camel-core -p camel-cli` exits 0
- `cargo test -p camel-cli` exits 0
- `cargo test -p camel-core --lib` exits 0 (rerun after Task 2 sweep — constructor signature is shared)
