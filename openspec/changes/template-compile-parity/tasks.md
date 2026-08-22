# Tasks: template-compile-parity

## Phase 1: Context parity (F1, F3, F9)

### Task 1.1 — Thread threshold + security context through materialize_and_compile

- **Files**:
  - `crates/camel-dsl/src/template/materializer.rs` (modified)
- **Steps**:
  1. Change `materialize_and_compile` signature (materializer.rs:187) to `pub fn materialize_and_compile(template: &RouteTemplateSpec, templated: &TemplatedRouteSpec, stream_cache_threshold: usize, security_ctx: SecurityCompileContext) -> Result<Vec<CompiledMaterializationResult>, CamelError>`.  2. Replace the `compile_declarative_route(route)?` call inside it with `compile_declarative_route_with_stream_cache_threshold(route, stream_cache_threshold, security_ctx.clone())?` (imported from `crate::compile`).
  3. Update every existing call in the `#[cfg(test)] mod tests` of materializer.rs (lines ~561, 580, 606, 681) to pass `camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD` and `SecurityCompileContext::default()`.
- **Tests** (existing tests, updated signatures only — no new tests here):
  - name: existing materializer unit tests (e.g. `resolve_params_with_all_provided` neighbors that call `materialize_and_compile`)
  - setup: unchanged
  - action: run the materializer test module
  - assert: all existing tests still pass with the new signature
  - command: `cargo test -p camel-dsl --lib template::materializer`
  - expected: pass after step 3 (compile fails between steps 1-2 because discovery.rs:358 still calls the old arity — that caller is updated in Task 1.2, so run the full-crate check only after 1.2)
- **Acceptance**: `cargo build -p camel-dsl` succeeds after Task 1.2; no `SecurityCompileContext::default()` remains inside `materialize_and_compile` itself; `cargo test -p camel-dsl --lib template::` passes.
- [x] 1.1

### Task 1.2 — Discovery Pass 2 + threshold-less branches use the security-aware path

- **Files**:
  - `crates/camel-dsl/src/discovery.rs` (modified)
- **Steps**:
  1. Update the Pass 2 call (discovery.rs:358) `materialize_and_compile(template, spec)` to pass `stream_cache_threshold.unwrap_or(camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD)` and `security_ctx.clone().unwrap_or_default()`.
  2. Fold the `None` threshold branches for direct routes (discovery.rs YAML `None` arm currently `parse_yaml(&content)`, JSON `None` arm currently `parse_json(&content)`) to call `parse_yaml_with_threshold_and_security(&content, DEFAULT_STREAM_CACHE_THRESHOLD, security_ctx.clone().unwrap_or_default())` and the JSON equivalent `parse_json_with_threshold_and_security`, so direct and templated routes share one compile path (F9).
  3. Keep `parse_yaml`/`parse_json` public API unchanged (they remain for external users).
- **Tests**:
  - name: `threshold_less_discovery_threads_security_context` (UNIT test in `crates/camel-dsl/src/discovery.rs` `mod tests`, calling the private `discover_routes_inner` directly)
  - setup: a tempdir YAML file (via `tempfile` dev-dependency) containing one direct route with `security_policy: roles`; a `SecurityCompileContext` built with a `TestAuthenticator` (clone the struct from `crates/camel-dsl/tests/security_policy_integration.rs:16`)
  - action: call `discover_routes_inner(&[pattern], None, Some(ctx))` — the (None-threshold, Some-ctx) combination only reachable through this private fn; the public `discover_routes` passes `None` ctx and MUST still fail closed on secured routes
  - assert: returns one route; the compiled route exposes a security authenticator; additionally assert `discover_routes_inner(&[pattern], None, None)` on the same file errs with `route requires an authenticator` (fail-closed pin)
  - command: `cargo test -p camel-dsl --lib discovery`
  - expected: fails before this task (None branch rejects secured route), passes after
- **Acceptance**: `grep -n "parse_yaml(&content)" crates/camel-dsl/src/discovery.rs` returns no matches in the route-parsing match arms; `cargo build -p camel-dsl` succeeds; new test passes.
- [x] 1.2

### Task 1.3 — Security and threshold parity regression tests

- **Files**:
  - `crates/camel-dsl/tests/route_template_parity.rs` (new)
- **Steps**:
  1. Create the integration test crate file with a `TestAuthenticator` implementing `camel_auth::TokenAuthenticator` (copy the pattern from `crates/camel-dsl/tests/security_policy_integration.rs:16-40`) and a helper `fn write_temp_yaml(dir: &std::path::Path, name: &str, body: &str) -> String` writing a file and returning a glob pattern `<dir>/<name>`.
  2. Write the five tests below. Template/spec YAML shape: `templates:` (id, parameters, routes) + `templated_routes:` (route_template_ref, route_id, parameters), matching `crates/camel-dsl/src/template/yaml.rs` parsers.
- **Tests** (all in `crates/camel-dsl/tests/route_template_parity.rs`, command `cargo test -p camel-dsl --test route_template_parity`, all expected: fail before Phase-1 code, pass after):
  - name: `secured_templated_route_compiles_with_real_context`
    - setup: tempdir file with a template whose route declares `security_policy: {roles: [admin]}` and one templated spec; `SecurityCompileContext::new(Some(Arc::new(TestAuthenticator)), None)`
    - action: `discover_routes_with_threshold_and_security(&[pattern], DEFAULT_STREAM_CACHE_THRESHOLD, ctx)`
    - assert: Ok, one route, `route.security_authenticator().is_some()` (mirror accessor usage in security_policy_integration.rs)
  - name: `secured_templated_route_fails_closed_with_default_context`
    - setup: same file; `SecurityCompileContext::default()`
    - action: same call
    - assert: Err containing `route requires an authenticator`
  - name: `mixed_direct_and_templated_secured_routes_both_materialize`
    - setup: one file with a direct secured route AND a templated secured route; real ctx
    - action: same call
    - assert: Ok, two routes with distinct route ids, both with authenticator
  - name: `templated_route_receives_configured_threshold` (UNIT test in `crates/camel-dsl/src/template/materializer.rs` `mod tests`)
    - AMENDED (conductor, post-plan-bless): the compiled `BuilderStep::Processor(OpaqueProcessor)` hides `StreamCacheService.config` and `BuilderStep` has no `PartialEq` — the two-sided step-config compare originally specified is unimplementable at ANY observability level the codebase exposes (the direct path's own `test_threshold_parse` at json.rs:297 asserts only `len()==1`). Replacement, matching the direct path's test depth plus a helper pin:
    - setup: a template whose route contains a `stream_cache` step (threshold not declared in the step); templated spec
    - action: `materialize_and_compile(template, spec, 7, SecurityCompileContext::default())` and, separately, `compile_declarative_route_with_stream_cache_threshold` on an equivalent direct `DeclarativeRoute` parsed from the same route JSON with threshold 7
    - assert: both compile Ok; the materialized route has the expected route structure (steps count / route_id), and the direct-path baseline compiles identically — parity of outcomes at the only observable depth the engine exposes
    - companion helper pin (same task): add `stream_cache_config_helper_resolves_threshold` to `crates/camel-dsl/src/compile.rs` `mod tests` asserting the private helper `stream_cache_config(None, 7).threshold == 7` and `stream_cache_config(Some(3), 7).threshold == 3` (both paths consume the threshold through this helper; `StreamCacheConfig.threshold` is pub)
    - command: `cargo test -p camel-dsl --lib template::materializer` and `cargo test -p camel-dsl --lib compile::tests::stream_cache_config_helper`
    - expected: helper pin passes trivially (pins shared seam); materializer parity test passes after Task 1.1
  - name: `cross_file_secured_template_compiles`
    - setup: tempdir A.yaml defining the template (secured route), B.yaml with the templated spec; real ctx; pattern covering both
    - action: discover
    - assert: Ok, one secured route materialized from B.yaml's spec
- **Acceptance**: `cargo test -p camel-dsl --test route_template_parity` — 4/4 integration tests pass; `templated_route_receives_configured_threshold` unit test passes in Task 1.1's module; `cargo test -p camel-dsl --test security_policy_integration` still passes (direct-route behavior unchanged).
- [x] 1.3

## Phase 2: Diagnostics (F2, F11)

### Task 2.1 — Structured template error with error class

- **Files**:
  - `crates/camel-api/src/template.rs` (modified)
  - `crates/camel-dsl/src/discovery.rs` (modified)
- **Steps**:
  1. Add variants to `TemplateError` (camel-api/src/template.rs:57): `#[error("security policy requires an authenticator (template {template_id}): {source}")] SecurityRequired { template_id: String, detail: String } (NOTE: field named `detail`, not `source` — thiserror reserves `source` for the error-chain and requires StdError; String cannot)` and `#[error("template not found: {0}")] NotFound(String)` (used by Task 2.2's aggregation for missing template refs, preserving its class).
  2. In camel-dsl discovery.rs add `pub struct MaterializationFailure { pub path: String, pub template_ref: String, pub route_id: Option<String>, pub error: TemplateError }` (per-failure `path` because Pass 2 iterates specs collected across multiple files) and a new `DiscoveryError` variant `MaterializationFailures { failures: Vec<MaterializationFailure> }` rendered as a multi-line error listing every failure with its path.
  3. In Pass 2 (discovery.rs:358-367), classify `CamelError::RouteError(msg)` whose text starts with `route requires an authenticator` as `TemplateError::SecurityRequired { template_id: spec.route_template_ref.clone(), source: msg }` instead of `InvalidBody`; keep other variants mapped as today.
- **Tests**:
  - name: `security_required_error_classified` (unit test in discovery.rs `mod tests`, following existing discovery test style)
  - setup: a template + spec whose materialized route has `security_policy` roles, compiled via the Pass 2 helper with `SecurityCompileContext::default()`
  - action: run the classification path (or the discovery fn on a tempdir file)
  - assert: returned error `matches!` `TemplateError::SecurityRequired { .. }` with `template_id` equal to the spec's ref
  - command: `cargo test -p camel-dsl --lib discovery`
  - expected: fails before (error is `InvalidBody`), passes after
- **Acceptance**: `cargo build -p camel-api -p camel-dsl` succeeds; unit test passes; no behavior change for success paths.
- [x] 2.1

### Task 2.2 — Aggregate Pass 2 failures before aborting

- **Files**:
  - `crates/camel-dsl/src/discovery.rs` (modified)
  - `crates/camel-cli/src/commands/run.rs` (modified)
- **Steps**:
  1. Restructure the Pass 2 loop (discovery.rs:349+): instead of `?` on the first `materialize_and_compile` error, collect `MaterializationFailure` entries into a `Vec` and continue with remaining specs; after the loop, if the Vec is non-empty return `DiscoveryError::MaterializationFailures`. Success-path results accumulate as today. `TemplateNotFound` (missing template ref) also becomes a collected failure entry (template_ref = requested ref, error = `TemplateError::NotFound(template_ref)` — new variant `#[error("template not found: {0}")] NotFound(String)` added in Task 2.1's camel-api edit) instead of an immediate abort, preserving its own error class.
  2. Keep Pass 1 (template/spec parsing) first-abort — no change there.
  3. Rewrite the existing unit test `missing_template_ref_returns_error` (discovery.rs:908-936) which currently matches `DiscoveryError::TemplateNotFound` and panics on other variants: it must now expect `DiscoveryError::MaterializationFailures` containing exactly one `TemplateError::NotFound` failure with the requested template ref. Remove the now-unused `DiscoveryError::TemplateNotFound` variant (no other consumer; Pass 1 uses `TemplateSpec`/`MaterializationFailed`).
  4. In camel-cli run.rs, where discovery errors print before exit (the site that leads to `exit(1)` around run.rs:502), render `MaterializationFailures` by printing each failure line (path, template ref, route id if set, error class, message) then exit non-zero.
- **Tests**:
  - name: `two_failing_specs_both_reported` (in `crates/camel-dsl/tests/route_template_parity.rs`)
    - setup: tempdir file with two templated specs referencing two different templates, both materializing to invalid routes (e.g. bad step kind) for different reasons
    - action: call `discover_routes_with_threshold_and_security(&[pattern], DEFAULT_STREAM_CACHE_THRESHOLD, SecurityCompileContext::default())`
    - assert: Err where the `MaterializationFailures` Vec has exactly 2 entries, each naming its distinct template_ref and its own cause
    - command: `cargo test -p camel-dsl --test route_template_parity`
    - expected: fails before (single `MaterializationFailed`), passes after
  - name: `distinct_error_classes_for_security_and_body` (same file)
    - setup: two templated specs — one secured (no authenticator configured), one with malformed body; default ctx
    - action: discover
    - assert: Err with 2 failures; one `SecurityRequired`, one `InvalidBody`
    - command: `cargo test -p camel-dsl --test route_template_parity`
    - expected: fails before (both InvalidBody), passes after
- **Acceptance**: both tests pass; `cargo build -p camel-cli` succeeds; a failing run still exits non-zero (fail-closed preserved).
- [x] 2.2

### Task 2.3 — Snapshot: sibling diagnostics never discarded

- **Files**:
  - `crates/camel-dsl/tests/route_template_parity.rs` (modified)
- **Steps**:
  1. Add test `good_sibling_routes_materialize_alongside_reported_failures`: a file with one GOOD templated spec and one bad one. Because startup is fail-closed the run errs — assert the error enumerates only the bad spec AND that this is observable via the `MaterializationFailures` Vec length == 1 (the good spec produced no failure entry). This pins that aggregation collects failures per-spec without conflating siblings.
  2. Add test `template_not_found_is_a_collected_failure`: spec referencing missing template id + one valid spec; assert Err with 1 failure whose template_ref is the missing id and whose error is `TemplateError::NotFound`.
- **Tests**: the two tests above; command `cargo test -p camel-dsl --test route_template_parity`; expected: pass after Task 2.2.
- **Acceptance**: full `route_template_parity` suite passes (now 8 tests); no change to success-path tests.
- [x] 2.3

## Phase 3: Typed parameters (F4)

### Task 3.1 — TemplateParamType + declared type on TemplateParameterSpec

- **Files**:
  - `crates/camel-api/src/template.rs` (modified)
  - `crates/camel-dsl/src/route_ast.rs` (modified)
  - `crates/camel-dsl/src/template/yaml.rs` (modified)
  - `crates/camel-dsl/src/template/json.rs` (modified)
  - `crates/camel-dsl/src/template/materializer.rs` (modified — struct-literal test sites only)
- **Steps**:
  1. In camel-api/src/template.rs add `#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)] #[serde(rename_all = "lowercase")] pub enum TemplateParamType { String, Number, Boolean }` with `impl Default -> TemplateParamType::String`.
  2. Add field to `TemplateParameterSpec`: `#[serde(default, rename = "type")] pub parameter_type: TemplateParamType`.
  3. Add the same field to `RouteDslTemplateParameter` (route_ast.rs:1374-1383): `#[serde(default, rename = "type")] pub parameter_type: camel_api::template::TemplateParamType`, and propagate it in the conversions to `TemplateParameterSpec`: `yaml_param_to_spec` (template/yaml.rs:76) and `json_param_to_spec` (template/json.rs:74) — without these, serde silently drops `type:` from YAML/JSON.
  4. Fix compile breakage at existing `TemplateParameterSpec` struct literals by adding `parameter_type: TemplateParamType::String`: camel-api/src/template.rs test literals (~lines 85, 97, 113) and materializer.rs `make_template` helper + test literals (~lines 238-615).
  5. Verify the YAML/JSON template parsers (`parse_yaml_templates` / `parse_json_templates`) pick the field up through the structs above; if they rebuild `TemplateParameterSpec` field-by-field, add the new field there too.
- **Tests**:
  - name: `template_param_type_parses_from_yaml` (unit test in `crates/camel-dsl/src/template/yaml.rs` mod tests)
    - setup: YAML string `route_templates:\n- id: t\n  parameters:\n  - name: delay\n    type: number\n  routes: []`
    - action: `parse_yaml_templates(&yaml)`
    - assert: one template, parameter `delay` has `parameter_type == TemplateParamType::Number`
    - command: `cargo test -p camel-dsl --lib template::yaml`
    - expected: fails before (field ignored), passes after
  - name: `template_param_type_defaults_to_string` (same module)
    - setup: same YAML without `type:` key
    - action: parse
    - assert: `parameter_type == TemplateParamType::String`
    - command: `cargo test -p camel-dsl --lib template::yaml`
    - expected: passes after step 2
- **Acceptance**: `cargo build -p camel-api -p camel-dsl` succeeds; both unit tests pass; existing template parse tests unchanged.
- [x] 3.1

### Task 3.2 — Whole-node typed substitution + resolution-time coercion validation

- **Files**:
  - `crates/camel-dsl/src/template/materializer.rs` (modified)
- **Steps**:
  1. In `resolve_params` (materializer.rs:34, returns `Result<BTreeMap<String, String>, TemplateError>`), after collecting resolved values, validate against each `TemplateParameterSpec.parameter_type`: `Number` values must parse as `f64` AND be finite (`serde_json::Number::from_f64(x).is_some()`); `Boolean` values must be exactly `true` or `false`. On violation return new `TemplateError` variant `#[error("parameter '{0}' declared type {1} but value '{2}' is not coercible")] InvalidParameter(String, String, String)` carrying (name, declared-type-name, value). Add the variant in camel-api/src/template.rs next to `MissingParameter`/`UnknownParameter` and follow their existing propagation through `materialize_template`.
  2. In the substitution walker `substitute_strings_in_json` (materializer.rs:66 — NOT placeholder.rs), accept the parameter specs (change its `declared_params: &[String]` parameter to `specs: &[TemplateParameterSpec]` or add a specs parameter, updating internal recursion and call sites). When a string node is EXACTLY a single placeholder `"{{p}}"` (whole node, no surrounding text) and `p` is declared `Number`/`Boolean`: emit `serde_json::Value::Number` / `Value::Bool` using the coerced value. Embedded occurrences (`"x{{p}}"`) and all-`String` params keep today's textual behavior unchanged.
- **Tests**:
  - name: `number_param_whole_node_populates_numeric_field` (in `route_template_parity.rs`)
    - setup: template with `parameters: [{name: delay, type: number}]` and route JSON using `"{{delay}}"` as the whole value of a numeric field (e.g. a step field typed `u64`); templated spec `parameters: {delay: "5000"}`
    - action: discover
    - assert: Ok; the materialized route's numeric field equals 5000 as a number (no serde error)
    - command: `cargo test -p camel-dsl --test route_template_parity`
    - expected: fails before (`data did not match any variant`), passes after
  - name: `non_coercible_number_param_rejected_loudly` (same file)
    - setup: same template; spec `parameters: {delay: "abc"}`
    - action: discover
    - assert: Err containing `parameter 'delay' declared type number`
    - command: same
    - expected: fails before (error is generic invalid-body), passes after
  - name: `typed_param_whole_node_and_embedded_in_same_template` (same file)
    - setup: `type: number` param `p`; route JSON uses `"{{p}}"` whole-node in one numeric field and `"x{{p}}"` inside a string-titled field (e.g. route id or a log message)
    - action: discover with `p: "7"`
    - assert: Ok; numeric field is number 7; string field equals `"x7"`
    - command: same
    - expected: fails before, passes after
- **Acceptance**: three tests pass; `cargo test -p camel-dsl --lib template::` all pass (string-embedded cases unchanged — ADR-0008 behavior preserved).
- [x] 3.2

### Task 3.3 — Boolean typed parameter coverage

- **Files**:
  - `crates/camel-dsl/tests/route_template_parity.rs` (modified)
- **Steps**:
  1. Add test `boolean_param_whole_node_substitutes`: template param `type: boolean`, whole-node `"{{flag}}"` in a bool DSL field (e.g. an option accepting true/false), spec value `"true"`; assert materialized bool field is true.
  2. Add test `non_coercible_boolean_param_rejected`: spec value `"yes"`; assert Err containing `parameter 'flag' declared type boolean`.
- **Tests**: the two above; command `cargo test -p camel-dsl --test route_template_parity`; expected: pass after Task 3.2.
- **Acceptance**: suite grows to 13 tests, all pass.
- [x] 3.3

## Phase 4: Identity + hot-reload coherence (F5, F6)

### Task 4.1 — Route-id override validation message

- **Files**:
  - `crates/camel-dsl/src/template/materializer.rs` (modified)
- **Steps**:
  1. The multi-route override guard ALREADY EXISTS at the top of `materialize_template` (~lines 131-137, message `route_id override not allowed for multi-route template`). Do NOT add a second check. Improve the existing guard's message in place to direct authors to per-route ids: `route_id override is only valid for single-route templates; set per-route ids inside the template body`. Single-route override behavior is unchanged.
  2. Update the existing unit test `materialize_multi_route_rejects_route_id_override` (materializer.rs:643-659) which asserts the OLD message substring: change its assertion to `err.to_string().contains("route_id override is only valid for single-route templates")`.
- **Tests**:
  - name: `multi_route_template_with_override_fails` (in `route_template_parity.rs`)
    - setup: template with two routes (distinct in-template ids); spec with `route_id: override-x`
    - action: discover
    - assert: Err containing `route_id override is only valid for single-route templates`
    - command: `cargo test -p camel-dsl --test route_template_parity`
    - expected: fails before this task (guard fires but with the old message), passes after
  - name: `single_route_template_three_instances_distinct_ids` (same file)
    - setup: one single-route template; three templated specs with `route_id: a / b / c` and identical parameters
    - action: discover
    - assert: Ok; three routes with ids exactly `a`, `b`, `c`
    - command: same
    - expected: passes before AND after (today's behavior already allows it) — regression pin
- **Acceptance**: both tests pass; `cargo test -p camel-dsl --lib template::materializer` passes (including the updated existing unit test from step 2).
- [x] 4.1

### Task 4.2 — Instance-sensitive source hash

Depends on Task 1.1's `materialize_and_compile` signature (it edits the same function) and Task 4.1 (guard message finalized first).

- **Files**:
  - `crates/camel-dsl/src/template/materializer.rs` (modified)
  - `crates/camel-dsl/src/lib.rs` (modified — only if `materialize_template`/`compute_source_hash` are re-exported there; update the re-export surface to match the new signatures)
- **Steps**:
  1. Add `pub fn compute_instance_source_hash(template_routes: &[serde_json::Value], resolved_params: &std::collections::BTreeMap<String, String>, effective_route_id: &str) -> u64` hashing (DefaultHasher, same style as `compute_source_hash` at line 108) the concatenation of: serialized raw `template_routes`, the resolved param map (iterate in BTreeMap order, hashing key then value), and `effective_route_id`.
  2. In `materialize_and_compile`, compute the per-route effective id (post-override, i.e. after Step 6 logic — reuse the value assigned to `route.route_id`) and replace `compute_source_hash(&template.routes)` with `compute_instance_source_hash(&template.routes, &resolved, &effective_id)` for each result. This requires `materialize_template` to expose the resolved params: change its return to `(Vec<DeclarativeRoute>, BTreeMap<String, String>)` or compute resolution twice via `resolve_params` — prefer threading the map through the return tuple.
  3. Update existing hash-related materializer tests (`source_hash_covers_multi_route_array` at ~line 671, others) to the new function; each route in a multi-route template hashes with its own effective id.
- **Tests**:
  - name: `override_only_instances_hash_distinctly` (unit test in materializer.rs mod tests)
    - setup: one template route body; `resolved = {"host": "h"}`; three ids `a`, `b`, `c`
    - action: call `compute_instance_source_hash` three times varying only the id
    - assert: three hash values, all pairwise distinct
    - command: `cargo test -p camel-dsl --lib template::materializer`
    - expected: fails before (fn absent), passes after
  - name: `param_value_changes_hash` (same module)
    - setup: same body; ids equal; params `{"delay": "1"}` vs `{"delay": "2"}`
    - action: hash both
    - assert: hashes differ
    - command: same
    - expected: fails before, passes after
- **Acceptance**: both tests pass; all Phase 1-3 suites still pass (hash change does not alter route ids or compilation).
- [x] 4.2

### Task 4.3 — Hot-reload observes per-instance changes

- **Files**:
  - `crates/camel-core/src/hot_reload/application/reload.rs` (modified — tests only)
- **Steps**:
  1. camel-core does NOT depend on camel-dsl (camel-dsl depends on camel-core; a reverse dep would cycle). Do NOT import `compute_instance_source_hash` here. The hash VALUE contract is already pinned by Task 4.2's unit tests; these reload tests need only the equality semantics of `compute_reload_actions`, so use arbitrary distinct `u64` hash constants standing in for instance hashes.
  2. In `mod tests` (reload.rs:200, helpers `make_controller()` + `compute_reload_actions` already exist), add the three tests below.
- **Tests** (command `cargo test -p camel-core --lib hot_reload`, expected: pass after 4.2; they pin reload semantics, not hash computation):
  - name: `param_change_triggers_swap_not_skip`
    - setup: controller with active route id `proxy-a`, from_uri `http://x`, source_hash 100; new definitions: same id/uri, source_hash 200
    - action: `compute_reload_actions(&defs, &controller)`
    - assert: `[ReloadAction::Swap { route_id: "proxy-a" }]`
  - name: `unchanged_sibling_skips`
    - setup: active routes `proxy-a` (hash 100) and `proxy-b` (hash 200); new definitions identical hashes
    - action: compute
    - assert: actions contain `Skip` for both ids and nothing else
  - name: `one_param_change_other_sibling_skips` (literal pin of the "unrelated instance untouched" scenario)
    - setup: active routes `proxy-a` (hash 100), `proxy-b` (hash 200); new definitions: `proxy-a` hash 100 (unchanged), `proxy-b` hash 300 (changed)
    - action: compute
    - assert: actions are exactly `[Skip { proxy-a }, Swap { proxy-b }]` (order-insensitive on ids)
- **Acceptance**: `cargo test -p camel-core --lib hot_reload` passes; existing reload tests unchanged and passing.
- [x] 4.3
