# Tasks: release-bridge-asset-verify

## Release scripts

### Task 1.1: Widen verify-release-assets.sh expected-set scan to all dist files

**Files:**
- `scripts/verify-release-assets.sh` (modified)

**Steps:**
1. In `verify_pass` (line ~93), change the expected-set scan from
   `files="$(find "$DIST_DIR" -maxdepth 1 -type f -name 'camel-*' | sort)"` to
   `files="$(find "$DIST_DIR" -maxdepth 1 -type f | sort)"` — drop the `-name 'camel-*'`
   filter so the expected set is every regular file in the dist directory. Change nothing
   else in the function (empty-dir pass-through comment stays valid).
2. Update the header comment (line ~13) wording from "requires every local `camel-*`
   file to be present with a remote size exactly equal to the local file size" to
   "requires every regular file in the dist directory to be present with a remote size
   exactly equal to the local file size". No other header/doc edits.
3. Do NOT touch the repair pass, WARN branch, retry windows, or usage contract
   (`<tag> <dist-dir>`).

**Tests:** (executable spec — existing battery must stay green; it proves core identity)
- `t_success`: existing fixture set (`camel-a.tar.gz` 100 B, `camel-a.tar.gz.sha256` 64 B, `camel-b.tar.gz` 200 B) with full listing → run battery → `t_success` reports PASS with exit 0 and no uploads (widened set equals old set for `camel-*`-only dist dirs).
- `t_missing_asset_reports_missing_files`: existing fixtures → run battery → PASS with exactly one `MISSING: camel-a.tar.gz` line (widening did not change core failure behavior).
- `command`: `bash scripts/tests/verify-release-assets-test.sh`
- `expected`: `8 passed, 0 failed` (battery unchanged at this point; widening is behavior-identical for the existing `camel-*` fixtures).

**Acceptance:**
- `bash scripts/tests/verify-release-assets-test.sh` prints `8 passed, 0 failed`.
- `grep -n "name 'camel-\*'" scripts/verify-release-assets.sh` exits 1 (filter gone).
- `shellcheck scripts/verify-release-assets.sh` exits 0 (if shellcheck installed; else record "N/A — shellcheck not installed").
- `git diff` on the file touches ONLY the `find` line and the header sentence.

- [x] 1.1

## Bridge release workflows

### Task 2.1: Add checkout + Verify release assets step to cxf/jms/xml bridge releases

**Files:**
- `.github/workflows/cxf-bridge-release.yml` (modified)
- `.github/workflows/jms-bridge-release.yml` (modified)
- `.github/workflows/xml-bridge-release.yml` (modified)

**Steps:**
1. In each file, inside the `release` job's `steps:` list (immediately before the
   existing `- uses: actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333 # v8.0.2`
   at line ~113), insert as the new first step:
   ```yaml
         - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v4
   ```
   followed by a blank line. This is the identical SHA pin already used at lines 16 and
   42 of the same files.
2. In each file, at the very end (immediately after the `Create GitHub Release` step's
   last body line, `**No JVM required — binaries are native images.**`), append:
   ```yaml

         - name: Verify release assets
           # bd rc-qd3ug: independent post-upload gate for the rc-6ep0i class of
           # silent asset loss (eventual-consistency 404 treated as success).
           env:
             GH_TOKEN: ${{ github.token }}
           shell: bash
           run: scripts/verify-release-assets.sh "$GITHUB_REF_NAME" artifacts
   ```
   (6-space indent for `- name:` inside the release job's steps, matching the
   `Create GitHub Release` step's indentation.)
3. Change NOTHING else: keep every existing step, pin, body line, and the job's
   `permissions: contents: write` (lines 110-111) byte-identical. No `if:` guards.

**Tests:** (executable structural spec — run from worktree root)
- `workflow_structure_ok`: for each of the three files, `python3` with `import yaml` loads the file, reads `jobs.release`, and asserts: `steps[0]["uses"].startswith("actions/checkout@")`; the step named `Verify release assets` immediately follows the step named `Create GitHub Release` in the steps list; the verify step's `env == {"GH_TOKEN": "${{ github.token }}"}`; its `shell == "bash"`; its `run == 'scripts/verify-release-assets.sh "$GITHUB_REF_NAME" artifacts'`; `jobs.release.permissions == {"contents": "write"}`; no step in the job gains an `if:` key that was not already there. Print `workflow structure OK` only when all three pass.
- `command`: the python3 assertion script per the spec above (worker writes it to `/tmp` or a heredoc; the assertion list is the contract). Any YAML parser is acceptable: if `python3 -c 'import yaml'` fails with `ModuleNotFoundError`, either `pip install pyyaml --user` first or use `node -e` with `js-yaml` — the assertion list, not the parser choice, is the contract.
- `expected`: `workflow structure OK`, exit 0.

**Acceptance:**
- The python3 structural check above exits 0 printing `workflow structure OK`.
- `git diff` on each of the three files shows ONLY: one inserted checkout line + blank line, and the appended verify step. `diff` counts: cxf +10 lines, jms +10, xml +10 (checkout 1 + blank 1 + verify block 8: leading blank + 7 step lines).
- No existing line removed or modified (`git diff --stat` shows 3 files, insertions only).

- [x] 2.1

## Verify-release-assets battery

### Task 3.1: Battery covers WARN branch and bridge-name regression (scenarios 9 and 10)

**Files:**
- `scripts/tests/verify-release-assets-test.sh` (modified)

**Steps:**
1. In `write_fixtures`, add a fourth fixture after `assets-wrong-size.json`:
   ```bash
     cat >"$FIXTURES/assets-bridge-partial.json" <<'JSON'
   {"assets":[
     {"name":"cxf-bridge-9.9.9.tar.gz","size":100},
     {"name":"cxf-bridge-9.9.9-SHA256SUMS","size":64}
   ]}
   JSON
   ```
2. Add fixture builder after `make_dist`:
   ```bash
   # Bridge-named dist set: exercises the prefix-agnostic expected-set scan.
   make_bridge_dist() {
     local dist="$1"
     mkdir -p "$dist"
     dd if=/dev/zero of="$dist/cxf-bridge-9.9.9.tar.gz" bs=100 count=1 status=none
     dd if=/dev/zero of="$dist/cxf-bridge-9.9.9.zip" bs=200 count=1 status=none
     dd if=/dev/zero of="$dist/cxf-bridge-9.9.9-SHA256SUMS" bs=64 count=1 status=none
   }
   ```
3. In the stub `gh` (inside `write_stub`'s heredoc): in the `release view` case
   statement, add a case before `*)`:
   ```bash
       repair_upload_failure_then_verify)
         if upload_log_has_a; then
           cat "$VERIFY_TEST_ASSETS_FULL"
         else
           cat "$VERIFY_TEST_ASSETS_MISSING"
         fi
         exit 0
         ;;
       bridge_missing)
         cat "$VERIFY_TEST_ASSETS_BRIDGE_PARTIAL"
         exit 0
         ;;
   ```
   In the `release upload` handler, replace the final `exit 0` with:
   ```bash
     if [[ "$scenario" == "repair_upload_failure_then_verify" ]]; then
       exit 1
     fi
     exit 0
   ```
   (the upload is still recorded in `$VERIFY_STUB_UPLOAD_LOG` and the `--clobber`
   check still runs before this).
4. Add the two test functions after `t_repair_then_listing_failure_no_stale_missing`:
   ```bash
   t_repair_upload_failure_warns_and_reverifies() {
     export VERIFY_TEST_SCENARIO=repair_upload_failure_then_verify
     run_verify 2 2
     assert_status 0 || return 1
     assert_stdout_line "Verified release assets for tag '$TAG' after repair" || return 1
     assert_stderr_line "WARN: repair upload failed; re-verifying from a fresh listing anyway" || return 1
     assert_no_missing_lines || return 1
     assert_uploads_exactly "camel-a.tar.gz" || return 1
     assert_view_count_gt 2 || return 1
   }

   t_bridge_named_missing_asset_fails() {
     rm -f "$WORK/dist"/camel-*
     make_bridge_dist "$WORK/dist"
     export VERIFY_TEST_SCENARIO=bridge_missing
     export VERIFY_TEST_ASSETS_BRIDGE_PARTIAL="$FIXTURES/assets-bridge-partial.json"
     run_verify 2 1
     assert_status 1 || return 1
     assert_single_missing_line "cxf-bridge-9.9.9.zip" || return 1
     assert_uploads_exactly "cxf-bridge-9.9.9.zip" || return 1
   }
   ```
5. Append both names to the `tests=(...)` array after
   `t_repair_then_listing_failure_no_stale_missing`. Keep the existing 8 entries and
   their order byte-identical. The runner's computed `printf '%d passed, %d failed'`
   needs no edit.
6. Do NOT modify any harness helper (`run_verify`, `assert_*`), the stub's existing
   cases, or the existing fixtures.

**Tests:** (executable spec)
- `t_repair_upload_failure_warns_and_reverifies`: stub listing serves missing-a until an upload is attempted then full; stub upload exits 1 after recording → script exhausts 2 initial attempts, issues failed repair upload, re-verifies from fresh full listing → assert exit 0, stdout `Verified release assets for tag 'vTEST' after repair`, stderr exact `WARN: repair upload failed; re-verifying from a fresh listing anyway`, no `MISSING:` lines, uploads exactly `camel-a.tar.gz`, view count > 2.
- `t_bridge_named_missing_asset_fails`: bridge-named dist set with listing omitting the `.zip` → script exhausts 2 initial attempts + 1 repair attempt → assert exit 1, exactly one `MISSING: cxf-bridge-9.9.9.zip` stderr line, uploads exactly `cxf-bridge-9.9.9.zip` (proves the scan is prefix-agnostic; a name-filtered scan would exit 0 vacuously).
- `command`: `bash scripts/tests/verify-release-assets-test.sh`
- `expected`: `10 passed, 0 failed`, exit 0, no network access.

**Acceptance:**
- `bash scripts/tests/verify-release-assets-test.sh` prints `10 passed, 0 failed` and exits 0.
- `shellcheck scripts/tests/verify-release-assets-test.sh` exits 0 (if shellcheck installed; else record "N/A — shellcheck not installed").
- `git diff` on the battery touches ONLY: the new fixture heredoc, `make_bridge_dist`, the two new stub cases, the upload-handler exit branch, the two new test functions, and the two array entries. Existing 8 scenario functions byte-identical.
- Runs offline (no network): the stub is the only `gh` on PATH during scenarios.

- [x] 3.1
