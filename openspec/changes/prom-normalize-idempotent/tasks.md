# Tasks: prom-normalize-idempotent

## Task 1: Idempotent camel_ prefix in normalize_prom_name

**Files:**
- `crates/services/camel-prometheus/src/metrics/mod.rs` (modified — prefix guard also accepts `camel.` dot form)
- `crates/services/camel-prometheus/src/metrics/tests.rs` (modified — 3 unit tests)

**Steps:**
1. RED: add `normalize_prom_name_dotted_camel_prefix_not_doubled`
   (`camel.cache.misses` → `camel_cache_misses`),
   `normalize_prom_name_underscore_camel_prefix_unchanged`
   (`camel_exchanges_total` → unchanged), and
   `normalize_prom_name_foreign_name_prefixed_once`
   (`exec_successes_total` → `camel_exec_successes_total`) to `helper_tests`;
   run `cargo test -p camel-prometheus normalize_prom_name` — the dotted test
   must fail with `camel_camel_cache_misses`.
2. GREEN: extend the prefix guard in `normalize_prom_name` to also treat a
   leading `camel.` as already-prefixed; sanitization unchanged.
3. Run `cargo test -p camel-prometheus` — full suite green.

**Acceptance:**
- All 3 new tests pass; full `cargo test -p camel-prometheus` green.
- `cargo clippy -p camel-prometheus --all-targets -- -D warnings` green.
- `cargo fmt --check` clean; `cargo xtask lint-metric-labels` OK.

- [x] 1

## Task 2: Spec delta for idempotent normalization

**Files:**
- `openspec/changes/prom-normalize-idempotent/specs/metrics-contract-hardening/spec.md` (new delta)

**Steps:**
1. Add `## ADDED Requirements` with `### Requirement: Prometheus name
   normalization is idempotent` and the three scenarios (dotted built-in
   single-prefix; underscore unchanged; foreign prefixed once).
2. `openspec validate prom-normalize-idempotent --type change --json` — no
   delta-structure errors.

- [x] 2
