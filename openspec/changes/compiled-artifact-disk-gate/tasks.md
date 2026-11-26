# Tasks: compiled-artifact-disk-gate

Single-phase change. All work is in one test file; Task 1.1 pins the gate
derivation, Task 1.2 rewrites root selection. Each task is TDD: the named
tests are written first and observed to fail, then the implementation until
they pass.

## camel-cli / compiled_artifact_test

### Task 1.1: Derive REQUIRED_ROOT_FREE from named footprint + margin constants

**Files:**
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. Write the test `required_free_tracks_fixture_footprint` (spec below) near
   the existing `REQUIRED_ROOT_FREE` constant. Run
   `cargo test -p camel-cli --no-default-features --features flavor-regular,exec --test compiled_artifact_test required_free_tracks_fixture_footprint --no-run`
   then run it: it must FAIL to compile (the three named constants do not
   exist yet) — that is the red state.
2. Replace the constant block at lines ~547-555: keep the
   `#[cfg_attr(not(unix), allow(dead_code))]` attribute and the
   `REQUIRED_ROOT_FREE` name, and define above it:
   `const FIXTURE_PERSISTENT_MIB: u64 = 4_526;` (doc: measured persistent
   footprint, 14 fixture compiles ≈ 4.42 GiB, bd rc-bpwa4 mission-538
   measurement),
   `const FIXTURE_TRANSIENT_MIB: u64 = 2_560;` (doc: measured transient
   peak, 9 concurrent whole-artifact copies × 296.7 MB on 12 threads ≈
   2.50 GiB, bd rc-bpwa4),
   `const FIXTURE_MARGIN_MIB: u64 = 1_106;` (doc: 1.16× the 6.92 GiB
   measured peak, absorbs binary growth between measurement and bump).
   Define `REQUIRED_ROOT_FREE: u64 = (FIXTURE_PERSISTENT_MIB +
   FIXTURE_TRANSIENT_MIB + FIXTURE_MARGIN_MIB) << 20;` — the value stays
   exactly `8 << 30` (8192 MiB); verify no other code depends on the old
   expression (`rg 'REQUIRED_ROOT_FREE'` in the file: the uses are
   `fixture_root` and the new test).
3. Rewrite the const's doc comment (lines ~547-553) to state the arithmetic:
   gate = measured persistent + measured transient + margin = 8192 MiB;
   cite bd rc-bpwa4 (measurement) and bd rc-fdkta (why the guard exists).
   Keep the existing "Bump this when the suite gains artifacts or the
   binary grows past what the headroom covers." sentence.
4. Update the module, `Fixture`, `cargo_target_dir`, and `fixture_root` docs.
   Remove stale footprint estimates and fallback-only descriptions. State
   the 6.92 GiB measured peak, the unchanged 8 GiB gate, and the Unix
   most-headroom rule with temp winning ties. Preserve the rc-fdkta citation,
   existing cleanup documentation, and unprobed non-Unix behavior.
5. Run the full file unit-surface:
   `cargo test -p camel-cli --no-default-features --features flavor-regular,exec --test compiled_artifact_test --no-run`
   (compile check only at this task; running the fixture suite is Task 1.2's
   acceptance plus the gate sweep).

**Tests:**
- `required_free_tracks_fixture_footprint`: setup — none (pure constant
  assertions) → action — evaluate the relation assertions → assert, all in
  numeric-operand `assert_eq!` form (bare `assert!` on constant booleans
  trips `clippy::assertions_on_constants` under `-D warnings`):
  (a) `assert_eq!(REQUIRED_ROOT_FREE, (FIXTURE_PERSISTENT_MIB +
  FIXTURE_TRANSIENT_MIB + FIXTURE_MARGIN_MIB) << 20);`
  (b) `assert_eq!((FIXTURE_PERSISTENT_MIB + FIXTURE_TRANSIENT_MIB) << 20,
  7086 << 20);` — the gate covers the real measured footprint (6.92 GiB),
  not an unrelated headroom number;
  (c) `assert_eq!((REQUIRED_ROOT_FREE >> 20) - FIXTURE_PERSISTENT_MIB -
  FIXTURE_TRANSIENT_MIB, FIXTURE_MARGIN_MIB);` — the margin is the residual
  over the footprint, not the whole gate;
  (d) `assert_eq!(REQUIRED_ROOT_FREE, 8 << 30);` — the validated value is
  pinned, so any future bump is a deliberate change to this assertion.
  Command:
  `cargo test -p camel-cli --no-default-features --features flavor-regular,exec --test compiled_artifact_test required_free_tracks_fixture_footprint`.
  Expected before implementation: compile failure (constants missing) = red;
  after: pass.

**Acceptance:**
- `required_free_tracks_fixture_footprint` passes.
- `REQUIRED_ROOT_FREE` value unchanged at `8 << 30` (assertion (d) proves it).
- `cargo fmt --check` clean on the touched file (workspace-wide fmt gate at
  task end), `cargo clippy -p camel-cli --no-default-features --features
  flavor-regular,exec --all-targets -- -D warnings` exits 0 (the leg that
  actually compiles this test file, ci.yml:435-436).

- [x] 1.1

### Task 1.2: Rewrite pick_root to most-headroom selection, keep fail-closed error

**Files:**
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. Write the four tests `pick_root_prefers_temp_dir_when_roomy`,
   `pick_root_falls_back_to_roomy_target_when_tmp_is_small`,
   `pick_root_prefers_most_headroom`,
   `pick_root_names_candidates_when_none_usable` (specs below) next to the
   existing `fixture_root_picking_prefers_roomy_candidates` test. Run them:
   `cargo test -p camel-cli --no-default-features --features flavor-regular,exec --test compiled_artifact_test pick_root_ -- --test-threads=1`.
   Expected red: `pick_root_prefers_most_headroom` fails (current code
   returns the first qualifying candidate — temp — instead of the roomier
   target); the other three may pass on the current implementation but pin
   the behaviors the rewrite must keep.
2. Rewrite `pick_root` (lines ~594-618) keeping its signature
   `fn pick_root(candidates: &[PathBuf], required: u64, free_of: impl Fn(&Path) -> Option<u64>) -> Result<PathBuf, String>`:
   probe every candidate once into a `Vec<(PathBuf, u64)>` of qualifying
   candidates (`free_of` gives `Some(free)` with `free >= required`; `None`
   or below-requirement is insufficient — unchanged); if any qualified,
   return the one with the maximum `free`; on equal `free`, the earliest
   candidate in `candidates` order wins (iterate once, keep the current best
   with a strict `>` comparison — this gives earliest-wins ties without a
   second pass); if none qualified, return the existing `Err(format!(
   "no temp root has the required free space: need {required} bytes,
   candidates [{listed}]"))` unchanged (byte-identical message).
3. Update the `pick_root` doc comment (lines ~594-598): it now selects the
   qualifying candidate with the most headroom, earliest wins ties,
   unprobeable counts as insufficient, and the Err shape is unchanged.
   Do NOT touch the pre-existing `fixture_root_picking_prefers_roomy_candidates`
   test or its doc comment (lines ~620-655) — byte-identical per mission
   order; its assertions hold under both semantics.
4. Verify the callers with `rg 'pick_root' crates/`. Run all six focused tests:
   `cargo test -p camel-cli --no-default-features --features flavor-regular,exec --test compiled_artifact_test -- pick_root_ fixture_root_picking_prefers_roomy_candidates required_free_tracks_fixture_footprint --test-threads=1`.
   Run `cargo clippy -p camel-cli --no-default-features --features flavor-regular,exec --all-targets -- -D warnings`.

**Tests:**
- `pick_root_prefers_temp_dir_when_roomy`: setup — closure mapping
  `temp`→`Some(10 GiB)`, `target`→`Some(9 GiB)` (both qualify); required
  `8 GiB` → action — `pick_root(&[temp, target], REQUIRED_ROOT_FREE,
  free_of)` → assert `Ok(temp)` — temp qualifies and holds the most
  headroom (target also qualifies with less). The unchanged CI path.
  Second assertion in the same test with equal headroom
  (`temp`→`Some(10 GiB)`, `target`→`Some(10 GiB)`): assert `Ok(temp)` —
  earliest-candidate tie-break, the shared-filesystem CI case. Command:
  `cargo test -p camel-cli --no-default-features --features flavor-regular,exec --test compiled_artifact_test pick_root_prefers_temp_dir_when_roomy`.
  Expected: pass before and after the rewrite (pins behavior).
- `pick_root_falls_back_to_roomy_target_when_tmp_is_small`: setup —
  `temp`→`Some(2 GiB)` (below requirement), `target`→`Some(20 GiB)`; required
  `8 GiB` → action — `pick_root(&[temp, target], REQUIRED_ROOT_FREE,
  free_of)` → assert `Ok(target)` (the only qualifying candidate is chosen).
  Expected: pass before and after (pins the rc-fdkta fallback).
- `pick_root_prefers_most_headroom`: setup — `temp`→`Some(9 GiB)`,
  `target`→`Some(12 GiB)`; required `8 GiB` → action — `pick_root(&[temp,
  target], REQUIRED_ROOT_FREE, free_of)` → assert `Ok(target)` — target
  wins even though temp
  qualifies and sits first in the list. Expected: FAIL before the rewrite
  (current code returns temp), PASS after — the red/green proof of this
  change.
- `pick_root_names_candidates_when_none_usable`: setup — `temp`→`Some(2 GiB)`,
  `target`→`Some(4 GiB)`; required `8 GiB` → action — `pick_root(&[temp,
  target], REQUIRED_ROOT_FREE, free_of)` → assert the result is `Err(msg)`
  with `msg ==
  "no temp root has the required free space: need 8589934592 bytes,
  candidates [temp, target]"` (exact bytes — the shape bd rc-bpwa4 quotes,
  with the real constant's value) — nothing is skipped: no `Ok` variant, and
  the message names every candidate and the requirement. Expected: pass
  before and after (pins the fail-closed contract).
  (In all four tests the paths are synthetic `PathBuf`s and `free_of` is a
  closure — no filesystem probes; mirror the existing test's arm-matching
  pattern (`match path { p if p == big => Some(10), p if p == small =>
  Some(2), _ => None }`).)

**Acceptance:**
- The four `pick_root_*` tests plus `required_free_tracks_fixture_footprint`
  and the pre-existing `fixture_root_picking_prefers_roomy_candidates` all
  pass.
- After compiling, if either Unix candidate probes at least 8 GiB free,
  `cargo test -p camel-cli --no-default-features --features flavor-regular,exec --test compiled_artifact_test`
  exits 0. If neither qualifies, run `--no-run` and all six focused tests;
  record `integration-verification-deferred-to-CI` in the mission report.
- `cargo fmt --check --all` exits 0; `cargo clippy -p camel-cli
  --no-default-features --features flavor-regular,exec --all-targets -- -D
  warnings` exits 0; no `unwrap()` introduced (`cargo xtask lint-unwrap`
  exits 0).

- [x] 1.2
