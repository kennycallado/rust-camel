# Design: stallflake

## Context

`WasmRuntime::drive_with_drain_watchdog` (camel-component-wasm
src/runtime.rs) guards invoke and drain phases. Phase 1 with
`Some(t)` loops over a four-branch select: run completion,
drain_started, progress_notify, and `tokio::time::sleep(t)`. Each
observed progress notification runs `continue`, which re-creates the
sleep future. That re-creation is the timer reset under test.

## The exact interleaving

Test shape before the fix:

```rust
for _ in 0..4 {
    p.notify_one();
    tokio::time::sleep(Duration::from_millis(25)).await;
}
ds.notify_one();
```

Watchdog timeout: `Some(Duration::from_millis(40))`.

Failure sequence on a loaded runner:

1. t=0: ping 1 stores one permit. The watchdog observes it and arms
   sleep(40 ms), deadline t=40.
2. t=25: ping 2 stores one permit and wakes the watchdog task.
3. The worker thread never polls the watchdog task before t=40.
   178 tests run in parallel on the macOS runner, so a 15 ms
   scheduling delay is routine.
4. t=40: the sleep deadline elapses. The stored permit from ping 2 is
   still unconsumed.
5. The worker polls the watchdog task at t>=40. Two select branches
   are ready at once: progress_notify (stored permit) and sleep
   (elapsed).
6. `tokio::select!` without `biased` polls branches in random order.
   With about 50 percent probability the sleep branch runs first. The
   watchdog returns `invoke stalled` although progress arrived at
   t=25, before the deadline.

`tokio::sync::Notify::notify_one` stores at most one permit. Pings
that arrive while a permit is already stored are lost. That is why
step 3 cannot be repaired by more pings: the permit exists, only the
poll is late.

Margin arithmetic: deadline (40 ms) minus last ping (25 ms) = 15 ms
of allowed scheduler delay. The prior weekly flake (09-07,
camel-ws `producer_retries_on_connection_refused`) belongs to the
same `platform-timing` class (epic rc-99d5): wall-clock margins that
loaded runners break.

## Why paused time removes the race

`#[tokio::test(start_paused = true)]` runs on a virtual clock. The
runtime advances the clock only when every task is idle. So the
sequence becomes deterministic:

1. Ping at virtual t=25 stores a permit and wakes the watchdog.
2. The runtime cannot jump the clock to t=40 while a woken task is
   ready to run. It runs the watchdog first.
3. The watchdog consumes the permit, re-arms sleep(40 ms), deadline
   moves to t=65.
4. The clock advances to the next timer (pinger sleep) only after
   both tasks are idle again.

The both-branches-ready state needs the clock to pass the deadline
while a permit sits unconsumed. Paused time forbids exactly that.
The test then measures the timer-reset logic alone, not scheduler
fairness. Precedent: camel-ws, camel-http, camel-processor,
camel-core, camel-cli already use `start_paused`.

## Mutation check

Goal: prove the rewritten test still detects a watchdog that does
not reset its timer.

Mutant: arm the sleep once outside the loop
(`let mut s = Box::pin(tokio::time::sleep(t));` before the loop,
`_ = &mut s` in the select). The progress branch still runs
`continue`, but the sleep future is not re-created.

Expected result under the mutant: the deadline stays at virtual
t=40. Pings at t=0/25/50/75 cannot move it. The clock reaches t=40
while the drive future still sleeps, so the watchdog returns
`invoke stalled` and the test fails.

Procedure (evidence, not committed code): apply the mutant, run
`cargo test -p camel-component-wasm --lib invoke_stall`, capture the
failure, revert. Paste the captured output into
`openspec/changes/stallflake/mutation-evidence.md`. The cargo test
harness cannot mutate source at test runtime, so no pinned
test-of-the-test exists.

## Sweep: wall-clock-margin tests in camel-component-wasm

| Test | Site | Shape | Margin | Verdict |
|---|---|---|---|---|
| `invoke_stall_progress_resets_timer` | runtime.rs | pings 25 ms vs 40 ms, expects no trip | 15 ms | FIX (observed flake) |
| `drain_watchdog_passes_when_chunks_flow` | runtime.rs | pings 10 ms vs 50 ms, expects no trip | 40 ms | FIX (same pattern, trivial) |
| `wasm_health_check_unhealthy_on_timeout` | health.rs | probe sleeps 50 ms, timeout 5 ms, expects the timeout | delay only delays the expected outcome | keep |
| `stop_does_not_wait_for_runtime_owned_run_task` | source_consumer.rs | stop() under a 100 ms timeout, expects no wait | 100 ms, no-wait path | keep, note for rc-99d5 |
| `stop_aborts_owned_run_task_after_grace` | source_consumer.rs | abort at ~5 s under a 15 s ceiling | 10 s | keep |
| `test_epoch_ticker_increments_epoch` | epoch.rs | sleep 20 ms, asserts is_running | delay-safe direction | keep |
| epoch busy-wait loop | epoch.rs:172 | 5 ms sleeps in a 100 ms window | liveness flag only | keep, note for rc-99d5 |
| stream_bridge chunk-progress asserts (4 sites) | stream_bridge.rs:635,690,739,788 | `timeout(50 ms, notified())`, permit stored before the await | window = one poll delayed past 50 ms; never observed | keep, note for rc-99d5 |

`invoke_stall_trips_on_no_progress`, `drain_watchdog_trips_on_stalled_drain`,
`invoke_stall_completes_before_drain_started`, and
`drain_watchdog_completes_without_drain_signal` have no race: delay
either delays the expected trip or is irrelevant.

## Production decision

The watchdog keeps 40 ms production semantics. The race needs a
worker poll delayed beyond the full 40 ms window while progress
flows. No production incident shows that condition. A `biased`
select (progress before sleep) would make coalesced progress always
win over an elapsed deadline, but that is a production behavior
change without production evidence. Noted for epic rc-99d5 as a
candidate hardening, not landed here.

## Phases

Single phase. Three tasks: dev-dependency plus the two test
rewrites, then evidence and gates. No spec delta
(`skip_specs: true`).
