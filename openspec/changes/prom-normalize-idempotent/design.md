# Design: prom-normalize-idempotent

## Approach

Normalize-side fix only: the prefix decision in `normalize_prom_name`
(`crates/services/camel-prometheus/src/metrics/mod.rs:14`) currently checks
`name.starts_with("camel_")`. The check gains `|| name.starts_with("camel.")`,
treating the dot-form prefix as already-prefixed. The dot is then replaced by
`_` during the existing sanitization pass, yielding a single `camel_` prefix.
Sanitization behavior is otherwise identical.

Renaming the recorded metric names to underscore form is NOT an option:
`openspec/specs/eip-cache/spec.md:843` mandates the dot-form recorded names
(`camel.cache.misses` etc.) at
`crates/camel-processor/src/cache_eip.rs:319,336`, and the exporter must stay
name-agnostic for foreign collectors.

## Alternatives considered

- Rename record calls to `camel_cache_misses`: violates the eip-cache spec and
  changes the OTel exporter surface too; rejected.
- Strip `camel.` / `camel_` and re-prefix: equivalent output but more churn;
  the one-line guard is minimal and keeps the pass-through path unchanged.

## Test strategy

Three unit tests in
`crates/services/camel-prometheus/src/metrics/tests.rs` (TDD, red before the
fix): dotted built-in normalizes to a single `camel_`; underscore form is
unchanged; foreign name gets exactly one prefix.
