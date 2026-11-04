# Tasks: release-asset-verify

Single-phase change. bd rc-6ep0i. No Rust is touched: workers run `bash -n` and the stub battery instead of cargo fmt/clippy.

Common facts all tasks rely on (blessed design):
- Script: `scripts/verify-release-assets.sh <tag> <dist-dir>`; exits 0 only when every `camel-*` file in `<dist-dir>` is present on the GitHub release with remote size exactly equal to the local file.
- Env knobs: `VERIFY_MAX_ATTEMPTS` (default 6), `VERIFY_SLEEP_SECS` (default 10), `VERIFY_REPAIR_MAX_ATTEMPTS` (default 3), `VERIFY_REPAIR_SLEEP_SECS` (default 10).
- Stub-`gh` recording convention (shared by task 1.2's harness): the harness exports `VERIFY_STUB_UPLOAD_LOG=<path>`, `VERIFY_STUB_VIEW_COUNT=<path>`, and `VERIFY_STUB_UNEXPECTED_LOG=<path>`; the stub `gh` appends the basenames of every `release upload ... --clobber` file argument to the upload log, increments (one per line) the view-count file on every `release view` invocation, logs unhandled invocations to the unexpected log (and exits 64), and the harness exports `VERIFY_TEST_ASSETS_FULL`, `VERIFY_TEST_ASSETS_MISSING`, `VERIFY_TEST_SCENARIO`, `VERIFY_TEST_404_CALLS` to drive stub behavior.
- Output markers (exact strings): missing assets are reported as `MISSING: <basename>` (one line per file); unverifiable listings are reported as `ERROR: release asset set for tag '<tag>' could not be verified (asset listing failed)` and NEVER as `MISSING:` lines.
- Listing command: `gh release view "$TAG" --json assets` parsed with `jq -r '.assets[] | "\(.name)\t\(.size)"'` (tab-separated). A non-zero exit of either `gh` or `jq` is a listing failure (retry), not a missing asset.
- Repair command: `gh release upload "$TAG" --clobber "$DIST_DIR/<unverified-file>" ...` (full paths, exactly the unverified set, never verified assets).
- Expected set: `find "$DIST_DIR" -maxdepth 1 -type f -name 'camel-*'`; local size via `stat -c %s`.

## scripts

### Task 1.1: Implement scripts/verify-release-assets.sh (verify + retry + repair)

**Files:**
- `scripts/verify-release-assets.sh` (new)

**Steps:**
1. Create `scripts/verify-release-assets.sh`, executable (`chmod +x`), bash, `set -euo pipefail`, with `# shellcheck shell=bash` header comment and a top comment block citing bd rc-6ep0i (v0.65.0 run 41810273383, action-gh-release eventual-consistency 404).
2. Argument and dependency validation: exactly 2 args (`TAG`, `DIST_DIR`), else print usage to stderr and exit 2. `command -v gh` and `command -v jq` must both succeed, else print `ERROR: gh and jq are required` to stderr and exit 2. `DIST_DIR` must be an existing directory, else exit 2.
3. `list_assets()`: runs `gh release view "$TAG" --json assets 2>/dev/null | jq -r '.assets[] | "\(.name)\t\(.size)"'`; under `set +e` capture the pipeline status; returns 0 and prints tab-separated `name<TAB>size` lines on success, returns 1 on any failure. No output on failure.
4. `verify_pass()`: calls `list_assets`; on listing failure returns 2 (distinguishable from "verified"=0 and "unverified"=1). On listing success, builds the unverified list: for each `camel-*` file in `DIST_DIR` (name + `stat -c %s`), a file is verified iff the listing contains a row with exactly that name and exactly that size (string equality on the decimal size). Stores the unverified basenames in a global array. Returns 1 if the unverified list is non-empty, 0 otherwise.
5. Initial retry loop: for attempt in `1..VERIFY_MAX_ATTEMPTS`: run `verify_pass`; on 0 → `exit 0`; on 2 → listing failure (do NOT set the observable-listing flag); on 1 → set the observable-listing flag and record the unverified set. Sleep `VERIFY_SLEEP_SECS` after every attempt that did not verify.
6. After the loop: if the observable-listing flag was never set, print the exact `ERROR: release asset set for tag '<tag>' could not be verified (asset listing failed)` line to stderr and exit 1 (no `MISSING:` lines).
7. Repair pass (only if the unverified set is non-empty after the initial loop): run ONE repair upload — `gh release upload "$TAG" --clobber` with the full paths of exactly the current unverified set (`|| true` — capture failure and continue to verification); then ONE bounded post-repair verification loop of up to `VERIFY_REPAIR_MAX_ATTEMPTS` fresh listing tries with `VERIFY_REPAIR_SLEEP_SECS` sleeps between tries: if `verify_pass` returns 0 → `exit 0`; if every try hit listing failures (observable-listing flag never set within this post-repair loop), print the exact could-not-verify ERROR line to stderr and exit 1 (no `MISSING:` lines — never derive MISSING from stale pre-repair data); otherwise, if the last observable listing still shows unverified files, fall through to step 8. (There is exactly one repair upload, matching design.md D4; `VERIFY_REPAIR_MAX_ATTEMPTS` bounds the post-repair verification tries, not repeated uploads.)
8. After the repair pass: if the post-repair verification ended with an observable listing that still shows unverified files, print `MISSING: <basename>` for every still-unverified file to stderr and `exit 1`.
9. Run `bash -n scripts/verify-release-assets.sh` and `shellcheck scripts/verify-release-assets.sh` if shellcheck is installed (if shellcheck is absent, run `bash -n` only and note it).

**Tests:**
- `smoke_success_exit_zero`: setup — temp dir `t/` with two fake dist files `camel-a.tar.gz` (100 bytes) and `camel-a.tar.gz.sha256` (64 bytes); a stub `gh` at `t/bin/gh` (PATH prefixed) that serves a JSON asset list matching both names/sizes → action — `VERIFY_SLEEP_SECS=0 VERIFY_MAX_ATTEMPTS=2 bash scripts/verify-release-assets.sh vTEST t` → assert exit status 0.
- `smoke_persistent_listing_failure`: setup — same dist set; stub `gh` always exits 1 for `release view` → action — `VERIFY_SLEEP_SECS=0 VERIFY_MAX_ATTEMPTS=2 bash scripts/verify-release-assets.sh vTEST t` → assert exit status 1 and stderr contains the exact could-not-verify line and no `MISSING:` line.
- (command for both) `bash -n scripts/verify-release-assets.sh` exits 0.

**Acceptance:**
- `bash -n scripts/verify-release-assets.sh` exits 0.
- Both smoke tests pass when executed exactly as specified (exit 0 / exit 1 with the pinned diagnostic).
- Script contains all four env knobs with the documented defaults.
- No hardcoded asset filename list (expected set derived from `$DIST_DIR`).

- [x] 1.1

### Task 1.2: Offline stub battery scripts/tests/verify-release-assets-test.sh

**Files:**
- `scripts/tests/verify-release-assets-test.sh` (new)

**Steps:**
1. Create `scripts/tests/verify-release-assets-test.sh`, executable, bash, `set -euo pipefail`. Structure: `mktemp -d` workdir with trap cleanup; builds a fake dist set (`camel-a.tar.gz` 100 bytes, `camel-a.tar.gz.sha256` 64 bytes, `camel-b.tar.gz` 200 bytes); writes JSON fixtures `assets-full.json` (all three names with exact local sizes), `assets-missing-a.json` (b + a-sidecar present, a-tarball absent), `assets-wrong-size.json` (a-tarball present at size 99); generates a stub `gh` into `$WORK/bin/gh` (harness prefixes PATH) implementing:
   - `release view <tag> --json assets`: increments (appends one line to) the file named by `$VERIFY_STUB_VIEW_COUNT`, then behavior keyed on `$VERIFY_TEST_SCENARIO`: `success` → always cat `assets-full.json`; `404_then_success` → first `$VERIFY_TEST_404_CALLS` calls exit 1 with `HttpError: Not Found` on stderr, later calls cat `assets-full.json`; `persistent_404` → always exit 1 with `HttpError: Not Found`; `missing_a` → cat `assets-missing-a.json` (regardless of uploads); `repair_success` → if `$VERIFY_STUB_UPLOAD_LOG` does not yet contain `camel-a.tar.gz` cat `assets-missing-a.json` else cat `assets-full.json`; `wrong_size` → if upload log does not contain `camel-a.tar.gz` cat `assets-wrong-size.json` else cat `assets-full.json`; `repair_then_listing_failure` → if upload log does not contain `camel-a.tar.gz` cat `assets-missing-a.json` else exit 1 with `HttpError: Not Found`.
   - `release upload <tag> --clobber <files...>`: appends each file argument's basename to `$VERIFY_STUB_UPLOAD_LOG`, exits 0. Any other invocation exits 64 AND appends one descriptive line to `$VERIFY_STUB_UNEXPECTED_LOG` (path exported by the harness) — the runner asserts this log is empty at the end, which also proves no network access was attempted.
2. Harness helper `run_verify()` exports `VERIFY_STUB_UPLOAD_LOG="$WORK/uploads.log"`, `VERIFY_STUB_VIEW_COUNT="$WORK/views.count"`, and `VERIFY_STUB_UNEXPECTED_LOG="$WORK/unexpected.log"` (all truncated first), `VERIFY_SLEEP_SECS=0`, `VERIFY_REPAIR_SLEEP_SECS=0`, `VERIFY_MAX_ATTEMPTS`/`VERIFY_REPAIR_MAX_ATTEMPTS` as given, runs the script with tag `vTEST` and dist `$WORK/dist`, captures stdout/stderr and exit status.
3. Implement the eight scenario tests, each a bash function asserting on exit status, stderr content, and the upload log (compare `sort`ed basenames):
   - `t_success`: scenario `success`, `VERIFY_MAX_ATTEMPTS=3` → expect exit 0, empty upload log.
   - `t_transient_404_then_success`: scenario `404_then_success`, `VERIFY_TEST_404_CALLS=1`, `VERIFY_MAX_ATTEMPTS=3` → expect exit 0, empty upload log.
   - `t_persistent_404_unverifiable`: scenario `persistent_404`, `VERIFY_MAX_ATTEMPTS=2` → expect exit 1, stderr has the exact could-not-verify line, stderr has NO `MISSING:` line, empty upload log.
   - `t_missing_asset_reports_missing_files`: scenario `missing_a`, `VERIFY_MAX_ATTEMPTS=2`, `VERIFY_REPAIR_MAX_ATTEMPTS=1` → expect exit 1 and stderr contains exactly `MISSING: camel-a.tar.gz` (and no other MISSING line). Facet: output contract.
   - `t_failed_repair_uploads_unverified_set_and_reverifies`: scenario `missing_a`, same knobs (`VERIFY_MAX_ATTEMPTS=2`, `VERIFY_REPAIR_MAX_ATTEMPTS=1` so exactly one upload happens) → expect exit 1, upload log contains exactly `camel-a.tar.gz` (one line), and the view count (`wc -l` on `$VERIFY_STUB_VIEW_COUNT`) is greater than `VERIFY_MAX_ATTEMPTS` (2 initial views + ≥1 post-upload verification view proves verification re-ran after upload). Facet: repair + re-verify behavior. (Note: the two tests share one stub configuration intentionally — they assert different facets of the same flow; do not merge them.)
   - `t_successful_repair`: scenario `repair_success`, `VERIFY_MAX_ATTEMPTS=2`, `VERIFY_REPAIR_MAX_ATTEMPTS=2` → expect exit 0, upload log contains exactly `camel-a.tar.gz`.
   - `t_size_mismatch_triggers_repair`: scenario `wrong_size`, `VERIFY_MAX_ATTEMPTS=2`, `VERIFY_REPAIR_MAX_ATTEMPTS=2` → expect exit 0, upload log contains exactly `camel-a.tar.gz` (present-but-truncated asset repaired).
   - `t_repair_then_listing_failure_no_stale_missing`: scenario `repair_then_listing_failure`, `VERIFY_MAX_ATTEMPTS=2`, `VERIFY_REPAIR_MAX_ATTEMPTS=2` → expect exit 1, stderr has the exact could-not-verify line, stderr has NO `MISSING:` line, upload log contains exactly `camel-a.tar.gz`.
4. Runner: each test runs in a fresh subdirectory of `$WORK` with its own dist copy and fresh upload log; print `PASS: <name>` / `FAIL: <name>: <detail>` lines; at end print a summary `N passed, M failed`; exit non-zero if any test failed.
5. Run `bash scripts/tests/verify-release-assets-test.sh` — all eight must pass against the task 1.1 script. Also `bash -n` both scripts.

**Tests:**
- `verify-release-assets-test-battery`: setup — the completed task 1.1 script on disk → action — `bash scripts/tests/verify-release-assets-test.sh` → assert exit 0 and output contains `8 passed, 0 failed`.
- (command) `bash -n scripts/tests/verify-release-assets-test.sh` exits 0.

**Acceptance:**
- `bash scripts/tests/verify-release-assets-test.sh` exits 0 with all eight scenario tests passing.
- The battery performs no network access (stub `gh` only): the runner asserts `$VERIFY_STUB_UNEXPECTED_LOG` is empty after all scenarios (the stub logs and exits 64 on any unhandled invocation).
- No placeholders anywhere in the harness; every scenario asserts exact strings per the pinned contract.

- [x] 1.2

## workflows

### Task 1.3: Wire verification into release-matrix.yml release job

**Files:**
- `.github/workflows/release-matrix.yml` (modified)

**Steps:**
1. In the `release` job, immediately after the `Create release` step (the `softprops/action-gh-release` step, lines 439-446), insert this new step with EXACTLY these bytes (step key 6 spaces, fields 8 spaces, `GH_TOKEN` 10 spaces — matching sibling steps in the file):
   ```yaml
      - name: Verify release assets
        # bd rc-6ep0i: v0.65.0 (run 41810273383) shipped 21 of 22 assets
        # because action-gh-release treated an eventual-consistency 404 on
        # asset metadata as success ("Matching asset is present after
        # refresh; continuing...") and camel-full-aarch64-unknown-linux-gnu.tar.gz
        # never landed while its .sha256 sidecar did. This step independently
        # verifies the published asset set against dist/ (name + size
        # equality), retries transient listing 404s, re-uploads exactly the
        # unverified set with --clobber, and fails the job listing anything
        # still missing. Do NOT remove as redundant: the upload action's
        # "continuing" message is not proof of a complete release.
        env:
          GH_TOKEN: ${{ github.token }}
        shell: bash
        run: scripts/verify-release-assets.sh "$GITHUB_REF_NAME" dist
   ```
2. Do not modify any other step, the matrix, the artifact naming, or the flavor-suffix assert step.

**Tests:**
- `release-workflow-step-structure`: setup — the edited file → action — a small bash check using `grep -n` line numbers: the line `      - name: Verify release assets` appears AFTER the line `      - name: Create release`; `GH_TOKEN: ${{ github.token }}` appears within 13 lines after the `Verify release assets` line; `run: scripts/verify-release-assets.sh "$GITHUB_REF_NAME" dist` appears within 15 lines after it; `bd rc-6ep0i` appears between the `Verify release assets` name line and its `run:` line (inside the step's comment block); `run: scripts/verify-release-assets.sh` occurs exactly once in the file; and the `Verify release assets` line matches exactly 6 leading spaces (`grep -c '^      - name: Verify release assets'` = 1) → assert all succeed (implement as a bash check with line numbers, not manual inspection).
- (command) `bash -n scripts/verify-release-assets.sh` still exits 0 (unchanged from task 1.1).

**Acceptance:**
- The step sits between `Create release` and the end of the `release` job's `steps:` list.
- `GH_TOKEN` explicitly bound; no other auth changes.
- No changes to any other workflow element (verify with `git diff .github/workflows/release-matrix.yml` run in the worktree — only the insertion block, nothing else).
- Note: no local YAML parser exists in this environment (no PyYAML/ruby/yq/actionlint) — YAML well-formedness is structurally checked here and parse-verified when CI runs the workflow; record this in the task report.

- [x] 1.3

### Task 1.4: Run stub battery in ci.yml quality job

**Files:**
- `.github/workflows/ci.yml` (modified)

**Steps:**
1. In the `quality` job (ci.yml ~line 311, "Quality Gates"), immediately after the `clippy` step block ends (the step whose `run:` starts `cargo clippy --workspace --all-features` and its continuation lines), insert this new step with EXACTLY these bytes (6-space key, 8-space fields):
   ```yaml
      - name: verify-release-assets stub tests
        # bd rc-6ep0i: offline battery for the release asset verification
        # script (stubbed gh, no network). Keeps the script from rotting
        # between releases.
        run: bash scripts/tests/verify-release-assets-test.sh
   ```
2. Do not modify any other step or job.
3. Run the full battery locally to confirm the step's command is exactly what passes.

**Tests:**
- `ci-quality-job-has-battery-step`: setup — the edited file → action — awk range from the line matching `^  quality:$` to the next `^  [a-z-]+:$` job heading; within that range assert: line `      - name: verify-release-assets stub tests` exists; line `        run: bash scripts/tests/verify-release-assets-test.sh` exists; `bd rc-6ep0i` appears; the step appears after the `clippy` step name line → assert all succeed (implement as a bash check function, not manual inspection).
- (command) `bash scripts/tests/verify-release-assets-test.sh` exits 0.

**Acceptance:**
- The battery step is inside the `quality` job, after clippy, before the `bench-smoke` job.
- Only one insertion; `git diff .github/workflows/ci.yml` run in the worktree shows the added block and nothing else.
- Battery passes locally as run by the new CI step.

- [x] 1.4
