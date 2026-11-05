# Tasks: simple-bytes-interpolation

Single-phase change. Groups: `camel-language-simple` (tests, then implementation),
then `docs` + CHANGELOG.

## camel-language-simple

### Task 1.1: Acceptance test suite (TDD — write first, verify red)

**Files:**
- `crates/languages/camel-language-simple/src/lib.rs` (modified — `#[cfg(test)]` module only)

**Steps:**
1. Rename the existing test `body_bytes_returns_byte_array` (near line 1976) to
   `bare_body_bytes_stays_byte_array`; keep its assertions exactly: `${body}` over
   `Body::from(vec![0u8, 128, 255])` yields
   `Value::Array([Number(0), Number(128), Number(255)])`. Do not touch any other
   line of the test.
2. Replace the existing test `body_bytes_in_interpolation` (near line 2036, which
   pins the old byte-list rendering `"Received: [112,105,110,103]"`) with the new
   `interpolated_bytes_render_as_utf8_text` test per the spec below.
3. Add the five new tests from the spec below (the four that follow plus
   `interpolated_bytes_async_path_renders_utf8_text`), placed next to the
   replaced test. Use the existing construction idiom for byte bodies:
   `ex.input.body = Body::from(b"hello".to_vec());` and
   `ex.input.body = Body::from(vec![0xffu8, 0xfe]);`, headers via
   `ex.input.set_header("CamelHttpResponseCode", Value::Number(200.into()));`.
4. Run the crate test suite. Expected state: the five new-behavior tests FAIL
   (red) because interpolation still stringifies the byte array; the two
   pin-behavior tests (`bare_body_bytes_stays_byte_array`,
   `non_bytes_interpolation_unchanged`) PASS.

**Tests:** (executable specs)
- `bare_body_bytes_stays_byte_array`: setup `Exchange::default()` with
  `input.body = Body::from(vec![0u8, 128, 255])` → act
  `SimpleLanguage::new().create_expression("${body}")` + `evaluate` → assert the
  value equals `Value::Array` of `Number(0)`, `Number(128)`, `Number(255)`
  (byte-sequence array, unchanged).
- `interpolated_bytes_render_as_utf8_text`: setup body
  `Body::from(b"hello".to_vec())` → act evaluate template `body=${body}` →
  assert `Value::String("body=hello")`, not `body=[104,101,108,108,111]`.- `interpolated_bytes_prefix_and_suffix`: setup body
  `Body::from(b"hello".to_vec())` → act evaluate template `pre${body}post` →
  assert `Value::String("prehellopost")` (no brackets, exactly one
  concatenation).
- `interpolated_invalid_utf8_bytes_fail_typed`: setup body
  `Body::from(vec![0xffu8, 0xfe])` → act evaluate template `body=${body}` →
  assert the evaluation returns `LanguageError::ConversionError` (class
  `conversion`; assert `source_type` names `Body::Bytes`), and assert the error
  fields contain no `\u{FFFD}` replacement character.
- `interpolated_mixed_headers_and_bytes`: setup header
  `CamelHttpResponseCode` = `200` and body `Body::from(b"ok".to_vec())` → act
  evaluate template `status=${header.CamelHttpResponseCode} body=${body}` (the
  reported camel-112 case) → assert
  `Value::String("status=200 body=ok")`.
- `non_bytes_interpolation_unchanged`: setup header `x` = `"a"` and body
  `Body::Text("b".to_string())` → act evaluate template `${header.x}-${body}` →
  assert `Value::String("a-b")` (Text-body interpolation unchanged).
- `interpolated_bytes_async_path_renders_utf8_text`: exercises the async
  interpolation walk (`eval_async_expr`), which runs only for delegate-containing
  templates. Setup: `let resolver = Arc::new(|name: &str| if name == "boollang" {
  Some(Arc::new(BoolLanguage) as Arc<dyn Language>) } else { None });` and
  `SimpleLanguage::with_resolver(resolver)` (the `BoolLanguage` mock already
  defined in this test module returns `Value::Bool(true)` for script `yes`);
  body `Body::from(b"hello".to_vec())` → act evaluate template
  `${body}${boollang:yes}` → assert `Value::String("hellotrue")` (Bytes slot
  rendered as UTF-8 text on the async path, delegate slot rendered as before;
  pre-implementation this renders `[104,101,108,108,111]true` → red).

  Command for all: `cargo test -p camel-language-simple`
  Expected before Task 1.2: 5 red / 2 green as described in step 4.

**Acceptance:**
- The seven named tests exist with exactly those names in
  `crates/languages/camel-language-simple/src/lib.rs`.
- `cargo test -p camel-language-simple` compiles; exactly the five
  new-behavior tests fail with byte-list rendering or missing behavior.
- `cargo fmt --check --all` exits 0.

- [x] 1.1

### Task 1.2: Interpolation-context UTF-8 rendering (make red tests green)

**Files:**
- `crates/languages/camel-language-simple/src/evaluator.rs` (modified)

**Steps:**
1. Add a private helper next to `body_value` (near line 212):
   `fn interpolate_body_text(exchange: &Exchange) -> Result<String, LanguageError>`:
   match on `exchange.input.body`; for `Body::Bytes(b)` return
   `std::str::from_utf8(b).map(str::to_owned).map_err(|_| conversion_error("Body::Bytes", "text"))`;
   for every other variant delegate to the existing chain — call `body_value(exchange)`
   and render with the same rules as the interpolation loop (`Value::Null` →
   `String::new()`, `Value::String(s)` → `s`, `other` → `other.to_string()`), so
   Text/Xml/Json/Empty behavior and the `Body::Stream` typed refusal are unchanged.
2. In the sync `Expr::Interpolated` arm of `eval_sync_expr` (near line 315) and the
   async arm of `eval_async_expr` (near line 487): inside
   `InterpolatedPart::Expr(sub_expr)`, special-case `sub_expr` matching
   `Expr::Body` — call `ctx.charge_work()?` first (preserving the per-node
   charge the generic path's sub-evaluation entry would have applied), then
   `interpolate_body_text(ctx.exchange)?` to obtain the rendered string instead
   of evaluating through the generic value path. Keep the existing loop-level
   `charge_work()?` (per-slot charge) and the
   `ctx.charge_output(rendered.len())?` charge and the running-total append for
   ALL parts, special case included, so a `${body}` slot in a template keeps its
   current two-unit cost (slot + node). All other sub-expression shapes keep the
   generic path unchanged.
3. Do NOT change: `body_value`, `body_field_value`, whole-slot sources, predicate
   evaluation, `apply_binop`, or any other crate.
4. Run `cargo test -p camel-language-simple` — all seven acceptance tests from
   Task 1.1 plus the full pre-existing suite must pass.

**Tests:** (executable specs — owned by Task 1.1, re-run here)
- All seven: `bare_body_bytes_stays_byte_array`,
  `interpolated_bytes_render_as_utf8_text`, `interpolated_bytes_prefix_and_suffix`,
  `interpolated_invalid_utf8_bytes_fail_typed`,
  `interpolated_mixed_headers_and_bytes`, `non_bytes_interpolation_unchanged`,
  `interpolated_bytes_async_path_renders_utf8_text`.
  Command: `cargo test -p camel-language-simple` → expected all green, plus
  pre-existing tests green (in particular `test_interpolated_text_with_body`,
  `test_interpolated_missing_header_becomes_empty`, `body_bytes_returns_byte_array`
  logic now covered by the renamed test, stream-refusal tests).

**Acceptance:**
- `cargo test -p camel-language-simple` exits 0 (all targets).
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-language-simple --all-targets -- -D warnings` exits 0.
- No `unwrap()` added outside tests (`cargo xtask lint-unwrap` exits 0).

- [x] 1.2

## docs

### Task 2.1: Document the interpolation rule + CHANGELOG entry

**Files:**
- `docs/src/languages/simple.md` (modified — one line near line 63)
- `CHANGELOG.md` (modified — one entry under `## Unreleased` → `### Fixes`)

**Steps:**
1. In `docs/src/languages/simple.md`, immediately after the sentence documenting
   the template-mode byte-array projection ("...`Bytes` as a JSON array of
   integers 0-255...", near line 63), add one line: "Inside a larger template
   string (any interpolation with more than one part), a `Bytes` body renders as
   UTF-8 text instead; bytes that are not valid UTF-8 fail the evaluation with a
   `conversion` error — never a byte list, never lossy decoding. A bare `${body}`
   source keeps the byte array."
2. In `CHANGELOG.md` under `## Unreleased` → `### Fixes`, add one entry in the
   existing style, first line of the entry area:
   `- fix(languages/simple): interpolating a \`Bytes\` body into a template string with literal text now renders valid UTF-8 as text instead of a byte list, and fails with a \`conversion\` error on invalid UTF-8; a bare \`${body}\` keeps the byte array (bd rc-0v855).`

**Tests:**
- N/A — docs-only. Verification is `cd docs && mdbook test` (runs in the final
  gate battery when `docs/**` changed) and the rendered line being present:
  `grep -F "renders as" docs/src/languages/simple.md` shows the new line near
  the Bytes note.

**Acceptance:**
- Both files contain exactly the added lines; no other docs edits.
- `cd docs && mdbook test` exits 0.

- [x] 2.1
