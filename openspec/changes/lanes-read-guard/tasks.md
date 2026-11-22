# Tasks: lanes-read-guard

<!-- Single-phase. Task 1.1 writes the failing case; Task 1.2 makes it pass. -->

## Gates tooling

### Task 1.1: Harness case for the lanes read/decode guard

**Files:**
- `scripts/gates-lanes-test.sh` (modified — new `case_50`, one new entry
  appended to `CASES`)

**Steps:**
1. Append `case_50` after `case_49` (before the `# ── driver` banner). Build it
   with the existing helpers; follow case 19's shape (`fixture_new`,
   `fixture_init_repo`, `crate_seed`, `fixture_base_commit`, `commit_work`,
   `fixture_meta`, `run_sweep`):
   - `fixture_new 50-lanes-read-guard`, `fixture_init_repo`,
     `crate_seed crates/ok camel-ok`.
   - Write `$REPO/AGENTS.md` by hand (do NOT use `fixture_agents_preamble`,
     its registry gate is `noop`). Content: the `# gates-lanes-test fixture`
     header, a `## QUALITY GATES` fence with ONE gate:
     `- name: corrupt` / `  run: printf '\377\376' >> AGENTS.md` (octal escapes,
     so the invalid UTF-8 bytes are produced at gate run time and the committed
     file stays clean valid UTF-8), then a `## COMPONENT LANES` fence with the
     single row `camel-ok | local | cargo test -p camel-ok`.
   - `fixture_base_commit`; append a comment line to `crates/ok/src/lib.rs`;
     `commit_work "touch ok"`; `fixture_meta 'camel-ok:crates/ok:lib'`.
   - `run_sweep "GATES_ONLY=corrupt lanes"`. NOTE: `run_sweep` passes its
     arguments as `env` assignments, so pass the single word
     `GATES_ONLY=corrupt lanes` as ONE argument (quoted).
   - Resolve `repo_root` with `git -C "$REPO" rev-parse --show-toplevel`;
     fail the case if that command fails.
   - Assert `assert_rc 2`, then
     `assert_err_has "gates-local: lanes: cannot read $repo_root/AGENTS.md: 'utf-8' codec can't decode"`,
     then `assert_err_lacks 'Traceback'` and `assert_argv_empty`. Return on the
     first failed assertion.
2. Append to `CASES` the name
   `"lanes: unreadable lane table is a named exit 2 (read/decode guard)"`
   as the new last entry (index 50).
3. Run `bash scripts/gates-lanes-test.sh` BEFORE Task 1.2. Expect exactly case
   50 to FAIL, on the missing `gates-local: lanes: cannot read` text (the
   pre-fix run has a traceback and exit 2). Record that failure.

**Tests:** (executable spec — the case itself)
- `case_50`: Run `bash scripts/gates-lanes-test.sh` with the fixture in Step 1.
  Assert exit 2, the full `AGENTS.md` path and UTF-8 decode error in the same
  named stderr line, no `Traceback`, and an empty cargo argv log. Before
  Task 1.2, expect the named-error assertion to fail.

**Acceptance:**
- Case 50 exists, is the last entry in `CASES`, and fails on unmodified
  `scripts/gates-local.sh` for the named-error assertion, not for a fixture
  error.
- `bash -n scripts/gates-lanes-test.sh` clean.

- [x] 1.1

### Task 1.2: Add the lanes read/decode guard

**Files:**
- `scripts/gates-local.sh` (modified — `lanes_grammar` heredoc only)

**Steps:**
1. In `lanes_grammar`, replace the line `lines = physical_lines(path)` with the
   try/except block shown in `design.md ## Approach` (label `lanes`, prints
   `path`, same exception tuple as the registry guard, `sys.exit(2)`).
2. Change nothing else in the heredoc or in the registry grammar.
3. Run `bash scripts/gates-lanes-test.sh`; case 50 and all 49 others pass.

**Tests:**
- Re-run `case_50` (Task 1.1): exit 2, named `gates-local: lanes: cannot read`
  line, no `Traceback`, empty argv log.
- Existing cases 1-49 unchanged and green (regression: valid tables parse
  exactly as before).

**Acceptance:**
- `bash scripts/gates-lanes-test.sh` prints `50/50 cases green`.
- `git diff --stat` for the task shows only `scripts/gates-local.sh`,
  6 added lines and 1 removed.
- `bash -n scripts/gates-local.sh` clean.
- `full_local_sweep`: From the change worktree root, run
  `env -u GATES_ONLY -u GATES_SKIP -u GATES_LANES bash scripts/gates-local.sh`
  and require exit 0.

- [x] 1.2
