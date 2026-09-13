## ADDED Requirements

### Requirement: SQL pool close drain is unit-tested

The SQL component test suite SHALL verify that the `SqlPoolFactory::close` seam completes successfully only after the created pool has no remaining connections.

#### Scenario: Close drains a released SQLite memory pool

- **GIVEN** a uniquely named SQLite shared-memory pool created by `SqlPoolFactory` and exercised with released pooled connections
- **WHEN** the test calls `SqlPoolFactory::close` through a `DatasourceHandle`
- **THEN** close returns successfully, the pool reports `is_closed()`, and `pool.size()` is zero without wall-clock waiting or infrastructure

### Requirement: SQLite memory URL classification has a truth table

The SQL component test suite SHALL cover the memory URL classifier with supported forms, case variants, and non-memory near misses.

#### Scenario: Classifier distinguishes memory and non-memory URLs

- **GIVEN** table rows containing bare SQLite memory URLs, named shared-cache `mode=memory` URLs, uppercase scheme/query variants, ordinary SQLite paths, a `memory.db` filename, and a non-SQLite URL with a memory query
- **WHEN** the classifier is evaluated for each row
- **THEN** each result matches its expected boolean, including uppercase memory forms and rejection of near misses

## MODIFIED Requirements

## REMOVED Requirements

## RENAMED Requirements
