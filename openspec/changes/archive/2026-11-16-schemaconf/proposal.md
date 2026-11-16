# Proposal: schemaconf

## Why

The workspace pins `jsonschema = { version = "0.60", default-features = false, features = ["resolve-file"] }`. With exactly those features, a schema `$ref: "file:///etc/hostname"` is read and parsed (the failure is a JSON parse error, not a policy rejection): any file the process can read is reachable through schema compilation, and a FIFO would block boot. Relative refs without a `$id` fail with `No base URI is available`. Three consumers share the hole: `camel-validator/src/compiled.rs`, `camel-processor/src/json_schema_validate.rs` (inline `request_schema` from route YAML), and `camel-component-mcp/src/adapter/server.rs` (operator config). bd: `rc-t5mgq` (p1, security).

## What Changes

- New leaf crate `crates/camel-json-schema`: `SchemaSource::{File(PathBuf), Inline(Value)}`, `FormatMode::{DraftDefault, Enforce, Annotate}`, `compile(..) -> Result<CompiledSchema, SchemaError>`, `CompiledSchema::violations(&Value) -> Vec<Violation { instance_path, message }>`. Deps: `jsonschema`, `serde_json`, `serde_yml` (via the `noyalib` compat shim), `thiserror`. No `camel-*` dependencies.
- Confined `$ref` resolution: `with_base_uri(file://<canonical schema path>)` plus a retriever that checks the real file path (file scheme only, percent-decode, canonicalize, containment in the canonical root schema directory, regular files only, 16 MiB per file, 256 resources / 32 MiB total, fail closed past any cap). Remote refs get a named error.
- Inline schemas (processor `request_schema`, `camel-embedded:`) get an inline-only retriever: `#` refs work, external refs fail with a named error.
- Remove `resolve-file` from the root `Cargo.toml`; a bare `validator_for` then fails closed on external refs.
- YAML include targets parse by target extension (`.yaml`/`.yml` → `serde_yml`).
- Draft handling: `$schema` selects the draft, 2020-12 when absent.
- Migrate the three consumers; the validator keeps a thin wrapper so existing message text stays byte-identical. Delete both "twin" comments.
- ADR-0100 "Confined JSON Schema reference resolution" + normative requirement *Schema reference resolution is confined* in `openspec/specs/json-schema-validation`.

Excluded: `camel-matchers` and test tiers (`rc-0lvd3`), alias map (`rc-h62ve`), embedded sibling refs (`rc-xbfkp`), scenario-tier target (`rc-v0bxl`), `resolve-http` (never enabled).

## Acceptance criteria

Eight adversarial tests fail closed with named errors (or pass where positive): sibling `defs.json#/$defs/x` resolves; `../outside.json` rejected naming the path; symlink escape rejected; absolute `file:///etc/hostname` rejected; `https://` ref (or `https` `$id` rebase) rejected with the local-files-only error; >16 MiB file and FIFO rejected without blocking; inline `request_schema` external ref rejected while `#` refs work; JSON root referencing a YAML file works. Plus (expert fixes): percent-encoded traversal rejected; out-of-root `file:` `$id` rebase rejected; explicit `type`/`schemaType` root override keeps sibling resolution; deterministic budget tests at/beyond 256 resources and 32 MiB; a pin test that bare `validator_for` fails closed without `resolve-file`. Validator error strings stay byte-identical. All gates green.

## Risk budget

Behavior change acceptable only for previously-unsafe external ref resolution (now rejected). Existing validation message text must not change. No network. No dependency edges between the new leaf crate and `camel-*` crates in either direction beyond the three consumers. Out of bounds: matcher/test-tier changes, any refactor beyond the migration surface.
