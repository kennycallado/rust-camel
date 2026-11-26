## MODIFIED Requirements

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

This rejection set SHALL hold on EVERY lowering entry point — the
declarative compile and the canonical lowering alike — through ONE shared
structural walk over the lowered step tree; no entry point may carry a
second copy of the list over the lowered step tree, and a structural
rejection on the canonical path SHALL be produced whether or not a
TransactionManager is registered for the block datasource — neither
replaced by the `ManagerMissing` backstop nor skipped when the manager gate
would pass. A dynamic-routing step (recipient list, routing slip, or
dynamic router) inside a block SHALL be rejected at load: its target is
chosen by an expression evaluated at exchange time, so membership in the
block cannot be proven statically (fail-closed default); the canonical form
cannot author such a step at all.

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

#### Scenario: canonical lowering rejects a parallel split despite a registered manager

- **GIVEN** a canonical `CanonicalStepSpec::Transacted` block containing a parallel split whose fragments reach a joined sql step, with a TransactionManager registered for the block datasource
- **WHEN** the canonical lowering runs
- **THEN** the lowering fails at load with the typed parallel-inside-block error (never `ManagerMissing`)

#### Scenario: both lowering entries share one structural walk

- **GIVEN** the declarative compile entry and the canonical lowering entry
- **WHEN** either entry loads a block containing the same structural offence
- **THEN** both fail with the same typed error variant (source-level assertion that both entries call the one walk module)

#### Scenario: dynamic-routing step inside a block is rejected at load

- **GIVEN** a `transacted` block containing a recipient list, routing slip, or dynamic router step
- **WHEN** the route is loaded through the declarative lowering entry (the canonical form cannot author such a step — it is rejected at parse as an unknown variant)
- **THEN** the load fails with the typed dynamic-target-unprovable error naming the step kind

Slice 2 (`sqltrans-runtime`) owns the runtime join-side rejections of
requirement "Join semantics for sql steps on the block datasource"
(`RuntimeSqlInJoinedStep`, `TransactionControlSql`, `JoinSessionMissing`,
`JoinScopeMismatch`, `DialectUnprovable`, `UnsupportedJoinDialect`); this
slice owns every load-time structural rejection above.

A `RouteDefinition` registered already lowered
(`CamelContext::add_route_definition`) passes no lowering entry and is
outside this clause until the shared walk runs there (ADR-0101 §h).
