# Proposal: dispatch-cancellation

## Why

`ConsumerContext::send_and_wait` pushes an `ExchangeEnvelope` onto the route
channel and awaits a one-shot reply. If the dispatcher gives up (its wait future
is dropped before the reply), only the reply receiver is released. The pipeline
future belongs to camel-core's per-envelope task, and `run_steps` observes only
the route-stop `CANCEL_TOKEN` between steps. The exchange therefore keeps
running after the client has gone. Mission 616's spec-bless REJECT recorded the
gap; `rc-sqpqn` (MCP dispatch timeout) cannot cancel the route today.

## What Changes

- A per-dispatch `CancellationToken` is stamped on the `Exchange` as an
  **enqueue-only marker** under the well-known key `CamelDispatchCancel`, with
  an additive RAII helper (`Exchange::begin_dispatch_cancel` →
  `DispatchCancellation`) in `camel-api` (the `CamelOriginalMessage` pattern).
  No public struct field is added.
- `send_and_wait` begins a **fresh, independent** dispatch cancellation and
  arms its guard: dropping the wait future cancels that one dispatch; a normal
  reply disarms it. A drop driven by route stop (the consumer-lifetime token is
  already cancelled) is defused, so the ADR-0043 drain grace still owns the
  exchange. The token is not derived from or linked to any other token.
- The pipeline **takes** (removes) the marker at entry, before running any
  step. Replies therefore carry no marker, and an exchange that is redistributed
  later cannot inherit an old cancellation. camel-core scopes the captured
  token as a second task-local around the pipeline call; `run_steps` checks it
  before each step; a dispatch-cancelled `ConsumerStopping` is not retried or
  absorbed by the error handler (the retry loop reads a defaulted
  `RetryableStep::dispatch_cancelled` hook in `camel-api`).
  `TransactedSegment` races the same signal in its existing body race and
  finalizes through the existing bounded rollback path, honouring the frozen
  commit cutoff.
- Route stop (`CANCEL_TOKEN`, ADR-0043) is untouched and remains an independent
  trigger with its drain-grace semantics.

Explicitly excluded: a new field on `ExchangeEnvelope` (a public struct-literal
break in a published crate), mid-step abortion of non-transactional steps, a new
`CamelError` variant, a `sql-transactions` delta (the existing cancellation
cutoff requirement already governs the block), and the MCP timeout knob itself
(`rc-sqpqn`).

## Acceptance criteria

- The six named tests pass: `cancelled_dispatch_stops_before_the_next_step`,
  `cancelled_dispatch_releases_the_exchange`,
  `route_stop_still_cancels_in_flight_dispatch`,
  `cancelled_transacted_block_finalizes_without_leaking_a_session`,
  `uncancelled_dispatch_is_unchanged`,
  `abandoned_wait_during_route_stop_drains_to_completion`.
- Route-stop cancellation through the envelope path behaves exactly as before.
- `camel-api` and `camel-component-api` gain additive surface only; no existing
  public item changes shape. `scripts/xtask/ratchet-cancel-tokens.max` is not
  raised (the fresh token lives outside the component scan scope).
- The AGENTS.md gate registry passes (conductor deviations per mission order).

## Risk budget

Accepted: a cancelled dispatch lets the in-flight non-transactional step finish
(one step boundary, ADR-0043 granularity), and each `send_and_wait` pays one
token allocation plus one extension insert/remove pair. A dispatch cancelled
while its last step is already running completes with that step's outcome (no
successor boundary). Out of bounds: aborting a non-transactional step
mid-`.await`, weakening the drain grace on route stop, and exposing pipeline
internals to components.
