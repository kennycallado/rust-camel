# BLESS-e_opus-holistic — cb-fallback-dsl (bd rc-bzf6)

VERDICT: **BLESS**

Final holistic gate. Reviewed full diff (merge-base `4e6b8e31`…HEAD `b79f416a`, 9
commits, 34 files), blessed artifacts, ORACLE-e_opus-round3, and the load-bearing
runtime. Targeted warm-target tests run against prebuilt binaries (disk 3.0G,
concurrent builders — no workspace-wide runs). All CB-specific edges hold.

## Verification (prebuilt binaries, CARGO_INCREMENTAL=0)

- `cb_fallback_lifecycle_test` (camel_core, linked 20:40, post-HEAD): **2/2 ok** —
  incl. the discriminating blocked-tap test proving `StepLifecycle::start`+`shutdown`
  merge and drain-blocks-stop.
- `integration_test` `circuit_breaker_fallback*` (linked 17:33; HEAD's b79f416a does
  NOT touch camel-test, confirmed via `git show --stat`): **gate/layer/miss = 3/3 ok**,
  `circuit_breaker_failing_fallback_asymmetry` **1/1 ok**.
- `camel_core` `canonical_route_conversion*` (linked 20:40): **3/3 ok** — incl.
  `..._rejects_unsupported_fallback_step` (Section C fail-closed pin) and
  `..._threads_circuit_breaker_fallback` (positive threading).

## A. State machine × fallback — SOUND

Traced every `CircuitState` × branch in both consumers.

- **Fallback never corrupts counters/transitions.** Gate: `before_call → Fallback`
  returns early at route_compiler.rs:684/688; `after_result` (706) runs ONLY on the
  `Allow`→pipeline path. Service: `call` returns the fallback future at
  circuit_breaker.rs:167, bypassing the state-update closure (199-238). Fallback is a
  pure side-path — no counter touch, no state write. Correct.
- **poll_ready HalfOpen "Always Err (even with fallback)" (circuit_breaker.rs:134) vs
  gate before_call HalfOpen→Fallback (283/293) — RECONCILED.** These are *different
  live consumers*: `eh_config=Some`→Gate, `None`→Service. The Service intentionally
  rejects concurrent half-open callers to hold the strict single-probe gate (pinned by
  `service_half_open_admits_only_one_probe`). The Gate serves fallback (stale) to
  concurrent half-open callers (`test_cb_gate_open_with_fallback_returns_fallback`
  covers open; the half-open probe-in-flight arm at 293 is reachable and consistent).
  Both are individually correct; neither runs fallback *instead of* a probe — the probe
  caller always takes the `Allow`/probe path.
- **poll_ready↔call TOCTOU (Open+fallback):** if `open_duration` elapses between
  poll_ready (returns Ok, :118) and call (:164 sees elapsed → Open→HalfOpen probe),
  the caller runs the real probe rather than fallback. Benign and *more* correct —
  pre-existing, unchanged by this PR.

## B. Both-branch parity — SOUND, adequately documented

Clean-stop `Ok(ex)` identical on both (tested). Genuine-`Err` asymmetry
(gate→`handle_boundary(CircuitBreaker)`→DLC; layer→raw `Err`) is architecturally forced
(the `eh_config=None` branch has no handler to route to) and stated loudly in
docs/src/yaml-dsl/route-structure.md:74-76 and docs/src/eip/cache.md:47. Not a trap:
users opting out of `error_handler` already accept raw-`Err` propagation route-wide.

## C. RegisterRoute canonical cache-arm gap — RULING: ACCEPTABLE, land now + bd follow-up

`canonical_step_to_builder_step` (commands.rs:916-1080) has no
`Cache`/`CacheInvalidate`/`CachePeekStale` arms → catch-all `_ => Err` (:1077). So the
ANCHOR (`cache_peek_stale` in CB fallback) ERRORS on the canonical RegisterRoute path
while working via YAML/DSL. **This is a PRE-EXISTING control-plane gap** (cache steps
were never wired into the canonical converter — independent of this change). This PR
does the right thing: it threads `cb.fallback` through the *same* converter and **fails
closed** (ADR-0016 no silent loss), pinned by
`canonical_route_conversion_rejects_unsupported_fallback_step`. The eip-cache delta
scopes its claim to "demonstrable end-to-end **from YAML**" — it does NOT overclaim
RegisterRoute. Blocking would punish this change for a debt it did not create.
→ **bd follow-up (P2, discovered-from:rc-bzf6):** "Wire Cache/CacheInvalidate/
CachePeekStale arms into `canonical_step_to_builder_step` so the canonical
control-plane reaches parity with the YAML/DSL path for cache EIPs."

## D. Lifecycle truth — SOUND

`attach_cb_fallback` + `lifecycle.extend(fallback_lifecycle)` at BOTH sites:
`compile_route_impl` (route_compiler_ext.rs:673-680, feeds hot-reload
`compile_route_definition_pipeline`) and `build_managed_route`
(route_controller.rs:413-420). No leak/drop path: the discriminating test proves stop
awaits both the in-flight fallback exchange (drain counter) AND the fallback step's
`StepLifecycle::shutdown`, in order. Hot-reload threads fallback handles into the
swapped pipeline's `CompiledPipeline.lifecycle`.

## E. Concurrency/cost — no new cost

`fallback.call()` clones `BoxProcessor` per invocation (pre-existing Tower pattern, only
when circuit is open/half-open — the cold path). `attach_cb_fallback` compiles once per
compile. Single-threaded compile (sidecar visibility trivially safe). No new lock or
per-call cost on the hot (closed-circuit) path — `Allow` skips fallback entirely.

## F. Spec honesty — accurate

10 scenarios covered; eip-cache MODIFIED delta reads as sound future canon (YAML-scoped,
Stop→Ok cited to ADR-0024/0025). Docs/example match tested behavior.

## G. The hidden thing — MINOR honesty gap (non-blocking)

The **half-open-concurrent** asymmetry compounds B into the half-open window: same YAML
modulo `error_handler`, a *concurrent* caller during a live probe gets **Gate→stale
fallback** but **Service→`CircuitOpen` Err**. Both individually sound; neither corrupts
state. The design's asymmetry note (design.md:49-56) and docs cover only the
*failing-fallback* Err-routing difference, NOT this half-open concurrent
fallback-vs-reject difference.
→ **bd follow-up (P3, discovered-from:rc-bzf6):** "Document the half-open-concurrent
CB-fallback asymmetry (gate serves stale fallback vs service rejects with CircuitOpen)
in docs/src/eip/circuit-breaker.md; consider unifying to gate semantics if the layer
path is later deprecated." Prior C0 (deleted `CamelError::Stopped`) confirmed absent —
no `Stopped` variant reintroduced; Stop→Ok stays at the single `into_tower_result` site.

## Findings

- CRITICAL: none.
- IMPORTANT: none.
- MINOR (bd follow-ups, non-blocking): (C) canonical cache-arm parity P2; (G) half-open
  concurrent asymmetry doc P3.

Merge-ready. Both bd follow-ups are documentation/parity debt, neither gates the CB
fallback semantics this change introduces.
