# Tasks: env-int-placeholder-typing

## Phase 1: DSL YAML arm

### camel-dsl: provenance + probe

#### Task 1.1: Provenance-tracking interpolation variant

**Files:**
- `crates/camel-dsl/src/env_interpolation.rs` (modified)

**Steps:**
0. Refresh the module-level typing canon comment (env_interpolation.rs:11-27 region) and the `interpolate_yaml_source` doc (~:94-99): they state substituted leaves keep STRING typing unconditionally — reword to "keep STRING typing at the interpolation seam; integer positions are coerced later by the loader-layer typed probe (env_int_probe), which this module feeds via provenance". No behavior change in this step.
1. Add `pub(crate) enum ProvenanceSeg { Key(String), Index(usize) }` and `pub(crate) type ProvenancePath = Vec<ProvenanceSeg>;` in `env_interpolation.rs`.
2. Add `pub(crate) fn interpolate_env_tree_with_provenance(raw: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<(String, Vec<ProvenancePath>), TreeInterpolateError>`: same parse/walk/serialize as `interpolate_env_tree`, but the scalar arm records the path of every leaf whose authored text `is_whole_scalar_env_token(s)` (new helper, see step 3) at the moment it substitutes. Keys and embedded-token leaves are NOT recorded.
3. Add `pub(crate) fn is_whole_scalar_env_token(s: &str) -> bool`: true iff the ENTIRE scalar is exactly one UNESCAPED `${env:NAME}` or `${env:NAME:-default}` token (a single `env_regex()` match spanning the whole string, not preceded by the `$` escape). Escaped forms (`$${env:...}`) and `$$` are never provenance — they are literal text, not substitutions.
4. Rewrite `interpolate_env_tree` as a delegate: `interpolate_env_tree_with_provenance(raw, lookup).map(|(doc, _)| doc)`.
5. Add `pub(crate) fn interpolate_yaml_source_with_provenance(raw: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<(String, Option<Vec<ProvenancePath>>), String>`: mirrors `interpolate_yaml_source` (same `Err(var_name)` surface, same fallback trigger); returns `None` for provenance when the legacy fallback path ran, `Some(paths)` otherwise. `interpolate_yaml_source` delegates and discards.

**Tests:** (inline `mod tests` in the same file; `cargo test -p camel-dsl --lib`)
- `provenance_records_whole_scalar_leaf`: source `routes:\n- id: r\n  steps:\n  - throttle:\n      max_requests: ${env:A:-2}\n      period_ms: 7\n` with lookup resolving `A` → result `Ok((doc, Some(paths)))` where `paths == vec![vec![Key("routes"), Index(0), Key("steps"), Index(0), Key("throttle"), Key("max_requests")]]` (period_ms absent, key positions absent).
- `provenance_excludes_embedded_and_keys`: source `k${env:A:-1}: v-${env:B:-2}\n` → `paths` empty.
- `provenance_excludes_escapes`: source `a: $${env:A:-2}\nb: ${env:B:-3}\n` → `paths` contains ONLY B's path (the escaped leaf is literal text).
- `fallback_yields_none_provenance`: a document with a tagged node `!!tag x` plus a whole-scalar token → `interpolate_yaml_source_with_provenance` returns legacy-spliced text with `None` provenance.
- `interpolate_yaml_source_behavior_unchanged`: existing tests in the module pass without modification (regression pin).

**Scenario ownership pins (existing tests stay green, unmodified):** `comment_placeholder_does_not_fail` (comments scenario), `quoted_hash_survives_interpolation`, `block_scalar_interpolates_as_value`, `numeric_leaf_stays_string_after_interpolation` (seam-level string-keeping), `tagged_node_falls_back_to_legacy` (fallback trigger), `escape_full_form_yields_literal` (escape grammar).

**Acceptance:**
- `cargo test -p camel-dsl --lib env_interpolation` passes.
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.

- [x] 1.1

#### Task 1.2: Typed probe over candidate subsets

**Files:**
- `crates/camel-dsl/src/env_int_probe.rs` (new)
- `crates/camel-dsl/src/lib.rs` (modified — add `mod env_int_probe;`)
- `crates/camel-dsl/Cargo.toml` (modified — add `tracing-subscriber.workspace = true` to `[dev-dependencies]`; it is absent today and the logging tests require it)

**Steps:**
1. Define `pub(crate) fn clean_integer(s: &str) -> Option<serde_yml::Value>` in `env_int_probe.rs`: trim nothing; require the lexical form `-?(0|[1-9][0-9]*)` via a hand-rolled check (first char `-` optional, then `0` alone or `[1-9]` followed by ASCII digits); return `Some(serde_yml::Value::Number)` built from `s.parse::<i64>()` else `s.parse::<u64>()`; `None` otherwise (leading zeros, floats, `1e3`, overflow past u64, empty).
2. Define `fn candidates(tree: &serde_yml::Value, provenance: &[ProvenancePath]) -> Vec<ProvenancePath>`: filters provenance paths that (a) start with `ProvenanceSeg::Key("routes")`, (b) resolve to a `Value::String` whose text `clean_integer` accepts. Document order preserved.
3. Define `pub(crate) fn parse_with_probe<P>(doc_text: &str, provenance: Option<&[ProvenancePath]>, parse: P) -> Result<Vec<camel_core::route::RouteDefinition>, camel_core::error::CamelError> where P: Fn(&str) -> Result<Vec<RouteDefinition>, CamelError>`:
   - `parse` MUST be a QUIET parser for this seam: speculative probe attempts are expected failures and MUST NOT emit `error!`/`warn!` logs. The quiet closure composes the two PURE helpers of Task 1.3 step 0 (`deserialize_route_doc` + `lower_declarative_to_routes`) with no logging; the logging wrapper (`parse_yaml`) logs only its from_str-stage failure exactly as today. The CALLER emits the existing single `error!` exactly once, only after the probe returns the final failure (replay per Task 1.3 step 1).
   - Run `parse(doc_text)`. On `Ok`, return.
   - On `Err(e)`: if provenance is `None` or the candidate list is empty or longer than 8, return `Err(e)`.
   - Enumerate candidate index subsets by ascending size (1..=k), each size in lexicographic document order; for each subset: parse `doc_text` to `serde_yml::Value` fresh, set each candidate path's leaf to its `clean_integer` number, serialize with the crate's YAML shim, run `parse` on the serialized text; first `Ok` wins and returns.
   - All failed → return the ORIGINAL `Err(e)`.
4. Guard: the function performs no environment reads and no `unwrap` (use `expect` only where infallible by construction, or propagate errors).

**Tests:** (inline `mod tests` in `env_int_probe.rs`; build route docs and use `crate::yaml::parse_yaml_for_probe` as `parse`)
- `probe_fixes_single_int_position`: doc with `throttle.max_requests: ${env:L:-2}` (interpolated pass-1 text fails) → probe returns routes with `max_requests == 2`.
- `probe_minimal_subset_skips_polymorphic`: doc with `set_header` value `${env:H:-123}` AND `throttle.max_requests: ${env:L:-2}` → routes load, header value string `"123"`, `max_requests == 2` (both orders: header-first and throttle-first variants of the same test).
- `probe_rejects_negative_at_unsigned`: `max_requests: -2` coerced copy fails; returns pass-1 error string.
- `probe_rejects_leading_zero_and_overflow`: `007` and `99999999999999999999999` are not candidates → pass-1 error.
- `probe_cap_nine_candidates_fails`: doc with nine int-position placeholders → pass-1 error; companion `probe_cap_eight_candidates_succeeds` with eight → loads.

**Tests:** add `probe_attempts_log_nothing`: call the REAL seam `parse_routes_with_env` (Task 1.3) on a probe-fixable doc under a `tracing_subscriber` fmt layer with a counting writer → zero `ERROR`-level events during the successful probe load; a companion all-attempts-fail doc through `parse_routes_with_env` emits exactly ONE error event (the final-failure replay through the logging parser). Add `probe_later_conversion_failure_logs_nothing_extra`: a doc that survives `serde_yml::from_str` but fails a later conversion stage (e.g. an invalid step shape that errors after from_str) under the counting subscriber → exactly the same error events the direct `parse_yaml` call produces on that doc (no probe-attempt amplification; non-`from_str` failures gain no logging).

**Acceptance:**
- `cargo test -p camel-dsl --lib env_int_probe` passes.
- `cargo clippy -p camel-dsl -- -D warnings` exits 0; `cargo xtask lint-unwrap` and `cargo xtask lint-log-levels` clean.

- [x] 1.2

#### Task 1.3: Public seam + loader and discovery wiring

**Files:**
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/src/discovery.rs` (modified)
- `crates/camel-dsl/src/lib.rs` (modified — root re-export of `parse_routes_with_env`)

**Steps:**
0. In `yaml.rs`, split the parse chain with a TYPED staged error so logging scope is dispatchable by type, not message: `pub(crate) enum DeserializeStageError { FromStr(serde_yml::Error), Shape(CamelError) }`; `pub(crate) fn deserialize_route_doc(yaml: &str) -> Result<Vec<DeclarativeRoute>, DeserializeStageError>` (the `serde_yml::from_str` stage maps to `FromStr`, the document-shape stage that follows maps to `Shape`); `pub(crate) fn lower_declarative_to_routes(routes: Vec<DeclarativeRoute>) -> Result<Vec<RouteDefinition>, CamelError>` (conversion/lowering stage, logging-free). Re-express the EXISTING `parse_yaml_to_declarative_inner` (yaml.rs:109-114) as: call `deserialize_route_doc`, log its existing `error!` ONLY on `DeserializeStageError::FromStr` (byte-identical behavior and logging scope — verify against the current code that the shape stage logs nothing today), map both variants into the error type it returns now. Build two thin wrappers over the same helpers: `pub(crate) fn parse_yaml_for_probe(yaml: &str) -> Result<Vec<RouteDefinition>, CamelError>` = deserialize + lower + `annotate_format(InputFormat::Yaml, ...)` (matching `parse_yaml`'s public contract, since probe results flow to the same callers), zero logging. `parse_yaml_inner` STAYS UNANNOTATED exactly as today (yaml.rs:160-164: deserialize+log-stage + `compile_declarative_route` lowering, no annotate); `parse_yaml` keeps `annotate_format(InputFormat::Yaml, parse_yaml_inner(yaml))` (yaml.rs:156-158) unchanged. The wrappers compose identical stage helpers so they cannot drift. The quiet DISCOVERY twin shares ONLY `deserialize_route_doc` — its lowering stays the threshold/security-aware conversion of `parse_yaml_with_threshold_and_security` (it must NOT reuse `lower_declarative_to_routes`, which carries no threshold/security semantics), likewise quiet and logging variants composed from the same pieces.
1. In `yaml.rs`, add `pub enum RoutesEnvError { Unresolved(String), Parse(CamelError) }` (derive Debug; `impl fmt::Display`: `Unresolved(var)` → `Environment variable '{var}' not set`, `Parse(e)` → the inner error's message). Add `pub fn parse_routes_with_env(raw: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<Vec<RouteDefinition>, RoutesEnvError>`: calls `interpolate_yaml_source_with_provenance(raw, lookup)`; `Err(var)` → `RoutesEnvError::Unresolved(var)`; `Ok((text, prov))` → `env_int_probe::parse_with_probe(&text, prov.as_deref(), crate::yaml::parse_yaml_for_probe)` mapping parse failure to `RoutesEnvError::Parse`. Callers distinguish the two cases BY TYPE — no message inspection. On FINAL probe failure the seam re-runs the original text through the logging `parse_yaml` exactly once (preserving today's error log for genuinely broken documents) and returns the ORIGINAL annotated first-pass error — the replay is for logging only; the returned error is the same object the caller would have seen before this change.
2. Rewrite `load_from_file_with_env` (yaml.rs:2086) to call `parse_routes_with_env(&content, lookup)`; on `Unresolved(var)` it returns `CamelError::RouteError(format!("Environment variable '{var}' not set (required by {})", path.display()))` (byte-for-byte today's wording, yaml.rs:2096-2100); on `Parse(e)` the annotated-error postlude at yaml.rs:2101+ applies to `e` exactly as today.
2b. Re-export `parse_routes_with_env` and `RoutesEnvError` from `crates/camel-dsl/src/lib.rs` alongside the existing `interpolate_yaml_source` re-export.
2c. Refresh the stale `load_from_file_with_env` rustdoc (yaml.rs:2077-2085): it describes tree-walk-first interpolation with string-kept leaves only — add the loader-layer typed-probe sentence (integer positions coerce after a failed typed parse via provenance; string/polymorphic positions untouched; cap 8; routes-only scope).
3. In `discovery.rs`, change the YAML arm of `interpolate_for_parse` (discovery.rs:284-292) to return `(String, Option<Vec<ProvenancePath>>)` by delegating to `interpolate_yaml_source_with_provenance`; the JSON/other arms return `(text, None)`. Update the caller at discovery.rs:367-390: pass the provenance into a `parse_with_probe` call wrapping a QUIET twin of `parse_yaml_with_threshold_and_security` (same parsing, no failure-path logging) for the YAML route arm; on final probe failure, re-run the original text through the LOGGING threshold/security parser exactly once so today's error log is preserved, then map through discovery's existing error path. The template arm (discovery.rs:394) keeps `&content` with NO probe (templates out of scope).
4. Keep `interpolate_yaml_source` exported and unchanged in behavior (it delegates per Task 1.1 step 5).

**Tests:** (inline test mods in `yaml.rs` and `discovery.rs`; `cargo test -p camel-dsl --lib`)
- `load_from_file_with_env_int_default` (yaml.rs): temp route file with `throttle: {max_requests: ${env:DWD_WARM_MAX_REQUESTS:-2}}`, lookup `|_| None` → `Ok`, first route's throttle step carries `2`. Assert via existing `assert_set_header_value`-style helper or step match on `BuilderStep`.
- `load_from_file_with_env_int_lookup_five` (yaml.rs): same file, lookup returns `Some("5")` for `DWD_WARM_MAX_REQUESTS` → `5`.
- `discovery_int_placeholder_loads` (discovery.rs): `.yaml` file in a temp dir, `discover_routes_with_threshold_security_and_env` with lookup `Some("5")` → route compiles with `max_requests == 5`.
- `discovery_int_placeholder_default` (discovery.rs): same, lookup `None` → `max_requests == 2`.
- `unresolved_int_position_names_variable` (yaml.rs): `max_requests: ${env:NOPE}` no default, lookup `None` → error text contains `NOPE` and mirrors `DiscoveryError::Env` wording (same assertion shape as the existing unresolved test).
- `embedded_token_int_position_fails` (yaml.rs): `throttle: {max_requests: p-${env:RC_N:-2}}` → pass-1 type error (embedded tokens are never candidates).
- `notanumber_int_position_fails` (yaml.rs): `throttle: {max_requests: ${env:RC_J:-notanumber}}` → pass-1 type error.
- `json_int_placeholder_still_fails` (discovery.rs): `.json` route file with string `"${env:J:-2}"` at `max_requests` → discovery error (non-goal pin).

**Scenario ownership pins (existing tests stay green, unmodified):** `load_from_file_ambient_env_ignored_by_default_path` (ambient-ignored scenario), `load_from_file_with_env_injection_point` (explicit-lookup scenario), `load_from_file_commented_no_default_placeholder_harmless` (commented scenario), `load_from_file_roundtrip_fragile_fails_with_parse_error_not_env_error` (fallback scenario), `discovers_route_with_env_var_in_uri_yaml`, `discover_fails_when_env_var_missing_yaml` (process-env-not-consulted + unresolved wording), `discovers_yaml_template_and_materializes` (templates scenario), `json_env_interpolation_with_valid_value_succeeds` (JSON arm).

**Acceptance:**
- `cargo test -p camel-dsl --lib` passes (all prior loader/discovery tests green).
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.

- [x] 1.3

## Phase 2: camel-config TOML arm

### camel-config: provenance + probe at try_into

#### Task 2.1: Provenance set in the resolver

**Files:**
- `crates/camel-config/src/config.rs` (modified)

**Steps:**
1. Add `pub(crate) enum ConfigSeg { Key(String), Index(usize) }` and `pub(crate) struct ProvenanceSet(Vec<Vec<ConfigSeg>>)` — STRUCTURAL segments, not dotted strings (a key containing `.` or `[0]` must resolve unambiguously). Navigation: `fn leaf_mut<'a>(root: &'a mut toml::Value, segs: &[ConfigSeg]) -> Option<&'a mut toml::Value>` walking tables/arrays. Rendering for diagnostics reuses the existing dot-join format only for display.
2. Refactor `resolve_tree_walk` (config.rs:3027) to carry `&mut Vec<Vec<ConfigSeg>>` provenance accumulator alongside the existing display `path` string: when a `toml::Value::String` leaf contains `${env:` or `$$` token text (reuse the token detection the plain/strict resolvers already key on), push its structural segment list BEFORE resolving. The STRICT_PREFIXES dispatch must derive its top-level segment from the FIRST `ConfigSeg::Key` of the structural path (never from `path.split('.')` — a key containing a dot must not misdispatch). Resolution behavior and error paths byte-identical.
3. Add `pub(crate) fn resolve_tree_with_provenance(root: &mut toml::Value, lookup: &dyn Fn(&str) -> Option<String>) -> Result<ProvenanceSet, ConfigError>`; `resolve_tree_with` (config.rs:3015) delegates and discards. Public signatures unchanged.
4. At the config build site that calls `resolve_tree_with(&mut merged_tree, lookup)` (config.rs:2936), switch to `resolve_tree_with_provenance` and carry the set to the `try_into` site (config.rs:2942).

**Tests:** (inline unit tests near the existing resolver tests; `cargo test -p camel-dsl` unaffected)
- `provenance_set_records_token_leaves`: tree `{timeout_ms: "${env:T:-1}", log_level: "debug"}` → set contains exactly `["timeout_ms"]`.
- `resolver_behavior_unchanged`: existing resolver/strict/plain tests pass unmodified.
- `provenance_segments_navigate_keys_with_dots`: tree `{a: {b.c: "${env:X:-1}"}}` (key literally containing a dot) → set holds segments `[Key("a"), Key("b.c")]`; `leaf_mut` resolves it; display renders `a.b.c`.

**Acceptance:**
- `cargo test -p camel-config --lib` passes.
- `cargo clippy -p camel-config -- -D warnings` exits 0.

- [x] 2.1

#### Task 2.2: Probe subsets at the deserialize boundary

**Files:**
- `crates/camel-config/src/config.rs` (modified)

**Steps:**
1. Add `fn clean_i64(s: &str) -> Option<i64>` (same lexical rule as Task 1.2 step 1; i64 only).
2. Add `fn deserialize_with_probe(merged_tree: &toml::Value, provenance: &ProvenanceSet) -> Result<CamelConfig, ConfigError>` at the config.rs:2942 site: first `merged_tree.clone().try_into()` mapping err to `ConfigError::Message(format!("Failed to deserialize merged config: {e}"))` (today's wrapper); on `Err(first_err)`: candidates = provenance paths resolving to `Value::String` leaves with `clean_i64` values (document order; more than 8 or empty → return `first_err`); enumerate candidate index subsets ascending size, coerce to `toml::Value::Integer` in cloned trees, `try_into` each; first `Ok` wins; all fail → `first_err`.
3. Normalize-empty-topology and later steps (config.rs:2945+) run on the WINNING `CamelConfig` exactly as today.

**Tests:** (`crates/camel-config/tests/placeholder_e2e.rs` extended; use existing `common::{env_lock, write_main, load}` helpers)
- `placeholder_int_root_field_coerces_default`: `timeout_ms = "${env:CFG_TIMEOUT_MS:-8000}"`, var unset → `Ok`, `config.timeout_ms == 8000`.
- `placeholder_int_root_field_coerces_env_value`: same file, `CFG_TIMEOUT_MS=9000` set via env_lock → `9000`.
- `placeholder_int_field_leading_zero_rejected`: `timeout_ms = "${env:CFG_T:-007}"` → `Err` mentioning `timeout_ms` or the type.
- `placeholder_int_field_overflow_rejected`: `timeout_ms = "${env:CFG_T:-9223372036854775808}"` → `Err`.
- `placeholder_int_field_notanumber_rejected`: `timeout_ms = "${env:CFG_T:-notanumber}"` → `Err` naming the field.
- `string_field_numeric_default_stays_string`: a string field `log_level = "${env:CFG_LL:-8080}"` (and NO failing int field in the same file) → `Ok` with `log_level == "8080"` (pass-1 wins).
- `quoted_numeric_root_field_is_rejected_after_materialization` (EXISTING, line 183): stays green unmodified — the literal carries no token.
- `token_bearing_override_coerces` (in the overrides test module, mirroring existing override tests): file sets `cache_repo` int field indirectly or root `timeout_ms`; env `CAMEL_...` allowlisted override carrying `${env:CFG_N:-8}` with `CFG_N` unset → effective `8`.
- `token_free_override_notanumber_rejected` (EXISTING behavior): override `=notanumber` → typed error (extend the existing override test if one exists, else add).
- `probe_cap_nine_config_candidates_fails`: a test TOML with nine distinct int fields each carrying `"${env:CFG_i:-N}"` → first-pass error; companion with eight fields loads.
- `unresolved_no_default_placeholder_names_variable`: `timeout_ms = "${env:CFG_MISSING}"` with the var unset → `Err` whose message contains `CFG_MISSING` (existing error surface).

**Acceptance:**
- `cargo test -p camel-config` passes (full suite).
- `cargo clippy -p camel-config -- -D warnings` exits 0.

- [x] 2.2

## Phase 3: lint mirror, camel-cli wiring, canon

### camel-lint: typing-mirror carve-out

#### Task 3.1: R-SCHEMA integer-position carve-out

**Files:**
- `crates/camel-lint/src/rules/rschema.rs` (modified)
- `crates/camel-lint/src/rules/rschema/tests.rs` (modified)
- `crates/camel-lsp/tests/lsp_session.rs` (modified)

**Steps:**
1. In the typing-mirror walk (`enforce_typing_mirror`, rschema.rs:335+), before emitting the STRING-typed validation copy for a whole-scalar with-default token whose default `clean_integer_like(d)` accepts (i64-or-u64 parse; new local helper mirroring camel-dsl's rule, SYNC-annotated), ALSO build a variant validation copy where that leaf is the NUMBER. Validate both copies against `ROUTE_SCHEMA` as the rule already does for the string copy.
2. Selection rule per leaf: if the string copy produces a schema type Error anchored at that leaf's instance path AND the number copy produces no error for that path, use the number copy (emit NO diagnostic for the leaf, no Info note — the default was not kept as a string). Otherwise keep today's behavior exactly (STRING copy; Info note for valid string positions; Error for bool positions, non-integer defaults, no-default tokens).
3. Update the three inline tests that pin the old Error (rschema/tests.rs near lines 345, 499, 575): int-position with-default now yields NO diagnostic; the mixed-doc test yields Info for the string leaf and nothing for the int leaf. Add `int_position_notanumber_still_error` and `int_position_no_default_still_error` if not already covered by the line-575 test's shape.
4. Audit `crates/camel-lint/src/env_interpolation.rs` SYNC mirror: extend the `// SYNC:` comment to cover the clean-integer rule; no behavioral change needed there if the rule lives in rschema.rs.

**Tests:** (`cargo test -p camel-lint` and `cargo test -p camel-lsp --test lsp_session`)
- `rschema_int_position_clean_default_no_diagnostic` (flipped line-345 test): `throttle.max_requests: ${env:MY_LIMIT:-2}` → zero R-SCHEMA diagnostics for the leaf.
- `placeholder_int_field_publishes_no_error` (lsp_session.rs, new async test mirroring `placeholder_string_field_publishes_info_not_error` at lsp_session.rs:821): open a doc with `throttle.max_requests: ${env:MY_LIMIT:-2}` → session publishes NO error diagnostic for the field. The existing `placeholder_string_field_publishes_info_not_error` stays green unmodified.
- `rschema_mixed_document_per_token` (flipped line-499 test): Info note for the string leaf; no diagnostic for the int leaf.
- `rschema_no_default_int_position_still_error` (line-575 shape): `${env:NO_DEFAULT_xyz}` at `max_requests` → Error.
- All other rschema tests pass unmodified (bool positions, escapes, comments, env-hermeticity).

**Acceptance:**
- `cargo test -p camel-lint` passes.
- `cargo clippy -p camel-lint -- -D warnings` exits 0.

- [x] 3.1

#### Task 3.2: camel-cli runner seam, baselines, driver tests

**Files:**
- `crates/camel-cli/src/commands/test/runner.rs` (modified)
- `crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron` (modified)
- `crates/camel-cli/src/commands/test/driver_tests.rs` (modified)

**Steps:**
1. In `runner.rs` inline branch (runner.rs:205-211), replace the `interpolate_yaml_source(&text, lookup)` + `parse_yaml(&interpolated)` pair with `camel_dsl::parse_routes_with_env(&text, lookup)`: `Err(RoutesEnvError::Unresolved(var))` → `format!("Environment variable '{var}' not set (required by inline routes)")` (today's exact wording); `Err(RoutesEnvError::Parse(e))` → `format!("inline routes: {e}")` (today's exact prefix); `Ok(defs)` → `Ok(defs)`. The typed variant makes the two mappings unambiguous.
2. Refresh the doc comment at runner.rs:148-162: "Typing semantics are unchanged..." paragraph now states integer-typed positions load through the loader's typed probe (boot parity), string-valued positions keep string typing.
3. Re-record `lint-corpus-baseline.ron`: any corpus fixture carrying an int-position with-default placeholder that previously expected an R-SCHEMA Error now expects no such diagnostic (regenerate per the fixture harness's update command; verify by diff that ONLY those entries changed).
4. Flip the named driver tests: `lean_file_route_int_placeholder_doc_error` (driver_tests.rs:1350) → rename `lean_file_route_int_placeholder_loads`, assert `open_duration_ms == 750`; `lean_inline_routes_int_placeholder_doc_error` (:1415) → rename `lean_inline_routes_int_placeholder_loads`, assert `750`; `doc_env_int_position_still_fails` (:1625) → rename `doc_env_int_position_loads_via_probe`, assert `open_duration_ms == 500`. `lean_file_route_string_placeholder_loads` (:1325), `lean_unset_no_default_doc_error_names_var` (:1455), `lean_ignores_ambient_env` (:1477), and `env_value_never_interpolated_route` (:1662) stay green unmodified.

**Tests:** (`cargo test -p camel-cli` — runner + driver + baseline suites)
- Driver `CB_MS` cases assert `open_duration_ms == 750` (unset) and `== 500` (env map).
- Baseline suite green after re-record; `cargo test -p camel-cli` passes.

**Acceptance:**
- `cargo test -p camel-cli` passes.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 3.2

#### Task 3.3: CONTEXT canon updates

**Files:**
- `crates/camel-dsl/CONTEXT.md` (modified)
- `crates/camel-config/CONTEXT.md` (modified)
- `crates/camel-lint/CONTEXT.md` (modified)
- `crates/camel-cli/CONTEXT.md` (modified)

**Steps:**
1. camel-dsl CONTEXT: extend the env-interpolation canon entry with the provenance + typed-probe layer (int positions coerce post-failure; string/polymorphic positions untouched; cap 8; routes-only scope).
2. camel-config CONTEXT: extend the placeholder-resolution entry with the provenance probe at deserialize (token-bearing leaves only; literals strict; overrides merge pre-resolution).
3. camel-lint CONTEXT: extend the R-SCHEMA typing-mirror entry with the integer-position carve-out and the SYNC note.
4. camel-cli CONTEXT: the LEAN runner canon currently states integer-typed placeholders fail to load (boot parity with the old rejection) — update to the typed-probe boot parity (integer positions load; string-valued positions unchanged; no-default tokens still fail).

**Tests:**
- `cargo xtask lint-context-citations` exits 0 (citations valid).

**Acceptance:**
- The four CONTEXT.md files reference the new behavior with file:line anchors valid at HEAD of the worktree.

- [x] 3.3

#### Task 3.4: Mission acceptance sweep

**Files:**
- none (verification only)

**Steps:**
1. Run the demo shape end to end: `cargo test -p camel-dsl --lib load_from_file_with_env_int` (unset=2, lookup=5) and the discovery pair — the exact `${env:DWD_WARM_MAX_REQUESTS:-2}` throttle shape.
2. Parity sweep: `cargo test -p camel-dsl` (includes `advanced_json_yaml_parity`, `json_yaml_non_printable_parity`, `route_template_parity`) green.
3. Schema: `cargo xtask schema --check` exits 0 untouched.
4. Full gates: `cargo fmt --check --all`; `cargo clippy -p camel-dsl -p camel-config -p camel-lint -p camel-cli -- -D warnings`; `cargo test -p camel-dsl -p camel-config -p camel-lint -p camel-cli`.

**Tests:**
- All commands above exit 0.

**Acceptance:**
- Gates green; acceptance criteria from proposal.md all demonstrably met.

- [x] 3.4
