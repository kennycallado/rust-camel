# Proposal: surreal-state-tier

## Why

The integration tier asserts data-at-rest through one state family today:
SQL. A `sql:` prepare action seeds state and a `validate` sql target reads
it back, both resolved through the boot's `DatasourceCatalog` by
datasource name (bd rc-25lup.1/.2, landed 2026-09-08/09). The epic
rc-25lup planned a second state family from the start: surrealdb
state-adapter parity (rc-25lup.5). The component side is ready —
`camel-component-surrealdb` already registers `SurrealDbPoolFactory`
under the same catalog seam and the same `datasource=<name>` URI
parameter — but the scenario grammar, the value mapping, and the
demand gate do not exist. The seam doc-comment in
`sql_validate.rs:339` records this as a known limitation "ahead of
surrealdb parity".

A second family also earns the rule-of-three for the waist extraction
queued as rc-25lup.6.

## What Changes

- Add a `surreal:` scenario prepare action: `{datasource: <name>,
  prepare: [SurrealQL write statements]}`, mirroring the `sql:` action.
- Add a `validate` surreal target: `{surreal: {datasource: <name>,
  query: <SurrealQL read>}}` with the shared expectation grammar
  (rows tuples, count bounds, `columns` projection, `unordered`,
  `deadline`).
- Define the SurrealQL read rule (read = `SELECT` prefix), the record
  projection into matcher tuples, and the redaction contract
  (ADR-0051) for both action families.
- Define the hermetic surreal tier: embedded `mem://` backend
  (workspace `surrealdb` gains `kv-mem`), fresh per boot by
  construction. Remote `ws://`/`http://` backends stay available
  through `db_url` steering; no new provisioning grammar.
- Demand-gate the family behind a `surreal` Cargo feature in
  `camel-integration-test` (forwarding `camel-bundles/surrealdb`),
  with an `integration-surreal` CLI gate mirroring `integration-sql`.
- Restore the `SQL prepare action` requirement to the canonical
  spec: the 2026-09-18 spec rebuild (commit 02eec1be) dropped it,
  leaving the landed `sql:` prepare grammar unspec'd. The delta
  restates it as the cross-family "State prepare actions" contract.

Excluded: component behavior changes in `camel-component-surrealdb`
beyond accepting the `mem` scheme and embedded engine features;
tier-3 container provisioning grammar (`testcontainer` /
`user-provided` stay reserved, ADR-0069 §9); waist extraction
(rc-25lup.6).

## Acceptance criteria

- A scenario document can seed SurrealQL state with a `surreal:`
  prepare action and assert it with a `validate` surreal target over
  an embedded `mem://` datasource, hermetically, in both feature
  configurations.
- The surreal family matches the sql seam point for point: named
  datasource resolution through the boot catalog, read/write split,
  matcher-tuple projection, poll semantics, redaction, teardown.
- Parity tests pin grammar, mapping, and isolation; the surreal
  feature stands alone in CI (build without `integration-http` or
  `integration-sql`).
- Canonical spec coverage restored: both prepare families and both
  state assertions live under `openspec/specs/integration-tier/`.

## Risk budget

Acceptable: new Cargo features and engine toggles confined to the
surreal surface; spec text that re-anchors the dropped prepare-action
requirement. Out of bounds: any change to the landed sql family's
observable behavior; new provisioning grammar; container dependencies
in the default test suite (ADR-0069 §8 default-suite rule).
