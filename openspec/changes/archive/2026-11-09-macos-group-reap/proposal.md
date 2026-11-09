# Proposal: macos-group-reap

## Why

`CI Weekly` run `42565698846` (2026-11-09, sha `14fd50e8a`) failed 2 of 103
`camel-bridge` lib tests on macOS: `group::tests::drop_reaps_without_signals`
(`group.rs:1075`) and `group::tests::dash_shell_establishes_and_drops`
(`group.rs:908`). A rerun of the same workflow on the same SHA passed
(`42588284828`) — this is a race, not a deterministic contract violation.
History: ~2 reds in 5 weeks, each time a different test of the same teardown
contract (`early_watchdog_death_surfaces` on 2026-10-26, fixed by `f2956dab`).

The asymmetry that makes the race non-convergent on macOS: the Linux
`pid_running` test helper filters `Z`/`X` states via `/proc` (`group.rs:776`),
while the non-Linux helpers use bare existence probes — `kill(pid, 0) == 0` and
`kill(-pgid, 0) == 0` (`group.rs:821-832`) — and both answer success for a
zombie. `Drop` reaps with `ReapMode::NonBlocking`, a single `try_wait`
(`group.rs:339-344`), so a group SIGKILL landing after that one probe leaves a
zombie the poll counts as alive forever; the 5 s `wait_until` can never
converge. The failure message then cannot distinguish "zombie we failed to
reap" (test-harness defect) from "a process that genuinely outlived the guard"
(real teardown defect) — the difference between a flaky red and a `rc-zctz2`
incident. bd: `rc-zctz2`.

## What Changes

All changes live in the test module (`mod tests`, `#[cfg(all(test, unix))]`)
of `crates/services/camel-bridge/src/group.rs`:

- Zombie-aware non-Linux helpers: `pid_running` and `group_running` stop
  counting zombies as running, matching Linux semantics.
- The macOS mechanism is a single `/bin/ps -A -o pid=,pgid=,stat=,comm=`
  snapshot; its parsing is extracted as pure functions
  (`survivors_from_ps`, `is_alive_state`) unit-tested on Linux with literal
  `ps` fixtures — the only verification path available on this host.
- Legible failures: when `wait_until` expires, the assertion message reports,
  per pid and for the group, the raw `ps` state, the `kill(0)` probe result,
  and the command name — zombie-vs-alive stated outright.
- The `zombie_probe_contract` macOS assertion flips from pinning the trap
  (`pid_running(zombie) == true`) to pinning the fix (`!pid_running(zombie)`).

Explicitly excluded: production `Drop`/reap behavior (stays non-blocking),
the 5 s bound (stays), any new sleeps or retry loops, any change to the Linux
`/proc`-based helpers, any shell dependency added on Linux.

## Acceptance criteria

- `cargo test -p camel-bridge --lib` passes on Linux (before/after evidence in
  the park).
- New unit tests lock the parser/classifier with literal `ps` fixtures
  (running, sleeping, zombie, mixed group) and pass on Linux.
- A fixture built from the failed run's survivor line produces a diagnostic
  naming the zombie class without a second iteration.
- Full local sweep `bash scripts/gates-local.sh` exits 0
  (`passed 28, failed 0, skipped 4`).

## Risk budget

Low: test-only code, no production surface. The macOS branch itself is
CI-only-verifiable; the risk of a wrong macOS helper is bounded by the
conservative fallback (if the `ps` snapshot is unavailable, the old strict
kill-probe semantics apply, so a broken helper cannot mask a real survivor).
Out of bounds: changing the teardown contract, widening the bound, or making
`Drop` blocking to "fix" the flake.

## Affected crates

- `camel-bridge`: test-module helpers only (`group.rs` `mod tests`).
