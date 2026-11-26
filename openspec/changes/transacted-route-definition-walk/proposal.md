# Proposal: transacted-route-definition-walk

## Why

The transacted block's fail-closed load walk runs at both LOWERING fan-ins:
the declarative compiler (`camel-dsl/src/compile.rs`) and the canonical
lowering (`camel-core/src/lifecycle/application/commands.rs`) share the ONE
visitor `camel_core::transacted_walk`. A `RouteDefinition` that reaches
registration already lowered — the fluent builder path
`RouteBuilder::transacted` → `CamelContext::add_route_definition` →
`RuntimeBus::register_route` → `handle_register_internal` — passes no
lowering entry, so no walk runs. Such a hand-authored block skips every
structural and per-arm rejection, and skips manager and dialect resolution.

Today the block still fails closed at route-add through the
`StepCompilerRegistry` dark gate (`TransactionError::JoinSessionMissing`),
but that error is stringified at the registration port. The executor slice
`rc-ggo3d` removes that backstop; before it does, the registry-backed walk
must run at the registration seam or a builder-authored block with
parallel-split / multicast / wiretap / async / seda / StreamList / foreign /
unnamed / nested / dynamic offences would execute unvalidated.

## What Changes

- Add a route-definition scan to `camel-core/src/transacted_walk.rs`: it
  finds each outermost `BuilderStep::Transacted` in `RouteDefinition::steps`
  and `RouteDefinition::circuit_breaker_fallback` and runs the SAME
  `walk_transacted_block` visitor with registry- and catalog-backed
  resolvers.
- Call the scan at the registration seam `handle_register_internal` before
  the runtime side effect (and before the duplicate check, mirroring the
  canonical `handle_register` contract-first order).
- Make the registration seam preserve the typed error: the internal
  `RouteRegistrationPort::register_route` returns `CamelError` instead of
  stringifying through `DomainError::InvalidState`.
- Update ADR-0101 §h to the exact new truth: the registration seam now runs
  the registry-backed walk.
- Explicitly NOT changed: the `JoinSessionMissing` dark gate in
  `StepCompilerRegistry::compile_step` (rc-ggo3d removes it deliberately),
  the two existing lowering fan-ins, the `TransactionError` taxonomy, any
  execution behavior.

## Affected crates

- `camel-core`: `transacted_walk` scan + `handle_register_internal` call +
  `RouteRegistrationPort` error type + tests.
- `docs/adr/0101-transacted-block-surface.md` (prose truth update).

## Acceptance criteria

- The five mission-named tests exist and pass with these fixtures:
  - `route_definition_block_rejects_parallel_split`: pre-lowered block on
    `pg` with a parallel split reaching `sql:...?datasource=pg`; no manager
    registered; `add_route_definition` returns
    `ParallelInBlock { step: "split", datasource: "pg" }` and no aggregate is
    persisted.
  - `route_definition_block_rejects_dynamic_uri`: the block sits under a
    container inside `circuit_breaker_fallback`; it contains a recipient
    list; registration returns
    `DynamicTargetUnprovable { step: "recipient-list" }`.
  - `route_definition_block_reports_missing_manager`: clean block on an
    unregistered `pg` returns `ManagerMissing { datasource: "pg" }` as the
    `TransactionError` variant, never prose.
  - `route_definition_plain_route_still_registers`: plain pre-lowered route
    registers and reaches `Registered`.
  - `registered_manager_dark_gate_no_block_compiles` passes unchanged.
- ADR-0101 §h states the seat where the walk now runs.

## Risk budget

- No execution-path change; the walk is synchronous and pure.
- A declarative-compiled route that already passed its (deferred) walk is
  re-validated at registration; the second pass reuses the structural checks
  and, with a registered manager, additionally runs dialect resolution. A
  registered manager with no catalog or no URL entry for the datasource
  fails `DialectUnprovable` at this seam, not the `JoinSessionMissing` dark
  gate.
- The dark gate keeps the last word at route-add until `rc-ggo3d`; no route
  with a block becomes executable in this change.
