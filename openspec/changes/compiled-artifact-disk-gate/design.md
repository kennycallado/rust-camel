# Design: compiled-artifact-disk-gate

## Approach

Two coordinated edits inside `crates/camel-cli/tests/compiled_artifact_test.rs`
(test file only; no source changes, no spec deltas — `skip_specs: true`).

**1. Need-based root selection (`pick_root`).** Today the function scans
candidates positionally and returns the first whose probed free space reaches
`required`. New semantics, same signature
(`candidates, required, free_of) -> Result<PathBuf, String>`):

- Probe every candidate once; a candidate qualifies when `free_of` returns
  `Some(free)` with `free >= required`. `None` (unprobeable filesystem)
  still counts as insufficient — unchanged.
- Among qualifying candidates, return the one with the **most headroom**
  (`free - required`); the **earliest candidate wins ties**. The candidate
  list stays `[std::env::temp_dir(), cargo_target_dir()]`, so on CI (both
  candidates on the runner root filesystem, equal headroom) the temp dir
  keeps winning — unchanged CI path.
- When nothing qualifies, return the same named error —
  `no temp root has the required free space: need {required} bytes,
  candidates [{listed}]` — byte-identical shape, so existing log triage
  (bd rc-bpwa4 quotes it) keeps matching. The suite aborts only when **no**
  candidate is usable; there is no skip path.

Rationale: choosing more headroom reduces mid-suite ENOSPC risk but does not
reserve space. Equal probe results preserve temp-first selection.

**2. The requirement derives from the measured footprint.** Replace the
bare `8 << 30` with named MiB constants summing to the same value, per the
mission-538 measurement recorded in bd rc-bpwa4:

```text
FIXTURE_PERSISTENT_MIB  = 4_526   // 14 fixture compiles, measured 4.42 GiB
FIXTURE_TRANSIENT_MIB   = 2_560   // 9 concurrent whole-artifact copies
                                  // x 296.7 MB on 12 threads, measured 2.50 GiB
FIXTURE_MARGIN_MIB      = 1_106   // 1.16x measured peak (6.92 GiB) -> headroom
REQUIRED_ROOT_FREE      = (PERSISTENT + TRANSIENT + MARGIN) << 20  // = 8 << 30
```

The value is unchanged — the mission-538 bd ruling validated 8 GiB as 1.16×
the measured peak and the order forbids raising it as the whole fix. What
changes: the arithmetic is named, documented, and pinned by a test, so the
next person who bumps it must confront the footprint components instead of
a magic number. Doc comments are updated to cite the measurement and drop
the stale `~3.7 GiB` / `~4.3 GiB` estimates (both superseded by the 6.92 GiB
measured peak; rc-fdkta stays cited as the reason the guard exists).

**Target/ reclaim:** out of scope. The fix changes selection and derivation
only; the gate value and the candidate set are unchanged. The residual CI
risk (one shared filesystem below the gate after the workspace compile) is
the workflow-side lever already recorded in bd (mission-538 options:
`--test-threads` cap, runner disk, df visibility); it is out of scope here
and restated in the park report.

## Affected crates

- camel-cli: `tests/compiled_artifact_test.rs` only — `pick_root`, the
  `REQUIRED_ROOT_FREE` constant block, `fixture_root`/`pick_root` doc
  comments, plus the new tests.

## Architecture boundaries

Test-harness-internal. No runtime, DSL, component, service, language, or
function-surface change; no public API; no `openspec/specs` content.
The hexagonal boundary suite is unaffected (no `camel-core` import change).

## Alternatives considered

- **Lower `REQUIRED_ROOT_FREE` to the bare 6.92 GiB peak.** Rejected: drops
  the safety margin that absorbed binary growth since the measurement; the
  mission-538 ruling calls the 8 GiB gate correct, and the failed run's
  free space is unknown (likely below even the bare peak).
- **Cap suite concurrency (`--test-threads=4`) to shrink the peak.** Workflow
  change, slower suite, explicitly a mission-538 follow-up option — out of
  scope for this test-only change.
- **Reclaim `target/` from inside the test.** Rejected: deleting workspace
  build artifacts from a test process is unsafe (concurrent builds, cargo
  fingerprint state) and the gate's job is to choose a writable root, not to
  janitor the workspace.
- **Skip the suite when no candidate qualifies.** Rejected by the order: a
  silent skip hides the ENOSPC class the guard exists to surface.
