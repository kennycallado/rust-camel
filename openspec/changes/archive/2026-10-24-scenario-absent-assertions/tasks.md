# Tasks: scenario-absent-assertions

Single-phase change (no `## Phase N` grouping; design.md has no `## Phases` section). All tasks land on one branch in order 1.1 → 1.4; each ends with the tree committed clean.

## camel-matchers

### Task 1.1: Expectation::Absent variant with fail-closed matching

**Files:**
- `crates/camel-matchers/src/lib.rs` (modified)

**Steps:**
1. Add `Absent` to the `#[non_exhaustive] pub enum Expectation` (after `Any`, before the closing brace), with a doc comment: "The value under validation resolves to nothing (path-scoped absence). Never matches a materialized value; the none-projection is evaluated by the runner, not here."
2. In `pub fn expectation_matches(expectation: &Expectation, value: &serde_json::Value) -> bool` (line ~390), add the match arm `Expectation::Absent => false` with the comment: "Fail-closed: absence is not a property of any materialized value. The scenario runner owns the Option<Value> projection and never calls expectation_matches for a none-projection."
3. Check `Display for Expectation` (if the enum renders anywhere) renders `Absent` as `absent`; the foreign-`#[non_exhaustive]` fallback arms elsewhere must keep compiling (compiler will point at any exhaustive match — fix each by naming the variant, never by widening a wildcard).

**Tests:** (in the existing `#[cfg(test)] mod tests` of `camel-matchers/src/lib.rs`, next to `expectation_matches_string_forms`)
- `absent_never_matches_materialized_values`: arrange `Expectation::Absent`; act `expectation_matches(&Expectation::Absent, v)` for `v` in `Null`, `json!(0)`, `json!("x")`, `json!({})`, `json!([])`; assert every call returns `false`.
- command: `cargo test -p camel-matchers absent_never_matches`
- expected: FAILS before step 1 (variant does not exist → compile error is the red state), PASSES after.

**Acceptance:**
- `cargo test -p camel-matchers` green.
- `cargo clippy -p camel-matchers --all-targets -- -D warnings` exits 0.
- `cargo fmt --check -p camel-matchers` exits 0.

- [x] 1.1

## camel-integration-test (parse + model)

### Task 1.2: absent verb parse, select field, and load-time rejection laws

**Files:**
- `crates/camel-integration-test/src/document/validate.rs` (modified)
- `crates/camel-integration-test/src/document.rs` (modified)
- `crates/camel-integration-test/src/document/redis_target.rs` (modified)
- `crates/camel-integration-test/src/runner.rs` (modified — mechanical compile-only changes owned here: `select: None` at every `ScenarioAction::Validate` construction site, wildcard/`..` destructuring updates the compiler names; no behavior)
- `crates/camel-integration-test/src/runner_test.rs` (modified — mechanical compile-only `select: None` in existing Validate constructions; no behavior)
- `crates/camel-integration-test/src/runner/partner_validate_test.rs` (modified — mechanical compile-only `select: None` in existing Validate constructions; no behavior)
- `crates/camel-integration-test/src/runner/surreal_validate_test.rs` (modified — mechanical compile-only `select: None`; no behavior)
- `crates/camel-integration-test/src/runner/redis_validate.rs` (modified — mechanical compile-only `select: None`; no behavior)
- `crates/camel-integration-test/src/doc_parse_test.rs` (modified)

**Steps:**
1. In `document/validate.rs`: add `"absent"` to `is_matcher_key`. In `expectation_from_value`, add the arm mirroring `exists`: `payload.is_null()` → `Ok(Expectation::Absent)`, else `Err(invalid("{field}: \`absent\` takes no argument"))`.
2. In `document.rs`: add `select: Option<String>` to `ScenarioAction::Validate` (after `elapsed_at_least`) and to `RawValidate` (`select: Option<String>`, camelCase `select`). Update every construction/pattern site the compiler names.
3. In the `"validate"` conversion arm of `document.rs` (after the `elapsed_at_least` block, ~line 1090), implement the rejection laws, each error via the existing `action_error(format!(...))` so it names the action index, mirroring the `elapsedAtLeast` wording style:
   - FIRST, a uniform pre-check before dispatching by target: if `raw.expectation` is an object whose single key is `absent` and the target is anything other than `LastReceived` (variable, partner, sql, surreal, redis) → "`absent` is only valid on a `lastReceived` validate target with `select`". This one check gives every non-`lastReceived` target the same absence-specific diagnostic.
   - `select` present while target is not `LastReceived` → "`select` is only valid on a `lastReceived` validate target, got `{selector}`".
   - `select` present while expectation is not `ValidateExpectation::Message(Expectation::Absent)` → "`select` exists solely for absence assertions; pair it with `expectation: {absent: null}`, got `{selector}`".
   - `absent` expectation (any `ValidateExpectation::Message(Expectation::Absent)`) while target is not `LastReceived` → "`absent` is only valid on a `lastReceived` validate target with `select`".
   - `absent` expectation on `LastReceived` without `select` → "`absent` requires `select` naming the header or body path to prove absent".
   - `select` present → validate the selector: split on the first `.`; head must be exactly `headers` or `body` AND the rest must be non-empty; otherwise → "`select` head must be `headers.<name>` or `body.<path>`; `{selector}` cannot prove absence (bare and scalar heads always resolve)". This one check covers unknown heads, bare `body`/`headers`, and scalar heads (`status`, `method`, `path`, and `status.why`-style sub-paths: any scalar head fails the head match).
4. Contextual rejections for grammar positions that reuse `expectation_from_value`. Redis delegates its cell parsing to `sql_expectation_from_value` (`document/redis_target.rs:236`), so guarding cells inside `sql_expectation_from_value` covers sql, surreal, and redis in one place. Guard sites: the row cells inside `sql_expectation_from_value`, the RequestShape `body` inside `partner_expectation_from_value`, and the `send.expectReply` conversion in `document.rs` — if a produced expectation is `Expectation::Absent`, return `Err` "`absent` is only valid on a `lastReceived` validate target with `select`" (cell/shape sites prefix the field name the site already uses). Give `reject_absent` the visibility its parent-module callers need (`pub(super)` or `pub(crate)` — the helper lives in `document/validate.rs`, `document.rs` and `document/redis_target.rs` are parent/sibling modules).
5. Keep `receive.extract` byte-identical: do not touch `select_from`, the extract arm of `receive_action`, or its error text.

**Tests:** (in `doc_parse_test.rs`, following the `ignore` no-argument-law tests at ~line 2198)
- `absent_null_payload_parses_to_absent`: `expectation_from_value(&json!({"absent": null}), 0, "expectation")` → assert `Ok(Expectation::Absent)`.
- `absent_non_null_payload_is_grammar_error`: `expectation_from_value(&json!({"absent": "x"}), 0, "expectation")` → assert Err, message contains "takes no argument".
- `bare_absent_string_stays_literal_equals`: `expectation_from_value(&json!("absent"), 0, "expectation")` → assert `Ok(Expectation::Equals(json!("absent")))`.
- `select_on_variable_target_is_load_error`: parse a document whose validate declares `target: {variable: v}`, `select: headers.X`, `expectation: {absent: null}` → assert Err naming "only valid on a `lastReceived`".
- `select_with_non_absent_expectation_is_load_error`: validate with `target: {lastReceived: "partner://app"}`, `select: headers.X`, `expectation: {equals: "v"}` → assert Err containing "solely for absence".
- `absent_without_select_is_load_error`: validate `target: {lastReceived: "partner://app"}`, `expectation: {absent: null}`, no select → assert Err containing "requires `select`".
- `absent_on_variable_target_is_load_error`: validate `target: {variable: v}`, `expectation: {absent: null}` → assert Err containing "only valid on a `lastReceived`".
- `absent_on_partner_target_is_load_error`: validate `target: {partner: "partner://app"}`, `expectation: {absent: null}` → assert Err containing the absence-specific restriction.
- `absent_on_sql_target_is_load_error`: validate `target: {sql: {datasource: ds, query: "SELECT 1"}}`, `expectation: {absent: null}` → assert Err containing the absence-specific restriction.
- `absent_on_surreal_target_is_load_error`: validate `target: {surreal: {datasource: ds, query: "SELECT * FROM t"}}`, `expectation: {absent: null}` → assert Err containing the absence-specific restriction.
- `absent_on_redis_target_is_load_error`: validate `target: {redis: {datasource: ds, key: k, type: string}}`, `expectation: {absent: null}` → assert Err containing the absence-specific restriction.
- `absent_in_redis_row_cell_is_load_error`: a redis validate whose row cell is `{absent: null}` → assert Err containing "only valid on a `lastReceived`".
- `select_unknown_head_is_load_error`: `select: header.X-Internal` → assert Err containing "cannot prove absence" and the selector text.
- `select_bare_head_is_load_error`: `select: body` → assert same load error.
- `select_scalar_head_is_load_error`: `select: status` → assert same load error.
- `select_scalar_subpath_is_load_error`: `select: status.why` → assert same load error.
- `absent_in_expect_reply_is_load_error`: a `direct:` send declaring `expectReply: {absent: null}` → assert Err containing "only valid on a `lastReceived`".
- `absent_in_partner_request_body_is_load_error`: a partner validate `requests` entry with `body: {absent: null}` → assert Err containing "only valid on a `lastReceived`".
- `absent_in_row_cell_is_load_error`: a sql validate with `rows: [[{absent: null}]]` and `columns: [c]` → assert Err containing "only valid on a `lastReceived`".
- `select_with_valid_grammar_parses`: validate `target: {lastReceived: "partner://app"}`, `select: headers.X-Internal-Token`, `expectation: {absent: null}` → assert the parsed `ScenarioAction::Validate` carries `select: Some("headers.X-Internal-Token")` and `ValidateExpectation::Message(Expectation::Absent)`.
- commands: `cargo test -p camel-integration-test --lib doc_parse_test`
- expected, relative to the task's starting tree: the parse-law tests (`absent_null_payload_parses_to_absent`, `absent_non_null_payload_is_grammar_error`) PASS after step 1; `bare_absent_string_stays_literal_equals` PASSES both before and after (behavior unchanged); the target/grammar rejection tests fail BEHAVIORALLY until steps 2-3 add the laws (the loader accepts or rejects with the wrong message); the contextual rejection tests (`absent_in_expect_reply_is_load_error`, `absent_in_partner_request_body_is_load_error`, `absent_in_row_cell_is_load_error`, `absent_in_redis_row_cell_is_load_error`) fail BEHAVIORALLY until step 4 adds the guards; `select_with_valid_grammar_parses` fails to compile until step 2 adds the `select` field, then passes.
- assertion strength: every rejection test additionally asserts the error is `DocError::Validation` carrying the action index the fixture placed the validate at, and — for the selector tests — that the message contains the exact selector text (`header.X-Internal`, `body`, `status`, `status.why`).

**Acceptance:**
- `cargo test -p camel-integration-test --lib doc_parse_test` green.
- `cargo clippy -p camel-integration-test --all-targets -- -D warnings` exits 0.
- `cargo fmt --check -p camel-integration-test` exits 0.
- Existing extract tests unmodified and green (`cargo test -p camel-integration-test --lib extract` and the pre-existing `resolved to nothing` assertions).

- [x] 1.2

## camel-integration-test (runner)

### Task 1.3: runner projection, absent evaluation, kind-only diagnostic

**Files:**
- `crates/camel-integration-test/src/runner.rs` (modified)
- `crates/camel-integration-test/src/runner_test.rs` (modified)

**Steps:**
1. In `validate_action`'s `(_, ValidateExpectation::Message(expectation))` arm: when the target is `LastReceived` and `select` is `Some(selector)`, resolve the message exactly as today (`vars.last_received`, existing no-message error), then project `let projected = select_from(&message, selector);`.
2. Evaluate the pairing explicitly (do not route the none-projection through `expectation_matches`):
   - `(Expectation::Absent, None)` → pass.
   - `(Expectation::Absent, Some(value))` → a `ScenarioFailure::ValidationMismatch` whose detail names the subject, the selector, and the observed KIND only: derive `kind` with a small helper on the value returning one of `null`, `string`, `number`, `boolean`, `object`, `array`, and render as `expected `headers.X-Internal-Token` to be absent, but a value is present (string)`. The detail MUST NOT render the value's content.
   - `(other, None)` → mismatch naming the selector and "resolved to nothing".
   - `(other, Some(value))` → existing path via `expectation_matches`.
   The parse-time laws (task 1.2) make the last two defensive: unreachable through the loader; a defensive `ScenarioFailure` naming the law is correct, not a panic.
3. When `select` is `None` and expectation is `Expectation::Absent` (defensive: loader rejects it), fail with detail "`absent` requires `select`" — same non-panic stance.
4. Extend `render_expectation_mismatch` with `Expectation::Absent => "expected the selector to resolve to nothing, but a value is present (kind only, content withheld)"` — the string constant used by the defensive path; the live message from step 3 names the selector.
5. `subjects`: reuse the existing subject rendering for `lastReceived` ("body last received on {redacted}" style) — for a selected projection the subject names the endpoint and the selector.

**Tests:** (in `runner_test.rs`, using the existing `FakeAdapter::scripted` + `router_for` + `run_scenario_document` pattern under `#[tokio::test]`; build `IncomingMessage`s with `headers: BTreeMap`, `body`, and the route already exercised by neighboring tests)
- `absent_header_passes`: scripted response WITHOUT header `X-Internal-Token`; validate `lastReceived` + `select: headers.X-Internal-Token` + `{absent: null}` → assert outcome verdict Pass.
- `present_header_fails_absent_without_disclosing_value`: scripted response WITH header `X-Internal-Token: s3cr3t`; same validate → assert verdict Fail, failure detail contains "X-Internal-Token" and "present (string)" and DOES NOT contain "s3cr3t" (sentinel).
- `null_valued_header_fails_absent`: scripted response with header `X-Flag` mapped to `Value::Null`; same validate → assert Fail with detail containing "present (null)"; and a second document extracting `headers.X-Flag` into a var + `variable` target `equals: null` → assert Pass (the documented equals-null path).
- `null_body_leaf_fails_absent`: scripted body `{"receiptId": null}`; validate `select: body.receiptId` + absent → assert Fail with detail containing "present (null)" (a verdict-only assertion would pass before implementation for the wrong reason; the content assertion pins present-null is not absent).
- `absent_body_path_passes`: scripted body `Value::Null`; validate `select: body.receiptId` + absent → assert Pass.
- `absent_on_endpoint_without_message_fails`: validate absent on an endpoint nothing received → assert Fail with the existing "no message has been received" detail.
- `extract_absent_header_still_hard_fails`: receive extracting `headers.X-Internal-Token` when absent → assert Fail with detail containing "resolved to nothing", the selector `headers.X-Internal-Token`, and the variable name (regression guard: extract unchanged, byte-identical detail).
- commands: `cargo test -p camel-integration-test --lib runner_test`
- expected, relative to the task's starting tree (task 1.2 landed the `select` field, so everything compiles): the absent-assertion tests (`absent_header_passes`, `absent_body_path_passes`) FAIL BEHAVIORALLY before steps 1-4 — the runner has no select projection yet, so `Absent` is evaluated against the body and the expectation fails; `absent_on_endpoint_without_message_fails` PASSES before and after (the no-message error predates this change — it is unchanged-behavior regression coverage); the failure-assertion tests (`present_header_fails_absent_without_disclosing_value`, `null_valued_header_fails_absent`, `null_body_leaf_fails_absent`) may pass BEFORE the task (a Fail verdict happens today for the wrong reason) — their content assertions (`present (string)`, `present (null)`, no `s3cr3t`) are what pin the fix, so verify the exact-detail assertions fail before and pass after; `extract_absent_header_still_hard_fails` passes BOTH before and after (it pins unchanged behavior — a pass before this task is the point).

**Acceptance:**
- `cargo test -p camel-integration-test` green (whole crate, all features default).
- `cargo clippy -p camel-integration-test --all-targets -- -D warnings` exits 0.
- `cargo fmt --check -p camel-integration-test` exits 0.
- `cargo xtask schema --check` exits 0 (if the schema covers the validate grammar and drifts, regenerate with `cargo xtask schema` in the same commit).

- [x] 1.3

## docs

### Task 1.4: grammar reference + scenario guide with worked example and migration note

**Files:**
- `crates/camel-integration-test/README.md` (modified)
- `docs/src/testing/scenario-documents.md` (modified)
- `crates/camel-integration-test/src/doc_parse_test.rs` (modified — the `guide_worked_example_parses` test)

**Steps:**
1. README (the crate's grammar reference): in the validate/expectation grammar section, document the `absent` verb (`{absent: null}`, no argument, mirroring `exists`), the `select` field on `lastReceived` validates (grammar `headers.<name>` ASCII-case-insensitive, `body.<path>`; head must be `headers` or `body` with a non-empty sub-path), the full load-rejection list, and the case table from design.md (all seven rows, including present-null and misspelled-name limits).
2. `docs/src/testing/scenario-documents.md`: add a section "Asserting absence" with a worked example proving an internal header does not leak: a route that responds, one validate with `select: headers.X-Internal-Token` + `{absent: null}`, plus the positive-control recommendation (also assert a header known present). State the case table, the path-scoped wording (absence proves the selected path resolves to nothing, not that the whole part is absent), the diagnostic rule (failure names the selector and the observed kind, never the content), and the migration note: `receive.extract` behavior is unchanged — what migrates is the headers-object + regex-allowlist workaround, which simplifies to one `select` + `absent` assertion.
3. Keep prose in Simplified Technical English per the ste-writing skill; both docs are English.

**Tests:**
- `mdbook` guide build: `cd docs && mdbook test` — this validates the guide page builds and any Rust doctests in docs; it does NOT validate Markdown links or YAML examples, so the executable coverage for the guide's YAML is the parse test below.
- `guide_worked_example_parses` (in `doc_parse_test.rs`): parse the guide's "Asserting absence" YAML example VERBATIM (copy the exact document into the test as a string literal; when the guide example changes, this test changes with it) → assert the document loads Ok, the validate carries `select: Some("headers.X-Internal-Token")` and `ValidateExpectation::Message(Expectation::Absent)`.
- command: `cd docs && mdbook test && cargo test -p camel-integration-test --lib guide_worked_example_parses`
- expected: green after (docs-only change plus one parse test); failure only on broken markup or a guide example that violates the grammar the tests pin.

**Acceptance:**
- `cd docs && mdbook test` exits 0 and `guide_worked_example_parses` passes.
- `cargo xtask lint-context-citations` exits 0.
- README and guide contain the case table and the migration note verbatim topics (absent header, present-null, misspelled selector, absent body).

- [x] 1.4
