# Design: explicit-task-watcher

## Approach

### 1. Outcome-accounted state (replaces a bare completion flag)

The Explicit arm (`_` wildcard arm, consumer_management.rs:465-577 — the
ONLY Explicit body; the Immediate arm owns its own at :303-464 and is
untouched) gains a per-task outcome state shared with the watcher:

```rust
// exactly two states: Pending (false) / Accounted (true)
// shared as Arc<AtomicBool>-backed OuterOutcomeCell (Relaxed ordering
// suffices; the oneshot provides the happens-before for the watcher's read)
```

- Err path: set `Accounted` IMMEDIATELY AFTER the failure publication
  (crash-notify + publish_runtime_failure) and BEFORE the cleanup
  `consumer.stop().await` — a panic during that stop must NOT re-publish.
- Bg-monitor path (bg JoinError/Err published inside the body): set
  `Accounted` after `publish_runtime_failure`, before the finally-`stop()`.
- Normal path: set `Accounted` only AFTER the final `consumer.stop().await`
  succeeds OR returns Err (a stop Err is not a crash — debug-log it, account
  it; the route's normal lifecycle owns it).
- Normal Ok-start with no bg handle: same finally rule.

`Pending` at termination time = ABNORMAL (a termination the body never
accounted for).

### 2. Termination signal via task-local drop guard (no polling)

A drop guard owned by the task body fires in ALL termination modes — normal
return, panic unwind, and abort (tokio abort drops the future, running
Drop). The guard sends on a oneshot `terminated_tx` (sync `send`, drop-safe)
— no poll loop, no wakeup cost while the task runs:

```rust
struct TerminationGuard { tx: Option<oneshot::Sender<()>> }
impl Drop for TerminationGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() { let _ = tx.send(()); }
    }
}
```
(oneshot::Sender::send consumes self, so the guard stores `Option<Sender>`
and `take()`s it inside Drop — moves out of `&mut self` legally.)

### 3. Detached outer-task watcher

`spawn_outer_task_watcher(inputs: OuterWatcherInputs) -> JoinHandle<()>`,
modeled on `spawn_failure_watcher` (:221-280):

```rust
struct OuterWatcherInputs {
    terminated: oneshot::Receiver<()>,     // drop guard fires on any end
    outcome: Arc<...>,                     // Accounted / Pending
    consumer_cancel: CancellationToken,    // cancelled stop → silent
    route_id, runtime(Weak), crash_notifier,
}
```

Body:
1. `terminated.await` (the ONLY wait — fires at task end).
2. If `consumer_cancel.is_cancelled()` → return silent. Ordering note: the
   START/RESUME rollback branches abort FIRST then cancel
   (route_controller_trait.rs:606-610, :813-814) — a termination observed
   before the cancel flag is visible is a ROLLBACK-owned termination, not a
   route failure. To stay silent there, the watcher ALSO treats
   `Pending + already-finished-at-spawn` as silent when spawned by a
   rollback in progress; concretely: the watcher is spawned ONLY after the
   handshake resolves Ok (see wiring) — rollback terminations happen before
   that point and are never watched. NORMAL stop cancels before aborting
   (consumer_management.rs:600, :633-640) → cancel flag is set first →
   step 2 exits silent.
3. If outcome is `Accounted` → return silent (failure already published by
   the body, or normal completion).
4. Else (`Pending`, not cancelled): ABNORMAL termination — panic or abort
   without accounting. Fire the standard trio exactly like the bg monitor:
   error log (system-broken), crash-notify, `publish_runtime_failure` with
   a "Consumer outer task terminated abnormally (panic/abort)" message
   (Supervision + Route Lifecycle Compensation vocabulary; do NOT claim
   JoinError inspection — the guard reports termination, the outcome state
   reports accounting).

No double-publish proof: every body path that publishes sets `Accounted`
BEFORE any fallible cleanup (stop) runs; a later panic during cleanup finds
`Accounted` → silent. The only `Pending` terminations are (a) panic between
mark_ready and the publish points, (b) panic/abort in unaccounted cleanup —
both SHOULD publish exactly once.

### 4. Wiring

In `spawn_consumer_task`'s `_` arm: construct outcome + oneshot + guard;
the task body takes the guard (dropped at end); spawn
`spawn_outer_task_watcher` ONLY AFTER the controller's
`await_consumer_startup` resolves Ok — i.e., the watcher spawn lives at the
CALL SITE (route_controller_trait.rs start + resume paths, right after the
handshake Ok branch), mirroring how `watcher_inputs` →
`spawn_failure_watcher` is already conditional there. Rollback branches
(Err handshake) abort the task before any watcher exists → nothing to
observe, no false positive. Extend the existing `watcher_inputs` tuple
channel: the Explicit arm returns outer-watcher inputs as a second
`Option<...>` (or a combined enum) so the call-site pattern stays uniform.

Exact plumbing: `spawn_consumer_task`'s return widens from
`(JoinHandle<()>, StartupReceiver, Option<ImmediateWatcherInputs>)` to a
4-tuple appending `Option<OuterWatcherInputs>` (None for the Immediate arm;
Some for the `_` arm). Both call sites destructure 4-way:
- start path (route_controller_trait.rs:578):
  `let (consumer_handle, startup_rx, watcher_inputs, outer_inputs) = ...;`
- resume path (:794): same 4-way destructure.
After the handshake-Ok branch (past the Err rollback), each site calls
`if let Some(inputs) = outer_inputs { consumer_management::spawn_outer_task_watcher(inputs); }`
— mirroring the existing conditional `spawn_failure_watcher` call shape
directly above it. ALL existing 3-tuple destructures gain a fourth
`_outer_inputs` element: 13 in consumer_management.rs tests, 5 in
handshake_tests.rs, and the third PRODUCTION call site (aggregate start,
route_controller.rs:1142-1150) — widened with the tuple change and wired
with its own watcher spawn in the controller task.

Controller abort/cancel rollback flows otherwise unchanged.

### 5. Tests (consumer_management.rs test module; `outer_task_watcher`
filter; one real-bus)

Fakes needed: `PanickingAfterReadyConsumer` (start: mark_ready → panic),
`PanickingStopConsumer` (start Ok+ready; stop panics), `CleanConsumer`
(± bg handle), `CancelAbortConsumer` (park until cancelled).

- `outer_task_watcher_failroute_on_panic_between_ready_and_completion` —
  REAL RuntimeBus (pattern: route_controller_tests.rs:3245-3315): build a
  full route via the controller with the panicking-after-ready consumer;
  after the watcher fires, `GetRouteStatus` must report `Failed` — proves
  lifecycle state, not just command recording.
- `outer_task_watcher_failroute_on_panic_in_stop` — RecordingRuntime
  harness (:1456-1483): one FailRoute recorded.
- `outer_task_watcher_silent_on_normal_completion` — ± bg handle: zero
  FailRoute.
- `outer_task_watcher_silent_on_cancel` — cancel then abort: zero.
- `outer_task_watcher_no_double_publish_when_stop_panics_after_bg_publish`
  — bg task fails (monitor publishes), then finally-stop panics: exactly
  ONE FailRoute (the bg one; outer stays silent — Accounted).
- `outer_task_watcher_failroute_on_uncancelled_abort` — a consumer whose
  body parks on a never-fired signal; the test aborts the outer task
  WITHOUT cancelling the consumer token (simulating an external/unowned
  abort): exactly one FailRoute (Pending + not-cancelled → abnormal).

## Affected crates

- camel-core: consumer_management.rs (outcome state, guard, watcher,
  wiring-out, tests); route_controller_trait.rs (watcher spawn after
  handshake Ok, start + resume). No public API.

## Architecture boundaries

Same seam as the Immediate failure watcher and bg monitor (pub(crate)
consumer task-management). Standard crash-notify + publish_runtime_failure
channels — no new control-plane surface, no handshake change.

## Phases

Single-phase: outcome state + guard + watcher + wiring + tests form one
slice; no useful sub-deliverable.
