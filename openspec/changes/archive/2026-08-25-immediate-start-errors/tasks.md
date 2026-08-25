# Tasks: immediate-start-errors

> REDESIGNED after the e_opus holistic rejection of the synchronous design.
> Tasks 2.1/2.2/2.3 rework the implementation landed in commits
> 5d243a0b..f939f8a8 (the synchronous-era tasks 1.1/1.2/1.3).

## camel-core rework

### Task 2.1: Async failure watcher + Phase 2a supersede tolerance

**Files:**
- `crates/camel-core/src/lifecycle/adapters/consumer_management.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/handshake_tests.rs` (modified)
- `crates/camel-core/src/lifecycle/application/runtime_bus.rs` (modified — dedup unit test)
- `crates/camel-core/src/lifecycle/application/commands.rs` (modified — Phase 2a
  supersede tolerance in `handle_lifecycle_start`'s confirm-start persist,
  BOTH the `deps.uow` and `deps.repo` branches, per the definitive ruling:
  on optimistic-lock conflict → re-load aggregate → terminal
  `Failed(_)` → `Ok(RouteStateChanged { status: "Failed" })`; any other
  reloaded state → existing Err path unchanged. Discriminator is
  terminal-Failed ONLY.)
- `crates/camel-core/src/lifecycle/adapters/route_helpers.rs` (modified —
  EXECUTED VARIANT, post-review: add `pub(super) fn fail_command_id(route_id:
  &str) -> String` returning `format!("ctrl-fail-{route_id}-{stamp}")` as
  the single shared stamp source; `runtime_failure_command` KEEPS its
  2-arg signature — the watcher builds its FailRoute inline with the
  pre-allocated id, so the planned `Option<&str>` param had zero `Some`
  callers and was dropped; the DIRECT test caller
  `route_controller_tests.rs:177` stays 2-arg; `publish_runtime_failure`
  and its callers UNTOUCHED per the r_glm ponytail finding)
- `crates/camel-core/src/lifecycle/application/commands_tests.rs` (modified
  — phase2a supersede unit tests; if a variant fits commands.rs's own
  `#[cfg(test)]` better, name that placement in the report — one home, not
  both)
- NOTE (not a Files entry): `crates/camel-core/Cargo.toml` already carries
  the tokio `test-util` dev-dep from the sync-era commit — keep it (the
  biased-boundary latch test and any remaining start_paused usage depend
  on it; the loop-style watcher test itself now uses the injected-grace
  seam instead); verify it remains after the rework.

**Steps:**
1. `consumer_management.rs` — `spawn_consumer_task` Immediate branch: RESTORE
   `StartupReceiver::immediate()` as the controller-facing receiver
   (replacing the real pair — the mark_ready-before-start call and its
   comment go away with the pair). KEEP the latch sends in the task body
   exactly as implemented (err-latch send first in the Err path; ok-latch
   send on Ok). KEEP `ImmediateLatches` and its biased `wait(grace)`. Delete
   `await_immediate_startup` and `await_startup` (the dispatch helper) and
   their doc comments (~lines 79, 285) — no call sites remain.
2. Define the watcher's inputs as a dedicated struct in
   `consumer_management.rs` — no free-vs-associated choice for the worker:

   ```rust
   /// Everything the detached failure watcher needs, produced once by
   /// `spawn_consumer_task` for Immediate consumers.
   pub(crate) struct ImmediateWatcherInputs {
       pub start_invoked: tokio::sync::oneshot::Receiver<()>, // fired by the
                                                          // task immediately
                                                          // BEFORE start()
       pub latches: ImmediateLatches,
       pub abort_handle: tokio::task::AbortHandle,       // consumer_handle.abort_handle()
       pub error_path_done: tokio::sync::oneshot::Receiver<()>, // fired AFTER the
                                                          // task's error-path block
       pub consumer_cancel: CancellationToken,
       pub route_id: String,
       pub command_id: String,                           // stamp format, per event
       pub runtime: Option<Weak<dyn RuntimeHandle>>,     // same weak handle the
   }                                                     // deferred paths use

   /// Production entry point — fixed production grace.
   pub(crate) fn spawn_failure_watcher(inputs: ImmediateWatcherInputs)
       -> tokio::task::JoinHandle<()>;

   /// Internal helper — `grace` is the test seam (loop-style test
   /// injects 10ms instead of pausing time against the constant).
   async fn run_failure_watcher(inputs: ImmediateWatcherInputs, grace: Duration);
   ```

   `spawn_consumer_task`'s return type changes to
   `(JoinHandle<()>, StartupReceiver, Option<ImmediateWatcherInputs>)`
   (call sites destructure three values; the watcher inputs carry the
   oneshot + command_id out — nothing else escapes). SOLE-PUBLISHER RULE
   (e_gpt round 5): the consumer task's error path does NOT call
   `publish_runtime_failure` for Immediate consumers — the watcher is the
   ONLY executor of this event's FailRoute. Rationale: RuntimeBus dedup
   RESERVES the command_id at first_seen and only forgets it when execute
   returns Err (runtime_bus.rs ~239-247); a task-side execute with the
   same id cancelled mid-flight (abort) would leave the reservation, and
   the watcher's retry would receive Duplicate and stop without the route
   ever reaching Failed. One id, one executor, no concurrent same-id
   executes. The task's error path keeps: mark_failed-where-applicable
   (Explicit only), `error!` log (GUARANTEED — runs synchronously in the
   task before any await), CrashNotification send (BEST-EFFORT — its
   `.send().await` can be cut short if the watcher's bounded
   error_path_done timeout elapses and abort lands mid-send; the error!
   log is the guaranteed observability floor), stop cleanup — and fires
   `error_path_done` AFTER that block (the oneshot Sender moves in; the
   Ok path simply drops it — receiver closes, the watcher never awaits it
   on the Ok branch). `publish_runtime_failure` and its `command_id`
   parameter remain for the OTHER (deferred/shutdown) callers only —
   unchanged semantics there. In the Immediate branch the task ALSO holds
   the `start_invoked` oneshot Sender and fires it immediately BEFORE
   calling `consumer.start()` (grace cannot begin before invocation;
   tokio may poll the watcher first). Watcher body (inside `tokio::spawn`,
   via `run_failure_watcher(inputs, CONSUMER_IMMEDIATE_GRACE)`):
   a. Await `start_invoked` first (no timeout — the task WILL fire it or
      drop it at/after `start()`); if the oneshot CLOSES without firing
      (task dropped pre-invocation, e.g. aborted externally), return —
      nothing to watch. The grace timer starts only after this resolves.
   b. `match latches.wait(grace).await` —
      `Ok(())` → return (ok-latch or grace elapse; loop-style and prompt-Ok
      consumers surface nothing).
   c. `Err(msg)` → FIRST await the error-path-completion oneshot bounded by
      the grace (`tokio::time::timeout(grace, error_path_done).await`) so
      the task's error path (crash notification, stop cleanup) finishes;
      on timeout OR after completion,
       `abort_handle.abort()` (idempotent if already finished) and
       `consumer_cancel.cancel()`; then the PRIMARY `FailRoute` attempt
       (t=0) with this `command_id`. Success → return. On rejection: at
       most ONE defensive retry after the grace (t=+grace), SAME
       `command_id` (defense-in-depth against transient persist errors —
       the startup-confirmation race is NOT handled here; Phase 2a in
       commands.rs absorbs it per the definitive ruling). On failed retry:
       emit `error!` (log-policy system-broken, route_id + msg, naming the
       un-projected failure) and return.
       Degenerate runtime cases: if `inputs.runtime` is `None` OR every
       `Weak::upgrade()` fails, the watcher skips the FailRoute attempts
       entirely (nothing to command), performs abort+cancel as above, emits
       the same system-broken `error!` naming the route and the missing
       runtime handle, and terminates — bounded, no retry loop.
       (command_id semantics live in step 2's struct contract: one stamp
       per failure event, owned solely by the watcher (sole-publisher) — a
       later second failure of the same route must not dedup against this
       one.)
3. `route_controller_trait.rs` start path (~548): destructure
   `(consumer_handle, startup_rx, watcher_inputs)` — the inputs arrive
   FULLY POPULATED from `spawn_consumer_task` (AbortHandle captured inside
   via `consumer_handle.abort_handle()` at construction; the call site
   never builds inputs itself). Store `consumer_handle` (the JoinHandle)
   in the managed route as the pre-change code did;
   `await_consumer_startup(startup_rx, "startup")` (pre-resolved →
   instant); if `watcher_inputs` is present, call
   `spawn_failure_watcher(inputs)` passing the struct through UNCHANGED —
   document this ownership split (route owns JoinHandle; watcher owns
   AbortHandle + oneshot + command_id) in a comment. DELETE
   the rc-slvd Err arm substitution (back to the pre-change Err arm — only
   Explicit failures reach it now). Same for resume path (~757): delete the
   cleanup-parity Err arm; spawn the watcher after the instant handshake.
4. `route_controller.rs` aggregate path (~1106): same — instant handshake,
   spawn watcher, restore the original Err arm (drop the added
   consumer_cancel line; the watcher owns abort/cancel now).
5. `handshake_tests.rs`: the repurposed immediate-receiver test pins the
   OLD contract again (pre-resolved immediate) — REVERT it to its original
   form (already the case if step 1 restores immediate(); verify the test
   text matches its original intent and the module doc header states the
   pre-resolved semantics; the 3-tuple destructure fixes stay).
6. `route_controller_tests.rs`: REWRITE the three loud-failure tests to the
   async contract (keep the mock families):
   - `start_route_fails_loudly_on_immediate_consumer_error` → rename
     `immediate_consumer_error_transitions_route_to_failed`:
     `start_route` returns Ok; poll the route status through a REAL
     RuntimeBus (`RuntimeQuery::GetRouteStatus` → assert the returned
     status equals `Failed`; 2s bound, 10ms interval — NEVER the
     handle-liveness `inferred_lifecycle_label` idiom, which cannot emit
     Failed, route_helpers.rs:162-171); then assert the consumer-side
     evidence (counter stopped —
     abort parity, same double-snapshot pattern) and
     `managed.consumer_handle` storage per the ownership chosen in step 3.
   - `resume_route_fails_loudly_on_immediate_consumer_error` → rename
     `immediate_consumer_error_on_resume_transitions_to_failed`: resume
     returns Ok; poll `GetRouteStatus` to `Failed` (same real-bus bound);
     counter double-snapshot equal (no detached tasks).
   - `aggregate_start_route_fails_loudly_on_immediate_consumer_error` →
     rename `aggregate_immediate_error_transitions_to_failed`: aggregate
     start returns Ok (resolved — the pre-change code returned Ok for
     Immediate consumers; the watcher owns the failure surface); poll
     `GetRouteStatus` to `Failed` (same real-bus bound).
   - NEW `context_start_does_not_fail_fast_on_immediate_error` (spec: ctx
     no-fail-fast): build a CamelContext (or use the context-lifecycle test
     idiom in camel-core) with one auto-startup immediatefail route and one
     healthy sibling; `CamelContext::start()` returns Ok; sibling reaches
     Started; the failing route polls `GetRouteStatus` to `Failed` (real
     bus, same 2s/10ms bound).
7. `consumer_management.rs` unit tests: REWRITE the four immediate_* tests:
   - `immediate_fast_error_propagates` → `immediate_fast_error_watcher_fails_route`:
     call `spawn_consumer_task` + `spawn_failure_watcher` against a minimal
     recording fake of `RuntimeHandle` (create it in-file: the trait needs
     only `execute` + `ask`, camel-api/src/runtime.rs:657-667 (RuntimeHandle blanket impl; NoopRuntime precedent); a
     `Arc<Mutex<Vec<RuntimeCommand>>>` recorder plus a configurable
     per-command accept/reject outcome); consumer start() returns Err
     promptly; assert the watcher issued FailRoute with the expected
     route_id (recorder) and the consumer task joined (error path ran).
   - `watcher_retries_after_rejected_failroute` (spec: one defensive
     retry): fake bus rejects the PRIMARY FailRoute attempt, accepts the
     retry; assert exactly 2 recorded attempts sharing ONE command_id and
     final success.
   - `watcher_logs_after_exhaustion` (spec: bounded exhaustion): fake bus
     rejects both attempts; assert exactly 2 attempts, the system-broken error!
     line (capture_logs tracing idiom, precedent in
     route_controller_trait_tests.rs:44), and watcher termination (handle
     joins).
   - `watcher_duplicate_failroute_is_noop` (spec: dedup no-op): execute the
     duplicate same-`command_id` `FailRoute` against the REAL RuntimeBus
     (add a unit test in `runtime_bus.rs` — the fake-bus path would make
     dedup vacuously green): issue FailRoute{id} twice against a real bus
     with a registered route; assert exactly ONE lifecycle transition to
     Failed (status query) and the second execute returns the
     Duplicate/no-op outcome — bus-level dedup (runtime_bus.rs:239-247,
     `Ok(RuntimeCommandResult::Duplicate { command_id })`; FailRoute handler
     in crates/camel-api/src/runtime.rs).
   - `immediate_prompt_ok_skips_grace` → `immediate_prompt_ok_watcher_exits_early`:
     ok-latch consumer; watcher handle resolves within 1s (outer timeout)
     WITHOUT any FailRoute issued.
   - `immediate_loop_style_grace_proceeds` → keep semantics under the new
     name `immediate_loop_style_watcher_exits_after_grace`: call the
     internal `run_failure_watcher(inputs, 10ms)` directly (injected-grace
     seam — no start_paused against the production constant); watcher
     exits within 10ms + bound, no FailRoute.
   - `immediate_biased_error_wins_over_grace`: keep as-is (still pins the
     latch contract) if it does not reference deleted fns; else port.
8. The three Explicit-pinning tests stay green unmodified:
   `start_route_aborts_consumer_task_on_startup_failure`,
   `spawn_consumer_task_explicit_consumer_start_error_propagates`,
   `await_consumer_startup_propagates_mark_failed_error`.
9. `commands.rs` Phase 2a supersede implementation step (BEFORE the
   tests): add `fn is_optimistic_conflict(err: &CamelError) -> bool` in
   commands.rs — matches the error whose chain contains
   `"optimistic lock conflict"` (the same discriminator the in-memory
   repo's own tests use, in_memory.rs:488-491; `DomainError` has no typed
   conflict variant — string-match is the established precedent, keep it
   local and private). Wrap the confirm-start persist (Phase 2a) in BOTH
   branches: on `Err(e)` where `is_optimistic_conflict(&e)` → re-load the
   aggregate → if state is terminal `Failed(_)` → return
   `Ok(RuntimeCommandResult::RouteStateChanged { route_id, status: "Failed" })`
   — else propagate `e` (existing Err path). Non-conflict Err propagates
   unchanged.
10. `commands.rs` Phase 2a supersede tests (unit; live in commands_tests.rs,
   driven through `execute_command(deps, RuntimeCommand::StartRoute)`;
   the CONFLICT is produced by a `deps.execution` side-effect fake whose
   `start_route()` mutates the SHARED backing repo DURING Phase 2 — after
   Phase 1's Starting@V+1 persist, before returning Ok — so the Phase 2a
   confirm persist (expected V+1) genuinely conflicts; FOUR named tests,
   `deps.uow` set vs None selecting the branch):
   - `phase2a_uow_supersede_failed_returns_ok`: setup = deps WITH uow
     backed by the same shared repo; register the route (aggregate at
     Starting@V+1 after Phase 1 runs inside execute_command); the
     execution fake's start_route loads the aggregate, applies
     `fail("watcher")` (→ Failed@V+2), and saves it to the shared repo,
     then returns Ok; action = `execute_command(deps, StartRoute)`;
     assert = returns
     `Ok(RuntimeCommandResult::RouteStateChanged { route_id, status: "Failed" })`,
     re-loaded repo state is `Failed`, no compensation-Err logged path
     taken. Expected: RED pre-implementation (version-conflict Err),
     GREEN after.
     Command: `cargo test -p camel-core --lib phase2a_uow_supersede_failed_returns_ok`.
   - `phase2a_repo_supersede_failed_returns_ok`: setup = deps WITHOUT uow
     (`deps.uow = None`, repo save_if_version branch); the same
     shared-repo execution fake applies `fail("watcher")` during Phase 2
     (→ Failed@V+2); action = `execute_command(deps, StartRoute)`;
     assert = `Ok(RouteStateChanged { status: "Failed" })`, repo state
     `Failed`. Expected: RED pre-implementation, GREEN after.
     Command: `cargo test -p camel-core --lib phase2a_repo_supersede_failed_returns_ok`.
   - `phase2a_uow_non_failed_conflict_returns_err`: setup = deps WITH uow;
     the execution fake instead applies TWO transitions during Phase 2 —
     first `fail("watcher")` (Starting@V+1 → Failed@V+2; Stop is invalid
     from Starting, route_runtime.rs:118-129), then the Stop transition
     (Failed@V+2 → Stopped@V+3) — and saves; action =
     `execute_command(deps, StartRoute)`; assert = returns the
     version-conflict `Err` exactly as before this change (the re-loaded
     state is `Stopped`, NOT terminal-Failed — discriminator is
     terminal-Failed ONLY). Expected: GREEN both before and after
     (regression pin).
     Command: `cargo test -p camel-core --lib phase2a_uow_non_failed_conflict_returns_err`.
   - `phase2a_repo_non_failed_conflict_returns_err`: setup = deps WITHOUT
     uow; the execution fake applies the same two transitions during
     Phase 2 (fail → Failed@V+2, then Stop → Stopped@V+3) and saves;
     action = `execute_command(deps, StartRoute)`; assert =
     version-conflict `Err` unchanged (re-loaded `Stopped` is not
     superseded). Expected: GREEN both before and after.
     Command: `cargo test -p camel-core --lib phase2a_repo_non_failed_conflict_returns_err`.

**Tests** (each: setup → action → assert → command; RED/GREEN
expectations stated where the test targets new behavior):
- `immediate_consumer_error_transitions_route_to_failed` (E2E through the
  REAL RuntimeBus — `inferred_lifecycle_label` cannot emit Failed, it maps
  handle-liveness only, route_helpers.rs:162-171): setup = immediatefail
  route REGISTERED via a real RuntimeBus wired to a
  DefaultRouteController; action = `bus.execute(StartRoute)` (asserts Ok)
  then poll `bus.ask(RuntimeQuery::GetRouteStatus)` (10ms interval, 2s
  bound; result `RuntimeQueryResult::RouteStatus`); assert =
  status eventually `Failed`. Counter double-snapshot (abort parity) via
  the controller's managed routes where accessible. Command:
  `cargo test -p camel-core --lib immediate_consumer_error_transitions_route_to_failed`.
  Expected: red against the sync-era code (status stays Started), green
  after.
- `immediate_consumer_error_on_resume_transitions_to_failed`: setup =
  flakyresume route started Ok then suspended, fail_next set; action =
  `resume_route` + poll; assert = resume Ok, `GetRouteStatus` reaches
  `Failed`, snapshots equal.
  Command: `cargo test -p camel-core --lib immediate_consumer_error_on_resume_transitions_to_failed`.
- `aggregate_immediate_error_transitions_to_failed`: setup = immediatefail
  aggregate route (force_completion_on_stop(true)); action = start + poll;
  assert = Ok + `Failed` via `GetRouteStatus`. Command:
  `cargo test -p camel-core --lib aggregate_immediate_error_transitions_to_failed`.
- `context_start_does_not_fail_fast_on_immediate_error`: setup = CamelContext
  (context-lifecycle test idiom) with immediatefail route + healthy sibling;
  action = `CamelContext::start()`; assert = Ok, sibling Started, failing
  route polls to `Failed`. Command:
  `cargo test -p camel-core --lib context_start_does_not_fail_fast_on_immediate_error`.
- `immediate_fast_error_watcher_fails_route` (unit — ALSO the
  sole-executor pin): setup = recorder fake RuntimeHandle shared by BOTH
  the consumer task and the watcher inputs (same Arc); action =
  spawn_consumer_task + spawn_failure_watcher with an Err consumer;
  assert = the recorder received EXACTLY ONE FailRoute (from the watcher —
  the consumer path published none), with the expected route_id and the
  event's command_id, and the consumer task joined (error path ran).
  Command: `cargo test -p camel-core --lib immediate_fast_error_watcher_fails_route`.
- `watcher_retries_after_rejected_failroute`: setup = fake rejecting the
  primary attempt then accepting; assert = exactly 2 records, ONE
  command_id, success on the retry. Command:
  `cargo test -p camel-core --lib watcher_retries_after_rejected_failroute`.
- `watcher_logs_after_exhaustion`: setup = fake rejecting all; assert =
  exactly 2 attempts (primary + single retry), system-broken error!
  captured (capture_logs idiom, route_controller_trait_tests.rs:44),
  watcher joins. Command:
  `cargo test -p camel-core --lib watcher_logs_after_exhaustion`.
- `watcher_duplicate_failroute_is_noop` (runtime_bus.rs unit, REAL bus —
  dedup pin; the sole-executor guarantee is pinned by
  immediate_fast_error_watcher_fails_route's shared-recorder exactly-one
  assert): issue FailRoute{id} twice against a real bus with a registered
  route; assert = one transition to Failed, second returns the dedup
  outcome — bus-level dedup, runtime_bus.rs:239-247, returning
  `Ok(RuntimeCommandResult::Duplicate { command_id })`; the FailRoute
  handler lives in crates/camel-api/src/runtime.rs). Command:
  `cargo test -p camel-core --lib watcher_duplicate_failroute_is_noop`.
- `watcher_no_runtime_terminates_bounded` (unit): setup = inputs with
  `runtime: None` and an Err consumer; action = spawn_failure_watcher;
  assert = watcher handle joins within 1s (outer timeout), NO FailRoute
  attempted, abort+cancel still ran (counter stopped), system-broken
  error! captured (capture_logs idiom). Command:
  `cargo test -p camel-core --lib watcher_no_runtime_terminates_bounded`.
- `immediate_prompt_ok_watcher_exits_early`: setup = ok-latch consumer
  with parked bg handle (senders held); action = watcher spawned; assert =
  watcher handle resolves within 1s, recorder has ZERO FailRoute. Command:
  `cargo test -p camel-core --lib immediate_prompt_ok_watcher_exits_early`.
- `immediate_loop_style_watcher_exits_after_grace`: setup = consumer
  awaiting ctx.cancelled() forever (fires start_invoked first); action =
  call the internal `run_failure_watcher(inputs, 10ms)` DIRECTLY (the
  injected-grace test seam — do NOT pause time against the production
  constant) and await its JoinHandle-equivalent future; assert = watcher
  exits within 10ms + bound (outer 1s timeout), ZERO FailRoute; then
  cancel. Command: `cargo test -p camel-core --lib immediate_loop_style_watcher_exits_after_grace`.
- `immediate_biased_error_wins_over_grace`: unchanged from the sync-era
  implementation if it still compiles against `ImmediateLatches::wait`
  (it tests the latch contract, not the deleted fns); else port to the
  same shape. Command:
  `cargo test -p camel-core --lib immediate_biased_error_wins_over_grace`.
**Acceptance (exit 0):** `cargo test -p camel-core --lib`; `cargo clippy -p
camel-core --all-features -- -D warnings`; `cargo fmt --check --all`;
`cargo xtask lint-unwrap`.

- [x] 2.1

### Task 2.2: Deterministic reentrancy regression + controlbus loop

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs` (modified)

**Steps:**
1. Add deterministic reentrancy regression `timer_emission_during_start_does_not_corrupt_sibling`
   implementing the two-way barrier from the spec WITHOUT deadlocking the
   controller actor.
   > DEVIATION (verified): B = held IMMEDIATE loop-style, commit-then-hold in `start()`; Explicit variant probe-proven unsatisfiable (Stop-from-Starting pre-validation reject, route_runtime.rs:117-129).
   Deadlock rules (r_glm review): B's start runs ON the
   controller actor; blocking inside the start window blocks the actor and
   the reentrant StopRoute needs that same actor. Therefore the hold lives
   in B's CONSUMER task (spawned off-actor by spawn_consumer_task), never
   in the actor path: B is a held IMMEDIATE loop-style consumer (per the
   verified deviation above) whose `start()` fires `start_entered` first,
   then parks on `hold_rx` — B's StartRoute COMMITS Started at once
   (pre-resolved handshake) while the consumer task stays parked
   mid-start, off-actor. The emission's StopRoute provably lands inside
   B's start()-execution window (post-commit) and is honored. Details in
   the exact sequence below.
   - `#[tokio::test(flavor = "multi_thread")]` (precedent
     route_controller_tests.rs:1566).
   - EMISSION path drives the REAL surface: A's consumer start calls
     `runtime.execute(RuntimeCommand::StopRoute{target B})` through a
     RECORDING PROXY runtime handle — a fake `RuntimeHandle` whose
     `execute`/`ask` record the command AND forward to the real
     RuntimeBus/controller (so the reentrant StopRoute genuinely reaches
     the controller actor; a pure recorder that swallows the command would
     make the test vacuously green). The test observes the dispatch via the
     proxy's recording, then releases B's hold.
   - Test location: camel-core `route_controller_tests.rs` (resolved — no
     camel-test variant). Sibling B is a held EXPLICIT consumer (resolved):
     its `start()` awaits `hold_rx` then calls `ctx.mark_ready()` — the
     controller actor legitimately awaits B's readiness, which IS the
     "sibling mid-start, uncommitted" state.
   - Exact sequence (first barrier then second, no timing luck):
     1. A's consumer start() blocks its first emission on `emit_gate`
        (oneshot) — A is Immediate loop-style.
     2. B's held consumer fires a `start_entered` oneshot as its FIRST
        action inside `start()` (before awaiting `hold_rx`) — the test
        awaits it, proving B is inside start (mid-handshake) rather than
        inferring from the StartRoute dispatch.
     3. DESIGN-DISCRIMINATING assertion (ordering fixed: start A's clock
        BEFORE awaiting B's barrier): the test wraps A's StartRoute
        resolution in a sub-grace timeout FIRST (`timeout(grace/2 = 25ms,
        a_start_future)`), THEN awaits B's `start_entered`, and requires
        A's resolution to have completed before B's barrier completes.
        Under the synchronous design the actor is parked inside A's grace
        select — B's StartRoute is never dispatched, B's start never
        entered, and either await fails the 25ms bound — RED on sync,
        GREEN on async (A resolves instantly; the actor proceeds to B's
        start while B parks on hold_rx). Concretely: record
        `a_resolved_at: tokio::time::Instant` when A's future resolves and
        `b_entered_at` when the start_entered oneshot fires; assert
        `a_resolved_at < b_entered_at` AND `a_resolved_at` occurred within
        25ms of context start.
     4. Test releases `emit_gate`; A emits the exchange that drives
        `runtime.execute(RuntimeCommand::StopRoute{target: B})` through the
        recording proxy — which RECORDS and FORWARDS to the real controller
        (a pure recorder that swallows the command would make the test
        vacuously green).
     5. Test observes the StopRoute dispatch in the proxy's recording, then
        releases B's `hold_rx` (B completes mark_ready → its StartRoute
        commits Started). The B-entry evidence (`start_entered`) plus the
        held release ordering guarantee the emission lands inside B's
        uncommitted window deterministically — B cannot have committed
        before the StopRoute arrives because its own start is parked on
        `hold_rx` until after the dispatch is observed.
     6. Assertions: A's StartRoute succeeded; B's StartRoute succeeded and
        committed; B's StopRoute ALSO succeeded (no lost command, no
        `Registered -> Stopped` / invalid-transition error recorded by the
        proxy); final B status is `Stopped` (stop intent honored after
        commit — no lost StopRoute); no error results in the proxy log.
2. Run the e_opus verification bar and record the numbers in the report:
   `cargo test -p camel-test --test controlbus_test` (full binary) once;
   then exactly 20 fresh invocations of
   `cargo test -p camel-test --test controlbus_test controlbus_stops_route -- --exact`
   (all must pass; shell: `for i in $(seq 1 20); do cargo test -p camel-test
   --test controlbus_test controlbus_stops_route -- --exact || exit 1; done`
   — report "20/20"); then 20 invocations of the new deterministic
   regression (`for i in $(seq 1 20); do cargo test -p camel-core --lib
   route_controller::tests::timer_emission_during_start_does_not_corrupt_sibling
   || exit 1; done` — report "20/20").

**Tests:** the regression test + the loop runs above.
**Acceptance:** 20/20 + 20/20 loop runs green; `cargo test -p camel-core
--lib` exit 0; clippy/fmt/lint-unwrap exit 0.

- [x] 2.2

### Task 2.3: Update docs to the watcher semantics

**Files:**
- `crates/components/camel-component-api/CONTEXT.md` (modified)
- `crates/camel-core/CONTEXT.md` (modified)

**Steps:**
1. component-api Startup handshake: replace the synchronous-grace sentence
   (added in a0481897) with: a promptly returning Err from an Immediate
   consumer's `start()` transitions the Route to Failed asynchronously
   (detached failure watcher issuing one FailRoute with at most one
   defensive same-command_id retry); the lifecycle operation itself returns
   without waiting — Immediate startup timing is unchanged. An error
   arriving after the grace keeps the existing logged/crash-notified
   behavior.
2. camel-core CONTEXT.md: rewrite the **Immediate startup grace** Language
   entry: the grace is the watcher's observation budget (route reaches
   Failed within ~grace on prompt errors); NO actor path waits for it;
   remove the fail-fast sentence (CamelContext::start no longer fails fast
   on Immediate errors — Explicit bind failures still do). Keep canonical
   terms, hard-wrap ~100 cols.
3. No code.

**Tests:** `cargo xtask lint-context-citations` exit 0; `cargo fmt
--check --all` exit 0; diff shows only the two CONTEXT.md edits.
**Acceptance:** both CONTEXT.md files carry the watcher semantics (no
actor wait, Failed within ~grace, Explicit unchanged); lint-context-
citations and fmt exit 0; `git diff --stat` limited to the two files.

- [x] 2.3
