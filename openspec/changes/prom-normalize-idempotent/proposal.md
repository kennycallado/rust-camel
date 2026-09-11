# Proposal: prom-normalize-idempotent

## Why

The Prometheus exporter doubles the `camel_` prefix on dotted built-in metric
names. `normalize_prom_name`
(`crates/services/camel-prometheus/src/metrics/mod.rs`) only recognizes the
underscore form `camel_` as already-prefixed, but cache EIP counters are
recorded in dot form (e.g. `camel.cache.misses`,
`crates/camel-processor/src/cache_eip.rs:319,336` — mandated by
`openspec/specs/eip-cache/spec.md:843`). The dotted name fails the prefix
check, receives a second `camel_`, and sanitization turns
`camel.camel.cache.misses` into `camel_camel_cache_misses`. Found live in the
alloc-demo scenario 2 export (bd rc-oo2w).

## What Changes

- `normalize_prom_name` treats a leading `camel.` (dot form) as
  already-prefixed, exactly like `camel_`; sanitization behavior is unchanged
  (the dot still becomes `_`), so `camel.cache.misses` normalizes to a single
  `camel_cache_misses`.
- No recorded metric names change: the eip-cache spec mandates dot-form
  recording, so renaming record calls is not an option — the fix is
  normalize-side only.

## Acceptance criteria

- `normalize_prom_name("camel.cache.misses") == "camel_cache_misses"`.
- `normalize_prom_name("camel_exchanges_total") == "camel_exchanges_total"`
  (underscore form unchanged).
- `normalize_prom_name("exec_successes_total") ==
  "camel_exec_successes_total"` (foreign names prefixed exactly once).
- `cargo test -p camel-prometheus` green; `openspec validate
  prom-normalize-idempotent --type change` reports no delta-structure errors.
