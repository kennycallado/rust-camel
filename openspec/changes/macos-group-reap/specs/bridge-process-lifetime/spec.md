## ADDED Requirements

### Requirement: Zombie-aware teardown survivor evidence

Teardown tests SHALL NOT count a zombie (an exited-but-unreaped process) as
a surviving process — unconditionally on Linux via the `/proc`-based `Z`/`X`
state filtering, and on other Unix platforms whenever a usable state
snapshot is available via the platform's process-state tooling (a
`/bin/ps -A -o pid=,pgid=,stat=,comm=` snapshot), so that a state letter of
`Z` is never reported as running there; non-Linux platforms SHALL NOT gain
a shell dependency for Linux builds. A state snapshot SHALL be treated as
usable only when the tool exits zero and every non-blank output row parses;
when the snapshot is unavailable or unusable on a non-Linux platform, each
predicate SHALL fall back to its bare signal-existence probe's answer (so
missing state evidence can only make a test stricter, never mask a real
survivor). When a
bounded teardown wait expires, the assertion message SHALL state, for each
probed pid and for the probed process group, the raw state token (or
absence), the signal-existence probe result, and the command name,
classifying each as zombie, alive, or gone — and as unknown when state
evidence is unavailable, never equating unavailable evidence with gone.
This requirement governs test evidence only: the 5 s teardown bound, the
non-blocking `Drop` reap, and the no-new-sleeps and no-new-retry-loops
rules are unchanged.

#### Scenario: A zombie is not a survivor

- **GIVEN** a usable state snapshot and a teardown whose group SIGKILL
  landed after the guard's single non-blocking reap probe, leaving a child
  exited but unreaped
- **WHEN** a teardown test probes the child's liveness on any Unix platform
- **THEN** the zombie is not counted as running and the bounded wait
  converges instead of timing out

#### Scenario: Group with only zombie members reads cleaned

- **GIVEN** a usable state snapshot in which every remaining member of the
  probed process group is a zombie
- **WHEN** the group liveness predicate is evaluated
- **THEN** the group is reported as cleaned, matching the Linux
  `/proc`-filtering semantics

#### Scenario: Unusable snapshot falls back to the stricter probe

- **GIVEN** a non-Linux platform where the state snapshot fails (non-zero
  exit, empty output, or a non-blank row that does not parse)
- **WHEN** a survivor predicate is evaluated
- **THEN** the predicate returns its bare signal-existence probe's answer —
  the pid probe may then count a zombie as running, the pre-change
  behavior — and cannot report a false-empty group from dropped rows

#### Scenario: Red is legible

- **GIVEN** a bounded teardown wait that expires with a survivor or zombie
  present
- **WHEN** the assertion fires
- **THEN** the message names, per pid and for the group, the raw `ps`
  state token, the `kill`-probe result, and the command name,
  distinguishing a zombie pending reap from a process that genuinely
  outlived the guard, and classifying as unknown (never gone) when the
  snapshot was unavailable

#### Scenario: Classifier is verified from fixtures

- **GIVEN** the pure `ps`-output classifier functions
- **WHEN** the test suite runs on Linux
- **THEN** unit tests over literal `ps` fixture outputs (running, sleeping,
  zombie, mixed-state group, and unusable-snapshot rows) pin the
  alive/zombie classification, the group-survivor filter, the
  fallback-on-unusable behavior, and the diagnostic rendering — including
  a fixture reproducing the survivor line from CI Weekly run `42565698846`
