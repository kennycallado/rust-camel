# Tasks: canonical-path-fuzzing

## fuzz crate (camel-fuzz)

### Task 1.1: three harness functions, bin targets, and parity dependencies

**Files:**
- `fuzz/src/lib.rs` (modified)
- `fuzz/Cargo.toml` (modified)
- `fuzz/fuzz_targets/dsl_json.rs` (new)
- `fuzz/fuzz_targets/dsl_template.rs` (new)
- `fuzz/fuzz_targets/dsl_parity.rs` (new)

**Steps:**
1. In `fuzz/Cargo.toml`, add plain (non-workspace) `serde_json` and `noyalib`
   entries copied verbatim from the root `Cargo.toml`
   `[workspace.dependencies]` (version and features identical — the fuzz
   crate is excluded from the workspace and cannot inherit). Add three
   `[[bin]]` entries (`dsl_json`, `dsl_template`, `dsl_parity`) mirroring
   the existing `dsl_yaml` entry shape (`test = false`, `doc = false`).
2. In `fuzz/src/lib.rs`, add `dsl_json_harness(data: &[u8])`: if
   `std::str::from_utf8(data)` is `Ok(s)`, call
   `camel_dsl::json::parse_json_with_threshold_and_security(s,
   camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
   SecurityCompileContext::default())` and discard the result. Mirror the
   doc-comment style of `dsl_yaml_harness`.
3. Add `dsl_template_harness(data: &[u8])`: on valid UTF-8 `s`, call
   `camel_dsl::template::json::parse_json_templates(s)` and
   `camel_dsl::template::json::parse_json_templated_routes(s)`. If BOTH
   return `Ok`, for each `TemplatedRouteSpec` whose `route_template_ref`
   equals a parsed template's `RouteTemplateSpec.id`, call
   `camel_dsl::materialize_and_compile(&template, &instance,
   camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
   SecurityCompileContext::default())` and discard the result. Instances
   with no matching template id are skipped (ordinary rejection).
4. Add `assert_step_layer_parity(json_routes: &RouteDslRoutes,
   yaml_routes: &RouteDslRoutes)` (crate-private): panic with a message
   starting `parity divergence:` when (a) `routes.len()` differs, (b) any
   route pair differs in `id` or `from`, or (c) the
   `format!("{:#?}", steps_vec)` renderings of the flattened
   `Vec<&RouteDslStep>` (built as `routes.iter().flat_map(|r|
   r.steps.iter()).collect::<Vec<_>>()`, the parity_tests.rs mechanism)
   differ. `RouteDslRoutes` is imported from
   `camel_dsl::route_ast::RouteDslRoutes`.
5. Add `dsl_parity_harness(data: &[u8])`: on valid UTF-8 `s`, first call
   `camel_dsl::yaml::parse_yaml_with_threshold_and_security(s, camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD, SecurityCompileContext::default())`
   and
   `camel_dsl::json::parse_json_with_threshold_and_security(s, camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD, SecurityCompileContext::default())`,
   discarding both results (panic coverage). Then deserialize with
   `serde_json::from_str::<RouteDslRoutes>(s)`; on `Err` return (outside
   the overlap). On `Ok(jr)` call a crate-private helper `expect_yaml_overlap(s, &jr)`:
   deserialize `s` with the YAML serde front-end
   (`noyalib::compat::serde_yaml::from_str::<RouteDslRoutes>(s)`, the
   same alias camel-dsl uses) and, via a nested crate-private fn
   `panic_if_yaml_rejects(result)`, panic `parity divergence: yaml
   rejects json-valid document` on `Err`; on `Ok(yr)` call
   `assert_step_layer_parity(jr, &yr)`.
6. Create the three fuzz targets, each exactly:
   `libfuzzer_sys::fuzz_target!(|data: &[u8]| { camel_fuzz::<fn>(data); });`
   with `<fn>` the matching harness (same shape as `dsl_yaml.rs`).
7. Add a `#[cfg(test)] mod tests` to `fuzz/src/lib.rs` with the tests
   listed below. Test fixtures are inline string literals (minimal
   single-route documents with `from: "timer:tick"` and one `to` step;
   a minimal template document with one template whose `id` is
   referenced by one `templated_routes` entry using a `{{param}}`
   placeholder and a parameters map supplying it).

**Tests:** (run with `nix develop . -c bash -c 'cd fuzz && cargo test --lib'`)
- `dsl_json_harness_valid_minimal_returns`: a minimal valid JSON route document → `dsl_json_harness` returns without panicking.
- `dsl_json_harness_invalid_utf8_returns`: input `b"\xff\xfe\xfd"` → returns without calling the parser.
- `dsl_template_harness_valid_templated_returns`: JSON doc with one template + one matching instance (placeholder substituted by the instance parameters) → returns without panicking.
- `dsl_template_harness_missing_ref_returns`: instance whose `route_template_ref` matches no template id → returns without panicking (skipped instance).
- `dsl_template_harness_invalid_utf8_returns`: `b"\xff"` → returns early.
- `dsl_parity_harness_valid_both_returns`: a JSON document that is also valid YAML → returns without panicking.
- `dsl_parity_harness_json_only_syntax_skips`: YAML-only syntax (e.g. `routes: []` plain scalar form) that `serde_json` rejects → returns without panicking regardless of the YAML outcome.
- `dsl_parity_harness_invalid_utf8_returns`: `b"\xff"` → returns early.
- `assert_step_layer_parity_equal_returns`: two hand-built identical `RouteDslRoutes` → returns.
- `assert_step_layer_parity_count_divergence_panics`: json has 1 route, yaml 2 → `#[should_panic(expected = "parity divergence")]`.
- `assert_step_layer_parity_id_divergence_panics`: same count, differing route `id` → `#[should_panic(expected = "parity divergence")]`.
- `assert_step_layer_parity_steps_divergence_panics`: same ids/from, differing step lists → `#[should_panic(expected = "parity divergence")]`.
- `panic_if_yaml_rejects_err_panics`: call `panic_if_yaml_rejects` with an `Err` obtained from `serde_yaml::from_str::<RouteDslRoutes>("{")` → `#[should_panic(expected = "parity divergence: yaml rejects")]`.
- `panic_if_yaml_rejects_ok_returns`: call with the `Ok` from deserializing a minimal valid document → returns.
- `assert_step_layer_parity_from_divergence_panics`: same route count and ids, differing `from` URIs → `#[should_panic(expected = "parity divergence")]`.

**Acceptance:**
- `nix develop . -c bash -c 'cd fuzz && cargo test --lib'` passes all 15 tests.
- `nix develop . -c bash -c 'cd fuzz && cargo clippy --all-targets -- -D warnings'` exits 0.
- `nix develop . -c bash -c 'cd fuzz && cargo +nightly fuzz build dsl_json'`,
  `nix develop . -c bash -c 'cd fuzz && cargo +nightly fuzz build dsl_template'`, and
  `nix develop . -c bash -c 'cd fuzz && cargo +nightly fuzz build dsl_parity'`
  each exit 0 (cargo-fuzz build takes exactly one target per invocation).

- [x] 1.1

### Task 1.2: seed corpora for the three new targets

**Files:**
- `fuzz/seeds/dsl_json/valid_minimal.json` (new)
- `fuzz/seeds/dsl_json/valid_steps.json` (new)
- `fuzz/seeds/dsl_json/valid_deep_nesting.json` (new)
- `fuzz/seeds/dsl_json/malformed_truncated.json` (new)
- `fuzz/seeds/dsl_template/valid_templated.json` (new)
- `fuzz/seeds/dsl_template/placeholder_heavy.json` (new)
- `fuzz/seeds/dsl_template/malformed_template.json` (new)
- `fuzz/seeds/dsl_parity/valid_minimal.json` (new)
- `fuzz/seeds/dsl_parity/valid_steps.json` (new)
- `fuzz/seeds/dsl_parity/malformed_both.json` (new)
- `fuzz/src/lib.rs` (modified — seed contract tests appended to the existing `#[cfg(test)]` module)

**Steps:**
1. Author `dsl_json` seeds from the JSON arms of `parity_cases()` in
   `crates/camel-dsl/src/parity_tests.rs` and camel-dsl JSON test
   fixtures: `valid_minimal.json` (single route, one step),
   `valid_steps.json` (two different step kinds), `valid_deep_nesting.json`
   (nested `choice`/`otherwise` beyond 8 levels, still parseable), and
   `malformed_truncated.json` (a valid document cut mid-object, e.g.
   ending `{"routes": [{`). Do not convert the YAML seeds.
2. Author `dsl_template` seeds from template parser/materializer test
   fixtures in `crates/camel-dsl/src/template/`: `valid_templated.json`
   (one template + one matching instance with one placeholder),
   `placeholder_heavy.json` (template body using three distinct
   placeholders), `malformed_template.json` (truncated templates
   section).
3. Author `dsl_parity` seeds: `valid_minimal.json` and
   `valid_steps.json` (documents BOTH serde front-ends accept — reuse
   the dsl_json valid seeds), `malformed_both.json` (e.g.
   `{"routes": [}` — rejected by both front-ends).
4. Append seed-contract tests to `fuzz/src/lib.rs` (paths resolved from
   `env!("CARGO_MANIFEST_DIR")/seeds/<target>/<file>`).

**Tests:** (run with `nix develop . -c bash -c 'cd fuzz && cargo test --lib'`)
- `seeds_dsl_json_contract`: every `fuzz/seeds/dsl_json/valid_*.json` deserializes via `serde_json::from_str::<RouteDslRoutes>` to `Ok`; `malformed_truncated.json` returns `Err`.
- `seeds_dsl_parity_contract`: every `fuzz/seeds/dsl_parity/valid_*.json` deserializes `Ok` via BOTH `serde_json` and the YAML serde front-end; `malformed_both.json` returns `Err` from both.
- `seeds_dsl_template_contract`: `valid_templated.json` and `placeholder_heavy.json` yield `Ok` from BOTH `parse_json_templates` and `parse_json_templated_routes` with at least one instance matching a template id; `malformed_template.json` yields `Err` from at least one of the two.

**Acceptance:**
- `nix develop . -c bash -c 'cd fuzz && cargo test --lib'` passes (Task 1.1 tests, 17 after the review round, + the 3 new seed-contract tests = 20).
- `find fuzz/seeds -type f | wc -l` = 16 (6 pre-existing yaml + 10 new).

- [x] 1.2

## xtask

### Task 2.1: extend KNOWN_TARGETS to the four fuzz targets

**Files:**
- `scripts/xtask/src/fuzz.rs` (modified)

**Steps:**
1. Change `const KNOWN_TARGETS: &[&str] = &["dsl_yaml"];` to
   `&["dsl_yaml", "dsl_json", "dsl_template", "dsl_parity"];`.
2. Add a unit test to the existing `#[cfg(test)]` module asserting all
   four names are present and the const has no duplicates.

**Tests:** (run with `cargo test -p xtask --lib` from the worktree root)
- `known_targets_cover_all_four`: assert `KNOWN_TARGETS` contains `dsl_yaml`, `dsl_json`, `dsl_template`, `dsl_parity` → passes (compile-time data, but locks the contract).
- `known_targets_seeds_dirs_exist`: for each name in `KNOWN_TARGETS`, assert `fuzz/seeds/<name>/` exists under the repo root → passes (couples 2.1 to 1.2's committed seeds).

**Acceptance:**
- `cargo test -p xtask` passes including the two new tests (xtask is bin-only; no --lib target).
- `cargo clippy -p xtask -- -D warnings` exits 0.

- [x] 2.1

## CI leg selection

### Task 3.1: fuzz-legs.sh selector script with self-test

**Files:**
- `scripts/fuzz-legs.sh` (new, executable)

**Steps:**
1. Write `scripts/fuzz-legs.sh` (bash, `set -euo pipefail`): reads
   changed file paths one per line on stdin; prints the selected
   targets space-separated on one line, deduplicated, in canonical order
   `dsl_yaml dsl_json dsl_template dsl_parity` restricted to the
   selection. Selection rules (ordered, union-combined): a path under
   `fuzz/seeds/<target>/**` selects that target;
   `crates/camel-dsl/src/yaml.rs` selects `dsl_yaml dsl_parity`;
   `crates/camel-dsl/src/json.rs` selects `dsl_json dsl_parity`; a path
   under `crates/camel-dsl/src/template/**` selects `dsl_template
   dsl_parity`; any path matching the workflow trigger but NOT the
   rules above (other `crates/camel-dsl/**`, other `fuzz/**`,
   `scripts/xtask/**`, `.github/workflows/fuzz-smoke.yml`) selects all
   four targets; paths outside the trigger sets are ignored. `--dispatch`
   ignores stdin and prints all four. `--self-test` runs the test cases
   below and exits 0 on full pass, 1 on any mismatch (no other output on
   stdout besides the selected targets in normal mode).
2. `chmod +x scripts/fuzz-legs.sh`.

**Tests:** (run with `./scripts/fuzz-legs.sh --self-test`)
- `wrapper-change-selects-all`: stdin `scripts/xtask/src/fuzz.rs` → stdout exactly `dsl_yaml dsl_json dsl_template dsl_parity`.
- `json-frontend-selects-json-and-parity`: stdin `crates/camel-dsl/src/json.rs` → exactly `dsl_json dsl_parity`.
- `yaml-frontend-selects-yaml-and-parity`: stdin `crates/camel-dsl/src/yaml.rs` → exactly `dsl_yaml dsl_parity`.
- `template-selects-template-and-parity`: stdin `crates/camel-dsl/src/template/materializer.rs` → exactly `dsl_template dsl_parity`.
- `seed-dir-selects-own-target`: stdin `fuzz/seeds/dsl_json/valid_minimal.json` → exactly `dsl_json`.
- `shared-downstream-selects-all`: stdin `crates/camel-dsl/src/compile.rs` → all four.
- `dispatch-selects-all`: `--dispatch` with empty stdin → all four.
- `mixed-changes-union`: stdin two lines (`crates/camel-dsl/src/yaml.rs`, `fuzz/seeds/dsl_json/valid_minimal.json`) → exactly `dsl_yaml dsl_json dsl_parity`.
- `non-trigger-path-ignored`: stdin two lines (`crates/camel-dsl/src/json.rs`, `README.md`) → exactly `dsl_json dsl_parity`.

**Acceptance:**
- `./scripts/fuzz-legs.sh --self-test` exits 0 (all 9 cases).
- `bash -n scripts/fuzz-legs.sh` exits 0.

- [x] 3.1

### Task 3.2: workflow serial legs, per-leg checks, dsl_json drill, per-leg summary, conditional timeout

**Files:**
- `.github/workflows/fuzz-smoke.yml` (modified)

**Steps:**
1. Set the job-level `timeout-minutes:` to
   `${{ github.event_name == 'workflow_dispatch' && 30 || 20 }}`.
2. Keep the workflow-level `paths:` trigger and the main-checkout
   refusal drill (`fuzz dsl_yaml`, target-agnostic guard) unchanged.
3. After checkout/setup, add a leg-selection step: on
   `workflow_dispatch` write all four targets to `$GITHUB_ENV` as
   `FUZZ_LEGS`; on `pull_request`, fetch the base commit
   (`git fetch origin "${{ github.event.pull_request.base.sha }}"`),
   emit `git diff --name-only "${{ github.event.pull_request.base.sha }}"..HEAD`,
   and pipe the path list through `./scripts/fuzz-legs.sh` to compute
   `FUZZ_LEGS`.
4. Replace the single `fuzz dsl_yaml --time "$SMOKE_TIME"` smoke step
   with a serial loop: `for leg in $FUZZ_LEGS; do cargo run --package
   xtask -- fuzz "$leg" --time "$SMOKE_TIME" > "smoke-$leg.log" 2>&1 ||
   SMOKES_FAILED=1; done` inside the linked worktree. The replacement
   step SHALL carry `- id: smoke` (the static test
   `aggregate-smoke-line-is-single` scopes its emission count to that
   step id). Preserve three
   invariants of the current step verbatim: the
   `CARGO_TARGET_DIR="$RUNNER_TEMP/xtask-build"` override on the cargo
   invocation, the per-step durations.txt recording, and the summary's
   check-line contract — the final missing-checks loop greps
   `^(PASS|FAIL) smoke( |$)` over a fixed name list, so the loop must
   emit EXACTLY ONE aggregate line: `PASS smoke` when every leg is
   clean, `FAIL smoke <per-leg detail>` otherwise (per-leg detail
   accumulated inside the loop, never as separate per-leg PASS/FAIL
   smoke lines).
5. Generalize the corpus/crash verification loop: for each leg in
   `$FUZZ_LEGS`, assert `target-fuzz/corpus/$leg` is populated and check
   `target-fuzz/artifacts/$leg/crash-*`; accumulate per-leg detail
   strings.
6. Move the tmin panic-injection drill to `dsl_json`: sed
   `camel_fuzz::dsl_json_harness(data);` in
   `fuzz/fuzz_targets/dsl_json.rs` (same injection pattern as the
   current dsl_yaml drill), run `fuzz dsl_json --time 20`, and point
   the artifact assertions at `target-fuzz/artifacts/dsl_json/`. The
   drill runs exactly once per workflow run regardless of leg count.
7. Generalize the findings summary: enumerate
   `target-fuzz/artifacts/<leg>/` for each leg in `$FUZZ_LEGS` that has
   findings, keep the triage command (`bd create -t bug -p 1`) and
   promotion rule verbatim, and keep the no-automation rule (no
   bd/issue creation).

**Tests:** (static verification — the workflow itself is exercised by the Task 5.1 dispatch)
- `workflow-yaml-parses`: `nix-shell -p python3Packages.pyyaml --run 'python3 -c "import yaml,sys; yaml.safe_load(open(\".github/workflows/fuzz-smoke.yml\"))"'` → exit 0.
- `timeout-is-conditional`: the file contains `timeout-minutes: ${{ github.event_name == 'workflow_dispatch' && 30 || 20 }}` → grep exit 0.
- `no-hardcoded-smoke-target`: the smoke step contains no literal `fuzz dsl_yaml --time` outside the refusal drill and sed sections → the only `dsl_yaml` literals remaining are the refusal drill, its guard assertions, and comments.
- `drill-target-is-dsl-json`: the sed pattern references `dsl_json_harness` and the artifact assertions reference `artifacts/dsl_json` → grep exits 0.
- `summary-enumerates-legs`: the summary step references `$FUZZ_LEGS` (or equivalent loop variable) when listing artifact directories → grep exit 0.
- `aggregate-smoke-line-is-single`: three exact commands against `.github/workflows/fuzz-smoke.yml`: (a) `grep -E '(PASS|FAIL) smoke-' .github/workflows/fuzz-smoke.yml` exits 1 (no per-leg verdict lines exist); (b) `test "$(awk '/- id: smoke/{f=1;next} f&&/- id:/{exit} f{print}' .github/workflows/fuzz-smoke.yml | grep -cF 'echo "PASS smoke"')" -eq 1` and `test "$(awk '/- id: smoke/{f=1;next} f&&/- id:/{exit} f{print}' .github/workflows/fuzz-smoke.yml | grep -cF 'echo "FAIL smoke')" -eq 1` (exactly one aggregate emission each); (c) `grep -F 'grep -qE "^(PASS|FAIL) $name( |$)" "$findings"' .github/workflows/fuzz-smoke.yml` exits 0 (the generic missing-check pattern is intact) and `grep -E 'for name in .* smoke( |;)' .github/workflows/fuzz-smoke.yml` exits 0 (`smoke` remains in the fixed check-name list).
- `selector-integrated`: the workflow pipes the diff path list through `scripts/fuzz-legs.sh` and exports the result as `FUZZ_LEGS` → grep for `fuzz-legs.sh` and `FUZZ_LEGS` exits 0.
- `smoke-runs-serially`: the smoke step iterates `for leg in $FUZZ_LEGS` (or equivalent) invoking `cargo run --package xtask -- fuzz "$leg"` → grep exit 0.
- `per-leg-corpus-and-crash-checks`: the verification loop checks `target-fuzz/corpus/$leg` (or `$leg`-parameterized equivalent) AND `target-fuzz/artifacts/$leg` per leg → grep exits 0 for both.

**Acceptance:**
- All nine static tests above pass.
- `./scripts/fuzz-legs.sh --self-test` still exits 0 (no drift introduced by 3.2).

- [x] 3.2

## Verification

### Task 4.1: local panic-injection drills for the three new targets

**Files:**
- `openspec/changes/canonical-path-fuzzing/verification.md` (new)

**Steps:**
1. Create a throwaway linked worktree from the feature branch HEAD
   (`git worktree add <tmp-path> -b drill/<target>`), one per target —
   or reuse one worktree sequentially, cleaning
   `target-fuzz/artifacts/<target>/` between drills (birth-time
   detection misses re-crashes, bd rc-0wb9).
2. For each target `T` in `dsl_json dsl_template dsl_parity`: sed the
   harness call in `fuzz/fuzz_targets/<T>.rs` to
   `panic!("local-drill"); camel_fuzz::<T>_harness(data);` (keep the
   original call after the panic).
3. From the drill worktree root run (per target, substituting `$T`):
   `nix develop . -c bash -c 'cargo run --quiet --package xtask -- fuzz "$T" --time 20'`
   (the flake lives in every worktree; this matches the repo's
   established devShell invocation convention).
4. Assert per target: non-zero exit; `new artifact(s):` in output;
   `minimized artifact:` line whose file exists under
   `target-fuzz/artifacts/<T>/`; NO regular files under
   `fuzz/artifacts/` (cargo-fuzz 0.13.x creates that directory
   unconditionally at startup — emptiness, not absence, is the
   contract); the promotion instruction printed. Clean the artifacts
   dirs after each target (`rm -rf target-fuzz/artifacts/<T>
   fuzz/artifacts` — rc-0wb9 birth-time protection).
5. Remove the drill worktree(s) and branches. Record per-target command
   outputs and assertions in `verification.md` under a
   `## Local drills` heading.

**Tests:** (the drill IS the test — repeat per target)
- `drill-<T>-catches-and-minimizes`: injected panic in `<T>`'s harness → wrapper exits non-zero, minimized artifact exists under `target-fuzz/artifacts/<T>/`, no regular files under `fuzz/artifacts/`, promotion instruction printed → verified for T = dsl_json, dsl_template, dsl_parity.

**Acceptance:**
- All three drills pass their assertions; `verification.md` contains the three records.
- `git status --porcelain` in the feature worktree is clean of drill residue (drills ran in throwaway worktrees).

- [x] 4.1

### Task 5.1: CI dispatch evidence and verification close-out (runs post-merge)

**Files:**
- `openspec/changes/canonical-path-fuzzing/verification.md` (modified — this task executes after the squash-merge and archive, editing the archived copy, with the evidence commit following the 9e8f36f6 precedent)

**Steps:**
1. Preconditions (human actions, never the conductor's): merge approved,
   squash-merged to main, pushed. Then dispatch from the repo root:
   `gh workflow run fuzz-smoke.yml --ref main` (all legs, default smoke
   time).
2. Poll `gh run watch` / `gh run view` until completion. Assert: job
   completes within the 30-minute dispatch ceiling; all four legs'
   smoke steps report corpus populated and no crash artifact; the
   dsl_json tmin drill passes its assertions; no regular files under
   `fuzz/artifacts/`; the findings summary renders leg-scoped artifact paths.
3. Record in `verification.md` under `## CI dispatch evidence`: run URL,
   total duration, per-leg outcomes, drill outcome.
4. Commit the evidence update on main:
   `docs(fuzz): canonical-path-fuzzing CI evidence` with `Bd: rc-fvah`.
5. Close bd: `bd close rc-fvah --reason "Completed"` from the repo root.

**Tests:**
- `dispatch-all-legs-green`: one `workflow_dispatch` run on main → job conclusion success (continue-on-error contract intact — success means no infrastructure failure; any fuzz finding appears as annotation + summary, handled per the findings flow, not as a red X).
- `duration-within-ceiling`: run duration < 30 minutes.

**Acceptance:**
- `verification.md` records the run URL with all-legs outcomes.
- bd rc-fvah closed.

- [x] 5.1
