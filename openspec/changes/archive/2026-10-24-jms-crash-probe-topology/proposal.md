# Proposal: jms-crash-probe-topology

## Why

Main CI is red in `Full Tests (ubuntu-latest) / JMS integration tests` (run
40095647981): both crash tests
(`jms_bridge_crash_before_completion_redelivers_activemq`, `_artemis`) panic at
`jms_crash_probe_cases.rs:523` — "no process running …/jms-bridge in probe
group <pgid>". Mission 390 (8aee52fb8) moved every bridge child into a
watchdog-led supervisor process group (`GroupGuard::establish`,
`setpgid(0, pgid)`), so the probes' group scan
(`find_group_process_by_exe(probe_pgid, …)`) can never find the bridge again.
The probe-side fix must follow the new topology. bd: rc-baq06.

## What Changes

- **Authoritative pid handoff**: the `jms_crash_probe` binary publishes the
  bridge pid it owns (`BridgeProcess::child_pid` via a new read-only
  `JmsBridgePool::bridge_pid`) in a small JSON artifact under `XDG_STATE_HOME`
  before it writes the handoff marker; the test consumes that pid instead of
  scanning the probe group. Validation: pid alive, canonical exe equals the
  expected bridge binary, `/proc` pgrp cross-check (bridge is in the
  supervisor group, not the probe group), destination lock owner equals the
  pid.
- **Test restructure for the new topology** (pre-flight e_opus finding): the
  probe process holds the liveness pipe write end, so killing the probe makes
  the watchdog SIGKILL the supervisor group within milliseconds. The old
  "orphan bridge survives while the survivor asserts" phase is unreachable.
  The proof becomes: kill probe → prove exact bridge pid dead + supervisor
  group empty + lock released → survivor asserts redelivery and count
  inheritance. The proof still FAILS if the watchdog is broken (bridge
  survives).
- **Regression guard**: a no-broker unit test proves the handoff validation
  fails with a diagnostic naming the probe group and the supervisor group when
  the artifact is absent or stale.
- **Spec delta** on `JMS bridge crash before route completion preserves the
  message`: the harness contract moves from "reap the orphan bridge" to
  "prove no orphan remains (watchdog mechanism)"; parent-death binding is now
  the watchdog mechanism, not out of scope.

Affected crates: `camel-jms` (accessor), `camel-test` (probe binary, test
support, crash cases). No change to `camel-bridge` (GroupGuard semantics are
untouched — non-goal).

## Acceptance criteria

- `cargo test -p camel-test --features integration-tests --test jms_test`
  green locally with the docker broker legs, including both crash tests.
- Both crash tests pass 10x in a loop, zero failures.
- `cargo fmt --check --all`, clippy (workspace per AGENTS.md, incl. camel-test
  and camel-bridge), `cargo xtask lint-unbounded-wait` at ceiling 0.
- Handoff validation failure diagnostics name both groups.

## Risk budget

Acceptable: bounded-poll waits added on death/release paths (existing
`wait_until` pattern). Out of bounds: weakening exact-owner assertions to
"some group member", sleeps/retries/longer timeouts, GroupGuard semantic
changes, bridge bump, version bump.
