# mock-component-correctness Specification

## Purpose
TBD - created by archiving change audit-fix-mock-correctness. Update Purpose after archive.
## Requirements
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

### Requirement: count expectations enforced by assert_satisfied

`MockEndpointInner` SHALL expose `expect_count(n: usize)` (exact) and
`expect_minimum_count(n: usize)` (minimum) expectation setters, and
`assert_satisfied` SHALL enforce them: exact means the number of
currently retained exchanges equals `n`; minimum means it is at least
`n`. Count expectations evaluate over the retained snapshot at
assertion time (bounded by the retention limit), not over the total
ever received. When both are set, both are enforced. Count checks run
before body expectation checks, and a count mismatch short-circuits
with a message naming the endpoint, the expected count, and the actual
count.

#### Scenario: exact count mismatch fails assertion

- **GIVEN** a `MockEndpointInner` with `expect_count(3)` set
- **WHEN** 2 exchanges are sent and `assert_satisfied().await` is awaited inside `futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(...))`
- **THEN** the panic is caught and its message contains the endpoint name, "expected 3", and "got 2"

#### Scenario: exact count satisfied passes

- **GIVEN** a `MockEndpointInner` with `expect_count(2)` set
- **WHEN** 2 exchanges are sent and `assert_satisfied().await` completes
- **THEN** no panic occurred

#### Scenario: minimum count satisfied by more exchanges

- **GIVEN** a `MockEndpointInner` with `expect_minimum_count(2)` set
- **WHEN** 5 exchanges are sent and `assert_satisfied().await` completes
- **THEN** no panic occurred

#### Scenario: minimum count violated fails assertion

- **GIVEN** a `MockEndpointInner` with `expect_minimum_count(4)` set
- **WHEN** 1 exchange is sent and `assert_satisfied().await` is awaited inside `AssertUnwindSafe(...).catch_unwind()`
- **THEN** the panic is caught and its message states at least 4 exchanges were expected

#### Scenario: exact and minimum enforced together

- **GIVEN** a `MockEndpointInner` with `expect_count(2)` and `expect_minimum_count(1)` set
- **WHEN** 3 exchanges are sent and `assert_satisfied().await` is awaited inside `AssertUnwindSafe(...).catch_unwind()`
- **THEN** the panic is caught and its message reports the exact-count mismatch (3 received, 2 expected) even though the minimum alone was satisfied

#### Scenario: count mismatch short-circuits before body checks

- **GIVEN** a `MockEndpointInner` with `expect_count(5)` and one `expect_body(...)` set
- **WHEN** 2 exchanges are sent (bodies also mismatching) and `assert_satisfied().await` is awaited inside `AssertUnwindSafe(...).catch_unwind()`
- **THEN** the panic message reports the count mismatch, not a body mismatch (counts are evaluated first)

#### Scenario: count expectation coexists with body expectations

- **GIVEN** a `MockEndpointInner` with `expect_count(2)` and two matching `expect_body(...)` expectations set
- **WHEN** 2 matching exchanges are sent and `assert_satisfied().await` completes
- **THEN** no panic occurred (counts and bodies both satisfied)

#### Scenario: count evaluates over retained exchanges under truncation

- **GIVEN** a `MockComponent` endpoint created with `retain=3` and `expect_count(3)` set on its inner
- **WHEN** 5 exchanges are sent, then `received_count().await` and `assert_satisfied().await` are called
- **THEN** `received_count()` is 3 (retention truncated the oldest) and the assertion passes (count evaluates the retained snapshot)

### Requirement: non-panicking assertion surface

`MockEndpointInner` SHALL expose
`pub async fn try_assert_satisfied(&self) -> Result<(), MockAssertionError>`
performing the same checks as `assert_satisfied` without panicking:
`Ok(())` when all expectations (count, bodies, headers, header
regexes) are satisfied, `Err(MockAssertionError)` on any mismatch or
malformed expectation. `MockAssertionError` SHALL be a
`#[non_exhaustive]` public enum implementing `std::error::Error` and
`Display`, with one variant per assertion branch and named payloads
(body details pre-formatted as strings):
`CountMismatch { endpoint, expected, actual }`,
`MinimumCountNotMet { endpoint, minimum, actual }`,
`BodyCountMismatch { endpoint, expected, actual }`,
`BodyMismatch { endpoint, index, expected, actual }`,
`BodyNotFound { endpoint, expected }`,
`HeaderNotFound { endpoint, key, value }`,
`HeaderRegexNotMatched { endpoint, key, pattern }`, and
`InvalidHeaderPattern { endpoint, key, pattern, source }`.
Every assertion branch maps to exactly one variant: exact count →
`CountMismatch`; minimum count → `MinimumCountNotMet`;
expected-bodies-count differs from received-count →
`BodyCountMismatch`; ordered body index mismatch → `BodyMismatch`;
any-order body not found → `BodyNotFound`; header value not found →
`HeaderNotFound`; header regex not matched → `HeaderRegexNotMatched`;
invalid header regex pattern → `InvalidHeaderPattern`. Its `Display`
output SHALL equal the panic message the panicking variant produces
for the same condition. An invalid header regex pattern SHALL be
reported as `Err` by `try_assert_satisfied` where the current
implementation panics.

#### Scenario: try_assert_satisfied returns Ok when satisfied

- **GIVEN** a `MockEndpointInner` with `expect_count(1)` and one matching `expect_body(...)` set
- **WHEN** 1 matching exchange is sent and `try_assert_satisfied().await` completes
- **THEN** the result is `Ok(())`

#### Scenario: try_assert_satisfied returns Err with details on mismatch

- **GIVEN** a `MockEndpointInner` with `expect_count(2)` set
- **WHEN** 0 exchanges are sent and `try_assert_satisfied().await` completes
- **THEN** the result is `Err(MockAssertionError)` whose `Display` output contains the endpoint name and "expected 2", and no panic occurred

#### Scenario: try_assert_satisfied sets fail-fast latch on mismatch

- **GIVEN** a `MockEndpoint` created with `fail_fast: true` and an unmet expectation
- **WHEN** `try_assert_satisfied().await` completes
- **THEN** the result is `Err` AND `inner.fail_fast_error()` returns `Some` (latch parity with the panicking variant)

#### Scenario: invalid header regex returns Err instead of panicking

- **GIVEN** a `MockEndpointInner` with `expect_header_regex("k", "(unclosed")` set
- **WHEN** `try_assert_satisfied().await` completes
- **THEN** the result is `Err` in the invalid-pattern class (no panic), and `fail_fast_error()` returns `None` (a malformed expectation is a caller programming error, not an expectation mismatch — it does not trip the latch)

#### Scenario: Display matches panicking variant message

- **GIVEN** the same unmet expectation evaluated two ways on two identically-configured endpoints
- **WHEN** one endpoint's `assert_satisfied().await` panic message is captured and the other's `try_assert_satisfied().await` `Err` `Display` output is captured
- **THEN** both strings are equal

### Requirement: URI parameter surface

The `mock` scheme SHALL accept five URI parameters, parsed manually in
`create_endpoint` from the canonical keys `retain`, `copy`, `failFast`,
`expectedCount`, `anyOrder` (controlbus pattern: `skip_impl` metadata
descriptor carries the `#[uri_param]` catalog entries; the parser reads
exactly these keys, no aliases). When present: `retain` (integer >= 1,
retention limit), `copy` (boolean, clone bodies on record), `failFast`
(boolean), `expectedCount` (non-negative integer, recorded as an exact
count expectation on the endpoint inner), `anyOrder` (boolean, relaxed
body matching order). `retain`, `copy`, `failFast`, and `anyOrder`
fall back to the component-level `MockConfig` values when absent; an
absent `expectedCount` means no count expectation. The component
metadata catalog SHALL list exactly these five `uri_options` with the
same names the parser accepts (catalog parity). Malformed values
(non-numeric `retain`/`expectedCount`, `retain=0`, non-boolean
`copy`/`failFast`/`anyOrder`) SHALL fail endpoint creation with
`EndpointCreationFailed` or `InvalidUri`. Registry semantics stay
first-creation-wins per endpoint name: parameters bind at first
creation; later creations of the same name reuse the existing inner
unchanged.

#### Scenario: URI params override component config

- **GIVEN** a `MockComponent` with default config and three endpoints: `mock:cap?retain=50`, `mock:relaxed?anyOrder=true`, `mock:tight?failFast=true`
- **WHEN** 55 exchanges are sent to `cap`; two matching-but-out-of-order bodies are asserted on `relaxed` via `assert_satisfied`; an unmet expectation is asserted on `tight` then one more exchange is processed
- **THEN** `cap` shows `received_count() == 50` (retention truncation proves `retain` parsed and applied), `relaxed` satisfies the assertion with bodies arriving out of order (proves `anyOrder` applied), and `tight` rejects the post-mismatch exchange (fail-fast tripped, proves `failFast` applied). `copy` has no positive behavioral contrast today — both producer branches clone identically (`Body::clone` preserves `Stream` since the audit fix) — so `copy` parsing is proven by malformed-value rejection and catalog parity instead

#### Scenario: absent URI params fall back to component config

- **GIVEN** a `MockComponent` built with `MockConfig { fail_fast: true, ..Default::default() }`
- **WHEN** `create_endpoint("mock:audit")` is called (no params)
- **THEN** the endpoint behaves with `fail_fast: true` as configured at the component level, and no count expectation is registered

#### Scenario: malformed numeric param fails endpoint creation

- **WHEN** `create_endpoint("mock:x?retain=abc")` is called
- **THEN** the call returns `Err` (`EndpointCreationFailed` or `InvalidUri`) with a message naming `retain`

#### Scenario: zero retain rejected

- **WHEN** `create_endpoint("mock:x?retain=0")` is called
- **THEN** the call returns `Err` with a message naming `retain` and the >= 1 constraint

#### Scenario: malformed boolean param fails endpoint creation

- **WHEN** `create_endpoint("mock:x?copy=maybe")` is called
- **THEN** the call returns `Err` with a message naming `copy`

#### Scenario: first-creation-wins with conflicting params

- **GIVEN** a `MockComponent` where `create_endpoint("mock:single?retain=5")` succeeded
- **WHEN** `create_endpoint("mock:single?retain=100")` is called and 7 exchanges are sent
- **THEN** `get_endpoint("single").received_count().await` is 5 (the first creation's `retain=5` still binds; the second creation did not reconfigure)

#### Scenario: catalog parity with parser keys

- **WHEN** the component metadata is derived
- **THEN** its `uri_options` names, sorted, equal `["anyOrder", "copy", "expectedCount", "failFast", "retain"]` and `cargo xtask schema --check` passes

### Requirement: expectedCount never affects live traffic

The `expectedCount` URI parameter SHALL be inert in the live runtime
path until an assertion method evaluates it: it records an exact count
expectation at endpoint creation and nothing else. Producer
`poll_ready` and `call` SHALL NOT consult it — under `camel run`
(where no test caller invokes assertions) exchanges MUST be recorded
and acknowledged exactly as if the parameter were absent. Before any
assertion runs, `expectedCount` must not trip the fail-fast latch,
reject, or drop live traffic. After an explicit assertion method
reports the count mismatch, the existing fail-fast requirement applies
(the latch may trip when `fail_fast` is enabled, as with any other
expectation mismatch).

#### Scenario: expectedCount does not reject live exchanges

- **GIVEN** a `MockEndpoint` created via `create_endpoint("mock:sink?expectedCount=2&failFast=true")`
- **WHEN** 7 exchanges are processed by the `MockProducer` and no assertion method is called
- **THEN** all 7 calls return `Ok(exchange)`, `received_count().await` is 7, and `fail_fast_error()` returns `None`

#### Scenario: expectedCount enforced only by explicit assertion

- **GIVEN** a `MockEndpoint` created via `create_endpoint("mock:sink?expectedCount=2")`
- **WHEN** 3 exchanges are processed, then `try_assert_satisfied().await` completes
- **THEN** the result is `Err` (count mismatch 2 vs 3) — enforcement happens only at assertion time

#### Scenario: failed assertion then applies normal fail-fast

- **GIVEN** a `MockEndpoint` created via `create_endpoint("mock:sink?expectedCount=2&failFast=true")`
- **WHEN** 3 exchanges are processed, then `try_assert_satisfied().await` completes with `Err`, then another exchange is processed by the `MockProducer`
- **THEN** that producer call is rejected (the latch tripped via the explicit failed assertion, per the existing fail-fast requirement)

### Requirement: exchange accessor current-thread safety

`MockEndpointInner::exchange(idx)` SHALL NOT deadlock when called
from a current-thread tokio runtime: it SHALL detect the runtime
flavor via `tokio::runtime::Handle::try_current()` and, only for
`CurrentThread`, fail immediately with a panic whose message names the
constraint and the remedies (multi-thread runtime flavor, or async
accessors `get_received_exchanges`/`await_exchanges`). On a
multi-thread runtime the existing `block_in_place` behavior is
unchanged. Outside any tokio runtime, behavior is unchanged from the
current implementation (the guard must not alter it).

#### Scenario: current-thread runtime yields clear panic, not deadlock

- **GIVEN** a `MockEndpointInner` with 1 recorded exchange
- **WHEN** `exchange(0)` is called from a `#[tokio::test]` (current-thread flavor) inside `std::panic::catch_unwind`
- **THEN** the panic is caught promptly (the test completes without hanging) and its message mentions the runtime flavor requirement

#### Scenario: multi-thread runtime unchanged

- **GIVEN** a `MockEndpointInner` with 2 recorded exchanges
- **WHEN** `exchange(1)` is called from a `#[tokio::test(flavor = "multi_thread")]` runtime
- **THEN** the call returns an `ExchangeAssert` for the second exchange (no panic)

#### Scenario: no-runtime behavior unchanged

- **GIVEN** a `MockEndpointInner` with 1 recorded exchange
- **WHEN** `exchange(0)` is called from a plain `#[test]` (no tokio runtime)
- **THEN** the call returns an `ExchangeAssert` for the recorded exchange without panicking (tokio's `block_in_place` runs the closure normally outside a runtime; the guard must not alter this)

