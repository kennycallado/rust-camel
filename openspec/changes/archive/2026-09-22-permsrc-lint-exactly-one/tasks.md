# Tasks: permsrc-lint-exactly-one

## camel-dsl — schema contract

### Task 1: Encode exactly-one-non-null in the generated schema

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `schemas/dsl/route-schema.json` (modified, regenerated)
- `crates/camel-lint/schema/route-schema.json` (modified, regenerated)

**Steps:**
1. RED RUN FIRST: before touching any source, run the three verification commands of the Tests block below and record results — the `oneOf`-content check FAILS (no `oneOf` key in the def), `schema --check` PASSES (pre-change artifacts are in sync), `diff` byte-equality PASSES. These pre-change results are the expected baseline.
2. In `crates/camel-dsl/src/route_ast.rs`, add a schema-feature-gated transform fn next to `RouteDslPermissionValueSource`:
   `#[cfg(feature = "schema")] fn permission_value_source_oneof(schema: &mut schemars::Schema)` — it inserts, into the schema's root object map, key `"oneOf"` with a `serde_json::Value` array of exactly three branch objects:
   - branch literal: `{"properties": {"literal": {"type": "string"}, "header": {"type": "null"}, "property": {"type": "null"}}, "required": ["literal"]}`
   - branch header: `{"properties": {"literal": {"type": "null"}, "header": {"type": "string"}, "property": {"type": "null"}}, "required": ["header"]}`
   - branch property: `{"properties": {"literal": {"type": "null"}, "header": {"type": "null"}, "property": {"type": "string"}}, "required": ["property"]}`
   (Build the value with `serde_json::json!`; the exact insertion mechanics — `Schema::ensure` / `to_object` / direct map insert — follow the schemars 1.2.2 `transform` fn signature `fn(&mut Schema)`.) The transform approach is PINNED — do not use the schemars `extend` attribute instead.
3. On the `RouteDslPermissionValueSource` struct, change its attribute line from `#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]` to also carry the transform: `#[cfg_attr(feature = "schema", schemars(transform = permission_value_source_oneof))]` placed directly above the derive line, keeping the derive unchanged. Do NOT change the struct's serde behavior, field types, or `deny_unknown_fields`.
4. Extend the `RouteDslPermissionValueSource` doc comment with two lines: the exactly-one-non-null contract and its runtime anchor (`yaml_source_to_value_source` in `camel-dsl/src/yaml.rs`, rc-gddb2).
5. Run `cargo xtask schema` (worktree root) to regenerate `schemas/dsl/route-schema.json`, sync `crates/camel-lint/schema/route-schema.json`, and rewrite TS artifacts under `schemas/ts/` (content-identical for TS — ts_rs ignores schemars transforms; any TS file appearing in `git status` is an anomaly: STOP and report it).
6. Re-run the three verification commands: the `oneOf`-content check now prints `ok`; `schema --check` and `diff` still pass.

**Tests:** (executable spec — name, arrange, act, assert)
- `schema_node_carries_exactly_one_oneof`: parse `schemas/dsl/route-schema.json` → assert `$defs.RouteDslPermissionValueSource.oneOf` exists, has exactly 3 branches, branch i has `required == [<key_i>]`, types `key_i` as `"string"`, and types the other two keys `"null"` → command: `python3 -c "import json; d=json.load(open('schemas/dsl/route-schema.json')); n=d['\$defs']['RouteDslPermissionValueSource']; ks=['literal','header','property']; b=n['oneOf']; assert len(b)==3 and all(b[i]['required']==[ks[i]] and b[i]['properties'][ks[i]]=={'type':'string'} and all(b[i]['properties'][ks[j]]=={'type':'null'} for j in range(3) if j!=i) for i in range(3)); print('ok')"` → expected: FAILS pre-change (step 1), prints `ok` after step 5
- `schema_check_gate_passes`: run `cargo xtask schema --check` → exit 0 with `OK: all schemas and TS types match` → expected: PASSES pre-change and post-change (the gate asserts regeneration round-trip, not the oneOf presence)
- `lint_schema_copy_byte_equal`: run `diff schemas/dsl/route-schema.json crates/camel-lint/schema/route-schema.json` → exit 0, no output → expected: PASSES pre-change and post-change

**Acceptance:**
- `cargo xtask schema --check` exits 0
- `cargo build -p camel-dsl --features schema` succeeds
- `git status --porcelain` in the worktree lists ONLY: `crates/camel-dsl/src/route_ast.rs`, `schemas/dsl/route-schema.json`, `crates/camel-lint/schema/route-schema.json` (plus already-committed OpenSpec artifacts produce no entries)
- `cargo fmt --check` and `cargo clippy -p camel-dsl -- -D warnings` exit 0

- [x] 1

## camel-lint — R-SCHEMA diagnostic rendering

### Task 2: Targeted exactly-one diagnostics in the R-SCHEMA rule

**Files:**
- `crates/camel-lint/src/rules/rschema.rs` (modified)
- `crates/camel-lint/src/rules/rschema/tests.rs` (modified)

**Steps:**
1. RED RUN FIRST: write ALL the unit tests of the Tests block below in `rschema/tests.rs` (using the existing `analyze` / `rschema_only` / `slice` helpers and the route skeleton `id: r1\nfrom: direct:start\nsteps: []\nsecurity_policy:\n  permission:\n    policy: keycloak-uma\n    resource:` followed by the per-test value-spec mapping; `action` variants substitute the `action` key). Run them ONCE against the unmodified rule and record: (a) which fail vs pass (the targeted tests FAIL because the schema from task 1 emits only the generic collapsed AnyOf diagnostic — which also means those failures show the generic message; the clean-input tests and the three byte-exact regression tests PASS); (b) for the three byte-exact regression tests, capture the generic diagnostic message strings VERBATIM from this red run and pin them into the assertions (that captured string IS the byte-exactness pin). Route skeleton note: unrelated skeleton diagnostics are acceptable in the regression tests — their assertions scope to the named anchored diagnostic and the absence of targeted text.
2. Add a private helper `fn permission_value_source_field(instance_path: &str) -> Option<&'static str>` in `rschema.rs`: split the raw validator instance path on `/`, drop the empty first segment; return `Some("resource")`/`Some("action")` when the LAST segment is `resource`/`action` respectively, else `None`. (Only `RouteDslPermissionPolicy` has these child keys, so the tail is the field context; prefix-agnostic.)
3. Add a private constant `const PERMISSION_VALUE_SOURCE_ONEOF_MARKER: &str = "RouteDslPermissionValueSource/oneOf";` — the (probe-confirmed) schema-path substring of the nested oneOf failure. The nested error's `schema_path()` contains this marker verbatim (empirically confirmed against jsonschema 0.52.1 with the injected oneOf; the targeted tests fail loudly if a future version changes the form).
4. Add a private helper `fn found_sources(value: &serde_json::Value) -> String`: given a validation-instance value (any shape), collect the keys among `literal`, `header`, `property` whose value is a non-null JSON value, in that canonical order; return `none set` for an empty collection, else the keys joined with `", "` (`found_sources(&Value::Null)` returns `none set`).
5. Add a private recursive walker collecting OWNED paths (no borrowed-error gymnastics — jsonschema 0.52's nested contexts are `ValidationError<'static>` inside `AnyOf`/`OneOfNotValid` kinds):
   `fn collect_permission_oneof_paths(err: &jsonschema::error::ValidationError<'_>, out: &mut Vec<String>)`
   - if `matches!(err.kind(), ValidationErrorKind::OneOfNotValid { .. })` AND `err.schema_path().as_str().contains(PERMISSION_VALUE_SOURCE_ONEOF_MARKER)` — push `err.instance_path().as_str().to_owned()` and return;
   - else if the kind is `AnyOf { .. }` or `OneOfNotValid { .. }` (both carry `context: Vec<Vec<ValidationError<'static>>>`) — recurse into every nested error of every branch;
   - otherwise do nothing.
6. In the `ValidationErrorKind::AnyOf { context }` arm of `RSchemaRule::analyze`, BEFORE the existing pattern-de-collapse logic: run `collect_permission_oneof_paths` (step 5) on the current top-level error into a `Vec<String>`; dedup preserving first-occurrence order (a two-field route yields two sibling matches under one collapsed AnyOf); for EACH path where `permission_value_source_field(&path)` is `Some(field)` (a `None` tail is a non-match — skip it):
   - `value_at` = `instance.pointer(&path)` (the instance path IS a JSON pointer); on `None`, use `&serde_json::Value::Null` (defensive — `found_sources` then reports `none set`);
   - span = `crate::document::value_span_for(&parsed, &instance_path_to_noyalib(&path, envelope_depth))` — the MATCH's own instance path (ends in `/resource` or `/action`, points at the value mapping);
   - push ONE `diagnostic_for(span, format!("security_policy permission {field} must specify exactly one of: literal, header, or property (set: {})", found_sources(value_at)))`.
   After the loop: if at least one diagnostic was emitted for this error, `continue` the outer error loop (remaining nested non-permission errors of that same collapsed AnyOf — e.g. a sibling `AdditionalProperties` — are intentionally subsumed, mirroring the arm's existing documented first-error-wins de-collapse limitation). If NONE was emitted, fall through to the existing pattern-de-collapse logic UNCHANGED.
7. NO standalone `ValidationErrorKind::OneOfNotValid` arm is added: `RouteDslPermissionValueSource` has exactly two schema ref-sites (`resource`, `action`), both anyOf-wrapped, so a permission oneOf error always surfaces ≥1 AnyOf deep and the walker in steps 5-6 owns it; top-level `OneOfNotValid` errors (e.g. `credential_sources` array items — plain `$ref`, no Option wrapper) keep the generic `_ =>` behavior untouched. The `rschema_credential_oneof_top_level_stays_generic` test pins that contract.
8. Re-run the full test battery: every targeted test now passes; every clean-input and regression test still passes.

**Tests:** (executable spec — name, arrange, act, assert; command: `cargo test -p camel-lint --lib <name>`. Targeted tests FAIL pre-change (generic collapsed AnyOf diagnostic instead of the targeted one) and PASS post-change; clean-input and regression tests PASS pre-change AND post-change.)
- `rschema_permission_zero_sources_reports_exactly_one_error`: `resource: {}` → exactly ONE R-SCHEMA diagnostic mentioning `resource`; message contains `must specify exactly one of: literal, header, or property` and `(set: none set)`; `slice(source, &d.span)` covers the `{}` mapping (contains `{`) — TARGETED
- `rschema_permission_all_null_sources_reports_exactly_one_error`: `resource: {literal: null, header: null, property: null}` → same single diagnostic with `(set: none set)` — TARGETED
- `rschema_permission_multi_sources_reports_found_set`: `resource: {literal: orders, header: x-resource-id}` → one R-SCHEMA diagnostic, message contains `resource` and `(set: literal, header)`; span covers the mapping — TARGETED
- `rschema_permission_all_three_sources_canonical_order`: `resource: {property: p, header: h, literal: l}` (authored in non-canonical key order) → message contains `(set: literal, header, property)` — TARGETED
- `rschema_permission_action_field_context`: `action: {literal: read, property: perms}` → diagnostic message contains `action` and `(set: literal, property)`; `slice(source, &d.span)` covers the action mapping — TARGETED
- `rschema_permission_exactly_one_clean`: `resource: {header: x-resource-id}` → `rschema_only` returns an EMPTY diagnostic list — CLEAN (passes pre and post)
- `rschema_permission_one_source_null_siblings_clean`: `resource: {literal: orders, header: null, property: null}` → no R-SCHEMA diagnostics — CLEAN (passes pre and post)
- `rschema_permission_diagnostic_count_zero_and_multi`: one route with BOTH `resource: {}` AND `action: {literal: a, header: b}` → exactly TWO R-SCHEMA diagnostics total, one mentioning `resource`, one mentioning `action` — TARGETED
- `rschema_permission_unknown_key_stays_collapsed_anyof`: `resource: {literal: orders, bogus: x}` (the oneOf PASSES — branches do not restrict extra keys; the nested `AdditionalProperties` surfaces only inside the collapsed Option-wrapper AnyOf) → the top-level emission is the generic pre-change collapsed AnyOf diagnostic; NO diagnostic whose message contains `exactly one of: literal, header, or property` — REGRESSION (passes pre and post)
- `rschema_permission_unknown_key_only_fires_targeted`: `resource: {bogus: x}` (zero sources AND an unknown key: the targeted exactly-one diagnostic fires, subsuming the nested AdditionalProperties signal per the documented first-error-wins rule) → exactly ONE R-SCHEMA diagnostic, message contains `resource` and `(set: none set)` — TARGETED
- `rschema_permission_scalar_value_stays_generic`: `resource: orders` (scalar — the oneOf-level error kind is not OneOfNotValid) → at least one R-SCHEMA diagnostic anchored on the scalar (a diagnostic whose `slice(source, &d.span)` is `orders`) with the generic message form; NO diagnostic whose message contains `exactly one of: literal, header, or property` — REGRESSION (passes pre and post)
- `rschema_credential_oneof_top_level_stays_generic` (byte-exact regression): `credential_sources: [{cookie: {name: 123}}]` under `security_policy` (type error inside CredentialSourceDsl's oneOf; `credential_sources` items are a plain `$ref` — no Option anyOf wrapper — so the error surfaces TOP-LEVEL as OneOfNotValid through the generic `_ =>` arm, which anchors on the credential-source mapping at that instance node) → at least one R-SCHEMA diagnostic exists whose message is EXACTLY the red-run-captured generic string (verbatim pin); NO diagnostic contains `security_policy permission` or `exactly one of: literal, header, or property` text — passes pre and post
- `rschema_exception_disposition_oneof_unchanged` (byte-exact regression): route with a step `- do_try: {steps: [<any minimal valid step>], catch: [{steps: [<same>], disposition: bogus}]}` (ExceptionDisposition is a direct `$ref` under `DoTryData.catch.items`; `bogus` matches none of the const branches) → the diagnostic anchored on `bogus` has EXACTLY the red-run-captured generic message; no targeted permission text; unrelated skeleton diagnostics acceptable — passes pre and post
- `rschema_rest_binding_oneof_unchanged` (byte-exact regression): envelope `rest: [{path: /demo, operations: [{method: get, binding: bogus}]}]` (RouteDslRestOperation.binding is anyOf-wrapped → exercises the AnyOf arm's no-match fall-through; the collapsed diagnostic anchors on the binding value node) → the diagnostic anchored on `bogus` has EXACTLY the red-run-captured generic message; no targeted permission text; unrelated skeleton diagnostics acceptable — passes pre and post

**Acceptance:**
- `cargo test -p camel-lint --lib` passes (all new + all existing tests in the file)
- `cargo fmt --check` and `cargo clippy -p camel-lint -- -D warnings` exit 0
- No new `unwrap()`/`expect()` outside test code (`cargo xtask lint-unwrap` stays clean)

- [x] 2

## camel-cli — corpus fixtures

### Task 3: Negative and positive corpus fixtures + baseline entries

**Files:**
- `crates/camel-cli/tests/fixtures/lint-corpus/permission-value-source-zero.yaml` (new)
- `crates/camel-cli/tests/fixtures/lint-corpus/permission-value-source-multi.yaml` (new)
- `crates/camel-cli/tests/fixtures/lint-corpus/permission-value-source-exactly-one.yaml` (new)
- `crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron` (modified)

**Steps:**
1. Write `permission-value-source-zero.yaml`: envelope mapping with a single route under `routes:` (`id`, `from: direct:start`, `steps: []`, `security_policy.permission.policy: keycloak-uma`) whose `resource` is an empty mapping `{}` — the ONLY R-SCHEMA defect is the zero-source value spec.
2. Write `permission-value-source-multi.yaml`: same skeleton, `resource: {literal: orders, header: x-resource-id}` — the only defect is the two-source value spec.
3. Write `permission-value-source-exactly-one.yaml`: same skeleton, `resource: {header: x-resource-id}` — lints clean.
4. Run `cargo test -p camel-cli --test lint_corpus`: it FAILS naming the two negative fixtures as emitted-but-not-baselined. Add exactly two entries to `lint-corpus-baseline.ron` (each `[("R-SCHEMA", "error")]`, with a `//` justification comment: `permission value source contract — mirrors runtime yaml_source_to_value_source exactly-one (rc-gddb2, bd rc-lkbqi)`), following the existing entry style/placement. The positive fixture gets NO entry (the gate only registers files WITH diagnostics).
5. Re-run `cargo test -p camel-cli --test lint_corpus` → passes.

**Tests:** (executable spec — name, arrange, act, assert)
- `corpus_zero_false_positives` (existing test, extended by fixtures): three new fixtures discovered by the glob → production lint engine runs over them → emitted set == baseline set (zero + multi each contribute `("R-SCHEMA", "error")`; exactly-one contributes nothing) → command: `cargo test -p camel-cli --test lint_corpus` → expected: FAILS between steps 1-3 and 4, PASSES after step 4

**Acceptance:**
- `cargo test -p camel-cli --test lint_corpus` exits 0
- `grep -c 'permission-value-source' crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron` returns 2
- The three fixture files contain no diagnostics-triggering content besides the intended value-spec shapes (route skeletons otherwise schema-clean)

- [x] 3
