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
failing `do_try` catch block keeps the CATCH error as the main error
so exception translation inside the catch block still reaches kind
matching and HTTP status mapping, whereas a failing `handled_by`
delegate propagates the ORIGINAL error as the main error because the
delegate is infrastructure, not route code. The `do_try` catch-block
failure envelope (how the original error stays observable) is specified
by the requirement "do_try catch-block failure envelope".

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

### Requirement: do_try catch-block failure envelope

When a `do_try` catch clause body itself fails, the system SHALL keep
the CATCH error as the main error in every disposition (`handled`,
`propagate`; `continued` is rejected at parse time). "Main error"
means all three observable results use the unwrapped catch error: the
value returned by the do_try processor, the error carried by the
`Failed` pipeline outcome, and the error used by route-level
`on_exceptions` kind matching and HTTP status mapping — so exception
translation inside a catch block works. The original error SHALL NOT
be discarded: the failure SHALL emit a `warn`-level log record
carrying both errors structured (`original_error` and `catch_error`,
message "do_try catch block failed; catch error supersedes original").
The log record is unconditional. When a span is active, the failure
SHALL also add an event with the `original_error` attribute to that
span and record the catch error on the span as an error; when no span
is active, the log record is the only additional surface. No exchange
property SHALL be added for the original error, because the exchange
does not leave the service on the failure path. No new `CamelError`
variant or field SHALL be introduced for this path; the catch error
SHALL be returned unwrapped. doFinally interplay SHALL stay as it
exists per runtime path: the builder-service path runs finally with
the catch error as the previous error (and restores the catch error if
finally also throws, logging both), while the compiled segment path
skips finally after a failed catch body (ADR-0025 invariant #4).

#### Scenario: translation route matches the catch error kind

- **GIVEN** a compiled route whose `do_try` body fails with an `Io`-kind
  error and whose matching catch clause body raises a domain-kind error
  (for example through a `throw_exception` step), and whose route-level
  `error_handler.on_exceptions` has one clause matching the `Io` kind
  and a different clause matching the domain kind, each with a distinct
  visible effect (disposition, destination steps, or mapped HTTP
  status)
- **WHEN** the catch clause body fails
- **THEN** the pipeline outcome is `Failed` with the catch error kind,
  and the returned error is the catch error
- **AND** the route-level clause that fires is the one matching the
  catch error kind (not the `Io` clause), and the mapped HTTP status
  derives from the catch error kind

#### Scenario: original error surfaces in log and span

- **GIVEN** a route whose `do_try` body fails with an original error and
  whose matching catch clause body then fails with a catch error
- **WHEN** the catch clause body fails
- **THEN** a `warn`-level log record carries `original_error` and
  `catch_error` structured
- **AND** when a span is active, that span carries an event with the
  `original_error` attribute and an error field recording the catch
  error

#### Scenario: envelope is disposition-independent

- **GIVEN** a `do_try` with a matching catch clause configured with the
  `propagate` disposition whose body fails
- **WHEN** the catch clause body fails
- **THEN** the returned error is the catch error (the same envelope as
  the `handled` disposition), and the original error is surfaced through
  the log record and span as above

#### Scenario: catch and finally both fail on the builder-service path

- **GIVEN** a builder-API `do_try` whose catch clause body fails and
  whose finally body also fails
- **WHEN** finally throws with a previous catch error present
- **THEN** the returned error is the catch error (restored over the
  finally error; the finally error never replaces it)
- **AND** a `warn`-level log record carries the `catch_error` and the
  `finally_error` structured — the only surface where the finally error
  appears

