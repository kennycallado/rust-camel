# Tasks: cfgdrop2

## camel-config

### Task 1.1: root-key policy module + runtime guard rework (routes overlay, near-miss reject)

**Files:**
- `crates/camel-config/src/root_key_policy.rs` (new)
- `crates/camel-config/src/lib.rs` (modified — add `pub mod root_key_policy;`)
- `crates/camel-config/src/config.rs` (modified)
- `crates/camel-config/src/config_tests/profile_loading_tests.rs` (modified)
- `crates/camel-config/src/config_tests/parity_golden_tests.rs` (modified — case_08 regen note)
- `crates/camel-config/src/config_tests/parity_goldens/case_08_error.txt` (modified — regenerate)
- `crates/camel-config/README.md` (modified)
- `docs/src/configuration/schema.md` (modified)

**Steps:**
1. Create `root_key_policy.rs` with: `pub const ROOT_ROUTES_KEY: &str = "routes";`, `pub fn is_known_top_level_key(name: &str) -> bool` (delegates to a crate-internal copy-free import of `KNOWN_TOP_LEVEL_KEYS` — move the const from `config.rs` into `root_key_policy.rs` as `pub(crate) const KNOWN_TOP_LEVEL_KEYS` and keep the `config.rs` uses pointing at it), and `pub fn near_miss_root_table(name: &str) -> Option<&'static str>` returning `Some(target)` iff `name != any known key`, `name.len() >= 4`, and `levenshtein(name, target) <= 2` for the minimal-distance target in the length-≥ 8 subset of `KNOWN_TOP_LEVEL_KEYS` (`runtime_journal`, `idempotent_repo`, `drain_timeout_ms`, `watch_debounce_ms`, `observability`, `stream_caching`, `datasources`, `supervision`, `components`, `cache_repo`, `timeout_ms`, `log_level`, `languages`, `security`, `platform`). Implement a private `fn levenshtein(a: &str, b: &str) -> usize` (two-row DP, `char`-based). Include a `#[cfg(test)] mod tests` with matcher table tests (see Tests).
2. In `config.rs` `build_from_toml_value_inner`, `has_profile_structure` branch: change the discarded-key filter from `KNOWN_TOP_LEVEL_KEYS.contains(k)` to `k != root_key_policy::ROOT_ROUTES_KEY && root_key_policy::is_known_top_level_key(k)`. After collecting `discarded`, collect `near_miss: Vec<(String, &'static str)>` from root TABLE-valued keys where `!is_known_top_level_key(k)` and `near_miss_root_table(k)` is `Some(target)`. If `discarded` non-empty → existing error text (unchanged wording) naming only non-`routes` keys. Else if `near_miss` non-empty → new `ConfigError::Message`: `"top-level table(s) {names} would be silently discarded by profile selection: '{name}' looks like a misspelling of '{target}' — move the table under [default] (overlaid by the selected profile section), or remove the profile sections to use a flat document"` (one message naming every near-miss pair; exact wording may be tuned but MUST name the table, the target, and both accepted shapes).
3. Routes overlay: before `apply_profile(&mut config_value, profile)?`, if the root table has a `routes` key, `let root_routes = table.remove("routes");`. After `apply_profile` returns Ok, `if let Some(routes) = root_routes` and the selected `config_value` root table does NOT contain `routes`, reinsert `routes`. (Any walked section that declared `routes` already won via array replacement in `select_profile_sections`/`merge_toml_values`; reinsert only fires when no declarer exists.)
4. rc-cflo warn (the `profile.is_none() && contains_key("default")` block): add `&& root_key_policy::near_miss_root_table(k).is_none()` to the `profile_like` filter so near-miss names stop double-reporting (they hard-error in the guard; the warn block runs first, so also skip warn-listing any name that `is_known_top_level_key` — no change needed there, that filter already exists).
5. Regenerate `case_08_error.txt`: run `cargo test -p camel-config --lib parity_golden` with the golden-update path used by the battery (inspect `parity_golden_tests.rs` for the env-var or helper convention it uses to rewrite goldens; if none exists, update the file by hand from the actual error `Display`). The new text names only `timeout_ms, watch` (no `routes`).
6. Extend `crates/camel-config/README.md` and `docs/src/configuration/schema.md`: root `routes` beside profile sections is the documented exception with the overlay semantic; near-miss root tables are rejected naming the intended key.

**Tests:** (all in `crates/camel-config/src/config_tests/`)
- `root_routes_beside_default_survives_selection`: temp `Camel.toml` with root `routes = ["r/*.yaml"]` + `[default] log_level = "info"` → `CamelConfig::from_file_with_profile(path, None)` → Ok, `cfg.routes == ["r/*.yaml"]`, `cfg.log_level == "info"`. Command: `cargo test -p camel-config --lib root_routes_beside_default_survives_selection`. Expected: fails before step 3, passes after.
- `root_routes_replaced_by_default_section_routes`: root `routes = ["a/*.yaml"]` + `[default] routes = ["b/*.yaml"]` → load → `cfg.routes == ["b/*.yaml"]` (default declares, root replaced). Same command pattern. Expected: fails before, passes after.
- `root_routes_replaced_by_selected_profile_routes`: root `routes = ["a/*.yaml"]` + `[default] routes = ["b/*.yaml"]` + `[prod] routes = ["c/*.yaml"]` with `CAMEL_PROFILE=prod` (env_lock + restore guard) → load → `cfg.routes == ["c/*.yaml"]`. Expected: fails before, passes after.
- `root_routes_with_other_known_keys_error_names_only_those`: root `routes` + `timeout_ms = 5` + `watch = true` + `[default]` → error `Display` contains `timeout_ms` and `watch`, does NOT contain `routes` as a discarded key (assert `!msg.contains("routes,")` and `!msg.contains(", routes")`). Expected: fails before (message names routes), passes after.
- `near_miss_table_beside_selected_profile_is_rejected`: `[prod] log_level = "warn"` + root `[obsevrability] sampled = false` with `CAMEL_PROFILE=prod` → error contains `obsevrability`, `observability`, `misspelling`, `[default]`. Expected: fails before (silently dropped), passes after.
- `near_miss_table_without_profile_is_rejected_not_warned`: `[default] x = 1` + root `[runtime_jounal] path = "j.db"`, no profile → error contains `runtime_jounal` and `runtime_journal`; captured `tracing` subscriber (existing warn-test pattern in `config_ergonomics_tests.rs`) asserts no rc-cflo warn lists `runtime_jounal`. Expected: fails before, passes after.
- `far_table_beside_active_profile_stays_silent`: `[default] x = 1` + `[staging] y = 2` + `CAMEL_PROFILE=prod` requires `[prod]` present (add `[prod] z = 3`) → Ok; `cfg` loads; no error mentions `staging`. Expected: passes before AND after (negative lock — must not regress).
- `flat_document_near_miss_table_stays_lenient`: root `[obsevrability] sampled = false`, no profile structure → Ok (flat leniency, `_extra`). Expected: passes before AND after (negative lock).
- `parity_case_08_error_lock_regenerated`: existing golden test `parity_section_routes_replace_toplevel` continues to assert an error lock; golden text equals the regenerated `case_08_error.txt`. Expected: red until step 5, green after.
- Matcher unit tests in `root_key_policy.rs`: `near_miss_typo_matrix` (`obsevrability`→`observability`, `componets`→`components`, `runtime_jounal`→`runtime_journal`, `datasorces`→`datasources`, `supervison`→`supervision` all `Some`, each with correct target) and `near_miss_far_names_stay_none` (`staging`, `prod`, `qa`, `canary`, `eu_west`, `job`, `bind`, `dev` all `None`) and `near_miss_known_keys_stay_none` (`routes`, `watch`, `runtime_journal` exact → `None`) and `levenshtein_cases` (`kitten/sitting`=3, `abc/abc`=0, `"/""x"`=1). Command: `cargo test -p camel-config --lib root_key_policy`.

**Acceptance:**
- `cargo test -p camel-config --lib` exits 0 with ≥ 322 passing (311 baseline + new tests), 0 failed.
- `cargo fmt --check --all` clean; `cargo clippy -p camel-config --all-features -- -D warnings` exits 0.
- `cargo xtask lint-unwrap` and `cargo xtask lint-log-levels` exit 0.
- `grep -rn "routes" crates/camel-config/src/config_tests/parity_goldens/case_08_error.txt` shows the error names only `timeout_ms`/`watch`.

- [x] 1.1

## camel-cli

### Task 2.1: compile-side mirror guard + cross-path parity asserts

**Files:**
- `crates/camel-cli/src/compile/sources.rs` (modified)
- `crates/camel-cli/tests/config_compile_parity.rs` (modified — extend; add parity-matrix section)
- `crates/camel-cli/tests/config-parity-project/Camel.toml` and sibling `Camel*.toml` fixtures (modified only if the step-1 scan finds a fixture mixing root non-routes known keys with profile sections — convert to error lock and report)

**Steps:**
1. Scan every fixture under `crates/camel-cli/tests/config-parity-project/` (and any other fixture dirs referenced by `config_compile_parity.rs`): list root keys vs profile sections per file. Expectation per bd rc-3d76f: fixtures mix ONLY root `routes` with profile sections. If any fixture mixes a root non-`routes` known key or a near-miss table with profile sections, convert that fixture's expectation to a compile error lock (following the test file's existing error-lock convention) and note it in the task report — do NOT silently edit fixture semantics.
2. In `compile/sources.rs` after the config document parses and `policy::reject_config_assets` runs (around line 744), add a guard when `camel_dsl::config_semantics::has_profile_structure(&config, &selection.profiles)`: collect root keys where `camel_config::root_key_policy::is_known_top_level_key(k) && k != camel_config::root_key_policy::ROOT_ROUTES_KEY` → non-empty → return a new `SourceError` variant `RootKeysDiscarded(Vec<String>)` whose `Display` names each key and the accepted shapes (move under `[default]`/selected profile section, or use a flat document) — same class as the loader's message, compile-local wording. Then collect near-miss root tables via `camel_config::root_key_policy::near_miss_root_table(k)` over TABLE-valued unknown root keys → new variant `RootTableMisspelling(Vec<(String, String)>)` naming each table→target pair and the same accepted shapes. Root `routes` is NOT rejected (documented pattern accumulator, lines 766–781 untouched).
3. Wire the two variants into the existing `SourceError` `Display`/`From` impls following the enum's current style; keep messages deterministic (sorted key order, same separator style as sibling variants).
4. Extend `tests/config_compile_parity.rs` with the new tests below plus a parity-matrix block: for each fixture-string class (root known scalar + `[default]`; root known table + `[default]`; root `routes` + `[default]`; near-miss table + `[prod]`; far table `[staging]` + `[prod]`; flat doc), assert the compile guard's disposition EQUALS `CamelConfig::from_file_with_profile`'s disposition on the SAME document text (both error with the same key named, or both accept). camel-cli already depends on camel-config (workspace dep with `otel` feature) — reuse it in the test.

**Tests:** (in `crates/camel-cli/tests/config_compile_parity.rs`, same harness style as existing cases)
- `compile_rejects_root_known_scalar_beside_default`: fixture text root `timeout_ms = 5` + `[default] x = 1` with `--config` selection → resolution fails; error `Display` contains `timeout_ms` and `flat document`. Command: `cargo test -p camel-cli --test config_compile_parity compile_rejects_root_known_scalar_beside_default`. Expected: fails before step 2, passes after.
- `compile_rejects_root_known_table_beside_default`: root `[runtime_journal] path = "j.db"` + `[default]` → fails; error contains `runtime_journal`. Expected: fails before, passes after.
- `compile_accepts_root_routes_beside_default_unchanged`: existing fixture(s) mixing root `routes` + `[default]` still compile and the committed goldens stay byte-identical (this is the existing suite's own assertion — the new test asserts explicitly that resolution of a root-`routes`+`[default]` doc succeeds and the plan's route patterns start from the root list). Expected: passes before AND after (golden negative lock).
- `compile_rejects_near_miss_table_beside_selected_profile`: `[prod] x = 1` + root `[obsevrability] y = 2` with `--profile prod` → fails; error contains `obsevrability` and `observability`. Expected: fails before, passes after.
- `compile_accepts_far_table_beside_selected_profile`: `[prod] x = 1` + `[staging] y = 2` with `--profile prod` → resolution succeeds (unselected profile stays embedded). Expected: passes before AND after (negative lock).
- `compile_guard_needs_config_selection`: no `--config` → guard never runs; a route document beside a stray Camel.toml with root keys + `[default]` compiles untouched (assert existing no-config path unaffected). Expected: passes before AND after.
- `parity_matrix_loader_vs_compile_same_disposition`: the six-class matrix above; for each document string, `(loader_result.is_err(), error-names-key)` equals `(compile_result.is_err(), error-names-key)`. Expected: fails before (classes diverge), passes after.

**Acceptance:**
- `cargo test -p camel-cli --test config_compile_parity` exits 0 (existing goldens byte-identical + new tests green).
- `cargo clippy -p camel-cli -- -D warnings` exits 0; `cargo fmt --check --all` clean.
- The step-1 scan result is recorded in the task report (fixture list + disposition, including any converted fixture).
- Parity matrix test green: no fixture class where loader and compile disagree.

- [x] 2.1

<!-- Task 2.1 step-1 fixture-scan record (r_glm finding 1):
`crates/camel-cli/tests/fixtures/config-parity-project/` — Camel.toml,
Camel-partial.toml, Camel-strict.toml, Camel-mixed.toml: root keys are
`include` + `routes` only beside profile sections (mix ONLY root
`routes`). Camel-flat.toml: flat, no in-document profile section, guard
cannot fire. includes/*.toml: never guard inputs (guard runs on the
primary document only). Zero fixtures mix root non-`routes` known keys
or near-miss tables with profile structure; zero conversions needed;
all committed goldens stayed byte-identical. -->
