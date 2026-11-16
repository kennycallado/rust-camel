# Design: schemaconf

## Approach

New leaf crate `crates/camel-json-schema` owns JSON Schema compilation everywhere.

**Public surface** (ADR-0091 style: enums over bool, named fields, `Into` where useful):

- `SchemaSource::{File(PathBuf), Inline(serde_json::Value)}`
- `SchemaSyntax::{Auto, Json, Yaml}` — root-document parser selection; `Auto` follows the root file extension (JSON for `Inline`). Explicit selection preserves the validator's `type`/`schemaType` query override (`config.rs` `resolve_schema_type`), which can disagree with the extension.
- `FormatMode::{DraftDefault, Enforce, Annotate}` — `DraftDefault` leaves `should_validate_formats` unset (native upstream draft-and-vocabulary format defaults, as `validator_for`); `Enforce`/`Annotate` force it (mirroring today's `validate_formats` bool at the validator call sites).
- `compile(source: &SchemaSource, syntax: SchemaSyntax, format: FormatMode) -> Result<CompiledSchema, SchemaError>`
- `CompiledSchema::violations(&self, instance: &Value) -> Vec<Violation { instance_path: String, message: String }>`
- `SchemaError` (`thiserror`): `ExternalRefRejected { uri, root }`, `RemoteRefRejected { uri }`, `FileTooLarge { path, limit }`, `RootTooLarge { path, limit }`, `TooManyResources { limit }`, `TotalBudgetExceeded { limit }`, `NotARegularFile { path }`, `Open { path, message }`, `Read { path, message }`, `Utf8 { path, message }`, `Parse { path, message }`, `Compile { message }` (textual fields named `message`, never `source` — `thiserror` reserves `source` for error chaining) — messages name the offending path/URI. `Parse` carries the existing final literals (`"invalid JSON schema '{path}': {e}"`, `"invalid YAML schema '{path}': {e}"`, `"failed to convert YAML schema to JSON '{path}': {e}"`), `Open`/`Read`/`Utf8`/cap Displays reproduce `"failed to open schema file '{path}': {e}"`, `"failed to read schema file '{path}': {e}"`, `"YAML schema '{path}' is not valid UTF-8: {e}"`, and `"schema file '{path}' exceeds {limit} bytes"` so the validator wrapper keeps its existing literals byte-identical. A file root stays `File` throughout compilation (never pre-parsed into `Inline`), so legitimate sibling references keep working under confinement; regression-tested with an explicit syntax override plus a sibling ref.

**Confined retrieval.** Root schema compiles with `with_base_uri(file://<canonical root schema path>)` AND a custom retriever (`jsonschema::options().with_retriever(..)`). Confinement is checked inside the retriever on the **real file path**, never on the URI: `file` scheme only; percent-decode; `std::fs::canonicalize` (also resolves symlinks); containment check against the canonical root schema directory (root schema dir for `SchemaSource::File`; none for `Inline`); `file_type().is_file()` (FIFO rejected without opening); 16 MiB per file; 256 resources / 32 MiB total; fail closed past any cap. A `$id` that rebases to `https:` or to an out-of-root `file:` URI is rejected — the base URI alone is not trusted (e_opus ruling: `$id` overrides it). Reads are **bounded** (read at most cap+1 bytes, reject if over — never metadata-only size checks; covers file growth during resolution), following the existing bounded-read precedent in `camel-validator/src/resolver.rs`. Budget accounting: the root schema counts as resource 1 and its bytes count toward the total; each distinct external resource is counted once per compilation (deduplicated by canonical path; repeated `$ref`s to the same file are free); the budget is per `compile` call and never shared or reset mid-compilation. Remote refs are rejected with the literal error `remote $ref '<uri>' rejected: schemas resolve from local files only (ADR-0100); vendor the file`, preserved verbatim through consumer error wrapping (both for direct remote refs and for a remote `$id` rebase).

**Inline-only retrieval** for `SchemaSource::Inline`: `#` fragment refs resolve from the root document; every external ref fails with a named error, classified by the resolved URI's scheme (upstream rebases relative refs before retrieval): the synthetic inline-base scheme and `file` fail with `ExternalRefRejected`; remote schemes fail with `RemoteRefRejected` and the literal local-files-only error. Used by processor `request_schema` and `camel-embedded:` schemas.

**YAML include targets** (`$ref: defs.yaml#..`) parse by the **target** extension: `.yaml`/`.yml` → `serde_yml` (via the `noyalib` compat shim, matching `camel-validator`'s existing migration off serde_yml for RUSTSEC-2025-0068), anything else → `serde_json`. Tested JSON root → `defs.yaml` and YAML root → `defs.json`.

**Drafts.** `$schema` selects the draft; 2020-12 when absent. Pinned: draft-07 `definitions` through an external ref; 2020-12 `$defs`; a referenced file declaring a different draft; draft-07 ignoring keywords beside `$ref` vs 2020-12 applying them (one assertion each).

**Workspace pin.** Root `Cargo.toml` drops `resolve-file` (`features = []`). The retriever API is exercised without that feature (verified by the new crate's tests) and reported in the park. A bare `validator_for` (camel-dsl tests, camel-lint, camel-jms tests, xtask — all inline-schema users) then fails closed on external refs; a pin test asserts a bare `jsonschema::validator_for` on a schema with an external `$ref` fails once `resolve-file` is gone. None of those callers resolve files today.

**Consumers.** `camel-validator` keeps a thin wrapper translating `Violation` into its existing message strings byte-identically (only new ref errors are new). `camel-processor` calls `compile` + `violations` directly (Enforce, as today). `camel-component-mcp` migrates `validator_for` → confined compile of an inline source with `FormatMode::DraftDefault`, preserving `validator_for`'s native draft-and-vocabulary format defaults exactly (no root-draft approximation — it would weaken format-assertion dialects). Both "twin" comments are deleted.

## Affected crates

- `crates/camel-json-schema` (new): confined compilation + violation reporting; deps `jsonschema`, `serde_json`, `noyalib` (compat-serde-yaml), `thiserror`; no `camel-*` edges.
- `crates/components/camel-validator`: `compiled.rs` delegates to the new crate behind a message-compatible wrapper; `camel-embedded:` uses inline-only retrieval.
- `crates/camel-processor`: `json_schema_validate.rs` migrates; inline-only retrieval.
- `crates/components/camel-component-mcp`: `adapter/server.rs:507` migrates (`validator_for` → confined compile of an inline source).
- Root `Cargo.toml`: `jsonschema` features `[]`; register `camel-json-schema` in `scripts/xtask/trustpub-registrations.toml` (publish order).
- `docs/adr/0100-confined-json-schema-reference-resolution.md` (new).

## Architecture boundaries

Components and DSL stay data-plane; the new crate is a pure leaf library (like `camel-lint`'s runtime-free engine) with no dependency on Runtime/DSL/Components in either direction — only the three consumers depend on it. Control-plane config loading (MCP operator config) compiles schemas through the same confined path. No new edges in the CONTEXT-MAP relationship graph beyond Components/DSL → leaf library.

## Alternatives considered

- Base URI alone (no retriever): rejected — `$id` overrides the base URI (e_opus ruling).
- Fixing the three call sites with hand-rolled path checks: rejected — the hole is the retriever contract; a shared leaf crate removes the twin-helper duplication and is the only place that compiles refs.
- Keeping `resolve-file` and layering checks on top: rejected — a bare `validator_for` would stay unsafe-by-default (decision 3).
- A `camel-core`-resident helper: rejected — creates dependency edges from the leaf consumers to Runtime.
