## ADDED Requirements

### Requirement: Transacted route block surface

The system SHALL provide an explicit `transacted { datasource: X }` route
block that composes the existing step model, authored identically through the
fluent builder, YAML, and the canonical form, with strict rejection of unknown
keys.

#### Scenario: YAML transacted block with two sql steps loads

- **GIVEN** a route YAML with `transacted: { datasource: pg, steps: [<two sql steps on pg>] }`
- **WHEN** the route is loaded
- **THEN** the block compiles to the same step model produced by the fluent builder and the canonical form

#### Scenario: unknown key in the transacted block is rejected

- **GIVEN** a `transacted` block containing an unrecognized key
- **WHEN** the route is loaded
- **THEN** the load fails with an error naming the offending key (parser rejection; no datasource fields asserted — none exist at that stage)

Slice 2 (`sqltrans-runtime`) owns the runtime execution of this block
("Lazy begin and block-owned commit/rollback"); slice 3 (`sqltrans-modes`)
owns mode semantics ("Transaction mode semantics"). This slice delivers the
surface, authoring equivalence, and the load-time rejection of requirement
"Load-time validation is fail-closed and typed".

### Requirement: Load-time validation is fail-closed and typed

The system SHALL reject at load, with a typed error asserting its variant,
the offender (route, step), and the datasource: StreamList production inside a
block (unconditional); parallel split, multicast, wiretap, async handoff, and
seda escape each reaching a joined sql step inside a block (each a distinct
case); a sql step on a datasource other than the block's; a sql step without a
named datasource inside a block; a nested `transacted` block; an unknown
datasource with no registered TransactionManager; an unknown block key.
StreamList production inside a block is rejected unconditionally, separate
from the reachability-based cases. Every structural rejection SHALL have a
named test with concrete input asserting the exact error variant and, where
applicable, the route, offender, and datasource; parser-level rejections
(unknown block key) assert only the offending key.

#### Scenario: parallel split reaching a joined sql step is rejected

- **GIVEN** a `transacted` block containing a parallel-mode split whose fragments contain a sql step on the block datasource
- **WHEN** the route is loaded
- **THEN** the load fails with the typed parallel-inside-block error naming the split and the datasource

#### Scenario: multicast reaching a joined sql step is rejected

- **GIVEN** a `transacted` block containing a multicast that reaches a sql step on the block datasource
- **WHEN** the route is loaded
- **THEN** the load fails with the typed error naming the multicast step and the datasource

#### Scenario: wiretap reaching a joined sql step is rejected

- **GIVEN** a `transacted` block containing a wiretap that reaches a sql step on the block datasource
- **WHEN** the route is loaded
- **THEN** the load fails with the typed error naming the wiretap step and the datasource

#### Scenario: async handoff reaching a joined sql step is rejected

- **GIVEN** a `transacted` block containing an async handoff that reaches a joined sql step
- **WHEN** the route is loaded
- **THEN** the load fails with the typed error naming the escape and the datasource

#### Scenario: seda escape reaching a joined sql step is rejected

- **GIVEN** a `transacted` block containing a seda endpoint handoff that reaches a joined sql step
- **WHEN** the route is loaded
- **THEN** the load fails with the typed error naming the seda step and the datasource

#### Scenario: StreamList inside a block is rejected unconditionally

- **GIVEN** a sql step producing StreamList anywhere inside a `transacted` block
- **WHEN** the route is loaded
- **THEN** the load fails with the typed error naming the step, regardless of whether a later step consumes the stream

#### Scenario: foreign datasource inside a block is rejected

- **GIVEN** a `transacted { datasource: pg }` block containing a sql step on datasource `audit`
- **WHEN** the route is loaded
- **THEN** the load fails with the typed foreign-datasource error naming both datasources (no 2PC)

#### Scenario: unnamed datasource inside a block is rejected

- **GIVEN** a `transacted` block containing a sql step configured only by inline `db_url`
- **WHEN** the route is loaded
- **THEN** the load fails with the typed unnamed-datasource error naming the step

#### Scenario: nested transacted block is rejected

- **GIVEN** a `transacted` block containing another `transacted` block
- **WHEN** the route is loaded
- **THEN** the load fails with the typed nested-block error naming the inner block

#### Scenario: transacted block on an unregistered datasource fails at load

- **GIVEN** a `transacted` block naming a datasource with no registered TransactionManager
- **WHEN** the route is loaded
- **THEN** the load fails with the typed manager-missing error naming the datasource

Slice 2 (`sqltrans-runtime`) owns the runtime join-side rejections of
requirement "Join semantics for sql steps on the block datasource"
(`RuntimeSqlInJoinedStep`, `TransactionControlSql`, `JoinSessionMissing`,
`JoinScopeMismatch`, `DialectUnprovable`, `UnsupportedJoinDialect`); this
slice owns every load-time structural rejection above.

### Requirement: TransactionManager port and registry resolution

The system SHALL expose a provider-neutral, object-safe `TransactionManager`
port in camel-api — full public rustdoc, `#[non_exhaustive]` error and outcome
types (including `TransactionOutcome`) — implemented by camel-sql over sqlx,
resolved by datasource name from a camel-core registry, failing closed with a
named error when no manager is registered, with no core→component dependency
in the other direction.

#### Scenario: port surface is documented and versioned

- **GIVEN** the public `TransactionManager`, `TransactionSession`, and outcome/error types
- **WHEN** the crate is documented
- **THEN** every public item carries rustdoc and the outcome/error types carry `#[non_exhaustive]`

Slice 2 (`sqltrans-runtime`) delivers the camel-sql implementation of this
port over sqlx and its registration; this slice delivers the port surface,
the `CamelError::Transaction` mapping, and the fail-closed registry
resolution.

### Requirement: ADR and operational documentation

The system SHALL ship ADR 0101 covering the chosen surface, both rejected
surfaces (per-exchange and per-endpoint), the concurrency and pool cost, the
at-least-once idempotent window, restart-on-upgrade statement-cache
clearing, and PgBouncer session-mode `server_reset_query=DISCARD ALL`
guidance; the camel-sql component documentation SHALL carry the same
operational note; the docker Postgres suite in `camel-test/tests/sql_test.rs`
SHALL exercise the runtime capability scenarios — outbox commit and rollback,
join proof, `max_connections=1` join, concurrent separate transactions,
discard-under-abort, commit-uncertainty backend replacement, pool exhaustion,
block timeout, joined mixed-shape batch
rollback, and the idempotent key dispositions — while the load rejections
SHALL be covered by unit-tier route-loading tests.

#### Scenario: ADR content

- **GIVEN** ADR 0101 in `docs/adr/`
- **WHEN** it is reviewed
- **THEN** it states the chosen `transacted` surface, rejected surfaces (a) per-exchange and (c) per-endpoint with reasons, pool/concurrency cost, the at-least-once window, and the operational notes

#### Scenario: docker verification exists

- **GIVEN** the `camel-test` integration suite
- **WHEN** `cargo test -p camel-test --features integration-tests --test sql_test` runs against Docker Postgres
- **THEN** the listed runtime scenarios of this capability are exercised against Docker Postgres

#### Scenario: load rejections are unit-tier

- **GIVEN** each load rejection of this capability
- **WHEN** the named unit-tier route-loading test runs (no Docker required)
- **THEN** it asserts the exact error variant and offender/datasource fields (parser-level unknown-key rejection asserts the offending key only)

This slice delivers ADR 0101 and the unit-tier load-rejection coverage;
slice 2 (`sqltrans-runtime`) delivers the camel-sql operational note, the
docker-suite runtime scenarios, and the idempotent-key disposition tests
(requirement "idempotent consumer composition"); slice 3
(`sqltrans-modes`) delivers the `Managed` join-required rework.
