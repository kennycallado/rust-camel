# Proposal: endpoints-created-counter

## Why

Endpoint churn under dynamic URIs was invisible without source reading. Components
that hold expensive per-endpoint state (redis, kafka, jms class) churned that state
under dynamic URIs, and the http leak showed no signal exposed the class of bug:
nothing counted endpoint creations. Operators could see exchange and error rates
but not that endpoints were being created repeatedly.

## What Changes

- `camel-core` records a counter at the endpoint resolver choke point
  (`lifecycle/adapters/endpoint_resolver_factory.rs`) on every successful endpoint
  creation. Recorded name: `camel.core.endpoints_created_total` (dot form);
  exported family: `camel_core_endpoints_created_total` (rc-oo2w idempotent
  normalization).
- The counter carries one label, `component=<URI scheme>`. The label value set is
  bounded by the registered component schemes; it is an open label by construction
  and is lint-annotated (`allow-open-label rc-haik`).
- No trait changes: `Component::create_endpoint` is untouched; the counter lives in
  camel-core where the shared metrics handle is already reachable through
  `RuntimeObservability::metrics()`.

## Acceptance criteria

- `cargo test -p camel-core --lib` green; the two new tests
  (`endpoints_created_counter_increments_per_created_endpoint`,
  `endpoints_created_counter_in_exported_registry`) pass.
- The recorded family name is exactly `camel.core.endpoints_created_total`, which
  exports as `camel_core_endpoints_created_total`.
- `cargo xtask lint-metric-labels` OK (annotation holds).
- `openspec validate endpoints-created-counter --type change --json` reports no
  delta-structure errors.
