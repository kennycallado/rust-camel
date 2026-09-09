# Proposal: sql-validate-target

## Why

The `sql:` prepare action (bd rc-25lup.1, commit 48a19bfc) seeds
data-at-rest through the booted `DatasourceCatalog`, but the integration
tier cannot ASSERT that state: every table check still steps outside the
harness. The papal design for epic rc-25lup (consult 2026-09-08) locked
the taxonomy — reads belong to a `validate` sql target, never mixed with
`prepare` mutations (the Citrus hard lesson). This change delivers epic
child 2/6: the assert sibling.

The assertion grammar reuses what already exists. ADR-0072 §3 ratifies
"same verbs, different subjects": a row set is another projection into
the `camel-matchers` algebra — per-cell `Expectation` verbs over
projected tuples, `CountBound` row-count bounds, and a poll-until-
deadline loop mirroring the partner poll driver, because database state
settles asynchronously exactly like partner request counts.

## What Changes

- New `ScenarioTarget::Sql(SqlTarget { datasource, query })` for
  `validate` documents. Load-time validation (ungated, both feature
  configurations): the `query` must be a read (`select`/`with` prefix —
  `is_read_statement`), else a `doc-validation` error naming the action
  index; `deadline` becomes valid on `sql` targets (partner precedent);
  `elapsedAtLeast` stays `lastReceived`-only.
- New `expectation` grammar for the sql target: optional `columns` projection
  by name, `rows` (a list of row tuples of per-cell expectations) or a
  row-count bound (`count` / `atLeast` / `atMost` / `atLeast`+`atMost`
  range — the partner count keys), exclusive; optional `unordered: true`
  (default ordered). Determinism: an ordered `rows` assertion whose
  query lacks `ORDER BY` emits a load-time WARN.
- `camel-matchers` gains `Expectation::Any` (grammar verb
  `{ignore: null}`, the wildcard — the `@ignore@` equivalent) and a pure
  `rows_match` (ordered zip; unordered via deterministic bipartite
  matching). Column projection stays at the call site (ADR-0072 §3).
- New `runner/sql_validate.rs` (feature `sql`, mirroring
  `partner_validate.rs`): resolves the pool through the threaded
  `DatasourceCatalog`, executes the query, maps cells to values
  (json-first for blobs, mirroring `reply_bytes_value`), and polls.
  Papal-mandated deviation from the partner lattice: SQL row sets are
  NOT monotone (a DELETE can shrink them), so nothing settles early —
  `above_ceiling` fails immediately, otherwise the final snapshot at
  deadline decides. Documented in the matcher doc-comments.
- Redaction (ADR-0051): mismatch diagnostics carry the datasource name,
  rendered bound, expected-versus-actual row counts, and column names —
  never the resolved `db_url` (`sanitize_db_error`) and never actual
  cell values.
- Affected crates: `camel-matchers`, `camel-integration-test`,
  `camel-cli` (symmetric e2e only). The `sql` / `integration-sql`
  feature wiring landed with child .1 — no new features.

Excluded: steering docs and the hermetic default datasource (child .3),
teardown/isolation idioms (.4), surrealdb parity (.5), waist extraction
(.6), query env-interpolation, statement binds, testcontainers,
transaction/isolation control, camel-mock grammar growth.

## Acceptance criteria

- A scenario seeds rows with `sql:` then asserts them with
  `validate {sql}` (ordered, unordered, wildcard, count bound) and
  passes; the same grammar fails closed on mutation queries, unknown
  fields, mixed row shapes, and unknown projection columns.
- A deadline poll passes when a row appears asynchronously within the
  window, and fails on the final snapshot otherwise.
- Mismatch and driver-error details contain neither `db_url` nor actual
  cell values (test-pinned).
- Ordered rows without `ORDER BY` produce exactly one load-time WARN.
- `cargo xtask validate` (fmt, clippy, lints) and the feature-`sql` test
  suite pass in the feature worktree.

## Risk budget

- Correctness over latency: the no-early-settle lattice makes deadline
  polls wait the full window (papal mandate) — acceptable for the test
  tier, authors use short deadlines.
- Unordered matching cost: bounded by deterministic polynomial
  bipartite matching (Kuhn augmenting paths), never factorial
  backtracking; `fetch_all` materialization stays test-tier-sized.
- Zero behavior change for documents that declare no sql target — the
  addition is purely additive to the grammar and enums (both
  `#[non_exhaustive]`).

Bd: rc-25lup.2
