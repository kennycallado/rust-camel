# observability Specification

## Purpose
TBD - created by archiving change steplatency. Update Purpose after archive.
## Requirements
### Requirement: Compiled To steps retain declared URI metadata

The Runtime SHALL retain the declared URI of each `To` step as immutable
compile-time metadata on its compiled process step, while leaving metadata
absent for steps that are not `To` steps.

#### Scenario: To step retains the authored URI

- **GIVEN** a route contains `To("direct:orders")`
- **WHEN** the route is compiled
- **THEN** the corresponding compiled process step exposes `direct:orders` as
  its declared URI metadata

#### Scenario: Non-To step has no URI metadata

- **GIVEN** a route contains a processor or structural step that is not `To`
- **WHEN** the route is compiled
- **THEN** its compiled process step has no declared URI metadata

### Requirement: OTEL exports per-step duration by declared URI

When duration metrics are enabled, the Runtime SHALL record a
`step_duration_secs` histogram for each call-time `To` step with `route` and
`to_uri` labels, using the existing dynamic-label metrics collector contract.
The histogram records elapsed call-time attempts for both successful and failed
processor results, but never records readiness attempts.

#### Scenario: Call-time To duration is recorded

- **GIVEN** duration metrics are enabled and a compiled `To("direct:orders")`
  step completes a call
- **WHEN** the tracing processor records step metrics
- **THEN** it records `step_duration_secs` with labels `route` equal to the
  route identifier and `to_uri` equal to `direct:orders`

#### Scenario: Duration lever disables the new histogram

- **GIVEN** duration metrics are disabled
- **WHEN** a `To` step completes a call
- **THEN** no `step_duration_secs` histogram is recorded

#### Scenario: Readiness failure does not create call-time duration

- **GIVEN** a producer fails during `poll_ready`
- **WHEN** readiness metrics are recorded
- **THEN** no `step_duration_secs` histogram is recorded, while existing
  readiness exchange/error metrics retain their current behavior

### Requirement: Bridge RPC trace context propagation

The Rust bridge client SHALL inject the current camel-otel trace context as
W3C `traceparent` gRPC metadata on every RPC it sends to a Java bridge
(Send, Subscribe, Health), reusing camel-otel propagation, and SHALL NOT
inject the header when no valid trace context exists. Metadata already
carrying a `traceparent` SHALL NOT be overwritten.

#### Scenario: Active context reaches every RPC shape

- **GIVEN** a camel-otel context with a valid span context is current at RPC
  time (ambient for Subscribe/Health, exchange-carried for Send)
- **WHEN** the bridge client issues Send, Subscribe, or Health
- **THEN** each request carries `traceparent` matching
  `00-<32hex trace id>-<16hex span id>-<2hex flags>` with the context's
  trace and span ids

#### Scenario: No-tracing fallback injects nothing

- **GIVEN** no valid camel-otel context exists at RPC time
- **WHEN** any bridge RPC is issued
- **THEN** no `traceparent` metadata is present and the RPC completes with
  existing semantics

#### Scenario: Bridge surfaces the received traceparent

- **GIVEN** a request with `traceparent` metadata arrives at the JMS bridge
- **WHEN** the interceptor processes the call and the service handles it
- **THEN** the bridge logs the received value at info level via
  java.util.logging and the value arrives intact through the bridge service

### Requirement: Bridge structured JSON logs

The Java bridges SHALL emit log records as structured JSON objects on
stdout through the existing java.util.logging-to-stdout pipeline, without
swapping the log framework.

#### Scenario: Log records render as JSON lines

- **GIVEN** a bridge runs with the JSON console formatter enabled
- **WHEN** any log record is emitted
- **THEN** stdout receives a single line that is a valid JSON object
  carrying at least timestamp, level, logger name, and message

#### Scenario: Ready-protocol line stays untouched

- **GIVEN** a bridge starts up
- **WHEN** the PortAnnouncer emits its readiness line
- **THEN** stdout still receives the exact `{"status":"ready",...}` JSON
  object, unaffected by the log formatter

