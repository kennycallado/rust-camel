# ADR: Compile-time URI retention for step latency metrics

- **Status:** Accepted (implemented in OpenSpec change `steplatency`; bd `rc-cd7o`)
- **Amends:** ADR-0042, ADR-0066

## Context

Route compilation currently turns a `To(uri)` builder step into a processor
without retaining the authored URI. The OTEL tracing adapter can therefore
record route and exchange metrics, but cannot attribute call-time latency to a
specific outbound step. The dynamic-label metrics API already supports the
required histogram labels.

## Decision

`CompiledStep::Process` carries optional compile-time URI metadata. The compiler
stamps the raw declared URI only for `To` steps before endpoint resolution and
interception. Tracing carries this metadata into `TracingProcessor` and records
`step_duration_secs` with `route` and `to_uri` labels when the duration lever is
enabled and the attempt is a call-time attempt. A failed call-time attempt also
records the histogram; the observation measures the failed attempt. Readiness
attempts never record the family.

The label contains trusted, operator-authored route configuration. It is not
derived from exchange data and is not the resolved runtime target. The existing
open-label lint justification is required because URI cardinality is an
intentional part of this profiling surface.

## Consequences

Operators using `camel run --otel` receive per-`to_uri` duration series without
changing component contracts or benchmark tooling. Every compiled process step
stores one optional shared string, and route composition must preserve it.
Readiness attempts remain excluded from duration histograms, consistent with
ADR-0066. Dynamic or exchange-derived URI attribution is not provided.

## Implementation evidence

Task 2.1 shipped the emission in `camel-core`. Unit tests in `tracer_tests.rs`
pin the label set, the failed-call emission, the `include_duration` gate, the
duration-lever gate, and the readiness exclusion. Task 3.1 pins the exported
family at the integration tier in `crates/camel-test/tests/metrics_wiring_test.rs`:
`otel_metrics_expose_step_duration_family` drives one exchange through a
`To("direct:orders")` route on the OTEL-enabled wiring probe and asserts the
collector observed `step_duration_secs` labeled with `route` and `to_uri`.
`otel_metrics_omit_step_duration_for_processor_route` drives a processor-only
route through the same wired path and asserts the family has zero data points.
