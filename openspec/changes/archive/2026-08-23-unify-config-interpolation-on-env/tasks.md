# Tasks: unify-config-interpolation-on-env

## Task 1 — `$$` escape in the DSL resolver

Files:
- `crates/camel-dsl/src/env_interpolation.rs` (modified — implementation + its inline tests module)

Steps:
1. Extend the regex in `crates/camel-dsl/src/env_interpolation.rs` (currently `\$\{env:([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\}`) to first match a doubled-dollar escape: `(\$\$)\$\{env:...` — capture `$${env:FOO}` and emit the literal text `${env:FOO}` (strip one `$`), and capture a standalone `$$` emitting a single `$`.
2. Keep the existing `${env:NAME}` / `${env:NAME:-default}` behavior byte-identical (no default → `Err(var_name)` fail-closed; default → used when unset; set → env value wins).
3. Order the regex alternatives so `$$` forms are consumed before the plain `${env:` form (leftmost-first with the escape branch preceding), preventing `$${env:X}` from being half-matched as `${env:X}` resolution.

Tests (all in the `env_interpolation.rs` tests module; unique env vars, restore on exit):
- name: `escape_full_form_yields_literal`
  setup: env `RUST_CAMEL_TEST_ESC_A=real-val` set
  action: `interpolate_env("$${env:RUST_CAMEL_TEST_ESC_A}")`
  assert: `Ok("${env:RUST_CAMEL_TEST_ESC_A}")` — the literal placeholder text, NOT `real-val`
  command: `cargo test -p camel-dsl --lib escape_full_form_yields_literal`
  expected: fails before the regex change, passes after
- name: `escape_standalone_dollar_yields_single`
  setup: input string `a$$b` (no env var involved)
  action: `interpolate_env("a$$b")`
  assert: `Ok("a$b")`
  command: `cargo test -p camel-dsl --lib escape_standalone_dollar_yields_single`
  expected: fails before, passes after
- name: `escape_then_placeholder_both_resolve`
  setup: env `RUST_CAMEL_TEST_ESC_B=val-b` set
  action: `interpolate_env("$${env:LIT} and ${env:RUST_CAMEL_TEST_ESC_B}")`
  assert: `Ok("${env:LIT} and val-b")`
  command: `cargo test -p camel-dsl --lib escape_then_placeholder_both_resolve`
  expected: fails before, passes after
- name: `plain_env_placeholder_semantics_unchanged`
  setup: existing tests in the module (missing-var error, default used, env wins) unchanged
  action: run the whole module
  assert: all pre-existing tests still pass
  command: `cargo test -p camel-dsl --lib env_`
  expected: passes before and after (regression guard)

Acceptance:
- `cargo test -p camel-dsl --lib` passes including the three new tests.
- `cargo fmt --check --all` and `cargo clippy -p camel-dsl -- -D warnings` exit 0.

- [x] task-1-dsl-dollar-escape

## Task 2 — Walk + leaf resolvers as public API (no load-path change yet)

Files:
- `crates/camel-config/src/config.rs` (modified)
- `crates/camel-config/tests/placeholder_walk.rs` (new)

Steps:
1. Add `pub fn resolve_tree_placeholders(root: &mut toml::Value) -> Result<(), ConfigError>` in `crates/camel-config/src/config.rs`: recursive descent over tables and arrays; for every `toml::Value::String` leaf compute its dotted path (e.g. `security.native.bearer_token`, `components.timer.period`, array indices as `[i]`). MUST be `pub` — integration tests under `tests/` call it directly (the `config` module is publicly re-exported).
2. Dispatch per path prefix, declared ONCE: add `pub(crate) const STRICT_PREFIXES: &[&str] = &["security", "datasources", "idempotent_repo", "cache_repo"];` — leaves whose top-level path segment is in `STRICT_PREFIXES` call `resolve_strict_leaf(value, &path)`; all other leaves call `resolve_plain_leaf(value, &path)`. Integration tests CANNOT import `pub(crate)` items (external crates): keep the const `pub(crate)`, put the content assertion in a config.rs UNIT test (Task 5 owns it; `placeholder_walk.rs` asserts dispatch BEHAVIOR only). `[idempotent_repo]`/`[cache_repo]` are strict-class per the design principle (redis `sentinel_password` + URL userinfo credential-bearing; hand-redacted Debug confirms).
3. Implement `resolve_strict_leaf`: (a) if the raw leaf contains `{{` return `Err(ConfigError::Message(...))` naming the field and the `${env:}` replacement forms (legacy rejection); (b) call `camel_dsl::env_interpolation::interpolate_env` (map its error to `ConfigError::Message` naming the field); (c) if the resolved value still contains `${` OR `{{` return `Err` "unresolved placeholder marker in {field}" — the check CONTENT is unchanged from the current gate (config.rs:1911 checks `contains("${")`); what changes is ORDERING (interpolation runs first, so consumed `${env:X}` leaves no `${`), meaning consumed forms pass and malformed/unconsumed forms (`${notenv:x}`, truncated `${env:`, escaped `$${env:X}` residue) die.
4. Implement `resolve_plain_leaf`: (a) same unconditional legacy `{{` rejection as the strict path; (b) if the leaf contains `${env:` OR `$$`, call `interpolate_env` (fail-closed: its `Err` propagates as `ConfigError` naming the field — uniform Q9); (c) leaves with neither marker pass through untouched.
5. Do NOT rewire the load path in this task and do NOT delete the old functions — Task 3 does both. This task lands the walk + tests with the old path still active.

Tests (new file `crates/camel-config/tests/placeholder_walk.rs`):
- name: `strict_leaf_resolves_and_fails_closed`
  setup: toml table `{security = {native = {bearer_token = "${env:RUST_CAMEL_TEST_WALK_A}"}}}`
  action: `resolve_tree_placeholders` with the var set (resp. unset)
  assert: set → leaf equals env value; unset → `Err` naming `security.native.bearer_token`
  command: `cargo test -p camel-config --test placeholder_walk strict_leaf`
  expected: fails before this task (function absent), passes after
- name: `plain_leaf_uniform_fail_closed`
  setup: `{observability = {otel = {endpoint = "${env:RUST_CAMEL_TEST_WALK_B}"}}}` var unset
  action: `resolve_tree_placeholders`
  assert: `Err` naming `observability.otel.endpoint` (Q9: no warn-and-continue)
  command: `cargo test -p camel-config --test placeholder_walk plain_leaf_uniform_fail_closed`
  expected: fails before, passes after
- name: `plain_leaf_passthrough_without_markers`
  setup: `{components = {timer = {period = "${body}"}}}` and a plain string `hello`
  action: `resolve_tree_placeholders`
  assert: `Ok`, values unchanged (`${body}` has no `env:` prefix; `hello` has no markers)
  command: `cargo test -p camel-config --test placeholder_walk plain_leaf_passthrough_without_markers`
  expected: passes after this task lands (guards the prefix gate)
- name: `strict_leaf_matrix_covers_spec_scenarios`
   setup: raw tree mixing: `security.native.bearer_token = "${env:RUST_CAMEL_TEST_MTX_A:-fallback-tok}"` (unset), `security.oidc.client_secret = "${env:RUST_CAMEL_TEST_MTX_B}"` (set), `security.keycloak.realm = "${env:RUST_CAMEL_TEST_MTX_C:-main}"` (unset), `datasources.main.db_url = "${env:RUST_CAMEL_TEST_MTX_D}"` (set), `datasources.main.extra.password = "${env:RUST_CAMEL_TEST_MTX_E}"` (set), `idempotent_repo.url = "${env:RUST_CAMEL_TEST_MTX_F}"` (set), `cache_repo.sentinel_password = "${env:RUST_CAMEL_TEST_MTX_G}"` (set)
   action: `resolve_tree_placeholders`
   assert: `Ok`; bearer_token equals `fallback-tok` (explicit default), oidc secret and db_url and extra.password equal their env values, realm equals `main` (non-credential leaf with default), repo url and sentinel_password equal their env values (both repo sections strict-class coverage: idempotent url + cache sentinel_password)
  command: `cargo test -p camel-config --test placeholder_walk strict_leaf_matrix_covers_spec_scenarios`
  expected: fails before, passes after
- name: `keycloak_secret_fails_closed_when_missing`
  setup: `security.keycloak.client_secret = "${env:RUST_CAMEL_TEST_KC_MISS}"` var unset
  action: `resolve_tree_placeholders`
  assert: `Err` naming `security.keycloak.client_secret` (the spec's keycloak-missing scenario, named)
  command: `cargo test -p camel-config --test placeholder_walk keycloak_secret_fails_closed_when_missing`
  expected: fails before, passes after
- name: `legacy_braces_rejected_everywhere`
  setup: `{log_level = "{{env:X}}"}` (plain leaf) and `{security = {native = {bearer_token = "{{env:X}}"}}}` (strict leaf)
  action: `resolve_tree_placeholders`
  assert: both `Err` with a message containing `${env:NAME}`
  command: `cargo test -p camel-config --test placeholder_walk legacy_braces_rejected_everywhere`
  expected: fails before, passes after
- name: `standalone_dollar_converts_on_all_leaf_classes`
  setup: `log_level = "a$$b"`, `security.keycloak.realm = "a$$b"`, `datasources.main.provider = "a$$b"`, `idempotent_repo.backend = "a$$b"`, `cache_repo.backend = "a$$b"`
  action: `resolve_tree_placeholders`
  assert: all become `a$b` (standalone escape leaves no marker — passes even the strict residual check, on every strict prefix AND plain)
  command: `cargo test -p camel-config --test placeholder_walk standalone_dollar_converts_on_all_leaf_classes`
  expected: fails before (no `$$` handling in walk), passes after Tasks 1+2
- name: `escaped_full_form_rejected_on_strict_leaves`
  setup: `security.native.bearer_token = "$${env:RUST_CAMEL_TEST_WALK_C}"`, `idempotent_repo.sentinel_password = "$${env:RUST_CAMEL_TEST_WALK_E}"`
  action: `resolve_tree_placeholders` per tree
  assert: both `Err` "unresolved placeholder marker" (residual `${env:` after escape conversion — security AND repo sections are strict-class)
  command: `cargo test -p camel-config --test placeholder_walk escaped_full_form_rejected_on_strict_leaves`
  expected: fails before, passes after
- name: `escaped_full_form_literal_on_plain_leaves`
  setup: `log_level = "$${env:RUST_CAMEL_TEST_WALK_D}"`
  action: `resolve_tree_placeholders`
  assert: `Ok`, value equals the literal `${env:RUST_CAMEL_TEST_WALK_D}` (config-surface half of the escape scenario)
  command: `cargo test -p camel-config --test placeholder_walk escaped_full_form_literal_on_plain_leaves`
  expected: fails before, passes after

Acceptance:
- `cargo test -p camel-config --test placeholder_walk` green; old path untouched (lib tests still pass).
- `cargo fmt --check --all` and `cargo clippy -p camel-config -- -D warnings` exit 0.

- [x] task-2-walk-and-leaf-resolvers

## Task 3 — Load-path rewiring: builder-output materialization + deletions

Files:
- `crates/camel-config/src/config.rs` (modified)
- `crates/camel-config/tests/placeholder_e2e.rs` (new)

Steps:
1. Materialize the MERGED tree: in `build_from_toml_value_inner` (config.rs:1673-1785), after `builder.build()` (which has already merged the main file + include files + `CAMEL_*` env overrides), deserialize the builder state to `toml::Value` first, run `resolve_tree_placeholders` on that merged tree, then `toml::from_value::<CamelConfig>` (replacing the direct `config.try_deserialize()` at config.rs:1780). This is the critical materialization: walking only the pre-builder `config_value` would miss placeholders arriving via include files and env overrides — the merge happens INSIDE the builder. All three production load entries (config.rs:1501 `load_from_file_inner`, 1576 and 1607 `from_file_with_profile_and_env` sync/async) funnel into `build_from_toml_value_inner`, so rewiring there covers all of them.
2. Delete `fn resolve_placeholders` (~1491), `fn resolve_string_in_place` (~2289), `fn resolve_toml_value_placeholders` (~2318), `fn resolve_fail_closed` (~2370), `fn resolve_fail_closed_in_place` (~2422), `fn resolve_security_fail_closed` (~2432), `fn resolve_datasources_fail_closed` (~2550) — post-rebase line positions on main edf13469+16, SYMBOLS authoritative if they drift again. Delete the `security_walk_exhaustiveness` guard test (~4437) with its fixture — Task 5 adds the replacement guard. Remove all `PropertiesResolver` call-sites from config.rs (properties.rs stays as public API).
3. Convert camel-config inline tests using `{{` placeholders on the load path — the real sites are the `resolve_string_in_place_tests` module (~2306, dies with the function deletion in step 2) and any `{{` fixtures inside the exhaustiveness guard (~4437, also deleted in step 2); the prometheus-tests region (~2800s) is with no placeholders (no-op there). Note the deserialize-semantics swap: `try_deserialize` (config-crate lenient string→numeric coercion) becomes `toml::from_value` (strict) — accept strict semantics and verify no repo fixture relies on string-number coercion (grep configs/tests for quoted numerics in Camel.toml contexts; fix fixtures if any).
4. Confirm `config.validate()` ordering is unchanged (post-deserialization) and the `_extra` capture (config.rs:95) still deserializes — unknown sections flow through the walk as raw leaves and land in `_extra` resolved.

Tests (new file `crates/camel-config/tests/placeholder_e2e.rs`):
- name: `future_section_resolves_without_code_change`
  setup: TOML string `[future_section]\nvalue = "${env:RUST_CAMEL_TEST_FUT_A}"` with the var set to `fut-val`
  action: full load path (materialize → walk → deserialize)
  assert: `Ok`; resolved `future_section.value` equals `fut-val` (assert on the walked tree AND on `CamelConfig._extra` if captured)
  command: `cargo test -p camel-config --test placeholder_e2e future_section_resolves_without_code_change`
  expected: fails before, passes after — THE anti-regression test for the allowlist class
- name: `placeholder_in_include_file_resolves`
  setup: main Camel.toml with `include = ["inc.toml"]` (top-level array; `[include]` table is invalid syntax) where `inc.toml` carries `[observability.otel]\nendpoint = "${env:RUST_CAMEL_TEST_INC_A:-http://localhost:4317}"` (var unset)
  action: full load path
  assert: `Ok`; endpoint equals `http://localhost:4317` — proves the walk runs on the POST-merge tree, not the pre-builder value (Critical-finding regression test)
  command: `cargo test -p camel-config --test placeholder_e2e placeholder_in_include_file_resolves`
  expected: fails before, passes after
- name: `security_bearer_token_e2e_never_literal`
  setup: `[security.native]\nbearer_token = "${env:RUST_CAMEL_TEST_E2E_A}"` var unset
  action: full load path
  assert: `Err` naming the field — literal never installed (rc-xb19 contract under new syntax)
  command: `cargo test -p camel-config --test placeholder_e2e security_bearer_token_e2e_never_literal`
  expected: fails before, passes after
- name: `otel_endpoint_default_honored_e2e`
  setup: `[observability.otel]\nendpoint = "${env:RUST_CAMEL_TEST_E2E_B:-http://localhost:4317}"` var unset
  action: full load path
  assert: `Ok`; deserialized `observability.otel.endpoint` equals `http://localhost:4317`
  command: `cargo test -p camel-config --test placeholder_e2e otel_endpoint_default_honored_e2e`
  expected: fails before, passes after
- name: `nested_array_leaves_walked`
  setup: TOML with `[[security.native.credentials]]` array: entry 0 `secret = "${env:RUST_CAMEL_TEST_CRED0}"` (set) + entry 1 `secret = "${env:RUST_CAMEL_TEST_CRED1}"` (unset)
  action: full load path
  assert: entry 0 resolves to env value; entry 1 → `Err` naming `security.native.credentials[1].secret` (array index in the path)
  command: `cargo test -p camel-config --test placeholder_e2e nested_array_leaves_walked`
  expected: fails before, passes after
- name: `quoted_numeric_root_field_is_rejected_after_materialization`
  setup: TOML string `timeout_ms = "1000"` (quoted numeric — relied on the old lenient `try_deserialize` string→numeric coercion)
  action: full load path (materialize → walk → strict `toml::from_value`)
  assert: `Err` (strict deserialization rejects the quoted numeric; the semantics swap is intentional and pinned by this test)
  command: `cargo test -p camel-config --test placeholder_e2e quoted_numeric_root_field_is_rejected_after_materialization`
  expected: fails before (lenient path accepted it), passes after

Acceptance:
- `cargo test -p camel-config` green (lib + integration).
- `rg 'fn resolve_placeholders|fn resolve_string_in_place|fn resolve_toml_value_placeholders|fn resolve_security_fail_closed|fn resolve_datasources_fail_closed|fn resolve_fail_closed' crates/camel-config/src` returns zero hits.
- `rg 'PropertiesResolver' crates/camel-config/src/config.rs` returns zero hits.
- `rg '\{\{' crates/camel-config/src` returns zero hits outside properties.rs (its own test module tests the legacy public API).
- `cargo fmt --check --all` and `cargo clippy -p camel-config -- -D warnings` exit 0.

- [x] task-3-load-path-rewiring

## Task 4 — Route-surface regression (frozen semantics)

Files:
- `crates/camel-dsl/tests/env_escape_regression.rs` (new)

Steps:
1. Add an integration test file asserting route-level `${env:}` behavior is unchanged by the escape addition: a route URI carrying `${env:RUST_CAMEL_TEST_ROUTE_A}` with the variable SET interpolates at discovery exactly as before; `$${env:...}` in a route body yields the literal (new capability, Task 1); a route with `${env:MISSING}` (no default, var unset) still fails discovery with the existing error.
2. Use the existing route-loading helpers used by other camel-dsl integration tests (see `crates/camel-dsl/tests/` neighbors for the pattern).

Tests:
- name: `route_env_set_interpolation_unchanged`
  setup: env `RUST_CAMEL_TEST_ROUTE_A=route-val` set; route YAML with `to: "log://${env:RUST_CAMEL_TEST_ROUTE_A}"` (set variable, no default — matches the spec scenario's set-variable form)
  action: load route through discovery
  assert: endpoint equals `log://route-val`
  command: `cargo test -p camel-dsl --test env_escape_regression route_env_set_interpolation_unchanged`
  expected: passes before and after Task 1 (regression guard)
- name: `route_env_missing_fails_discovery`
  setup: route YAML with `to: "log://${env:RUST_CAMEL_TEST_ROUTE_MISSING}"` var unset
  action: load route
  assert: discovery error naming the variable (fail-closed preserved)
  command: `cargo test -p camel-dsl --test env_escape_regression route_env_missing_fails_discovery`
  expected: passes before and after
- name: `route_escape_yields_literal_in_body`
  setup: route step body containing `$${env:RUST_CAMEL_TEST_ROUTE_B}`
  action: load route
  assert: body equals the literal `${env:RUST_CAMEL_TEST_ROUTE_B}` text
  command: `cargo test -p camel-dsl --test env_escape_regression route_escape_yields_literal_in_body`
  expected: fails before Task 1, passes after
- name: `route_standalone_dollar_converts`
  setup: route step body containing `a$$b`
  action: load route
  assert: body equals `a$b` (standalone escape at the route surface)
  command: `cargo test -p camel-dsl --test env_escape_regression route_standalone_dollar_converts`
  expected: fails before Task 1, passes after

Acceptance:
- `cargo test -p camel-dsl` green (lib + integration).
- `cargo fmt --check --all` and `cargo clippy -p camel-dsl -- -D warnings` exit 0.

- [x] task-4-route-regression

## Task 5 — Strict-gate exhaustiveness guard replacement

Files:
- `crates/camel-config/tests/placeholder_walk.rs` (modified)
- `crates/camel-config/src/config.rs` (modified — unit-test module only: const-content tripwire)

Steps:
1. Add the path-prefix exhaustiveness guard tests to `placeholder_walk.rs`: the raw-tree dispatch routes EVERY string leaf under a strict prefix through the strict leaf resolver — a leaf planted at any depth under `security` (including a hypothetical new sub-table) with an unresolvable `${env:...}` must fail closed. This keeps the old guard's spirit (config.rs:4437 `mod security_walk_exhaustiveness`, deleted in Task 3: a new field can't silently skip strictness) without hand-enumeration. `placeholder_walk.rs` (integration test — external crate) asserts dispatch BEHAVIOR only; the const-content tripwire lives in a config.rs UNIT test added in step 2.
2. In the config.rs unit-test module add `strict_prefixes_content_is_deliberate`: assert `STRICT_PREFIXES == ["security", "datasources", "idempotent_repo", "cache_repo"]` — the expected literal is the intentional tripwire; extending the strict class must be a deliberate, reviewed edit to the single production home.
3. Ownership note: Task 3 deletes the old guard test together with `resolve_security_fail_closed`; THIS task adds the replacement. The gap exists only between Task 3 and Task 5 commits — acceptable within one change.

Tests (all appended to `crates/camel-config/tests/placeholder_walk.rs`):
- name: `strict_prefixes_content_is_deliberate`
  setup: `STRICT_PREFIXES` const in config.rs (unit test lives in the config.rs test module, listed here for completeness — placeholder_walk.rs cannot import pub(crate))
  action: assert `STRICT_PREFIXES == ["security", "datasources", "idempotent_repo", "cache_repo"]`
  assert: equality holds — extending the strict class requires editing the single production home AND this tripwire deliberately
  command: `cargo test -p camel-config --lib strict_prefixes_content_is_deliberate`
  expected: passes with Task 5 (const + unit test land together)
- name: `repository_leaves_follow_strict_gate`
  setup: three trees: (a) `idempotent_repo.url = "${env:RUST_CAMEL_TEST_REPO_U}"` set + `cache_repo.sentinel_password = "${env:RUST_CAMEL_TEST_REPO_P}"` set; (b) `cache_repo.sentinel_password = "${env:RUST_CAMEL_TEST_REPO_P}"` unset; (c) `idempotent_repo.url = "$${env:RUST_CAMEL_TEST_REPO_U}"` and `cache_repo.sentinel_password = "$${env:RUST_CAMEL_TEST_REPO_P}"`
  action: `resolve_tree_placeholders` per tree
  assert: (a) `Ok`, both values equal env values; (b) `Err` naming `cache_repo.sentinel_password`; (c) both `Err` "unresolved placeholder marker" (escaped full-form rejected on repo leaves — strict class)
  command: `cargo test -p camel-config --test placeholder_walk repository_leaves_follow_strict_gate`
  expected: fails before, passes after
- name: `strict_dispatch_is_exhaustive_over_security_subtree`
  setup: raw tree `{security = {brand_new_section = {deep = {token = "${env:RUST_CAMEL_TEST_GUARD_A}"}}}}` (section unknown to any struct) var unset
  action: `resolve_tree_placeholders`
  assert: `Err` naming `security.brand_new_section.deep.token` — strictness reaches unknown depth by path prefix, no enumeration
  command: `cargo test -p camel-config --test placeholder_walk strict_dispatch_is_exhaustive_over_security_subtree`
  expected: fails before Task 2, passes after Tasks 2+5 (added here as the permanent guard)
- name: `strict_residual_rejects_malformed_dollar_forms`
  setup: `security.native.bearer_token` values: truncated `"${env:"`, wrong-namespace `"${notenv:x}"`
  action: `resolve_tree_placeholders` per value
  assert: both `Err` "unresolved placeholder marker" — the residual check keeps its current content (`contains("${") || contains("{{")`); interpolation runs FIRST so consumed `${env:X}` passes (ordering change, not check narrowing — malformed `${notenv:x}` is untouched by interpolation and dies on the unchanged check)
  command: `cargo test -p camel-config --test placeholder_walk strict_residual_rejects_malformed_dollar_forms`
  expected: fails before, passes after
- name: `valid_new_syntax_passes_strict_gate`
  setup: `security.keycloak.client_secret = "${env:RUST_CAMEL_TEST_GUARD_B}"` var set
  action: `resolve_tree_placeholders`
  assert: `Ok`, secret equals env value (residual check does not fire on consumed forms)
  command: `cargo test -p camel-config --test placeholder_walk valid_new_syntax_passes_strict_gate`
  expected: fails before, passes after

Acceptance:
- `cargo test -p camel-config` green.
- The path-prefix guard exists in `placeholder_walk.rs` (old guard was deleted in Task 3).
- `cargo fmt --check --all` and `cargo clippy -p camel-config -- -D warnings` exit 0.

- [x] task-5-strict-guard-migration

## Task 6 — Store-guard verification (guard carries forward unchanged)

Files:
- `crates/services/camel-auth/src/native_auth.rs` (modified — tests only)

Steps:
1. NO functional change: `ensure_no_placeholder_markers` (native_auth.rs:72) ALREADY rejects any secret containing `${` (a superset of `${env:`), in addition to `{{`. The proposal and security spec state the guard carries forward unchanged — this task verifies that claim under the new syntax with tests, and confirms no false positives on standalone-escape output.
2. Add the two tests below to the existing `native_auth.rs` test module.

Tests:
- name: `store_guard_rejects_new_syntax_marker`
  setup: `NativeCredentialSecret::Plaintext` with value `x${env:y}z`
  action: `NativeCredentialStore::try_new`
  assert: `Err` (existing `${` check fires — verification that the guard covers the new syntax with zero code change)
  command: `cargo test -p camel-auth store_guard`
  expected: passes before AND after (guard already live — this pins the behavior)
- name: `store_guard_accepts_clean_secrets`
  setup: plaintext `a$b` (standalone escape result) and a normal opaque token
  action: `NativeCredentialStore::try_new`
  assert: `Ok` for both (no false positive on lone `$` — important now that `$$`→`$` conversion is load-bearing)
  command: `cargo test -p camel-auth store_guard`
  expected: passes before AND after

Acceptance:
- `cargo test -p camel-auth` green.
- Zero functional diff in `native_auth.rs` outside the test module.
- `cargo fmt --check --all` and `cargo clippy -p camel-auth -- -D warnings` exit 0.

- [x] task-6-store-guard-verification

## Task 7 — Docs sweep: one interpolation chapter

Files:
- `crates/camel-config/README.md` (modified)
- `docs/src/configuration/schema.md` (modified)
- `docs/src/configuration/env-interpolation.md` (modified)
- `crates/camel-dsl/README.md` (modified — cross-link section)

Steps:
1. Rewrite the placeholder sections of `crates/camel-config/README.md` and `docs/src/configuration/schema.md`: `${env:NAME}` / `${env:NAME:-default}` as THE Camel.toml syntax; uniform fail-closed (unset var without default aborts — document `:-default` for optional values); legacy `{{...}}` hard-error with the actionable message text.
2. Update `docs/src/configuration/env-interpolation.md` to cover BOTH surfaces under one chapter: routes (unchanged semantics) + Camel.toml (new unified behavior), including the `$$` and `$${env:...}` escape table and the security-leaf exception (escaped full form rejected on credentials).
3. In `crates/camel-dsl/README.md` (env-interpolation section ~lines 496-511): add the `$$` escape documentation and cross-link the config chapter.
4. Remove the two-syntax boundary notes added by the v0.33 security landing (search for "single colon" / "route-file syntax" notes).

Tests:
- name: `docs_no_legacy_placeholders_remain`
  setup: the four files updated per steps 1-4
  action: `rg '\{\{env:' crates/camel-config/README.md docs/src/configuration/schema.md`
  assert: zero matches (exit 1)
  command: `! rg '\{\{env:' crates/camel-config/README.md docs/src/configuration/schema.md`
  expected: passes after the sweep
- name: `docs_new_syntax_present_in_every_file`
  setup: same four files: `crates/camel-config/README.md`, `docs/src/configuration/schema.md`, `docs/src/configuration/env-interpolation.md`, `crates/camel-dsl/README.md`
  action: for EACH file run `rg -q '\$\{env:' <file>`
  assert: every individual check exits 0 (≥1 new-syntax hit per file — proven per-file, not in aggregate)
  command: `for f in crates/camel-config/README.md docs/src/configuration/schema.md docs/src/configuration/env-interpolation.md crates/camel-dsl/README.md; do rg -q '\$\{env:' "$f" || { echo "MISSING in $f"; exit 1; }; done`
  expected: passes after the sweep
- name: `docs_build_succeeds`
  setup: docs tree with the updated chapters
  action: `nix shell nixpkgs#mdbook -c mdbook build docs`
  assert: exit 0
  command: `nix shell nixpkgs#mdbook -c mdbook build docs`
  expected: passes after the sweep

Acceptance:
- `! rg '{{env:' crates/camel-config/README.md docs/src/configuration/schema.md` (zero hits).
- Per-file new-syntax check loop (above) exits 0 for all four files.
- `nix shell nixpkgs#mdbook -c mdbook build docs` exits 0.
- `cargo xtask lint-context-citations` exits 0 (docs citations intact).

- [x] task-7-docs-sweep

## Task 8 — `camel run` config-load error surfacing (fail-closed at the CLI boundary)

Files:
- `crates/camel-cli/src/commands/run.rs` (modified — config-load block + test module)

Context: run.rs:56 `CamelConfig::from_file(&config_path).unwrap_or_else(|_| { ...empty config... })`
swallows EVERY load error into silent defaults — parse errors, broken includes, and (after
Task 3) unresolved `${env:...}` placeholders. The empty-config fallback is intended ONLY for
file-not-found. Without this task, the fail-closed contract delivered by Tasks 2-3 dies at
the CLI surface: `camel run` with a bad Camel.toml boots on defaults without a word.

Steps:
1. Extract the config-load decision into a testable helper, e.g.
   `fn load_config_or_default(config_path: &str) -> Result<CamelConfig, camel_api::CamelError>`:
   call `Path::try_exists(config_path)` FIRST — `Ok(false)` → return the empty default
   config (built exactly as today, serde defaults); `Err(e)` (io error on the check itself)
   → `Err(CamelError::Config(...))` naming path and cause; `Ok(true)` → `CamelConfig::from_file`,
   and EVERY error it returns propagates as `Err(CamelError::Config(format!("failed to load {config_path}: {e}")))`.
   Rationale: a missing INCLUDE also surfaces as file-not-found inside ConfigError — it must
   abort, not fall back — so the not-found decision is made on the main path only, before
   loading. Do NOT downcast or classify ConfigError by io kind (it is dependency-owned and
   `read_capped` erases the kind into `ConfigError::Message`).
2. Replace the run.rs:56 `unwrap_or_else(|_| ...)` block with a call to the helper;
   `camel run` now fails fast with a visible message on any load failure of an existing file.
3. Add the unit tests below to the run.rs test module.

Tests:
- name: `missing_config_file_yields_defaults`
  setup: path to a nonexistent file
  action: `load_config_or_default`
  assert: `Ok`, config equals serde defaults (fallback preserved for the not-found case only)
  command: `cargo test -p camel-cli missing_config_file_yields_defaults`
  expected: fails before (helper absent), passes after
- name: `malformed_config_aborts_instead_of_defaults`
  setup: temp Camel.toml with invalid TOML (`[observability` unclosed)
  action: `load_config_or_default`
  assert: `Err` whose message names the path and contains "failed to load" AND the parse-cause text (e.g. "unclosed" / TOML error detail — not only the prefix)
  command: `cargo test -p camel-cli malformed_config_aborts_instead_of_defaults`
  expected: fails before (returned Ok-defaults), passes after
- name: `broken_include_aborts_instead_of_defaults`
  setup: temp Camel.toml with `include = ["missing.toml"]` — main file exists, include does not
  action: `load_config_or_default`
  assert: `Err` naming the main path and the missing include (NOT the defaults fallback — the try_exists pre-check only covers the main file; include errors must propagate)
  command: `cargo test -p camel-cli broken_include_aborts_instead_of_defaults`
  expected: fails before, passes after
- name: `unresolved_placeholder_aborts_instead_of_defaults`
  setup: temp Camel.toml with `endpoint = "${env:RUST_CAMEL_TEST_RUN_A}"`, var unset (requires Tasks 2-3 landed)
  action: `load_config_or_default`
  assert: `Err` whose message contains BOTH the config path and `RUST_CAMEL_TEST_RUN_A` as the unresolved cause (full contract, not bare `Err`)
  command: `cargo test -p camel-cli unresolved_placeholder_aborts_instead_of_defaults`
  expected: fails before, passes after (ordering: after Task 3)

Acceptance:
- `cargo test -p camel-cli` green.
- `! rg 'from_file\([^\)]*\)\.unwrap_or_else\(\|_\|' crates/camel-cli/src/commands/run.rs` (the blanket swallow is gone; zero matches).
- `cargo fmt --check --all` and `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] task-8-cli-config-error-surfacing
