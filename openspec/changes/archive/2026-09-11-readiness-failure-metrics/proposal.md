# Proposal: readiness-failure-metrics

## Why

With `failIfNoConsumers=true` (the default), `DirectProducer::poll_ready`
fails at pipeline readiness BEFORE the traced wrapper executes, so the
tracer adapter recorded nothing: no exchanges, duration, or errors
families for the failure. The metrics wiring test had to opt out with
`failIfNoConsumers=false` to observe the pipeline families at all (bd
rc-mn8n).

## What Changes

- `TracingProcessor::poll_ready` records the same families with the same
  labels as the call-time Err arm — readiness-attempt duration, exchange
  count, error class (CIRCUIT_OPEN still excluded) — through the shared
  `record_step_metrics` choke point.
- The metrics wiring error leg (`add_failing_route`) drops the
  `failIfNoConsumers=false` opt-out: the readiness-phase failure is now
  observable end to end. Legs that pin the call-time component path keep
  their own explicit `failIfNoConsumers=false` routes.

## Acceptance criteria

- `readiness_err_records_families` (camel-core unit) passes: a readiness
  Err records the exchange, the duration, and the `processor` error label.
- `prometheus_only_emits_pipeline_and_component_metrics` passes with the
  default `failIfNoConsumers=true`: `camel_exchanges_total{` and
  `camel_errors_total{` samples present in the scrape.
- `cargo test -p camel-core --lib` exits 0.
- `openspec validate readiness-failure-metrics --type change --json`
  reports no delta-structure errors.
