# Tasks: nondsl-channel-fuzzing

## fuzz crate

### Task 1: Channel harness functions + semantics tests

**Files:**
- `fuzz/src/lib.rs` (modified)

**Steps:**
1. Add `pub fn dsl_rest_harness(data: &[u8])` next to the existing harness fns: if `std::str::from_utf8(data)` is `Ok(s)`, call `camel_dsl::json::parse_json_with_threshold_and_security(s, camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD, SecurityCompileContext::default())` and then `camel_dsl::yaml::parse_yaml_with_threshold_and_security(s, camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD, SecurityCompileContext::default())`, discarding both results. Doc-comment: never-panic invariant, both front-ends lower `rest:` blocks, invalid UTF-8 skipped.
2. Add `pub fn dsl_mcp_harness(data: &[u8])` with the identical two-parse shape (both front-ends lower `mcp:` blocks).
3. Add `pub fn dsl_openapi_harness(data: &[u8])`: if UTF-8 ok, run TWO legs. YAML leg: `camel_dsl::yaml::extract_rest_blocks(s)` → if `Ok(blocks)` and `!blocks.is_empty()`, `camel_dsl::rest::lower_all_rest_to_routes(&blocks)` then `camel_dsl::rest::check_duplicate_route_ids(&lowered)`; if both `Ok`, `camel_dsl::openapi::generate_openapi(&blocks, "camel-fuzz", "0.0.0")`, discard. JSON leg: `serde_json::from_str::<camel_dsl::route_ast::RouteDslRoutes>(s)` → take `.rest`, same validate-then-generate sequence. Mirror the camel-cli `run_generate` validated-generation stage (crates/camel-cli/src/commands/openapi.rs:42-64).
4. Add semantics tests in the `#[cfg(test)]` tests module of `fuzz/src/lib.rs` (the location that already holds the dsl_json/dsl_template harness tests), one trio per harness, using fixtures adapted from `crates/camel-dsl/src/json.rs` tests `json_rest_block_expands_into_routes` (line ~626) and the mcp equivalents in `crates/camel-dsl/src/mcp.rs` tests.

**Tests:** (in the file chosen in step 4; command run from the worktree root)
- `dsl_rest_harness_valid_minimal_returns`: minimal JSON doc with one `rest:` block (host/port/base-path/one GET operation) → `dsl_rest_harness(bytes)` → returns without panic.
- `dsl_rest_harness_malformed_returns`: `b"{"` and a truncated `{"rest": [{"` → harness → no panic.
- `dsl_rest_harness_yaml_shaped_returns`: same minimal rest doc authored as YAML → harness → no panic.
- `dsl_rest_harness_invalid_utf8_returns`: `b"\xff\xfe"` → harness → no panic (skipped before parse).
- `dsl_mcp_harness_valid_minimal_returns`: JSON doc with `mcp:` block (server name/bind + one tool with `input_schema` + one resource with uri) → harness → no panic.
- `dsl_mcp_harness_hostile_rejected`: docs with tool name `bad name!` (fails name pattern), `bind: "not-an-ip:port"` (fails bind pattern), tool `input_schema: "string"` (fails schema validation), blank `tls.cert_path` → harness → no panic (each rejected with Err inside).
- `dsl_mcp_harness_invalid_utf8_returns`: `b"\xff"` → no panic.
- `dsl_openapi_harness_valid_returns`: rest doc whose blocks pass lowering (one operation with explicit `operationId`, one with no response schemas) → harness → no panic.
- `dsl_openapi_harness_duplicate_across_listeners_returns`: two blocks on different `host:port` claiming the same `(path, verb)` with distinct `operationId`s → harness → no panic (passes per-listener validation; generate records duplicate warning, discarded).
- `dsl_openapi_harness_unvalidated_skips_generation`: same-listener duplicate `(path, verb)` doc → harness → no panic (lowering Err skips generate).
- `dsl_openapi_harness_invalid_utf8_returns`: `b"\xff"` → no panic.
- `dsl_openapi_harness_empty_docs_skip_generation`: `{}` and `{"routes":[]}` → harness → no panic (JSON leg's empty-`rest` guard mirrors the YAML leg's `!blocks.is_empty()`; no extraction/validation/generation input).
- Command: `TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo test --manifest-path fuzz/Cargo.toml` → all pass. (`--manifest-path` keeps the run inside the worktree; fuzz crate is workspace-excluded.)

**Acceptance:**
- `cargo fmt --check` clean over `fuzz/`.
- All new tests pass via the command above.
- No `unwrap`/`expect` added (lint-unwrap clean): harnesses discard results; tests use existing fixture style.

- [x] 1.1

### Task 2: Fuzz target bins + xtask registration

**Files:**
- `fuzz/fuzz_targets/dsl_rest.rs` (new)
- `fuzz/fuzz_targets/dsl_mcp.rs` (new)
- `fuzz/fuzz_targets/dsl_openapi.rs` (new)
- `fuzz/Cargo.toml` (modified)
- `scripts/xtask/src/fuzz.rs` (modified)

**Steps:**
1. Create each `fuzz_targets/<name>.rs` exactly in the existing shape: `#![no_main]` + `libfuzzer_sys::fuzz_target!(|data: &[u8]| { camel_fuzz::<name>_harness(data); });` (copy `fuzz/fuzz_targets/dsl_json.rs` verbatim and swap the harness call).
2. Append three `[[bin]]` entries to `fuzz/Cargo.toml` (`name = "dsl_rest"` / `"dsl_mcp"` / `"dsl_openapi"`, `path = "fuzz_targets/<name>.rs"`, `test = false`, `doc = false`), matching the existing entries.
3. Extend `KNOWN_TARGETS` in `scripts/xtask/src/fuzz.rs` to `["dsl_yaml", "dsl_json", "dsl_template", "dsl_parity", "dsl_rest", "dsl_mcp", "dsl_openapi"]`. The file's tests module pins the current list: rewrite `known_targets_cover_all_four` (fuzz.rs:~577, asserts `len() == 4` plus exactly-once per name) into a seven-name check (rename without "four"); `known_targets_seeds_dirs_exist` (fuzz.rs:~581) stays as-is — it requires `fuzz/seeds/<name>/` for every entry and only goes green once Task 3 lands the three new seed directories (expected red between Task 2 and Task 3; do not weaken it).
4. Verify with `cd fuzz && cargo fuzz list` printing all seven targets (this task's xtask-run acceptance is deferred: `xtask fuzz` copies seeds via `fs::read_dir(fuzz/seeds/<target>/)` and errors before Task 3 creates the dirs — the "xtask accepts new names" run happens in Task 5).

**Tests:**
- `cargo-fuzz list shows seven targets`: `cargo fuzz list` (cwd `fuzz/`) → stdout contains `dsl_yaml dsl_json dsl_template dsl_parity dsl_rest dsl_mcp dsl_openapi`.
- `xtask known-targets test updated`: `cargo test -p xtask` → `known_targets_cover_all_four`-successor passes; `known_targets_seeds_dirs_exist` may fail ONLY on the three new names until Task 3 (record its output; it must pass after Task 3).

**Acceptance:**
- `cargo build -p xtask` exits 0; `cargo clippy -p xtask -- -D warnings` clean.
- `cargo fuzz list` shows seven targets.
- xtask cover test asserts seven names exactly-once (seeds-dirs test green deferred to Task 3).

- [x] 1.2

### Task 3: Seed corpora + seed-contract tests

**Files:**
- `fuzz/seeds/dsl_rest/` (new directory, 6 files: `valid_minimal.json`, `valid_multiple_operations.json`, `malformed_truncated.json`, `malformed_duplicate_tuple.json`, `malformed_ambiguous_template.json`, `malformed_bad_media.json`)
- `fuzz/seeds/dsl_mcp/` (new directory, 6 files: `valid_minimal.json`, `valid_tool_and_resource.json`, `malformed_truncated.json`, `malformed_bad_name.json`, `malformed_bad_bind.json`, `malformed_blank_tls_path.json`)
- `fuzz/seeds/dsl_openapi/` (new directory, 5 files: `valid_minimal.json`, `warning_weak_stub.json`, `warning_duplicate_across_listeners.json`, `malformed_truncated.json`, `malformed_same_listener_duplicate.json`)
- `fuzz/src/lib.rs` (modified — `#[cfg(test)]` tests module, alongside the existing `seeds_dsl_json_contract` tests)

**Steps:**
1. Author each seed by hand as a minimal doc exercising exactly one behavior (reuse fixtures from `crates/camel-dsl` tests: json.rs `json_rest_block_expands_into_routes`, rest.rs ambiguity tests, mcp.rs validation tests, openapi.rs warning tests). Keep every file under 40 lines.
2. `valid_minimal` seeds must parse `Ok` through BOTH `parse_json_with_threshold_and_security` and `parse_yaml_with_threshold_and_security` (JSON is valid YAML).
3. Add seed-contract tests mirroring `seeds_dsl_json_contract`: for each target, pin the exact directory shape (sorted name list equality), run every seed through that target's harness (no panic), assert each valid seed parses `Ok` through both front-ends, and assert each malformed seed is rejected by its documented path (named function returning `Err`).
4. dsl_openapi extra contract: `warning_weak_stub.json` → extract + validate + `generate_openapi` → assert `warnings` contains the exact weak-stub warning substring from `crates/camel-dsl/src/openapi.rs` (`build_operation`'s missing-schema warning — copy the literal); `warning_duplicate_across_listeners.json` → assert `warnings` contains the duplicate-operation warning substring.

**Tests:**
- `seeds_dsl_rest_contract`: pin 6-file shape; all 6 through `dsl_rest_harness` no panic; `valid_*` parse Ok both front-ends; `malformed_duplicate_tuple.json` and `malformed_ambiguous_template.json` rejected by the parse-with-lowering path (Err); `malformed_bad_media.json` rejected with a media error.
- `seeds_dsl_mcp_contract`: pin 6-file shape; all through `dsl_mcp_harness`; `valid_*` Ok both front-ends; `malformed_bad_name.json` + `malformed_bad_bind.json` + `malformed_blank_tls_path.json` rejected with the corresponding validation errors.
- `seeds_dsl_openapi_contract`: pin 5-file shape; all through `dsl_openapi_harness`; `valid_minimal.json` + both `warning_*` seeds parse Ok both front-ends AND their blocks pass `lower_all_rest_to_routes`; `malformed_truncated.json` fails extraction/deserialization; `malformed_same_listener_duplicate.json` fails lowering; the two warning assertions from step 4.
- Command: `TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo test --manifest-path fuzz/Cargo.toml`.

**Acceptance:**
- All seed-contract tests pass; directories contain exactly the pinned files (`ls fuzz/seeds/dsl_rest | wc -l` → 6, etc.).
- Every seed file < 40 lines, text-only, no binary blobs.
- `cargo test -p xtask` fully green (the deferred `known_targets_seeds_dirs_exist` from Task 2 now finds all seven seed directories).
- `cargo run --package xtask -- fuzz dsl_rest --time 1` accepts the new name (seeds copy succeeds; run or clean timeout — existence check only, deep runs are Task 5's).

- [x] 1.3

## fleet tooling

### Task 4: fuzz-legs.sh rules + self-test

**Files:**
- `scripts/fuzz-legs.sh` (modified)
- `.github/workflows/fuzz-smoke.yml` (modified — stale "all four legs" comment at ~line 60 only)

**Steps:**
1. Extend `ALL_TARGETS` to `(dsl_yaml dsl_json dsl_template dsl_parity dsl_rest dsl_mcp dsl_openapi)` — canonical order, ranking order for the three new legs (rest > mcp > openapi per design.md audit).
2. Extend `classify()`: `crates/camel-dsl/src/rest.rs` → `dsl_rest dsl_openapi`; `crates/camel-dsl/src/mcp.rs` → `dsl_mcp`; `crates/camel-dsl/src/openapi.rs` → `dsl_openapi`; change the `yaml.rs` rule to `dsl_yaml dsl_parity dsl_rest dsl_mcp dsl_openapi` and the `json.rs` rule to `dsl_json dsl_parity dsl_rest dsl_mcp dsl_openapi` (every channel harness drives both front-ends). Extend the catch-all pattern with `Cargo.toml | Cargo.lock` so a manifest-only PR (which the workflow trigger already admits) selects all legs instead of zero. Keep the seeds-directory rule generic over `ALL_TARGETS` (it already loops).
3. Extend the `--self-test` cases with: `crates/camel-dsl/src/rest.rs` → `dsl_rest dsl_openapi`; `crates/camel-dsl/src/mcp.rs` → `dsl_mcp`; `crates/camel-dsl/src/openapi.rs` → `dsl_openapi`; updated `json.rs`/`yaml.rs` expectations; `fuzz/seeds/dsl_openapi/warning_weak_stub.json` → `dsl_openapi`; `fuzz/seeds/dsl_rest/valid_minimal.json` → `dsl_rest`; `Cargo.lock` → all seven legs. ALSO update every existing case whose expectation embeds a front-end rule output or pins the target count: `mixed-changes-union` (json.rs leg set grows), `non-trigger-path-ignored` (json.rs expectation grows), and the `local ALL`/dispatch case that pins the four-name list → seven names.
4. Read `.github/workflows/fuzz-smoke.yml` end-to-end and confirm no step hardcodes the four-target set in a way that breaks with seven legs (the smoke drill iterates `FUZZ_LEGS`; the tmin/crash drills hardcode `dsl_json` by design and stay). Update the yml comment that says "all four legs" (if present) to say "all legs". If any other hardcode would break, fix it; otherwise record in the task report that none needed changing.
5. Verify the workflow's `on.pull_request.paths` trigger list contains `Cargo.toml` and `Cargo.lock` alongside the four path patterns (existing behavior the MODIFIED requirement restates — grep-confirm, no edit expected).

**Tests:**
- `fuzz-legs self-test passes`: `scripts/fuzz-legs.sh --self-test` → exit 0.
- `channel source selects channel legs`: `printf '%s\n' crates/camel-dsl/src/rest.rs | scripts/fuzz-legs.sh` → stdout exactly `dsl_rest dsl_openapi`.
- `front-end change selects front-end + parity + channel legs`: `printf '%s\n' crates/camel-dsl/src/json.rs | scripts/fuzz-legs.sh` → `dsl_json dsl_parity dsl_rest dsl_mcp dsl_openapi`.
- `seed-only change selects its leg`: `printf '%s\n' fuzz/seeds/dsl_mcp/valid_minimal.json | scripts/fuzz-legs.sh` → `dsl_mcp`.
- `manifest-only change selects all legs`: `printf '%s\n' Cargo.lock | scripts/fuzz-legs.sh` → all seven targets in canonical order.
- `unrelated path selects nothing`: `printf '%s\n' crates/camel-core/src/lib.rs | scripts/fuzz-legs.sh` → empty.
- `workflow manifest triggers present`: `grep -c 'Cargo.toml\|Cargo.lock' .github/workflows/fuzz-smoke.yml` → at least 2 (path trigger entries).

**Acceptance:**
- `scripts/fuzz-legs.sh --self-test` exit 0.
- The four stdin cases above produce exactly the listed outputs (verify manually or via a one-liner loop in the task report).

- [x] 2.1

## verification

### Task 5: First scoped runs + triage (conductor-executed — fleet scope mandate)

**Files:**
- `openspec/changes/nondsl-channel-fuzzing/verification.md` (new)
- `crates/camel-dsl/tests/` regression test file (new, ONLY if a crash is found)

**Steps:**
1. For each of `dsl_rest`, `dsl_mcp`, `dsl_openapi`, run a first fuzz session under the fleet containment scope exactly:
   `systemd-run --user --scope --collect --unit=fleet-fuzznondsl -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache cargo run --package xtask -- fuzz <target> --time 60`
   then `systemctl --user stop fleet-fuzznondsl.scope`. Before/after each run: `pgrep -c compiled_artifa` must be 0.
2. Record per target: exit code, corpus file count under `target-fuzz/corpus/<target>/`, crash artifacts under `target-fuzz/artifacts/<target>/` (expect none), wall time.
3. If any crash artifact appears: minimize with `cargo fuzz tmin`, commit the minimized reproducer as a normal regression test in `crates/camel-dsl/tests/` (never the raw artifact), and file a bd issue (`bd create "fuzz <target>: <panic>" -t bug -p <severity> --deps discovered-from:rc-rs2v` from the repo root).
4. Write `verification.md` in the change dir: per-target run evidence, triage outcomes (or "no crashes"), and the ranking note from design.md.

**Tests:**
- `first run completes per target`: three scoped runs finish with exit 0 and populated corpora (`find target-fuzz/corpus/<target> -type f | wc -l` > 0).
- `no crash artifacts on clean runs`: `ls target-fuzz/artifacts/<target>/crash-* 2>/dev/null` → empty for all three (else triage path step 3 executed and documented).

**Acceptance:**
- `verification.md` exists with per-target evidence.
- Any crash found is converted to a committed regression test + bd issue (reference recorded in verification.md).
- No raw crash artifacts or corpus files committed to the repo (`git status` clean of target-fuzz/ paths — they are git-ignored).

- [x] 3.1
