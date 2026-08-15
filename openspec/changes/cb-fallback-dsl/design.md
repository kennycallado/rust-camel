# Design: cb-fallback-dsl

Full evidence trail: RESEARCH-e_opus.md in this directory (file:line citations for every
claim below).

## Approach

Route-level CB `fallback:` sub-pipeline, threaded as UNRESOLVED steps to camel-core and
compiled there via the existing `StepCompilerRegistry`. Unresolved-step threading follows
`cache_peek_stale.on_miss`; lifecycle packing follows `Cache.on_miss` / IdempotentConsumer
(step_compilers/core.rs:105-119, 151-166). No new Stop handling — the
composed fallback pipeline already surfaces Stop as `Ok(ex)` at its own
`into_tower_result` boundary (ADR-0024), and both CB runtime consumers already return
that `Ok` (gate: route_compiler.rs:683-684; layer: circuit_breaker.rs:167).

1. **AST** — `RouteDslCircuitBreaker` (route_ast.rs:176) gains
   `#[serde(default)] pub fallback: Vec<RouteDslStep>`; `deny_unknown_fields` stays.
2. **Declarative** — `DeclarativeCircuitBreaker` (model.rs) carries
   `fallback: Vec<DeclarativeStep>`; yaml.rs:329-341 converts the AST steps with the
   existing AST→DeclarativeStep helper.
3. **RouteDefinition sidecar** — add a `pub(crate)` sidecar
   `circuit_breaker_fallback: Vec<BuilderStep>` (route_definition.rs, sibling of
   `circuit_breaker` at :321) + `with_circuit_breaker_fallback` setter (mirror :389).
   The resolved `CircuitBreakerConfig.fallback` (BoxProcessor) is NOT populated by the
   DSL layer — it stays `None` until camel-core compiles the sidecar.
4. **Canonical** — `CanonicalCircuitBreakerSpec` gains
   `#[serde(default, skip_serializing_if = "Vec::is_empty")] fallback:
   Vec<CanonicalStepSpec>` (mirrors `Cache.on_miss`; runtime.rs:153-159). DSL ↔ canonical
   round-trips losslessly; existing serialized specs deserialize with empty fallback.
5. **camel-core compile (the load-bearing change).** In `compile_route_impl`
   (route_compiler_ext.rs:556-637), **before** the `collect_lifecycle` at :582, resolve
   `def.circuit_breaker_fallback` via `resolve_steps` (:297) → `Vec<CompiledStep>`,
   **merge its lifecycle handles into the route `lifecycle` vec** via `collect_lifecycle`
   (route_helpers.rs:38 — mirrors how on_miss child lifecycles reach the route vec via the
   packed `Segment.lifecycle`), then compose into a `BoxProcessor` with
   **`compose_traced_pipeline_with_contracts`** (route_compiler.rs:237 — body-contract
   coercion parity with on_miss's `BodyCoercingSegment`; `compose_traced_pipeline` and
   `compose_pipeline` do NOT coerce). Attach via `CircuitBreakerConfig::fallback(fb)` and
   pass the config into `build_eh_config_pipeline`. This single attach point feeds **both**
   runtime branches: the `CircuitBreakerGate` (`eh_config = Some`, :222; reads
   `config.fallback` at circuit_breaker.rs:283/293) **and** the Tower
   `CircuitBreakerService` (`eh_config = None`, :245-247; reads `config.fallback` at
   circuit_breaker.rs:118/166-168). Both are live for the DSL surface; tests cover both.
6. **Outcome semantics — NO production code; regression tests only.** Tests SHALL cover
   both runtime branches, lifecycle start/shutdown for a stateful fallback, and the
   failing-fallback asymmetry below. A stopping / peek-MISS
   fallback yields `Ok(ex)` on **both** paths (gate: route_compiler.rs:683-684; layer:
   circuit_breaker.rs:167) because the composed fallback pipeline maps `Stopped→Ok` at its
   own `into_tower_result` boundary (ADR-0024/0025). **Documented asymmetry for a *failing*
   (genuine `Err`) fallback:** the gate routes it through
   `handle_boundary(BoundaryKind::CircuitBreaker, …)` (route_compiler.rs:685-688 → DLC /
   disposition); the layer surfaces the raw `Err` to the caller (circuit_breaker.rs:167) —
   correct, because the `eh_config = None` branch has no error handler to route to. Spec
   deltas stay path-agnostic on the failure case; the clean-stop case is identical on both.
   Adding a Stop→Ok shim at any caller would duplicate the ADR-0024
   single-translation-site rule — we assert the behavior instead.
7. **validate_contract** — recurse step validation into `cb.fallback`
   (runtime.rs, after the CB scalar checks; mirror the top-level loop at :461).
8. **Builder canonical path** — `canonicalize_circuit_breaker` (camel-builder
   lib.rs:1107) hard-errors when `config.fallback.is_some()` (unrecoverable BoxProcessor;
   ADR-0016 "No silent loss"). DSL/canonical authors are unaffected.
9. **Schema/docs/example** — regen both route-schema.json copies + `schemas/ts/*.ts`;
   document `circuit_breaker.fallback` in docs/src/yaml-dsl/; one example route:
   CB → `cache_peek_stale` stale-on-error composition.

**Spec reconciliation**: the eip-cache canon scenario is amended from the aspirational
CB-as-step shape (`circuitBreaker: { steps:, fallback: }`) to the implemented route-level
shape (`circuit_breaker: { failure_threshold:, fallback: }` on the route). CB-as-step is
rejected: the runtime wraps CB as a route layer (ADR-0025 OutcomePipeline segments), and
a step-verb redesign buys nothing for the anchor use case.

## Affected crates

- camel-dsl: route_ast.rs (AST field), yaml.rs (conversion + tests), compile.rs
  (declarative fallback → BuilderStep, canonical map at :288, canonical rehydrate at :366
  + tests).
- camel-api: runtime.rs — `CanonicalCircuitBreakerSpec.fallback` field +
  `validate_contract` recursion.
- camel-core: route_definition.rs (sidecar field + setter), route_compiler_ext.rs —
  fallback resolve + lifecycle-merge + compose + attach, **before the `collect_lifecycle`
  at :582**; optionally a `pub(crate) fn compile_cb_fallback` helper sibling of
  `resolve_steps`. **This is the load-bearing change.**
- camel-processor: none — both fallback consumers (gate at circuit_breaker.rs:283/293,
  Tower service at :118/:166-168) already read `config.fallback`; no gate or service edit
  is needed.
- camel-builder: lib.rs:1107 (fail-closed on opaque fallback) + struct literal updates.
- schemas: schemas/dsl/route-schema.json, crates/camel-lint/schema/route-schema.json,
  schemas/ts/*.ts (regen).
- docs, examples.

## Architecture boundaries

DSL/declarative/canonical planes only ever carry UNRESOLVED steps (`Vec<…Step>`);
processor construction is camel-core's monopoly via `StepCompilerRegistry`
(step_resolution.rs:206). CB fallback obeys the same rule as `cache_peek_stale.on_miss`.
No new error→outcome translation is introduced: Stop stays a pipeline-boundary concern
owned by `run_steps`/`into_tower_result` (ADR-0024/0025), never re-implemented at the CB
caller. Canonical spec stays lossless for DSL/canonical authors; the builder path — the
only place fallback arrives pre-compiled — fails closed per ADR-0016.

Single-phase change: one coherent vertical slice (field → threading → camel-core compile
→ contract guards → schema/docs), no milestone grouping needed.

## Alternatives considered

- **BoxProcessor produced in camel-dsl**: impossible — needs
  StepCompilerRegistry/ProducerContext/CacheRegistry (camel-core only).
- **Unresolved steps inside `CircuitBreakerConfig`**: rejected — its `fallback` is
  `Option<BoxProcessor>` (a runtime type); a `Vec<BuilderStep>` there would pollute the
  camel-api contract with a compile-stage concept.
- **`Err(CamelError::Stopped) => Ok(ex)` at the CB caller**: impossible (variant removed
  by ADR-0024) AND redundant (the composed fallback pipeline already returns `Ok` for
  Stop) AND a single-translation-site violation.
- **Canonical-lossy fallback everywhere**: rejected for DSL/canonical (steps stay
  `Vec`, round-trip is free) but REQUIRED at the builder (opaque BoxProcessor).
- **CB-as-step verb** (literal spec shape): rejected — YAGNI; contradicts the
  route-layer CB model.
