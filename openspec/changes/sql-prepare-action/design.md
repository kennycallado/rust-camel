# Design: sql-prepare-action

Papal-locked architecture (bd rc-25lup, consult 2026-09-08): state-at-rest is
a new scenario action over the booted `DatasourceCatalog` — NOT a
PartnerAdapter, NOT an endpoint. Scope rulings 1–7 approved by conductor
2026-09-08.

## Context

- Grammar lives in `camel-integration-test/src/document.rs`
  (`ScenarioAction`, `#[non_exhaustive]`, raw serde stage with
  `deny_unknown_fields` + camelCase, typed `DocError`).
- Boot: `boot_scenario` loads `<root>/Camel.toml` sealed, prepares the
  context, runs `camel_bundles::boot` which builds
  `RuntimeDatasourceCatalog::new(config.datasources)` and registers the
  sqlx `PoolFactory` through the SQL bundle; `ScenarioRun { ctx, boot }`
  carries the `BootHandle`.
- Pool: `SqlPoolFactory` creates `sqlx::AnyPoolOptions` pools with
  `max_connections` default 5 — the per-connection `:memory:` trap.
- Steering seam already exists: `[datasources.<name>]` in Camel.toml is a
  strict env-interpolation surface (STRICT_PREFIXES, camel-config
  config.rs:2997); `${env:}` resolves through `LayeredEnv` at sealed load.
  Zero new steering code.

## Goals / Non-Goals

Goals: `sql:` prepare action end to end (grammar, validation, execution,
feature gate); `sql-memory-not-shared` boot lint; single-catalog execution;
ADR-0051 redaction shape.

Non-goals: validate{sql}/poll (.2), hermetic default wiring + steering docs
(.3), teardown idioms (.4), surrealdb (.5), waist (.6), statement
env-interpolation, testcontainer, tx/isolation, second driver enablement
beyond compiled-in sqlx drivers.

## Phases

- **Phase 1 — Foundations behind the gate (Tasks spa-1..spa-3)**:
  BootHandle accessor, sql_action.rs substance, boot lint. Zero
  `document.rs` edits; executable and testable without the concurrent
  document.rs sweep landing. Exit criteria: camel-bundles and
  camel-integration-test tests green in both feature configurations,
  nothing in the forbidden-zone files touched.
- **Phase 2 — Grammar wiring (Task spa-4)**: enum variant + raw arm +
  validation hook in document.rs (minimal, conductor-gated until the
  scenario-tier-p3-sweep lands and this worktree rebases), runner
  dispatch, `sql`/`integration-sql` features, end-to-end scenario tests.
  Exit criteria: all delta-spec scenarios executed by tests; acceptance
  greps and clippy/fmt clean.

## Decisions

### D1 — Grammar (ruling 1: minimal document.rs touch)

`ScenarioAction::Sql { datasource: String, prepare: Vec<String> }` — enum
variant, raw serde arm, and a validation hook in document.rs; everything
else (raw struct, validation logic, executor) lives in a new
`sql_action.rs` module. YAML shape:

```yaml
scenario:
  - sql:
      datasource: appdb
      prepare:
        - "CREATE TABLE IF NOT EXISTS t (id INTEGER PRIMARY KEY, v TEXT)"
        - "INSERT INTO t (v) VALUES ('seed')"
```

`datasource` is an identifier (rc-4hexo parity): plain string, never passed
through env interpolation. `prepare` statements are raw SQL, not
env-interpolated (v1 simplification; the epic reserves no statement
interpolation). The name `prepare` is the Citrus phase name (seed before
traffic), not SQL statement preparation: statements execute as ad-hoc SQL
with no bind parameters in v1. One statement per list item — a string
carrying `;`-separated statements fails at execute, naming its index. Unknown fields rejected by the existing raw serde stage.
Statements with `select`/`with` prefix (case-insensitive, after trim,
leading `(` skipped) are load-time `doc-validation` errors naming the action
index and statement index; an empty `prepare` is a load error too.

### D2 — Execution seam (ruling 3)

`BootHandle` (camel-bundles) gains a private field holding the
`Arc<dyn DatasourceCatalog>` it already builds, exposed as
`pub fn datasource_catalog(&self) -> Arc<dyn DatasourceCatalog>`. No boot
signature change; camel-cli/camel-run callers unaffected. The runner reads
`run.boot.datasource_catalog()`, `get_pool(name)`, downcasts to
`sqlx::AnyPool`, executes statements sequentially
(`sqlx::query(stmt).execute(&*pool)`), stops at the first error. One name =
one catalog = one pool. Feature-gated (`sql`) like the HttpPartner adapter:
without the feature the executor compiles out and a `sql:` action fails at
load with a named demand-gate error mirroring `inbound:` behavior.

### D3 — Feature gate (ruling 7)

`camel-integration-test`: optional `sqlx` dep
(`runtime-tokio`, `any`, `sqlite`, `chrono` features — hermetic tier needs
sqlite only; other compiled-in drivers work through `install_default_drivers`
best effort), feature `sql = ["dep:sqlx"]` mirroring `http = ["dep:.."]`.
`camel-cli`: `integration-sql = ["camel-integration-test/sql"]`, added to
the default list beside `integration-http`. Default suite runtime unchanged
(the itest sql tests are `sql`-feature-gated).

### D4 — The lint (ruling 2)

`boot_scenario.rs`, right after the sealed config load, before context
preparation: for every configured datasource whose `db_url` starts with
`sqlite::memory:` or `sqlite://:memory:` (case-insensitive scheme) and
whose query string lacks `cache=shared`, fail with error class
`sql-memory-not-shared`, naming the datasource NAME (never the URL).
Pure string check: ungated by the `sql` feature and independent of whether
any `sql:` action exists — the trap lives in the pool, which routes can hit
through the SQL component without any scenario action. Precedent: the
ungated tier security gate (config-shape check).

### D5 — Redaction (ruling 4)

Failure messages: `datasource 'appdb' statement [i]: <ADR-0051-sanitized
database error text>`. Never the resolved or unresolved `db_url`
(DatasourceConfig Debug already redacts; the executor never formats the
URL), never row values — prepare executes through the non-fetching execute
path, so any row results the driver produces (SQLite
`INSERT .. RETURNING`) are discarded and never appear in diagnostics.
Tests for the failure path embed `db_url` and row-value sentinels inside
the failing statement's error content and assert neither sentinel appears
in the diagnostic. Unknown-datasource failure names the missing name and
lists nothing else.

### D6 — Non-SELECT enforcement (ruling 5)

Load-time only, prefix test after trimming whitespace and leading `(`:
`select` and `with` (CTEs read) are rejected. The executor does not
re-check (belt off: the grammar cannot reach it with reads, and re-parsing
SQL in the executor duplicates the rule).

## Risks / Trade-offs

- Prefix-based read detection accepts exotic reads (e.g. `VALUES`-only
  queries) — acceptable: prepare's contract is mutations/DDL; misses are
  lint weaknesses, not correctness bugs (a read that returns rows would
  error at execute or be ignored by sqlx execute path).
- `cache=shared` string detection: no URL parsing library — matches the
  mandate's exact remedy (`?cache=shared`), documented in the error text.
  `sqlite://file::memory:?cache=shared` URI forms also accepted when the
  `cache=shared` query param is present.
- Two catalogs would resurrect the trap — D2's single-catalog rule is the
  load-bearing invariant; a scenario executor building its own pool is a
  review-reject.
- Parallel scenarios sharing one shared-cache memory DB: known limitation
  (epic: per-boot unique memory URI when parallel lands, child .4/.6
  territory).

## Migration Plan

Additive grammar. Existing documents unchanged. Builds without `sql` keep
today's behavior except the ungated lint (which only fires on
sqlite::memory: without cache=shared — previously a silent trap, now a named
error; that is the intended behavioral change).

## Open Questions

None — rulings 1–7 settled by conductor adjudication 2026-09-08.

### Self-grill record

**Questions generated:**
1. [glossary] Does `prepare` collide with existing vocabulary — sqlx
   "prepared statements" (bind parameters) vs the action field name?
2. [sharpen] "prepare never returns rows" — SQLite `INSERT .. RETURNING`
   returns rows; is the redaction claim still exact?
3. [scenario] Constructed inputs: `  ( select 1 )`, write-CTEs, one string
   with `;`-separated statements, `cache=private`, uppercase
   `SQLITE::memory:` — which break the contract?
4. [cross-ref] Do the cited seams exist in code today (inbound demand-gate
   precedent, BootHandle shape, STRICT_PREFIXES, itest features, sqlx
   workspace dep)?

**Answers (with citations):**
1. [glossary] Two concepts near one word; papal epic text and Citrus use
   "prepare" as the seed phase (bd rc-25lup description). Sharpened design
   D1: no bind parameters, ad-hoc execution (`document.rs` ADR-0069 §11
   action list is the receiving vocabulary; sqlx prepared statements are a
   different mechanism, unused here).
2. [sharpen] `sqlx::query(..).execute()` is the non-fetching path; driver
   row results are discarded. Reworded D5 and the spec clause to "row
   results are discarded and never appear in diagnostics" (camel-sql
   producer/query.rs uses the same split).
3. [scenario] `( select 1 )` → trim + leading-`(` skip rejects it;
   write-CTEs rejected by `with` prefix — documented lint weakness
   (Postgres-only form, v1 target is SQLite); multi-statement strings fail
   at execute naming the index (one-statement-per-item documented);
   `cache=private` lacks `cache=shared` → rejected; uppercase scheme →
   case-insensitive check rejects (design D4).
4. [cross-ref] inbound-without-feature named load error: document.rs module
   doc (`Provisioning runs behind the http feature; a declaration in a
   build without the feature is a named load error`); BootHandle fields
   `{jms_pool, cxf_pool}` at camel-bundles lib.rs:71-74 with catalog built
   at lib.rs:241 inside `boot()`; STRICT_PREFIXES at
   camel-config/src/config.rs:2997; itest features `security`/`http` at
   camel-integration-test/Cargo.toml:61-77; `sqlx = "0.8"` at workspace
   Cargo.toml:255 with camel-sql feature gates at its Cargo.toml:18-25.

**Outcome:** refine — D1 disambiguation + one-statement-per-item rule;
D5/spec "discarded, never echoed" wording. No open questions.
**Self-grill mode:** self-grill-proposals skill
