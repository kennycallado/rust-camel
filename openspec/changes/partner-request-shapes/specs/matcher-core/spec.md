## ADDED Requirements

### Requirement: per-request shape matching over projected observations

Per-request shape matching SHALL be generic over projected
`(method, path_and_query, body value)` observations — never over harness
observation types. A `RequestShape` SHALL carry the same optional filters
as a `RequestExpectation` (`method` case-insensitive, one `PathFilter`,
`query` subset) plus an optional `Expectation` over the projected body
value. The judgment SHALL be a pure function taking the declared shape
sequence and the projected sequence, comparing positionally, and reporting
at most the FIRST mismatch with its zero-based index and the failed
aspect (`method`, `path`, `query`, or `body`). An absent element at a
declared index SHALL NOT be a mismatch of the judgment function itself —
length is the count bound's business; the judgment reports mismatches
only for indices present in both sequences.

#### Scenario: positional shape match on present elements

- **GIVEN** shapes `[{method: POST, body: {jsonSubset: {k: "1"}}}, {method: GET}]`
- **WHEN** judged against projections
  `[("POST", "/o", {"k": "1", "x": 2}), ("GET", "/h", null)]`
- **THEN** no mismatch is reported

#### Scenario: first mismatch wins with index and aspect

- **GIVEN** shapes `[{method: POST}, {method: GET}]`
- **WHEN** judged against projections `[("POST", "/o", null), ("POST", "/h", null)]`
- **THEN** the single reported mismatch names index 1 and aspect `method`

#### Scenario: body expectation over a text-projected value

- **GIVEN** a shape with `body: {contains: "idempotency"}`
- **WHEN** judged against a projection whose body value is the string
  `"idempotency-key-42"` (bytes that did not parse as JSON)
- **THEN** the shape matches

#### Scenario: purity boundary holds

- **GIVEN** the crate's manifest and public API
- **WHEN** audited for harness, wire, or redaction coupling
- **THEN** the shape surface carries none: types plus pure functions only,
  dependencies unchanged (`regex`, `serde_json`, `form_urlencoded`)
