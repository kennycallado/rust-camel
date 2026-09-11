# Design: readiness-failure-metrics

## Approach

Single-choke-point fix: the recording lives in `TracingProcessor::poll_ready`'s
readiness Err arm (`crates/camel-core/src/shared/observability/adapters/
tracer.rs`), reusing `record_step_metrics` — the exact function the call-time
Err arm uses — so families, levers, and labels cannot drift:
`increment_exchanges` and `increment_errors(e.classify())` with the
CIRCUIT_OPEN skip (the breaker counts its own rejections;
dashboard-observability D2). Duration is call-time only (ADR-0066 population
contracts): the readiness Err arm passes `include_duration = false`, so
`camel_exchange_duration_seconds` never samples a readiness attempt.

rc-mn8n review: the pre-call gate polls must not record. On handler routes
the pipeline poll in `RouteChannelService::poll_ready` is skipped and
`TracedPipeline::poll_ready` skips its first-step poll when a handler is
present — every invoke re-polls the step (`RetryableStep::invoke` calls
`ready()` before `call`), so the invoke re-poll is the single recording site
and Pending backpressure is preserved at invoke. Non-handler routes are
untouched: the poll's Err is their only readiness signal (the call never
runs on failure), so they record exactly once.

## Alternatives considered

- Record in `invoke_processor` (camel-processor): it has no metrics or
  route-id access; recording there would plumb observability through a
  generic helper. Rejected — the tracer adapter already owns per-step
  recording.
- Record in `DirectProducer`: component-specific, misses every other
  component's readiness failures. Rejected.
- Convert readiness failures into call-time errors: masks the phase
  distinction and changes error semantics. Rejected.

## Test strategy

- Near-tier: `readiness_err_records_families` in `tracer_tests.rs` — a
  `poll_ready`-failing double proves the exchange and error families record
  with the call-time label (`increment_errors:r:processor`) and no duration
  sample lands.
- Integration: the metrics wiring error leg drops its
  `failIfNoConsumers=false` opt-out and asserts sample-level
  `camel_exchanges_total{` / `camel_errors_total{` in the scrape. RED
  before the fix (the poll never sees a sample), GREEN after.
