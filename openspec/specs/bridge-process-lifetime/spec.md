# bridge-process-lifetime Specification

## Purpose
TBD - created by archiving change bridge-child-lifetime. Update Purpose after archive.
## Requirements
### Requirement: Supervisor-first process group supervision

On Unix the system SHALL establish a watchdog process as its own process-group
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

On Unix, when the host binary installed the startup hook —
`camel_bridge::watchdog::maybe_run()` called as the first statement of a
synchronous `main`, before any other argv consumer — the watchdog SHALL be
the supervising executable itself, re-executed in a hidden watchdog role
selected by the argv marker `__camel-bridge-watchdog` followed by the
acknowledgement fd number. In that role the process SHALL refuse to arm —
exiting non-zero before writing the acknowledgement — unless it is its own
process-group leader, SHALL ignore `SIGTERM`, SHALL write the single
acknowledgement byte to the inherited fd only after SIGTERM immunity is
established, SHALL read stdin until EOF, and SHALL then kill its own process
group. On Linux the supervising executable SHALL be re-executed via
`/proc/self/exe`; on other Unix systems via the executable path captured at
startup. When the hook is NOT installed, the system SHALL fall back to a
POSIX-shell watchdog using builtins only (`trap "" TERM`, a `read` loop on
the pipe, its own group kill). When neither the hook nor a POSIX shell is
available, establishment SHALL fail closed with an error naming both
remedies: installing the hook and providing a shell. In both forms the
liveness pipe is close-on-exec with its write end held exclusively by the
parent, stdout/stderr are NOT inherited from the parent, on pipe EOF the
watchdog kills its own process group, and arming SHALL be verified by the
parent (watchdog alive, leading its group, SIGTERM immunity established)
before the bridge spawns. Establishment SHALL be fail-closed: inability to
arm aborts `start()` with no bridge process existing.

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

#### Scenario: Start succeeds with an empty PATH and no shell

- **GIVEN** a host binary that installed the hook, spawned with an empty
  `PATH` and no shell present in the environment or image
- **WHEN** `BridgeProcess::start` runs and the parent is stopped
- **THEN** establishment succeeds through the self-executed watchdog, the
  bridge becomes ready, and teardown leaves no group member running

#### Scenario: No hook and no shell fails closed naming both remedies

- **GIVEN** a host binary that never installed the hook, running where no
  POSIX shell can be spawned
- **WHEN** `BridgeProcess::start` runs
- **THEN** it fails with an error that names both remedies — installing the
  `maybe_run()` hook and providing a POSIX shell — and no bridge process
  was spawned

#### Scenario: Watchdog refuses to arm when it is not a group leader

- **GIVEN** the watchdog role is invoked manually without becoming a process
  group leader
- **WHEN** the role starts
- **THEN** it exits non-zero without writing the acknowledgement byte, and
  the parent's arming verification fails closed

#### Scenario: Hook precedes artifact self-detection

- **GIVEN** a compiled artifact image whose runtime would otherwise consume
  the process argv
- **WHEN** the binary is re-executed with the watchdog role argv
- **THEN** the role is handled by the hook before artifact self-detection
  runs, and the watchdog arms normally

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
platform: full guarantee on Linux and macOS (`setpgid`, `waitid`, and the
watchdog process — self-executed when the hook is installed, POSIX shell
otherwise), and explicit-path teardown only on Windows, where no process
groups or watchdog exist. The documentation SHALL state the container
guarantee: bridges start in the `production` and `gnu` images with NO shell
and NO `/tmp` present, provided the operator supplies a writable cache
directory (or a writable `$TMPDIR`), and SHALL state the embedder contract
for installing the hook.

#### Scenario: Platform statement present

- **WHEN** the bridge service documentation is inspected
- **THEN** it states the Linux, macOS, and Windows child-lifetime behavior

#### Scenario: Container statement present

- **WHEN** the bridge service documentation is inspected
- **THEN** it states that bridge routes start in the shell-less
  `production`/`gnu` images without `/tmp`, and names the operator
  requirements (writable cache dir or `$TMPDIR`, hook installation)

### Requirement: TLS material directory without /tmp

The TLS material generator SHALL resolve its base directory without requiring
`/tmp`: `$TMPDIR` when set and usable, then the system temp directory when
usable, then `{cache_dir}/{version}/tls` created with mode 0700. When no
candidate is usable the error SHALL name the platform cache-directory
override (`XDG_CACHE_HOME` on Linux) as the remedy. The per-spawn PEM files
keep their unique directory, 0700/0600 permissions, and lifetime cleanup.

#### Scenario: Falls through to the cache dir when TMPDIR and /tmp are unusable

- **GIVEN** `$TMPDIR` unset or pointing at an unusable path and no usable
  system temp directory
- **WHEN** `BridgeTlsMaterial::generate()` runs
- **THEN** the PEM files live under `{cache_dir}/{version}/tls` in a unique
  0700 subdirectory and are removed on drop

#### Scenario: Cache-dir failure names the env override

- **GIVEN** no usable `$TMPDIR`, no usable system temp, and an unwritable
  cache directory
- **WHEN** `BridgeTlsMaterial::generate()` runs
- **THEN** it fails with an error naming `XDG_CACHE_HOME` as the remedy

