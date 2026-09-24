# Tasks: splitbody

Single-phase change. Tests land first (Tasks 1–2, red where the bug bites),
the production fix second (Task 3, turns red green), pin/docs last. All e2e
guards reuse the existing harness patterns in their test files
(`CamelTestContext`, `ensure_*_registered`, `send_to_direct`, mock assertion
helpers — copy the file's own idioms; every wait MUST be bounded via
`acquire_deadline`/mock helpers, never an unbounded or raw-sleep wait).

## camel-test

### Task 1.1: xpath split e2e guards — repro, count parity, aggregate indifference

**Files:**
- `crates/camel-test/tests/xpath_test.rs` (modified)

**Steps:**
1. Read the file's existing harness (context builder, `ensure_xpath_registered`,
   `send_to_direct`, mock endpoint access) and reuse it — EXCEPT the settle
   idiom: do NOT copy the `tokio::time::sleep(...)` settle lines (the
   lint-test-sleep ratchet forbids new sleeps). Settle via the bounded
   `MockEndpoint::await_exchanges(count, timeout)` primitive instead; every
   wait must carry an explicit timeout.
2. Add `xpath_split_three_matches_fragments_are_raw_text` (guard a): YAML route
   `direct:start` → `split: {expression: {xpath: "//item/link"}, steps: [{to:
   "mock:fragments"}]}`; send `Body::Xml` with 3 `<item><link>` entries
   (`https://example.org/1.xml`, `/2.xml`, `/3.xml`).
3. Add `xpath_split_match_count_parity` (guard b): same route shape twice —
   once with XML containing 1 link, once with 3 links sharing the same first
   value `https://example.org/bar.xml`.
4. Add `xpath_split_collect_all_aggregate_byte_identical` (guard c): same
   split with `aggregation: collect_all` then `to: "mock:agg"`; read the
   aggregated body after the scope closes.
5. Run the three tests; confirm the two fragment-typing tests FAIL pre-fix
   (fragment bodies are `Body::Json`, `${body}`-rendered strings carry literal
   `"` characters) — that red is the repro. Guard (c) may already pass; record
   its actual pre-fix result in the task report.

**Tests:** (executable spec)
- `xpath_split_three_matches_fragments_are_raw_text`: 3-link XML sent to the
  xpath-split route → mock:fragments receives 3 exchanges → every fragment
  body is `Body::Text` equal to the raw link string; none contains a `"`
  character. command: `cargo test -p camel-test --test xpath_test xpath_split_three`
  — expected: FAIL pre-fix (quotes present), PASS post-fix.
- `xpath_split_match_count_parity`: 1-link XML and 3-link XML (same first link
  value) sent through the same split → fragment[0] of both runs is
  `Body::Text("https://example.org/bar.xml")`, identical variant and content.
  command: `cargo test -p camel-test --test xpath_test xpath_split_match_count_parity`
  — expected: FAIL pre-fix, PASS post-fix.
- `xpath_split_collect_all_aggregate_byte_identical`: 3-link split with
  `collect_all` → `mock:agg` receives one exchange whose body stringifies to
  `["https://example.org/1.xml","https://example.org/2.xml","https://example.org/3.xml"]`
  (Value::Array of Value::String — byte-identical to pre-fix aggregation).
  command: `cargo test -p camel-test --test xpath_test xpath_split_collect_all`
  — expected: PASS pre- and post-fix (aggregation indifference guard).

**Acceptance:**
- The three tests compile and run; the two typing tests are demonstrably red
  at the quote/variant assertion before Task 3.1 (paste the failing assert
  line into the task report).
- No new sleep or unbounded waits: settles use
  `await_exchanges(count, timeout)`; `cargo xtask lint-test-sleep` ratchet
  does not grow.

- [x] 1.1

### Task 1.2: jsonpath split guards — string-array text, non-string stays JSON

**Files:**
- `crates/camel-test/tests/jsonpath_test.rs` (modified)

**Steps:**
1. Read the file's existing harness (context builder, jsonpath registration,
   YAML route pattern, mock endpoint access) and reuse it — EXCEPT the settle
   idiom: do NOT copy `tokio::time::sleep(...)` lines (lint-test-sleep
   ratchet); settle via bounded `MockEndpoint::await_exchanges(count, timeout)`.
2. Add `jsonpath_split_string_array_yields_text_fragments` (guard d): route
   `direct:start` → `split: {expression: {jsonpath: "$.links"}, steps: [{to:
   "mock:frags"}]}`; send `Body::Json({"links":["a","b"]})`.
3. Add `jsonpath_split_non_string_elements_stay_json` (guard e): same shape
   over `$.items` with `Body::Json({"items":[41,{"k":"v"}]})`.
4. Run both; guard (d) is expected red pre-fix, guard (e) green pre-fix
   (regression pin) — record both actual results in the task report.

**Tests:**
- `jsonpath_split_string_array_yields_text_fragments`: send
  `{"links":["","a","b"]}` → mock:frags receives 3 exchanges with bodies
  `Body::Text("")`, `Body::Text("a")`, `Body::Text("b")` (no `"`
  characters; the leading empty element pins the accepted empty-string
  delta). command:
  `cargo test -p camel-test --test jsonpath_test jsonpath_split_string_array`
  — expected: FAIL pre-fix, PASS post-fix.
- `jsonpath_split_non_string_elements_stay_json`: send
  `{"items":[41,{"k":"v"}]}` → fragments are `Body::Json(Number 41)` and
  `Body::Json(Object {"k":"v"})` (variant unchanged by Task 3.1). command:
  `cargo test -p camel-test --test jsonpath_test jsonpath_split_non_string`
  — expected: PASS pre- and post-fix.

**Acceptance:**
- Both tests compile and run; (d) red at the variant/quote assertion pre-fix;
  (e) green pre-fix. Actual results pasted in the task report.
- No new sleep or unbounded waits: settles use
  `await_exchanges(count, timeout)`; `cargo xtask lint-test-sleep` ratchet
  does not grow.

- [x] 1.2

## camel-api

### Task 2.1: pin programmatic split_body_json_array typing (unchanged)

**Files:**
- `crates/camel-api/src/splitter.rs` (modified — tests module only)

**Steps:**
1. In the existing `#[cfg(test)] mod tests` of `splitter.rs` (which already
   tests `split_body_lines`), add `split_body_json_array_string_elements_stay_json`
   modeled on `test_split_body_lines`.
2. Run it — must be green immediately (this pins current behavior; the
   separate bd tracks changing it).

**Tests:**
- `split_body_json_array_string_elements_stay_json`: exchange with
  `Body::Json(["a","b"])` → `split_body_json_array()(&ex)` yields 2 fragments,
  each `Body::Json(Value::String)` (NOT `Body::Text`). command:
  `cargo test -p camel-api --lib split_body_json_array_string` — expected:
  PASS pre- and post-fix (guard f).

**Acceptance:**
- Test green; `cargo fmt --check` and
  `cargo clippy -p camel-api -- -D warnings` clean.

- [x] 2.1

## camel-core

### Task 3.1: element-driven fragment typing in DeclarativeSplit Value::Array branch

**Files:**
- `crates/camel-core/src/lifecycle/adapters/step_compilers/splitting.rs` (modified)

**Steps:**
1. In the `DeclarativeSplit` arm's `split_fn` closure, replace the
   `Value::Array(arr)` per-element mapping `fragment.input.body = Body::from(v)`
   with an inner match: `Value::String(s) => Body::Text(s)`, `other =>
   Body::Json(other)` (exact code in `design.md ## Approach`).
2. Do NOT touch the `Value::String` branch, the error arm, or anything outside
   this closure; do NOT touch `split_body_json_array`, the simple evaluator,
   or camel-language-xpath.
3. Run the Task 1.1/1.2 red tests — they must now pass; Task 2.1/1.2 guard (e)
   stay green.

**Tests:**
- `cargo test -p camel-test --test xpath_test xpath_split` — all 3 green.
- `cargo test -p camel-test --test jsonpath_test jsonpath_split` — both green.
- `cargo test -p camel-api --lib split_body_json_array_string` — still green.

**Acceptance:**
- All six guards green in one run.
- `cargo fmt --check` and
  `cargo clippy -p camel-core -- -D warnings` clean; no new `unwrap()`
  (`cargo xtask lint-unwrap` at STAGE 4 gates).

- [x] 3.1

## docs

### Task 4.1: splitter doc fragment-typing callout

**Files:**
- `docs/src/eip/splitter.md` (modified)

**Steps:**
1. Add a short "Fragment typing" paragraph after the existing "A split
   expression decides how to divide the body" paragraph: when the split
   expression evaluates to an array, string elements become `Body::Text`
   fragments (so `${body}` renders the raw string, no JSON quotes — identical
   whether the expression matched one node or many); number, boolean, object,
   nested-array, and null elements become `Body::Json` fragments (JSON text
   form); an empty-string element renders as an empty string, not `""`.
2. Mention the aggregation consequence in one sentence: `CollectAll` output
   is the same either way.

**Tests:**
- Docs-only; verified by reading. command: n/a.

**Acceptance:**
- Paragraph present, states the string→`Body::Text` and
  non-string (number/bool/object/nested-array/null)→`Body::Json` rule and
  count-independence explicitly, matches the delta spec wording.

- [x] 4.1
