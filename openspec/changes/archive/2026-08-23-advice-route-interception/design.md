# Design: advice-route-interception

## Approach

Boot-time route interception as a camel-core compile-time concern. An ordered
`InterceptRules` set is attached to the route controller before any route
compiles. The `EndpointsCompiler` `To` arm consults the rules before component
resolution and applies one of two actions:

- **SkipTo** — URI substitution before `parse_uri`: the rule target URI goes
  through the normal resolve/endpoint/producer/lifecycle path; the original
  URI is never parsed and its component never resolved. This makes skip usable
  in lean boots where the real component (kafka, http) is not registered.
- **DivertCopyTo** — the real producer resolves normally; the copy-target
  producer resolves too (compile-time failure to resolve either is a hard
  compile error — misconfiguration must fail before traffic). The step
  composes as `WireTapService(copy target)` followed by the real producer:
  the wiretap stage clones the exchange for the copy and passes the original
  through, then the real producer runs and its outcome is the step's outcome.
  Timing mirrors `camel_processor::WireTapService` exactly (verified
  `crates/camel-processor/src/wire_tap.rs:250-365`): when a semaphore permit
  is available the copy runs as a detached tracked task and the main future
  proceeds without awaiting it; under saturation the copy runs INLINE on the
  caller's future (CallerRuns), back-pressuring until the inline copy
  finishes. Copy-phase failures (poll_ready or call) are logged at `warn!`
  and suppressed in both paths. Deterministic copy completion in tests goes
  through the decorator's `StepLifecycle` drain (shutdown joins tracked
  copies) — no sleeps.

Both compile-time target resolutions (skip target, copy target) are hard
compile errors when they fail — misconfiguration must surface at boot, never
as a silent runtime drop.

Matching: exact full-URI string equality, first match in declaration order.
Duplicate rule URIs are permitted (declaration order preserved; later
duplicates are unreachable for that URI and harmless). No deduplication.

Action targets are restricted to `mock:` scheme URIs (ADR-0064 vocabulary:
real URIs map to mock names). A non-`mock:` target is rejected when rules are
set, not at compile time.

No-rules path: an empty rule set short-circuits to the current `To` arm body
unchanged — interception is strictly opt-in.

seda needs no dedicated code: the send point is the enqueue producer. The
ADR-0064 §6 fences (consumer side, fanout-partial, post-queue observation)
fall out of send-point-only scope.

## Affected crates

- camel-core: `InterceptRules` model + controller plumbing (builder +
  pre-freeze setter command) + threading rules into the step-compiler
  context + `To` arm application + hot-reload recompile consistency.
- camel-processor: reuse `WireTapService` as the divert stage via a new
  `compose_divert(tap: WireTapService, real: BoxProcessor) -> BoxProcessor`
  (direct service sequencing — the public `WireTapService` copy stage
  followed by the real producer; NOT `WireTapLayer`, whose `layer` ignores
  the inner service, `wire_tap.rs:387-392`; private `WireTapShared`/`run_tap`
  are not touched) + restart-reopen lifecycle surgery + unit tests.
  Lifecycle composition of [copy endpoint, tap, real] stays in camel-core
  (`CompositeStepLifecycle` is `pub(crate)` there; a camel-processor
  dependency on camel-core would be a publish cycle).

## Architecture boundaries

- Data-plane only: interception is Tower `Service<Exchange>` composition in
  pipeline compilation. No RuntimeBus/RuntimeQuery variant, no registry
  handle in the query plane (ADR-0002/0045 ceiling untouched).
- Rules freeze: compilation is owned by the route-controller actor
  (`controller_actor.rs` processes commands sequentially), so freezing is
  race-free by construction. Rules freeze on the first successful route
  registration or context start, whichever occurs first. Context start is
  made freeze-visible even with zero routes: `CamelContext::start` awaits an
  explicit controller command (`MarkStarted`) before returning success, so
  an empty context still trips the freeze. A FAILED start does not freeze
  (freeze trips only on success). After the freeze point, later
  `SetInterceptRules` commands are rejected. Stop/restart does not unfreeze
  (v1). Hot-reload recompiles (`reload_actions.rs`) read the same frozen
  set, so every compiled snapshot (ADR-0042/0004) carries identical rules.
- No new crate (ADR-0055).
- Camel inspiration, not conformance (ADR-0046): no `adviceWith` API shape
  port; a static rule set with two actions only.

## D1 — API shape

```rust
// camel-core
pub struct InterceptRule { pub uri: String, pub action: InterceptAction }
pub enum InterceptAction {
    /// Replace the send entirely; the real URI is never resolved.
    SkipTo { uri: String },        // uri must be mock:-scheme
    /// Detached outcome-isolated copy (WireTap semantics); real send proceeds.
    DivertCopyTo { uri: String },  // uri must be mock:-scheme
}
#[derive(Debug, Clone, Default)]
pub struct InterceptRules { /* ordered Vec<InterceptRule> */ }

impl InterceptRules {
    /// Rejects non-`mock:` targets with a rule-indexed error.
    pub fn new(rules: Vec<InterceptRule>) -> Result<Self, CamelError>;
}

impl CamelContextBuilder { pub fn with_intercept_rules(self, rules: InterceptRules) -> Self }
impl CamelContext     { pub async fn set_intercept_rules(&self, rules: InterceptRules) -> Result<(), CamelError> }
// Err variants: frozen (first successful route registration or start,
// whichever occurred first).
```

Builder-set rules are extracted and passed to the `DefaultRouteController`
during `build()` (same path as existing controller configuration,
`context_builder.rs:131`). The context setter routes a command to the
controller actor; sequential command processing makes the freeze atomic.

## D2 — Where rules live during compilation

The step-compiler context struct that `EndpointsCompiler` already receives
gains an `intercept: InterceptRules` field, populated by every compile entry
path from the controller's frozen set (initial `AddRoute`, dry pipeline,
hot-reload recompile). The `To` arm consults it before `parse_uri`.

## D3 — Divert semantics (mirrors WireTapService)

Composition: direct service sequencing — `WireTapService(copy producer)`
followed by the real producer (`WireTapLayer` is NOT used: its `layer`
ignores the inner service). `poll_ready`: always ready. `call`: clone
exchange for the copy; if a permit is available, spawn a detached tracked
copy task and pass the original through; if saturated, run the copy INLINE
on the caller's future (CallerRuns) with back-pressure before proceeding.
Either way the composite then awaits the real producer's readiness before
invoking it, and the real producer's readiness/call result is returned
verbatim (`Result<Exchange, CamelError>`: `Ok` exchanges and `Err` errors
pass through unchanged).
Copy `poll_ready`/`call` errors: `warn!` + suppressed. Restart: route
stop invokes `WireTapLifecycle::shutdown`, which closes admission and
cancels the tracker token (`wire_tap.rs:191-225`), while route restart
reuses the same compiled pipeline (`consumer_management.rs:398-418`);
therefore `WireTapLifecycle::start` MUST reopen admission, reset the
tracker, install a fresh cancellation token, and reset the
`shutdown_called` latch (otherwise the second shutdown after restart is a
silent no-op) so a restarted route keeps diverting and stops cleanly. Shutdown drains
tracked copies via the composed `StepLifecycle` (copy endpoint lifecycle +
WireTap tracker lifecycle + real endpoint lifecycle). Tests assert copy
completion through drain, not sleeps. Lifecycle composition is inline in
the `To` arm (camel-core `CompositeStepLifecycle`, children
`[copy endpoint?, tap tracker, real endpoint?]`); `shutdown` iterates in
REVERSE — real tears down first, then the tracker drains in-flight copies,
then the copy endpoint closes — which is the intended ordering (copies
drain before their target endpoint tears down). The semaphore bound is the
`WireTapService` construction default (20) and is NOT configurable through
`InterceptAction` in v1; the saturation path is asserted by holding 20
admitted copies in flight, sending exchange 21, and verifying its copy runs
inline (its effect precedes its real send's effect) with the real outcome
verbatim.

## D4 — Explicit non-goals (pinned by ADR-0064)

`from:` interception, `WireTap` step interception, wildcards/patterns,
non-`mock:` action targets, post-freeze mutation, seda consumer side,
fanout-partial, post-queue assertions, Stage B YAML surface, standalone/IPC
modes.

## D5 — Documentation surfaces

This is a public, cross-cutting contract: update `CONTEXT-MAP.md`
(interception entry + relationships to route compilation and WireTap),
`crates/camel-core/CONTEXT.md` and `crates/camel-processor/CONTEXT.md`
(new rule model / divert composition), the user-facing testing guide under
`docs/src/` (registered in `docs/src/SUMMARY.md`) with one skip and one
divert example, and `docs/src/concepts/glossary.md` with `InterceptRule`,
`SkipTo`, `DivertCopyTo` entries.

## D6 — Test mapping (executable scenario definitions)

Every spec scenario maps 1:1 to a named Rust test; scenario names become
snake_case test function names:

- Rule-model + freeze scenarios → `crates/camel-core/src/intercept.rs`
  inline `#[cfg(test)]` (model validation, e.g.
  `non_mock_action_targets_are_rejected_at_rule_construction`,
  table-driven) and `crates/camel-core/tests/route_interception_test.rs`
  (context-level freeze, e.g.
  `setting_rules_after_the_first_route_registration_is_rejected`,
  `setting_rules_after_start_of_an_empty_context_is_rejected`,
  `a_failed_start_does_not_freeze_rules` — failed start arranged with a
  failing `ConfigCheck` startup check).
- Send-path scenarios (skip, divert, seda, empty-rules parity) →
  `crates/camel-core/tests/route_interception_test.rs`,
  each function named after its scenario, including
  `real_producer_readiness_is_driven_before_call` (two cases: success order
  — a real-producer stub records readiness-before-call events and returns a
  sentinel `Ok` exchange asserted verbatim; readiness failure — a sentinel
  readiness error is returned verbatim and the stub's `call` is never
  invoked). Divert verbatim-outcome asserts compare the
  exact `Result<Exchange, CamelError>` returned by the real producer stub
  using sentinel Exchange payloads and sentinel `CamelError` variants (Ok
  and Err cases) — Tower producers return `Result`, not `PipelineOutcome`
  (which exists only above the Tower layer; reserve `PipelineOutcome`
  asserts for route-executor tests, `pipeline_outcome.rs:3-7`); copy-failure
  warnings are captured via a `tracing` test subscriber; copy completion is
  observed through `StepLifecycle` drain or the target's notify primitive —
  never sleeps.
- Saturation scenario → same file, `saturated_divert_runs_the_copy_inline_before_the_real_send`:
  a copy-target stub whose `call` blocks on a held permit; 20 in-flight
  admitted copies; the 21st asserted inline (recorded order) with verbatim
  outcome.
- camel-processor composition/lifecycle unit tests →
  `crates/camel-processor/src/intercept_compose.rs` `#[cfg(test)]`
  (sequencing, lifecycle ordering, restart-reopen semantics of the WireTap
  lifecycle: start-after-shutdown reopens admission with a fresh token).
- Route stop/restart divert survival →
  `crates/camel-core/tests/route_interception_test.rs`
  `divert_survives_route_stop_and_restart` (stop the route, restart it,
  traverse an exchange, assert copy + real delivery both happen).
- Data-plane boundary scenario →
  `crates/camel-core/tests/hexagonal_architecture_boundaries_test.rs`
  (existing suite): extend the dependency-edge analysis so interception
  modules (`intercept`, compiler application, divert composition) are
  asserted to declare no `RuntimeBus`/`RuntimeQuery`/`RuntimeQueryBus`
  dependency.

tasks.md (STAGE 2) repeats these file/function names verbatim — spec
scenarios, test names, and task tests stay three views of one contract.
