# Tasks: 475-rest-form-binding

## camel-processor (form data format)

### Task 1.1: FormDataFormat implementation, wiring, registration

**Files:**
- `crates/camel-processor/src/data_format/form.rs` (new)
- `crates/camel-processor/src/data_format/mod.rs` (modified)

**Steps:**
1. Create `form.rs` with `FormConfig` (serde `#[serde(default, deny_unknown_fields)]`):
   `max_fields: Option<usize>` (default `Some(1_000)`, counts total non-empty pairs,
   duplicates included), `max_bytes: Option<usize>` (default `Some(1_048_576)`,
   input length). Manual `Default` impl (mirror `CsvConfig`).
2. Add `pub(crate) fn parse_form_pairs(input: &[u8], cfg: &FormConfig) -> Result<Vec<(String, String)>, CamelError>`:
   enforce `max_bytes` on `input.len()` FIRST (before any parsing) with
   `CamelError::TypeConversionFailed` message starting `form` and naming `max_bytes`;
   split on `&` (empty segments skipped, uncounted); per non-empty segment count
   toward `max_fields` and error naming `max_fields` when exceeded; split on the
   FIRST `=` (missing `=` → empty value; empty key allowed); decode key and value:
   `+` → space (0x20), `%XX` strict hex (anything else → error naming the malformed
   escape and its byte offset), accumulate raw bytes then validate UTF-8 strictly
   (`std::str::from_utf8`; failure → error, no lossy replacement — do NOT use the
   `form_urlencoded` crate, it is lossy).
3. Add `pub(crate) fn encode_form_component(s: &str) -> String` (WHATWG
   urlencoded serializer): ASCII alphanumerics and `* - . _` pass unescaped,
   space → `+`, every other byte → uppercase `%XX` over the UTF-8 encoding.
4. Add `pub struct FormDataFormat` (field `cfg: FormConfig`) + `FormDataFormat::new(cfg: FormConfig)` and a COMPLETE `impl DataFormat for FormDataFormat` (all required methods in one impl — the trait has no partial impl):
   - `fn name(&self) -> &str` → `"form"`
   - `fn unmarshal(&self, body: Body) -> Result<Body, CamelError>` (signature per `camel-api/src/data_format.rs`, body by value):
     `Body::Text(s)` / `Body::Bytes(b)` → `parse_form_pairs`, fold pairs into a `serde_json::Map` (first occurrence → `Value::String`; duplicate key → push onto array, upgrading `String` to `array![old, new]`, encounter order; zero pairs → `{}`), return `Body::Json(Value::Object(map))`;
     `Body::Empty` → no-op `Ok(Body::Empty)` (mirror `json.rs`);
     `Body::Stream` → `Err(TypeConversionFailed)` message starts `form`, says streams must be materialized first (mirror `json.rs` wording; body is dropped unpolled);
     other variants → `Err(TypeConversionFailed)` listing supported variants (mirror `json.rs`).
   - `fn marshal(&self, body: Body) -> Result<Body, CamelError>`:
     `Body::Empty` → no-op `Ok(Body::Empty)`;
     `Body::Stream` → `Err(TypeConversionFailed)` starting `form` (mirror `json.rs` marshal wording);
     `Body::Json(Value::Object(map))` → per entry: `Value::String` as-is; number/bool via `value.to_string()`; `Value::Null` → `"null"`; `Value::Array` repeats the key per element (elements must be scalars, else `Err` naming the key); `Value::Object` or array-inside-array → `Err(TypeConversionFailed)` starting `form` naming the key; join `k=v` (key and value through `encode_form_component`) with `&`; empty map → `Ok(Body::Text(""))`;
     `Body::Json` non-object (array/scalar) → `Err` starting `form` naming the shape;
     other variants → `Err` listing supported variants.
5. Wire in `mod.rs` (this task, so the file compiles and tests run): `mod form;` + `pub use form::{FormDataFormat, FormConfig};` + registry arm `"form" => Arc::new(FormDataFormat::new(parse_cfg::<FormConfig>(name, config)?))` (match existing arm style). All helpers get production callers via the trait impl and registry — no `dead_code`, no `#[allow]`.

**Tests:** (in `form.rs` `#[cfg(test)] mod tests`; RED before impl, GREEN after)
- `parse_single_pair` — setup: cfg default; action: `parse_form_pairs(b"name=camel", &cfg)`; assert: `Ok(vec![("name".into(),"camel".into())])`; command: `cargo test -p camel-processor --lib data_format::form::tests::parse_`; expected: fails before step 2 exists.
- `parse_plus_and_percent` — `b"a=one+two&b=one%20two&c=1%2B1"` → pairs `[("a","one two"),("b","one two"),("c","1+1")]`.
- `parse_missing_equals_empty_value` — `b"flag&x="` → `[("flag",""),("x","")]`.
- `parse_empty_key` — `b"=v"` → `[("","v")]`.
- `parse_empty_segments_skipped` — `b"a=1&&b=2"` → `[("a","1"),("b","2")]` and `b"&a=1&"` → `[("a","1")]`; also `max_fields=Some(1)` with `b"a=1&&"` still Ok (empty segment uncounted).
- `parse_malformed_percent` — `b"a=100%2"` and `b"a=%zz"` each → `Err` with `TypeConversionFailed`, message starts `form` and names the escape (assert via variant match + `message.contains`).
- `parse_invalid_utf8` — `b"a=%FF"` and `b"a=%E9"` → `Err(TypeConversionFailed)` message starts `form`.
- `parse_max_fields_exceeded` — cfg `max_fields=Some(2)`, input `b"a=1&b=2&c=3"` → Err naming `max_fields`.
- `parse_max_fields_exact_passes` — cfg `max_fields=Some(3)`, input `b"a=1&b=2&c=3"` → Ok with 3 pairs.
- `parse_max_bytes_before_parsing` — cfg `max_bytes=Some(4)`, input `b"a=%zz&b=2"` (BOTH oversized AND malformed) → Err naming `max_bytes` and NOT naming the escape (proves the cap runs before parsing); also input `b"a=1&b=2"` → Err naming `max_bytes`, message must NOT mention any pair content.
- `encode_whatwg_matrix` — `encode_form_component("a b&c=d+é")` == `"a+b%26c%3Dd%2B%C3%A9"`; `encode_form_component("AZaz09*-._")` == `"AZaz09*-._"`; `encode_form_component("café")` == `"caf%C3%A9"`.

**Acceptance:**
- `cargo test -p camel-processor --lib data_format::form` exits 0 AND the run reports at least 10 passed tests in the form module (nonzero — proves compilation/execution, not a zero-match filter pass).
- `cargo fmt --check` and `cargo clippy -p camel-processor --all-targets -- -D warnings` exit 0 with no `#[allow(dead_code)]` added.
- No `unwrap()` in new code (`cargo xtask lint-unwrap` green).

- [x] 1.1

### Task 1.2: Unmarshal behavior tests

**Files:**
- `crates/camel-processor/src/data_format/form.rs` (modified, tests only unless a test exposes an implementation defect — then fix the defect minimally)

**Steps:**
1. Add the unmarshal body-level tests below to the existing `mod tests` (implementation exists from task 1.1; each test must pass — if one exposes a defect in the 1.1 implementation, fix the implementation minimally and re-run).

**Tests:**
- `unmarshal_text_to_object` — `FormDataFormat::new(FormConfig::default())`; body `Body::Text("name=camel".into())` through `unmarshal(body)` → `Body::Json` object `{"name":"camel"}` (assert `serde_json::Value` equality after extracting the body).
- `unmarshal_duplicates_array_encounter_order` — `Body::Text("tag=a&tag=b&tag=c")` → `tag` == `["a","b","c"]` (Value equality only; never assert key order).
- `unmarshal_empty_input_object` — `Body::Text("".into())` → `{}`.
- `unmarshal_bytes_path` — `Body::Bytes(b"a=1".to_vec().into())` → `{"a":"1"}` (match the crate's `Bytes` construction idiom).
- `unmarshal_empty_noop` — `Body::Empty` in → `Body::Empty` out, no error.
- `unmarshal_stream_fail_closed` — `Body::Stream` (construct per crate idiom) → `Err(TypeConversionFailed)`, message starts `form`.
- `unmarshal_utf8_payload` — `Body::Text("nome=café".into())` → `{"nome":"café"}`.
- command: `cargo test -p camel-processor --lib data_format::form::tests::unmarshal_`.

**Acceptance:**
- All 7 tests pass; `cargo test -p camel-processor --lib data_format::form` exits 0.
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0.

- [x] 1.2

### Task 1.3: Marshal and registry tests

**Files:**
- `crates/camel-processor/src/data_format/form.rs` (modified, tests only unless a test exposes an implementation defect — then fix the defect minimally)

**Steps:**
1. Add the marshal and registry tests below (implementation and registration exist from task 1.1; fix defects minimally if exposed).

**Tests:**
- `marshal_flat_object_pairset` — body `Body::Json(json!({"a":"1","b":"2"}))` → `Body::Text(t)`; re-split `t` on `&`, collect into `BTreeSet`, equals `{"a=1","b=2"}` (no key-order assertion).
- `marshal_array_values_repeat_key` — `json!({"tag":["a","b"]})` → exactly `tag=a&tag=b`.
- `marshal_scalars_lexical` — `json!({"n":1,"b":true,"z":null})` → pair-set equals `{"n=1","b=true","z=null"}`.
- `marshal_whatwg_encoding` — `json!({"q":"a b&c=d+é"})` → exactly `q=a+b%26c%3Dd%2B%C3%A9`.
- `marshal_utf8_roundtrip` — `json!({"nome":"café"})` → exactly `nome=caf%C3%A9`.
- `marshal_empty_object` — `json!({})` → `Body::Text("")`.
- `marshal_empty_noop` — `Body::Empty` → `Body::Empty`, no error.
- `marshal_stream_fail_closed` — `Body::Stream` → `Err(TypeConversionFailed)` starting `form`.
- `marshal_rejects_nested_object` — `json!({"a":{"b":"1"}})` → Err starting `form`.
- `marshal_rejects_nested_array` — `json!({"a":[["1"]]})` → Err starting `form`.
- `marshal_rejects_non_object` — `Body::Json(json!([1]))` and `Body::Json(json!(true))` → Err starting `form`.
- `registry_resolves_form` — `builtin_data_format("form")` returns Some, `df.name() == "form"`.
- `registry_config_denies_unknown` — `builtin_data_format_with_config("form", &serde_json::json!({"bogus": 1}))` → Err mentioning the unknown key (deny_unknown_fields path).
- command: `cargo test -p camel-processor --lib data_format::form`.

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0 (whole crate, no regressions).
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0.

- [x] 1.3

## camel-dsl (binding mode)

### Task 2.1: `Form` variant + media validation

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/media.rs` (modified)
- `crates/camel-dsl/src/rest.rs` (modified)

**Steps:**
1. `route_ast.rs`: add `Form` variant to `RouteDslRestBinding` after `Json`, before `Raw`, with doc comment `/// application/x-www-form-urlencoded structured binding (form ⇄ JSON object).` Enum attrs unchanged (lowercase rename keeps unknown strings failing at deserialize).
2. `media.rs`: add `pub(crate) fn is_form_media_type(media: &str) -> bool`: `split_media_base` equals `application/x-www-form-urlencoded` case-insensitively (ASCII lowercase compare); every `;`-parameter, trimmed, must match `charset=utf-8` case-insensitively; at most one parameter; anything else → false.
3. `rest.rs` validation: add `RouteDslRestBinding::Form` arm mirroring the Json arm shape: `is_form_media_type(&op.consumes)` else `RouteError` `"rest operation '{op_label}': binding 'form' requires 'application/x-www-form-urlencoded' for 'consumes' (got '{}')"`, same for `produces`; NO schema rejection (request_schema/response.schema allowed).

**Tests:** (in `rest.rs` test module, follow existing naming)
- `form_binding_exact_media_loads` — binding form + form media on both fields lowers Ok.
- `form_binding_charset_utf8_accepted` — `application/x-www-form-urlencoded; charset=utf-8` on consumes lowers Ok.
- `form_binding_non_form_media_fails` — `consumes: application/json` → Err RouteError; message contains op label, `consumes`, offending value, `application/x-www-form-urlencoded`.
- `form_binding_non_utf8_charset_fails` — `produces: application/x-www-form-urlencoded; charset=iso-8859-1` → Err naming op label and value.
- `form_binding_allows_request_and_response_schema` — request_schema Some + response.schema Some lowers Ok (no RouteError).
- `unknown_binding_still_fails_deserialize` — YAML op with `binding: multipart` → document deserialization fails unknown-variant (mirror the existing `binding: yaml` test shape).
- command: `cargo test -p camel-dsl --lib rest`.

**Acceptance:**
- `cargo test -p camel-dsl --lib rest` exits 0; existing json/raw validation tests stay green.
- `cargo clippy -p camel-dsl --all-features -- -D warnings` exits 0.

- [x] 2.1

### Task 2.2: Lowering + OpenAPI parity + schema artifacts

**Files:**
- `crates/camel-dsl/src/rest.rs` (modified)
- `crates/camel-dsl/src/openapi.rs` (modified, tests only)

**Steps:**
1. `rest.rs` lowering: where the Json/Raw branch selects the data-format strings, make binding `Form` inject `UnmarshalStep { unmarshal: "form".to_string(), schema: op.request_schema.clone(), config: None }` for body-bearing verbs and `MarshalStep { marshal: "form".to_string(), config: None }` — keep negotiation step, SetHeader Content-Type (trimmed produces), SetHeaderIfAbsent default status, and user-step position byte-identical to the json path (parameterize the format string; do not duplicate the pipeline block).
2. `openapi.rs`: verify no code change is needed (`raw` flag false for Form); add a parity test.
3. Regenerate schema artifacts if `cargo xtask schema --check` fails: run `cargo xtask schema` and commit the regenerated files (enum gains `form`).

**Tests:**
- `form_post_step_sequence` — POST, binding form, `to` target: lowered steps are exactly `[ContentNegotiation(consumes, produces), Unmarshal(form, schema=None), To(target), Marshal(form), SetHeader(Content-Type=produces), SetHeaderIfAbsent(verb default)]` (assert variant + format string per element, mirroring the existing json sequence test).
- `form_get_no_unmarshal` — GET, binding form: sequence starts with ContentNegotiation, contains Marshal(form), no Unmarshal.
- `form_request_schema_carried` — POST with request_schema: Unmarshal step `schema` field is Some(request_schema) and `unmarshal == "form"`.
- `form_binding_openapi_authored_schema` — POST, binding form, form media, request_schema `{"type":"object"}`: OpenAPI requestBody content under `application/x-www-form-urlencoded` carries the authored schema (NOT binary), same as json mode (mirror `raw_request_body_binary_schema` test harness).
- commands: `cargo test -p camel-dsl --lib rest` and `cargo test -p camel-dsl --lib openapi`; `cargo xtask schema --check` exits 0 after regen.

**Acceptance:**
- `cargo test -p camel-dsl --lib` exits 0 (json/raw lowering tests untouched and green).
- `cargo xtask schema --check` exits 0.
- `cargo clippy -p camel-dsl --all-features -- -D warnings` exits 0.

- [x] 2.2

### Task 2.3: Executed end-to-end pipeline

**Files:**
- `crates/camel-dsl/tests/rest_form_e2e.rs` (new)

**Steps:**
1. Mirror `crates/camel-dsl/tests/rest_raw_e2e.rs` harness (YAML doc → compile → execute against an in-process pipeline; reuse its common helpers/imports).
2. Route: `rest` block, POST op, `binding: form`, form media, one user step that captures the exchange body (mirror how raw e2e observes bodies).
3. Case A (request): send request body `Text("name=ada&role=admin&role=dev")` with matching Content-Type → captured body is `Body::Json` object `{"name":"ada","role":["admin","dev"]}` (Value equality).
4. Case B (response): user step sets body `Body::Json({"status":"ok","tag":["a","b"]})` → response body is form text whose pair-set equals `{status=ok, tag=a, tag=b}` (split on `&`, collect to sorted Vec).
5. Case C (gate-opaque charset): request Content-Type `application/x-www-form-urlencoded; charset=iso-8859-1` with body `Text("a=%E9")` → passes negotiation gate, pipeline errors from unmarshal with `TypeConversionFailed` starting `form` (assert error variant/prefix per how raw e2e asserts failures).
6. Case D (schema validates the parsed JSON — valid): route carries `request_schema` `{"type":"object","required":["name"],"properties":{"name":{"type":"string"}}}`; request `Text("name=ada")` with form Content-Type → pipeline completes and the captured body is the JSON object `{"name":"ada"}`.
7. Case E (schema validates the parsed JSON — invalid): same route; request `Text("role=admin")` (missing required `name`) → pipeline fails with the schema-validation error kind the json binding produces (assert per the json-schema e2e harness in `rest_schema_e2e.rs`), NOT a form parse error.

**Tests:**
- `form_request_unmarshals_to_json_body` (Case A), `form_response_marshal_from_json` (Case B), `form_gate_opaque_charset_fails_at_unmarshal` (Case C), `form_schema_valid_accepts_parsed_object` (Case D), `form_schema_invalid_rejects_parsed_object` (Case E) — names/setup/action/assert as above.
- command: `cargo test -p camel-dsl --test rest_form_e2e`.

**Acceptance:**
- `cargo test -p camel-dsl --test rest_form_e2e` exits 0.
- `cargo test -p camel-dsl --test rest_raw_e2e` and `--test rest_negotiation_e2e` and `--test rest_schema_e2e` still exit 0 (regressions untouched).

- [x] 2.3

## docs + terminology + gates

### Task 3.1: REST binding + data-format documentation + terminology landing

**Files:**
- `docs/src/yaml-dsl/top-level-blocks.md` (modified)
- `docs/src/data-formats/form.md` (new)
- `docs/src/data-formats/index.md` (modified)
- `CONTEXT-MAP.md` (modified)
- `crates/camel-processor/CONTEXT.md` (modified)
- `crates/camel-dsl/CONTEXT.md` (modified)

**Steps:**
1. `top-level-blocks.md` REST binding section: add `binding: form` beside json/raw — media requirement (exact `application/x-www-form-urlencoded`, optional `; charset=utf-8`), schema allowed with the form-parse → JSON → schema-validate ordering, pipeline identical to json with `form` format.
2. New `docs/src/data-formats/form.md`: usage (`unmarshal: form` / `marshal: form`, `config:` block with `max_fields`/`max_bytes` defaults 1000/1 MiB), semantics table (duplicates → arrays with encounter order, `+` = space, strict `%XX`, UTF-8 only, empty segments skipped, `Body::Empty` no-op, `Body::Stream` fail-closed), WHATWG marshal encoding, and an explicit "No header auto-mapping" note (form fields never become headers).
3. `index.md`: add the form entry to the data-format list linking `form.md`.
4. Terminology landing (CONTEXT-MAP term-landing rule, same-change): extend the REST DSL paragraph in `CONTEXT-MAP.md` (the one beginning `REST DSL:`, around line 186) with one sentence covering `binding: form` (form ⇄ JSON object pipeline, schema validated on the parsed JSON). Grep `crates/camel-processor/CONTEXT.md` for the data-format enumeration and add `form`; grep `crates/camel-dsl/CONTEXT.md` for the binding-mode enumeration and add `form`; if a file has no such enumeration, leave it untouched and note that in the report.

**Tests:**
- `docs_build_clean` — setup: edited docs; action: `cd docs && mdbook test`; assert: exits 0, no broken links.
- `grep_contract` — action: grep per file; assert: `docs/src/data-formats/form.md` contains `max_fields`, `max_bytes`, `application/x-www-form-urlencoded`, `duplicates`, `UTF-8`, and `No header auto-mapping`; `docs/src/yaml-dsl/top-level-blocks.md` contains `binding: form` and `application/x-www-form-urlencoded`; `docs/src/data-formats/index.md` contains a `form.md` link; `CONTEXT-MAP.md` contains `binding: form` in the REST DSL paragraph.
- command: `cd docs && mdbook test`.

**Acceptance:**
- Doc gate exits 0; grep contract passes; prose in English (language policy).

- [x] 3.1

### Task 3.2: Final gates sweep

**Files:**
- none (verification only; any failure loops back to the owning task)

**Steps:**
1. Run every command below from the worktree root; record each exit code; on any failure, fix in the owning file (or loop the finding back to the conductor) — do not weaken a gate.
2. Commands: `cargo test -p camel-processor -p camel-dsl`; `cargo fmt --check --all`; `cargo clippy -p camel-processor --all-targets -- -D warnings`; `cargo clippy -p camel-dsl --all-features -- -D warnings`; every `cargo xtask lint-*` in the AGENTS.md QUALITY GATES list (24 commands); `cargo xtask schema --check`; `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-processor -p camel-dsl --no-deps`; `cd docs && mdbook test`; `openspec validate --all --strict`.

**Tests:**
- `gates_report` — setup: all prior tasks checked; action: run the command list; assert: every exit code is 0 (record the code+duration per command in the report; a pre-existing failure unrelated to this change is reported, verified against main, and filed per the pre-existing-failure exemption rule — never silently skipped).

**Acceptance:**
- All listed commands exit 0 (or carry an explicitly recorded exemption with main-branch evidence).
- Report lists each command with its exit code.

- [x] 3.2
