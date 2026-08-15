# Research report: cb-fallback-dsl redesign (e_opus)

READ-ONLY research. No source modified. Every claim carries `file:line` evidence
from the `cb-fallback-dsl` worktree.

## Headline verdict

The blessing REJECT stands, and the true depth is worse than e_gpt found. Two of the
four findings need reframing, and a **new CRITICAL** overrides design.md §5 entirely:

- **NEW CRITICAL (C0): `CamelError::Stopped` does not exist.** ADR-0024 removed the
  variant "entirely… no `#[deprecated]`, no alias" (docs/adr/0024:30, :101-103,
  :207). design.md:24-28, proposal.md:33-34, and both spec deltas
  (specs/dsl/spec.md:39-41, specs/eip-cache/spec.md:30-31) all instruct
  `Err(CamelError::Stopped(ex)) => Ok(ex)`. That arm **cannot compile**. The whole
  "Stopped mapping" section is built on a removed type. This is the single largest
  reason a re-bless would fail as written.

- **C1 (e_gpt) CONFIRMED and sharpened:** camel-dsl cannot build a `BoxProcessor`;
  worse, the resolved `CircuitBreakerConfig` already *owns* the `Option<BoxProcessor>`
  fallback (camel-api/src/circuit_breaker.rs:31), so the unresolved steps cannot ride
  inside it. They need a **new sidecar carrier**.

- **C2 (e_gpt) CONFIRMED and re-scoped:** the fallback is invoked through **exactly one
  live path** — `CircuitBreakerGate` (route_compiler.rs:676-698), NOT the Tower
  `CircuitBreakerService` (that path is dead for CB-with-error-handler routes; see B).

- **I3 (validate_contract recursion) CONFIRMED** — trivially correct fix.

- **I4 (builder reconstruction lossy) CONFIRMED** — precedent exists; pick the
  loss-report escape hatch, not a panic.

---

## A. Layering / threading redesign

### A.1 The exact route today (YAML → running route), CB path

1. `RouteDslCircuitBreaker` AST — route_ast.rs:176, `#[serde(deny_unknown_fields)]` at
   :175, fields `failure_threshold` + `open_duration_ms` only.
2. yaml.rs:329-341 converts → `DeclarativeCircuitBreaker { failure_threshold,
   open_duration_ms }` (validates `>0` at :332), attached at :464.
3. `DeclarativeRoute` → two forks:
   - **Programmatic/add_route fork:** compile.rs:228-230
     `compile_declarative_route(...)` → `compile_circuit_breaker(def)` (compile.rs:836)
     → `CircuitBreakerConfig::new().failure_threshold(..).open_duration(..)` →
     `definition.with_circuit_breaker(config)` → stored on `RouteDefinition`
     (route_definition.rs:321 `circuit_breaker: Option<CircuitBreakerConfig>`).
   - **Canonical/hot-reload fork:** compile.rs:288-291 maps →
     `CanonicalCircuitBreakerSpec { failure_threshold, open_duration_ms }`
     (runtime.rs:289-293). Rehydration compile.rs:366-371 rebuilds a
     `CircuitBreakerConfig` from the two scalars.
4. `RouteDefinition` → `compile_route_impl` (route_compiler_ext.rs:556):
   - main steps compiled at :565 `detect_and_validate_route_split(def.steps,…)` →
     `resolve_steps` (step_resolution.rs:191) → `build_registry()` +
     `CompilationContext` → `Vec<CompiledStep>`.
   - `def.circuit_breaker` passed at :602 into `build_eh_config_pipeline`
     (route_compiler_ext.rs:167,180).
   - :222 `let cb_gate = circuit_breaker.map(CircuitBreakerGate::new);` — the **resolved**
     config (fallback `BoxProcessor` and all) becomes the gate.
5. `RouteChannelService::new(handler, security, cb_gate, pipeline, …)`
   (route_compiler.rs:605) → Gate order Security→CB→Pipeline (route_compiler.rs:590).

### A.2 Where `cache_peek_stale.on_miss` (the precedent) ACTUALLY compiles to processors

**In camel-core, not camel-dsl.** The dsl side only threads `Vec<CanonicalStepSpec>` /
`Vec<BuilderStep>`:
- Canonical: compile.rs:440-457 `CanonicalStepSpec::Cache { on_miss } →
  BuilderStep::Cache { on_miss: compile_canonical_steps(on_miss) }` — still
  `Vec<BuilderStep>`, no processor.
- The processor is born in camel-core: **step_compilers/core.rs:151-158** —
  `ctx.compile_children_segments(on_miss, registry)` → `compose_outcome_segment(...)` →
  fed to `CacheService::new(...)` → wrapped as `CompiledStep::Segment` (core.rs:159-165).

So the DSL precedent is: **carry unresolved `Vec<…Step>` all the way to camel-core; the
`StepCompilerRegistry` turns it into a processor/segment there.** The CB fallback must
reuse this identical shape.

### A.3 What RouteDefinition must carry instead of a resolved BoxProcessor

`CircuitBreakerConfig.fallback` is `Option<BoxProcessor>` (camel-api/circuit_breaker.rs:31)
— a runtime type camel-dsl cannot construct (needs `StepCompilerRegistry`,
`ProducerContext`, `CacheRegistry`; step_resolution.rs:206-214). Therefore:

**Add a sidecar field on `RouteDefinition`:**
```
pub(crate) circuit_breaker_fallback: Vec<BuilderStep>,   // route_definition.rs, near :321
```
Empty vec = no fallback (backward compatible; `with_circuit_breaker` leaves it empty).
A new `with_circuit_breaker_fallback(steps)` setter mirrors `with_circuit_breaker`
(route_definition.rs:389).

**Compile it in camel-core**, at `compile_route_impl` (route_compiler_ext.rs:565-603),
using the SAME registry already in scope:
```
// after detect_and_validate_route_split, before build_eh_config_pipeline
let mut cb = def.circuit_breaker;                        // Option<CircuitBreakerConfig>
if let (Some(cfg), false) = (cb.as_mut(), def.circuit_breaker_fallback.is_empty()) {
    let compiled = self.resolve_steps(def.circuit_breaker_fallback, &producer_ctx,
        self.registry, Some(&route_id), staging_mode)?;   // Vec<CompiledStep>
    let fb = compose_pipeline(compiled, build_pipeline_ctx(self.tracer_metrics, &route_id));
    *cfg = cfg.clone().fallback(fb);                       // BoxProcessor now attached
}
// pass `cb` (not def.circuit_breaker) at :602
```
`resolve_steps` returns `Vec<CompiledStep>` (step_resolution.rs:205) and
`compose_pipeline(Vec<CompiledStep>, ctx) -> BoxProcessor` (route_compiler.rs:114) is the
exact Vec→BoxProcessor bridge. This is production-clean and reuses proven machinery.

### A.4 Precedent for RouteDefinition carrying unresolved steps — YES, everywhere

`RouteDefinition.steps: Vec<BuilderStep>` is itself unresolved (route_definition.rs:317,
struct doc "to URIs have not been resolved to producers yet"). Every structural EIP
`BuilderStep` variant carries unresolved `Vec<BuilderStep>` sub-pipelines:
`Filter/Choice/Split/Multicast/Loop/DoTry` (route_definition.rs:99,104,120,145,168,190,
221,227,274) and the direct twin **`Cache { on_miss: Vec<BuilderStep> }`
(route_definition.rs:286)**. The pattern is named "unresolved route definition"
(route_definition.rs:314). CB fallback fits it exactly; the sidecar field is idiomatic.

**Naming note:** the fallback is a route-level layer, not a `BuilderStep`, so it lands as
a sibling field on `RouteDefinition` (like `error_handler`, `security_policy`,
route_definition.rs:319,322) rather than a step variant — consistent with CB's existing
non-step modeling (CONTEXT-MAP Key Terms: "CircuitBreaker … Not a Pipeline Step").

---

## B. Stopped mapping — the correct site set (design.md §5 is wrong)

### B.1 Enumerate fallback invocation paths

| # | Site | file:line | Live for CB-fallback routes? |
|---|------|-----------|------------------------------|
| 1 | `CircuitBreakerGate::before_call` → `CircuitBreakerDecision::Fallback(fb)` invoked via `invoke_processor(&mut fb, ex)` | route_compiler.rs:676-690 | **YES — the only live path** |
| 2 | Tower `CircuitBreakerService::call` Open branch `fallback.call(exchange)` | circuit_breaker.rs:161-168 | **NO for these routes** (see B.2) |
| 3 | `CircuitBreakerService::poll_ready` Open+fallback `Poll::Ready(Ok(()))` | circuit_breaker.rs:189-191 | tied to #2 |
| 4 | `CircuitBreakerGate::before_call` HalfOpen probe-in-flight `Fallback` | circuit_breaker.rs (before_call HalfOpen arm) | YES (same treatment as #1) |

### B.2 Why the Tower service path (#2/#3) is dead for these routes

A route-level `circuit_breaker` in the DSL is only meaningful when it reaches
`build_eh_config_pipeline`. That function has **two branches** (route_compiler_ext.rs:182
vs :232). CB fallback (the `CircuitBreakerGate`) exists **only in the `eh_config = Some`
branch** (:222). The `eh_config = None` branch wraps the Tower `CircuitBreakerLayer`
(:245-247) — and today that path never gets a fallback because nothing sets it. So for the
DSL surface we're adding, **the gate path (#1) is the sole runtime path.** The Tower
`CircuitBreakerService` fallback branch is legacy/programmatic-only.

Design.md:23-27 targets circuit_breaker.rs:166-167 (the Tower service) — the **wrong
site**. The correct site is route_compiler.rs:676-690.

### B.3 What actually needs mapping — and it's NOT `Err(Stopped)`

Because `CamelError::Stopped` is gone (C0), a stopped fallback sub-pipeline does not
produce `Err(Stopped)`. Trace what a stopping fallback yields:

- The fallback is a `BoxProcessor` built by `compose_pipeline` →
  `SequentialPipeline` (route_compiler.rs:118). Its `Service::call` runs `run_steps` and
  translates via `into_tower_result()` — **`Completed(ex)` AND `Stopped(ex)` both map to
  `Ok(ex)`** (route_compiler.rs:323, :371; ADR-0024:125-128). A `Stop`/`CamelStop` inside
  the fallback therefore already surfaces as **`Ok(ex)`**, not an error.

**Consequence:** at route_compiler.rs:680 `invoke_processor(&mut fb, ex).await` already
returns `Ok(result)` for a stopped fallback. The gate's existing arm
`Ok(result) => return Ok(result)` (route_compiler.rs:681) **already does the right
thing.** No `Err→Ok` mapping is needed at all.

The ONLY residual question is `cache_peek_stale` MISS with `on_miss: stop`. That sets the
`CamelStop` property (ADR-0024:216-247) / returns `PipelineOutcome::Stopped` from the
`CachePeekStaleService` segment; `run_steps` catches it (route_compiler.rs:432,447,451)
and `into_tower_result` → `Ok(ex)`. **Verified end-to-end: the stop stays `Ok` before it
ever reaches the gate boundary.**

### B.4 The MINIMAL correct mapping set

**Zero new Stopped mappings are required.** The design's entire §5 dissolves. What the
change MUST do instead is *assert* (regression tests) that:
1. A fallback whose last step is `Stop` returns `Ok(ex)` out of the gate
   (route_compiler.rs:681), Exchange state intact.
2. A `cache_peek_stale` MISS→stop fallback returns `Ok(ex)`, never `CircuitOpen`, never a
   `Failed`/error at the boundary.

What ADR-0024 dictates about *where* Stop translation lives: it belongs at the pipeline
executor boundary (`run_steps` / `into_tower_result`), NOT scattered at each caller
(ADR-0024:87 "adapter lives at exactly one site per pipeline"; ADR-0025 invariant 6
"`PipelineOutcome` never becomes public `Service<Exchange>::Response`"). Since the fallback
is itself a composed pipeline, it already owns that boundary. **Adding a Stop→Ok shim at
the gate would duplicate the boundary rule and violate the single-translation-site
principle.** This is the deep ADR-grounded reason the design.md approach is not just
non-compiling but architecturally wrong.

**Edge case to cover explicitly:** if the fallback pipeline *fails* (Err), the gate routes
it through `handle_boundary(BoundaryKind::CircuitBreaker, …)` (route_compiler.rs:684-688),
which returns `Ok(exchange_with_error)` for Propagate (error_handler.rs:344-346). That is
correct and unchanged — only genuine failures reach the DLC, stops do not.

---

## C. Canonical contract

### C.1 validate_contract recursion (I3 — confirmed, trivial)

`validate_contract` (runtime.rs:341) calls `validate_steps(&self.steps)` (:359) then checks
CB scalars (:360-373) but **never recurses into a fallback**. `validate_steps`
(runtime.rs:410) already recurses into Filter/Choice/Split/**Cache.on_miss**
(:461 `validate_steps(on_miss)?`). Fix: after the CB scalar checks, add
`validate_steps(&cb.fallback)?`. One line, mirrors the Cache precedent exactly.

### C.2 CanonicalCircuitBreakerSpec construction sites (workspace grep)

All sites that build `CanonicalCircuitBreakerSpec` and must gain the `fallback` field:
1. camel-api/src/runtime.rs:289 (definition) + :845,:852 (test builders)
2. camel-dsl/src/compile.rs:288 (declarative→canonical) — add
   `fallback: compile_declarative_steps_to_canonical(cb.fallback)`
3. camel-builder/src/lib.rs:1107 `canonicalize_circuit_breaker` (the lossy site, I4)
4. Tests: camel-core/tests/runtime_commands_test.rs:784,
   camel-core/src/lifecycle/application/commands_tests.rs:605
5. Rehydration read site: compile.rs:366-371 (canonical→RouteDefinition) — must set the
   new `RouteDefinition.circuit_breaker_fallback` sidecar from `cb.fallback` via
   `compile_canonical_steps`.

### C.3 The one-way (runtime→canonical) problem and its precedent (I4)

`canonicalize_circuit_breaker(config: CircuitBreakerConfig)` (camel-builder/lib.rs:1107)
receives an opaque `CircuitBreakerConfig` whose fallback is a `BoxProcessor` — **a
processor cannot be reversed into `Vec<CanonicalStepSpec>`.** This is the identical
"runtime type has no canonical inverse" problem already solved for `error_handler` and
`unit_of_work`.

**Precedent (authoritative):** ADR-0016 "Strict Rejection Policy" + "Lossy Escape Hatch".
`compile_declarative_route_to_canonical(route, allow_loss)` (compile.rs:249) already:
- errors by default for `error_handler`/`unit_of_work` (compile.rs:257-268),
- with `allow_loss=true`, records `CanonicalFieldLoss` into a `CanonicalLossReport`
  (compile.rs:270-283; types at runtime.rs:383,390).

**Decision:** the *builder→canonical* reconstruction path (`canonicalize_circuit_breaker`)
takes the **loss-report** route, NOT a panic and NOT a silent drop. Because the builder
path (lib.rs:800-818) doesn't currently thread a `CanonicalLossReport`, the minimal
correct move is:

- If `config.fallback.is_some()`, the builder cannot round-trip it. Return an explicit
  error from the builder's canonical path (`RouteError("circuit_breaker.fallback set via
  the programmatic builder cannot be lowered to a canonical spec; author the fallback in
  DSL/JSON or via a canonical spec directly")`). This matches ADR-0016 "No silent loss"
  and the existing `security_policy` "always error" tier (ADR-0016 Strict Rejection table)
  — the builder is the one entry where the fallback arrives pre-compiled and is therefore
  genuinely unrecoverable.
- The **DSL→canonical** path (compile.rs:288) has the raw `Vec` and round-trips
  **losslessly** (like `Cache.on_miss`), so it needs no loss marking.

This split is the crux: lossless where steps are still `Vec` (DSL, canonical), hard-error
only where they've already become a `BoxProcessor` (builder). It respects ADR-0016 and the
`CanonicalStepSpec`-is-serializable property the design correctly identified
(design.md:70-72) — while fixing the design's blind spot that the *builder* site has no
`Vec` to serialize.

---

## D. Schema / ts_rs

### D.1 Recursion compiles in both derives — precedent proven

`CanonicalStepSpec::Cache { on_miss: Vec<CanonicalStepSpec> }` (runtime.rs:153-159) already
recurses under the full derive set `Serialize, Deserialize, schemars::JsonSchema,
ts_rs::TS` (runtime.rs:104-117). Adding `fallback: Vec<CanonicalStepSpec>` to
`CanonicalCircuitBreakerSpec` (runtime.rs:277-293, same derive block :277-286) is the
identical recursive shape — **compiles in both derives** (the enum already self-references
via Cache/Filter/Choice, so the TS/schemars machinery handles the cycle). Mirror the
serde attrs from `CachePeekStale.on_miss` (runtime.rs:167-169):
`#[serde(default, skip_serializing_if = "Vec::is_empty")]` for backward-compat empty.

For the AST side, `RouteDslCircuitBreaker` (route_ast.rs:176) gains
`#[serde(default)] pub fallback: Vec<RouteDslStep>`; `RouteDslStep` is the established
recursive sub-pipeline type (route_ast.rs:297, used at :35,149,269,568…). Its derives
`#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]`
(route_ast.rs:173) already handle `Vec<RouteDslStep>` recursion elsewhere. `deny_unknown_fields`
stays (route_ast.rs:175) — satisfies the DSL "unknown fields rejected" scenario.

### D.2 TS export consumers to update

`schemas/ts/CanonicalCircuitBreakerSpec.ts` and `schemas/ts/CanonicalStepSpec.ts` are the
generated bindings (dir listing confirms both exist). `cargo xtask schema --check` (AGENTS
quality gate) regenerates and diff-checks them. **Regeneration is mechanical**; both
`schemas/dsl/route-schema.json` and `crates/camel-lint/schema/route-schema.json` are
byte-identical (59383 bytes each) and must be regenerated together. The CB block is at
route-schema.json:1640; `failure_threshold` at :1372; `on_miss` sub-pipeline schema
already present at :155,:223 — the `fallback` block reuses that step-array `$ref` shape.

---

## E. Revised design skeleton (paste-adapt into design.md)

```md
## Approach

Route-level CB `fallback:` sub-pipeline, threaded as UNRESOLVED steps to camel-core and
compiled there via the existing `StepCompilerRegistry`, exactly mirroring
`cache_peek_stale.on_miss` (step_compilers/core.rs:151-165). No new Stopped handling —
the composed fallback pipeline already surfaces Stop as `Ok(ex)` at its own
`into_tower_result` boundary (ADR-0024), and the CB gate already returns that `Ok`
verbatim (route_compiler.rs:681).

1. **AST** — `RouteDslCircuitBreaker` (route_ast.rs:176) gains
   `#[serde(default)] pub fallback: Vec<RouteDslStep>`; `deny_unknown_fields` stays.
2. **Declarative** — `DeclarativeCircuitBreaker` carries `fallback: Vec<BuilderStep>`;
   yaml.rs:329-341 converts the AST steps.
3. **RouteDefinition sidecar** — add `circuit_breaker_fallback: Vec<BuilderStep>`
   (route_definition.rs, sibling of `circuit_breaker`) + `with_circuit_breaker_fallback`.
   The resolved `CircuitBreakerConfig.fallback` (BoxProcessor) is NOT populated by the DSL
   layer — it stays `None` until camel-core compiles the sidecar.
4. **Canonical** — `CanonicalCircuitBreakerSpec` gains
   `#[serde(default, skip_serializing_if = "Vec::is_empty")] fallback: Vec<CanonicalStepSpec>`
   (mirrors `Cache.on_miss`; runtime.rs:153-159). DSL↔canonical round-trips losslessly.
5. **camel-core compile** — in `compile_route_impl` (route_compiler_ext.rs:565-603),
   after `detect_and_validate_route_split`, resolve `def.circuit_breaker_fallback` via
   `resolve_steps` → `compose_pipeline` → `BoxProcessor`, attach with
   `CircuitBreakerConfig::fallback(fb)`, then pass into `build_eh_config_pipeline`.
6. **Stopped semantics — NO code change; regression tests only.** Assert a stopped /
   peek-MISS fallback yields `Ok(ex)` out of the gate (route_compiler.rs:681) and never
   `CircuitOpen`/`Failed`. (Removes the non-compiling `Err(CamelError::Stopped)` mapping
   from the prior design — that variant was deleted by ADR-0024.)
7. **validate_contract** — recurse `validate_steps(&cb.fallback)` (runtime.rs, after CB
   scalar checks; mirror :461).
8. **Builder canonical path** — `canonicalize_circuit_breaker` (camel-builder/lib.rs:1107)
   hard-errors when `config.fallback.is_some()` (unrecoverable BoxProcessor; ADR-0016
   "No silent loss"). DSL/canonical authors are unaffected.
9. **Schema/docs/example** — regen both route-schema.json copies + ts bindings; document
   `circuit_breaker.fallback`; one example: CB → `cache_peek_stale` stale-on-error.

## Affected crates
- camel-dsl: route_ast.rs (AST field), yaml.rs (conversion), compile.rs (declarative
  fallback→BuilderStep, canonical map at :288, canonical rehydrate at :366).
- camel-api: runtime.rs (CanonicalCircuitBreakerSpec.fallback + validate recursion).
- camel-core: route_definition.rs (sidecar field + setter), route_compiler_ext.rs
  (fallback compile at :565-603). This is the load-bearing change the prior design missed.
- camel-processor: NONE (design.md's fallback-call mapping is deleted). Only if we choose
  to also wire the legacy Tower `CircuitBreakerService` fallback (out of scope).
- camel-builder: lib.rs:1107 (hard-error on opaque fallback) + struct literals.
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

## Alternatives considered
- **BoxProcessor produced in camel-dsl** (prior design): impossible — needs
  StepCompilerRegistry/ProducerContext/CacheRegistry (camel-core only).
- **Stuff unresolved steps inside CircuitBreakerConfig**: rejected — that struct's
  `fallback` is `Option<BoxProcessor>` (a runtime type); a `Vec<BuilderStep>` there would
  pollute the camel-api contract with a compile-stage concept.
- **`Err(CamelError::Stopped) => Ok(ex)` at the CB caller** (prior design §5): impossible
  (variant removed, ADR-0024) AND redundant (the composed fallback pipeline already
  returns `Ok` for Stop) AND an ADR-0024/0025 violation (duplicate boundary translation).
- **Canonical-lossy fallback everywhere**: rejected for DSL/canonical (steps are still
  `Vec`, round-trip is free) but REQUIRED at the builder (opaque BoxProcessor).
- **CB-as-step verb**: rejected — YAGNI, contradicts route-layer CB model (unchanged from
  prior design; still correct).
```

## E.2 tasks.md boundaries (not full text)

- **Task 1 — AST + declarative + canonical fields** (camel-dsl route_ast.rs, yaml.rs,
  compile.rs:288/:366; camel-api runtime.rs field). Unit: parse, deny_unknown_fields,
  serde default/skip-empty, DSL↔canonical round-trip (mirror runtime.rs:969 test).
- **Task 2 — RouteDefinition sidecar + camel-core compile**
  (route_definition.rs field/setter; route_compiler_ext.rs:565-603 fallback resolution).
  Test: open circuit runs fallback; body from fallback.
- **Task 3 — validate_contract recursion** (runtime.rs). Test: invalid nested fallback
  step rejected.
- **Task 4 — Stopped/peek-MISS regression tests** (camel-core integration; NO prod code).
  Assert `Ok(ex)`, no `CircuitOpen`, Exchange state intact.
- **Task 5 — builder fail-closed** (camel-builder lib.rs:1107 + struct literals + tests).
- **Task 6 — schema + ts regen + docs + example**; `cargo xtask schema --check` green.

Task boundaries are phase-coherent: 1-2 are the vertical thread, 3-5 are contract
guards, 6 is surface. No task leaves a non-compiling intermediate (each adds a
serde-defaulted field or an additive setter).

---

## F. Spec-delta sanity re-check

The 8 scenarios mostly hold; **two must change** because they assert the deleted-variant
mapping, and one compile-level assertion should move to a runtime-observable.

- **specs/dsl/spec.md:34-41 "stopped fallback yields a clean outcome":** the THEN clause
  "`Err(Stopped)` never escapes … (mirror of run_steps Stop bypass)" references a removed
  variant. **Rewrite THEN:** "the circuit breaker gate returns `Ok(exchange)` with
  Exchange state intact; no `CircuitOpen` and no error escape — because the composed
  fallback pipeline already translates Stop to `Ok` at its `into_tower_result` boundary
  (ADR-0024/0025)." Drop the "mirror … bypass" clause.
- **specs/eip-cache/spec.md:24-31 "fallback miss yields a clean outcome":** same edit —
  replace "no `Err(Stopped)` … escapes the circuit breaker fallback branch" with the
  `Ok(exchange)`/no-`CircuitOpen` phrasing above.
- **specs/dsl/spec.md:12-18 "fallback declared in YAML parses and compiles":** THEN asserts
  "resulting `CircuitBreakerConfig` has a fallback processor configured." Under the
  redesign the `CircuitBreakerConfig.fallback` is populated **in camel-core at route
  compile**, not by the DSL compile step. **Tighten THEN** to: "parsing succeeds; the
  compiled route's `CircuitBreakerGate` returns `Fallback(_)` when open (or: the route
  compiles without error and the fallback runs when the circuit is open)." This moves the
  assertion from a DSL-layer struct field (which is now `None` at DSL stage) to a runtime
  observable — otherwise the scenario is false under the corrected layering.
- Scenarios "absent/empty unchanged", "open circuit executes fallback", "canonical
  roundtrip preserves fallback steps", "unknown fields rejected", and the eip-cache
  "serves stale entry" scenario all remain valid as written.

---

## Verdict on re-bless

**With the C0/§B correction, C1 sidecar, I3, and I4 fail-closed applied — a re-bless will
very likely PASS.** The redesign now:
- compiles (removes the phantom `CamelError::Stopped` arm),
- targets the correct crate (camel-core, the missing load-bearing change),
- reuses an in-tree, tested precedent (`cache_peek_stale.on_miss`) end-to-end,
- respects ADR-0016 (no silent loss) and ADR-0024/0025 (single Stop-translation site),
- keeps the canonical contract lossless where it can be and fail-closed where it cannot.

**Residual risk the re-blesser will probe:**
1. The two spec-delta THEN clauses MUST be edited (F) — a blesser will reject if the specs
   still name `Err(Stopped)`.
2. design.md MUST drop camel-processor as an affected crate for the Stopped mapping and add
   camel-core route_compiler_ext.rs as the primary site — the prior "Affected crates" list
   (design.md:40-50) omits camel-core entirely, which is the disqualifying gap.
3. Confirm at implementation time that `compose_pipeline` (no handler) is the right
   composer for the fallback vs `compose_traced_pipeline` — pick traced for observability
   parity (ADR-0012), minor.
