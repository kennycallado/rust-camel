# Verification: nondsl-channel-fuzzing

First scoped fuzz runs (Task 3.1, conductor-executed under fleet
containment). All runs:
`systemd-run --user --scope --collect --unit=fleet-fuzznondsl
-p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill
-- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache
cargo run --package xtask -j4 -- fuzz <target> --time 60`, scope
stopped after each run; `pgrep -c compiled_artifa` = 0 before/after
every run.

| Target | Runs | Wall | Corpus files | Crash artifacts |
|---|---|---|---|---|
| dsl_rest | 884,320 | 61 s | 4,416 | none |
| dsl_mcp | 939,314 | 61 s | 4,111 | none |
| dsl_openapi | 1,183,990 | 61 s | 4,566 | none |

Crash triage: **no crashes found** — no tmin minimization, no
regression-test promotion, no bd issues filed from the runs. The
standard pipeline applies to future finds (tmin → committed regression
test → bd, per fuzz-tooling spec "Crash minimization and promotion").

Corpus/artifact hygiene: bulk corpora live under worktree-local
`target-fuzz/` (1.9 GB, git-ignored, purgeable); no corpus or crash
files enter the repo; committed seeds are the minimized
`fuzz/seeds/<target>/` sets only.

Ranking note (design.md entry-point audit): rest (1921 LoC,
control-plane, listener-constructing) > mcp (1449 LoC, control-plane,
schema/URI → runtime routing keys) > openapi (900 LoC, generation-only —
no external-document ingestion path exists; bd hypothesis corrected).

Prior-task verification evidence (reviewed, r_glm APPROVE ×4):
- fuzz crate tests green scoped (`cargo test --manifest-path
  fuzz/Cargo.toml`: 37 lib + 5 harness_semantics + 3 seeds).
- xtask tests green scoped (`known_targets_cover_all_seven`,
  `known_targets_seeds_dirs_exist` over 7 seed dirs).
- `cargo fuzz list` = 7 targets; `scripts/fuzz-legs.sh --self-test`
  15/15; stdin cases spec-exact.
- Live instrumented smoke of each target via xtask (seeds copied,
  runs completed, exit 0).
