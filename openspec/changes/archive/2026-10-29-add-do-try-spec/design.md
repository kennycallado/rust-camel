# Design: add-do-try-spec

## Approach

Record the corrected doTry semantics as a new `do-try` capability spec.
The delta adds two requirements. Requirement 1 fixes the finally
body's run count: exactly once per scope exit while the finally clause
is enabled; a false `on_when` predicate skips the body and leaves the
pending outcome standing, and a Stop from the try or catch body skips
it entirely. Requirement 2 pins the catch-body-failure rule introduced
by rc-h3i8d: an enabled finally body runs, the catch error surfaces,
the catch chain is not re-entered, and the existing
`fail_after_finally` parity rules (throwing finally restores the
previous error, a failed finally `on_when` predicate supersedes, Stop
in finally stops the outer route) hold unchanged.

Each scenario names behavior that an existing test already pins, so the
spec and the test suite stay in lockstep:

- Try body succeeds — `happy_path_try_succeeds_finally_runs`
- No catch matches — `no_clause_matches_runs_finally_and_propagates_original`
- Handled catch — `do_try_finally_runs_after_handled_catch` (e2e) and
  `catch_by_variant_handled_returns_ok`
- Catch body fails — `catch_body_failure_runs_finally_once_then_returns_catch_err`
- Finally body also fails — `catch_throws_and_throwing_finally_still_returns_catch_err`
- Stop in finally after catch-body failure —
  `catch_body_failure_then_finally_stops_stops_outer_route`

## Affected crates

- camel-processor: none in this change. `DoTrySegment` already
  implements the specified behavior (commit 757ccf2a6).

## Architecture boundaries

The spec documents the Runtime layer only. `DoTrySegment` implements
`OutcomePipeline` and propagates `PipelineOutcome` per ADR-0025. The
DSL, builder, and components keep their existing compilation to
`DoTrySegment`; no boundary is crossed or moved.

## Alternatives considered

- Extend an existing capability: no current spec covers doTry or
  structural EIP error handling, so there is nothing to extend.
- Keep the rule in prose docs only: prose already drifted from the
  runtime once (the skip). A normative spec with validate --strict
  closes that gap.
- Specify catch-matching and dispositions too: out of scope for
  rc-h3i8d. The delta stays on the finally semantics.
