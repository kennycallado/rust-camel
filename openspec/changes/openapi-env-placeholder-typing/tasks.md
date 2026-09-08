# Tasks: openapi-env-placeholder-typing

## camel-dsl

### Task 1.1: `extract_rest_blocks_from_file_with_env` sibling in yaml.rs

**Files:**
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/src/lib.rs` (modified — add the new fn to the existing yaml re-export block at lib.rs:50)

**Steps:**
1. Add `pub fn extract_rest_blocks_from_file_with_env(path: &Path, lookup: &dyn Fn(&str) -> Option<String>) -> Result<Vec<RouteDslRest>, CamelError>` in `crates/camel-dsl/src/yaml.rs`, placed directly after `load_from_file_with_env` (yaml.rs:2086 region).
2. Body: read via `crate::util::read_route_file_capped(path)` (pub(crate), 16 MiB cap) with an error-logged failure like `load_from_file_with_env` does (log-policy: system-broken).
3. Extension dispatch mirroring `interpolate_for_parse` (discovery.rs:284-292):
   - `path.extension() == Some("json")`: `crate::env_interpolation::interpolate_env_with(&content, lookup)`; on `Err(var)` return `CamelError::RouteError(format!("Environment variable '{var}' not set (required by {})", path.display()))` — the loader's verbatim wording (yaml.rs:2099 region); then `serde_json::from_str::<crate::route_ast::RouteDslRoutes>(&interpolated)` mapped to `.rest` with a path-annotated parse error.
   - otherwise (`yaml`/`yml`/no extension): `crate::env_interpolation::interpolate_yaml_source(&content, lookup)` with the same `Err(var)` wrapping, then the existing `extract_rest_blocks(&interpolated)` with a path-annotated error.
4. Note the ONE intentional divergence from `interpolate_for_parse`: discovery sends no-extension/odd-extension files to the splice `_` arm (discovery.rs:289-292), while this fn sends them to the tree-walk arm — deliberate, to preserve this surface's current YAML-parse behavior and comment hermeticity for the `_ => extract_rest_blocks` fallback files. Do NOT "fix" the dispatch to match discovery.
5. Add the fn to the existing yaml re-export block in `crates/camel-dsl/src/lib.rs` (lib.rs:50).
6. Write the unit tests below in the existing `#[cfg(test)] mod tests` in yaml.rs (tempfiles via the same `tempfile` dev-dependency pattern the file's tests already use; check Cargo.toml dev-deps first — `load_from_file_with_env` tests exist from ac147d07, mirror their style and their ambient-env/round-trip-fragile patterns where applicable).

**Tests:** (executable spec — name, setup, action, assert)
- `extract_rest_from_file_string_default_resolves`: temp YAML file containing:\n`rest:\n  - host: ${env:HOST:-0.0.0.0}\n    port: 9090\n    path: /api/users\n    operations:\n      - method: GET\n        operation_id: listUsers\n        to: direct:listUsers\n` → call with `&|_| None` → Ok; `rest[0].host == "0.0.0.0"`.
- `extract_rest_from_file_int_field_fails_type_mismatch`: temp YAML with `port: ${env:PORT:-8080}` → call → Err; assert the error class mirrors `load_from_file_int_placeholder_fails_boot_parity` (yaml.rs:5147-5153): type-mismatch wording (`invalid type: string`, `expected u16`), and NO env wording (no variable name, no `not set`).
- `extract_rest_from_file_no_default_names_variable`: temp YAML with `host: ${env:REST_HOST}` → call → Err whose message contains `REST_HOST` and `not set`.
- `extract_rest_from_file_escape_stays_literal`: temp YAML with `host: $${env:KEEP:-x}` → Ok; `rest[0].host == "${env:KEEP:-x}"` (one `$` stripped). Same test also asserts a second field `path: a$$b` → `rest[0].path == "a$b"` (bare `$$` yields one `$`).
- `extract_rest_from_file_comment_placeholder_harmless`: temp YAML whose comment contains `${env:MISSING}` and whose body is literal → Ok.
- `extract_rest_from_file_round_trip_fragile_parse_error_not_env`: temp YAML using a tagged node (mirror the loader's round-trip-fragile construction, yaml.rs:5249-5278) with a defaulted token in a value position → Err whose message is the document's own YAML parse error; assert it contains NO env wording (no variable name, no `not set`) — the token resolved, so the failure belongs to the document, proving the splice fallback ran.
- `extract_rest_from_file_json_string_resolves`: temp `.json` file `{"rest":[{"host":"${env:HOST:-0.0.0.0}","port":9090,"path":"/api/users","operations":[{"method":"GET","operation_id":"listUsers","to":"direct:listUsers"}]}]}` → Ok; `rest[0].host == "0.0.0.0"`.
- `extract_rest_from_file_json_int_fails`: temp `.json` with `"port":"${env:PORT:-8080}"` → Err (type mismatch).
- `extract_rest_from_file_json_no_default_names_variable`: temp `.json` with `"host":"${env:REST_HOST}"` → Err containing `REST_HOST` and `not set`.
- `extract_rest_from_file_oversized_rejected`: temp file larger than `MAX_ROUTE_FILE_SIZE` (mirror `load_from_file_rejects_oversized_file`, yaml.rs:5071-5088 — sparse `set_len` trick, not 16 MiB of real bytes) → Err with the size-cap error, not a parse error.
- `extract_rest_from_file_ambient_env_ignored`: mirror `load_from_file_ambient_env_ignored_by_default_path` (yaml.rs:5192-5217) exactly — same `EnvGuard` + SAFETY-commented `unsafe { std::env::set_var(...) }` pattern (no `#[serial]`; serial_test is not a camel-dsl dev-dep), temp YAML with `host: ${env:RC_GYKDS_VAR:-0.0.0.0}`, ambient `RC_GYKDS_VAR=evil` set, call with `&|_| None` → Ok and `rest[0].host == "0.0.0.0"`; remove the var in the guard's teardown.

**Acceptance:**
- `cargo test -p camel-dsl yaml:: --lib` passes including the new tests.
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.
- `cargo fmt --check` clean for the touched files.
- `rg -n "pub use yaml" crates/camel-dsl/src/lib.rs` shows the new fn re-exported.

- [x] 1.1

## camel-cli

### Task 1.2: `run_generate` routes through the interpolating sibling

**Files:**
- `crates/camel-cli/src/commands/openapi.rs` (modified)

**Steps:**
1. Replace the `std::fs::read_to_string` + extension `match` block (openapi.rs:38-56) with a single call: `let rest_blocks = camel_dsl::extract_rest_blocks_from_file_with_env(Path::new(&args.file), &|_| None).map_err(|e| format!("route file error: {e}"))?;` (error-prefix wording consistent with the command's existing style; keep the `no 'rest:' blocks found in file` guard and the lowering/duplicate validation untouched).
2. Remove the now-unused `use camel_dsl::yaml::extract_rest_blocks;` import; keep `use std::path::Path;`.
3. Update the module doc comment (openapi.rs:1-2) and `run_generate` doc comment to state that `${env:}` placeholders in `rest:` blocks resolve default-only at generate time (string positions take the concrete default; int/bool positions fail per the tree-walk canon; ambient env is never read).
4. Add the command-level tests below to the existing `mod tests`.

**Tests:** (executable spec)
- `generate_resolves_string_default_in_server_url`: tempfile `.yaml` with `host: ${env:HOST:-0.0.0.0}`, `port: 9090`, one GET op → `run_generate` Ok; `doc["servers"][0]["url"] == "http://0.0.0.0:9090"` and the serialized document contains no `${env` substring.
- `generate_int_port_placeholder_errors`: tempfile `.yaml` with `port: ${env:PORT:-8080}` → Err; message is the type-mismatch class (`invalid type: string` / `expected u16`), with no env wording and no variable name.
- `generate_no_default_names_variable`: tempfile `.yaml` with `host: ${env:REST_HOST}` → Err containing `REST_HOST` and `not set`.
- `generate_success_status_placeholder_errors`: tempfile `.yaml` with `success_status: ${env:CODE:-201}` → Err of the type-mismatch class (`invalid type: string` / `expected u16`), with no env wording.
- `generate_free_value_schema_position_resolves`: tempfile `.yaml` with `request_schema.properties.name.type: ${env:T:-string}` → Ok; emitted schema property `type == "string"` and document contains no `${env`.
- `generate_json_arm_parity`: tempfile `.json` (string-default host resolves; port placeholder fails; no-default names variable) — three assertions mirroring the YAML tests.
- Existing tests `generate_from_yaml_file`, `generate_from_json_file`, `error_when_no_rest_blocks`, `validation_error_on_duplicate_path_verb`, `error_when_file_not_found` still pass unchanged (they carry no placeholders — behavior within the size cap is unchanged).

**Acceptance:**
- `cargo test -p camel-cli openapi::` passes (all new + existing).
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- `cargo fmt --check` clean.
- `rg -n "interpolate|read_to_string" crates/camel-cli/src/commands/openapi.rs` returns no scanner/read logic (all interpolation lives in camel-dsl).

- [x] 1.2

## docs

### Task 1.3: document default-only resolution on the openapi surface

**Files:**
- `docs/src/cli/openapi-plugin.md` (modified)
- `docs/src/getting-started/cli.md` (modified)

**Steps:**
1. In `docs/src/cli/openapi-plugin.md`: append the paragraph at the END of the `### How generation works` section (do not touch `### Input file` or `## camel plugin new`): `${env:}` placeholders in `rest:` blocks resolve default-only at generate time — string-typed positions take the concrete default in the emitted document; integer/boolean-typed positions with a placeholder fail generation (tree-walk canon parity with boot/lint/LEAN); a token without default fails naming the variable; `$${env:X}` stays literal; the process environment is never read.
2. In `docs/src/getting-started/cli.md`: add the same information as one or two sentences in the BODY of the `## camel openapi` section (leave the command table row at :14 untouched).
3. Keep both additions in English, no new examples with untested output shapes.

**Tests:** (non-Rust — verbatim presence checks)
- `rg -n "default-only" docs/src/cli/openapi-plugin.md` returns ≥1 hit inside the new paragraph.
- `rg -n "default-only|never read|not read" docs/src/getting-started/cli.md` returns ≥1 hit.

**Acceptance:**
- Both files contain the semantics paragraph; no other sections changed (`git diff --stat` shows only these two files with small additions).
- `cargo xtask lint-context-citations` exits 0 (docs change introduces no CONTEXT citation drift).

- [x] 1.3
