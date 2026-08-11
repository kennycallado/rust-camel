# Design: wiretap-camel-model

## Approach

Rebuild `WireTapService` around three Camel-faithful primitives, anchored
on the existing `StepLifecycle` drain (ADR-0022) rather than the route
cancellation token. The bound is the DETACHED-task capacity; under
saturation the caller runs the tap inline (one extra concurrent
execution), matching Camel's `CallerRuns`.

**1. Bounded admission before spawn (CallerRuns back-pressure).** Acquire
the semaphore permit in `call()`, before spawning. If a permit is
immediately available, detach the tap (step 2). If not, run the tap inline
on the calling future (await it before returning the original Exchange) —
this is `CallerRuns`: the caller is back-pressured, the main route
throttles. In-flight detached tasks equal outstanding permits, so the
system is structurally leak-free. Default bound exactly 20
(`WireTapConfig::default()`; this changes the prior unbounded default — a
deliberate, Camel-faithful behavior change). Each detached task holds an
`OwnedSemaphorePermit` for its lifetime and drops it on completion.

**2. Detached spawn with TaskTracker liveness, no JoinSet.** Admitted taps
run as `tokio::spawn` detached tasks. Liveness is tracked via
`tokio_util::task::TaskTracker` (already in the dep tree alongside
`CancellationToken`): `tracker.spawn(...)` registers the task; the tracker
counts in-flight tasks and exposes `len()`. Detached tasks self-reap on
completion — no `JoinSet`, no reaper loop, no accumulation.

**3. StepLifecycle teardown (graceful drain then abort).** Every detached
task SHALL `select!` between its endpoint work and a private
`CancellationToken`, so cancellation aborts promptly. `WireTapLifecycle`
(a new `StepLifecycle` impl) wraps the token, the tracker, and the
admission gate. `shutdown(reason)`:
1. Close the admission gate (reject new `call()` taps — logged `warn!`).
2. Close the tracker (enables `wait()` to complete once empty).
3. `tokio::time::timeout(shutdown_grace, tracker.wait())` — drain taps
   that complete naturally.
4. On timeout: `token.cancel()` — tasks selecting on the token abort.
5. Await tracker completion (bounded).

`shutdown` is idempotent (guard flag). Default `shutdown_grace` exactly
5s, configurable via `WireTapConfig.shutdown_grace`. The runtime drives
`StepLifecycle::shutdown` in route order on stop (ADR-0022; reverse order
is only for startup rollback), after intake is cancelled and the pipeline
task is joined.

**4. Composite lifecycle wiring (camel-core, not camel-api).**
`CompiledStep::Process.lifecycle` is a single `Option<Arc<dyn
StepLifecycle>>`; the WireTap compiler arm (`endpoints.rs:57-63`) already
places the endpoint's handle there. Introduce `CompositeStepLifecycle` in
camel-core (runtime sequencing is a camel-core concern, not a camel-api
contract): stores `Vec<Arc<dyn StepLifecycle>>` in child order
`[endpoint, WireTap]`. `start()` runs forward (endpoint first);
`shutdown()` runs reverse (WireTap first, so taps drain before the
endpoint tears down). On `start` failure: roll back already-started
children in reverse. `shutdown` is best-effort: every child attempted,
errors aggregated. The compiler stores the composite as the single handle.

**5. Additive lifecycle accessor (preserves stable API).** Constructors
`WireTapService::new`, `with_config`, `WireTapLayer::new`, `bounded` keep
their exact signatures (they are stable public exports). A new additive
`WireTapService::lifecycle(&self) -> Arc<dyn StepLifecycle>` returns the
`WireTapLifecycle` handle. The compiler calls it to compose with the
endpoint handle. `WireTapConfig` gains `pub shutdown_grace: Duration`
(`Default` = 5s) — a minor additive field change, documented.

## Affected crates

- **camel-api**: NO change. `StepLifecycle` trait and `CompiledStep`
  schema unchanged. (e_gpt correction: `CompositeStepLifecycle` does NOT
  belong in camel-api.)
- **camel-core**: new internal `CompositeStepLifecycle`; `endpoints.rs`
  WireTap arm composes `[endpoint, WireTap]` handles via the accessor.
- **camel-processor**: rewrite `WireTapService` — admission-before-spawn,
  detached spawn + `OwnedSemaphorePermit`, `TaskTracker` liveness, private
  `CancellationToken` (every task selects on it), `WireTapLifecycle`
  handle, additive `lifecycle()` accessor. `WireTapConfig` gains
  `shutdown_grace`, default bound now 20.
- **camel-processor/CONTEXT.md**: document divergences (flat-semaphore,
  CallerRuns transient-exceed, route-level teardown, absent pool-profile
  knobs) per ADR-0046; update EIP catalog + poll_ready table (WireTap
  moves from `pending-fix` to `migrated`: `Ready(Ok(()))` unconditional).

## Architecture boundaries

This change stays within the **Services** boundary (`WireTapService` is a
Tower middleware service). It does not touch Runtime control plane, DSL
parsing, Components, or Languages. It reuses the ADR-0022 lifecycle drain
— the same primitive `AggregatorService`/`ResequencerService` rely on —
rather than a new shutdown mechanism. It deliberately avoids the route
`CancellationToken` (tracked separately as `rc-c7ll`), so the
Runtime/Services contract is unchanged.
