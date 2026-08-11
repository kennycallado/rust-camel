# Proposal: wiretap-camel-model

## Why

The WireTap EIP processor (rc-vq91 follow-up `rc-wmuc`) leaks memory on
long-running routes. After the clone-abort fix shared the tap `JoinSet`
via `Arc<Mutex<JoinSet<()>>>`, completed task entries accumulate for the
entire route lifetime because nothing calls `join_next()`. The
bounded-concurrency semaphore does not bound `JoinSet` growth: every
`call()` spawns an entry before acquiring the permit (`wire_tap.rs:91-95`).
Leak magnitude is `route_lifetime × tap_throughput` — a real OOM path on
the high-volume fire-and-forget workload this EIP targets.

The leak is a symptom of an architecturally wrong task model. The current
implementation diverges from Apache Camel on three axes: unbounded default
admission (Camel uses a bounded pool + `maxQueueSize` + `CallerRuns`),
spawn-then-acquire ordering (Camel back-pressures the caller), and
immediate-abort-on-drop teardown (Camel drains gracefully then aborts at
shutdown).

## What Changes

Model WireTap on Camel's real thread-pool semantics (Option C from the
e_opus architect review,
`docs/reviews/wiretap-rc-wmuc-architect-guidance.md`):

- Bounded admission semaphore acquired BEFORE spawn, with `CallerRuns`
  back-pressure at the bound (run inline on the calling future when no
  permit is free). The bound is the DETACHED-task capacity (default
  exactly 20); under saturation total execution may transiently reach
  `bound + 1`. Default changes from unbounded to 20 — a deliberate,
  Camel-faithful behavior change.
- Detached `tokio::spawn` holding an `OwnedSemaphorePermit`, tracked by a
  `TaskTracker` — self-reaping, no `JoinSet`, no reaper. Every task
  selects on a private `CancellationToken` for prompt abort.
- `StepLifecycle`-backed teardown (ADR-0022): close admission → drain via
  `timeout(grace, tracker.wait())` → cancel stragglers. Route-order drain
  on stop. Default grace exactly 5s, configurable.
- New `CompositeStepLifecycle` in camel-core (NOT camel-api — runtime
  sequencing is a core concern): children `[endpoint, WireTap]`, start
  forward, shutdown reverse, start-rollback + best-effort shutdown.
- Additive `WireTapService::lifecycle()` accessor — constructors keep
  stable signatures. `WireTapConfig` gains `shutdown_grace` (default 5s).

**Excluded** (separate bd `rc-c7ll`, `discovered-from: rc-wmuc`): threading
`route_cancel` through `CompilationContext` to fix the Aggregate
step-compiler local-token gap. WireTap anchors on `StepLifecycle`, not the
route token, so the two changes never collide.

## Acceptance criteria

- Detached tap count never exceeds the bound; caller back-pressured under
  saturation (deterministic blocked-call timing test).
- In-flight tracker drains to 0 across repeated 1000-tap bursts.
- Graceful-drain-then-abort: fast tap drains, slow tap aborts after grace,
  shutdown idempotent, calls-after-close rejected.
- Tap readiness/processing errors (detached + inline) logged `warn!` and
  suppressed; main route never blocks/fails on the tap.
- Per-request clone drop does not abort admitted taps (rc-vq91 regression
  preserved).
- Public constructor signatures unchanged; lifecycle via additive accessor.
- Composite drains WireTap before endpoint; start-rollback on failure.
- Divergences documented in `camel-processor/CONTEXT.md` (ADR-0046).

## Risk budget

Acceptable: a new internal `CompositeStepLifecycle` in camel-core; an
additive `lifecycle()` accessor and an additive `WireTapConfig`
`shutdown_grace` field; the default-bound change (unbounded → 20). Out of
bounds: any change to `CompilationContext`, the route token plumbing,
`CompiledStep` schema, the `StepLifecycle` trait, or the endpoint
lifecycle contract; any breaking change to existing constructor signatures.
