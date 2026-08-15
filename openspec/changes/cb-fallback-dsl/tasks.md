# Tasks: cb-fallback-dsl

## camel-dsl + camel-api: declarative surface and canonical contract

### Task 1.1: AST, declarative, and canonical fallback fields

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/model.rs` (modified)
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-api/src/runtime.rs` (modified)

**Steps:**
1. In `route_ast.rs`, add to `RouteDslCircuitBreaker` (struct at ~line 176):
   `#[serde(default)] pub fallback: Vec<RouteDslStep>,` — `deny_unknown_fields` stays.
2. In `crates/camel-dsl/src/model.rs`, add to `DeclarativeCircuitBreaker` (~line 56):
   `pub fallback: Vec<DeclarativeStep>,` (serde-defaulted like the AST field; drop the
   `Eq` derive on the struct if `DeclarativeStep` is not `Eq` — it already derives only
   what its step list supports).
3. In `yaml.rs` conversion site (~line 329, where `DeclarativeCircuitBreaker` is built
   after threshold validation), convert the AST fallback with the SAME AST→
   `DeclarativeStep` conversion helper the route body steps already use (find the
   helper mapping `RouteDslStep` → `DeclarativeStep` for `def.steps` and map each
   fallback element through it).
4. In `camel-api/src/runtime.rs`, add to `CanonicalCircuitBreakerSpec` (~line 289):
   `#[serde(default, skip_serializing_if = "Vec::is_empty")] pub fallback:
   Vec<CanonicalStepSpec>,` — mirror the `CanonicalStepSpec::Cache { on_miss }` serde
   pattern (~line 153).
5. Update every struct literal constructing `CanonicalCircuitBreakerSpec` /
   `DeclarativeCircuitBreaker` / `RouteDslCircuitBreaker` in these crates to carry the
   new field (`grep -rn "CanonicalCircuitBreakerSpec {" crates/` and
   `grep -rn "DeclarativeCircuitBreaker {" crates/`; tests in compile.rs ~line 2949
   and runtime.rs ~line 845 are known sites — those two compile.rs/runtime.rs literal
   fixes may land here or in Task 1.2, wherever `cargo check` demands).

**Tests:**
- `name`: `yaml_circuit_breaker_fallback_parses`
  `setup`: none.
  `action`: parse a YAML route with `circuit_breaker: { failure_threshold: 1,
  open_duration_ms: 60000, fallback: [ cache_peek_stale: { repository: persistent,
  key: "tile-xyz" } ] }` through the existing yaml.rs parse entry point used by
  neighboring tests (~line 2080 has a CB fixture to mirror).
  `assert`: parse Ok; declarative route's `circuit_breaker.fallback` has 1 step whose
  verb is the cache-peek-stale builder variant.
  `command`: `cargo test -p camel-dsl --lib yaml_circuit_breaker_fallback_parses`
  `expected`: fails before step 1-3, passes after.
- `name`: `yaml_circuit_breaker_unknown_field_rejected`
  `setup`: none.
  `action`: parse a YAML route with `circuit_breaker: { failure_threshold: 1,
  unknown_key: 1 }`.
  `assert`: parse Err containing `unknown field`.
  `command`: `cargo test -p camel-dsl --lib yaml_circuit_breaker_unknown_field_rejected`
  `expected`: passes before AND after (regression guard).
- `name`: `canonical_circuit_breaker_fallback_roundtrip`
  `setup`: a `CanonicalRouteSpec` with `circuit_breaker.fallback` containing one
  canonical cache-peek-stale step.
  `action`: serde roundtrip (serialize → deserialize) mirroring the existing
  `on_miss` roundtrip test in runtime.rs (~line 969).
  `assert`: fallback step list preserved; a spec serialized WITHOUT the `fallback` key
  deserializes with empty fallback.
  `command`: `cargo test -p camel-api --lib canonical_circuit_breaker_fallback_roundtrip`
  `expected`: fails before step 6, passes after.

**Acceptance:**
- `cargo test -p camel-dsl --lib` passes including the two new yaml tests.
- `cargo test -p camel-api --lib` passes including the roundtrip test.
- `cargo clippy -p camel-dsl -p camel-api -- -D warnings` exits 0.

- [x] 1.1

### Task 1.2: RouteDefinition sidecar and camel-core fallback compile

**Files:**
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_ext.rs` (modified)
- `crates/camel-dsl/src/compile.rs` (modified)
- `crates/camel-test/tests/integration_test.rs` (modified)

**Steps:**
1. In `route_definition.rs`, add a sibling field of `circuit_breaker` (~line 321):
   `pub(crate) circuit_breaker_fallback: Vec<BuilderStep>,` (Default::default() in the
   impl) and a builder setter `pub fn with_circuit_breaker_fallback(mut self, steps:
   Vec<BuilderStep>) -> Self` mirroring `with_circuit_breaker` at ~line 389.
2. In `compile.rs` declarative path (~line 228): convert the declarative
   `fallback: Vec<DeclarativeStep>` with the existing `compile_declarative_steps`
   helper (~line 859 — same one the route body uses) and route the resulting
   `Vec<BuilderStep>` into the sidecar setter (from step 1) BEFORE the
   with_circuit_breaker call. At the canonical-map site (~line 288) reuse
   `compile_declarative_steps_to_canonical` (~line 1490) to fill
   `CanonicalCircuitBreakerSpec.fallback`; at the rehydrate site (~line 366) reuse
   `compile_canonical_steps` to rebuild `Vec<BuilderStep>` into the same sidecar
   setter. Zero new converters.
3. In `route_compiler_ext.rs` `compile_route_impl` (~line 556): BEFORE the
   `collect_lifecycle(&processors_with_contracts)` call at ~line 582 — take
   `def.circuit_breaker_fallback` by value; if non-empty AND `def.circuit_breaker` is
   `Some(cfg)`: resolve the steps via the same `resolve_steps` ext method the main
   pipeline uses (~line 297) → `Vec<CompiledStep>`; build the route lifecycle as
   `let mut lifecycle = collect_lifecycle(&processors_with_contracts);` then
   `lifecycle.extend(collect_lifecycle(&fb_compiled));`; compose the fallback with
   `compose_traced_pipeline_with_contracts` (already imported at ~line 37; same call
   shape as ~line 206 — pass `None` as the in-pipeline error handler); attach with
   `cfg.fallback(fb)`; pass the resulting config into `build_eh_config_pipeline`
   instead of the raw `def.circuit_breaker`. If the sidecar is empty or no CB is
   configured, behavior is byte-identical to today (fallback stays `None`).
4. Add gate-path e2e test (below) to `crates/camel-test/tests/integration_test.rs`
   next to `circuit_breaker_with_error_handler` (~line 686).

**Tests:**
- `name`: `circuit_breaker_fallback_gate_path_serves_body`
  `setup`: a full YAML route (string or fixture file) with
  `circuit_breaker: { failure_threshold: 1, open_duration_ms: 60000, fallback: [
  cache_peek_stale: { repository: persistent, key: "tile-xyz" } ] }`, a failing
  upstream step in the route body, and an error handler (forces `eh_config = Some` →
  gate branch). Parse it with the camel-dsl YAML entry point (`parse_yaml` or the
  equivalent used by neighboring DSL e2e tests) to obtain the `RouteDefinition` —
  this exercises the full AST → DeclarativeStep → BuilderStep → sidecar threading.
  Register a seeded `"persistent"` repository holding a past-expiry entry under
  `"tile-xyz"` and add the definition to the `CamelTestContext` (camel-test already
  depends on camel-core and camel-dsl).
  `action`: send enough requests to open the circuit, then one more exchange.
  `assert`: final exchange body is the stale cached value; no `CircuitOpen` error.
  `command`: `cargo test -p camel-test --test integration_test
  circuit_breaker_fallback_gate_path_serves_body`
  `expected`: fails before step 2, passes after.
- `name`: `stateful_fallback_step_lifecycle_invoked`
  `setup`: a new `crates/camel-core/tests/cb_fallback_lifecycle_test.rs` following
  the `peek_stale_on_miss_test.rs` precedent (direct `RouteDefinition::new` +
  `CamelContext` wiring, no YAML). Build a route whose CB fallback contains a
  `wire_tap` draining to a `mock:` endpoint, with a `CamelTestContext`-style mock
  consumer capturing deliveries.
  `action`: compile + start the route; send one exchange to open the circuit so the
  fallback (and its blocked tap processor) runs; begin route shutdown while the tap
  is still blocked; assert shutdown has NOT completed; release the tap; await
  shutdown completion.
  `assert`: the mock delivery arrives only after release AND route shutdown completes
  only after the tap drains. If the fallback's lifecycle handles were NOT merged
  into the route lifecycle vec, shutdown would complete while the tap is still
  blocked — the test fails. (Blocked-tap: the tap signals entry, awaits a tokio
  release primitive, records delivery only after release.)
  `command`: `cargo test -p camel-core --test cb_fallback_lifecycle_test`
  `expected`: fails before step 2 (shutdown never called), passes after.

- `name`: `circuit_breaker_without_fallback_behavior_unchanged`
  `setup`: the existing `circuit_breaker_with_error_handler` test (~line 686) — a CB
  route with NO fallback key.
  `action`: run the whole existing CB test suite after the change.
  `assert`: all pre-existing CB tests pass unmodified (empty/absent fallback →
  `CircuitBreakerConfig.fallback == None`, no behavior change).
  `command`: `cargo test -p camel-test --test integration_test circuit_breaker`
  `expected`: passes before AND after (regression guard).

**Acceptance:**
- `cargo test -p camel-core --test cb_fallback_lifecycle_test` passes.
- `cargo test -p camel-test --test integration_test circuit_breaker` passes (all CB
  tests including the new one).
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- No fallback `BoxProcessor` construction exists in camel-dsl (verified by
  `grep -rn "fallback(" crates/camel-dsl/src/` finding only sidecar threading).

- [x] 1.2

## camel-api + camel-builder: contract guards

### Task 2.1: validate_contract recursion into fallback steps

**Files:**
- `crates/camel-api/src/runtime.rs` (modified)

**Steps:**
1. In `validate_contract` (~line 341), after the existing CB scalar checks
   (`failure_threshold > 0`, `open_duration_ms > 0` at ~line 359-370), add one call:
   `validate_steps(&cb.fallback)?` — the per-step validation is already factored into
   `validate_steps` (the same fn the top-level loop at ~line 461 uses). No extraction,
   no duplication, no top-level behavior change.

**Tests:**
- `name`: `canonical_contract_rejects_invalid_fallback_step`
  `setup`: a `CanonicalRouteSpec` with valid CB scalars but a fallback step that
  violates the same rule the top-level loop enforces (pick the rule the ~line 461 loop
  checks for the chosen step kind — e.g. an empty-body step where a body is required;
  mirror the existing `canonical_contract_rejects_invalid_aggregate_and_circuit_breaker`
  test at ~line 812 for style).
  `action`: call `validate_contract()`.
  `assert`: returns Err naming the fallback step (same error text shape as the
  top-level violation).
  `command`: `cargo test -p camel-api --lib
  canonical_contract_rejects_invalid_fallback_step`
  `expected`: fails before step 1 (validation passes silently), errors after.

**Acceptance:**
- `cargo test -p camel-api --lib` passes.
- `cargo clippy -p camel-api -- -D warnings` exits 0.

- [x] 2.1

### Task 2.2: builder reverse path fails closed on opaque fallback

**Files:**
- `crates/camel-builder/src/lib.rs` (modified)
- `crates/camel-builder/tests/canonical_spec_test.rs` (modified)

**Steps:**
1. In `canonicalize_circuit_breaker` (~line 1107, reconstructs
   `CanonicalCircuitBreakerSpec` from a `CircuitBreakerConfig`): when
   `config.fallback.is_some()`, return an explicit error (the function's existing
   error type / `CamelError::Config` — follow how sibling canonicalize functions
   report unrecoverable state) with a message naming the fallback processor as
   non-canonical (ADR-0016 no-silent-loss). When `None`, behavior unchanged.
2. Propagate: the caller that builds `CanonicalRouteSpec.circuit_breaker` handles the
   new error variant path (compile error if the fn previously returned the struct
   directly — restructure to `Result` following the file's existing error conventions).
3. Update any struct literals across the workspace that construct
   `CanonicalCircuitBreakerSpec` and now fail to compile (`grep -rn
   "CanonicalCircuitBreakerSpec {" crates/`).

**Tests:**
- `name`: `builder_rejects_opaque_circuit_breaker_fallback`
  `setup`: a builder-constructed route with `.circuit_breaker(config.with_fallback(
  some_box_processor))`.
  `action`: attempt canonicalization (the API exercised by neighboring tests in
  `canonical_spec_test.rs`).
  `assert`: Err whose message contains `fallback`; no spec is produced.
  `command`: `cargo test -p camel-builder --test canonical_spec_test
  builder_rejects_opaque_circuit_breaker_fallback`
  `expected`: fails before step 1 (silently drops fallback), errors after.
- `name`: `builder_canonicalizes_circuit_breaker_without_fallback`
  `setup`: same route shape without a fallback processor.
  `action`: canonicalize.
  `assert`: Ok, `circuit_breaker.fallback` empty.
  `command`: `cargo test -p camel-builder --test canonical_spec_test
  builder_canonicalizes_circuit_breaker_without_fallback`
  `expected`: passes before AND after (regression guard).

**Acceptance:**
- `cargo test -p camel-builder` passes.
- `cargo clippy -p camel-builder -- -D warnings` exits 0.

- [x] 2.2

## camel-test: outcome regression on both runtime branches

### Task 3.1: clean-outcome and failing-fallback regression, both branches

**Files:**
- `crates/camel-test/tests/integration_test.rs` (modified)

**Steps:**
1. Add layer-path test (below) — a route with `circuit_breaker` + fallback and NO
   error handler (route-level or global) so `build_eh_config_pipeline` takes the
   `eh_config = None` branch → `CircuitBreakerLayer`/`CircuitBreakerService`
   (route_compiler_ext.rs ~line 232-247).
2. Add peek-MISS clean-stop assertions for both branches (extend the Task 1.2 gate test
   and the new layer test with the miss variant, or add dedicated tests — one per
   branch).
3. Add the failing-fallback asymmetry test (below).

**Tests:**
- `name`: `circuit_breaker_fallback_layer_path_serves_body`
  `setup`: `CamelTestContext` route with `circuit_breaker(failure_threshold 1)` +
  fallback (peek-stale on a seeded past-expiry entry) + failing upstream + NO error
  handler.
  `action`: open the circuit, send one more exchange.
  `assert`: body is the stale value (Tower service read `config.fallback` at
  circuit_breaker.rs ~line 166-168); no `CircuitOpen` error.
  `command`: `cargo test -p camel-test --test integration_test
  circuit_breaker_fallback_layer_path_serves_body`
  `expected`: fails before Task 1.2 lands, passes after (this test pins the layer
  branch specifically).
- `name`: `circuit_breaker_fallback_miss_stops_cleanly_per_branch`
  `setup`: two routes (gate variant with error handler; layer variant without), each
  with CB + fallback `cache_peek_stale` on a key with NO entry (default
  `on_miss: stop`), circuit open.
  `action`: send one exchange per route.
  `assert`: each surfaces `Ok(exchange)` with exchange state intact — no
  `CircuitOpen`, no boundary error (Stop translated to `Ok` inside the fallback
  pipeline's `into_tower_result`).
  `command`: `cargo test -p camel-test --test integration_test
  circuit_breaker_fallback_miss_stops_cleanly_per_branch`
  `expected`: passes once Task 1.2 lands (behavior pre-exists; test pins it).
- `name`: `circuit_breaker_failing_fallback_asymmetry`
  `setup`: two routes (gate variant with DLC error handler; layer variant without),
  CB open, fallback containing a step that always returns a genuine `Err` (e.g. an
  http producer to a dead URI, or a processor returning `Err(CamelError::Config)`.
  Use the lightest failing step the harness offers).
  `action`: send one exchange per route.
  `assert`: gate variant — the error reaches the DLC/error-handler disposition
  (exchange completes via handler, not a raw Err to the caller); layer variant — raw
  `Err` surfaces to the caller. This pins the documented asymmetry.
  `command`: `cargo test -p camel-test --test integration_test
  circuit_breaker_failing_fallback_asymmetry`
  `expected`: passes once Task 1.2 lands (behavior pre-exists; test pins it).

**Acceptance:**
- `cargo test -p camel-test --test integration_test circuit_breaker` passes (all CB
  tests).
- `cargo clippy -p camel-test --all-targets -- -D warnings` exits 0.

- [x] 3.1

## surface: schema, TS bindings, docs, example

### Task 4.1: schema regen, TS bindings, docs, and example route

**Files:**
- `schemas/dsl/route-schema.json` (modified, regenerated)
- `crates/camel-lint/schema/route-schema.json` (modified, byte-identical copy)
- `schemas/ts/` (modified, regenerated — includes `CanonicalCircuitBreakerSpec.ts` and
  any `RouteDslCircuitBreaker`-derived export)
- `docs/src/yaml-dsl/route-structure.md` (modified — documents `circuit_breaker` and
  now gains the `fallback` sub-pipeline surface)
- `docs/src/eip/cache.md` (modified — cross-reference the CB fallback composition
  from the cache_peek_stale docs)
- `examples/` (modified — new or extended example demonstrating CB →
  `cache_peek_stale`; follow the include-driven examples pattern with anchors used by
  `examples/cache-example/routes.yaml`)

**Steps:**
1. Regenerate the DSL schema with the workspace command used by the `schema` xtask
   (`cargo xtask schema` — same flow that produced the enum update in commit
   `4e6b8e31`); copy byte-identically to the camel-lint schema path.
2. Regenerate TS bindings the same way the `Cache.on_miss` change did (the ts_rs
   export step in the schema/xtask flow — `schemas/ts/*.ts`).
3. Document `circuit_breaker.fallback` in the route-config doc page: YAML shape,
   semantics (runs when circuit open, both runtime branches), the clean-stop behavior
   on peek MISS, and a cross-reference from the `cache_peek_stale` docs
   (`docs/src/eip/cache.md`) to the composition pattern. English prose, STE style.
4. Add an example route under `examples/` mirroring
   `examples/cache-example/routes.yaml` conventions: a route with
   a `circuit_breaker.fallback` list containing a `cache_peek_stale` step, plus comments explaining the
   stale-on-error composition.
5. Build the docs to verify links/anchors: `nix shell nixpkgs#mdbook -c mdbook build
   docs` (or skip if unavailable — then verify markdown by inspection and note it).

**Tests:**
- `name`: `schema_check_passes`
  `setup`: regenerated schemas committed.
  `action`: run the gate.
  `assert`: exit 0; the two route-schema.json copies are byte-identical
  (`diff schemas/dsl/route-schema.json crates/camel-lint/schema/route-schema.json`).
  `command`: `cargo xtask schema --check`
  `expected`: fails before regen (schema lacks the fallback field), passes after.
- `name`: `schema_validation_accepts_fallback`
  `setup`: existing schema-validation test suite in
  `crates/camel-dsl/tests/schema_validation.rs`.
  `action`: add a case validating a route with `circuit_breaker.fallback` against the
  schema; also one rejecting `unknown_key` under `circuit_breaker`.
  `assert`: accept-case passes, reject-case fails validation.
  `command`: `cargo test -p camel-dsl --test schema_validation`
  `expected`: fails before regen, passes after.

**Acceptance:**
- `cargo xtask schema --check` exits 0.
- `cargo test -p camel-dsl --test schema_validation` passes.
- Docs build (or inspection-verified) with no broken anchors.

- [x] 4.1
