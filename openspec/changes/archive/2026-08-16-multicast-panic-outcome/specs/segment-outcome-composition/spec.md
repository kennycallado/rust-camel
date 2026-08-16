# Delta Spec: multicast-panic-outcome

Domain: segment-outcome-composition

## MODIFIED Requirements

### Requirement: multicast zero-success operational failure returns Failed

`multicast` (the parallel OutcomePipeline sibling of `recipient_list`) is
governed by two coordinated rules, reconciling it with `recipient_list`
(this requirement supersedes the earlier text that declared partial success
out of scope; bd rc-b41j):

1. **Zero-success operational failure.** When at least one branch executed,
   zero branches returned `Completed`, AND no branch returned `Stopped`,
   the multicast SHALL report `Failed(last_error)` (per the parallel
   last-error determinism requirement).
2. **Partial-success aggregation.** When `stop_on_exception=false`, at
   least one branch returned `Completed`, and no branch returned
   `Stopped`, the multicast SHALL report
   `Completed(aggregator applied to the successful branches' outputs only)`.
   The `Failed` branch outputs are discarded. This mirrors
   `recipient_list` partial-success semantics and Apache Camel
   `stopOnException=false` behavior. A warn-level log with the
   discarded-failure count is best-effort observability. Its emission is an
   implementation concern, not a normative clause of this requirement.

When at least one branch returns `Stopped`, multicast SHALL propagate
`Stopped(exchange)` as an intentional halt (ADR-0025 §3). This is NOT an
operational failure and SHALL NOT be reported as `Failed` or `Completed`.
With `stop_on_exception=true`, the existing fail-fast behavior is
unchanged: the first `Failed` branch (lowest branch index in parallel
mode) propagates.

**Panicked branch classification (bd rc-f88o).** A parallel-mode branch
whose sub-pipeline panics (including a panic that occurs inside the
per-branch timeout window before the timeout elapses) SHALL be treated as
a branch that executed and returned `Failed`: the panic SHALL be mapped to
a representative `CamelError::ProcessorError` identifying the panicked
branch, the branch SHALL count toward any discarded-failure accounting of
rule 2, its representative error SHALL be eligible as `last_error` under
the parallel last-error determinism requirement, and an all-panicked run
SHALL fire the zero-success guard of rule 1. A panicked branch SHALL NOT
be silently dropped from the outcome, the accounting, or the
representative-error selection. This mirrors the `recipient_list`
panic-handling contract: a panic is zero-success attempted work under
ADR-0058 and MUST NOT launder to success. Task cancellation (e.g. the
Stop path aborting in-flight branches) is NOT a panic and remains
excluded from failure accounting. These clauses apply to unwinding panics.
An aborting panic terminates the process and cannot produce a
`PipelineOutcome`.

#### Scenario: multicast sequential all-branches-failed returns Failed

- **GIVEN** a sequential `multicast` Segment with two branches whose
  sub-pipelines both return `Failed(error)` and no branch returns
  `Stopped` or `Completed`
- **WHEN** the multicast runs
- **THEN** the outcome is `Failed(error)` carrying the iteration-last error,
  and is NOT `Completed(original)` and NOT `Stopped(_)`

#### Scenario: multicast parallel all-branches-failed returns Failed

- **GIVEN** a parallel `multicast` Segment with two branches whose
  sub-pipelines both return `Failed(error)` and no branch returns
  `Stopped` or `Completed`
- **WHEN** the multicast runs
- **THEN** the outcome is `Failed(error)` carrying the highest-branch-index
  error (branch-index LastWins, per the parallel last-error determinism
  requirement), and is NOT `Completed(original)` and NOT `Stopped(_)`

#### Scenario: multicast with a Stopped branch propagates Stopped over Completed and Failed

- **GIVEN** a `multicast` Segment with branches where at least one returns
  `Stopped(ex_b)` (other branches may return `Completed` or `Failed`)
- **WHEN** the multicast runs
- **THEN** the outcome is `Stopped(ex_b)` (intentional halt wins per ADR-0025
  §3), and is NOT `Failed(_)` and NOT `Completed(_)`

#### Scenario: multicast sequential partial success aggregates successful branches only

- **GIVEN** a sequential `multicast` Segment with `stop_on_exception=false`
  and two branches where branch 0 returns `Completed(ex_a)` and branch 1
  returns `Failed(error)`
- **WHEN** the multicast runs
- **THEN** the outcome is `Completed(aggregator([ex_a]))` built from the
  successful branch only, and is NOT `Failed(error)`

#### Scenario: multicast parallel partial success aggregates successful branches only

- **GIVEN** a parallel `multicast` Segment with `stop_on_exception=false`
  and three branches where branches 0 and 2 return `Completed` and branch 1
  returns `Failed(error)`
- **WHEN** the multicast runs
- **THEN** the outcome is `Completed(aggregator([ex_0, ex_2]))` built from the
  successful branches only (branch-index order), and is NOT
  `Failed(error)`

#### Scenario: multicast parallel panicked branch counts as a failed branch in partial success

- **GIVEN** a parallel `multicast` Segment with `stop_on_exception=false`
  and three branches where branch 0 returns `Completed(ex_a)`, branch 1
  returns `Failed(error)`, and branch 2's sub-pipeline panics
- **WHEN** the multicast runs
- **THEN** the outcome is `Completed(aggregator([ex_a]))` built from the
  successful branch only, both non-successful branches are accounted as
  discarded failures (failed-branch count = 2), and the outcome is NOT
  `Failed(_)`

#### Scenario: multicast parallel all-branches-panicked returns Failed

- **GIVEN** a parallel `multicast` Segment with two branches whose
  sub-pipelines both panic, and no branch returns `Stopped` or `Completed`
- **WHEN** the multicast runs
- **THEN** the outcome is `Failed` carrying the representative
  `ProcessorError` of the highest-branch-index panicked branch
  (branch-index LastWins), and is NOT `Completed(_)` (ADR-0058: a panic
  is zero-success attempted work and MUST NOT launder to success) and NOT
  `Stopped(_)`

#### Scenario: multicast parallel mixed failed and panicked branches report full failure count

- **GIVEN** a parallel `multicast` Segment with `stop_on_exception=false`
  and two branches where branch 0 returns `Failed(error_a)` and branch 1's
  sub-pipeline panics
- **WHEN** the multicast runs
- **THEN** the outcome is `Failed` whose error is branch 1's representative
  panic `ProcessorError` (highest branch index)

#### Scenario: multicast Stopped branch wins over a panicked branch

- **GIVEN** a parallel `multicast` Segment where branch 0 returns
  `Stopped(ex_b)` and branch 1's sub-pipeline panics
- **WHEN** the multicast runs
- **THEN** the outcome is `Stopped(ex_b)` (intentional halt wins per
  ADR-0025 §3), and is NOT `Failed(_)` and NOT `Completed(_)`

#### Scenario: multicast branch panicking inside the timeout window maps to Failed

- **GIVEN** a parallel `multicast` Segment with a per-branch timeout
  configured and a single branch whose sub-pipeline panics before the
  timeout elapses
- **WHEN** the multicast runs
- **THEN** the outcome is `Failed` carrying the representative panic
  `ProcessorError` for that branch, and is NOT `Completed(_)` and NOT a
  silently dropped result
