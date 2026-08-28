# Tasks: k8s-lease-epoch

## Task 1: monotonic renewal epoch + tests

- Spec: kubernetes-leadership — Requirement: Renewal-path epoch
  monotonicity; owns all four scenarios.

- **Files**:
  - `crates/platforms/camel-platform-kubernetes/src/leadership_fsm.rs` (modified)
  - `crates/platforms/camel-platform-kubernetes/src/platform_service.rs` (modified)
  - `crates/platforms/camel-platform-kubernetes/Cargo.toml` (modified)
  - `crates/platforms/camel-platform-kubernetes/CONTEXT.md` (modified)
- **Command** (from worktree root):
  `cargo test -p camel-platform-kubernetes && cargo fmt --check && cargo clippy -p camel-platform-kubernetes -- -D warnings`
  Expected (final): all green — existing tests (leadership_fsm 12: 10 #[test] + 2 #[tokio::test] +
  platform_service 14 + wiring_test) + 10 new (4 clamp + 4 apply + 2 note), fmt clean, clippy clean.
- **Steps**:
  0. `Cargo.toml`: add `[dev-dependencies] tracing-test = { workspace = true, features = ["no-env-filter"] }`
     (matches every existing workspace consumer; `no-env-filter` prevents
     `RUST_LOG` from suppressing warning capture; `#[traced_test]`
     captures logs for the note_renewal_epoch tests). `CONTEXT.md` Self-fencing section: append
     one sentence — while leading, the stored epoch is clamped monotonic;
     an observed regression (deleted/recreated Lease) is ignored and logged.
  1. STUB + TESTS + RED FIRST: introduce in `leadership_fsm.rs` the
     `pub(crate) enum EpochUpdate { Keep, Store(u64) }` and
     `pub(crate) fn clamp_epoch(current: u64, observed: Option<u64>) -> EpochUpdate`
     with TODAY's behavior-preserving semantics (`None` → Keep; `Some(t)`
     with `t != current` → Store(t) — regressions included); implement
     `apply_renewal_epoch` (load → clamp → conditional store → prior) and
     `note_renewal_epoch` (no warning yet) in `platform_service.rs`; wire
     the `ContinueLeading` arm (L272-278) to `note_renewal_epoch`. Write
     all 10 tests, run, confirm ASSERT-FAIL red (regression + warning tests
     fail against the stub; compile-fail is not acceptable red evidence).
  2. BEHAVIOR CHANGE: `clamp_epoch` regression arm goes `Keep` (`Some(t)`
     with `t > current` → Store(t); else Keep); `note_renewal_epoch`
     emits `warn!` on observed `Some(t)` < prior (lease_name, both values,
     the words `ignoring epoch regression`). Update the fsm module doc
     comment (only-if-Some defensive update is now also monotonic) and
     append the new policy to the ContinueLeading doc comment. All 10
     tests green.
- **Tests** (name / arrange / act / assert):
  - In `leadership_fsm.rs` `mod tests` — truth table:
    - `clamp_epoch_none_keeps` / current 7, observed None / `clamp_epoch` /
      Keep.
    - `clamp_epoch_equal_keeps` / current 7, observed Some(7) / clamp / Keep.
    - `clamp_epoch_increase_stores` / current 7, observed Some(9) / clamp /
      Store(9).
    - `clamp_epoch_regression_keeps` / current 7, observed Some(1) / clamp /
      Keep.
  - In `platform_service.rs` test module:
    - `apply_renewal_epoch_none_keeps` / `apply_renewal_epoch_equal_keeps` /
      `apply_renewal_epoch_increase_stores` /
      `apply_renewal_epoch_regression_keeps` / fresh `AtomicU64(7)` each;
      act `apply_renewal_epoch(&e, obs)` for None/Some(7)/Some(9)/Some(1) /
      assert returned prior is 7 each time and stored value is
      7/7/9/7 respectively (four named tests — consistent with design D3).
    - `note_renewal_epoch_regression_logs_warning` / `AtomicU64(7)`,
      captured-log subscriber; act `note_renewal_epoch(&e, Some(1), "test-lease")` /
      assert stored stays 7 AND one captured record contains
      `ignoring epoch regression`, `test-lease`, `7`, and `1`.
    - `note_renewal_epoch_equal_and_none_emit_no_epoch_log` / `AtomicU64(7)`,
      captured; act Some(7) then None / assert stored 7, zero captured
      records containing `ignoring epoch regression`, and zero captured
      records whose message contains both `test-lease` and `epoch`
      (exact predicate: `record.message().contains("test-lease") &&
      record.message().contains("epoch")`, expected count 0).
- **Acceptance criteria**:
  - All named tests pass; full crate suite green; fmt + clippy clean.
  - The `ContinueLeading` arm contains no inline epoch store.
  - Assert-fail red evidence from the behavior-preserving stub in the
    report; compile-fail does not satisfy this criterion.

- [x] 1
