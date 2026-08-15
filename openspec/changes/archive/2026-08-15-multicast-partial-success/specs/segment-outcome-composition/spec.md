# segment-outcome-composition Delta

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
   `Completed(aggregator applied to the successful branches' outputs only)`
   — the `Failed` branch outputs are discarded. This mirrors
   `recipient_list` partial-success semantics and Apache Camel
   `stopOnException=false` behavior. (Best-effort observability — a single
   warn-level log with the discarded-failure count — is an implementation
   concern, not a normative clause of this requirement.)

When at least one branch returns `Stopped`, multicast SHALL propagate
`Stopped(exchange)` as an intentional halt (ADR-0025 §3) — this is NOT an
operational failure and SHALL NOT be reported as `Failed` or `Completed`.
With `stop_on_exception=true`, the existing fail-fast behavior is
unchanged: the first `Failed` branch (lowest branch index in parallel
mode) propagates.

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
- **THEN** the outcome is `Completed(aggregator([ex_0, ex_2]))` built from
  the successful branches only (branch-index order), and is NOT
  `Failed(error)`
