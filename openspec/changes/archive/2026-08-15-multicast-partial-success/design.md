# Design: multicast-partial-success

## Approach

Two surgical guards in `crates/camel-processor/src/multicast_segment.rs`,
mirroring the zero-success guard pattern already shipped in
`RecipientListService` (ADR-0058 migration notes):

1. `sequential_multicast` — after the branch loop, replace
   `if let Some(err) = last_error { return Failed(err) }` with a guard:
   `Failed(err)` only when `last_error.is_some() && outputs.is_empty()`;
   otherwise `Completed((seg.aggregator)(outputs))`. Before aggregating
   under partial success, emit one `warn!` with the count of branches whose
   `Failed` outcome is being discarded (derived: `total - outputs.len()` —
   `Stopped` returns early, so the subtraction counts exactly Failed
   branches; no accumulator variable).
2. `parallel_multicast` — in the `stop_on_exception=false` else-arm, collect
   `last_error`; keep the shared tail `filter_map` aggregation as the single
   aggregation point and guard after it with `completed.is_empty()` before
   returning `Failed`. The `stop_on_exception=true` arm (first-failed by
   lowest index) is untouched. One `warn!` on the partial-success path
   (failed count derived by filtering `results` for `Some(Failed)` slots —
   pre-start-gate-skipped branches whose slot is `None` are not counted).

Error identity rules are unchanged (spec: Parallel last-error determinism):
sequential zero-success = iteration-last error; parallel zero-success =
highest-branch-index error (LastWins) — results are already sorted by
branch index before selection.

Doc comments on `stop_on_exception` in the struct definition are updated:
they currently promise "the last error is propagated after all branches
complete" unconditionally; they must say the propagation applies to the
zero-success case, and partial success aggregates.

Spec delta (MODIFIED `segment-outcome-composition`): the multicast
requirement currently ends with an "Out of scope" block stating this
change SHALL update it. The delta rewrites the requirement to govern
partial success: with `stop_on_exception=false` and ≥1 `Completed` branch
and no `Stopped`, multicast SHALL return `Completed(aggregated over
successes only)`; zero-success SHALL return `Failed(last_error)`. Existing
scenarios (all-failed, Stopped-wins) are preserved verbatim; two new
scenarios added (sequential partial success, parallel partial success).
ADR-0058 `### Multicast outcome` updated per its own SHALL clause, same
for its Migration section sentence. camel-processor `CONTEXT.md` multicast
entry synced.

## Affected crates

- camel-processor: `multicast_segment.rs` (2 arms + doc comments), tests
  co-located in the same file's `mod tests`.
- No other crate. camel-tests integration coverage for multicast partial
  success is NOT extended (unit scenarios are exhaustive for outcome
  semantics; existing integration tests must stay green).

## Architecture boundaries

Runtime-only change inside the camel-processor Segment layer. No DSL
surface change (the `stop_on_exception` knob already exists and reaches
`MulticastSegment`), no component change, no camel-api change, no schema
regeneration (`schemas/` untouched). The `PipelineOutcome` contract
(camel-api) is consumed, not modified — `Completed`/`Failed`/`Stopped`
variants and ADR-0025 §7 stopped-wins precedence are respected as-is.

## Test strategy

Executable test specs (name/arrange/act/assert) live in tasks.md. New
co-located `#[tokio::test]` functions in `multicast_segment.rs::mod tests`,
reusing existing `always_failed_body`/`always_completed_body` helpers:

- `multicast_sequential_partial_success_aggregates_successes` — 3 branches
  [pass, fail-at-idx-1, pass], stop_on_exception=false → Completed over the
  2 successes, all 3 branches execute (invocations=3).
- `multicast_sequential_partial_success_two_branches` — exact delta-spec
  scenario: 2 branches [Completed, Failed] → Completed over the 1 success
  (count-aggregator body `n=1`).
- `multicast_parallel_partial_success_aggregates_successes` — 3 parallel
  branches [Completed, Failed, Completed] → Completed over branches 0 and 2.
- Existing tests stay green (two strengthened with identity assertions —
  error-message identity for iteration-last/LastWins selection, exchange
  identity for Stopped propagation): zero-success sequential
  (`multicast_all_branches_failed_no_stopped_returns_failed`) and parallel
  LastWins identity, stop_on_exception=true fail-fast, Stopped-wins
  (`multicast_stopped_branch_wins_over_failed`).

The warn log is best-effort observability, not asserted (no tracing-test
dependency in camel-processor; adding one is out of the risk budget). The
warn line carries the discarded-failure count for operator diagnosis.
