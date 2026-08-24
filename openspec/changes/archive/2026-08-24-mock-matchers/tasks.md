# Tasks: mock-matchers

## camel-component-mock

### Task 1.1: BodyMatcher and HeaderMatcher vocabulary

**Files:**
- `crates/components/camel-mock/src/matcher.rs` (new)
- `crates/components/camel-mock/src/lib.rs` (modified — add `pub mod matcher;` and re-export `BodyMatcher`, `HeaderMatcher`)

**Steps:**
1. Define `#[derive(Clone, Debug)] #[non_exhaustive] pub enum BodyMatcher` with variants `Equals(Body)`, `Regex(String)`, `Contains(String)`, `StartsWith(String)`, `EndsWith(String)`, `Exists`, `JsonSubset(serde_json::Value)` (`Clone`+`Debug` are required because `ExpectSet` holds and derives over these types in camel-cli; `#[non_exhaustive]` is required by the `lint-non-exhaustive` gate; `Body` is itself `#[non_exhaustive]`, camel-api/src/body.rs:162, so matching over it needs `_` arms).
2. Define `#[derive(Clone, Debug)] #[non_exhaustive] pub enum HeaderMatcher` with variants `Equals(serde_json::Value)`, `Regex(String)`, `Exists`.
3. Implement `impl BodyMatcher { pub fn matches(&self, actual: &Body) -> bool }` with the pinned semantics: string matchers (`Regex`, `Contains`, `StartsWith`, `EndsWith`) operate only on `Body::Text` (extract the `&str`, apply the operation) and fail on every other variant; `Equals` uses the existing variant-tagged body equality already used by ordered-body assertions (`assert.rs` `body_eq` — reuse it via `pub(crate)` visibility or move it into `matcher.rs` and keep `assert.rs` calling it); `Exists` passes for every variant except `Body::Empty`; `JsonSubset` requires its pattern value to be a JSON object (non-object patterns fail the match), accepts `Body::Json` directly or `Body::Text` parsed with `serde_json::from_str` (parse failure fails the match), requires the received top-level JSON to be an object, then matches recursively: every pattern key must exist; object values match by JSON equality or recursive subset; arrays compare exactly (length, order, elements JSON-equal); `null` pattern values require JSON `null`.
4. Implement `impl HeaderMatcher { pub fn matches(&self, actual: Option<&serde_json::Value>) -> bool }` — `Exists` passes iff the option is `Some` (any value, including null); `Equals(v)` passes iff `Some(actual)` and `actual == v`; `Regex(p)` passes iff `Some(Value::String(s))` and the compiled pattern matches (non-string or absent fails — strict-string, unlike the legacy `expect_header_regex` coercion).
5. Implement `std::fmt::Display` for both enums rendering the matcher kind and payload exactly as diagnostics will print them: `equals <json>`, `regex <pattern>`, `contains <needle>`, `startsWith <prefix>`, `endsWith <suffix>`, `exists`, `jsonSubset <json>`.
6. Add accessors: `impl BodyMatcher { pub fn regex_pattern(&self) -> Option<&str> }` and `impl HeaderMatcher { pub fn regex_pattern(&self) -> Option<&str> }` (evaluation uses them to distinguish malformed patterns from mismatches), and `impl BodyMatcher { pub fn mismatch_note(&self, actual: &Body) -> Option<&'static str> }` plus `impl HeaderMatcher { pub fn mismatch_note(&self, actual: Option<&serde_json::Value>) -> Option<&'static str> }` returning the spec-mandated reason strings: body — `Some("body is not text")` for string matchers against a non-`Text` variant, `Some("body is not JSON")` for `JsonSubset` against unparseable `Text` or a non-object pattern, `Some("body is not a JSON object")` for `JsonSubset` against a non-object top level, `None` otherwise; header — `Some("value is not a string")` for `Regex` against a present non-string value or absent value, `None` otherwise (evaluation appends the note to the failure message).
7. Write unit tests in `matcher.rs` (same file, `#[cfg(test)] mod tests`).

**Tests:** (all in `matcher.rs::tests`, command `cargo test -p camel-component-mock --lib matcher`)
- `regex_body_pass_and_fail`: `BodyMatcher::Regex("^order-[0-9]+$").matches(&Body::Text("order-42"))` is true; `.matches(&Body::Text("refunded-42"))` is false.
- `substring_and_anchor_matchers`: `Contains("total")`, `StartsWith("order-")`, `EndsWith("-42")` all match `Body::Text("order-total-42")`.
- `exists_body_variants`: `Exists.matches(&Body::Text("x"))` true; `Exists.matches(&Body::Empty)` false.
- `string_matchers_fail_non_text`: `Contains("a").matches(&Body::Json(serde_json::json!({"a":1})))` and `.matches(&Body::Bytes(vec![97u8].into()))` are both false; `mismatch_note` for both returns `Some("body is not text")`.
- `json_subset_recursive_ignores_extra`: `JsonSubset({"status":"ok","meta":{"seq":3}})` matches body `{"id":7,"status":"ok","meta":{"seq":3,"ts":9}}`.
- `json_subset_arrays_exact`: `JsonSubset({"tags":["a","b"]})` does NOT match `{"tags":["b","a"]}` and does match `{"tags":["a","b"]}`.
- `json_subset_parses_text`: `JsonSubset({"status":"ok"})` matches `Body::Text("{\"status\": \"ok\"}")`; does not match `Body::Text("ok")` with `mismatch_note` = `Some("body is not JSON")`.
- `json_subset_null_requires_null`: `JsonSubset({"err":null})` matches `{"err":null}`, not `{"err":0}`.
- `header_null_and_missing`: with `X-A: null` present and `X-B` absent — `Exists` on `Some(&null)` true, `Exists` on `None` false, `Equals(null)` on `Some(&null)` true, `Regex` on `Some(&null)` false with `mismatch_note` = `Some("value is not a string")`.

**Acceptance:**
- `cargo test -p camel-component-mock --lib matcher` passes.
- `cargo clippy -p camel-component-mock -- -D warnings` exits 0.
- `cargo xtask lint-non-exhaustive` exits 0.
- No new `unwrap()` (lint-unwrap clean).

- [x] 1.1

### Task 1.2: Unified body-expectation list, matcher setters, evaluation, and diagnostics

**Files:**
- `crates/components/camel-mock/src/expectations.rs` (modified)
- `crates/components/camel-mock/src/inner.rs` (modified)
- `crates/components/camel-mock/src/assert.rs` (modified)

**Steps:**
1. In `expectations.rs`, REPLACE the separate exact/matcher storage with ONE ordered expectation list: define `pub(crate) enum BodyExpectation { Exact(Body), Matcher(BodyMatcher) }` and re-type the existing `expected_bodies` field to `Vec<BodyExpectation>` (existing field is `pub(crate)` at expectations.rs:10-12; update every construction/read site — they live in this crate). `expect_body` (inner.rs:198) pushes `BodyExpectation::Exact`; add `pub fn expect_body_matcher(&self, matcher: BodyMatcher)` pushing `BodyExpectation::Matcher` — insertion order across both setters is preserved by construction, so mixed sequences (exact, matcher, exact) keep their slots. Add `pub(crate) expected_header_matchers: Vec<(String, HeaderMatcher)>` beside `expected_header_regexes` (default empty).
2. In `assert.rs` add error variants to `MockAssertionError` (struct-variant style with `endpoint: String` first): `InvalidBodyPattern { endpoint: String, pattern: String, source: Box<dyn std::error::Error + Send + Sync> }` — exact field-shape parity with `InvalidHeaderPattern` (assert.rs:181-191: String fields + boxed source), and implement `Error::source()` for it mirroring how `InvalidHeaderPattern` provides its source; `BodyMatcherFailed { endpoint: String, index: usize, matcher: String, received: String }` and `HeaderMatcherFailed { endpoint: String, key: String, matcher: String, received: String }`; write their `Display` arms naming the endpoint, index/key, the matcher's Display form, and the received value rendered whole (no truncation); when the matcher's `mismatch_note` returns `Some(note)`, append ` (<note>)`.
3. Rework the ordered-body evaluation over the unified list: slot `i` compares via `BodyExpectation` — `Exact` keeps the existing `BodyMismatch` path unchanged; `Matcher` uses `BodyMatcher::matches` and on mismatch produces `BodyMatcherFailed { index: i, matcher: matcher.to_string(), received: <whole received body rendering> }` (for `JsonSubset` failures the rendered `matcher` string contains the pattern JSON, so the failing key name appears). The `BodyCountMismatch` count guard (assert.rs:338-345) now compares the unified list length against the received count — fewer received bodies than expectations fails with `BodyCountMismatch`, never an index panic. Before matching, if any `Regex` body matcher in the list holds an invalid pattern (`BodyMatcher::regex_pattern()` + `regex::Regex::new`), return `InvalidBodyPattern` — never a pass, never latched into `fail_fast_error`.
4. Extend the any-order body surface over the unified list with the same rules it applies today: each expectation (Exact or Matcher) must be matched by some received body; matcher expectations use `BodyMatcher::matches`. Keep its existing error shape.
5. Extend header evaluation (assert.rs:382-434 area) with ANY-EXCHANGE semantics identical to the existing exact-header and header-regex checks (`.any(|ex| …)` over received exchanges, matching assert.rs:383-385 and 414-422): `expected_header_matchers` entries evaluate after the exact headers — `Exists` fails naming the absent key; `Equals` fails naming key + expected + received values (reuse the diagnostic clause shape of `HeaderNotFound`); `Regex` fails naming key + pattern + received values (same shape as `HeaderRegexNotMatched`), appending the `mismatch_note` when present. A header-matcher failure latches and returns for that endpoint exactly as `HeaderNotFound` does. Before evaluation, a `HeaderMatcher::Regex` holding an invalid pattern returns the EXISTING `InvalidHeaderPattern` variant (never a pass, never latched) — direct-API users get the same malformed-pattern class the legacy engine provides.
6. Leave the legacy `expect_header_regex` engine exactly as-is for direct-API users (its non-string coercion via `to_string()` stays); add a one-line code comment noting `HeaderMatcher::Regex` is strict-string and the divergence is intentional.
7. Write tests in `tests.rs` beside the existing header-regex tests (5 exist: tests.rs:1095, 1116, 1851, 2340, 2533).

**Tests:** (command `cargo test -p camel-component-mock`)
- `header_matcher_setter_pass`: `expect_header_matcher("X-Trace", HeaderMatcher::Regex("^[a-f0-9]{8}$"))` + received header `ab12cd34` → `try_assert_satisfied` passes.
- `header_matcher_setter_fail_names_values`: same with received `xyz` → error text contains `X-Trace`, `regex`, and `xyz`.
- `header_matcher_any_exchange`: endpoint receives two exchanges, only the second carrying `X-A: ok`; `expect_header_matcher("X-A", Equals("ok"))` passes.
- `header_matcher_invalid_regex_direct_api`: `expect_header_matcher("X", HeaderMatcher::Regex("(unclosed"))` + any received header → returns `InvalidHeaderPattern`; `fail_fast_error` is NOT set.
- `ordered_mixed_exact_and_matcher_slots`: `expect_body(Body::Text("x"))` then `expect_body_matcher(Regex("^b-"))` with received `x`,`b-2` passes; received `x`,`a-1` fails naming index 1; insertion order exact-then-matcher is enforced (received `b-2`,`x` fails).
- `matcher_count_mismatch_fails_not_panics`: two `expect_body_matcher` calls, only ONE received body → `BodyCountMismatch` (list length 2 vs 1), no panic.
- `matcher_any_order_passes`: the any-order surface (the existing any-order expectation path) with two `expect_body_matcher` entries (`Regex("^a-")`, `Regex("^b-")`) and received bodies in either order passes.
- `body_matcher_failure_text_identifies`: ordered `Regex("^ok$")` at index 1 receiving `denied` → error text contains `1`, `regex`, `^ok$`, `denied`.
- `string_matcher_failure_states_not_text`: `Contains("a")` at index 0 against `Body::Json` → error text contains `body is not text`.
- `json_subset_failure_states_not_json`: `JsonSubset({"a":1})` against `Body::Text("nope")` → error text contains `body is not JSON`.
- `json_subset_failure_names_key_via_pattern`: `JsonSubset({"err": null})` against `{"err": 0}` → error text contains `err` (from the rendered pattern) and the received body.
- `invalid_body_regex_is_error_not_pass`: `expect_body_matcher(BodyMatcher::Regex("(unclosed"))` + any received body → returns `InvalidBodyPattern` naming the pattern in its source; `fail_fast_error` is NOT set.
- `exists_header_absent_key`: `expect_header_matcher("X-B", HeaderMatcher::Exists)` with no `X-B` received → failure names `X-B` absent.
- `exists_body_failure_names_matcher`: `expect_body_matcher(BodyMatcher::Exists)` against `Body::Empty` → failure text contains `exists` (spec's exists-matcher diagnostic).
- `json_subset_array_failure_names_matcher_and_array`: `expect_body_matcher(JsonSubset({"tags":["a","b"]}))` against body `{"tags":["b","a"]}` → failure text contains `jsonSubset` and the received array `["b","a"]`.

**Acceptance:**
- `cargo test -p camel-component-mock` passes (existing 5 header-regex tests and all exact-body tests stay green).
- `cargo clippy -p camel-component-mock -- -D warnings` exits 0.

- [x] 1.2

## camel-cli

### Task 2.1: Matcher grammar parsing and runner wiring

This task lands the CLI parsing and the runner wiring TOGETHER so the crate compiles and its acceptance gates can run at the task boundary (widening `ExpectSet` alone would leave `runner.rs` uncompilable).

**Files:**
- `crates/camel-cli/Cargo.toml` (modified — add `regex` as a workspace dependency for parse-time pattern validation; the workspace already versions `regex`)
- `crates/camel-cli/src/commands/test/document.rs` (modified)
- `crates/camel-cli/src/commands/test/document_tests.rs` (modified)
- `crates/camel-cli/src/commands/test/runner.rs` (modified)
- `crates/camel-cli/tests/test_runner.rs` (modified — endpoint-expectation integration tests)
- `crates/camel-cli/tests/test_replies.rs` (modified — expectReply integration tests, subprocess style `std::process::Command`, extend its scenario→test mapping header at lines 10-54)

**Steps:**
1. In `document.rs`, change `ExpectSet.bodies` from `Option<Vec<String>>` to `Option<Vec<camel_component_mock::BodyMatcher>>` and `ExpectSet.headers` from `Option<HashMap<String, serde_json::Value>>` to `Option<HashMap<String, camel_component_mock::HeaderMatcher>>`; change `ExpectReply.body` (struct at document.rs:131) from its current input-body type to `camel_component_mock::BodyMatcher` and `ExpectReply.headers` values to `HeaderMatcher`. Deserialize via custom field mappers (follow the existing custom-deserialization pattern used for `InputBody` at document.rs:180) so `deny_unknown_fields` behavior is preserved.
2. Add `fn parse_body_entry(value: &serde_json::Value, field: &str) -> Result<BodyMatcher, TestDocError>` implementing the STRICT grammar: a string maps to `BodyMatcher::Equals(Body::Text(s))`; a map with exactly one recognized body-matcher key maps to its matcher — `equals` maps its value to a body via the same value-to-Body mapping `InputBody` uses (string value → `Body::Text`, object/array/number/boolean/null value → `Body::Json(value)`); `regex`/`contains`/`startsWith`/`endsWith` require a string payload (non-string payload is an error naming the field); `exists` requires a `null` value (any non-null payload is an error stating `exists` takes no argument); `jsonSubset` requires a JSON object payload (else error stating it must be an object). Every `regex` payload (this parser and the two below) is compile-verified with `regex::Regex::new`, returning the crate's error text in the document error (exit-2 class). A sole `predicate` key → error `predicate matchers are not supported`; any other scalar, a bare array, or a map with zero/multiple/unrecognized keys → error stating body entries must be strings or matcher maps, naming the key when present.
3. Add `fn parse_header_value(value: &serde_json::Value, field: &str) -> Result<HeaderMatcher, TestDocError>` implementing the DUAL grammar: a sole recognized header-matcher key maps to its matcher — `equals` wraps its value as `HeaderMatcher::Equals(value)`; `regex` requires a string payload (compile-verified); `exists` requires `null` (non-null → error stating `exists` takes no argument). A sole `jsonSubset` key → error stating `jsonSubset` applies to bodies only; a sole `predicate` key → reserved-key error; ANY other value (scalar, array, multi-key object, object whose keys are not exactly one recognized matcher key) maps to `HeaderMatcher::Equals(value.clone())` unchanged.
4. Add `fn parse_reply_body(value: &serde_json::Value, field: &str) -> Result<BodyMatcher, TestDocError>` implementing the DUAL body grammar: every bare scalar (string, number, boolean, null) and every array maps to `Equals(<value as body>)` (string → `Body::Text`, others → `Body::Json(value)`); an object with exactly one recognized body-matcher key maps to that matcher (same payload rules as step 2); a sole `predicate` key → reserved-key error; any other object maps to `Equals(Body::Json(value))` (structural equality, reply-capture behavior preserved).
5. Wire the three mappers into `TestDocument` deserialization for `expects.bodies` list entries (strict), `expects.headers` values (dual), `expectReply.body` (dual), `expectReply.headers` values (dual). All errors flow through `TestDocError` as a new variant `InvalidMatcher(String)` rendered like `InvalidBeans`/`InvalidReply` (document.rs:246-249, 319-320 style) — parse failures remain exit 2 via the existing `parse_test_document` path (document.rs:350).
6. In `runner.rs`, rewire `set_expectations` (runner.rs:252): `bodies` entries call `inner.expect_body_matcher(matcher)` (replacing the `Body::Text` construction at runner.rs:261); `headers` entries call `inner.expect_header_matcher(key, matcher)` (replacing `expect_header`; exact values arrive as `HeaderMatcher::Equals` from parsing; `HeaderMatcher::Regex` reaches the engine through the new setter).
7. Delete `reply_body_eq` (runner.rs:299) and rewrite the body check inside `evaluate_reply_expectation` (runner.rs:313) to call the parsed `BodyMatcher::matches(&message.body)`; rewrite the header check to call `HeaderMatcher::matches(actual)` per key (absent key → `None`), preserving the existing `<missing>` reporting shape for absent keys on failure. The failure message for a matcher mismatch includes the matcher Display form and the received body/headers. Exit taxonomy untouched: reply matcher mismatch stays an assertion failure counted into the summary with exit 1 (existing FAIL-line path).
8. Add the parsing tests to `document_tests.rs` and the integration tests to `tests/test_runner.rs` (endpoint scenarios, follow its fixture style) and `tests/test_replies.rs` (subprocess style; add each new scenario to its mapping header).

**Tests:**
Parsing (command `cargo test -p camel-cli --lib document_tests`):
- `bare_string_body_stays_exact`: doc with `expects: {mock:result: {count: 1, bodies: ["plain"]}}` parses to `Equals(Body::Text("plain"))`.
- `matcher_map_body_accepted`: `bodies: [{regex: "^order-[0-9]+$"}]` parses to `Regex`.
- `matcher_map_header_accepted`: `headers: {X-Trace: {regex: "^[a-f0-9]{8}$"}}` parses to `HeaderMatcher::Regex`.
- `header_literal_object_stays_equals`: `headers: {mode: {batch: 1, predicate: "raw"}}` (multi-key object) parses to `Equals` of that object.
- `unknown_matcher_key_rejected`: `bodies: [{xpath: "//id"}]` → `InvalidMatcher` naming `bodies` and `xpath`.
- `reserved_predicate_key_rejected_bodies`: `bodies: [{predicate: "x"}]` → `InvalidMatcher` with the not-supported message.
- `reserved_predicate_key_rejected_header`: `headers: {X: {predicate: "x"}}` → `InvalidMatcher` with the not-supported message.
- `reserved_predicate_key_rejected_reply_body`: `expectReply: {body: {predicate: "x"}}` → `InvalidMatcher` with the not-supported message.
- `reserved_predicate_key_rejected_reply_header`: `expectReply: {headers: {X: {predicate: "x"}}}` → `InvalidMatcher` with the not-supported message.
- `matcher_map_wrong_key_count_rejected`: `bodies: [{}]` and `bodies: [{regex: "a", contains: "b"}]` → `InvalidMatcher` stating exactly one key.
- `bare_scalar_bodies_rejected`: `bodies: [7]` → `InvalidMatcher` stating body entries must be strings or matcher maps.
- `bare_array_bodies_rejected`: `bodies: [[1,2]]` → `InvalidMatcher` stating body entries must be strings or matcher maps.
- `exists_non_null_payload_rejected_bodies`: `bodies: [{exists: "x"}]` → `InvalidMatcher` stating `exists` takes no argument.
- `exists_non_null_payload_rejected_header`: `headers: {X: {exists: "y"}}` → `InvalidMatcher` stating `exists` takes no argument.
- `invalid_regex_rejected_at_parse_bodies`: `bodies: [{regex: "(unclosed"}]` → `InvalidMatcher` naming `bodies` and containing the regex error.
- `invalid_regex_rejected_at_parse_header`: `headers: {X: {regex: "(unclosed"}}` → `InvalidMatcher` naming the field and containing the regex error.
- `invalid_regex_rejected_at_parse_reply_body`: `expectReply: {body: {regex: "(unclosed"}}` → `InvalidMatcher` containing the regex error.
- `json_subset_non_object_rejected`: `bodies: [{jsonSubset: [1,2]}]` → `InvalidMatcher` stating it must be an object.
- `json_subset_on_header_rejected`: `headers: {X: {jsonSubset: {a: 1}}}` and `expectReply: {headers: {X: {jsonSubset: {a: 1}}}}` → `InvalidMatcher` stating bodies only.
- `equals_wrapped_scalar_maps_to_json`: `bodies: [{equals: 7}]` parses to `Equals(Body::Json(json!(7)))` (matcher-wrapped scalars are Json bodies).
- `reply_body_object_without_matcher_keys_literal`: `expectReply: {body: {status: "ok"}}` parses to `Equals(Body::Json({"status":"ok"}))`.
- `reply_body_scalar_literals`: `expectReply: {body: 7}` parses to `Equals` of the number body; `expectReply: {body: null}` parses to `Equals` of a null body (newly accepted under the dual grammar — the old `InputBody` path rejected null scalars for inputs; reply-body null is now a valid literal).
- `reply_body_matcher_map`: `expectReply: {body: {regex: "^order-"}}` parses to `Regex`.
- `reply_body_multi_key_predicate_literal`: `expectReply: {body: {predicate: "x", mode: "y"}}` (multi-key object) parses to `Equals(Body::Json(serde_json::json!({"predicate":"x","mode":"y"})))` — dual position keeps literals.
- `backcompat_all_bare_documents_parse`: an existing-style document (bare strings everywhere, JSON header values, JSON reply body) parses to all-`Equals` matchers — regression guard.

Endpoint matchers (command `cargo test -p camel-cli --test test_runner`):
- `endpoint_regex_body_matcher_passes`: route emitting `order-42` + `expects: {mock:result: {count: 1, bodies: [{regex: "^order-[0-9]+$"}]}}` → PASS, exit 0.
- `endpoint_contains_mismatch_names_matcher`: route emitting `total: 12` + `bodies: [{contains: "total: 13"}]` → FAIL text contains `contains`, `total: 13`, `total: 12`; exit 1.
- `endpoint_header_regex_evaluated`: route emitting `X-Trace: ab12cd34` + `headers: {X-Trace: {regex: "^[a-f0-9]{8}$"}}` → PASS.
- `endpoint_json_subset_partial_match`: route emitting `{"id":7,"status":"ok","meta":{"ts":"t1","seq":3}}` + `bodies: [{jsonSubset: {status: "ok", meta: {seq: 3}}}]` → PASS.
- `endpoint_bare_bodies_backcompat_execution`: existing-style document with bare-string bodies end-to-end → PASS, exit 0 (spec's "bare string body stays exact match" execution half).

Reply matchers (command `cargo test -p camel-cli --test test_replies`):
- `reply_regex_matcher_passes`: route setting body `order-42`, input `expectReply: {body: {regex: "^order-[0-9]+$"}}` → reply PASS.
- `reply_json_subset_passes`: body `{"status":"ok","ts":1234}` + `expectReply: {body: {jsonSubset: {status: "ok"}}}` → PASS.
- `reply_mismatch_names_matcher_exit_1`: body `done` + `expectReply: {body: {contains: "unfinished"}}` → FAIL names `contains` and `done`; exit 1.
- `reply_header_regex_passes`: route setting header `X-Trace` to 8 hex chars + `expectReply: {headers: {X-Trace: {regex: "^[a-f0-9]{8}$"}}}` → PASS.
- `literal_json_reply_body_backcompat`: existing-style `expectReply: {body: {"status": "ok"}}` against JSON body → PASS (regression).

**Acceptance:**
- `cargo test -p camel-cli --lib document_tests`, `cargo test -p camel-cli --test test_runner`, and `cargo test -p camel-cli --test test_replies` pass.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- `rg 'reply_body_eq' crates/` returns zero hits.

- [x] 2.1

### Task 2.2: Docs for the matcher grammar and new mock surface

**Files:**
- `docs/src/testing/index.md` (modified — section around lines 146-156)
- `docs/src/components/mock.md` (modified — section around line 87)

**Steps:**
1. In `docs/src/testing/index.md`, extend the expectations documentation: matcher grammar for `expects.bodies` (strict: string or single-key map), dual grammar for headers and `expectReply.body`/`.headers` (literal values stay exact; sole-key maps select matchers), the v1 matcher set per position, the sole-`predicate` reserved-key error, and the wrap-migration note (`{equals: {equals: "x"}}`) from the proposal's compatibility note. Keep the user's link style (`…in the repository — outside the rendered book`).
2. In `docs/src/components/mock.md`, document the new public surface: `BodyMatcher`/`HeaderMatcher` enums, `expect_body_matcher`/`expect_header_matcher` setters, matcher-aware failure diagnostics, and the intentional divergence note (legacy `expect_header_regex` coerces non-strings; `HeaderMatcher::Regex` is strict-string).

**Tests:** (documentation task — verify by inspection and build)
- `docs_build`: after edits, `mdbook build docs` exits 0 and both edited pages render; `cargo doc -p camel-component-mock --no-deps` exits 0 with the `matcher` module listing both enums.

**Acceptance:**
- `mdbook build docs` exits 0.
- Both pages render the grammar examples exactly as the spec scenarios state them (spot-check: regex body example, jsonSubset example, dual-grammar header literal example).

- [x] 2.2
