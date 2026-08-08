## ADDED Requirements

### Requirement: fail-fast error trigger

The `MockEndpointInner` SHALL expose a `trigger_fail_fast(&self, error:
CamelError)` method that sets the internal `fail_fast_error` field to
`Some(error)`. The `fail_fast_error` field is a **presence sentinel**:
the stored `CamelError` value is NOT propagated to callers — the
`MockProducer` returns a fixed reject message
(`"mock endpoint in fail-fast mode: a previous exchange caused an
error"`) regardless of which error was supplied to `trigger_fail_fast`.
When `fail_fast` is `true`, all subsequent `MockProducer` `poll_ready`
and `call` invocations (after the trigger is set) SHALL return that
fixed error.

#### Scenario: trigger_fail_fast rejects subsequent producer calls

- **GIVEN** a `MockEndpoint` created with `fail_fast: true`
- **WHEN** `inner.trigger_fail_fast(CamelError::ProcessorError("boom"))` is called, then a `MockProducer` processes an exchange
- **THEN** the producer returns `Err(CamelError::ProcessorError(...))` containing the fixed string "fail-fast mode" (the supplied "boom" is NOT in the message — it is a presence sentinel only)

#### Scenario: trigger_fail_fast is no-op when fail_fast flag is false

- **GIVEN** a `MockEndpoint` created with `fail_fast: false`
- **WHEN** `inner.trigger_fail_fast(error)` is called, then a `MockProducer` processes an exchange
- **THEN** the producer returns `Ok(exchange)` (the error is set but never checked because `fail_fast` is false)

#### Scenario: reset clears trigger_fail_fast error

- **GIVEN** a `MockEndpointInner` with `fail_fast_error` set to `Some(error)` via `trigger_fail_fast`
- **WHEN** `inner.reset()` is called, then a `MockProducer` processes an exchange
- **THEN** the producer returns `Ok(exchange)` (the error was cleared by `reset`)

### Requirement: assert_satisfied sets fail_fast_error on mismatch

When `fail_fast` is `true`, `MockEndpointInner::assert_satisfied()` SHALL
set `fail_fast_error` to `Some(error)` before panicking on any
expectation mismatch (wrong body count, missing body, missing header,
header regex mismatch). This ensures that a producer's **next**
`poll_ready` or `call` invocation on a concurrent task sees the error
and rejects. Note: an exchange already past the in-`call` error check
(lib.rs lines ~633-641) when the error is set may still record `Ok`;
the guarantee applies to subsequent invocations only.

#### Scenario: body count mismatch sets fail_fast_error

- **GIVEN** a `MockEndpoint` created with `fail_fast: true` and an expectation of 2 bodies
- **WHEN** 1 exchange is sent and `assert_satisfied()` is called in a `std::panic::catch_unwind` block
- **THEN** the panic is caught AND `inner.fail_fast_error()` returns `Some`

#### Scenario: body mismatch sets fail_fast_error

- **GIVEN** a `MockEndpoint` created with `fail_fast: true` and an expectation of `Body::Text("expected")`
- **WHEN** an exchange with `Body::Text("actual")` is sent and `assert_satisfied()` is called in a `std::panic::catch_unwind` block
- **THEN** the panic is caught AND `inner.fail_fast_error()` returns `Some`

#### Scenario: assert_satisfied does not set error when fail_fast is false

- **GIVEN** a `MockEndpoint` created with `fail_fast: false` and an unmet expectation
- **WHEN** `assert_satisfied()` is called in a `std::panic::catch_unwind` block
- **THEN** the panic is caught AND `inner.fail_fast_error()` returns `None` (fail_fast disabled)

### Requirement: clone_body preserves Body::Stream

The `clone_body` helper function SHALL preserve `Body::Stream` variants
by cloning the inner `StreamBody` (Arc-shared, single-consumption),
instead of silently dropping them to `Body::Empty`.

#### Scenario: clone_body preserves a streaming body

- **GIVEN** an exchange with `Body::Stream(StreamBody { ... })`
- **WHEN** the mock producer processes it with `copy_on_exchange: true`
- **THEN** the recorded exchange's body matches `Body::Stream(_)` (assert via `matches!(recorded.input.body, Body::Stream(_))`, NOT equality, because `Body: PartialEq` has no `Stream` arm — two streams are never equal)

#### Scenario: cloned stream body shares the Arc handle

- **GIVEN** an exchange with `Body::Stream(StreamBody { stream: arc, ... })` where `arc` contains `Some(stream)`
- **WHEN** `clone_body` clones the body
- **THEN** the clone's `stream` field is `Arc::clone` of the original (same `Arc` pointer), so the first consumer to call `into_bytes` drains it and the second gets `CamelError::AlreadyConsumed`
