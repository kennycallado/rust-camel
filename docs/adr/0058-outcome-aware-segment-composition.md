# ADR-0058: Outcome-aware Segment Composition Contract

**Date:** 2026-08-13
**Status:** Accepted
**References:** ADR-0024, ADR-0025
**Bd:** rc-65fs (epic); ADR number reserved rc-zfov.

## Context

A demo surfaced three defects that share one missing contract. Outcome-aware
Segments (EIPs that implement `OutcomePipeline` from ADR-0025, or that reach the
outcome layer through the Tower `Result<Exchange, CamelError>` adapter) had no
stated rule for what they may report as `Completed` after their work failed.

The visible defects:

- rc-20yn: `recipient_list` returned `Ok(original)` when every recipient failed.
  The adapter wrapped that into `Completed(original)`. The `cache:` Segment then
  wrote the inbound body back under the key for the full time-to-live. The
  corruption was silent on `timer:` routes.
- rc-n8rc: `Body::Stream` was consumed twice on the error path (the HTTP
  symptom-masker of rc-20yn).
- rc-65yi: the body was lost when `cache_peek_stale` ran inside a `do_try` catch
  that shared a key with a `cache:` step.

ADR-0025 defined the `PipelineOutcome` type (`Completed`, `Stopped`, `Failed`).
It did not state the body-propagation contract for a Segment whose work produced
zero successes. This ADR pins that contract.

## Decision

### The invariant

When a Segment's attempted work results in zero successes (an operational
failure), the Segment SHALL report `Failed(error)`. It MAY report
`Stopped(exchange)` only when the zero-success outcome is an intentional halt
governed by the Stop EIP (ADR-0025 section 3). It SHALL NOT report `Completed`.

The invariant is outcome-based. It is not body-equality-based. A zero-success
Segment may not return `Completed` even when its body differs from the inbound
body.

A Segment that attempted no work is not an operational failure. It MAY report
`Completed(original)`. Example: a `recipient_list` whose expression resolves to
an empty list attempts no recipient call.

### Per-Segment definitions

Each governed Segment defines "attempted work", "success", "operational
failure", and "intentional halt" as follows.

- `recipient_list` (Tower `Service<Exchange>`, reaches the invariant through the
  `Result` to `PipelineOutcome` adapter): attempt = one recipient endpoint call.
  Success = the call returned `Ok`. Operational failure = at least one call was
  attempted and zero calls returned `Ok`. Intentional halt = none
  (`recipient_list` does not produce `Stopped`).
- `multicast` (OutcomePipeline Segment, parallel sibling of `recipient_list`):
  attempt = one branch sub-pipeline run. Success = the branch returned
  `Completed`. Operational failure = at least one branch ran and zero branches
  returned `Completed`. Intentional halt = a branch returned `Stopped` and that
  halt propagates.
- `cache` (OutcomePipeline Segment; propagator, not generator): attempt = the
  `on_miss` sub-pipeline run on a cache MISS. Success = `on_miss` returned
  `Completed`. A `Stopped` or `Failed` propagated from `on_miss` is not the
  cache's operational failure. The cache propagates it with no write-back. The
  cache's own operational failure is a repository error surfaced as `Failed`
  (Contract C1 from ADR-0023). Intentional halt = a `Stopped` propagated from
  `on_miss`.
- `do_try` (OutcomePipeline Segment): attempt = `try_body` run. Success =
  `try_body` returned `Completed`, or `try_body` returned `Failed` and a matching
  catch clause ran and returned `Completed`. Operational failure = `try_body`
  returned `Failed` and no catch matched, or every matching catch re-propagated.
  Intentional halt = a `Stopped` from `try_body` (propagated; skips catch and
  finally per ADR-0025 section 5.1), or a `Stopped` from a catch body.

### Cache write-back trust rule

The `cache:` Segment SHALL write back a body only when `on_miss` reports
`Completed`. It SHALL skip write-back when `on_miss` reports `Stopped` or
`Failed`. This is already true at `crates/camel-processor/src/cache_eip.rs` step
3 (the `on_miss` outcome match returns `Stopped`/`Failed` before any
`repository.set` call). The rule is the downstream guarantor of the invariant
for the cache-warming pattern. See the `eip-cache` spec capability.

### Last-error determinism

For `recipient_list` and `multicast`, the `last_error` carried by a zero-success
`Failed` outcome is determined as follows.

- Sequential arm: the last error encountered in iteration order.
- Parallel arm (`recipient_list`): the error from the task returned
  by the last `JoinSet::join_next().await` call that completed with
  an error (completion order).
- Parallel arm (`multicast`): the error from the highest-index
  branch that returned `Failed` (branch-index order — results are
  sorted by branch index before the representative error is
  selected). This is the legacy LastWins semantics.

The parallel-arm error is a representative error in both siblings. It is not
the causally-first error. The two siblings use different selection orders
(completion vs. branch-index), but both yield a representative, not a
causal, error. A test may assert a specific error identity only when it
controls the selection order through a synchronization primitive
(`recipient_list`) or fixed branch arrangement (`multicast`).

### Multicast outcome

`multicast` is governed by the zero-success invariant and the Stopped-wins rule.
When at least one branch returns `Stopped`, `multicast` propagates `Stopped`
(ADR-0025 section 3).

With `stop_on_exception=false`, zero success reports `Failed(last_error)`.
The error is the highest-branch-index representative in the parallel arm.
It is the iteration-last error in the sequential arm. Partial success
aggregates successful branch outputs only. `multicast` reports `Completed`
with the aggregated outputs. Discarded failures are logged at warn. This
resolves the `recipient_list` inconsistency tracked as bd rc-b41j.

### Governed Segments

This ADR is the compliance authority for: `recipient_list`, `multicast`,
`cache`, `do_try`, `split`, `streaming-split`, `load_balance`.

The normative test scenarios in the `segment-outcome-composition` spec
capability cover the first four (delivered by the `outcome-aware-segment-composition` change). The remaining three (`split`, `streaming-split`,
`load_balance`) inherit compliance from this ADR. They are verified separately.

## Migration / existing-code alignment

`multicast` already complies with the zero-success invariant. Verified in
`crates/camel-processor/src/multicast_segment.rs`: `sequential_multicast` and
`parallel_multicast` track `last_error`, return `Stopped` on the first
`Stopped` branch, and return `Failed(last_error)` when the success set is empty
and `last_error` is set. Both arms enforce the partial-success guard per bd
rc-b41j. The guard checks `outputs.is_empty()` in `sequential_multicast` and
`completed.is_empty()` in `parallel_multicast`. Partial success aggregates the
successful branches and logs discarded failures at warn.

`recipient_list` is the non-compliant Segment. It is corrected in
`outcome-aware-segment-composition` Task 2.1, with this ADR as authority. The
fix changes the zero-success path of `RecipientListService::call` to return
`Err(last_error)` instead of `Ok(original)`. The existing `Result` to
`PipelineOutcome` adapter then yields `Failed`, and `cache:` skips write-back.

`stop_on_exception` defaults to `false` in `RecipientListConfig`. This matches
Apache Camel. The default is unchanged.

## Consequences

- A zero-success `recipient_list` inside a `cache:` on_miss no longer poisons the
  cache. The cache retains the previously seeded stale entry.
- `do_try` catches keyed on `Failed` receive the error. They do not receive a
  laundered `Completed(original)`.
- A Segment author can determine, from this ADR alone, whether a new Segment
  implementation is compliant.

## Alternatives considered

- Report `Stopped(original)` on zero-success. Rejected. `Stopped` is reserved for
  the intentional Stop EIP (ADR-0025 section 3). Using it for an operational
  error would hide the failure from `do_try` catches keyed on `Failed`.
- Change `stop_on_exception` default to `true`. Rejected. It would break Apache
  Camel parity. Partial-multicast routes legitimately continue past a failed
  branch.
- Body-equality invariant ("a Segment must not report `Completed` with an
  unchanged body when its work failed"). Rejected. It would reject legitimate
  no-op Segments that return `Completed(original)` after no attempted work. The
  outcome-based rule keys on "attempted work produced zero successes".

## Glossary

- Operational failure: a Segment attempted one or more units of work and zero
  succeeded.
- Intentional halt: a `Stopped` governed by the Stop EIP (ADR-0025 section 3).
- Representative error: the parallel-arm `last_error`. For `recipient_list`,
  selection is by `JoinSet::join_next` completion order; for `multicast`, by
  highest branch index (legacy LastWins). Neither is the causally-first error.
