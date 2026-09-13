# Proposal: pooldrain

## Why

`SqlPoolFactory::close` now protects named SQLite memory databases by waiting for the sqlx pool to reach zero connections, but this contract lacks a deterministic unit test. The private `is_sqlite_memory_url` classifier also needs a truth-table test for supported forms, case normalization, and near misses. These gaps can let future sqlx or URL changes silently reintroduce boot-to-boot memory leakage (bd rc-61g4m).

## What Changes

- Add a unit test proving factory close completes with an empty, closed pool after connections are released.
- Add a table-driven unit test for SQLite memory URL classification, including uppercase schemes and contrived filenames.
- Keep production behavior, integration fixtures, lint behavior, and the separate `file::memory:` lint issue (bd rc-acrek) out of scope.

## Acceptance criteria

- A no-infrastructure unit test asserts `SqlPoolFactory::close` returns successfully and the pool size is zero.
- A classifier table covers bare memory URLs, named shared-cache URLs, uppercase forms, near-miss filenames, and wrong schemes.
- Tests pass with the affected crate's normal Rust quality checks.

## Risk budget

Tests only. No production behavior or public API changes are allowed. The drain test must assert final state, not timing or sqlx internal loop counts, and must not use a wall-clock stall test.
