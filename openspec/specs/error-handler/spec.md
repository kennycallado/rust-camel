# error-handler Specification

## Purpose
TBD - created by archiving change handledby. Update Purpose after archive.
## Requirements
### Requirement: clause-level handled_by delegation

An `error_handler.on_exceptions` clause SHALL accept `handled_by` at the
clause level (beside `kind`, `message_contains`, `handled`, `continued`,
`retry`, `steps`). The `retry` block SHALL be optional and independent of
delegation: delegation is a disposition, not a retry policy. `retry` and
`handled_by` SHALL compose — the failing step is retried first, and the
delegate runs once retries are exhausted (Apache Camel
`onException().maximumRedeliveries().handled(true).to()` parity,
ADR-0019). With no `retry` block, the step SHALL run exactly once and
then delegate; no redelivery SHALL occur and the `CamelRedelivered`
header SHALL NOT be set. `max_attempts == 0` SHALL remain rejected. The
top-level `error_handler.retry` block SHALL NOT accept `handled_by` —
`dead_letter_channel` is the catch-all delegate.

`handled_by` without `handled` or `continued` SHALL behave as a tap: the
delegate receives the failed exchange as a side effect, and the original
error propagates (disposition `Propagate`).

#### Scenario: (a) zero-retry delegation runs once then delegates

- **GIVEN** a route with `error_handler.on_exceptions: [{kind: "Io",
  handled: true, handled_by: "direct:shaper"}]` and no `retry` block
- **WHEN** a step fails once with `Io("boom")`
- **THEN** the step is executed exactly once (no re-invocation), the
  `direct:shaper` delegate receives the failed exchange, and the
  exchange carries NO `CamelRedelivered` header
- **AND** the pipeline completes with the delegate's output as the final
  result (`Completed`, error cleared)

#### Scenario: (b) retry composes with handled_by

- **GIVEN** a route with `error_handler.on_exceptions: [{kind: "Io",
  handled: true, retry: {max_attempts: 2, ...}, handled_by:
  "direct:shaper"}]`
- **WHEN** a step fails on every attempt
- **THEN** the step is executed exactly three times total (the original
  execution plus 2 redeliveries), and the delegate is invoked exactly
  once after retries exhaust
- **AND** the delegated exchange carries `CamelRedelivered: true`,
  `CamelRedeliveryCounter: 2`, and `CamelRedeliveryMaxCounter: 2`

#### Scenario: (h) handled_by without handled is a tap

- **GIVEN** a route with `error_handler.on_exceptions: [{kind: "Io",
  handled_by: "direct:audit"}]` (no `handled`, no `continued`)
- **WHEN** a step fails with `Io("boom")`
- **THEN** the `direct:audit` delegate receives the failed exchange
- **AND** the original `Io` error propagates: the pipeline outcome is
  `Failed` with the original error kind

### Requirement: delegate failure propagates the original error

When the `handled_by` delegate (or the dead-letter channel) itself fails
— its `ready()` or `call()` returns an error — the route error handler
SHALL map that delegate failure to `StepDisposition::Propagate(original
error)` in EVERY step disposition (`handled`, `continued`,
`propagate`). `handle_boundary` returns `Result<Exchange, CamelError>`,
not a step disposition: its delegate-failure arm SHALL return
`Err(original error)`. The pipeline outcome SHALL be `Failed` carrying
the ORIGINAL error kind; it SHALL NEVER be `Completed` when a delegate
has failed. The original error SHALL remain the main error so kind
matching and HTTP status mapping stay business-tied. The delegate error
SHALL be reported through the log policy as `system-broken` with both
errors structured (original and delegate) and recorded on the span as an
error. No new `CamelError` variant SHALL be introduced for this path.

This rule is what distinguishes `handled_by` from `do_try/catch`: a
failing `do_try` catch block propagates the CATCH error and loses the
original, whereas a failing `handled_by` delegate propagates the
ORIGINAL error. `do_try` semantics remain unchanged (tracked rc-zgbqq).

#### Scenario: (c) failed delegate with handled true fails with the original kind

- **GIVEN** a route with `error_handler.on_exceptions: [{kind: "Io",
  handled: true, handled_by: "direct:broken-shaper"}]` where the delegate
  endpoint fails on invocation
- **WHEN** a step fails with `Io("original failure")`
- **THEN** the pipeline outcome is `Failed` with the ORIGINAL `Io` error
  (not `Completed`, and not the delegate's error kind)
- **AND** a `system-broken` log record carries both the original and the
  delegate error structured, and a span error records the delegate error

#### Scenario: (d) failed delegate with continued fails with the original kind

- **GIVEN** the same route as scenario (c) but with `continued: true`
  instead of `handled: true`
- **WHEN** a step fails and the delegate then fails
- **THEN** the pipeline outcome is `Failed` with the original error kind
  (the failure is not absorbed by `continued`)

#### Scenario: (e) failed delegate at the security or circuit boundary

- **GIVEN** a route whose boundary gate (security denial or circuit
  breaker rejection) routes to a `handled_by` delegate that fails
- **WHEN** the boundary error is handled through `handle_boundary` and
  the delegate fails
- **THEN** `handle_boundary` returns `Err` carrying the ORIGINAL boundary
  error, and the pipeline outcome is `Failed` with the original error
  kind, under the same original-error-wins rule as in-pipeline step
  failures

### Requirement: typed layout conflicts and hard load errors

An `on_exceptions` clause that sets BOTH `steps` and `handled_by` SHALL
be rejected at load time with a typed `ConfigValidationError` variant
(never via message-text matching). The legacy layout `retry: {handled_by:
...}` SHALL be a HARD load error in both the YAML and the JSON route
formats — the redelivery models SHALL use `deny_unknown_fields` so the
removed field is rejected as unknown, never silently ignored.

#### Scenario: (f) steps plus handled_by is a typed rejection

- **GIVEN** a route with an `on_exceptions` clause carrying both
  non-empty `steps` and `handled_by`
- **WHEN** the route loads
- **THEN** loading fails with the typed `ConfigValidationError` variant
  for the steps/handled_by conflict

#### Scenario: (g) legacy retry handled_by layout is a hard load error

- **GIVEN** a YAML route with `on_exceptions: [{kind: "*", handled: true,
  retry: {max_attempts: 1, handled_by: "direct:shaper"}}]`
- **WHEN** the route loads
- **THEN** loading fails with an unknown-field error naming `handled_by`
  inside the retry block (the field no longer exists there)
- **GIVEN** the equivalent JSON route with
  `"retry": {"max_attempts": 1, "handled_by": "direct:shaper"}`
- **WHEN** the route loads
- **THEN** loading fails the same way

