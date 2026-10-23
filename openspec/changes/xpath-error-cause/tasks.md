# Tasks: xpath-error-cause

## camel-language-api

### Task 1.1: Redacted cause preservation in the shared error envelope

**Files:**
- `crates/languages/camel-language-api/src/error.rs` (modified)
- `crates/languages/camel-language-api/src/eval.rs` (modified)
- `crates/languages/camel-language-api/src/lib.rs` (modified — export only)

**Steps:**
1. In `error.rs`, add `pub fn redacted_detail(&self) -> Option<String>` to
   `impl LanguageError`: for `EvalFailure { detail, .. }` return
   `detail.clone()` (the field is already `Option<String>`: `None` stays
   `None`); `Some(format!("expected {expected}, actual {actual}"))`
   for `TypeMismatch { expected, actual, .. }`;
   `Some(format!("cannot convert {source_type} to {target}"))` for
   `ConversionError { source_type, target }`; `None` for every other variant
   (`EvalError`, `ParseError`, `UnknownVariable`, `NotFound`,
   `NotSupported`). Doc comment states the redaction contract: `Some` only
   for payloads contractually free of exchange/provider data; free-form
   payloads return `None`.
2. In `eval.rs`, add a private zero-size marker: `#[derive(Debug)] struct
   ExpressionDetailCause;` (`Debug` is required by `std::error::Error`) with
   `impl std::fmt::Display` (`"expression
   evaluation diagnostic"`) and `impl std::error::Error`. Keep it private to
   the crate.
3. In `eval.rs`, add `pub fn is_expression_detail_cause(err: &CamelError) ->
   bool`: true iff `err` is `ExpressionFailed { cause: Some(c), .. }` where
   `c` (after `AsRef::<dyn std::error::Error>::as_ref(&**c)` /
   `(&**c).source()` chain: the cause's own `source()`) downcasts via
   `downcast_ref::<ExpressionDetailCause>()` to the marker. Doc comment:
   exact mapper provenance, never shape matching. Export it from `lib.rs`
   next to `to_expression_failed`.
4. In `eval.rs` `to_expression_failed`, replace `cause: None` with
   `cause: err.redacted_detail().map(|d| Box::new(CamelError::
   ProcessorErrorWithSource(d, Arc::new(ExpressionDetailCause))))`. Update
   the function doc: cause carries a fixed redacted detail when the
   LanguageError carries a contractually redacted payload; free-form
   payloads keep `cause: None`.

**Tests:** (in the existing `#[cfg(test)] mod tests` of `eval.rs`;
`test_meta` helper exists)
- `to_expression_failed_carries_redacted_detail_as_cause`: build
  `LanguageError::EvalFailure { class: ExpressionErrorClass::Arithmetic,
  position: None, detail: Some("overflow".into()) }` → `to_expression_failed`
  with `test_meta(None)` → assert mapped `ExpressionFailed` has
  `cause: Some`, `cause.to_string() == "Processor error: overflow"`, and
  `cause.as_ref().unwrap().as_ref()` (as `dyn std::error::Error`) `.source()`
  is `Some` (the marker).
- `to_expression_failed_keeps_cause_none_for_free_form`: `EvalError("x".into())`
  → `cause: None`; `ParseError { expr: "e".into(), reason: "r".into() }` →
  `cause: None`.
- `redacted_detail_covers_typed_variants`: `TypeMismatch { expected: "bool",
  actual: "string", position: None }` → `Some("expected bool, actual
  string")`; `ConversionError { source_type: "null", target: "f64" }` →
  `Some("cannot convert null to f64")`; `EvalError` and `ParseError` →
  `None`.
- `is_expression_detail_cause_is_exact`: mapper-generated failure (as in
  test 1) → `true`; hand-built `ExpressionFailed` with
  `cause: Some(Box::new(CamelError::ProcessorError("caller diagnostic".into())))`
  → `false`; `ExpressionFailed` with `cause: None` → `false`.
- `mapped_typed_variants_carry_cause`: map `TypeMismatch { expected: "bool",
  actual: "string", position: None }` through `to_expression_failed` →
  class `TypeMismatch`, `cause.to_string() == "Processor error: expected
  bool, actual string"`, `is_expression_detail_cause` → `true`; map
  `ConversionError { source_type: "null", target: "f64" }` → class
  `Conversion`, `cause.to_string() == "Processor error: cannot convert null
  to f64"`, `is_expression_detail_cause` → `true`.
- Keep existing test `to_expression_failed_defaults_eval_error_to_runtime`
  passing (class Runtime; now also asserts `cause: None`).

**Acceptance:**
- All cargo commands run as
  `systemd-run --user --unit=<name> --collect --wait --pipe --same-dir -p
  MemoryMax=12G -p MemorySwapMax=2G --setenv=TMPDIR=/home/shared/tmp cargo
  ...` from inside the worktree (`--wait --pipe` so the cargo exit status
  propagates; `--same-dir` pins the worktree cwd). If the wrapper exits
  254, retry once with `RUSTC_WRAPPER= cargo ...`.
- `cargo test -p camel-language-api` exits 0.
- `cargo fmt --check -p camel-language-api` and `cargo clippy -p
  camel-language-api -- -D warnings` exit 0.
- No `unwrap()`/`expect()` added outside `#[cfg(test)]` (cargo xtask
  lint-unwrap clean).

- [x] 1.1

## camel-language-xpath + camel-api

### Task 1.2: Classify xpath failure paths and pin the ~100 KB discrimination

**Files:**
- `crates/languages/camel-language-xpath/src/lib.rs` (modified)
- `crates/languages/camel-language-xpath/Cargo.toml` (modified — dev-deps only)
- `crates/camel-api/src/error.rs` (modified — doc comments only)

**Steps:**
1. `camel-api/src/error.rs`: extend the `ExpressionErrorClass::Parse` doc
   line from "The expression source could not be parsed." to "The expression
   or document source could not be parsed." Extend the `Conversion` doc line
   from "The result could not be converted to the declared destination
   type." to "The result or input body could not be converted to the
   declared destination type." No code changes in this crate.
2. `lib.rs` `run_query`: size branch emits `LanguageError::EvalFailure {
   class: ExpressionErrorClass::Limit, position: None, detail:
   Some("input exceeds maximum allowed size".into()) }`.
3. `lib.rs` `run_query`: sxd parse-failure branch emits `EvalFailure {
   class: ExpressionErrorClass::Parse, position: None, detail:
   Some("body is not valid XML".into()) }` (keep the existing `debug!` log).
4. `lib.rs` `run_query`: sxd eval-failure branch emits `EvalFailure {
   class: ExpressionErrorClass::Runtime, position: None, detail:
   Some("xpath query failed: expression could not be evaluated".into()) }`
   (keep the existing `debug!` log).
5. `lib.rs` `extract_xml`: the `try_into_xml` failure arm emits `EvalFailure
   { class: ExpressionErrorClass::Conversion, position: None, detail:
   Some("body is not XML and cannot be coerced".into()) }` — the
   `format!("...: {e}")` interpolation is REMOVED (provider text); the
   produced-non-XML arm emits `EvalFailure { class:
   ExpressionErrorClass::Conversion, position: None, detail:
   Some("body coercion did not produce XML".into()) }`.
6. `Cargo.toml`: add `camel-api` as a `[dev-dependencies]` entry (the xpath
   tests must name `camel_api::CamelError` when asserting on
   `to_expression_failed` output; check whether a workspace-histogram
   dev-dep already exists before adding).
7. Rewrite test `expression_rejects_oversized_input` (currently asserts
   `EvalError`): assert `EvalFailure { class: Limit, .. }` with `detail`
   exactly `"input exceeds maximum allowed size"`; then map through
   `camel_language_api::to_expression_failed` with an `EvalMeta { language:
   "xpath", route_id: "r1", step_id: "s1", verb: "set_body", target: None }`
   and assert class `Limit` and
   `cause.to_string() == "Processor error: input exceeds maximum allowed
   size"`.
8. Strengthen test `predicate_rejects_oversized_input`: assert
   `EvalFailure { class: Limit, .. }`.
9. Add test `coercion_failure_classifies_as_conversion`: exchange whose
   body is `Body::Json(serde_json::json!("SECRETSENTINEL"))` — a JSON
   SCALAR, which is a known `try_into_xml` failure (JSON objects DO convert;
   do not use an object) → mapped class `Conversion`, cause payload exactly
   `"Processor error: body is not XML and cannot be coerced"` (exact
   equality enforces the `{e}` removal).
10. Add test `malformed_xml_classifies_as_parse`: body constructed as
    `Body::Xml("<root><unclosed>".to_string())` (the sxd parse branch is
    reachable only for already-typed `Body::Xml`; a malformed TEXT body
    fails coercion as Conversion instead) → mapped class `Parse`, cause
    payload exactly `"Processor error: body is not valid XML"`.
11. Add test `genuine_eval_failure_keeps_runtime`: expression
    `"$__undefined_var__"` on valid XML `<root/>` (Body::Xml) → mapped
    class `Runtime`, cause payload exactly `"Processor error: xpath query
    failed: expression could not be evaluated"`. VERIFY FIRST that
    `$__undefined_var__` fails at sxd EVALUATION time, not compile time
    (quick probe run of the test); if it fails at compile time instead,
    STOP and report `test-design-gap: no known eval-time-failing xpath
    expression` — do not guess another expression.
12. Add test `mapped_100kb_size_vs_malformed_distinguishable`: config
    `max_input_bytes: Some(96 * 1024)`; both bodies as `Body::Xml` (via the
    existing `exchange_with_xml` helper). Body A = well-formed
    `format!("<root>{}</root>", "x".repeat(96 * 1024))` (length > limit) →
    mapped class `Limit`, cause contains `"input exceeds maximum allowed
    size"`. Body B = malformed
    `format!("<root><a>{}</root>", "y".repeat(96 * 1024 - 32))` (length
    < limit, ~96 KB ≈ 100 KB scale) → mapped class `Parse`, cause contains
    `"body is not valid XML"`. Assert B's byte length is below the limit
    before mapping (guard against silent size-rejection), the two classes
    differ and the two cause renders differ. Both tellable apart with no
    logging enabled.
13. Add test `sentinel_never_enters_mapped_cause` covering ALL FOUR
    reclassified paths:
    - Limit: body A from step 12 with `SECRETSENTINEL` inside the padding
      text → class `Limit`, sentinel absent from `cause.to_string()` and
      from the full `Error::source()` traversal.
    - Parse: body B from step 12 rebuilt with padding reduced so that even
      with `SECRETSENTINEL` inside a tag name the byte length stays BELOW
      the limit (assert `body.len() < limit` in the test) → class `Parse`,
      sentinel absent from cause and traversal.
    - Conversion: `Body::Json(json!("SECRETSENTINEL"))` per step 9 → class
      `Conversion`, sentinel absent from cause and traversal.
    - Runtime: the step-11 expression evaluated against
      `<root>SECRETSENTINEL</root>` → class `Runtime`, sentinel absent from
      cause and traversal.
    Traversal helper: walk `std::error::Error::source()` from the mapped
    error to the end of the chain, format each link, assert the sentinel
    appears nowhere.

**Tests:** listed per step above; ALL cargo commands wrapped as
`systemd-run --user --unit=<name> --collect --wait --pipe --same-dir -p
MemoryMax=12G -p MemorySwapMax=2G --setenv=TMPDIR=/home/shared/tmp cargo
...` (exit status propagates; on wrapper exit 254 retry once with
`RUSTC_WRAPPER= cargo ...`).
Expected: the listed class/cause tests FAIL before steps 2–5 (they assert
the new classes) and PASS after; `expression_rejects_oversized_input` fails
before (old `EvalError` assertion) and passes after rewrite.

**Acceptance:**
- `cargo test -p camel-language-xpath` exits 0; `cargo test -p camel-api`
  exits 0.
- `cargo fmt --check -p camel-language-xpath -p camel-api` and
  `cargo clippy -p camel-language-xpath -p camel-api -- -D warnings` exit 0.
- No occurrence of `format!` interpolation from body/provider errors in the
  three `extract_xml`/`run_query` error arms (inspectable in the diff).

- [x] 1.2

## camel-processor

### Task 1.3: Marker-based cause replacement in catch chaining

**Files:**
- `crates/camel-processor/src/do_try.rs` (modified)

**Steps:**
1. Update `chain_predicate_error`: chain the original error when
   `cause.is_none()` OR
   `camel_language_api::is_expression_detail_cause(&predicate_err)`;
   otherwise pass through untouched. Keep the function signature.
2. Update the doc comment: mapper-generated diagnostic causes (provenance
   marker, not shape) are replaced by the original route error; genuine
   caller-supplied chains pass through untouched.
3. Existing unit tests in the file that build `ExpressionFailed` with
   `cause: None` keep passing unchanged.

**Tests:** (in `do_try.rs` `#[cfg(test)]`)
- `chain_replaces_mapper_generated_cause`: build a mapper-generated failure
  with `camel_language_api::to_expression_failed(LanguageError::EvalFailure
  { class: ExpressionErrorClass::Parse, position: None, detail:
  Some("body is not valid XML".into()) }, &meta)` (meta: language "xpath",
  route "r1", step "s1", verb "when", target None) → run
  `chain_predicate_error(failure, CamelError::ProcessorError("boom".into()))`
  → assert cause is `Some` and renders text containing `"boom"` (the
  diagnostic cause was replaced).
- `chain_preserves_caller_cause`: build `ExpressionFailed` with
  `cause: Some(Box::new(CamelError::ProcessorError("caller
  diagnostic".into())))` → run `chain_predicate_error` with original
  `"boom"` → assert cause still renders `"caller diagnostic"` and not
  `"boom"`.
- `chain_preserves_caller_source_bearing_cause`: build `ExpressionFailed`
  with `cause: Some(Box::new(CamelError::ProcessorErrorWithSource("caller
  diagnostic".into(), Arc::new(std::io::Error::new(std::io::ErrorKind::
  Other, "caller source")))))` → run `chain_predicate_error` with original
  `"boom"` → assert cause still renders `"caller diagnostic"` AND its
  `source()` still renders `"caller source"` (no marker, untouched).
- Command: ALL cargo commands wrapped as
  `systemd-run --user --unit=<name> --collect --wait --pipe --same-dir -p
  MemoryMax=12G -p MemorySwapMax=2G --setenv=TMPDIR=/home/shared/tmp cargo
  ...`; run `cargo test -p camel-processor do_try`. Expected:
  `chain_replaces_mapper_generated_cause` FAILS before step 1 (cause stays
  the diagnostic) and PASSES after; `chain_preserves_caller_cause` and
  `chain_preserves_caller_source_bearing_cause` PASS both before and after
  (document today's behavior).

**Acceptance:**
- `cargo test -p camel-processor` exits 0.
- `cargo fmt --check -p camel-processor` and `cargo clippy -p
  camel-processor --all-targets -- -D warnings` exit 0.

- [x] 1.3

## camel-core

### Task 1.4: Integration proof — non-breaking catch chaining and Runtime caller

**Files:**
- `crates/camel-core/tests/expression_failure_propagation_test.rs` (modified)

**Steps:**
1. Update the stale doc comment above `escaped_env_slot_fails_at_runtime`
   (~line 1265): replace "The failure detail is discarded by
   `to_expression_failed` (`cause: None`), so no error text is asserted
   here" with "The free-form `EvalError` detail keeps `cause: None`, so no
   error text is asserted here" (accurate post-change: typed payloads now
   carry cause).
2. Add test `do_try_xpath_onwhen_predicate_failure_chains_original`: reuse
   the file's existing route harness and language registration (the file
   already exercises xpath and doTry/onWhen flows; register languages the
   same way existing tests in this file do). Initialize the exchange body
   as `Body::Xml` (NOT the harness default `Text("payload")` — a malformed
   TEXT body fails coercion as Conversion, never reaching the sxd parse
   branch) with malformed XML `<root><unclosed>`. Build: doTry whose try
   body raises a failing step (same mechanism the existing chained-cause
   test at ~line 874 uses, error text marker `boom-A`), one doCatch with an
   onWhen xpath predicate (e.g. `/root/flag='yes'`) evaluated against the
   malformed `Body::Xml` body, catch steps no-op. Act: run the route.
   Assert: surfaced error is `ExpressionFailed` with class `Parse`; its
   cause chain renders text containing `boom-A`; and a standard
   `std::error::Error::source()` traversal from the surfaced error reaches
   the intact original `CamelError` (traverse `.source()` from the
   ExpressionFailed and assert the reached error renders `boom-A`).
3. Add Runtime-caller assertion `xpath_eval_failure_still_runtime_for_callers`:
   route with a set_body xpath expression `$__undefined_var__` over valid
   XML `<root/>` in a `Body::Xml` body (registered like the other tests) →
   assert surfaced `ExpressionFailed` class == `ExpressionErrorClass::Runtime`
   (a caller matching `Runtime` for genuinely-runtime paths still matches).
   Reuse the verified eval-time-failing expression from Task 1.2 step 11; if
   Task 1.2 reported `test-design-gap`, mirror the gap: STOP and report
   `test-design-gap: no known eval-time-failing xpath expression`.
   EXPECTATION NOTE: this test PASSES both before and after the change
   (today's `EvalError` also maps to Runtime) — it is compatibility
   coverage, not a fail-first regression test.

**Tests:** the two new tests above. This suite is feature-gated: inspect the
existing tests' `#[cfg(feature = ...)]` gates and camel-core's
`Cargo.toml` language features; enable every feature the two new tests need
(xpath plus whatever the harness requires, e.g. lang-rhai/lang-jsonpath) in
the test command, and gate the two new tests with the SAME feature
attribute the file's xpath/language tests use. Run: ALL cargo commands
wrapped as `systemd-run --user --unit=<name> --collect --wait --pipe
--same-dir -p MemoryMax=12G -p MemorySwapMax=2G
--setenv=TMPDIR=/home/shared/tmp cargo ...`; then
`cargo test -p camel-core --test expression_failure_propagation_test
--features <required features>` and VERIFY from the output that both new
test names actually executed (0 tests run = wrong features = failure).
Expected: `do_try_xpath_onwhen_predicate_failure_chains_original` FAILS
before Tasks 1.1–1.3 land and PASSES after;
`xpath_eval_failure_still_runtime_for_callers` PASSES both before and
after.

**Acceptance:**
- `cargo test -p camel-core --test expression_failure_propagation_test
  --features <required>` exits 0 with both new test names executed.
- `cargo fmt --check -p camel-core` and `cargo clippy -p camel-core
  --all-targets -- -D warnings` exit 0.

- [x] 1.4

## Task dependency order

1.1 → 1.2 → 1.3 → 1.4 (strictly sequential: 1.2 maps through 1.1's cause
carrier; 1.3 needs 1.1's `is_expression_detail_cause`; 1.4 needs all prior).
