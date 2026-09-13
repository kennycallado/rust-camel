# Tasks: pooldrain

## camel-component-sql / pool factory tests

### Task 1.1: Add deterministic close drain-to-zero unit test

**Files:**
- `crates/components/camel-sql/src/pool_factory.rs` (modified)

**Steps:**
1. Extend the existing SQL pool factory test module with a uniquely named SQLite shared-memory datasource configuration using `max_connections: Some(3)` and no minimum connection maintainer.
2. Create the pool through `SqlPoolFactory::create`, downcast it to `AnyPool`, hold the first acquired connection while acquiring the second, execute `SELECT 1` through each, release both connections, and wrap the pool in a `DatasourceHandle`.
3. Call `SqlPoolFactory::close` with the handle and assert the future succeeds, `pool.is_closed()` is true, and `pool.size()` equals zero; do not pause Tokio time, sleep in the test, or assert implementation loop counts.

**Tests:**
- `sql_pool_factory_close_drains_pool_to_zero`: arrange a unique `sqlite:file:<name>?mode=memory&cache=shared` pool with two released acquired connections; act by awaiting `SqlPoolFactory::close`; assert `Result::is_ok()`, `pool.is_closed()`, and `pool.size() == 0`; command `cargo test -p camel-component-sql --lib pool_factory::tests::sql_pool_factory_close_drains_pool_to_zero -- --exact`; expected: passes after the test is added.

**Acceptance:**
- The named test executes without Docker, Redis, Kafka, or other live infrastructure.
- The test proves the observable drain-to-zero contract using final pool state, not timing or sqlx internal iteration details.
- `cargo test -p camel-component-sql --lib pool_factory::tests::sql_pool_factory_close_drains_pool_to_zero -- --exact` exits 0.

- [x] 1.1

### Task 1.2: Add SQLite memory URL classifier truth table

**Files:**
- `crates/components/camel-sql/src/pool_factory.rs` (modified)

**Steps:**
1. Add one table-driven unit test in the existing SQL pool factory test module that calls private `is_sqlite_memory_url` for each row.
2. Include true rows for `sqlite::memory:`, `sqlite://:memory:`, named `sqlite:file:memdb1?mode=memory&cache=shared`, uppercase `SQLITE::MEMORY:`, uppercase `Sqlite:file:MEMDB2?MODE=MEMORY`, and contrived `sqlite:file:demo_mode=memory.db`.
3. Include false rows for `sqlite:data.db`, `sqlite:file:memory.db`, `postgres://host/db?mode=memory`, and `sqlite:file::memory:?cache=shared`; assert each row's expected result and identify the last row as the separate rc-acrek scope boundary.

**Tests:**
- `sqlite_memory_url_classifier_table`: arrange the ten listed URL/expected-value rows; act by evaluating the classifier for each row; assert every actual boolean equals its row's expected boolean and reports the URL on mismatch; command `cargo test -p camel-component-sql --lib pool_factory::tests::sqlite_memory_url_classifier_table -- --exact`; expected: passes after the test is added.

**Acceptance:**
- The table explicitly covers uppercase scheme and query spellings, named shared-cache memory, ordinary paths, contrived filenames, and wrong schemes.
- The test preserves current `file::memory:` behavior without implementing the separate rc-acrek lint/classifier change.
- `cargo test -p camel-component-sql --lib pool_factory::tests::sqlite_memory_url_classifier_table -- --exact` exits 0.

- [x] 1.2
