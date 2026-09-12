# Proposal: steplatency

## Why

`camel run --otel` exports route-level duration, but it does not identify which
`to:` step consumed that time. Route compilation currently discards the
declared URI when it creates a `CompiledStep`, so the supported exporter path
cannot provide per-step latency. This blocks useful operator profiling and is
tracked by bd `rc-cd7o`.

## What Changes

- Retain the declared `to:` URI as compile-time metadata on compiled process
  steps.
- Carry that metadata into the tracing processor and emit a
  `step_duration_secs` histogram with `route` and `to_uri` labels for call-time
  duration metrics.
- Add focused compiler and metrics tests, including disabled-duration and
  readiness-path behavior.
- Record the compile-time retention and declared-URI cardinality decision in a
  new ADR.

The internal `BENCH_LATENCY_FILE` tooling and its output format are explicitly
out of scope.

## Acceptance criteria

- A compiled `To` step retains its declared URI; non-`To` steps do not.
- OTEL metrics emit `step_duration_secs` with `route` and `to_uri` labels for
  call-time `To` steps when duration metrics are enabled.
- Disabled duration metrics and readiness polling do not emit the histogram.
- Existing route and exchange metric behavior remains unchanged.
- BENCH_LATENCY tooling is unchanged.

## Risk budget

The change is limited to camel-core compilation and observability adapters plus
tests and one ADR. Raw declared URIs are accepted as an intentional open label
because they are static route configuration, not exchange-derived values; URI
normalization, benchmark changes, and exporter redesign are out of bounds.
