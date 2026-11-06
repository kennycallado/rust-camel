# builder Specification

## Purpose
TBD - created by archiving change builder-layer-middleware. Update Purpose after archive.
## Requirements
### Requirement: Type-erased processor layer factory

The system SHALL expose `camel_api::ProcessorLayer`, an object-safe factory that wraps a
`BoxProcessor` with a `tower::Layer` and returns the layered processor as a
`BoxProcessor`. A blanket implementation SHALL accept every `tower::Layer<BoxProcessor>`
where the layer is `Send + Sync + 'static` and its associated service is
`Service<Exchange, Response = Exchange> + Clone + Send + Sync + 'static` with an error
convertible into `tower::BoxError` and a `Send` future. Layers that do not satisfy these
bounds SHALL fail to compile at the call site.

#### Scenario: Standard tower layer accepted

- **GIVEN** a `tower::timeout::TimeoutLayer` configured with a duration
- **WHEN** the layer is passed to `LayerFactory::new` and used to wrap an existing
  `BoxProcessor`
- **THEN** the wrap succeeds and the returned `BoxProcessor` invokes the original
  processor through the timeout layer

#### Scenario: Non-conforming layer rejected at compile time

- **GIVEN** a `tower::Layer<BoxProcessor>` whose service type is not `Clone` (for
  example a direct `tower::limit::RateLimit` service)
- **WHEN** the layer is passed to the blanket `ProcessorLayer` implementation
- **THEN** compilation fails with a trait-bound error instead of failing at runtime

#### Scenario: Same-handle readiness completes under bounded concurrency

- **GIVEN** a producer wrapped with `ConcurrencyLimitLayer` driven by 100 concurrent
  invocations of the wrapped `BoxProcessor`
- **WHEN** each invocation runs the readiness call then the invoke call on the same
  service handle, with a per-test watchdog bounding the total run to a finite deadline
- **THEN** all 100 invocations complete within that deadline (no readiness deadlock)
  and no more than the configured limit run inside the producer at any moment

### Requirement: BoxError adapter for CamelError

The system SHALL implement `From<tower::BoxError> for CamelError` such that: a boxed
`CamelError` maps to itself unchanged (passthrough without re-wrapping); an error whose
source chain contains `tower::timeout::error::Elapsed` maps to
`CamelError::ProcessorErrorWithSource` whose message names a middleware timeout and
whose source chain retains the original boxed error; any other error maps to
`CamelError::ProcessorErrorWithSource` exposing the original error through `source()`.

#### Scenario: Boxed CamelError passes through unchanged

- **GIVEN** an inner `CamelError`-typed service failure boxed into a `tower::BoxError`
- **WHEN** the boxed error crosses the adapter
- **THEN** the resulting error is the original `CamelError` variant with its fields
  intact, not a nested `ProcessorErrorWithSource` wrapper

#### Scenario: Timeout error arrives as a typed Camel error

- **GIVEN** a producer wrapped with `TimeoutLayer` that needs longer than the configured
  timeout
- **WHEN** an exchange is sent through the wrapped processor
- **THEN** the call fails with a `CamelError` whose `source()` chain reaches the tower
  `Elapsed` error, discriminable via `downcast_ref`

#### Scenario: Timeout detected through a nested error chain

- **GIVEN** a middleware error whose source chain wraps the tower `Elapsed` error one or
  more levels deep
- **WHEN** the error crosses the adapter
- **THEN** the resulting `CamelError` message names a middleware timeout and the full
  original chain remains reachable via `source()`

#### Scenario: Non-timeout middleware error preserves its source

- **GIVEN** a middleware that fails with an arbitrary `BoxError`
- **WHEN** the error crosses the adapter
- **THEN** the resulting `CamelError::ProcessorErrorWithSource` exposes the original
  error through `source()`

### Requirement: Builder hook for layered producers

The system SHALL provide `StepAccumulator::to_layered(uri, layer)` which compiles a send
to `uri` whose resolved producer is wrapped with the given layer, keeping endpoint
lifecycle, body contract, and declared-URI metadata identical to the equivalent
unlayered `to` step.

#### Scenario: Producer wrapped with a concurrency limit

- **GIVEN** a route `from("direct:in").to_layered("mock:out", ConcurrencyLimitLayer)`
- **WHEN** exchanges are sent concurrently through the route
- **THEN** the number of producer invocations in flight never exceeds the configured
  limit

#### Scenario: Layered producer keeps endpoint metadata

- **GIVEN** a route with `.to_layered("direct:out", layer)`
- **WHEN** the step is compiled
- **THEN** the compiled step carries the declared `to:` URI metadata and the endpoint
  lifecycle handle, like the equivalent `.to("direct:out")` step

#### Scenario: Producer readiness error propagates

- **GIVEN** a layered producer whose underlying endpoint producer fails readiness
  (`poll_ready` returns an error)
- **WHEN** the pipeline polls the compiled layered step for readiness
- **THEN** the readiness error surfaces from the layered step instead of being swallowed

### Requirement: Builder hook for layered route sections

The system SHALL provide `StepAccumulator::layered(layer, section)` which compiles the
steps built by the `section` closure into a single segment whose composed pipeline is
wrapped with the given layer, and the wrap SHALL be segment-mode so that a Stop inside
the section stops the route instead of being flattened into a completed exchange. When
the section outcome is stopped, the exchange SHALL carry the `CamelStop` property set to
true after crossing the Tower-result boundary, regardless of any prior property value.

#### Scenario: Section wrapped with a rate limit

- **GIVEN** a route whose section closure adds two steps, wrapped with a buffered rate
  limit
- **WHEN** more exchanges than the configured rate arrive within one period
- **THEN** the section invocations are delayed by the shared rate limit state, not
  bypassed per step

#### Scenario: Stop inside a layered section stops the route

- **GIVEN** a layered section whose steps include a stop step followed by another step
- **WHEN** the route runs an exchange that reaches the stop step
- **THEN** the route outcome is stopped and the step after the stop, inside the section
  and downstream of it, never runs

#### Scenario: Stopped exchange re-marked even when CamelStop was false

- **GIVEN** a layered section whose inner pipeline ends with a stopped outcome and the
  exchange property `CamelStop` set to false by an earlier step
- **WHEN** the stopped outcome crosses the Tower-result boundary of the wrap
- **THEN** the exchange leaving the wrap carries `CamelStop` true and the route stops

### Requirement: Builder hook for whole-route layers

The system SHALL provide `RouteBuilder::route_layer(layer)` which wraps the entire
compiled route pipeline with the given layer at build time, and repeated calls SHALL
nest so the last-added layer is the outermost. The wrapped route behaves as one retry
unit for the route error handler: a step failure inside the route causes the handler to
retry the whole wrapped route.

#### Scenario: Whole-route concurrency cap

- **GIVEN** a route with `route_layer(ConcurrencyLimitLayer::new(n))` and several steps
- **WHEN** more than `n` exchanges are in flight inside the route simultaneously
- **THEN** at most `n` exchanges are inside the route steps at any moment

#### Scenario: Repeated route layers nest outermost-last

- **GIVEN** a route with `route_layer(a).route_layer(b)`
- **WHEN** an exchange enters the route
- **THEN** `b` wraps `a`, and `a` wraps the step pipeline

#### Scenario: Error handler retries the whole wrapped route

- **GIVEN** a route with `route_layer(layer)` and a configured route error handler with
  a retry policy
- **WHEN** a mid-route step fails
- **THEN** the handler retries the whole wrapped route (all steps from the start), not
  only the failed step

### Requirement: Canonical spec rejects layered steps

The system SHALL reject canonical-spec mapping of layered steps with an explicit
`CamelError::RouteError` naming the step, so layered routes fail canonicalization
loudly instead of silently dropping the middleware.

#### Scenario: Layered step canonicalization rejected

- **GIVEN** a route containing `.to_layered(...)` or `.layered(...)` or
  `.route_layer(...)` steps
- **WHEN** `build_canonical()` runs
- **THEN** it returns `CamelError::RouteError` naming the layered step as not supported
  in canonical v2

### Requirement: Middleware probe evidence

The system SHALL ship an integration-style probe battery over a mock producer proving:
`Timeout` surfaces the expected timeout-classified `CamelError` near the configured
deadline without firing early; `ConcurrencyLimit` strictly enforces the cap under 100
concurrent exchanges; `RateLimit` combined with `Buffer` shares its state across
processor clones. Measured growth ratios SHALL stay inside the envelope: concurrency
ratio at most 1.15 against the configured limit, timeout ratio at most 1.25 against the
configured deadline, and rate-limit delay ratio at most 1.25 against the configured
boundary. The probe battery SHALL be reproducible from the pre-design compile probe
evidence recorded in design.md.

#### Scenario: Concurrency envelope under 100 concurrent exchanges

- **GIVEN** a layered producer with `ConcurrencyLimitLayer::new(n)` driven by 100
  concurrent exchanges
- **WHEN** the probe measures the maximum number of in-flight producer invocations
- **THEN** the measured maximum is at most `n` (strict enforcement) and the measured
  maximum divided by `n` is at most 1.15

#### Scenario: Timeout envelope without early firing

- **GIVEN** a layered producer with `TimeoutLayer` configured for `t` and a producer
  that needs longer than `t`
- **WHEN** the probe measures the elapsed time until the timeout-classified `CamelError`
  surfaces, with the clock starting at the exchange entering the layered processor
- **THEN** the surfaced error is timeout-classified, the measured elapsed time is at
  least `t` (no early firing), and at most `1.25 * t`

#### Scenario: Rate limit state shared through Buffer

- **GIVEN** a `RateLimit` layer allowing `r` requests per period `p` around a producer,
  buffered, cloned into several concurrent handles, and a burst of `k` requests where
  `k > r`
- **WHEN** the burst is sent through the clones concurrently, with the clock starting at
  the first request arriving at the layered service
- **THEN** all clones share one rate budget and the batch takes at least
  `floor((k - 1) / r) * p` (the required delay for the `k - r` requests beyond the first
  burst, the denominator of the ratio) and at most `1.25 *` that required delay

### Requirement: Custom middleware documentation

The system SHALL document the middleware hooks in a "Custom middleware" guide that
includes snippets from a runnable example crate using the repo's include conventions,
and every new public API item SHALL carry rustdoc doctests that compile and run under
`cargo test`.

#### Scenario: Guide compiles under mdbook test

- **GIVEN** the "Custom middleware" guide page registered in the book summary, with
  example snippets included via `{{#include}}` fences tagged `rust,ignore`
- **WHEN** `mdbook test` runs on the docs tree
- **THEN** the run exits zero: every fence is either an explicitly ignored include
  (verified separately through the standalone example crate below) or a fence that
  compiles under rustdoc, and no fence is an untagged non-Rust block

#### Scenario: Example crate compiles standalone

- **GIVEN** the `middleware-layer` example crate whose snippets the guide includes
- **WHEN** `cargo check -p middleware-layer` runs
- **THEN** the crate compiles without warnings from this change

#### Scenario: API doctests compile and run

- **GIVEN** rustdoc doctests on `ProcessorLayer`, the builder hooks, and
  `From<BoxError> for CamelError`
- **WHEN** `cargo test -p camel-api --doc` and `cargo test -p camel-builder --doc` run
- **THEN** all doctests pass

