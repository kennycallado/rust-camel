# Design

## The two contracts

| change | statement | runtime |
|---|---|---|
| 614 | a timed-out queued exchange still executes after release | true before 622 |
| 622 | an abandoned wait cancels the exchange at its next step boundary | true after 622 |

Both were blessed; they cannot both hold. The owner's ruling: cancellation
wins, because a request the caller abandoned must not produce effects.

## What the code actually does (measured, not assumed)

- **Channel path** (`Concurrent` drain): the per-envelope task takes the
  dispatch token at `route_controller_trait.rs:495`/`:653` and `run_steps`
  checks it before each step. So a caller that gives up gets: the step it is
  parked in finishes, the following step never runs. A dispatch that never
  entered the pipeline never starts.
- **Inline path** (Sequential consumer, `dispatcher` available): the pipeline
  runs on the caller's own task (`inline_dispatcher.rs:123-206`), so the
  caller's timeout drops the future mid-`.await` instead of reaching a
  boundary. No marker is minted there, and this change does not add one.
- **Route stop / suspend** is unchanged: the consumer token is the trigger and
  ADR-0043 drain grace still owns the exchange.

The test run in this worktree confirms the first bullet: the parked step logs
`begin`/`end`, the `to: mock` step never executes (mock count 0), and the
queued exchange never logs `begin:p1`.

## Why this is the coherent rule

`timeout_ms` is the caller's statement that it stopped waiting. Both "the
queued one never starts" and "the admitted one stops at the next boundary"
follow from one rule: the dispatch dies at the next boundary after the caller
gives up. The inline deviation is documented rather than papered over, because
the inline path has no envelope to carry a token.

## Risks

- A `direct:` caller timeout is now destructive for the callee route: effects
  of the step already running stand, later steps do not run. That is the
  intended consequence and the reason the MCP knob can rely on it.
- The spec's channel/inline split must stay honest if the inline dispatcher
  ever gains a token carrier; the requirement says so explicitly.

## Verification

- `direct_timeout_still_counts_admission_wait` — queued dispatch never enters,
  admitted step finishes, nothing reaches `to: mock`, in-flight settles to 0.
- `abandoned_dispatch_stops_before_the_next_step` — same shape with one
  admitted dispatch.
