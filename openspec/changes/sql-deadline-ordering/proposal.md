# Proposal: sql-deadline-ordering

## Why

A transacted block runs with two deadlines: the **block timeout** (the frozen
spec "Block timeout" timer the segment arms at block entry) and the **manager
admission timeout** (the pool acquire the session races inside the open). The
port's `open_session()` carries no block instant, so the production manager
arms a stand-in internal one and cannot decide the both-elapsed case itself.
Conformance round 2 (bd `rc-9awnc`, reviewed at `88e92e297`) recorded the
seam: a ready `PoolExhausted` that resolves with the real block deadline
already spent — or exactly tied with it — used to outreport the block timer.

The fix (absolute-instant re-adjudication at the segment's open race) landed
with `e1f0fe94b` at the segment tier, but only behind a mock manager. The
frozen requirement has **no production-tier conformance fixtures**: nothing
pins that the production manager (`SqlTransactionManager`) through the real
segment reports the earlier deadline as the winner — including the exact tie
and the both-ready case — or that a pending open never outlives the block
deadline.

## What Changes

- Add `crates/camel-test/tests/sql_deadline_ordering.rs` with a narrow
  `integration-sql` feature in `camel-test`: five fixtures exercising the
  **production manager through the real segment** under paused `tokio`
  virtual time — no mock manager:
  `earlier_admission_deadline_reports_pool_exhausted`,
  `earlier_block_deadline_reports_timeout`,
  `exact_tie_reports_block_timeout`,
  `both_ready_after_block_deadline_reports_block_timeout`,
  `pending_open_does_not_outlive_block_deadline`.
- Wire the suite: local lane rows in `AGENTS.md` (triggered by `camel-test`,
  `camel-core`, and `camel-component-sql`), the matching expected-argv lists
  in `scripts/gates-lanes-test.sh`, and a narrow CI step in
  `.github/workflows/ci.yml`.
- Home rationale: `camel-component-sql` must not dev-depend on `camel-core`
  (`camel-core` already dev-depends on `camel-component-sql`; ADR-0055
  publish cycle), so the fixtures ride `camel-test`, the designated
  publish-order leaf sink.
- No production code changes: the re-adjudication guard in
  `crates/camel-core/src/transacted_segment.rs` already implements the
  required behavior. The red runs are recorded by reverting that guard
  locally — both wrong-winner fixtures fail with `PoolExhausted` where
  `Timeout` is required (evidence in `design.md`).

## Acceptance criteria

- The five fixtures pass with the guard in place and the two wrong-winner
  fixtures (`exact_tie`, `both_ready`) fail with the guard reverted
  (red/green evidence recorded).
- The frozen "Block timeout" requirement text is unchanged: absolute
  deadline ordering is already its stated rule (`PoolExhausted` iff
  `acquire_abs < block_abs` strictly, tie → `Timeout`). No spec delta, no
  new exception.

## Risk budget

- Test-only change: no behavioral risk to the transacted family.
- Out of bounds: any change to the frozen `TransactionError` family, the
  port signatures, or the "Block timeout" requirement text.

Bd: rc-9awnc
