# Proposal: multicast-partial-success

## Why

`MulticastSegment` (crates/camel-processor/src/multicast_segment.rs) reports
`Failed(last_error)` when ANY branch fails, even when other branches
succeeded. `RecipientListService` — the Tower sibling — aggregates successful
results on partial success and returns `Ok(aggregated)`. Apache Camel
multicast with `stopOnException=false` also aggregates successful responses
on partial failure. rust-camel multicast is the outlier: a route with
branches `[Completed(a), Failed(e)]` yields `Failed(e)` and discards `a`.

The zero-success invariant (ADR-0058) is unaffected: multicast remains
compliant (empty outputs + last_error still yields `Failed`). The
partial-success aggregation policy was deliberately deferred out of
rc-65fs to avoid scope creep; this change reconciles the two siblings.
bd: rc-b41j.

## What Changes

- `sequential_multicast`: with `stop_on_exception=false`, guard the
  `last_error` return with `outputs.is_empty()`. Partial success returns
  `Completed((aggregator)(outputs))`; zero-success keeps returning
  `Failed(last_error)` (iteration-last error, unchanged).
- `parallel_multicast`: same guard in the `stop_on_exception=false` arm.
  Partial success aggregates `Completed` outcomes only; zero-success keeps
  returning `Failed(last_error)` (highest-branch-index error, LastWins,
  unchanged).
- `stop_on_exception=true` behavior unchanged (fail-fast / first-failed
  propagation after in-flight completion).
- `Stopped` propagation unchanged (ADR-0025 §7 stopped-wins).
- One best-effort `warn!` per arm when branch failures are discarded during
  partial-success aggregation (observability parity with rc-uow1 concerns;
  lint-log-levels compliant; non-normative — not spec-asserted).
- Spec delta: MODIFIED `segment-outcome-composition` — the multicast
  requirement that declared partial success out of scope now governs it
  (the requirement text itself mandates this update).
- ADR-0058 `### Multicast outcome` section updated per its own SHALL clause.
- camel-processor CONTEXT.md synced.

**Excluded:** DSL surface (no new knob — `stop_on_exception` already exists),
legacy `multicast.rs` aggregation strategies, `recipient_list` code,
`split`/`load_balance` segments, error-body surfacing on partial success
(beyond the warn log).

## Acceptance criteria

- Sequential, `stop_on_exception=false`, branches `[Completed, Failed]`:
  outcome is `Completed(aggregated-over-successes-only)`.
- Parallel, `stop_on_exception=false`, mixed `[Completed, Failed, Completed]`:
  outcome is `Completed(aggregated-over-successes-only)`, representative
  selection order for aggregation is branch-index (existing sorted results).
- Zero-success (all `Failed`, no `Stopped`): outcome stays `Failed(last_error)`
  — sequential: iteration-last; parallel: highest-branch-index (LastWins).
- `stop_on_exception=true`: existing fail-fast tests stay green unchanged.
- Stopped-wins tests stay green unchanged.
- Existing spec scenarios for multicast zero-success/Stopped preserved
  (all-failed scenario split into sequential/parallel variants for error
  -identity precision).

## Risk budget

Behavior change visible to routes that relied on any-failure→Failed with
`stop_on_exception=false`. Accepted: that behavior contradicts the sibling
and Apache Camel parity. In-bounds: the two arms of `multicast_segment.rs`,
their tests, spec delta, ADR-0058 section, CONTEXT.md sync. Out of bounds:
new public API, DSL knob, aggregator signature change, recipient_list
refactor.
