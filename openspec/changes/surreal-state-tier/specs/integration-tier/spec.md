## ADDED Requirements

### Requirement: State prepare actions

The scenario grammar SHALL accept a state prepare action under two
family keys: `sql:` and `surreal:`. Each carries a `datasource` field
(a configured datasource name, never env-interpolated — the identifier
law) and a non-empty ordered `prepare` list of statements in the
family's query language. The document loader SHALL reject, as
load-time `doc-validation` errors naming the action index, a `prepare`
list containing a read statement and an empty `prepare` list. A read
is a statement whose trimmed prefix (skipping leading `(`) is, in any
letter case: `select` or `with` for the `sql` family; `select` for the
`surreal` family (SurrealQL has no other read prefix). Statement text
SHALL NOT be env-interpolated. When a family's Cargo feature is off,
a document declaring that family's action SHALL fail at load with a
named demand-gate error, mirroring `inbound:` activation behavior.

The executor SHALL resolve the datasource through the same
`DatasourceCatalog` the booted composition root built (one datasource
name, one pool or client; the executor SHALL NOT construct its own),
SHALL execute the statements sequentially in document order, and SHALL
stop at the first failure. Failure diagnostics SHALL carry the
datasource name, the failing statement index, and ADR-0051-sanitized
driver error text, and SHALL NOT contain the resolved `db_url` or any
row or record values; execution uses the non-fetching execute path,
so driver row results are discarded.

This requirement restates the contract first archived as `SQL prepare
action` (2026-09-08-sql-prepare-action). The 2026-09-18 canonical
spec rebuild (commit 02eec1be) dropped that requirement; this change
restores the sql coverage and extends it to the surreal family with no
observable behavior change to the landed sql grammar.

#### Scenario: sql prepare seeds schema and rows hermetically

- **GIVEN** a scenario document whose Camel.toml declares
  `[datasources.appdb] db_url = "sqlite::memory:?cache=shared"` and
  whose `scenario:` begins with a `sql:` action carrying one DDL and
  one INSERT statement
- **WHEN** the scenario runs
- **THEN** both statements execute in order against the booted pool
  and the scenario proceeds

#### Scenario: sql select-prefixed statement is a load error

- **GIVEN** a `sql:` action whose `prepare` list item 0 is
  `"(SELECT 1)"`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation`, naming the action index
  and statement index 0, whether or not the `sql` feature is compiled

#### Scenario: sql with-prefixed statement is a load error

- **GIVEN** a `sql:` action whose `prepare` list item 0 is
  `"with cte as (select 1) select * from cte"`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation`, naming the action index
  and statement index 0, whether or not the `sql` feature is compiled

#### Scenario: surreal prepare seeds records over the catalog

- **GIVEN** a scenario document whose Camel.toml declares
  `[datasources.statedb] db_url = "mem://"` with
  `provider = "surrealdb"`, and whose `scenario:` begins with a
  `surreal:` action carrying one DEFINE TABLE and one CREATE
  statement
- **WHEN** the scenario runs
- **THEN** both statements execute in order over the boot's surreal
  client and the scenario proceeds

#### Scenario: surreal select-prefixed statement is a load error

- **GIVEN** a `surreal:` action whose `prepare` list item 0 is
  `"SELECT * FROM user"`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation`, naming the action index
  and statement index 0, whether or not the `surreal` feature is
  compiled

#### Scenario: empty prepare list is a load error

- **GIVEN** a `sql:` or `surreal:` action with an empty `prepare`
  list
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the action index

#### Scenario: statement failure stops the sequence and redacts

- **GIVEN** a `sql:` action whose second statement violates a
  constraint and whose first statement seeds a row carrying a
  sentinel value, with the datasource `db_url` carrying its own
  sentinel query parameter
- **WHEN** the executor runs
- **THEN** the run fails naming the datasource name and statement
  index 1 with ADR-0051-sanitized database error text, neither
  sentinel appears in the diagnostic, and no later statement executes

#### Scenario: surreal statement failure stops and redacts

- **GIVEN** a `surreal:` action whose second statement fails at
  execution, a first statement that seeded a record carrying a
  sentinel field value, and a datasource `db_url` embedding a
  credential
- **WHEN** the executor runs
- **THEN** the run fails naming the datasource and statement index 1
  with sanitized driver error text, and the diagnostic contains
  neither the `db_url` nor the sentinel field value

#### Scenario: unknown datasource name fails closed

- **GIVEN** a `sql:` or `surreal:` action naming a datasource absent
  from the booted config
- **WHEN** the executor runs
- **THEN** the run fails naming the missing datasource name only

#### Scenario: family action without the feature is a named load error

- **GIVEN** a build without the `sql` feature and a document declaring
  `sql:`, and separately a build without the `surreal` feature and a
  document declaring `surreal:`
- **WHEN** each document loads
- **THEN** each load fails with the demand-gate error naming its
  family feature, exit 2

#### Scenario: single catalog invariant

- **GIVEN** a booted scenario with a configured datasource and a
  `sql:` or `surreal:` action
- **WHEN** the executor resolves the pool or client
- **THEN** the handle is the one the composition root's catalog built
  for that name — no second pool, client, or catalog is constructed

### Requirement: Surreal state assertion

A `validate` action MAY carry `target: {surreal: {datasource: <name>,
query: <read>}}`. The `datasource` SHALL be a configured datasource
name, never env-interpolated (the identifier law). The `query` SHALL
be a read: its trimmed prefix (skipping leading `(`) SHALL be
`select` in any letter case; any other prefix is a load-time
`doc-validation` error naming the action index, in both feature
configurations. The `expectation` node, the `columns` projection, and
the `unordered` switch SHALL follow the SQL state assertion grammar
exactly: exactly one row shape (`rows` tuples through the shared dual
matcher grammar, or one row-count bound), optional `columns` list
projecting fields by name in declared order, `unordered: true`
switching to multiset matching, and the same load-time error catalog
naming the action index and the offending field.

Execution (feature `surreal`): the action SHALL resolve the datasource
through the same `DatasourceCatalog` the booted composition root
built, execute the query, and project each result record into a tuple
of matcher values by `columns` order — Null to null, Bool to boolean,
Number (integer or float) to number, Strand to string, Uuid and
Datetime to their string forms, a record id to its `table:key` string
form, and Object and Array to structured values the matcher verbs see
whole. Any other value kind (Bytes, Geometry, ...) SHALL fail closed
naming the field and its SurrealQL type — never a silent null and
never a sentinel a wildcard could match away. An unknown `columns`
field SHALL fail closed naming the field. Without the feature, the
action SHALL fail with a named demand-gate error naming the `surreal`
feature; without a booted catalog, the action SHALL fail closed.

Poll semantics SHALL match the SQL state assertion: record sets are
not monotone, so the action never settles early; without a deadline
one immediate snapshot decides; with a deadline the action polls at a
fixed interval, fails immediately on a snapshot above the bound's
ceiling, and otherwise decides on the final snapshot. A `deadline` is
valid on a surreal target.

Hermetic tier: the embedded `mem://` SurrealDB backend is the
scenario-tier in-memory convention for this family. A `mem://`
datasource requires no credentials: the factory SHALL skip signin
for the `mem` scheme (a fresh embedded instance has no root user),
and `namespace` / `database` are optional extras defaulting to
`test` / `test`, while remote schemes keep the mandatory extras and
the signin step. The pool factory builds one client per datasource
name and the boot's catalog owns its lifecycle, so every boot starts
from an empty database — the SQLite per-connection `:memory:`
hazard does not exist, and no additional boot lint is required.
Remote backends (`ws://`, `wss://`, `http://`,
`https://`) address a real SurrealDB instance through the same
`db_url` steering; their state is durable and outside the boot
freshness guarantee, with isolation through the prepare-action
clean-first idiom (`REMOVE TABLE` or `DELETE` as the first
`surreal:` prepare statement).

Redaction (ADR-0051): a mismatch or driver failure SHALL carry the
datasource name, the rendered bound or expected row count, the actual
row count, and (for the `rows` shape) the projection's field names —
and SHALL NOT contain the resolved `db_url` (driver errors pass
through the ADR-0051 sanitizer) or any actual cell value.

#### Scenario: ordered rows assertion passes

- **GIVEN** a `surreal:` action that defines a table and creates two
  records, and a `validate` surreal target selecting both fields with
  `ORDER BY`
- **WHEN** the scenario runs with `expectation: {rows: [[1, "alice"],
  [2, "bob"]]}`
- **THEN** the action passes and the document passes

#### Scenario: unordered rows match out of order

- **GIVEN** records `(1, "alice")` and `(2, "bob")` seeded, and a
  query without `ORDER BY` selecting both
- **WHEN** the validate declares `unordered: true` and `rows: [[2,
  "bob"], [1, "alice"]]`
- **THEN** the action passes

#### Scenario: wildcard ignore cell matches any value

- **GIVEN** one record `(7, null)`
- **WHEN** the validate declares `rows: [[{ignore: null}, {equals:
  null}]]`
- **THEN** the action passes

#### Scenario: count bound passes on record count

- **GIVEN** three records seeded and a query selecting all three
- **WHEN** the validate declares `expectation: {atLeast: 2}` with no
  `rows`
- **THEN** the action passes

#### Scenario: mutation query is a load error

- **GIVEN** a validate surreal target whose query is `"DELETE user"`
  or `"CREATE user SET name = 'x'"`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the action index,
  whether or not the `surreal` feature is compiled

#### Scenario: rows mixed with a count bound is a load error

- **GIVEN** a validate surreal target whose expectation declares both
  `rows` and `count: 1`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the expectation node
  and the action index

#### Scenario: row length mismatch is a load error

- **GIVEN** a validate surreal target with `columns: [id, name]` and
  a row tuple of three cells
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the row index

#### Scenario: ordered rows without ORDER BY warn exactly once

- **GIVEN** a validate surreal target with an ordered `rows` shape
  whose query lacks `ORDER BY`
- **WHEN** the document loads
- **THEN** the load emits exactly one WARN naming the action index and
  succeeds

#### Scenario: field projection reorders by name

- **GIVEN** a query selecting `id, name` and `columns: [name, id]`
- **WHEN** the validate declares `rows: [["alice", "user:1"]]`
- **THEN** the action passes

#### Scenario: record id cell maps to its string form

- **GIVEN** one record whose SurrealQL id is `user:1` and a query
  selecting `id`
- **WHEN** the validate declares `rows: [["user:1"]]`
- **THEN** the action passes — the record id projects as the
  `table:key` string

#### Scenario: unknown projection field fails closed

- **GIVEN** `columns: [id, missing]`
- **WHEN** the action executes
- **THEN** the action fails naming `missing`

#### Scenario: unknown value kind fails closed

- **GIVEN** a record field holding a value kind outside the mapping
  table (for example a geometry value)
- **WHEN** the projection walks that field
- **THEN** the action fails naming the field and its SurrealQL type —
  never a silent null

#### Scenario: deadline poll passes when state settles in window

- **GIVEN** an empty table that a running route populates within the
  deadline
- **WHEN** the validate declares `deadline` and the `rows` the route
  will write
- **THEN** the final snapshot at expiry matches and the action passes

#### Scenario: no early settle — a matching snapshot is not proof

- **GIVEN** a record present at the first poll snapshot and deleted
  before the deadline, with `expectation: {rows: [[...]]}` and a
  `deadline`
- **WHEN** the deadline expires
- **THEN** the final snapshot decides and the action fails as a
  validation mismatch

#### Scenario: ceiling breach fails immediately

- **GIVEN** two records present and `expectation: {atMost: 1}` with a
  deadline
- **WHEN** the first snapshot reads two records
- **THEN** the action fails immediately without waiting the deadline

#### Scenario: deadline on a surreal target is accepted

- **GIVEN** a `validate` action whose target is `surreal` and whose
  node carries `deadline: 2s`
- **WHEN** the document loads
- **THEN** the load succeeds

#### Scenario: mismatch detail elides cells and db_url

- **GIVEN** a failing rows assertion against a datasource whose
  `db_url` embeds a credential
- **WHEN** the action fails
- **THEN** the detail names the datasource, the expected and actual
  row counts and the field names, and contains neither the `db_url`
  nor any actual cell value

#### Scenario: driver error is sanitized

- **GIVEN** a query that fails at execution (a missing table)
- **WHEN** the action executes
- **THEN** the failure carries the datasource name and sanitized
  driver error text, and never the resolved `db_url`

#### Scenario: feature off fails naming the gate

- **GIVEN** a document declaring a validate surreal target, built
  without the `surreal` feature
- **WHEN** the action runs
- **THEN** the action fails naming the `surreal` feature

#### Scenario: no catalog fails closed

- **GIVEN** a validate surreal action run through the single-action
  loop (no booted catalog)
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
is a partner, a sql target, or a surreal target. Without a deadline
the assertion SHALL read one immediate snapshot of the recorder and
decide on it, for every bound. With a deadline, because arrivals only
add (the filtered count is monotone non-decreasing), the bounds decide
as follows:

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

A bound or filter mismatch, immediate or at deadline, SHALL be a
verdict failure of the existing `validation-mismatch` class naming the
partner, the bound (in its own grammar, e.g. `at least 3`), the
filters (by kind), and the expected and actual counts. Recorded-path
diagnostics SHALL route through the redaction law (ADR-0051). Filter
payloads SHALL NOT render raw in diagnostics: declared `query` pairs
render `key=value` except keys in the harness secret set, which render
redacted; `pathContains` and `pathMatches` payloads render by kind
only. A `deadline` on a target that is neither partner, sql, nor
surreal, a missing bound, `count` mixed with `atLeast` or `atMost`, an
inverted range (`atLeast > atMost`), more than one path filter, a
negative or non-integer bound, an unknown expectation field, an
invalid `pathMatches` pattern, or an unparseable `deadline` SHALL
report `doc-validation` at load, naming the offending field, and exit
2.

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

- **GIVEN** a partner holding one matching `POST` and a route that
  sends no further matching request (a nonmatching `GET` may still
  land)
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

- **GIVEN** a partner bound and dialed elsewhere, holding zero
  recorded requests
- **WHEN** the scenario validates with
  `expectation: {atMost: 0}` and `deadline: 1s`
- **THEN** the action waits the window and passes on the final
  snapshot

#### Scenario: range fails fast above the maximum

- **GIVEN** a partner whose filtered count reaches 5 with a range
  bound of `atLeast: 2, atMost: 4` and an open deadline
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

- **GIVEN** a partner whose recorder holds `/q?b=2&a=1%2B1` on the
  wire
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

- **GIVEN** a validate action targeting `lastReceived` with a
  `deadline`
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

### Requirement: Demand-gated activation and CI isolation

The system SHALL gate adapter activation behind Cargo features — `http`
for v1, `sql` for the sql state family, `surreal` for the surreal
state family — SHALL reserve `testcontainer` and `user-provided`
partner provisioning values as grammar that the v1 runner rejects as
unsupported, and SHALL keep the default test suite unchanged in
runtime and composition. The `integration-http` and `integration-sql`
CI jobs SHALL run their loopback scenarios on relevant pull request
paths, and an `integration-surreal` CI job SHALL prove the surreal
feature stands alone (building `camel-cli` without `integration-http`
and `integration-sql`); loopback scenarios SHALL carry no `#[ignore]`
marker.

#### Scenario: http scenarios isolated behind their feature

- **GIVEN** a workspace built without the `http` adapter feature
- **WHEN** the integration-tier tests compile
- **THEN** no HTTP partner code is compiled in and no http scenario
  runs

#### Scenario: surreal state isolated behind its feature

- **GIVEN** a workspace built without the `surreal` state feature
- **WHEN** the integration-tier tests compile
- **THEN** no surreal executor code is compiled in and no surreal
  scenario runs; a document declaring `surreal:` fails at load
  naming the `surreal` feature, and a surreal validate target that
  passes load fails at action time naming the `surreal` feature

#### Scenario: reserved provisioning value rejected

- **GIVEN** a scenario endpoint declaring the `testcontainer` or
  `user-provided` provisioning source
- **WHEN** the document loads in v1
- **THEN** the run reports the source as unsupported and exits 2

#### Scenario: default suite untouched

- **GIVEN** the default test suite before and after this change
- **WHEN** both run in CI
- **THEN** their runtime and executed set are identical, and only the
  opt-in `integration-http`, `integration-sql`, and
  `integration-surreal` jobs exercise scenario documents

### Requirement: Scenario datasource teardown

Each scenario boot owns its `DatasourceCatalog`. The boot teardown
SHALL close the SQL datasource pools the boot opened, after the
context stops: a close timeout warns and does not fail the shutdown,
while a close error fails it, matching the bridge-pool teardown
semantics (providers without an explicit close keep their default
no-op). The same teardown SHALL close the surreal clients the boot
opened. With the pools closed, a SQLite in-memory database dies with
its boot: a later document booting the same `[datasources]` alias in
the same process SHALL start from an empty database, including named
shared-memory URIs (`file:<name>?mode=memory&cache=shared`), where a
lingering connection would otherwise carry rows into the later boot.
An embedded `mem://` SurrealDB datasource SHALL die with its boot the
same way: the factory builds one client per datasource name and the
catalog closes it at teardown, so a later boot over the same alias
starts from an empty database. File-backed (and other durable)
datasources — including remote `ws://`/`http://` SurrealDB instances
— are outside this guarantee: their rows persist across boots, and
isolation is the document author's responsibility through the
prepare-action clean-first idiom (a `DELETE FROM` or table-recreating
statement as the first `sql:` prepare statement, or a `REMOVE TABLE`
or `DELETE` statement as the first `surreal:` prepare statement),
mirroring ADR-0069 §9 — user-provided durable infrastructure is never
the harness's hermeticity contract.

The boot freshness regression verifies this requirement with a
test-unique named URI. The implementation must preserve its current
policy: close errors fail shutdown, while a close timeout warns
without failing shutdown, unless a separately reviewed change
explicitly amends that policy. The named shared-cache memory URI
remains the adopted sequential-boot convention; the per-test name is
fixture hardening, while concurrent boots sharing one alias remain
outside the v1 contract.

#### Scenario: All registered pools are closed before return

- **GIVEN** a scenario has initialized one or more datasource pools
- **WHEN** its boot shutdown invokes datasource teardown
- **THEN** the close future is awaited before the next boot is
  created, and the existing close-error and timeout policy is
  preserved

#### Scenario: a second boot over the same memory alias starts empty

- **GIVEN** a first document boot whose `sql:` prepare seeded rows
  over a `sqlite::memory:?cache=shared` alias, and that boot
  completed teardown
- **WHEN** a second document boots over the same alias in the same
  process
- **THEN** the second boot's reads see zero of the first boot's rows —
  the in-memory database died with the first boot's pools

#### Scenario: a named shared memory URI dies with its boot

- **GIVEN** a first document boot seeding rows over a named
  shared-memory URI (`sqlite:file:<name>?mode=memory&cache=shared`),
  whose connections share one named database, and that boot completed
  teardown
- **WHEN** a second document boots over the same URI in the same
  process
- **THEN** the second boot's reads see zero of the first boot's rows

#### Scenario: a second boot over a mem surreal datasource starts
empty

- **GIVEN** a first document boot whose `surreal:` prepare seeded
  records over a `mem://` datasource, and that boot completed
  teardown
- **WHEN** a second document boots over the same alias in the same
  process
- **THEN** the second boot's reads see zero of the first boot's
  records — the embedded database died with the first boot's client

#### Scenario: shutdown closes the datasource pools

- **GIVEN** a booted scenario whose document opened a SQL datasource
  pool through a `sql:` action or a sql `validate` target, or a
  surreal client through a `surreal:` action or a surreal `validate`
  target
- **WHEN** the boot-owning caller runs the boot teardown
- **THEN** the pools and clients that boot opened are closed before
  the teardown returns

#### Scenario: file-backed state persists across boots

- **GIVEN** a first document boot seeding rows into a file-backed
  sqlite datasource, and that boot completed teardown
- **WHEN** a second document boots over the same file-backed alias
- **THEN** the second boot observes the first boot's rows, and the
  second document's prepare is responsible for cleaning them
  (clean-first idiom)
