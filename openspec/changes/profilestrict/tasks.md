# Tasks: profilestrict

## Spec scenario → test map

Delta scenarios of the MODIFIED requirement (`specs/config-loader-semantics/spec.md`):

- "Compile enforces the strict unknown-profile mirror" → Task 1.1 `compile_include_only_profile_rejected` (new test).
- "Compile strict mirror defers to chain-wide absence" → Task 1.1 `compile_include_only_profile_defers_to_total_absence` (new test).
- "Mixed multi-profile selection stays acceptable" → Task 1.2 `compile_mixed_profile_selection_accepted` (new test).
- "Flat configurations keep lenient include profiles at compile time" → Task 1.2 `compile_flat_config_include_profile_accepted` (new test).

Inherited scenarios (carried verbatim by the MODIFIED requirement; exercised by existing tests, no new task):

- "camel-config delegates to the canonical helpers" → `parity_profile_deep_merge_and_array_replace`, `parity_ordered_includes_merge_order`, `parity_include_with_own_profile_sections` in `crates/camel-config/src/config_tests/parity_golden_tests.rs` (`cargo test -p camel-config --lib parity_`).
- "camel-cli compile walks the canonical section order" → existing parity goldens `compiled_artifact_resolved_config_production_golden` / `compiled_artifact_resolved_config_qa_golden` in `crates/camel-cli/tests/config_compile_parity.rs` (ordered includes carrying profile sections).
- "Strict unknown-profile predicate is canonical where the policy coincides" → `parity_unknown_profile_error_string_locked` in `crates/camel-config/src/config_tests/parity_golden_tests.rs` and `virtual_config_unknown_profile_backstop_error_locked` in `crates/camel-dsl/src/discovery.rs`.
- "Per-profile chain-wide compile validation is preserved" → existing `compile_partial_profile_absence_error_locked` in `crates/camel-cli/tests/config_compile_parity.rs` (golden `partial_absence_error.txt`).

## camel-cli compile::sources

### Task 1.1: Strict compile-time profile gate with `IncludeOnlyProfiles` error

**Files:**
- `crates/camel-cli/src/compile/sources.rs` (modified)
- `crates/camel-cli/tests/fixtures/config-parity-project/Camel-strict.toml` (new)
- `crates/camel-cli/tests/fixtures/config-parity-project/includes/strict-prod.toml` (new)
- `crates/camel-cli/tests/config_compile_parity.rs` (modified)
- `crates/camel-cli/tests/goldens/config-parity/include_only_profile_error.txt` (new, captured)
- `crates/camel-cli/tests/goldens/config-parity/include_only_then_absence_error.txt` (new, captured)
- `crates/camel-cli/tests/goldens/config-parity/no_profiles_resolved.toml` (new, captured)

**Steps:**
1. In `SourceError` (crates/camel-cli/src/compile/sources.rs), add a variant after `UnknownProfile(String)`:
   `IncludeOnlyProfiles { names: Vec<String> }` with a doc comment stating: selected profile sections exist only in includes while the configuration document carries `[default]`; the strict mirror of camel-config's `apply_profile` and the virtual-store `MalformedVirtualConfig` backstop rejects the selection at compile time.
2. Add the `Display` arm for `IncludeOnlyProfiles` with this exact text (names joined by `", "` in selection order):
   `profiles {names} exist only in includes: a configuration with a [default] section must declare at least one selected profile itself; move at least one selected profile section into the configuration document`
3. Call the predicate fully qualified by its path `camel_dsl::config_semantics::has_selected_profile`, matching the neighboring fully-qualified `camel_dsl::config_semantics::include_declarations` / `section_walk` call style — no new `use` import is needed.
4. In `resolve`, inside the `if let Some(config_path) = &selection.config_path` block: BEFORE the `for name in selected_profiles` fragment loop, snapshot the deduplicated selection order (`let selected_names: Vec<String> = selected_profiles.iter().map(|n| n.to_string()).collect();` — the loop consumes `selected_profiles`, and the gate payload needs the names afterwards). Then add the strict gate AFTER that loop (so per-profile chain-wide `UnknownProfile` keeps precedence) and BEFORE the route-pattern expansion:
   ```rust
   if camel_dsl::config_semantics::has_profile_structure(&config, &selection.profiles)
       && !selection.profiles.is_empty()
       && !camel_dsl::config_semantics::has_selected_profile(&config, &selection.profiles)
   {
       return Err(SourceError::IncludeOnlyProfiles {
           names: selected_names,
       });
   }
   ```
   The empty-profiles guard keeps every profile-less compile of a `[default]`-carrying config green (the runtime mirror guards the same at virtual_config.rs `!refs.profiles.is_empty()`); `has_profile_structure` is the canonical first conjunct (truth-equivalent to `contains_key("default")` under the third conjunct), so no local half-copy of the predicate exists. Comment names it the strict-at-compile mirror and notes the loop above already errored on chain-wide absence.
5. Create fixture `Camel-strict.toml` (content: top-level `include = ["includes/strict-prod.toml"]`, `routes = ["routes/*.yaml"]`, and a `[default]` section with `log_level = "info"` and `timeout_ms = 30000`; header comment explains the strict-mirror purpose).
6. Create fixture `includes/strict-prod.toml` (content: a `[prod]` section with `log_level = "warn"`; no `[default]`, no top-level keys).
7. Add the three integration tests and one unit test listed under Tests, run them RED where the plan predicts failure (gate absent: `compile_include_only_profile_rejected` fails because compile exits 0; the deferral and no-profile tests may already pass), then implement steps 1-4, run GREEN.
8. Capture the three new goldens with targeted invocations so frozen goldens are never rewritten: `UPDATE_GOLDENS=1 cargo test -p camel-cli --test config_compile_parity include_only`, then `UPDATE_GOLDENS=1 cargo test -p camel-cli --test config_compile_parity no_profiles_default_config` (test-name filters are substring matches). Then re-run the full suite WITHOUT the env var to verify byte-for-byte comparison, including the three pre-existing goldens.

**Tests:** (executable spec — name, arrange, act, assert)
- `compile_include_only_profile_rejected` (in crates/camel-cli/tests/config_compile_parity.rs): arrange `Camel-strict.toml` fixture where `[prod]` exists only in `includes/strict-prod.toml` and `[default]` is in the config; act `compile_with_profiles("Camel-strict.toml", &["prod"], &artifact)`; assert `!output.status.success()`, `!artifact.exists()`, and `lock_text_golden("include_only_profile_error.txt", &stderr_of(&output))` — expected stderr starts with `camel compile: profiles prod exist only in includes:` and contains `must declare at least one selected profile itself`. Command: `cargo test -p camel-cli --test config_compile_parity compile_include_only_profile_rejected`. Expected: fails before step 4 (compile exits 0), passes after.
- `compile_include_only_profile_defers_to_total_absence` (same file): arrange same fixture; act `compile_with_profiles("Camel-strict.toml", &["prod", "qa"], &artifact)`; assert `!output.status.success()`, `!artifact.exists()`, `lock_text_golden("include_only_then_absence_error.txt", &stderr_of(&output))` — expected stderr is exactly `camel compile: unknown profile 'qa': the selected profile section must exist in the explicit configuration` (the frozen per-profile form; `[qa]` exists nowhere). Command: `cargo test -p camel-cli --test config_compile_parity compile_include_only_profile_defers_to_total_absence`. Expected: passes before AND after the gate (locks precedence).
- `include_only_profiles_display_names_rule_and_remedy` (unit, in the sources.rs `#[cfg(test)]` mod): arrange `SourceError::IncludeOnlyProfiles { names: vec!["prod".into(), "canary".into()] }`; act `err.to_string()`; assert the string equals `profiles prod, canary exist only in includes: a configuration with a [default] section must declare at least one selected profile itself; move at least one selected profile section into the configuration document`. Command: `cargo test -p camel-cli --lib include_only_profiles_display`. Expected: fails before step 2, passes after.
- `compile_no_profiles_default_config_accepted` (in crates/camel-cli/tests/config_compile_parity.rs): arrange the existing `Camel.toml` fixture (carries `[default]`, `[production]`, `[qa]`); act `compile_with_profiles("Camel.toml", &[], &artifact)` — the helper's profile loop adds no `--profile` args for an empty slice; assert `output.status.success()`, `artifact.exists()`, and `lock_text_golden("no_profiles_resolved.toml", &resolved_config_toml(&decoded_store(&std::fs::read(&artifact).unwrap())))` — the no-profile resolved configuration (default-section selection only). Command: `cargo test -p camel-cli --test config_compile_parity compile_no_profiles_default_config_accepted`. Expected: passes before AND after the gate (locks the empty-profiles guard; a gate missing the guard fails this test).

**Acceptance:**
- `cargo test -p camel-cli --test config_compile_parity` exits 0 (all tests, including the three pre-existing goldens `production.toml`, `qa.toml`, `partial_absence_error.txt` compared byte-identical — no regeneration of pre-existing goldens).
- `cargo test -p camel-cli --lib` exits 0.
- `grep -c 'has_selected_profile' crates/camel-cli/src/compile/sources.rs` shows the delegated predicate call (no local re-implementation of the presence check).
- `cargo fmt --check` and `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 1.1

### Task 1.2: Regression locks for unchanged acceptance regions (mixed and flat)

**Files:**
- `crates/camel-cli/tests/fixtures/config-parity-project/Camel-mixed.toml` (new)
- `crates/camel-cli/tests/fixtures/config-parity-project/includes/mixed-canary.toml` (new)
- `crates/camel-cli/tests/fixtures/config-parity-project/Camel-flat.toml` (new)
- `crates/camel-cli/tests/fixtures/config-parity-project/includes/flat-prod.toml` (new)
- `crates/camel-cli/tests/config_compile_parity.rs` (modified)
- `crates/camel-cli/tests/goldens/config-parity/mixed_resolved.toml` (new, captured)
- `crates/camel-cli/tests/goldens/config-parity/flat_include_profile_resolved.toml` (new, captured)

**Steps:**
1. Create fixture `Camel-mixed.toml` (content: top-level `include = ["includes/mixed-canary.toml"]`, `routes = ["routes/*.yaml"]`, a `[default]` section with `log_level = "info"` and `timeout_ms = 30000`, and a `[prod]` section with `log_level = "warn"`; header comment names the mixed multi-profile acceptance case).
2. Create fixture `includes/mixed-canary.toml` (content: a `[canary]` section with `log_level = "debug"` and `watch = true`; nothing else).
3. Create fixture `Camel-flat.toml` (content: top-level `include = ["includes/flat-prod.toml"]`, `routes = ["routes/*.yaml"]`, and flat keys `log_level = "info"` and `timeout_ms = 30000`; NO `[default]` and NO profile section; header comment names the lenient include-profile case).
4. Create fixture `includes/flat-prod.toml` (content: a `[prod]` section with `log_level = "warn"`; nothing else).
5. Add the two integration tests listed under Tests; both must pass WITHOUT any further implementation change (Task 1.1's gate accepts both inputs; runtime already boots them).
6. Capture the two resolved-config goldens with targeted invocations so frozen goldens are never rewritten: `UPDATE_GOLDENS=1 cargo test -p camel-cli --test config_compile_parity mixed_profile_selection`, then `UPDATE_GOLDENS=1 cargo test -p camel-cli --test config_compile_parity flat_config_include`. Then re-run the full suite WITHOUT the env var.

**Tests:** (executable spec — name, arrange, act, assert)
- `compile_mixed_profile_selection_accepted` (in crates/camel-cli/tests/config_compile_parity.rs): arrange `Camel-mixed.toml` where `[prod]` is in the config and `[canary]` only in `includes/mixed-canary.toml`; act `compile_with_profiles("Camel-mixed.toml", &["prod", "canary"], &artifact)`; assert `output.status.success()`, `artifact.exists()`, and `lock_text_golden("mixed_resolved.toml", &resolved_config_toml(&decoded_store(&std::fs::read(&artifact).unwrap())))` — expected resolved configuration carries the `[default]` base with canary's surviving keys (`watch = true`); config-declared sections win over include-derived fragments per include-below-config precedence (`log_level` ends as `warn` from `[prod]`, matching the frozen `production.toml` layering). Command: `cargo test -p camel-cli --test config_compile_parity compile_mixed_profile_selection_accepted`. Expected: passes immediately (regression lock).
- `compile_flat_config_include_profile_accepted` (same file): arrange `Camel-flat.toml` with no profile structure and `[prod]` only in `includes/flat-prod.toml`; act `compile_with_profiles("Camel-flat.toml", &["prod"], &artifact)`; assert `output.status.success()`, `artifact.exists()`, and `lock_text_golden("flat_include_profile_resolved.toml", &resolved_config_toml(&decoded_store(&std::fs::read(&artifact).unwrap())))` — expected resolved configuration carries the flat keys with the `[prod]` overlay (`log_level = "warn"`, `timeout_ms = 30000`), proving the lenient path. Command: `cargo test -p camel-cli --test config_compile_parity compile_flat_config_include_profile_accepted`. Expected: passes immediately (regression lock).

**Acceptance:**
- `cargo test -p camel-cli --test config_compile_parity` exits 0 with all eight tests green: three pre-existing golden tests (`production.toml`, `qa.toml`, `partial_absence_error.txt` compared byte-identical — no regeneration), three added by Task 1.1 (including the no-profile regression lock), two added by Task 1.2.
- Pre-existing goldens `production.toml`, `qa.toml`, `partial_absence_error.txt` remain byte-identical (no regeneration).
- `cargo fmt --check` and `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 1.2
