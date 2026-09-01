# Tasks: fuzz-smoke-ci

## Phase 1: fuzz-smoke CI workflow + wrapper tmin fix

### Task 1: forward `-artifact_prefix` to `cargo fuzz tmin`

Files:
- `scripts/xtask/src/fuzz.rs` (modified)

Steps:
1. In `scripts/xtask/src/fuzz.rs`, add a pure helper next to `libfuzzer_args`:
   ```rust
   /// libFuzzer argument suffix for `cargo fuzz tmin`: separator plus the
   /// artifact prefix (so the minimized output lands in the scanned
   /// `target-fuzz/artifacts/<target>/` — cargo-fuzz's default is
   /// `fuzz/artifacts/`, which the wrapper never scans) plus a per-round
   /// time cap (each minimization round is bounded; total may span
   /// several rounds — the job's timeout is the hard ceiling).
   fn tmin_arg_suffix(prefix: &str, max_total_time: u64) -> Vec<String> {
       vec![
           "--".to_string(),
           format!("-artifact_prefix={prefix}"),
           format!("-max_total_time={max_total_time}"),
       ]
   }
   ```
2. In `fn minimize(root, target, target_dir, artifact, artifacts)`, change the command construction from
   `.args(["+nightly", "fuzz", "tmin", target]).arg(artifact)`
   to
   `.args(["+nightly", "fuzz", "tmin", target]).arg(artifact).args(tmin_arg_suffix(&artifact_prefix(root, target), 120))`
   reusing the existing `artifact_prefix` helper (fuzz.rs:31) as the single owner of the trailing-slash format, keeping `CARGO_TARGET_DIR` and the stderr-in-error-message behavior unchanged.
   Treat tmin's exit status as ADVISORY (amendment after r_glm round 1, sourced from cargo-fuzz 0.13.2 `exec_tmin`: after a successful minimization it scans `fuzz/artifacts/<target>/` for new artifacts and propagates NotFound as a non-zero exit — so exit ≠ 0 does NOT imply minimization failed in the wrapper's redirected layout): regardless of exit status, run `entries_created_after(artifacts, started)`; if a fresh artifact exists → return `Ok(Some(newest))` (when the status was non-zero, also `eprintln!` one line noting tmin exited non-zero but the minimized artifact was produced); `Err` (with the captured-stderr note, existing format) only when the status failed AND no fresh artifact exists; `Ok(None)` (existing diagnostic path) when the status succeeded but no fresh artifact appeared.
3. Run `cargo test -p xtask fuzz` and `cargo test -p xtask tmin` — all pass.

Tests:
- name: `tmin_arg_suffix_forwards_prefix_and_cap`
  setup: helper `tmin_arg_suffix` exists (introduced in step 1)
  action: `assert_eq!(tmin_arg_suffix("/wt/target-fuzz/artifacts/dsl_yaml/", 120), vec!["--".to_string(), "-artifact_prefix=/wt/target-fuzz/artifacts/dsl_yaml/".to_string(), "-max_total_time=120".to_string()])`
  assert: exact three-element vector
  command: `cargo test -p xtask tmin`
  expected: fails to compile before step 1 (missing symbol), passes after step 2
- name: `tmin_arg_suffix_separator_first`
  setup: same helper
  action: inspect first element of the returned vector
  assert: first element is exactly `"--"` (libFuzzer args come after the separator)
  command: `cargo test -p xtask tmin`
  expected: passes after implementation
- name: existing `fuzz::tests` suite
  setup: the 7 pre-existing tests in `mod tests` (incl. `libfuzzer_args_shape`, `main_checkout_detected`)
  action: `cargo test -p xtask fuzz`
  assert: all pass, zero regressions
  command: `cargo test -p xtask fuzz`
  expected: pass before and after (minimize() is not unit-tested end-to-end; its artifact-prefix effect is asserted by the CI tmin drill in Task 2)

Acceptance:
- `cargo test -p xtask` exits 0 (full crate suite)
- `cargo clippy -p xtask -- -D warnings` exits 0
- `cargo fmt --check --all` exits 0
- `cargo run --package xtask -- lint-unwrap` exits 0
- `grep -n 'tmin_arg_suffix' scripts/xtask/src/fuzz.rs` shows helper + call site (2+ hits)

- [x] 1.1

### Task 2: `.github/workflows/fuzz-smoke.yml` with drills

Files:
- `.github/workflows/fuzz-smoke.yml` (new)
- `.github/workflows/fuzz-smoke-trigger-check.py` (new)

Steps:
1. Create `.github/workflows/fuzz-smoke.yml` with:
   - `name: fuzz-smoke`; `on: pull_request` targeting `branches: ["main"]` with `paths: [crates/camel-dsl/**, fuzz/**, scripts/xtask/**, .github/workflows/fuzz-smoke.yml]`; `on: workflow_dispatch` with `inputs.time` (`type: number`, `default: 60`, `description: seconds per target`);
   - `permissions: contents: read`; `concurrency: group: fuzz-smoke-${{ github.ref }}`, `cancel-in-progress: true`;
   - env: `CARGO_FUZZ_VERSION: "0.13.2"` (newest crates.io stable, 2026-06-09 — never `latest`), `SMOKE_TIME: ${{ inputs.time || 60 }}`;
   - one job `fuzz-smoke`: `runs-on: ubuntu-latest`, `continue-on-error: true`, `timeout-minutes: 20`;
   - **execution semantics (load-bearing)**: job-level `continue-on-error` does NOT run later steps after a failed step — so every drill/audit step ends with `exit 0` and appends its outcome to `$RUNNER_TEMP/findings.md` (format: `PASS <name>` / `FAIL <name> <detail>`); only the summary step exits non-zero when findings exist. Infra steps (checkout/toolchain/install) keep normal fail-fast semantics — their failure is a broken workflow, not a drill finding. Every step carries an `id:`.
   - steps (ids in parens):
     a. (`checkout`) `actions/checkout` at the repo's pinned SHA (`3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1`);
     b. (`cold-baseline`) `mkdir -p "$RUNNER_TEMP/fuzz-smoke-state" && : > "$RUNNER_TEMP/findings.md"`; `test ! -d ./target` and append `PASS/FAIL cold-baseline` to findings.md;
     c. (`nightly`) `dtolnay/rust-toolchain` at the repo's pinned SHA (`4cda84d5c5c54efe2404f9d843567869ab1699d4`) with `toolchain: nightly`;
     d. (`cache-cargo-fuzz`) `actions/cache` at the repo's pinned SHA (`55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0`, the pin the bridge-release workflows use) with `path: ~/.cargo/bin`, `key: cargo-fuzz-${{ env.CARGO_FUZZ_VERSION }}-${{ runner.os }}`;
     e. (`install-cargo-fuzz`) `if: steps.cache-cargo-fuzz.outputs.cache-hit != 'true'` → `cargo install cargo-fuzz --locked --version "${CARGO_FUZZ_VERSION}"` (single install path — no tool-branching);
     e2. (`verify-cargo-fuzz`) unconditional: `cargo +nightly fuzz --version | grep -q "${CARGO_FUZZ_VERSION}"` (fails the job on version drift, cached or fresh);
     f. (`rust-cache`) `Swatinem/rust-cache` at the repo's pinned SHA (`e18b497796c12c097a38f9edb9d0641fb99eee32 # v2`) with `cache-targets: false`;
     g. (`refusal`) drill step: `set +e`; `CARGO_TARGET_DIR="$RUNNER_TEMP/xtask-refusal-target" cargo run --package xtask -- fuzz dsl_yaml > "$RUNNER_TEMP/fuzz-smoke-state/refusal.log" 2>&1`; `ec=$?`; assertions: `ec -eq 1` AND log contains `refusing: cargo xtask fuzz must run in a linked worktree` AND `test ! -d ./target`; append `PASS/FAIL refusal-drill`; `exit 0`;
     h. (`worktree`) `git worktree add "$RUNNER_TEMP/fuzz-wt" HEAD`;
     i. (`smoke`) drill step: `set +e`; from `"$RUNNER_TEMP/fuzz-wt"`: `cargo run --package xtask -- fuzz dsl_yaml --time "$SMOKE_TIME" > smoke.log 2>&1`; `ec=$?`; assertions: `ec -eq 0` AND `find target-fuzz/corpus/dsl_yaml -type f | grep -q .` AND no `target-fuzz/artifacts/dsl_yaml/crash-*` file; back in the checkout `test ! -d ./target`; append `PASS/FAIL smoke`; `exit 0`;
     j. (`tmin`) drill step: `set +e`; inject panic into the WORKTREE copy only — `fuzz/fuzz_targets/dsl_yaml.rs` is a 5-line `libfuzzer_sys::fuzz_target!` macro (there is no `fn dsl_yaml`); `sed -i 's|camel_fuzz::dsl_yaml_harness(data);|panic!("fuzz-smoke drill"); camel_fuzz::dsl_yaml_harness(data);|' "$RUNNER_TEMP/fuzz-wt/fuzz/fuzz_targets/dsl_yaml.rs"`; from the worktree `cargo run --package xtask -- fuzz dsl_yaml --time 20 > tmin.log 2>&1`; `ec=$?`; assertions: `ec -ne 0` AND log contains `new artifact(s):`, `minimized artifact:`, `promote this input` AND ≥1 worktree file matching `target-fuzz/artifacts/dsl_yaml/minimized-from-*` AND `test ! -d "$RUNNER_TEMP/fuzz-wt/fuzz/artifacts"` AND in the checkout `git status --porcelain | wc -l` is 0 AND `test ! -d ./target`; append `PASS/FAIL tmin-drill`; `exit 0`;
     k. (`install-cargo-audit`) `taiki-e/install-action` (pinned SHA `eba66cc6f87204a1e73f96e528e759b6c1fcf573`) with `tool: cargo-audit`;
     k2. (`audit`) drill step: `set +e`; `cargo audit --file fuzz/Cargo.lock > "$RUNNER_TEMP/fuzz-smoke-state/audit.log" 2>&1`; `ec=$?`; append `PASS/FAIL fuzz-lock-audit` (advisories = FAIL); `test ! -d ./target`; `exit 0`;
     l. (`summary`) `if: always()`: final `test ! -d ./target` appended as `PASS/FAIL cold-main-final`; then integrity-check findings.md against the expected outcome set — `cold-baseline`, `refusal-drill`, `smoke`, `tmin-drill`, `fuzz-lock-audit`, `cold-main-final` — if ANY expected entry is MISSING, write to `$GITHUB_STEP_SUMMARY` "infrastructure failure — checks skipped: <missing list>" and `exit 1` (an infra-failed job must never render all-clear); if any entry is `FAIL`: write the failing checks, artifact paths under the worktree's `target-fuzz/artifacts/`, the promotion instruction (minimized input → committed `#[test]` in `crates/camel-dsl`, never a raw corpus blob), the triage command `bd create -t bug -p 1`, then `exit 1` (job-level `continue-on-error: true` turns this into an annotation, not a red X); only if all six entries are present and PASS, write the all-clear with measured durations and `exit 0`.
2. Create `.github/workflows/fuzz-smoke-trigger-check.py` implementing exactly the `trigger_semantics_exact` spec below (pyyaml `on:`→True key handling included); `python3 .github/workflows/fuzz-smoke-trigger-check.py` exits 0.
3. Local structural verification (see Tests).

Tests (local, deterministic):
- name: `workflow_yaml_parses`
  setup: file exists
  action: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/fuzz-smoke.yml'))"`
  assert: exit 0
  command: as above, from the worktree root
  expected: pass after step 1
- name: `trigger_semantics_exact`
  setup: file exists
  action: python3 parses the YAML and asserts the trigger contract (note: pyyaml parses the `on:` key as boolean True — resolve with `doc.get(True) or doc.get('on')`)
  assert: `'push'` is NOT a trigger key; `pull_request.branches == ["main"]`; `pull_request.paths == ["crates/camel-dsl/**", "fuzz/**", "scripts/xtask/**", ".github/workflows/fuzz-smoke.yml"]` (exactly 4, exact order-insensitive set match); `workflow_dispatch` present with `inputs.time.default == 60`; `jobs.fuzz-smoke['continue-on-error'] is True`; `jobs.fuzz-smoke['timeout-minutes'] == 20`
  command: `python3 .github/workflows/fuzz-smoke-trigger-check.py` (the worker writes this exact assertion script as part of the task, from this spec)
  expected: pass
- name: `workflow_structural_assertions`
  setup: file exists
  action: grep for each pattern: `cache-targets: false`, `CARGO_FUZZ_VERSION: "0.13.2"`, `3d3c42e5aac5ba805825da76410c181273ba90b1`, `4cda84d5c5c54efe2404f9d843567869ab1699d4`, `e18b497796c12c097a38f9edb9d0641fb99eee32`, `55cc8345863c7cc4c66a329aec7e433d2d1c52a9`, `eba66cc6f87204a1e73f96e528e759b6c1fcf573`, `refusing: cargo xtask fuzz must run in a linked worktree`, `minimized-from-*`, `cargo audit --file fuzz/Cargo.lock`, `if: always()`, `inputs.time || 60`, `findings.md`, `exit 0`
  assert: every pattern has ≥1 hit
  command: `for p in "cache-targets: false" "CARGO_FUZZ_VERSION: \"0.13.2\"" "3d3c42e5aac5ba805825da76410c181273ba90b1" "4cda84d5c5c54efe2404f9d843567869ab1699d4" "e18b497796c12c097a38f9edb9d0641fb99eee32" "55cc8345863c7cc4c66a329aec7e433d2d1c52a9" "eba66cc6f87204a1e73f96e528e759b6c1fcf573" "refusing: cargo xtask fuzz must run in a linked worktree" "minimized-from-*" "cargo audit --file fuzz/Cargo.lock" "if: always()" "inputs.time || 60" "findings.md" "exit 0"; do grep -qF "$p" .github/workflows/fuzz-smoke.yml || { echo "MISSING: $p"; exit 1; }; done && echo STRUCTURAL-OK`
  expected: pass
- name: `no_floating_or_short_action_pins`
  setup: file exists
  action: grep every `uses:` line
  assert: every `uses:` pins a full 40-hex-char SHA (no floating `@v2`/`@v7` tags, no short SHAs)
  command: `! grep -E 'uses: [^ ]+@' .github/workflows/fuzz-smoke.yml | grep -vE '@[0-9a-f]{40}'`
  expected: pass (exit 0; the negated pipeline yields no output)
- name: `actionlint_if_present`
  setup: file exists
  action: if the `actionlint` binary is on PATH, run it on the workflow; otherwise report skipped
  assert: actionlint exits 0 when present; the test never masks an actionlint failure with the skip branch
  command: `if command -v actionlint >/dev/null; then actionlint .github/workflows/fuzz-smoke.yml; else echo "actionlint absent — skipped"; fi`
  expected: pass (or explicit skip)

Acceptance:
- All five local tests above pass from the worktree root
- After Task 1's commit, `git status --porcelain` shows exactly two entries: `?? .github/workflows/fuzz-smoke.yml` and `?? .github/workflows/fuzz-smoke-trigger-check.py` — no modifications to tracked files
- The workflow reads no secrets: `! grep -rE 'secrets\.' .github/workflows/fuzz-smoke.yml`
- Every functional step carries an explicit `id:` (grep `-c 'id: '` ≥ 14)

- [x] 2.1

### Task 3: verification record with three evidence slots

Files:
- `openspec/changes/fuzz-smoke-ci/verification.md` (new)

Steps:
1. Create `openspec/changes/fuzz-smoke-ci/verification.md` with sections:
   - `## Local verification (executed)` — table of the gates actually run in this worktree for this change: `cargo test -p xtask`, `cargo clippy -p xtask -- -D warnings`, `cargo fmt --check --all`, `cargo run --package xtask -- lint-unwrap`, plus the five Task 2 local tests, each with exit code recorded at the time of writing; the two pyyaml-dependent tests (yaml parse, trigger check) record HOW they ran (system python3 is a pip-less Nix build — they executed under a pinned pyyaml venv, `pyyaml 6.0.3`, placed first on PATH; nothing installed into the repo);
   - `## Evidence slot 1 — introducing PR (PENDING)` — the three drills (tmin, cold-main, refusal) close when the pushed PR's `fuzz-smoke` run is green; record the checklist items to observe (drill assertions passed, job duration in seconds from the Actions log for the < 6 min criterion);
   - `## Evidence slot 2 — post-merge 300 s dispatch (PENDING)` — `workflow_dispatch` with `time=300` after merge; record: dispatch ref (`main`), expected exit 0, where to note the run URL;
   - `## Evidence slot 3 — first real crash promotion (PENDING by design)` — promotion closes on the first real crash triage per decision §6.3; the synthetic drill panic is the bug, not a promotable input (superseded wording, per design.md D1);
   - `## rc-7rw2 closure checklist` — ordered: (1) PR green, (2) cost measured < 6 min steady-state, (3) 300 s dispatch green, (4) `bd close rc-7rw2`.
2. Verify the file parses and contains all five headings.

Tests:
- name: `verification_sections_present`
  setup: file exists
  action: grep for the five headings `## Local verification (executed)`, `## Evidence slot 1 — introducing PR (PENDING)`, `## Evidence slot 2 — post-merge 300 s dispatch (PENDING)`, `## Evidence slot 3 — first real crash promotion (PENDING by design)`, `## rc-7rw2 closure checklist`
  assert: each has exactly 1 hit
  command: `for h in "## Local verification" "## Evidence slot 1" "## Evidence slot 2" "## Evidence slot 3" "## rc-7rw2 closure"; do test "$(grep -c "$h" openspec/changes/fuzz-smoke-ci/verification.md)" -eq 1 || exit 1; done && echo HEADINGS-OK`
  expected: pass
- name: `no_false_closure_claims`
  setup: file exists
  action: grep for `passed` in the Evidence-slot sections
  assert: slots 1–3 say PENDING; the word `passed` appears only in the Local verification section or as explicit future-tense criteria
  command: `! grep -A6 '## Evidence slot' openspec/changes/fuzz-smoke-ci/verification.md | grep -E '^Status: .*(passed|complete)'`
  expected: pass

Acceptance:
- Both tests above pass
- `openspec validate fuzz-smoke-ci --type change --json` still reports `valid: true`

- [x] 3.1
