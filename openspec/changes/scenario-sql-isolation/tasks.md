# Tasks: scenario-sql-isolation

## Close seam

### Task 1.1: `PoolFactory::close` + `DatasourceCatalog::close_all` (camel-api)

**Files:**
- `crates/camel-api/src/datasource.rs` (modified)

**Steps:**
1. Add `pub type CloseFuture<'a> = Pin<Box<dyn Future<Output =
   Result<(), CamelError>> + Send + 'a>>;` beside `CreatePoolFuture`.
2. Add to `PoolFactory` a default `close` method: `fn close<'a>(&'a self,
   _handle: &'a DatasourceHandle) -> CloseFuture<'a> { Box::pin(async {
   Ok(()) }) }` — providers without an explicit close keep compiling.
3. Add `pub type CloseAllFuture<'a> = Pin<Box<dyn Future<Output =
   Result<(), CamelError>> + Send + 'a>>;` and to `DatasourceCatalog` a
   default `close_all` method returning `Ok(())` (object-safe, boxed).

**Tests:**
- Existing `camel-api` unit tests still compile/pass (trait is additive).

**Acceptance:**
- `cargo clippy -p camel-api --all-targets -- -D warnings` exits 0.

- [x] 1.1

### Task 1.2: `RuntimeDatasourceCatalog::close_all` (camel-core)

**Files:**
- `crates/camel-core/src/datasource.rs` (modified)

**Steps:**
1. Override the `DatasourceCatalog::close_all` default: iterate the `pools`
   DashMap; for each initialized `OnceCell` handle, look the factory up by
   the handle's provider kind (`factories()` read lock) and await
   `factory.close(handle)`; skip uninitialized cells; aggregate failures —
   log each (`tracing::warn!`, log-policy `outside-contract`) and return the
   first error only after all closes were attempted.
2. Keep the method non-async on the trait object shape (returns the boxed
   future), matching `get_pool`'s shape.

**Tests:**
- `crates/camel-core/tests/datasource_catalog_test.rs`: new test — a
  factory whose `close` records invocation; `get_pool` (to initialize the
  cell), then `close_all()`; assert the record exists and a second
  `close_all()` is also Ok (idempotent).

**Acceptance:**
- `cargo test -p camel-core --test datasource_catalog_test` passes;
  `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.

- [x] 1.2

### Task 1.3: `SqlPoolFactory::close` (camel-sql)

**Files:**
- `crates/components/camel-sql/src/pool_factory.rs` (modified)

**Steps:**
1. Override `close`: `handle.downcast::<AnyPool>()`, then `pool.close()
   .await` mapped to `CamelError::ProcessorError` with `redact_db_url`
   context on failure (never echo the URL itself — ADR-0051 discipline;
   reuse the `create` error shape).

**Tests:**
- Unit test in the file's test module: create a pool over
  `sqlite::memory:?cache=shared`, wrap in a `DatasourceHandle`, `close` it,
   assert `pool.is_closed()`.

**Acceptance:**
- `cargo test -p camel-sql` passes;
  `cargo clippy -p camel-sql --all-targets -- -D warnings` exits 0.

- [ ] 1.3

### Task 1.4: BootHandle teardown step 4 (camel-bundles)

**Files:**
- `crates/camel-bundles/src/lib.rs` (modified)

**Steps:**
1. In `shutdown_with_deadline`, after the CXF pool shutdown, add the
   deadline-wrapped `self.datasource_catalog.close_all()` with the same
   match shape as the JMS/CXF arms: `Ok(Ok(()))` no-op, error logs
   (`system-broken`) and fills `failure` if empty, timeout warns naming the
   step ("datasource pool close timed out after {}s").
2. Update the doc comment's numbered teardown ordering (step 4: datasource
   catalog pools).

**Tests:**
- Existing `boot_handle_exposes_datasource_catalog` test still passes.

**Acceptance:**
- `cargo test -p camel-bundles` passes;
  `cargo clippy -p camel-bundles --all-targets -- -D warnings` exits 0.

- [x] 1.4

## Adversarial boot-freshness tests

### Task 2.1: sequential-boot isolation tests (camel-integration-test)

**Files:**
- `crates/camel-integration-test/src/boot_scenario_test.rs` (modified —
  new `mod boot_freshness` gated `#[cfg(feature = "sql")]`)

**Steps:**
1. `second_boot_over_same_memory_alias_starts_empty`: boot doc A over a
   Camel.toml with `[datasources.appdb] db_url =
   "sqlite::memory:?cache=shared"`; run a `sql:` prepare (CREATE TABLE +
   INSERT) through the booted catalog; run `boot.shutdown(&mut ctx)`; drop
   the run. Boot doc B over the SAME config; CREATE the same table shape
   via `CREATE TABLE IF NOT EXISTS` (or CREATE+catch), then SELECT COUNT(*)
   — assert 0 rows of A's seed.
2. `shutdown_closes_the_datasource_pools`: after a boot that ran a `sql:`
   action, `boot.shutdown(&mut ctx)`, then downcast the catalog's pool for
   the alias and assert `pool.is_closed()`.
3. `file_backed_state_persists_across_boots`: tempdir file URL
   `sqlite:file:<tmp>/shared.db` (or `sqlite:<path>` form the factory
   accepts); doc A seeds rows, shuts down; doc B counts the rows — assert
   A's rows ARE visible (pins the contract that justifies clean-first).

**Tests:**
- `cargo test -p camel-integration-test --features sql boot_freshness`.

**Acceptance:**
- All three green, no sleeps (no timing dependence — the close seam makes
  freshness deterministic).
- `cargo clippy -p camel-integration-test --all-targets --features sql --
  -D warnings` exits 0.

- [x] 2.1

## Docs + filing

### Task 3.1: isolation-and-teardown docs section

**Files:**
- `docs/src/testing/index.md` (modified — new `#### Isolation and teardown`
  subsection after `#### Datasource steering`)
- `crates/camel-integration-test/CONTEXT.md` (modified — canon lines)

**Steps:**
1. Subsection covers: per-boot catalog + pool close at teardown; memory
   sqlite dies with its boot; file-backed and user-provided datasources are
   the author's isolation responsibility (ADR-0069 §9 mirror); the
   clean-first idiom (DELETE/TRUNCATE or table-recreate as the FIRST
   `sql:` prepare statement) with a short example; the parallel-mode
   known-limitation + the per-boot unique memory URI recipe
   (`file:memdb_{scenario}?mode=memory&cache=shared`).
2. CONTEXT.md: one compact paragraph — the teardown close seam, the
   freshness guarantee, pointers to the spec requirement and the bd issue.

**Acceptance:**
- `cargo xtask lint-context-citations` passes for the touched CONTEXT.md;
  docs render consistent with the steering sibling's section style.

- [x] 3.1

### Task 3.2: file the parallel-mode known-limitation issue

**Steps:**
1. `bd create "Parallel scenario mode needs per-boot unique sqlite memory
   URIs" --description="..." -p 3 -t task --deps
   discovered-from:rc-25lup.4 --json` describing the unnamed shared
   `:memory:` alias collision and the `file:memdb_{scenario}?mode=memory&
   cache=shared` remedy (only needed when parallel document execution
   lands).

**Acceptance:**
- Issue id recorded in the parked report and CONTEXT.md references it.

- [x] 3.2
