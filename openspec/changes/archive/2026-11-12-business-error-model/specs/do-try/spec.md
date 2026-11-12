## ADDED Requirements

### Requirement: catch clause business matchers

A `doTry` catch clause SHALL accept `code` and `business_kind` as main matchers:
`code` matches a `CamelError::Business` error by payload equality, and
`business_kind` matches one by its `kind` field. The matcher grammar SHALL stay
exactly-one-main-matcher: `exception`, `when`, `code`, and `business_kind` are
mutually exclusive on one clause, at load time, with the existing typed error.
The `on_when` refinement predicate SHALL be allowed with `exception`, `code`, or
`business_kind` as the main matcher, and SHALL stay rejected with `when` alone.
A clause with `code` or `business_kind` SHALL NOT match any non-`Business`
error. Catch matching order SHALL stay first-match-wins, and the
catch-body-failure envelope ("do_try catch-block failure envelope") SHALL apply
to business errors like any other caught error.

#### Scenario: catch by code with on_when combination

- **GIVEN** a doTry whose body throws `Business { code: "ORDER_NOT_FOUND", kind: "not-found", .. }`,
  with catch clauses `[{code: "ORDER_NOT_FOUND", on_when: "${header.retry} == 'true'",
  disposition: handled}, {code: "ORDER_NOT_FOUND", disposition: handled}]`
- **WHEN** the header `retry` is `"true"` and the body throws
- **THEN** the first clause matches (code equality and predicate true) and handles the error
- **WHEN** the header `retry` is `"false"` and the body throws again
- **THEN** the first clause does not match and the second clause handles the error

#### Scenario: catch by business_kind

- **GIVEN** a doTry whose catch clause is `[{business_kind: "conflict", disposition: handled}]`
- **WHEN** the body throws `Business { kind: "conflict", .. }`
- **THEN** the clause matches and handles the error
- **WHEN** the body throws `ValidationError("bad input")`
- **THEN** the clause does not match, because the error is not a business error

#### Scenario: two payload matchers on one clause fail at load

- **GIVEN** a doTry catch clause carrying both `code` and `business_kind`
- **WHEN** the route loads
- **THEN** loading fails with the typed matcher-exclusivity error, the same
  grammar rule that rejects `exception` plus `when`

#### Scenario: on_when with when alone stays rejected

- **GIVEN** a doTry catch clause carrying `when` plus `on_when`
- **WHEN** the route loads
- **THEN** loading fails with the existing `on_when`-requires-a-non-when-main-matcher error
