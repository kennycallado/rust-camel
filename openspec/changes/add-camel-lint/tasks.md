# Tasks: add-camel-lint

## Phase 1: Engine scaffolding

### camel-lint

#### Task 1.1: Create camel-lint crate scaffold + embed route schema

**Files:**
- `crates/camel-lint/Cargo.toml` (new)
- `crates/camel-lint/src/lib.rs` (new)
- `crates/camel-lint/schema/route-schema.json` (new — byte copy of `schemas/dsl/route-schema.json`)
- `Cargo.toml` (modified — add `crates/camel-lint` to workspace `[workspace] members`, `default-members`, and `[workspace] deps` if a workspace-dep alias is wanted)
- `[workspace.dependencies]` block of root `Cargo.toml` (modified — ensure `noyalib`, `jsonschema`, `ariadne` are present as workspace deps if not already)

**Steps:**
1. Add `ariadne = "0.4"` (pin to the latest 0.x compatible release at implementation time) to root `Cargo.toml`'s `[workspace.dependencies]` block (it is not currently a workspace dep). Create `crates/camel-lint/Cargo.toml` with `edition = "2024"`, `rust-version = "1.89"`, and dependencies: `camel-api = { workspace = true }`, `noyalib = { workspace = true }`, `jsonschema = { workspace = true }`, `ariadne = { workspace = true }`, `serde = { workspace = true, features = ["derive"] }`, `serde_json = { workspace = true }`, `thiserror = { workspace = true }`. No dependency on `camel-core`, `camel-dsl`, or `camel-cli`.
2. Copy `schemas/dsl/route-schema.json` verbatim to `crates/camel-lint/schema/route-schema.json`.
3. In `crates/camel-lint/src/lib.rs`, add `pub const ROUTE_SCHEMA: &str = include_str!("../schema/route-schema.json");` and a `pub mod` skeleton for `diagnostic`, `document`, `route_view`, `rule`, `engine` (empty module files created in later tasks).
4. Add `crates/camel-lint` to the workspace `members` and `default-members` arrays in root `Cargo.toml`.
5. Run `cargo build -p camel-lint` to confirm the crate compiles with the embedded schema.

**Tests:**
- `route_schema_is_embedded`: act = read `ROUTE_SCHEMA` constant → assert it is non-empty and starts with `{` and ends with `}`. command = `cargo test -p camel-lint --lib route_schema_is_embedded`. expected = pass after step 5.

**Acceptance:**
- `cargo build -p camel-lint` exits 0.
- `crates/camel-lint/Cargo.toml` contains no `camel-core`/`camel-dsl`/`camel-cli` dependency (verifiable by `grep -E 'camel-(core|dsl|cli)' crates/camel-lint/Cargo.toml` returning nothing).
- `ROUTE_SCHEMA` constant is non-empty.

- [x] 1.1

#### Task 1.2: Core types — Span, Severity, Diagnostic, Rule trait, LintEngine skeleton

**Files:**
- `crates/camel-lint/src/diagnostic.rs` (new)
- `crates/camel-lint/src/rule.rs` (new)
- `crates/camel-lint/src/engine.rs` (new)
- `crates/camel-lint/src/lib.rs` (modified — declare modules + re-exports)

**Steps:**
1. In `diagnostic.rs`, define:
   - `#[derive(Clone, Debug, PartialEq, Eq)] pub struct Span { pub start: usize, pub end: usize }` with a constructor `Span::new(start: usize, end: usize) -> Self`.
   - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Severity { Error, Warning, Info }`.
   - `#[derive(Clone, Debug, PartialEq, Eq)] pub enum DiagnosticCode { RSyn, RSchema, RUriKnown(UriKnownSubCode), RSecret, RDeprecated }` where `UriKnownSubCode` is `#[derive(Clone, Debug, PartialEq, Eq)] pub enum UriKnownSubCode { UnverifiedScheme, UnknownOption, MissingRequiredOption, KindMismatch }`.
   - `#[derive(Clone, Debug)] pub struct Diagnostic { pub code: DiagnosticCode, pub severity: Severity, pub span: Span, pub message: String, pub fix: Option<Fix> }`.
   - `#[derive(Clone, Debug)] pub struct Fix { pub span: Span, pub replacement: String }`.
2. In `rule.rs`, define `pub trait Rule: Send + Sync { fn analyze(&self, doc: &Document, catalog: &dyn camel_api::component_metadata::ComponentMetadataCatalog) -> Vec<Diagnostic>; fn code(&self) -> DiagnosticCode; }`.
3. In `engine.rs`, define `pub struct LintEngine { catalog: std::sync::Arc<dyn camel_api::component_metadata::ComponentMetadataCatalog>, rules: Vec<Box<dyn Rule>> }` with `pub fn new(catalog: std::sync::Arc<dyn camel_api::component_metadata::ComponentMetadataCatalog>) -> Self` (empty rules vector), `pub fn with_rule(mut self, rule: Box<dyn Rule>) -> Self`, and `pub fn lint(&self, source: &str) -> Vec<Diagnostic>` that builds a `Document` via `Document::parse` (Task 1.3 — which ALWAYS returns a `Document`, never errors), runs each rule, and returns the concatenated diagnostics. `lint` returns `Vec<Diagnostic>` (NOT `Result`): parse failures are NOT engine errors — they are captured in `Document.parse_failure` (Task 1.3) and surfaced by R-SYN. Add `thiserror = { workspace = true }` to `Cargo.toml` (used by `Document::apply_fix`'s error type in Task 3.3). NOTE: `lint()` depends on `Document::parse` from Task 1.3; the two engine behavior tests below are therefore authored in Task 1.3's test block (where `Document::parse` exists), NOT in Task 1.2. Also add `pub fn with_default_rules(self) -> Self` (registers all five rules — fully populated once Task 2.5 lands; until then it registers the rules implemented so far).
4. Re-export the public types from `lib.rs` (`pub use diagnostic::*; pub use rule::*; pub use engine::*;`).
5. Add a test helper module `crates/camel-lint/src/test_support.rs` declared in `lib.rs` as `#[cfg(test)] pub(crate) mod test_support;` (cfg-gated so it does NOT leak into the crate's published public API, and `pub(crate)` so all in-crate `#[cfg(test)]` modules — including the rule modules' test submodules — can use it): define `pub(crate) struct StubCatalog { entries: std::collections::HashMap<String, camel_api::component_metadata::ComponentMetadata> }` with `pub(crate) fn empty() -> Self`, `pub(crate) fn with(mut self, scheme: &str, meta: camel_api::component_metadata::ComponentMetadata) -> Self`, and a full `impl camel_api::component_metadata::ComponentMetadataCatalog for StubCatalog` (`get_metadata` → entries.get().cloned(), `schemes` → keys collected, `all_metadata` → values collected). All Phase-2 rule tests use `StubCatalog` to inject controlled metadata.

**Tests:**
- `stub_catalog_trait_object_safe`: setup = `StubCatalog::empty().with("timer", ComponentMetadata::minimal("timer"))` → action = bind as `&dyn ComponentMetadataCatalog` → assert `get_metadata("timer").is_some()` and `get_metadata("log").is_none()` and `schemes() == ["timer"]`. command = `cargo test -p camel-lint --lib stub_catalog_trait_object_safe`. expected = pass at Task 1.2.

**Acceptance:**
- `cargo build -p camel-lint` exits 0.
- `cargo clippy -p camel-lint -- -D warnings` exits 0.
- `lint` signature is `fn lint(&self, source: &str) -> Vec<Diagnostic>` (verifiable by `grep -n 'fn lint' crates/camel-lint/src/engine.rs` showing no `Result`).
- `StubCatalog` implements all three `ComponentMetadataCatalog` trait methods (verifiable by `grep -cE 'fn get_metadata|fn schemes|fn all_metadata' crates/camel-lint/src/test_support.rs` returning 3).

- [x] 1.2

#### Task 1.3: LintRoute view + schema-driven CST traversal (captures all URI-bearing locations with spans)

**Files:**
- `crates/camel-lint/src/route_view.rs` (new)
- `crates/camel-lint/src/document.rs` (new)
- `crates/camel-lint/src/engine.rs` (modified — call `Document::parse`)

**Steps:**
1. In `route_view.rs`, define the span-carrying view. A route node is EITHER an endpoint (a `to`/`from`/`uri` leaf with options) OR a structural branch (a `choice`/`multicast`/`scatter_gather` container with children but no URI of its own):
   - `#[derive(Clone, Debug)] pub struct Spanned<T> { pub value: T, pub span: Span }`.
   - `#[derive(Clone, Debug)] pub struct LintRoute { pub from: Option<Spanned<String>>, pub nodes: Vec<Spanned<LintNode>> }` with `pub fn endpoints(&self) -> Vec<Endpoint>` that returns the route-level `from` synthesized as `Endpoint { uri: self.from.clone().map(|s| Spanned{value: s.value, span: s.span}), options: vec![] }` (if present) FOLLOWED BY every `LintNode::Endpoint` node (including those nested in branches), in source order — so rules iterate a flat endpoint list covering `from` + all `to`/`uri` at any depth. (Return owned `Vec<Endpoint>` by value; cloning the spans/strings is acceptable — the endpoint list is small.)
   - `#[derive(Clone, Debug)] pub enum LintNode { Endpoint(Endpoint), Branch { kind: Spanned<String>, children: Vec<Spanned<LintNode>> } }`.
   - `#[derive(Clone, Debug)] pub struct Endpoint { pub uri: Spanned<String>, pub options: Vec<LintOption> }`.
   - `#[derive(Clone, Debug)] pub struct LintOption { pub key: Spanned<String>, pub value: Option<Spanned<String>> }`.
2. Implement `LintOption::parse_from_query(uri_value: &str, uri_span: Span) -> Vec<LintOption>` that splits the `?key=value&key2=value2` query portion of a URI into `LintOption`s whose `key`/`value` spans are computed as byte offsets into the ORIGINAL source by adding the query token's offset relative to `uri_value`'s start to `uri_span.start`.
3. In `document.rs`, define `pub struct ParseFailure { pub span: Span, pub message: String }` and `pub struct Document { pub raw: String, pub route_view: LintRoute, pub parse_failure: Option<ParseFailure> }`, and `impl Document { pub fn parse(source: &str) -> Document }` that:
   - parses `source` with the noyalib CST API directly (`noyalib::cst::parse_document` + `Document::span_at`/`key_span`/`replace_span`, and `noyalib::error::Location`/`Error::location()` for error spans — these exist at noyalib 0.0.18); on a syntax error, returns a `Document` with `parse_failure = Some(ParseFailure { span, message })` (carrying BOTH the byte-exact span AND the parser's message, so R-SYN can report both) and an empty `LintRoute`; the function ALWAYS returns a `Document` — it never returns `Err` (parse failure is data, not an engine error);
   - on success, walks the CST to populate `LintRoute` capturing: the top-level `from` value (string node + its byte span); each entry under `steps` (the `to`/`uri` scalar + span, plus options parsed from the URI query and from any sibling option-map keys); and nested child steps by recursing into every step-container the schema defines — `choice` (and its `when`/`otherwise` branches), `multicast`, and `scatter_gather.endpoints` (the containers present in `schemas/dsl/route-schema.json`; `pipeline` does NOT exist and must not be referenced) — discovered by reading `ROUTE_SCHEMA` so a new container requires only re-syncing the embedded copy.
4. Drive the container-name set from `ROUTE_SCHEMA` via a concrete algorithm. Provide a helper `fn uri_bearing_keys(schema: &serde_json::Value) -> std::collections::HashSet<String>` that: (a) starts at the schema root; (b) recursively descends through `properties`, `items`, `additionalProperties`, and the composition keywords `anyOf`/`oneOf`/`allOf`, resolving local `$ref` pointers into `$defs` (e.g. `#/$defs/step` — resolve by splitting the fragment on `/` and walking from the schema root); (c) classifies each property name into ONE of three shapes — (i) SCALAR-URI: the property's subschema allows `"type": "string"` directly (e.g. `to`, `from`, `uri`); (ii) URI-ARRAY: the property's subschema is `{type: array, items: {type: string}}` — the array ITEMS are URIs (e.g. `scatter_gather.endpoints`, whose `ScatterGatherData.endpoints` is `items: {type: string}`); (iii) CONTAINER: the property's subschema is an object/array that holds further steps (`steps`, `choice`, `when`, `otherwise`, `multicast`, `scatter_gather`). Record names in two sets: `scalar_or_array_uri_keys` (shapes i + ii) and `container_keys` (shape iii). The CST walker then: recurses into any mapping key in `container_keys`; for a key in `scalar_or_array_uri_keys` of shape (i), emits ONE `Endpoint` whose `uri` is the string value's span; for a key of shape (ii), iterates the YAML/JSON sequence and emits ONE `Endpoint` per string item (each item's span is the item's own node span — e.g. each `direct:a` / `direct:b` entry in an `endpoints:` array). (This is the schema-instance lockstep traversal flagged for tasks.md by the spec-bless.)
5. Wire `LintEngine::lint` to call `Document::parse(source)` (which always returns a `Document`). The engine passes the `Document` (including its `parse_failure`, if set) to each rule; R-SYN (Task 2.1) reads `parse_failure` to emit its diagnostic, and the semantic rules (Tasks 2.2–2.5) check `doc.parse_failure.is_some()` and return an empty `Vec<Diagnostic>` early (they skip a document that failed to parse). `lint` returns the concatenated `Vec<Diagnostic>` — never `Err`.

**Tests:**
- `from_uri_span_is_byte_exact`: setup = source `"from: direct:start\n"` where `direct:start` starts at byte offset 12 → action = `Document::parse(source)` → assert `route_view.from.unwrap().span.start == 12`. command = `cargo test -p camel-lint --lib from_uri_span_is_byte_exact`.
- `nested_child_step_uri_captured`: setup = source with a `multicast` (or `choice.when`) containing a child `- to: log:nested` → action = parse → assert the child step's `uri.value == "log:nested"` and its span is distinct from the parent. command = `cargo test -p camel-lint --lib nested_child_step_uri_captured`.
- `scatter_gather_endpoints_captured`: setup = source with a `scatter_gather` step whose `endpoints` array holds `direct:a` and `direct:b` → action = parse → assert `route_view.endpoints()` yields both URIs as `Endpoint.uri` values with distinct spans. command = `cargo test -p camel-lint --lib scatter_gather_endpoints_captured`.
- `option_key_value_spans_byte_exact`: setup = source `"steps:\n  - to: timer:foo?period=1s\n"` where `period` starts at offset 30 and `1s` at 37 → action = parse → assert the option key span starts at 30 and value span starts at 37. command = `cargo test -p camel-lint --lib option_key_value_spans_byte_exact`.
- `partial_input_records_failure_span`: setup = source `"steps:\n  - to: timer:foo\n  bad: ["` → action = parse → assert returns a `Document` with `parse_failure.is_some()` (with both a span and a non-empty message) and an empty `route_view` (no panic). command = `cargo test -p camel-lint --lib partial_input_records_failure_span`.
- `engine_with_no_rules_returns_empty`: setup = `LintEngine::new(Arc::new(StubCatalog::empty()))` → action = `engine.lint("from: direct:start\nsteps:\n  - to: log:out\n")` → assert the returned `Vec<Diagnostic>` is empty. command = `cargo test -p camel-lint --lib engine_with_no_rules_returns_empty`.
- `engine_tolerates_partial_input`: setup = engine with no rules → action = `engine.lint("from: direct:start\n  unclosed: [")` (malformed YAML) → assert returns an empty `Vec<Diagnostic>` (no panic; `Document.parse_failure` is set for R-SYN to report when rules are present). command = `cargo test -p camel-lint --lib engine_tolerates_partial_input`.

**Acceptance:**
- `cargo test -p camel-lint --lib` passes (all seven tests above — the five Document/parse tests plus the two engine behavior tests moved from Task 1.2).
- `cargo clippy -p camel-lint -- -D warnings` exits 0.
- No `camel-dsl` import appears in `route_view.rs`/`document.rs` (`grep -rn 'camel_dsl\|camel-dsl' crates/camel-lint/src/` returns nothing).

- [x] 1.3

#### Task 1.4: Schema byte-equality xtask gate + architecture boundary assertion

**Files:**
- `scripts/xtask/src/main.rs` (modified — extend the `schema --check` subcommand to compare the camel-lint copy)
- `crates/camel-core/tests/hexagonal_architecture_boundaries_test.rs` (modified — add a NEW dep-graph-based assertion that camel-lint has no camel-core/camel-dsl dep edge)

**Steps:**
1. In `scripts/xtask/src/main.rs`, extend the existing `schema --check` subcommand (the gate already lives in `main.rs` — there is no separate `schema.rs`) to additionally read `crates/camel-lint/schema/route-schema.json` and compare it byte-for-byte against the generated `schemas/dsl/route-schema.json`; on mismatch, print both paths and exit non-zero.
2. In `crates/camel-core/tests/hexagonal_architecture_boundaries_test.rs`, add a NEW test `camel_lint_has_no_runtime_dep`. The existing test uses source-string import scanning, which CANNOT detect a `Cargo.toml` dependency edge, so this test must use a dep-graph mechanism. The PRIMARY mechanism is network-free Cargo.toml parsing: read `crates/camel-lint/Cargo.toml` with the `toml` crate (add `toml` as a `camel-core` `[dev-dependencies]` entry if not already present — `toml` is already a workspace dep), parse the `[dependencies]` table, and assert it contains neither `camel-core` nor `camel-dsl`. (Do NOT use `cargo metadata` as the primary path: it can touch the registry index when `Cargo.lock` is stale, and a unit test must not require network. If a graph-based check is desired later, gate it behind `--offline`/`--frozen`.) Add a small helper `fn crate_declared_deps(manifest_path: &str) -> HashSet<String>`; do not claim to reuse an existing helper (none exists).

**Tests:**
- `schema_check_rejects_drift`: setup = temporarily append a byte to `crates/camel-lint/schema/route-schema.json` in a scratch copy → action = run `cargo run -p xtask -- schema --check` → assert non-zero exit naming the two divergent paths; then restore. command = `cargo run -p xtask -- schema --check` (must exit 0 on the real tree). expected = the gate passes on the committed tree.
- `camel_lint_has_no_runtime_dep`: action = run the architecture test → assert it passes (camel-lint deps exclude camel-core and camel-dsl). command = `cargo test -p camel-core --test hexagonal_architecture_boundaries_test camel_lint_has_no_runtime_dep`. expected = pass.

**Acceptance:**
- `cargo run -p xtask -- schema --check` exits 0 on the committed tree.
- `cargo test -p camel-core --test hexagonal_architecture_boundaries_test` passes (including the new assertion).
- `cargo build --workspace` exits 0.

- [x] 1.4

## Phase 2: Rules

### camel-lint

#### Task 2.1: R-SYN rule — syntax errors with byte-exact span

**Files:**
- `crates/camel-lint/src/rules/mod.rs` (new)
- `crates/camel-lint/src/rules/rsyn.rs` (new)
- `crates/camel-lint/src/engine.rs` (modified — `with_default_rules` registers all five rules; used by Phase 3 CLI)

**Steps:**
1. Create `rules/mod.rs` declaring `pub mod rsyn;` (and placeholders for the four later rules as they land).
2. In `rules/rsyn.rs`, define `pub struct RSynRule;` implementing `Rule`: `code()` returns `DiagnosticCode::RSyn`; `analyze()` inspects `doc.parse_failure` — when `Some(ParseFailure { span, message })`, returns a single `Diagnostic { code: RSyn, severity: Error, span, message, fix: None }` (the parser's message is carried verbatim); when `None`, returns an empty vec.
3. `with_default_rules` was added on `LintEngine` in Task 1.2; Task 2.1 registers `RSynRule` into it (and the other four rules as they land in Tasks 2.2–2.5).

**Tests:**
- `rsyn_reports_at_parser_error_location`: setup = `Document::parse("steps:\n  - to: timer:foo\n  bad: [")` → action = `RSynRule.analyze(&doc, &stub_catalog)` → assert exactly one `Diagnostic` with `code == RSyn`, `severity == Error`, a non-empty `message`, and `span` a single byte at the parser-reported error location (byte 27, the `b` of `bad:` — noyalib `ErrorKind::Syntax` has no sub-kind to identify the unclosed `[` specifically; the general requirement "derived from the parser's error location" governs). command = `cargo test -p camel-lint --lib rsyn_reports_at_parser_error_location`.
- `rsyn_silent_on_valid_doc`: setup = valid `Document` with `parse_failure == None` → action = `RSynRule.analyze` → assert empty vec. command = `cargo test -p camel-lint --lib rsyn_silent_on_valid_doc`.
- `rsyn_emits_and_others_skip_on_broken_doc`: setup = `LintEngine::new(Arc::new(StubCatalog::empty())).with_default_rules()` (with all five rules registered) over source `"steps:\n  - to: timer:foo\n  bad: ["` → action = `engine.lint(source)` → assert exactly one diagnostic, `code == RSyn` (R-SYN emits), and ZERO `RSchema`/`RUriKnown`/`RSecret`/`RDeprecated` diagnostics (the other rules skip a document whose `parse_failure` is set). command = `cargo test -p camel-lint --lib rsyn_emits_and_others_skip_on_broken_doc`. (Requires Task 2.5's `with_default_rules` to be complete; author the assertion now, it passes once Phase 2 is done. If run before 2.5, scope the assertion to the rules registered so far.)

**Acceptance:**
- `cargo test -p camel-lint --lib` passes including the three R-SYN tests above.
- `cargo clippy -p camel-lint -- -D warnings` exits 0.

- [x] 2.1

#### Task 2.2: R-SCHEMA rule — jsonschema validation with per-keyword anchoring

**Files:**
- `crates/camel-lint/src/rules/rschema.rs` (new)
- `crates/camel-lint/src/rules/mod.rs` (modified — add `pub mod rschema;`)

**Steps:**
1. In `rules/rschema.rs`, define `pub struct RSchemaRule;`. In `analyze`:
   - First, if `doc.parse_failure.is_some()`, return `vec![]` (R-SCHEMA cannot run on a document that failed to parse — R-SYN owns that case).
   - Convert `doc.raw` to a `serde_json::Value` via `fn raw_to_json_value(doc: &Document) -> Option<serde_json::Value>`: deserialize via `noyalib`'s serde compat shim into a YAML value then `serde_json::to_value`; on ANY conversion error return `None`. If `raw_to_json_value` returns `None`, R-SCHEMA returns `vec![]` (cannot validate an unconvertible document — no panic, no abort).
   - Build a `jsonschema::Validator` for the embedded `ROUTE_SCHEMA` using `jsonschema::validator_for` (the same pattern as `crates/camel-dsl/tests/schema_validation.rs`). Cache the compiled validator in a `std::sync::OnceLock<Validator>` so it is built once per process.
   - Validate the `serde_json::Value`; iterate `validator.iter_errors(&value)`.
   - For each error, map it to a `Span` by keyword-specific anchoring. Provide a helper `fn span_for_instance_path(doc: &Document, instance_path: &str) -> Span` that walks the noyalib CST following the JSON-pointer instance path (unescaping `~1`→`/` and `~0`→`~`), returning the span of the resolved node. For the rare keyword that has no single offending leaf, anchor on the resolved parent node. Keyword → anchor map: `type`/`enum`/`pattern`/`const`/`format` → offending value node (the instance_path node); `minimum`/`exclusiveMinimum` → offending numeric value node; `anyOf`/`oneOf` → value node; `required` → parent object node (the missing key has no span); `minItems`/`maxItems` → array node; `additionalProperties` → the offending additional KEY node. For `additionalProperties`, the offending key name is NOT in `instance_path` (which points at the parent object) — extract it from the error's `ValidationErrorKind::AdditionalProperties { unexpected }` variant (the `unexpected` set carries the offending property names), then resolve each unexpected key's span by appending it to the parent's instance_path in the CST walk.
   - Return one `Diagnostic { code: RSchema, severity: Error, span, message: <jsonschema error.to_string()>, fix: None }` per violation.

**Tests:**
- `rschema_wrong_type_reports_value`: setup = source where `steps` is a string not an array → action = `RSchemaRule.analyze` → assert a `Diagnostic` with `code == RSchema` whose span covers the string value, body carries the jsonschema `type` message. command = `cargo test -p camel-lint --lib rschema_wrong_type_reports_value`.
- `rschema_missing_required_reports_parent`: setup = a route mapping omitting a `required` property → action = analyze → assert span covers the parent mapping node. command = `cargo test -p camel-lint --lib rschema_missing_required_reports_parent`.
- `rschema_minimum_reports_numeric_value`: setup = a numeric field below the schema `minimum` where the value starts at offset 50 → action = analyze → assert span start == 50. command = `cargo test -p camel-lint --lib rschema_minimum_reports_numeric_value`.
- `rschema_anyof_failure_reports_value`: setup = a field constrained by `anyOf` whose value matches no subschema → action = analyze → assert span covers the value. command = `cargo test -p camel-lint --lib rschema_anyof_failure_reports_value`.
- `rschema_additional_properties_reports_key`: setup = a route mapping with a property not allowed by the schema (and `additionalProperties: false`) → action = analyze → assert span covers the offending key. command = `cargo test -p camel-lint --lib rschema_additional_properties_reports_key`.
- `rschema_skips_when_parse_failure`: setup = a `Document` with `parse_failure = Some(_)` → action = `RSchemaRule.analyze` → assert empty vec. command = `cargo test -p camel-lint --lib rschema_skips_when_parse_failure`.

**Acceptance:**
- `cargo test -p camel-lint --lib` passes including the six R-SCHEMA tests above.
- `cargo clippy -p camel-lint -- -D warnings` exits 0.
- `rschema.rs` contains the two helpers `raw_to_json_value` and `span_for_instance_path` (verifiable by `grep -cE 'fn raw_to_json_value|fn span_for_instance_path' crates/camel-lint/src/rules/rschema.rs` returning 2).

- [x] 2.2

#### Task 2.3: R-URI-known rule — scheme + option validation against catalog

**Files:**
- `crates/camel-lint/src/rules/ruriknown.rs` (new)
- `crates/camel-lint/src/rules/mod.rs` (modified — add `pub mod ruriknown;`)

**Steps:**
1. In `rules/ruriknown.rs`, define `pub struct RUriKnownRule;`. In `analyze`: first, if `doc.parse_failure.is_some()`, return `vec![]`. Then iterate `doc.route_view.endpoints()` (the flattened `Endpoint` list — includes the route-level `from` if present, plus every nested `to`/`uri` at any depth). For each `Endpoint` (with its spanned `uri` and `options`):
   - Split the URI into `scheme` (text before the first `:`) using the spanned value so the scheme span is a sub-span of the URI span.
   - Call `catalog.get_metadata(scheme)`:
     - `None` → emit `Diagnostic { code: RUriKnown(UnverifiedScheme), severity: Info, span: scheme_span, message: "scheme not registered in catalog; cannot verify options", fix: None }`. Emit NO option diagnostics for this step.
     - `Some(meta)` where `meta.uri_options` is empty → emit nothing (the scheme is known but minimal; nothing to validate). Emit NO option diagnostics.
     - `Some(meta)` with `uri_options` → for each `LintOption`: if its key matches no `UriOption.name` and no `UriOption.aliases` entry → emit `RUriKnown(UnknownOption)` Error on the key span; else resolve the canonical option (an alias maps to its canonical `name`) and check the value's declared `OptionKind` against the provided value. Because `OptionKind` is `#[non_exhaustive]` and camel-lint is an EXTERNAL crate, the kind check MUST use `matches!(opt.kind, OptionKind::Bool)` (or a `match` with a `_` arm that treats unknown kinds as non-erroring) — a bare exhaustive `match` will not compile. A `Bool` option given a non-boolean string (`true`/`false` case-insensitive is boolean; anything else is not) → `RUriKnown(KindMismatch)` Error on the value span. For each `UriOption` with `required == true` absent from the step's options → emit `RUriKnown(MissingRequiredOption)` Error on the URI span.

**Tests:**
- `unverified_scheme_for_absent_metadata`: setup = stub catalog without `kafka`, source with a `kafka:topic` step → action = analyze → assert exactly one `RUriKnown(UnverifiedScheme)` Info on the `kafka` token and zero option diagnostics. command = `cargo test -p camel-lint --lib unverified_scheme_for_absent_metadata`.
- `minimal_known_scheme_is_silent`: setup = stub catalog with `redis` returning `ComponentMetadata::minimal("redis")` (empty `uri_options`), source with a `redis://x` step → action = analyze → assert no `UnverifiedScheme` note and no option diagnostics. command = `cargo test -p camel-lint --lib minimal_known_scheme_is_silent`.
- `unknown_option_for_known_scheme`: setup = stub catalog with `timer` whose only option is `period` (no `frequency` alias), source `timer:foo?frequency=1s` → action = analyze → assert one `RUriKnown(UnknownOption)` Error on the `frequency` key span. command = `cargo test -p camel-lint --lib unknown_option_for_known_scheme`.
- `missing_required_option`: setup = stub catalog with `timer` declaring `period` as `required = true`, source `timer:foo` → action = analyze → assert one `RUriKnown(MissingRequiredOption)` Error on the URI span. command = `cargo test -p camel-lint --lib missing_required_option`.
- `accepted_alias_silent`: setup = stub catalog where `period` has alias `interval`, source `timer:foo?interval=1s` → action = analyze → assert no diagnostic for that option. command = `cargo test -p camel-lint --lib accepted_alias_silent`.
- `kind_mismatch_reported`: setup = stub catalog with a `bool` option, source providing a non-boolean value `maybe` → action = analyze → assert `RUriKnown(KindMismatch)` Error on the value span. command = `cargo test -p camel-lint --lib kind_mismatch_reported`.

**Acceptance:**
- `cargo test -p camel-lint --lib` passes including the six new tests.
- `cargo clippy -p camel-lint -- -D warnings` exits 0.

- [x] 2.3

#### Task 2.4: R-SECRET rule — literal secret values

**Files:**
- `crates/camel-lint/src/rules/rsecret.rs` (new)
- `crates/camel-lint/src/rules/mod.rs` (modified — add `pub mod rsecret;`)

**Steps:**
1. In `rules/rsecret.rs`, define `pub struct RSecretRule;`. In `analyze`: first, if `doc.parse_failure.is_some()`, return `vec![]`. Then iterate `doc.route_view.endpoints()`; for each endpoint with a known scheme (catalog returns `Some(meta)` with `uri_options`): for each `LintOption` whose canonical `UriOption.secret == true` and whose value is present and does NOT match an interpolation/reference marker, emit `Diagnostic { code: RSecret, severity: Warning, span: value_span, message: "secret option set to a literal value; use an interpolation reference", fix: None }`. The marker check: a value is a reference if it contains a `${` substring (env interpolation, e.g. `${VAR}`) or a `{{` substring (placeholder interpolation, e.g. `{{name}}`).
2. Do NOT emit any error for an absent secret option (that is `RUriKnown(MissingRequiredOption)` when the option is also `required`).

**Tests:**
- `literal_secret_warned`: setup = stub catalog with a `password` option `secret = true`, source setting `password=hunter2` literally → action = analyze → assert one `RSecret` Warning on the `hunter2` value span. command = `cargo test -p camel-lint --lib literal_secret_warned`.
- `interpolated_secret_silent`: setup = same catalog, source `password={{ secrets.db.password }}` → action = analyze → assert no `RSecret` diagnostic. command = `cargo test -p camel-lint --lib interpolated_secret_silent`.

**Acceptance:**
- `cargo test -p camel-lint --lib` passes including the two new tests.
- `cargo clippy -p camel-lint -- -D warnings` exits 0.

- [x] 2.4

#### Task 2.5: R-DEPRECATED rule — deprecated options

**Files:**
- `crates/camel-lint/src/rules/rdeprecated.rs` (new)
- `crates/camel-lint/src/rules/mod.rs` (modified — add `pub mod rdeprecated;`)

**Steps:**
1. In `rules/rdeprecated.rs`, define `pub struct RDeprecatedRule;`. In `analyze`: first, if `doc.parse_failure.is_some()`, return `vec![]`. Then iterate `doc.route_view.endpoints()`; for each endpoint with a known scheme: for each `LintOption` whose canonical `UriOption.deprecated` is `Some(msg)`, emit `Diagnostic { code: RDeprecated, severity: Warning, span: key_span, message: msg, fix: None }`. Aliases resolve to the canonical option before checking `deprecated`.
2. Complete `LintEngine::with_default_rules` to register all five rules (`RSynRule`, `RSchemaRule`, `RUriKnownRule`, `RSecretRule`, `RDeprecatedRule`).

**Tests:**
- `deprecated_option_reported_with_message`: setup = stub catalog where option `oldFreq` has `deprecated = Some("use \`period\` instead")`, source using `oldFreq` → action = analyze → assert one `RDeprecated` Warning on the `oldFreq` key span carrying the deprecation message. command = `cargo test -p camel-lint --lib deprecated_option_reported_with_message`.
- `all_five_rules_registered`: setup = `LintEngine::new(stub).with_default_rules()` → action = inspect the rules vector length → assert it equals 5. command = `cargo test -p camel-lint --lib all_five_rules_registered`.
- `all_five_rules_silent_on_valid_doc`: setup = a stub catalog with `timer`/`log`/`direct` metadata and a clean fixture route using only valid options → action = `LintEngine::new(stub).with_default_rules().lint(fixture)` → assert the returned `Vec<Diagnostic>` is empty (covers the spec scenario "Valid document yields no diagnostics" at the engine level, not only via the Phase 3 CLI). command = `cargo test -p camel-lint --lib all_five_rules_silent_on_valid_doc`.

**Acceptance:**
- `cargo test -p camel-lint --lib` passes including the three new tests (`deprecated_option_reported_with_message`, `all_five_rules_registered`, `all_five_rules_silent_on_valid_doc`).
- `cargo clippy -p camel-lint -- -D warnings` exits 0.
- `LintEngine::with_default_rules` registers exactly five rules.

- [x] 2.5

## Phase 3: CLI + zero-false-positives gate

### camel-cli

#### Task 3.1: register_builtin_components_for_lint + camel lint subcommand

**Files:**
- `crates/camel-cli/src/lib.rs` (modified — add `pub fn register_builtin_components_for_lint(ctx: &mut camel_core::CamelContext)`)
- `crates/camel-cli/src/commands/lint.rs` (new)
- `crates/camel-cli/src/commands/mod.rs` (modified — declare `pub mod lint;`)
- `crates/camel-cli/src/main.rs` or the clap command enum (modified — add the `lint` subcommand variant + flag parsing, mirroring `openapi.rs`)
- `crates/camel-cli/Cargo.toml` (modified — add `camel-lint = { workspace = true }` runtime dep)

**Steps:**
1. In `crates/camel-cli/src/lib.rs`, add `pub fn register_builtin_components_for_lint(ctx: &mut camel_core::CamelContext)`. NOTE: the runtime type is `CamelContext`, NOT `Context` (verify `crates/camel-core/src/context.rs:41`). The function registers builtins for METADATA purposes with empty/default config and DROPS every pool/bridge/lifecycle/datasource/path handle. Inclusion/skip matrix (mirrors `run.rs` but handle-free):
   - **Register config-independent (handle-free):** `ctx.register_component(TimerComponent::new())` for timer, cron, log, direct, seda, mock, controlbus.
   - **Register bridge components WITHOUT their handles:** register validator/xslt/xj components but do NOT call `xsd_bridge_backend()`/`bridge_runtime()` and do NOT install `BridgeCleanup` (lint is short-lived; no shutdown cleanup needed).
   - **Register always-on bundles with empty config:** http, ws, file, container, template via `ComponentBundle::from_toml(empty_table)` + `register_all` (drop any returned pool). Also master.
   - **Register feature-gated bundles when the feature is enabled** (`#[cfg(feature="kafka")]` kafka, mqtt, grpc, llm, opensearch, redis, jms, cxf) via the same empty-config path; for jms/cxf drop the returned pool handle.
   - **Register datasource bundles with an empty datasource catalog:** sql, surrealdb via `bundle.with_catalog(Arc::clone(&empty_datasource_catalog))` then `register_all` (an empty `DatasourceCatalog` is acceptable — metadata is queryable regardless of datasources).
   - **Skip (path/route-coupled, accept as `unverified-scheme`):** wasm (needs a config-relative `base_dir`), exec (route-conditional). Document these skips.
   - **Bundle-error policy:** if any `ComponentBundle::from_toml(empty)` returns `Err`, LOG at warn level and SKIP that scheme (it surfaces as `unverified-scheme` in lint output) — do NOT abort the whole registration (lint must degrade gracefully, not fail to construct its catalog).
   - Do NOT capture or return any handles. Document the drift-from-`run` tradeoff in a doc-comment referencing the bd follow-up.
2. Create `crates/camel-cli/src/commands/lint.rs` implementing the `camel lint <file>` subcommand, mirroring the structure of `crates/camel-cli/src/commands/openapi.rs`. It: builds a `camel_core::CamelContext` (via `CamelContext::builder().build()` or the minimal equivalent — confirm the exact builder entry point at implementation time), calls `register_builtin_components_for_lint(&mut ctx)`, obtains `let catalog: Arc<dyn ComponentMetadataCatalog> = Arc::new(ctx.metadata_catalog());` (note: `ctx.metadata_catalog()` at `context.rs:783` returns a `RuntimeComponentMetadataCatalog` BY VALUE, so wrap it in `Arc::new`), constructs `LintEngine::new(catalog).with_default_rules()`, reads the target file(s), runs `engine.lint(source)` (returns `Vec<Diagnostic>`), renders each `Diagnostic` with `ariadne` to stderr, and exits: 0 if no diagnostics, 1 if any `Severity::Error`, 2 on CLI misuse (missing/unreadable file — print a CLI error and `std::process::exit(2)`).
3. Add the `lint` subcommand to the clap command enum in `main.rs` (or wherever the enum lives) with a positional/flag for the file path(s).
4. Add `camel-lint = { workspace = true }` to `crates/camel-cli/Cargo.toml` `[dependencies]`.

**Tests:**
- `lint_clean_route_exits_zero`: setup = a temp file with a valid route (timer + log, valid options) → action = run the lint subcommand logic on it (invoke via a test helper that calls the same function the CLI dispatches to) → assert it produces zero diagnostics (which the CLI maps to exit 0). command = `cargo test -p camel-cli --lib lint_clean_route_exits_zero`.
- `lint_route_with_error_exits_one`: setup = a temp file producing an error-severity diagnostic (e.g. `timer:foo?bogusOption=1`) → action = run lint → assert at least one Error diagnostic (CLI maps to exit 1). command = `cargo test -p camel-cli --lib lint_route_with_error_exits_one`.
- `lint_missing_file_exits_two`: setup = a non-existent path → action = run lint → assert it returns the CLI-misuse outcome (exit 2). command = `cargo test -p camel-cli --lib lint_missing_file_exits_two`.
- `register_for_lint_does_not_capture_handles`: setup = inspect `register_builtin_components_for_lint` → action = compile-time/signature check → assert the function signature is `fn(&mut CamelContext)` and returns `()` (no handle bundle). command = `cargo build -p camel-cli`.

**Acceptance:**
- `cargo build -p camel-cli` exits 0.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- `cargo test -p camel-cli --lib lint_` passes (the three behavior tests above).

- [x] 3.1

#### Task 3.2: Corpus zero-false-positives gate (integration test + baseline)

**Files:**
- `crates/camel-cli/tests/lint_corpus.rs` (new)
- `crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron` (new)
- `crates/camel-cli/Cargo.toml` (modified — add `ron = { workspace = true }` to `[dev-dependencies]`; add `glob = { workspace = true }` to `[dev-dependencies]` if not already present)

**Steps:**
1. Add `ron = "0.8"` (pin to the latest 0.x compatible release at implementation time) to root `Cargo.toml`'s `[workspace.dependencies]` block (it is not currently a workspace dep). Create `crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron` as a `[(file_relative_path, [(DiagnosticCode, severity)])]` mapping (initially empty or seeded with the first run's output — every entry must be an agreed real defect). The format: `{ ("examples/foo.yaml", [("RUriKnown(UnknownOption)", "error")]), }`.
2. Create `crates/camel-cli/tests/lint_corpus.rs` with a `#[test] fn corpus_zero_false_positives()` that:
   - discovers route files via a glob: `examples/**/*.{yaml,json}` and `crates/**/tests/fixtures/**/*.{yaml,json}` (using the `glob` crate), deduplicated;
   - builds the production catalog via `register_builtin_components_for_lint` (constructing a `CamelContext`, registering, obtaining `ctx.metadata_catalog()`);
   - constructs `LintEngine::new(catalog).with_default_rules()`;
   - for each discovered file, runs `engine.lint(source)` and collects `(relative_path, diagnostic)`;
   - parses the baseline with `ron::from_str`;
   - asserts: every emitted diagnostic is present in the baseline (FAIL with the file + code on a false positive), AND every baseline diagnostic is emitted (FAIL on a missing-regression). The corpus file count is computed at runtime (not hardcoded).
3. Seed the baseline: on the first run, if the gate fails only because the baseline is empty but the engine emits diagnostics, inspect each emitted diagnostic against the corpus file — if it is a real defect, add it to the baseline; if it is a false positive, file a bd issue and fix/gate the rule. The committed baseline contains only agreed defects.

**Tests:**
- `corpus_zero_false_positives`: as above. command = `cargo test -p camel-cli --test lint_corpus`. expected = pass with the committed baseline.
- `corpus_gate_detects_false_positive`: setup = temporarily inject a rule that emits a spurious diagnostic on a known-clean file (in a scratch test variant) → action = run the gate → assert it fails naming the file and code. command = `cargo test -p camel-cli --test lint_corpus corpus_gate_detects_false_positive`.

**Acceptance:**
- `cargo test -p camel-cli --test lint_corpus` passes on the committed baseline (zero diagnostics outside baseline, zero baseline regressions).
- The baseline file `crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron` is non-empty only for files/diagnostics that are agreed real defects, and every entry maps to a real defect (verifiable by review of the baseline diff at commit time — no machine gate beyond the corpus test, which already enforces the set matches exactly).

- [x] 3.2

#### Task 3.3: Production catalog non-empty test + incremental apply_fix hook

> Cross-crate scope: this task modifies `crates/camel-lint/src/document.rs` (the `apply_fix`
> hook) AND adds `crates/camel-cli/tests/lint_production_catalog.rs`. The camel-lint work is a
> small extension to `Document`; the production-catalog test lives in camel-cli (which wires
> the real catalog). Both ship in this single task because the test exercises the hook's
> end-to-end path.

**Files:**
- `crates/camel-cli/tests/lint_production_catalog.rs` (new)
- `crates/camel-lint/src/document.rs` (modified — add `pub fn apply_fix(&mut self, fix: &Fix) -> Result<(), LintError>` using noyalib CST `replace_span` + re-parse of the affected region)
- `crates/camel-lint/src/lib.rs` (modified — define the `LintError` enum)

**Steps:**
1. In `crates/camel-lint/src/lib.rs` (or a small `error.rs` module), define `#[derive(Debug, thiserror::Error)] pub enum LintError { #[error("internal lint error: {0}")] Internal(String) }`. This is the only error type the crate returns; `lint()` does NOT use it (it returns `Vec<Diagnostic>`), but `Document::apply_fix` does.
2. In `crates/camel-lint/src/document.rs`, add `pub fn apply_fix(&mut self, fix: &Fix) -> Result<(), LintError>` that uses the noyalib CST `replace_span` to substitute `fix.replacement` into `fix.span`, re-parses the affected region, and updates `self.raw` + `self.route_view` (and `self.parse_failure`). On an edit that breaks syntax, return `Err(LintError::Internal("apply_fix produced invalid syntax".into()))` and leave the document unchanged (do not panic). DECISION (resolving the plan-review's ownership hole): the engine does NOT retain a `Document` field — `lint(&self, source)` is immutable and stateless. Incremental editing is therefore a DOCUMENT-level operation, not an engine method: a caller applies a fix via `document.apply_fix(&fix)` and then re-runs `engine.lint(&document.raw)` to get refreshed diagnostics. Do NOT add `apply_edit` to `LintEngine`.
3. Create `crates/camel-cli/tests/lint_production_catalog.rs` with `#[test] fn production_catalog_reports_invalid_timer_option()` that builds the production catalog via `register_builtin_components_for_lint` (constructing a `CamelContext`, registering, `let catalog: Arc<dyn ComponentMetadataCatalog> = Arc::new(ctx.metadata_catalog());`), constructs `LintEngine::new(catalog).with_default_rules()`, lints a source containing `timer:tick?bogusOption=1`, and asserts an `RUriKnown(UnknownOption)` Error on `bogusOption` (proving the catalog is populated and R-URI-known consults it).

**Tests:**
- `production_catalog_reports_invalid_timer_option`: as above. command = `cargo test -p camel-cli --test lint_production_catalog`.
- `apply_fix_reparses_and_refreshes`: setup = a source with a known unknown-option diagnostic on `timer:foo?bogus=1`; compute the span of the FULL query segment `?bogus=1` (or at minimum `bogus=1`) from `Document::parse`; CONSTRUCT a `Fix { span: <query-segment-span>, replacement: String::new() }` manually (no rule currently produces a `Fix` — the test builds the `Fix` itself, and replacing the whole `?bogus=1` segment is what actually removes the option; replacing only the key `bogus` would leave `?=1` and the diagnostic would persist) → action = `document.apply_fix(&fix)` then `engine.lint(&document.raw)` → assert the `UnknownOption` diagnostic is no longer emitted. command = `cargo test -p camel-lint --lib apply_fix_reparses_and_refreshes`.
- `apply_fix_rejects_syntax_breaking_edit`: setup = a `Fix` whose `replacement` is malformed YAML → action = `document.apply_fix(&fix)` → assert `Err(LintError::Internal(_))` and the document is unchanged. command = `cargo test -p camel-lint --lib apply_fix_rejects_syntax_breaking_edit`.

**Acceptance:**
- `cargo test -p camel-cli --test lint_production_catalog` passes.
- `cargo test -p camel-lint --lib apply_fix_reparses_and_refreshes apply_fix_rejects_syntax_breaking_edit` passes.
- `LintEngine` has NO `apply_edit` method and NO retained `Document` field (verifiable by `grep -nE 'apply_edit|last_doc' crates/camel-lint/src/engine.rs` returning nothing).
- `LintError` is defined with exactly an `Internal(String)` variant (verifiable by `grep -nE 'enum LintError' crates/camel-lint/src/*.rs`).
- `cargo build --workspace` exits 0.

- [x] 3.3
