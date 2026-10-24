# bridge-process-lifetime Specification

## Purpose
TBD - created by archiving change bridge-child-lifetime. Update Purpose after archive.
## Requirements
### Requirement: Supervisor-first process group supervision

On Unix the system SHALL establish a shell watchdog as its own process-group
leader BEFORE spawning the bridge, and the bridge child SHALL join that group
via a child-side `pre_exec` `setpgid` before exec (a failed join SHALL abort
the spawn before exec). All teardown paths SHALL signal process groups only —
never bare PIDs — with bounded SIGTERM-then-SIGKILL escalation applied
unconditionally within the teardown window, independent of group-leader
liveness.

#### Scenario: Launcher descendants die with the group on graceful stop

- **GIVEN** a bridge child spawned through `BridgeProcess::start` whose
  executable is a script that starts a TERM-ignoring long-running grandchild
- **WHEN** `stop()` completes
- **THEN** neither the child nor the grandchild appears as a running process

#### Scenario: Escalation does not depend on leader liveness

- **GIVEN** a bridge group whose leader exited while TERM-ignoring
  descendants remain
- **WHEN** teardown completes its bounded escalation
- **THEN** no group member remains as a running process

#### Scenario: Parent death between teardown signals

- **GIVEN** a bridge group whose teardown has sent the group SIGTERM but not
  yet escalated
- **WHEN** the parent binary is killed with SIGKILL in that window
- **THEN** no group member remains as a running process within a bounded wait

### Requirement: Parent-liveness watchdog

On Unix the watchdog SHALL be a POSIX-shell process using builtins only —
`trap "" TERM` for SIGTERM immunity, a `read` loop on the pipe, and its own
group kill — with stdin connected to the read end of a close-on-exec pipe
whose write end is held exclusively by the parent, and stdout/stderr NOT
inherited from the parent. On pipe EOF the watchdog SHALL kill its own
process group. Arming SHALL be verified by the parent (watchdog alive,
leading its group, SIGTERM immunity established) before the bridge spawns,
and establishment SHALL be fail-closed: inability to arm aborts `start()`
with no bridge process existing.

#### Scenario: Parent killed by SIGKILL

- **GIVEN** a live bridge group containing a launcher child and its
  grandchild, spawned through the real spawn path
- **WHEN** the parent test binary is killed with SIGKILL
- **THEN** no group member appears as a running process within a bounded wait

#### Scenario: Harness exit without destructors

- **GIVEN** a live bridge group whose owning handle was leaked (simulating
  test-harness `exit()` skipping destructors)
- **WHEN** the parent process exits normally
- **THEN** no group member appears as a running process within a bounded wait

#### Scenario: Panic unwind before exit

- **GIVEN** a live bridge group and a parent that panics, unwinds, and exits
- **WHEN** the parent exits
- **THEN** no group member appears as a running process

#### Scenario: Teardown SIGTERM does not disarm the watchdog

- **GIVEN** a live bridge group
- **WHEN** teardown sends the group SIGTERM
- **THEN** the watchdog remains alive (SIGTERM-immune) until group cleanup
  completes

#### Scenario: Caller thread retirement does not kill a live bridge

- **GIVEN** a bridge child spawned from a thread that then exits while the
  process lives
- **WHEN** the child is later stopped through its handle
- **THEN** the child was alive and stoppable throughout, and teardown succeeds

#### Scenario: Watchdog establishment failure is fail-closed

- **GIVEN** the watchdog cannot be spawned or fails arming verification
- **WHEN** `start()` runs
- **THEN** it returns an error and no bridge process was spawned or remains
  running

### Requirement: Startup-failure group cleanup

The system SHALL own group cleanup from before the bridge spawn through
readiness handoff: any early return from `start()` (arming failure,
readiness timeout, malformed ready line, closed stdout, cancellation) SHALL
leave no member of the bridge's process group running while the parent
remains alive. The system SHALL detect early watchdog exit while the parent
lives and SHALL trigger group cleanup when it happens.

#### Scenario: Readiness timeout with parent alive

- **GIVEN** a bridge child that never emits its ready line
- **WHEN** `start()` returns its timeout error
- **THEN** no member of the bridge's process group appears as a running
  process

#### Scenario: Early watchdog death triggers cleanup

- **GIVEN** a live bridge group whose watchdog exits while the parent is
  alive
- **WHEN** the monitor detects the watchdog exit via non-reaping observation
- **THEN** the bridge group receives group cleanup and the condition is
  surfaced to the caller or log

### Requirement: Single-owner signaling and PGID anchoring

Every guard-executed group signal SHALL be serialized through the guard.
Exit observation before the final group signal SHALL NOT reap: the bridge and
the watchdog SHALL be observed via `waitid` with `WNOWAIT` (death seen, child
kept waitable). The stored group id SHALL be invalidated after the final
group SIGKILL and before any reap, and no group signal SHALL be issued after
invalidation. The monitor SHALL own nothing — not the write end, not the
child handles, not a strong guard reference — and SHALL terminate when the
guard drops.

#### Scenario: Handle drop without stop still fires EOF cleanup

- **GIVEN** a live bridge group whose handle is dropped without `stop()`
- **WHEN** the guard drops (closing the parent's write end)
- **THEN** the watchdog sees EOF and the group is cleaned up within a bounded
  wait

#### Scenario: No signal after reap

- **GIVEN** teardown that has reaped the bridge child
- **WHEN** any late cleanup request arrives
- **THEN** no group signal is issued (the group id was already invalidated)

### Requirement: Descriptor discipline for the liveness pipe

The liveness pipe SHALL be created close-on-exec; the parent SHALL hold the
only write end (never writing to it); no auxiliary copy SHALL reach any
child; and concurrent bridge spawns SHALL each use their own pipe. Child
stdio other than the specified pipes SHALL NOT be inherited from the parent.

#### Scenario: Concurrent spawns keep EOF discipline

- **GIVEN** two bridge groups spawned concurrently
- **WHEN** one owning handle drops
- **THEN** only that spawn's group is cleaned and the other remains live

### Requirement: Bridge stderr is owned, not inherited

By default the system SHALL pipe bridge stderr to the parent and drain it with
the same bounded pattern as stdout (bounded line size, rate-limited
forwarding, overflow dropped with a summary), and the drain SHALL start
immediately after spawn before readiness parsing, so stderr traffic can never
delay or deadlock readiness. The `CAMEL_BRIDGE_LOG_STDERR` file-redirect
override SHALL keep its current behavior and failure semantics.

#### Scenario: Pipeline on the parent's stderr reaches EOF

- **GIVEN** the parent test binary's stderr is connected to a consumer that
  waits for EOF
- **WHEN** a bridge child is spawned and the parent exits
- **THEN** the stderr consumer observes EOF and completes instead of hanging

#### Scenario: Chatty stderr does not delay readiness

- **GIVEN** a bridge child that floods stderr before emitting its ready line
- **WHEN** `BridgeProcess::start` runs
- **THEN** readiness is detected within the normal timeout and memory stays
  bounded

#### Scenario: File capture override unchanged

- **GIVEN** `CAMEL_BRIDGE_LOG_STDERR` points at a directory
- **WHEN** a bridge child writes to stderr
- **THEN** the output lands in the per-bridge log file as before

### Requirement: Reaping and cleanup completion

Graceful stop SHALL synchronously reap the bridge child and the watchdog
within a bounded time, and only AFTER the final group signal and group-id
invalidation. Handle drop SHALL perform best-effort group kill, invalidation,
and non-blocking reap. Cleanup completion SHALL be defined as no RUNNING group
member within a bounded wait; already-killed zombies pending reaping are not
survivors.

#### Scenario: Graceful stop reaps both children

- **GIVEN** a live bridge group
- **WHEN** `stop()` completes
- **THEN** both the bridge child and the watchdog are reaped and no group
  member is running

### Requirement: Documented platform guarantees

The bridge documentation SHALL state the child-lifetime guarantees per
platform: full guarantee on Linux and macOS (POSIX shell builtins, `setpgid`,
`trap`, and `waitid`, no `/proc` dependency), and explicit-path teardown only
on Windows, where no process groups or watchdog exist.

#### Scenario: Platform statement present

- **WHEN** the bridge service documentation is inspected
- **THEN** it states the Linux, macOS, and Windows child-lifetime behavior

