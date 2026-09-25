# Tasks: splitjson

## Task 1.1 — Flip the transitional pin (RED)

**Files:** `crates/camel-api/src/splitter.rs` (tests module only)

**Steps**

1. Replace `split_body_json_array_string_elements_stay_json` (251's
   transitional pin of the OLD behavior) with
   `split_body_json_array_string_elements_become_text`.
2. Add `test_split_body_json_array_non_string_elements_stay_json` pinning
   number, object, and null elements.

**Tests (name / arrange / act / assert)**

- `split_body_json_array_string_elements_become_text`
  - arrange: `Exchange::new(Message::new(serde_json::json!(["", "a", "b"])))`
  - act: `split_body_json_array()(&ex)`
  - assert: 3 fragments; fragment 0 `Body::Text("")`, fragment 1
    `Body::Text("a")`, fragment 2 `Body::Text("b")`; no fragment body carries
    a literal `"` character. Comment pins the empty-string delta per 251.
- `test_split_body_json_array_non_string_elements_stay_json`
  - arrange: `Exchange::new(Message::new(serde_json::json!([1, {"k":"v"}, null()])))`
  - act: `split_body_json_array()(&ex)`
  - assert: 3 fragments, each `Body::Json` with the element value unchanged.

**Acceptance:** `cargo test -p camel-api --lib split_body_json_array` FAILS
against the unfixed production code. Record the RED evidence (expected
observed shape: `Body::Json(String(""))` for fragment 0).

- [x] 1.1

## Task 2.1 — Element-driven typing in split_body_json_array (GREEN)

**Files:** `crates/camel-api/src/splitter.rs`

**Steps**

1. In `split_body_json_array`, replace the `Body::Json(val.clone())` mapping
   with the two-arm match: `serde_json::Value::String(s)` →
   `fragment_exchange(exchange, Body::Text(s.clone()))`; `other` →
   `fragment_exchange(exchange, Body::Json(other.clone()))`.
2. Update the fn doc comment: string elements produce `Body::Text` fragments
   (raw string, `${body}` renders unquoted); all other elements produce
   `Body::Json`.

**Tests:** task 1.1's tests turn green; existing `test_split_body_json_array`
(numbers `[1,2,3]` → `Body::Json`), wrong-type error tests, and empty-body /
empty-array pass-through tests stay green unchanged; 251's six declarative
guards stay green (no camel-core change).

**Acceptance:** `cargo test -p camel-api --lib` passes;
`cargo fmt --check` clean; `cargo clippy -p camel-api --all-targets --
-D warnings` clean.

- [x] 2.1

## Task 3.1 — Same-class scan of other programmatic splitters

**Files:** none (scan). Fix only a trivial, in-crate instance of the same
defect class; everything else is reported, not fixed.

**Steps**

1. Enumerate fragment-body construction sites reachable from
   `crates/camel-api/src/splitter.rs` and the streaming split
   (`crates/camel-api/src/runtime.rs`): `split_body_lines` (expected
   `Body::Text` per line), `split_body(f)` custom closure (owns its typing
   policy), streaming split fragment typing.
2. Record each site's typing rule and whether a string value can surface as
   `Body::Json` there.

**Acceptance:** scan result recorded for the park report — findings with file:
line evidence, or a clean verdict naming every checked site.

**Scan result (2026-09-25):** `split_body_lines` (splitter.rs:367) —
`Body::Text` per line, clean. `split_body(f)` (splitter.rs:423) — closure
owns typing policy, by design. Streaming split — no in-crate fragment
construction site (runtime.rs:222 is config/spec only); camel-processor
stream codecs checked: lines → `Body::Text` (clean), chunks → `Body::Bytes`
(clean), ndjson → `Body::Json(record)` — a bare-string NDJSON line surfaces
as `Body::Json(Value::String)` (same class, out-of-crate, semantics
defensible: each line is a JSON document). Parked as bd rc-rit6f
(discovered-from rc-fflhv). r_glm minors (shared typing helper;
processor-level string-element test) parked in tech-debt-sweep bd rc-xj1b1.

- [x] 3.1

## Task 4.1 — Docs callout clause

**Files:** `docs/src/eip/splitter.md`

**Steps**

1. Extend the "Fragment typing" paragraph with one clause: the programmatic
   `split_body_json_array()` splitter follows the same element-driven rule.
   Keep the STE-plain wording style of the existing paragraph.

**Acceptance:** paragraph states the rule for both paths; rustdoc doc-build
gate passes (no code doc links touched).

- [x] 4.1
