# Proposal: compiled-artifact-disk-gate

## Why

bd rc-bpwa4: CI run 41801330337 aborted the `compiled_artifact_test` suite
with 79 errors at `crates/camel-cli/tests/compiled_artifact_test.rs:678` —
`no temp root has the required free space: need 8589934592 bytes, candidates
[/tmp, target]`. The runner had reclaimed to 24 GiB free before the workspace
compile; the compile consumed most of it, and at suite start both candidates
sat below the 8 GiB gate on the same root filesystem, so the whole suite
aborted instead of running.

Two defects, both in the test's fixture-root picking:

1. **The gate is an unlabeled magic number.** `REQUIRED_ROOT_FREE = 8 << 30`
   carries no relation to what the suite actually writes. The mission-538
   measurement (bd rc-bpwa4 notes) puts the real peak at 6.92 GiB: 4.42 GiB
   persistent (14 fixture compiles) + 2.50 GiB transient (9 concurrent
   whole-artifact copies × 296.7 MB on 12 threads), so 8 GiB is a 1.16×
   margin over a measured footprint — but the code does not say so, and the
   stale `~3.7 GiB` estimate in the `fixture_root` doc contradicts the const
   doc's `~4.3 GiB`.
2. **Root selection is positional, not need-based.** `pick_root` returns the
   first candidate that clears the bar. When `/tmp` and the target directory
   sit on different filesystems, a barely-qualifying first candidate beats a
   roomier fallback, adding mid-suite ENOSPC risk that the gate exists to
   prevent.

The guard itself must stay: rc-fdkta is real (the fixture exhausted a 4 GiB
`/tmp`). The fix keeps the check fail-closed, changes how the root is chosen
and how the requirement is derived.

## What Changes

- `pick_root` selects, among candidates that clear the requirement, the one
  with the most free-space headroom; the earliest candidate wins ties (the
  OS temp dir stays first in the candidate list, so unchanged CI behavior —
  shared filesystem, equal headroom — keeps picking `/tmp`).
- The suite still aborts only when **no** candidate is usable, with the
  same named error (requirement bytes + full candidate list). No silent
  skip: a skip hides the ENOSPC class.
- `REQUIRED_ROOT_FREE` is derived from named constants — measured persistent
  footprint, measured transient copy peak, explicit margin — keeping the
  8 GiB value the mission-538 ruling validated. Doc comments state the
  arithmetic.
- Not included: any change to `REQUIRED_ROOT_FREE`'s value, workflow/reclaim
  changes, source (non-test) changes.

## Acceptance criteria

- The five new named tests pass (see tasks.md); the pre-existing
  `fixture_root_picking_prefers_roomy_candidates` still passes unchanged.
- After compiling, run the full `compiled_artifact_test` target if either Unix
  candidate probes at least 8 GiB free. If neither qualifies, run `--no-run`
  and all six focused tests; defer fixture integration verification to CI.
- Implementation changes are limited to `crates/camel-cli/tests/compiled_artifact_test.rs`.
- `skip_specs: true` holds: no delta specs, no `openspec/specs` change
  (test-only fix).

## Risk budget

- Acceptable: selection-order change visible only when candidates are on
  different filesystems; doc-comment rewrites.
- Out of bounds: raising the constant as the whole fix, deleting the gate,
  touching CI workflows or anything under `.opencode/agents/*`,
  `.opencode/plugins/fleet.js`, `opencode.json`.

Affected crates: camel-cli (tests only). bd: rc-bpwa4.
