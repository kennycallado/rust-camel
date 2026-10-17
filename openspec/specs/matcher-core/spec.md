# matcher-core Specification

## Purpose
The matcher algebra in `camel-matchers`. It owns count-bound judgment, request
matching, value expectations, and per-request shape matching, generic over
projected observations and free of `camel-*` dependencies.

## Requirements
### Requirement: pure matcher algebra ownership

The matcher algebra SHALL live in the `camel-matchers` crate as its single
definition site, with zero `camel-*` dependencies (foundational third-party
crates — `regex`, `serde_json`, `form_urlencoded` — allowed, serde derives
permitted). `camel-integration-test` SHALL consume the crate rather than carry
duplicates; unit-tier adoption (`camel-test`) is the pinned future direction
(ADR-0072 staged step 2), not part of this change.

#### Scenario: carve, not copy

- **GIVEN** wave D's matcher types and judgment functions inside `camel-integration-test`
- **WHEN** the carve change lands
- **THEN** `camel-integration-test` contains no duplicate definition of `CountBound`, `PathFilter`, `RequestExpectation` (ex-partner expectation), `Expectation`, the bound judgment functions, bound rendering, the value-matcher evaluation helpers (`stringify`, `json_subset`), or the query-pair parser, and its validation paths import them from `camel_matchers`

#### Scenario: publish topology stays acyclic

- **GIVEN** the new crate declares no `camel-*` dependency
- **WHEN** `cargo xtask lint-publish-cycles` runs
- **THEN** it passes, and no crate that depends on `camel-matchers` publishes before it

### Requirement: count-bound judgment semantics

The bound judgment functions SHALL preserve the documented poll semantics:
arrivals only add (the filtered count is monotone non-decreasing), so `Exact`
and `AtLeast` may settle early at equality/floor, while `AtMost` and `Range`
are absence claims over the window that never settle early and fail
immediately once a snapshot is above the ceiling.

#### Scenario: absence claims never settle early

- **GIVEN** an `AtMost(n)` or `Range(min, max)` bound and a current count inside bounds
- **WHEN** `settles_early` is evaluated
- **THEN** it returns `false` regardless of the count

#### Scenario: ceiling breach fails immediately

- **GIVEN** an `AtMost(2)` bound and an actual count of 3 (or a `Range(1, 2)` with count 3)
- **WHEN** `above_ceiling` is evaluated
- **THEN** it returns `true`, while for `Exact` and `AtLeast` bounds it returns `false` for any count

#### Scenario: final-snapshot decision honors every form

- **GIVEN** `bound_holds` and counts around each bound's edges (equality, floor, ceiling, range bounds inclusive)
- **WHEN** evaluated
- **THEN** `Exact(n)` holds iff equal, `AtLeast(n)` iff `>=`, `AtMost(n)` iff `<=`, `Range(min, max)` iff inclusive-in-range

### Requirement: request matching over projected observations

Request matching SHALL be generic over projected `(method, path_and_query)`
string tuples — never over harness observation types — with method comparison
case-insensitive, path filters as `Exact` (strict bytes), `Contains`
(substring), `Matches` (regex, invalid pattern fails closed by matching
nothing), and query subset order- and encoding-independent (percent-decoded,
`+` decoded).

#### Scenario: query subset is order- and encoding-independent

- **GIVEN** a declared query subset `{"a": "1", "b": "2"}`
- **WHEN** matching a projection whose path-and-query is `/x?b=2&a=1` and another whose value arrives percent-encoded (`%31`)
- **THEN** both match

#### Scenario: invalid regex fails closed

- **GIVEN** a `PathFilter::Matches("(")` pattern that cannot compile
- **WHEN** `matching_count` runs over projections that would otherwise match
- **THEN** the count is 0

### Requirement: value expectation evaluation

The `Expectation` value matcher SHALL evaluate over `serde_json::Value` with
the forms `Equals`, `Regex` (compile-verified, invalid matches nothing),
`Contains`, `StartsWith`, `EndsWith`, `Exists`, and `JsonSubset` (recursive
subset: objects match when every declared key's value matches recursively;
scalars by equality), as a pure function with no harness or redaction
coupling.

#### Scenario: recursive subset match

- **GIVEN** a `JsonSubset` declaring `{"user": {"name": "María"}}`
- **WHEN** evaluated against `{"user": {"name": "María", "role": "admin"}, "extra": 1}`
- **THEN** it matches

#### Scenario: pure evaluation boundary

- **GIVEN** the crate's manifest and public API
- **WHEN** audited for harness, wire, or redaction coupling
- **THEN** it exposes no observation type, no async runtime dependency, and no redaction function

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

