# Proposal: sql-prepare-action

## Why

The integration tier proves REST-side behavior (traffic peers, HTTP partner
grammar) but cannot prove data-at-rest behavior: seeding tables, schema DDL,
or cleanup between scenarios. Tests that want DB state today must step outside
the harness — a parallel sqlx dependency in the test crate that bypasses the
booted composition root, so the pool the routes use and the pool the test
uses are different pools (on `sqlite::memory:` they are different databases).

The papal design for epic rc-25lup (bd `rc-25lup`, consult 2026-09-08) locked
the taxonomy: state-at-rest is a separate adapter branch from traffic peers.
It is NOT a PartnerAdapter and NOT an endpoint. It is a new scenario action
that reads through the booted context's `DatasourceCatalog` (Citrus
precedent: actions over a configured datasource). This change delivers child
`.1`: the `sql:` prepare action plus the shared-cache SQLite lint. Row
assertions (`validate {sql}`, child `.2`), steering docs and the hermetic
default (child `.3`), and teardown idioms (child `.4`) are out of scope.

## What Changes

- New scenario action `sql:` with two fields: `datasource` (a configured
  datasource NAME, identifier law: never interpolated) and `prepare` (an
  ordered list of non-SELECT SQL statements executed sequentially, stopping
  at the first error).
- Load-time document validation: `prepare` statements with a `select` or
  `with` prefix (case-insensitive, after trim) are rejected — reads belong
  to the `validate` target grammar (child `.2`), never mixed with mutations
  (Citrus hard lesson). An empty `prepare` list is rejected. Statement text
  is NOT env-interpolated in v1.
- Execution: the action resolves `datasource` through the SAME
  `DatasourceCatalog` the booted composition root built (`camel_bundles::
  boot`), so one datasource name means one pool. `camel_bundles::BootHandle`
  gains a public `datasource_catalog()` accessor (always populated; `ScenarioRun`
  already carries the handle). The pool handle downcasts to `sqlx::AnyPool`
  and statements run via `sqlx::query(..).execute(..)`.
- Boot-time lint `sql-memory-not-shared`: after the sealed config load in
  `boot_scenario`, any configured datasource whose `db_url` is SQLite
  in-memory (`sqlite::memory:` or `sqlite://:memory:`, case-insensitive)
  without `cache=shared` fails boot. The trap:
  SQLite `:memory:` is per-connection and the pool default is 5 connections —
  INSERT and SELECT can hit different databases, a silent green lie. The
  check is a pure string check, ungated (it fires with or without the `sql`
  feature, with or without a `sql:` action), mirroring the ungated security
  config gate.
- Demand-gated activation (ADR-0069 §8): a `sql` Cargo feature on
  `camel-integration-test` (optional `sqlx` dependency,
  `sqlite` + `any` drivers, mirroring the `http` feature shape) and an
  `integration-sql` feature on `camel-cli` mapping to
  `camel-integration-test/sql`, included in the CLI default like
  `integration-http`.
- Redaction (ADR-0051): action failures carry the datasource NAME, the
  failing statement INDEX, and ADR-0051-sanitized database error text —
  never the resolved `db_url` and never row values. Execution uses the
  non-fetching execute path, so any row results the driver produces are
  discarded and never appear in diagnostics.

## Capability

`integration-tier` — extends the scenario action grammar (Ordered scenario
actions) with the state-at-rest action and adds the SQLite shared-cache
mandate.

## Impact

- Files: `crates/camel-integration-test/src/sql_action.rs` (new: raw serde,
  validation, executor), `document.rs` (minimal: enum variant + raw arm +
  validation hook), `runner.rs` (dispatch arm), `boot_scenario.rs` (lint +
  catalog threading), `Cargo.toml` (feature), `crates/camel-bundles/src/lib.rs`
  (BootHandle accessor), `crates/camel-cli/Cargo.toml` (integration-sql).
- Emergency: none — additive action. Existing documents keep today's
  behavior except one intended change: a bare SQLite in-memory datasource
  (`sqlite::memory:` without `cache=shared`) that booted before now fails
  with `sql-memory-not-shared` (the silent green-lie trap, surfaced).
- Non-goals v1: `validate {sql}` row projection and poll (child .2),
  hermetic default datasource wiring (child .3), teardown/isolation idioms
  (child .4), surrealdb (child .5), waist extraction (child .6), statement
  env-interpolation, testcontainer provisioning, transaction/isolation
  control, postgres/mysql driver enablement beyond what sqlx compiles in.
