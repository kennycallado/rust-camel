## ADDED Requirements

### Requirement: `tracing-opentelemetry` span layer exports no engine tracing spans

In the supported configuration, the `tracing_opentelemetry` span layer SHALL
be constructed with the library default no-op tracer, and no real tracer
SHALL be wired into it. The engine's own `tracing` spans (`pipeline_step` and
`error_handler` from route execution, `function`, and the `camel_tracer`
`step` twin) SHALL therefore not be exported through that layer. Spans
created through the OpenTelemetry SDK tracer SHALL continue to export: a
route exchange exports its SDK route root and SDK step spans. The `fmt` log
layer SHALL be unaffected and SHALL continue to emit the same log lines.
Code comments SHALL NOT claim that `tracing_opentelemetry::layer()` picks up
the global tracer provider.

#### Scenario: Production-equivalent layer exports no engine tracing spans

- **Given** a subscriber containing `tracing_opentelemetry::layer()` with no
  `.with_tracer(...)` wiring
- **When** the engine emits `pipeline_step` on the route-compiler target,
  `error_handler` on the route-compiler target, `function` on the
  `camel_function` target, and `step` on the `camel_tracer` target
- **Then** none of those spans is exported

#### Scenario: One route exchange exports exactly the SDK root and step spans

- **Given** a real global SDK tracer provider backed by an in-memory exporter
  and a normal route with one exchange run under the production-equivalent
  no-op layer
- **When** the exchange completes
- **Then** the exported span names are exactly the route root and its SDK
  step spans, and no exported span is named `pipeline_step`,
  `error_handler`, `function` or `step`

#### Scenario: fmt layer still emits route log lines

- **Given** the same subscriber includes a `fmt` layer writing to a buffer
- **When** the route executes a log step
- **Then** the buffer contains the route log message
