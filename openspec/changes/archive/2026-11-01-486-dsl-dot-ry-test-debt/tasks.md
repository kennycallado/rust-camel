# Tasks: 486-dsl-dot-ry-test-debt

## Task 1: Add the shared compiled-step drive rig to `tests/common`

**Files**
- `crates/camel-dsl/tests/common/mod.rs` (modified)

**Steps**
1. Extend the imports: add `Exchange` to the `use camel_api::{...}` list; change `use camel_dsl::ValueSourceDef;` to `use camel_dsl::{ValueSourceDef, parse_yaml};`; add `SetBody` to `use camel_processor::{SetHeader, SetHeaderIfAbsent};`; add `use tower::ServiceExt;` (for `.oneshot`).
2. Add `pub type Mutator<'a> = &'a dyn Fn(Exchange) -> Exchange;` with a doc comment saying the rig runs it before the injected step index.
3. Add `pub fn compiled_rest_steps(yaml: &str, op_id: &str, marshal_label: &str) -> Vec<BuilderStep>`: `parse_yaml(yaml).expect("REST YAML must parse + compile")`, find the route whose `route_id() == op_id` with the shared panic `operation '{op_id}' must lower to exactly one route`, clone `route.steps()`, then assert `steps.len() == 6` with `format!("POST must compile to exactly [ContentNegotiation, Unmarshal, user, {marshal_label}, SetHeader, SetHeaderIfAbsent], got: {steps:?}")`; step 0 and 1 are `BuilderStep::Processor(_)`; step 2 is `BuilderStep::DeclarativeSetHeader { .. }`; step 3 is `BuilderStep::Processor(_)` with `format!("step 3 must be the compiled {marshal_label}, got: {:?}", steps[3])`. The step 0/1/2 assertion messages stay verbatim. Callers pass `marshal_label = "Marshal"` (XML battery) or `"ResponseMarshal"` (marshal-error battery). The parse `expect`, the length message, and the step-3 message become the shared templates above; that changes the XML battery's failure strings (`xml REST YAML…` → `REST YAML…`, `xml POST must compile…` → `POST must compile…`, `compiled marshal` → `compiled Marshal`). These are failure-only diagnostics, not assertions: accept the shared prefix.
4. Add `pub fn compile_step_for_drive(step: &BuilderStep) -> camel_api::BoxProcessor`: `Processor` unwraps `op.0.clone()`; `DeclarativeSetHeader` and `DeclarativeSetHeaderIfAbsent` delegate to `compile_header_step`; `DeclarativeSetBody { value }` with `ValueSourceDef::Literal` compiles through `SetBody::new(IdentityProcessor, source)` with `source: Arc<dyn Fn(&Exchange) -> serde_json::Value + Send + Sync>`, exactly as in the marshal-error binary; a non-literal `DeclarativeSetBody` keeps the nested panic `expected literal set_body value, got {other:?}`; any other step panics with `step cannot be driven in this rig: {other:?}`.
5. Add `pub async fn drive_steps(steps: &[BuilderStep], mut ex: Exchange, inject: Option<(usize, Mutator<'_>)>) -> Result<Exchange, CamelError>`: iterate with the index; when `index == inject.0` apply the mutator before running the step; run `compile_step_for_drive(step).oneshot(ex).await?`; return the final exchange.
6. Add `pub async fn serve_pipeline(mut rx: mpsc::Receiver<ExchangeEnvelope>, steps: &[BuilderStep], expected: usize, inject: Option<(usize, Mutator<'_>)>)`: keep the 10 s per-iteration `tokio::time::timeout` and both `expect` messages from the XML binary; on `Ok(passed)` assign `envelope.exchange` and call `reply_ok`; on `Err(e)` send `Err(e)` through `envelope.reply_tx.take()` when present.
7. Update the module-level `//!` doc comment: the consumer list must name all five REST e2e binaries (`rest_negotiation_e2e`, `rest_raw_e2e`, `rest_stream_contract_e2e`, `rest_xml_http_e2e`, `rest_marshal_error_http_e2e`) and the bullet list must name the new step-drive family.

**Tests**
- name: compile check of the shared module
- setup: the new helpers exist in `tests/common/mod.rs`; no binary calls them yet; the module already carries `#![allow(dead_code)]`
- action: `cargo test -p camel-dsl --tests --no-run` (every binary that includes `common` must compile)
- assert: compilation succeeds (exit 0); no unused-import warning is promoted to error
- command: `cargo test -p camel-dsl --tests --no-run`
- expected: pass after the step, fail before it only if `common` does not compile

**Acceptance**
- `rg -n "pub (fn compiled_rest_steps|fn compile_step_for_drive|async fn drive_steps|async fn serve_pipeline|type Mutator)" crates/camel-dsl/tests/common/mod.rs` finds all five declarations.
- `cargo test -p camel-dsl --tests --no-run` exits 0.
- No production file under `crates/*/src/` changes in this task.
- [x] 1.1

## Task 2: Rewire `rest_xml_http_e2e.rs` to the shared rig

**Files**
- `crates/camel-dsl/tests/rest_xml_http_e2e.rs` (modified)

**Steps**
1. Delete the local `compiled_steps`, `compile_step_for_drive`, `Mutator`, `drive_steps`, and `serve_pipeline` definitions.
2. Import `compiled_rest_steps` and `serve_pipeline` from `common`. Remove every import the deletion makes unused: `CamelError`, `ExchangeEnvelope`, `BuilderStep`, `parse_yaml`, `mpsc`, `ServiceExt`, `Duration` (if no other use remains), and `compile_header_step`/`reply_ok` from the `common::{...}` list. Keep `bytes::Bytes`, `camel_api::{Body, Exchange}`, `camel_component_api::test_support::*`, `camel_dsl::parse_yaml` only if still referenced, `serde_json::json`, and `SERVER_MUTEX`/`ServerHandle`/`http_roundtrip`/`simple_request`. Verify with clippy (Acceptance).
3. Replace the local `compiled_steps(XML_HTTP_YAML, "xmlHttpPost")` call with `compiled_rest_steps(XML_HTTP_YAML, "xmlHttpPost", "Marshal")`.
4. Keep `const MARSHAL_STEP_INDEX: usize = 3;` and pass `Some((MARSHAL_STEP_INDEX, &mutator))` where the tests call `serve_pipeline`; the malformed test passes `None`.
5. Keep `spawn_xml_server`, `XML_MEDIA`, `XML_HTTP_YAML`, both test names, and every assertion body unchanged.

**Tests**
- name: `e2e_malformed_xml_returns_400`
- setup: real HTTP consumer on a fresh bind-drop port; compiled XML POST route; malformed XML body
- action: send `POST /xml/post` with `Content-Type: application/xml` and body `<user><name>`
- assert: status 400; JSON body has `error == "bad_request"`; `message` contains `unexpected end of XML input`
- command: `cargo test -p camel-dsl --test rest_xml_http_e2e e2e_malformed_xml_returns_400`
- expected: pass
- name: `e2e_valid_xml_returns_201_xml`
- setup: same server; mutator injects `Body::Json({"user":{"name":"kenny"}})` before the marshal step
- action: send `POST /xml/post` with a valid `<user><name>kenny</name></user>` body
- assert: status 201; `content-type == application/xml`; body equals `<user><name>kenny</name></user>`
- command: `cargo test -p camel-dsl --test rest_xml_http_e2e e2e_valid_xml_returns_201_xml`
- expected: pass

**Acceptance**
- `cargo test -p camel-dsl --test rest_xml_http_e2e` reports `2 passed; 0 failed`.
- Test names are exactly `e2e_malformed_xml_returns_400` and `e2e_valid_xml_returns_201_xml`.
- `rg -c "fn (compiled_steps|compile_step_for_drive|drive_steps|serve_pipeline)|type Mutator" crates/camel-dsl/tests/rest_xml_http_e2e.rs` returns 0 matches (four helpers plus the `Mutator` alias).
- `git diff` on the file touches only the helper block, the `use` lines, and the four call sites (two `compiled_rest_steps`, two `serve_pipeline`).
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-dsl --tests -- -D warnings` exits 0.
- [x] 2.1

## Task 3: Rewire `rest_marshal_error_http_e2e.rs` to the shared rig

**Files**
- `crates/camel-dsl/tests/rest_marshal_error_http_e2e.rs` (modified)

**Steps**
1. Delete the local `compiled_steps`, `compile_step_for_drive`, `drive_steps`, and `serve_pipeline` definitions.
2. Keep `authored_steps`, `replace_user_step`, `USER_STEP_INDEX`, all YAML fixtures, `spawn_server`, and every assertion. Remove every import the deletion makes unused: `ValueSourceDef`, `SetBody`, `IdentityProcessor`, `CamelError`, `ExchangeEnvelope`, `mpsc`, `ServiceExt`, `Duration` (if unused), and `compile_header_step`/`reply_ok` from the `common::{...}` list. `parse_yaml` and `BuilderStep` stay because `authored_steps` and `replace_user_step` use them; verify with clippy (Acceptance).
3. Replace `compiled_steps(YAML, op)` with `compiled_rest_steps(YAML, op, "ResponseMarshal")` for the six-step routes.
4. Pass `None` as the fourth `serve_pipeline` argument in every test.
5. Leave `authored_steps` as the local 7-step assertion (its layout is unique to this binary).

**Tests**
- name: the seven existing tests, unchanged
  (`e2e_json_response_marshal_failure_returns_500`,
  `e2e_form_response_marshal_failure_returns_500`,
  `e2e_xml_response_marshal_failure_returns_500`,
  `e2e_xml_user_step_body_xml_returns_declared_media`,
  `e2e_xml_user_step_body_xml_uses_vnd_media`,
  `e2e_json_malformed_request_still_returns_400`,
  `e2e_authored_marshal_in_rest_user_steps_stays_400`)
- setup: real HTTP consumer per test; six-step or seven-step compiled route; user step replaced where the test needs it
- action: send the request each test already sends
- assert: the status, media type, body, and JSON marker assertions each test already makes stay byte-identical
- command: `cargo test -p camel-dsl --test rest_marshal_error_http_e2e`
- expected: `7 passed; 0 failed`

**Acceptance**
- `cargo test -p camel-dsl --test rest_marshal_error_http_e2e` reports `7 passed; 0 failed`.
- The seven names above are present.
- `rg -c "fn (compiled_steps|compile_step_for_drive|drive_steps|serve_pipeline)|type Mutator" crates/camel-dsl/tests/rest_marshal_error_http_e2e.rs` returns 0 matches (four helpers plus the `Mutator` alias).
- `cargo test -p camel-dsl --test rest_xml_http_e2e --test rest_marshal_error_http_e2e` reports 9 tests total (2 + 7), matching the before count recorded in `evidence/test-counts.md`.
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-dsl --tests -- -D warnings` exits 0.
- [x] 3.1

## Task 4: Make the ADR rule normative in the delta and close the doc cross-reference

**Files**
- `openspec/changes/486-dsl-dot-ry-test-debt/specs/do-try/spec.md` (modified; already drafted)
- `docs/adr/0025-outcome-aware-structural-eips.md` (modified)
- `openspec/changes/486-dsl-dot-ry-test-debt/evidence/test-counts.md` (modified; fill the After section only)

**Steps**
1. In the delta spec, verify the MODIFIED requirement names `CamelExceptionHandled` (Handled sets, Continued does not), the nested-marker survival, and the `CamelExceptionCaught` non-reliance rule, each with a scenario. Missing text is a defect to fix, not a deferral.
2. In ADR-0025, replace the sentence "The existing do-try spec scenarios cover the `Handled` and `Continued` cases; rc-ud3rl defers the additional scenarios." with a sentence that names the four rule cases now covered by `openspec/specs/do-try/spec.md` in this change (bd rc-ud3rl): (a) `Handled` sets the marker; (b) `Continued` does not; (c) a marker set by a nested `Handled` doTry inside an outer `Continued` catch body survives that outer catch's error clearing, while a marker set in a try body that later fails is discarded with the pre-try snapshot; (d) steps after the block cannot rely on `CamelExceptionCaught`.
3. Fill the "After extraction" section of `evidence/test-counts.md`: `rest_xml_http_e2e` 2 / `rest_marshal_error_http_e2e` 7; the same after the extraction; command line used; date.
4. Do not edit `openspec/specs/do-try/spec.md` directly — the canonical file is synced from the delta at archive time.

**Tests**
- name: delta parses and syncs
- setup: change directory with the `specs/do-try/spec.md` delta
- action: `openspec validate 486-dsl-dot-ry-test-debt --type change --strict --json`
- assert: exit 0; no delta-structure error
- command: `openspec validate 486-dsl-dot-ry-test-debt --type change --strict --json`
- expected: pass
- name: whole corpus stays valid
- setup: the delta present alongside every canonical spec
- action: `openspec validate --all --strict`
- assert: exit 0
- command: `openspec validate --all --strict`
- expected: pass

**Acceptance**
- `openspec validate --all --strict` exits 0.
- `rg -n "defers the additional scenarios" docs/adr/0025-outcome-aware-structural-eips.md` returns 0 matches.
- `rg -n "Scenario: Nested Handled marker survives an outer Continued|Scenario: A marker set in a failing try body does not reach the catch chain|Scenario: Steps after a Handled or Continued exit cannot rely on the caught property" openspec/changes/486-dsl-dot-ry-test-debt/specs/do-try/spec.md` finds all three exact headings.
- `cd docs && mdbook test` exits 0.
- Post-landing (master archive): `rg -n "Nested Handled marker survives an outer Continued" openspec/specs/do-try/spec.md` matches after `openspec archive`; the conductor records this as deferred-to-landing.
- [x] 4.1

## Task 5: Pin the three new do-try scenarios with unit tests

**Files**
- `crates/camel-processor/src/do_try_segment.rs` (modified; `#[cfg(test)] mod tests` only)

**Steps**
1. Add `PROPERTY_EXCEPTION_CAUGHT` to the test module import: `use camel_api::exchange::{PROPERTY_EXCEPTION_CAUGHT, PROPERTY_EXCEPTION_MESSAGE};`.
2. Add `#[tokio::test] async fn nested_handled_marker_survives_outer_continued()`: build the inner block `DoTrySegment { try_body: seg_fail(CamelError::Io("inner".into())), catches: vec![catch_variant(&["Io"], seg_complete())], finally: None }` (`catch_variant` defaults to `Handled`); wrap it as `camel_api::OutcomeSegment::new(Box::new(inner))` (the doTry segment implements `OutcomePipeline`). Build the outer block `DoTrySegment { try_body: seg_fail(CamelError::ProcessorError("outer".into())), catches: vec![catch_variant_with(&["ProcessorError"], inner_as_body, ExceptionDisposition::Continued)], finally: None }`. Run on `Exchange::default()`; on `Completed(ex)` assert `!ex.has_error()`, `ex.properties.get(PROPERTY_EXCEPTION_HANDLED) == Some(&Value::Bool(true))`, and `!ex.properties.contains_key(PROPERTY_EXCEPTION_CAUGHT)`; any other outcome panics naming the expected `Completed`.
3. Add a test-only `struct MarkThenFail;` implementing `OutcomePipeline` (the stand-in for "a nested Handled doTry followed by a failing step" in the spec scenario — the snapshot rule treats both identically): `clone_box` returns `Box::new(MarkThenFail)`; `run` inserts `PROPERTY_EXCEPTION_HANDLED = Value::Bool(true)` into the exchange, then returns `PipelineOutcome::Failed(CamelError::ProcessorError("try".into()))`.
4. Add `#[tokio::test] async fn failing_try_body_marker_does_not_reach_catch_chain()`: outer `DoTrySegment { try_body: camel_api::OutcomeSegment::new(Box::new(MarkThenFail)), catches: vec![catch_variant_with(&["ProcessorError"], seg_complete(), ExceptionDisposition::Continued)], finally: None }`; run on `Exchange::default()`; on `Completed(ex)` assert `!ex.has_error()` and `!ex.properties.contains_key(PROPERTY_EXCEPTION_HANDLED)`, because the catch chain starts from the pre-try snapshot; any other outcome panics.
5. Add `#[tokio::test] async fn caught_property_absent_after_handled_and_continued_exit()`: for `disposition` in `[ExceptionDisposition::Handled, ExceptionDisposition::Continued]`, build `DoTrySegment { try_body: seg_fail(CamelError::ProcessorError("boom".into())), catches: vec![catch_variant_with(&["ProcessorError"], seg_check_then_complete(|ex| assert!(ex.properties.contains_key(PROPERTY_EXCEPTION_CAUGHT), "catch body must see CamelExceptionCaught")), disposition)], finally: None }`; run on `Exchange::default()`; on `Completed(ex)` assert `!ex.has_error()` and `!ex.properties.contains_key(PROPERTY_EXCEPTION_CAUGHT)` (clearing removed it), and for `Handled` additionally assert the handled marker is `true` while for `Continued` assert it is absent; any other outcome panics.
6. No production logic changes: the tests must pass against the existing `DoTrySegment` without touching `run`, `handle_error`, or `clear_error`. The nested block must sit in the outer **catch body**, not the outer try body: a failed try outcome carries no exchange, so the catch chain starts from the pre-try snapshot (`do_try_segment.rs:137-140`). The nested-block scenario asserts only the outer result; the nested `Completed` is implied because a nested failure would surface as the catch-body failure.

**Tests**
- name: `nested_handled_marker_survives_outer_continued`
- setup: outer try fails `ProcessorError`; the outer `Continued` catch body is a nested doTry whose own try fails `Io` and whose catch is `Handled`
- action: `outer.run(Exchange::default()).await`
- assert: outcome `Completed`; `has_error()` false; `CamelExceptionHandled == true`; `CamelExceptionCaught` absent
- command: `cargo test -p camel-processor do_try_segment::tests::nested_handled_marker_survives_outer_continued`
- expected: pass (code already matches)
- name: `failing_try_body_marker_does_not_reach_catch_chain`
- setup: the outer try body sets `CamelExceptionHandled` and then fails `ProcessorError`; the outer catch is `Continued`
- action: `outer.run(Exchange::default()).await`
- assert: outcome `Completed`; `has_error()` false; marker absent (pre-try snapshot wins)
- command: `cargo test -p camel-processor do_try_segment::tests::failing_try_body_marker_does_not_reach_catch_chain`
- expected: pass (code already matches)
- name: `caught_property_absent_after_handled_and_continued_exit`
- setup: two blocks, one per disposition, each with a catch body that observes `CamelExceptionCaught`
- action: run each block on a fresh `Exchange::default()`
- assert: both catch bodies see `CamelExceptionCaught`; both returned exchanges have `has_error()` false and no `CamelExceptionCaught`; only `Handled` carries `CamelExceptionHandled == true`
- command: `cargo test -p camel-processor do_try_segment::tests::caught_property_absent_after_handled_and_continued_exit`
- expected: pass (code already matches)

**Acceptance**
- `cargo test -p camel-processor do_try_segment` reports all three new tests passing and no existing `do_try_segment` test regressed.
- `git diff crates/camel-processor/src/do_try_segment.rs` touches only `#[cfg(test)] mod tests` (three tests, `MarkThenFail`, and one import line).
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-processor --tests -- -D warnings` exits 0.
- [x] 5.1
