# Proposal: partner-request-shapes

## Why

Partner `validate` targets assert filtered COUNTS only (bd rc-7ura, deferred
by design in itest-partner-faults-and-asserts). A count proves the retry
happened; it does not prove WHAT was sent — payload idempotency keys,
retry-identical bodies, header propagation. Count semantics are field-proven
since 2026-09-05 (b6054478). The itest-batch final review held this follow-up
for the next matcher-algebra change; the fleet dispatched it now as a
test-support-crate-only mission (master order 331).

## What Changes

- `camel-matchers` (algebra, additive, pure): new `RequestShape` type
  (`method`, one path filter, `query` subset, `body: Expectation`), a new
  `requests: Option<Vec<RequestShape>>` field on `RequestExpectation`, and a
  pure per-request shape judgment function over projected
  `(method, path_and_query, body-value)` observations, reporting the first
  mismatch by index.
- `camel-integration-test` (grammar + observation): the partner expectation
  grammar gains a `requests` list — positional shape asserts over the
  FILTERED recorded sequence, exclusive with `count`/`atLeast`/`atMost`
  (the parser synthesizes the exact bound `requests.len()`); each entry
  reuses the existing `method`/`path`/`pathContains`/`pathMatches`/`query`
  readers plus `body` under the shared `Expectation` dual grammar. The
  runner projects recorded bodies as values (`reply_bytes_value` precedent)
  and judges shapes on the same snapshots the count logic already reads.
- Diagnostics: mismatch names the partner, the 1-based filtered index, the
  failed aspect, and the expected-versus-observed values — each side
  rendered under per-aspect ADR-0051 rules; headers are never rendered.
- One scenario fixture demonstrates the retry story: a faulted partner
  (500 then 200) with a route retry, asserting retry-identical projected
  body values via full-body `equals` on both entries.

Explicitly excluded: header asserts (blocked by a missing header-secret
redaction rule — needs an ADR-0051 extension), `unordered` shape matching,
shape asserts combined with `atLeast`/`atMost`, negative/`last` indexing,
cross-partner-key sequences, body truncation in diagnostics.

## Acceptance criteria

- A partner validate with `requests: [...]` asserts the Nth filtered
  recorded request's method, path, query, and body — not just counts.
- `requests` mixed with any count-bound form fails at load with
  `doc-validation`; an empty `requests` list fails at load.
- A shape mismatch fails with `validation-mismatch` naming the index and
  the failed aspect; pass/fail decide on snapshots under the existing
  deadline rules, failing fast on any present mismatched element.
- A `.test.yaml` scenario demonstrates a shape assert proving
  retry-identical projected body values on a faulted partner.

## Risk budget

Test-support crates only (`camel-matchers`, `camel-integration-test`):
no engine crate may change. Additive, `#[non_exhaustive]`-respecting
algebra surface; purity law (ADR-0072) and redaction law (ADR-0051) hold.
Bd: rc-7ura (P3).
