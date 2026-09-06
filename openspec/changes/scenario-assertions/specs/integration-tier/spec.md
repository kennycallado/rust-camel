# Delta: integration-tier

## MODIFIED Requirements

### Requirement: Scripted partner declarations

A document MAY declare a top-level `partners:` section. It SHALL be a
map from the exact declared endpoint string, the `:0` URI as written in
the endpoint reference, to a sequence of script entries. Each entry
SHALL be a map with optional matchers `method` (case-insensitive) and
`path` (exact, query included), and SHALL carry exactly one of:

- `response`: a map with optional `status` (100-599, default 200),
  `headers`, and `body`.
- `fault: close`: drop the connection without an HTTP response.

An entry MAY also declare:

- `times: N` (integer >= 1, default 1): the entry serves its first N
  matching requests, then is spent.
- `delay: <humantime>` (default none): hold before serving the response
  or committing the fault.

The `response.body` encoding SHALL mirror the client send path
(`value_to_wire` semantics): a JSON string serves its exact bytes
verbatim — no quoting, no escaping; `null` serves an empty body; any
other JSON value serves as compact JSON. An absent `body` serves an
empty body.

The listener SHALL serve the first unspent entry whose matchers match, in
declaration order; when no entry matches it SHALL serve the unmatched
status (500 with an empty body, or the permissive default). Every request
that reaches the wire SHALL be recorded before any scripting decision.

#### Scenario: scripted response serves the matching request

- **GIVEN** a `partners:` entry scripting method `POST` on `/orders`
  with status 201 and a body
- **WHEN** the scenario sends `method: POST` to that path
- **THEN** the response carries status 201 and the scripted body, and
  the scenario validates both

#### Scenario: unmatched request serves the unmatched status

- **GIVEN** a `partners:` entry scripting only method `POST` on
  `/orders`
- **WHEN** the scenario sends `method: DELETE` to the same path
- **THEN** the response carries status 500 with an empty body, and
  validation of any scripted body fails

#### Scenario: absent partners section keeps permissive behavior

- **GIVEN** a scenario with a harness endpoint reference and no
  `partners:` section
- **WHEN** any request reaches the partner
- **THEN** the response is status 200 with an empty body, exactly as
  before this section existed

#### Scenario: unknown partner entry field is a load error

- **GIVEN** a `partners:` entry with a field `responsez`
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the field and exits 2

#### Scenario: partners key with no matching declared ref is a load error

- **GIVEN** a declared harness reference `http://127.0.0.1:0/orders`
  and a `partners:` key `http://127.0.0.1:0/order` (a typo of the
  declared URI)
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the unmatched key
  and exits 2

#### Scenario: delay holds the response before serving

- **GIVEN** a partner entry with `delay: 500ms` and a 200 response
- **WHEN** the scenario sends to the partner
- **THEN** the send completes no sooner than the delay and observes 200

#### Scenario: fault close breaks the connection without a response

- **GIVEN** a partner entry with `fault: close`
- **WHEN** the scenario sends to the partner
- **THEN** the send fails with a transport-class error rather than an HTTP
  status, and the recorder still holds the request

#### Scenario: times serves N matching requests then spends the entry

- **GIVEN** a partner entry with `times: 2` and a fallback entry matching
  the same request
- **WHEN** the scenario sends the matching request three times
- **THEN** the first two sends observe the timed entry's response and the
  third observes the fallback's

#### Scenario: times below one is a load error

- **GIVEN** a partner entry with `times: 0`
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the partner key and
  `times`, and exits 2

#### Scenario: response and fault together is a load error

- **GIVEN** a partner entry declaring both `response` and `fault`
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the partner key and
  both fields, and exits 2

#### Scenario: neither response nor fault is a load error

- **GIVEN** a partner entry declaring matchers but neither `response`
  nor `fault`
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the partner key and
  the missing fields, and exits 2

#### Scenario: unknown fault name is a load error

- **GIVEN** a partner entry with `fault: reset`
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the partner key and
  `fault`, and exits 2

#### Scenario: delay applies before a fault

- **GIVEN** a partner entry with `delay: 300ms` and `fault: close`
- **WHEN** the scenario sends to the partner
- **THEN** the send fails with a transport-class error no sooner than the
  delay

#### Scenario: plain-string body is served verbatim

- **GIVEN** a partner script whose `response.body` is the JSON string
  `exact text` (not a JSON document)
- **WHEN** a client-role send hits the scripted entry and the response
  body is validated on the wire
- **THEN** the served bytes equal `exact text` exactly — no quotes, no
  escapes

#### Scenario: null body serves an empty body

- **GIVEN** a partner script whose `response.body` is `null`
- **WHEN** a client-role send hits the scripted entry
- **THEN** the served body is empty (zero bytes), not the literal `null`

#### Scenario: partner and client body encodings agree

- **GIVEN** the same JSON `Value` used as a partner script `body` and as
  a client send `body`
- **WHEN** both cross the wire
- **THEN** the wire bytes are identical for strings, `null`, and
  structured values alike

### Requirement: Partner request verification

A `validate` action MAY target a partner with
`target: {partner: <declared endpoint URI>}`. The URI MUST equal a
declared harness `http` endpoint ref; otherwise the run SHALL report
`doc-validation` naming the URI and exit 2. The action SHALL assert on
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

- **GIVEN** a validate action whose `partner` URI equals no declared
  harness `http` endpoint ref
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the URI and exits 2

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

## ADDED Requirements

### Requirement: Minimum-elapsed arrival assertion

A `validate` action MAY carry `elapsedAtLeast: <humantime>` when, and
only when, its target is `lastReceived`. The runner SHALL anchor a
scenario-start instant. Each adapter SHALL capture the wire-arrival
instant — the monotonic time at which the transport finished receiving
the message — and the assertion SHALL compare THAT instant (not the
time the `receive` action consumed it from the queue) against the
scenario start. The assertion SHALL pass if and only if the wire
arrival of the last received message on the target endpoint is at least
`elapsedAtLeast` after the scenario start. A failed assertion SHALL be
a verdict failure of the existing `validation-mismatch` class naming
the endpoint, the bound, and the actual elapsed duration. An
`elapsedAtLeast` on a non-`lastReceived` target or an unparseable value
SHALL report `doc-validation` at load, naming the offending field, and
exit 2. This requirement delivers the not-before-X proof dimension
ADR-0069 §5 names; no §5 amendment is required (§13.3 admits new
assertion kinds through their own changes).

#### Scenario: waited arrival passes the bound

- **GIVEN** a partner script with `delay: 300ms` and a receive on that
  endpoint that completes
- **WHEN** the scenario validates the endpoint with
  `elapsedAtLeast: 200ms`
- **THEN** the action passes

#### Scenario: early arrival fails even when consumed late

- **GIVEN** a message whose wire arrival happens 20ms after scenario
  start but whose `receive` action runs after a later `sleep` and
  consumes it at 2s
- **WHEN** the scenario validates the endpoint with
  `elapsedAtLeast: 1s`
- **THEN** the scenario fails with `validation-mismatch` naming the
  endpoint, `1s`, and the actual wire-arrival elapsed of about 20ms

#### Scenario: too-early arrival fails naming the actual elapsed

- **GIVEN** a receive that completes within 50ms of scenario start
- **WHEN** the scenario validates the endpoint with
  `elapsedAtLeast: 10s`
- **THEN** the scenario fails with `validation-mismatch` naming the
  endpoint, `10s`, and the actual elapsed

#### Scenario: elapsedAtLeast on a partner target is a load error

- **GIVEN** a validate action targeting a `partner` with
  `elapsedAtLeast`
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the action and
  `elapsedAtLeast`, and exits 2

#### Scenario: unparseable elapsedAtLeast is a load error

- **GIVEN** a validate action whose `elapsedAtLeast` is not a humantime
  duration
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the field and
  exits 2
