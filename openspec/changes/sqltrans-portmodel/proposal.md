# Proposal: sqltrans-portmodel

## Why

The blessed `sqltrans` family spec (15 requirements, 63 scenarios, 14 expert
rounds) was split by the e_opus ruling of 2026-11-20 into three slices so no
gate ever re-litigates the design. This change is **slice 1**: it lands the
transaction model and the `camel-api` port **dark** — every `transacted` block
is rejected at load with a typed "no TransactionManager for datasource X"
error, `Managed` keeps today's endpoint-creation rejection, and nothing runs at
runtime. Slices 2 and 3 add execution and mode-semantics changes on top.

## What Changes

- `camel-api`: object-safe `TransactionManager` + `TransactionSession` port,
  `TransactionError`, `TransactionOutcome`, `TransactionState`,
  `CamelError::Transaction`, re-exports. No sqlx dependency.
- `camel-core`: `TransactionManagerRegistry` (per-datasource-name lookup,
  fail-closed typed error when absent); `BuilderStep::Transacted`.
- `camel-dsl` / `camel-builder`: `transacted { datasource, steps }` authoring
  via YAML, fluent builder, and canonical form; canonical round-trip preserves
  every field; unknown keys rejected.
- Load-time rejections, fail-closed and typed, each naming offender and
  datasource: parallel split, multicast, wiretap, async handoff, seda escape,
  StreamList production inside a block reaching a joined sql step; sql step on
  another datasource; sql step without a named datasource; nested block;
  datasource with no registered manager.
- ADR 0101 (Accepted, family-wide): surface ruling, rejected surfaces, pool
  cost, slice order, at-least-once window; docs skeleton.
- Excluded (slices 2/3): executors, producer/consumer join, `plan_binds`
  changes, `Managed` join-required semantics, removal of the endpoint-creation
  rejection or Auto-fallback warnings, hooks, per-exchange transactions, XA/2PC.

## Acceptance criteria

- `partition.md` in the change dir: every family requirement owned by exactly
  one slice, same wording; "verbatim: no requirement reworded".
- Port compiles and is fully rustdoc'd; outcome/error types `#[non_exhaustive]`.
- Every load rejection has a unit-tier failing-input fixture asserting the
  exact variant, offender, and datasource (parser-level unknown-key asserts the
  key only).
- Dark proof: a `transacted` block fails at load with the named error; the
  existing `txnreject` tests stay green unmodified; nothing registers a manager.

## Risk budget

Additive model code only; no behavior change outside new `transacted` blocks.
The one-blessing-round cap applies (owner 2026-11-20); a semantic gate finding
parks `blocked: design` instead of fix-and-retry.

Bd: rc-qv2iz.5 (parent rc-qv2iz)
