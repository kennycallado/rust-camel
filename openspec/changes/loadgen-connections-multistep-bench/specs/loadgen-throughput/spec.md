# loadgen-throughput — loadgen-connections-multistep-bench

## ADDED Requirements

### Requirement: Connection concurrency knob

`loadgen measure-throughput` SHALL accept `--connections N` controlling the number of concurrent in-flight request tasks, independent of `--workers` (tokio worker-thread count). When the flag is absent, connections SHALL default to the workers value, reproducing current behavior.

#### Scenario: Explicit connection count drives in-flight concurrency

- **WHEN** `measure-throughput --url U --connections 300 --workers 4` runs against a counting endpoint
- **THEN** the number of concurrently in-flight requests observed by the endpoint reaches ~300 while the client process uses 4 worker threads

#### Scenario: Default preserves legacy behavior

- **WHEN** `measure-throughput --url U` runs without `--connections`
- **THEN** in-flight concurrency and runtime sizing match the pre-change behavior (connections = workers)

### Requirement: Concurrency profile in results artifacts

Published result JSON SHALL include flat `workers` and `connections` fields recording the measurement's concurrency profile.

#### Scenario: Results embed the profile

- **WHEN** a measure-throughput run completes with `--workers 4 --connections 300` and writes its JSON artifact
- **THEN** the artifact contains `"workers": 4` and `"connections": 300` as top-level greppable fields

### Requirement: Multi-step benchmark scenario

A scenario fixture under `benchmarks/scenarios/multi-step/` SHALL provide an HTTP route whose per-exchange pipeline executes at least three distinct in-DSL step kinds (script execution, header derivation, branch evaluation) without external infrastructure. Semantic correctness SHALL be asserted by a preflight request (deterministic body + headers flowing through every intended step) before the load phase; the sustained load phase SHALL NOT emit per-exchange logs.

#### Scenario: Preflight proves the pipeline is not a no-op

- **WHEN** the fixture route starts and receives the preflight request
- **THEN** the response body and headers match the deterministic values that only complete execution of all steps can produce

#### Scenario: Multi-step route serves concurrent load end-to-end

- **WHEN** the multi-step fixture route runs under a locally built camel-cli and loadgen drives it at c16, c300, and c1000
- **THEN** all runs complete with error rate below 1%, per-second throughput buckets are non-degenerate at c1000 (no zero-bucket collapse), and each run's JSON artifact records its concurrency profile

#### Scenario: Convoy signature is machine-computable

- **WHEN** the c16, c300, and c1000 result artifacts of a multi-step comparison are inspected
- **THEN** a reader can compute the c300/c16 and c1000/c16 throughput ratios from greppable fields alone (`connections`, `workers`, per-second buckets) without rerunning the harness — a future serialization regression manifests as a collapsing ratio visible in artifacts. Threshold calibration for automated pass/fail gating is out of scope for this change
