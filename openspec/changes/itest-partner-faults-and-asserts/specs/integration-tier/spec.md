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

The listener SHALL serve the first unspent entry whose matchers match, in
declaration order; when no entry matches it SHALL serve the unmatched
status (500 with an empty body, or the permissive default). Every request
that reaches the wire SHALL be recorded before any scripting decision.
Partners SHALL bind before route boot. Every `partners:` key MUST equal a
declared harness `http` endpoint ref. Unknown entry or response fields,
`times` below 1, both or neither of `response`/`fault`, unknown fault
names, and unparseable `delay` values SHALL fail `doc-validation` at
load, naming the partner key and the offending field, and exit 2. A
document with no `partners:` section SHALL keep the `permissive(200)`
default: status 200 with an empty body, exactly as before this section
existed.

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

## ADDED Requirements

### Requirement: Partner request verification

A `validate` action MAY target a partner with
`target: {partner: <declared endpoint URI>}`. The URI MUST equal a
declared harness `http` endpoint ref; otherwise the run SHALL report
`doc-validation` naming the URI and exit 2. The action SHALL assert on the partner's recorded requests
through an `expectation` map with required `count` (exact integer) and
optional `method` (case-insensitive) and `path` (exact, query included)
filters, where `count` is a non-negative integer. A `validate` action MAY
carry a `deadline` (humantime) when, and only when, its target is a
partner: without it the assertion SHALL read one immediate snapshot of
the recorder; with it the action SHALL poll until equality or
expiration. When expiration is first observed, the action SHALL take one
final recorder snapshot. That final snapshot remains eligible to pass
only when its filtered count equals `count`; otherwise it fails. The
assertion SHALL pass if and only if a snapshot's filtered count equals
`count`; a count above `count` never passes and polling continues to the
deadline. The reported actual count SHALL be the filtered count from
the final snapshot. A count mismatch, immediate or at deadline, SHALL be
a verdict failure of the existing `validation-mismatch` class naming the
partner, the filters, and the expected and actual counts. A `deadline`
on a non-partner target, a missing, negative, or non-integer `count`, an
unknown expectation field, or an unparseable `deadline` SHALL report
`doc-validation` at load, naming the offending field, and exit 2.

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
- **WHEN** the document loads
- **THEN** the run reports `doc-validation` naming the expectation and
  exits 2
