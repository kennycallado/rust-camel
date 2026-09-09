## ADDED Requirements

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

Non-normative note (bd rc-gcf9n): concurrent boots sharing one
in-memory alias are outside the v1 contract; a per-boot unique memory
URI (`file:memdb_<scenario>?mode=memory&cache=shared`) is the planned
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
