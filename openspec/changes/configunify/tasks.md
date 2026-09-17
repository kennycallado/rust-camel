# Tasks: configunify

## Phase 1: Parity golden harness (lock pre-refactor behavior)

### camel-config

#### Task 1.1: Filesystem-loader parity goldens in camel-config

**Files:**
- `crates/camel-config/src/config_tests/parity_golden_tests.rs` (new)
- `crates/camel-config/src/config_tests/parity_goldens/` (new directory, committed goldens: JSON projections `case_01.json`, `case_02.json`, `case_04.json`, `case_05.json`, `case_06.json`, `case_07a.json`, `case_07b.json`, `case_08.json`, plus text goldens `case_03_error.txt`, `case_05_warn.txt`, `case_09_error.txt`, `case_10_error.txt`)
- `crates/camel-config/src/config.rs` (modified — add the `#[path = "config_tests/parity_golden_tests.rs"] mod ...` declaration alongside the sibling declarations at ~1250)

**Steps:**
1. Read the loader entry points used by the existing suites: `profile_loading_tests.rs` (public loading entry, `CAMEL_PROFILE` env serialization guard) and `virtual_store_file_parity_tests.rs` (`CamelConfig::from_file_sealed`, `CamelConfig::from_toml_value_with_env`, injected deployment lookup). Reuse the same entries and the same env-guard convention; follow `cache_repo_env_override_tests.rs` for real-env-override serialization.
2. Create the test module with 10 matrix cases, each writing its input documents to a fresh tempdir and loading through the public loader: (a) flat config, no profile; (b) `[default]`+`[production]` with `CAMEL_PROFILE=production`, deep-merge and `routes` array replacement; (c) unknown profile `staging` with `[default]` present; (d) ordered includes `base.toml` including `a.toml` then `b.toml`, config merged above them; (e) include fragment that itself declares `include` (ignored — assert the include key never loads a third document AND capture the loader's diagnostic via a capturing `tracing_subscriber` layer into `case_05_warn.txt`, byte-compared; if the filesystem loader emits no diagnostic for this case, record that finding in the report and lock an empty golden); (f) include fragment carrying its own `[default]`/`[production]` sections (lenient per-file selection); (g) value `${env:PARITY_GOLDEN_VAR:-fallback}` resolved with the var set to `pinned` and (separately) unset; (h) section-level `routes` replacing top-level `routes`; (i) top-level `include = 42` (invalid type) → load fails, full error `Display` locked byte-for-byte in `case_09_error.txt`; (j) `[default].include = 42` (invalid type at section level) → load fails, full error `Display` locked in `case_10_error.txt`. Cases (i)/(j) exist because Task 2.2 rewires the error-label construction inside `extract_includes` — these goldens freeze both label spellings.
3. For success cases serialize a test-local JSON projection of the resolved `CamelConfig` — `CamelConfig` has no `Serialize` derive and cannot get one (closure-bearing fields), so each case builds a `serde_json::json!` object from exactly the matrix-relevant resolved fields (for example `timeout_ms`, `routes`, `log_level`, `watch`, `components.http.max_connections`, include-provided leaves, env-resolved leaves) and writes/compares `serde_json::to_string_pretty` output byte-for-byte against `crates/camel-config/src/config_tests/parity_goldens/case_<n>.json`. Every field the matrix exercises appears in the projection. For case (c) assert the error message contains exactly `Unknown profile: staging` and lock that string in a golden `case_03_error.txt`.
4. Implement regeneration: when `UPDATE_GOLDENS=1` is set in the environment, write the golden files instead of comparing.
5. Run with `UPDATE_GOLDENS=1` to capture goldens from the un-refactored tree, commit them, then run without the variable to verify comparison passes.

**Tests:**
- `parity_flat_config_matches_golden`: tempdir flat `Camel.toml` → load with no profile → JSON projection equals `parity_goldens/case_01.json` byte-for-byte.
- `parity_profile_deep_merge_and_array_replace`: `[default]`+`[production]`, `CAMEL_PROFILE=production` → nested table merges recursively and `routes` array equals the production list exactly (golden `case_02.json`).
- `parity_unknown_profile_error_string_locked`: `[default]` present, `CAMEL_PROFILE=staging` → load fails; error Display contains `Unknown profile: staging` and equals `case_03_error.txt`.
- `parity_ordered_includes_merge_order`: config includes a then b, overlapping keys → golden `case_04.json` shows config value winning over include values and later include over earlier.
- `parity_recursive_include_decl_ignored`: fragment declaring `include` → load succeeds, no third document loaded (golden `case_05.json` shows only the two documents' merged values); loader diagnostic (if any) equals `case_05_warn.txt`.
- `parity_include_with_own_profile_sections`: fragment with `[default]`/`[production]` under active profile → per-file selection applied, golden `case_06.json`.
- `parity_env_override_set_and_unset`: `${env:PARITY_GOLDEN_VAR:-fallback}` → resolved leaf is `pinned` when set, `fallback` when unset (goldens `case_07a.json`, `case_07b.json`); serialize env-mutating cases with the existing static-`Mutex` convention.
- `parity_section_routes_replace_toplevel`: `[default].routes` overriding top-level `routes` → golden `case_08.json`.
- `parity_toplevel_include_invalid_type_error_locked`: top-level `include = 42` → load fails; full error `Display` equals `case_09_error.txt` byte-for-byte.
- `parity_section_include_invalid_type_error_locked`: `[default].include = 42` → load fails; full error `Display` equals `case_10_error.txt` byte-for-byte (locks the `default.include` label spelling).
- Command: `cargo test -p camel-config parity_` (all pass).
- Expected: all pass on the un-refactored tree (this task changes no production code).

**Acceptance:**
- `cargo test -p camel-config parity_` green with committed goldens; regeneration via `UPDATE_GOLDENS=1` proven once.
- `cargo fmt --check` and `cargo clippy -p camel-config -- -D warnings` clean.
- No production file under `crates/camel-config/src/` other than the `#[path]` test-module declaration line in `config.rs` is modified (no derive additions — the projection seam is test-local).

- [x] 1.1

### camel-dsl

#### Task 1.2: Virtual-store parity goldens in camel-dsl

**Files:**
- `crates/camel-dsl/src/discovery.rs` (modified — extend the existing `mod tests` at line ~1156; no production code changes)
- `crates/camel-dsl/tests/goldens/virtual_config/` (new directory, committed goldens: `case_01.toml`, `case_02.toml`, `case_03.toml`, `case_04.toml`, `case_05.toml`, `case_06.toml`, `case_08.toml`, plus `case_04_warn.txt` and `case_07_error.txt`)

**Steps:**
1. Read `build_virtual_config` (607), `classify_virtual_config` (550), and `crates/camel-config/src/config_tests/virtual_store_file_parity_tests.rs` for the established store construction pattern (`VirtualDocumentStore::build` with `StoreDocument` entries, `config_references` ordering, `discover_virtual_store`); reuse it verbatim for the fixtures.
2. Add store-form matrix cases to the tests module: (a) config only, flat; (b) config `[default]`+`[production]`, selected `[production]` — merge + replace; (c) includes in declaration order below config; (d) fragment declaring `include` (warn + strip); (e) multi-profile ordered selection `profiles=["production","qa"]` both present — later `qa` overlays `production`; (f) partial absence `profiles=["production","qa"]` with only `production` present — merges `production`, NO error (store backstop only fires when no selected section exists); (g) config with `[default]` and `profiles=["staging"]` absent → `MalformedVirtualConfig` whose message is locked in a golden; (h) `${env:PARITY_STORE_VAR:-fallback}` passes through unresolved into the merged value. Case (d) asserts BOTH the strip (include key absent from merged output) and the warning: install a capturing `tracing_subscriber` layer for the test (tracing-subscriber is a regular dep) and assert one WARN record whose message contains `recursive includes are unsupported`; lock the full message in `case_04_warn.txt`.
3. Serialize `build_virtual_config` output with `toml::to_string_pretty`, compare byte-for-byte to `crates/camel-dsl/tests/goldens/virtual_config/case_<n>.toml` (load via `env!("CARGO_MANIFEST_DIR")` relative path). Error case golden `case_07_error.txt` holds the full error Display string.
4. Implement `UPDATE_GOLDENS=1` regeneration; capture goldens from the un-refactored tree; commit; verify comparison pass.

**Tests:**
- `virtual_config_flat_matches_golden`: store with one flat config doc → merged equals `case_01.toml`.
- `virtual_config_profile_selection_merges_and_replaces`: `[default]`+`[production]`, profiles `["production"]` → equals `case_02.toml` (tables deep-merged, `routes` replaced).
- `virtual_config_includes_below_config`: two includes then config → equals `case_03.toml`.
- `virtual_config_recursive_include_warns_and_strips`: fragment with `include` key → key absent from merged output (golden `case_04.toml`) AND one captured WARN record equals `case_04_warn.txt`.
- `virtual_config_multi_profile_ordered_overlay`: profiles `["production","qa"]` both present → equals `case_05.toml` with qa-winning conflicts.
- `virtual_config_partial_absence_merges_present`: profiles `["production","qa"]`, only `production` present → equals `case_06.toml`, no error.
- `virtual_config_unknown_profile_backstop_error_locked`: `[default]` + absent `staging` → `MalformedVirtualConfig` Display equals `case_07_error.txt`.
- `virtual_config_env_placeholder_passes_through`: `${env:PARITY_STORE_VAR:-fallback}` leaf → merged golden `case_08.toml` contains the raw placeholder.
- Command: `cargo test -p camel-dsl virtual_config_` (all pass).
- Expected: all pass on the un-refactored tree.

**Acceptance:**
- `cargo test -p camel-dsl virtual_config_` green with committed goldens; `UPDATE_GOLDENS=1` proven once.
- `cargo fmt --check` and `cargo clippy -p camel-dsl -- -D warnings` clean.
- Zero changes to production code paths in `discovery.rs` (test module only).

- [x] 1.2

### camel-cli

#### Task 1.3: Compiled-artifact parity goldens in camel-cli

**Files:**
- `crates/camel-cli/tests/fixtures/config-parity-project/` (new directory: `Camel.toml`, `includes/common.toml`, `includes/extra.toml`, one route file, `routes/` layout)
- `crates/camel-cli/tests/config_compile_parity.rs` (new)
- `crates/camel-cli/tests/goldens/config-parity/` (new directory, committed goldens)

**Steps:**
1. Read `crates/camel-cli/tests/compiled_artifact_test.rs` and `compile_command_test.rs` for the established patterns: invoking `CARGO_BIN_EXE_camel` with `compile --config ... --profile ...`, and decoding the artifact trailer via the pattern `compile_command_test.rs` lines 14-15 use (`camel_cli::compile::trailer` decode + `camel_cli::compile::store` access).
2. Build the fixture project: `Camel.toml` with `[default]`, `[production]`, `[qa]` sections, top-level `include = ["includes/common.toml"]` and `[default].include = ["includes/extra.toml"]`, top-level and `[production].routes` patterns; `includes/common.toml` (flat values + its own `[production]` fragment values); `includes/extra.toml`; one route file.
3. New test invoking compile with `--profile production`, reading the embedded store from the artifact, computing the resolved merged configuration through the same discovery entry the runtime uses (`discover_virtual_store`), serializing with `toml::to_string_pretty`, comparing to `goldens/config-parity/production.toml`. Second golden with `--profile qa` → `qa.toml`.
4. Error-parity test: compile with `--profile production --profile qa` where the fixture chain (config + includes) defines `[qa]` ONLY in a variant fixture where it is absent — assert non-zero exit and capture the actual stderr/stdout error line into `goldens/config-parity/partial_absence_error.txt` from the pre-refactor binary (the Display text is `unknown profile 'qa': the selected profile section must exist in the explicit configuration` — but the GOLDEN is the authority, not this prose); the test then compares against the golden content byte-for-byte.
5. Implement `UPDATE_GOLDENS=1` regeneration; capture from the un-refactored tree; commit; verify.

**Tests:**
- `compiled_artifact_resolved_config_production_golden`: compile fixture with `--profile production` → runtime-discovery merged config equals `production.toml` byte-for-byte.
- `compiled_artifact_resolved_config_qa_golden`: `--profile qa` → equals `qa.toml`.
- `compile_partial_profile_absence_error_locked`: `--profile production --profile qa` on the variant fixture without `[qa]` → non-zero exit, error text equals `partial_absence_error.txt`.
- Command: `cargo test -p camel-cli --test config_compile_parity` (passes on un-refactored tree).
- Expected: all pass pre-refactor.

**Acceptance:**
- `cargo test -p camel-cli --test config_compile_parity` green with committed goldens; `UPDATE_GOLDENS=1` proven once.
- `cargo fmt --check` and `cargo clippy -p camel-cli -- -D warnings` clean.
- Fixture project loads and compiles through the real `CARGO_BIN_EXE_camel` binary (no in-process shortcuts).

- [x] 1.3

**Phase 1 exit gate (inter-phase checkpoint, design.md exit-criteria):**
- `cargo test -p camel-config -p camel-dsl -p camel-cli` exits 0 (full combined suite, goldens included) on the un-refactored tree with goldens committed — run BEFORE any Phase 2 production edit begins.

## Phase 2: Canonical config_semantics + delegation

### camel-dsl

#### Task 2.1: Create canonical config_semantics module

**Files:**
- `crates/camel-dsl/src/config_semantics.rs` (new)
- `crates/camel-dsl/src/lib.rs` (modified — add `pub mod config_semantics;`)

**Steps:**
1. Create `config_semantics.rs` with the canonical public API, bodies taken verbatim from the current `discovery.rs` implementations where they exist (689 `strip_include_keys`, 715 `select_profile_sections`, 747 `merge_toml_values`) and new pure helpers otherwise:
   - `pub fn merge_toml_values(base: &mut toml::Value, overlay: &toml::Value)` — verbatim deep-merge (tables recursive, all other values replaced).
   - `pub fn section_walk(profiles: &[String]) -> Vec<String>` — `["default"]` followed by each selected non-default profile in selection order, deduplicated preserving first occurrence.
   - `pub fn include_declarations<'a>(value: &'a toml::Value, profiles: &[String]) -> Vec<(String, &'a toml::Value)>` — ordered collection: top-level `include` (label `""`) then each walked section's `include` (label = section name), skipping absent keys. The explicit `'a` output lifetime borrows from `value`.
   - `pub fn strip_include_keys(value: &mut toml::Value, profiles: &[String])` — verbatim.
   - `pub fn select_profile_sections(value: &mut toml::Value, profiles: &[String])` — verbatim.
   - `pub fn has_profile_structure(value: &toml::Value, profiles: &[String]) -> bool` — `[default]` present or any selected section present.
   - `pub fn has_selected_profile(value: &toml::Value, profiles: &[String]) -> bool` — any selected section present.
2. Module docs: state this is the single canonical implementation of the config-loader TOML semantics (bd rc-io2zl), consumed by camel-config's loader, camel-dsl's virtual-store assembly, and camel-cli `compile::sources`; consumer error policies stay local.
3. Unit tests in the module covering: deep-merge table recursion + array replacement; `section_walk` order, dedup, `default`-in-list handling; `include_declarations` order and labels; strip from top-level + walked sections only; selection base/overlay/replace semantics including empty profiles and flat documents; both predicates.

**Tests:**
- `merge_recurses_tables_and_replaces_arrays`: base `{a={x=1,y=2}, r=[1]}`, overlay `{a={y=9}, r=[2,3]}` → `{a={x=1,y=9}, r=[2,3]}`.
- `section_walk_orders_default_first_and_dedups`: `["b","default","a","b"]` → `["default","b","a"]`.
- `include_declarations_walk_order_and_labels`: doc with top-level + `[default]` + `[production]` include keys, profiles `["production"]` → labels `["", "default", "production"]` in order.
- `strip_include_keys_removes_from_all_walked_sections`: only top-level + `[default]` + selected sections lose `include`; `[other]` keeps it.
- `select_uses_default_base_else_first_selected`: no `[default]`, profiles `["p1","p2"]` with both present → base is `p1`, `p2` overlays.
- `select_flat_document_stays_as_is`: no `[default]`, no selected section → value unchanged.
- `select_empty_profiles_keeps_default_section`: profiles `[]` with `[default]` → root becomes `[default]` content.
- `has_profile_structure_and_selected_profile_predicates`: table combos → booleans per doc comment.
- Command: `cargo test -p camel-dsl config_semantics` (green).

**Acceptance:**
- `cargo test -p camel-dsl config_semantics` green.
- `cargo fmt --check` and `cargo clippy -p camel-dsl -- -D warnings` clean.
- `discovery.rs` NOT yet modified in this task (module is additive only).

- [x] 2.1

### camel-config

#### Task 2.2: camel-config delegates to canonical helpers

**Files:**
- `crates/camel-config/src/config.rs` (modified)

**Steps:**
1. Replace the body of `pub(crate) fn merge_toml_values` (1930) with a delegation call to `camel_dsl::config_semantics::merge_toml_values` (keep the wrapper signature; callers at 3568/3591 unchanged).
2. Rewrite `apply_profile` (3559): with `Some(p)`, if `config_semantics::has_selected_profile(value, &[p.to_string()])` then `config_semantics::select_profile_sections(value, &[p.to_string()])`, else return the existing `ConfigError::Message(format!("Unknown profile: {}", p))` unchanged; with `None`, `config_semantics::select_profile_sections(value, &[])`. Verify each branch against the original: both-present → merge; profile-only → replace; absent → same error; None → default-or-keep.
3. Rewrite `apply_profile_lenient` (3585) as `config_semantics::select_profile_sections(value, &profile.map(|p| vec![p.to_string()]).unwrap_or_default())` — verify branch-for-branch equivalence against the original match (including `(Some(default), None)` → default-only and `(None, None)` → keep).
4. Rewrite `extract_includes` (2643) to iterate `config_semantics::include_declarations(raw_value, &profiles)` — derive the slice first: `let profiles: Vec<String> = profile.map(|p| vec![p.to_string()]).unwrap_or_default();` (`extract_includes`' existing parameters: `raw_value` and the profile parameter at line 2643 — keep their names) — mapping labels to the existing error-text prefixes (`""` → the current top-level wording, section → the current `"{section}.include"` wording), and to strip via `config_semantics::strip_include_keys`; preserve every existing error string and the returned declaration order.
5. The `has_profile_structure` inline check at the strict/lenient dispatch (~2983) switches to `config_semantics::has_profile_structure`.
6. Do not touch the `clean_i64` SYNC comment (~3400).

**Tests:**
- All Phase 1 goldens must pass UNCHANGED (no regeneration): `cargo test -p camel-config parity_` green against the committed goldens.
- Existing suite green: `cargo test -p camel-config` passes fully.
- `unknown_profile_error_string_unchanged`: covered by `parity_unknown_profile_error_string_locked` (golden `case_03_error.txt` still byte-identical).
- Command: `cargo test -p camel-config` (full crate).
- Expected: green with zero golden diffs.

**Acceptance:**
- `cargo test -p camel-config` fully green (including the pre-existing `virtual_store_config_matches_filesystem_loader` and `virtual_store_config_tests` — they must stay green through every later task); `cargo test -p camel-config parity_` shows zero golden mismatches.
- No local body remains for `merge_toml_values` beyond the delegation call; `apply_profile`/`apply_profile_lenient` contain no selection logic beyond predicate + canonical call + error branch.
- `cargo fmt --check` and `cargo clippy -p camel-config -- -D warnings` clean.

- [x] 2.2

### camel-dsl

#### Task 2.3: discovery.rs consumes canonical helpers; SYNC comments die

**Files:**
- `crates/camel-dsl/src/discovery.rs` (modified)

**Steps:**
1. Delete the local `strip_include_keys` (689), `select_profile_sections` (715), `merge_toml_values` (747) and import them from `crate::config_semantics`.
2. In `build_virtual_config` (607): the unknown-profile structure check uses `config_semantics::has_profile_structure` / `has_selected_profile` with the identical condition (structure present AND profiles non-empty AND `[default]` present AND no selected present) and the byte-identical `MalformedVirtualConfig` message.
3. Remove the SYNC comments at 603, 711, 744; replace the module-level narrative on `build_virtual_config` with a pointer to `config_semantics` as the canonical implementation.
4. If any existing unit tests in `discovery.rs` reference the moved helpers, adjust import paths only, never assertions; if none exist (per the pre-step grep), record that.

**Tests:**
- `cargo test -p camel-dsl virtual_config_` green with zero golden diffs (Phase 1.2 goldens unchanged).
- `cargo test -p camel-dsl` fully green.
- Command: `cargo test -p camel-dsl`.
- Expected: green with zero golden mismatches.

**Acceptance:**
- `grep -c "SYNC" crates/camel-dsl/src/discovery.rs` returns 0.
- Zero behavioral diff: Phase 1.2 goldens byte-identical without regeneration.
- `cargo fmt --check` and `cargo clippy -p camel-dsl -- -D warnings` clean.

- [x] 2.3

### camel-cli

#### Task 2.4: compile::sources consumes canonical walk + declarations

**Files:**
- `crates/camel-cli/src/compile/sources.rs` (modified)

**Steps:**
1. Replace the manual ordered include-walk assembly (~550-570: `include_decls` collection over top-level + sections) with iteration over `camel_dsl::config_semantics::include_declarations(&config, &selection.profiles)`; map label `""` to the existing `"include"` error prefix and section labels to `"{section}.include"` via the existing `toml_string_list`; the collected order must be identical.
2. Replace the route-overlay walk's section list with `camel_dsl::config_semantics::section_walk(&selection.profiles)`; the replace-in-order loop structure and `toml_string_list` errors stay.
3. Keep the per-profile chain-wide unknown-profile validation VERBATIM (the loop validating every selected profile against the root config plus `include_tables`, each miss producing its own `UnknownProfile("<name>")`-carrying `SourceError` with today's exact Display text). Do not substitute `has_selected_profile` here.
4. Update the module doc lines that narrate "mirroring camel-config's ordered include walk" to reference `camel_dsl::config_semantics` as the single source of the walk order.

**Tests:**
- Phase 1.3 goldens pass UNCHANGED: `cargo test -p camel-cli --test config_compile_parity` green, zero golden diffs, including `compile_partial_profile_absence_error_locked`.
- Existing compile suites green: `cargo test -p camel-cli --test compile_command_test --test compiled_artifact_test`.
- Command: `cargo test -p camel-cli --test config_compile_parity --test compile_command_test --test compiled_artifact_test`.
- Expected: green with zero golden mismatches.

**Acceptance:**
- `cargo test -p camel-cli --test config_compile_parity --test compile_command_test --test compiled_artifact_test` green.
- `cargo test -p camel-cli --test feature_profiles` green, run 3 consecutive times (design.md stability gate).
- No hand-assembled section walk remains in `sources.rs` (grep for `vec!["default"]` / manual `"default"` push loops in the include/route walks returns nothing).
- `cargo fmt --check` and `cargo clippy -p camel-cli -- -D warnings` clean.

- [x] 2.4

## Phase 3: virtual_config.rs pure-move extraction

### camel-dsl

#### Task 3.1: Extract virtual config assembly into virtual_config.rs

**Files:**
- `crates/camel-dsl/src/virtual_config.rs` (new)
- `crates/camel-dsl/src/discovery.rs` (modified — assembly removed, import added)
- `crates/camel-dsl/src/lib.rs` (modified — add `pub(crate) mod virtual_config;`)

**Steps:**
1. First grep `discovery.rs` for unit tests referencing `build_virtual_config`, `classify_virtual_config`, `VirtualConfigRefs`, `virtual_config_text`, or `parse_virtual_config_toml` that PREDATE this change (reviewer found none in the tests mod at 1156 — if the grep confirms zero pre-existing ones, record that in the report and skip relocation; the Phase 1.2 parity-golden tests are NOT in scope for relocation — they stay in `discovery.rs` tests mod and switch to importing `crate::virtual_config::build_virtual_config`). Then move VERBATIM (no signature, doc-comment, or behavior edits) from `discovery.rs`: `struct VirtualConfigRefs`, `classify_virtual_config`, `build_virtual_config`, `virtual_config_text`, `parse_virtual_config_toml`, plus any pre-existing tests the grep found (relocated into a `mod tests` inside `virtual_config.rs`).
2. In `virtual_config.rs` import `VirtualDocumentStore`, `StoreEntryKind`, `StoreError` from `embedded_store`, `DiscoveryError` from the crate root, and the canonical helpers from `crate::config_semantics`. Items consumed by `discovery.rs` become `pub(crate)`.
3. `discovery.rs` calls `crate::virtual_config::build_virtual_config` (and `discover_virtual_store` keeps its public signature); `lib.rs` declares `pub(crate) mod virtual_config;`.
4. Verify the move is pure: no logic edits, no reformatting beyond what `cargo fmt` requires.

**Tests:**
- Phase 1.2 goldens pass UNCHANGED: `cargo test -p camel-dsl virtual_config_` green, zero golden diffs.
- Full crate: `cargo test -p camel-dsl` green (relocated unit tests included).
- Command: `cargo test -p camel-dsl`.
- Expected: green; test count unchanged from pre-move (relocated tests, if any, keep their names; the moved-code tests are the Phase 1.2 additions which stay put in discovery.rs).

**Acceptance:**
- `discovery.rs` no longer defines `build_virtual_config`/`classify_virtual_config`/`VirtualConfigRefs`/`virtual_config_text`/`parse_virtual_config_toml`.
- Zero behavioral diff: goldens byte-identical, no assertion modified.
- `cargo fmt --check` and `cargo clippy -p camel-dsl -- -D warnings` clean.

- [x] 3.1

## Phase 4: Ride-along A — nested-cargo color audit (rc-0omir)

### camel-cli

#### Task 4.1: Color-sensitivity audit + fixes for the 9 nested-cargo tests

**Files:**
- `crates/camel-cli/tests/common/mod.rs` (modified — only if the fix lands in the shared spawn helpers)
- `crates/camel-cli/tests/compile_command_test.rs` (modified if sensitive)
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified if sensitive)
- `crates/camel-cli/tests/job_one_shot_test.rs` (modified if sensitive)
- `crates/camel-cli/tests/job_signal_test.rs` (modified if sensitive)
- `crates/camel-cli/tests/lint_test_doc_skip.rs` (modified if sensitive)
- `crates/camel-cli/tests/new_test.rs` (modified if sensitive)
- `crates/camel-cli/tests/run_empty_discovery_test.rs` (modified if sensitive)
- `crates/camel-cli/tests/run_exec_guard_test.rs` (modified if sensitive)
- `crates/camel-cli/tests/run_signal_test.rs` (modified if sensitive)

**Steps:**
1. For each of the 9 tests, trace the spawn chain: which child processes are spawned (camel binary via `common::spawn_*`, direct `cargo` invocations, `sh`), which captured stream is parsed, and whether any parsed bytes can transit a process that colorizes under inherited `CARGO_TERM_COLOR=always` (cargo does; check whether the camel binary re-emits cargo child output to the parsed stream — trace the compile/run command implementations in `crates/camel-cli/src/` for cargo spawn + output relaying).
2. Classify each test: SENSITIVE (parses cargo-transit bytes) or IMMUNE (parses only camel-authored output, or parses nothing), with the concrete spawn+parse evidence per file.
3. For SENSITIVE tests apply the feature_profiles pattern (see `crates/camel-cli/tests/feature_profiles.rs` lines 9-95): pin `.env("CARGO_TERM_COLOR", "never")` on the cargo-spawning child and/or strip ANSI escapes at the parse site. Prefer fixing in `common/mod.rs` spawn helpers when all call sites route through them.
4. Produce the verdict table (test → verdict → evidence → fix applied or immunity reason) and return it in the worker report — it feeds the bd rc-0omir closure note.
5. Verify: run the full camel-cli test suite with `CARGO_TERM_COLOR=always` exported in the harness environment.

**Tests:**
- `suite_under_color_always`: `CARGO_TERM_COLOR=always cargo test -p camel-cli` exits 0 (all 9 audited tests included).
- Each SENSITIVE test passes with `CARGO_TERM_COLOR=always` explicitly set on its own process environment during the audit run.
- Expected: suite green under the exported color env.

**Acceptance:**
- `CARGO_TERM_COLOR=always cargo test -p camel-cli` exits 0.
- The worker report contains the 9-row verdict table with per-test evidence.
- `cargo fmt --check` and `cargo clippy -p camel-cli -- -D warnings` clean.

- [x] 4.1

## Phase 5: Ride-along B — CI compile-gate (rc-khjnb)

### CI

#### Task 5.1: Gated-test compile-gate step in ci.yml

**Files:**
- `.github/workflows/ci.yml` (modified)

**Steps:**
1. In the `full-tests-linux` job, insert a new step between "Reclaim disk before full workspace test" and "Test (full workspace)":
   - `name: Compile-gate gated integration test set`
   - `run: cargo check -p camel-test --features integration-tests --tests`
   - comment lines referencing bd rc-8g35d (compile-broken `master_kubernetes_test` invisible to CI) and rc-khjnb, noting compile-only (no Docker) and why plain `cargo test --workspace` skips `#![cfg(feature = "integration-tests")]` files.
2. Prove the command compiles the gated set locally in the worktree, including `master_kubernetes_test` — `cargo check` produces no test executable and plain output need not name targets, so prove coverage with:
   `cargo check -p camel-test --features integration-tests --tests --message-format=json | grep -c '"target":{"kind":\["test"\].*"name":"master_kubernetes_test"'` — assert at least one `compiler-artifact`/`check-finished` message carries `"name":"master_kubernetes_test"` (adjust the jq/grep shape to the actual JSON; the assertion, not the tooling, is the contract). The CI step command itself stays exactly `cargo check -p camel-test --features integration-tests --tests`.
3. Validate the YAML parses.

**Tests:**
- `gated_set_compiles_locally`: `cargo check -p camel-test --features integration-tests --tests` exits 0 in the worktree AND the `--message-format=json` run contains a message whose target name is `master_kubernetes_test` (compile coverage proven).
- `ci_yaml_still_parses`: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml'))"` exits 0.
- Expected: both pass.

**Acceptance:**
- The new step exists verbatim before "Test (full workspace)" with the rc-8g35d/rc-khjnb comment.
- `cargo check -p camel-test --features integration-tests --tests` exits 0 in the worktree.
- YAML parse check passes.

- [x] 5.1
