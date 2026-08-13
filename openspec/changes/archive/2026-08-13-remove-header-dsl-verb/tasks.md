# Tasks: remove-header-dsl-verb

## camel-processor

### Task 1: RemoveHeader processor (mirrors SetHeader, input-only)

**Files:**
- `crates/camel-processor/src/remove_header.rs` (new)
- `crates/camel-processor/src/lib.rs` (modified — add `pub mod remove_header;`)

**Steps:**
1. Create `remove_header.rs` with a `RemoveHeader<P>` struct holding `inner: P` and `key: String`, mirroring `SetHeader<P>` in `set_header.rs:11-27`. Include `pub fn new(inner: P, key: impl Into<String>) -> Self`.
2. Implement `Service<Exchange> for RemoveHeader<P>` with the same bounds as `SetHeader` (set_header.rs:52-64). In `call()`, execute `exchange.input.headers.remove(&self.key);` then delegate to `self.inner.call(exchange)`. This is the ONLY difference from SetHeader (`remove` instead of `insert`).
3. Add a `RemoveHeaderLayer` struct with `pub fn new(key: impl Into<String>) -> Self` and `impl<S> tower::Layer<S> for RemoveHeaderLayer`, mirroring `SetHeaderLayer` (set_header.rs:30-49).
4. Add inline `#[cfg(test)] mod tests` with the test cases listed below.
5. Add `pub mod remove_header;` to `crates/camel-processor/src/lib.rs` in alphabetical order (after `recipient_list`, before `resequencer`), AND add `pub use remove_header::{RemoveHeader, RemoveHeaderLayer};` in the re-export block (near L67 where `DynamicSetHeader` is re-exported). Without the re-export, `camel_processor::RemoveHeader` is unreachable from camel-core.

**Tests:**
- `test_remove_header_deletes_existing`: exchange with input header `CamelHttpPath=Some(Value::String("x".into()))` → run `RemoveHeader::new(IdentityProcessor, "CamelHttpPath")` → assert `exchange.input.headers` does NOT contain `CamelHttpPath`.
- `test_remove_header_noop_on_missing`: exchange with input headers `{A=1, B=2}` but NOT `C` → run `RemoveHeader::new(IdentityProcessor, "C")` → assert headers still `{A=1, B=2}`, no error.
- `test_remove_header_preserves_other_headers`: exchange with input headers `{X=1, Y=2, Z=3}` → run `RemoveHeader::new(IdentityProcessor, "Y")` → assert headers still contain `X` and `Z`, only `Y` is gone.
- `test_remove_header_preserves_body`: exchange with body `"hello"` → run `RemoveHeader::new(IdentityProcessor, "anything")` → assert body is still `"hello"`.

**Acceptance:**
- `cargo test -p camel-processor --lib remove_header` passes all 4 tests.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [ ] 1

## camel-dsl + camel-core

### Task 2: Wire RemoveHeader through the full declarative pipeline (types + all match arms + BuilderStep)

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/contract.rs` (modified)
- `crates/camel-dsl/src/model.rs` (modified)
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/src/compile.rs` (modified)
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified)

**Steps:**
1. **AST** (`route_ast.rs`): Add `RemoveHeader(RemoveHeaderStep)` variant to the `RouteDslStep` enum, placed after `SetHeaderIfAbsent(SetHeaderStep)` (near L301). RouteDslStep is `#[serde(untagged)]` — do NOT add a variant-level serde rename (the YAML key comes from the inner field name).
2. **AST structs** (`route_ast.rs`): Add `RemoveHeaderStep` and `RemoveHeaderData` near L369, mirroring `SetHeaderStep`/`SetHeaderData` exactly but with only `key`:
   ```rust
   #[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
   #[derive(Deserialize, Debug, Clone)]
   #[serde(deny_unknown_fields)]
   pub struct RemoveHeaderStep {
       pub remove_header: RemoveHeaderData,
   }

   #[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
   #[derive(Deserialize, Debug, Clone)]
   #[serde(deny_unknown_fields)]
   pub struct RemoveHeaderData {
       pub key: String,
   }
   ```
3. **Kind registry** (`contract.rs`): Add `RemoveHeader` variant to `DeclarativeStepKind` (L2) with `#[serde(rename = "remove_header")]`. Append `DeclarativeStepKind::RemoveHeader` to `MANDATORY_DECLARATIVE_STEP_KINDS` (L49), changing `[DeclarativeStepKind; 38]` → `[DeclarativeStepKind; 39]`. Update the length assertion at L153 from `38` to `39`.
4. **YAML mandatory array** (`yaml.rs`): Append `DeclarativeStepKind::RemoveHeader` to `YAML_IMPLEMENTED_MANDATORY_STEPS` (L56), changing `[DeclarativeStepKind; 38]` → `[DeclarativeStepKind; 39]`.
5. **Lowered model** (`model.rs`): Add `RemoveHeader(RemoveHeaderStepDef)` variant to `DeclarativeStep` (L572). Define `pub struct RemoveHeaderStepDef { pub key: String }` (with Debug, Clone, PartialEq derives matching other def structs). Add `DeclarativeStep::RemoveHeader(_) => DeclarativeStepKind::RemoveHeader` arm to `kind()` (L619).
6. **YAML parse** (`yaml.rs`): Add a match arm for `RouteDslStep::RemoveHeader(RemoveHeaderStep { remove_header })` that validates `remove_header.key.trim().is_empty()` → returns `CamelError` with message `"remove_header: key must not be empty"` (mirror set_header guard at L613-615), then returns `Ok(DeclarativeStep::RemoveHeader(RemoveHeaderStepDef { key: remove_header.key }))`.
7. **BuilderStep** (`route_definition.rs`, camel-core): Add `DeclarativeRemoveHeader { key: String }` variant to the `BuilderStep` enum (L62), placed near `DeclarativeSetHeader`.
8. **Compile lowering** (`compile.rs`): Add a match arm for `DeclarativeStep::RemoveHeader(RemoveHeaderStepDef { key })` (near L896) returning `Ok(BuilderStep::DeclarativeRemoveHeader { key })`.
9. **Compile name** (`compile.rs`): Add `DeclarativeStep::RemoveHeader(_) => "remove_header"` to `declarative_step_name()` (near L1538).
10. **Compile validation** (`compile.rs`): Add empty-key validation arm (near L1801): `DeclarativeStep::RemoveHeader(RemoveHeaderStepDef { key, .. })` → if `key.trim().is_empty()` → error `"remove_header key must not be empty"`.

**Tests:**
- `test_declarative_step_kind_remove_header`: construct `DeclarativeStep::RemoveHeader(RemoveHeaderStepDef { key: "X".into() })`, call `.kind()`, assert returns `DeclarativeStepKind::RemoveHeader`.
- `test_yaml_parse_remove_header`: parse YAML step `- remove_header: { key: CamelHttpPath }` → assert it produces `DeclarativeStep::RemoveHeader(RemoveHeaderStepDef { key: "CamelHttpPath" })`.
- `test_yaml_parse_remove_header_empty_key_rejected`: parse YAML `- remove_header: { key: "" }` → assert compilation returns an error containing `"remove_header"` and `"empty"`.
- `test_yaml_parse_remove_header_whitespace_key_rejected`: parse YAML `- remove_header: { key: "   " }` → assert compilation returns an error containing `"remove_header"` and `"empty"` (trim().is_empty() catches whitespace).

**Acceptance:**
- `cargo check -p camel-dsl -p camel-core` exits 0 (all exhaustive matches satisfied).
- `cargo test -p camel-dsl --lib` passes (including the 4 new tests + contract length assertion 39).
- `cargo clippy -p camel-dsl -p camel-core --all-features -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [ ] 2

## camel-core

### Task 3: step compiler branch + end-to-end integration test

**Files:**
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified)

**Steps:**
1. In `core.rs`, add a match arm for `BuilderStep::DeclarativeRemoveHeader { key }` (near L242, after the DeclarativeSetHeader block). The arm creates `let svc = camel_processor::RemoveHeader::new(IdentityProcessor, key);` and returns it boxed, mirroring the DeclarativeSetHeader static-value branch (L243-250). Note: `core.rs` has a `_ => StepCompilationResult::NotHandled` catch-all (confirmed at L537), so the BuilderStep variant addition does NOT break camel-core compilation on its own — but the arm must be added for the step to actually function.
2. Add an integration test (inline `#[cfg(test)]` in core.rs or the existing test module) that constructs `BuilderStep::DeclarativeRemoveHeader { key: "CamelHttpPath".into() }`, passes it to `reg.compile_step(step, 0, &ctx)` (mirror existing test pattern at L628), runs the resulting processor against an exchange with header `CamelHttpPath` set, and asserts the header is absent after execution. NOTE: camel-core does NOT depend on camel-dsl, so tests must construct `BuilderStep` directly — never `DeclarativeStep`.
3. Add a second integration test verifying input-only semantics: set the SAME header key on both input and output, run RemoveHeader, assert input header is removed but output header is preserved.

**Tests:**
- `test_remove_header_end_to_end`: construct `BuilderStep::DeclarativeRemoveHeader { key: "CamelHttpPath".into() }` → compile via `reg.compile_step(step, 0, &ctx)` → run on exchange with `input.headers["CamelHttpPath"] = Value::String("x".into())` → assert `output.input.headers` does NOT contain `CamelHttpPath`.
- `test_remove_header_input_only_preserves_output`: exchange where BOTH `input.headers` and `output.headers` contain key `X-Shared` → run RemoveHeader with key `X-Shared` → assert `input.headers` no longer has `X-Shared` but `output.headers` STILL has `X-Shared`.

**Acceptance:**
- `cargo test -p camel-core --lib` passes (including the 2 new tests).
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [ ] 3

## schema

### Task 4: JSON route schema regen + sync embedded copy + lint validation test

**Files:**
- `schemas/dsl/route-schema.json` (regenerated)
- `crates/camel-lint/schema/route-schema.json` (modified — manual copy)
- `crates/camel-lint/src/rules/rschema.rs` (modified — add test)

**Steps:**
1. Confirm that Task 2 added `#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]` to both `RemoveHeaderStep` and `RemoveHeaderData` — this is what makes `schemars::schema_for!(RouteDslSchemaEnvelope)` auto-include `remove_header` in the generated schema. No hand-editing of the schema generator is needed.
2. Run `cargo run -p xtask -- schema` to regenerate `schemas/dsl/route-schema.json`. The `remove_header` step should appear automatically.
3. Copy the regenerated file to the embedded lint schema: `cp schemas/dsl/route-schema.json crates/camel-lint/schema/route-schema.json`.
4. Verify: `cargo run -p xtask -- schema --check` exits 0 (zero drift on both files).
5. Confirm `schemas/dsl/route-schema.json` contains `"remove_header"` as a recognized step (grep the regenerated file).
6. Add a test in `crates/camel-lint/src/rules/rschema.rs` (inline `#[cfg(test)]`) that validates a JSON route document containing a `remove_header` step against the embedded `ROUTE_SCHEMA` (using `jsonschema::validator_for(ROUTE_SCHEMA)` as at L140), asserting zero validation errors. This proves `camel-lint` accepts routes using the new verb — not just that the schema contains the string.

**Tests:**
- `schema_check_zero_drift`: `cargo run -p xtask -- schema --check` exits 0 after regen + copy.
- `schema_contains_remove_header`: `grep -c '"remove_header"' schemas/dsl/route-schema.json` returns ≥ 1.
- `lint_accepts_remove_header_route`: construct a minimal JSON route `{"routes": [{"from": {...}, "steps": [{"remove_header": {"key": "CamelHttpPath"}}]}]}`, validate against `ROUTE_SCHEMA` via `jsonschema::validator_for`, assert zero validation errors.

**Acceptance:**
- `cargo run -p xtask -- schema --check` exits 0.
- `schemas/dsl/route-schema.json` contains `"remove_header"` as a recognized step.
- `crates/camel-lint/schema/route-schema.json` is byte-identical to `schemas/dsl/route-schema.json` (verify with `diff`).
- `cargo test -p camel-lint` passes (including the new `lint_accepts_remove_header_route` test).
- `cargo fmt --check --all` exits 0.

- [ ] 4
