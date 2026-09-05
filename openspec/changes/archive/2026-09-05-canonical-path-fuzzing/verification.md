# Verification

## Local drills

Local panic-injection drills for the canonical-path fuzz targets, run on
2026-09-05 in a detached throwaway worktree at `d7808ae8`
(`/home/shared/rust-camel-worktrees/cpf-drill`, removed after the drills).
For each target the harness call in `fuzz/fuzz_targets/<T>.rs` was preceded
by an uncommitted `panic!("local-drill");`, the xtask wrapper ran the target
for 20 seconds, and five assertions were checked: non-zero exit, a
`new artifact(s):` line, a `minimized artifact:` line whose file exists
under `target-fuzz/artifacts/<T>/`, no regular files under `fuzz/artifacts/`,
and the regression-test promotion instruction. Both artifact locations were
removed and the injection reverted after each drill (bd rc-0wb9 birth-time
protection). Local cargo-fuzz is 0.13.1 (CI pins 0.13.2); `tmin` semantics
are identical for this drill.

Evidence note: the first `dsl_json` round was reported BLOCKED because the
fifth assertion originally demanded directory absence, but cargo-fuzz 0.13.x
creates `fuzz/artifacts/<target>/` unconditionally; the assertion was
amended to emptiness (no regular files) and `dsl_json` was re-run clean and
passed all assertions.

### dsl_json

- New artifact(s) line: present
- Command: `nix develop . -c bash -c 'cargo run --quiet --package xtask -- fuzz dsl_json --time 20'`
- Exit code: `1` (non-zero, expected on the injected panic)
- Minimized artifact line: `minimized artifact: /home/shared/rust-camel-worktrees/cpf-drill/target-fuzz/artifacts/dsl_json/crash-da39a3ee5e6b4b0d3255bfef95601890afd80709`
- Artifact exists under `target-fuzz/artifacts/dsl_json/`: yes (`test -f` pass; 0-byte input, already minimal — the panic fires on any input)
- `fuzz/artifacts/` emptiness: pass (no regular files under it)
- Promotion instruction: present (`promote this input into a #[test] regression case; do not commit the raw artifact`)

Verdict: `drill-dsl_json-catches-and-minimizes` — PASS.

### dsl_template

- New artifact(s) line: present
- Command: `nix develop . -c bash -c 'cargo run --quiet --package xtask -- fuzz dsl_template --time 20'`
- Exit code: `1` (non-zero, expected on the injected panic)
- Minimized artifact line: `minimized artifact: /home/shared/rust-camel-worktrees/cpf-drill/target-fuzz/artifacts/dsl_template/crash-da39a3ee5e6b4b0d3255bfef95601890afd80709`
- Artifact exists under `target-fuzz/artifacts/dsl_template/`: yes (`test -f` pass; 0-byte input — identical crash-da39a3ee hash means the ALREADY_MINIMAL fallback fired here too)
- `fuzz/artifacts/` emptiness: pass (no regular files under it)
- Promotion instruction: present (`promote this input into a #[test] regression case; do not commit the raw artifact`)

Verdict: `drill-dsl_template-catches-and-minimizes` — PASS.

### dsl_parity

- New artifact(s) line: present
- Command: `nix develop . -c bash -c 'cargo run --quiet --package xtask -- fuzz dsl_parity --time 20'`
- Exit code: `1` (non-zero, expected on the injected panic)
- Minimized artifact line: `minimized artifact: /home/shared/rust-camel-worktrees/cpf-drill/target-fuzz/artifacts/dsl_parity/crash-da39a3ee5e6b4b0d3255bfef95601890afd80709`
- Artifact exists under `target-fuzz/artifacts/dsl_parity/`: yes (`test -f` pass; 0-byte input — identical crash-da39a3ee hash means the ALREADY_MINIMAL fallback fired here too)
- `fuzz/artifacts/` emptiness: pass (no regular files under it)
- Promotion instruction: present (`promote this input into a #[test] regression case; do not commit the raw artifact`)

Verdict: `drill-dsl_parity-catches-and-minimizes` — PASS.

Summary: 3 drills, 15/15 assertions pass. The wrapper caught every injected
panic, kept all artifacts under `target-fuzz/artifacts/<target>/`, and left
`fuzz/artifacts/` without files.

## CI dispatch evidence

- Run: https://github.com/kennycallado/rust-camel/actions/runs/33976608549
  (workflow_dispatch, all legs, 2026-09-05, ~15 min wall — well under the
  30 min ceiling and the 26 min papal comfort threshold; cache-cold for
  cargo-fuzz-root which was naturally evicted earlier).
- `FUZZ_LEGS: dsl_yaml dsl_json dsl_template dsl_parity` — all four legs
  selected and ran (papal condition: leg-set assertion ✓).
- **Real finding recorded (the no-gating design working as intended):**
  the `dsl_parity` smoke leg found a crash — the summarize step exited 1
  rendering findings/triage, the job stayed green via continue-on-error.
  See `## First real finding` below. Every other check passed
  (cold-baseline, refusal-drill, smoke for the other three legs,
  tmin-drill on dsl_json — 1.9 s fast-path: instant crash + already-minimal,
  fuzz-lock-audit with the allowlisted RUSTSEC-2025-0134, cold-main-final).
- Cache eviction (papal condition): vacuously satisfied this run —
  post-save steps were skipped on job failure (`CACHE_ON_FAILURE: false`),
  so nothing was saved and nothing could evict main-CI caches.

## First real finding

- `dsl_parity` differential harness caught a front-end divergence within
  60 s of its first real run (CI leg and local repro, 2026-09-05):
  input `[]` (2 bytes) deserializes `Ok` via serde_json (positional-seq
  quirk with all-default fields) while the YAML serde front-end rejects
  the seq form. Panic: `parity divergence: yaml rejects json-valid
  document`.
- Local confirmation runs (feature worktree, 60 s each): `dsl_json`
  1,505,141 runs clean; `dsl_template` 1,304,487 runs clean;
  `dsl_parity` crashes deterministically.
- Filed as bd rc-m5ah (p1, fix direction: require mapping form in both
  front-ends per schemas/dsl/route-schema.json type:object; promoted
  #[test] regression with the inline 2-byte input). Wrapper near-minimal
  tmin gap observed during the same run filed as bd rc-fqfe (p3).

## Recorded residuals (papal review, 2026-09-03)

- (a) The flattened step-list comparison cannot detect a step
  redistribution across routes that preserves route count, every route's
  id/from, and the total step multiset. Accepted: blessed mechanism
  (mirrors parity_tests.rs); the count/id/from pre-checks narrow the
  escaping class to route-boundary permutations only.
- (b) camel-fuzz unit tests have no automated CI execution. Accepted:
  the fuzz crate is workspace-excluded by the disk-isolation doctrine;
  the CI smoke/tmin drills exercise the harnesses end-to-end, and the
  tests pass locally (20/20).
