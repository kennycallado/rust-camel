## ADDED Requirements

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
