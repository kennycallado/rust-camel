## `integration-tier` (`sql-prepare-action` delta)

## ADDED Requirements

### Requirement: SQL prepare action

The scenario grammar SHALL accept a `sql:` action carrying a `datasource`
field (a configured datasource name, never env-interpolated) and a
non-empty ordered `prepare` list of SQL statements. The document loader
SHALL reject, as load-time `doc-validation` errors naming the action index,
a `prepare` list containing a statement whose trimmed prefix (skipping
leading `(`) is `select` or `with` in any letter case, and an empty
`prepare` list. Statement text SHALL NOT be env-interpolated. When the
`sql` Cargo feature is off, a document declaring `sql:` SHALL fail at load
with a named demand-gate error, mirroring `inbound:` activation behavior.

The executor SHALL resolve the datasource through the same
`DatasourceCatalog` the booted composition root built (one datasource name,
one pool; the executor SHALL NOT construct its own pool), SHALL execute the
statements sequentially in document order, and SHALL stop at the first
failure. Failure diagnostics SHALL carry the datasource name, the failing
statement index, and ADR-0051-sanitized database error text, and SHALL NOT
contain the resolved `db_url` or any row values; execution uses the
non-fetching execute path, so driver row results are discarded.

#### Scenario: prepare seeds schema and rows hermetically

- **GIVEN** a scenario document whose Camel.toml declares
  `[datasources.appdb] db_url = "sqlite::memory:?cache=shared"` and whose
  `scenario:` begins with a `sql:` action carrying one DDL and one INSERT
  statement
- **WHEN** the scenario runs
- **THEN** both statements execute in order against the booted pool and the
  scenario proceeds

#### Scenario: select-prefixed statement is a load error

- **GIVEN** a `sql:` action whose `prepare` list item 0 is `"(SELECT 1)"`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation`, naming the action index and
  statement index 0, whether or not the `sql` feature is compiled

#### Scenario: with-prefixed statement is a load error

- **GIVEN** a `sql:` action whose `prepare` list item 0 is
  `"with cte as (select 1) select * from cte"`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation`, naming the action index and
  statement index 0, whether or not the `sql` feature is compiled

#### Scenario: empty prepare list is a load error

- **GIVEN** a `sql:` action with an empty `prepare` list
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the action index

#### Scenario: statement failure stops the sequence and redacts

- **GIVEN** a `sql:` action whose second statement violates a constraint
  and whose first statement seeds a row carrying a sentinel value, with the
  datasource `db_url` carrying its own sentinel query parameter
- **WHEN** the executor runs
- **THEN** the run fails naming the datasource name and statement index 1
  with ADR-0051-sanitized database error text, neither sentinel appears in
  the diagnostic, and no later statement executes

#### Scenario: unknown datasource name fails closed

- **GIVEN** a `sql:` action naming a datasource absent from the booted
  config
- **WHEN** the executor runs
- **THEN** the run fails naming the missing datasource name only

#### Scenario: sql action without the feature is a named load error

- **GIVEN** a build without the `sql` feature and a document declaring
  `sql:`
- **WHEN** the document loads
- **THEN** the load fails with the demand-gate error naming `sql`, exit 2

#### Scenario: single catalog invariant

- **GIVEN** a booted scenario with a configured datasource and a `sql:`
  action
- **WHEN** the executor resolves the pool
- **THEN** the pool handle is the one the composition root's catalog built
  for that name — no second pool or catalog is constructed

## ADDED Requirements

### Requirement: SQLite shared-cache mandate

The scenario boot SHALL reject any configured datasource whose `db_url` is
a SQLite in-memory URL (`sqlite::memory:` or `sqlite://:memory:` in any
letter case) whose query string does not carry `cache=shared`, failing
with the `sql-memory-not-shared` error class naming the datasource. The
check SHALL run ungated: with or without the `sql` Cargo feature, and for
documents with or without `sql:` actions.

Rationale: SQLite `:memory:` databases are per-connection and the pool
default is more than one connection — INSERT and SELECT on different pool
connections hit different databases, a silent green lie. `cache=shared` is
the mandated remedy.

#### Scenario: bare memory sqlite fails boot

- **GIVEN** a Camel.toml declaring `[datasources.appdb] db_url =
  "sqlite::memory:"`
- **WHEN** the scenario boots
- **THEN** boot fails with `sql-memory-not-shared` naming `appdb`, whether
  or not the document uses a `sql:` action

#### Scenario: shared cache passes

- **GIVEN** a Camel.toml declaring `db_url = "sqlite::memory:?cache=shared"`
- **WHEN** the scenario boots
- **THEN** boot proceeds past the check

#### Scenario: file-backed sqlite is unaffected

- **GIVEN** a Camel.toml declaring a `sqlite::file:` or `sqlite:file:` URL
- **WHEN** the scenario boots
- **THEN** the shared-cache check does not apply

#### Scenario: lint fires without the sql feature or sql actions

- **GIVEN** a workspace built without the `sql` feature, a scenario
  document with no `sql:` action, and a bare `sqlite::memory:` datasource
- **WHEN** the scenario boots
- **THEN** the boot still fails `sql-memory-not-shared`
