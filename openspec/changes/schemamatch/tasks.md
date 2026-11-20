# Tasks: schemamatch

Single-phase change (design.md has no `## Phases` section). Task order is the
implementation order: the matcher variant (1.1) is the API surface every later
task consumes; grammar (1.2) and failure surfacing (1.3, 1.4) depend on it;
docs (1.5) and lanes (1.6) close the change.

## camel-component-mock

### Task 1.1: BodyMatcher::JsonSchema variant, constructor, and violation note

**Files:**
- `crates/components/camel-mock/Cargo.toml` (modified)
- `crates/components/camel-mock/src/matcher.rs` (modified)

**Steps:**
1. Add `camel-json-schema.workspace = true` to `[dependencies]` in
   `crates/components/camel-mock/Cargo.toml` (alphabetical position among the
   `camel-*` entries; `camel-json-schema` is a leaf crate — `lint-component-deps`
   accepts component → leaf).
2. In `crates/components/camel-mock/src/matcher.rs`, add `use
   std::path::PathBuf;` and `use std::sync::Arc;` to the existing import
   block (one module per `use` statement, ADR-0091), and add the variant to
   `BodyMatcher` (the enum keeps `#[non_exhaustive]` and its derived
   `Clone, Debug`):
   ```rust
   /// The body validates against a compiled JSON Schema (the confined
   /// compiler of ADR-0100; see `camel-json-schema`). Compiled once at
   /// load — never per assertion.
   JsonSchema {
       /// Compiled schema, shared because `CompiledSchema` is not `Clone`.
       schema: Arc<camel_json_schema::CompiledSchema>,
       /// Schema path as passed at load (document directory joined when
       /// the declared path was relative), rendered in failure messages.
       path: String,
   },
   ```
3. Add the constructor to `impl BodyMatcher`:
   ```rust
   /// Compile a JSON Schema file into a [`BodyMatcher::JsonSchema`].
   ///
   /// The single compile entry for the testkit: `syntax` maps the
   /// document's declared `type` (`json`/`yaml`, omitted = extension rule)
   /// onto [`camel_json_schema::SchemaSyntax`]. Format handling is
   /// [`camel_json_schema::FormatMode::DraftDefault`] — this matcher's
   /// explicit policy (the root document's `$schema` selects the draft),
   /// distinct from the validator component's configured `Enforce`/
   /// `Annotate`.
   ///
   /// # Errors
   ///
   /// Fails closed at load, naming `path`: missing or unreadable file,
   /// invalid schema, unresolvable `$ref` (per the ADR-0100 confinement
   /// rules of the `json-schema-validation` spec).
   pub fn json_schema_file(
       path: impl Into<PathBuf>,
       syntax: camel_json_schema::SchemaSyntax,
   ) -> Result<Self, camel_json_schema::SchemaError> {
       let path = path.into();
       let schema = Arc::new(camel_json_schema::compile(
           &camel_json_schema::SchemaSource::File(path.clone()),
           syntax,
           camel_json_schema::FormatMode::DraftDefault,
       )?);
       Ok(Self::JsonSchema { path: path.display().to_string(), schema })
   }
   ```
4. Add the `matches!` arm: `BodyMatcher::JsonSchema { schema, .. } =>
   json_value(actual).is_some_and(|value| schema.violations(&value).is_empty())`
   (reuses the existing private `json_value` helper — no second body→JSON
   path; a non-JSON body is a non-match, not an error).
5. Add the `mismatch_note` arm: `BodyMatcher::JsonSchema { .. } => match
   json_value(actual) { None => Some("body is not JSON"), Some(_) => None }`.
6. Add the public method:
   ```rust
   /// The first schema violation when a JSON body fails a
   /// [`BodyMatcher::JsonSchema`], formatted `"{message} at
   /// {instance_path}"` (the shape `camel-processor` established).
   /// `None` for every other matcher and for non-JSON bodies (the
   /// mismatch note covers those).
   #[must_use]
   pub fn violation_note(&self, actual: &Body) -> Option<String> {
       match self {
           BodyMatcher::JsonSchema { schema, .. } => schema
               .violations(&json_value(actual)?)
               .first()
               .map(|violation| {
                   format!("{} at {}", violation.message, violation.instance_path)
               }),
           _ => None,
       }
   }
   ```
7. Add the `Display` arm: `BodyMatcher::JsonSchema { path, .. } => write!(f,
   "jsonSchema {path}")`.

**Tests:** (in the existing `mod tests` at the bottom of `matcher.rs`;
construct matchers inline via
`camel_json_schema::compile(&SchemaSource::Inline(json!({..})),
SchemaSyntax::Json, FormatMode::DraftDefault)` wrapped in `Arc` — no files on
disk needed for these)
- `json_schema_valid_body_matches`: inline schema
  `{"type":"object","required":["id"],"properties":{"id":{"type":"string"}}}`
  → `matches(&Body::Json(json!({"id":"order-1"})))` is `true`.
- `json_schema_violation_is_non_match`: same schema →
  `matches(&Body::Json(json!({})))` is `false`; `violation_note` returns
  `Some(note)` where `note` contains `" at "`; `format!("{matcher}")`
  contains the `path` passed at construction.
- `json_schema_text_body_parses_as_json`: same schema →
  `matches(&Body::Text(r#"{"id":"order-1"}"#.into()))` is `true` (text that
  parses as JSON participates).
- `json_schema_non_json_body_is_non_match`: same schema →
  `matches(&Body::Text("not json".into()))` is `false`;
  `mismatch_note` is `Some("body is not JSON")`; `violation_note` is `None`;
  `matches(&Body::Empty)` is `false`.
- `json_schema_other_matchers_have_no_violation_note`:
  `BodyMatcher::Regex("^a".into()).violation_note(&Body::Text("b".into()))`
  is `None` (the method is JsonSchema-only).
- `json_schema_clone_and_debug_derive`: `let m = matcher.clone();` compiles
  and `format!("{m:?}")` contains `"JsonSchema"`.
- `json_schema_file_constructor_fails_closed`:
  `BodyMatcher::json_schema_file("/nonexistent/dir/nope.json".into(),
  SchemaSyntax::Auto)` returns `Err` whose `Display` names the path
  (assert on the error string containing `nope.json`).

**Acceptance:**
- `cargo test -p camel-component-mock` exits 0 with the new tests green.
- `cargo clippy -p camel-component-mock --all-targets -- -D warnings` exits 0.
- `cargo fmt --check -p camel-component-mock` exits 0.

- [x] 1.1

## camel-cli document grammar

### Task 1.2: jsonSchema grammar arm, document-directory threading, load rejections

**Files:**
- `crates/camel-cli/src/commands/test/document.rs` (modified)
- `crates/camel-cli/src/commands/test/document_parse.rs` (modified)
- `crates/camel-cli/src/commands/test/document_tests/schema.rs` (new)
- `crates/camel-cli/src/commands/test/document_tests.rs` (modified — register
  the `schema` module)

**Steps:**
1. In `document.rs`, extend `is_body_matcher_key` with `"jsonSchema"`.
2. Add a scoped document-directory context (the standard channel for
   context-free serde field deserializers — see design.md D3):
   ```rust
   /// Base directory the in-flight parse resolves `jsonSchema` paths
   /// against. Set at the start of [`parse_test_document_at`] and cleared
   /// by a Drop guard (panic-safe: a panic inside `from_str` cannot leave
   /// a stale base on the thread); serde field deserializers cannot take
   /// context parameters.
   thread_local! {
       static SCHEMA_BASE: std::cell::RefCell<std::path::PathBuf> =
           const { std::cell::RefCell::new(std::path::PathBuf::new()) };
   }

   /// The base directory for `jsonSchema` path resolution of the
   /// in-flight parse (empty → CWD-relative, as `PathBuf::new().join`
   /// yields the relative path — the inline-text-parse semantics).
   fn schema_base() -> std::path::PathBuf {
       SCHEMA_BASE.with(|base| base.borrow().clone())
   }

   /// Clears [`SCHEMA_BASE`] when the parse call ends, by panic or return.
   struct SchemaBaseGuard;
   impl Drop for SchemaBaseGuard {
       fn drop(&mut self) {
           SCHEMA_BASE.with(|base| base.borrow_mut().clear());
       }
   }
   ```
3. Add the primary parse entry (above `parse_test_document`):
   ```rust
   /// [`parse_test_document`] with an explicit document directory: the
   /// base every relative `jsonSchema` path resolves against. The runner
   /// path (`parse_document`) always passes the document's parent
   /// directory; nothing reads the ambient CWD implicitly.
   ///
   /// # Errors
   ///
   /// Same as [`parse_test_document`].
   pub fn parse_test_document_at(
       text: &str,
       doc_dir: &std::path::Path,
   ) -> Result<TestDocument, TestDocError> {
       SCHEMA_BASE.with(|base| *base.borrow_mut() = doc_dir.to_path_buf());
       let _guard = SchemaBaseGuard;
       parse_test_document(text)
   }
   ```
   and keep `parse_test_document(text)` unchanged in behavior (its
   `SCHEMA_BASE` stays empty → `.` — documented CWD semantics for
   inline-text callers). Its doc comment gains one sentence: relative
   `jsonSchema` paths resolve against the document directory set by
   [`parse_test_document_at`]; inline-text parses use `.`.
4. In `document_parse.rs`, route the unit-tier arm through the new entry:
   ```rust
   let doc_dir = path.parent().filter(|p| !p.as_os_str().is_empty())
       .unwrap_or_else(|| std::path::Path::new("."));
   parse_test_document_at(text, doc_dir)
   ```
5. In `body_matcher_from_map`, add the `"jsonSchema"` arm. Parse the payload
   strictly (every rejection is
   `TestDocError::InvalidMatcher` and names the `field` argument and the
   `jsonSchema` key):
   - payload not an object → `{field}: `jsonSchema` must be an object with
     a `path``;
   - `path` missing, not a string, or empty → `{field}: `jsonSchema`
     requires a non-empty `path``;
   - unknown member present → `{field}: unknown `jsonSchema` field
     `{member}`; expected `path` and optional `type``;
   - `type` present and null/non-string → `{field}: `jsonSchema` `type`
     must be `json` or `yaml``;
   - `type` a string outside `{"json","yaml"}` → `{field}: `jsonSchema`
     `type` `{value}` for `{path_str}` is not supported; supported types:
     json, yaml``;
   - `Path::new(path_str).is_absolute()` → `{field}: `jsonSchema` path
     `{path_str}` must be relative to the test document's directory`;
   - otherwise resolve `schema_base().join(path_str)` (the base is `.` when
     unset, so inline-text parses resolve against the CWD), map `type`:
     `Some("json")` → `SchemaSyntax::Json`, `Some("yaml")` →
     `SchemaSyntax::Yaml`, `None` → `SchemaSyntax::Auto`; call
     `BodyMatcher::json_schema_file(resolved, syntax)` and map `Err(e)` to
     `TestDocError::InvalidMatcher(format!("{field}: `jsonSchema` failed to
     load `{}`: {e}", resolved.display()))`.
   Implement the strict-field walk with a single `serde_json::Map`
   iteration (mirroring `body_entry_shape_error`'s style): first classify
   `path` (required), then iterate the remaining members rejecting unknowns,
   then validate `type`. Keep messages exact per the strings above.
6. In `parse_header_value`, add the `"jsonSchema"` arm next to the existing
   `"jsonSubset"` arm: `Err(TestDocError::InvalidMatcher(format!("{field}:
   `jsonSchema` applies to bodies only")))`.
7. Create `document_tests/schema.rs` with the tests listed below, writing
   fixture schemas with `tempfile::tempdir()` and parsing through
   `parse_test_document_at(&yaml, doc_dir)` where `doc_dir` is the tempdir
   holding the document text (an absolute path — this pins
   CWD-independence, since resolution against the test process's CWD would
   not find the fixture). Register `mod schema;` in
   `document_tests.rs`. Follow the existing helpers' YAML-document shape
   (`routes` + `expects` with `bodies`, or reply-only documents).

**Tests:** (in `document_tests/schema.rs` unless stated; fixture schema for
the matching cases:
`{"type":"object","required":["id"],"properties":{"id":{"type":"string"}}}`)
- `json_schema_valid_body_matches`: schema file `order.schema.json` in the
  doc dir; document `bodies: [{jsonSchema: {path: "order.schema.json"}}]`;
  `parse_test_document_at` succeeds;
  `doc.expects["result"].bodies[0].matches(&Body::Json(json!({"id":"x"})))`
  is `true`, and against `Body::Json(json!({}))` is `false` with
  `format!("{}", matcher)` containing `order.schema.json`.
- `json_schema_yaml_by_extension`: same content as YAML at
  `order.schema.yaml`; expectation `{jsonSchema: {path: "order.schema.yaml"}}`
  parses and the valid body matches.
- `json_schema_type_override_selects_yaml_parser`: schema content written
  to `order.schema.txt` verbatim in YAML block style with unquoted scalars
  (invalid as JSON because `object`/`string` are bare tokens):
  ```yaml
  type: object
  required: [id]
  properties:
    id:
      type: string
  ```
  expectation `{jsonSchema: {path: "order.schema.txt", type: "yaml"}}`
  parses and the valid body matches (the extension rule alone would have
  JSON-parsed it and failed the parse).
- `json_schema_two_matchers_resolve_same_base`: document with two
  `jsonSchema` expectations in one `bodies` list (two fixture files
  `a.schema.json`, `b.schema.json` in the doc dir); both parse and both
  resolve against the doc dir — pins that the base survives more than one
  matcher read in a single document.
- `json_schema_missing_file_rejected`: expectation naming
  `missing.schema.json`; `parse_test_document_at` returns
  `TestDocError::InvalidMatcher` whose message contains
  `missing.schema.json`.
- `json_schema_unreadable_file_rejected`: a directory created at the
  declared schema path (`order.schema.json/` as a dir); parse error message
  contains the path.
- `json_schema_out_of_dir_ref_rejected`: schema with
  `$ref: "../outside.json#/$defs/x"`; parse error message contains the
  schema path and `rejected` (confinement wording).
- `json_schema_missing_in_dir_ref_rejected`: schema with
  `$ref: "defs.json#/$defs/x"`, no `defs.json` in the dir; parse error
  message contains the schema path.
- `json_schema_invalid_schema_rejected`: malformed JSON content; parse error
  names the path. Second document with `{"maximum": "five"}`; parse error
  names the path (compile-stage rejection).
- `json_schema_unsupported_type_rejected`: expectation
  `{jsonSchema: {path: "order.xsd", type: "xsd"}}`; parse error message
  contains `xsd`, `json`, `yaml`, and `order.xsd`.
- `json_schema_absolute_path_rejected`: expectation
  `{jsonSchema: {path: "/abs/order.schema.json"}}`; parse error contains the
  path and `relative`.
- `json_schema_unknown_field_rejected`: expectation
  `{jsonSchema: {path: "order.schema.json", format: "draft7"}}` (schema
  file exists); parse error contains `format` and `jsonSchema`.
- `json_schema_missing_path_rejected`: expectation `{jsonSchema: {type:
  "json"}}`; parse error contains `path`. Expectation `{jsonSchema:
  "order.schema.json"}` (non-object); parse error contains `object`.
- `json_schema_non_string_type_rejected`: expectation `{jsonSchema: {path:
  "order.schema.json", type: 42}}`; parse error contains `type`. Same for
  `type: null`.
- `json_schema_on_header_rejected`: `headers: {X-Id: {jsonSchema: {path:
  "order.schema.json"}}}`; parse error contains `bodies only` and the field.
- `json_schema_reply_body_grammar`: reply-only document with
  `expectReply: {body: {jsonSchema: {path: "order.schema.json"}}}`; parses;
  `doc.inputs[0].expect_reply.as_ref().unwrap().body` is a `JsonSchema`
  matcher that matches the valid body.
- `json_schema_relative_to_doc_dir_not_cwd`: doc text and schema placed at
  `nested/` inside the tempdir; `parse_test_document_at(&yaml,
  tempdir.join("nested"))` succeeds while the process CWD is elsewhere
  (assert on parse success — resolution against the CWD would fail to find
  the schema).

**Acceptance:**
- `cargo test -p camel-cli --no-default-features --features
  flavor-regular,exec document_tests` exits 0 with the new tests green and
  every pre-existing parsing/reply test still green (jsonSubset unchanged).
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- `cargo fmt --check -p camel-cli` exits 0.
- `thread_local` usage is confined to `SCHEMA_BASE`, `schema_base`, and
  `SchemaBaseGuard` in steps 2–3; no other call site reads `SCHEMA_BASE`.

- [x] 1.2

## camel-component-mock failure surfaces

### Task 1.3: violation notes on the ordered and anyOrder assertion paths

**Files:**
- `crates/components/camel-mock/src/assert.rs` (modified)
- `crates/components/camel-mock/src/tests.rs` (modified)

**Steps:**
1. Widen `received_body_with_note(body: &Body, note: Option<&'static str>)`
   to `note: Option<&str>` (call sites adapt with `.map(String::as_str)` or
   by passing `note.as_deref()`).
2. At the ordered `BodyExpectation::Matcher` failure site (assert.rs ~line
   510), build the note as the shape note else the violation note:
   ```rust
   let note = match matcher.mismatch_note(received_bodies[i]) {
       Some(shape) => Some(shape.to_string()),
       None => matcher.violation_note(received_bodies[i]),
   };
   ```
   and pass `received: received_body_with_note(received_bodies[i],
   note.as_deref())`. The `BodyMatcherFailed` Display already renders
   `expected {matcher}, got {received}` — the schema path arrives via the
   matcher's `Display`, the violation via the appended note.
3. Add the field `diagnostic: Option<String>` to
   `MockAssertionError::BodyNotFound` (doc: "Deterministic candidate for a
   failing `jsonSchema` matcher under `anyOrder`: the first unmatched body
   in receive order with its shape note or first violation. `None` for
   exact expectations and every other matcher."). Extend the `Display` arm
   to append ` (candidate: {diagnostic})` when `Some`.
4. In the anyOrder arm's `None` branch (assert.rs ~line 487), when the
   failing expectation is `BodyExpectation::Matcher(m)` and `m` is
   `BodyMatcher::JsonSchema`, compute the candidate from `unmatched` (the
   list after consumption — the failing scan found no match, so every
   candidate failed the expectation): take `unmatched.first()`, render its
   note chain (`mismatch_note(..).map(str::to_string).or_else(||
   m.violation_note(body))`) with the body rendered whole via
   `received_body_with_note(body, note.as_deref())`, and pass it as
   `diagnostic`. All other expectations pass `diagnostic: None`. Matching
   semantics do not change.
5. Add the tests below to `tests.rs`, using the existing component harness
   conventions of that file (or direct `assert`-module evaluation where the
   harness allows; the mock expectation builders already accept
   `BodyExpectation::Matcher`).

**Tests:**
- `body_matcher_failed_renders_schema_path_and_violation`: endpoint
  expectation with a `JsonSchema` matcher (inline-compiled schema requiring
  string `id`); one received body `{}` → the
  `MockAssertionError::BodyMatcherFailed` Display contains
  `jsonSchema`, the matcher `path`, and the first violation in the
  `"{message} at {instance_path}"` shape.
- `body_matcher_failed_non_json_renders_shape_note`: same matcher, received
  body `not json` → Display contains `body is not JSON` and the path.
- `any_order_json_schema_diagnoses_unconsumed_body`: anyOrder endpoint, two
  identical `JsonSchema` expectations, received bodies `[{"id":"ok"}, {}]`
  in that order → the `BodyNotFound` Display contains the schema path and
  the violation of the second body (`{}`), not the first.
- `any_order_json_schema_non_json_diagnoses_shape_note`: anyOrder endpoint,
  one `JsonSchema` expectation, one received body `not json` → the
  `BodyNotFound` Display contains the schema path and `body is not JSON`.
- `any_order_exact_expectations_have_no_candidate`: anyOrder endpoint, two
  identical exact-body expectations, neither received → `BodyNotFound`
  Display contains no `candidate:` clause.

**Acceptance:**
- `cargo test -p camel-component-mock` exits 0 with the new tests green and
  all pre-existing assertion tests green.
- `cargo clippy -p camel-component-mock --all-targets -- -D warnings`
  exits 0.

- [x] 1.3

## camel-cli reply path

### Task 1.4: reply evaluator renders the note chain; driver tests

**Files:**
- `crates/camel-cli/src/commands/test/runner.rs` (modified)
- `crates/camel-cli/src/commands/test/driver_tests.rs` (modified)

**Steps:**
1. In `evaluate_reply_expectation` (runner.rs ~line 588), when the body
   matcher fails, compute the note exactly like the ordered assert path:
   ```rust
   let note = match matcher.mismatch_note(&message.body) {
       Some(shape) => Some(shape.to_string()),
       None => matcher.violation_note(&message.body),
   };
   ```
   and append ` ({note})` to the existing
   `reply body mismatch: expected {matcher}, actual {body}` message when
   `Some` (non-JSON reply → shape note; violating JSON reply → first
   violation). Failure-classification and exit codes are unchanged.
2. In `driver_tests.rs`, switch the document-parse helper `parse_doc_at`
   (~line 2099) from `document::parse_test_document(&text)` to
   `document::parse_test_document_at(&text, path.parent().unwrap_or(
   std::path::Path::new(".")))` — behavior-preserving for every existing
   driver document (none uses `jsonSchema`), and it routes jsonSchema
   documents through the document-directory base. Do not touch any other
   driver harness function.
3. In `driver_tests.rs`, add the tests below following that file's existing
   document+route harness. `evaluate_reply_expectation` is private to
   `runner.rs` and not reachable from `driver_tests.rs` (a sibling module);
   assert through the public `runner::run_test_doc(&doc, doc_dir)` instead:
   pass the temp dir holding the document and fixture schema, and inspect
   `TestDocResult::endpoint_results` — match the row whose `endpoint` field
   (populated with the label) starts with `reply[0] direct:` and assert on
   its `outcome`.

**Tests:** (driver-level, `driver_tests.rs`; assertions run through
`run_test_doc(...)` → `endpoint_results`, matching the `reply[0] direct:`
row)
- `reply_json_schema_conforming_reply_passes`: document with
  `expectReply: {body: {jsonSchema: {path: "reply.schema.json"}}}` and the
  schema fixture written next to the document in the temp dir; a lean route
  replying `{"id":"r-1"}` → the reply row's outcome is `Ok(())`.
- `reply_json_schema_violating_reply_fails_with_violation`: same document,
  route replying `{}` → the reply row's outcome is `Err(msg)` where `msg`
  contains `reply body mismatch`, `jsonSchema`, `reply.schema.json`, and
  ` at ` (the `{message} at {instance_path}` shape).
- `reply_json_schema_non_json_reply_fails_with_shape_note`: route replying
  text `not json` → `Err(msg)` containing `reply body mismatch`,
  `reply.schema.json`, and `body is not JSON`.
- `reply_json_subset_unchanged`: same harness, expectation
  `expectReply: {body: {jsonSubset: {id: "r-1"}}}` against the replying
  route → the reply row's outcome is `Ok(())`, and against a route
  replying `{"id":"other"}` → `Err` (guards deliverable: `jsonSubset`
  behavior unchanged).

**Acceptance:**
- `cargo test -p camel-cli --no-default-features --features
  flavor-regular,exec driver_tests` exits 0 with the new tests green.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 1.4

## Docs and domain map

### Task 1.5: testkit docs section and CONTEXT-MAP consumer note

**Files:**
- `docs/src/testing/camel-test.md` (modified)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. In `docs/src/testing/camel-test.md`, extend the body-matcher list line
   (line ~61) with `jsonSchema`, and add a `## JSON Schema assertions`
   section after the endpoint-expectations matcher prose containing:
   - the grammar: `jsonSchema: {path: <file>, type: <json|yaml>}`; `type`
     optional; omitted follows the file extension (`.yaml`/`.yml` are YAML,
     everything else JSON); `path` is relative to the test document's
     directory; absolute paths are rejected.
   - a runnable example: one schema file (`order.schema.json` requiring a
     string `id`) and one `*.test.yaml` document snippet using
     `bodies: [{jsonSchema: {path: "order.schema.json"}}]`.
   - load behavior: the schema compiles once when the document loads;
     missing/unreadable files, unresolvable `$ref`s, invalid schemas, and
     unsupported `type` values fail at load (exit 2) naming the path.
   - failure shape: assertion failures (exit 1) name the schema path and
     the first violation as `{message} at {instance_path}`; a non-JSON body
     is a non-match.
   - the limitation note: `xsd` and `protobuf` schemas have no engine yet —
     declaring them fails at load naming the supported set (json, yaml).
   - one sentence pointing at ADR-0100 confinement: references resolve only
     inside the schema's directory (json-schema-validation spec).
   Follow ste-writing prose rules (short sentences, no filler).
2. In `CONTEXT-MAP.md`, extend the ADR-0100 index line's consumer list with
   the mock testkit (`camel-component-mock` via the unit-tier `jsonSchema`
   matcher).

**Tests:**
- `mdbook` build: `cd docs && mdbook test` exits 0 (the added blocks are
  yaml, not rust doctests — they must not break the build).

**Acceptance:**
- `cd docs && mdbook test` exits 0.
- The section contains a schema file example and a document example that
  match the grammar implemented in 1.2 byte-for-byte on the key names
  (`jsonSchema`, `path`, `type`).

- [x] 1.5

## Component lanes

### Task 1.6: declare the camel-cli and camel-component-mock lanes

**Files:**
- `AGENTS.md` (modified)

**Steps:**
1. In the `## COMPONENT LANES` ```lanes``` block, add the two rows
   byte-identical to the mission order, under the existing own-tests
   section with a short comment line for the camel-cli declaration:
   ```
   # --- camel-cli: declared deliberately (mission 601) — the lane sweep
   #     fail-safe asked for this declaration; matches ci.yml:288
   camel-cli | local | cargo test -p camel-cli --no-default-features --features flavor-regular,exec --no-fail-fast
   # --- camel-component-mock: own tests/ suite
   camel-component-mock | local | cargo test -p camel-component-mock
   ```
2. Run the registry parser locally to confirm the rows are picked up.

**Tests:**
- `GATES_ONLY=lanes bash scripts/gates-local.sh` exits 0 in the worktree
  and prints `PASS lane camel-cli ::` and
  `PASS lane camel-component-mock ::`, with no `NO LANE for camel-cli`
  line (the lane phase of `gates-local.sh` parses the real `AGENTS.md`
  table; `scripts/gates-lanes-test.sh` is hermetic and cannot print lane
  rows — it stays as the STAGE 4 regression run only).

**Acceptance:**
- `GATES_ONLY=lanes bash scripts/gates-local.sh` exits 0 with both new
  lanes passing and no `NO LANE` warning.
- `bash scripts/gates-lanes-test.sh` exits 0 (hermetic regression, guards
  the lane machinery itself).
- `git -C <worktree> diff AGENTS.md` shows exactly the two rows plus the
  comment lines and nothing else.

- [x] 1.6
