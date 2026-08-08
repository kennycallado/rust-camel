## ADDED Requirements

### Requirement: Principal Debug redaction of untrusted claims

The system SHALL NOT render the raw `claims` payload of `Principal` through the
`Debug` formatting path. The `claims` field SHALL render as the literal
`[REDACTED]` under any `{:?}` formatting. The retained fields (`subject`,
`issuer`, `audience`, `scopes`, `roles`) are an intentional operator-visible
allowlist of identity descriptors; only the free-form untrusted `claims` blob is
suppressed, because claims carry provider-mapped identity data (ADR-0032) that
may include personally identifiable information.

Note on achievable contract: a claim value may coincidentally equal a retained
descriptor (for example a `sub` claim equal to `subject`). The contract therefore
targets the claims payload itself: the `claims` field renders as `[REDACTED]`,
and any value that appears ONLY in `claims` (not in a retained descriptor) is
absent from the Debug output. It does not require the absence of strings that
also appear in the retained allowlist.

#### Scenario: Debug output redacts claims payload (compact formatting)

- **GIVEN** a `Principal` whose `claims` is a JSON object containing a sentinel
  key `piid` with value `SENTINEL_CLAIM_VALUE_9kq2` (a value that appears in no
  retained descriptor), and whose retained fields are populated
- **WHEN** the principal is formatted with `format!("{principal:?}")` (compact
  Debug)
- **THEN** the formatted string contains `claims: "[REDACTED]"`, does NOT
  contain `SENTINEL_CLAIM_VALUE_9kq2`, and DOES contain the retained descriptor
  values (`subject`, `issuer`, `audience`, `scopes`, `roles`)

#### Scenario: Debug output redacts claims payload (pretty formatting)

- **GIVEN** the same `Principal` as above
- **WHEN** the principal is formatted with `format!("{principal:#?}")` (pretty
  Debug)
- **THEN** the formatted string contains `[REDACTED]` for the `claims` field and
  does NOT contain `SENTINEL_CLAIM_VALUE_9kq2`

#### Scenario: serialization is unaffected

- **GIVEN** a `Principal` with populated `claims`
- **WHEN** the principal is serialized with serde (`Serialize`)
- **THEN** the serialized output contains the full `claims` value, because
  claims are the principal's legitimate data payload crossing the auth boundary
