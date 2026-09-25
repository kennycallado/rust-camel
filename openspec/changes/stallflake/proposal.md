# Proposal: stallflake

## Why

CI Weekly macOS leg (run 35563884247, 2026-09-21) failed
`runtime::tests::invoke_stall_progress_resets_timer` with
`progress should have reset the stall timer`
(camel-component-wasm, 1/178 tests). Linux CI passed at the same
commits. This is the `platform-timing` flake class from epic rc-99d5,
not a regression.

Root cause (reasoned from code, see design.md): the test drives the
production watchdog with wall-clock margins. Ping interval 25 ms
against a 40 ms timeout leaves a 15 ms window. A loaded macOS runner
can delay the watchdog task poll past that window. When the poll
finally runs, both a stored `Notify` permit and the elapsed
`tokio::time::sleep` are ready. `tokio::select!` without `biased`
polls branches in random order. The timeout branch can win, so the
watchdog reports a stall even though progress arrived on time.

## What Changes

- Rewrite `invoke_stall_progress_resets_timer` with
  `#[tokio::test(start_paused = true)]`. Paused time removes the
  wall-clock margin. The clock only advances when all tasks are idle,
  so a stored progress permit is always observed before the timeout
  deadline.
- Apply the same rewrite to `drain_watchdog_passes_when_chunks_flow`.
  It has the same pattern: 5 pings 10 ms apart against a 50 ms drain
  watchdog (40 ms window).
- Add `tokio = { workspace = true, features = ["test-util"] }` to
  camel-component-wasm dev-dependencies. Nine crates already use this
  pattern, for example camel-ws and camel-http.
- Mutation check: with the timer reset broken (sleep armed once
  outside the loop), the rewritten test must fail. Recorded as
  evidence in the change, not committed as code. The cargo test
  harness cannot pin a source mutation as a test.
- Production watchdog (`drive_with_drain_watchdog`) stays at 40 ms
  semantics. No production incident shows the race at production
  load. The unfair-select observation goes to epic rc-99d5 as a note.
- Sweep of other wall-clock-margin tests in camel-component-wasm:
  findings recorded in design.md. Delay-safe direction tests stay
  unchanged. Out-of-scope sites are noted for rc-99d5.

Affected crate: `crates/components/camel-component-wasm` (test code
and dev-dependencies only). bd: rc-3mdrx.

## Acceptance criteria

- `cargo test -p camel-component-wasm --lib` passes 178+ tests,
  including both rewritten tests.
- Mutation check evidence: broken timer reset makes
  `invoke_stall_progress_resets_timer` fail; the evidence is in the
  change directory.
- `cargo xtask lint-test-sleep` count does not rise above the
  ratchet-test-sleep ceiling (no net new sleep sites).
- `cargo xtask lint-unbounded-wait` count does not rise above 296.
- Spec delta: none (`skip_specs: true`).

## Risk budget

- Test-only change. Production watchdog code is out of bounds.
- `start_paused` changes what the tests measure: scheduling fairness
  is no longer under test, timer-reset logic still is. That is the
  intent.
- The two rewritten tests must still fail when the reset is broken.
  A rewrite that passes under any watchdog code is rejected.
