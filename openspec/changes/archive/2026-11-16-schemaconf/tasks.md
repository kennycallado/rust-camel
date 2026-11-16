# Tasks: schemaconf

## camel-json-schema (new crate)

### Task 1.1: Crate skeleton, public API, inline-only compilation, draft selection

**Files:**
- `crates/camel-json-schema/Cargo.toml` (new)
- `crates/camel-json-schema/src/lib.rs` (new)
- `crates/camel-json-schema/src/error.rs` (new)
- `crates/camel-json-schema/tests/inline_and_drafts.rs` (new)

**Steps:**
1. Create `crates/camel-json-schema/Cargo.toml`: package `camel-json-schema`, `version.workspace = true`, `edition.workspace = true`, publish handled exactly as `crates/camel-lint/Cargo.toml` does (no `publish` key). Dependencies: `jsonschema` (workspace), `serde_json` (workspace), `noyalib` (workspace, feature `compat-serde-yaml`; alias `use noyalib::compat::serde_yaml as serde_yml;` exactly as `camel-validator/src/compiled.rs` does), `thiserror` (workspace). NO `camel-*` dependencies and no dev-dependencies on `camel-*`.
2. In `src/error.rs` define `#[derive(Debug, thiserror::Error)] pub enum SchemaError` with variants whose `Display` messages name the offending path/URI AND preserve existing validator literals where byte-identity matters: `ExternalRefRejected { uri: String, root: String }` ("external $ref '{uri}' rejected: outside schema root '{root}'"), `RemoteRefRejected { uri: String }` ("remote $ref '{uri}' rejected: schemas resolve from local files only (ADR-0100); vendor the file"), `FileTooLarge { path: String, limit: u64 }` ("schema file '{path}' exceeds {limit} bytes"), `RootTooLarge { path: String, limit: u64 }` (same literal), `TooManyResources { limit: usize }`, `TotalBudgetExceeded { limit: u64 }`, `NotARegularFile { path: String }`, `Open { path: String, message: String }` ("failed to open schema file '{path}': {message}"), `Read { path: String, message: String }` ("failed to read schema file '{path}': {message}"), `Utf8 { path: String, message: String }` ("YAML schema '{path}' is not valid UTF-8: {message}"), `Parse { path: String, message: String }`, `Compile { message: String }` (textual fields are named `message`, NOT `source` — `thiserror` treats a `source` field as an error source and `String` does not implement `Error`). `Parse` carries the full existing final text (`invalid JSON schema '{path}': {e}` / `invalid YAML schema '{path}': {e}` / `failed to convert YAML schema to JSON '{path}': {e}`) so a consumer can surface it verbatim; `Compile` carries compile-failure text.
3. In `src/lib.rs` define the public surface exactly as `design.md` states: `pub enum SchemaSource { File(PathBuf), Inline(serde_json::Value) }`, `pub enum SchemaSyntax { Auto, Json, Yaml }`, `pub enum FormatMode { DraftDefault, Enforce, Annotate }`, `pub struct Violation { pub instance_path: String, pub message: String }`, `pub struct CompiledSchema` holding a `jsonschema::Validator`, and `pub fn compile(source: &SchemaSource, syntax: SchemaSyntax, format: FormatMode) -> Result<CompiledSchema, SchemaError>`.
4. Implement the `Inline` arm of `compile` with an inline-only retriever that fails EVERY retrieval, classified by the RESOLVED URI's scheme (upstream resolves relative refs against the base BEFORE the retriever sees them — `defs.json` against base `inline-schema:/` arrives as `inline-schema:/defs.json`): the synthetic inline-base scheme (`inline-schema`) and `file` → `SchemaError::ExternalRefRejected { uri, root }`; every other scheme (`http`/`https`/anything else) → `SchemaError::RemoteRefRejected { uri }` with the exact Display `remote $ref '{uri}' rejected: schemas resolve from local files only (ADR-0100); vendor the file`. Set `with_base_uri` to the opaque non-file base (e.g. `"inline-schema:/"`) so plain `#` fragment refs resolve from the root document while any external ref hits the retriever and fails closed. **Typed error recovery (introduced here, reused in Task 1.2)**: upstream wraps retrieval failures in `referencing::Error::Unretrievable` and `ValidationError` exposes no `source()` chain, so boxing alone loses the variant — the retriever holds a shared capture slot (e.g. `Arc<Mutex<Option<SchemaError>>>`) that stores the typed error BEFORE boxing, and `compile` drains the slot on build failure and returns the stored `SchemaError` verbatim (falling back to `SchemaError::Compile { message }` only when the slot is empty). Root document parsing for `Inline` is `serde_json::Value` directly (no re-parse); `SchemaSyntax` is ignored for `Inline` except `Yaml` being rejected with `SchemaError::Compile` if ever requested (a `Value` cannot be YAML-parsed).
5. Implement `CompiledSchema::violations(&self, instance: &Value) -> Vec<Violation>` mapping each `jsonschema` error to `Violation { instance_path: e.instance_path().to_string(), message: e.to_string() }` so a consumer can format `"{message} at {instance_path}"` byte-identically to today.
6. Wire `FormatMode`: `Enforce` → `should_validate_formats(true)`, `Annotate` → `should_validate_formats(false)`, `DraftDefault` → leave `should_validate_formats` UNSET so upstream's native draft-and-vocabulary defaults apply exactly as `jsonschema::validator_for` behaves (draft-07 and format-assertion vocabularies enforce; 2020-12 without the format-assertion vocabulary annotates). `DraftDefault` is the behavior-preserving mode for the MCP migration.
7. Implement draft selection: pass the root document unchanged so its `$schema` selects the draft (2020-12 when absent) — no draft forcing in `compile`.
8. `src/lib.rs` crate-level doc comment citing ADR-0100 and stating the confinement contract.

**Tests:** (file `tests/inline_and_drafts.rs`; command `cargo test -p camel-json-schema`; expected: fail before implementation, pass after)
- `inline_internal_ref_resolves_and_validates`: Inline source `{"$defs":{"pos":{"type":"integer","minimum":0}},"$ref":"#/$defs/pos"}` (Enforce) → `compile` Ok → `violations(&json!(-1))` non-empty and `violations(&json!(3))` empty.
- `inline_external_ref_rejected_with_named_error`: Inline source `{"$ref":"defs.json#/$defs/x"}` → `compile` returns `Err(SchemaError::ExternalRefRejected { .. })` whose `to_string()` contains `defs.json`.
- `inline_external_ref_rejected_absolute_path`: Inline source `{"$ref":"file:///etc/hostname"}` → `Err`, message contains `/etc/hostname`, and the error is the named rejection (not a JSON parse error).
- `inline_remote_ref_rejected_with_local_files_only_error`: Inline source `{"$ref":"https://example.com/s.json"}` → `Err(SchemaError::RemoteRefRejected)` whose `to_string()` equals `remote $ref 'https://example.com/s.json' rejected: schemas resolve from local files only (ADR-0100); vendor the file` (exact literal).
- `inline_remote_dollar_id_rebase_rejected_with_local_files_only_error`: Inline source `{"$id":"https://example.com/b.json","$ref":"s.json"}` → `Err(SchemaError::RemoteRefRejected)` with the same exact local-files-only literal naming the rebased remote URI.
- `draft_2020_12_defs_used_when_absent`: Inline source without `$schema` using `$defs` + `$ref: "#/$defs/x"` → compiles and enforces (2020-12 default).
- `draft07_ignores_keywords_beside_ref`: Inline `{"$schema":"http://json-schema.org/draft-07/schema#","type":"integer","$ref":"#/definitions/x","definitions":{"x":{"type":"string"}}}` against instance `json!("hi")` → draft-07 ignores the sibling `type` keyword beside `$ref` and validates against the referenced schema only (no violation).
- `draft202012_applies_keywords_beside_ref`: same shape with `"$schema":"https://json-schema.org/draft/2020-12/schema"` and `$defs` against `json!("hi")` → sibling `type: integer` applies in 2020-12 → `violations` non-empty.
- `format_enforce_rejects_bad_email`: Inline `{"type":"string","format":"email"}` Enforce → `violations(&json!("not-an-email"))` non-empty.
- `format_annotate_accepts_bad_email`: same schema, `FormatMode::Annotate` → `violations(&json!("not-an-email"))` empty.

**Acceptance:**
- `cargo fmt --check --all` exits 0
- `cargo clippy -p camel-json-schema --all-targets -- -D warnings` exits 0
- `cargo test -p camel-json-schema` passes with exactly the tests above
- `grep -r "camel-" crates/camel-json-schema/Cargo.toml` finds no `camel-*` dependency

- [x] 1.1

### Task 1.2: Confined file retriever and rejection rules

**Files:**
- `crates/camel-json-schema/src/retriever.rs` (new)
- `crates/camel-json-schema/src/lib.rs` (modified)
- `crates/camel-json-schema/tests/confined_refs.rs` (new)

**Steps:**
1. Define a minimal `struct Budget { resources: usize, total_bytes: u64 }` in `src/retriever.rs` (populated in Task 1.3) and implement `fn resolve_confined(uri: &str, root_dir: &Path, budget: &mut Budget) -> Result<ResolvedResource, SchemaError>`, where `struct ResolvedResource { canonical_path: PathBuf, bytes: Vec<u8> }` is plan-owned. Checks run in order: (a) parse the URI; scheme must be `file` or empty-relative → else `RemoteRefRejected { uri }` for `http`/`https`/other remote schemes, `ExternalRefRejected` for unparseable; (b) percent-decode the path component; (c) `std::fs::canonicalize` the decoded path → on error `ExternalRefRejected { uri, root }`; (d) containment: canonical path must start with the canonical `root_dir` (component-wise `starts_with`, not string prefix) → else `ExternalRefRejected { uri, root }`; (e) regular-file check BEFORE any open so a FIFO never blocks (`symlink_metadata`/`canonicalize` metadata `file_type().is_file()`) → else `NotARegularFile { path }`; (f) size/budget checks per Task 1.3. After opening the file, re-verify regular-file on the open handle (`file.metadata()`) as TOCTOU hardening (a local writer swapping a symlink at the canonical path is a residual risk — recorded in ADR-0100 Consequences).
2. Implement the `jsonschema::Retrieve` trait impl: its `retrieve` returns `Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>>` (the pin's sync trait) — it calls `resolve_confined`, then parses `ResolvedResource.bytes` by the TARGET extension (Task 1.3) into the `Value`. Store the typed `SchemaError` in the capture slot before boxing (the slot mechanism is defined in Task 1.1 step 4 and shared by both retriever arms); `compile` recovers it verbatim on build failure. Test assertions match typed variants through this recovery mechanism (e.g. `matches!(err, SchemaError::RemoteRefRejected { .. })`), never string matching — string equality is used only where pinning a Display literal (the remote-error text).
3. For `SchemaSource::File(path)`: canonicalize the root schema path itself; `root_dir` = its parent directory (canonical); base URI = `file://<canonical root schema path>` via `with_base_uri`; the root document is read with the same bounded-read helper as includes (Task 1.3) and parsed per `SchemaSyntax` (`Auto` = extension: `.yaml`/`.yml` → serde_yml → `serde_json::to_value`, else serde_json). `Parse` errors preserve the existing literal shapes: JSON → `Parse { path, message: format!("invalid JSON schema '{path}': {e}") }`, YAML → `Parse { path, message: format!("invalid YAML schema '{path}': {e}") }` and conversion failure `format!("failed to convert YAML schema to JSON '{path}': {e}")` (field is `message`, matching the enum at step 2).
4. Reject a `$id`-driven rebase by intercepting resolved URIs in the retriever: any retrieved URI whose scheme is not `file`, or whose canonical path escapes `root_dir`, is rejected exactly like a direct `$ref` (a `RemoteRefRejected` for a remote `$id` rebase must carry the same literal local-files-only message).
5. Verify and record (for the park): `jsonschema::options().with_retriever(...)` + `with_base_uri(...)` compile and function with the workspace pin's features — this task's tests run WITHOUT `resolve-file` only after Task 1.4; until then note the API surface used.

**Tests:** (file `tests/confined_refs.rs`; each test builds a `tempfile::TempDir` with its own schema files; command `cargo test -p camel-json-schema`; expected: fail before implementation, pass after)
- `sibling_defs_ref_resolves_without_dollar_id`: root `root.json` (NO `$id`) `{"type":"object","properties":{"x":{"$ref":"defs.json#/$defs/x"}}}` + sibling `defs.json` `{"$defs":{"x":{"type":"integer"}}}` → `compile(SchemaSource::File(root), Auto, Enforce)` Ok; `violations(&json!({"x": 1}))` empty; `violations(&json!({"x": "s"}))` non-empty.
- `parent_directory_ref_rejected_naming_path`: root `$ref: "../outside.json"` with `outside.json` existing in the temp parent → `Err`, message contains `outside.json`, and the variant is the named rejection.
- `percent_encoded_traversal_rejected`: root `$ref: "%2e%2e%2foutside.json"` with `outside.json` outside the root dir → `Err` rejection naming the decoded path (contains `outside.json`).
- `symlink_escape_rejected`: root dir with symlink `link.json` → tempdir-outside `outside.json`; root `$ref: "link.json#/$defs/x"` → `Err` rejection (NOT silently resolved).
- `absolute_file_ref_outside_root_rejected`: root `$ref: "file:///etc/hostname"` → `Err` rejection naming `/etc/hostname`; assert the error is the named rejection (message matches the `ExternalRefRejected` shape), not a parse error.
- `remote_ref_rejected_with_local_files_only_error`: root `$ref: "https://example.com/s.json"` → `Err(SchemaError::RemoteRefRejected)` whose `to_string()` equals `remote $ref 'https://example.com/s.json' rejected: schemas resolve from local files only (ADR-0100); vendor the file`.
- `remote_dollar_id_rebase_rejected_with_local_files_only_error`: root `{"$id":"https://example.com/base.json","$ref":"s.json"}` (sibling `s.json` exists) → `Err` with the same local-files-only message naming the remote URI that the `$id` rebase produced.
- `out_of_root_file_dollar_id_rebase_rejected`: root `{"$id":"file:///tmp/outside-dir/base.json","$ref":"s.json"}` → `Err` rejection naming the escaped path.
- `fifo_rejected_without_blocking`: create a FIFO (via `std::process::Command::new("mkfifo")` or nix) named `pipe.json` in the root dir; root `$ref: "pipe.json"`; run `compile` under a 5-second `std::thread` + channel timeout guard → `Err(NotARegularFile)` and the call returns well before the timeout (assert the Ok/Err arrives within the join window; no `sleep`).

**Acceptance:**
- `cargo fmt --check --all` exits 0
- `cargo clippy -p camel-json-schema --all-targets -- -D warnings` exits 0
- `cargo test -p camel-json-schema` passes including Task 1.1's tests
- No `unwrap()` in non-test code (`cargo xtask lint-unwrap` exits 0)

- [x] 1.2

### Task 1.3: Resource caps, bounded reads, YAML includes, syntax override

**Files:**
- `crates/camel-json-schema/src/retriever.rs` (modified)
- `crates/camel-json-schema/src/lib.rs` (modified)
- `crates/camel-json-schema/tests/caps_and_formats.rs` (new)

**Steps:**
1. Add `struct Budget { resources: usize, total_bytes: u64 }` with constants `MAX_FILE_BYTES: u64 = 16 * 1024 * 1024`, `MAX_RESOURCES: usize = 256`, `MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024` as `pub const` in `lib.rs` (the park reports these).
2. Bounded read helper: `fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, SchemaError>` reading with `std::io::Read::take(limit + 1)`; if more than `limit` bytes arrive → `FileTooLarge { path, limit }` / `RootTooLarge` for the root. Never a metadata-only length check (file may grow during resolution). The root's bytes count toward the total budget.
3. Budget accounting in the retriever: root schema = resource 1 with its bytes counted; each distinct external resource counted once per `compile` (dedup by canonical path in a `HashSet<PathBuf>` inside `Budget`; repeated `$ref`s to the same canonical path do not re-count); on the 257th distinct resource → `TooManyResources { limit: 256 }`; when cumulative bytes would exceed 32 MiB → `TotalBudgetExceeded { limit: 32*1024*1024 }`. Budget is per `compile` call (fresh `Budget` in each `compile`), never shared across compilations. Fail closed: any arithmetic-checked exceed returns Err.
4. YAML include targets: the retriever parses retrieved documents by the TARGET extension (`.yaml`/`.yml` → `serde_yml::from_str::<serde_yml::Value>` → `serde_json::to_value`; anything else → `serde_json::from_slice`). Root parsing keeps following `SchemaSyntax` as in Task 1.2.
5. Cross-draft include support: do not force a draft on retrieved documents — the retrieved document's own `$schema` participates per jsonschema's referencing rules; no draft filtering in the retriever.

**Tests:** (file `tests/caps_and_formats.rs`; command `cargo test -p camel-json-schema`; expected: fail before implementation, pass after)
- `file_over_16_mib_rejected`: root `$ref: "big.json"` where `big.json` is a JSON file of size `16 * 1024 * 1024 + 1` bytes (write a valid JSON string padded with spaces) → `Err(FileTooLarge)` naming `big.json`.
- `file_at_16_mib_accepted`: same at exactly `16 * 1024 * 1024` bytes → `compile` Ok.
- `resource_budget_allows_256_rejects_257`: root + 255 sibling files (root chain: root `$ref`s `d1.json` … via `$defs` chains or a single `allOf` of 255 refs; each `dN.json` unique) → 256 resources compiles Ok; identical layout with 256 siblings (257 total) → `Err(TooManyResources)`.
- `total_budget_rejects_over_32_mib`: root + siblings whose combined sizes exceed 32 MiB while each file stays under 16 MiB → `Err(TotalBudgetExceeded)`; combined just under → Ok. (Generate padded JSON; keep the accepted run under 64 MiB disk in tempdir and clean up in the test.)
- `repeated_ref_counts_once`: root with 300 `$ref`s to the SAME sibling file (allOf of 300 identical refs) → compiles Ok (dedup: 2 resources); identical layout WITHOUT dedup semantics would count 301 resources and return `Err(TooManyResources)`, so the Ok result pins dedup enforcement.
- `json_root_referencing_yaml_works`: root `root.json` `{"properties":{"x":{"$ref":"defs.yaml#/$defs/x"}}}` + `defs.yaml` (`$defs: x: type: integer`) → Ok; `violations(&json!({"x":"s"}))` non-empty.
- `yaml_root_referencing_json_works`: root `root.yaml` referencing `defs.json#/$defs/x` → Ok and enforces.
- `explicit_syntax_override_keeps_sibling_resolution`: file `root.json` whose content is YAML (`$defs: x: type: integer` plus `$ref: "#/$defs/x"`-style sibling ref to `defs.json`) compiled with `SchemaSyntax::Yaml` despite the `.json` extension → Ok, sibling `defs.json` ref resolves; and the same file with `SchemaSyntax::Json` → `Err(Parse)` whose message contains `invalid JSON schema`.
- `referenced_file_declaring_other_draft_compiles`: root (no `$schema`, 2020-12) referencing `d7.json` that declares `"$schema":"http://json-schema.org/draft-07/schema#"` with `definitions` → compiles Ok and enforces the referenced definition.
- `draft07_definitions_through_external_ref`: root `{"$schema":"http://json-schema.org/draft-07/schema#","$ref":"defs.json#/definitions/x"}` + `defs.json` draft-07 with `definitions` → Ok and enforces.

**Acceptance:**
- `cargo fmt --check --all` exits 0
- `cargo clippy -p camel-json-schema --all-targets -- -D warnings` exits 0
- `cargo test -p camel-json-schema` passes (all three test files)
- The cap constants appear as `pub const` in `lib.rs` with doc comments citing ADR-0100

- [x] 1.3

### Task 1.4: Workspace wiring and fail-closed bare validator_for pin

**Files:**
- `Cargo.toml` (modified — root workspace)
- `scripts/xtask/trustpub-registrations.toml` (modified)
- `crates/camel-json-schema/tests/workspace_pin.rs` (new)

**Steps:**
1. Root `Cargo.toml`: the `members` glob `crates/camel-*` already covers the new crate — VERIFY this (`cargo metadata` lists `camel-json-schema`) and add a literal member entry ONLY if the glob does not cover it. Add to `[workspace.dependencies]` following the `camel-lint` precedent (Cargo.toml:95): `camel-json-schema = { path = "crates/camel-json-schema", version = "=0.69.0" }`.
2. Root `Cargo.toml`: change the `jsonschema` workspace dependency to `jsonschema = { version = "0.60", default-features = false }` (drop `features = ["resolve-file"]`; keep any other listed features if present — there are none today).
3. `scripts/xtask/trustpub-registrations.toml`: add a `[[crates]]` entry for `camel-json-schema` at the exact position matching `cargo xtask publish-order`. Read `scripts/xtask/src/lint_publish_registration.rs` first to enumerate the allowed `state` literals and pick the one meaning "publishable but trustpub not yet configured"; do NOT claim `state = "registered"` (trustpub cannot be configured before first publish) unless the manifest admits no other literal — if so, use `registered` and note the follow-up in the task result.
4. Confirm the whole workspace still builds: `cargo build --workspace` and that crates using `jsonschema::validator_for` on inline schemas (camel-dsl tests, camel-lint, camel-jms tests, xtask) are unaffected.

**Tests:** (file `tests/workspace_pin.rs`; command `cargo test -p camel-json-schema`; expected: fail before the feature removal semantics hold, pass after)
- `bare_validator_for_fails_closed_without_resolve_file`: the ORIGINAL VULNERABILITY SHAPE: create a tempdir with a valid schema file `defs.json` and pass a root schema `{"$ref":"file://<absolute-path>/defs.json#/$defs/x"}` (absolute `file:` URI to an existing file) to bare `jsonschema::validator_for` → must return `Err` (before `resolve-file` removal this compiled — the test must fail against the old pin and pass after Task 1.4 step 2). A relative ref without base URI is NOT sufficient here (it already failed before). Directly asserts the workspace pin property. If `jsonschema` is not re-exported by `camel-json-schema`, add `dev-dependency` `jsonschema` (workspace) on `camel-json-schema` — that is allowed (it is not a `camel-*` edge).
- `confined_crate_still_resolves_refs`: `compile(SchemaSource::File(...))` with a sibling ref (same arrangement as `sibling_defs_ref_resolves_without_dollar_id`) still compiles and enforces — proving the retriever API works with `features = []`.

**Acceptance:**
- `cargo build --workspace` exits 0
- `cargo xtask lint-publish-registration` exits 0 OR yields exactly one Case B finding for `camel-json-schema` pending owner first-publish + trustpub registration (degraded-gate, owner-action follow-up; claiming `registered` would be a false maintainer assertion)
- `cargo test -p camel-json-schema` passes including both pin tests
- `grep -n "resolve-file" Cargo.toml` finds nothing

- [x] 1.4

## camel-validator

### Task 2.1: Migrate compiled.rs to camel-json-schema behind a message-identical wrapper

**Files:**
- `crates/components/camel-validator/Cargo.toml` (modified)
- `crates/components/camel-validator/src/compiled.rs` (modified)

**Steps:**
1. Add dependency `camel-json-schema.workspace = true` (workspace dep registered in Task 1.4).
2. Replace `compile_json_schema` (and its "Twin of the helper…" doc comment — DELETE the twin comment entirely) with delegation to `camel_json_schema::compile`. For FILE-BACKED schemas, `compile_json`/`compile_yaml_schema` call `compile(&SchemaSource::File(path), <syntax from SchemaType>, <FormatMode from validate_formats>)`. `SchemaType::Json` → `SchemaSyntax::Json`, `SchemaType::Yaml` → `SchemaSyntax::Yaml` (the `type`/`schemaType` override is preserved because the wrapper passes the resolved type, never `Auto`, for `File` sources).
3. Content sources: when the schema comes from `embedded_ref_bytes` (registry, `camel-embedded:`), the wrapper pre-parses the bytes to `serde_json::Value` per `SchemaType` (JSON → `serde_json::from_slice`, YAML → `serde_yml` parse + `serde_json::to_value`), preserving today's parse literals exactly, and compiles `SchemaSource::Inline(value)` with `SchemaSyntax::Auto` (syntax is meaningless for a `Value`; embedded schemas keep their existing YAML/JSON dispatch and the `embedded_yaml_schema_resolves_from_registry` test passes unmodified). When the schema comes from `FilesystemResolver::resolve`, use `SchemaSource::File(path)` — do NOT pre-parse to `Value`, so sibling refs resolve under confinement. Preserve the existing MAX_SCHEMA_BYTES cap behavior and its error literal `"schema file '{path_str}' exceeds {MAX_SCHEMA_BYTES} bytes"` (the crate's `RootTooLarge` Display carries this literal).
4. Preserve compile-error literals byte-identically by variant: `SchemaError::Parse` ⇒ `CamelError::EndpointCreationFailed(message)` VERBATIM (its text already equals today's final `invalid JSON schema '{path}': {e}` / `invalid YAML schema '{path}': {e}` / conversion literal — do NOT re-wrap in `failed to compile …`); `SchemaError::Compile` ⇒ `CamelError::EndpointCreationFailed(format!("failed to compile JSON schema '{path}': {e}"))` for JSON and `"failed to compile YAML schema '{path}': {e}"` for YAML. `SchemaError::Open`/`Read`/`Utf8`/cap Displays reproduce today's `failed to open schema file '{path}': {e}` / `failed to read schema file '{path}': {e}` / `YAML schema '{path}' is not valid UTF-8: {e}` / `schema file '{path}' exceeds {limit} bytes` texts (Task 1.1), so surface them verbatim as `EndpointCreationFailed`. New ref-resolution errors (`ExternalRefRejected`, `RemoteRefRejected`, `NotARegularFile`, budget caps) are allowed to be new text (they name the rejected path/URI) — note them in the task result for the park.
5. Validation-time formatting: replace direct `jsonschema::Validator::iter_errors` usage with `CompiledSchema::violations` and format each as `format!("{message} at {instance_path}")` joined with `"\n"` under `format!("JSON Schema validation failed:\n{}", ...)` and `format!("YAML Schema validation failed:\n{}", ...)` — byte-identical to `compiled.rs:569–608` today. Keep `CompiledValidator::{Json,Yaml}` enum shape and all body-handling error strings unchanged.
6. `validate_formats` bool maps to `FormatMode::{Enforce, Annotate}` (`true` → Enforce).
7. Remove the `jsonschema` dependency from `crates/components/camel-validator/Cargo.toml` once nothing in the crate references it (`grep -n "jsonschema" crates/components/camel-validator/src/` must find nothing; `cargo build -p camel-component-validator` confirms). This enforces the ADR claim that `camel-json-schema` is the only place that compiles refs.

**Tests:** (in `crates/components/camel-validator/src/compiled.rs` existing `mod tests`; commands `cargo test -p camel-component-validator`; expected: existing tests pass UNMODIFIED, new tests fail before implementation, pass after)
- ALL existing tests in this file pass without any modification to their assertions (byte-identical contract): `json_format_email_rejected_by_default`, `json_format_email_accepted_when_opted_out`, `yaml_format_enforced_by_default`, `yaml_format_opt_out_accepts`, `json_unknown_format_is_annotation`, `embedded_json_schema_resolves_from_registry`, `embedded_yaml_schema_resolves_from_registry`.
- `embedded_schema_external_ref_rejected`: register a `camel-embedded:` schema whose content contains `$ref: "defs.json#/$defs/x"` → `CompiledValidator::compile` returns `Err` whose message contains `defs.json` and the named rejection (not a JSON parse error).
- `validator_sibling_ref_resolves_from_file`: a file-backed JSON schema dir with `root.json` + `defs.json` (no `$id`) → compiles via the validator config path and validates an instance correctly.
- `schema_type_override_yaml_root_with_sibling_ref`: `ValidatorConfig` with `schema_type = Yaml` on a path `root.json` whose content is YAML referencing `defs.yaml` → compiles and enforces (the override beats the extension; sibling refs still confined).
- `validation_failure_message_byte_identical`: one failing instance (`{"x": "s"}` against `{"type":"object","properties":{"x":{"type":"integer"}}}`) in a file-backed JSON schema; assert the exact `CamelError` string equals `JSON Schema validation failed:\n` followed by the failing error's Display text, a space, `at`, a space, and the instance path `/x`, with the Display text hardcoded as the exact literal produced by the pre-change code (run this test against the unmodified behavior first to capture that literal). Expected: passes BEFORE the migration (capture run) and passes AFTER — it guards byte-identity.

**Acceptance:**
- `cargo fmt --check --all` exits 0
- `cargo clippy -p camel-component-validator --all-targets -- -D warnings` exits 0
- `cargo test -p camel-component-validator` passes
- `grep -n "Twin of the helper" crates/components/camel-validator/src/compiled.rs` finds nothing

- [x] 2.1

## camel-processor

### Task 3.1: Migrate json_schema_validate.rs to camel-json-schema (inline-only)

**Files:**
- `crates/camel-processor/Cargo.toml` (modified)
- `crates/camel-processor/src/json_schema_validate.rs` (modified)
- `crates/camel-processor/tests/json_schema_validate.rs` (modified)

**Steps:**
1. Add dependency `camel-json-schema.workspace = true` (workspace dep registered in Task 1.4).
2. Replace the local `compile_json_schema` (DELETE its twin doc comment) and the `jsonschema::Validator` field with `camel_json_schema::CompiledSchema` compiled from `SchemaSource::Inline(schema.clone())` with `SchemaSyntax::Auto`, `FormatMode::Enforce` (today's behavior forces `should_validate_formats(true)`; the existing spec requirement "REST request_schema has no format opt-out" holds).
3. Keep the service shape (`JsonSchemaValidateService<P>`), body handling (only `Body::Json` validated), and error literals: construction failure `CamelError::RouteError(format!("invalid JSON schema: {e}"))`; per-error `format!("{message} at {instance_path}")` joined `"; "`; failure `CamelError::ValidationError(format!("JSON schema validation failed: {msg}"))`.
4. External `$ref` in a `request_schema` now fails at construction with the named rejection (this is the decision-5 behavior change — intended).
5. Remove the `jsonschema` dependency from `crates/camel-processor/Cargo.toml` once nothing in the crate references it (`grep -n "jsonschema" crates/camel-processor/src/` must find nothing; `cargo build -p camel-processor` confirms).

**Tests:** (in `crates/camel-processor/tests/json_schema_validate.rs`; command `cargo test -p camel-processor`; expected: existing tests pass UNMODIFIED, new tests fail before implementation, pass after)
- ALL existing tests pass unmodified: `passes_valid_body`, `rejects_missing_required_field`, `rejects_wrong_type`, `non_json_body_skips_validation`, `invalid_schema_returns_error_at_build`, `empty_body_skips_validation`, `can_be_chained_behind_inner_processor`, `format_email_rejects_malformed_value`, `format_email_accepts_valid_value`, `pattern_enforcement_unchanged`, `unknown_format_is_annotation`, `draft07_schema_enforces_format`, `schema_without_dollar_schema_enforces_format_2020_12`.
- `request_schema_internal_ref_works`: schema `{"$defs":{"obj":{"type":"object","required":["a"]}},"$ref":"#/$defs/obj"}` → service builds Ok; `{"a":1}` passes; `{}` fails with `ValidationError`.
- `request_schema_external_ref_rejected_at_build`: schema `{"$ref":"defs.json#/$defs/x"}` → `JsonSchemaValidateService::new` returns `Err` whose message contains `defs.json` and `rejected`.
- `request_schema_remote_ref_error_preserves_literal`: schema `{"$ref":"https://example.com/s.json"}` → `Err` whose message contains the exact literal `remote $ref 'https://example.com/s.json' rejected: schemas resolve from local files only (ADR-0100); vendor the file` (proves the literal survives `CamelError` wrapping).

**Acceptance:**
- `cargo fmt --check --all` exits 0
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0
- `cargo test -p camel-processor` passes
- `grep -n "carries the twin" crates/camel-processor/src/json_schema_validate.rs` finds nothing

- [x] 3.1

## camel-component-mcp

### Task 4.1: Migrate MCP validate_arguments to camel-json-schema (inline-only)

**Files:**
- `crates/components/camel-component-mcp/Cargo.toml` (modified)
- `crates/components/camel-component-mcp/src/adapter/server.rs` (modified)

**Steps:**
1. Add dependency `camel-json-schema.workspace = true` (workspace dep registered in Task 1.4).
2. In `validate_arguments` (~line 507), replace `jsonschema::validator_for(input_schema)` with `camel_json_schema::compile(&SchemaSource::Inline(input_schema.clone()), SchemaSyntax::Auto, FormatMode::DraftDefault)`. `DraftDefault` leaves `should_validate_formats` unset so upstream's native draft-and-vocabulary format defaults are preserved EXACTLY (no helper approximating them from the root `$schema` — that would weaken validation for format-assertion dialects and mixed-draft resources). Keep the existing error literals: invalid schema → `ErrorData::invalid_params(format!("tool '{name}' has an invalid input schema: {error}"))`; violations → message `"tool '{name}' arguments failed input schema validation"` with `data = {"violations": [...]}` where each violation string stays `format!("{error} at {}", error.instance_path())` (reconstruct from `Violation { message, instance_path }` as `format!("{message} at {instance_path}")`).
3. Update the doc comment on `validate_arguments` ("Uses the same crate as `camel-processor`…") to name `camel-json-schema` as the shared crate.
4. Remove the `jsonschema` dependency from `crates/components/camel-component-mcp/Cargo.toml` once nothing in the crate references it (`grep -n "jsonschema" crates/components/camel-component-mcp/src/` must find nothing; `cargo build -p camel-component-mcp` confirms).

**Tests:** (add to the crate's existing `#[cfg(test)]` coverage near `validate_arguments` or `crates/components/camel-component-mcp/src/adapter/server.rs` tests module; command `cargo test -p camel-component-mcp`; expected: existing tests pass unmodified, new tests fail before implementation, pass after)
- `validate_arguments_internal_ref_works`: tool input schema with `{"$defs":{...},"$ref":"#/$defs/..."}` and matching arguments → Ok.
- `validate_arguments_external_ref_rejected`: tool input schema `{"$ref":"defs.json#/$defs/x"}` → `invalid_params` error whose message contains `defs.json` and `rejected`.
- `validate_arguments_violation_shape_unchanged`: invalid arguments against a simple schema → error data `violations` contains strings shaped `"{error} at {instance_path}"` (assert one exact expected string).
- `format_default_absent_schema_is_annotation`: tool input schema `{"type":"string","format":"email"}` (no `$schema`) with argument `"not-an-email"` → accepted (formats are annotations under `validator_for` for the 2020-12 default) — identical before and after the migration.
- `format_default_2020_12_is_annotation`: same with `"$schema":"https://json-schema.org/draft/2020-12/schema"` and `"not-an-email"` → accepted — identical before and after.
- `format_default_draft07_enforces`: same with `"$schema":"http://json-schema.org/draft-07/schema#"` and `"not-an-email"` → rejected — identical before and after.
- `format_default_bundled_format_assertion_enforces`: tool input schema `{"$schema":"https://json-schema.org/draft/2020-12/meta/format-assertion","type":"string","format":"ipv4"}` with argument `"not-an-ipv4"` → REJECTED (the format-assertion vocabulary enforces despite 2020-12) — identical before and after; this pins the case a root-draft approximation would weaken.
- `format_default_mixed_draft_inline_resources_match_native`: tool input schema with `$ref: "#/$defs/x"` where the bundled `$defs.x` declares draft-07 `format` semantics while the root declares 2020-12 → behavior identical before and after (no assertion direction change).

**Acceptance:**
- `cargo fmt --check --all` exits 0
- `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exits 0
- `cargo test -p camel-component-mcp` passes

- [x] 4.1

## docs

### Task 5.1: ADR-0100 and CONTEXT-MAP index entry

**Files:**
- `docs/adr/0100-confined-json-schema-reference-resolution.md` (new)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Write ADR-0100 "Confined JSON Schema reference resolution" in the house format (header: `- Date: 2026-11-16` / `- Status: Accepted (2026-11-16)` / `- Context: change \`schemaconf\`; bd \`rc-t5mgq\`` / `- Amends: none` / cross-refs to ADR-0091 (style) and any prior validation ADRs). Sections: Context (the measured `resolve-file` hole, the three consumers, relative-ref ergonomics), Decision (`with_base_uri` + confined retriever, path-based checks, caps 16 MiB / 256 resources / 32 MiB total, bounded reads, inline-only retrieval for inline schemas, `resolve-file` removed from the workspace pin, `resolve-http` never enabled, remote error literal `remote $ref '<uri>' rejected: schemas resolve from local files only (ADR-0100); vendor the file`, leaf crate `camel-json-schema`), Consequences (bare `validator_for` fails closed; vendor files to include them; new ref errors are new message text while existing validation messages stay byte-identical).
2. Add the ADR index line to the `## Architecture Decisions` list in `CONTEXT-MAP.md`, matching the house shape (index lines carry a trailing consumer parenthetical): `- [0100](./docs/adr/0100-confined-json-schema-reference-resolution.md) — Confined JSON Schema reference resolution (camel-json-schema; consumed by camel-component-validator, camel-processor, camel-component-mcp)`.
3. Run the docs gate: `cd docs && mdbook test`.
4. In ADR-0100's Consequences record the residual TOCTOU risk: confinement checks run on the real path before open with a regular-file re-verification on the open handle; a local writer swapping a symlink at the canonical path inside that window is out of threat model (operator-supplied config) but documented.

**Tests:** (docs gate; command `cd docs && mdbook test`; expected: passes)
- `mdbook test`: all doc tests pass with the new ADR present (the ADR contains no Rust doc-tests; the gate asserts the docs toolchain builds).

**Acceptance:**
- `cd docs && mdbook test` exits 0
- ADR-0100 exists with the header fields above and the Consequences section records the residual TOCTOU risk
- `CONTEXT-MAP.md` index line for 0100 matches the house line shape of its neighbors

- [x] 5.1

### Task 5.2: Crate-local CONTEXT.md for camel-json-schema

**Files:**
- `crates/camel-json-schema/CONTEXT.md` (new)

**Steps:**
1. Write `crates/camel-json-schema/CONTEXT.md` in the house `CONTEXT.md` shape (follow `crates/camel-lint/CONTEXT.md` as the leaf-crate precedent) covering: the crate's role (the ONLY place that compiles JSON Schema `$ref`s — runtime-free leaf; three consumers: camel-component-validator, camel-processor, camel-component-mcp); the confinement contract (base URI + path-based retriever checks; fail closed; remote error literal); the resource budgets (16 MiB per file, 256 resources / 32 MiB total, dedup by canonical path, per-compile, bounded reads); inline-only retrieval for `SchemaSource::Inline`; typed error recovery (`SchemaError` capture slot — boxing alone loses the variant behind `referencing::Error::Unretrievable`); `FormatMode` semantics including `DraftDefault` preserving native draft-and-vocabulary format defaults; and the residual TOCTOU risk (documented in ADR-0100).
2. Reference ADR-0100 from the CONTEXT.md.

**Tests:** (docs gate; command `cd docs && mdbook test`; expected: passes)
- `mdbook test`: docs toolchain passes with the new CONTEXT.md present.

**Acceptance:**
- `crates/camel-json-schema/CONTEXT.md` exists with the sections above
- `cargo xtask lint-context-citations` exits 0 (crate CONTEXT.md files are what this lint walks)

- [x] 5.2
