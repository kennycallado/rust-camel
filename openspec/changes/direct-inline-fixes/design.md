# Design: direct-inline-fixes

## Approach

Two independent, adjacent fixes — no shared state beyond the
dispatcher construction site.

**Fix A (rc-2sba) — publication guard.** The publication site
(`crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs (start site ~:369-378 and resume site ~:840-871)`)
publishes the dispatcher wrapping `managed.pipeline` for every
non-Concurrent topology. Add one guard: when
`managed.aggregate_split.is_some()`, skip publication entirely —
aggregate routes remain channel-dispatched, which is the correct
pre-623cca62 behavior (`start_aggregate_route` runs pre/agg/post from
the split pipelines; the channel consumer drives them; the capability
must not exist, so producers take the channel path by the existing
capability-unavailable fallback). Invert
`aggregate_route_gets_capability` (inline_dispatcher_tests.rs:676) to
assert the capability is ABSENT for a split route, in BOTH lifecycle
windows: after initial start AND after suspend/resume (resume mirrors
start's publication path; a dedicated aggregate resume test is
required — the generic non-aggregate resume test does not cover
split topology). Keep a non-aggregate Sequential control assertion in
the same file. New end-to-end regression (mirror the existing
aggregate split tests that use `timer:` entry, swapping in `direct:`
entry): N fragments via `to("direct:agg-in")` → exactly 1 aggregated
reply carrying all N.

**Fix B (rc-y5nn) — b′ visibility for unhandled dispatch failures.**
Ground truth from the code (`camel-direct/src/lib.rs:455-540`): the
producer already holds a context-threaded runtime handle
(`self.runtime` — component-level, not producing-route-wired) and
emits b′ for dispatch-result errors at :518-532. The regression is
narrower than "site moved consumer→producer": the initial registry
lookup failure `?`-exits at :471-483 BEFORE the emission block, and
the pre-change consumer-side emission the red test relied on was
deleted with the receive-loop restructure. Fix shape
(mechanism-light contract, site decided by the red test):
cover ALL unhandled failures — initial lookup, admission, in-pipeline
— through a context-threaded handle, emitted exactly once per failing
dispatch invocation, `ConsumerStopping` excluded, channel-path
emission unchanged, no double-count with a wired traced wrapper.
Implementation (pinned producer-side, wiring verified): the runtime
handle is `Arc::clone(&component_ctx)` taken at route compile
(route_compiler_ext.rs:347-348) and the late-registration test
registers its lifecycle BEFORE `start()` compiles routes, so the
producer's `self.runtime.metrics()` reaches the collector. Route the
lookup error through the existing producer emission block (one site,
one handle, endpoint-derived attribution for the no-entry case), AND
emit on the outer timeout branch (`.map_err(|_|
dispatch_timeout_error)` — tokio drops the inner future on expiry, so
timeouts bypass the inner block and emit nothing today; plan review
finding C1). No dispatcher-side alternative exists in this change;
camel-core is untouched by Fix B.

Spec amend rides with Fix A: the selection requirement gains the
aggregate exclusion (an aggregate-split consumer is Sequential but
SHALL NOT be inline-eligible), and a new requirement pins the b′
visibility contract for unhandled direct-dispatch failures.

Single-phase change, ~4 tasks (guard + inverted/unit tests, e2e
regression test, b′ emission + no-double-emit, spec amend + full
suite verification).

## Affected crates

- `camel-core`: `route_controller_trait.rs` (guard at both publication
  sites) — nothing else; Fix B touches no camel-core file (the
  dispatcher gains no tracer arg; emission is producer-side).
- `camel-component-direct` (camel-direct): producer call restructure
  (lookup-miss into the shared emission site, timeout-arm emission,
  ConsumerStopping exclusion).
- `camel-test`: red `metrics_wiring_test` case turns green (no edit
  needed to the test itself — it is the contract).
- `openspec/specs/direct-dispatch/spec.md`: amended via delta.

## Architecture boundaries

Pure data-plane controller/lifecycle change — no DSL, no new public
SPI (the tracer arg is constructor-internal to the adapter's builder).
ADR-0022 drain semantics, ADR-0012 b′ semantics, and the Concurrent
fallback are untouched: Fix A removes inline eligibility for one
topology (restoring channel semantics), Fix B moves an emission to
the layer that owns the failure. The 9.3x plain-hop path is
unchanged; `cargo bench -p camel-bench --bench direct` re-run
confirms no fallback leakage after the guard.

## Alternatives considered

- **Revert 623cca62**: loses a genuinely gated 9.3x; fixes are small;
  rejected.
- **Build the dispatcher over pre+agg+post**: aggregates would gain
  inline perf, but the split pipelines are driven by the aggregate
  engine's own lifecycle (timeout/force-completion windows) — wiring
  them under the producer's dispatch admits new drain interactions.
  Not worth it until a benchmark demands it.
- **Re-contract the metrics test** (observed producer handle instead
  of NoOp): defensible but weaker — hides the invariant that
  unhandled dispatch failures are always operator-visible through a
  controller-threaded handle; rejected in favor of Fix B.
