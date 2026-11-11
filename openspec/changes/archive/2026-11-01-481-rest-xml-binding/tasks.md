# Implementation Plan: 481-rest-xml-binding

Single delivery phase (see `design.md` §Phases). All tasks run in the mission
worktree with `CARGO_TARGET_DIR=<worktree>/target`, `TMPDIR=/home/shared/tmp`.
TDD order per task: write the listed tests first, watch them fail, then make
them pass. Do not invent extra test design.

---

## Task 1.1 — XML media predicate

**Files**
- `crates/camel-dsl/src/media.rs` (modified)

**Steps**
1. Add `pub(crate) fn is_xml_media_type(media: &str) -> bool` beside
   `is_form_media_type`, documented with the accepted family.
2. Body: take `split_media_base(media)`, `split_once('/')`; reject unless both
   parts pass `valid_header_token` **untrimmed**, so `image / svg+xml` fails
   (only the outer media string is trimmed by `split_media_base`). Lowercase the
   subtype with `to_ascii_lowercase`; accept when it equals `xml` or when it
   ends with `+xml` and has at least one character before the suffix. Then take
   `media.split(';').skip(1)`; allow at most one entry and require it, after
   `trim()`, to `eq_ignore_ascii_case("charset=utf-8")`.
3. Do not modify `is_json_media_type`, `is_form_media_type`, or
   `is_valid_media_declaration`.

**Tests** (`crates/camel-dsl/src/media.rs`, `mod tests`)
- `is_xml_media_type_accepts_xml_family` — arrange: table of
  `application/xml`, `text/xml`, `application/atom+xml`, `image/svg+xml`,
  `APPLICATION/XML`, `application/xml; charset=utf-8`,
  `text/xml;charset=UTF-8`; act: call the predicate; assert: every entry true.
- `is_xml_media_type_rejects_non_xml_media` — entries `application/json`,
  `text/plain`, `application/xmlx`, `xml`, `application/+xml`,
  `application/x-www-form-urlencoded`, `text/xml-plus`; assert: every entry
  false.
- `is_xml_media_type_rejects_malformed_parameters` — entries
  `application/xml;`, `application/xml; charset=iso-8859-1`,
  `application/xml; charset="utf-8"`, `application/xml; charset=utf-8; foo=bar`,
  `image / svg+xml`; assert: every entry false.
- Command: `cargo test -p camel-dsl --lib media::`
- Expected before the predicate exists: compile failure (function absent), then
  red assertions; after: all green.

**Acceptance**
- `cargo test -p camel-dsl --lib media::` exits 0.
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.

- [x] 1.1

---

## Task 1.2 — `Xml` binding variant, validation arm, lowering

**Files**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/rest.rs` (modified)
- `schemas/dsl/route-schema.json` (modified by `cargo xtask schema`)
- `schemas/ts/RouteDslRestBinding.ts` (modified by `cargo xtask schema`)
- `schemas/ts/RouteDslRestOperation.ts` (modified by `cargo xtask schema`)

**Steps**
1. In `RouteDslRestBinding` add `Xml` after `Form`, with the doc comment
   `/// XML structured binding (XML body -> JSON -> XML body).` Update the enum
   doc comment and the `binding` field doc comment to name the full closed set
   (`json`, `raw`, `form`, `xml`) instead of the old "json or raw" text.
2. In `lower_operation`'s validation `match`, add the `Xml` arm mirroring the
   `Form` arm: `!is_xml_media_type(&op.consumes)` → `CamelError::RouteError`
   with message `rest operation '{op_label}': binding 'xml' requires an XML
   media type (application/xml, text/xml, or */*+xml) for 'consumes' (got
   '{value}')`; same for `produces`. Parameters are not validated beyond the
   predicate.
3. In the `data_format` selector add `RouteDslRestBinding::Xml => "xml"`.
4. Extend the existing `use crate::media::{is_form_media_type, is_json_media_type,
   is_valid_media_declaration};` import with `is_xml_media_type` (the other
   three names stay).
5. Leave every `json`/`form`/`raw` arm, string, and step position untouched.
6. Run `cargo check -p camel-dsl --all-targets` and add `Xml` to every
   non-wildcard `match` on `RouteDslRestBinding` outside the lowering arm, using
   the same arm as `Json` (openapi.rs currently compares with `==`, so it needs
   no arm; fix any exhaustive match the compiler reports, and do not change
   `generate_openapi` behavior).
7. Regenerate the derived artifacts with `cargo xtask schema` and commit the
   three files listed above (the enum flows into the DSL JSON schema and the
   exported TS types).

**Tests** (`crates/camel-dsl/src/rest.rs`, `mod tests`, after the form battery)
- `xml_binding_exact_media_loads` — arrange: `make_rest` POST, binding `Xml`,
  consumes/produces `application/xml`; act: `lower_all_rest_to_routes`;
  assert: `is_ok()`.
- `xml_binding_text_xml_loads` — consumes/produces `text/xml`; assert:
  `is_ok()`.
- `xml_binding_structured_suffix_loads` — consumes/produces
  `application/atom+xml`; assert: `is_ok()`.
- `xml_binding_charset_utf8_accepted` — consumes
  `application/xml; charset=utf-8`, produces `application/xml`; assert:
  `is_ok()`.
- `xml_binding_non_xml_media_fails` — table-driven over
  `application/json` and `text/plain` in `consumes`: assert `Err(RouteError)`
  whose message contains the operation id, `consumes`, the offending value, and
  `application/xml`.
- `xml_binding_non_utf8_charset_fails` — produces
  `application/xml; charset=iso-8859-1`; assert: `Err(RouteError)` message
  contains the operation id and the offending value.
- `xml_binding_empty_prefix_suffix_fails` — consumes `application/+xml`;
  assert: `Err(RouteError)` message contains the operation id and the offending
  value.
- `xml_binding_allows_request_and_response_schema` — POST with
  `request_schema` and `response.schema` set; assert: `is_ok()` and the
  `Unmarshal` step carries the schema.
- `xml_post_step_sequence` — POST with `to`; use `step_kind_names`; assert the
  names are exactly the same sequence as `form_post_step_sequence` and that
  the `Unmarshal` step has `unmarshal == "xml"` and the `Marshal` step has
  `marshal == "xml"`.
- `xml_get_no_unmarshal` — GET with `request_schema`; assert: the first step is
  `ContentNegotiation` and no `Unmarshal` step exists, while `Marshal` with
  `marshal == "xml"` is present.
- `xml_request_schema_carried` — POST with `request_schema`; assert: the
  `Unmarshal` step's `schema` equals the authored value.

**Tests** (`crates/camel-dsl/src/route_ast.rs`, `mod tests`, beside
`parse_rest_binding_invalid_value_fails`)
- `parse_rest_binding_xml` — arrange: the same YAML fixture used by the
  neighbouring binding tests with `binding: xml`; act: parse; assert:
  `op.binding == Some(RouteDslRestBinding::Xml)`.
- Existing `parse_rest_binding_invalid_value_fails` must stay green unchanged.

- Command: `cargo test -p camel-dsl --lib rest::` and
  `cargo test -p camel-dsl --lib route_ast::`
- Expected before implementation: the new tests fail to compile (variant
  absent); after: all green.

**Acceptance**
- `cargo test -p camel-dsl --lib rest:: route_ast::` (both filters run) exits 0.
- `cargo test -p camel-dsl --lib` exits 0 — no `json`/`raw`/`form` regression.
- `cargo check -p camel-dsl --all-targets` exits 0.
- `cargo xtask schema --check` exits 0 (regenerated artifacts committed).
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.

- [x] 1.2

---

## Task 1.3 — OpenAPI parity test (no generator change)

**Files**
- `crates/camel-dsl/src/openapi.rs` (modified: tests only)

**Steps**
1. Add a test beside `form_binding_openapi_authored_schema`. Do not change
   `generate_openapi` — the `raw` boolean already routes `Xml` through the
   authored-schema/weak-stub path.

**Tests**
- `xml_binding_openapi_authored_schema` — arrange: POST op, binding `Xml`,
  consumes/produces `application/xml`, authored `request_schema` and
  `response.schema` `{ "type": "object" }`; act: `generate_openapi`; assert:
  `requestBody.content["application/xml"].schema` and
  `responses["201"].content["application/xml"].schema` equal the authored
  object, and no warning mentions the operation id.
- `xml_binding_openapi_weak_stub_warns` — same op without schemas; assert:
  both content schemas equal `{ "type": "object" }` and `warnings` contains
  the weak-stub message naming the operation.
- Command: `cargo test -p camel-dsl --lib openapi::`
- Expected: both fail if `Xml` were mishandled; green with no generator edit.

**Acceptance**
- `cargo test -p camel-dsl --lib openapi::` exits 0.
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.

- [x] 1.3

---

## Task 1.4 — XML format empty-body arms + behavior pins

**Files**
- `crates/camel-processor/src/data_format/xml.rs` (modified)

**Steps**
1. In `impl DataFormat for XmlDataFormat::marshal`, add
   `Body::Empty => Ok(Body::Empty),` before the catch-all, with a comment
   stating the parity rationale (a REST operation with no body, e.g. 204,
   must not fail; matches `JsonDataFormat`).
2. Leave every other arm and `XmlConfig` untouched. `unmarshal` keeps rejecting
   `Body::Empty`.
3. Before adding tests, `grep -n "fn .*"` the existing `mod tests` and rename
   any new test that would collide. Delete the existing
   `test_marshal_empty_returns_error` (it pins the old error behavior and is
   superseded by `marshal_empty_is_noop`); touch no other existing test.

**Tests** (`crates/camel-processor/src/data_format/xml.rs`, `mod tests`)
- `marshal_empty_is_noop` — arrange: `XmlDataFormat::default()`; act:
  `marshal(Body::Empty)`; assert: `Ok(Body::Empty)`.
- `unmarshal_empty_fails_closed` — act: `unmarshal(Body::Empty)`; assert:
  `Err(CamelError::TypeConversionFailed)` whose message contains
  `XmlDataFormat::unmarshal`.
- `unmarshal_truncated_fails` — act: `unmarshal(Body::Text("<user><name>"))`;
  assert: `Err(TypeConversionFailed)` whose message contains
  `unexpected end of XML input` (this pins the string Task 1.6 asserts).
- `marshal_output_has_no_declaration` — act: `marshal(Body::Json(json!({"user":
  {"name": "kenny"}})))`; assert: result is exactly
  `Body::Text("<user><name>kenny</name></user>".to_string())` — no `<?xml`
  declaration, `Body::Text` not `Bytes`.
- `marshal_multi_key_object_fails` — act: `marshal(Body::Json(json!({"a": 1,
  "b": 2})))`; assert: `Err(TypeConversionFailed)` with `exactly one root
  element` in the message.
- `marshal_top_level_array_fails` — act: `marshal(Body::Json(json!([{"a": 1}])))`;
  assert: `Err(TypeConversionFailed)`.
- `marshal_top_level_scalar_fails` — act: `marshal(Body::Json(json!("hello")))`;
  assert: `Err(TypeConversionFailed)`.
- `unmarshal_leaf_text_is_string` — act: `unmarshal(Body::Text("<n>42</n>"))`;
  assert: result equals `Body::Json(json!({"n": "42"}))`.
- `unmarshal_drops_xmlns_declarations` — act: `unmarshal(Body::Text(r#"<r
  xmlns="urn:x" xmlns:p="urn:p"><child>v</child></r>"#))`; assert: result
  equals `Body::Json(json!({"r": {"child": "v"}}))` (no `xmlns`/`@xmlns` key).
- `unmarshal_empty_element_is_null` — act:
  `unmarshal(Body::Text("<r><empty/></r>"))`; assert: result equals
  `Body::Json(json!({"r": {"empty": null}}))`.
- `unmarshal_doctype_entity_ref_fails_closed` — act:
  `unmarshal(Body::Text("<!DOCTYPE foo [<!ENTITY x \"BOOM\">]><foo>&x;</foo>"))`;
  assert: `Err(TypeConversionFailed)` whose message contains `unrecognized
  entity` (the declared replacement text is never expanded).
- `unmarshal_doctype_without_entity_ok` — act:
  `unmarshal(Body::Text("<!DOCTYPE foo><foo>v</foo>"))`; assert: equals
  `Body::Json(json!({"foo": "v"}))` (no DTD-derived key).
- Command: `cargo test -p camel-processor --lib data_format::xml::`
- Expected before implementation: `marshal_empty_is_noop` fails (error); after:
  all green.

**Acceptance**
- `cargo test -p camel-processor --lib data_format::xml::` exits 0.
- `cargo test -p camel-processor --lib` exits 0.
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0.

- [x] 1.4

---

## Task 1.5 — Executed step-level XML pipeline e2e

**Files**
- `crates/camel-dsl/tests/rest_xml_e2e.rs` (new)

**Steps**
1. Model the file on `crates/camel-dsl/tests/rest_form_e2e.rs`: `mod common;`,
   a `REST_XML_YAML` document with a POST op (`binding: xml`, consumes and
   produces `application/xml`) and a step counter, plus `compiled_steps`,
   `compile_step_for_drive`, `drive_steps`, and an `xml_request` builder that
   installs `Body::Bytes` (the inbound HTTP variant) and a `Content-Type:
   application/xml` header.
2. `drive_steps` runs the compiled steps in order and applies an
   `Exchange -> Exchange` mutator immediately before the `Marshal` step (after
   the user steps). The YAML op carries one harmless `set_header` step so
   lowering is valid (an operation with neither `to` nor `steps` is rejected);
   bodies that YAML cannot express (`Body::Empty`, an exact `Body::Json(value)`)
   and body observations come from that mutator.
3. Drive the compiled steps in declared order exactly as the runtime would,
   counting user-step executions when a test asserts the step never ran.

**Tests**
- `xml_request_unmarshals_to_json_body` — act: drive a POST with
  `<user><name>kenny</name><n>42</n></user>`; assert at the point after the
  `Unmarshal` step (before `Marshal`): body equals
  `json!({"user": {"name": "kenny", "n": "42"}})` (leaf is a string).
- `xml_response_marshals_from_json` — user step leaves
  `json!({"user": {"@id": "7", "name": "kenny"}})`; after the full drive:
  body is `Body::Text` equal to `<user id="7"><name>kenny</name></user>` and the
  `Content-Type` header equals `application/xml`.
- `xml_malformed_request_fails_before_user_step` — body `<user><name>`; assert:
  the drive returns `Err(CamelError::TypeConversionFailed)` and a step
  counter proves the user step never ran.
- `xml_empty_request_body_fails_closed` — body `Body::Empty`; assert:
  `Err(TypeConversionFailed)` and the user step never ran.
- `xml_doctype_entity_fails_closed` — body
  `<!DOCTYPE foo [<!ENTITY x "BOOM">]><foo>&x;</foo>`; assert:
  `Err(TypeConversionFailed)` and the user step never ran (the entity is never
  expanded).
- `xml_schema_valid_accepts_projection` — op with `request_schema`
  `{"type":"object","properties":{"user":{"type":"object"}}}`; assert: drive
  succeeds.
- `xml_schema_invalid_rejects_projection` — op with `request_schema`
  `{"type":"object","required":["missing"]}`; assert:
  `Err(CamelError::ValidationError)`.
- `xml_multi_root_response_fails` — user step leaves
  `json!({"a": 1, "b": 2})`; assert: `Err(TypeConversionFailed)` at marshal.
- `xml_response_schema_is_metadata_only` — op with
  `response.schema {"type":"object","required":["must"]}` and a user step
  leaving `json!({"user": {"name": "kenny"}})` (non-conforming); assert: the
  drive succeeds and the body is XML — no runtime response validation runs.
- `xml_empty_response_body_stays_empty` — user step leaves `Body::Empty`;
  assert: drive succeeds, body is `Body::Empty`, `Content-Type` header equals
  `application/xml`, and `CamelHttpResponseCode` is absent-then-defaulted by the
  final `SetHeaderIfAbsent` (POST → 201).
- Command: `cargo test -p camel-dsl --test rest_xml_e2e`
- Expected before implementation: the binary compiles but the pipeline cannot
  lower `binding: xml`, so every test fails; after: all green.

**Acceptance**
- `cargo test -p camel-dsl --test rest_xml_e2e` exits 0.
- The new file does not modify `rest_form_e2e.rs`, `rest_json` suites, or
  `tests/common/mod.rs`.
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.

- [x] 1.5

---

## Task 1.6 — HTTP-boundary 400 for malformed XML

**Files**
- `crates/camel-dsl/tests/rest_xml_http_e2e.rs` (new)

**Steps**
1. Mirror `crates/camel-dsl/tests/rest_negotiation_e2e.rs` Part 2: `mod
   common;`, `SERVER_MUTEX` via `acquire_deadline`, a local
   `spawn_xml_server(path, method)` built from `common::spawn_test_server`
   (same shape as `spawn_negotiation_server`), and a `serve_pipeline` helper
   shaped like `serve_gate`: receive each `ExchangeEnvelope` under a 10s
   `tokio::time::timeout`, drive the compiled steps in order with
   `oneshot`, then `reply_ok(envelope)` on `Ok` or
   `envelope.reply_tx.take().map(|tx| tx.send(Err(e)))` on `Err` — the
   HttpConsumer's own `pipeline_error_to_reply` then renders the HTTP reply.
2. Read what `crates/camel-dsl/tests/common/mod.rs` exposes
   (`spawn_test_server`, `http_roundtrip`, `simple_request`, `reply_ok`) and
   reuse it; do not edit `common/mod.rs`.

**Tests**
- `e2e_malformed_xml_returns_400` — arrange: boot the server for the POST
  `/xml/post` route; act: `simple_request("POST", "/xml/post", &[("Content-
  Type", "application/xml")], b"<user><name>")` while the serve task drives the
  compiled steps and replies with the typed error; assert: `resp.status == 400`,
  the body parses as JSON with `error == "bad_request"`, and the message
  contains `unexpected end of XML input` (pinned by Task 1.4).
- `e2e_valid_xml_returns_201_xml` — act: the same rig with
  `<user><name>kenny</name></user>`, and `serve_pipeline` applies the same
  `Exchange -> Exchange` mutator as Task 1.5 immediately before the `Marshal`
  step, leaving
  `json!({"user": {"name": "kenny"}})`; assert: `resp.status == 201` (POST
  default status from the final `SetHeaderIfAbsent`) and
  `resp.body == b"<user><name>kenny</name></user>"` with
  `Content-Type: application/xml`.
- Command: `cargo test -p camel-dsl --test rest_xml_http_e2e`
- Expected before implementation: tests fail (binding cannot lower); after:
  green.

**Acceptance**
- `cargo test -p camel-dsl --test rest_xml_http_e2e` exits 0.
- No fixed sleeps; readiness uses the existing connect-retry helper.
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.

- [x] 1.6

---

## Task 1.7 — Docs and context map

**Files**
- `docs/src/yaml-dsl/top-level-blocks.md` (modified)
- `docs/src/data-formats/index.md` (modified)
- `docs/src/data-formats/xml.md` (new)
- `CONTEXT-MAP.md` (modified, REST DSL bullet near line 187)

**Steps**
1. In `top-level-blocks.md`: change the `binding` table row to
   `Binding mode: `json`, `raw`, `form`, or `xml``. Add XML prose beside the form
   prose: accepted media (`application/xml`, `text/xml`, `type/*+xml`, at most
   one `charset=utf-8`), the XML → JSON → schema-validate request order, the
   JSON → schema-metadata → XML response order, and the JSON projection note
   (leaf text is a string; schemas describe the projection, not the wire XML).
   Add a short YAML example block that literally contains `binding: xml` next
   to the existing binding examples.
2. In `data-formats/index.md`: change the `xml` row to link
   `[`xml`](xml.md)` and keep the body mapping column truthful
   (`Text/Bytes ↔ Json`).
3. Create `docs/src/data-formats/xml.md`: available format, body mapping,
   `max_depth` default 100, the JSON projection rules (`@name`, `#text`, arrays,
   `null`, `xmlns` dropped, leaf strings), marshal input contract (single
   root-element key; multi-key/array/scalar fail; number/bool leaves
   stringified), `Body::Empty` behavior (marshal no-op; unmarshal fails closed —
   a recorded divergence from `json`/`form`), and a sentence that XML REST
   binding is not multipart. Use `yaml`/`text` code fences; no `rust` fences.
4. In `CONTEXT-MAP.md` REST DSL bullet: add `explicit `binding: xml` is
   structured XML binding that unmarshals XML to JSON before schema validation
   and marshals JSON back to XML` after the form sentence.
5. Grep gate: `grep -rn "binding: form" CONTEXT-MAP.md docs/src` and
   `grep -rn "json, raw" CONTEXT-MAP.md docs/src` — every binding list must now
   include `xml`; fix each hit, or record in the task report why a hit is
   unrelated.

**Tests**
- Command: `cd docs && mdbook test` — expected exit 0 (no runnable Rust fences
  were added).
- Command: `grep -c "xml" docs/src/data-formats/xml.md` — expected > 0.
- Command: `grep -n "binding: xml" docs/src/yaml-dsl/top-level-blocks.md` —
  expected ≥ 1 hit.
- Command: `grep -n "binding: xml" CONTEXT-MAP.md` — expected ≥ 1 hit, added
  in Step 4 by quoting the new mode name.

**Acceptance**
- `mdbook test` exits 0.
- `cargo xtask lint-context-citations` exits 0.
- `grep -n "xml" CONTEXT-MAP.md` finds the REST DSL bullet update, and every
  binding-list hit found by the Step 5 greps includes `xml`.
- `grep -n "binding: xml" docs/src/yaml-dsl/top-level-blocks.md CONTEXT-MAP.md`
  returns at least one hit per file.

- [x] 1.7
