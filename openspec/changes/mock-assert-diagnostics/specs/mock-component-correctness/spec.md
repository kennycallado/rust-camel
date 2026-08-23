# mock-component-correctness Delta

## MODIFIED Requirements

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
`BodyNotFound { endpoint, expected, received_count }`,
`HeaderNotFound { endpoint, key, value, received_count, actual_values, last_headers }`,
`HeaderRegexNotMatched { endpoint, key, pattern, received_count, actual_values, last_headers }`,
and `InvalidHeaderPattern { endpoint, key, pattern, source }`.
Every assertion branch maps to exactly one variant: exact count →
`CountMismatch`; minimum count → `MinimumCountNotMet`;
expected-bodies-count differs from received-count →
`BodyCountMismatch`; ordered body index mismatch → `BodyMismatch`;
any-order body not found → `BodyNotFound`; header value not found →
`HeaderNotFound`; header regex not matched → `HeaderRegexNotMatched`;
invalid header regex pattern → `InvalidHeaderPattern`.

The diagnostic fields SHALL carry the received state at evaluation:
`received_count` is the number of received exchanges; `actual_values`
holds the `{:?}`-formatted values of the expected key across received
exchanges that carry it, capped at 8 entries with a `+N more` suffix on
overflow; `last_headers` holds the pre-formatted key list of the last
received exchange, capped at 8 entries with a `+N more` suffix on
overflow, and is `None` when no exchange was received.

`Display` for `BodyNotFound`, `HeaderNotFound`, and
`HeaderRegexNotMatched` SHALL append a received-state clause to the
existing message: `(received 0 exchanges)` when no exchange was
received; the key's actual values when the key is present under other
values; the last exchange's header keys when the key is absent
everywhere but at least one exchange was received. Its `Display`
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

#### Scenario: HeaderNotFound with zero received exchanges reports arrival state

- **GIVEN** a `MockEndpointInner` with `expect_header("k", "v")` set and no exchanges sent
- **WHEN** `try_assert_satisfied().await` completes
- **THEN** the `Err` `Display` output contains "received 0 exchanges"

#### Scenario: HeaderNotFound with the key under other values reports actual values

- **GIVEN** a `MockEndpointInner` with `expect_header("k", "expected")` set
- **WHEN** 2 exchanges are sent, each carrying header `k` with value "actual-1" / "actual-2", and `try_assert_satisfied().await` completes
- **THEN** the `Err` `Display` output contains "received 2 exchanges" and both actual values

#### Scenario: HeaderNotFound with the key absent reports last exchange headers

- **GIVEN** a `MockEndpointInner` with `expect_header("k", "v")` set
- **WHEN** 1 exchange carrying headers `a` and `b` (but not `k`) is sent and `try_assert_satisfied().await` completes
- **THEN** the `Err` `Display` output reports the key absent and contains the header keys `a` and `b`

#### Scenario: BodyNotFound reports received count

- **GIVEN** a `MockEndpointInner` with `expect_body("x")` under any-order evaluation
- **WHEN** 1 exchange with a different body is sent and `try_assert_satisfied().await` completes
- **THEN** the `Err` `Display` output contains "received 1" and the expected body

#### Scenario: diagnostic lists cap at 8 entries

- **GIVEN** a `MockEndpointInner` with `expect_header("k", "v")` set
- **WHEN** 10 exchanges carrying header `k` under 10 distinct values are sent and `try_assert_satisfied().await` completes
- **THEN** the `Err` `Display` output lists at most 8 values and contains "+2 more"
