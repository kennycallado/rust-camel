# Tasks: 364-simpleset

Run every cargo command from the worktree with
`CARGO_TARGET_DIR=/home/shared/rust-camel-worktrees/364-simpleset/target CARGO_BUILD_JOBS=4 TMPDIR=/home/shared/tmp`.
Line references are to the pre-change tree at HEAD `20aef559`. Binding inputs:
`/home/shared/tmp/364-simpleset/{decision.md,research.md,inventory.md}` and the
blessed delta `specs/language-value-boundary/spec.md`.

The only `...` bytes anywhere in this file are literal content: the two
spec-mandated error-message format literals in Task 1.1 step 4 (the wrapper hint
`` `{verb}: { value: { ... } }` ``, R1 requirement, line 48 of the delta) and the
cargo test-output line `verb_set_body_expression_error_fails_step ... ok`. They
are literal bytes, not instructions.

## Scope check (STAGE 2)

- Phase 1 (`Reject invalid value-source mappings`): 5 tasks, one deliverable —
  shared lowering rejection, one schema branch and its two generated copies,
  lint container-key pin, fixture and doc migration. Crate `camel-dsl`.
- Phase 2 (`Require mutating scripts`): 7 tasks, one deliverable — one shared
  compiler, entry-point tests, four known test migrations, parser-only and doc
  migration, one sweep in two tasks. Crate `camel-core` (+ the camel-test sweep).
- Each task budget is `<= 30 min`. Phases are crate-disjoint. Verdict: ACCEPTABLE.

## Coverage matrix

| Spec scenario / acceptance | Owning task(s) | Owning test(s) |
|---|---|---|
| R1 S1 Unknown map key, both verbs and formats | 1.1 | `unknown_key_rejected_both_formats_both_verbs` |
| R1 S2 Mixed keys, sorted unknown, secrets | 1.1 | `mixed_unknown_key_wins_both_formats_both_verbs`, `multiple_unknown_keys_sorted_both_formats_both_verbs` |
| R1 S3 Wrong type does not disclose value | 1.1 | `wrong_type_reported_both_formats_both_verbs`, `wrong_type_declaration_order_both_formats_both_verbs` |
| R1 S4 Wrapped object and existing literals valid | 1.1 | `wrapped_object_is_literal_both_formats_both_verbs`, `scalar_array_null_and_interpolation_unchanged_both_formats_both_verbs`, `empty_config_and_multiple_sources_fail_both_formats_both_verbs` |
| R1 S5 Schema rejects bare maps, preserves containers | 1.2, 1.3 | schema negatives/positives both verbs, `schema_set_body_data_restricts_literal_branch`, `set_body_data_container_classification_unchanged` |
| R2 S6 Read-only fails before execution on every surface | 2.1, 2.2, 2.3 | `compile_script_rejects_non_mutating_language`, four route-add tests, `canonical_script_readonly_language_rejected_through_bus`, `dry_compile_rejected_script_replacement_preserves_route` |
| R2 S7 Both builder variants byte-identical | 2.1 | `compile_script_both_variants_byte_identical` |
| R2 S8 Mutating scripts remain supported | 2.1, 2.2 | `compile_script_custom_mutating_language_executes_mutation`, `rhai_script_route_add_still_succeeds`, `cargo test -p camel-test --test script_test`, `--test gh62_language_boundary_regression_test` |
| R3 S9 Inventory migrations and new sites | 1.4, 2.4 (T5), 2.5, 2.6, 2.7 | B1 test, CXF compile check, `runtime_commands_test::register_route_accepts_advanced_canonical_steps`, `docs/src/eip/script.md` semicolon fix, `route_examples_parse_without_new_rejection`, baseline ledger |
| R3 S10 Redaction coverage survives | 2.4 | `expression_failure_propagation_test` (feature-gated) surviving `secret=abc` seed |
| R4 S11 Mutating verb fails loudly (MODIFIED requirement) | 2.4 | `verb_set_property_*`, `verb_set_header_*`, `cargo test -p camel-core --features lang-rhai,lang-jsonpath,lang-js,lang-xpath --test expression_failure_propagation_test` |
| R4 S12 Predicate verb fails loudly (MODIFIED) | 2.4 | `verb_filter_*`, `verb_choice_when_*`, same feature-gated suite |
| R4 S13 Routing verb fails loudly (MODIFIED) | 2.4 | `verb_sort_*`, `verb_dynamic_router_*`, same feature-gated suite |
| A1 `set_body` YAML unknown key | 1.1 | `unknown_key_rejected_both_formats_both_verbs` (YAML×set_body cell) |
| A2 `transform` YAML unknown key | 1.1 | same test (YAML×transform cell) |
| A3 JSON form of A1/A2 | 1.1 | same test (JSON cells) |
| A4 mixed key names only unknown | 1.1 | `mixed_unknown_key_wins_both_formats_both_verbs` |
| A5 `{simple: 123}` wrong type, no `123` | 1.1 | `wrong_type_reported_both_formats_both_verbs` |
| A6 sorted unknown keys, no values | 1.1 | `multiple_unknown_keys_sorted_both_formats_both_verbs` |
| A7 `{value: {constant: ok}}` is literal, both verbs/formats | 1.1 | `wrapped_object_is_literal_both_formats_both_verbs` |
| A8 scalars/arrays/null/`${}` regression | 1.1 | `scalar_array_null_and_interpolation_unchanged_both_formats_both_verbs` |
| A9 schema gate + container keys | 1.2, 1.3 | schema tests, `cargo xtask schema --check` |
| A10 B1 migrated and passes | 1.4 | `expect_reply_json_subset_on_direct_body` |
| A11 script fails at add, body intact | 2.1, 2.2, 2.3 | entry-point and DryCompile tests |
| A12 byte-identical variants | 2.1 | `compile_script_both_variants_byte_identical` |
| A13 jsonpath + canonical fail the same | 2.2, 2.3 | `yaml_script_jsonpath_rejected_at_route_add`, `canonical_script_readonly_language_rejected_through_bus` |
| A14 rhai/js + custom mutating still execute | 2.1, 2.2 | mutation-execute test, `script_test`, `gh62_language_boundary_regression_test` |
| A15 T1–T5 migrated, seed moved | 2.4 | five migrated tests + redaction assertion |
| A16 inventory migration + parser sweep | 2.5, 2.6, 2.7 | eleven `sweep_flags_*` fixtures (incl. corrected root-sequence `Mutating` and malformed-source `CreationError`), `route_examples_parse_without_new_rejection`, `sweep_baseline_fence_identity_is_exact`, `route_sweep_baseline.tsv` |
| A17 quality gates | Conductor verification section | AGENTS.md gate commands |

## Commit plan (conductor-owned)

Two independent breaking implementation commits. The spec and plan commits are
NOT breaking. The conductor checks boxes and commits; workers do not commit.

- Commit 1 (Phase 1): subject `fix(dsl)!: reject bare maps in set_body/transform`.
  Footer, then its own trailer:
  ```
  BREAKING CHANGE: a mapping under set_body/transform is value-source config.
  Wrap object literal bodies in `value:` (`set_body: {value: {constant: ok}}`).

  Bd: rc-me8oe
  ```
- Commit 2 (Phase 2): subject `fix(core)!: fail script with non-mutating language`.
  Footer, then its own trailer:
  ```
  BREAKING CHANGE: `script:` requires a language with mutating expressions.
  Replace read-only `script:` with `transform: {language, source}`.

  Bd: rc-i7cfx
  ```
- Do not hand-edit CHANGELOG.md. A squash landing must preserve both `!:`
  markers and both footers (`git commit -C`). Commit 2 is separable: if it is
  dropped, revert only the Phase-2 script doc edits in Task 2.5 and keep Phase 1.

## Phase 1: Reject invalid value-source mappings

### Task 1.1: Reject object-literal maps in shared lowering

**Budget:** <= 30 min.

**Files:**
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/tests/value_source_map_rejection_test.rs` (new)

**Steps:**
1. Add `const VALUE_SOURCE_ALTERNATIVES: &str = "`value`, `language`+`source`, `simple`, `rhai`, `jsonpath`, `xpath`";`.
2. Add `const VALUE_SOURCE_CONFIG_KEYS: [&str; 7] = ["value", "language",
   "source", "simple", "rhai", "jsonpath", "xpath"];` and
   `const VALUE_SOURCE_FIELD_ORDER: [&str; 6] = ["language", "source", "simple",
   "rhai", "jsonpath", "xpath"];`.
3. Add `fn literal_value_source(value: serde_json::Value, verb: &str) ->
   Result<ValueSourceDef, CamelError>`: `Value::Object(map) =>
   Err(reject_bare_map(&map, verb))`; every other variant `=>
   Ok(bare_string_value_source(value))`.
4. Add `fn reject_bare_map(map: &serde_json::Map<String, serde_json::Value>,
   verb: &str) -> CamelError`:
   - `unknown` = keys not in `VALUE_SOURCE_CONFIG_KEYS`, then `sort_unstable`.
   - non-empty `unknown` returns
     `format!("{verb}: unknown key(s) {keys} in value-source mapping; expected exactly one of {VALUE_SOURCE_ALTERNATIVES}. To set an object literal body, wrap it: `{verb}: {{ value: {{ ... }} }}`")`
     with `keys` = the unknown names backticked and joined by `, `.
   - else the first `field` in `VALUE_SOURCE_FIELD_ORDER` present in `map` whose
     value is neither `Value::String` nor `Value::Null` returns
     `format!("{verb}: wrong type for key `{field}`; expected exactly one of {VALUE_SOURCE_ALTERNATIVES}. To set an object literal body, wrap it: `{verb}: {{ value: {{ ... }} }}`")`.
   - else the same string with the clause `{verb}: invalid value-source mapping`.
   - No branch formats a map value.
5. In `route_step_to_declarative_step` replace
   `SetBodyData::Literal(value) => bare_string_value_source(value),` in the
   `RouteDslStep::SetBody` arm with
   `SetBodyData::Literal(value) => literal_value_source(value, "set_body")?,`
   and in the `RouteDslStep::Transform` arm with `"transform"`.
6. Add `crates/camel-dsl/tests/value_source_map_rejection_test.rs`: helpers
   `yaml_doc(steps) -> String`, `json_doc(steps) -> String`, and
   `fn assert_rejected(doc: &str, is_json: bool) -> String` returning the
   rendered error. Each negative test builds the four cells (YAML/JSON ×
   set_body/transform) as a table and loops, so the matrix is explicit.

**Tests:** command `cargo test -p camel-dsl --test value_source_map_rejection_test`.
All error assertions use the rendered `CamelError::Display`. Public parse wraps
the lowering `RouteError` with `"{fmt} DSL error: "` and `RouteError` displays as
`Route error: {msg}`, so the exact prefix chain is `YAML DSL error: set_body:`
(never assert the raw inner verb prefix with `starts_with`).
- `unknown_key_rejected_both_formats_both_verbs` — 4 cells with
  `{constant: "LEAK-ONE"}`. assert each rendered message contains
  `{FMT} DSL error: {verb}: unknown key(s) `constant` in value-source mapping`,
  the alternatives text, and `` value: { ``; assert it does not contain
  `LEAK-ONE`. expected: PASS after step 5.
- `mixed_unknown_key_wins_both_formats_both_verbs` — 4 cells with
  `{simple: "${body}", foo: "LEAK-TWO"}`. assert contains
  `` unknown key(s) `foo` ``, does not contain `` unknown key(s) `simple` ``,
  does not contain `LEAK-TWO`; alternatives still list `` `simple` ``. expected: PASS.
- `wrong_type_reported_both_formats_both_verbs` — 4 cells with `{simple: 123}`.
  assert contains `` wrong type for key `simple` ``, the alternatives text and
  `` value: { ``; assert it does not contain `123`. expected: PASS.
- `wrong_type_declaration_order_both_formats_both_verbs` — 4 cells with
  `{rhai: [1], simple: 2}` (reports `` `simple` `` — declaration order `simple`
  before `rhai`) and a second 4-cell table with `{xpath: [1], language: 2}`
  (reports `` `language` ``). assert the reported key is the earlier one in
  `language, source, simple, rhai, jsonpath, xpath`. expected: PASS.
- `multiple_unknown_keys_sorted_both_formats_both_verbs` — 4 cells with
  `{zeta: "LEAK-A", alpha: "LEAK-B", simple: "${body}"}`. assert contains
  `` unknown key(s) `alpha`, `zeta` in value-source mapping`` and neither
  `LEAK-A` nor `LEAK-B`. expected: PASS.
- `wrapped_object_is_literal_both_formats_both_verbs` — 4 cells with
  `{value: {constant: ok}}`. assert the single step is
  `DeclarativeStep::SetBody(step)` with `step.value ==
  ValueSourceDef::Literal(serde_json::json!({"constant":"ok"}))`. expected: PASS.
- `scalar_array_null_and_interpolation_unchanged_both_formats_both_verbs` —
  4 cells for `42`, `[1, 2, {"a": 1}]`, `null` (all `ValueSourceDef::Literal`,
  the array including its nested object) and `"${body}"`
  (`ValueSourceDef::Expression` with `language == "simple"`). expected: PASS.
- `empty_config_and_multiple_sources_fail_both_formats_both_verbs` — 4 cells for
  `{}` and 4 for `{value: "x", simple: "y"}`. assert the rendered message
  contains `{FMT} DSL error: {verb}` and `must define exactly one value source`
  (the pre-existing exactly-one behavior is preserved). expected: PASS.
- `error_prefix_is_format_and_verb_exact` — one YAML `set_body` and one JSON
  `transform` case. assert `err.to_string().contains("YAML DSL error: set_body:")`
  and `err.to_string().contains("JSON DSL error: transform:")`. expected: PASS.

**Acceptance:**
- `cargo test -p camel-dsl --test value_source_map_rejection_test` exits 0.
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.

- [x] 1.1

### Task 1.2: Narrow the `SetBodyData` JSON schema and regenerate copies

**Budget:** <= 30 min.

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/tests/schema_validation.rs` (modified)
- `schemas/dsl/route-schema.json` (modified)
- `crates/camel-lint/schema/route-schema.json` (modified)

**Steps:**
1. On `enum SetBodyData` (`route_ast.rs:739-750`): change
   `#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]` to
   `#[cfg_attr(feature = "schema", derive(ts_rs::TS))]` and delete the
   `#[cfg_attr(feature = "schema", schemars(untagged))]` line. Keep
   `#[serde(untagged)]` and both variants.
2. Add under `#[cfg(feature = "schema")]` (the parameter is named
   `generator`; `gen` is a reserved keyword in Rust 2024):
   ```
   impl schemars::JsonSchema for SetBodyData {
       fn schema_name() -> std::borrow::Cow<'static, str> { "SetBodyData".into() }
       fn schema_id() -> std::borrow::Cow<'static, str> { concat!(module_path!(), "::SetBodyData").into() }
       fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
           let config = generator.subschema_for::<SetBodyConfig>();
           schemars::json_schema!({
               "anyOf": [ config, { "type": ["string", "number", "boolean", "array", "null"] } ]
           })
       }
   }
   ```
   (`json_schema!` interpolates expressions through `serde_json::json!`; `Schema`
   is `Serialize`, so `config` becomes the `$ref` branch.) Do not use `not`.
3. Update the `SetBodyData` explanatory comment at `route_ast.rs:744-747`. Keep
   it a line comment (`//`), not a doc comment (`///`): ts-rs emits `///` text as
   JSDoc, which would change `schemas/ts/SetBodyData.ts` and break the
   unchanged-TS acceptance. The comment states that a mapping under
   `set_body`/`transform` is always `SetBodyConfig`, and that an object literal
   body requires the explicit `value:` wrapper.
4. Add `#[cfg(feature = "schema")] #[test] fn
   schema_set_body_data_restricts_literal_branch()` in `route_ast.rs` `mod tests`:
   `let value = serde_json::to_value(schemars::schema_for!(RouteDslStep)).unwrap();`
   navigate `/$defs/SetBodyData`; assert `anyOf` has exactly 2 branches, the
   second branch `type` equals
   `serde_json::json!(["string","number","boolean","array","null"])`, and no
   branch equals `true`.
5. Add to `crates/camel-dsl/tests/schema_validation.rs`:
   - `schema_rejects_bare_map_both_verbs`: instances with `set_body: {constant:
     "ok"}` and `transform: {constant: "ok"}` each have `iter_errors` non-empty.
   - `schema_accepts_wrapped_object_both_verbs`: `set_body` and `transform` with
     `{"value": {"constant": "ok"}}` each have zero `iter_errors`.
   - `schema_accepts_valid_configs_both_verbs`: `set_body` and `transform` with
     `{"simple": "${body}"}` and with `{"language": "simple", "source": "${body}"}`
     each have zero `iter_errors`.
6. Run `cargo xtask schema` to regenerate `schemas/dsl/route-schema.json` and sync
   the byte-identical `crates/camel-lint/schema/route-schema.json`. TS is
   regenerated too; `schemas/ts/SetBodyData.ts` must stay
   `SetBodyConfig | JsonValue` (broad, advisory — see Task 1.3).

**Tests:** commands
`cargo test -p camel-dsl --features schema --lib route_ast::tests::schema_set_body_data_restricts_literal_branch`,
`cargo test -p camel-dsl --test schema_validation`, `cargo xtask schema --check`.
- `schema_set_body_data_restricts_literal_branch` and
  `schema_rejects_bare_map_both_verbs` are red-first (the current `true` branch
  accepts everything). `schema_accepts_wrapped_object_both_verbs` and
  `schema_accepts_valid_configs_both_verbs` pass before steps 1–6 and must still
  pass after; the `route_ast` branch assertion and the negatives are the only
  FAIL-before cases.
- `cargo xtask schema --check` exits 0 after regeneration.

**Acceptance:**
- The two test commands above exit 0.
- `cargo xtask schema --check` exits 0.
- `cmp schemas/dsl/route-schema.json crates/camel-lint/schema/route-schema.json` exits 0.
- `schemas/ts/SetBodyData.ts` is unchanged and still `SetBodyConfig | JsonValue`.

- [x] 1.2

### Task 1.3: Pin lint container-key derivation and the TS advisory posture

**Budget:** <= 20 min.

**Files:**
- `crates/camel-lint/src/schema_context.rs` (modified)

**Steps:**
1. In `schema_context.rs` `mod tests` add
   `fn set_body_data_container_classification_unchanged()`.
2. Assert `CONTAINER_KEYS.contains("set_body")` and `CONTAINER_KEYS.contains("transform")`.
3. Assert `is_container(SCHEMA.pointer("/$defs/SetBodyData").expect("SetBodyData def"), &SCHEMA)`
   is true, and that the literal branch
   `SCHEMA.pointer("/$defs/SetBodyData/anyOf/1").expect("literal branch")` is
   neither `is_container(..)` nor `is_free_form(..)`.

**Tests:** command
`cargo test -p camel-lint --lib schema_context::tests::set_body_data_container_classification_unchanged`.
Setup: embedded `SCHEMA`/`CONTAINER_KEYS`. action: the three assertions.
expected: PASS after Task 1.2 regenerates the embedded copy; FAIL before
(`/anyOf/1` is absent while the `true` branch is present).

**Acceptance:**
- `cargo test -p camel-lint --lib schema_context::tests::set_body_data_container_classification_unchanged` exits 0.
- `cargo clippy -p camel-lint --all-targets -- -D warnings` exits 0.

- [x] 1.3

### Task 1.4: Migrate bare-map fixtures (B1–B3 and four CXF constant maps)

**Budget:** <= 30 min.

**Files:**
- `crates/camel-integration-test/tests/direct_reply_test.rs` (modified)
- `crates/camel-test/tests/cxf_test.rs` (modified)
- `docs/src/components/opensearch.md` (modified)

**Steps:**
1. B1: in `expect_reply_json_subset_on_direct_body` (`direct_reply_test.rs:157-159`)
   change the `set_body:` map with keys `status`/`seq` to `set_body:` then a
   nested `value:` with the same `status`/`seq`; keep the `jsonSubset` assertion.
2. Four CXF maps (`cxf_test.rs:264`, `:526`, `:580`, `:643`): each inline
   `- set_body:` followed by
   `constant: "<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>ok</return></hel:sayHelloResponse>"`
   becomes `- set_body:` then a nested `value:` then the identical `constant:`
   line. Preserve the XML string byte-for-byte and the JSON-object semantics (a
   JSON object whose `constant` member holds that XML string); do not flatten to
   a plain string.
3. B2 (`opensearch.md:33-35`): wrap the `set_body:` map (`name`/`age`) in `value:`.
4. B3 (`opensearch.md:66-69`): wrap the `set_body:` map (`query`: `match`: `name`)
   in `value:`, preserving the nested structure.

**Tests:** commands
`cargo test -p camel-integration-test --test direct_reply_test expect_reply_json_subset_on_direct_body`,
`cargo check -p camel-test --tests --features integration-tests`.
- `expect_reply_json_subset_on_direct_body`: expected FAIL at parse before step 1,
  PASS after.
- CXF: the four routes live behind `#![cfg(feature = "integration-tests")]` and
  require the native CXF bridge; they run in CI/native only. Local verification
  is the compile check of the migrated inline YAML.

**Acceptance:**
- `cargo test -p camel-integration-test --test direct_reply_test expect_reply_json_subset_on_direct_body` exits 0.
- `cargo check -p camel-test --tests --features integration-tests` exits 0.
- At every migrated site `rg -n -A3 "set_body:"` shows a `value:` child.

- [x] 1.4

### Task 1.5: Phase-1 DSL docs, glossary and TS advisory posture

**Budget:** <= 30 min.

**Files:**
- `docs/src/yaml-dsl/step-verbs/basic.md` (modified)
- `docs/src/eip/transform.md` (modified)
- `crates/camel-dsl/CONTEXT.md` (modified)

**Steps:**
1. `basic.md` `## set_body` table (126-143): state that a mapping is always
   `SetBodyConfig` config, and add a `Migration (BREAKING)` callout next to the
   forms table with the object-literal wrapper text
   ``Wrap object literals in `value:` (`set_body: {value: {constant: ok}}`).`` plus the
   `value:` example. Do not add the script sentence (Phase 2, commit 2).
2. `eip/transform.md:28`: replace the "three shapes" prose with a version that
   lists the wrapped object shape and states that a bare mapping is rejected.
3. `crates/camel-dsl/CONTEXT.md`: add a `**object literal wrapper gate**` glossary
   entry marked BREAKING (a map under `set_body`/`transform` is value-source
   config; object bodies require `value:`; object literals nested in arrays stay
   literal, D2) and cross-reference `bare top-level string value gate`. In the
   same entry state: the broad TS binding `schemas/ts/SetBodyData.ts`
   (`SetBodyConfig | JsonValue`) is advisory, and the generated
   `schemas/dsl/route-schema.json` is the enforcement artifact.

**Tests:** no runtime test. Machine-checkable acceptance via `rg`:
- `rg -q "Migration \(BREAKING\)" docs/src/yaml-dsl/step-verbs/basic.md`
- `rg -q "value:" docs/src/eip/transform.md`
- `rg -q "object literal wrapper gate" crates/camel-dsl/CONTEXT.md`
- `rg -q "enforcement artifact" crates/camel-dsl/CONTEXT.md`

**Acceptance:**
- The four `rg -q` checks above exit 0.
- `cargo doc -p camel-dsl --no-deps` is unaffected (CONTEXT.md is not rustdoc).

- [x] 1.5

## Phase 2: Require mutating scripts

### Task 2.1: One shared `compile_script` compiler

**Budget:** <= 30 min.

**Files:**
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_resolution.rs` (modified)

**Steps:**
1. In `step_resolution.rs`, at module scope, add the shared test fixtures once:
   `#[cfg(test)] pub(crate) struct MutatingTestLanguage;` implementing
   `camel_language_api::Language` (`name` = `"testmut"`) whose
   `create_mutating_expression` returns `Ok(Box::new(MutatingTestExpr))` and
   whose `create_expression`/`create_predicate` return local const stubs;
   `#[cfg(test)] pub(crate) struct MutatingTestExpr;` implementing
   `MutatingExpression` with
   `async fn evaluate(&self, exchange: &mut Exchange) -> Result<Value, LanguageError>`
   that calls `exchange.set_property("testmut", Value::String("ran".into()))` and
   returns `Ok(Value::Null)`.
2. In `core.rs` add
   `fn compile_script(ctx: &CompilationContext, language: &str, source: &str, step_index: usize) -> Result<CompileOutcome, CamelError>`:
   `let meta = eval_meta(ctx, language, "script", step_index, None);`
   `let lang = resolve_language(ctx.languages, language)?;`
   `match lang.create_mutating_expression(source)`:
   - `Ok(mut_expr)` => `CompileOutcome::Matched(CompiledStep::Process { kind_hint: SpanKindHint::Internal, processor: BoxProcessor::new(ScriptMutator::with_meta(mut_expr, meta)), body_contract: None, lifecycle: None, label: None, to_uri: None })`.
   - `Err(LanguageError::NotSupported { .. })` => `CamelError::RouteError(format!("script: language '{language}' does not support mutating expressions; a `script` step requires a language that implements MutatingExpression. To replace the body with a read-only expression, use a `set_body`/`transform` step with this language instead"))`.
   - `Err(e)` => `"Failed to create mutating expression for language '{language}': {e}"`.
3. Replace the `BuilderStep::DeclarativeScript { expression }` arm body with
   `compile_script(ctx, &expression.language, &expression.source, step_index)`.
4. Replace the `BuilderStep::Script { language, script }` arm body with
   `compile_script(ctx, &language, &script, step_index)`.
5. Delete the `NotSupported` SetBody fallback block
   (`compile_value_source(.., "script", .., Some("body"))` + `SetBody`); drop
   now-unused imports and update both arm comments.
6. Add tests in `core.rs` `mod tests`. Reuse
   `crate::lifecycle::adapters::step_resolution::MutatingTestLanguage` and
   `MutatingTestExpr` (defined once in step 1); do not add another mutating
   fixture. Add two distinct `#[cfg(test)]` fixtures for the failure paths:
   - `CreationFailLanguage` (registered as `"creationfail"`), whose
     `create_mutating_expression` returns
     `Err(LanguageError::EvalError("boom".into()))` (fails before a processor
     exists);
   - `EvalFailLanguage` (registered as `"evalfail"`), whose
     `create_mutating_expression` returns `Ok(Box::new(EvalFailExpr))` and whose
     `EvalFailExpr::evaluate(&mut Exchange)` returns
     `Err(LanguageError::EvalError("boom".into()))` (fails after a processor
     exists).
   Execute compiled steps with `tower::ServiceExt::oneshot`.

**Tests:** command `cargo test -p camel-core --lib step_compilers::core::tests::compile_script`.
- `compile_script_rejects_non_mutating_language` — setup: ctx with `SimpleLanguage`. action: compile `DeclarativeScript { language: "simple", source: "${body}" }`. assert: `Err` equals the shared message with `{lang}=simple`. expected: PASS after step 2; FAIL before (SetBody fallback).
- `compile_script_both_variants_byte_identical` — setup: ctx with `simple`. action: compile `BuilderStep::Script { language: "simple", script: "${body}" }` and the equivalent `DeclarativeScript`. assert: `err_script.to_string() == err_declarative.to_string()`. expected: PASS.
- `compile_script_custom_mutating_language_executes_mutation` — setup: registry with `MutatingTestLanguage`. action: compile `DeclarativeScript { language: "testmut", source: "body = 1;" }`, then `processor.oneshot(Exchange::default()).await`. assert: the returned exchange has `property("testmut") == Some(&Value::String("ran".into()))`. expected: PASS (the language actually mutates).
- `compile_script_mutating_eval_error_propagates_with_metadata` — setup: registry with `EvalFailLanguage`. action: compile `DeclarativeScript { language: "evalfail", source: "x" }`, then `processor.oneshot(Exchange::default()).await`. assert: `Err(CamelError::ExpressionFailed { language, verb, .. })` with `language == "evalfail"` and `verb == "script"`. expected: PASS (existing eval-metadata propagation, from an executing processor).
- `compile_script_creation_error_unchanged` — setup: registry with `CreationFailLanguage`. action: compile `DeclarativeScript { language: "creationfail", source: "x" }` (compile only; no processor). assert: `Err` contains `Failed to create mutating expression for language 'creationfail'` and `boom`. expected: PASS.
- `compile_script_unknown_language_error_unchanged` — action: compile `Script { language: "nope", script: "x" }`. assert: `Err` contains `not registered`. expected: PASS.

**Acceptance:**
- `cargo test -p camel-core --lib step_compilers::core::tests::compile_script` exits 0.
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.

- [x] 2.1

### Task 2.2: YAML and JSON route-add entry-point tests

**Budget:** <= 30 min.

**Files:**
- `crates/camel-test/tests/script_readonly_rejected_test.rs` (new)

**Steps:**
1. Each test builds a full harness with checked component registration:
   `let h = CamelTestContext::builder().with_direct().with_mock().build().await;`
   The harness registry comes from `languages_from_config`, which pre-registers
   `rhai`/`jsonpath` when the `camel-core/lang-*` features are unified on
   (workspace builds), so register tolerantly inside a lock scope using the
   established `crates/camel-test/tests/script_test.rs:20-24` pattern:
   ```
   let mut guard = h.ctx().lock().await;
   match guard.register_language("jsonpath", Box::new(JsonPathLanguage::new())) {
       Ok(()) | Err(camel_core::LanguageRegistryError::AlreadyRegistered { .. }) => {}
   }
   drop(guard);
   ```
   and the same for `rhai` (`RhaiLanguage::new()`). `LanguageRegistryError` is
   re-exported by `camel-core` and has the single `AlreadyRegistered { name }`
   variant, so any other outcome is impossible and no error can be swallowed.
   `simple` is registered by the default registry, so do not re-register it.
2. Routes are complete documents: `from: direct:start`, the `script` step, then
   `to: mock:result`. Parse with `camel_dsl::parse_yaml` / `camel_dsl::parse_json`,
   take the single `RouteDefinition`, call `h.add_route(route).await`.
3. For rejected routes assert `Err(CamelError::RouteError)` whose `to_string()`
   contains the exact shared `script: language '{lang}' does not support mutating
   expressions` text. Then prove the rejected route registered nothing by adding
   a valid `direct:start`→`mock:result` route with the SAME route id (for
   example the rejected id `script-simple`) and asserting it is `Ok`; if the
   rejected add had left registration state, the duplicate-id add would fail.

**Tests:** command `cargo test -p camel-test --test script_readonly_rejected_test`.
- `yaml_script_simple_rejected_at_route_add` — action: add YAML
  `script: {language: simple, source: "${body}"}`. assert: error contains
  ``script: language 'simple' does not support mutating expressions``. expected: PASS after Task 2.1.
- `yaml_script_jsonpath_rejected_at_route_add` — `script: {language: jsonpath, source: "$.a"}`. assert: error contains `does not support mutating expressions` and `'jsonpath'`. expected: PASS.
- `json_script_simple_rejected_at_route_add` — `{"script":{"language":"simple","source":"${body}"}}` via `parse_json`. assert: same as the YAML simple case. expected: PASS.
- `json_script_jsonpath_rejected_at_route_add` — JSON jsonpath form. assert: same as the YAML jsonpath case. expected: PASS.
- `rhai_script_route_add_still_succeeds` — YAML `script: {language: rhai, source: 'body = "ok";'}`. assert: `h.add_route(route)` is `Ok`. expected: PASS (A14 registration regression).

**Tests (existing execution suites, exact executable commands):**
- `cargo test -p camel-test --test script_test` — rhai script side effects execute through the direct→mock harness. expected: PASS.
- `cargo test -p camel-test --test gh62_language_boundary_regression_test` — js/rhai end-to-end value-boundary execution. expected: PASS.
Neither file is feature-gated; both use unconditional dev-dependencies on the
language crates. A14 is verified by execution, not compilation.

**Acceptance:**
- `cargo test -p camel-test --test script_readonly_rejected_test` exits 0.
- `cargo test -p camel-test --test script_test` exits 0.
- `cargo test -p camel-test --test gh62_language_boundary_regression_test` exits 0.

- [x] 2.2

### Task 2.3: Canonical commands and DryCompile replacement guards

**Budget:** <= 30 min.

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs` (modified)

**Steps:**
1. In `wired_bus_and_controller`, after constructing the controller and before
   `spawn_controller_actor`, insert `SimpleLanguage` into `controller.languages`
   (make the binding `let mut controller`) so canonical routes resolve `simple`.
2. Add `#[tokio::test] async fn canonical_script_readonly_language_rejected_through_bus()`:
   build `CanonicalRouteSpec::new("canon-script", "timer:tick")`, set
   `spec.steps = vec![CanonicalStepSpec::Script { expression:
   LanguageExpressionDef { language: "simple".into(), source: "${body}".into() } }]`,
   then `bus.execute(RuntimeCommand::RegisterRoute { spec, command_id:
   "canon-script-1".into(), causation_id: None }).await`.
   Assert the result is `Err` containing `does not support mutating expressions`.
   Then assert the compensation ran: `bus.ask(RuntimeQuery::GetRouteStatus {
   route_id: "canon-script".into() })` is exactly
   `RouteStatus { status, .. }` with `status == "Failed"` (commands.rs:154-168
   compensates the durable aggregate). Then assert the runtime side effect is
   absent: `matches!(ctrl_handle.route_exists("canon-script").await, Ok(false))`
   and `matches!(ctrl_handle.route_count().await, Ok(0))` (CamelError is not
   `PartialEq`, so `== Ok(..)` would not compile).
3. Add `#[tokio::test] async fn dry_compile_rejected_script_replacement_preserves_route()`:
   `build_controller_with_components()` + `register_simple_language(&mut controller)`;
   `controller.add_route(RouteDefinition::new("timer:tick", vec![BuilderStep::DeclarativeSetBody { value: ValueSourceDef::Literal(serde_json::json!("keep")) }]).with_route_id("keep")).await.unwrap()`;
   then `controller.compile_route_definition(RouteDefinition::new("timer:tick", vec![BuilderStep::DeclarativeScript { expression: LanguageExpressionDef { language: "simple".into(), source: "${body}".into() } }]).with_route_id("keep"))`.
   Assert the compile is `Err` containing `does not support mutating expressions`.
   Then execute the retained pipeline: `controller.get_pipeline("keep").expect("route retained")`
   via `.oneshot(Exchange::new(Message::new("ignored")))`, and assert the resulting
   exchange body equals `Body::Text("keep")` (the retained valid route, not the
   rejected replacement). `compile_route_definition` uses `DryCompile` staging,
   the same staging hot reload uses before swap.

**Tests:** two separate commands (one filter each):
- `cargo test -p camel-core --lib route_controller::tests::canonical_script_readonly_language_rejected_through_bus` — expected PASS after Task 2.1; exactly one test must execute.
- `cargo test -p camel-core --lib route_controller::tests::dry_compile_rejected_script_replacement_preserves_route` — expected PASS; exactly one test must execute.

**Acceptance:**
- Each of the two commands above exits 0 and reports one executed, passing test.

- [x] 2.3

### Task 2.4: Migrate T1–T4 and RuntimeLang

**Budget:** <= 30 min.

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_resolution.rs` (modified)
- `crates/camel-core/src/context_tests.rs` (modified)
- `crates/camel-core/tests/expression_failure_propagation_test.rs` (modified)
- `crates/camel-core/tests/runtime_commands_test.rs` (modified)

**Steps:**
1. T1 `resolve_steps_covers_declarative_and_eip_variants`
   (`route_controller_tests.rs:274`): register
   `MutatingTestLanguage` under `"testmut"` and change the
   `BuilderStep::DeclarativeScript` step to
   `LanguageExpressionDef { language: "testmut".into(), source: "${body}".into() }`.
   Keep every other step; do not drop the variant.
2. T2 `resolve_steps_covers_non_endpoint_variants`
   (`step_resolution.rs:727`): register `MutatingTestLanguage` under `"testmut"`
   in `languages_with_simple()`'s registry and change the `DeclarativeScript`
   step to language `"testmut"`. Keep every other step.
3. T3 `context_tests.rs` `RuntimeLang` (`~234-268`): add
   `struct DummyMutatingExpression` implementing
   `camel_language_api::MutatingExpression` (`evaluate` returns `Ok(Value::Null)`)
   and implement `create_mutating_expression` on `RuntimeLang` returning
   `Ok(Box::new(DummyMutatingExpression))`. Keep `assert!(result.is_ok())`.
4. T4 `expression_failure_propagation_test.rs`: delete the `"script_readonly"`
   arm in `failing_steps` (277), the `"script_readonly"` arm in
   `verb_diagnostics` (410), and the three `verb_script_readonly_*` macro
   invocations (668-673). Move the redaction seed: add
   `set_header_literal("secret", "abc")` as the first element of the `"set_body"`
   row's step list, and update the comments at 253 and 536. Keep the table-wide
   assertion `!rendered.contains("abc") && !rendered.contains("secret")`.
5. T5 `runtime_commands_test.rs` `register_route_accepts_advanced_canonical_steps`
   (`:695`, Script step at `:778-783`): this test registers
   `CanonicalStepSpec::Script { language: "simple", source: "${body}" }` through
   `RuntimeBus` + `RuntimeExecutionAdapter` and `.unwrap()`s the result, so the
   new compiler makes it panic. Add file-local mutating fixtures — `struct
   TestMutLang` implementing `camel_language_api::Language` (`name` = `"testmut"`)
   whose `create_mutating_expression` returns `Ok(Box::new(TestMutExpr))`, and
   `struct TestMutExpr` implementing `camel_language_api::MutatingExpression`
   (`evaluate` returns `Ok(Value::Null)`) — using the file's existing
   `use async_trait::async_trait;` and `camel_language_api`. Insert `TestMutLang`
   under `"testmut"` in `simple_languages()` (it returns a fresh map per call),
   then change the Script step's `language` to `"testmut"` and keep the step, the
   rest of the variant list, and the `Registered` result assertion unchanged.

**Tests:** commands
`cargo test -p camel-core --lib route_controller::tests::resolve_steps_covers_declarative_and_eip_variants`,
`cargo test -p camel-core --lib step_resolution::tests::resolve_steps_covers_non_endpoint_variants`,
`cargo test -p camel-core --lib context_tests::test_add_route_definition_uses_runtime_registered_language`,
`cargo test -p camel-core --features lang-rhai,lang-jsonpath,lang-js,lang-xpath --test expression_failure_propagation_test`,
`cargo test -p camel-core --test runtime_commands_test register_route_accepts_advanced_canonical_steps`.
- T1/T2 expected FAIL before, PASS after (steps kept, mutating test language).
- T3 expected PASS after adding the mutating impl.
- T4 must actually run: the suite is
  `#![cfg(all(feature = "lang-rhai", feature = "lang-jsonpath"))]`, so the
  command above passes the features; assert the run output contains
  `verb_set_body_expression_error_fails_step ... ok` (test count is not zero).
  The `set_body` row carries `secret=abc` and the table-wide redaction assertion
  remains.
- T5 expected FAIL (panic on `.unwrap()`) before the T5 edit, PASS after.

**Acceptance:**
- The five commands above exit 0 and execute nonzero tests; the T4 run prints at least the
  `verb_set_body_expression_error_fails_step ... ok` line.
- `rg -n '"script_readonly"|verb_script_readonly' crates/camel-core/tests/expression_failure_propagation_test.rs` returns no matches. The canonical negative-test name from Task 2.3 is retained and is not an obsolete fallback row.
- `rg -n 'set_header_literal\("secret", "abc"\)' crates/camel-core/tests/expression_failure_propagation_test.rs` matches the `set_body` row.
- `rg -n -A4 'CanonicalStepSpec::Script' crates/camel-core/tests/runtime_commands_test.rs` shows `language: "testmut"` for the Script step.

- [x] 2.4

### Task 2.5: Migrate parser-only script samples and script docs

**Budget:** <= 30 min.

**Files:**
- `crates/camel-dsl/src/parity_tests.rs` (modified)
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/README.md` (modified)
- `docs/src/languages/minijinja.md` (modified)
- `docs/src/yaml-dsl/step-verbs/transformation.md` (modified)
- `docs/src/eip/script.md` (modified)
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified)

**Steps:**
1. `parity_tests.rs` `ParityCase { name: "Script", .. }`: YAML
   `language: "rhai"` with valid source `body = "ok";`; JSON
   `{"language":"rhai","source":"body = \"ok\";"}`. Keep both sides byte-equal.
2. `yaml.rs:2613` `test_parse_yaml_to_canonical_supports_advanced_declarative_steps`:
   `script:` language `"rhai"`, source `body = "ok";`; keep `steps.len() == 6`.
3. `yaml.rs:3419` `test_parse_yaml_supports_all_declarative_step_kinds`: same
   rhai change.
4. `crates/camel-dsl/README.md:419`: replace the `script` row example with
   `- transform: { language: "simple", source: "${body}" }` (read-only language
   moves to `transform`).
5. `docs/src/languages/minijinja.md:27`: change the `script:` key to `transform:`
   keeping `language: minijinja` and the `source: |` block.
6. `docs/src/yaml-dsl/step-verbs/transformation.md`: add a `Migration (BREAKING)`
   callout next to the `transform` forms with
   ``Replace `script:` that uses simple/jsonpath/xpath/minijinja with `transform: {language, source}`. The behavior is identical.``;
   update the `script` table row to state `language` must support mutating
   expressions (`rhai`, `js`).
7. `docs/src/eip/script.md` (current lines 19-20 in the root-sequence
   `script-demo` example): the Rhai source has two statements,
   `headers["tenant"] = "acme"` and `body = body + "_processed"`; the first lacks
   a semicolon, so `RhaiLanguage::create_mutating_expression` returns
   `LanguageError::ParseError` and the new sweep would classify it as
   `CreationError`. Terminate the first assignment with `;` (it becomes
   `headers["tenant"] = "acme";`). Keep the route, both mutations, the second
   statement and the trailing `to` step unchanged.
8. `crates/camel-core/src/lifecycle/application/route_definition.rs:106`: change
   the `DeclarativeScript` doc comment to state it requires a mutating language
   and writes the Exchange back.

**Tests:** commands
`cargo test -p camel-dsl --lib parity_tests`,
`cargo test -p camel-dsl --lib yaml::tests::test_parse_yaml_to_canonical_supports_advanced_declarative_steps`,
`cargo test -p camel-dsl --lib yaml::tests::test_parse_yaml_supports_all_declarative_step_kinds`,
`rg -n 'headers\["tenant"\] = "acme";' docs/src/eip/script.md`.
- All three parse tests expected PASS before and after (DSL-only); the migration
  makes every parser-only sample a valid mutating script per the spec.
- The `rg` check confirms the corrected semicolon is present in the doc.

**Acceptance:**
- The three cargo commands above exit 0.
- `rg -q 'headers\["tenant"\] = "acme";' docs/src/eip/script.md` exits 0.
- `rg -n 'language: "simple"' crates/camel-dsl/src/parity_tests.rs crates/camel-dsl/src/yaml.rs` returns no `script:` sample.
- `rg -q "Migration \(BREAKING\)" docs/src/yaml-dsl/step-verbs/transformation.md` exits 0.

- [x] 2.5

### Task 2.6: Route-example sweep classifier and regression fixtures

**Budget:** <= 30 min.

**Files:**
- `crates/camel-test/tests/route_example_parser_sweep.rs` (new)

**Steps:**
1. `fn repo_root() -> PathBuf` = `PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")`.
2. `enum DocKind { Yaml, Json }`, `enum FenceIndex { File, Fence(usize) }`, and
   `fn collect_sites() -> Vec<Site>` where
   `Site { path: PathBuf, fence_index: FenceIndex, kind: DocKind }`:
   - Recursively walk `examples`, `benchmarks/scenarios`, `docs` with
     `std::fs::read_dir` (no new crate). Files with extension `yaml|yml` are
     `Yaml`; `json` are `Json`. Files under a path segment `camel-quarkus` or
     `camel-standalone` are recorded as `ForeignExcluded` (foreign Apache Camel
     syntax, untouched per the spec).
   - `.md` files: extract every fenced block whose info string is
     `yaml`/`yml`/`json`, in document order. `Fence(usize)` is the **0-based**
     ordinal over those blocks including non-selected ones, so ordinals stay
     stable when a fence is excluded.
   - Normalization: drop the info-string line, remove the minimum common leading
     whitespace of non-empty lines, trim leading/trailing blank lines.
   - Envelope selection and normalization. `camel_dsl::parse_yaml_to_declarative`
     sends text through `serde_yml::from_str::<RouteDslRoutes>`; the mapping
     struct declares `routes`, `rest`, `mcp`, `templates` and `templated_routes`
     all `#[serde(default)]`, and `deserialize_route_doc` then expands `rest` and
     `mcp` into routes. A document is therefore a route-producing envelope when
     it is:
     - a mapping containing any of `routes`, `rest` or `mcp` (a `rest:`-only or
       `mcp:`-only block is complete and is parsed with its existing YAML shape,
       never excluded);
     - a root sequence (`- id:` at column 0), normalized by prepending `routes:`
       and indenting every line by two spaces (required for the root sequences at
       `docs/src/languages/minijinja.md` fence 21 and `docs/src/eip/script.md`
       fence 13);
     - a single complete route object (a mapping with `from` and `steps`, and
       none of `routes`/`rest`/`mcp`), normalized to `routes:` with that object
       as a one-element sequence.
     JSON is handled by parsing the normalized text as `serde_json::Value`:
     an object with any of `routes`/`rest`/`mcp` is serialized back and parsed;
     an array becomes `serde_json::json!({ "routes": array })` and is serialized;
     an object with `from` and `steps` becomes
     `serde_json::json!({ "routes": [object] })` and is serialized. The concrete
     envelope used by the fixtures is
     `{"routes":[{"id":"r","from":"direct:start","steps":[{"set_body":{"value":"ok"}}]}]}`.
     Any other document (a partial fragment, an object with none of the
     envelope keys and no `from`+`steps`, a step list only) is `PartialExcluded`
     (path + index).
3. `fn parse_site(site: &Site) -> Result<Vec<DeclarativeRoute>, CamelError>`
   dispatches on `kind`, using the normalized envelope text, to
   `camel_dsl::parse_yaml_to_declarative` / `parse_json_to_declarative`.
4. Introduce `type LangMap = HashMap<String, Arc<dyn Language>>;` once.
   `camel_core::languages_from_config(&LanguagesConfig::default())` returns
   `Arc<Mutex<HashMap<String, Arc<dyn Language>>>>`; lock it, clone the inner
   map into `LangMap`, then insert `Arc::new(..)` constructors:
   `camel_language_jsonpath::JsonPathLanguage::new()`,
   `camel_language_xpath::XPathLanguage::new()`,
   `camel_language_rhai::RhaiLanguage::new()`,
   `camel_language_js::JsLanguage::new()` under `"jsonpath"`, `"xpath"`,
   `"rhai"`, `"js"`, `"javascript"`. The capability policy is the registry
   itself (dynamic), not a hardcoded capability name list. `minijinja` is
   supplied by the existing `camel-core/lang-minijinja` feature, which every
   sweep command enables (`--features camel-core/lang-minijinja`); no new
   dependency is added.
5. `enum Capability { Mutating, NonMutating, CreationError, Unresolved }` and
   `fn capability(languages: &LangMap, name: &str, source: &str) -> Capability`:
   missing name => `Unresolved`; `create_mutating_expression(source)`:
   `Ok` => `Mutating`, `Err(LanguageError::NotSupported { .. })` => `NonMutating`,
   other `Err` => `CreationError`. `CreationError` (a supported language whose
   example source is malformed) is a distinct outcome and is never counted as
   `Mutating`.
6. `fn script_languages(steps: &[DeclarativeStep], out: &mut Vec<(String, String)>)`
   recurses over the child-step variants of the actual enum:
   `Filter` (`FilterStepDef.steps`), `Choice` (`ChoiceStepDef.whens[].steps` and
   `otherwise`), `Split` (`SplitStepDef.steps`), `Throttle`
   (`ThrottleStepDef.steps`), `LoadBalance` (`LoadBalanceStepDef.steps`),
   `Multicast` (`MulticastStepDef.steps`), `Loop` (`LoopStepDef.steps`),
   `IdempotentConsumer` (`IdempotentConsumerStepDef.steps`), `Cache`
   (`CacheStepDef.on_miss`), and `DoTry { steps, catch, finally }`
   (`DoTryCatchClauseDef.steps`, `DoTryFinallyDef.steps`). Then traverse the
   route-level child collections with the same recursive helper:
   `route.circuit_breaker.as_ref().map(|cb| cb.fallback.as_slice())`
   (`DeclarativeCircuitBreaker.fallback`) and
   `route.error_handler.on_exceptions[].steps`
   (`DeclarativeErrorHandler.on_exceptions`). Collect every
   `DeclarativeStep::Script(ScriptStepDef { expression })` as
   `(expression.language, expression.source)`.
7. `fn classify_text(text: &str, kind: DocKind, languages: &LangMap) -> Classified`
   with
   `enum Classified { Parsed { scripts: Vec<(String, Capability)> }, BareMap, WrongType, ParseError(String) }`.
   A `RouteError` message containing `in value-source mapping` => `BareMap`;
   containing `wrong type for key` => `WrongType`; else `ParseError`.
8. Regression fixtures via `classify_text` on inline strings, all named with the
   `sweep_flags_` prefix so one filter runs them all:
   - `sweep_flags_yaml_bare_map` — a YAML document whose `set_body` is
     `{constant: ok}` => `BareMap`.
   - `sweep_flags_json_bare_map_fence` — input
     `{"routes":[{"id":"r","from":"direct:x","steps":[{"set_body":{"constant":"ok"}}]}]}`
     parsed as JSON => `BareMap`.
   - `sweep_flags_markdown_fenced_bare_map` — the same YAML inside a `yaml`
     fence in a doc string, collected then classified => `BareMap`.
   - `sweep_flags_root_sequence_script` — the corrected two-statement Rhai source
     from `docs/src/eip/script.md` (`headers["tenant"] = "acme";` newline
     `body = body + "_processed"`) inside a root-sequence `- id: r` envelope,
     normalizes to a `routes:` envelope and yields `Mutating`.
   - `sweep_flags_nested_script_non_mutating` — a `filter` whose nested `script:`
     is `simple`, yielding `NonMutating` (proves recursion).
   - `sweep_flags_circuit_breaker_fallback_non_mutating` — a route with
     `circuit_breaker: {failure_threshold: 1, open_duration_ms: 1000, fallback:
     [{script: {language: simple, source: "${body}"}}]}`, yielding `NonMutating`
     (proves fallback traversal).
   - `sweep_flags_rest_block_steps_inspected` — a complete `rest:` block
     (`host`, `port`, `path`, one operation `{method: GET, operation_id:
     sweepGet, steps: [{script: {language: simple, source: "${body}"}}]}`),
     yielding one `("simple", NonMutating)` from the REST-generated route steps.
   - `sweep_flags_mcp_block_steps_inspected` — a complete `mcp:` block
     (`server: {name: sweep, bind: 127.0.0.1:19100}`, one tool
     `{name: sweepTool, input_schema: {type: object}, steps: [{script: {language:
     simple, source: "${body}"}}]}`), yielding one `("simple", NonMutating)` from
     the MCP-generated route steps.
   - `sweep_flags_non_mutating_script` — a `script` step with `language: simple`,
     `source: "${body}"`, yielding `NonMutating`.
   - `sweep_flags_accepts_mutating_script` — a `script` step with `language:
     rhai`, yielding `Mutating`.
   - `sweep_flags_malformed_source_is_creation_error` — a supported mutating
     language with the malformed missing-semicolon source
     (`language: rhai`, source `headers["tenant"] = "acme"` newline
     `body = body + "_processed"`, first statement unterminated), yielding
     `CreationError`, never `NonMutating` or `Mutating`.

**Tests:** command
`cargo test -p camel-test --features camel-core/lang-minijinja --test route_example_parser_sweep sweep_flags`.
- The eleven regression-fixture tests above (all share the `sweep_flags_` prefix,
  so `sweep_flags_accepts_mutating_script` is executed, not skipped). expected:
  FAIL before this task, PASS after. They prove bare-map, JSON-envelope,
  Markdown-fence, root-sequence, nested-script, circuit-breaker-fallback, REST,
  MCP, non-mutating-script and malformed-source detection, so no marker heuristic
  can silently miss a class.

**Acceptance:**
- `cargo test -p camel-test --features camel-core/lang-minijinja --test route_example_parser_sweep sweep_flags` exits 0.
- No new dependency is added (`git diff --stat crates/camel-test/Cargo.toml` is empty for this change); `minijinja` comes from the existing `camel-core/lang-minijinja` feature.

- [x] 2.6

### Task 2.7: Repo sweep, fixed per-site baseline ledger and A16 acceptance

**Budget:** <= 30 min.

**Files:**
- `crates/camel-test/tests/route_example_parser_sweep.rs` (modified)
- `crates/camel-test/tests/fixtures/route_sweep_baseline.tsv` (new)

**Steps:**
1. Add `#[test] fn route_examples_parse_without_new_rejection()`: iterate
   `collect_sites()`, classify each selected site, and build a ledger
   `Vec<LedgerEntry>` with outcome
   `Parsed { scripts } | BareMap | WrongType | ParseError(String) | PartialExcluded | ForeignExcluded`.
   Write the ledger to
   `PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("route_sweep_ledger.tsv")`
   (`CARGO_TARGET_TMPDIR` is always provided to integration tests by Cargo;
   `CARGO_TARGET_DIR` is not set at test runtime) and print it with
   `--nocapture`.
2. Single baseline source: read
   `crates/camel-test/tests/fixtures/route_sweep_baseline.tsv` (repo-relative, no
   in-code const). One entry per line, three tab-separated columns: the
   repo-relative `path`, the `fence_index`, and the `reason`. `fence_index` is
   the literal `file` for whole files and the **0-based** fence ordinal from
   Task 2.6 otherwise (so per-fence entries stay unique). Lines starting with
   `#` are comments. `reason` is mandatory and is error text only (never a
   variable value).
3. Assertions:
   - no `BareMap`, `WrongType`, `NonMutating`, `CreationError`, or `Unresolved`
     anywhere — these categories can NEVER be baselined;
   - the set of `ParseError` keys (the `path` and `fence_index` columns) equals
     the baseline key set exactly in both directions (any new, removed, or
     renamed failure fails the test);
   - every baseline entry has a non-empty reason, and no reason embeds a raw
     variable value;
   - the examined count is at least 1 and `ForeignExcluded`/`PartialExcluded`
     are recorded (not empty stubs). In particular the complete root-sequence
     route in `docs/src/eip/script.md` is a selected site; it MUST parse and
     MUST yield `Mutating` after Task 2.5 step 7. It is never placed in the
     baseline and never `PartialExcluded`; a `CreationError` there is a hard
     failure, not a baseline candidate.
4. Pre-change confirmation without touching the worktree or HEAD: for every
   baseline entry, read the same document by running `git show HEAD:` followed by
   the repo-relative path (whole-file mode), or fetch that document the same way
   and extract the same 0-based `Fence(usize)` ordinal (fence mode), classify it
   with the same `classify_text`, and assert the outcome is an unrelated
   `ParseError` — never `BareMap`, `WrongType`, `NonMutating`, `CreationError`,
   or `Unresolved`. Because the baseline holds only unrelated documents this
   change never migrates, the classification is stable; a change-related failure
   can never reach the baseline (it is a hard failure asserted before the
   baseline is consulted). This reads git objects only.
5. Fence-identity regression: add `sweep_baseline_fence_identity_is_exact`,
   which builds a synthetic `.md` string with two `yaml` fences and asserts the
   collector yields distinct keys `Fence(0)` and `Fence(1)` (ordinals do not
   collapse and cannot mask one another).
6. Generate the baseline: run once with `--nocapture`, inspect the ledger, and
   add ONLY genuinely unrelated parse failures (for example a non-route
   YAML/JSON file under the swept roots), each with a reason; commit the TSV. A
   complete selected route, including `docs/src/eip/script.md`, is never a
   baseline entry and is never excluded.

**Tests:** command
`cargo test -p camel-test --features camel-core/lang-minijinja --test route_example_parser_sweep -- --nocapture`.
- `route_examples_parse_without_new_rejection`: setup: post-migration tree.
  action: classify every selected site and apply the capability policy. assert:
  the four conditions in step 3 plus the pre-change confirmation in step 4.
  expected: PASS after Tasks 1.2, 1.4, 2.5, 2.6; FAIL if any bare map, wrong
  type, non-mutating/creation-error script, unresolved script language, or
  unbaselined parse failure remains, or if any baseline entry does not classify
  as an unrelated pre-change parse failure.
- `sweep_baseline_fence_identity_is_exact`: expected PASS after step 5.

**Acceptance:**
- `cargo test -p camel-test --features camel-core/lang-minijinja --test route_example_parser_sweep` exits 0.
- The baseline file lists only unrelated parse failures, each with `path`,
  `fence_index` and a reason; no change-related category appears.

- [x] 2.7

## Conductor verification and park (procedure, not worker tasks)

The conductor owns commits, checkboxes and gates. Every command runs in the
worktree with `CARGO_TARGET_DIR=/home/shared/rust-camel-worktrees/364-simpleset/target CARGO_BUILD_JOBS=4
TMPDIR=/home/shared/tmp`. Record a separate PASS/FAIL status for each command.

### Phase 1 park (after Tasks 1.1–1.5)

Run only the per-task acceptance commands (the per-task checks ledger) and record
each status:
- Task 1.1: `cargo test -p camel-dsl --test value_source_map_rejection_test`.
- Task 1.2: `cargo test -p camel-dsl --features schema --lib route_ast::tests::schema_set_body_data_restricts_literal_branch`,
  `cargo test -p camel-dsl --test schema_validation`, `cargo xtask schema --check`.
- Task 1.3: `cargo test -p camel-lint --lib schema_context::tests::set_body_data_container_classification_unchanged`.
- Task 1.4: `cargo test -p camel-integration-test --test direct_reply_test expect_reply_json_subset_on_direct_body`,
  `cargo check -p camel-test --tests --features integration-tests`.
- Task 1.5: the four `rg -q` documentation checks.

Then commit 1 with the subject, `BREAKING CHANGE:` footer and `Bd: rc-me8oe`
trailer from the Commit plan, and check boxes 1.1–1.5. The full gate suite is
owned by Final verification below; do not run it at this park.

### Final verification (A17)
```

- name: fmt
  run: cargo fmt --check --all
- name: clippy-workspace
  run: cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings
- name: clippy-kafka
  run: cargo clippy -p camel-component-kafka --all-targets -- -D warnings
- name: clippy-cli
  run: cargo clippy -p camel-cli -- -D warnings
- name: clippy-cli-nodefault
  run: cargo clippy -p camel-cli --no-default-features --features flavor-regular,exec --all-targets -- -D warnings
- name: build-workspace
  run: cargo build --workspace
- name: test-workspace-lib
  run: cargo test --workspace --lib
- name: test-camel-core-integration
  run: cargo test -p camel-core --tests --features lang-rhai,lang-jsonpath,lang-js,lang-xpath
- name: hexagonal-boundaries
  run: cargo test -p camel-core --test hexagonal_architecture_boundaries_test
- name: lint-unwrap
  run: cargo xtask lint-unwrap
- name: lint-secrets
  run: cargo xtask lint-secrets
- name: lint-single-source
  run: cargo xtask lint-single-source
- name: lint-non-exhaustive
  run: cargo xtask lint-non-exhaustive
- name: lint-log-levels
  run: cargo xtask lint-log-levels
- name: lint-log-redaction
  run: cargo xtask lint-log-redaction
- name: lint-cancel-tokens
  run: cargo xtask lint-cancel-tokens
- name: lint-test-sleep
  run: cargo xtask lint-test-sleep
- name: lint-unbounded-wait
  run: cargo xtask lint-unbounded-wait
- name: lint-ignore
  run: cargo xtask lint-ignore
- name: lint-publish-cycles
  run: cargo xtask lint-publish-cycles
- name: lint-publish-registration
  run: cargo xtask lint-publish-registration
- name: lint-component-deps
  run: cargo xtask lint-component-deps
- name: lint-gate-forwarding
  run: cargo xtask lint-gate-forwarding
- name: lint-context-citations
  run: cargo xtask lint-context-citations
- name: lint-metric-labels
  run: cargo xtask lint-metric-labels
- name: schema-check
  run: cargo xtask schema --check
- name: doc-build
  run: RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core -p camel-builder -p camel-dsl -p camel-endpoint --no-deps
- name: cargo-audit
  run: cargo audit
- name: openspec-validate
  run: openspec validate 364-simpleset --type change --json
```

Skipped, with a note recorded per command:
- `cargo xtask lint-commits` — A17 explicitly excludes it.
- `cargo xtask changelog --check` — needs a network fetch, forbidden in this
  worktree; the conductor reports it as SKIPPED (not PASS).

Then run `cargo test -p camel-dsl --lib` and `cargo test -p camel-core --lib`,
commit 2 with the subject, footer and `Bd: rc-i7cfx` trailer, and check boxes
2.1–2.7. If commit 2 is dropped, revert only the Task 2.5 script doc edits and
keep Phase 1.

CXF runtime verification is CI/native-only and is reported explicitly as such;
the local check is the `cargo check -p camel-test --tests --features integration-tests`
compile.
