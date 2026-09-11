# Proposal: master-is-leader-gauge

## Why

"Who leads lock X right now" is not answerable from the metrics surface: the
existing master metrics (`master_leadership_transitions_total`,
`master_delegate_lifecycle_total`) are transition counters whose value reads
0 while leadership is steady — correct semantics for counters, but it forces
operators to `kubectl get lease` during incidents. A demo-team split-brain
incident cost hours of diagnosis for exactly this reason (bd rc-02dx).

## What Changes

- Add a typed gauge setter `MetricsCollector::set_master_leadership(lock,
  leader)` following the established setter convention
  (`set_route_state` pattern: default no-op, forwarders pass through).
- The master supervision path emits the gauge on the same observed
  leadership-state edges as `master_leadership_transitions_total`: 1 on the
  acquire edge, 0 on the lose edge. No startup initialization: the gauge
  series appears on the first observed edge.
- The Prometheus exporter registers the static family
  `camel_master_is_leader{lock}` using the same `lock` label the leadership
  counters use. The counters stay transition-only; the gauge exists for
  steady-state readability.

## Acceptance criteria

- `camel_master_is_leader{lock="my-lock"} 1` renders while leadership is
  held; the same series flips to `0` after the lose edge.
- The gauge is emitted by the same code path as the transition counters
  (`emit_leadership_transition`), so both edges are always paired.
- `cargo test -p camel-master` and `cargo test -p camel-prometheus` green;
  `openspec validate master-is-leader-gauge --type change --json` reports no
  delta-structure errors.
