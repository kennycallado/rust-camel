# Design: clitesthook

## Approach

Two moving parts, prescribed by the archived `flakytests` design (Flake 2
section), with the observation boundary fixed in spec-bless round 1
(e_gpt REJECT → prescribed correction, 2026-11-12):

**1. Opt-in seam at route pipeline entry, AFTER the drain claim exists
(production, default off).**
Round 1 rejected a seda `forward_envelope`-entry placement: the seda
forwarder prints the marker, then can be descheduled before
`ctx.send_and_wait`; shutdown aborts the forwarder before the route
pipeline acquires its `DrainGuard` (`route_controller_trait.rs:695`),
the drain counter stays zero, teardown completes before TERM — the
original race, reproduced. The observation must sit AFTER the route's
teardown-holding claim exists.

The seam therefore lives in the sequential route controller loop
(`crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs`):
a read-once hoist before the consumer loop —

```rust
let pipeline_marker = std::env::var_os("CAMEL_JOB_PIPELINE_MARKER").is_some();
```

— and the gated emission immediately after the `DrainGuard` binding
(~line 695) and before the `CANCEL_TOKEN.scope(... pipeline.call ...)`:

```rust
if pipeline_marker {
    eprintln!("camel job: pipeline entered");
}
```

The hoist (holistic-review minor, e_opus: per-exchange `var_os` is a
getenv + env lock on the hot path; resolved in 73957e86a, delta
APPROVE by r_glm) keeps the hot path free of per-exchange env
lookups; the env is immutable after process spawn, so the task-start
read is semantically identical.

At this point `drain_in_flight >= 1` for this exchange: any shutdown
started after the marker is observed must wait on the drain (budget
`max(remaining, 5s)` ≈ 55 s vs the 30 s-delay exchange) before it can
finish — the guard window is seconds wide. This matches the mission
order's literal wording ("when the route pipeline is entered"); the
archived 576 note ("at consumer dispatch") described the same
requirement coarsely and is superseded by the round-1 boundary fix.
The gated write is not a refactor of the route runtime (explicitly out
of bounds); it is the worker-path marker the order authorizes.
Per-dispatch emission (no once-guard): the test fixture sends exactly
one exchange, so one dispatch = one line; `wait_for_marker` matches
first occurrence, so extra lines would be inert anyway. `eprintln!`
mirrors the `CAMEL_JOB_SIGNAL_MARKER` / `CAMEL_JOB_SHUTDOWN_MARKER`
precedent (`camel-cli/src/commands/job/mod.rs:1012`, `:2128`):
unbuffered, subscriber-independent. Marker text is collision-free
workspace-wide (pre-flight check). `lint-log-redaction` targets
tracing only; `eprintln!` test seams are established precedent.

**2. Deterministic test replacing the blind pair.**
`job_signal_second_signal_forces_exit` keeps its name and contract but swaps
the blind `sh -c "kill -INT; kill -TERM"` pair for the observed sequence:

1. Spawn with `spawn_camel_job_with_env(..., &[("CAMEL_JOB_PIPELINE_MARKER", "1")])`
   on the held one-shot fixture forced onto the sequential dispatch arm
   (`write_sequential_held_one_shot_fixture`: `seda:work`, `delay: 30000`,
   `waitForTaskToComplete=Always`, `timeout: 60s`, `log_level = "INFO"`, plus
   route-level `sequential: true` — seda declares
   `ConcurrencyModel::Concurrent`, production dispatch takes the concurrent
   arm, and the seam lives in the sequential loop, so the flag is what makes
   the teardown-holding claim observable; task-review amendment 2026-11-12,
   e_opus ruling on worker deviation).
2. Wait for `CamelContext started` (`camel-core/src/lifecycle/application/
   context_lifecycle.rs:221`, INFO → stderr) AND `camel job: pipeline entered`.
   The marker proves worker claim + dispatch of the teardown-holding exchange
   before any signal — the observation 576's experts required.
3. `kill -INT` (elapsed clock starts here); the send race is now in flight
   (held by the 30 s pipeline), so the interrupted branch wins.
4. Wait for `Stopping CamelContext` (`context_lifecycle.rs:254`). The
   force-exit guard is spawned BEFORE `shutdown()` in the interrupted branch
   (`job/mod.rs` ~1650), so observing this line proves the guard owns the
   signal streams; any later TERM is buffered and consumed on the guard's
   first poll.
5. `kill -TERM` → force-exit guard fires: WARN + `exit(1)`.

Assertions byte-identical to today: `exit == 1`, `elapsed < 15 s`
(`FORCE_EXIT_BOUND`), `forcing exit` WARN present in captured output. The
teardown budget (`shutdown_budget`, one-shot floor `max(remaining, 5s)` ≈ 55 s)
and the 30 s-delay exchange hold the drain for the whole window — the guard
window is seconds wide, so the TERM never races teardown completion.

## Affected crates

- camel-core: 3-line env-gated marker in the sequential route controller loop
  (`route_controller_trait.rs`, after `DrainGuard` acquisition).
- camel-cli: test-only (`tests/job_signal_test.rs` rewrite; harness
  `spawn_camel_job_with_env` already exists in `tests/common/mod.rs`).

## Architecture boundaries

Runtime/DSL/Services/Languages/Functions untouched. The camel-core change is
one gated write inside the existing sequential controller loop — not a
refactor of the route runtime, its queue/claim semantics, the delayer, or
seda (all explicitly out of bounds). No consumer sees different behavior with
the env var unset (default off: no emission, no I/O — the unset gate costs
one env lookup and a branch per dispatch, mirroring the camel-cli precedent
markers). No spec delta: the seam is test-only and default-off; the
`cli-jobs` second-signal scenario (`specs/cli-jobs/spec.md` force-exit
scenario) is behavioral and remains satisfied verbatim by the rewritten
test; no documented CLI contract changes. `skip_specs: true`. Do not rename
`job_signal_test.rs` (it is named in the `test-determinism` color-env audit
set).

## Phases (optional)

Omitted — single-phase change (one coherent slice: seam + test + evidence).
