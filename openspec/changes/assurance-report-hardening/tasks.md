# Tasks: assurance-report-hardening

Single-phase change (design.md has no `## Phases` section). Tasks run in
order: 1 (body script) → 2 (workflow) → 3 (offline battery, exercises both).

## scripts (body builder)

### Task 1.1: Create scripts/assurance-report-body.sh — failure-tolerant issue-body builder

**Files:**
- `scripts/assurance-report-body.sh` (new)

**Steps:**
1. Create the script with `#!/usr/bin/env bash`, `# shellcheck shell=bash`, a
   header comment block (bd rc-g3nls + run 41999707610 story: `find` under
   `set -euo pipefail` died on a missing artifact dir before the infra guard
   or `gh issue create` ran; this script makes body construction offline,
   testable, and impossible to kill with missing evidence), usage line, then
   `set -euo pipefail`.
2. Inputs (env vars, all optional unless marked required):
   `ARTIFACTS_ROOT` (default `.`), `FUZZ_RESULT`/`BASE_RESULT`/`ROT_RESULT`
   (rendered verbatim; unset renders as `unknown`), `FUZZ_SECS` (default
   `900`), `OUT_BODY` (required), `OUT_ENV` (required).
3. Usage/real-error exits, each with an actionable stderr message:
   unset `OUT_BODY`/`OUT_ENV` → message `ERROR: OUT_BODY and OUT_ENV are required` → exit 2;
   `OUT_BODY`/`OUT_ENV` parent dir unwritable or target not writable →
   message naming the path → exit 3.
4. Evidence lookup — NEVER fatal. Resolve each artifact by filename from
   `$ARTIFACTS_ROOT/<artifact-dir>` (upload-artifact LCA lesson: never hardcode
   flat paths), each as `find "$dir" -name <file> -print -quit 2>/dev/null || true`:
   `art-mutants-baseline`/`outcomes.json` → `OUT_BASE`;
   `art-mutants-rotating`/`outcomes.json` → `OUT_ROT`;
   `art-fuzz-deep`/`fuzz-findings.txt` → `FUZZF`;
   `art-mutants-rotating`/`rotation-rationale.txt` → `RATIONALE`.
   A find on a nonexistent dir yields empty via the `|| true` — never aborts.
5. jq helper `jq_q <filter> <file>`: runs `jq -r ...`; if `jq` fails on a
   file that IS present, print `ERROR: <file> is not valid JSON — regenerate the artifact (real error, not missing evidence)` to stderr and exit 4.
   Used for all outcomes.json reads (counts, survivor names, markers).
6. Build the body file at `$OUT_BODY`, section order and byte-identical
   content preserved from the original workflow step (assurance-monthly.yml
   lines 376-439), with exactly these changes for missing evidence:
   - Header `# assurance-monthly: ${STAMP}` (`STAMP=$(date -u +%Y-%m)`),
     intro `Generated work order. bd stays the tracker — file follow-ups there.`
   - `## Tool jobs` table: rows `| fuzz-deep | ${FUZZ_RESULT} |`,
     `| mutants-baseline | ${BASE_RESULT} |`, `| mutants-rotating | ${ROT_RESULT} |`,
     then `failure here means infrastructure, not findings.`
   - `## Fuzz deep (target matrix, ${FUZZ_SECS}s each)` + fenced block:
     contents of `$FUZZF`, or the line `MISSING FUZZ FINDINGS (infra failure)`
     when FUZZF empty/not a file (replaces old `(no fuzz findings artifact — infra failure)`).
   - Baseline + rotating sections: when the outcomes file is missing/empty
     path/not a file → heading `## Mutants <name>: MISSING OUTCOMES (infra failure)`
     (exact existing wording); else heading `## Mutants <name>`, count line
     `tested=<n> caught=<n> missed=<n> unviable=<n>` via `jq_q`, fenced block
     of sorted unique missed-mutant `.name` lines via `jq_q` (`.name` form
     preserved — no null-line duplication).
   - `## Triage checklist (dedup is mandatory)` block: byte-identical text
     from the original (bd dedup list, fuzz-crash promotion rule, kill-rates
     informational). NOTE: the original contains literal backticks
     (`` `discovered-from:rc-eba8` ``) — emit via single-quoted printf/echo so
     bash does not command-substitute them.
   - `## Rotation` + fenced block: contents of `$RATIONALE`, or the line
     `MISSING ROTATION RATIONALE (infra failure)` (replaces old `(no rationale)`).
   - `Next window:` block ending with `Candidates: ...` — candidates read via
     `jq_q` from `$ARTIFACTS_ROOT/.github/assurance-monthly.json`; when that
     file is absent/unreadable, render `Candidates: (no .github/assurance-monthly.json — rotation config unreadable)` instead, never abort.
7. Append the machine-readable marker block to `$OUT_BODY` (byte-identical
   form): `<!--baseline-data-start-->`, then sorted unique missed-mutant
   `.name` lines from `$OUT_BASE` when present (nothing when absent), then
   `<!--baseline-data-end-->`.
8. Append env-file lines to `$OUT_ENV` (all five, always):
   `OUT_BASE=`, `OUT_ROT=`, `FUZZF=`, `STAMP=`,
   `INFRA_MISSING=` — `INFRA_MISSING` holds a space-separated list from
   {`baseline`,`rotating`,`fuzz`} for each missing piece, empty when all
   present. (No `RATIONALE` key: no downstream consumer — the rotation
   section is rendered entirely inside the body.)
9. `chmod +x` the script. Run `bash -n scripts/assurance-report-body.sh` (syntax) and
   `command -v shellcheck >/dev/null && shellcheck scripts/assurance-report-body.sh || true`
   (shellcheck advisory here; the battery in Task 3.1 is the hard gate).

**Tests:** (executable spec — direct shell invocations, fixture root built under mktemp)
- `t_zero_artifacts_exit_0`: empty `ARTIFACTS_ROOT` dir, FUZZ_RESULT=survives-from-tool-job etc. set → run script → exit code 0; body first line matches `^# assurance-monthly: [0-9]{4}-[0-9]{2}$`; body contains `| fuzz-deep |`, `| mutants-baseline |`, `| mutants-rotating |` rows; body contains all three lines `MISSING OUTCOMES (infra failure)` (×2), `MISSING FUZZ FINDINGS (infra failure)`, `MISSING ROTATION RATIONALE (infra failure)`; env file has `INFRA_MISSING=baseline rotating fuzz` and empty `OUT_BASE=`.
- `t_all_present_no_missing`: fixture root with both `outcomes.json` (valid outcomes JSON with exactly 4 outcomes: Missed×2 carrying `.scenario.Mutant.name`, Caught×1, Unviable×1 → tested=4 caught=1 missed=2 unviable=1), `fuzz-findings.txt`, `rotation-rationale.txt`, `.github/assurance-monthly.json` with `next_candidates` → exit 0; body has `tested=4 caught=1 missed=2 unviable=1`; no `MISSING` line anywhere; env file `INFRA_MISSING=` empty and paths populated.
- `t_baseline_missing_rotating_renders`: baseline absent; rotating fixture present (outcomes+rationale); fuzz findings present; config present → exit 0; body has `## Mutants baseline: MISSING OUTCOMES (infra failure)` AND a rendered `## Mutants rotating` with counts; `INFRA_MISSING=baseline` (exactly one key — fuzz/config deliberately present).
- `t_fuzz_missing`: baseline+rotating fixtures present, no fuzz → exit 0; body has `MISSING FUZZ FINDINGS (infra failure)`; `INFRA_MISSING=fuzz`.
- `t_malformed_present_outcomes_exit_4`: baseline `outcomes.json` containing `not-json{{` → exit code 4; stderr contains `is not valid JSON`.
- `t_usage_exit_2`: run with `OUT_BODY`/`OUT_ENV` unset → exit 2, stderr names both vars.
- command: `bash scripts/assurance-report-body.sh` per scenario (or via battery once Task 3.1 lands); expected: as listed, all failing before implementation exists (script absent → command fails).

**Acceptance:**
- All six scenario checks above pass when run manually against the script.
- `bash -n scripts/assurance-report-body.sh` exits 0.
- Script contains no `gh`/network invocation on an executable line
  (`grep -nE '\bgh\b|curl|wget' scripts/assurance-report-body.sh | grep -vE ':[[:space:]]*#'` → empty output; the header comment may mention `gh issue create` prose — comments are exempt).
- `git grep -c 'Cargo' -- scripts/assurance-report-body.sh || true` → prints 0 (no Rust, no dependency files touched; `|| true` absorbs grep's exit-1-on-zero).

- [x] 1.1

## workflow (report job)

### Task 2.1: Harden report job — script call, split publish step, INFRA_MISSING guard

**Files:**
- `.github/workflows/assurance-monthly.yml` (modified)

**Steps:**
1. Replace the "Build issue body" step (lines 357-477) with two steps:
   a. **Build issue body** (no `GH_TOKEN` on this step — the script must stay
      offline): `env:` keeps nothing beyond the job-level results;
      `run:` is exactly
      `OUT_ENV="$GITHUB_ENV" OUT_BODY="$RUNNER_TEMP/issue-body.md" bash scripts/assurance-report-body.sh`
      (`ARTIFACTS_ROOT` unset → script default `.` = repo root, where
      download-artifact placed the artifacts). Job-level
      `FUZZ_RESULT`/`BASE_RESULT`/`ROT_RESULT` env stays untouched.
   b. **Publish issue**: `env:` only `GH_TOKEN: ${{ github.token }}`.
      Shell begins with `set -euo pipefail` (carried from old line 361 —
      GitHub's default is only `bash -e`; without `-u` an unset `STAMP`
      renders a silent empty title suffix). Then the original prev-issue
      diff + `gh issue create` block (assurance-monthly.yml lines 454-477)
      preserved with exactly two changes:
      - The current-survivors pipeline gains `|| true` with comment
        `# Empty survivor set is valid input to the diff, not an error (empty`
        `# baseline marker block must not kill publication — bd rc-g3nls).`
        Applied to the `sed -n ... | grep -v '<!--' > "$RUNNER_TEMP/curr-survivors.txt"` pipeline.
      - Title/body unchanged: `--title "assurance-monthly: ${STAMP}"`,
        `--body-file "$RUNNER_TEMP/issue-body.md"` — `STAMP` now arrives via
        `$GITHUB_ENV` from the body step (subsequent-step semantics).
2. Add a comment block at the top of the `report:` job (above `needs:`)
   explaining: body construction extracted to scripts/assurance-report-body.sh
   for offline testability + the 41999707610 post-mortem (find died on missing
   artifact dir → step died → no issue created → durable alarm never fired);
   publication now survives zero artifacts; contract preserved (findings never
   fail; only missing evidence does; markers byte-identical for next-month diff).
3. On the "Download evidence" step, add the documented comment: the bare
   download-all form (no `name`/`pattern`/`artifact-ids`) is the documented
   zero-artifact-tolerant mode of actions/download-artifact@v8 — verified in
   action source at the pinned tag: empty artifact list logs
   `Total of 0 artifact(s) downloaded` and exits success. Do NOT add inputs.
4. Rewrite "Fail only on infra (missing evidence)" step: keep its name and
   anti-obstruction comment; replace the per-path checks with
   `if [ -n "${INFRA_MISSING:-}" ]; then echo "infra failure: missing evidence for: ${INFRA_MISSING} (see issue body)"; exit 1; fi`.
5. Do NOT touch jobs `fuzz-deep`, `mutants-baseline`, `mutants-rotating`, the
   `on:`/`permissions:`/`concurrency:`/workflow-level `env:` blocks, or the
   action pins. Minimal diff only.

**Tests:** (executable spec — verified in Task 3.1's battery; also direct here)
- `t_yaml_parses`: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/assurance-monthly.yml'))"` (pyyaml) → exit 0. If pyyaml unavailable: `python3` structural fallback — assert the file's report job contains steps named exactly `Build issue body`, `Publish issue`, `Fail only on infra (missing evidence)` (grep-based structural check; state which check ran).
- `t_markers_live_in_publish`: the new workflow still extracts the marker block for the diff — the "Publish issue" run block contains the `sed -n '/<!--baseline-data-start-->/,/<!--baseline-data-end-->/p'` patterns for both prev and current survivor files, and `scripts/assurance-report-body.sh` emits `<!--baseline-data-start-->`/`<!--baseline-data-end-->` (marker emission moved to the script, marker consumption stays in the publish step; diff the new files, not the old step's deleted lines).
- `t_tool_jobs_untouched`: `git diff "$(git merge-base main HEAD)" -- .github/workflows/assurance-monthly.yml | grep -E '^[+-]' | grep -cE 'fuzz-deep:|mutants-baseline:|mutants-rotating:' || true` → 0 (no job-definition lines added/removed; `|| true` keeps grep's exit-1-on-zero-matches from tripping `set -e`).
- command: `git diff "$(git merge-base main HEAD)" -- .github/workflows/assurance-monthly.yml` and the YAML parse check above; expected: parse clean, markers present in script + publish step, tool-job blocks untouched.

**Acceptance:**
- YAML parse check exits 0 (state which checker ran: pyyaml or structural).
- report job steps in order: checkout → Download evidence → Build issue body
  → Publish issue → Fail only on infra (missing evidence).
- `gh` invocations exist ONLY in the Publish issue step (grep).
- No change to any other job, no action version bump, no new dependency.

- [x] 2.1

## scripts/tests (offline battery)

### Task 3.1: Create scripts/tests/assurance-report-body-test.sh — offline scenario battery

**Files:**
- `scripts/tests/assurance-report-body-test.sh` (new)

**Steps:**
1. Write the battery in the `scripts/tests/verify-release-assets-test.sh`
   convention: `#!/usr/bin/env bash`, `# shellcheck shell=bash`, header
   comment (bd rc-g3nls, offline: no network, no real `gh` — a PATH stub
   serves `gh issue list/view/create` and records calls), `set -euo pipefail`,
   `SCRIPT_DIR`/`REPO_ROOT` resolution, `ROOT=$(mktemp -d)` + trap cleanup,
   fixture builders, `t_*` test functions, runner loop with PASS/FAIL
   counting and nonzero exit on failure.
2. Fixture builders — exact composition, so `INFRA_MISSING` assertions are
   deterministic: `make_outcomes <dir>` writes a valid outcomes.json
   (4 outcomes: Missed×2 with `.scenario.Mutant.name` values, Caught×1,
   Unviable×1); `make_fuzz <dir>` = `fuzz-findings.txt`;
   `make_rationale <dir>` = `rotation-rationale.txt`;
   `make_config <dir>` (.github/assurance-monthly.json with
   `next_candidates: ["a","b"]`).
   **Composition rule:** the *baseline* fixture is `make_outcomes` alone; the
   *rotating* fixture is `make_outcomes` + `make_rationale` (rationale is part
   of the rotating evidence — scenarios asserting `INFRA_MISSING=baseline` or
   `=fuzz` must build the full rotating fixture so only the intended key is
   missing); `t_rotation_missing` drops ONLY the rationale from an otherwise
   complete tree. Artifact dirs follow the LCA shape:
   `art-mutants-baseline/mutants.out/.../outcomes.json` (any subpath — script
   resolves by filename), rotating under `art-mutants-rotating/mut-wt/target-mutants/...`,
   fuzz under `art-fuzz-deep/`.
3. Script scenarios (each: build fixture root, set env, run
   `bash "$REPO_ROOT/scripts/assurance-report-body.sh"`, capture rc/body/env
   file, assert exactly):
   - `t_all_present`: exit 0; counts line `tested=4 caught=1 missed=2 unviable=1`; zero `MISSING` lines; **triage checklist verbatim** — header `## Triage checklist (dedup is mandatory)`, the `` `discovered-from:rc-eba8` `` line, and the `Kill rates are informational` line all present with their original text; markers contain the two sorted survivor `.name` lines between start/end markers; `INFRA_MISSING=` empty.
   - `t_baseline_missing`: baseline absent; rotating (outcomes+rationale) present; fuzz present; config present → exit 0; `## Mutants baseline: MISSING OUTCOMES (infra failure)` present; `## Mutants rotating` rendered with counts; `INFRA_MISSING=baseline` exactly.
   - `t_fuzz_missing`: exit 0; `MISSING FUZZ FINDINGS (infra failure)` present; `INFRA_MISSING=fuzz`.
   - `t_rotation_missing`: exit 0; `MISSING ROTATION RATIONALE (infra failure)` present; `INFRA_MISSING=rotating`.
   - `t_zero_artifacts`: empty root → exit 0; body starts `# assurance-monthly: `; contains all three job-result rows and all three MISSING lines; `INFRA_MISSING=baseline rotating fuzz`.
   - `t_markers_preserved`: all-present fixture → markers present with sorted `.name` lines.
   - `t_malformed_present_file`: invalid-JSON outcomes.json → exit 4; stderr message.
   - `t_unwritable_output`: `OUT_BODY=<dir-without-write-perm>/body.md` (chmod 0555 dir) → exit 3; actionable message. (skip when running as root; guard with `[[ $(id -u) -eq 0 ]] && skip`.)
   - `t_usage_errors`: unset OUT_ENV → exit 2.
4. Publish-path coverage: implement `extract_publish_step` — awk from the
   line matching `name: Publish issue` to the next `- name:` (or EOF) inside
   `.github/workflows/assurance-monthly.yml`, strip the run block's base
   indentation detected from its first non-empty line after `run: |`, write
   the extracted shell to a temp file. Stub `gh` on PATH (scenario-driven like
   the reference battery: `gh issue list --state all --search ... --json ...`
   returns a prev issue number for prev-exists scenarios, `[]`/empty for
   no-prev; `gh issue view <n> --json body --jq .body` returns a body
   containing a baseline-data marker block with one old survivor line;
   `gh issue create` records `--title` and `--body-file` to a log and exits 0;
   any other invocation → exit 64 + record to unexpected log).
   Publish scenarios export, as a subsequent step would see them from
   `$GITHUB_ENV`: `STAMP`, `OUT_BASE`, `OUT_ROT`, `FUZZF`, `INFRA_MISSING`
   **and `RUNNER_TEMP`** (the extracted shell writes
   `$RUNNER_TEMP/prev-survivors.txt` / `curr-survivors.txt`) — set
   `RUNNER_TEMP` to a fresh temp dir per scenario.
   Scenarios:
   - `t_publish_zero_artifacts_prev_exists`: body built by the real script
     from an empty fixture root; STAMP+env from the script's env file
     exported into the publish shell's environment (simulating
     subsequent-step GITHUB_ENV); stub prev exists → extracted shell exits 0;
     stub log shows `gh issue create` with
     `--title "assurance-monthly: <stamp>"`; body-file argument is the script
     body; survivor-diff section present (diff vs the stub's one survivor).
   - `t_publish_zero_survivors_prev_exists`: all-present fixture with a
     baseline whose outcomes contain zero MissedMutant entries → script body
     has empty marker set; stub prev exists → publish exits 0, create reached.
   - `t_publish_no_prev`: stub returns no previous issue → publish exits 0;
     create reached; no `## Baseline survivor diff` section in the
     created body (stub captures body-file content; assert section absent).
   - Assert stub unexpected-call log empty after every publish scenario.
5. Runner: register all `t_*` in order, run each in its own `$ROOT/$name`
   workdir, PASS/FAIL lines, final `N passed, M failed`, exit nonzero on any
   failure. `chmod +x` the battery.

**Tests:** (executable spec — the battery IS the test artifact)
- command: `bash scripts/tests/assurance-report-body-test.sh`
- expected: all scenarios PASS, `0 failed`, exit 0. Before Tasks 1.1/2.1 exist
  this battery cannot pass (script and publish step missing) — it is the
  final integration gate of the change.

**Acceptance:**
- `bash scripts/tests/assurance-report-body-test.sh` exits 0 with every
  scenario PASS (list captured in the task report).
- Battery makes no network calls (stub-only `gh`; grep: no `curl`/`wget`).
- Battery file passes `bash -n`.

- [x] 3.1
