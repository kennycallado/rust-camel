# Tasks: rhai-big-int-json-number

Single-phase change. Execution order: 1.1 → 1.2 → 2.1 → 2.2. Worktree:
`/home/shared/rust-camel-worktrees/rhaiu64`. All `cargo` commands run from the
worktree root. Primary crate: `camel-language-rhai`; Task 2.1 also rebuilds
one fixture in `camel-core` integration tests and Task 2.2 moves repo docs.
The Tests blocks are exact:
write the named tests first, run them, record the result, then implement.
Do NOT invent additional test cases; if a specified test is impossible as
written, STOP and report `test-design-gap: <what>`.

## camel-language-rhai: boundary converter and JSON host

### Task 1.1: Bind big inbound integers as `json number`; outbound exact-image conversion

**Files:**
- `crates/languages/camel-language-rhai/src/converter.rs` (modified)
- `crates/languages/camel-language-rhai/src/json/mod.rs` (modified)

**Steps:**
1. In `converter.rs` `mod tests`: replace `u64_above_i64_max_refused_inbound`
   with `u64_above_i64_max_binds_json_number_inbound` (spec below). Replace
   `json_number_refused_outbound_stable_label` with
   `json_number_non_u64_token_still_refused_outbound` (spec below; the old
   token `"1"` case is covered by the new `"5"` case — same stable label).
   Add `json_number_u64_round_trips_outbound` (spec below).
2. Run `cargo test -p camel-language-rhai --lib converter::`. Record:
   `u64_above_i64_max_binds_json_number_inbound` and
   `json_number_u64_round_trips_outbound` must FAIL (current code refuses
   inbound and all wrappers outbound);
   `json_number_non_u64_token_still_refused_outbound` may already PASS
   (blanket refusal) — it stays as the regression pin.
3. In `json/mod.rs`, next to `token_projects_to_i64`, add:
   `pub(crate) fn as_u64_above_i64(token: &str) -> Option<u64>` with a doc
   comment stating: returns `Some(u)` only for canonical unsigned decimal
   tokens — every byte an ASCII digit, NO leading zero — whose parsed value
   is strictly greater than `i64::MAX`; anything else is `None` (the exact
   inbound image, bd rc-141om). Implementation: reject on any non-digit byte
   or a leading `'0'` when `token.len() > 1`, then `token.parse::<u64>().ok()`,
   then keep only values `> i64::MAX as u64`. No `unwrap`/`expect` outside
   that one lossless `as` cast with a justification comment.
4. In `converter.rs`, add `use crate::json::JsonNumber;` (one module per
   `use`, explicit import — lint-glob-imports). Rewrite the doc comment of
   `json_to_dynamic`: delete the refusal sentence and state — a JSON integer
   in `(i64::MAX, u64::MAX]` binds as the `json number` wrapper carrying its
   exact decimal token on every inbound path (bd rc-141om; refines owner
   decision 2026-10-02, sealed Q4: no lossy float; the explicit lossy path
   is `x.to_float()`). Keep the `target` discipline paragraphs unchanged.
5. Inbound arm (`Value::Number`): in the `n.as_u64()` branch, replace
   `return Err(refused("u64 > i64::MAX"))` with
   `rhai::Dynamic::from(JsonNumber::new(n.to_string()))` and update the
   branch comment (serde_json renders a `u64`-backed number as the exact
   plain decimal).
6. Outbound arm (`dynamic_to_value`): replace the
   `Err(refused(crate::json::JSON_NUMBER_TYPE_NAME))` arm with:
   match `crate::json::as_u64_above_i64(<token>.as_str())` —
   `Some(u)` → `Ok(Value::from(u))`;
   `None` → `Err(refused(crate::json::JSON_NUMBER_TYPE_NAME))`.
   Obtain the token via `d.cast::<JsonNumber>()` after the existing
   `is::<JsonNumber>()` guard (no unwrap). Update the arm comment: only the
   exact inbound image converts; every other token keeps the stable
   `json number` refusal (rc-qka42 deferral).
7. Run `cargo test -p camel-language-rhai --lib converter::` — all green.
8. `cargo fmt -p camel-language-rhai -- --check` and
   `cargo clippy -p camel-language-rhai --all-targets -- -D warnings` — fix
   anything they flag.

**Tests:** (executable spec)
- `u64_above_i64_max_binds_json_number_inbound`: arrange
  `Value::from(u64::MAX)` → act `json_to_dynamic(&v, "property big")` →
  assert Ok; `result.is::<JsonNumber>()`; cast and `as_str()` equals
  `"18446744073709551615"`. command:
  `cargo test -p camel-language-rhai --lib converter::tests::u64_above_i64_max_binds_json_number_inbound`.
  expected before step 5: FAIL (currently refuses).
- `json_number_u64_round_trips_outbound`: arrange inbound
  `json_to_dynamic(&Value::from(u64::MAX), "value")` → act
  `dynamic_to_value(d, "value")` → assert Ok, result equals
  `Value::from(u64::MAX)`, and `result.is_u64()` is true. command:
  `cargo test -p camel-language-rhai --lib converter::tests::json_number_u64_round_trips_outbound`.
  expected before step 6: FAIL.
- `json_number_non_u64_token_still_refused_outbound`: for each token in
  `["1.5", "5", "18446744073709551616", "09223372036854775808"]` arrange
  `Dynamic::from(JsonNumber::new(token))` → act `dynamic_to_value(d, "value")`
  → assert `Err(LanguageError::ConversionError { source_type, .. })` with
  `source_type == "json number"`. command:
  `cargo test -p camel-language-rhai --lib converter::tests::json_number_non_u64_token_still_refused_outbound`.
  expected: PASS before and after (regression pin; the leading-zero token
  pins the canonical-spelling rule from the blessed spec).

**Acceptance:**
- `cargo test -p camel-language-rhai --lib converter::` passes. NOTE: the
  four lib.rs tests that pin the old refusal
  (`json_inbound_u64_property_still_refused`,
  `conversion_error_precedence_body_before_header`,
  `conversion_error_precedence_header_before_property`,
  `group_conversion_failure_is_indexed_to_clause_zero`) stay RED until Task
  2.1 rebuilds them — do NOT touch `lib.rs` in this task.
- `cargo clippy -p camel-language-rhai --all-targets -- -D warnings` exits 0.
- `cargo fmt -p camel-language-rhai -- --check` exits 0.
- No new `unwrap()` outside tests (`cargo xtask lint-unwrap` exits 0).
- `cargo xtask lint-glob-imports` and `cargo xtask lint-missing-docs-ratchet`
  exit 0.

- [x] 1.1

## camel-language-rhai: transaction diff

### Task 1.2: Compare `json number` by token in the transaction diff

**Files:**
- `crates/languages/camel-language-rhai/src/transaction.rs` (modified)

**Steps:**
1. Add `#[cfg(test)] mod tests` at the end of `transaction.rs` with
   `json_number_token_equality_diffs_by_token` (spec below). Run it — it
   compiles, and the first equality assertion FAILS until steps 2-3 land
   (expected RED; both sides classify `Kind::Other` pre-change).
2. In `transaction.rs`, add `use crate::json::JsonNumber;` and a new
   `Kind::JsonNumber` variant to the private `Kind` enum. In `kind()`, add a
   branch `d.is::<JsonNumber>() → Kind::JsonNumber` placed after the
   `Blob` branch, before the `Other` fallthrough.
3. In `rhai_values_differ`, add the arm
   `Kind::JsonNumber => !json_numbers_equal(pre, post)`, and add
   `fn json_numbers_equal(pre: &Dynamic, post: &Dynamic) -> bool` mirroring
   `blobs_equal`: `read_lock::<JsonNumber>()` both sides, `Some`/`Some`
   compares `as_str()` equality, any other shape is `false` (→ reported as
   differing). Update the `Kind::Other` comment so `JsonNumber` is excluded
   from the "exotic / conservative" list.
4. Run `cargo test -p camel-language-rhai --lib transaction::` — green.
5. `cargo fmt -p camel-language-rhai -- --check` and
   `cargo clippy -p camel-language-rhai --all-targets -- -D warnings`.

**Tests:** (executable spec)
- `json_number_token_equality_diffs_by_token`: at the top of the new
  `#[cfg(test)] mod tests` add `use super::rhai_values_differ;` and
  `use crate::json::JsonNumber;`. Arrange
  `engine = rhai::Engine::new()`, `a = Dynamic::from(JsonNumber::new("18446744073709551615"))`,
  `b = Dynamic::from(JsonNumber::new("18446744073709551615"))`,
  `c = Dynamic::from(JsonNumber::new("5"))`, `i = Dynamic::from(5_i64)` →
  act/assert `rhai_values_differ(&engine, &a, &b)` is `false` (same token —
  no delta, original value handle kept), `rhai_values_differ(&engine, &a, &c)`
  is `true` (different token), `rhai_values_differ(&engine, &a, &i)` is
  `true` (kind mismatch). command:
  `cargo test -p camel-language-rhai --lib transaction::tests::json_number_token_equality_diffs_by_token`.
  expected before steps 2-3: compiles, but the FIRST assertion fails
  (pre-change both sides classify as `Kind::Other`, conservatively
  "differing").

**Acceptance:**
- `cargo test -p camel-language-rhai --lib transaction::` passes. The lib.rs
  refusal tests noted in Task 1.1 stay RED until Task 2.1 — do NOT touch
  `lib.rs` in this task.
- `cargo clippy -p camel-language-rhai --all-targets -- -D warnings` exits 0.
- `cargo fmt -p camel-language-rhai -- --check` exits 0.

- [x] 1.2

## camel-language-rhai: language eval integration

### Task 2.1: Integration tests — eval paths bind big integers without failing

**Files:**
- `crates/languages/camel-language-rhai/src/lib.rs` (modified)
- `crates/camel-core/tests/expression_failure_propagation_test.rs` (modified)

**Steps:**
1. Update the `snapshot` doc comment (search for
   `u64 > i64::MAX,` in the doc text near `fn snapshot`): replace the
   refusal wording with — a header, property, or body holding a JSON integer
   in `(i64::MAX, u64::MAX]` binds as the `json number` wrapper (bd
   rc-141om); inbound conversion refuses nothing else for serde_json values.
2. Replace `json_inbound_u64_property_still_refused` with
   `json_inbound_u64_property_binds_json_number` (spec below).
3. Add `big_header_does_not_fail_unrelated_later_step` (spec below) using
   `RhaiLanguage::create_mutating_expression` + `evaluate(&mut ex)` twice on
   the same exchange (mirror `test_mutating_set_header_propagates_to_exchange`).
4. Add `json_body_u64_consistent_with_headers` (spec below) using the
   `eval_script` helper.
5. Add `json_number_vs_int_comparison_refused` (spec below) using
   `eval_script` and the existing `assert_class` helper with
   `ExpressionErrorClass::TypeMismatch`.
6. Add `nested_big_int_survives_sibling_mutation` (spec below) using
   `create_mutating_expression("body[\"b\"] = 2")`.
7. Rebuild `conversion_error_precedence_body_before_header` per the blessed
   design Test-design note 1: keep the name, assert the NEW Ok behavior
   (spec below).
8. Rebuild `conversion_error_precedence_header_before_property` per design
   note 2: mutating script assigns `timestamp()` to both a header and a
   property; assert the error target is `header entry` (write-back order:
   header deltas validate before property deltas) (spec below).
9. Add `copied_json_number_entry_converts_exact_u64` (spec below) — the
   blessed "Changed entry converts to the exact u64" scenario: a changed
   top-level entry holding a `json number` converts through the outbound
   converter to the exact `Value::Number(u64)`.
10. Invert `group_conversion_failure_is_indexed_to_clause_zero` per design
    note 3: a `u64::MAX` header no longer refuses pre-loop; assert
    `first_match` returns `Ok(Some(0))` and `eval_log() == vec![0]` (spec
    below).
11. In `crates/camel-core/tests/expression_failure_propagation_test.rs`,
    rename `inbound_scope_refusal_keeps_generic_target` (line ~1302,
    feature-gated `lang-rhai` + `lang-jsonpath`) to
    `inbound_scope_big_property_binds_and_step_succeeds` and rebuild it per
    the spec below: the same two route steps (property `huge` =
    `Value::from(u64::MAX)`, then rhai `"1"`) now SUCCEED — the inbound
    property-entry refusal is no longer constructible via rhai. The
    carrier's trusted-target rewrite stays pinned by
    `set_property_conversion_refusal_names_trusted_target` (float
    non-finite, unchanged).
12. Run `cargo test -p camel-language-rhai --lib` — all green (Tasks 1.1
    and 1.2 landed the behavior; these tests pin it at the language level).
13. Run
    `cargo test -p camel-core --features lang-rhai,lang-jsonpath --test expression_failure_propagation_test`
    — all green.
14. `cargo fmt -p camel-language-rhai -- --check` and
    `cargo fmt -p camel-core -- --check`; `cargo clippy -p camel-language-rhai --all-targets -- -D warnings`.

**Tests:** (executable spec — command prefix for the lib.rs tests:
`cargo test -p camel-language-rhai --lib <name>`; the final camel-core
bullet carries its own command; expected: GREEN once
steps 1-11 compile, RED on the pre-change base — EXCEPT
`conversion_error_precedence_header_before_property`, which pins the
already-existing write-back order and is GREEN on the pre-change base too)
- `json_inbound_u64_property_binds_json_number`: arrange exchange with
  `properties["big"] = Value::from(u64::MAX)` → act `eval_script("type_of(property(\"big\"))", &ex)`
  and `eval_script("property(\"big\").to_string()", &ex)` → assert first
  returns `Value::String("json number")` and second returns
  `Value::String("18446744073709551615")`.
- `big_header_does_not_fail_unrelated_later_step`: arrange exchange with
  `headers["big"] = Value::from(u64::MAX)` → act step 1
  `create_mutating_expression("headers[\"x\"] = 1")` + `evaluate(&mut ex)`,
  step 2 `create_mutating_expression("body = \"ok\"")` + `evaluate(&mut ex)`
  → assert both steps `Ok`; `ex.input.headers.get("big") == Some(&Value::from(u64::MAX))`
  (bit-identical); `ex.input.headers.get("x") == Some(&Value::from(1))`;
  `ex.input.body.as_text() == Some("ok")`.
- `json_body_u64_consistent_with_headers`: arrange exchange with
  `Body::Json(Value::from(u64::MAX))` and
  `headers["big"] = Value::from(u64::MAX)` → act `eval_script("type_of(body)", &ex)`
  and `eval_script("type_of(header(\"big\"))", &ex)` → assert both return
  `Value::String("json number")` (the Q4 body ruling: body and headers
  bind uniformly).
- `json_number_vs_int_comparison_refused`: arrange exchange with
  `headers["big"] = Value::from(u64::MAX)` → act
  `eval_script("header(\"big\") == 1", &ex)` → assert Err with class
  `TypeMismatch` via `assert_class` (no implicit coercion).
- `nested_big_int_survives_sibling_mutation`: arrange exchange with
  `Body::Json(Value::Object({"a": Value::from(u64::MAX), "b": Value::from(1)}))`
  → act `create_mutating_expression("body[\"b\"] = 2")` + `evaluate(&mut ex)`
  → assert Ok; read the committed body by matching
  `ex.input.body` against `Body::Json(v)` and taking `v.as_object()`:
  `obj.get("a") == Some(&Value::from(u64::MAX))` (exact image survived),
  `obj.get("b") == Some(&Value::from(2))`.
- `conversion_error_precedence_body_before_header` (rebuilt): arrange
  exchange with `Body::Json(Value::from(u64::MAX))` and
  `headers["big"] = Value::from(u64::MAX)` → act
  `lang.create_expression("body")` + `evaluate(&ex)` → assert Ok and the
  returned value equals `Value::from(u64::MAX)`; header untouched. Doc
  comment states: inbound conversion is now total for serde_json values;
  the body-before-header snapshot precedence remains defined for the
  forward-compat unknown-Body-variant arm.
- `conversion_error_precedence_header_before_property` (rebuilt): arrange
  exchange with `headers["old"]` and `properties["old"]` set to plain
  strings → act `create_mutating_expression("headers[\"big\"] = timestamp(); properties[\"big\"] = timestamp()")`
  + `evaluate(&mut ex)` → assert Err `ConversionError` with
  `target == "header entry"` (write-back validates header deltas before
  property deltas; `timestamp()` is the refused-outbound `Instant`).
- `copied_json_number_entry_converts_exact_u64`: arrange exchange with
  `properties["big"] = Value::from(u64::MAX)` → act
  `create_mutating_expression("properties[\"copy\"] = property(\"big\")")` +
  `evaluate(&mut ex)` → assert Ok; committed
  `ex.properties.get("copy") == Some(&Value::from(u64::MAX))` (the changed
  entry converted through the outbound converter to the exact u64 image);
  `ex.properties.get("big")` keeps its original value handle (identical u64).
- `group_conversion_failure_is_indexed_to_clause_zero` (inverted): arrange
  `rhai_group(&lang, &["true", "true"])` and exchange with
  `headers["big"] = Value::from(u64::MAX)` → act `first_match(&exchange)` →
  assert `Ok(Some(0))` (clause 0 matched; no pre-loop refusal — the group
  path binds `json number`) and `group.eval_log() == vec![0]`. Doc comment
  states: no constructible pre-loop refusal fixture remains after rc-141om.
- `inbound_scope_big_property_binds_and_step_succeeds` (renamed and rebuilt
  from `inbound_scope_refusal_keeps_generic_target`): keep the same two
  route steps — `DeclarativeSetProperty { key: "huge", value_source:
  literal(Value::from(u64::MAX)) }` then `DeclarativeSetProperty { key: "p",
  value_source: Expression(rhai("1")) }` — and `run_route("inbound-scope-refusal", steps, None)`
  → assert `result.is_ok()`; on the returned `Exchange`,
  `ex.properties.get("huge") == Some(&Value::from(u64::MAX))` (the big
  property binds and survives the route). Doc comment states: the inbound
  property-entry refusal is no longer constructible via rhai (bd rc-141om);
  the carrier's trusted-target rewrite stays pinned by
  `set_property_conversion_refusal_names_trusted_target`. command:
  `cargo test -p camel-core --features lang-rhai,lang-jsonpath --test expression_failure_propagation_test inbound_scope_big_property_binds_and_step_succeeds`.

**Acceptance:**
- `cargo test -p camel-language-rhai --lib` passes.
- `cargo test -p camel-core --features lang-rhai,lang-jsonpath --test expression_failure_propagation_test`
  passes (all tests in the target, including the rebuilt one).
- `cargo clippy -p camel-language-rhai --all-targets -- -D warnings` exits 0.
- `cargo fmt -p camel-language-rhai -- --check` exits 0.

- [x] 2.1

## docs

### Task 2.2: CONTEXT.md, user guide, and CHANGELOG entry

**Files:**
- `crates/languages/camel-language-rhai/CONTEXT.md` (modified)
- `docs/src/languages/rhai.md` (modified)
- `CHANGELOG.md` (new)

**Steps:**
1. In `CONTEXT.md`, replace the paragraph beginning "The outbound converter
   `dynamic_to_value` refuses `JsonValue` and `JsonNumber`" (lines ~64-67)
   with: the outbound converter refuses `JsonValue` with `json value`, and
   refuses `JsonNumber` with `json number` EXCEPT when its token is a plain
   decimal integer in canonical spelling within `(i64::MAX, u64::MAX]` — the
   exact inbound image, which converts back to the identical `u64`. Keep the
   sentence about persisting parsed JSON via `to_json` or a native leaf, and
   exempt the accepted token class exactly as the blessed delta does.
2. In `CONTEXT.md`, update the Authority paragraph (lines ~69-70): `rc-m01r9`
   (eager scope conversion) remains out of scope; `rc-141om` is resolved by
   the `json number` inbound binding, refining owner decision 2026-10-02
   (sealed Q4) without reversing it.
3. In `docs/src/languages/rhai.md`, replace the inbound-side paragraph
   "On the inbound side, a JSON integer greater than `i64::MAX` is refused
   (`u64 > i64::MAX`)..." (line ~127) with: on the inbound side, a JSON
   integer in `(i64::MAX, u64::MAX]` binds as `json number` with its exact
   decimal token — uniformly for body, headers, and properties, nested or
   not. Detect it with `type_of(x) == "json number"`, read the exact decimal
   with `x.to_string()`, and take the lossy path only explicitly with
   `x.to_float()`. Every other unrepresentable value keeps its typed
   refusal, the error target still names the exchange slot, and keys inside a map value
   never appear in the error target.
4. In `docs/src/languages/rhai.md`, amend the one-way-conversions paragraph
   "Direct outbound conversion of a wrapper is deferred..." (lines ~264-266):
   a wrapper assigned to a body still fails as an explicit compatibility
   break, EXCEPT a `json number` whose token is a plain decimal integer in
   canonical spelling within `(i64::MAX, u64::MAX]`, which converts to the
   identical `u64`. Keep the `to_json` guidance.
5. In `docs/src/languages/rhai.md`, replace the closing sentences "The
   inbound exchange conversion is unchanged. A JSON integer greater than
   `i64::MAX` on that path is still refused." (lines ~272-273) with: the
   inbound exchange conversion binds a JSON integer in
   `(i64::MAX, u64::MAX]` as `json number` (bd rc-141om, refining the
   no-lossy-float owner decision); it no longer refuses.
6. Create `CHANGELOG.md` at the repo root with a `# Changelog` heading, a
   `## Unreleased` heading, a `### Fixes` heading, and exactly one entry:
   `- fix(languages/rhai): a JSON integer above i64::MAX in a header,
   property, or body no longer fails every later rhai script step — it binds
   as a json number carrying the exact decimal token, and fails only later,
   at the operation, if used in a refused comparison or converted outbound
   as a non-image token (bd rc-141om).`
7. `git -C /home/shared/rust-camel-worktrees/rhaiu64 diff --stat` — only the
   three files above changed beyond Tasks 1.1-2.1.

**Tests:** (executable spec)
- No Rust tests. Verification: `grep -c "rc-141om"
  crates/languages/camel-language-rhai/CONTEXT.md` returns ≥ 1 with the
  resolved (not out-of-scope) wording; `grep -c "u64 > i64::MAX"
  docs/src/languages/rhai.md` returns 0 (the stale line-127 refusal label
  is gone); `grep -c "is unchanged. A JSON integer"
  docs/src/languages/rhai.md` returns 0 (the stale lines-272-273 sentence
  is gone); `test -f CHANGELOG.md` succeeds;
  `grep -c "fix(languages/rhai)" CHANGELOG.md` returns exactly 1.

**Acceptance:**
- `cargo xtask lint-ignore` exits 0 (no new ignored paths).
- All three files contain the exact statements in Steps 1-6; no other docs
  move.
- `cd docs && mdbook test` exits 0 (docs/** changed, so the doc-guide-test
  gate is live).
- `mdbook build docs` from the worktree root exits 0 (docs/AGENTS.md build
  gate).
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-language-rhai --no-deps`
  exits 0 (the rewritten Rust doc comments get intra-doc-link validation;
  use `crate::` paths for `pub(crate)` links).

- [x] 2.2
