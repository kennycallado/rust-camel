# Design: sql-validate-target

## Approach

Grammar-first, mirroring the landed `sql:` prepare action (bd
rc-25lup.1): raw serde shape → load-time validation (ungated, both
feature configurations) → typed model → feature-gated executor that
resolves the pool through the SAME `DatasourceCatalog` the booted
composition root built (`runner.rs` already threads
`Option<&Arc<dyn DatasourceCatalog>>` into `run_action`).

The assertion semantics reuse the partner poll driver's SHAPE
(`runner/partner_validate.rs`): snapshot loop at a fixed interval, one
decision lattice, mismatch renderers. The papal review (e_opus,
2026-09-09) mandated one deviation, recorded in the matcher
doc-comments: partner counts are monotone (arrivals only add), SQL row
sets are NOT (a DELETE shrinks them) — so nothing settles early.
`above_ceiling` still fails immediately (a breached ceiling at any
snapshot is a defect worth failing fast), and the final snapshot at
deadline decides everything else. Without a deadline, one immediate
snapshot decides (partner precedent).

Row algebra lives in `camel-matchers` (ADR-0072 §3: "parameterize the
algebra, not the observation"). The runner projects each `sqlx::AnyRow`
into a `Vec<serde_json::Value>` tuple at the call site — cell mapping:
NULL → `Null`, INTEGER → number, REAL → number, TEXT → string, BLOB →
json-first with lossy-UTF-8 fallback (mirroring `reply_bytes_value`,
`runner.rs`). The `columns` key projects by name before matching;
an unknown column name fails closed naming the column.

Unordered `rows` matching uses Kuhn's augmenting-path bipartite
matching over the boolean cell-compatibility matrix (each expected row
pattern versus each actual row): deterministic and polynomial — the
papal factorial-backtracking risk dissolves without arbitrary caps.

Determinism: an ordered `rows` assertion whose query lacks `ORDER BY`
(case-insensitive substring, string-literal-blind — v1 accepts the
false positive of a literal containing the words) emits exactly one
load-time `tracing::warn!`. The predicate is a pure function, tested
directly; the emission stays a one-line call.

Wildcard: `Expectation::Any` joins the shared dual grammar as the verb
`{ignore: null}` — null payload enforced like `exists`, no string
special-casing of Citrus's `@ignore@`. `expectation_matches` returns
true for any value including null.

## Affected crates

- `camel-matchers`: `Expectation::Any` + `expectation_matches` arm;
  `RowsExpectation` (columns, unordered, rows XOR bound); pure
  `rows_match`; doc-comment notes on the non-monotone SQL lattice
  (doc-comments today bake in "arrivals only add").
- `camel-integration-test`: `document.rs` (`ScenarioTarget::Sql`,
  `ValidateExpectation::Rows`, `RawValidate` deadline gate widening,
  `sql_expectation_from_value`, `ORDER BY` WARN, bindings arm);
  `runner/sql_validate.rs` (new, feature `sql`: pool resolution, query,
  projection, poll, mismatch renderers); `runner.rs` dispatch arm;
  `lib.rs` re-exports. No `boot_scenario.rs` change — catalog threading
  landed with child .1.
- `camel-cli`: symmetric e2e test only (the `integration-sql` feature
  and CLI wiring landed with child .1).

## Architecture boundaries

Test-tier only: no Runtime, DSL, Components, Services, or Languages
change. `camel-matchers` keeps its purity law (ADR-0072 §2: zero camel
deps, no async, no features; the row algebra adds types and pure
functions only). Grammar and demand-gating follow ADR-0069 §2 (mixing
ban untouched — the sql target is a `validate` target, not a new
action) and §8's pattern as applied to `sql` by child .1 (the feature
and CLI wiring landed there). Redaction follows
ADR-0051: `sanitize_db_error` on driver errors; mismatch details carry
datasource name, rendered bound, row counts, and column names — never
`db_url`, never actual cell values. Query text is doc-authored operator
config (ADR-0032 trust boundary: trusted), and its execution is
deadline-bounded; row data never reaches a control-plane action.

## Phases

### Phase 1: matcher algebra
- **Goal:** `Expectation::Any`, `RowsExpectation`, `rows_match` with
  unit coverage.
- **Dependencies:** none (pure crate).
- **Externally-visible types/interfaces:** the three additions above.
- **Deliverable:** commit on `feature/validate-sql`.
- **Exit-criteria:** `camel-matchers` tests green; doc-comments record
  the non-monotone caveat.

### Phase 2: document grammar
- **Goal:** `ScenarioTarget::Sql` parse+validation, expectation grammar,
  deadline gate widening, ORDER-BY WARN; parse-test matrix.
- **Dependencies:** Phase 1 types.
- **Deliverable:** commit.
- **Exit-criteria:** doc parse tests green in BOTH feature
  configurations (ungated law).

### Phase 3: executor + e2e
- **Goal:** `runner/sql_validate.rs` poll executor with redaction;
  runner dispatch; CLI e2e.
- **Dependencies:** Phases 1-2; `sql` feature (sqlx).
- **Deliverable:** commit.
- **Exit-criteria:** feature-`sql` suite green (hermetic
  `sqlite::memory:?cache=shared`), redaction test-pinned, e2e passes.

## Alternatives considered

- Local row algebra in `camel-integration-test` until the waist
  (rc-25lup.6): rejected — ADR-0072's charter is the algebra home; the
  waist defers the cross-family TRAIT, not pure functions (papal
  ruling, QUESTION 1).
- Citrus-literal `"@ignore@"` string: rejected — special-cases string
  equality inside the dual grammar (papal QUESTION 2).
- Heuristic credential-ish column masking in diagnostics: rejected —
  false negatives; elide all actual cells (papal QUESTION 4).
- Early-settle on matching snapshots (the partner lattice verbatim):
  rejected as unsound under DELETE (papal mandated change 1-2).
