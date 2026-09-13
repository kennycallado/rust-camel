## ADDED Requirements

### Requirement: Boot freshness regression protects the landed datasource teardown

The integration boot harness SHALL retain regression coverage for the landed
scenario datasource teardown contract: a named shared-cache SQLite database
cannot remain observable through a later boot after the first boot's awaited
shutdown succeeds.

#### Scenario: Sequential boots do not share named-memory rows

- **GIVEN** boot A and boot B use the same test-unique named shared-cache
  SQLite URI
- **WHEN** boot A inserts one row, awaits shutdown, and boot B starts and
  inserts one row
- **THEN** boot B observes exactly its own row

### Requirement: Boot freshness synchronization is observable

The boot freshness regression SHALL synchronize on awaited lifecycle and pool
state transitions, not on an unconditional sleep.

#### Scenario: Load does not change teardown ordering

- **GIVEN** the test runs while other workspace builds or test tasks create
  scheduler load
- **WHEN** boot A shutdown completes before boot B is created
- **THEN** the existing pool-drain contract is exercised and the result is
  independent of elapsed wall-clock delay

## MODIFIED Requirements

### Requirement: Scenario datasource teardown

Each scenario boot owns its `DatasourceCatalog`. The boot teardown SHALL close
the SQL datasource pools the boot opened, after the context stops: a close
timeout warns and does not fail the shutdown, while a close error fails it,
matching the bridge-pool teardown semantics (providers without an explicit
close keep their default no-op). With the pools closed, a SQLite in-memory
database dies with its boot: a later document booting the same
`[datasources]` alias in the same process SHALL start from an empty database,
including named shared-memory URIs (`file:<name>?mode=memory&cache=shared`),
where a lingering connection would otherwise carry rows into the later boot.
File-backed (and other durable) datasources are outside this guarantee: their
rows persist across boots, and isolation is the document author's
responsibility through the prepare-action clean-first idiom (a `DELETE FROM`
or table-recreating statement as the first `sql:` prepare statement),
mirroring ADR-0069 §9 — user-provided durable infrastructure is never the
harness's hermeticity contract.

The boot freshness regression verifies this requirement with a test-unique
named URI. The implementation must preserve its current policy: close errors
fail shutdown, while a close timeout warns without failing shutdown, unless a
separately reviewed change explicitly amends that policy. The named
shared-cache memory URI remains the adopted sequential-boot convention; the
per-test name is fixture hardening, while concurrent boots sharing one alias
remain outside the v1 contract.

#### Scenario: All registered pools are closed before return

- **GIVEN** a scenario has initialized one or more datasource pools
- **WHEN** its boot shutdown invokes datasource teardown
- **THEN** the close future is awaited before the next boot is created, and
  the existing close-error and timeout policy is preserved

#### Scenario: a second boot over the same memory alias starts empty

- **GIVEN** a first document boot whose `sql:` prepare seeded rows over a
  `sqlite::memory:?cache=shared` alias, and that boot completed teardown
- **WHEN** a second document boots over the same alias in the same process
- **THEN** the second boot's reads see zero of the first boot's rows — the
  in-memory database died with the first boot's pools

#### Scenario: a named shared memory URI dies with its boot

- **GIVEN** a first document boot seeding rows over a named shared-memory URI
  (`sqlite:file:<name>?mode=memory&cache=shared`), whose connections share one
  named database, and that boot completed teardown
- **WHEN** a second document boots over the same URI in the same process
- **THEN** the second boot's reads see zero of the first boot's rows

#### Scenario: shutdown closes the datasource pools

- **GIVEN** a booted scenario whose document opened a SQL datasource pool
  through a `sql:` action or a sql `validate` target
- **WHEN** the boot-owning caller runs the boot teardown
- **THEN** the pools that boot opened are closed before the teardown returns

#### Scenario: file-backed state persists across boots

- **GIVEN** a first document boot seeding rows into a file-backed sqlite
  datasource, and that boot completed teardown
- **WHEN** a second document boots over the same file-backed alias
- **THEN** the second boot observes the first boot's rows, and the second
  document's prepare is responsible for cleaning them (clean-first idiom)
