# Proposal: correlationfix

## Why

The aggregate `correlation_key` contract is split three ways (bd `rc-q8ng`,
investigation 91). The declarative model accepts the field and the canonical
lowering path honors it (`CorrelationStrategy::Expression`), but the normal
builder lowering path (`compile_aggregate_step`) silently discards it and
correlates by `header` instead. Validation then requires `correlation_key`
for every aggregate. Net effect: a valid-looking YAML route with
`correlation_key: "${header.orderId}"` executes with WRONG correlation
semantics (header bucket) and no diagnostic — messages merge into incorrect
groups, which is silent business-level data corruption. Root cause class:
implementation drift from an incomplete contract design, not a typo.

## What Changes

- `camel-api`: add `AggregatorConfigBuilder::correlate_by_expr(expr, language)`
  — the missing enabling operation for the existing
  `CorrelationStrategy::Expression` variant.
- `camel-dsl` compile: wire `correlation_key` through
  `compile_aggregate_step` via one shared correlation-mapping helper used by
  BOTH lowering paths, so normal and canonical lowering construct identical
  `AggregatorConfig` semantics. Delete the stale "intentionally not wired"
  note. Precedence when both sources are present: `correlation_key`
  (expression) overrides `header` — mirrors what the canonical path already
  does and what builder canonicalization round-trips.
- `camel-dsl` authoring + validation: `header` becomes optional at YAML parse
  (serde default `""`); validation becomes requires-one-of — at least one of
  `header` / `correlation_key` non-empty (empty `correlation_key` string
  rejected). Header-only aggregates become valid (today they are rejected).
- `camel-builder`: canonicalization round-trip tests for expression
  correlation (no behavior change expected).
- Docs: `docs/src/yaml-dsl/step-verbs.md` and `docs/src/eip/aggregator.md`
  updated to the post-fix truth (one-of source, override precedence).
- Schema artifacts regenerated for the route-AST `header` default change.

Excluded: runtime processor changes (`CorrelationStrategy::Expression`
already supported), `CanonicalAggregateSpec` shape changes, camel-cli,
scripts/xtask edits.

## Acceptance criteria

- Expression key through normal declarative lowering yields
  `CorrelationStrategy::Expression` with language `simple`.
- Expression key through canonical lowering yields the identical strategy.
- Header-only aggregate compiles to `CorrelationStrategy::HeaderName` on both
  paths (previously rejected by validation).
- Both sources present: expression wins on both paths, identically.
- Missing both (or empty strings) fails validation with a stable config
  error on both entry points (`validate_route` gates both).
- Builder-to-canonical round trip preserves expression correlation.
- Docs match the implemented contract; no stale header-only claims.

## Risk budget

Accepted: header-only authoring flips from validation-error to valid — this
is the documented intent, not a regression. Expression evaluation at runtime
is a pre-existing language-registry capability; `correlation_key` is
operator config (trusted side of the ADR-0032 trust boundary). Out of
bounds: any `camel-processor` change, canonical serialization break,
cross-crate signature breaks beyond the additive builder method.
