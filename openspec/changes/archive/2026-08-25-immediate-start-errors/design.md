# Design: immediate-start-errors

## Approach

Asynchronous failure watcher for Immediate consumers (e_gpt ruling,
2026-08-25, after e_opus REJECT of the synchronous-await design). The
controller-facing handshake for Immediate consumers returns to the
pre-change fire-and-forget timing — `StartupReceiver::immediate()`
pre-resolves, `start_route`/`resume_route`/aggregate start return without
waiting — so the controller actor can never be delayed by an Immediate
consumer's startup (the reentrancy window that regressed
controlbus_stops_route cannot open). What changes is that a detached
watcher observes the consumer's prompt `start()` outcome and surfaces
failures loudly.

### Data flow

```
spawn_consumer_task (Immediate branch):
  startup_receiver = StartupReceiver::immediate()          // controller sees instant Ok
  let latches = ImmediateLatches::pair();                  // unchanged: watch pairs,
                                                           // one outcome send total,
                                                           // biased err > ok > grace
  task:
    let result = consumer.start(ctx).await;
    match result {
      Ok(())  => ok_tx.send(()),                           // watcher resolves, no action
      Err(e)  => { err_tx.send(e.to_string()) FIRST;       // watcher fires
                   …error path minus publish… }             // (error!, crash notifier,
                                                             // stop cleanup; NO FailRoute —
                                                             // sole-publisher rule)
    }

controller call sites (trait start / trait resume / aggregate):
  await_consumer_startup(startup_rx, op).await             // pre-resolved → zero yield
  spawn_failure_watcher(watcher_inputs)                    // DETACHED, never awaited;
                                                           // inputs arrive fully populated
                                                           // from spawn_consumer_task
                                                           // (AbortHandle, oneshot, id)
```

### Failure watcher

`spawn_failure_watcher(inputs: ImmediateWatcherInputs)` — spawned detached
at each production call site after the pre-resolved handshake resolves;
the call site passes the struct through UNCHANGED (the route keeps the
JoinHandle; the watcher owns the AbortHandle, the error-path-completion
oneshot, and the command_id); never awaited, never holding the actor:

1. `inputs.start_invoked` await — a oneshot the consumer task fires
   immediately BEFORE calling `start()`. The grace timer does NOT start
   until the invocation signal arrives (tokio may poll the watcher before
   the consumer task; without this latch the grace could elapse before
   `start()` even begins and the error would be missed). If the oneshot
   closes without firing (task dropped before invoking), return — nothing
   to watch.
2. `inputs.latches.wait(grace)` — the same biased select. Ok-latch or
   grace elapse → return (no failure to surface).
3. Err-latch → cleanup + projection, mirroring the rc-kh7c semantics:
   await the error-path-completion oneshot bounded by the grace, then
   `inputs.abort_handle.abort()` (idempotent),
   `inputs.consumer_cancel.cancel()`, then RuntimeBus
   `FailRoute { route_id, command_id, .. }` — the watcher is the event's
   sole FailRoute executor (sole-publisher rule).
4. Retry: at most ONE defensive retry of the FailRoute command after the
   grace boundary with the SAME `command_id`, against transient persistence
   errors. The RuntimeBus deduplicates successful commands; a duplicate
   landing after the route already reached `Failed` is a harmless no-op.
   The startup-confirmation race is NOT handled here — Phase 2a absorbs it
   (see "Phase 2a supersede tolerance" below; per the definitive ruling,
   exactly one durable outcome obtains and the confirm-start loser
   recognizes the supersede).

API shape: `spawn_failure_watcher(inputs)` is the production entry point —
it delegates to `run_failure_watcher(inputs, CONSUMER_IMMEDIATE_GRACE)`.
The internal helper's `grace` parameter is the test seam (the loop-style
watcher test injects 10ms instead of pausing time against the production
constant).

`Fail` accepts every state (route_runtime.rs transition table — same
semantics supervision already uses for consumer crash failures), so a
`FailRoute` landing on a `Starting`/`Started`/`Suspended` route is legal.

### Why not the alternatives (grill record)

- **Synchronous latch await in the actor** (first design, rejected by e_opus
  holistic): the actor yield lets a timer's immediate first tick reenter the
  RuntimeBus with StopRoute against a sibling whose Phase 2a `Started`
  commit has not run (`invalid transition: Registered -> Stopped`).
  Not grace-duration-bound (1ms still failed) — ANY await is the defect.
- **CQRS idempotent stop-on-Registered**: later queued StartRoute would
  still start the target; making stop intent win needs deferred-command
  infrastructure across Stop/Suspend/Reload/Remove — changes CQRS
  semantics, excessive scope.
- **Commit Started before the side-effect**: violates ADR-0018 two-phase
  ordering (persist Starting → side effect → confirm Started).
- **Batch start (dispatch all StartRoutes before any side-effect)**:
  future-architecture-scale; still needs command deferral.

**Ruling amendment (post-2.2, verified):** the startup-reentrancy
regression's original barrier parked the emission inside the sibling's
"uncommitted startup window" — that clause is unsatisfiable in
combination with a successful StopRoute, because the bus pre-validation
rejects Stop against a `Starting` aggregate before the command reaches
the controller actor (route_runtime.rs:117-129). The regression instead
pins the actor-never-yields invariant via a start()-window hold: the
sibling is a loop-style Immediate consumer whose `start()` fires
`start_entered` then parks on `hold_rx`; its StartRoute commits
`Started` while `start()` stays parked, and the emission's StopRoute
lands inside that start()-execution window (post-commit) and is honored.

### Ordering guarantee for fast errors

The task writes `returned_err` BEFORE entering the existing error path
(logging, CrashNotification, stop()) so the detached watcher observes the
latch the moment the error exists, independent of how long the error path's
own awaits take. No new races: the latches are write-once.

### Watcher retry policy

At most ONE defensive FailRoute retry, grace-spaced (t=0 on err-latch,
then +grace), with the same `command_id` — and the watcher is the
SOLE executor of that command (the consumer task's error path logs and
crash-notifies but never executes a RuntimeBus command for this event;
one id, one executor — a concurrent same-id execute cancelled mid-flight
would strand the dedup reservation and make every watcher retry return
Duplicate, so the rule is structural). Terminal behavior on
exhaustion (retry fails): emit the existing `error!` (system-broken)
log entry naming the route and the un-projected failure, then the watcher
returns — the consumer-side error log and CrashNotification already ran, so
observability never regresses below
pre-change. The watcher terminates on: ok-latch, grace elapse (no latch), or
the failed retry.

### Phase 2a supersede tolerance (the ordering guarantee)

`handle_lifecycle_start` and the detached watcher are concurrent writers
to one optimistically-versioned aggregate. Exactly one of two durable
outcomes obtains: **Started@V+1** (Phase 2a committed first; the watcher's
later `FailRoute` legally transitions Started→Failed@V+2 — the intended
"fails loudly" result) or **Failed@V+2** (the watcher committed first;
Phase 2a's version conflict is caught, the reloaded terminal-`Failed`
state is recognized as a supersede, and the operation returns
`Ok(RouteStateChanged { status: "Failed" })`). The optimistic lock makes
lost updates impossible; the re-load-on-conflict makes the supersede
observable. Implementation (commands.rs `handle_lifecycle_start` Phase 2a,
BOTH the `deps.uow` and `deps.repo` branches): on optimistic-lock conflict
from the confirm-start persist, re-load the aggregate; if the reloaded
state matches terminal `Failed(_)` → supersede-Ok; any other reloaded
state → the existing Err path unchanged (the discriminator is
terminal-Failed ONLY — it cannot mask an unrelated concurrent Stop/Suspend
conflict). `CamelContext::start` already continues on Ok, so a
status="Failed" Ok result does not fail the startup loop. No command is
deferred, no actor yields, no port signature changes.

### Rejected alternatives

- **Full await of `start()` for Immediate**: deadlocks lifetime-loop
  consumers (timer/file/sql/cron/keycloak `start()` runs until cancellation) —
  `start_route` would block for the route's entire lifetime.
- **Yield-sweep (yield N times, then check an error slot)**: no scheduler
  bound (a not-yet-scheduled task writes nothing; N is arbitrary) and no
  error-return bound (an error arriving just after the sweep is missed with
  no way to size the window) — timing luck, not a contract.
- **Sync bounded grace in the actor (Design 1)**: implemented and
  holistically rejected — ANY actor yield during the Immediate handshake
  lets a timer's first emission reenter RuntimeBus with StopRoute against a
  sibling still `Registered` (invalid transition); the grace duration is
  irrelevant, the yield itself is the defect.
- **Post-confirm hook (defer watcher spawn until after Phase 2a)**: works,
  but requires a `RuntimeExecutionPort::start_route` signature change and
  churns every test mock; the conflict-path hook invocation also fires the
  watcher when the aggregate may be in an unrelated failed state — rejected
  by the definitive ruling in favor of Phase 2a supersede tolerance.

### Grace budget

`CONSUMER_IMMEDIATE_GRACE: Duration = 50ms` (named const beside
`CONSUMER_STARTUP_BUDGET`; a direct parameter of the watcher's wait so
tests inject small values). Rationale: fast start errors (validation,
double-start, ownership) return in well under a millisecond of task time;
50ms covers scheduler jitter under CI load. Under the async-watcher design
the grace is paid only by the DETACHED watcher (route reaches `Failed`
within ~grace); no actor path waits for it.

### Resume path

`resume_route` (route_controller_trait.rs:~757) resolves the pre-resolved
receiver instantly and spawns the same detached watcher. A prompt resume
failure transitions the route to `Failed` via the watcher (abort + cancel +
FailRoute) — no detached consumer or child tasks remain.

### In-tree census (verified 2026-08-24, startup_mode + start-body inspection)

- Loop-style Immediate (start() runs until cancellation): **timer, file, sql,
  cron, keycloak** (sql polls inside start(), consumer.rs:401,559; cron awaits
  service.run(), lib.rs:230,283; keycloak loops until cancel,
  keycloak_consumer.rs:227,245).
- Spawn-and-return Immediate: seda, mock, controlbus, log, master, validator,
  template, exec, wasm, opensearch, xj, container, llm
  (default mode, start spawns/returns).
- Explicit: direct, http (+static), ws, kafka, mqtt, redis, jms, cxf, mcp,
  surrealdb, grpc.
- camel-test fake consumers in route tests: mixed; the census records which
  consumers are loop-style (under the detached-watcher design the actor
  never waits — the grace is paid only by the detached watcher, not by
  sequential route starts).

## Affected crates

- `camel-core` (all changes here): `consumer_management.rs` — Immediate
  branch of `spawn_consumer_task` (pre-resolved receiver restored + latch
  sends retained), `ImmediateLatches` (kept), `failure_watcher`, grace
  const; `route_controller_trait.rs` — start/resume spawn the watcher after
  the instant handshake; `route_controller.rs` — aggregate path (~1102)
  same wiring; unit tests in `consumer_management.rs`; route-level tests in
  `route_controller_tests.rs`; CONTEXT.md watcher note.
- `camel-component-api`: CONTEXT.md startup-handshake doc note ONLY (a
  promptly-returning Err from an Immediate `start()` transitions the route
  to Failed asynchronously). No code.

## Architecture boundaries

Runtime-internal fix: no `camel-api`/`camel-component-api` code or trait
changes, no `#[non_exhaustive]` surface movement (ADR-0049 untouched), no
component edits. Respects rc-w1u9's two-mode handshake (Explicit path
byte-identical, still awaited synchronously) and ADR-0018 two-phase start
ordering (the watcher never reorders commits; FailRoute is a normal command
against the transition table). Log-policy: the task's error path keeps its
existing `error!` (system-broken) annotation; the watcher's failure surfaces
as route status `Failed` via the RuntimeBus — the same projection channel
supervision uses for consumer crashes.

Out of scope (follow-up bd tickets): general startup reentrancy /
activation-barrier audit for Explicit consumers that emit after mark_ready
(**rc-ava7**, e_opus-flagged latent risk: Kafka/JMS/Redis class); migration
of the five loop-style Immediate consumers to spawn-and-return + Explicit;
any change to `ConsumerStartupMode`/`StartupReceiver` public API;
seda-specific behavior (already fixed).

## Phases

Single-phase: one coherent contract-behavior fix (~2 files + tests + docs).
