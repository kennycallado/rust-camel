## MODIFIED Requirements

### Requirement: Existing job lifecycle contracts remain stable

The fix SHALL preserve existing diagnostics, exit codes, shutdown budgets, signal handling, transport handling, successful completion, and batch behavior. The job boot projection's removal of inherited process-scoped diagnostic Lifecycles — durable runtime journal, OTel providers, Prometheus and health listeners — is the one sanctioned change to job boot composition: teardown borders, shutdown budgets, reports, exit codes, and signal behavior SHALL remain unchanged when those Lifecycles are absent.

#### Scenario: Early failure remains an exit-2 result

- **GIVEN** a deterministic post-boot early failure
- **WHEN** teardown completes
- **THEN** the command reports the existing diagnostic and exits with code 2

#### Scenario: Signal teardown remains unchanged

- **GIVEN** a running job receives its existing shutdown signal
- **WHEN** the signal path executes
- **THEN** it uses the existing signal teardown and force-exit behavior without regression

#### Scenario: Transport, success, and batch paths retain their shutdown behavior

- **GIVEN** the existing successful and batch job fixtures, plus a deterministic transport-failure fixture whose send returns `SendError::Transport`
- **WHEN** their current integration tests run
- **THEN** their existing shutdown budgets, diagnostics, reports, and exit outcomes remain unchanged, and the transport-failure path performs exactly one shutdown

#### Scenario: Diagnostic lifecycle removal preserves the teardown border

- **GIVEN** a job run against an ambient config enabling the runtime journal and all observability surfaces
- **WHEN** the job boots, runs, and tears down
- **THEN** it performs the existing bounded context shutdown, emits the existing report schema, and exits with the existing outcome codes for success, failure, timeout, and signal paths
