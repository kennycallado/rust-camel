# Tasks: add-rest-raw-binding

## camel-dsl

### Task 1.1: `RouteDslRestBinding` enum and `binding` field on `RouteDslRestOperation`

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/rest.rs` (modified — struct-literal fixes only)
- `crates/camel-dsl/src/openapi.rs` (modified — struct-literal fixes only)

**Steps:**
1. In `route_ast.rs`, above `RouteDslRestOperation`, add:
   `#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]`
   `#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]`
   `#[serde(rename_all = "lowercase")]`
   `pub enum RouteDslRestBinding { Json, Raw }`
2. Add to `RouteDslRestOperation` (after `produces`): `#[serde(default)] pub binding: Option<RouteDslRestBinding>,` with a doc comment: "Optional binding mode: `json` (default) or `raw`. Never inferred from media type."
3. Fix every exhaustive `RouteDslRestOperation` struct literal in the three files by adding `binding: None` — sites: `rest.rs` test helpers `make_rest`, `make_op`, `make_rest_with_base`, and the inline literals in tests `lower_user_set_status_preserved`, `within_block_identical_verb_path_is_duplicate`, `within_block_same_shape_distinct_param_is_ambiguous`, `multiple_ops_same_verb_preserve_declaration_order`; `openapi.rs` test helper `make_rest` and any inline literals in its tests; `route_ast.rs` REST parser tests if they use literals (most use YAML strings — verify with `rg "RouteDslRestOperation \{" crates/camel-dsl/`).
4. Confirm `deny_unknown_fields` still accepts the new field (it is now known) and that `binding: yaml` produces serde's unknown-variant error (covered by test below).

**Tests:** (all in `route_ast.rs` `mod tests`)
- `parse_rest_binding_default_is_none`: setup: YAML REST op without `binding` (reuse `parse_rest_defaults` fixture shape) → action: deserialize with `serde_yml::from_str::<RouteDslRoutes>` → assert: `op.binding.is_none()`.
- `parse_rest_binding_explicit_raw`: setup: YAML op with `binding: raw` → action: deserialize → assert: `op.binding == Some(RouteDslRestBinding::Raw)`.
- `parse_rest_binding_explicit_json`: setup: YAML op with `binding: json` → assert: `op.binding == Some(RouteDslRestBinding::Json)`.
- `parse_rest_binding_invalid_value_fails`: setup: YAML op with `binding: yaml` → action: deserialize → assert: `Err` whose message contains `unknown variant` and mentions `json`/`raw`.

**Acceptance:**
- `cargo test -p camel-dsl --lib route_ast` passes including the 4 new tests.
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.
- `cargo fmt --check` clean.

- [x] 1.1

### Task 1.2: media-declaration helpers in `rest.rs`

**Files:**
- `crates/camel-dsl/src/rest.rs` (modified)

**Steps:**
1. Add private helper `fn is_http_token(s: &str) -> bool` — true iff `s` is non-empty and every char is ASCII alphanumeric (`char::is_ascii_alphanumeric`, NOT Unicode-aware) or one of `` !#$%&'*+-.^_`|~ `` (RFC 9110 tchar — ASCII only).
2. Add private helper `fn split_media_base(media: &str) -> &str` — `media.trim().split(';').next().unwrap_or("")`.
3. Add private helper `fn is_valid_media_declaration(media: &str) -> bool` — trim; split base at `;`; require `split_once('/')` yielding non-empty tokens for type and subtype via `is_http_token`; parameters after the first `;` are ignored (opaque).
4. Add private helper `fn is_json_media_type(media: &str) -> bool` — take `split_media_base`, require valid `type/subtype` shape (reuse the same token checks), then subtype lowercased equals `json` or ends with `+json`.

**Tests:** (in `rest.rs` `mod tests`, plain `#[test]` fns)
- `media_helpers_accept_json_forms`: `is_json_media_type` true for `application/json`, `Application/JSON`, `application/json; charset=utf-8`, `application/problem+json`, `text/json`, `" application/json "`; false for `application/xml`, `json`, `application/jsonx`, ``, `png`, `application/jsoné`.
- `media_helpers_accept_valid_declarations`: `is_valid_media_declaration` true for `image/png`, `text/plain; charset=utf-8`, `application/octet-stream`, `" image/png "`; false for `png`, `/png`, `png/`, `image / png`, `im age/png`, `image/é`, ``, `"   "`.

**Acceptance:**
- `cargo test -p camel-dsl --lib media_helpers` passes.
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.

- [x] 1.2

### Task 1.3: binding-aware json-mode gate + trimmed `produces` Content-Type

**Files:**
- `crates/camel-dsl/src/rest.rs` (modified)

**Steps:**
1. In `lower_operation`, replace the two exact-string checks (current lines 223-237) with a match on `op.binding.unwrap_or(RouteDslRestBinding::Json)`:
   - `Json` arm: for `consumes` then `produces`, if `!is_json_media_type(&op.consumes)` (respectively `&op.produces`) return `CamelError::RouteError` with message exactly of the form: `rest operation '{op_label}': binding 'json' requires a JSON media type for '{consumes|produces}' (got '{value}') — declare 'binding: raw' to use non-JSON media` where `{op_label}` is `operation_id` or the verb fallback and `{value}` is the trimmed declared string.
2. In the Content-Type step injection (current lines 313-324), change the value from the literal `"application/json"` to `serde_json::Value::String(op.produces.trim().to_string())`.
3. Leave everything else in the json path unchanged (unmarshal injection, marshal injection, status-if-absent).

**Tests:** (in `rest.rs` `mod tests`; add a local test helper `fn step_kind_names(steps: &[RouteDslStep]) -> Vec<&'static str>` mapping each step to its variant name — `"To"`, `"Unmarshal"`, `"Marshal"`, `"SetHeader"`, `"SetHeaderIfAbsent"`, etc. — since `RouteDslStep` has no `PartialEq`; pair kind-name vectors with targeted positional field assertions)
- `json_mode_accepts_parameterized_json_media`: setup: `make_rest` POST with `consumes`/`produces` = `application/json; charset=utf-8` → action: `lower_all_rest_to_routes` → assert: Ok; Content-Type step value is the trimmed string `application/json; charset=utf-8`.
- `json_mode_accepts_problem_plus_json`: same with `produces: application/problem+json` → assert: Ok.
- `json_mode_rejects_xml_with_precise_error`: setup: op with `consumes: application/xml`, no binding → action: lower → assert: Err message contains the op label, `consumes`, `application/xml`, and `raw`.
- `json_mode_rejects_non_json_produces`: setup: `produces: text/plain`, no binding → assert: Err message contains `produces` and `text/plain`.
- `v1_route_step_sequence_is_byte_identical`: setup: `make_rest("getUser", "get", "/{id}", "bean:svc")` (no binding, json media; v1 pushes `Marshal(json)` unconditionally for every verb — only unmarshal is verb-guarded) → action: lower → assert: `from` == `http://0.0.0.0:8080/users/{id}?httpMethod=GET`, route id == `getUser`, `step_kind_names` == `["To", "Marshal", "SetHeader", "SetHeaderIfAbsent"]`, and positional field checks: step 2 sets `Content-Type` = `application/json`, step 3 sets `CamelHttpResponseCode` = 200 if-absent.
- `v1_post_step_sequence_pinned`: setup: `make_rest("createUser", "post", "/", "bean:create")` with `request_schema: Some(json!({"type":"object"}))` → action: lower → assert: `step_kind_names` == `["Unmarshal", "To", "Marshal", "SetHeader", "SetHeaderIfAbsent"]` with unmarshal schema attached, `Content-Type` = `application/json`, status 201 if-absent (spec Req 3 "POST step sequence").
- `json_mode_get_with_schema_has_no_unmarshal`: setup: `make_rest("getUser", "get", "/{id}", "bean:svc")` with `request_schema: Some(json!({"type":"object"}))` → action: lower → assert: no step of kind `"Unmarshal"` (spec Req 3 "GET receives no unmarshal").
- `v1_existing_pins_still_green`: run `cargo test -p camel-dsl --lib rest::` and `cargo test -p camel-dsl --test rest_schema_e2e` — all pre-existing tests pass without any assertion edits (this is a verification step, not a new test fn).

**Acceptance:**
- All new tests pass; pre-existing REST unit + `rest_schema_e2e` integration tests pass unmodified.
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.

- [x] 1.3

### Task 1.4: raw-mode lowering — no auto binding steps, schema rejection, media validation

**Files:**
- `crates/camel-dsl/src/rest.rs` (modified)

**Steps:**
1. In `lower_operation`'s binding match, add the `Raw` arm:
   a. If `op.request_schema.is_some()` return `RouteError`: `rest operation '{op_label}': 'binding: raw' does not support 'request_schema' — schema validation requires binding 'json'`.
   b. If `op.response.as_ref().and_then(|r| r.schema.as_ref()).is_some()` return `RouteError`: `rest operation '{op_label}': 'binding: raw' does not support 'response.schema' — schema validation requires binding 'json'`.
   c. For `consumes` then `produces`: if `!is_valid_media_declaration(&op.consumes)` (respectively `&op.produces`) return `RouteError`: `rest operation '{op_label}': binding 'raw' requires a valid media declaration for '{consumes|produces}' (got '{value}') — expected 'type/subtype' with RFC 9110 token characters`.
2. In the step-injection section, skip the `Unmarshal` push and the `Marshal` push when binding is `Raw`; keep the user steps, the `SetHeader(Content-Type: op.produces.trim())` push, and the final `SetHeaderIfAbsent` status push unchanged.

**Tests:** (in `rest.rs` `mod tests`)
- `raw_mode_step_sequence_exact`: setup: `make_rest`-style GET op with `binding: Some(Raw)`, `produces: image/png`, `to: bean:svc` → action: lower → assert: `step_kind_names` == `["To", "SetHeader", "SetHeaderIfAbsent"]` (raw pushes NO marshal — v1's unconditional marshal is exactly what raw omits), positional checks: step 1 sets `Content-Type` = `image/png`, step 2 sets 200 if-absent; assert: no step of kind `"Unmarshal"` or `"Marshal"`.
- `raw_mode_post_still_defaults_201_and_has_no_unmarshal`: setup: POST op raw with `to` → assert: no Unmarshal/Marshal steps; last step SetHeaderIfAbsent 201.
- `raw_mode_rejects_request_schema`: setup: raw POST with `request_schema: Some(json!({"type":"object"}))` → assert: Err message contains op label and `request_schema`.
- `raw_mode_rejects_response_schema`: setup: raw GET with `response: Some(RouteDslRestResponse { description: None, schema: Some(json!({"type":"object"})), headers: BTreeMap::new() })` → assert: Err message contains `response.schema`.
- `raw_mode_accepts_header_only_response`: setup: raw GET with `response` carrying only `headers` (e.g. `{"X-Rate-Limit": {"type":"integer"}}`), `schema: None` → assert: Ok.
- `raw_mode_rejects_missing_separator`: `produces: png` → Err message contains `png`.
- `raw_mode_rejects_empty_subtype`: `produces: /png` → Err.
- `raw_mode_rejects_whitespace_inside_base`: `produces: "image / png"` → Err.
- `raw_mode_trims_outer_whitespace`: `produces: " image/png "` → Ok AND Content-Type step value is exactly `image/png`.
- `raw_mode_accepts_parameterized_media`: `produces: text/plain; charset=utf-8` → Ok (parameters are opaque); Content-Type step value is the full trimmed string `text/plain; charset=utf-8`.
- `raw_mode_accepts_non_json_media`: `consumes: application/octet-stream`, `produces: image/png` → Ok.

**Acceptance:**
- `cargo test -p camel-dsl --lib rest::` passes with all new tests.
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.

- [x] 1.4

### Task 1.5: compile-path wiring e2e + YAML/JSON parity (new integration test file)

**Files:**
- `crates/camel-dsl/tests/rest_raw_e2e.rs` (new)
- `crates/camel-dsl/Cargo.toml` (modified)

**Steps:**
1. Write integration tests using the harness pattern of `crates/camel-dsl/tests/rest_schema_e2e.rs`: `camel_dsl::yaml::parse_yaml` (public crate paths, NOT `crate::` — this is an integration test) → `routes[0].steps()` → extract a `BuilderStep::Processor(p)` and drive `p` with `tower::ServiceExt::oneshot` (only the inner processor implements `Service`; `BuilderStep` itself does not).
2. Add `futures` and `bytes` to `crates/camel-dsl/Cargo.toml` `[dev-dependencies]`, each using the workspace version from the root `Cargo.toml` `[workspace.dependencies]` table (neither is currently a direct dev-dependency) — needed to construct a bounded `Body::Stream` input.
3. Test the compile-level shape of a raw route and the parity of YAML vs JSON authoring.

**Tests:**
- `raw_post_compiles_to_exactly_three_steps`: setup: YAML rest block, POST, `binding: raw`, `consumes/produces: application/octet-stream`, one user `to: direct:rawSink` step (spec's "exactly one user `to` step") → action: `camel_dsl::yaml::parse_yaml` → assert: one route; `routes[0].steps().len() == 3`; additionally run `camel_dsl::yaml::parse_yaml_to_declarative` on the same YAML and `assert_eq!` the lowered route's declarative step vector against the literal `[To(direct:rawSink), SetHeader(Content-Type: application/octet-stream), SetHeaderIfAbsent(201)]` (`DeclarativeStep` derives `PartialEq`).
- `raw_pipeline_preserves_stream_body`: setup: a second raw POST route whose single user step is `steps: [{set_header: {key: X-Trace, value: t1}}]` (a pure processor, executable in isolation like rest_schema_e2e's unmarshal); parse + compile it; extract the `BuilderStep::Processor` for that first user step; build an `Exchange` whose `Message` body is a `Body::Stream` constructed from a bounded one-item stream (e.g. `futures::stream::once` yielding a bytes item, wrapped in the `StreamBody` type `camel_api::body::Body::Stream` holds — mirror the construction used in camel-api/camel-http stream tests) → action: `.oneshot(exchange)` on the extracted user-step processor, then on the two injected header-step processors in order → assert: every call returns `Ok`, the final body still matches `Body::Stream(_)`, the `Content-Type` header equals `application/octet-stream`, and `CamelHttpResponseCode` equals 201. Combined with the 3-entry count above, no processor ahead of the user step can have materialized the stream.
- `json_authored_raw_lowers_like_yaml`: setup: the same rest block (with `to: direct:rawSink`) authored as a JSON document (object with a single `rest` array; the operation object carries `method`, `path`, `operation_id`, `binding: "raw"`, `consumes`, `produces`, and `to`) parsed via the declarative entries — `camel_dsl::yaml::parse_yaml_to_declarative` (yaml.rs, ~line 105) for YAML and `camel_dsl::json::parse_json_to_declarative` (json.rs, ~line 26) for JSON — because `DeclarativeStep` derives `PartialEq` while compiled `BuilderStep` does not → action: lower both → assert: same route id, same `from` URI, `assert_eq!` on the full declarative step vectors of the lowered routes.
- `raw_yaml_rejects_schema_at_parse`: setup: YAML raw op with `request_schema` → action: `camel_dsl::yaml::parse_yaml` → assert: Err (RouteError) containing `request_schema` — proves the gate fires on the full authoring path, not only the unit-level lowering.

**Acceptance:**
- `cargo test -p camel-dsl --test rest_raw_e2e` passes.
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.

- [x] 1.5

### Task 1.6: OpenAPI raw binary schemas

**Files:**
- `crates/camel-dsl/src/openapi.rs` (modified)
- `crates/camel-dsl/tests/openapi_integration.rs` (modified)

**Steps:**
1. In `build_operation`, branch on `op.binding.unwrap_or(RouteDslRestBinding::Json)`:
   - Non-204 response, `Raw`: response content under `op.produces.trim()` key uses `json!({"type": "string", "format": "binary"})`; do NOT push the no-response-schema weak-stub warning; if the op's `response.schema` is present (`op.response.as_ref().and_then(|r| r.schema.clone()).is_some()`), push warning `operation '{op_label}' — binding 'raw' rejects 'response.schema' at route load; emitting binary schema`.
   - 204 response, `Raw`: keep the existing contentless 204 branch and its existing 204-ignores-schema warning exactly; do not stack the raw warning.
   - Request body (body verbs), `Raw`: content under `op.consumes.trim()` key uses the binary schema; no weak-stub warning; if `op.request_schema` is present, push warning `operation '{op_label}' — binding 'raw' rejects 'request_schema' at route load; emitting binary schema`.
   - `Json` arm: unchanged behavior (schemas or weak stubs + warnings), but key content by `op.produces.trim()`/`op.consumes.trim()` for consistency with the trimming rule.
2. Update `openapi_integration.rs` with raw fixtures.

**Tests:** (unit in `openapi.rs` `mod tests` + integration)
- `raw_response_binary_schema`: setup: rest op `binding: raw`, `produces: application/octet-stream`, no response schema → action: `generate_openapi` → assert: success response `content["application/octet-stream"].schema` == `{"type":"string","format":"binary"}` and `result.warnings` contains no weak-stub entry for this op.
- `raw_request_body_binary_schema`: setup: POST `binding: raw`, `consumes: text/plain` → assert: `requestBody.content["text/plain"].schema` binary.
- `raw_with_response_schema_warns`: setup: raw + `response.schema` → assert: warnings contain the raw-rejects warning; response still binary.
- `raw_204_stays_contentless`: setup: raw, `success_status: 204`, `produces: application/octet-stream` → assert: `responses["204"]` has no `content` key.
- `raw_204_with_schema_keeps_single_warning`: setup: raw, 204, `response.schema` → assert: warnings contain the 204-ignores-schema warning and NOT the raw-rejects warning.
- `raw_openapi_keys_trim_media`: setup: raw GET with `produces: " image/png "` (outer whitespace) → assert: response content is keyed by exactly `image/png` (no leading/trailing space in the key) with the binary schema (spec Req 4 trimming scenario's OpenAPI half).
- (integration, `openapi_integration.rs`) `raw_operation_generates_binary_content`: setup: extend the REST_CRUD-style fixture with a raw GET (`binding: raw`, `produces: application/octet-stream`) → assert: path content binary + document still valid structure (info/paths keys present).

**Acceptance:**
- `cargo test -p camel-dsl --lib openapi` and `cargo test -p camel-dsl --test openapi_integration` pass.
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.

- [x] 1.6

## Docs

### Task 1.7: documentation sync + gate sweep

**Files:**
- `CONTEXT-MAP.md` (modified)
- `crates/camel-dsl/CONTEXT.md` (modified)
- `docs/src/yaml-dsl/step-verbs.md` (modified)
- `docs/src/concepts/glossary.md` (modified)

**Steps:**
1. `CONTEXT-MAP.md` Key Terms `REST DSL` entry: replace "with JSON binding, path templates, and optional schema validation" with binding-mode phrasing: operations default to JSON binding (auto unmarshal/marshal + JSON Schema validation); explicit `binding: raw` accepts non-JSON media, injects no automatic data-format steps, sets the declared `produces` as response Content-Type, and leaves the request as `Body::Stream`. Keep the existing spec pointer line.
2. `crates/camel-dsl/CONTEXT.md` REST DSL entry (current text at ~line 204): replace the two sentences starting "REST v1 supports only `application/json` for both" and ending "A future v2 may lift the restriction." with the same binding-mode description (default json; json mode accepts JSON-essence media including parameters and `+json` suffixes; raw mode per above; schema fields rejected in raw).
3. `docs/src/yaml-dsl/step-verbs.md` REST section: add a `binding` row to the operation field table (`| binding | string | no | json | Binding mode: json or raw |`) and rewrite the JSON-only paragraph (~line 862) to describe both modes with a short raw example snippet (3-6 lines YAML).
4. `docs/src/concepts/glossary.md` REST DSL entry: mirror the CONTEXT-MAP phrasing.
5. Run the gate sweep and fix anything these changes trip.

**Tests:** (verification, not new test fns)
- `docs_alignment_scan`: `rg -n "supports only .application/json|JSON-only" CONTEXT-MAP.md crates/camel-dsl/CONTEXT.md docs/src/` → zero hits that describe the REST DSL as JSON-only (a hit inside an ADR/historical doc is fine; live glossary/CONTEXT/dsl-guide hits are not).

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- `cargo xtask schema --check` exits 0.
- `cargo fmt --check` and `cargo clippy -p camel-dsl --all-targets -- -D warnings` exit 0.
- `cargo test -p camel-dsl` (lib + all integration tests) passes.

- [x] 1.7
