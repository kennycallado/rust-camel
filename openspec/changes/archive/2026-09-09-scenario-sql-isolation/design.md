# Design: scenario-sql-isolation

## Approach

The freshness guarantee is delivered at the teardown seam, not at boot. The
catalog is already per-boot (`RuntimeDatasourceCatalog::new` in
`camel-bundles::boot`); what leaks is the pool's lifetime. The fix adds an
explicit close path through the same provider seam that creates pools:

1. `camel-api` — `PoolFactory` gains `close<'a>(&'a self, handle: &'a
   DatasourceHandle) -> CloseFuture<'a>` with a default `Ok(())` (providers
   without an explicit close stay untouched). `DatasourceCatalog` gains
   `close_all<'a>(&'a self) -> CloseAllFuture<'a>` with a default `Ok(())`.
   Object-safe boxed futures, mirroring `CreatePoolFuture`/`GetPoolFuture`.
2. `camel-core` — `RuntimeDatasourceCatalog::close_all` walks the initialized
   `pools` DashMap, resolves each handle's factory by provider kind, and
   awaits `factory.close(handle)`. Uninitialized lazy cells are skipped (no
   pool was ever opened). Close errors are logged (`outside-contract`) and
   collected, never panic.
3. `camel-sql` — `SqlPoolFactory::close` downcasts to `sqlx::AnyPool` and
   awaits `pool.close()` (the `check` method already downcasts the same way).
4. `camel-bundles` — `BootHandle::shutdown_with_deadline` gains step 4:
   deadline-wrapped `datasource_catalog.close_all()` after the CXF pool
   shutdown. A close timeout warns and does not fail the shutdown; a close
   error fails it (the JMS/CXF precedent); `camel run` inherits the same
   teardown for free because it shares this handle.

Adversarial tests prove the mechanism, not the hope. The load-bearing
shape is the named shared-memory URI
(`sqlite:file:<name>?mode=memory&cache=shared`, provider-pinned to the
sqlx factory): connections share one named database across boots, and
with the close step disabled the `boot_freshness` test measurably fails
(boot B counts boot A's rows — verified red during this change). The
plain `sqlite::memory:?cache=shared` alias, shutdown pool-closure
(`pool.is_closed()`), and the opposite file-backed contract (rows
persist; the author's clean-first prepare owns isolation, mirroring
ADR-0069 §9) complete the suite.

Adversarial finding, corrected against source and experiment (sqlx 0.8.6):
`SqliteConnectOptions::from_str(":memory:")` assigns every parse a globally
unique name (`file:sqlx-in-memory-{seqno}`, options/parse.rs:14-24), so a
later boot's pools open a DIFFERENT database object even for the identical
URL — sequential isolation already holds today, but as an accident of sqlx
internals, not a contract. Two facts sharpen the guarantee: (a) with the Any
driver each pooled connection still gets a PRIVATE in-memory database (the
landed family convention pins `max_connections = 1` in memory fixtures so
CREATE/INSERT/SELECT stay on one connection — pinned by the failing-seed
discovery during this change); (b) nothing at teardown drained pools, so the
freshness guarantee rested entirely on (a)+(seqno). The `close_all` seam
replaces the accident with an explicit boot-scoped lifecycle: pools closed at
teardown, database dies with its boot regardless of sqlx naming internals,
and the guarantee survives the day a fixture uses a named shared memory URI
(`file:memdb_x?mode=memory&cache=shared`), where a lingering connection
WOULD leak state across boots.

## Affected crates

- `camel-api`: additive `close`/`close_all` trait surface (default no-op).
- `camel-core`: `RuntimeDatasourceCatalog::close_all` implementation.
- `camel-sql`: `SqlPoolFactory::close` (AnyPool close).
- `camel-bundles`: shutdown step 4 (catalog close, deadline-wrapped).
- `camel-integration-test`: adversarial boot-freshness tests; CONTEXT.md canon
  line.
- `docs/src/testing/index.md`: isolation-and-teardown subsection.

## Architecture boundaries

Per CONTEXT-MAP zones: the close seam lives in `camel-api` (contract) +
providers (camel-sql; a Runtime-side adapter, not the DSL). `camel-bundles`
is the composition root that already owns the catalog Arc and pool shutdown
ordering — the new step reuses that authority. No DSL, Services, Languages,
or Functions surface changes. The scenario tier (`camel-integration-test`)
only gains tests; its runner code is untouched.

## Known limitation (filed, not implemented)

Parallel document execution does not exist; when it lands, two concurrently
booted documents sharing the same `sqlite::memory:?cache=shared` alias could
collide on the unnamed shared in-memory database. Mitigation (deferred to the
parallel-mode issue): per-boot unique memory URI
`file:memdb_{scenario}?mode=memory&cache=shared`. Sequential mode needs no
unique URI once `close_all` guarantees the database dies with its boot.
