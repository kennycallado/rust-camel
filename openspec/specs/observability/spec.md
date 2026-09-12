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

