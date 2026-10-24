# Proposal: bridge-child-lifetime

## Why

Bridge child processes (jms/xml/cxf) are spawned with stderr inherited from the
parent (`crates/services/camel-bridge/src/process.rs`). When a test binary
spawns a bridge and exits, the harness `exit()` skips Rust destructors, so
`kill_on_drop` never fires and the child outlives the test while holding the
test's stderr file descriptor. Any consumer piping that stderr waits for EOF
that never comes: published-bridge-smoke run 39578427824 passed all 3 WSS e2e
tests in 2 s and then hung until step timeout with three `cxf-bridge` processes
alive at cancel. The CI workflow now redirects stderr to a file and kills
leftovers — a workaround that hides the bug from every other consumer.

bd: rc-r8od1.

## What Changes

- Bridge children live in a process group supervised by a shell watchdog that
  is established BEFORE the bridge spawns: the watchdog is its own group
  leader, immune to SIGTERM, holding the read end of a close-on-exec pipe
  whose write end is held exclusively by the parent; the bridge joins the
  group at exec time. Parent death by ANY route — including during teardown
  windows — closes the pipe; the watchdog sees EOF and kills the group with
  its own shell-built-in kill, including descendants a launcher spawns. A
  single guard exclusively owns the write end, both child handles, and all
  group signaling; exit observation before the final group signal is
  non-reaping (`waitid` with `WNOWAIT`); the stored group id is invalidated
  after the final SIGKILL and before any reap; guard drop, startup failure,
  cancellation, and early watchdog death all trigger group cleanup. Teardown
  signals only process groups, never bare PIDs, with unconditional bounded
  escalation; no mechanism relies on Rust destructors or exit handlers
  running in the parent.
- Bridge stderr stops being inherited by default: it is piped and drained by
  the same bounded pattern as stdout (64 KiB line bound, rate-limited
  forwarding, overflow dropped with a summary). The drain starts immediately
  after spawn, before readiness parsing, so a chatty JVM cannot deadlock
  startup. `CAMEL_BRIDGE_LOG_STDERR` file capture is unchanged.
- Exit-path evidence: an ignored integration suite reproduces the pipeline-hang
  before the fix and proves EOF + no group survivor after, across normal exit,
  panic, and signal paths — including parent death between teardown signals —
  using a launcher-script bridge with a grandchild, through the real
  `BridgeProcess::start` spawn path.
- Docs state the platform guarantees (Linux and macOS full; Windows
  explicit-path teardown only).
- Included: camel-bridge only. The single shared spawn path covers all bridge
  specs consumed by camel-jms, camel-cxf, camel-validator, camel-xj,
  camel-xslt. Excluded: bridge supervision/restart redesign, smoke assertion
  changes, version bumps. The CI workflow workaround stays (see design).

## Acceptance criteria

- Before-fix reproduction shows the hang (pipeline blocked, child orphaned);
  after the fix the same harness shows pipeline EOF and an empty process table.
- No bridge group member (child or grandchild) survives its parent binary on
  normal completion, panic unwind, SIGTERM, SIGINT, or SIGKILL (Unix) —
  including parent death between teardown signals — verified by process table;
  zombies (reaped-pending) are not survivors.
- Startup failures, cancellation, and early watchdog death while the parent
  lives leave no group member running.
- Teardown never signals an unrelated process or group; watchdog arming
  (including SIGTERM immunity) is verified before the bridge spawns and is
  fail-closed.
- Existing stderr file capture (log-capture consumers) still works; stderr
  flood cannot buffer unboundedly or delay readiness.
- `cargo test -p camel-bridge` passes; full gate registry per AGENTS.md passes
  (pre-existing failures rc-yfsw6, rc-83l5f exempted).

## Risk budget

Acceptable: teardown changes alter only kill mechanics, not supervision or
restart policy; the watchdog adds one short-lived `sh` child per bridge and
uses shell builtins only. Unbounded buffering and readiness-deadlock
regressions are guarded by dedicated tests. Out of bounds: changing smoke
assertions, bridge Java side, restart/health-probe logic.
