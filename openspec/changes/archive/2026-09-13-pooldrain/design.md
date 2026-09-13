# Design: pooldrain

## Approach

Add two focused tests to the existing `#[cfg(test)] mod tests` in `crates/components/camel-sql/src/pool_factory.rs`.

The drain test creates a uniquely named SQLite shared-memory pool through `SqlPoolFactory::create`, acquires and releases enough connections to exercise the pool, calls the production `close` seam through a `DatasourceHandle`, and asserts both successful completion and `pool.size() == 0` plus `pool.is_closed()`. It tests the observable contract from the bootfreshness change: awaited close must not return while a pooled connection remains. It does not pause Tokio time, count poll iterations, or test the ten-second error bound; those choices would couple the test to sqlx 0.8.6 internals or mix `std::time::Instant` with virtual time.

The classifier test uses one table-driven test over the private pure function. Rows cover `sqlite::memory:`, `sqlite://:memory:`, named `mode=memory` shared-cache URLs, uppercase scheme/query spelling, ordinary SQLite files, a `memory.db` filename near miss, and a non-SQLite URL containing `mode=memory`. The existing classifier intentionally does not recognize `sqlite:file::memory:?cache=shared`; that separate lint/classifier gap is bd rc-acrek and remains excluded.

## Affected crates

- `camel-component-sql`: add unit coverage only in `pool_factory.rs`.

## Architecture boundaries

The tests invoke the existing SQL component adapter seam and private classifier without changing Runtime, DSL, API contracts, or control-plane behavior. The pool test uses in-process SQLite and no live external service, consistent with the unit tier and the datasource close boundary documented by bootfreshness and ADR-0069.

## Alternatives considered

- Testing the ten-second stalled-connection path was rejected because it requires real wall-clock waiting and cannot be made deterministic while the implementation uses `std::time::Instant`.
- Reusing the bootfreshness integration test was rejected because it cannot directly assert pool size and does not provide a compact unit truth table.
- Changing classifier behavior for `file::memory:` or substring-looking filenames was rejected as separate production work (bd rc-acrek), not test-only scope.
