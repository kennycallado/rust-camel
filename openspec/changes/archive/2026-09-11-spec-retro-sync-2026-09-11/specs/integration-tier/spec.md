## MODIFIED Requirements

### Requirement: SQLite shared-cache mandate

The scenario boot SHALL reject any configured datasource whose `db_url` is
a SQLite in-memory URL (`sqlite::memory:` or `sqlite://:memory:` in any
letter case) whose query string does not carry `cache=shared`, failing
with the `sql-memory-not-shared` error class naming the datasource. The
check SHALL run ungated: with or without the `sql` Cargo feature, and for
documents with or without `sql:` actions.

The scenario-tier in-memory convention SHALL be the named shared-cache
memory URI — `sqlite:file:<name>?mode=memory&cache=shared`, with
`provider = "sqlx"` pinned because `sqlite:file:` matches no automatic
datasource prefix: one named database shared by every pool connection,
with pool size an author choice rather than a correctness constraint. The
bare `sqlite::memory:?cache=shared` form remains accepted, but
cross-connection sharing is not guaranteed there. The
`sql-memory-not-shared` failure SHALL name the datasource and steer
authors to the named form. (Convention landed in 7cc0cb23, bd rc-gcf9n.)

Rationale: SQLite `:memory:` databases are per-connection and the pool
default is more than one connection — INSERT and SELECT on different pool
connections hit different databases, a silent green lie. The named
shared-cache URI is the mandated remedy.

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

#### Scenario: named shared-cache URI shares across pool connections

- **GIVEN** a datasource with
  `db_url = "sqlite:file:memdb_probe?mode=memory&cache=shared"` and a pool
  of more than one connection
- **WHEN** one pool connection INSERTs a row and another connection
  SELECTs the count
- **THEN** the reading connection sees the inserted row — one named
  database backs the whole pool

#### Scenario: lint steers bare memory to the named form

- **GIVEN** a Camel.toml declaring `[datasources.appdb] db_url =
  "sqlite::memory:"`
- **WHEN** the scenario boots
- **THEN** the `sql-memory-not-shared` error names `appdb` and presents
  the named shared-memory URI (with the `sqlx` provider pin) as the
  remedy

### Requirement: Scenario datasource teardown

Each scenario boot owns its `DatasourceCatalog`. The boot teardown SHALL
close the SQL datasource pools the boot opened, after the context stops:
a close timeout warns and does not fail the shutdown, while a close error
fails it, matching the bridge-pool teardown semantics (providers without
an explicit close keep their default no-op). With the pools closed, a
SQLite in-memory database dies with its boot: a later document booting
the same `[datasources]` alias in the same process SHALL start from an
empty database — including named shared-memory URIs
(`file:<name>?mode=memory&cache=shared`), where a lingering connection
would otherwise carry rows into the later boot. File-backed (and other
durable) datasources are outside this guarantee: their rows persist
across boots, and isolation is the document author's responsibility
through the prepare-action clean-first idiom (a `DELETE FROM` or
table-recreating statement as the first `sql:` prepare statement),
mirroring ADR-0069 §9 — user-provided durable infrastructure is never
the harness's hermeticity contract.

Non-normative note (bd rc-gcf9n, landed in 7cc0cb23): the named
shared-cache memory URI is the adopted scenario-tier convention for
sequential boots. Concurrent boots sharing one in-memory alias remain
outside the v1 contract; a per-boot unique memory URI
(`file:memdb_<scenario>?mode=memory&cache=shared`) is the planned
remedy when parallel document execution lands.

#### Scenario: a second boot over the same memory alias starts empty

- **GIVEN** a first document boot whose `sql:` prepare seeded rows over a
  `sqlite::memory:?cache=shared` alias, and that boot completed teardown
- **WHEN** a second document boots over the same alias in the same process
- **THEN** the second boot's reads see zero of the first boot's rows — the
  in-memory database died with the first boot's pools

#### Scenario: a named shared memory URI dies with its boot

- **GIVEN** a first document boot seeding rows over a named shared-memory
  URI (`sqlite:file:<name>?mode=memory&cache=shared`), whose connections
  share one named database, and that boot completed teardown
- **WHEN** a second document boots over the same URI in the same process
- **THEN** the second boot's reads see zero of the first boot's rows

#### Scenario: shutdown closes the datasource pools

- **GIVEN** a booted scenario whose document opened a SQL datasource pool
  through a `sql:` action or a sql `validate` target
- **WHEN** the boot-owning caller runs the boot teardown
- **THEN** the pools that boot opened are closed before the teardown
  returns

#### Scenario: file-backed state persists across boots

- **GIVEN** a first document boot seeding rows into a file-backed sqlite
  datasource, and that boot completed teardown
- **WHEN** a second document boots over the same file-backed alias
- **THEN** the second boot observes the first boot's rows, and the second
  document's prepare is responsible for cleaning them (clean-first idiom)
