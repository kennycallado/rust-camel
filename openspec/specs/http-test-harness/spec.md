# http-test-harness Specification

## Purpose
TBD - created by archiving change httpflake. Update Purpose after archive.
## Requirements
### Requirement: Registry-poll readiness with bounded budget

The consumer-test readiness helper SHALL determine readiness by polling
`ServerRegistry::bound_addr` for the staged port — never by TCP probes —
with a fail-loud wall-clock deadline of 10 seconds and a retry backoff that
doubles from 1 ms, capped at 64 ms. A deadline miss SHALL panic with a
message naming the port and a cause hint naming the two known classes
(concurrent registry reset, scheduler starvation).

#### Scenario: staged-consumer-becomes-ready

- **GIVEN** a listener bound to `127.0.0.1:0` is staged and a consumer
  `start()` is spawned
- **WHEN** the readiness helper polls the registry
- **THEN** the poll observes the bound entry and returns before the
  deadline, without sending any TCP probe

#### Scenario: deadline-miss-is-loud-and-diagnosable

- **GIVEN** no registry entry ever appears for the staged port
- **WHEN** the poll loop runs past the 10-second deadline
- **THEN** the helper panics with a message that names the port and hints
  "registry entry absent (concurrent reset or starvation)"

### Requirement: Registry-mutation serialization during setup

The consumer-test readiness helper SHALL hold `REGISTRY_TEST_MUTEX` from
`stage_listener` until readiness-complete (including the tail-yield loop),
so a test-concurrent `ServerRegistry::reset()` that itself follows the
mutex law cannot clear the staged listener or the spawned entry inside the
setup window. The guard SHALL be released before the helper returns.

#### Scenario: concurrent-legal-reset-survival

- **GIVEN** a hammer thread loops legal resets — try-lock the
  `REGISTRY_TEST_MUTEX` (counting a contention when it blocks), then
  `ServerRegistry::reset()` — under a stop flag
- **WHEN** the readiness helper performs setups on fresh ephemeral ports,
  always at least 25, continuing past 25 only until one contended reset is
  observed, with a hard cap of 50 total setups
- **THEN** the test asserts at least one contended reset occurred, every
  setup that ran reports ready within its deadline with no readiness
  panic, and the hammer thread is stopped and joined by a Drop guard even
  if the test panics

<!-- Amendment 2026-09-14 (conductor, r_glm task-1.3-review minor): the
deadline scenario's quoted hint string was reconciled to the parenthetical
form actually implemented and specified verbatim in blessed tasks.md
("registry entry absent (concurrent reset or starvation)"). Quote-only
reconciliation; requirement semantics unchanged. -->

