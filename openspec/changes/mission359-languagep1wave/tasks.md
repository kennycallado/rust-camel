# Tasks: mission359-languagep1wave

Run cargo commands with
`CARGO_TARGET_DIR=<worktree>/target CARGO_BUILD_JOBS=4 TMPDIR=/home/shared/tmp`.
Line references are to the pre-change tree. Probe evidence and decisions are in
`design.md`.

## Phase 1: Simple template engine + DSL gate

Phase 1 does NOT make the whole workspace green. These tests are EXPLICITLY
KNOWN-RED until Phase 2 and Phase-1 workers MUST NOT touch them: `camel-processor`
aggregator `evaluate_completion_predicate_or_combines_any` (~2430) and
`completion_predicate_expr_completes_on_match` (~2515) (Task 2.1);
`camel-core` `expression_failure_propagation_test.rs` ~279 / ~794 and
`verb_diagnostics("script_readonly")` ~409-412 (Task 2.2). Phase-1 acceptance
is limited to `camel-language-simple` + `camel-dsl` (+ the two migrated test
files in Task 1.3).

### camel-language-simple

#### Task 1.1: Add TEMPLATE parser mode

**Files:**
- `crates/languages/camel-language-simple/src/parser.rs` (modified)

**Steps:**
1. Add `pub fn parse_template(input: &str) -> Result<Expr, LanguageError>`.
2. Outer-trim `input`; empty → `ParseError`. Scan the trimmed source for a
   single whole `${...}` token spanning it; if found, return the slot atom via
   `parse_expr_atom` unchanged (`Expr::Header`, `Expr::Body`, `Expr::BodyField`, `Expr::ExchangeProperty`, `Expr::ExceptionMessage`, `Expr::LanguageDelegate`).
3. Otherwise split into `InterpolatedPart::Literal` (verbatim, including
   whitespace/quotes/operators/keywords) and `InterpolatedPart::Expr`
   (`parse_expr_atom`) parts. No `${}` slot → return `Expr::StringLit(trimmed)`.
4. Keep the unclosed-`${` rule: with no completed slot and preceding text it
   is literal (`Got ${body`); after at least one completed slot
   (`${header.h} ${`) it is `ParseError { reason: "unclosed interpolation" }`;
   alone it is a `ParseError`.
5. Leave `parse` (predicate mode) untouched.

**Tests:** (`parser::tests`, command `cargo test -p camel-language-simple --lib parser::`)
- `parse_template_operator_text_is_literal`: `parse_template("Got -> ${header.h}")` → `Interpolated([Literal("Got -> "), Expr(Header("h"))])`; expected PASS after step 3.
- `parse_template_whole_slot_returns_atom`: `parse_template("${header.h}")` → `Expr::Header("h")`; expected PASS.
- `parse_template_whole_quoted_literal_is_raw`: `parse_template("'abc'")` → `Expr::StringLit("'abc'")`; expected PASS.
- `parse_template_preserves_whitespace`: `parse_template("${header.h} 3 items")` → literal `" 3 items"` intact; expected PASS.
- `parse_template_outer_trim`: `parse_template("Got ${header.h}\n")` → Literal `"Got "` (no trailing newline); expected PASS.
- `parse_template_empty_is_error`: `parse_template("")` and `parse_template("   ")` → `Err`; expected PASS.
- `parse_template_unclosed_after_slot_is_error`: `parse_template("${header.h} ${")` → `Err`; expected PASS.
- `parse_template_unclosed_without_slot_is_literal`: `parse_template("Got ${body")` → `Expr::StringLit("Got ${body")`; expected PASS.
- `parse_template_unknown_slot_is_error` — setup: none. action:
  `parser::parse_template("${unknown}")`. assert: `Err(LanguageError::ParseError)`
  (the unknown slot names no atom). command:
  `cargo test -p camel-language-simple --lib parser::parse_template_unknown_slot_is_error`.
  expected: PASS after step 2.
- `parse_template_adjacent_literal_and_slot`: action
  `parse_template("Got${body}")`. assert: `Interpolated([Literal("Got"),
  Expr(Body)])` (no space required). command:
  `cargo test -p camel-language-simple --lib parser::parse_template_adjacent_literal_and_slot`.
  expected: PASS.
- `parse_template_unclosed_at_start_is_error`: action
  `parse_template("${error")`. assert: `Err(LanguageError::ParseError)`
  (source begins with an unclosed slot). command:
  `cargo test -p camel-language-simple --lib parser::parse_template_unclosed_at_start_is_error`.
  expected: PASS.

**Acceptance:**
- `cargo test -p camel-language-simple --lib parser::` passes.
- `cargo clippy -p camel-language-simple --all-targets -- -D warnings` exits 0.

- [x] 1.1

#### Task 1.2: Wire create_expression to template mode; migrate expression tests; add corpus

**Files:**
- `crates/languages/camel-language-simple/src/lib.rs` (modified)
- `crates/languages/camel-language-simple/benches/expression_eval.rs` (modified)
- `crates/languages/camel-language-simple/tests/template_mode.rs` (new)

**Steps:**
1. `Language::create_expression` calls `parser::parse_template`; keep
   `create_predicate` on `parser::parse`.
2. Migrate the obsolete comparison scripts in `benches/expression_eval.rs`
   (`header_eq` `${header.foo} == 'bar'`, `header_gt` `${header.count} > 10`,
   `body_contains` `${body} contains 'world'`) out of the `create_expression`
   bench: move them to the `create_predicate` bench, and replace them in the
   expression bench with value templates (for example `${header.foo}`,
   `${header.count}`, `${body}`) so the bench still measures expression
   evaluation.
3. Migrate the existing expression tests that change under raw/template
   semantics. Predicate-shaped comparisons and the non-finite check move to
   `create_predicate`: `simple_coercion_error_has_no_operand` (383),
   `test_non_finite_number_parse_error` (789), `body_field_in_predicate_true`
   (1206), `body_field_in_predicate_false` (1220),
   `body_field_numeric_predicate` (1259). Literal-shape tests update to raw
   text: `test_string_literal_expression` (295) → `'hello'`;
   `test_double_quoted_literal_unescapes_newline` (304),
   `test_double_quoted_literal_unescapes_tab_and_quote` (313),
   `test_double_quoted_literal_unescapes_backspace_formfeed_and_slash` (322),
   `test_double_quoted_string_with_operator` (668) → raw source incl. quotes;
   `test_null_expression` (331) → `"null"`;
   `test_operator_inside_string_literal_not_split` (497) → `'a>=b'`;
   `test_bool_literal_true` (677) → `"true"`;
   `test_bool_literal_false` (686) → `"false"`.
4. Add the audit regression corpus in `tests/template_mode.rs` (public API).

**Tests:** (`crates/languages/camel-language-simple/tests/template_mode.rs`,
command `cargo test -p camel-language-simple --test template_mode`)
- `template_operator_text_is_literal`: header `h=B`; expression
  `Got -> ${header.h}` → evaluate → `Value::String("Got -> B")`.
- `template_angle_brackets_do_not_parse`: `<order>${header.h}</order>` →
  `<order>B</order>`.
- `template_null_word_is_literal`: `value is null: ${header.h}` →
  `value is null: B`.
- `template_whitespace_and_number_text`: `${header.h} 3 items` → `B 3 items`.
- `template_quotes_preserved`: `literal 'quoted' text` unchanged.
- `template_whole_slot_native_number`: header `n` = JSON number `7`;
  `create_expression("${header.n}")` → `Value::Number(7)`.
- `template_whole_slot_native_array`: JSON body `{"items":[1,2]}`;
  `${body.items}` → `serde_json::json!([1,2])`.
- `template_whole_body_slot_serializes_json` — setup: exchange body
  `Body::Json(serde_json::json!({"a":1}))`. action: evaluate
  `create_expression("${body}")`. assert: `Value::String("{\"a\":1}")`
  (existing `Expr::Body` serialization), NOT a native object. command:
  `cargo test -p camel-language-simple --test template_mode template_whole_body_slot_serializes_json`.
  expected: PASS.
- `template_unknown_language_fails_at_evaluation` — setup: `SimpleLanguage::new()`
  with no resolver. action: `create_expression("${nosuchlang:x}")` then evaluate
  on an empty exchange. assert: creation is `Ok`; evaluation is `Err`
  (`LanguageError::EvalError`, no language resolver). command:
  `cargo test -p camel-language-simple --test template_mode template_unknown_language_fails_at_evaluation`.
  expected: PASS.
- `escaped_env_slot_error_text` — setup: `SimpleLanguage::with_resolver(Arc::new(|_| None))`.
  action: `create_expression("$${env:NAME}")` then evaluate. assert: creation is
  `Ok`; evaluation is `Err(LanguageError::EvalError)` whose rendered text
  contains `Language 'env' not found`. command:
  `cargo test -p camel-language-simple --test template_mode escaped_env_slot_error_text`.
  expected: PASS. (The detail text is only observable at the language-crate
  boundary; `to_expression_failed` discards it, so the wrap-level assertion
  lives in Task 2.2.)
- `template_whole_slot_missing_is_null`: `${header.missing}` → `Value::Null`.
- `template_mixed_missing_is_empty`: `prefix-${header.missing}-suffix` →
  `"prefix--suffix"`.
- `predicate_mode_unchanged`: `create_predicate("${header.type} == 'order'")`
  matches with `type=order`; `create_predicate("${header.h}")` → `Err(TypeMismatch)`.
- `expression_does_not_coerce_operators`: `create_expression("a == b")` →
  `Value::String("a == b")`.

**Acceptance:**
- `cargo test -p camel-language-simple` passes.
- `cargo clippy -p camel-language-simple --all-targets -- -D warnings` exits 0.
- The commit records the raw-literal semantic change and the migrated tests,
  and carries a breaking marker (`!` in the Conventional Commit subject or a
  `BREAKING CHANGE:` footer) — `cargo xtask lint-commits` rejects the word
  "breaking" in a body without that marker.

- [x] 1.2

### camel-dsl

#### Task 1.3: Top-level `${}` gate shared by YAML and JSON

**Files:**
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/CONTEXT.md` (modified)
- `crates/camel-dsl/tests/bare_string_interpolation.rs` (new)
- `crates/camel-dsl/tests/env_escape_regression.rs` (modified)
- `crates/camel-cli/src/commands/test/driver_tests.rs` (modified)

**Steps:**
1. Add `fn bare_string_value_source(value: serde_json::Value) -> ValueSourceDef`:
   for a top-level `Value::String` containing `"${"`, return
   `ValueSourceDef::Expression(LanguageExpressionDef { language: "simple", source })`;
   otherwise `ValueSourceDef::Literal(value)`. Non-string values and nested
   strings are returned as `Literal` unchanged.
2. Use it in the `SetBodyData::Literal` arms of `SetBody`/`Transform` AND in the
   `literal` branch of `parse_value_source` (so the object `value:` forms of
   `set_body`/`transform`/`set_header`/`set_property` and the config
   `log: {message: {value: ...}}` form are gated identically). A bare
   `log: "..."` or `log: {message: "<plain>"}` stays ALWAYS Simple through the
   existing `LogMessageData::Literal` arm.
3. Keep `log:` compiling its bare string to a Simple expression. Leave explicit
   `simple:`/`language:` untouched. JSON reuses the same path via
   `route_dsl_to_declarative_route`.
4. Add a decision entry to `crates/camel-dsl/CONTEXT.md`: top-level bare string
   values containing `${}` compile to a Simple template; nested literals are
   not gated; one helper serves YAML and JSON.
5. Add an env-interpolation docs row (module docs in
   `crates/camel-dsl/src/env_interpolation.rs` and/or the YAML-DSL docs)
   recording that route substitution handles `${env:...}` ONLY, that an
   unescaped `${arg:NAME}` fails to load (args are substituted only on the
   `camel-cli` job path), and that `$${env:X}` → literal `${env:X}` becomes a
   `lang:expr` slot that fails at runtime with `Language 'env' not found`;
   cite the existing escape tests (`escape_full_form_yields_literal`,
   `escape_arg_form_yields_literal`).
6. Migrate the two existing tests that assert the old literal behavior (M2):
   - `crates/camel-dsl/tests/env_escape_regression.rs:114`
     `route_escape_yields_literal_in_body`: `$${env:RUST_CAMEL_TEST_ROUTE_B}`
     now lowers to `ValueSourceDef::Expression` with Simple source
     `${env:RUST_CAMEL_TEST_ROUTE_B}`, not `Literal`. Rename to
     `route_escape_yields_simple_slot_in_body` and update the assertion
     (`route_standalone_dollar_converts` `a$$b` → `Literal("a$b")` is
     unchanged).
   - `crates/camel-cli/src/commands/test/driver_tests.rs:2118`
     `env_value_never_interpolated_route`: the env-carried value
     `${env:B:-x}literal` lands verbatim, but the top-level gate now lowers it
     to `ValueSourceDef::Expression{simple, "${env:B:-x}literal"}`. Update the
     `header_value` assertion from `Literal(String(...))` to that Expression,
     preserving the no-second-interpolation contract (the source text keeps
     `${env:B:-x}` unresolved). Keep the test name.

**Tests:** (`crates/camel-dsl/tests/bare_string_interpolation.rs`,
`parse_yaml_to_declarative`/`parse_json_to_declarative`; command
`cargo test -p camel-dsl --test bare_string_interpolation`)
- `bare_set_body_with_slot_is_simple_expression`: YAML `set_body: "Got ${header.h}"` → `Expression{language:"simple", source:"Got ${header.h}"}`.
- `object_value_form_interpolates`: `set_body: {value: "Got ${header.h}"}` → `Expression{simple}` (I4 object form); likewise `set_header: {key: x, value: ...}` and `log: {message: {value: "Got ${header.h}"}}` (the nested value form; a plain `log: {message: "..."}` is always Simple, covered separately).
- `bare_set_header_with_slot_is_simple_expression`: `set_header: {key: x, value: "Got ${header.h}"}` → same shape.
- `bare_set_property_with_slot_is_simple_expression`: `set_property: {name: p, value: "Hi ${header.h}"}` → same.
- `bare_transform_with_slot_is_simple_expression` — setup: YAML route with
  `transform: "Got ${header.h}"`. action: `parse_yaml_to_declarative`. assert:
  the `SetBody` step value is
  `Expression{language:"simple", source:"Got ${header.h}"}` (the `transform`
  arm shares the helper). command:
  `cargo test -p camel-dsl --test bare_string_interpolation bare_transform_with_slot_is_simple_expression`.
  expected: PASS.
- `set_header_if_absent_shares_value_gate` (in `yaml.rs` unit tests, since
  `parse_value_source` is crate-private): `SetHeaderIfAbsent` is
  `#[serde(skip_deserializing)]` (REST-lowered, not authorable), so no YAML
  parse test; assert `parse_value_source(Some(String("Hi ${header.h}")), …,
  "set_header_if_absent")` yields `Expression{simple}`.
- `plain_literal_stays_literal`: `set_body: "order"` → `Literal(String("order"))`.
- `nested_literal_is_not_gated`: `set_body: {value: {nested: "${header.h}"}}` → the nested string stays `Literal("${header.h}")`; no `Expression`.
- `json_and_yaml_agree`: same route in JSON and YAML each with
  `set_body: "Got ${header.h}"` → both produce an identical Simple expression
  `ValueSourceDef`.
- `bare_log_slot_is_simple_expression`: `log: "Got ${header.h}"` → `Expression{simple}` (unchanged).
- `substituted_env_slot_value`: lookup `NAME="${header.h}"` over route text
  `set_body: "Hello ${env:NAME}"`, then `parse_yaml_to_declarative` on the
  interpolated text → `Expression{simple, source:"Hello ${header.h}"}`. The
  runtime result (header `h=B` → body `Hello B`) is asserted in Task 2.4.
- `substituted_env_plain_stays_literal`: lookup `NAME="B"` over the same route
  → `Literal(String("Hello B"))` (no `${` remains).
- `unescaped_route_arg_fails_load`: `parse_routes_with_env` with an env-only
  lookup over route text `set_body: "Hello ${arg:NAME}"` → `Err` (name `NAME`
  unresolved); `${arg:...}` is job-path only.
- `template_looking_literal_becomes_expression`: `parse_yaml_to_declarative` on
  `set_body: "echo ${HOME}"` → `Expression{simple, "echo ${HOME}"}` (NOT
  `Literal`); the `unrecognized token` compile failure this causes is asserted
  in Task 2.2 (route add). This is the breaking gate change.
- `escape_yields_slot_documented`: `interpolate_env_with` over
  `set_body: "$${env:NAME}"` with `NAME=B` yields literal `${env:NAME}` text;
  `parse_yaml_to_declarative` then compiles it as a Simple slot `${env:NAME}`
  (which fails at runtime with `Language 'env' not found`).
- `route_escape_yields_simple_slot_in_body` (migrated, in
  `env_escape_regression.rs`): `discover` on a route whose body is
  `$${env:RUST_CAMEL_TEST_ROUTE_B}` → `BuilderStep::DeclarativeSetBody` with
  `ValueSourceDef::Expression{language:"simple",
  source:"${env:RUST_CAMEL_TEST_ROUTE_B}"}`. command:
  `cargo test -p camel-dsl --test env_escape_regression route_escape_yields_simple_slot_in_body`.
- `env_value_never_interpolated_route` (migrated, in
  `camel-cli/src/commands/test/driver_tests.rs`): route header value lands as
  `ValueSourceDef::Expression{simple, "${env:B:-x}literal"}` (verbatim source,
  no second pass). command:
  `cargo test -p camel-cli env_value_never_interpolated_route` (focused suite
  only).
- `log_object_plain_message_is_always_simple`: `log: {message: "plain"}`
  → `LogStepDef.message == Expression{simple, "plain"}` even with no `${}`.
- `log_object_value_form_is_gated`: `log: {message: {value: "Got ${header.h}"}}`
  → `Expression{simple, "Got ${header.h}"}` via `parse_value_source`. command:
  `cargo test -p camel-dsl --test bare_string_interpolation log_object_value_form_is_gated`.

**Acceptance:**
- `cargo test -p camel-dsl` passes; `declarative_compile_test.rs` log test
  unchanged.
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.
- The gate commit carries a breaking marker (`!` subject or `BREAKING CHANGE:`
  footer); previously-literal top-level strings containing `${}` now fail to
  compile.

- [x] 1.3

### docs

#### Task 1.4: Align Simple docs with strict predicates and template mode

**Files:**
- `docs/src/languages/simple.md` (modified)
- `crates/languages/camel-language-simple/CONTEXT.md` (modified)
- `crates/languages/CONTEXT.md` (modified)
- `docs/src/yaml-dsl/step-verbs/basic.md` (modified — body/header/property value gate)
- `docs/src/yaml-dsl/step-verbs/transformation.md` (modified — transform value gate)

**Steps:**
1. Remove the truthiness claim; state that a predicate must evaluate to a real
   boolean and a bare non-boolean (including `null`/empty string) is a
   `type-mismatch` error.
2. Fix the `approved == true` example: compare against `'true'` or set a
   boolean header.
3. Document template mode for expressions vs predicate grammar for predicates;
   raw-literal semantics; whole-slot native return; bare top-level `${}` gate
   and nested-literal rule; `$${` escape behavior; and the literal-`${`
   workaround until rc-luew6: emit the literal through an explicit
   `language:`+`source:` quoted rhai/js string, escaping env/arg tokens as
   `$$` because env interpolation walks every string (for example
   `source: '"$${env:X}"'` → literal `${env:X}`; a non-env `'"${header.h}"'`
    needs no escape). Note `$$` is NOT a Simple-template escape: at the Simple
    source level after substitution, `$${header.h}` is `$` plus a live slot.
    In a route file, the env pass first turns `$$` into `$`, so `$${header.h}`
    reaches Simple as the whole slot `${header.h}` and retains its native
    value. Write `$$$${header.h}` in a route file for `$` plus a slot.
4. Glossary disambiguation (M4): add "Simple template mode" and "Simple
   predicate mode" entries to `crates/languages/CONTEXT.md` and the Simple
   crate `CONTEXT.md`, each with an `_Avoid_` line; explicitly say Simple
   template mode is NOT the MiniJinja engine (ADR-0047) and NOT RouteTemplate
   JSON-tree substitution (ADR-0008).
5. Gate docs (M6): record the top-level `${}` gate on the real YAML-DSL value
   pages — `docs/src/yaml-dsl/step-verbs/basic.md` (body/header/property) and
   `docs/src/yaml-dsl/step-verbs/transformation.md` (transform). A top-level
   string with a slot interpolates; nested object/array strings stay literal.
   `set_header_if_absent` shares the same gate but is REST-lowered
   (`#[serde(skip_deserializing)]`), so the docs need not present it as an
   authorable step.

**Tests:** (docs prose are not parsed by any test; the executable coverage for
the documented behavior lives in the Simple/DSL task tests above)
- `cargo xtask lint-context-citations` exits 0.
- The `simple.md` YAML example's semantics are the same as the executable
  `bare_set_body_with_slot_is_simple_expression` /
  `predicate_mode_unchanged` cases; no separate doc-parse test exists (the
  `documentation_examples` suite does not parse docs source).

**Acceptance:**
- `simple.md` no longer promises truthiness or `== true` coercion.
- `CONTEXT.md` null/predicate section matches `lib.rs:57-70` and template mode.

- [x] 1.4

## Phase 2: A callsite migration + integration

### camel-processor

#### Task 2.1: Aggregator completion predicate uses strict Predicate evaluation

**Files:**
- `crates/camel-processor/src/aggregator.rs` (modified)

**Steps:**
1. In `evaluate_completion_predicate` (830-854), replace `lang.create_expression(expr)`
   with `lang.create_predicate(expr)` and replace the
   `expression.evaluate(incoming)` + `value.as_bool().unwrap_or(false)` block
   with `predicate.matches(incoming).await`; map `Err(LanguageError)` to
   `CamelError::ProcessorError(e.to_string())` (the Simple type-mismatch text
   rides inside the `ProcessorError`; the aggregator has no `EvalMeta`, so no
   `ExpressionFailed` variant — pre-existing gap filed as **rc-dhppc**).
   Registry guard must still drop before await.
2. Leave `lang.create_expression` at `:782` (a value use) unchanged.
3. Preserve the existing `"${body} == 'DONE'"` source test
   `completion_predicate_expr_completes_on_match` (~2515). The unrelated
   `evaluate_completion_predicate_all_miss_returns_false` NOPE test stays as
   is; `evaluate_completion_predicate_or_combines_any` (~2430) is the other
   known-red Any test.

**Tests:** (`aggregator.rs` tests, command `cargo test -p camel-processor aggregator`)
- `completion_predicate_expr_completes_on_match`: existing test with source
  `${body} == 'DONE'` → still completes; expected PASS.
- `evaluate_completion_predicate_all_miss_returns_false`: source
  `${body} == 'NOPE'` → `Ok(false)`; expected PASS.
- `completion_predicate_non_bool_is_error`: registry with a predicate whose
  source is a bare string (e.g. Simple `${body}`), exchange body `x` → assert
  `matches!(err, CamelError::ProcessorError(msg) if msg.contains("bool"))`,
  NOT `Ok(false)`, and do NOT assert an `ExpressionFailed`/`ExpressionErrorClass`
  value (the aggregator has no `EvalMeta`); expected PASS after step 1.
- `completion_predicate_eval_error_propagates`: predicate source
  `${body} > 1` with non-numeric body → `Err(CamelError::ProcessorError)`;
  expected PASS.

**Acceptance:**
- `cargo test -p camel-processor aggregator` passes.
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0.

- [x] 2.1

### camel-core

#### Task 2.2: Audit and pin expression-vs-predicate callsites

**Files:**
- `crates/camel-core/src/lifecycle/adapters/step_resolution.rs` (modified tests only)
- `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs` (modified tests only; included as `route_controller::tests`)
- `crates/camel-core/tests/expression_failure_propagation_test.rs` (modified)

**Steps:**
1. Confirm each value wrapper in
   `crates/camel-core/src/lifecycle/adapters/step_resolution.rs` is a value
   use and stays Expression/template: `compile_language_expression` (~63),
   `compile_value_source` (~138), `compile_sort_expression` (~114),
   `compile_message_id_expression` (~158), `compile_key_expression` (~172).
   Confirm the `route_compiler_ext` correlation/sort/sequence uses stay value
   uses (their tests live in `route_controller_tests.rs`, NOT in
   `route_compiler_ext.rs`, which has no test module).
2. Confirm predicate verbs route through `compile_filter_predicate` /
   `compile_language_predicate` (~79, ~98).
3. Migrate the two Simple-expression comparator sources that change under
   template mode (I5): the `"script_readonly"` row at ~279
   (`expr("simple", "${header.secret} > 1")`) and the `set_property` row at
   ~794 (`expr("simple", "${header.x} > 1")`) → `expr("simple", "${nosuchlang:x}")`,
   and update each `assert_expression_failed` expected class from `TypeMismatch`
   to `Runtime`; keep the no-operand-leak assertions (`secret`/`abc` absent).
   The read-only-script row is built by `failing_steps("script_readonly")`
   (there is no `steps_for_verb`); update `verb_diagnostics("script_readonly")`
   (~409-412) to `Runtime` (keep `Pos::None`). Do not change the `filter`
   predicate row at ~812 (`${header.x} > 1`), which stays a predicate.
4. Add THREE route-level tests to the EXISTING
   `crates/camel-core/tests/expression_failure_propagation_test.rs` using its
   `new_context()` / `run_route(...)` helpers (do not create a new file):
   `route_add_fails_on_template_looking_literal`,
   `substituted_env_slot_runtime`, `escaped_env_slot_fails_at_runtime`.
5. Add the wrapper tests to the `step_resolution.rs` test module and the
   resequence test to `route_controller_tests.rs` (`route_controller::tests`),
   not `step_resolution`.

**Tests:**
- `compile_language_expression_is_template` —
  `cargo test -p camel-core --lib lifecycle::adapters::step_resolution::tests::compile_language_expression_is_template`:
  source `Got ${header.h}` with header `h=B` → expression evaluates to `Got B`.
- `compile_value_source_is_template` —
  `...step_resolution::tests::compile_value_source_is_template`: the
  `compile_value_source` ValueSource for `Got ${header.h}` evaluates to `Got B`.
- `compile_sort_expression_is_template` —
  `...step_resolution::tests::compile_sort_expression_is_template`: the
  synthetic sort element exchange has NO headers, so use `X-${body.id}` on a
  synthetic body `{"id":"7"}` → key `X-7`.
- `compile_message_id_expression_is_template` —
  `...step_resolution::tests::compile_message_id_expression_is_template`:
  `m-${header.id}` with header `id=k` → id `m-k`.
- `compile_key_expression_is_template` —
  `...step_resolution::tests::compile_key_expression_is_template`:
  `k-${header.id}` with header `id=k` → key `k-k`.
- `compile_language_predicate_still_compares` —
  `...step_resolution::tests::compile_language_predicate_still_compares`:
  source `${header.h} == 'B'` with header `h=B` → matches `true`.
- `compile_language_expression_propagates_template_errors` —
  `...step_resolution::tests::compile_language_expression_propagates_template_errors`:
  source `${header.}` → `Err`.
- `resequence_correlation_template_source` —
  `cargo test -p camel-core --lib lifecycle::adapters::route_controller::tests::resequence_correlation_template_source`:
  compile a route with resequence correlation `X-${header.id}-01` → compiles.
- `route_add_fails_on_template_looking_literal` (route-level) —
  `cargo test -p camel-core --test expression_failure_propagation_test route_add_fails_on_template_looking_literal`:
  via `new_context()`/`run_route`, a route whose `set_body` value source is
  `Expression{simple, "echo ${HOME}"}` fails at route add/run with an
  `unrecognized token` parse error.
- `substituted_env_slot_runtime` (route-level) — same command filter with the
  test name: value source `Expression{simple, "Hello ${header.h}"}`, header
  `h=B` → body `Hello B`.
- `escaped_env_slot_fails_at_runtime` (route-level) — same command filter:
  value source `Expression{simple, "$${env:NAME}"}` compiles, then the run
  returns `Err`; assert with `assert_expression_failed(&err, route_id,
  "simple", "<verb>", ExpressionErrorClass::Runtime, Pos::None)`. Do NOT assert
  the error text: `to_expression_failed` discards the detail and sets
  `cause: None`, so `Language 'env' not found` is only observable at the
  language-crate boundary (Task 1.2 `escaped_env_slot_error_text`).
- `cross_language_simple_and_jsonpath_rows` (migrated; command
  `cargo test -p camel-core --test expression_failure_propagation_test cross_language_simple_and_jsonpath_rows`):
  `set_property` source `${nosuchlang:x}` → `Err` class `Runtime`; no
  `abc`/`secret` operand leak.
- `verb_script_readonly_expression_error_fails_step` (migrated; command
  `cargo test -p camel-core --test expression_failure_propagation_test verb_script_readonly`):
  `failing_steps("script_readonly")` source `${nosuchlang:x}` and
  `verb_diagnostics("script_readonly")` class `Runtime` (keep `Pos::None`);
  companions `verb_script_readonly_do_try_visible` / `..._on_exception_visible`
  follow the same source.

**Acceptance:**
- `cargo test -p camel-core` passes.
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.

- [x] 2.2

### camel-component-file

#### Task 2.3: Filename template regression

**Files:**
- `crates/components/camel-file/src/lib.rs` (modified)

**Steps:**
1. No production change expected: `resolve_filename` already calls
   `SimpleLanguage::create_expression`; template mode now preserves the slot.
2. Add a regression test with `CamelFileName` or `fileName`
   `${header.id}-01.txt` and header `id=X`.

**Tests:** (`camel-file` tests, command `cargo test -p camel-component-file`)
- `test_file_producer_filename_simple_language_exact_suffix`: header `id=X`,
  `fileName: "${header.id}-01.txt"` → resolved filename exactly `X-01.txt`.
- Existing `test_file_producer_filename_simple_language_from_header` (2688)
  and `test_file_producer_filename_simple_language_from_uri_param` (2722)
  still pass.

**Acceptance:**
- `cargo test -p camel-component-file` passes.
- `cargo clippy -p camel-component-file --all-targets -- -D warnings` exits 0.

- [x] 2.3

### camel-integration-test

#### Task 2.4: End-to-end bundle route for template interpolation

**Files:**
- `crates/camel-integration-test/tests/simple_template_wave_test.rs` (new)
- `crates/camel-integration-test/tests/fixtures/logs/simple-template.routes.yaml` (new)

**Steps:**
1. Add the fixture `tests/fixtures/logs/simple-template.routes.yaml`:
   ```
   routes:
     - id: simple-template
       from: direct:start
       steps:
         - set_body: "X-${header.id}"
         - to: "file:${env:OUT_DIR}?fileName=${header.id}-01.txt"
         - set_body: "Got -> ${header.id}"
         - to: "log:marker?level=INFO&showBody=true"
   ```
   `OUT_DIR` resolves from the document `env:`. The camel-bundles dependency
   already includes `camel-component-file`; the harness context already
   registers the Simple language — do NOT register Simple or add file deps.
2. In `simple_template_wave_test.rs`: `mod common;`, `use
   common::{lock_run, run_logs_document};`, build the document string at
   runtime with a `tempfile::tempdir()` value interpolated into its `env:
   OUT_DIR`, and call `run_logs_document(&doc, "simple-template.routes.yaml")`
   under `lock_run()`. Annotate the test
   `#[allow(clippy::await_holding_lock)]` (the guard is held across the
   awaited run; same pattern as `log_assertion_test.rs`).

**Tests:** (command
`cargo test -p camel-integration-test --test simple_template_wave_test`)
- `simple_template_bundle_route` — setup: `tempdir()`; doc string with
  `env: {OUT_DIR: "<tempdir>"}`, `routeFiles: [routes.yaml]`, one
  `send: {to: direct:start, body: x, headers: {id: X}}`, and
  `logs: {contains: ['Got -> X']}`; fixture as in step 1. action: `lock_run()`
  then `run_logs_document(&doc, "simple-template.routes.yaml")`. assert:
  `outcome.verdict == Some(ScenarioVerdict::Pass)` and
  `outcome.logs_failure.is_none()`; then
  `std::fs::read_to_string(tempdir.path().join("X-01.txt")) == "X-X"`. Both
  the log capture and the file bytes are observed through the existing
  harness.

**Acceptance:**
- `cargo test -p camel-integration-test --test simple_template_wave_test`
  passes.
- `cargo clippy -p camel-integration-test --all-targets -- -D warnings` exits 0.

- [x] 2.4

## Phase 3: JS body write-back + Rhai fixes

### camel-language-js

#### Task 3.1: Normalized changed-only body write-back + null→Empty

**Files:**
- `crates/languages/camel-language-js/src/engine.rs` (modified)
- `crates/languages/camel-language-js/src/engines/worker.rs` (modified)
- `crates/languages/camel-language-js/src/expression.rs` (modified)
- `crates/languages/camel-language-js/src/language.rs` (modified)

**Steps:**
1. Add `pub body_snapshot: serde_json::Value` to `JsEvalResult` (engine.rs)
   with a doc contract: it is the pre-run body after the SAME conversion the
   result body passes through; it is meaningful only for mutating evaluation.
   An external engine that cannot reproduce the conversion SHALL set it to the
   inbound body — the worst case is extra write-backs, never data loss. This is
   an explicit public-shape (breaking field addition) to bless; do NOT add a
   JsEngine trait method and do NOT add a separate blocking job.
2. In `worker.rs::eval_cached`, only when `sentinel.is_none()` (mutating eval),
   immediately BEFORE running the script, call a NEW helper
   `fn extract_body_snapshot(ctx: &mut Context, detector: Option<&ArrayDetector>)
   -> Result<serde_json::Value, JsLanguageError>` (added in `bindings.rs` or
   `worker.rs`) that reads the installed `camel.body` and converts it with
   `js_to_value_with_detector` — the SAME conversion the result body uses —
   and store the result as `body_snapshot`. Do not use the state-wide
   `extract_camel_state`; this is a body-only helper. In read-only mode
   (`sentinel.is_some()`) skip the helper and set `body_snapshot` to the
   inbound body.
3. Update the other `JsEvalResult { .. }` construction sites. In read-only
   mode the cached `eval_cached` does NOT hold the exchange, so pass
   `exchange.body.clone()` down from `run_eval` (which owns `exchange: JsExchange`)
   as an explicit argument and set `body_snapshot` to it; `expression.rs` mock
   engines set it to the inbound body; `language.rs` and the `worker.rs` test
   blocks set the inbound body. A mock/simulated engine that does not run Boa
   therefore compares the result against the inbound body.
4. In `JsMutatingExpression::evaluate` / `apply_result_to_exchange`, compare
   `result.body != result.body_snapshot` and write `value_to_body(&result.body)`
   only when they differ; `value_to_body(Value::Null)` → `Body::Empty`.
   Keep header/property write-back and rollback unchanged.

**Tests:** (`expression.rs` tests, real `BoaEngine`; command
`cargo test -p camel-language-js`)
- `js_header_only_preserves_xml_body`: body `Body::Xml("<order/>")`, script
  `camel.headers.set('x','1')` → after eval `matches!(body, Body::Xml(_))`;
  expected PASS after step 4.
- `js_header_only_preserves_integral_float_json_body`: body
  `Body::Json(json!({"n":10.0}))`, header-only script → `body` still
  `Body::Json` with `n` equal to `10.0`; expected PASS.
- `js_null_body_becomes_empty`: body `Body::Text("x")`, script
  `camel.body = null` → `matches!(body, Body::Empty)`; expected PASS.
- `js_null_on_bytes_is_noop`: body `Body::Bytes`, script `camel.body = null`
  → still `Body::Bytes`.
- `js_null_on_empty_is_noop`: body `Body::Empty`, script `camel.body = null`
  → still `Body::Empty`.
- `js_reader_sees_live_mutation` — setup: header `k=old`; script
  `camel.headers.set("k","new"); camel.headers.get("k")`. action: evaluate the
  mutating expression. assert: `return_value` is `"new"` and the exchange header
  `k` is `"new"` (JS observes the live map, unlike rhai). command:
  `cargo test -p camel-language-js js_reader_sees_live_mutation`.
  expected: PASS.
- Existing `test_mutating_expression_propagates_body`,
  `preserves_bytes_body`, `preserves_empty_body` still pass.

**Acceptance:**
- `cargo test -p camel-language-js` passes.
- `cargo clippy -p camel-language-js --all-targets -- -D warnings` exits 0.
- The commit carries a breaking marker (`!` subject or `BREAKING CHANGE:`
  footer) for the `JsEvalResult` public-field addition.

- [x] 3.1

### camel-language-rhai

#### Task 3.2: Pure trim shadow (trim only)

**Files:**
- `crates/languages/camel-language-rhai/src/string_shadow.rs` (new)
- `crates/languages/camel-language-rhai/src/lib.rs` (modified)
- `docs/src/languages/rhai.md` (modified)

**Steps:**
1. Add `string_shadow::register(engine: &mut Engine)` registering
   `trim(&str) -> String` (`s.trim().to_string()`). Do NOT add any `_inplace`
   alias.
2. Call it at the end of `create_base_engine`, after `StandardPackage` and the
   json host module.
3. Update `rhai.md` "String methods mutate in place": `trim()` is now pure;
   document `make_upper`, `make_lower`, `clear`, `truncate`, `remove`, `crop`,
   `replace`, `pad` as in-place with pure alternatives (`to_upper`, `to_lower`,
   `sub_string`) where they exist. For methods with NO pure alternative
   (`replace`, `pad`, `remove`, `clear`), document the copy-then-mutate
   pattern (`let y = x; y.replace("a", "b"); y`). Record the probe result that
   `trim_start`/`trim_end` and the `*_ascii`/`trim_to_string` variants are
   absent from rhai 1.26.1 `StandardPackage` (nothing to shadow). Note the
   breaking change.
4. Update the crate-level doc block in
   `crates/languages/camel-language-rhai/src/lib.rs` (~lines 82-89) which
   currently says "String methods such as `replace`, `trim`, and `pad` mutate
   the subject in place and return unit": move `trim` to a pure-returns-string
   note and keep `replace`/`pad` in the in-place list.

**Tests:** (`crates/languages/camel-language-rhai/tests/string_shadow.rs`,
command `cargo test -p camel-language-rhai --test string_shadow`)
- `trim_returns_string`: expression `let x = "  a  "; x.trim() == "a"` →
  `Value::Bool(true)`.
- `trim_blank_check_is_correct`: `let x = "   "; x.trim() == ""` → `true`;
  `let x = "   "; x.trim() != ""` → `false`.
- `trim_does_not_mutate_subject`: `let x = "  a  "; x.trim(); x == "  a  "` →
  `true`.
- `trim_shadow_applies_in_script_step`: mutating expression
  `let x = "  a  "; x.trim() == "a"` → `Value::Bool(true)`.

**Acceptance:**
- `cargo test -p camel-language-rhai --test string_shadow` passes.
- `cargo clippy -p camel-language-rhai --all-targets -- -D warnings` exits 0.
- The commit carries a breaking marker (`!` subject or `BREAKING CHANGE:`
  footer) for the pure-`trim` semantic change.

- [x] 3.2

#### Task 3.3: Read surface (properties map, script readers, rhai hint)

**Files:**
- `crates/languages/camel-language-rhai/src/lib.rs` (modified)
- `crates/languages/camel-language-rhai/tests/read_only_surface.rs` (new)

**Steps:**
1. In `eval_sync`, push `properties` into the read-only scope.
2. Extract the `header()`/`property()` registration from `create_eval_engine`
   into `fn register_snapshot_readers(engine: &mut Engine, headers: rhai::Map,
   properties: rhai::Map)`; call it from `create_eval_engine` and from
   `eval_mut_sync` with the pre-eval snapshot maps.
3. Change ONLY the rhai `set_header()`/`set_property()` rejection hint to name
   `headers["k"] = value`, `properties["k"] = value`, `body = value`. Leave the JS
   hint/error unchanged.
4. Update the rustdoc in the same `lib.rs` (m6): the `RhaiLanguage` read-only
   doc list (~304-311) adds `properties` alongside `body`/`headers`; the
   `RhaiMutatingExpression` "Note on API differences" (~823-834) currently
   says it does NOT provide `header()`/`property()`/`set_header()`/
   `set_property()` — rewrite to state `header()` and `property()` ARE now
   provided (backed by the pre-eval snapshot) while `set_*` remain
   map-assignment only.

**Tests:** (`crates/languages/camel-language-rhai/tests/read_only_surface.rs`,
command `cargo test -p camel-language-rhai --test read_only_surface`)
- `read_only_properties_map_is_readable`: expression `properties["p"]` with
  property `p=pv` → `Value::String("pv")`; expected PASS after step 1.
- `script_step_property_reader_works`: mutating expression `property("p")` →
  `Value::String("pv")`; expected PASS after step 2.
- `script_step_header_reader_works`: mutating expression `header("h")` →
  `Value::String("hv")`; expected PASS.
- `set_property_hint_names_map_assignment`: `create_expression("set_property(\"a\",\"b\")")`
  error text contains `headers[` or `properties[`; expected PASS after step 3.
- `rhai_unit_on_bytes_becomes_empty` (I1): body `Body::Bytes`, mutating script
  `body = ()` → `matches!(body, Body::Empty)`; expected PASS.
- `rhai_unit_on_empty_unchanged` (I1): body `Body::Empty`, mutating script
  `body = ()` → still `Body::Empty`; expected PASS.
- `script_reader_sees_pre_run_snapshot` — setup: exchange header `k=old`,
  property `p=old`; mutating script
  `let a = header("k"); headers["k"] = "new"; let b = header("k"); [a, b]`.
  action: evaluate. assert: returned value is the array `["old","old"]`
  (readers reflect the pre-run snapshot), the exchange header `k` is `"new"`
  (the write propagates), and the same holds for `property("p")` with
  `properties["p"] = "newp"`. command:
  `cargo test -p camel-language-rhai --test read_only_surface script_reader_sees_pre_run_snapshot`.
  expected: PASS after step 2.
- `same_value_property_assignment_is_noop` — setup: exchange property
  `p` set as a JSON integer `1`, two rhai mutating scripts. action: (a)
  `properties["p"] = 1`; (b) `properties["p"] = 1.0`. assert (observable value
  only; the stored handle is not observable): (a) after eval the property
  reads back as an integer — assert `value.as_i64() == Some(1)` and
  `value.as_f64()` is not the discriminator; (b) after eval the property reads
  back as the float `1.0` — assert `value.as_f64() == Some(1.0)` and
  `value.as_i64() == None` (the int→float change is type-sensitive and written
  back). command:
  `cargo test -p camel-language-rhai --test read_only_surface same_value_property_assignment_is_noop`.
  expected: PASS (Rhai only; JS map parity is rc-zwy7e).

**Acceptance:**
- `cargo test -p camel-language-rhai` passes.
- `cargo clippy -p camel-language-rhai --all-targets -- -D warnings` exits 0.

- [x] 3.3

#### Task 3.4: Reject read-only assignments to body/headers/properties (conservative)

**Files:**
- `crates/languages/camel-language-rhai/src/lib.rs` (modified)
- `crates/languages/camel-language-rhai/tests/read_only_surface.rs` (modified)

**Steps:**
1. Extend `find_read_only_mutation` in `lib.rs` to inspect
   `Stmt::Assignment`: reject when
   the LHS root variable name is `body`, `headers`, or `properties`. Resolve the
   root by descending `Expr::Variable`, `Expr::Index { lhs }`, `Expr::Dot { lhs }`.
   Cover `=` and `OpAssignment` (`+=`, `-=`, `*=`, `/=`) and nested/index
   targets.
2. Conservative: do not attempt scope analysis; a local shadowing the name is
   rejected too (documented).
3. Keep rejecting `set_header()`/`set_property()`. Do not reject `x = headers`
   or pure method calls. Method-call mutations on those roots
   (`headers.clear()`, `headers.remove(x)`) are the KNOWN REMAINING GAP
   rc-qpjps, not covered here.

**Tests:** (add to `read_only_surface.rs`; command as above)
- `read_only_index_assignment_rejected`: `headers["x"] = "1"` → `Err`.
- `read_only_body_assignment_rejected`: `body = "changed"` → `Err`.
- `read_only_nested_assignment_rejected`: `properties["a"]["b"] = "c"` → `Err`.
- `read_only_compound_update_rejected`: `headers["x"] += 1` → `Err`.
- `read_only_shadowing_local_rejected`: `let headers = #{}; headers["x"] = 1` → `Err` (conservative).
- `read_only_alias_is_not_rejected`: `let x = headers; x` → `Ok`.
- `read_only_pure_method_ok`: `body.len` → `Ok` and evaluates.

**Acceptance:**
- `cargo test -p camel-language-rhai` passes.
- `cargo clippy -p camel-language-rhai --all-targets -- -D warnings` exits 0.

- [x] 3.4

### docs

#### Task 3.5: Rhai docs read-surface note

**Files:**
- `docs/src/languages/rhai.md` (modified)
- `crates/languages/camel-language-rhai/CONTEXT.md` (modified)

**Steps:**
1. Document that read-only and script-step modes both expose `body`, `headers`,
   `properties`, `header()`, and `property()`.
2. Note that rhai script-step `header()`/`property()` read the pre-run snapshot,
   so after `headers["k"] = "new"` a call to `header("k")` still returns the
   pre-run value, whereas JS `camel.headers.get("k")` sees the live mutated map
   (M7: pin the divergence).
3. Note the conservative read-only assignment rejection and the rhai-only hint.
4. Note the deferred gaps: `has_header()`/`has_property()` (rc-w1mds) and
   method-call mutations (rc-qpjps).
5. Document the literal-`${` workaround until rc-luew6: emit a literal `${...}`
   through an explicit `language:`+`source:` quoted rhai/js string, escaping
   env/arg tokens as `$$` (for example `language: rhai,
   source: '"$${env:X}"'` → literal `${env:X}`; `source: '"${header.h}"'` →
   literal `${header.h}` without an escape). State that `$$` is not a
    Simple-template escape. At the Simple source level after substitution,
    `$${header.h}` is `$` plus a live slot. In a route file, the env pass first
    turns `$$` into `$`: `$${header.h}` reaches Simple as the whole slot
    `${header.h}` and retains its native value. Write `$$$${header.h}` in a
    route file for `$` plus a slot.

**Tests:**
- `cargo test -p camel-language-rhai` doc-tests / `cargo doc -p camel-language-rhai --no-deps`; expected PASS.
- `cargo xtask lint-context-citations` exits 0.

**Acceptance:**
- `rhai.md` and `CONTEXT.md` match the implemented surfaces.

- [x] 3.5
