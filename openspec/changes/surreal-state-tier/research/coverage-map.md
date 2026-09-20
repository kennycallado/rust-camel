# Research: surreal coverage map (rc-25lup.5)

Evidence base: worktree `feature/surrealtier` @ 62d41222. All
citations are file:line against that tree. Written 2026-09-20.

## 1. What the scenario tier has today (sql family — landed)

The integration tier asserts data-at-rest through exactly one state
family, SQL:

- `sql:` prepare action — `crates/camel-integration-test/src/sql_action.rs:22`
  defines the `sql` action key; `:34-36` the raw shape
  (`datasource`, `prepare: Vec<String>`); `:49` `is_read_statement`
  (read = trimmed `select`/`with` prefix, leading-`(` tolerant);
  `:65-81` load validation (reads and empty lists are load errors);
  `:88` `sanitize_db_error` (ADR-0051); `:95+` the ungated
  `sql-memory-not-shared` boot lint.
- `validate` sql target —
  `crates/camel-integration-test/src/runner/sql_validate.rs` (381
  lines): rows/bound expectation grammar, `columns` projection,
  `unordered` multiset matching, no-early-settle poll, ceiling-breach
  fail-fast, redaction. `:339` documents the seam's forward promise:
  "a documented limitation of the any-tier mapping ahead of surrealdb
  parity (bd rc-25lup.2)".
- Catalog seam — `crates/camel-integration-test/src/runner.rs:37`
  imports `camel_api::datasource::DatasourceCatalog`; `:411-412` and
  `:434` thread the booted catalog into the executor; `:648-653` the
  no-catalog fail-closed arm; `:662` prepare execution.
- Grammar parse — `crates/camel-integration-test/src/document.rs:149`
  the `ScenarioAction` enum (no surreal arm); `:191` the poll
  deadline "only valid on `partner` and `sql`" rule; `:206-209` the
  sql target and its read/write vocabulary split, demand-gated
  behind the harness `sql` feature.
- Feature gate — `crates/camel-integration-test/Cargo.toml:97-106`:
  `sql = ["dep:sqlx", "camel-bundles/sql"]` (the bundles forward is
  the lint-gate-forwarding Rule 1 obligation);
  `src/sql_stub.rs:30,66,97` the feature-off stub catalog and seed.
- Canonical spec — `openspec/specs/integration-tier/spec.md:1244`
  "SQL state assertion", `:1174` "SQLite shared-cache mandate",
  `:1441` "Scenario datasource teardown".
- Docs — `docs/src/testing/scenario-sql.md` (grammar page);
  `docs/src/testing/index.md:368-376` datasource steering
  (`[datasources.appdb] provider = "sqlx"`, `db_url` env
  interpolation).
- CI — `integration-sql` independence job (bd rc-7lrl2, archived
  change `2026-09-09-scenario-sql-isolation`).

## 2. What the surreal side has (component tier — landed, scenario
tier — absent)

The component layer is ready and already speaks the same seam:

- `crates/components/camel-component-surrealdb/src/pool_factory.rs:83`
  `SurrealDbPoolFactory` implements `PoolFactory`; `:176-179`
  `supported_schemes` = `["ws", "wss", "http", "https"]` — remote
  transports only, no embedded engine; `:21` `redact_db_url`
  (ADR-0051 pattern from camel-sql); `:49+` one `Surreal<Any>` client
  per datasource with signin → use_ns → use_db setup.
- `src/config.rs:16-29` the twelve `SurrealDbOperation` variants;
  `:70-92` the endpoint config parsed from
  `surrealdb:<op>?datasource=<name>&...` — the same
  `datasource=<name>` parameter convention the scenario grammar
  uses; `:91` `allow_dynamic_query` (fail-fast in `validate()`), the
  lever behind the scenario-startup fail-closed rule
  (`openspec/specs/integration-tier/spec.md:934-941`, sql arm).
- `src/query.rs:1-60` identifier validation and raw SurrealQL
  passthrough for the `query` operation.
- Registration — `crates/camel-bundles/Cargo.toml:53,74,93`: the
  `surrealdb` feature is optional and in `default`;
  `crates/camel-bundles/src/lib.rs:388-399` registers the bundle's
  factories into the boot's `RuntimeDatasourceCatalog`; `:113-114`
  the deadline-wrapped `datasource_catalog.close_all()` teardown the
  sql teardown requirement rides on.
- CLI — `crates/camel-cli/Cargo.toml:227` the `surrealdb` feature;
  `:160-166` the BUSL-1.1 flavor note (excluded from `flavor-regular`,
  included in `flavor-full`); `:143` `full` includes `surrealdb`. No
  `integration-surreal` gate exists.
- Workspace dependency — `Cargo.toml:165`:
  `surrealdb = { version = "3", default-features = false, features =
  ["protocol-ws", "protocol-http", "rustls"] }`. No `kv-mem` (or any
  embedded engine feature) is compiled anywhere: the embedded
  hermetic tier is unavailable today.
- Harness — `crates/camel-integration-test` contains no surreal
  reference at all (grep over `src/` matches only the
  `sql_validate.rs:339` doc-comment).
- Test-side real-instance precedent —
  `crates/camel-test/tests/support/surrealdb.rs:12-46`: a shared
  `surrealdb/surrealdb:v3.1.4` testcontainer bound to a WebSocket
  endpoint, started once per process; deps in
  `crates/camel-test/Cargo.toml:41-42`;
  `examples/surrealdb-example/Cargo.toml` uses testcontainers the
  same way. This is the tier-3 shape, outside scenario documents.

## 3. What is missing (this change's surface)

1. No `surreal:` scenario prepare action (`document.rs:149` has no
   arm; `sql_action.rs` has no surreal twin).
2. No `validate` surreal target; the deadline-validity set is
   partner + sql only (`document.rs:191`;
   `openspec/specs/integration-tier/spec.md:535-536,565`).
3. No SurrealQL read rule, record-to-matcher-tuple projection, or
   surreal redaction contract in the tier.
4. No `surreal` feature in `camel-integration-test` (the sql twin is
   `Cargo.toml:97-106`); no `integration-surreal` CLI gate or CI job
   (ADR-0069 §8 ladder names http, sql, ws, grpc — not surreal;
   `docs/adr/0069-integration-tier-testing-contract.md:249-274`).
5. No embedded engine: factory schemes exclude `mem`
   (`pool_factory.rs:176-179`); workspace dep lacks `kv-mem`
   (`Cargo.toml:165`). The hermetic in-process tier does not exist.
6. No surreal freshness/teardown scenario coverage (the teardown
   requirement is sql-only text,
   `openspec/specs/integration-tier/spec.md:1441-1465`).
7. No docs page (no `scenario-surreal.md`; `docs/src/testing/`).

## 4. Spec drift found (repairs riding along)

The canonical `integration-tier` spec was rebuilt wholesale on
2026-09-18 (commit 02eec1be, 1562 insertions — the file's only
commit in history). The rebuild dropped the "SQL prepare action"
requirement that `openspec/changes/archive/2026-09-08-sql-prepare-action/specs/integration-tier/spec.md`
archived (its scenarios and normative text exist only there; grep of
`openspec/specs/` finds no "prepare action" requirement). The
"Demand-gated activation and CI isolation" requirement
(`spec.md:324-331`) also names only `http`, though `integration-sql`
landed (rc-7lrl2). Consequence: the landed `sql:` prepare grammar
has no canonical requirement. This change restores the coverage as
the cross-family "State prepare actions" requirement and updates the
activation requirement to name all three gates — no behavior change,
spec-only repair.

## 5. Premise check

Fresh. No archived or active openspec change covers surreal scenario
grammar (archive grep for surreal: none; active changes are
cache/guide work). bd rc-25lup.5 is open, P3, acceptance = seam
parity. The component seam (`datasource=<name>`, catalog
registration) matches the bd's premise. No STOP condition.
