# Tasks: endpoints-created-counter

## Task 1: Counter at the endpoint resolver choke point

**Files:**
- `crates/camel-core/src/lifecycle/adapters/endpoint_resolver_factory.rs` (modified — record `camel.core.endpoints_created_total{component}` via `rt.metrics()` after successful `create_endpoint`, with the `allow-open-label rc-haik` annotation)

**Steps:**
1. RED: add `endpoints_created_counter_increments_per_created_endpoint`
   (resolve one `direct:` and one `mock:` URI; assert one increment per
   component label) and `endpoints_created_counter_in_exported_registry`
   (recorded family name is exactly `camel.core.endpoints_created_total`);
   run `cargo test -p camel-core --lib` — both fail with zero recorded
   counter calls.
2. GREEN: record the counter in `make_endpoint_resolver` through the shared
   handle (`rt.metrics()`), annotated for the open `component` label.
3. Run `cargo test -p camel-core --lib` (full suite green) and
   `cargo test -p camel-core --test hexagonal_architecture_boundaries_test`.

**Acceptance:**
- Both new tests pass; full `cargo test -p camel-core --lib` green.
- `cargo fmt --check` clean; `cargo clippy -p camel-core --all-targets -- -D warnings` green.
- `cargo xtask lint-metric-labels` OK.

- [x] 1

## Task 2: Spec delta for endpoint-creation visibility

**Files:**
- `openspec/changes/endpoints-created-counter/specs/component-metrics-emission/spec.md` (new delta)

**Steps:**
1. Add `## ADDED Requirements` with `### Requirement: Endpoint creation is
   visible` and the two scenarios (one increment per component across two
   schemes; repeated dynamic URIs accumulate on the component label).
2. `openspec validate endpoints-created-counter --type change --json` — no
   delta-structure errors.

- [x] 2
