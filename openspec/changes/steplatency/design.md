# Design: steplatency

## Approach

Add optional `Arc<str>` metadata to `CompiledStep::Process`. The step compiler
registry stamps this field from the original `BuilderStep::To(uri)` before
dispatching to endpoint compilation, matching the existing label and kind-hint
metadata flow. The declared URI is retained, rather than a resolved or
intercept-substituted URI, so the metric identifies the route configuration
that the operator authored and avoids exchange-derived cardinality.

Thread the metadata through the tracing pipeline composition into
`TracingProcessor`. Extend the existing `record_step_metrics` helper with the
URI and, when call-time durations are enabled, call the existing dynamic-label
`MetricsCollector::record_histogram` API using metric name
`step_duration_secs` and labels `route` and `to_uri`. The same helper is used by
the readiness path, so it must keep both `include_duration` and the configured
duration lever as gates for this new histogram. Call-time attempts record the
duration whether the processor succeeds or returns an error; readiness attempts
remain excluded. Add the metric-label lint justification at the emission site.

## Affected crates

- `camel-core`: compiled-step metadata, route composition, tracing emission,
  and unit/integration tests.
- `camel-api`: no code change; its existing dynamic-label metrics contract is
  reused.
- `docs/adr`: document the architectural retention and label decision.

## Architecture boundaries

This is a Runtime data-plane observability adapter change. Compilation records
trusted operator configuration; execution only observes the immutable metadata
and exchange result. No DSL, Component, control-plane, or benchmark protocol
changes are required. The design preserves the metrics lifetime and family
gating rules from ADR-0066 and the compiled snapshot discipline from ADR-0042.

## Alternatives considered

- **Recover the URI from the processor or endpoint at runtime:** rejected;
  processors do not provide a stable URI contract and this would couple
  observability to component implementations.
- **Add a separate benchmark output family:** rejected; `BENCH_LATENCY_FILE`
  is an internal bridge-tax tool and is explicitly outside this user-facing
  OTEL feature.
- **Use the resolved/intercepted target URI:** rejected; the metric should
  report the declared route step and must not expose runtime exchange data.
