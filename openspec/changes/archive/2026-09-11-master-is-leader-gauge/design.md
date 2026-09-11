# Design: master-is-leader-gauge

## Approach

Typed setter, not `record_gauge`: there is no generic gauge escape hatch on
`MetricsCollector`, by design. The new
`set_master_leadership(&self, lock: &str, leader: bool)` follows the
existing typed-setter convention exactly (`set_route_state` pattern):
default no-op body in the trait (backward-compatible), explicit forward in
`MetricsHandle`, fan-out in `CompositeMetricsCollector`, real registration
in the Prometheus exporter.

Edges-only emission: `emit_leadership_transition`
(`crates/components/camel-master/src/leadership.rs`) already fires exactly
once per observed acquire/lose edge. The gauge emission
(`emit_leadership_state`) is called from that helper, so gauge and counter
ride the same path and can never disagree about edge counts. The gauge
value is `leader = (event == "acquired")`; both edge kinds are covered,
including the initial leadership-watch snapshot.

Exported name: the Prometheus family is registered statically as
`master_is_leader` with `namespace("camel")` (the
`crates/services/camel-prometheus/src/metrics/families.rs` convention), so
the exported name is `camel_master_is_leader`. This mirrors how the master
counters are passed bare (`master_leadership_transitions_total`) and
exported with the `camel_` prefix.

## Alternatives considered

- Record a dynamic gauge through `record_counter`-style dynamic paths:
  there is no dynamic gauge path; abusing a counter would preserve the
  unreadable steady-state semantics. Rejected.
- Dotted recorded name `camel.master.is_leader`: would also normalize to
  `camel_master_is_leader` post-rc-oo2w, but the typed setter carries no
  recorded name — the family name is declared in `families.rs`. Moot; the
  underscore form is declared directly.
- Emitting the gauge from `reconcile_event` acquire/lose sites instead of
  the transition helper: duplicates the edge condition in three places
  (initial snapshot plus the watch loop's two branches). Rejected.

## Test strategy

TDD, red first. Two async tests in
`crates/components/camel-master/src/tests.rs` drive the existing
leadership-transition harness (`FakeLeadershipService` + recording
collector): gauge reads 1 while leadership is held; gauge reads 0 after
the `StoppedLeading` edge. One render test in
`crates/services/camel-prometheus/src/metrics/tests.rs` asserts the
exported series `camel_master_is_leader{lock="my-lock"}` flips 1 → 0 in
place. One forwarding test in `crates/camel-api/src/metrics.rs` covers
handle + composite delegation.
