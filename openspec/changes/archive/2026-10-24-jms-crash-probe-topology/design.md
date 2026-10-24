# Design: jms-crash-probe-topology

## Topology after mission 390 (verified)

```
jms_test (parent)
└── probe (group leader, pgid == pid; group holds ONLY the probe)
    └── owns JmsBridgePool → GroupGuard
        ├── watchdog `sh` (own group leader, TERM-immune, holds liveness-pipe READ end as stdin)
        └── jms-bridge (joins the watchdog's group at exec: setpgid(0, watchdog_pgid))
```

The liveness pipe is `O_CLOEXEC`; only the probe process holds the write end.
Probe death (even SIGKILL) closes it → watchdog `read` returns EOF →
`kill -KILL -0` takes down the supervisor group (watchdog + bridge). The
bridge is a child of the probe process but a member of the supervisor group.

Consequence (pre-flight e_opus, NO-GO on the naive fix): the old test's
orphan phase — bridge survives `kill_leader_only`, holds the lock, releases
it on stream end while the survivor asserts — is unreachable. Fixing only the
discovery step would move the panic to the post-kill survival assertion. The
spec's "harness SHALL reap the orphan bridge" scenario describes a state that
no longer exists; the normative redelivery/count/lock contract is unaffected
(the lock is an OS lock the kernel releases at bridge process exit).

## Handoff design

- Publication lives in the probe's processor, BEFORE the marker/receipt write
  (expert fix for the publication race: the marker can only appear after an
  accepted exchange, which implies the pool slot exists; writing the handoff
  first means "marker visible" ⇒ "handoff published"). No poll on the test
  side.
- The probe reads the pid via a new read-only accessor
  `JmsBridgePool::bridge_pid(broker_name) -> Option<u32>`:
  `resolve_broker_name` → `slots.get` → `slot.process.lock()` →
  `BridgeProcess::child_pid()`. A tiny bounded poll (deadline 5s) covers
  slot-mutex timing; `None` at deadline is a processor error (probe exits
  non-zero, writes no marker).
- Supervisor pgid via `libc::getpgid(bridge_pid)` (unix-gated).
- Artifact: `<XDG_STATE_HOME>/bridge-handoff-<probe_pid>.json` =
  `{"probe_pid":u32,"bridge_pid":u32,"supervisor_pgid":i32}`, written with the
  existing atomic-rename helper. Per-probe filename: probe and survivor never
  collide; the publisher pid inside makes a stale artifact detectable.
- Test-side validation (`read_bridge_handoff`): (a) exists+parses, else Err
  naming "probe group <expected>" and the supervisor group; (b) `probe_pid`
  field equals the expected probe pid, else Err "stale" naming probe group and
  the artifact's supervisor group; (c) bridge pid alive non-zombie; (d)
  canonical `/proc/<pid>/exe` equals canonical expected binary; (e) pgrp
  cross-check: `/proc/<bridge_pid>/stat` pgrp == `supervisor_pgid` and != the
  probe pid — this is what proves the topology, not just the pid.

## Proof restructure

1. Marker wait (unchanged) → read handoff → owner_before == bridge_pid
   (exact-owner assertion kept).
2. `kill_leader_only` → bounded wait probe dead.
3. NEW death phase: bounded waits — `!process_alive(bridge_pid)` AND
   `live_group_members(supervisor_pgid).is_empty()`; a surviving bridge is a
   hard failure naming the watchdog (the proof can still FAIL when the
   watchdog is broken). Bounded lock-release observation: owner stops naming
   `bridge_pid`.
4. Survivor phase unchanged: same JMSMessageID/body/header,
   `reservation_count == 2`, two RESERVE frames, no COMMIT; survivor's own
   handoff read and its bridge's death asserted after cleanup.
5. The rc-icvzl "continued exact-owner lock while orphan alive" apparatus
   branch is removed with its unreachable state.

## Alternatives rejected

- Scan `/proc` globally by exe: a weaker identity than the owned pid and
  still not the supervisor group; rejected (mission mandates the handoff).
- Stdout marker line: the probe's stdio is inherited, not captured by the
  test; a file is race-free and needs no plumbing change.
- Keeping a fake-orphan phase by NOT killing the probe (killing only some
  detachable supervisor): would require GroupGuard semantic changes —
  non-goal.
- Publication from `run()` after `ctx.start()`: races the marker (pre-flight
  finding); processor-side publication removes the race.

## Gates notes

- `lint-unbounded-wait` ceiling is 0: every new wait is the bounded
  `wait_until` pattern; the probe-binary poll is main-scoped (not test-body)
  and deadline-bounded anyway.
- New public fn carries a doc comment (doc-build gate).
- `/proc` code unix-gated; crash tests are already `#[cfg(unix)]`.
