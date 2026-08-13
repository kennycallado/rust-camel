# segment-outcome-composition

## Purpose

Pin the outcome-propagation invariant that outcome-aware Segments (EIPs
implementing `OutcomePipeline`, ADR-0025, or reaching the OutcomePipeline layer
via the Tower `Result → PipelineOutcome` adapter) must satisfy when their
attempted work produces zero successes. This capability is the spec expression
of ADR-0058. The invariant applies to all Segments enumerated in ADR-0058
(`recipient_list`, `multicast`, `cache`, `do_try`, `split`, `streaming-split`,
`load_balance`); the normative scenarios below cover the four Segments this
change delivers and repairs (`recipient_list`, `multicast`, `cache`, `do_try`).
Compliance of the remaining Segments is established by ADR-0058 and verified
separately.

## ADDED Requirements

### Requirement: Outcome-based composition invariant — zero successes yields Failed or Stopped, never Completed

When a Segment's attempted work results in **zero successes** (an operational
failure), the Segment SHALL report `PipelineOutcome::Failed(error)`, or
`PipelineOutcome::Stopped(exchange)` ONLY when the zero-success outcome is an
intentional halt governed by the Stop EIP (ADR-0025 §3). It SHALL NOT report
`PipelineOutcome::Completed`. The invariant is stated in terms of the reported
outcome, NOT body equality: a zero-success Segment may not return `Completed`
even if its body differs from the inbound body. A Segment that attempted NO work
(empty recipient list, no sub-pipeline execution, a no-op passthrough) is NOT an
operational failure and MAY report `Completed(original)`.

#### Scenario: zero-success operational failure is not reported as Completed

- **GIVEN** any governed Segment that attempted one or more units of work and
  observed zero successes
- **WHEN** the Segment produces its outcome
- **THEN** the outcome is `Failed(error)` or `Stopped(exchange)` (intentional
  halt only), and is NOT `Completed(_)`

### Requirement: Per-Segment definitions of attempted work and success

Each governed Segment SHALL define "attempted work", "success", "operational
failure", and "intentional halt" as follows, and these definitions SHALL be
applied when classifying an outcome under the outcome-based composition
invariant:

- **`recipient_list`** (Tower `Service<Exchange>`, reaches the invariant via the
  `Result → PipelineOutcome` adapter): attempt = one recipient endpoint call;
  success = the call returned `Ok(exchange)`; operational failure = at least one
  call was attempted AND zero calls returned `Ok`; intentional halt = none
  (recipient_list does not produce `Stopped`).
- **`multicast`** (OutcomePipeline Segment, parallel sibling of recipient_list):
  attempt = one branch sub-pipeline execution; success = the branch returned
  `Completed`; operational failure = at least one branch executed AND zero
  branches returned `Completed`; intentional halt = a branch returned `Stopped`
  and that halt is being propagated.
- **`cache`** (OutcomePipeline Segment, propagator not generator): attempt =
  the `on_miss` sub-pipeline execution on a MISS; success = `on_miss` returned
  `Completed`; a propagated `Stopped(exchange)` or `Failed(error)` from `on_miss`
  is NOT cache's operational failure — cache propagates it as-is with no
  write-back. Cache's own operational failure is limited to repository errors
  surfaced as `Failed` (Contract C1). Intentional halt = a `Stopped` propagated
  from `on_miss`.
- **`do_try`** (OutcomePipeline Segment): attempt = `try_body` execution;
  success = `try_body` returned `Completed`, OR `try_body` returned `Failed` and
  a matching catch clause ran and returned `Completed` (the failure was handled);
  operational failure = `try_body` returned `Failed` AND no catch matched or
  every matching catch re-propagated; intentional halt = `Stopped` from
  `try_body` (propagated, skipping catch and finally per ADR-0025 §5.1) or from
  a catch body.

#### Scenario: cache propagating on_miss Stopped is not cache's operational failure

- **GIVEN** a `cache:` Segment whose `on_miss` returns `Stopped(exchange)` (e.g.
  an inner Stop EIP in the on_miss sub-pipeline)
- **WHEN** the cache runs on a MISS
- **THEN** the cache returns `Stopped(exchange)` with no write-back, and this is
  an intentional-halt propagation, NOT an operational failure of cache

### Requirement: Canonical zero-success outcome is Failed(last_error)

For a zero-success operational failure, the canonical reported outcome SHALL be
`Failed(last_error)`. `Stopped(exchange)` SHALL be used only for an intentional
halt (Stop EIP, ADR-0025 §3). This preserves `do_try` catch matching keyed on
`Failed` and keeps operational error semantics visible to observers and handlers.

#### Scenario: recipient_list all-failed surfaces as Failed through the cache adapter

- **GIVEN** a `cache:` Segment whose `on_miss` is a `recipient_list` that
  all-fails, wrapped through the `Result → PipelineOutcome` adapter
- **WHEN** the on_miss runs on a MISS
- **THEN** the on_miss outcome is `Failed(CamelError)` (the adapter maps
  `Err(e)` to `Failed(e)`), NOT `Completed(original)` and NOT `Stopped(original)`

### Requirement: Parallel last-error determinism (representative error)

For `recipient_list` in parallel mode, the "last error" carried by the
zero-success `Failed` outcome SHALL be the error from the task returned by the
last `JoinSet::join_next().await` call that completed with an error (completion
order). For `multicast` in parallel mode, the representative error SHALL be
the error from the highest-index branch that returned `Failed` (branch-index
order — results are sorted by branch index before selection). This is the
legacy LastWins semantics.

Both selection orders yield a "representative error, not causally-first error"
and SHALL be documented as such in ADR-0058. A test asserting the zero-success
path SHALL assert that a `Failed` is returned; it MAY assert a specific error
identity only when the test controls the selection order — completion order via
an explicit synchronization primitive (barrier/oneshot channel) for
`recipient_list`, or fixed branch arrangement for `multicast`.

#### Scenario: parallel zero-success returns a Failed with synchronization-controlled error identity

- **GIVEN** a parallel `RecipientListService` with two recipients A and B that
  each return distinct errors `Err(A)` and `Err(B)`, and a synchronization
  primitive that forces recipient B's task to complete-after-fail last
- **WHEN** `call(exchange)` is invoked
- **THEN** the result is `Err(B)` (the error from the last failing task to
  complete), demonstrating the JoinSet `join_next` order policy

### Requirement: recipient_list zero-success returns Err, not Ok(original)

In `RecipientListService::call`, for both sequential and parallel arms, when at
least one recipient endpoint call was attempted and zero returned `Ok`, the
result SHALL be `Err(last_error)`. Partial success (at least one `Ok`) SHALL
aggregate over successes only and return `Ok(aggregated)`. An empty
resolved-recipient list (the expression yields `""` or all-empty tokens) SHALL
return `Ok(original)` — no work was attempted, so the invariant does not apply.
`stop_on_exception` SHALL default to `false` (Apache Camel parity) and is
unchanged by this requirement.

#### Scenario: recipient_list sequential all-recipients-failed returns Err

- **GIVEN** a sequential `RecipientListService` with `stop_on_exception=false`
  and one recipient whose endpoint call returns `Err(CamelError)`
- **WHEN** `call(exchange)` is invoked with inbound body `"timer:t tick #1"`
- **THEN** the result is `Err(CamelError)` carrying the iteration-last error,
  and is NOT `Ok(exchange)` with the inbound body

#### Scenario: recipient_list parallel all-recipients-failed returns Err

- **GIVEN** a parallel `RecipientListService` with `stop_on_exception=false` and
  two recipients whose endpoint calls both return `Err(CamelError)`
- **WHEN** `call(exchange)` is invoked
- **THEN** the result is `Err(CamelError)` carrying a representative error from
  the failed join-set tasks, and is NOT `Ok(exchange)`

#### Scenario: recipient_list partial success preserves aggregation and returns Ok

- **GIVEN** a `RecipientListService` with `stop_on_exception=false` and two
  recipients where one returns `Err` and one returns `Ok(exchange_with_body)`
- **WHEN** `call(exchange)` is invoked
- **THEN** the result is `Ok(aggregated)` built from the successful recipient(s)
  only (the invariant does NOT fire because at least one success occurred)

#### Scenario: recipient_list empty resolved-recipient list is a legitimate no-op Ok

- **GIVEN** a `RecipientListService` whose expression evaluates to an empty
  string or all-empty tokens
- **WHEN** `call(exchange)` is invoked with the original exchange
- **THEN** the result is `Ok(original)` (no work was attempted, so the invariant
  does not apply)

### Requirement: multicast zero-success operational failure returns Failed

`multicast` (the parallel OutcomePipeline sibling of `recipient_list`) is
governed by the zero-success operational-failure invariant: when at least one
branch executed, zero branches returned `Completed`, AND no branch returned
`Stopped` (no intentional halt is being propagated), the multicast SHALL report
`Failed(last_error)` (per the parallel last-error determinism requirement).
When at least one branch returns `Stopped`, multicast SHALL propagate
`Stopped(exchange)` as an intentional halt (ADR-0025 §3) — this is NOT an
operational failure and SHALL NOT be reported as `Failed` or `Completed`.

**Out of scope:** the partial-success aggregation policy (some branches
`Completed`, some `Failed`, no `Stopped`) is NOT governed by this requirement.
The current rust-camel `multicast` returns `Failed` on any branch failure
regardless of successes; `recipient_list` aggregates successes on partial
failure. This inconsistency is tracked separately (rc-b41j) and is deliberately
out of scope for the zero-success invariant this change delivers. A future
change that reconciles the two siblings' partial-success behavior SHALL update
this requirement.

#### Scenario: multicast all-branches-failed with no Stopped returns Failed

- **GIVEN** a `multicast` Segment with two branches whose sub-pipelines both
  return `Failed(error)` and no branch returns `Stopped` or `Completed`
- **WHEN** the multicast runs
- **THEN** the outcome is `Failed(error)` (a representative error per JoinSet
  `join_next` order), and is NOT `Completed(original)` and NOT `Stopped(_)`

#### Scenario: multicast with a Stopped branch propagates Stopped over Completed and Failed

- **GIVEN** a `multicast` Segment with branches where at least one returns
  `Stopped(ex_b)` (other branches may return `Completed` or `Failed`)
- **WHEN** the multicast runs
- **THEN** the outcome is `Stopped(ex_b)` (intentional halt wins per ADR-0025
  §3), and is NOT `Failed(_)` and NOT `Completed(_)`

### Requirement: Cache write-back trust rule

`cache:` MAY write back only a body that a successful (`Completed`) `on_miss`
produced. It SHALL skip write-back when `on_miss` reports `Stopped(exchange)` or
`Failed(error)` (already true at `cache_eip.rs` step 3). This is the downstream
guarantor of the composition invariant for the cache-warming pattern and is the
cache-side expression of the zero-success rule. The normative owner of this rule
is the MODIFIED/ADDED `eip-cache` capability; this requirement cross-references
it for completeness.

#### Scenario: cache skips write-back when on_miss returns Failed

- **GIVEN** a `cache:` Segment with key `k`, a seeded stale entry under `k`, and
  an `on_miss` sub-pipeline that returns `Failed(CamelError)`
- **WHEN** the cache runs on a MISS (entry's in-band expiry elapsed)
- **THEN** no `repository.set` call is made for `k`, the Segment returns
  `Failed(error)`, and `cache_peek_stale(k)` afterwards returns the previously
  seeded stale entry (NOT the inbound body, NOT empty)

### Requirement: do_try catch body propagation through outer cache write-back

When a `do_try` catch clause runs on a `Failed` try_body and the catch body
produces a `Completed(exchange)` carrying a body, that body SHALL survive through
any outer `cache:` write-back boundary. A `cache_peek_stale` step inside a catch
that shares a key with an outer `cache:` step SHALL deliver the cached stale body
on the response, not an empty body (rc-65yi).

#### Scenario: cache_peek_stale inside do_try catch serves the stale body

- **GIVEN** a route `cache:{key:k, on_miss:[do_try:{ steps:[recipient_list
  url→broken], catch:[cache_peek_stale:{key:k}] }]}` with a seeded stale body
  under `k`
- **WHEN** the recipient_list fails (broken host) and the catch runs
- **THEN** the response carries the stale body (HTTP 200 with the stale body
  content), NOT an empty 200 and NOT the inbound body

### Requirement: Stream ownership on the error path — single consumption

A Segment that consumes a `Body::Stream` on the success path SHALL NOT read the
same stream a second time on the error path. When a recipient returns an error
response carrying a streaming inbound body, the stream SHALL be consumed at most
once.

> **Verification status:** Verified by reachability analysis (Task 3.1 of the
> outcome-aware-segment-composition change). Post-rc-20yn, a
> `recipient_list` zero-success returns `Err(last_error)` instead of
> `Ok(original)`, making the camel-http consumer reply path
> (`lib.rs:1569` "Body::Stream already consumed") unreachable for that
> scenario. Empirical verification of the producer 403 path
> (`lib.rs:2288-2291`) is tracked in bd rc-n8rc.

#### Scenario: recipient 403 with streaming body does not double-consume the stream

- **GIVEN** a `recipient_list` whose single recipient returns HTTP 403 with a
  streaming inbound body on the exchange
- **WHEN** `call(exchange)` is invoked
- **THEN** no `Body::Stream already consumed` error is emitted in the runtime
  log, and the stream is read at most once

### Requirement: Stream ownership on the error path — status and body propagation

When a recipient returns an error response carrying a streaming inbound body, the
error reply SHALL reach the caller with the correct HTTP status AND the response
body intact.

> **Verification status:** Verified by reachability analysis; empirical
> verification deferred to bd rc-n8rc (producer 403 path). See the note on
> the single-consumption requirement above.

#### Scenario: recipient 403 error reply reaches caller with status and body

- **GIVEN** a `recipient_list` whose single recipient returns HTTP 403 with a
  streaming inbound body on the exchange
- **WHEN** `call(exchange)` is invoked and the result propagates to the caller
- **THEN** the caller observes HTTP status 403 AND the response body content
  from the upstream error response
