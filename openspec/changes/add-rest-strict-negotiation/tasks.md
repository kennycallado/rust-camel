# Tasks: add-rest-strict-negotiation

## camel-api

### Task 1.1: Typed negotiation error variants

**Files:**
- `crates/camel-api/src/error.rs` (modified)

**Steps:**
1. Add two variants to the `#[non_exhaustive]` `CamelError` enum
   (after `EndpointUri(EndpointUriError)`, keeping the tail order):
   `UnsupportedMediaType { consumed: String, declared: String }` with
   `#[error("Unsupported media type: consumed {consumed}, declared {declared}")]`,
   and `NotAcceptable { accept: String, produced: String }` with
   `#[error("Not acceptable: accept {accept}, produced {produced}")]`.
2. Extend `CamelError::classify()` (line ~177): `Self::UnsupportedMediaType { .. } => "unsupported_media_type"` and `Self::NotAcceptable { .. } => "not_acceptable"`.
3. Extend `CamelError::variant_name()` (line ~214): the two new arms
   returning `"UnsupportedMediaType"` and `"NotAcceptable"` (the
   exhaustive defining-crate match and `variant_name_tests` fail
   compilation if omitted).
4. Add unit tests in the `error.rs` test module asserting classify
   categories, variant names, and Display strings for both variants.

**Tests:**
- `classify_negotiation_errors`
  - arrange: `CamelError::UnsupportedMediaType { consumed: "text/plain".into(), declared: "application/json".into() }` and `CamelError::NotAcceptable { accept: "application/xml".into(), produced: "application/json".into() }`
  - act: call `classify()` on each
  - assert: `"unsupported_media_type"` and `"not_acceptable"`
  - command: `cargo test -p camel-api --lib classify_negotiation_errors`
  - expected: fails before steps 1-2 (variants absent), passes after
- `variant_names_negotiation_errors`
  - arrange: the same two error values
  - act: call `variant_name()` on each
  - assert: `"UnsupportedMediaType"` and `"NotAcceptable"`
  - command: `cargo test -p camel-api --lib variant_names_negotiation_errors`
  - expected: fails before, passes after
- `display_negotiation_errors`
  - arrange: the same two error values
  - act: `to_string()`
  - assert: the consumed/declared and accept/produced values appear in the rendered strings
  - command: `cargo test -p camel-api --lib display_negotiation_errors`
  - expected: fails before, passes after

**Acceptance:**
- `cargo test -p camel-api --lib` exits 0 (including the pre-existing `variant_name_tests`).
- `cargo clippy -p camel-api -- -D warnings` exits 0.

- [x] 1.1

## camel-processor

### Task 2.1: ContentNegotiationProcessor shell

**Files:**
- `crates/camel-processor/src/content_negotiation.rs` (new)
- `crates/camel-processor/src/lib.rs` (modified — add module + re-export)
- `crates/camel-processor/CONTEXT.md` (modified — catalog entry)

**Steps:**
1. Define in `content_negotiation.rs`:
   `pub type ContentNegotiationCheck = Arc<dyn Fn(Option<&str>, Option<&str>) -> Result<(), CamelError> + Send + Sync>;`
   and `pub struct ContentNegotiationProcessor { check: ContentNegotiationCheck }` with
   `pub fn new(check: ContentNegotiationCheck) -> Self`.
2. Implement the Tower `Service<Exchange>` for it following the
   clone-poll pattern of neighboring processors (see
   `crates/camel-processor/src/set_header.rs`): `poll_ready` always
   `Ready(Ok(()))`; `call` extracts the request `Content-Type` and
   `Accept` header values from the exchange headers via
   `Message::header_ic` (camel-api/src/message.rs:40-44 — the
   case-insensitive header getter; it returns `Option<&Value>`, so
   resolve to a string with `.and_then(serde_json::Value::as_str)` for
   each header before invoking the closure) and calls
   `(self.check)(content_type, accept)`. On `Ok(())` return the
   exchange UNCHANGED (same value moved through); on `Err(e)` return
   `Err(e)`. The body is never read, polled, wrapped, or replaced — no
   `StreamCacheService`, no body access at all. `Clone` derives on the
   `Arc`.
3. Re-export from `lib.rs`: `pub use content_negotiation::{ContentNegotiationProcessor, ContentNegotiationCheck};` and register the module following house `mod` ordering.
4. Add the catalog entry to `crates/camel-processor/CONTEXT.md` in
   the processor list: name, one-line purpose (header-only media
   negotiation gate; verdict injected as a check closure; never
   touches the body), and the public symbols.
5. Unit tests in `content_negotiation.rs` (`#[cfg(test)]`), driving
   with `tower::ServiceExt::oneshot` and an exchange whose headers
   carry the probed casings. Note: `Body::Stream` does not implement
   meaningful `PartialEq` — assert body-variant preservation with
   `matches!(ex.input.body, Body::Stream { .. })`, never `assert_eq!`.

**Tests:**
- `gate_passes_exchange_through_untouched`
  - arrange: exchange with headers `Content-Type: application/json`, `Accept: application/json`, body = poll-counting one-chunk `Body::Stream` (counter `Arc<AtomicUsize>` at 0); processor built with a check closure returning `Ok(())` that records its two arguments
  - act: `processor.oneshot(exchange)`
  - assert: Ok; returned exchange has identical headers; body variant still `Body::Stream` via `matches!`; poll counter == 0; closure saw `Some("application/json")` twice
  - command: `cargo test -p camel-processor --lib gate_passes_exchange_through_untouched`
  - expected: fails before step 2, passes after
- `gate_propagates_check_error`
  - arrange: check closure returning `Err(CamelError::UnsupportedMediaType { consumed: "text/plain".into(), declared: "application/json".into() })`; exchange with `Content-Type: text/plain`
  - act: `processor.oneshot(exchange)`
  - assert: Err whose variant is `UnsupportedMediaType` with the exact payload `{consumed: "text/plain", declared: "application/json"}`
  - command: `cargo test -p camel-processor --lib gate_propagates_check_error`
  - expected: fails before, passes after
- `gate_header_lookup_resolves_casings`
  - arrange: exchange with lowercase header names `content-type: application/json`, `accept: */*`; capturing closure
  - act: `processor.oneshot(exchange)`
  - assert: closure received `Some("application/json")` and `Some("*/*")` (header_ic resolves any casing)
  - command: `cargo test -p camel-processor --lib gate_header_lookup_resolves_casings`
  - expected: fails before, passes after
- `gate_absent_headers_yield_none`
  - arrange: exchange with no Content-Type and no Accept header; capturing closure
  - act: `processor.oneshot(exchange)`
  - assert: closure received `(None, None)` and the exchange passed through Ok
  - command: `cargo test -p camel-processor --lib gate_absent_headers_yield_none`
  - expected: fails before, passes after
- `gate_non_string_header_values_yield_none`
  - arrange: exchange with `Content-Type` and `Accept` set to NON-string JSON values (e.g. `Value::Number(1)` and `Value::Bool(true)`); capturing closure
  - act: `processor.oneshot(exchange)`
  - assert: closure received `(None, None)` (`.and_then(Value::as_str)` resolves non-strings to None) and the exchange passed through Ok
  - command: `cargo test -p camel-processor --lib gate_non_string_header_values_yield_none`
  - expected: fails before, passes after

**Acceptance:**
- `cargo test -p camel-processor --lib content_negotiation` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.
- `git -C <worktree> diff --stat crates/camel-processor` shows only the three files above.

- [x] 2.1

## camel-dsl

### Task 3.1: media.rs — RFC 7231/9110 subset parser and matcher

**Files:**
- `crates/camel-dsl/src/media.rs` (new)
- `crates/camel-dsl/src/lib.rs` (modified — `mod media;`)
- `crates/camel-dsl/src/rest.rs` (modified — remove the three private helpers, import from `media`)

**Steps:**
1. Move `split_media_base`, `is_valid_media_declaration`,
   `is_json_media_type` from `rest.rs` (lines ~461-496) into
   `media.rs` as `pub(crate)` with unchanged behavior; update
   `rest.rs` to `use crate::media::{is_json_media_type, is_valid_media_declaration};` (it does not use `split_media_base` directly). All existing `rest.rs` lowering-validation tests must stay green unchanged.
2. Add to `media.rs` (all `pub(crate)`):
   - `pub(crate) struct MediaRange { pub type_: String, pub subtype: String, pub suffix: Option<String> }` — stored lowercase; wildcard representation: `type_ == "*"` and/or `subtype == "*"`; `suffix` is the structured-syntax suffix (`+json` part, lowercase).
   - `pub(crate) fn parse_concrete_media_type(value: &str) -> Option<MediaRange>` — parses `type "/" subtype ("+" suffix)?` with optional `;`-parameters (all skipped), validates tokens with the same RFC 9110 token-character rule `is_valid_media_declaration` uses, rejects ANY wildcard, returns None on any parse failure.
   - `pub(crate) fn parse_accept_entry(value: &str) -> Option<(MediaRange, f32)>` — like the above but allows `*/*` and `type/*`; extracts `;q=` (case-insensitive param name), default quality `1.0`; `q` values outside `0.0..=1.0` fail the entry.
   - `pub(crate) struct MediaContract { pub consumes: Option<MediaRange>, pub produces: Option<MediaRange>, pub consumes_declared: String, pub produces_declared: String }` — `None` on a parsed side means that side is permissive (no concrete declaration to enforce); the `_declared` fields keep the TRIMMED ORIGINAL declaration strings because the error payloads echo them (parsed ranges are lowercased and param-stripped and cannot reconstruct the strings). `pub(crate) fn parse_contract(consumes: &str, produces: &str) -> MediaContract` — each side: `parse_concrete_media_type(trimmed)`; None (including wildcard or garbage declarations, which raw-mode lowering's tchar-only check can admit) becomes `None` = permissive side. Infallible by construction — no unwrap needed.
   - `fn is_json_family(range: &MediaRange) -> bool` — `(type_, subtype) == ("application", "json")` or `suffix == Some("json")`.
   - `fn entry_satisfies(entry: &MediaRange, declared: &MediaRange) -> bool` — exact `type_`/`subtype` equality; or both JSON-family (essence rule); or `entry` is `*/*`; or `entry.subtype == "*"` and `entry.type_ == declared.type_`.
   - `fn specificity(entry: &MediaRange, declared: &MediaRange) -> Option<u8>` — tier 1 exact match, tier 2 JSON-essence match, tier 3 `type/*`, tier 4 `*/*`; `None` when the entry does not satisfy.
   - `pub(crate) fn check_request(content_type: Option<&str>, accept: Option<&str>, contract: &MediaContract) -> Result<(), CamelError>`:
     - Content-Type side: skip when `None` (argument None or permissive contract side). Parse via `parse_concrete_media_type`; parse failure (including wildcards) → `Err(CamelError::UnsupportedMediaType { consumed: raw-trimmed value, declared: contract.consumes_declared.clone() })`. Match via `entry_satisfies(candidate, &consumes)`; no match → same error.
     - Accept side: skip when `None` (argument None or permissive contract side). Split on commas; parse each entry with `parse_accept_entry`; if ANY entry fails to parse, treat the whole header as a single `*/*` entry with q=1.0. Governing entry: best (lowest) specificity tier; among entries tied at that tier, the LOWEST quality governs. No satisfying entry → `Err(CamelError::NotAcceptable { accept: raw trimmed header, produced: contract.produces_declared.clone() })`. Governing quality == 0.0 → same error. Else Ok.
3. Unit tests in `media.rs` covering the full ruling matrix (below).

**Tests (all in `crates/camel-dsl/src/media.rs` test module, command `cargo test -p camel-dsl --lib media::`):**
- `parse_concrete_accepts_params_suffix_and_case`
  - arrange: input strings `application/json`, `APPLICATION/JSON; charset=utf-8`, `application/vnd.api+json`
  - act: `parse_concrete_media_type`
  - assert: all Some; normalized lowercase `type_`/`subtype`; suffix `Some("json")` for the third; params dropped
- `parse_concrete_rejects_wildcards_and_garbage`
  - arrange: `*/*`, `application/*`, `application`, `application/`, `application/ json`
  - act: `parse_concrete_media_type`
  - assert: all None
- `parse_accept_entry_q_handling`
  - arrange: `application/json`, `application/json;q=0`, `application/json;Q=0.5`, `application/*;q=0.9`, `*/*`, `application/json;q=1.5`
  - act: `parse_accept_entry`
  - assert: qualities 1.0, 0.0, 0.5 (case-insensitive `q`), 0.9, 1.0; the `q=1.5` entry is None
- `parse_contract_wildcard_declaration_is_permissive`
  - arrange: `parse_contract("*/*", "application/json")` and `parse_contract("garbage", "application/json")`
  - act/assert: consumes side is None (permissive), produces side is Some; `check_request(Some("text/plain"), None, &contract)` is Ok for the first contract
  - command: `cargo test -p camel-dsl --lib media::parse_contract_wildcard_declaration_is_permissive`
  - expected: fails before step 2, passes after
- `check_request_415_paths`
  - arrange: contract from `("application/json", "application/json")`
  - act: `check_request` with content types `text/plain`, `not a type`, `*/*`
  - assert: each is `Err(CamelError::UnsupportedMediaType { .. })` with `consumed` echoing the trimmed input and `declared == "application/json"`
- `check_request_415_passes`
  - arrange: same contract
  - act: content types `application/json`, `application/json; charset=utf-8`, `application/vnd.api+json`, `APPLICATION/JSON`, and `None`
  - assert: all Ok (with accept argument `None`)
- `check_request_406_reject_paths`
  - arrange: same contract
  - act: accepts `application/xml`, `application/json;q=0`, `application/json;q=0, */*;q=1`, `application/json;q=0, application/json;q=1`
  - assert: each `Err(CamelError::NotAcceptable { .. })` with `accept` echoing the trimmed header and `produced == "application/json"`; the third proves precedence (exact tier beats wildcard), the fourth proves the equal-specificity tie takes the lowest q
- `check_request_406_pass_paths`
  - arrange: same contract
  - act: accepts `application/json`, `APPLICATION/JSON; charset=utf-8`, `*/*`, `application/*`, `text/html, application/xhtml+xml, application/json;q=0.9`, and `None`
  - assert: all Ok (with content-type argument `None`)
- `check_request_malformed_accept_is_permissive`
  - arrange: same contract
  - act: accepts `garbage header!!`, `application/json, garbage entry!!`
  - assert: both Ok (whole header degrades to `*/*`)
- `moved_helpers_behavior_unchanged`
  - arrange/act: drive `is_valid_media_declaration` and `is_json_media_type` over the value matrix the pre-move `rest.rs` tests exercise (`application/json`, `image/png`, `png`, `image / png`, `/png`, `text/plain; charset=utf-8`, `application/vnd.api+json`)
  - assert: identical boolean results to the values those helpers returned before the move
  - command: `cargo test -p camel-dsl --lib` (full module green proves rest.rs lowering tests unaffected)
  - expected: the full `cargo test -p camel-dsl --lib` suite passes with zero modifications to pre-existing tests

**Acceptance:**
- `cargo test -p camel-dsl --lib` exits 0 with zero modifications to pre-existing test assertions.
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.
- `media.rs` stays within ~350 LoC excluding tests (escape hatch requires the workspace review per design D3 — NOT part of this change).

- [x] 3.1

### Task 3.2: Step plumbing — route_ast, conversion, model, contract, compile

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/src/model.rs` (modified)
- `crates/camel-dsl/src/contract.rs` (modified)
- `crates/camel-dsl/src/compile.rs` (modified)

**Steps:**
1. `route_ast.rs`: add
   `pub struct ContentNegotiationStep { pub consumes: String, pub produces: String, pub check_content_type: bool }`
   (with the same `Deserialize`/schema derives pattern as `SetHeaderStep`) and the enum arm carrying ALL THREE skip attributes exactly as the `SetHeaderIfAbsent` arm does (route_ast.rs:513-515):
   `#[serde(skip_deserializing)]` + `#[cfg_attr(feature = "schema", schemars(skip))]` + `#[cfg_attr(feature = "schema", ts(skip))]`
   on `RouteDslStep::ContentNegotiation(ContentNegotiationStep)` (lowering-only precedent: untagged + skip_deserializing means authoring input can never spell it; schemars/ts skips keep the authoring schema unchanged). `check_content_type` is the verb-awareness carrier: `true` for body-ful verbs, `false` for body-less (the compile layer has no verb context — compile.rs:990 receives only the step).
2. `model.rs`: add `pub struct ContentNegotiationStepDef { pub consumes: String, pub produces: String, pub check_content_type: bool }` and the `DeclarativeStep::ContentNegotiation(ContentNegotiationStepDef)` arm. Extend the exhaustive `DeclarativeStep::kind()` match (model.rs ~766) with a new `contract::DeclarativeStepKind::ContentNegotiation` variant (contract.rs) — mirror `SetHeaderIfAbsent`'s exact treatment: it is a member of `is_rust_only_kind` (contract.rs ~97-105), NOT of the `MANDATORY_DECLARATIVE_STEP_KINDS` array (contract.rs ~52); extend `is_rust_only_kind` (and its test if one enumerates kinds) the same way.
3. `compile.rs`: three sites gain arms — `declarative_step_name` (~1709, exhaustive, no catch-all: return `"content_negotiation"`), `validate_step` (~1966, grouped arms: treat like the other always-valid binding-independent steps), and the DeclarativeStep→BuilderStep conversion: parse `crate::media::parse_contract(&def.consumes, &def.produces)` once, build
   `let ct_enabled = def.check_content_type; let check = Arc::new(move |ct: Option<&str>, acc: Option<&str>| crate::media::check_request(if ct_enabled { ct } else { None }, acc, &contract));`
   then emit exactly as the existing sites do (compile.rs:1005-1010 — `OpaqueProcessor` is a public TUPLE struct, processor.rs:65; `BoxProcessor::new` boxes the service, processor.rs:160):
   `Ok(BuilderStep::Processor(camel_api::OpaqueProcessor(camel_api::BoxProcessor::new(ContentNegotiationProcessor::new(check)))))`
4. `yaml.rs`: in `route_dsl_to_declarative_route`'s step conversion (the `SetHeaderIfAbsent` arm sits at ~1727), add the arm mapping `RouteDslStep::ContentNegotiation(step)` to `DeclarativeStep::ContentNegotiation(ContentNegotiationStepDef { consumes: step.consumes.trim().to_string(), produces: step.produces.trim().to_string(), check_content_type: step.check_content_type })`. The JSON path reuses this function (json.rs:38) — no json.rs change.
5. Tests: compile-emission and conversion tests (below).

**Tests:**
- `compile_content_negotiation_step_emits_processor` (compile.rs test module)
  - arrange: a `DeclarativeStep::ContentNegotiation(ContentNegotiationStepDef { consumes: "application/json".into(), produces: "application/json".into(), check_content_type: true })` fed through the DeclarativeStep→BuilderStep conversion entry the sibling tests use
  - act: convert
  - assert: result matches `BuilderStep::Processor(_)`
  - command: `cargo test -p camel-dsl --lib compile_content_negotiation_step_emits_processor`
  - expected: fails before step 3, passes after
- `compile_negotiation_check_respects_bodyless_flag` (compile.rs test module)
  - arrange: two `DeclarativeStep::ContentNegotiation` steps identical except `check_content_type: true` vs `false`, both converted through the real DeclarativeStep→BuilderStep conversion; an exchange with header `Content-Type: text/plain` and `Accept: application/json`, body untouched (empty body variant is fine — the gate must not read it)
  - act: destructure each emitted step as `BuilderStep::Processor(op)` then `let OpaqueProcessor(p) = op;` and drive `p.oneshot(exchange.clone())` (tower `ServiceExt` over the dev-dep; `BoxProcessor` is `Clone + Service<Exchange>`)
  - assert: the flag-true processor returns `Err(CamelError::UnsupportedMediaType { .. })`; the flag-false processor returns `Ok` with the exchange passed through
  - command: `cargo test -p camel-dsl --lib compile_negotiation_check_respects_bodyless_flag`
  - expected: fails before step 3, passes after
- `authoring_path_produces_negotiation_declarative_step` (yaml.rs test module)
  - arrange: a hand-built `RouteDslStep::ContentNegotiation(ContentNegotiationStep { consumes: " application/json ".into(), produces: "application/json".into(), check_content_type: true })` fed directly into `route_dsl_to_declarative_route` (full authoring-path coverage lands in Task 3.3)
  - act: convert
  - assert: the declarative steps begin with `DeclarativeStep::ContentNegotiation` carrying trimmed `application/json` and `check_content_type: true`
  - command: `cargo test -p camel-dsl --lib authoring_path_produces_negotiation_declarative_step`
  - expected: fails before step 4, passes after
- `authoring_cannot_spell_negotiation_step` (yaml.rs test module)
  - arrange: a plain route YAML whose steps contain a map with key `content_negotiation` (any shape)
  - act: `parse_yaml_to_declarative`
  - assert: parse fails (no step variant matches the unknown key) — regression pin proving authoring input can never construct the lowering-only step
  - command: `cargo test -p camel-dsl --lib authoring_cannot_spell_negotiation_step`
  - expected: passes before AND after step 1 (regression pin, mirrors the SetHeaderIfAbsent guard test at yaml.rs ~5884)

**Acceptance:**
- `cargo test -p camel-dsl --lib` exits 0.
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.
- `cargo xtask schema --check` exits 0 (route schema unchanged — the new RouteDslStep arm is skipped by schemars like `SetHeaderIfAbsent`, and `DeclarativeStep` carries no serde/schemars derives).

- [x] 3.2

### Task 3.3: Lowering injection and v1 pin prefix updates

**Files:**
- `crates/camel-dsl/src/rest.rs` (modified)
- `crates/camel-dsl/tests/rest_raw_e2e.rs` (modified — sequence pins gain the prefix)
- `crates/camel-dsl/tests/rest_schema_e2e.rs` (modified — sequence pins gain the prefix)
- `crates/camel-dsl/tests/rest_stream_contract_e2e.rs` (modified — sequence pins gain the prefix)

**Steps:**
1. In `lower_operation` (rest.rs ~188), at the point where steps are assembled (~line 327, before the request-binding `UnmarshalStep` push at ~341), push FIRST for BOTH bindings and ALL verbs:
   `steps.push(RouteDslStep::ContentNegotiation(ContentNegotiationStep { consumes: op.consumes.trim().to_string(), produces: op.produces.trim().to_string(), check_content_type: verb_has_body(&verb_lc) }));`
   using the existing `verb_has_body` helper (rest.rs ~456) — body-less verbs keep the Accept-side gate with the Content-Type side suppressed via the flag.
2. Update every existing lowering sequence assertion to carry the prefix: the rest.rs `#[cfg(test)]` tests (`lower_single_get_operation`, `lower_post_has_201_default`, `lower_get_defaults_to_200`, `lower_delete_defaults_to_204`, `lower_default_status_is_last_step_if_absent`, `lower_if_absent_step_compiles_through_full_chain`, `lower_user_set_status_preserved`, and the binding-json/raw validation tests) plus the three integration files listed above (the raw pipeline becomes exactly `[ContentNegotiation, To, SetHeader(Content-Type), SetHeaderIfAbsent(status)]`). Two kinds of mechanical adjustment are allowed and expected in the integration files — (a) sequence assertions gain exactly one leading entry, and (b) step-selection helpers shift: `rest_schema_e2e.rs` (~70-77) selects the unmarshal processor at `steps[0]` and must re-index to `steps[1]`; `rest_raw_e2e.rs` (~211-219) and `rest_stream_contract_e2e.rs` (~121-137, `drive_raw_pipeline` asserts exactly 3 steps and routes EVERY step through `compile_header_step`) must assert 4 steps and SKIP the new `BuilderStep::Processor` negotiation entry (it is not a header step). NO behavioral assertion may change — only prefix entries, step counts, and selection indices. Run `cargo test -p camel-dsl` to find every affected site.
3. Add the new lowering-order tests (below) to the rest.rs test module.

**Tests:**
- `lower_json_post_sequence_has_negotiation_first` (rest.rs test module)
  - arrange: `make_rest("op1", "post", "/x", "direct:t")` (house helper, binding json defaults)
  - act: lower
  - assert: step sequence is exactly `[ContentNegotiation { consumes: application/json, produces: application/json, check_content_type: true }, Unmarshal(json), To(direct:t), Marshal(json), SetHeader(Content-Type: application/json), SetHeaderIfAbsent(201)]`
  - command: `cargo test -p camel-dsl --lib lower_json_post_sequence_has_negotiation_first`
  - expected: fails before step 1, passes after
- `lower_raw_sequence_has_negotiation_first`
  - arrange: `make_rest` variant with `binding: raw`, `consumes: application/octet-stream`, `produces: image/png`, POST
  - act: lower
  - assert: exactly `[ContentNegotiation { consumes: application/octet-stream, produces: image/png, check_content_type: true }, To, SetHeader(Content-Type: image/png), SetHeaderIfAbsent(status)]`
  - command: `cargo test -p camel-dsl --lib lower_raw_sequence_has_negotiation_first`
  - expected: fails before, passes after
- `lower_bodyless_verbs_carry_flag_false`
  - arrange: `make_rest("op1", "get", "/x", "direct:t")` and a DELETE variant
  - act: lower both
  - assert: each sequence starts with `ContentNegotiation { check_content_type: false, .. }` and contains no `Unmarshal`
  - command: `cargo test -p camel-dsl --lib lower_bodyless_verbs_carry_flag_false`
  - expected: fails before, passes after

**Acceptance:**
- `cargo test -p camel-dsl` exits 0 (lib + all integration tests).
- Diff of updated pin files shows ONLY: leading-entry additions to sequence assertions, step-count updates, and selection-index/helper adjustments that skip the negotiation step — no behavioral assertion edits.
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.

- [x] 3.3

## camel-http

### Task 4.1: Finalizer 415/406 mapping and CONTEXT.md

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified — `pipeline_error_to_reply` arms + unit tests only)
- `crates/components/camel-http/CONTEXT.md` (modified)

**Steps:**
1. In `pipeline_error_to_reply` (lib.rs:3334), add two arms BEFORE the catch-all, mirroring the `TypeConversionFailed` shape (tracing::warn + JSON body with `error` and `message` fields + Content-Type application/json):
   - `CamelError::UnsupportedMediaType { consumed, declared }` → status 415, error code `"unsupported_media_type"`, message `format!("consumed {consumed}, declared {declared}")`
   - `CamelError::NotAcceptable { accept, produced }` → status 406, error code `"not_acceptable"`, message `format!("accept {accept}, produced {produced}")`
2. Add the finalizer unit tests (below) near the existing `pipeline_error_to_reply` tests in lib.rs.
3. Update `CONTEXT.md` (finalizer mapping paragraph, ~lines 288-295): the error-mapping list gains `UnsupportedMediaType` → 415 and `NotAcceptable` → 406; note they originate in the media negotiation step injected by REST lowering.

**Tests:**
- `finalizer_maps_unsupported_media_type` (lib.rs test module)
  - arrange: `pipeline_error_to_reply(CamelError::UnsupportedMediaType { consumed: "text/plain".into(), declared: "application/json".into() }, "/x")`
  - act: inspect the `HttpReply`
  - assert: status 415; headers contain `Content-Type: application/json`; body parses as JSON with `"error": "unsupported_media_type"` and a message containing both values
  - command: `cargo test -p camel-component-http --lib finalizer_maps_unsupported_media_type`
  - expected: fails before step 1, passes after
- `finalizer_maps_not_acceptable`
  - arrange: same with `NotAcceptable { accept: "application/xml".into(), produced: "application/json".into() }`
  - act/assert: status 406; same body shape with `"error": "not_acceptable"`
  - command: `cargo test -p camel-component-http --lib finalizer_maps_not_acceptable`
  - expected: fails before, passes after

**Acceptance:**
- `cargo test -p camel-component-http --lib` exits 0.
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0.
- `git -C <worktree> diff --stat crates/components/camel-http` shows only `src/lib.rs` and `CONTEXT.md` — registry.rs, rest_match.rs, and all other files untouched.

- [x] 4.1

## camel-dsl (end-to-end)

### Task 5.1: End-to-end negotiation battery (camel-dsl hosts the live-HTTP rig)

Runs AFTER Task 4.1: the live 415/406 statuses require the finalizer
arms; without them the typed errors hit the catch-all and surface as
500.

**Files:**
- `crates/camel-dsl/tests/rest_negotiation_e2e.rs` (new)

**Steps:**
1. Create the battery in `crates/camel-dsl/tests/` — the live-consumer rig (spawn `camel_component_http::HttpConsumer` over a lowered REST document + real TCP/HTTP client requests through the axum handler) lives HERE, not in camel-http: `camel-dsl` dev-depends on `camel-component-http` (no reverse dev-dependency exists; see the direction note in the header of `tests/rest_stream_contract_e2e.rs` and its rig helpers). Reuse that rig's spawn/client helpers.
2. Document under test: one POST operation binding `json` (`consumes`/`produces` `application/json`, `to` an echo/direct sink reachable from the test), one GET operation binding `json`, one DELETE operation binding `json`.
3. Write the e2e tests (below). All assert status codes and JSON error-body shapes; body-variant assertions use `matches!` (Body::Stream has no meaningful PartialEq).

**Tests (command `cargo test -p camel-dsl --test rest_negotiation_e2e`; all fail before Tasks 3.2, 3.3, AND 4.1 land, pass after all three):**
- `e2e_post_wrong_content_type_415_and_sink_untouched`
  - arrange: consumer over the POST op with a sink the test can inspect
  - act: POST with `Content-Type: text/plain` and a JSON body
  - assert: status 415; body parses as JSON with `"error": "unsupported_media_type"`; the route sink received NOTHING (negotiation rejected the exchange before the pipeline ran); status is 415 — not 404 — which also proves media is not a routing key (registry media-blindness at runtime)
- `e2e_post_parameterized_matching_content_type_passes`
  - act: POST `Content-Type: application/json; charset=utf-8`
  - assert: 2xx and the sink received the exchange
- `e2e_post_plus_json_suffix_passes`
  - act: POST `Content-Type: application/vnd.api+json` (valid JSON bytes)
  - assert: 2xx
- `e2e_post_malformed_content_type_415`
  - act: POST `Content-Type: garbage type`
  - assert: 415
- `e2e_post_wildcard_content_type_415`
  - act: POST `Content-Type: */*`
  - assert: 415
- `e2e_get_and_delete_no_content_type_check`
  - act: GET and DELETE with `Content-Type: text/plain` (body-less verbs)
  - assert: neither is 415 (each returns the op's own outcome)
- `e2e_accept_mismatch_406`
  - act: GET with `Accept: application/xml`
  - assert: 406, body `"not_acceptable"`
- `e2e_accept_q0_406`
  - act: GET with `Accept: application/json;q=0`
  - assert: 406
- `e2e_accept_precedence_q0_406`
  - act: GET with `Accept: application/json;q=0, */*;q=1`
  - assert: 406
- `e2e_accept_tie_lowest_q_406`
  - act: GET with `Accept: application/json;q=0, application/json;q=1`
  - assert: 406
- `e2e_accept_wildcard_passes`
  - act: GET with `Accept: */*`, then GET with `Accept: application/*`
  - assert: both 2xx
- `e2e_accept_multi_entry_passes`
  - act: GET with `Accept: text/html, application/xhtml+xml, application/json;q=0.9`
  - assert: 2xx
- `e2e_accept_malformed_permissive`
  - act: GET with `Accept: garbage header!!`
  - assert: 2xx
- `e2e_absent_headers_permissive_pinned_bytes`
  - act: POST well-formed JSON, no Accept, matching Content-Type
  - assert: 2xx; response body equals the byte string this test pins inline (write the expected exact response payload as a literal in this file — the pin lives here, not in another suite)
- `dsl_compiled_negotiation_rejects_without_polling` (DSL-level, same file — drives compiled steps directly, mirroring the L3 rest_stream_contract_e2e pattern, because a live TCP request is itself polled by the transport and cannot prove pipeline-side poll counts. NOTE: `Service::call` returning `Err` drops the exchange — assert only the error variant and the external counter, not the returned exchange)
  - arrange: parse a POST binding `json` REST document through the standard authoring path; take the compiled route's steps; exchange body = poll-recording one-chunk `Body::Stream` (counter `Arc<AtomicUsize>` at 0, kept by the test), headers `Content-Type: text/plain`
  - act: drive the compiled steps the way the sibling suites drive compiled steps
  - assert: the chain fails with `CamelError::UnsupportedMediaType`; the kept poll counter is 0
- `dsl_compiled_negotiation_pass_preserves_body` (pass-path counterpart — the returned exchange exists only on Ok)
  - arrange: same document and rig, headers matching (`Content-Type: application/json`, `Accept: application/json`), body = poll-recording stream with a kept identity handle to the stream mutex (L3 pattern); drive ONLY the compiled negotiation step
  - act: drive the negotiation step alone via oneshot
  - assert: Ok; the poll counter is 0; the returned exchange body variant is still `Body::Stream` via `matches!` and `Arc::ptr_eq` proves no replacement `StreamBody` was installed; `StreamMetadata` is unchanged
- `e2e_raw_stream_passthrough_with_negotiation` (binding `raw` — under binding `json` the unmarshal step legitimately consumes the stream, so stream identity is provable only in raw mode, mirroring the L3 contract)
  - arrange: consumer over a POST operation binding `raw`, `consumes: application/octet-stream`, `produces: application/octet-stream`, echoing the request stream
  - act: POST matching `Content-Type: application/octet-stream`, `Accept: application/octet-stream`, streamed body
  - assert: 2xx; response body bytes equal the request bytes (negotiation did not wrap, replace, or consume the stream)

**Acceptance:**
- `cargo test -p camel-dsl --test rest_negotiation_e2e` exits 0.
- `cargo test -p camel-dsl --test rest_raw_e2e --test rest_schema_e2e --test rest_stream_contract_e2e` exits 0 (L1/L3 pins green through the same rig).
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.

- [ ] 5.1
