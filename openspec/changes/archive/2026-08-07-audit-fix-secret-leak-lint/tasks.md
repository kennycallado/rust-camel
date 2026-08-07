# Tasks: audit-fix-secret-leak-lint

## scripts/xtask

### Task 1.1: Implement credential derive lint core

**Files:**
- `scripts/xtask/Cargo.toml` (modified)
- `scripts/xtask/src/main.rs` (modified)

**Steps:**
1. Add `syn = { workspace = true, features = ["full"] }` to `scripts/xtask/Cargo.toml` under `[dependencies]`.
2. Add a `Classification` enum to `scripts/xtask/src/main.rs` with variants `ManualRedaction`, `RedactingWrapper`, `ProtocolDto`.
3. Add a `fn parse_classification(attrs: &[syn::Attribute]) -> Result<Option<Classification>, String>` that scans doc comments for the pattern `ADR-0051 credential boundary: <value>`, validates the value against the closed vocabulary (`manual-redaction`, `redacting-wrapper`, `protocol-dto`), and returns the classification or an error for unknown/malformed/duplicate values.
4. Add a `fn has_zeroizing_field(item: &syn::Item) -> bool` that checks whether a struct or enum has any field whose type path contains `Zeroizing` (matching both `Zeroizing<T>` and `zeroize::Zeroizing<T>`).
5. Add a `fn extract_derive_names(attrs: &[syn::Attribute]) -> Vec<String>` that extracts trait names from `#[derive(...)]` attributes, handling multiline derives.
6. Add a `fn lint_credential_derives_src(src: &str, file_path: &str) -> Result<Vec<SecretViolation>, String>` that parses source text with `syn::parse_file`, iterates items, and applies the classification rules:
   - For each struct/enum: parse classification from doc attrs. If classification is `Some(cls)`, check derive consistency (Debug/Serialize rules per spec table). If classification is `None` but the type has a `Zeroizing` field, report `credential-derive: Zeroizing field requires manual-redaction classification`. If classification parse returns an error (unknown/malformed/duplicate), report the error as a violation.
   - On `syn::parse_file` failure, return `Err` with the parse error message.
7. Add a `pub fn lint_credential_derives(workspace_root: &Path) -> Result<Vec<SecretViolation>, String>` that walks `crates/**/src/**/*.rs` using `walkdir`, calls `lint_credential_derives_src` per file, and merges violations. On any parse failure, return `Err` (hard-fail). Exclude files under `#[cfg(test)]` modules by skipping any item whose `attrs` contain `cfg(test)` or whose enclosing module is a `#[cfg(test)] mod tests` block (match the existing `is_test_file` helper convention used by `lint_unwrap`).

**Tests:**
- `test_manual_redaction_debug_violation`: source with struct annotated `/// ADR-0051 credential boundary: manual-redaction` and `#[derive(Debug)]` → `lint_credential_derives_src` returns exactly 1 violation with rule containing `manual-redaction forbids Debug` → expected FAIL before implementation, PASS after.
- `test_manual_redaction_serialize_violation`: same struct with `#[derive(Serialize)]` → 1 violation with `manual-redaction forbids Serialize`.
- `test_manual_redaction_clean`: struct with annotation and no Debug/Serialize derives → 0 violations.
- `test_manual_redaction_with_manual_impl_debug`: struct annotated `manual-redaction` with `impl Debug for Foo` (manual impl, no derive) and no derives → 0 violations. This is the actual shape of all 12 real targets.
- `test_redacting_wrapper_debug_ok`: struct annotated `redacting-wrapper` with `#[derive(Debug)]` → 0 violations.
- `test_redacting_wrapper_serialize_violation`: struct annotated `redacting-wrapper` with `#[derive(Serialize)]` → 1 violation.
- `test_protocol_dto_serialize_ok`: struct annotated `protocol-dto` with `#[derive(Serialize)]` → 0 violations.
- `test_protocol_dto_debug_violation`: struct annotated `protocol-dto` with `#[derive(Debug)]` → 1 violation.
- `test_zeroizing_without_classification`: struct with field `value: Zeroizing<String>`, no annotation → 1 violation with `Zeroizing field requires manual-redaction classification`.
- `test_qualified_zeroizing_without_classification`: struct with field `value: zeroize::Zeroizing<String>`, no annotation → 1 violation.
- `test_zeroizing_with_classification`: struct with `Zeroizing<String>` field annotated `manual-redaction`, no Debug/Serialize derives → 0 violations.
- `test_unannotated_no_zeroizing`: struct with field `path: String`, no annotation → 0 violations.
- `test_credential_suggesting_name_no_violation`: struct with field `client_key_path: String`, no annotation → 0 violations.
- `test_multiline_derive`: struct annotated `manual-redaction` with `#[derive(\n    Debug,\n    Clone,\n)]` → 1 violation for Debug.
- `test_unknown_classification`: struct annotated `/// ADR-0051 credential boundary: unknown-value` → 1 violation with `unknown classification`.
- `test_malformed_classification`: struct annotated `/// ADR-0051 credential boundary:` with no value → 1 violation with `malformed classification`.
- `test_conflicting_duplicate`: struct with two `/// ADR-0051 credential boundary:` doc comments with different values → 1 violation with `conflicting duplicate classifications`.
- `test_parse_failure_returns_error`: source string `struct Broken {` (invalid syntax) → `lint_credential_derives_src` returns `Err`.
- `test_violation_includes_file_and_line`: source with struct annotated `manual-redaction` and `#[derive(Debug)]` at a known line → returned violation's `.file` equals the file_path argument and `.line` matches the struct's line number (verify span extraction works).
- `test_violations_present_exit_nonzero`: temp workspace with one credential-derive violation → `lint_credential_derives` returns `Ok` with 1 violation (caller exits non-zero per exit-code requirement).

**Acceptance:**
- `cargo test -p xtask lint_credential_derives` exits 0 (all tests pass — note: xtask is a bin crate, no `--lib` flag).
- `cargo clippy -p xtask -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.1

### Task 1.2: Integrate credential derive lint into lint-secrets

**Files:**
- `scripts/xtask/src/main.rs` (modified)

**Steps:**
1. Inside the `lint_secrets` function (around line 2241), after the existing regex scanner collects sink violations, add a call to `lint_credential_derives(workspace_root)`.
2. Merge the two violation vectors inside `lint_secrets`: existing sink-pattern violations + new credential-derive violations. If `lint_credential_derives` returns `Err`, propagate it as `Err` from `lint_secrets` (hard-fail).
3. The `Commands::LintSecrets` match arm already calls `lint_secrets` and handles `Err` — no change needed there (it already prints and exits non-zero on error).

**Tests:**
- `test_lint_secrets_combines_sink_and_derive`: create a temp workspace with one file containing a format!/password sink violation and one file with a manual-redaction struct deriving Debug → `lint_secrets(&ws)` returns exactly 2 violations.
- `test_lint_secrets_parse_failure_returns_err`: create a temp workspace with a syntactically invalid .rs file → `lint_secrets(&ws)` returns `Err`.

**Acceptance:**
- `cargo test -p xtask lint_secrets` exits 0 (existing tests still pass + new tests pass).
- `cargo clippy -p xtask -- -D warnings` exits 0.

- [x] 1.2

## Crate annotations

### Task 2.1: Annotate credential-bearing types with ADR-0051 classifications

**Files:**
- `crates/services/camel-auth/src/native_auth.rs` (modified)
- `crates/services/camel-auth/src/native_client_store.rs` (modified)
- `crates/services/camel-auth/src/introspection.rs` (modified)
- `crates/services/camel-auth/src/native_issuer.rs` (modified)
- `crates/services/camel-auth/src/oauth2.rs` (modified)
- `crates/components/camel-component-wasm/src/state_store.rs` (modified)
- `crates/services/camel-otel/src/config.rs` (modified)
- `crates/services/camel-bridge/src/config.rs` (modified)
- `crates/components/camel-kafka/src/config.rs` (modified)
- `crates/components/camel-kafka/src/broker_config.rs` (modified)

**Steps:**
1. Add `/// ADR-0051 credential boundary: manual-redaction` as the last doc comment line before each of these type definitions:
   - `native_auth.rs`: `NativeCredentialSecret` (line ~14), `ResolvedCredential` (line ~20)
   - `native_client_store.rs`: `M2mClientSecret` (line ~33), `ResolvedM2mClient` (line ~38)
   - `introspection.rs`: `CachingTokenIntrospector` (line ~68)
   - `native_issuer.rs`: `TokenResponse` (line ~122). Note: this type carries `#[non_exhaustive]`; the doc annotation must sit above all attributes (above `#[derive(...)]` and `#[non_exhaustive]`).
   - `oauth2.rs`: `TokenResponse` (line ~30), `CachedToken` (line ~48), `ClientCredentialsProvider` (line ~70)
   - `state_store.rs`: `StateStore`
   - `config.rs` (otel): `OtelConfig`
   - `config.rs` (bridge): `BridgeProcessConfig`
2. Add `/// ADR-0051 credential boundary: redacting-wrapper` before:
   - `config.rs` (kafka): `KafkaConfig`
   - `broker_config.rs` (kafka): `KafkaBrokerConfig` (defined in `broker_config.rs`, NOT `config.rs`)
3. Verify that no annotated type derives Debug or Serialize in a way that violates its classification. All `manual-redaction` types must have neither Debug nor Serialize derives. The `redacting-wrapper` types (KafkaConfig, KafkaBrokerConfig) may keep Debug but must not have Serialize.

**Tests:**
- `cargo xtask lint-secrets` on the full workspace exits 0 with `lint-secrets: OK` (proves all 14 annotations are consistent).

**Acceptance:**
- `cargo xtask lint-secrets` exits 0.
- `cargo fmt --check --all` exits 0.
- No compilation errors from the doc annotations (they are ordinary comments).

- [x] 2.1

## Documentation

### Task 3.1: Amend ADR-0051 § Enforcement

**Files:**
- `docs/adr/0051-credential-redaction-at-diagnostic-boundaries.md` (modified)

**Steps:**
1. Replace the § Enforcement section content. Remove the deferral paragraph ("Do not add a derive-name lint yet...") and the revisit condition ("Revisit mechanical enforcement at the T2 audit sweep...").
2. Replace with: the `lint-secrets` xtask now performs AST-based derive inspection. Types annotated with `/// ADR-0051 credential boundary: <classification>` must comply with derive rules for their classification. `Zeroizing<T>` fields trigger auto-detection requiring `manual-redaction`. Unknown/malformed classifications are violations. Parse failures hard-fail.
3. Keep the first paragraph about code review and crate-local regression tests.

**Tests:**
- Manual review: the § Enforcement section describes the implemented lint, not a deferral.

**Acceptance:**
- No `TBD`, `TODO`, or deferral language remains in § Enforcement.
- `cargo xtask lint-secrets` exits 0 (ADR text contains `password` but is in `docs/`, not `src/`, so the sink scanner does not flag it).

- [x] 3.1
