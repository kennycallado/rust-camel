# error-handler Specification

## Purpose
Route error-handler behavior. `on_exceptions` clauses accept clause-level
`handled_by` delegation, delegate failures propagate the original error in
every disposition, conflicting layouts fail at load with a typed error, and
`do_try` catch failures keep the catch error.
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

### Requirement: opt-in original-message restoration before dead-letter dispatch

When a route's `error_handler` sets `use_original_message: true`, the
system SHALL snapshot the inbound message before route processing and
restore that original message before dispatching the failed exchange to
the dead-letter channel (or a matched `on_exceptions` clause's
`handled_by` delegate). The YAML DSL SHALL carry the flag from the
document through `DeclarativeErrorHandler` into the compiled
`ErrorHandlerConfig`, so the restoration the runtime already implements
becomes reachable from YAML routes. When the flag is unset the behavior
SHALL stay opt-out: the dead-letter channel receives the exchange as
mutated by route processing (no snapshot, no restoration). The flag
SHALL survive every `error_handler` config shape: `dead_letter_channel`
only, top-level `retry` only, and `on_exceptions` clauses. Exception:
when a route step replaces the Exchange object wholesale, the stashed
snapshot is not carried and restoration cannot occur (a documented,
test-enforced limitation of the runtime snapshot mechanism); for that
exchange the flag behaves as if unset.

#### Scenario: YAML use_original_message true routes the original body to the DLC

- **GIVEN** a YAML route where a producer route sends `ORIGINAL-EVENT`
  to a consumer route whose `error_handler` sets
  `dead_letter_channel: "mock:dlc"` and `use_original_message: true`,
  and whose steps set the body to `MUTATED` and then fail
- **WHEN** the input is processed and the step failure is handled
- **THEN** `mock:dlc` receives exactly one exchange whose body is
  `ORIGINAL-EVENT` (the pre-mutation message, not the mutated one)

#### Scenario: default (flag unset) routes the mutated body to the DLC

- **GIVEN** the same YAML route but WITHOUT `use_original_message`
- **WHEN** the step failure is handled
- **THEN** `mock:dlc` receives exactly one exchange whose body is
  `MUTATED` (opt-out behavior is preserved)

#### Scenario: the flag survives the dead-letter-channel-only shape

- **GIVEN** a `DeclarativeErrorHandler` with only `dead_letter_channel`
  set and `use_original_message: true`
- **WHEN** the handler is compiled
- **THEN** the resulting `ErrorHandlerConfig` carries
  `use_original_message == true`

#### Scenario: the flag survives the retry-only shape

- **GIVEN** a `DeclarativeErrorHandler` with only a top-level `retry`
  policy and `use_original_message: true`
- **WHEN** the handler is compiled
- **THEN** the resulting `ErrorHandlerConfig` carries
  `use_original_message == true` together with the retry policy

#### Scenario: the flag survives the on_exceptions shape

- **GIVEN** a `DeclarativeErrorHandler` with `on_exceptions` clauses and
  `use_original_message: true`
- **WHEN** the handler is compiled
- **THEN** the resulting `ErrorHandlerConfig` carries
  `use_original_message == true` together with the compiled exception
  policies

#### Scenario: the default is false when the flag is absent

- **GIVEN** any `DeclarativeErrorHandler` shape without the flag
- **WHEN** the handler is compiled
- **THEN** the resulting `ErrorHandlerConfig` carries
  `use_original_message == false`

### Requirement: business error envelope and variant

`CamelError` SHALL gain a `Business` variant carrying
`{ code: String, kind: String, message: String, data: Option<RedactedJson> }`.
`code` and `kind` SHALL be non-empty at construction. `message` is a
log-and-diagnostics field: it SHALL NOT reach an HTTP client — no mapping,
disposition, or configuration authorizes its disclosure. `data` carries
optional structured diagnostics and SHALL NOT reach an HTTP client and SHALL
NOT appear in log records. `data` SHALL be typed as a redacting newtype over
`serde_json::Value` whose `Debug` implementation prints a redaction marker and
never the payload, so accidental structured logging cannot leak exchange
data; the newtype preserves `PartialEq`/`Eq` through the inner value.
`classify()` SHALL return `"business"` and `variant_name()` SHALL return
`"Business"` for the variant, so the variant joins the exception-kind
vocabulary and the catch-by-variant matcher. The variant addition is additive:
the enum stays `#[non_exhaustive]`, and clause types keep their existing
derives. The variant's fields are plain public strings: constructors and
compilers that build the variant SHALL reject blank `code` or blank `kind` at
their own boundaries (the `throw` step validates at load time), and no runtime
guarantee beyond that boundary is made. The variant's rustdoc SHALL document
this constructor contract (non-blank `code`/`kind`, `message` log-only,
`data` redacted).

#### Scenario: classify and variant_name name the business variant

- **GIVEN** a `CamelError::Business` value with code, kind, message, and data set
- **WHEN** `classify()` and `variant_name()` run
- **THEN** they return `"business"` and `"Business"` respectively

#### Scenario: the vocabulary guard classifies the business variant

- **GIVEN** the classification test table enumerating every `CamelError` variant
- **WHEN** the `Business` variant is checked
- **THEN** the table classifies it as matchable under the `Business` kind value

#### Scenario: Debug on the data newtype redacts the payload

- **GIVEN** a `Business` error whose `data` holds structured values
- **WHEN** the error or the data field is formatted with `{:?}` into any string
- **THEN** the output contains a redaction marker and none of the data values

### Requirement: on_exceptions code matching

An `error_handler.on_exceptions` clause SHALL accept an optional `code` field.
The clause SHALL match a `CamelError::Business` error when the clause's `code`
equals the error's `code`, conjunctively with the existing `kind` and
`message_contains` matchers (this is a conjunction, not the `doTry`
exactly-one-main-matcher grammar). A clause with `code` set SHALL NOT match any
non-`Business` error. First-match-wins clause order SHALL stay unchanged. A
clause that matches a business error SHALL run its disposition (handled,
continued, handled_by, steps) exactly once; the redelivery policy SHALL NOT
redeliver it (see "business error observability and failure policy"). The new
Simple paths SHALL add no additional exchange properties; the exception
properties `Exchange::set_error()` already derives (including
`CamelExceptionKind`) keep their existing population and clearing semantics.

#### Scenario: code matches the business error

- **GIVEN** a route with `on_exceptions: [{code: "ORDER_NOT_FOUND", handled: true}]`
- **WHEN** a step fails with `Business { code: "ORDER_NOT_FOUND", .. }`
- **THEN** the clause matches, the error is handled, and the pipeline completes

#### Scenario: code does not match a different business code or a system error

- **GIVEN** the same route
- **WHEN** a step fails with `Business { code: "OTHER_CODE", .. }` and, in a
  second request, with `Io("disk")`
- **THEN** neither error matches the clause; the `Io` error keeps the
  unmatched-error path and the other business error propagates or matches a
  later clause by first-match-wins order

#### Scenario: code combines with kind by conjunction

- **GIVEN** a clause `[{kind: "Business", code: "ORDER_NOT_FOUND", handled: true}]`
- **WHEN** a step fails with `Business { code: "ORDER_NOT_FOUND", kind: "not-found", .. }`
- **THEN** the clause matches
- **WHEN** a step fails with `Io("disk")`
- **THEN** the clause does not match, because `kind: "Business"` excludes non-business variants

### Requirement: Simple exception paths

The Simple language SHALL parse and evaluate `${exception.code}`,
`${exception.kind}`, `${exception.business_kind}`, and
`${exception.data.<field>}`, where `<field>` is a dot path traversing nested
`data` objects. The paths read the exchange error present at evaluation time,
preserving the canonical catch-error visibility: the error is visible inside a
`doTry` catch body and gone after a `Handled` catch clears it. Resolution
rules: `${exception.code}` resolves to the business `code` when the error is
`Business`, else to empty; `${exception.kind}` resolves to the `classify()`
name of the present error — the `CamelExceptionKind` value — for business and
non-business errors alike, else to empty (owner decision 2026-11-12: `kind`
names the variant classification, never the payload category); 
`${exception.business_kind}` resolves to the business payload `kind` when the
error is `Business`, else to empty; a missing, null, or non-traversable data
field resolves to empty; a non-string data value renders per the existing
scalar rules (ADR-0092). No exchange property SHALL be added for the error;
the exception properties `Exchange::set_error()` already derives keep their
existing population and clearing semantics. The `${exception}` and
`${exception.message}` paths keep their existing semantics. The
`${exception.kind}` parse support is owned by bd `rc-7qv90`, which is a
prerequisite of the implementation mission; this change adds
`business_kind`, `code`, and `data` paths on top of it.

#### Scenario: business error resolves code, classification kind, payload kind, and nested data

- **GIVEN** an exchange whose error is `Business { code: "ORDER_NOT_FOUND",
  kind: "not-found", data: {"order": {"id": "A-1"}} }`
- **WHEN** Simple evaluates `${exception.code}`, `${exception.kind}`,
  `${exception.business_kind}`, and `${exception.data.order.id}`
- **THEN** they resolve to `ORDER_NOT_FOUND`, `business`, `not-found`, and `A-1`

#### Scenario: non-business error resolves classify name and empty business paths

- **GIVEN** an exchange whose error is `Io("disk")`
- **WHEN** Simple evaluates `${exception.kind}`, `${exception.code}`, and
  `${exception.business_kind}`
- **THEN** they resolve to `io`, empty, and empty

#### Scenario: no error resolves empty

- **GIVEN** an exchange with no error
- **WHEN** Simple evaluates `${exception.code}`, `${exception.kind}`,
  `${exception.business_kind}`, and `${exception.data.any}`
- **THEN** all four resolve to empty and evaluation does not fail

#### Scenario: missing and non-traversable data fields resolve empty

- **GIVEN** an exchange whose error is `Business { code: "X", kind: "failed", data: {"a": {"b": 1}} }`
- **WHEN** Simple evaluates `${exception.data.zzz}`, `${exception.data.a.c}`,
  and `${exception.data.a.b}`
- **THEN** the first two resolve to empty and the third renders the scalar `1`

#### Scenario: catch visibility follows the canonical clearing contract

- **GIVEN** a doTry whose body throws a business error and whose matching
  `Handled` catch body evaluates `${exception.code}` then completes
- **WHEN** the catch body runs and the block exits
- **THEN** `${exception.code}` resolves inside the catch body and the exchange
  after the block carries no error, so a later step evaluating
  `${exception.code}` resolves empty

### Requirement: business error observability and failure policy

Business error handling SHALL apply a global policy, not a per-kind or per-code
configuration. The policy:

- Logs: a `Business` error SHALL log at debug when raised; at info when a
  clause, a catch, or a `handled_by`/send-to-handler delegate handles it. For
  a propagated business error, logging is transport-owned and never doubled:
  on seam routes (mapping present) the error handler logs at debug only, and
  the HTTP finalizer logs at info when the effective mapped status is below
  500 and at warn at 500-or-above (the built-in 500 closure counts as
  500-or-above); on non-seam routes (no mapping) the error handler logs the
  unmatched propagate at warn. Unhandled business errors on non-HTTP
  transports (no mapping, no dead-letter dispatch skip) surface through the
  dead-letter or failure path and log at warn. Log records SHALL carry `code`
  and `kind` structured and SHALL NOT serialize `data`.
- Redelivery: the declarative redelivery engine SHALL NOT redeliver a
  `Business` error — clause-level retry blocks and the route-level retry block
  produce zero redeliveries, the step runs once, the matching disposition
  still runs, and the `CamelRedelivered` header SHALL NOT be set. This
  governs declarative route redelivery only; broker-level client redelivery
  (a messaging client re-delivering after consumer failure) is a transport
  concern and is NOT governed or changed by this policy.
- Dead-letter bypass is CONDITIONAL on the HTTP reply seam (owner-accepted C1
  correction), and the bypass covers the SYNTHETIC catch-all policy too: the
  route compiler inserts a synthetic `Handled` catch-all for dead-letter-only
  configurations (`route_compiler_ext.rs:77-86`), and suppressing its
  dead-letter producer still clears the error (`error_handler.rs:349-353`) —
  so on seam routes (route-level `http_error_mapping: Some(_)`, which rest
  lowering always sets, with empty tables when the block declares none) a
  `Business` error SHALL be excluded from that synthetic policy as well as
  from the default dead-letter channel, and SHALL propagate toward the
  transport so mapping applies. The bypass also covers a matched clause
  without `handled_by`, which otherwise falls back to the dead-letter channel
  (`policy_producer.or(dlc)`, `error_handler.rs:179`). A business error SHALL
  never reach the no-error-handler `error!` terminal site
  (`error_handler.rs:710`). On non-seam routes (no mapping field), business
  errors keep dead-letter dispatch per the existing semantics — Apache Camel
  parity: `throwException` lands in the dead-letter channel when one is
  configured — and a successfully handled (cleared) error may complete the
  pipeline and answer 2xx on HTTP; that is the documented error-handler
  outcome, outside the finalizer's never-2xx guarantee. Explicit
  `on_exceptions` clauses (handled, handled_by, steps, continued) still catch
  business errors exactly as configured on every route.
- No-handler routes: when the route compiler installs no error handler at all
  (`route_compiler_ext.rs:260-274`, configuration absent), no error-handler
  log site exists; failure logging for business errors on such routes is
  owned by the transport's existing failure logging (out of this epic's
  scope), and the HTTP finalizer still maps the failed or error-carrying
  result (never 2xx). The error-handler warn propagate record applies only to
  routes with a handler configured whose policy chain produced no producer
  (`forward_or_propagate(ex, None, err)`).
- Circuit breakers: a `Business` error SHALL count as a SUCCESS for both
  circuit breaker implementations — `CircuitBreakerService` and
  `CircuitBreakerGate` — in every state, including half-open: a probe that
  ends in any business error completes as a success and the breaker closes.
  The tracer's error metrics SHALL continue to apply the existing
  `CircuitOpen`-style exclusion for business failures; the tracer is
  observability and is not a breaker implementation.

#### Scenario: raise logs at debug with data redacted

- **GIVEN** a route whose `throw` step carries `code`, `kind`, `message`, and `data`
- **WHEN** the step executes and fails
- **THEN** a debug-level log record carries `code` and `kind` structured, and no
  log record contains the `data` payload

#### Scenario: clause-handled business error logs at info without data

- **GIVEN** a route whose clause `[{code: "X", handled: true}]` catches a business error
- **WHEN** the clause handles the error
- **THEN** an info-level log record carries `code` and `kind`, and `data` is absent from the record

#### Scenario: handled_by delegate path logs at info without data

- **GIVEN** a route whose clause `[{code: "X", handled_by: "direct:shaper"}]`
  delegates a business error
- **WHEN** the delegate runs
- **THEN** an info-level log record carries `code` and `kind`, and `data` is absent from the record

#### Scenario: mapped client-fault status logs at info on a seam route

- **GIVEN** a mapping that maps the error's kind to 404 (seam route, mapping present)
- **WHEN** an unmatched business error propagates to the HTTP transport
- **THEN** the finalizer's propagate log record is at info level and the error
  handler logged the propagate only at debug (never twice)

#### Scenario: mapped server-fault or unmapped status logs at warn on a seam route

- **GIVEN** a mapping that maps the error's kind to 502, and a second seam
  route with empty tables (no explicit entries)
- **WHEN** an unmatched business error propagates to the HTTP transport on
  each route
- **THEN** both finalizer propagate log records are at warn level (the
  explicit 502 and the built-in 500 closure are both 500-or-above)

#### Scenario: non-seam route propagate logs at warn

- **GIVEN** a non-HTTP consumer route with no mapping field and no dead-letter
  channel, whose step fails with a business error
- **WHEN** the unmatched error propagates
- **THEN** the error handler's propagate log record is at warn level

#### Scenario: clause retry never redelivers a business error

- **GIVEN** a route with `on_exceptions: [{code: "X", retry: {max_attempts: 3}, handled: true}]`
- **WHEN** a step fails with `Business { code: "X", .. }`
- **THEN** the step executes exactly once, no redelivery occurs, the
  `CamelRedelivered` header is absent, and the clause disposition still runs

#### Scenario: route-level retry never redelivers a business error

- **GIVEN** a route with a route-level `retry` block and no matching clause
- **WHEN** a step fails with a business error
- **THEN** the step executes exactly once and no redelivery occurs

#### Scenario: business errors skip the default dead-letter channel only on seam routes

- **GIVEN** a seam route with `error_handler` carrying `dead_letter_channel:
  "mock:dlc"` AND a route-level mapping (`kinds: {"not-found": 404}`), and a
  second non-seam route with only `dead_letter_channel: "mock:dlc"`
- **WHEN** a step fails with a business error on each route, and in a third
  exchange with `Io("disk")` on the non-seam route
- **THEN** the seam-route business error does NOT reach `mock:dlc` and
  propagates toward the transport (mapped reply), while the non-seam-route
  business error IS forwarded to `mock:dlc`, and the `Io` error IS forwarded
  to `mock:dlc`

#### Scenario: seda route with a dead-letter channel receives unmatched business errors

- **GIVEN** an InOnly `seda:jobs` consumer route with
  `dead_letter_channel: "mock:dlc"` and no mapping field (non-seam route)
- **WHEN** the consumer's step fails with an unmatched business error
- **THEN** `mock:dlc` receives the exchange (Apache Camel parity:
  `throwException` lands in the dead-letter channel) and the producer is not
  affected

#### Scenario: both breaker implementations count a business error as success

- **GIVEN** routes guarded by `CircuitBreakerService` and by `CircuitBreakerGate`, each with failure threshold 1
- **WHEN** a step fails with `Business { .. }` on each
- **THEN** neither breaker opens; the business error is recorded as a success by both

#### Scenario: half-open probe closes on a business error for both implementations

- **GIVEN** `CircuitBreakerService` and `CircuitBreakerGate` each held in
  half-open by prior system failures
- **WHEN** a probe request ends in a business error (handled or propagated,
  any mapping)
- **THEN** each breaker records the probe as a success and closes

#### Scenario: tracer skips the error increment for business failures

- **GIVEN** a route whose step fails with a business error
- **WHEN** the tracer records the step outcome
- **THEN** the error metric is not incremented by the business failure; an `Io`
  failure still increments it

### Requirement: business error propagation across dispatch boundaries

A `Business` error SHALL follow each transport's existing delivery-mode
contract. `direct:` SHALL propagate the error unchanged to the producing
exchange in every mode, so the caller's catch clauses, `on_exceptions`, and
HTTP mapping see it. For fire-and-forget delivery — an InOnly `seda:` or
broker send whose completion the producer does not wait for — the consumer
SHALL own the failure: the consumer route's matched clauses apply, and the
producing call SHALL NOT observe the error. Request-reply and waiting modes
keep their existing contracts: SEDA InOut and `waitForTaskToComplete=Always`
SHALL deliver the consumer-side result, including a business error, to the
producer; a broker InOut producer whose consumer fails unhandled observes its
existing reply-timeout contract, not the business error itself. This mission
defines no new transport policy; broker-level client redelivery after an
unhandled consumer failure is unaffected (see "business error observability
and failure policy").

#### Scenario: direct dispatch propagates the business error to the caller

- **GIVEN** routes `direct:start` with a `doTry` whose body calls `direct:inner`,
  and `direct:inner` whose `throw` step raises `code: "ORDER_NOT_FOUND"`, with a
  catch clause matching that code
- **WHEN** `direct:start` runs
- **THEN** the catch clause in the caller matches, because the error propagated unchanged

#### Scenario: fire-and-forget seda consumer owns the failure and the producer does not see it

- **GIVEN** a `seda:jobs` consumer route with
  `on_exceptions: [{code: "ORDER_NOT_FOUND", handled: true, handled_by: "direct:dlq-shim"}]`
  and a producer route that sends InOnly to `seda:jobs` and then sets a marker
  in its body
- **WHEN** the consumer route's `throw` step raises the error
- **THEN** the consumer-side clause runs and the error does not surface in the
  producer's exchange, which completes normally

#### Scenario: waiting seda delivery propagates the consumer result

- **GIVEN** an InOut send to `seda:jobs` (or `waitForTaskToComplete=Always`)
  whose consumer route's `throw` step raises the error
- **WHEN** the producer waits for completion
- **THEN** the producer observes the consumer-side failure per the existing
  waiting-delivery contract, which the business error follows unchanged

