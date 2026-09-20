# Design: surreal-state-tier

## Approach

Mirror the landed SQL family point for point; add nothing the seam
does not already promise. Four decisions carry the design.

**1. Grammar parity, second family.** A `surreal:` prepare action
(`{datasource: <name>, prepare: [...]}`) and a `validate` surreal
target (`{surreal: {datasource: <name>, query: <read>}}`) reuse the
sql shapes verbatim. The read rule is the same prefix law scoped to
the SurrealQL vocabulary: a validate query is a read iff its trimmed
prefix (leading-`(` tolerant) is `select`; SurrealQL has no `with`
read form, so the surreal read set is `select` alone. Prepare
statements reject that prefix — writes own prepare, reads own
validate, per family.

**2. Record projection.** SurrealQL SELECT returns objects; the
executor projects each result record through `columns: [<field>,
...]` into the shared matcher-tuple grammar. Cell mapping: Null to
null, Bool to bool, Number (int/float) to number, Strand to string,
Uuid and Datetime to their string forms, RecordId to its `table:key`
string, Object and Array to structured values (the matcher verbs see
them whole). Any other value kind (Bytes, Geometry, ...) fails
closed naming the field and its SurrealQL type — the
`any_row_to_tuple` fail-closed precedent, extended to the second
family as its doc-comment promises (sql_validate.rs:339).

**3. Hermetic tier: embedded `mem://`, fresh by construction.** The
workspace `surrealdb` dependency gains `kv-mem`; the factory accepts
the `mem` scheme; `connect("mem://")` creates a new isolated
database per client. The factory builds one client per datasource
name and the boot's catalog owns its lifecycle, so every boot starts
empty with no cross-connection hazard — the sqlite `cache=shared`
problem cannot occur, and no new boot lint is needed (the spec
records this rationale). The factory's auth contract splits by
scheme: a fresh `mem://` instance has no root user, so the factory
SHALL skip `signin` for the `mem` scheme, and `namespace` /
`database` become optional extras defaulting to `test` / `test`
(remote schemes keep the mandatory extras and the unconditional
signin, `pool_factory.rs:89-134` today). A hermetic datasource
declares exactly `db_url = "mem://"` and
`provider = "surrealdb"`. Tier-2 coverage is therefore in-process,
Docker-free, seconds-budget (ADR-0069 §8). Tier-3 — a real instance
over `ws://`/`http://` — needs no new grammar: the datasource
`db_url` in Camel.toml steers it through the existing
env-interpolation surface, and durable-state cleanup is the
clean-first idiom (`REMOVE TABLE` / `DELETE` as the first prepare
statement), mirroring file-backed sqlite. `testcontainer` and
`user-provided` provisioning stay reserved (ADR-0069 §9). The remote
tier is verified by the existing component tests
(`crates/camel-test/tests/surrealdb_test.rs`); this change re-pins
only the embedded tier.

**4. Demand gate.** `surreal = ["dep:surrealdb",
"camel-bundles/surrealdb"]` in camel-integration-test — the bundles
forward is mandatory (lint-gate-forwarding Rule 1, same as `sql`).
Feature off: the action and target are named load errors. CLI gains
`integration-surreal`; an `integration-surreal` CI job proves the
feature stands alone, mirroring `integration-sql` (rc-7lrl2).

**Spec repair rides along.** The 2026-09-18 canonical spec rebuild
(02eec1be) dropped the "SQL prepare action" requirement that
2026-09-08-sql-prepare-action archived. The delta re-states it as
the cross-family "State prepare actions" requirement — sql coverage
restored, surreal added under the same contract, no observable
behavior change.

**Test matrix.**

| Case | Family | Tier |
|---|---|---|
| prepare seeds + validate reads back | surreal | mem |
| select-prefixed prepare is a load error | surreal | load |
| read-gate both feature configs | surreal | load |
| rows ordered/unordered, ignore wildcard, bounds | surreal | mem |
| column projection by field name; unknown field fails closed | surreal | mem |
| record-id cell maps to `table:key` string | surreal | mem |
| unknown value kind fails closed | surreal | mem |
| deadline poll: settle-in-window, no-early-settle, ceiling breach | surreal | mem |
| second boot over `mem://` starts empty | surreal | mem |
| teardown closes the surreal client | surreal | mem |
| mismatch/driver diagnostics redact db_url and cells | surreal | mem |
| feature off names the `surreal` gate (prepare at load, validate at run) | surreal | load |
| no catalog fails closed | surreal | stub |
| sql prepare-action scenarios restated | sql | load |

The remote `ws://` tier is steered by `db_url` convention only; its
behavior is pinned by the existing component tests
(`crates/camel-test/tests/surrealdb_test.rs`), not re-pinned here.

## Affected crates

- `camel-integration-test`: `surreal:` action + surreal validate
  target (parse, validate, execute), `surreal` feature, stub arm.
- `camel-component-surrealdb`: accept `mem` scheme in
  `SurrealDbPoolFactory`; teardown close path verified.
- `Cargo.toml` (workspace): `surrealdb` gains `kv-mem`.
- `camel-cli`: `integration-surreal` feature.
- `camel-bundles`: no change (gate exists).
- `.github/workflows`: `integration-surreal` job (apply phase).

## Architecture boundaries

Runtime: the executor resolves pools only through the boot's
`DatasourceCatalog` — no private clients. DSL: no change; scenario
documents parse in camel-integration-test. Components: the surreal
component changes only scheme acceptance. Core purity fences hold —
no tier concept enters core. The harness mutates no process-global
state; hermeticity rules (ADR-0069 §4) apply unchanged.

## Phases

### Phase 1: Grammar and load-time contract
- **Goal:** parse/validate `surreal:` action + surreal target; read
  gate; feature-off named errors; sql prepare-action spec restore.
- **Dependencies:** none.
- **Externally-visible types/interfaces:** document.rs action enum
  arm, RawSurrealAction/SurrealAction types, the `surreal` Cargo
  feature in camel-integration-test.
- **Deliverable:** load-error parity tests green in both feature
  configurations.
- **Exit-criteria:** doc-validation tests pin every malformed shape;
  `openspec validate` passes.

### Phase 2: Executor, projection, and hermetic mem tier
- **Goal:** execute prepare/read over the catalog; `mem://` support;
  record projection; poll semantics; redaction; teardown.
- **Dependencies:** Phase 1 grammar and feature.
- **Externally-visible types/interfaces:** the executor surface
  (execute_surreal_prepare, surreal_validate_action) behind the
  `surreal` feature.
- **Deliverable:** full test matrix above green.
- **Exit-criteria:** mem-tier e2e passes; second-boot freshness and
  teardown scenarios green.

### Phase 3: Gate forwarding, CLI, CI, docs
- **Goal:** `integration-surreal` CLI gate + CI job;
  scenario-surreal.md docs page; ADR-0069 §8 ladder update.
- **Dependencies:** Phase 2.
- **Externally-visible types/interfaces:** CLI feature, docs page.
- **Deliverable:** independence build green; docs published.
- **Exit-criteria:** CI job builds `--no-default-features --features
  integration-surreal,itest-e2e` and runs the suite.

## Alternatives considered

- **Raw `sql:` reuse (surreal queries through the sql family).**
  Rejected: SurrealQL is not SQL — the sqlx any-driver cannot route
  it, and the read-gate vocabulary differs. bd rc-25lup.5 names a
  second grammar family.
- **Container-only surreal (tier-3 first).** Rejected as default:
  the default suite must stay Docker-free (ADR-0069 §8); embedded
  `mem://` gives hermetic parity with sqlite memory.
- **`kv-rocksdb` embedded file backend now.** Deferred: durable
  state adds cleanup complexity without a scenario need; the remote
  tier covers durability today.
