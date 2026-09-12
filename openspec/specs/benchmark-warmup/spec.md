# benchmark-warmup Specification

## Purpose
TBD - created by archiving change benchwarmup. Update Purpose after archive.
## Requirements
### Requirement: Protocol A warmup uses a trailing stability window

The benchmark harness SHALL collect Protocol A warmup samples until the configured wall-clock bound, then evaluate stability against the latest `max_messages` samples. Reaching `max_messages` SHALL NOT terminate warmup or force a `MessageBoundUnconverged` result. `MessageBoundUnconverged` SHALL remain available for compatibility but SHALL NOT be emitted by Protocol A.

#### Scenario: Late convergence replaces early drift

- **GIVEN** a 30-second warmup with a 1,000-sample comparison window, and the first 1,000 samples have p50 halves outside tolerance
- **WHEN** the warmup reaches its wall-clock bound after later samples form a trailing 1,000-sample window whose p50 halves are within tolerance
- **THEN** the harness returns `Stable` using the trailing window

#### Scenario: Stable trailing window at time bound

- **GIVEN** the warmup reaches its configured wall-clock bound with at least one complete trailing comparison window
- **WHEN** the trailing window halves are within tolerance
- **THEN** the harness returns `Stable` and reports the actual elapsed time

#### Scenario: Unstable trailing window at time bound

- **GIVEN** the warmup reaches its configured wall-clock bound with a complete trailing comparison window
- **WHEN** the trailing window halves remain outside tolerance
- **THEN** the harness returns `FailedStability` with reason `TimeBoundUnconverged`

#### Scenario: Insufficient trailing samples at time bound

- **GIVEN** the warmup reaches its configured wall-clock bound before collecting one complete comparison window
- **WHEN** stability is evaluated
- **THEN** the harness returns `FailedStability` with reason `InsufficientSamples`

### Requirement: Warmup documentation states the future-run protocol

Benchmark harness documentation SHALL state that Protocol A uses a trailing sample window and a wall-clock termination bound, and SHALL state that sealed records are not rewritten.

#### Scenario: Operator reads warmup policy

- **GIVEN** an operator consults the benchmark harness notes
- **WHEN** they look up Protocol A warmup
- **THEN** they find the trailing-window rule, timeout behavior, and sealed record policy

### Requirement: Warmup enforces its wall-clock deadline

The benchmark harness SHALL enforce the Protocol A wall-clock deadline before
starting a request and while awaiting an in-flight response and body drain.

#### Scenario: In-flight request cannot extend warmup indefinitely

- **GIVEN** a warmup request remains in flight when the wall-clock deadline is reached
- **WHEN** the deadline guard runs
- **THEN** the request is abandoned for warmup evaluation and no later request is started

