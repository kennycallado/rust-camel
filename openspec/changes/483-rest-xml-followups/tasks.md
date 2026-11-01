# Tasks: 483-rest-xml-followups

<!-- Single-phase change: no `## Phase N` headings. -->

## camel-processor (data format + marshal service)

### Task 1.1: Accept pre-serialized XML bodies in `XmlDataFormat::marshal`

**Files:**
- `crates/camel-processor/src/data_format/xml.rs` (modified)

**Steps:**
1. In `impl DataFormat for XmlDataFormat::marshal`, replace the `Body::Bytes(_)` arm with a block: `Body::Bytes(b) => { if let Err(e) = std::str::from_utf8(&b) { return Err(CamelError::TypeConversionFailed(format!("XmlDataFormat::marshal: Body::Bytes is not valid UTF-8 (first invalid byte offset {})", e.valid_up_to()))); } Ok(Body::Bytes(b)) }`.
2. Replace the `Body::Xml(_)` arm with `Body::Xml(s) => Ok(Body::Xml(s))` (passthrough, no parse).
3. Keep `Body::Json`, `Body::Text`, `Body::Empty`, and `Body::Stream` arms byte-identical.
4. Add a doc comment on `marshal` stating: `Body::Xml` and valid-UTF-8 `Body::Bytes` are already-serialized XML and pass through unchanged; invalid UTF-8 fails closed; no content sniffing, no well-formedness re-parse, no re-encoding; UTF-8-only mirrors the REST XML charset gate.

**Tests:** (`cargo test -p camel-processor --lib data_format::xml`)
- `marshal_xml_body_passes_through`: setup `Body::Xml("<user><name>kenny</name></user>".to_string())` → `XmlDataFormat::default().marshal(body)` → result is `Ok(Body::Xml(...))` with the identical string.
- `marshal_valid_utf8_bytes_passes_through`: setup `Body::Bytes(Bytes::from_static(b"<user/>"))` → marshal → result is `Ok(Body::Bytes(b"<user/>"))`; assert the returned body is still the `Body::Bytes` variant and the bytes are identical.
- `marshal_invalid_utf8_bytes_fails_naming_body_bytes`: setup `Body::Bytes(Bytes::from_static(&[0xFF, 0xFE]))` → marshal → assert `Err(CamelError::TypeConversionFailed(msg))` where `msg` contains `"Body::Bytes"`, `"UTF-8"`, and `"0"`.
- `marshal_stream_still_fails_closed`: setup `Body::Stream` built from an empty `BoxStream` (`Ok(Body::Stream(_))` is unreachable) → marshal → `Err(TypeConversionFailed(msg))` and `msg` contains `"stream_cache"`.
- Keep `test_marshal_json_to_text`, `test_marshal_text_noop`, `marshal_empty_is_noop` green unchanged.

**Acceptance:**
- `cargo test -p camel-processor --lib data_format::xml` passes.
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0.
- No new `unwrap()` in non-test code (`cargo xtask lint-unwrap` exits 0).

- [x] 1.1

### Task 1.2: Wrap response-side marshal errors as `ProcessorErrorWithSource`

**Files:**
- `crates/camel-processor/src/marshal.rs` (modified)

**Steps:**
1. Add a private `#[derive(Clone, Copy, Debug, PartialEq, Eq)] enum MarshalSide { Step, Response }` and a `side: MarshalSide` field to `MarshalService<P>`.
2. Keep `MarshalService::new(inner, format)` setting `side: MarshalSide::Step`.
3. Add `pub fn for_response(inner: P, format: Arc<dyn DataFormat>) -> Self` setting `side: MarshalSide::Response`.
4. In `MarshalService::call`, when `format.marshal_in_exchange(...)` returns `Err(e)` and `self.side == MarshalSide::Response`, return `Err(CamelError::ProcessorErrorWithSource(format!("response marshal failed ({})", format_name), Arc::new(e)))` where `format_name` is captured from `self.format.name()` before the call. `MarshalSide::Step` keeps returning `Err(e)` unchanged.
5. Add `const _: fn() = || { fn assert_send_sync_static<T: Send + Sync + 'static>() {} assert_send_sync_static::<CamelError>(); };` next to the wrapping helper so a future non-`Send` error variant fails the build.
6. Document on `for_response`: the wrapper is deliberate so the HTTP finalizer maps the failure to 500 while request-side `UnmarshalService` keeps `TypeConversionFailed` → 400.

**Tests:** (`cargo test -p camel-processor --lib marshal`)
- `for_response_wraps_marshal_error_with_source`: setup `MarshalService::for_response(IdentityProcessor, Arc::new(XmlDataFormat::default()))` with an exchange whose body is `Body::Json(json!({"a": 1, "b": 2}))` (XmlDataFormat returns `TypeConversionFailed` for a multi-root object) → drive `.oneshot(ex)` → assert `Err(CamelError::ProcessorErrorWithSource(msg, source))`, `msg.contains("response marshal failed")`, and `source.downcast_ref::<CamelError>()` is `Some(CamelError::TypeConversionFailed(_))`.
- `new_keeps_type_conversion_failure`: the same `XmlDataFormat` and multi-root body through `MarshalService::new` → assert `Err(CamelError::TypeConversionFailed(_))`.
- `for_response_passes_success_through`: `MarshalService::for_response` with `JsonDataFormat::default()` and `Body::Json(json!({"a": 1}))` → assert the resulting body is `Body::Text("{\"a\":1}")`.

**Acceptance:**
- `cargo test -p camel-processor --lib marshal` passes.
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0.

- [x] 1.2

## camel-dsl (internal response-marshal step)

### Task 1.3: Add the internal `ResponseMarshal` AST step and compile wiring

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/model.rs` (modified)
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/src/compile.rs` (modified)
- `crates/camel-dsl/src/rest.rs` (modified)
- `crates/camel-dsl/src/parity_tests.rs` (modified)

**Steps:**
1. In `route_ast.rs`, add `ResponseMarshal(MarshalStep)` to `RouteDslStep` with the same attributes as `ContentNegotiation`: `#[serde(skip_deserializing)]`, `#[cfg_attr(feature = "schema", schemars(skip))]`, `#[cfg_attr(feature = "schema", ts(skip))]`.
2. In `model.rs`, add `ResponseMarshal(DataFormatDef)` to `DeclarativeStep`, and map it to `DeclarativeStepKind::Marshal` in the `kind()` match.
3. In `yaml.rs`, add a `RouteDslStep::ResponseMarshal(MarshalStep { marshal, config })` arm that mirrors the existing `Marshal` arm exactly but returns `DeclarativeStep::ResponseMarshal(DataFormatDef { format: marshal, schema: None, config })` after the same empty-format rejection.
4. In `compile.rs`, extract the existing marshal format-resolution block (protobuf branch + `builtin_data_format_with_config`, lines in the `DeclarativeStep::Marshal` `schema: None` arm) into `fn resolve_marshal_data_format(format: &str, config: &Option<serde_json::Value>) -> Result<Arc<dyn DataFormat>, CamelError>`. Use it from both `DeclarativeStep::Marshal` and the new `DeclarativeStep::ResponseMarshal` arms; `ResponseMarshal` builds `MarshalService::for_response`, `Marshal` keeps `MarshalService::new`. Both arms reject `schema: Some(_)` with the existing "marshal step does not support 'schema'" error.
5. In `compile.rs`, map `DeclarativeStep::ResponseMarshal(_) => "marshal"` in `declarative_step_name`, and add it to the no-validation group in `validate_step`. The canonical conversion catch-all (`other =>`) already rejects it by name; no canonical support is added.
6. In `rest.rs`, change the response injection at the end of `lower_operation` from `RouteDslStep::Marshal(MarshalStep { .. })` to `RouteDslStep::ResponseMarshal(MarshalStep { marshal: data_format.to_string(), config: None })`. Do not change the user-step or raw-binding branches.
7. In `parity_tests.rs`, add `RouteDslStep::ResponseMarshal(_) => ()` next to the `Marshal` arm.

**Tests:** (`cargo test -p camel-dsl --lib`)
- `response_binding_uses_response_marshal_step`: for each binding (`json`, `form`, `xml`) lower a POST with one `to` step → `step_kind_names` is `["ContentNegotiation", "Unmarshal", "To", "ResponseMarshal", "SetHeader", "SetHeaderIfAbsent"]`.
- Update the existing pinned vectors in `rest.rs` (`v1_route_lowers_with_negotiation_prefix`, `v1_post_step_sequence_pinned`, form/XML sequence tests, raw-mode "no marshal" tests) so `"Marshal"` becomes `"ResponseMarshal"` and `!names.contains("Marshal")` remains true under raw binding.
- `response_marshal_is_not_authorable_from_yaml`: parse a YAML route whose `steps:` contains `response_marshal: { marshal: json }` → `parse_yaml` returns `Err(_)` whose message contains `"did not match any variant"` (same assertion shape as `authoring_cannot_spell_negotiation_step` in `yaml.rs`).
- `authored_marshal_step_stays_marshal`: lower/parse a non-REST route with `steps: [{ marshal: json }]` → the lowered `RouteDslStep` is `Marshal`, not `ResponseMarshal`.
- `compile_response_marshal_json_to_processor`: `compile_declarative_step(DeclarativeStep::ResponseMarshal(DataFormatDef { format: "json", schema: None, config: None }))` → `Ok(BuilderStep::Processor(_))`.
- `compile_response_marshal_rejects_schema`: same with `schema: Some(json!({"type":"object"}))` → `Err(CamelError::RouteError(msg))` containing `"does not support 'schema'"`.
- `compile_authored_marshal_xml_multi_root_is_type_conversion_failed`: `compile_declarative_step(DeclarativeStep::Marshal(DataFormatDef { format: "xml", schema: None, config: None }))` → drive it with `.oneshot(ex)` where `ex.input.body = Body::Json(json!({"a": 1, "b": 2}))` → assert `Err(CamelError::TypeConversionFailed(_))`.
- `response_marshal_error_does_not_match_type_conversion_kind`: build `CamelError::ProcessorErrorWithSource("response marshal failed (xml)".into(), Arc::new(CamelError::TypeConversionFailed("two roots".into())))`; compile an `on_exceptions` policy with `kind: Some("TypeConversionFailed")` via `compile_error_handler` → assert `(policy.matches)(&err) == false`; repeat with `kind: Some("ProcessorErrorWithSource")` → assert `true`.
- `wrapped_response_marshal_error_variant_name_is_processor_error`: same wrapped error → `err.variant_name() == "ProcessorError"` (the `do_try` catch-by-variant alias).
- `every_supported_exception_kind_matches_some_error` and `test_exception_kind_vocabulary_classification_guard` stay green unchanged (no `CamelError` variant added).

**Acceptance:**
- `cargo test -p camel-dsl --lib` passes, including all updated sequence pins.
- `cargo xtask schema --check` exits 0 (the internal variants are schema-skipped; no schema artifact changes).
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.

- [x] 1.3

## camel-component-http (finalizer pin)

### Task 1.4: Pin `ProcessorErrorWithSource` → 500 in the HTTP finalizer

**Files:**
- `crates/components/camel-http/src/lib_tests.rs` (modified)

**Steps:**
1. In the `rc-1dk4` test section next to `other_error_still_maps_to_500`, add a unit test that calls the private `pipeline_error_to_reply` with `CamelError::ProcessorErrorWithSource("response marshal failed (xml)".to_string(), Arc::new(CamelError::TypeConversionFailed("two root keys".to_string())))`.
2. Assert status 500 and that the reply body is exactly `Internal Server Error`; assert the body does not contain `"response marshal failed"` or `"two root keys"`.
3. Leave `pipeline_error_to_reply` production code unchanged: the generic 500 arm already covers the variant.

**Tests:** (`cargo test -p camel-component-http --lib`)
- `processor_error_with_source_maps_to_500_and_hides_detail`: setup as above → `pipeline_error_to_reply(err, "/api/x")` → `reply.status == 500`; `String::from_utf8_lossy(reply_body_bytes) == "Internal Server Error"`.
- Keep `type_conversion_failed_maps_to_400` and `other_error_still_maps_to_500` green unchanged.

**Acceptance:**
- `cargo test -p camel-component-http --lib` passes.
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0.

- [x] 1.4

## camel-dsl HTTP boundary (e2e)

### Task 1.5: HTTP-boundary tests for 500 vs 400 and XML response bodies

**Files:**
- `crates/camel-dsl/tests/rest_marshal_error_http_e2e.rs` (new)
- `crates/camel-dsl/tests/rest_xml_e2e.rs` (modified)

**Steps:**
1. Create the test binary with the same rig as `rest_xml_http_e2e.rs`: `mod common;`, an `acquire_deadline(&SERVER_MUTEX, ...)` guard per test, `common::spawn_test_server(label, path, "POST", 1 MiB, 1 MiB)`, `common::http_roundtrip`, `common::simple_request`, `common::reply_ok`.
2. Define three REST POST YAML constants with a harmless `set_header` user step: `json` (`/json-marshal`, `application/json`), `form` (`/form-marshal`, `application/x-www-form-urlencoded`), `xml` (`/xml-marshal`, `application/xml`).
3. Add `compiled_steps(yaml, op_id) -> Vec<BuilderStep>` (same body as the XML suite) and `replace_user_step(steps, producer)` that swaps index 2 (`set_header`, a user step) for `BuilderStep::Processor(camel_api::OpaqueProcessor(camel_api::BoxProcessor::new(camel_api::ProcessorFn::new(move |mut ex| { let body = producer(); async move { ex.input.body = body; Ok(ex) } }))))`. `producer` is a `Fn() -> Body + Send + Sync + 'static`.
4. Add `serve_pipeline(rx, steps, expected)` that drives each envelope through `compile_step_for_drive` (copy the helper from `rest_xml_http_e2e.rs`; include `DeclarativeSetHeader`/`DeclarativeSetHeaderIfAbsent` via `common::compile_header_step` and the literal `DeclarativeSetBody` branch from `rest_form_e2e.rs`) and sends `Err(e)` to `reply_tx` on failure.
5. Keep the `SetHeader(Content-Type)` and `SetHeaderIfAbsent` steps in the driven sequence so the 500 path is the finalizer's, not a reply-writer default.
6. In `crates/camel-dsl/tests/rest_xml_e2e.rs`, update `xml_multi_root_response_fails` to assert `Err(CamelError::ProcessorErrorWithSource(msg, source))` with `msg.contains("response marshal failed")` and `source.downcast_ref::<CamelError>()` matching `TypeConversionFailed`. This test owns the rest-dsl scenario "malformed response JSON fails closed".

**Tests:** (`cargo test -p camel-dsl --test rest_marshal_error_http_e2e`)
- `e2e_json_response_marshal_failure_returns_500`: json route, user step produces `Body::Stream` built exactly like `crates/camel-processor/src/data_format/json.rs::test_unmarshal_stream_rejected` (`StreamBody { stream: Arc::new(tokio::sync::Mutex::new(Some(Box::pin(futures::stream::empty())))), metadata: StreamMetadata::default() })` → POST `{"a":1}` with `Content-Type: application/json` → status 500; body `Internal Server Error`.
- `e2e_form_response_marshal_failure_returns_500`: form route, user step produces `Body::Json(json!([1, 2]))` (form marshal only accepts a JSON object) → POST `a=1` with form media → status 500.
- `e2e_xml_response_marshal_failure_returns_500`: xml route, user step produces `Body::Bytes(Bytes::from_static(&[0xFF, 0xFE]))` → POST `<root/>` with XML media → status 500.
- `e2e_xml_user_step_body_xml_returns_declared_media`: xml route, user step produces `Body::Xml("<user><name>kenny</name></user>")` → POST `<user><name>kenny</name></user>` → status 201; body is exactly `b"<user><name>kenny</name></user>"`; `content-type` header equals `application/xml`.
- `e2e_xml_user_step_body_xml_uses_vnd_media`: same pipeline with a second XML YAML constant declaring `consumes: application/vnd.acme+xml`, `produces: application/vnd.acme+xml` → POST the same body → status 201 and `content-type` equals `application/vnd.acme+xml` exactly (this discriminates the lowering `SetHeader` from the finalizer's inferred `application/xml`).
- `e2e_json_malformed_request_still_returns_400`: json route, no user-step replacement → POST `{"a=` with `Content-Type: application/json` → status 400; JSON body `error == "bad_request"`.
- `e2e_authored_marshal_in_rest_user_steps_stays_400`: xml route whose `steps:` are `[{ set_body: { value: { a: 1, b: 2 } } }, { marshal: xml }]` → POST `<request/>` → status 400 and JSON body `error == "bad_request"` (the authored step fails with `TypeConversionFailed` before the injected response step).
- Add a module-doc note that the mutator-free path is exercised by `rest_xml_http_e2e.rs` and must stay green.

**Acceptance:**
- `cargo test -p camel-dsl --test rest_marshal_error_http_e2e` passes.
- `cargo test -p camel-dsl --test rest_xml_http_e2e` passes unchanged; `cargo test -p camel-dsl --test rest_xml_e2e` passes with `xml_multi_root_response_fails` updated to the wrapped error.
- `cargo test -p camel-dsl --test rest_form_e2e` and `--test rest_schema_e2e` pass unchanged.
- No test binds a fixed port; the shared `SERVER_MUTEX` guards every server test.

- [x] 1.5

## docs

### Task 1.6: Document XML marshal body acceptance and response-side 500

**Files:**
- `docs/src/data-formats/xml.md` (modified)
- `docs/src/yaml-dsl/top-level-blocks.md` (modified)

**Steps:**
1. In `docs/src/data-formats/xml.md`, update the opening paragraph so marshal is described as "JSON to XML text; `Body::Xml` and valid-UTF-8 `Body::Bytes` pass through unchanged".
2. In "Marshal semantics", add rows/bullets: `Body::Xml` accepted as already-serialized XML (no re-parse); `Body::Bytes` accepted only when valid UTF-8 and passed through unchanged; invalid UTF-8 fails with `CamelError::TypeConversionFailed` naming `Body::Bytes` and the byte offset; no content sniffing, no well-formedness validation.
3. In "REST binding", state that a response-marshal failure is a server error reported as HTTP 500 (request-side XML parse failures stay 400) and that `Body::Xml`/valid-UTF-8 `Body::Bytes` from a user step are returned with the declared `produces` Content-Type.
4. In `docs/src/yaml-dsl/top-level-blocks.md`, in the binding paragraph, replace "then `marshal: xml`" (and the equivalent form/json phrasing where it says the response pipeline injects `marshal: <format>`) with wording that names the internal response-marshal step while keeping authored `marshal:` steps described; state that a failure of the injected step is a 500.

**Tests:**
- `doc-gap scan`: setup the two docs → action `rg -n "Body::Xml|Body::Bytes|response-marshal|500" docs/src/data-formats/xml.md docs/src/yaml-dsl/top-level-blocks.md` → assert each file has at least one hit for the new behavior and no line still claims `Body::Xml` is rejected on marshal.
- `mdbook` gate: `cd docs && mdbook test` → expected pass (no new Rust blocks introduced unless tagged `rust,ignore`; prefer plain fences).

**Acceptance:**
- Both docs contain the new behavior text.
- `cd docs && mdbook test` exits 0.
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-processor -p camel-dsl -p camel-component-http --no-deps` exits 0 (doc links only to public items).

- [x] 1.6

## Verification

### Task 1.7: Targeted gate sweep and park ledger

**Files:**
- `openspec/changes/483-rest-xml-followups/tasks.md` (modified, checkboxes)

**Steps:**
1. Run `cargo fmt --check --all`.
2. Run `cargo clippy -p camel-dsl -p camel-processor -p camel-component-http --all-targets -- -D warnings`.
3. Run `cargo test -p camel-dsl -p camel-processor -p camel-component-http`.
4. Run `cargo xtask schema --check`.
5. Run `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core -p camel-builder -p camel-dsl -p camel-endpoint -p camel-processor -p camel-component-http --no-deps`.
6. Run `cd docs && mdbook test`.
7. Run `openspec validate 483-rest-xml-followups --type change --strict` and `openspec validate --all --strict`.
8. Mark every checkbox `- [x]` and record each command's exit code in the stage-4 park report.

**Tests:**
- The commands above are the executable checks; each must exit 0. A failure is fixed in the owning task, never waived.

**Acceptance:**
- Every command above exits 0.
- `openspec status --change 483-rest-xml-followups --json` reports all tasks done.

- [x] 1.7
