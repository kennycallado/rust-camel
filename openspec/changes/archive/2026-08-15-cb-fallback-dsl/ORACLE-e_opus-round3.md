# Oracle report: cb-fallback-dsl round-3 unblock (e_opus)

READ-ONLY diagnosis. No source modified. Every claim carries `file:line` evidence from
the `cb-fallback-dsl` worktree. This report supersedes `RESEARCH-e_opus.md` where the two
conflict (the composer and the compile-site) and drives the FINAL design.md revision.

---

## Headline verdict

The redesign is fundamentally sound (sidecar `Vec<BuilderStep>` on `RouteDefinition`,
compile in camel-core, no phantom `CamelError::Stopped`). But it is still **not airtight**:
`RESEARCH-e_opus.md` §A.3 and design.md §5 pick the **wrong compile site** and the **wrong
composer**, and both **miss the two live consumers** e_gpt flagged in R2. Three concrete
mechanism corrections make round 3 blessable. All are verified line-by-line below.

**The three load-bearing corrections:**

1. **Compile site (R2-C2):** the fallback MUST be resolved+composed **inside
   `compile_route_impl` BEFORE line 582 `collect_lifecycle`**, and its lifecycle handles
   MUST be merged into the route `lifecycle` vec. RESEARCH §A.3 / design.md:27-33 put the
   compile "before `build_eh_config_pipeline`" (i.e. between :582 and :589) — that is
   *after* `collect_lifecycle`, so stateful fallback steps (aggregator sweep, wire-tap
   drain, cache/idempotent repos) get **no route start/shutdown**. Confirmed silent-loss bug.

2. **Composer (R2-I):** the fallback MUST use **`compose_traced_pipeline_with_contracts`**
   (route_compiler.rs:237), NOT `compose_traced_pipeline` (:149) as RESEARCH §E.5 waffled,
   and NOT `compose_pipeline` (:114) as design.md:315 states. Only the `_with_contracts`
   variant applies body-contract coercion (`wrap_if_needed`), which is exactly what the
   on_miss precedent gets via `BodyCoercingSegment`. Using a non-coercing composer drops
   body contracts on the fallback steps — a divergence from every other sub-pipeline.

3. **Two-branch ownership (R2-C1):** attaching the compiled `BoxProcessor` to
   `CircuitBreakerConfig.fallback` **before** `build_eh_config_pipeline` feeds **BOTH**
   branches (gate + Tower layer). The design/spec/RESEARCH all declare the layer path
   "dead" — it is **live**, reads `config.fallback` at three sites, and must be owned,
   asymmetry-documented, and tested.

---

## A. The on_miss precedent — full call chain (mandate A)

`cache_peek_stale.on_miss` is NOT the precedent that matters for lifecycle — it passes
`on_miss` raw to the service (step_compilers/core.rs:199) and its `CompiledStep::Segment`
carries `lifecycle: None` (core.rs:203). The **structurally identical** precedent is
`Cache.on_miss` / `IdempotentConsumer` (core.rs:105-119, :151-166), which pack child
lifecycles. Full chain:

```
CoreCompiler::compile  (BuilderStep::Cache arm)   core.rs:125-167
 └─ ctx.compile_children_segments(on_miss, registry)          core.rs:151
     step_compilers/mod.rs:139-194
      ├─ self.compile_children(steps, registry)               mod.rs:150 → Vec<CompiledStep>
      ├─ for each CompiledStep:
      │   Process{lifecycle}  → push lc into lifecycle_handles  mod.rs:160-162
      │   Segment{lifecycle}  → extend lifecycle_handles        mod.rs:186-188
      │   (Process wrapped in BoxProcessorSegment, then
      │    BodyCoercingSegment IF body_contract.is_some())      mod.rs:163-175  ← CONTRACT COERCION
      └─ returns (Vec<Box<dyn OutcomePipeline>>, Vec<Arc<dyn StepLifecycle>>)
 └─ compose_outcome_segment(child_segments)                   core.rs:153 → OutcomeSegment
 └─ CacheService::new(..., child_pipeline, ...)               core.rs:154
 └─ CompiledStep::Segment {
        segment,
        body_contract: None,
        lifecycle: pack_lifecycles(child_lifecycles),          core.rs:165  ← LIFECYCLE PACKED HERE
    }
```

Then, because that `Segment` is a member of `processors_with_contracts`, the route-level
`collect_lifecycle` (route_helpers.rs:38-56) picks up `Segment{lifecycle}` at :47-51 and
merges the child handles into the route lifecycle vec **for free**. That "for free" is the
whole trick — and it is exactly what CB fallback CANNOT rely on, because the fallback is a
route-level layer, not a member of `processors_with_contracts`.

**Contract coercion in on_miss:** `BodyCoercingSegment` (mod.rs:169-173). The fallback's
`BoxProcessor` analog is `wrap_if_needed` inside `compose_*_with_contracts`
(route_compiler.rs:219,263). These are the two faces of the same coercion; the fallback
must use the `_with_contracts` composer to match.

---

## B. compile_route_impl, line-by-line — the single attach point (mandate B)

```
compile_route_impl(def, staging_mode)                        route_compiler_ext.rs:556
 :561  route_id = def.route_id()
 :563  producer_ctx = build_producer_context
 :565  detect_and_validate_route_split(def.steps, ...)       ← def.steps MOVED here
        → (aggregate_split, processors_with_contracts)
 :575  reject aggregate_split
 :582  let lifecycle = collect_lifecycle(&processors_with_contracts)   ← LIFECYCLE COLLECTION
 :584  eh_config = def.error_handler.or(global)
 :589  build_eh_config_pipeline(..., def.security_policy, def.circuit_breaker)  ← BRANCH SPLIT
 :606  UoW wrap
 :633  return CompiledPipeline { processor: pipeline, lifecycle }
```

**Key finding:** `def.steps` is moved at :565, but `def.circuit_breaker` is not consumed
until :602. The sidecar `def.circuit_breaker_fallback: Vec<BuilderStep>` must therefore be
**taken by value before :565** (e.g. `let cb_fallback = std::mem::take(&mut def)`… — but
`def` is owned, so `let cb_fallback = def.circuit_breaker_fallback;` and
`let cb_cfg = def.circuit_breaker;` extracted up front, `def` rebound `mut`). Concretely the
single attach point is **between :582 and :589** for the *compose+attach*, but the
*lifecycle merge* must happen **at/after the compose and be folded into the `:582` vec**.
Ordering that satisfies both:

```rust
// after :565 detect_and_validate_route_split, BEFORE :582 collect_lifecycle:
let mut lifecycle = super::route_helpers::collect_lifecycle(&processors_with_contracts);

// NEW: compile CB fallback sidecar (mirrors on_miss lifecycle packing)
let circuit_breaker = match def.circuit_breaker.take() {
    Some(mut cfg) if !def.circuit_breaker_fallback.is_empty() => {
        let fb_steps = std::mem::take(&mut def.circuit_breaker_fallback);
        let fb_compiled = self.resolve_steps(          // Vec<CompiledStep>, lifecycles intact
            fb_steps, &producer_ctx, self.registry, Some(&route_id), staging_mode,
        )?;
        // merge fallback lifecycle handles into the ROUTE lifecycle (the on_miss "for free"
        // step, made explicit because the fallback is not in processors_with_contracts):
        lifecycle.extend(super::route_helpers::collect_lifecycle(&fb_compiled));
        let fb = compose_traced_pipeline_with_contracts(   // BoxProcessor WITH coercion
            fb_compiled, &route_id, self.tracing_enabled,
            self.tracer_detail_level.clone(), self.tracer_metrics.clone(),
            None,                                          // no route error handler inside fallback
            build_pipeline_ctx(self.tracer_metrics, &route_id),
        );
        Some(cfg.fallback(fb))                             // BoxProcessor attached to CONFIG
    }
    other => other,                                        // None, or Some-without-fallback
};
// ...
let mut pipeline = build_eh_config_pipeline(
    eh_config.as_ref(), ..., def.security_policy,
    circuit_breaker,                                       // ← both branches now see fallback
)?;
```

- `resolve_steps` is the ext method at route_compiler_ext.rs:297-333 (already builds the
  `ControllerComponentContext` + registry + `rt`); returns `Vec<CompiledStep>`
  (step_resolution.rs) with lifecycle handles populated per step. It is the same call the
  main pipeline uses (:539). **No BoxProcessor leaks into camel-dsl** — the DSL only threads
  `Vec<BuilderStep>` (design invariant intact).
- `compose_traced_pipeline_with_contracts` (route_compiler.rs:237) is **`pub(crate)`** and
  **already imported** at route_compiler_ext.rs:37 and used at :206/:235. Directly callable.
- `collect_lifecycle` (route_helpers.rs:38) is **`pub(crate)`**, already used at :582.
- `build_pipeline_ctx(self.tracer_metrics, &route_id)` is already used at :204/:234/:378.

This is the **single attach point that feeds both branches**: the compiled `BoxProcessor`
lands on `CircuitBreakerConfig.fallback` (camel-api/circuit_breaker.rs:31, setter :71) BEFORE
`build_eh_config_pipeline` consumes the config at :589/:602.

---

## C. Two-branch verification (mandate C) — the layer path is LIVE

`build_eh_config_pipeline` (route_compiler_ext.rs:167-257) has two branches:

| Branch | Condition | CB wiring | Reads `config.fallback`? |
|---|---|---|---|
| **Gate** | `eh_config = Some` (:182) | `CircuitBreakerGate::new(config)` (:222) | **YES** — `before_call` :283, :293 → `Fallback(fallback.clone())` |
| **Layer** | `eh_config = None` (:232) | `CircuitBreakerLayer::new(config).layer(pipeline)` (:245-247) | **YES** — `CircuitBreakerService::poll_ready` :118 (`config.fallback.is_some()`) and `call` :166-168 (`config.fallback.clone()` → `fallback.call(exchange)`) |

**Both branches read `config.fallback`.** Attaching the `BoxProcessor` to the config before
:589 makes BOTH branches serve it. RESEARCH §B.2 (lines 138-149) and design.md:31-33 assert
the layer path is "dead / legacy / programmatic-only" — **this is FALSE and is exactly the
R2-C1 disqualifier**. A route with a `circuit_breaker` and **no error_handler** (neither
route-level nor `global_error_handler`) hits `eh_config = None` at :182/:232 and gets the
Tower `CircuitBreakerService`. That is a first-class DSL-reachable configuration.

**No other consumer of `RouteDefinition.circuit_breaker` sees a half-compiled fallback.**
The sidecar `circuit_breaker_fallback: Vec<BuilderStep>` is only read in `compile_route_impl`;
the `CircuitBreakerConfig.fallback` stays `None` at DSL/canonical/builder stages and is
populated exactly once, in camel-core, at compile. Serialization/canonicalization never touch
a `BoxProcessor` (they operate on the `Vec` sidecar / `CanonicalStepSpec` — lossless), so
there is no half-compiled-state exposure.

---

## D. Lifecycle packing for the fallback (mandate D)

**Who owns packing in on_miss:** the *step compiler* (core.rs:165 `pack_lifecycles`), and the
route-level `collect_lifecycle` then harvests the `Segment.lifecycle` for free (route_helpers
.rs:47-51). For CB fallback there is **no step compiler and no `CompiledStep::Segment`** in the
route vec, so nothing harvests automatically.

**Can the fallback reuse the on_miss helpers directly?**
- `compile_children_segments` / `compose_outcome_segment` require a `CompilationContext`
  (`&self` is the step compiler's `CompilationContext`, mod.rs:139) and produce an
  `OutcomeSegment`, NOT a `BoxProcessor`. The CB gate/service need a `BoxProcessor`
  (`Option<BoxProcessor>`), so the outcome-segment helpers are the **wrong shape**.
- The correct route-level equivalent is: `resolve_steps` (→ `Vec<CompiledStep>`, lifecycles
  intact) + `collect_lifecycle` (harvest) + `compose_traced_pipeline_with_contracts` (→
  `BoxProcessor` with coercion). All three are `pub(crate)`/ext-method and callable from
  `compile_route_impl` (verified §B).

**Exact mechanism (proposed):** inline in `compile_route_impl` per the §B snippet (no new
free function strictly required — every helper is in scope). If task authors prefer a named
helper for testability, add a `pub(crate) fn compile_cb_fallback(&self, steps, producer_ctx,
route_id, staging_mode) -> Result<(BoxProcessor, Vec<Arc<dyn StepLifecycle>>), CamelError>`
in `route_compiler_ext.rs` (sibling of `resolve_steps`). Signature is fully satisfiable from
`RouteCompilerExt` fields. **This is the crux fix RESEARCH §A.3 missed**: it merges fallback
lifecycles into the route vec, which RESEARCH's "compile before build_eh_config_pipeline"
placement (after :582) structurally cannot do.

---

## E. handle_boundary asymmetry (mandate E) — must be documented, spec must be path-agnostic

**Stopped/clean-outcome path (identical on both branches):** a stopping fallback returns
`Ok(ex)` regardless of branch, because the fallback `BoxProcessor` is a `TracedPipeline`/
`SequentialPipeline` whose `into_tower_result()` maps `Stopped(ex) → Ok(ex)`
(route_compiler.rs:112 doc, ADR-0024). Gate: `invoke_processor(&mut fb, ex)` → `Ok(result)`
→ `return Ok(result)` (route_compiler.rs:683-684). Layer: `fallback.call(exchange)` → `Ok(ex)`
returned raw (circuit_breaker.rs:167). **Both surface `Ok(ex)` for a stop. Spec scenarios
"stopped fallback yields a clean outcome" and "fallback miss yields a clean outcome" hold on
BOTH paths.**

**Genuine-failure path (ASYMMETRIC):**
- **Gate:** fallback `Err(e)` → `handler.handle_boundary(BoundaryKind::CircuitBreaker, original,
  err)` (route_compiler.rs:685-688) → DLC / error-handler disposition (Propagate returns
  `Ok(exchange_with_error)`).
- **Layer:** fallback `Err(e)` → returned **raw** to the caller (`fallback.call(exchange).await`
  at circuit_breaker.rs:167 with no boundary routing) — there is no error handler in the
  `eh_config = None` branch by definition, so raw `Err` is the *only* correct behavior.

**Verdict on the asymmetry:** it is **acceptable and inherent**, not a defect. The layer
branch exists precisely because no error handler is configured; there is nothing to route a
boundary error *to*. Forcing DLC semantics there would fabricate an error handler that the
author did not declare. **But it MUST be documented** in design.md and the spec deltas must
stay **path-agnostic** for the failure case.

**Spec-delta audit:**
- specs/dsl/spec.md:39 — "the **circuit breaker gate** returns `Ok(exchange)`" — **overclaims
  gate**. A no-error-handler route uses the layer, which has no "gate." **Reword to
  path-agnostic**: "the circuit breaker fallback path surfaces `Ok(exchange)`…". The *reason*
  clause ("because the composed fallback pipeline translates Stop to `Ok` at its
  `into_tower_result` boundary") is correct on both paths — keep it.
- specs/eip-cache/spec.md:30-33 — already path-agnostic ("the route surfaces `Ok(exchange)`…
  no error escapes the circuit breaker fallback path"). **OK as-is.**
- specs/dsl/spec.md:16 ("the compiled route's circuit breaker runs the fallback sub-pipeline")
  — already path-agnostic. **OK.**
- Neither spec asserts DLC/handle_boundary behavior on a *failing* fallback, so neither
  overclaims the gate-only failure routing. Good — but design.md SHOULD note the asymmetry so
  the blesser sees it was consciously owned. **Add one design paragraph (see §G).**

---

## F. Tests for both branches (mandate F)

- **Gate-path precedent:** `circuit_breaker_with_error_handler`
  (crates/camel-test/tests/integration_test.rs:686-738) — `RouteBuilder` + `.circuit_breaker(..)`
  + `.error_handler(dead_letter_channel("mock:dlc"))` → `eh_config = Some` → gate. This is the
  template for the gate fallback test (add a `fallback` sub-pipeline, force the circuit open,
  assert the fallback body reaches the sink and a stopping fallback yields `Ok`).
- **Layer-path precedent:** there is **no existing test** exercising `eh_config = None` + CB +
  fallback (grep of `crates/camel-test/tests/integration_test.rs` and camel-core tests shows CB
  tests always pair with an error handler or use the gate). This is the **missing coverage** the
  blesser will demand. Author a sibling test: `RouteBuilder` + `.circuit_breaker(..)` with a
  `fallback` and **no** `.error_handler(..)` and **no global handler** → drives the
  `CircuitBreakerLayer` branch (route_compiler_ext.rs:245-247) / `CircuitBreakerService`
  (circuit_breaker.rs:166-168). Assert: open circuit runs fallback; stopping fallback → `Ok(ex)`;
  **failing** fallback → raw `Err` surfaces (documenting the asymmetry as a test).
- **Lifecycle test:** a fallback containing a stateful step (e.g. `wire_tap` or an aggregator)
  must have its `StepLifecycle::start`/`shutdown` invoked on route start/stop. Assert via the
  same hook mechanism used by resequencer/aggregator lifecycle tests. This pins R2-C2.
- **Files:** gate + layer + failing-fallback e2e tests → `crates/camel-test/tests/integration_test.rs`
  (alongside :686). DSL parse/compile + canonical roundtrip + deny_unknown_fields →
  `crates/camel-dsl/tests/declarative_compile_test.rs`. `validate_contract` recursion →
  camel-api runtime unit tests (mirror the `Cache.on_miss` validation test). Builder fail-closed →
  `crates/camel-builder/tests/canonical_spec_test.rs`.

---

## G. Final design.md Approach / affected-crates patch

Replace design.md steps 3, 5, 6 and the "Affected crates" + add an asymmetry note. Deltas only:

**Step 3 (RouteDefinition sidecar)** — unchanged in intent; clarify field is `pub(crate)`
sibling of `circuit_breaker` (route_definition.rs:321) with `with_circuit_breaker_fallback`
setter (mirror :389).

**Step 5 (camel-core compile) — REWRITE:**

> 5. **camel-core compile (the load-bearing change).** In `compile_route_impl`
>    (route_compiler_ext.rs:556-637), **before** the `collect_lifecycle` at :582, resolve
>    `def.circuit_breaker_fallback` via `resolve_steps` (:297) → `Vec<CompiledStep>`, **merge
>    its lifecycle handles into the route `lifecycle` vec** via `collect_lifecycle`
>    (route_helpers.rs:38 — mirrors how on_miss child lifecycles reach the route vec via the
>    packed `Segment.lifecycle`), then compose into a `BoxProcessor` with
>    **`compose_traced_pipeline_with_contracts`** (route_compiler.rs:237 — body-contract
>    coercion parity with on_miss's `BodyCoercingSegment`; `compose_traced_pipeline` and
>    `compose_pipeline` do NOT coerce). Attach via `CircuitBreakerConfig::fallback(fb)` and
>    pass the config into `build_eh_config_pipeline`. This single attach point feeds **both**
>    runtime branches: the `CircuitBreakerGate` (`eh_config = Some`, :222; reads
>    `config.fallback` at circuit_breaker.rs:283/293) **and** the Tower `CircuitBreakerService`
>    (`eh_config = None`, :245-247; reads `config.fallback` at circuit_breaker.rs:118/166-168).
>    Both are live for the DSL surface.

**Step 6 (Stopped semantics) — keep, and ADD asymmetry note:**

> 6. **Outcome semantics — NO production code; regression tests only.** A stopping / peek-MISS
>    fallback yields `Ok(ex)` on **both** paths (gate: route_compiler.rs:683-684; layer:
>    circuit_breaker.rs:167) because the composed fallback pipeline maps `Stopped→Ok` at its
>    own `into_tower_result` boundary (ADR-0024/0025). **Documented asymmetry for a *failing*
>    (genuine `Err`) fallback:** the gate routes it through
>    `handle_boundary(BoundaryKind::CircuitBreaker, …)` (route_compiler.rs:685-688 → DLC /
>    disposition); the layer surfaces the raw `Err` to the caller (circuit_breaker.rs:167) —
>    correct, because the `eh_config = None` branch has no error handler to route to. Spec
>    deltas stay path-agnostic on the failure case; the clean-stop case is identical on both.

**Affected crates — corrections:**
- camel-core: `route_definition.rs` (sidecar field + `with_circuit_breaker_fallback`),
  `route_compiler_ext.rs` (fallback resolve+lifecycle-merge+compose+attach, **before :582**;
  **not** :565-603 as prior). Optionally a `pub(crate) fn compile_cb_fallback` helper.
- camel-processor: **NONE** (both fallback-invocation sites already read `config.fallback`;
  no change to gate or `CircuitBreakerService`). Prior design's processor edit is deleted —
  correct, but the *reason* is "both consumers already read the config," not "the layer is dead."
- camel-dsl, camel-api, camel-builder, schemas, docs: unchanged from current design.

**Spec-delta wording (§E):** edit specs/dsl/spec.md:39 gate→path-agnostic phrasing; leave
specs/eip-cache/spec.md and the other seven scenarios as-is.

**tasks.md boundary sketch (currently ABSENT — must be authored):** the prior 6-task shape holds
with two amendments:
- **Task 2 (RouteDefinition sidecar + camel-core compile):** acceptance MUST include (a)
  lifecycle-merge before :582 and (b) `compose_traced_pipeline_with_contracts`. Tests: gate-path
  fallback e2e **and** lifecycle start/shutdown on a stateful fallback step.
- **Task 4 (regression):** rename to **"clean-outcome + failing-fallback regression, BOTH
  paths"**. Add the **layer-path** (no-error-handler) fallback test (§F), the failing-fallback
  asymmetry test, and the peek-MISS clean-stop assertion on both paths.

---

## Corrected RESEARCH-e_opus.md addendum (as report text — no file edit)

The following claims in `RESEARCH-e_opus.md` are **superseded** by this report; a self-consistent
blessed design must carry these corrections:

- **§A.3 (lines 93-108) — compile placement WRONG.** "Compile it… after
  detect_and_validate_route_split, before build_eh_config_pipeline" places the compile *after*
  `collect_lifecycle` (:582), losing fallback lifecycle handles (R2-C2). **Correct: compile and
  merge lifecycles BEFORE :582.** Add `lifecycle.extend(collect_lifecycle(&fb_compiled))`.
- **§A.3 line 101 & §E.5 (lines 431-433) — composer WRONG.** It uses `compose_pipeline`
  (no coercion, no tracing) and later waffles toward `compose_traced_pipeline` (no coercion).
  **Correct: `compose_traced_pipeline_with_contracts` (route_compiler.rs:237)** — the only
  BoxProcessor composer that applies `wrap_if_needed` body-contract coercion, matching the
  on_miss `BodyCoercingSegment` (mod.rs:169-173). This directly answers R2-I.
- **§B.2 (lines 138-149) — "Tower service path is dead" WRONG (R2-C1).** The
  `CircuitBreakerService` layer branch (route_compiler_ext.rs:245-247) is **live** for routes
  without any error handler and **reads `config.fallback`** at circuit_breaker.rs:118 and
  :166-168. Both branches must be owned and tested. The correct framing is not "one live path"
  but "one attach point (the config), two live consumers (gate + layer)."
- **§B.3-B.4, §C, §D — CONFIRMED, no change.** No phantom `CamelError::Stopped`; validate_contract
  recursion; builder fail-closed via ADR-0016; schema/ts recursion parity — all correct.

---

## Verdict on round-3 re-bless likelihood

**HIGH (blessable) — provided the three corrections above land in design.md and tasks.md is
authored with both-branch coverage.** The redesign then:

- **compiles** (no `CamelError::Stopped`; ADR-0024 confirmed by camel-api/CONTEXT.md and
  error.rs — `Stopped` is not a `CamelError` variant),
- **preserves lifecycles** (fallback handles merged into the route vec before :582 — closes R2-C2),
- **preserves body contracts** (`compose_traced_pipeline_with_contracts` — closes R2-I),
- **owns both runtime branches** (config-level attach feeds gate + layer; both read
  `config.fallback` — closes R2-C1), with the failure-path asymmetry consciously documented,
- **keeps camel-dsl BoxProcessor-free** (only `Vec<BuilderStep>` threaded; processor born in
  camel-core via `resolve_steps` + registry),
- **reuses proven, in-scope machinery** (`resolve_steps` :297, `collect_lifecycle` :38,
  `compose_traced_pipeline_with_contracts` :237, `CircuitBreakerConfig::fallback` :71 — all
  `pub(crate)`+ already used in `compile_route_impl`/`build_eh_config_pipeline`).

**Residual probes the blesser will run (pre-empt in the revision):**
1. The design MUST state the compile happens **before line 582** and merges lifecycle — a
   blesser who checks `collect_lifecycle` ordering will reject "before build_eh_config_pipeline."
2. The design MUST NOT call the layer path "dead" — it must own both branches and reference the
   `config.fallback` reads at circuit_breaker.rs:118/166-168 and :283/293.
3. The composer MUST be named `compose_traced_pipeline_with_contracts` (not `compose_pipeline`).
4. tasks.md MUST exist and MUST include a layer-path (no-error-handler) fallback test and a
   stateful-fallback lifecycle test.
5. specs/dsl/spec.md:39 gate-specific phrasing → path-agnostic.
