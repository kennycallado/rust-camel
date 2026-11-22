# Tasks: gates-workspace-suites

<!--
  Single-phase change. LANE_ROW below is the single source for BOTH AGENTS.md
  and the gates-lanes-test.sh fixture — keep it byte-identical everywhere.

  LANE_ROW (one physical line, no wrapping):
  camel-test | local | suites="${CAMEL_TEST_WORKSPACE_SUITES-dev_watch_script_test dev_watch_proto_test}"; allowed="dev_watch_script_test dev_watch_proto_test"; [[ -n "${suites//[[:space:]]/}" ]] || { echo "camel-test workspace lane: empty CAMEL_TEST_WORKSPACE_SUITES suite list" >&2; exit 2; }; set -f; for s in $suites; do case "$s" in *[\*\?\[]*) echo "camel-test workspace lane: CAMEL_TEST_WORKSPACE_SUITES must list literal suite names, got '$s'" >&2; exit 2;; esac; ok=0; for a in $allowed; do if [[ "$s" == "$a" ]]; then ok=1; break; fi; done; if [[ "$ok" != 1 ]]; then echo "camel-test workspace lane: CAMEL_TEST_WORKSPACE_SUITES entry '$s' is not an allowlisted local suite (allowed: $allowed)" >&2; exit 2; fi; done; rc=0; for s in $suites; do cargo test --workspace --test "$s" || rc=1; done; exit "$rc"

  LANE_ROW semantics: `${VAR-default}` (no colon) keeps an explicitly empty
  value empty; the `${suites//[[:space:]]/}` guard rejects empty AND
  whitespace-only lists (fail-closed, exit 2, zero cargo test dispatches);
  `set -f` disables pathname expansion so glob metachars always reach
  validation (unquoted `$suites` would otherwise expand to matching cwd
  filenames), and the case pattern rejects literal `*`, `?`, `[` entries;
  the `allowed` allowlist (in-row, initially the two reviewed local suites)
  rejects any override entry that is not on it — Docker-classified suites
  such as `redis_state_test` cannot be pulled into a local run (exit 2
  before any cargo test dispatch); `set -f` and the whole row run in their
  own `bash -c` process, so nothing leaks into the sweep's shell; the run
  loop aggregates failures in `rc` so one bad suite never masks another.
-->

## Gates tooling

### Task 1.1: Declare the workspace-unification lane row

**Files:**
- `AGENTS.md` (modified — `## COMPONENT LANES` table, after the existing
  `camel-test | local | cargo test -p camel-test --features integration-tests --no-run`
  row)
- `scripts/gates-local.sh` (modified — header comment only)

**Steps:**
1. In `AGENTS.md`, insert immediately after the line
   `camel-test | local | cargo test -p camel-test --features integration-tests --no-run`
   exactly these 3 rationale lines (mission item 4 keeps the block to 3):

   ```
   # --- camel-test workspace-unification suites (rc-4xka4): under `--workspace`, feature
   #     unification enables camel-core/lang-rhai (rc-lhd0y), a class invisible to `-p` lanes;
   #     default list in-row (rc-lhd0y's fix set), `CAMEL_TEST_WORKSPACE_SUITES` overrides; NOT full `cargo test --workspace` (Docker/bridges stay CI).
   ```

2. Directly after that comment block, add LANE_ROW verbatim from the header
   comment of this file (byte-identical, one physical line — copy the exact
   text, do not re-wrap). The existing row
   `camel-test | docker | cargo test -p camel-test --features integration-tests`
   stays after it; no other row changes.
3. In `scripts/gates-local.sh`, extend the header comment block (after the
   `GATES_LANES=all` usage line) with exactly these 3 lines:

   ```
   #   Lane rows may embed a default suite list overridable by an env var
   #   (camel-test's CAMEL_TEST_WORKSPACE_SUITES, rc-4xka4); the parser needs
   #   no change — it splits on the first two `|` and runs the rest verbatim.
   ```

4. Nothing else: no `crates/**` file, no CI workflow, no lane-parser or
   registry-parser code change. If a crate source edit seems needed, STOP
   and report `blocked: scope`.

**Tests:** (executable spec)
- `lane-table-parses` — setup: worktree with the new AGENTS.md row; action:
  `GATES_ONLY=lanes bash scripts/gates-local.sh` from the worktree root;
  assert: exit 0 and stdout contains `no-lane:       0` (this change touches
  no crate, so the row parses but executes nothing; a malformed row would
  exit 2 naming the row). Command:
  `cd /home/shared/rust-camel-worktrees/gates-workspace-suites && GATES_ONLY=lanes bash scripts/gates-local.sh`.
  Expected before the row exists: exit 0 too (parser sees the old table) —
  so also assert the row is really in the table (next test).
- `row-byte-shape` — setup: as above; action:
  `grep -c '^camel-test | local | suites=' AGENTS.md` (expect `1` — one row
  line) and
  `grep -o 'dev_watch_script_test dev_watch_proto_test' AGENTS.md | wc -l`
  (expect `2` — once in the row's default list, once in its `allowed`
  allowlist; `grep -c` would return `1` because both matches sit on the same
  line, so count occurrences, not lines); assert both counts and that
  `grep -q 'camel-test | docker |' AGENTS.md` still holds. Expected before
  implementation: first grep returns 0 → fail.

**Acceptance:**
- `GATES_ONLY=lanes bash scripts/gates-local.sh` exits 0 on the worktree.
- `git -C $WT diff --stat` names only `AGENTS.md` and `scripts/gates-local.sh`
  (plus the change dir); zero `crates/**` paths.
- The three rationale lines and LANE_ROW are adjacent; the docker row is
  untouched.
- [x] 1.1

### Task 1.2: Pin the lane contract in the parser regression suite

**Files:**
- `scripts/gates-lanes-test.sh` (modified — stub-cargo gets one guarded
  failure mode; new `case_49`; one new `CASES` entry)

**Steps:**
1. Extend the stub `cargo` in `install_stub_cargo`: replace its non-metadata
   tail — the exact two lines
   `printf '%s\n' "$*" >>"${STUB_ARGV_LOG:?stub cargo: STUB_ARGV_LOG is unset}"`
   and `exit 0` — with: that same printf line, then
   `if [[ -n "${STUB_TEST_FAIL_AT:-}" ]] && [[ "$(wc -l <"$STUB_ARGV_LOG")" -eq "$STUB_TEST_FAIL_AT" ]]; then exit 1; fi`
   before `exit 0`. Unset `STUB_TEST_FAIL_AT` keeps every existing case
   byte-identical in behavior.
2. Resolve the production declaration path at the top of the case section:
   `PROD_AGENTS="$(cd "$SCRIPT_DIR/.." && pwd)/AGENTS.md"` (the REAL repo
   AGENTS.md next to the suite), and extract its unique new-row command with
   the same anchored selector Task 1.3 uses:
   `prod_row="$(sed -n 's/^camel-test | local | suites=/suites=/p' "$PROD_AGENTS")"`.
   case_49 MUST exercise this production declaration, not a copy: the main
   sub-cases seed the fixture repo with `cp "$PROD_AGENTS" "$REPO/AGENTS.md"`
   (the full real table — registry phase is skipped under `GATES_ONLY=lanes`,
   Docker rows stay opt-in), so if the production row is deleted or moved
   outside the component-lane fence, the fixture sweep dispatches no
   `--workspace` calls and the case goes red. A fixture guard pins this:
   after the copy, `grep -c '^camel-test | local | suites=' "$REPO/AGENTS.md"`
   must be ≥ 1, else `FAIL_DETAIL="production lane declaration absent from the COMPONENT LANES fence"; return 1`.
   Expected argv stays LITERAL (independent of the row text).
3. Write `case_49` with nine sub-cases. Common fixture per sub-case — for
   example sub-case 49a uses `fixture_new 49a-default-list`, 49b uses
   `fixture_new 49b-allowlisted-override`, and so on through 49i — then:
   `fixture_init_repo`, seed AGENTS.md per sub-case below, `crate_seed crates/test camel-test`,
   `mkdir -p "$REPO/crates/test/tests"` +
   `printf '#[test]\nfn basic() {}\n' >"$REPO/crates/test/tests/basic.rs"`,
   `fixture_base_commit`, append `// touched` to
   `$REPO/crates/test/src/lib.rs`, `commit_work "touch camel-test"`,
   `fixture_meta 'camel-test:crates/test:lib,test'`. The copied real table
   makes the registry's compile-only row
   (`cargo test -p camel-test --features integration-tests --no-run`)
   dispatch alongside the new row — one extra logged cargo call, zero
   compilation (stub). Sub-cases:
   - **49a default-list exact dispatch**: case-local
     `unset CAMEL_TEST_WORKSPACE_SUITES` (NOT `run_sweep -u ...`: `env`
     receives the operands of `run_sweep` after its `PATH=`/`STUB_*`
     assignments and GNU env does not permute, so a trailing `-u` becomes
     the command name and exits 127); seed with the copied real AGENTS.md +
     guard; `run_sweep GATES_ONLY=lanes` → `assert_rc 0`;
     `assert_out_has 'PASS lane camel-test'`;
     `assert_out_lacks 'NO LANE for camel-test'`; `assert_argv_exact` with
     the three literal lines in table order
     `'test -p camel-test --features integration-tests --no-run'$'\n''test --workspace --test dev_watch_script_test'$'\n''test --workspace --test dev_watch_proto_test'`.
   - **49b allowlisted subset override**: `run_sweep GATES_ONLY=lanes CAMEL_TEST_WORKSPACE_SUITES='dev_watch_proto_test'`
     → `assert_rc 0`; `assert_argv_exact` with
     `'test -p camel-test --features integration-tests --no-run'$'\n''test --workspace --test dev_watch_proto_test'`
     (the override may subset the reviewed local set).
   - **49c empty list**: `run_sweep GATES_ONLY=lanes CAMEL_TEST_WORKSPACE_SUITES=`
     → `assert_rc_nonzero`; the new row must dispatch NO cargo test call
     (the compile-only row and the sweep's `cargo metadata` probe still run —
     `assert_argv_line 'test -p camel-test --features integration-tests --no-run'`,
     then a case-local check `grep -q -- '--workspace' "$ARGV"` → FAIL_DETAIL
     "empty override dispatched a workspace cargo call"); 
     `assert_out_has 'FAIL lane camel-test :: suites='`;
     `assert_err_has 'empty CAMEL_TEST_WORKSPACE_SUITES suite list'`.
   - **49d whitespace-only list**: `run_sweep GATES_ONLY=lanes 'CAMEL_TEST_WORKSPACE_SUITES=   '`
     → same assertions as 49c (blank list is an empty list).
   - **49e wildcard entry**: first create a glob-matching file in the fixture
     repo root — `printf 'x\n' >"$REPO/matching_test"` — so the assertion
     exercises the `set -f` guard (without it, the unquoted `$suites` would
     expand `*_test` to `matching_test` and validation would never see the
     metachars). Then `run_sweep GATES_ONLY=lanes 'CAMEL_TEST_WORKSPACE_SUITES=dev_watch_script_test *_test'`
     → same assertions as 49c but `assert_err_has 'literal suite names'`.
   - **49f first-suite failure still runs the second**: case-local
     `unset CAMEL_TEST_WORKSPACE_SUITES`, then
     `run_sweep GATES_ONLY=lanes STUB_TEST_FAIL_AT=2` (the compile-only row
     is cargo call 1; the FIRST workspace suite is call 2)
     → `assert_rc_nonzero`; `assert_argv_exact` with all three lines in
     table order (proves the second suite still ran);
     `assert_out_has 'FAIL lane camel-test :: suites='`.
   - **49g GATES_SKIP=lanes disables the row**: `run_sweep GATES_ONLY=lanes GATES_SKIP=lanes`
     → `assert_rc 0`; `assert_argv_empty`;
     `assert_out_has 'component lanes: DISABLED (GATES_SKIP=lanes)'`.
   - **49h malformed twin row**: fresh minimal fixture (`fixture_new 49h-malformed-twin`,
     `fixture_agents` with the twin derived from the production text so the
     bytes stay identical except the mode — require
     `[[ "$(printf '%s\n' "$prod_row" | wc -l)" -eq 1 && -n "$prod_row" ]]`
     first, then `fixture_agents "camel-test | locall | $prod_row"`)
     → `run_sweep GATES_ONLY=lanes` → `assert_rc 2`;
     `assert_err_names_row 'AGENTS.md'`; `assert_argv_empty`.
   - **49i Docker-classified override rejected**: `run_sweep GATES_ONLY=lanes 'CAMEL_TEST_WORKSPACE_SUITES=redis_state_test'`
     → `assert_rc_nonzero`; the row dispatches no cargo test call (same
     --no-run-present + no-`--workspace` argv checks as 49c);
     `assert_err_has "not an allowlisted local suite"`;
     `assert_err_has 'redis_state_test'`.
4. Append the `CASES` array entry (goes last, so driver index 49 maps to
   `case_49`):
   `"workspace-unification lane: production declaration, allowlist override, blank/wildcard guard, failure aggregation, skip, malformed row"`
5. Nothing else: no helper/assertion changes, no new fixture machinery
   beyond the one stub flag and the `PROD_AGENTS`/`prod_row` resolution.

**Tests:** (executable spec)
- `case_49` (the nine sub-cases above) is the executable test. Command:
  `bash scripts/gates-lanes-test.sh`. Red/green sequence: after Task 1.1
  lands the row, register the `CASES` entry with `case_49` still absent —
  the suite then reports `FAIL case 49` and `48/49 cases green` (the
  unchanged suite without the entry reports `48/48 cases green`; the red
  step is defined AFTER registering the new case). After implementation:
  `PASS case 49` and `49/49 cases green`, suite exit 0. All 48 previous
  cases must stay green (the stub change is inert while `STUB_TEST_FAIL_AT`
  is unset — case_08/09/17/21/23's exact argv assertions are the regression
  guard for that).

**Acceptance:**
- `bash scripts/gates-lanes-test.sh` exits 0 printing `49/49 cases green`.
- `git -C $WT diff --stat scripts/gates-lanes-test.sh` shows only the stub
  tail, the `PROD_AGENTS`/`prod_row` resolution, `case_49`, and one `CASES`
  line — no copied row literal anywhere in the suite.
- [x] 1.2

### Task 1.3: Prove the lane end to end in this worktree

**Files:** none (verification only; results recorded for the park report)

**Steps:**
1. Run the parser suite from the worktree root:
   `bash scripts/gates-lanes-test.sh` → record exit code and the
   `49/49 cases green` line.
2. Extract the lane command from the REAL AGENTS.md row — the selector MUST
   anchor on the new row's distinctive prefix, because after Task 1.1 the
   table holds TWO `camel-test | local |` rows and a loose selector would
   extract both commands (the `--no-run` row would then run cargo during the
   empty-guard proof). Use:
   `lane_cmd="$(sed -n 's/^camel-test | local | suites=/suites=/p' AGENTS.md)"`
   then require exactly one non-empty line:
   `[[ "$(printf '%s\n' "$lane_cmd" | wc -l)" -eq 1 && -n "$lane_cmd" ]] || exit 1`.
3. Run the extracted command exactly as the sweep would, override unset,
   from the worktree root:
   `env -u CAMEL_TEST_WORKSPACE_SUITES bash -o pipefail -c "$lane_cmd"`
   → record exit code (expected 0; both suites compile the dev profile under
   workspace unification — allow sccache time) and capture each suite's
   test-result line.
4. Run the empty guard: `env CAMEL_TEST_WORKSPACE_SUITES= bash -o pipefail -c "$lane_cmd"`
   → assert exit 2, stderr contains `empty CAMEL_TEST_WORKSPACE_SUITES suite
   list`, and no cargo output.
5. Record all exit codes for the STAGE 4 park report (the full registry
   sweep itself runs in STAGE 4; do not run it here).

**Tests:** (executable spec)
- `lane-direct-run` — setup: worktree at the plan-blessed commit, AGENTS.md
  row from Task 1.1; action: step 3's command; assert: exit 0 and both
  suites' summaries show `test result: ok`.
- `lane-empty-guard` — setup: `CAMEL_TEST_WORKSPACE_SUITES=`; action: step
  4's command; assert: exit 2, the stderr message, no `test result:` output.

**Acceptance:**
- Recorded exit codes: parser suite 0; direct run 0; empty guard 2.
- `git -C $WT status --short` shows no `crates/**` paths.
- [x] 1.3
