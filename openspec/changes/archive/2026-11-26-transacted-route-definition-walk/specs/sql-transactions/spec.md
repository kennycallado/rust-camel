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

This rejection set SHALL hold on EVERY entry point that admits a
`transacted` block — the declarative compile, the canonical lowering, and
the internal registration seam for an already-lowered `RouteDefinition`
alike — through ONE shared structural walk over the lowered step tree, for
every offence representable as a lowered `BuilderStep` at that entry
(unknown block keys remain parser-only; the canonical form cannot author a
dynamic-routing step); no
entry point may carry a second copy of the list over the lowered step tree,
and a structural rejection on the canonical path SHALL be produced whether
or not a TransactionManager is registered for the block datasource — neither
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

#### Scenario: all entries share one structural walk

- **GIVEN** the declarative compile entry, the canonical lowering entry, and the internal registration seam
- **WHEN** any entry loads a block containing the same structural offence
- **THEN** all fail with the same typed error variant produced by the one walk module (source-level assertion that all entries call it)

#### Scenario: dynamic-routing step inside a block is rejected at load

- **GIVEN** a `transacted` block containing a recipient list, routing slip, or dynamic router step
- **WHEN** the route is loaded through the declarative lowering entry (the canonical form cannot author such a step — it is rejected at parse as an unknown variant)
- **THEN** the load fails with the typed dynamic-target-unprovable error naming the step kind

Slice 2 (`sqltrans-runtime`) owns the runtime join-side rejections of
requirement "Join semantics for sql steps on the block datasource"
(`RuntimeSqlInJoinedStep`, `TransactionControlSql`, `JoinSessionMissing`,
`JoinScopeMismatch`, `DialectUnprovable`, `UnsupportedJoinDialect`); this
slice owns every load-time structural rejection above.

## ADDED Requirements

### Requirement: Registration-seam walk for pre-lowered route definitions

The system SHALL run the shared transacted-block walk at the internal route
registration seam (`RuntimeBus::register_route` → `handle_register_internal`)
over an already-lowered `RouteDefinition`, with registry-backed manager
resolution and catalog-backed dialect resolution, BEFORE the runtime side
effect and BEFORE the duplicate-route check. The scan SHALL enter every
outermost `BuilderStep::Transacted` reachable in the definition's step tree,
including the circuit-breaker fallback; a block nested inside another block
remains the shared walk's structural `NestedBlock` rejection. The seam SHALL
preserve the typed `TransactionError` variant produced by the scan across the
internal registration port — it SHALL NOT be stringified into `RouteError` —
while every non-scan registration failure keeps its existing formatting
(`invalid state: …`). The `JoinSessionMissing` route-add backstop SHALL
remain in force unchanged until the executor slice removes it deliberately;
its surface stays as today (mapped to `RouteError` by the unchanged runtime
execution adapter). A definition with no transacted block SHALL register
exactly as before.

#### Scenario: registration rejects a pre-lowered parallel split

- **GIVEN** a pre-lowered `RouteDefinition` whose `BuilderStep::Transacted` block on datasource `pg` contains a parallel split reaching a joined sql step, with no TransactionManager registered
- **WHEN** the definition is registered through `CamelContext::add_route_definition`
- **THEN** registration fails with `TransactionError::ParallelInBlock { step: "split", datasource: "pg" }` and no route aggregate is persisted

#### Scenario: registration rejects a pre-lowered dynamic routing target

- **GIVEN** a pre-lowered `RouteDefinition` whose `circuit_breaker_fallback` holds a container wrapping a `BuilderStep::Transacted` block that contains a recipient list
- **WHEN** the definition is registered
- **THEN** registration fails with `TransactionError::DynamicTargetUnprovable { step: "recipient-list" }`

#### Scenario: registration reports a missing manager as a typed error

- **GIVEN** a pre-lowered `RouteDefinition` whose clean block on datasource `pg` has no registered TransactionManager
- **WHEN** the definition is registered
- **THEN** registration fails with `TransactionError::ManagerMissing { datasource: "pg" }` (asserted as the variant, not as prose), and the `JoinSessionMissing` backstop is never reached

#### Scenario: registration of a plain pre-lowered route is unchanged

- **GIVEN** a pre-lowered `RouteDefinition` with no transacted block
- **WHEN** the definition is registered
- **THEN** registration succeeds and the route reaches `Registered` state
