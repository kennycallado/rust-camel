# job Specification

## Purpose
TBD - created by archiving change jobteardown. Update Purpose after archive.
## Requirements
### Requirement: Post-boot job failures tear down the context

The `camel job` command SHALL invoke the existing bounded shutdown machinery before returning from every failure path that occurs after the boot handle is acquired, including route loading, consumer validation, target selection, route registration, and context start failures.

#### Scenario: Ambiguous target failure shuts down the booted context

- **GIVEN** a job boots its context and resolves more than one matching consumer target
- **WHEN** `job_early_failure_shutdown_is_observed` runs the duplicate-consumer fixture with `CAMEL_JOB_SHUTDOWN_MARKER=1`
- **THEN** stderr contains `camel job: shutdown complete` and `is ambiguous: 2 consumer routes share its base`, and the child exits with code 2

#### Scenario: Context start failure shuts down the booted context

- **GIVEN** a job has acquired a boot handle and context start returns an error
- **WHEN** job execution handles the start failure through the shared teardown border
- **THEN** the command performs bounded context shutdown before returning exit code 2

#### Scenario: Embedded jobs use the same teardown border

- **GIVEN** an embedded job invokes the shared executor without signal handles
- **WHEN** any post-boot validation or start failure occurs
- **THEN** the shared executor still performs bounded context shutdown and preserves the failure exit outcome

### Requirement: Existing job lifecycle contracts remain stable

The fix SHALL preserve existing diagnostics, exit codes, shutdown budgets, signal handling, transport handling, successful completion, and batch behavior.

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

