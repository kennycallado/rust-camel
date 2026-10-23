# Tasks: ci-cxf-narrow-compile

## ci-workflow

### Task 1.1: Add narrow cxf compile gate step to ci.yml

**Files:**
- `.github/workflows/ci.yml` (modified)

**Steps:**
1. In job `full-tests-linux`, insert a new step immediately AFTER the
   existing step named `Compile-gate gated integration test set` and
   BEFORE the step named `Test (full workspace)`, with exactly:
   name: `Compile-gate narrow cxf integration feature`,
   run: `cargo test -p camel-test --features integration-tests-cxf --test cxf_test --no-run`
2. Give the new step a YAML comment block (above `name:`) that: cites
   bd rc-z1ea2; states that `--all-features` clippy and the wide
   `--features integration-tests` steps activate
   `dep:camel-component-cxf` as part of a wider union and therefore
   cannot catch a defect confined to the `integration-tests-cxf`
   feature line; states the command mirrors the compile the
   published-bridge smoke runs (compile + link, no execution).
3. Do not add `if:`, `continue-on-error`, `working-directory`, or env
   keys to the step; do not modify any other step, job, or workflow
   file.
4. Validate the workflow still parses as YAML (Task 1.1 tests below).

**Tests:** (executable spec — non-Rust: shell assertions)
- `yaml-parse-and-step-assertions`: setup → worktree at any state;
  action → `python3 -c` script that `yaml.safe_load`s
  `.github/workflows/ci.yml`, locates job `full-tests-linux`, and
  asserts: (a) exactly one step has `run` equal to
  `cargo test -p camel-test --features integration-tests-cxf --test cxf_test --no-run`;
  (b) that step's `name` is `Compile-gate narrow cxf integration feature`;
  (c) the step dict has no `if` and no `continue-on-error` keys;
  (d) its index is greater than the index of the step whose `name`
  contains `Compile-gate gated integration test set` and less than the
  index of the step whose `name` is `Test (full workspace)`;
  (e) `published-bridge-smoke.yml` is byte-identical to the base commit
  (`git diff 6c4ab151c -- .github/workflows/published-bridge-smoke.yml`
  empty) → assert exit code 0.
- `diff-scope`: `git diff 6c4ab151c --name-only` lists exactly
  `.github/workflows/ci.yml` (plus `openspec/changes/…` plan files) →
  assert no other `.github/` path appears.

**Acceptance:**
- `python3` YAML assertion script exits 0.
- `git diff 6c4ab151c --stat -- .github/` shows exactly one file
  changed, with a small addition (~10-15 lines) in `ci.yml`.
- No `if:`/`continue-on-error` on the new step (assertion (c) covers
  this mechanically).

- [x] 1.1

## evidence

### Task 2.1: Sabotage demonstration + evidence file

**Files:**
- `openspec/changes/ci-cxf-narrow-compile/evidence.md` (new)
- `crates/camel-test/Cargo.toml` (modified TEMPORARILY during the task
  only — MUST be restored before the task reports done; never committed)

**Steps:**
1. Record provenance in evidence.md header: worktree path, branch
   `feature/ci-cxf-narrow`, base `6c4ab151c`, HEAD at task start, and
   the exact command templates used.
2. Baseline pass: run the exact new step command in the worktree via
   `systemd-run --user --unit=386-narrow-baseline --collect --wait
   --pipe -p MemoryMax=12G -p MemorySwapMax=2G
   --setenv=TMPDIR=/home/shared/tmp
   --working-directory=/home/shared/rust-camel-worktrees/386-ci-cxf-narrow
   -- cargo test -p camel-test --features integration-tests-cxf
   --test cxf_test --no-run`. `--wait --pipe` makes the call
   synchronous and forwards Cargo's exit status and output to the
   caller — assert on THAT status, never on systemd unit start. Each
   invocation MUST finish before the manifest is mutated or restored
   (guaranteed by `--wait`; do not background). Capture
   exit code and elapsed wall-clock. If sccache fails with exit 254,
   retry once with `RUSTC_WRAPPER=` empty and record both attempts.
   Record cache state (warm target dir from prior builds in this
   worktree) and that CARGO_INCREMENTAL is unset locally (CI sets "0").
3. Sabotage: change
   `integration-tests-cxf = ["dep:camel-component-cxf"]` to
   `integration-tests-cxf = []` in `crates/camel-test/Cargo.toml`
   (line 122, working tree only, do NOT stage or commit). Re-run the
   same contained command (same flags as step 2, unit name
   `386-narrow-sabotage`, after the baseline invocation has fully
   returned). Capture: nonzero exit code, and the rustc
   diagnostic naming `camel_component_cxf` (E0432/E0463 unresolved
   crate) from the forwarded output.
4. Restore: `git checkout -- crates/camel-test/Cargo.toml`; assert
   `git status --porcelain crates/camel-test/Cargo.toml` is empty;
   re-run the contained command (unit name `386-narrow-restore`) and
   assert exit 0 (restoration proof).
5. Write evidence.md with these sections: (a) Inventory — every job in
   `.github/workflows/*.yml` that compiles camel-test with ANY
   integration feature, with file:line refs (ci.yml wide run line 129,
   wide compile gate line 169, clippy --all-features in AGENTS.md
   gates, published-bridge-smoke.yml narrow lines 151/172 release-only)
   and the conclusion that no PR job compiles the narrow feature;
   (b) Diff — the ci.yml step added by Task 1.1 (quote it); (c) Cost —
   measured local elapsed from step 2, mission 377 cold bound 449.262 s
   as provenance, 120-minute `full-tests-linux` timeout (ci.yml:73);
   (d) Sabotage — transcript excerpts + exit codes from steps 2-4,
   including the restore proof; (e) Gates — table of gates run/N-A for
   this diff (no actionlint/yamllint exists in the repo; YAML parse
   check from Task 1.1 is the syntax gate).

**Tests:** (executable spec — non-Rust: command outcomes)
- `baseline-pass`: setup → clean tree after Task 1.1; action →
  contained step command; assert exit code 0 and record elapsed.
- `sabotage-fail`: setup → `integration-tests-cxf = []`; action → same
  contained command; assert exit code nonzero AND output contains an
  unresolved-crate diagnostic for `camel_component_cxf`.
- `restore-proof`: action → `git checkout --` then
  `git status --porcelain crates/camel-test/Cargo.toml` → assert
  empty; re-run contained command → assert exit 0.

**Acceptance:**
- evidence.md exists with all five sections (a)-(e) and real exit
  codes (no placeholders, no TBD).
- `git log --oneline` for the task shows NO commit touching
  `crates/camel-test/Cargo.toml`; final
  `git status --porcelain` shows only intended files.
- Sabotage failure output quoted in evidence.md names
  `camel_component_cxf`.

- [x] 2.1
