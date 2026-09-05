# Proposal: mutation-testing-tooling

## Why

The fuzzing-mutation adoption epic (bd rc-4g9j, decision memo 2026-08-31,
e_opus) is half-delivered: cargo-fuzz phase 1 shipped (rc-a456 — `xtask fuzz`,
excluded `fuzz/` crate, `dsl_yaml` target, non-blocking `fuzz-smoke.yml`) and
canonical-path fuzzing is owned by rc-fvah (another agent, in progress). The
remaining unowned piece is bd **rc-eba8**: scoped, informational
**cargo-mutants** adoption — a periodic test-quality probe on security-critical
modules, per the adjudicated decision:

- **cargo-mutants — ADOPT SCOPED, INFORMATIONAL**: periodic probe on
  security-critical modules; never a gate, never workspace-wide, no thresholds.

rc-eba8's success criterion (from e_opus): the audit's adversarial tests kill
**> 90%** of mutants in the probed modules — a measured, informational
baseline, not an enforced number.

## What Changes

**Included (single phase)**:
- `cargo xtask mutants [--file P | --diff] [--json]` subcommand: main-checkout
  refusal guard, `cargo-mutants` presence check, forced
  `CARGO_TARGET_DIR=<worktree>/target-mutants` (git-ignored).
- flake.nix devShell: `pkgs.cargo-mutants` (locked nixpkgs = 27.1.0, the
  pinned version) next to `pkgs.cargo-fuzz`; stable toolchain, no shim.
- `.cargo/mutants.toml`: two-tier scope (see design for exact globs) —
  baseline `examine_globs` on the three narrow security files; event-driven
  `--file` runs for broad files.
- `.gitignore` gains `/target-mutants/`.
- Baseline informational run + survivor triage: actionable survivors become bd
  issues; kill-rate measurement recorded in the bd issue, enforced nowhere.

**Explicitly excluded (binding anti-obstruction policy)**:
- No mutation-score threshold anywhere (not hard, not ratchet, not CI-enforced).
- Neither tool enters `QUALITY GATES`; no required checks.
- No workspace- or crate-scope mutants (30–60 h trap).
- No fuzz-target work (owned by rc-fvah; this change ships zero fuzz code).
- No production-code changes to the probed modules (survivor FIXES are
  follow-up bd issues, not part of the tooling change).

## Acceptance criteria

- The prebuilt wrapper binary (`$WT/target/debug/xtask mutants` — the
  `cargo xtask` alias may itself build xtask, so isolation acceptance uses
  the binary directly) runs the baseline probe on exactly the three pinned
  narrow files; MUTATION artifacts under `target-mutants/` only; both
  target trees' mtimes unchanged (fingerprint-verified).
- `cargo xtask mutants` in the main checkout exits non-zero with the
  main-checkout refusal message and starts no instrumented build.
- Baseline probe measures < 15 min against the RECORDED dev-workstation
  environment (12 cores / 27 GB at plan time; each measurement records
  `nproc` + `free -h`) — engineering guidance, not an enforced timeout and
  not an ubuntu-latest claim (no CI job runs mutants in this change).
- Kill-rate baseline (> 90% target, scoped to the three-file baseline —
  broad families produce survivor lists, not rates) recorded informationally
  in bd rc-eba8; no CI job or gate consumes it. Measurement runs AFTER
  rc-fvah lands (epic ordering); the tooling itself lands independently.
- Root `Cargo.lock` byte-identical (cargo-mutants is a dev-installed tool,
  not a dependency).
- Developer loop untouched: `cargo build`/`test`/`clippy` behavior and
  timings unchanged.

## Risk budget

- Acceptable: +2–5 GB worktree-local, git-ignored, purgeable
  (`target-mutants/`); a one-off baseline run of ~15 min.
- Out of bounds: any instrumented build in the default `./target`; any new
  top-level dependency; any gate/threshold; mutating outside the pinned
  scope; CI time additions (baseline runs on developer request only in this
  change).
