## ADDED Requirements

### Requirement: SQL state assertion

A `validate` action MAY carry `target: {sql: {datasource: <name>, query:
<read>}}`. The `datasource` SHALL be a configured datasource name,
never env-interpolated (the identifier law). The `query` SHALL be a
read: its trimmed prefix (skipping leading `(`) SHALL be `select` or
`with` in any letter case; any other prefix is a load-time
`doc-validation` error naming the action index, in both feature
configurations. The `expectation` node of a sql target SHALL carry exactly
one row shape:

- `rows`: a non-empty list of row tuples; every cell parses through the
  shared dual matcher grammar (a bare value is a literal `equals`; a
  single-key matcher object is that matcher — including `ignore`, the
  wildcard verb whose payload SHALL be null and which matches any cell
  value including null);
- a row-count bound using the partner count keys: exactly one of
  `count: n`, `atLeast: n`, `atMost: n`, or `atLeast` combined with
  `atMost` (a range, `atLeast <= atMost`).

An optional `columns: [<name>, ...]` list projects the query result by
column name, in the declared order, before matching; omitted, the
projection is every column in query order. An optional `unordered:
true` (default `false`) switches the `rows` shape from ordered
(positional) to set (multiset) matching. Declaring both `rows` and a
count bound, an empty `rows` list, an unknown `expectation` field, an
unknown target field, an empty `columns` list, a non-string column
name, or a row tuple whose length differs from the projection width
SHALL be a load-time `doc-validation` error naming the action index
and the offending field — and, for a row-length mismatch, the row
index.

Determinism: when the `rows` shape is ordered (no `unordered: true`)
and the query text lacks `ORDER BY` (case-insensitive substring), the
loader SHALL emit exactly one WARN naming the action index; the load
still succeeds.

Execution (feature `sql`): the action SHALL resolve the datasource
through the same `DatasourceCatalog` the booted composition root built
(one datasource name, one pool; the executor SHALL NOT construct its
own pool), execute the query, and project each row into a tuple of
matcher values — NULL to null, INTEGER and REAL to numbers, TEXT to
strings, BLOB to json-first with lossy-UTF-8 fallback (the
`reply_bytes_value` precedent). An unknown `columns` name SHALL fail
closed naming the column. Without the feature, the action SHALL fail
with a verdict-class error naming the `sql` feature; without a booted
catalog, the action SHALL fail closed.

Poll semantics (papal deviation, recorded in the matcher doc-comments):
unlike partner request counts, SQL row sets are NOT monotone — a
DELETE can shrink them. A `validate` sql action therefore NEVER settles
early. Without a `deadline`, one immediate snapshot decides. With a
`deadline`, the action SHALL poll snapshots at a fixed interval,
failing immediately on a snapshot whose count is above the bound's
ceiling (`atMost` breach or above a range maximum), and otherwise SHALL wait
the full deadline and decide on the final snapshot: the `rows` shape
passes when the final snapshot's projected tuples match (ordered:
length and cell-wise; unordered: a perfect multiset matching), the
bound shape passes when `bound_holds` holds on the final count.

Redaction (ADR-0051): a mismatch or driver failure SHALL carry the
datasource name, the rendered bound or expected row count, the actual
row count, and (for the `rows` shape) the projection's column names —
and SHALL NOT contain the resolved `db_url` (driver errors pass
through the ADR-0051 sanitizer) or any actual cell value.

#### Scenario: ordered rows assertion passes

- **GIVEN** a `sql:` action that creates a table and inserts two rows,
  and a `validate` sql target selecting both columns with `ORDER BY`
- **WHEN** the scenario runs with `expectation: {rows: [[1, "alice"], [2,
  "bob"]]}`
- **THEN** the action passes and the document passes

#### Scenario: unordered rows match out of order

- **GIVEN** rows `(1, "alice")` and `(2, "bob")` seeded, and a query
  without `ORDER BY` selecting both
- **WHEN** the validate declares `unordered: true` and `rows: [[2,
  "bob"], [1, "alice"]]}`
- **THEN** the action passes

#### Scenario: wildcard ignore cell matches any value

- **GIVEN** one row `(7, null)`
- **WHEN** the validate declares `rows: [[{ignore: null}, {equals:
  null}]]` — the wildcard matches the integer, `equals` matches null
- **THEN** the action passes

#### Scenario: count bound passes on row count

- **GIVEN** the rows `(1, "alice")`, `(2, "bob")`, and `(3, "carol")`
  seeded, and a query selecting all three
- **WHEN** the validate declares `expectation: {atLeast: 2}` with no
  `rows`
- **THEN** the action passes

#### Scenario: mutation query is a load error

- **GIVEN** a validate sql target whose query is `"DELETE FROM users"`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the action index,
  whether or not the `sql` feature is compiled

#### Scenario: rows mixed with a count bound is a load error

- **GIVEN** a validate sql target whose expectation declares both `rows` and
  `count: 1`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the expectation node and
  the action index

#### Scenario: row length mismatch is a load error

- **GIVEN** a validate sql target with `columns: [id, name]` and a row
  tuple of three cells
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the row index

#### Scenario: ordered rows without ORDER BY warn exactly once

- **GIVEN** a validate sql target with an ordered `rows` shape whose
  query lacks `ORDER BY`
- **WHEN** the document loads
- **THEN** the load emits exactly one WARN naming the action index and
  succeeds

#### Scenario: unordered rows without ORDER BY do not warn

- **GIVEN** the same document with `unordered: true`
- **WHEN** the document loads
- **THEN** no WARN is emitted

#### Scenario: deadline poll passes when state settles in window

- **GIVEN** an empty table that a running route populates within the
  deadline
- **WHEN** the validate declares `deadline` and the `rows` the route
  will write
- **THEN** the final snapshot at expiry matches and the action passes

#### Scenario: no early settle — a matching snapshot is not proof

- **GIVEN** a row present at the first poll snapshot and deleted before
  the deadline, with `expectation: {rows: [[...]]}` and a `deadline`
- **WHEN** the deadline expires
- **THEN** the final snapshot decides and the action fails as a
  validation mismatch

#### Scenario: ceiling breach fails immediately

- **GIVEN** two rows present and `expectation: {atMost: 1}` with a deadline
- **WHEN** the first snapshot reads two rows
- **THEN** the action fails immediately without waiting the deadline

#### Scenario: column projection reorders by name

- **GIVEN** a query selecting `id, name` and `columns: [name, id]`
- **WHEN** the validate declares `rows: [["alice", 1]]`
- **THEN** the action passes

#### Scenario: unknown projection column fails closed

- **GIVEN** `columns: [id, missing]`
- **WHEN** the action executes
- **THEN** the action fails naming `missing`

#### Scenario: mismatch detail elides cells and db_url

- **GIVEN** a failing rows assertion against a datasource whose
  `db_url` embeds a credential
- **WHEN** the action fails
- **THEN** the detail names the datasource, the expected and actual
  row counts and the column names, and contains neither the `db_url`
  nor any actual cell value

#### Scenario: driver error is sanitized

- **GIVEN** a query that fails at execution (a missing table)
- **WHEN** the action executes
- **THEN** the failure carries the datasource name and sanitized driver
  error text, and never the resolved `db_url`

#### Scenario: feature off fails naming the gate

- **GIVEN** a document declaring a validate sql target, built without
  the `sql` feature
- **WHEN** the action runs
- **THEN** the action fails naming the `sql` feature

#### Scenario: no catalog fails closed

- **GIVEN** a validate sql action run through the single-action loop
  (no booted catalog)
- **WHEN** the action runs
- **THEN** the action fails closed naming the missing catalog

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

A `validate` action MAY carry a `deadline` (humantime) when its target
is a partner or a sql target. Without a deadline the assertion SHALL
read one immediate snapshot of the recorder and decide on it, for every
bound. With a deadline, because arrivals only add (the filtered count
is monotone non-decreasing), the bounds decide as follows:

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
`deadline` on a target that is neither partner nor sql, a missing
bound, `count` mixed with `atLeast` or `atMost`, an inverted range
(`atLeast > atMost`), more than one path filter, a negative or
non-integer bound, an unknown expectation field, an invalid
`pathMatches` pattern, or an unparseable `deadline` SHALL report
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

#### Scenario: deadline on a sql target is accepted

- **GIVEN** a `validate` action whose target is `sql` and whose node
  carries `deadline: 2s`
- **WHEN** the document loads
- **THEN** the load succeeds

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
