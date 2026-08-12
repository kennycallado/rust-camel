# Tasks: migrate-xj-xslt-to-metadata

## Execution order

Tasks 1 and 2 are independent (one per component) and MUST both complete
before Task 3 (catalog integration + corpus audit). Task 4 (schema regen)
depends on Tasks 1+2 for the metadata to regenerate. The conductor dispatches
sequentially: 1 → 2 → 3 → 4.

## Change-level verification

After all tasks complete, run:

```
cargo test -p camel-xj -p camel-xslt
cargo test -p camel-cli --test lint_corpus
cargo xtask schema --check
openspec validate migrate-xj-xslt-to-metadata --type change --json
```

All four MUST pass. Additionally, verify no LSP golden test hardcodes xj/xslt
as minimal: `rg 'xj|xslt' crates/camel-lsp/ -g '*.rs'` (if hits exist, confirm
they do not assert empty `uri_options`).

---

## Task 1: camel-xj metadata descriptor + Component override + parity tests

### Files

- `crates/components/camel-xj/src/metadata.rs` (new)
- `crates/components/camel-xj/src/lib.rs` (modified — add `pub(crate) mod metadata;`)
- `crates/components/camel-xj/src/component.rs` (modified — add `fn metadata()` override)

### Steps

1. Create `crates/components/camel-xj/src/metadata.rs` with a `XjMetadataDescriptor` struct following the `camel-jms/src/metadata.rs` template. The struct MUST use:
   - `use camel_component_api::UriConfig;`
   - `#[allow(dead_code)]`
   - `#[derive(UriConfig)]`
   - `#[uri_scheme = "xj"]`
   - `#[uri_config(skip_impl, metadata(scheme = "xj", description = "XJ XML↔JSON transformation via xml-bridge", producer), crate = "camel_component_api")]`
   - Fields (ALL with explicit `name = "camelCaseKey"`, ALL prefixed `_`):
     - `#[uri_param(name = "direction", desc = "Transform direction: xml2json or json2xml")] pub _direction: String` — required (no Option, no default). Correct: runtime parser rejects absence.
     - `#[uri_param(name = "maxPayloadBytes", desc = "Max payload size in bytes before rejecting")] pub _max_payload_bytes: Option<usize>` — optional.
     - `#[uri_param(name = "retryCount", default = "3", desc = "Retry count for bridge operations")] pub _retry_count: u32` — optional via default. Runtime default is `3u32` (`config.rs:53`).
     - `#[uri_param(name = "retryDelayMs", default = "500", desc = "Retry delay in milliseconds")] pub _retry_delay_ms: u64` — optional via default. Runtime default is `500u64` (`config.rs:54`).
     - `#[uri_param(pattern = "param.", desc = "Open namespace: param.<name>=<value> stylesheet parameters")] pub _params: Vec<(String, String)>` — open namespace. The macro derives option name `"param"` (trailing `.` stripped). Never required (pattern fields are always optional).

2. Add `pub(crate) mod metadata;` to `crates/components/camel-xj/src/lib.rs` alongside the existing `mod config;` declaration.

3. Add the `metadata()` override to `impl Component for XjComponent` in `crates/components/camel-xj/src/component.rs`. Insert between `fn scheme(&self)` (line 477) and `fn create_endpoint` (line 481):
   ```rust
   fn metadata(&self) -> camel_component_api::ComponentMetadata {
       crate::metadata::XjMetadataDescriptor::metadata()
   }
   ```

4. Add a `#[cfg(test)] mod tests` block at the bottom of `metadata.rs` with parity tests (see Tests below).

### Tests

All tests live in `metadata.rs` under `#[cfg(test)] mod tests`. Use `use camel_component_api::ComponentMetadata;` (confirmed re-export, same as jms template). For `OptionKind` and `UriOptionMatch`, import from `camel_component_api` (verify the exact path compiles — these live in `camel_api::component_metadata` which `camel_component_api` re-exports). `camel-component-api` is already in camel-xj's `[dev-dependencies]` with `features = ["test-support"]`.

- **name**: `xj_metadata_uri_options_names`
  - **setup**: call `XjMetadataDescriptor::metadata()` to get `ComponentMetadata`
  - **action**: collect `meta.uri_options` names into a sorted `Vec<&str>`
  - **assert**: names equal `["direction", "maxPayloadBytes", "param", "retryCount", "retryDelayMs"]` (sorted). Exactly 5 options. No `stylesheetUri` (path component, intentionally omitted).
  - **command**: `cargo test -p camel-xj xj_metadata_uri_options_names`
  - **expected**: pass after step 4

- **name**: `xj_metadata_param_option_has_prefix_pattern`
  - **setup**: call `XjMetadataDescriptor::metadata()`
  - **action**: find the option with name `"param"`
  - **assert**: `option.pattern == Some(UriOptionMatch::Prefix { separator: "param." })` and `option.kind == OptionKind::String`
  - **command**: `cargo test -p camel-xj xj_metadata_param_option_has_prefix_pattern`
  - **expected**: pass after step 4

- **name**: `xj_metadata_numeric_options_derive_int_kind`
  - **setup**: call `XjMetadataDescriptor::metadata()`
  - **action**: find options `"maxPayloadBytes"`, `"retryCount"`, `"retryDelayMs"`
  - **assert**: all three have `kind == OptionKind::Int`. This prevents silent kind-inference regressions that would cause false `KindMismatch` diagnostics on valid routes.
  - **command**: `cargo test -p camel-xj xj_metadata_numeric_options_derive_int_kind`
  - **expected**: pass after step 4

- **name**: `xj_metadata_direction_is_required`
  - **setup**: call `XjMetadataDescriptor::metadata()`
  - **action**: find option `"direction"`
  - **assert**: `option.required == true` (bare `String`, no Option, no default → required). Matches runtime parser which rejects absence.
  - **command**: `cargo test -p camel-xj xj_metadata_direction_is_required`
  - **expected**: pass after step 4

### Acceptance

- `cargo build -p camel-xj` exits 0
- `cargo clippy -p camel-xj -- -D warnings` exits 0
- `cargo test -p camel-xj` passes all tests (existing + 4 new)
- `XjMetadataDescriptor::metadata()` returns exactly 5 `uri_options`
- No `stylesheetUri` option exists in the descriptor

- [ ] 1

---

## Task 2: camel-xslt metadata descriptor + Component override + parity tests

### Files

- `crates/components/camel-xslt/src/metadata.rs` (new)
- `crates/components/camel-xslt/src/lib.rs` (modified — add `pub(crate) mod metadata;`)
- `crates/components/camel-xslt/src/component.rs` (modified — add `fn metadata()` override)

### Steps

1. Create `crates/components/camel-xslt/src/metadata.rs` with a `XsltMetadataDescriptor` struct following the same template as Task 1. The struct MUST use:
   - `use camel_component_api::UriConfig;`
   - `#[allow(dead_code)]`
   - `#[derive(UriConfig)]`
   - `#[uri_scheme = "xslt"]`
   - `#[uri_config(skip_impl, metadata(scheme = "xslt", description = "XSLT 3.0 transformation via xml-bridge", producer), crate = "camel_component_api")]`
   - Fields (ALL with explicit `name = "camelCaseKey"`, ALL prefixed `_`):
     - `#[uri_param(name = "output", desc = "Output method: xml, html, or text")] pub _output_method: Option<String>` — optional.
     - `#[uri_param(name = "transformerCacheSize", desc = "Max compiled stylesheets to keep in cache")] pub _transformer_cache_size: Option<usize>` — optional.
     - `#[uri_param(name = "failOnNullBody", default = "false", desc = "Fail if input body is null or empty")] pub _fail_on_null_body: bool` — optional via default. Runtime default is `false` (`config.rs`).
     - `#[uri_param(name = "maxPayloadBytes", desc = "Max payload size in bytes")] pub _max_payload_bytes: Option<usize>` — optional.
     - `#[uri_param(pattern = "param.", desc = "Open namespace: param.<name>=<value> stylesheet parameters")] pub _params: Vec<(String, String)>` — open namespace. Option name derived as `"param"`.

2. Add `pub(crate) mod metadata;` to `crates/components/camel-xslt/src/lib.rs` alongside the existing `mod config;` declaration.

3. Add the `metadata()` override to `impl Component for XsltComponent` in `crates/components/camel-xslt/src/component.rs`. Insert between `fn scheme(&self)` (line 264) and `fn create_endpoint` (line 268):
   ```rust
   fn metadata(&self) -> camel_component_api::ComponentMetadata {
       crate::metadata::XsltMetadataDescriptor::metadata()
   }
   ```

4. Add a `#[cfg(test)] mod tests` block at the bottom of `metadata.rs` with parity tests (see Tests below).

### Tests

All tests live in `metadata.rs` under `#[cfg(test)] mod tests`. Use `use camel_component_api::ComponentMetadata;` (confirmed re-export, same as jms template). For `OptionKind` and `UriOptionMatch`, import from `camel_component_api` (verify the exact path compiles — these live in `camel_api::component_metadata` which `camel_component_api` re-exports). `camel-component-api` is already in camel-xj's `[dev-dependencies]` with `features = ["test-support"]`.

- **name**: `xslt_metadata_uri_options_names`
  - **setup**: call `XsltMetadataDescriptor::metadata()` to get `ComponentMetadata`
  - **action**: collect `meta.uri_options` names into a sorted `Vec<&str>`
  - **assert**: names equal `["failOnNullBody", "maxPayloadBytes", "output", "param", "transformerCacheSize"]` (sorted). Exactly 5 options. No `stylesheetUri`.
  - **command**: `cargo test -p camel-xslt xslt_metadata_uri_options_names`
  - **expected**: pass after step 4

- **name**: `xslt_metadata_param_option_has_prefix_pattern`
  - **setup**: call `XsltMetadataDescriptor::metadata()`
  - **action**: find the option with name `"param"`
  - **assert**: `option.pattern == Some(UriOptionMatch::Prefix { separator: "param." })` and `option.kind == OptionKind::String`
  - **command**: `cargo test -p camel-xslt xslt_metadata_param_option_has_prefix_pattern`
  - **expected**: pass after step 4

- **name**: `xslt_metadata_numeric_options_derive_int_kind`
  - **setup**: call `XsltMetadataDescriptor::metadata()`
  - **action**: find options `"transformerCacheSize"` and `"maxPayloadBytes"`
  - **assert**: both have `kind == OptionKind::Int`
  - **command**: `cargo test -p camel-xslt xslt_metadata_numeric_options_derive_int_kind`
  - **expected**: pass after step 4

- **name**: `xslt_metadata_no_required_options`
  - **setup**: call `XsltMetadataDescriptor::metadata()`
  - **action**: iterate all `uri_options`
  - **assert**: none have `required == true`. All xslt query params are optional (the stylesheet path is not a query param; no query param is mandatory).
  - **command**: `cargo test -p camel-xslt xslt_metadata_no_required_options`
  - **expected**: pass after step 4

### Acceptance

- `cargo build -p camel-xslt` exits 0
- `cargo clippy -p camel-xslt -- -D warnings` exits 0
- `cargo test -p camel-xslt` passes all tests (existing + 4 new)
- `XsltMetadataDescriptor::metadata()` returns exactly 5 `uri_options`
- No `stylesheetUri` option exists in the descriptor

- [ ] 2

---

## Task 3: catalog integration test + YAML fixture routes + lint_corpus audit

### Files

- `crates/components/camel-xj/tests/catalog_integration_test.rs` (new)
- `crates/components/camel-xslt/tests/catalog_integration_test.rs` (new)
- `crates/components/camel-xj/tests/fixtures/xj-param-namespace.yaml` (new)
- `crates/components/camel-xslt/tests/fixtures/xslt-param-namespace.yaml` (new)

### Context

`examples/xj-example/` and `examples/xslt-example/` are Rust binary crates, not
YAML route files. The `lint_corpus` test only globs `examples/**/*.{yaml,json}`
and `crates/**/tests/fixtures/**/*.{yaml,json}`. No xj/xslt YAML fixtures exist
today, so the lint would never exercise the new metadata without explicit
fixtures. Change 1 already tests prefix-match lint resolution in isolation
(`ruriknown.rs:366-404` — `param.foo` matches, `param.` rejects, discrete wins),
but this change must provide end-to-end coverage through the real catalog.

### Steps

1. Create `crates/components/camel-xj/tests/catalog_integration_test.rs` that constructs an `XjComponent::default()`, calls `.metadata()` on it, and asserts the returned `ComponentMetadata` has non-empty `uri_options` containing a `"param"` option. This verifies the `Component::metadata()` override wiring end-to-end (not just the descriptor in isolation).

2. Create `crates/components/camel-xslt/tests/catalog_integration_test.rs` with the same shape for `XsltComponent::default()`.

3. Create `crates/components/camel-xj/tests/fixtures/xj-param-namespace.yaml` — a minimal YAML route that uses a `param.*` URI key. Example content:
   ```yaml
   - from: xj:classpath:identity.xslt?direction=xml2json&param.mode=debug&param.lang=en
     steps:
       - to: mock:result
   ```
   This route exercises both the discrete `direction` option and the `param.*`
   open-namespace prefix match. The lint MUST resolve `param.mode` and
   `param.lang` without emitting `UnknownOption`.

4. Create `crates/components/camel-xslt/tests/fixtures/xslt-param-namespace.yaml` — analogous route for xslt:
   ```yaml
   - from: xslt:classpath:transform.xslt?output=xml&param.title=Hello
     steps:
       - to: mock:result
   ```

5. Run `cargo test -p camel-cli --test lint_corpus`. The xj/xslt fixture routes MUST produce zero `UnknownOption` diagnostics on `param.*` keys — if any appear, the metadata descriptor is wrong (missing `name`, wrong `pattern`, or a key the parser handles but the descriptor does not list). Fix the descriptor, do NOT add the diagnostics to the baseline. The baseline must NOT grow with xj/xslt entries from this change.

6. Verify the spec scenario "xj/xslt param namespace resolves via prefix match" is exercised: confirm `param.mode`/`param.lang`/`param.title` resolve via prefix and that no `UnknownOption` is emitted on these routes.

### Tests

- **name**: `xj_component_metadata_non_empty_via_override`
  - **setup**: construct `camel_xj::XjComponent::default()`
  - **action**: call `.metadata()` on the component
  - **assert**: returned `ComponentMetadata` has `uri_options.len() == 5` and contains a name `"param"` with `pattern == Some(UriOptionMatch::Prefix { separator: "param." })`
  - **command**: `cargo test -p camel-xj --test catalog_integration_test`
  - **expected**: pass after step 1

- **name**: `xslt_component_metadata_non_empty_via_override`
  - **setup**: construct `camel_xslt::XsltComponent::default()`
  - **action**: call `.metadata()` on the component
  - **assert**: returned `ComponentMetadata` has `uri_options.len() == 5` and contains a name `"param"` with `pattern == Some(UriOptionMatch::Prefix { separator: "param." })`
  - **command**: `cargo test -p camel-xslt --test catalog_integration_test`
  - **expected**: pass after step 2

- **name**: `lint_corpus_xj_xslt_prefix_match_resolves`
  - **setup**: tasks 1+2 completed (metadata published); steps 3+4 completed (YAML fixtures created)
  - **action**: run `cargo test -p camel-cli --test lint_corpus`
  - **assert**: the xj/xslt fixture routes produce zero `UnknownOption` diagnostics on `param.*` keys — they resolve via prefix match against the newly published metadata. Any `UnknownOption` on a `param.*` key is a test FAILURE (broken descriptor), NOT a baseline entry. The baseline must NOT grow with xj/xslt entries from this change.
  - **command**: `cargo test -p camel-cli --test lint_corpus`
  - **expected**: pass (xj/xslt param.* keys resolve via prefix match, zero UnknownOption)

### Acceptance

- `cargo test -p camel-xj --test catalog_integration_test` exits 0
- `cargo test -p camel-xslt --test catalog_integration_test` exits 0
- `cargo test -p camel-cli --test lint_corpus` passes (xj/xslt fixture routes resolve `param.*` via prefix match, no unhandled `UnknownOption`)
- YAML fixtures exist at `crates/components/camel-xj/tests/fixtures/xj-param-namespace.yaml` and `crates/components/camel-xslt/tests/fixtures/xslt-param-namespace.yaml`

- [x] 3

---

## Task 4: schema-snapshot regeneration

### Files

- `schemas/component-metadata.json` (modified — regenerated)

### Steps

1. Run `cargo xtask schema` to regenerate the schema snapshot. This picks up the new xj/xslt metadata.

2. Run `cargo xtask schema --check` to verify the snapshot is up-to-date. If it fails, the snapshot is stale — re-run step 1.

3. Inspect the diff: `git diff schemas/component-metadata.json`. Confirm the changes are additive (new entries for `xj` and `xslt` schemes with their `uri_options`). No existing entries should be removed or modified.

4. Run `cargo xtask schema` a second time and confirm zero diff (idempotency check).

### Tests

- **name**: `schema_check_passes`
  - **setup**: tasks 1+2 completed (metadata published)
  - **action**: run `cargo xtask schema --check`
  - **assert**: exits 0 (snapshot matches generated output)
  - **command**: `cargo xtask schema --check`
  - **expected**: pass after step 2

- **name**: `schema_gen_is_idempotent`
  - **setup**: step 2 passed (snapshot committed)
  - **action**: run `cargo xtask schema` again, then `git diff --exit-code schemas/component-metadata.json`
  - **assert**: zero diff (no further changes)
  - **command**: `cargo xtask schema && git diff --exit-code schemas/component-metadata.json`
  - **expected**: pass (zero diff)

### Acceptance

- `cargo xtask schema --check` exits 0
- The schema diff is additive (only new xj/xslt entries)
- Schema generation is idempotent (second run produces zero diff)

- [ ] 4
