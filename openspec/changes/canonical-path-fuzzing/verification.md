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
