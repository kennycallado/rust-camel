# Proposal: explicit-task-watcher

## Why

rc-ava7 audit (e_gpt, 2026-08-25) handshake-gap finding (bd rc-a7rh): three
mechanisms cover Explicit-consumer startup failures — first-signal-wins
handshake (consumer.rs), the Result-error path in the spawned task
(consumer_management.rs Err branch → mark_failed + crash-notify +
publish_runtime_failure), and the background-handle monitor (bg JoinError →
crash-notify + publish_runtime_failure) — but an OUTER consumer task that
panics or is aborted BETWEEN `mark_ready()` and normal task-body completion
bypasses all three: the handshake already resolved Ok, no `Result` is
produced, and the bg monitor watches the consumer's own background task, not
the outer task. The controller then installs a dead handle
(route_controller_trait.rs, `managed.consumer_handle = Some(consumer_handle)`)
and confirms Started — the route is up-but-dead.

Complicating fact: for Explicit consumers WITHOUT a `background_task_handle`,
the outer task ends normally right after `start()` returns + `stop()` runs —
a finished outer handle is ROUTINE. The defect is specifically ABNORMAL
termination (JoinError) that nobody observes.

bd: rc-a7rh (discovered-from rc-ava7).

## What Changes

Close the gap with an outcome-accounted state + termination drop guard:

1. The Explicit task body (the `_` wildcard arm — the only Explicit body;
   the Immediate arm is untouched) shares an `OuterOutcome` state
   (Pending/Accounted) with a watcher: every body path that publishes a
   failure sets `Accounted` BEFORE any fallible cleanup (the finally
   `stop()`); the normal path accounts after `stop()` completes. A
   termination that finds `Pending` is abnormal — nobody accounted for it.
2. A task-local drop guard fires a oneshot in ALL termination modes
   (normal return, panic unwind, abort — tokio abort drops the future):
   no polling loop, zero cost while the task runs.
3. A detached outer-task watcher (modeled on the Immediate failure
   watcher) awaits the oneshot termination signal — no polling anywhere;
   cancelled terminations and `Accounted` outcomes are silent; `Pending` +
   not-cancelled publishes the standard failure trio (crash-notify +
   publish_runtime_failure → FailRoute). The watcher spawns only after the
   startup handshake resolves Ok, so rollback terminations
   (abort-then-cancel ordering in the start/resume error branches) are
   never watched.
4. Whole-lifetime watch (readiness window through post-install), closing
   also the post-install panic (e.g. inside the finally-`stop()`).

Explicitly excluded: no change to the handshake protocol, the Immediate
watcher, controller abort/cancel rollback flows, or public API.

## Acceptance criteria

- Deterministic regression test: an Explicit consumer that calls
  `mark_ready()` then panics → route reaches Failed (FailRoute published),
  NOT Started-with-dead-handle.
- A consumer that panics inside `stop()` after a successful start → also
  reaches Failed (whole-lifetime watch).
- Normal-completion consumers (with and without bg handles) and
  cancelled-consumer terminations produce NO FailRoute (no false positives).
- Existing startup/crash tests green; `cargo test -p camel-core --lib`
  green; gates green (rc-q74u/rc-oo0c exemptions as usual).

## Risk budget

Acceptable: one extra watcher task per Explicit consumer start (bounded,
dies with the watched task via the oneshot termination signal — no polling,
zero steady-state cost; detection is immediate at task end). This is
observability + state-repair, not a data-plane path.
Out of bounds: changing the handshake, changing controller flows, watching
the consumer's internal unmanaged tasks (a separate, pre-existing gap).
