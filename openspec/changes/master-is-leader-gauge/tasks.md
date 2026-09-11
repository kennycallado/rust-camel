# Tasks: master-is-leader-gauge

## Task 1: Typed setter and master emission

**Files:**
- `crates/camel-api/src/metrics.rs` (modified — `set_master_leadership` default no-op; handle forward; composite fan-out; forwarding + parity tests)
- `crates/components/camel-master/src/leadership.rs` (modified — `emit_leadership_state` helper called from `emit_leadership_transition`)
- `crates/components/camel-master/src/tests.rs` (modified — gauge capture, accessor, 2 edge tests)

**Steps:**
1. RED: add `set_master_leadership` to the trait (default no-op), teach the
   recording test double to log gauge edges, and add
   `is_leader_gauge_is_one_while_leadership_held` +
   `is_leader_gauge_is_zero_after_leadership_lost`; run `cargo test -p
   camel-master --lib is_leader_gauge` — both fail on the 5 s gauge-await
   bound because nothing emits.
2. GREEN: add `emit_leadership_state` and call it from
   `emit_leadership_transition` (gauge rides the same observed edge);
   forward through `MetricsHandle` and `CompositeMetricsCollector`; add the
   handle/composite forwarding test and extend the full-surface parity test.
3. `cargo test -p camel-master -p camel-api` green.

**Acceptance:**
- Both gauge edge tests pass after failing first.
- `cargo clippy -p camel-master -p camel-api --all-targets -- -D warnings`
  green; `cargo fmt --check` clean.

- [x] 1

## Task 2: Prometheus static family

**Files:**
- `crates/services/camel-prometheus/src/metrics/families.rs` (modified — `master_is_leader` IntGaugeVec, namespace `camel`, label `lock`)
- `crates/services/camel-prometheus/src/metrics/mod.rs` (modified — `set_master_leadership` override)
- `crates/services/camel-prometheus/src/metrics/tests.rs` (modified — render test)

**Steps:**
1. Register the static family and implement the setter: value 1 while held,
   0 after loss, same series flipped in place.
2. `master_is_leader_gauge_edges` renders
   `camel_master_is_leader{lock="my-lock"} 1` then `0`.
3. `cargo test -p camel-prometheus` green.

**Acceptance:**
- Exported name is exactly `camel_master_is_leader`.
- `cargo clippy -p camel-prometheus -p camel-otel --all-targets -- -D
  warnings` green.

- [x] 2

## Task 3: Spec delta

**Files:**
- `openspec/changes/master-is-leader-gauge/specs/master-component/spec.md` (new delta)

**Steps:**
1. `## ADDED Requirements` with `### Requirement: Master component exports
   leadership state gauge` and the three scenarios (1 while held; 0 after
   yielding; uniform lock label).
2. `openspec validate master-is-leader-gauge --type change --json` — no
   delta-structure errors.

- [x] 3
