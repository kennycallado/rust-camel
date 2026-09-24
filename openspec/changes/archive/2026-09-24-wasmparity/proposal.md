# Proposal: wasmparity

## Why

The 2026-09-24 camel-test perf measurement pass
(docs/audits/2026-09-24-cameltest-perf-measurements.md, "Wasm") found that
`camel test` cannot execute wasm scenarios at all while `camel run` boots the
same project cleanly. Users have no wasm coverage in the declarative testkit,
and the one signal they get — the tier label — lies.

Two defects, one mission (bd rc-l3zrr, rc-4sb1x):

1. **Scenario boot drops the wasm base dir** (rc-l3zrr). When a scenario
   document is named as a bare relative filename (`camel test wdoc.test.yaml`
   from the project directory), `Path::parent()` yields the empty path,
   `find_camel_toml_root("")` accepts the empty ancestor (its
   `Camel.toml` join resolves against the process CWD), and the empty root
   propagates through `run_scenario_doc` → `boot_scenario` →
   `camel_bundles::boot` into `WasmBundle`/`WasmComponent`. The base dir
   arrives as `""`, `canonicalize()` fails, and the run exits 2 with
   `failed to resolve base directory: ` (empty path display). `camel run`
   is immune: `try_canonical_project_root` (run.rs) maps an empty parent to
   `.` before use. Reproduced on main @ fec12d64: bare filename fails,
   absolute path boots the same document with all actions passing.

2. **Unit-tier `[full]` label is dishonest** (rc-4sb1x). A unit document
   with a `wasm:` step derives tier FULL (`[full]` annotation), but unit
   documents always execute on the lean 5-component registry (ADR-0064:
   direct, log, mock, seda, timer — pinned set). The run then fails with a
   bare `Component not found: wasm` that names neither the lean registry
   nor the integration-tier alternative.

## What Changes

- **Scenario boot root normalization**: `boot_scenario`
  (camel-integration-test) normalizes an empty boot root to `.` (camel-run
  parity) so the wasm base dir always receives a usable directory. No
  change to `camel run`, `find_camel_toml_root` walk semantics, or the
  lean-tier route resolution.
- **Tier-label honesty**: unit documents that derive FULL annotate
  `[full*]` (derived full, executed on the lean boot) with one stderr
  advisory line, and the lean runner's route-add registry miss becomes
  actionable (names the lean set, points at the scenario vocabulary). The
  lean registry itself is NOT grown (ADR-0064 amendment is the only gate).
- **Spec deltas**: integration-tier (boot-root usability clause),
  mock-testkit (tier annotation honesty, actionable lean registry miss).

## Acceptance Criteria

- Scenario document with a `wasm:` route, invoked by bare relative
  filename from its project root, boots and executes the wasm step with a
  non-empty base dir (regression: absolute-path invocation unchanged).
- Unit document with a `wasm:` step reports `[full*]` and a failure naming
  the lean registry and the scenario-tier alternative — never a bare
  `Component not found: wasm`.
- `camel run` wasm behavior untouched (no run.rs change).

## Risk Budget

Low. The scenario fix is a one-seam path normalization inside the offline
test boot; the label change touches annotation text only (stdout contract
`[lean]`/`[full]` gains the `full*` form for unit-derived-full documents).
Risk of parser breakage for CI consumers of the annotation is accepted:
the old `[full]` on lean-executed documents was false advertising.
