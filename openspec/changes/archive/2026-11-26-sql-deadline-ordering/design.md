# Design: sql-deadline-ordering

## Approach

**Ordering rule.** A transacted block is bounded by two absolute deadlines:
`block_abs = block_entry + block_timeout` (the frozen "Block timeout"
requirement: default 30 s, per-block override) and
`acquire_abs = open_start + acquire_deadline()` (the manager admission bound,
sqlx's 30 s pool default; `DatasourceConfig` exposes no acquire knob). The
deadline that expires first wins the reported primary; on the exact tie the
block timer wins. Concretely, the admission outcome is `PoolExhausted` iff
`acquire_abs < block_abs` strictly, else `Timeout` measured from block entry
— the frozen requirement's own words ("Deadlines are compared as absolute
instants, not by outer-timeout polling order").

**Where the two deadlines are compared.** Three sites, each already on
`main` at `d84bfd804`:

1. `crates/components/camel-sql/src/session.rs` (`SqlTxSession::open`,
   biased race `min(block_abs, acquire_abs)`): compares the instants the
   port hands it; with the production manager the block instant is a
   stand-in (`NO_BLOCK_DEADLINE`, 24 h — `manager.rs:138-139`), so this
   race can never decide the both-elapsed case itself.
2. `crates/components/camel-sql/src/manager.rs` (`open_session()`): arms
   only the acquire deadline and reports it through the port's
   `acquire_deadline()`.
3. `crates/camel-core/src/transacted_segment.rs` (the open race, rc-9awnc
   re-adjudication): when a `PoolExhausted` resolves with the real block
   deadline spent (`now >= block_abs`), the segment re-derives
   `acquire_abs = open_start + acquire_deadline()` and decides —
   `block_abs <= acquire_abs` (tie included) → `Timeout` from entry;
   acquire strictly first → `PoolExhausted` passes through. A pending open
   is bounded by the segment timer (expiry drops the open future; the R0
   discard guard detaches any checked-out connection).

**This change adds no production code.** The guard is the fix and it landed
with `e1f0fe94b`; what is missing is the production-tier conformance the
frozen requirement deserves: the five mission-named fixtures run the
PRODUCTION manager (`SqlTransactionManager` over a held single-connection
in-memory SQLite pool) through the real `TransactedSegment` under paused
tokio virtual time — no mock manager, closing the gap the mock-tier
fixtures (`camel-core` `FakeManager` scripts) leave. They live in
`crates/camel-test/tests/sql_deadline_ordering.rs` behind a narrow
`integration-sql` feature: `camel-component-sql` cannot dev-depend on
`camel-core` (ADR-0055 publish cycle — `camel-core` already dev-depends on
`camel-component-sql`), so the fixtures ride the leaf-sink crate where both
sides of the port meet. Branch: the mission's gate list keeps
`cargo test -p camel-component-sql` for the component's own surface; the
new suite's lane is
`cargo test -p camel-test --features integration-sql --test sql_deadline_ordering`.

**Probe evidence (recorded 2026-11-26, worktree at `d84bfd804`).**

- Guard in place: 5/5 fixtures pass.
- Guard locally reverted to the `88e92e297` shape (unconditional
  `Err(e) => PoolExhausted`): `exact_tie_reports_block_timeout` FAILS with
  `PoolExhausted { waited_ms: 31 }` and
  `both_ready_after_block_deadline_reports_block_timeout` FAILS with
  `PoolExhausted { waited_ms: 50 }`, where `Timeout` is required — the
  wrong-winner cases bite; `earlier_admission`, `earlier_block`, and
  `pending_open` stay green (they are not wrong-winner cases).
- Guard mutated from `block_abs <= acquire_abs` to `==`:
  `both_ready_after_block_deadline_reports_block_timeout` FAILS with
  `PoolExhausted` — the strict first-wins half (block strictly earlier than
  acquire) is pinned by that fixture.

**Determinism constraints the fixtures encode.** Setup (pool connect, hold)
runs on the real clock before `tokio::time::pause()`. The block task is
spawned and yielded once, so the segment records its block entry at the
paused instant BEFORE any advance; the caller then drives the deadline wake
with one explicit `tokio::time::advance` past the deadlines under test. In a
single advance both timers expire together, the biased open arm resolves
first, and the block timer is already spent when it does — the both-ready
shape. Elapsed assertions are exact to the advance amount (±1 ms).
`earlier_block` and `pending_open` advance past the block deadline while the
acquire deadline is still pending, so the segment timer resolves them.
Same-pool re-acquire after a dropped open is not assertable under the
paused clock (sqlx returns a `PoolConnection` on drop through real-time
churn), so `pending_open` probes usability through the connection the test
holds; a pending open never checked a connection out, so the R0
discard-detach path is not exercised by that fixture.

**Spec delta.** None. The frozen "Block timeout" requirement already states
absolute deadline ordering; the fix implements it and this change only pins
it with production fixtures. No new frozen-spec exception. `skip_specs:
true` is set in `.openspec.yaml`.

## Affected crates

- `camel-test`: new integration test file `tests/sql_deadline_ordering.rs`
  + narrow `integration-sql` feature (`dep:camel-component-sql`,
  `dep:sqlx`).
- `camel-component-sql`, `camel-core`: untouched (guard verified
  byte-identical to the branch merge-base — no production edits).
- `AGENTS.md`: three local lane rows (triggers: `camel-test`, `camel-core`,
  `camel-component-sql`; duplicate-owner precedent).
- `scripts/gates-lanes-test.sh`: the three expected-argv lists (49a/49b/49f)
  lead with the sql command; 49f's failure index moves to the third call.
- `.github/workflows/ci.yml`: narrow Docker-free step after the cxf gate.

## Architecture boundaries

Runtime (camel-core) stays generic over the `camel-api` transaction port —
the hexagonal boundary (`hexagonal_architecture_boundaries_test`) forbids
camel-core referencing camel-sql, which is exactly why the fixtures live
above both, in `camel-test`. ADR-0055 forbids the component → core
dev-dependency that would close the publish cycle (`camel-core` already
dev-depends on `camel-component-sql`); `camel-test` is the designated
publish-order leaf sink, so the fixtures ride it behind the narrow
`integration-sql` feature instead of pulling the full container-bearing
`integration-tests` set.
