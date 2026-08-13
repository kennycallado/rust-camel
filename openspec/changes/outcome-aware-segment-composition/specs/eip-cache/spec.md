# eip-cache

## Purpose

Extend the cache EIP contract with the write-back trust rule and the
stale-body-preservation requirement. These are the cache-side guarantors of the
segment-outcome-composition invariant (ADR-0058). All unchanged requirements from
the canonical `openspec/specs/eip-cache/spec.md` remain in force.

## ADDED Requirements

### Requirement: Cache write-back skips on Stopped and Failed on_miss outcomes

The cache Segment SHALL write back a body ONLY when the `on_miss` sub-pipeline
reports `PipelineOutcome::Completed(exchange)`. When the on_miss reports
`Stopped(exchange)` or `Failed(error)`, the cache SHALL propagate that outcome
as-is and SHALL NOT write any entry to the repository. This prevents poisoning
the cache with an inbound body that a failed on_miss did not legitimately
produce (rc-20yn). This requirement is the cache-side expression of the
segment-outcome-composition zero-success invariant.

#### Scenario: cache skips write-back when on_miss returns Failed

- **GIVEN** a `cache:` Segment with key `k`, a seeded stale entry under `k`, and
  an `on_miss` sub-pipeline that returns `Failed(CamelError)`
- **WHEN** the cache runs on a MISS (the entry's in-band expiry has elapsed)
- **THEN** no `repository.set` call is made for `k`, the Segment returns
  `Failed(error)`, and `cache_peek_stale(k)` afterwards returns the previously
  seeded stale entry (NOT the inbound body, NOT empty)

#### Scenario: cache skips write-back when on_miss returns Stopped

- **GIVEN** a `cache:` Segment with key `k` and an `on_miss` sub-pipeline that
  returns `Stopped(exchange)` (e.g. an inner Stop EIP)
- **WHEN** the cache runs on a MISS
- **THEN** no `repository.set` call is made for `k` and the Segment returns
  `Stopped(exchange)` with the exchange state intact

### Requirement: Stale body survives through do_try catch + cache write-back

When a `cache_peek_stale` step runs inside a `do_try` catch clause that shares a
key with an outer `cache:` step, the stale body retrieved by `cache_peek_stale`
SHALL survive through the do_try `Completed` outcome and any outer cache
write-back boundary. The response SHALL carry the stale body, not an empty body
(rc-65yi).

#### Scenario: stale-serve route returns the stale body, not empty 200

- **GIVEN** a route `cache:{key:k, on_miss:[do_try:{ steps:[recipient_list
  url→broken], catch:[cache_peek_stale:{key:k}] }]}` and a seeded stale body
  under `k`
- **WHEN** the recipient_list fails (broken host) and the catch runs
- **THEN** the response carries the stale body (HTTP 200 with the stale body
  content), NOT an empty 200 and NOT the inbound body
