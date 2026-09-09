# Proposal: scenario-sql-isolation

Bd: rc-25lup.4 (epic rc-25lup child 4/6, effort S — adversarial hardening).

## Why

The scenario tier runs every admitted document sequentially in one process
(`camel test`, `crates/camel-cli/src/commands/test.rs`). The rc-25lup family
shipped `sql:` prepare actions and `validate` sql targets reading through the
booted cascade's `DatasourceCatalog`. The standing claim "hermetic sqlite is
per-boot = safe" was never adversarially verified. Pre-flight investigation
(e_opus, PROCEED-WITH-CHANGES) broke the claim by reading the teardown path:

- Each boot builds a fresh `RuntimeDatasourceCatalog` (`crates/camel-bundles`)
  with lazy per-name pools — the catalog is per-boot.
- But `BootHandle::shutdown_with_deadline` tears down only the JMS and CXF
  bridge pools. It never closes the datasource pools, and the sqlx pool keeps
  `min_connections = 1` alive past the boot's scope. A shared-cache sqlite
  in-memory database dies only when its LAST connection closes, so a later
  document booting the same `[datasources]` alias in the same process can
  observe the earlier document's seeded rows — a silent green-lie of the same
  class the `sql-memory-not-shared` lint exists to prevent.

## What Changes

- **Production fix (in scope):** `DatasourceCatalog` gains `close_all()`
  (default no-op), `PoolFactory` gains `close()` (default no-op), the sqlx
  factory closes its `AnyPool`, and `BootHandle::shutdown_with_deadline`
  closes the datasource catalog's opened pools after `ctx.stop()` —
  deadline-wrapped; a close timeout warns, a close error fails the
  shutdown, matching the existing bridge-pool teardown semantics.
- **Adversarial boot-freshness tests** (`camel-integration-test`): a second
  boot over the same memory-sqlite alias must not see the first boot's rows;
  shutdown must leave pools closed; file-backed rows persisting across boots
  is pinned as contract (author's clean-first responsibility).
- **Spec delta** (`specs/integration-tier/spec.md`): one ADDED requirement
  "Scenario datasource teardown" with the isolation scenarios, re-diffed
  against the CURRENT main spec (siblings 2026-09-08-sql-prepare-action and
  2026-09-09-sql-validate-target already merged).
- **Docs** (`docs/src/testing/index.md`): isolation-and-teardown subsection —
  per-boot freshness, the prepare-action clean-first idiom (DELETE/TRUNCATE as
  the first statement) for shared/file-backed datasources, the user-provided
  isolation responsibility (mirrors ADR-0069 §9), and the parallel-mode
  known-limitation with the per-boot unique memory URI recipe
  (`file:memdb_{scenario}?mode=memory&cache=shared`).
- **Known-limitation filed as bd issue** for future parallel mode.

Excluded: parallel document execution; unique-URI generation in the harness
(files now, lands with parallel mode); surrealdb/redis-kv adapters (epic
children 5-6); any change to the `sql-memory-not-shared` lint.

## Acceptance criteria

- A second sequential boot over the same `sqlite::memory:?cache=shared` alias
  reads zero rows seeded by the first boot (test green, no sleeps).
- After `shutdown`, the boot's datasource pools report closed.
- File-backed sqlite persistence across boots is pinned by test and
  documented as the author's clean-first responsibility.
- Docs carry the clean-first idiom, the §9 mirror, and the parallel
  known-limitation; a bd issue tracks the unique-URI work.
- Gates: fmt, clippy (`-D warnings`) on touched crates, their test suites,
  and the scenario-tier tests green.

## Risk budget

Acceptable: a default-no-op trait method on `DatasourceCatalog`/`PoolFactory`
(additive, no implementor breaks); shutdown ordering extension after the
existing steps. Out of bounds: any behavior change to live datasource serving
during a boot; any new load-time rejection; parallel mode.
