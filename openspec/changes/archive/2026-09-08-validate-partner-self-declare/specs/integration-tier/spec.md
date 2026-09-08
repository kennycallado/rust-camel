## MODIFIED Requirements

### Requirement: Partner request verification

A `validate` action MAY target a partner with
`target: {partner: <declared endpoint URI>}`. The URI MUST equal a
harness `http` endpoint ref declared by the scenario's own
`send`/`receive` actions, OR the reference MUST use the object form
with `provisioning: harness` on an `http` endpoint while a `partners:`
entry names the URI — in which case the validate action's own
reference declares the harness partner and SHALL wire it exactly as a
`send`/`receive` reference does: the driver binds the partner, fills
the reference's `bindVar` with the bound authority, and exposes it
through the harness-provisioned env fold. Any other partner URI is a
load error: the run SHALL report `doc-validation` naming the URI and
exit 2. The action SHALL assert on
the partner's recorded requests through an `expectation` map carrying
exactly one bound form — `count: n` (exact), `atLeast: n`, `atMost: n`,
or `atLeast` combined with `atMost` (a range, requiring
`atLeast <= atMost`) — plus optional filters:

- `method` (case-insensitive),
- one path filter at most: `path` (exact wire bytes, query included),
  `pathContains` (substring of the recorded path-and-query), or
  `pathMatches` (regular expression, compile-verified at load),
- `query`: a map of string keys to string values matched as a subset —
  every declared pair SHALL be present among the recorded query's
  percent-decoded pairs, order-independent and encoding-independent.

All filters compose by logical AND. Absence SHALL be expressed as
`atMost: 0`.

A `validate` action MAY carry a `deadline` (humantime) when, and only
when, its target is a partner. Without a deadline the assertion SHALL
read one immediate snapshot of the recorder and decide on it, for every
bound. With a deadline, because arrivals only add (the filtered count is
monotone non-decreasing), the bounds decide as follows:

- `count` — poll until a snapshot's filtered count equals it or the
  deadline expires; a count above it never passes. On first-observed
  expiry, one final snapshot decides; the reported actual is that
  snapshot's filtered count.
- `atLeast(n)` — poll until the filtered count reaches `n` (early
  success: once reached, always reached); at expiry the final snapshot
  decides and fails below `n`.
- `atMost(n)` — an upper bound is an absence claim over the window:
  the action SHALL wait the full deadline (failing immediately on any
  snapshot above `n`) and decide on the final snapshot — `n` or below
  passes. Early success is invalid: a passing early snapshot cannot
  prove the count stays within bounds.
- range (`atLeast` + `atMost`) — fail immediately on any snapshot above
  the maximum; otherwise wait the full deadline and decide on the final
  snapshot — within `[minimum, maximum]` passes.

A bound or filter mismatch, immediate or at deadline, SHALL be a verdict
failure of the existing `validation-mismatch` class naming the partner,
the bound (in its own grammar, e.g. `at least 3`), the filters (by kind),
and the expected and actual counts. Recorded-path diagnostics SHALL
route through the redaction law (ADR-0051). Filter payloads SHALL NOT
render raw in diagnostics: declared `query` pairs render `key=value`
except keys in the harness secret set, which render redacted;
`pathContains` and `pathMatches` payloads render by kind only. A
`deadline` on a non-partner target, a missing bound, `count` mixed with
`atLeast` or `atMost`, an inverted range (`atLeast > atMost`), more than
one path filter, a negative or non-integer bound, an unknown expectation
field, an invalid `pathMatches` pattern, or an unparseable `deadline`
SHALL report `doc-validation` at load, naming the offending field, and
exit 2.

#### Scenario: immediate count assert passes

- **GIVEN** a partner whose recorder holds three `POST` requests after
  earlier actions
- **WHEN** the scenario validates `target: {partner: ...}` with
  `expectation: {count: 3, method: POST}`
- **THEN** the action passes without a deadline

#### Scenario: count mismatch fails naming partner and counts

- **GIVEN** a partner whose recorder holds one request
- **WHEN** the scenario validates the partner with `count: 3`
- **THEN** the scenario fails with `validation-mismatch` naming the
  partner, the expected count 3, and the actual count 1

#### Scenario: filters narrow the counted requests

- **GIVEN** a partner holding two `GET` and two `POST` requests
- **WHEN** the scenario validates with `expectation: {count: 2, method: GET}`
- **THEN** the action passes

#### Scenario: deadline polls until the count settles

- **GIVEN** a route that retries a faulted partner asynchronously
- **WHEN** the scenario validates the partner with `count: 3` and
  `deadline: 5s` while the retries are still in flight
- **THEN** the action polls and passes once the third request lands,
  without waiting the full deadline

#### Scenario: count that never settles fails at the deadline

- **GIVEN** a partner whose filtered count reaches 4 without any poll
  observing 3
- **WHEN** the scenario validates the partner with `count: 3` and
  `deadline: 1s`
- **THEN** the scenario fails with `validation-mismatch` naming the
  partner, the expected count 3, and the actual count from the final
  snapshot

#### Scenario: atLeast settles early and passes

- **GIVEN** a partner holding two requests with a third in flight
- **WHEN** the scenario validates with `expectation: {atLeast: 3}` and
  `deadline: 5s`
- **THEN** the action polls and passes once the third lands, without
  waiting the full deadline

#### Scenario: atLeast fails at the deadline naming the actual

- **GIVEN** a partner whose recorder holds one request
- **WHEN** the scenario validates with `expectation: {atLeast: 3}` and
  `deadline: 1s`
- **THEN** the scenario fails with `validation-mismatch` naming the
  partner, `at least 3`, and the actual count 1

#### Scenario: atMost without a deadline decides on the snapshot

- **GIVEN** a partner whose recorder holds two requests
- **WHEN** the scenario validates with `expectation: {atMost: 2}`
- **THEN** the action passes on the immediate snapshot

#### Scenario: atMost with a deadline waits the full window

- **GIVEN** a partner holding one matching `POST` and a route that sends
  no further matching request (a nonmatching `GET` may still land)
- **WHEN** the scenario validates with
  `expectation: {atMost: 1, method: POST}` and `deadline: 2s`
- **THEN** the action waits the full deadline (no early pass) and
  decides on the final snapshot

#### Scenario: atMost fails fast when exceeded mid-window

- **GIVEN** a partner whose count rises above the bound while the
  validate window is open
- **WHEN** a poll observes the filtered count above `atMost: 2`
- **THEN** the scenario fails immediately with `validation-mismatch`
  naming the partner, `at most 2`, and the observed count

#### Scenario: atMost zero asserts absence over the window

- **GIVEN** a partner bound and dialed elsewhere, holding zero recorded
  requests
- **WHEN** the scenario validates with
  `expectation: {atMost: 0}` and `deadline: 1s`
- **THEN** the action waits the window and passes on the final snapshot

#### Scenario: range fails fast above the maximum

- **GIVEN** a partner whose filtered count reaches 5 with a range bound
  of `atLeast: 2, atMost: 4` and an open deadline
- **WHEN** a poll observes the count 5
- **THEN** the scenario fails immediately, before the deadline expires

#### Scenario: range passes on the final snapshot

- **GIVEN** a partner whose filtered count settles at 3 with a range
  bound of `atLeast: 2, atMost: 4` and `deadline: 1s`
- **WHEN** the deadline expires
- **THEN** the action passes on the final snapshot

#### Scenario: inverted range is a load error

- **GIVEN** a partner-target expectation carrying `atLeast: 4` and
  `atMost: 3`
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the expectation and
  exits 2

#### Scenario: pathContains tolerates encoding drift

- **GIVEN** a partner whose recorder holds `/q?bbox=1.5%2C2.5` on the
  wire
- **WHEN** the scenario validates with
  `expectation: {atLeast: 1, pathContains: "bbox="}`
- **THEN** the action passes without pinning the encoded byte sequence

#### Scenario: pathMatches narrows by regular expression

- **GIVEN** a partner holding `/orders/42` and `/health`
- **WHEN** the scenario validates with
  `expectation: {count: 1, pathMatches: "^/orders/\\d+$"}`
- **THEN** the action passes

#### Scenario: query subset matches order- and encoding-independent

- **GIVEN** a partner whose recorder holds `/q?b=2&a=1%2B1` on the wire
- **WHEN** the scenario validates with
  `expectation: {atLeast: 1, query: {a: "1+1", b: "2"}}`
- **THEN** the action passes

#### Scenario: query subset fails when a declared pair is absent

- **GIVEN** a partner whose recorder holds `/q?a=1`
- **WHEN** the scenario validates with
  `expectation: {atLeast: 1, query: {a: "1", c: "3"}}`
- **THEN** the scenario fails with `validation-mismatch`

#### Scenario: count mixed with atLeast is a load error

- **GIVEN** a partner-target expectation carrying both `count` and
  `atLeast`
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the expectation and
  exits 2

#### Scenario: two path filters are a load error

- **GIVEN** a partner-target expectation carrying both `path` and
  `pathContains`
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the expectation and
  exits 2

#### Scenario: invalid pathMatches pattern is a load error

- **GIVEN** a partner-target expectation whose `pathMatches` does not
  compile
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the pattern and
  exits 2

#### Scenario: partner target with undeclared URI is a load error

- **GIVEN** a validate action whose `partner` URI is a plain string,
  or an object form no `partners:` entry names, and it equals no
  harness `http` endpoint ref declared by the scenario's
  `send`/`receive` actions
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the URI and exits 2

#### Scenario: object-form validate partner self-declares

- **GIVEN** a scenario whose only reference to an `http` partner is a
  validate action using the object form with `provisioning: harness`
  and a `bindVar`, and a `partners:` entry naming the URI, with no
  `send`/`receive` action referencing it
- **WHEN** the scenario runs
- **THEN** the driver binds the partner, fills the `bindVar` with the
  bound authority, exposes it through the harness-provisioned env
  fold, and the validate asserts on the partner's recorded requests —
  with no sacrificial `receive` and no timeout verdict

#### Scenario: deadline on a non-partner target is a load error

- **GIVEN** a validate action targeting `lastReceived` with a `deadline`
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the action and
  `deadline`, and exits 2

#### Scenario: missing count is a load error

- **GIVEN** a partner-target validate whose expectation lacks `count`
  and carries no `atLeast` or `atMost` either
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the expectation and
  exits 2

#### Scenario: missing bound is a load error

- **GIVEN** a partner-target validate whose expectation carries no
  `count`, `atLeast`, or `atMost`
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the expectation and
  exits 2

