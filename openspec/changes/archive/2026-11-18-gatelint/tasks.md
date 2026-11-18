# Tasks: gatelint

## CI workflow wiring

### Task 1.1: Add `lint-gate-parity` step to ci.yml quality job and fix the clippy-step comment

**Files:**
- `.github/workflows/ci.yml` (modified)

**Steps:**
1. In `.github/workflows/ci.yml`, in the `quality` job's `steps:` list,
   insert immediately after the `fmt check` step (currently the last line of
   that step is `run: cargo fmt --check --all`) and immediately before the
   comment block that starts with `# Source of truth: the clippy registry
   command in AGENTS.md ## QUALITY GATES`:
   ```yaml
         - name: lint-gate-parity
           run: cargo xtask lint-gate-parity
   ```
   (indentation: `      - name:` / `        run:` — six spaces before the
   dash, matching the sibling steps; no `if:` key, no `continue-on-error`
   key, no other key.)
2. Replace the existing three-line comment above the `clippy` step
   (the current comment block above the `clippy` step, lines that begin
   `# Source of truth:` and end `# fails the local sweep on drift.`) with:
   ```yaml
         # Source of truth: the clippy registry command in AGENTS.md ## QUALITY GATES
         # (bd rc-2s1vm). If you change the clippy gate there, change this step in the
         # same commit. The lint-gate-parity step above runs in CI and fails this
         # job on drift between the two.
   ```
3. Change nothing else in the file: not the clippy `run:` block, not the
   doc-build step (it is in `docs.yml` and is out of scope), not any other
   step, job, trigger, or permission.
4. Verify with the parsed-YAML assertion below (bun is installed locally,
   `Bun.YAML` is its built-in parser) and the parity lint clean run before
   handing back.
5. Do NOT commit and do NOT check off. Return to the conductor: the
   uncommitted patch (`git diff`), the assertion output+exit code, the lint
   output+exit code, and the diff-scope proof. After the reviewer approves,
   the CONDUCTOR checks off `- [x] 1.1` and makes the single checkpoint
   commit containing both the ci.yml wiring and the `tasks.md` checkbox
   update, records the sha, re-runs the committed-content evidence below,
   and only then dispatches Task 2.1.

**Tests:**
- `yaml_parse_step_assertions`: setup — working tree with steps 1-2 applied,
  committed not required; action — run
  `bun -e 'const t = await Bun.file(".github/workflows/ci.yml").text(); const d = Bun.YAML.parse(t); const s = d.jobs.quality.steps; const hits = s.map((x,i)=>[x,i]).filter(([x])=>x&&x.name==="lint-gate-parity"); if(hits.length!==1) throw new Error("expected exactly 1 lint-gate-parity step, got "+hits.length); const[,i]=hits[0]; const st=s[i]; if(st.run!=="cargo xtask lint-gate-parity") throw new Error("run mismatch: "+JSON.stringify(st.run)); if("if" in st||"continue-on-error" in st) throw new Error("suppression key present"); if(!s[i-1]||s[i-1].name!=="fmt check") throw new Error("prev step is not fmt check: "+JSON.stringify(s[i-1]&&s[i-1].name)); if(!s[i+1]||s[i+1].name!=="clippy") throw new Error("next step is not clippy: "+JSON.stringify(s[i+1]&&s[i+1].name)); console.log("yaml assertions OK")'
  from the repo root; assert — stdout is `yaml assertions OK` AND exit code
  is 0. Use the `Bun.file(...).text()` form exactly as written (no
  `require()`): the `require`-based form swallows throws and exits 0 on
  every failure path (verified on bun 1.3.13), making exit code a false
  green; this form exits non-zero with the error on each failure branch,
  on YAML parse errors, and when a misindent turns a step into a scalar
  (all three verified).
- `wiring_diff_scope`: setup — the edit applied, index UNCHANGED (worker
  must NOT `git add` — staged changes make plain `git diff` empty and hide
  the patch from the reviewer; staging is the conductor's job after
  approval); action — inspect the unstaged
  `git diff --name-only` and
  `git diff -- .github/workflows/ci.yml`; assert — the name-only list is
  exactly `.github/workflows/ci.yml`, and the file diff consists of:
  (a) added lines exactly the two-step block from step 1, (b) replaced
  lines exactly the comment block from step 2 — zero other
  added/removed/changed lines in the file.
- `parity_lint_clean`: setup — committed tree; action —
  `cargo xtask lint-gate-parity` from the repo root; assert — exit code 0
  and stdout contains `lint-gate-parity: OK (2 gates in parity)`.

**Acceptance:**
- The `bun` parsed-YAML assertion above prints `yaml assertions OK` AND
  exits 0 (both checked — exit 0 alone proves nothing, stdout must match).
- `cargo xtask lint-gate-parity` exits 0 printing
  `lint-gate-parity: OK (2 gates in parity)` on the tree under review.
- Worker returns the UNCOMMITTED patch and evidence (no commit, checkbox
  still unchecked — the reviewer must see the real diff).
- After the conductor's checkpoint commit (clean tree, HEAD = recorded
  sha): the assertion and the lint are re-run against the committed
  content with the same results.
- The checkpoint commit's path list
  (`git show --name-only --format= HEAD`) is exactly
  `.github/workflows/ci.yml` and `openspec/changes/gatelint/tasks.md`.
- The committed `ci.yml` diff against merge-base 97327cceb shows only the
  step insertion and the comment replacement
  (`git diff 97327cceb HEAD -- .github/workflows/ci.yml`).
- The new step has no `if:` and no `continue-on-error` key.

- [x] 1.1

### Task 2.1: Mutation demonstration — lint fails on workflow drift, tree restored byte-identically

**Files:**
- `.github/workflows/ci.yml` (modified temporarily; MUST be byte-identical
  to HEAD at task end — net file change: none)

**Steps:**
1. Handoff check (abort-gate): `git status --porcelain` MUST be empty and
   HEAD MUST be the Task 1.1 checkpoint commit containing the wiring. If
   either fails, STOP and report — do not edit.
2. Install the safety net BEFORE any edit:
   `BASE="$(git rev-parse HEAD)"`;
   `cleanup() { git restore --source="$BASE" -- .github/workflows/ci.yml; }`;
   `trap cleanup EXIT`. Pre-check the mutation target is unambiguous:
   `grep -c '^            --all-targets \\' .github/workflows/ci.yml` must
   print exactly `1` (the workspace clippy invocation's flag line; the
   `--all-targets` occurrences inside the `camel-component-kafka` /
   `camel-cli` single-crate invocations do NOT match this anchored pattern).
3. Mutate: delete that single line, e.g.
   `sed -i '\|^            --all-targets \\$|d' .github/workflows/ci.yml`.
4. Run the lint WITHOUT aborting on its expected failure (the drift
   assertion must not skip the restore):
   `set +e; MUT_OUT="$(cargo xtask lint-gate-parity 2>&1)"; MUT_RC=$?; set -e`.
   Assert exactly: `MUT_RC` equals `1` AND `MUT_OUT` contains the header
   `lint-gate-parity: MISMATCH gate 'clippy' (.github/workflows/ci.yml)`
   AND `MUT_OUT` shows both the expected and the actual clippy command
   blocks for that gate.
5. Restore on the success path: run `cleanup` and `trap - EXIT`. (The EXIT
   trap already guarantees the restore if an earlier step failed or the
   script was interrupted catchably.)
6. Prove byte-identical restoration three ways: `cmp .github/workflows/ci.yml <(git show "$BASE:.github/workflows/ci.yml")`
   (no output, exit 0 — raw byte comparison against the recorded commit's
   blob); `git diff HEAD --stat` prints nothing; `git status --porcelain`
   prints nothing.
7. Re-run `cargo xtask lint-gate-parity`; assert exit 0 and stdout contains
   `lint-gate-parity: OK (2 gates in parity)`.
8. Record all of the above verbatim (commands, MUT_RC, MUT_OUT, the three
   restoration proofs, the post-restore OK run) for the mission park, plus
   the explicit note that YAML parsing is not Actions schema validation and
   `cargo xtask schema --check` does not validate workflow YAML.

**Tests:**
- `mutation_detects_drift`: setup — clean tree at the Task 1.1 checkpoint,
  trap installed, target line removed by step 3; action —
  `cargo xtask lint-gate-parity` with the exit code captured non-fatally;
  assert — exit code is exactly `1`, output contains
  `lint-gate-parity: MISMATCH gate 'clippy' (.github/workflows/ci.yml)`
  and the expected/actual command blocks.
- `restore_byte_identical`: setup — post-mutation dirty tree (trap or
  explicit `cleanup` runs regardless of the drift assertion's outcome);
  action — restore from `$BASE`, then `cmp` against
  `git show "$BASE:.github/workflows/ci.yml"`, `git diff HEAD --stat`, and
  `git status --porcelain`; assert — cmp silent with exit 0 and both git
  outputs are empty (zero bytes).
- `clean_run_after_restore`: setup — restored tree; action —
  `cargo xtask lint-gate-parity`; assert — exit 0 and stdout contains
  `lint-gate-parity: OK (2 gates in parity)`.

**Acceptance:**
- The mutation run exited exactly `1` and reported the exact
  `MISMATCH gate 'clippy'` header with expected/actual command blocks.
- The restore ran even though the lint failed (guarded sequence with the
  EXIT trap installed before the edit).
- `cmp` against the checkpoint blob is byte-identical; `git diff HEAD
  --stat` and `git status --porcelain` are both empty.
- Post-restore lint run exits 0 with the OK line.
- The verbatim record (commands, exit codes, outputs, the three restoration
  proofs, the YAML/schema note) exists and is handed back for the park.

- [x] 2.1
