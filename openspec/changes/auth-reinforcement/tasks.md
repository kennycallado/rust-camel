# Tasks: auth-reinforcement

## Phase 1: Config honesty & fail-closed

### Task 1.1: Security/datasource placeholder resolution with uniform fail-closed semantics

**Files:**
- `crates/camel-config/src/config.rs` (modified)
- `crates/camel-config/src/properties.rs` (modified)

**Steps:**
1. Add `fn resolve_fail_closed(raw: &str, field: &str) -> Result<String, ConfigError>` in `config.rs`: resolves `{{env:VAR}}` / `{{env:VAR:default}}` via the existing `PropertiesResolver`; on unset var without default returns `ConfigError` with message `security placeholder unresolved: {field}: env var {VAR} not set`; if the default segment starts with `-` returns `ConfigError` `ambiguous default starting with '-' in {field}: use single-colon {{env:VAR:default}}`; if the resolved-or-raw value still contains `{{` or `${` returns `ConfigError` `unresolved placeholder marker in {field}`.
2. `resolve_placeholders` (config.rs:1068) currently returns `()` — convert it to `Result<(), ConfigError>` and thread the error through its callers so any fail-closed leaf aborts config load. Add a `security` branch walking every string leaf under `SecurityConfig` recursively (including nested Keycloak subtables `validation`, `jwks`, `introspection`, `uma`, and the `oidc`/`native` blocks) through `resolve_fail_closed`, and a `datasources` branch covering `db_url` and every string value inside `extra` (SurrealDB `password` included), same helper.
3. Non-credential single-colon defaults resolve normally (`{{env:X:fallback}}`, `X` unset → `fallback`); no warning.

**Tests:** (in `crates/camel-config/src/config.rs` test module; command `cargo test -p camel-config --lib placeholder`)
- `security_bearer_token_env_resolves`: Camel.toml text with `[security.native] bearer_token = "{{env:AUTH_TOKEN}}"`, env `AUTH_TOKEN=real-secret` set → `load_config` succeeds; `config.security.native.bearer_token == Some("real-secret")`.
- `security_unset_env_fails_closed`: same config, `AUTH_TOKEN` unset → `load_config` returns `ConfigError` whose message contains `AUTH_TOKEN` and `bearer_token`.
- `security_single_colon_default_resolves`: `bearer_token = "{{env:AUTH_TOKEN:fallback-secret}}"`, unset → resolves to `fallback-secret`, no error.
- `dash_default_rejected_on_any_covered_leaf`: `bearer_token = "{{env:X:-changeme}}"` (credential leaf), then `db_url = "{{env:X:-url}}"` (datasource leaf), each with `X` unset → both configs fail with `ConfigError` mentioning `-`.
- `noncredential_security_leaf_resolves`: `[security.keycloak] realm = "{{env:KC_REALM:main}}"`, `KC_REALM` unset → realm resolves to `main`.
- `datasource_leaves_resolve`: `[datasources.main] db_url = "{{env:DB_URL}}"` and SurrealDB-style `extra.password = "{{env:SURREAL_PASS}}"`, both set → both resolve to env values.
- `surviving_marker_rejected`: `bearer_token = "{{env:"` (malformed, survives resolution) → `ConfigError` with `marker`.

**Acceptance:**
- `cargo test -p camel-config --lib placeholder` passes (7 new tests).
- `cargo clippy -p camel-config -- -D warnings` exits 0.

- [x] 1.1

### Task 1.2: Authenticator boundary guard against marker secrets

**Files:**
- `crates/services/camel-auth/src/native_auth.rs` (modified)
- `crates/camel-cli/src/lib.rs` (modified)

**Steps:**
1. Add `pub fn ensure_no_placeholder_markers(secret: &str) -> Result<(), CamelError>` in `native_auth.rs`: returns `Err(CamelError::Config("credential contains unresolved placeholder marker: {{ or ${".to_string()))` when `secret.contains("{{") || secret.contains("${")`; `Ok(())` otherwise. (`CamelError::Config` takes an owned `String`; `Plaintext` values wrap `Zeroizing<String>` — compare via `secret_value.as_str()`.)
2. Call it at the top of the credential loop in `NativeCredentialStore::try_new` on the resolved secret value. Lookup logic untouched.
3. In `camel-cli/src/lib.rs`, call `ensure_no_placeholder_markers` on `keycloak.client_secret` at the TOP of `keycloak_authenticator` (before any realm/JWKS construction, so the guard fires without network). The oidc guard lands in Task 1.3 with the oidc wiring itself (today oidc has no construction path).

**Tests:** (command `cargo test -p camel-auth --lib marker && cargo test -p camel-cli --lib keycloak_guard`)
- `store_rejects_marker_secret` — setup: `NativeCredentialStore::try_new` called with one `NativeCredential { secret: Plaintext { value: Zeroizing::new("{{env:X}}".to_string()) }, principal }`; action: call `try_new`; assert: `Err` whose message contains `marker`.
- `store_accepts_clean_secret` — setup: same with value `clean-secret-value`; action: `try_new` then `lookup("clean-secret-value")`; assert: `Ok`, lookup returns the principal.
- `keycloak_guard_fires_before_network` — setup: `KeycloakSecurityConfig` with `client_secret = "{{env:KC}}"` (owned `String`, not `Option`) and any `server_url`; action: call `keycloak_authenticator`; assert: `Err` containing `marker`, returned before any network I/O (guard is the first statement).

**Acceptance:**
- `cargo test -p camel-auth --lib marker` and `cargo test -p camel-cli --lib keycloak_guard` pass (3 new tests).
- Existing constant-time and redaction tests stay green: `cargo test -p camel-auth --lib` exits 0.
- `cargo clippy -p camel-auth -p camel-cli -- -D warnings` exits 0.

- [x] 1.2

### Task 1.3: OIDC-only wiring — real authenticator or explicit ConfigError

**Files:**
- `crates/camel-cli/src/lib.rs` (modified)
- `crates/camel-cli/Cargo.toml` (modified — add dev-dependency `jsonwebtoken` (workspace dep) for signing test JWTs)
- `crates/camel-cli/tests/fixtures/` (new — copy the RSA test key PEM pair from `crates/services/camel-auth/tests/fixtures/` used by camel-auth jwt.rs tests)

**Steps:**
1. Add `async fn oidc_authenticator(oidc: &OidcSecurityConfig, ssrf: &SsrfPolicy) -> Result<Arc<dyn TokenAuthenticator>, CamelError>` in `lib.rs`: first call `ensure_no_placeholder_markers` on `oidc.client_secret` when `Some` (guard before network, mirroring keycloak); error `ConfigError("security.oidc.jwks_uri is required".to_string())` when `jwks_uri` is `None`; construct `RemoteJwksProvider::new(jwks_uri, ssrf.clone())`, then PREFETCH: `provider.get_signing_keys().await` — both the construction error (SSRF rejection) and the prefetch error are mapped to `ConfigError` including the `jwks_uri` URL in the message.
2. Structure for testability (no TLS test harness): split the assembly into `fn oidc_validator(oidc: &OidcSecurityConfig, jwks: Arc<dyn JwksProvider>) -> LocalJwtValidator` — `LocalJwtValidator::new` already takes `Arc<dyn JwksProvider>` (jwt.rs:28-33) — using the same claim-path construction pattern as the keycloak path (subject `/sub`, roles `/realm_access/roles` plus `/resource_access/{oidc.client_id}/roles` when `client_id` is `Some`, scopes `/scope` — mirror `camel_component_keycloak::keycloak_claim_paths`). `oidc_authenticator` = guard + jwks_uri check + `RemoteJwksProvider` + prefetch + `Arc::new(oidc_validator(oidc, jwks))`.
3. In `resolve_authenticator`, replace the `Ok(None)` oidc branch with `oidc_authenticator(oidc, &SsrfPolicy::PublicHttpsOnly).await.map(Some)` — production path uses the default policy. No path returns `None` for a configured oidc block.
4. Note for tests: `OidcSecurityConfig` has 7 fields and no `Default` — construct every field explicitly.

**Tests:** (command `cargo test -p camel-cli --lib oidc`; tests inject an in-memory `JwksProvider` through the `oidc_validator` seam — a test impl returning a `Jwk` built from the fixture RSA key's PEM `n`/`e` (jwt.rs:47-52) — no network, no TLS; JWT signing uses the camel-cli dev-dep `jsonwebtoken` + the fixture PEMs, mirroring camel-auth jwt.rs:128-132)
- `oidc_missing_jwks_uri_fails_closed` — setup: `resolve_authenticator` with a fully-populated `OidcSecurityConfig` except `jwks_uri: None`; action: await the call; assert: `Err` message contains `security.oidc.jwks_uri` — this also proves the production branch TAKES the oidc path (no silent `Ok(None)`), which is the registration-level half of the positive scenario.
- `oidc_unreachable_jwks_fails_closed_at_startup` — setup: `jwks_uri = Some("https://127.0.0.1:1/certs")`; action: await `oidc_authenticator`; assert: `Err` `ConfigError` whose message contains the URL (fails at construction via the loopback SSRF guard, or at prefetch — either stage's error must carry the URL per step 1).
- `oidc_validator_authenticates_jwt` — setup: `oidc_validator` with the in-memory JWKS provider + a JWT signed by the fixture key for `oidc.issuer` with `sub` claim; action: `authenticate_bearer(jwt)`; assert: principal with the `sub` subject.
- `oidc_marker_secret_guard_fires_before_network` — setup: `oidc_authenticator` with `client_secret = Some("{{env:X}}")` and otherwise-valid config; action: await the call; assert: `Err` containing `marker` (guard is the first statement; no provider construction attempted).

**Acceptance:**
- `cargo test -p camel-cli --lib oidc` passes (4 new tests).
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 1.3

### Task 1.4: Honest error texts and loud native-principal synthesis

**Files:**
- `crates/camel-dsl/src/compile.rs` (modified)
- `crates/camel-cli/src/lib.rs` (modified)

**Steps:**
1. `compile.rs` `require_authenticator` error (line ~54): replace `requires a JWT authenticator (configure [security] in Camel.toml)` with `route requires an authenticator: configure one of [security.keycloak], [security.oidc], [security.native] in Camel.toml`.
2. `native_authenticator` in `lib.rs`: after building the `Principal` with empty `claims`/`audience`, emit `warn!("native principal '{}' synthesized with empty claims/audience", native.subject)`.

**Tests:** (command `cargo test -p camel-dsl --lib require_authenticator`)
- `require_authenticator_error_names_providers`: build a route with `security_policy: {roles: ["admin"]}` against an empty `SecurityCompileContext` → compile error message contains `[security.keycloak]`, `[security.oidc]`, `[security.native]`.
- Existing `require_authenticator` tests updated only if they assert the old string.

**Acceptance:**
- `cargo test -p camel-dsl --lib` exits 0.
- `cargo clippy -p camel-dsl -p camel-cli -- -D warnings` exits 0.

- [x] 1.4

### Task 1.5: Docs honesty — placeholder recipes, OIDC default, yaml-dsl credential_sources

**Files:**
- `docs/src/configuration/schema.md` (modified)
- `crates/camel-config/README.md` (modified)
- `docs/src/yaml-dsl/route-structure.md` (modified — the rc-fxfl target; this is where the yaml-dsl route `security_policy` section lives)

**Steps:**
1. schema.md security section: replace every "Prefer a placeholder" note on `bearer_token`/`api_key`/`client_secret` with "Resolves `{{env:VAR}}`; unset variable without default fails startup" style text; delete the Keycloak-specific `jwks_uri` default row under `[security.oidc]` and mark `jwks_uri` required.
2. Add a syntax-boundary note (schema.md + camel-config README near line 357): `{{env:VAR:default}}` single-colon is canonical in Camel.toml; the `{{env:VAR:-default}}` double-dash form is rejected with a startup error; route files use a different `${env:…}` system (link to DSL docs, no unification claims).
3. yaml-dsl docs: document `credential_sources` (the four source forms, load-time rejection of malformed entries, default `[authorization_header]`), closing rc-fxfl.

**Tests:** (no Rust tests; doc-level verification)
- `command`: `grep -rn "Prefer a placeholder" docs/src/configuration/schema.md crates/camel-config/README.md` → expected: zero hits.
- `command`: `grep -rc "credential_sources" docs/src/yaml-dsl/route-structure.md` → expected: ≥ 3 (documented, not absent).
- `command`: `cargo xtask lint-context-citations` exits 0.

**Acceptance:**
- The three grep/command checks above hold.
- No doc shows `{{env:VAR:-default}}` as a working recipe.

- [x] 1.5

## Phase 2: Multi-credential native + dead-surface removal

### Task 2.1: `[[security.native.credentials]]` config surface

**Files:**
- `crates/camel-config/src/config.rs` (modified)

**Steps:**
1. Add `pub struct NativeCredentialEntry { pub subject: String, pub secret_env: Option<String>, pub secret: Option<String>, #[serde(default)] pub roles: Vec<String>, #[serde(default)] pub scopes: Vec<String> }` with `#[serde(deny_unknown_fields)]`, `Clone/Deserialize/Serialize/PartialEq`, and a redacting `Debug` (`secret`/`secret_env` → `[REDACTED]`, per ADR-0051 pattern used by `NativeAuthConfig`).
2. Add `#[serde(default)] pub credentials: Vec<NativeCredentialEntry>` to `NativeAuthConfig` (after `clients`, which still exists until 2.3 deletes it); extend the existing `Debug for NativeAuthConfig` with `credentials` count only.
3. Add `pub fn validate_credentials(&self) -> Result<(), ConfigError>` on `NativeAuthConfig` — each entry has exactly one of `secret_env`/`secret` (error: `security.native.credentials[{i}] must set exactly one of secret_env or secret`), empty `subject` rejected (error names the index) — and call it from `load_config` in the same post-deserialize validation pass that runs the other `deny_unknown_fields`-adjacent checks (locate the existing validation call site for `SecurityConfig` in `load_config` and add the call beside it).

**Tests:** (command `cargo test -p camel-config --lib native_credentials`)
- `credentials_array_parses` — setup: TOML text with two `[[security.native.credentials]]` entries (one `secret_env`, one `secret`); action: `load_config`; assert: both entries present with roles/scopes intact.
- `entry_with_both_secrets_rejected` — setup: entry with `secret_env` AND `secret`; action: `load_config`; assert: `ConfigError` naming the index.
- `entry_with_no_secret_rejected` — setup: entry with neither; action: `load_config`; assert: `ConfigError` naming the index.
- `credentials_debug_redacts` — setup: parsed config holding a known secret value; action: `format!("{:?}", config.security.native)`; assert: output contains no secret value (count/`[REDACTED]` only).

**Acceptance:**
- `cargo test -p camel-config --lib native_credentials` passes (4 new tests).
- `cargo xtask lint-secrets` exits 0.
- `cargo clippy -p camel-config -- -D warnings` exits 0.

- [x] 2.1

### Task 2.2: CLI builds the multi-credential store with env secrets

**Files:**
- `crates/camel-cli/src/lib.rs` (modified)

**Steps:**
1. Rework `native_authenticator`: build a `Vec<NativeCredential>` from (a) each `native.credentials` entry — `NativeCredentialSecret::Env { name }` when `secret_env` is `Some`, `Plaintext { value }` when `secret` is `Some`, principal per entry with its own roles/scopes; (b) scalar `bearer_token` (existing behavior, single entry); (c) scalar `api_key` — same store entry keyed by its secret value, principal from `subject`/`roles`/`scopes`.
2. Drop the hard `bearer_token is required` error: startup with only `credentials`, only `api_key`, or only `bearer_token` all build the store; startup with an EMPTY store (no credentials of any kind AND `[security.native]` present) fails with `ConfigError("security.native configured without any credential: set bearer_token, api_key, or [[security.native.credentials]]")`.
3. `NativeCredentialStore::try_new` already fails on unset/empty env vars (existing) — surface that error unchanged.

**Tests:** (command `cargo test -p camel-cli --lib native_auth`)
- `multi_entry_store_builds`: `native_authenticator` over a `NativeAuthConfig` with two credential entries (subjects `ops` role `admin`, `svc` role `service`) → authenticator authenticates token-of-ops to principal with `admin`, token-of-svc to principal with `service` (drive via `authenticate_bearer`).
- `api_key_only_starts` — setup: config with only `api_key = "k-1"` + `subject`/`roles`; action: `native_authenticator`; assert: `Ok`, no `bearer_token` required (request-level enforcement of this config is the 2.4 `api_key_only_custom_header_enforces_e2e` test).
- `legacy_scalar_unchanged`: config with only `bearer_token` + `subject` + `roles` → single-entry store, `lookup(token)` finds principal (v0.29.0 behavior).
- `empty_native_config_fails`: `[security.native]` present with `subject` set but no credential anywhere → `ConfigError` containing `without any credential` (`subject` stays serde-required, so the test config includes it).
- `missing_secret_env_fails_closed`: config with `[[security.native.credentials]]` entry `secret_env = "AUTH_SVC_TOKEN"` and `AUTH_SVC_TOKEN` unset → `native_authenticator` returns `Err` whose message names `AUTH_SVC_TOKEN` (surfaced from `NativeCredentialStore::try_new`).

**Acceptance:**
- `cargo test -p camel-cli --lib native_auth` passes (5 new tests).
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 2.2

### Task 2.3: Delete dead issuer surface (config fields, modules, wrapper)

**Files:**
- `crates/camel-config/src/config.rs` (modified)
- `crates/services/camel-auth/src/native_issuer.rs` (deleted)
- `crates/services/camel-auth/src/native_client_store.rs` (deleted)
- `crates/services/camel-auth/src/native_jwks.rs` (deleted)
- `crates/services/camel-auth/src/lib.rs` (modified — remove module decls and re-exports)
- `crates/services/camel-auth/src/native_auth.rs` (modified — remove `ApiKeyAuthenticator`)
- `crates/components/camel-http/src/auth.rs` (deleted)
- `crates/components/camel-http/src/lib.rs` (modified — remove `pub mod auth;`)
- `crates/camel-test/tests/http_test.rs` (modified — re-point the `camel_component_http::auth::extract_bearer_token` import at line ~288 to `camel_auth::extract_bearer_token`; camel-test already depends on camel-auth)
- `crates/camel-config/README.md` (modified — remove the `token_issuer` recipe block near lines 355-365)
- `crates/camel-api/src/security_policy.rs` (modified — update the `CredentialSource::Header` doc comment at ~line 105 that references `ApiKeyAuthenticator` staying as programmatic API; it is being deleted)
- `crates/services/camel-auth/Cargo.toml` (modified — drop now-unused deps: `jsonwebtoken` encoding side, `pkcs1`, `pkcs8` if no other module uses them; keep deps other modules still use)

**Steps:**
1. Delete the three issuer modules and their `mod`/`pub use` lines in `camel-auth/src/lib.rs`.
2. Remove `ApiKeyAuthenticator` (struct + inherent methods + tests) from `native_auth.rs`; `NativeCredentialStore`, `StaticTokenAuthenticator`, `ensure_no_placeholder_markers` stay.
3. Delete `crates/components/camel-http/src/auth.rs` and its `pub mod auth;` declaration in `crates/components/camel-http/src/lib.rs`.
4. Re-point the camel-test import (Files list above) so `cargo build --workspace` stays green.
5. In `camel-config/src/config.rs`: remove `token_issuer`, `clients` fields from `NativeAuthConfig`; delete `NativeIssuerConfig`, `NativeM2mClientConfig` structs and their redacting Debug arms; `deny_unknown_fields` now rejects stale keys.
6. Rewrite the `crates/camel-config/README.md` security section to post-deletion reality (drop the `token_issuer` recipe; point m2m use at `[[security.native.credentials]]`).
7. Remove now-unused `Cargo.toml` deps in camel-auth; run `cargo build -p camel-auth -p camel-http -p camel-config -p camel-test` to confirm.
8. Workspace sweep: `grep -rn "NativeTokenIssuer\|M2mClientStore\|NativeJwksProvider\|ApiKeyAuthenticator\|token_issuer\|NativeM2mClientConfig" crates/ --include="*.rs"` → zero hits; `grep -rn "token_issuer" crates/ docs/` → zero hits.

**Tests:** (command `cargo test -p camel-config --lib stale_native`)
- `stale_token_issuer_rejected_loudly`: TOML with `[security.native.token_issuer]` → `load_config` fails with unknown-field error (serde `deny_unknown_fields`).
- `stale_clients_rejected_loudly`: TOML with `[[security.native.clients]]` → same class of failure.

**Acceptance:**
- `cargo test -p camel-config --lib stale_native` passes (2 new tests).
- The step-8 greps return zero hits.
- `cargo build --workspace` exits 0; `cargo test -p camel-auth --lib` exits 0 (issuer tests removed with modules); `cargo test -p camel-test --test http_test` exits 0.
- `cargo clippy -p camel-auth -p camel-api -p camel-config -p camel-test -- -D warnings` exits 0.

- [x] 2.3

### Task 2.4: Multi-credential E2E + schema.md rows rewritten to reality

**Files:**
- `crates/camel-test/tests/auth_multi_credential_test.rs` (new — camel-test owns the HTTP harness: mirror the `HttpTestServer` + context-startup pattern from `crates/camel-test/tests/http_test.rs`)
- `docs/src/configuration/schema.md` (modified)

**Steps:**
1. E2E test file in camel-test: load a Camel.toml (tempfile) with two `[[security.native.credentials]]` entries (`ops`/role `admin` via `secret_env`, `svc`/role `service` via plaintext `secret`), one route `from: http://localhost:0/secure` with `security_policy: {roles: ["admin"]}`; start the context per the http_test.rs pattern.
2. Assert: request with ops token → 200; request with svc token → 403; request with unknown token → 401.
3. Second E2E (`api_key_only_custom_header_enforces_e2e`): Camel.toml with `[security.native]` scalar `api_key` only, route with `security_policy: {roles: ["reader"], credential_sources: [{header: {name: X-API-Key}}]}`; assert: request carrying `X-API-Key: <key>` → 200; missing header → 401. This closes the api-key-only scenario's route-enforcement half.
4. schema.md: rewrite `[security.native]` section to the post-2.3 truth — table with `subject`, `issuer`, `bearer_token`, `api_key`, `roles`, `scopes`, `[[security.native.credentials]]` array (fields per `NativeCredentialEntry`); remove `token_issuer`/`clients` rows and their subsections.

**Tests:** (command `cargo test -p camel-test --test auth_multi_credential_test --features integration-tests`)
- `two_principals_enforce_roles_e2e` — setup: two-credential Camel.toml + admin-role route; action: three requests (ops token, svc token, unknown token); assert: 200 / 403 / 401 respectively.
- `api_key_only_custom_header_enforces_e2e` — setup: api_key-only Camel.toml + custom-header credential_sources route; action: request with and without `X-API-Key`; assert: 200 with valid key, 401 without header.

**Acceptance:**
- `cargo test -p camel-test --test auth_multi_credential_test --features integration-tests` passes (2 new tests).
- `grep -n "token_issuer\|m2m clients" docs/src/configuration/schema.md` → zero hits.
- `cargo clippy -p camel-test -- -D warnings` exits 0.

- [x] 2.4

## Phase 3: Named providers

### Task 3.1: `SecurityCompileContext` provider registry

**Files:**
- `crates/camel-dsl/src/model.rs` (modified)
- `crates/camel-dsl/src/lib.rs` (modified)

**Steps:**
1. Add `providers: HashMap<String, Arc<dyn TokenAuthenticator>>` to `SecurityCompileContext` (model.rs) beside the existing single-authenticator field (the first `new(authenticator, …)` argument — that legacy field stays).
2. Add `pub fn with_named_authenticator(mut self, name: &str, auth: Arc<dyn TokenAuthenticator>) -> Self` and `pub fn authenticator_for(&self, name: Option<&str>) -> Result<Option<Arc<dyn TokenAuthenticator>>, String>`: `name = None` → `Ok` of the sole provider's value when `providers.len() == 1`; `Ok` of the legacy single field when the map is empty (back-compat for pre-3.1 callers); `Err("multiple authenticators configured: <names>; route must declare security_policy.provider")` when `len() > 1`; `name = Some(n)` → value or `Err("unknown provider: {n}; available: <names>")`.
3. Existing `new`-based single-provider construction keeps today's behavior untouched — no existing method is removed or renamed.

**Tests:** (command `cargo test -p camel-dsl --lib provider_registry`)
- `sole_provider_resolves_without_name`: one `with_named_authenticator("native", …)` → `authenticator_for(None)` → `Ok(Some(_))`.
- `legacy_field_fallback_when_map_empty`: context built only via `new(auth, …)` (no named entries) → `authenticator_for(None)` → `Ok(Some(_))` from the legacy field.
- `multiple_providers_require_name`: two named providers → `authenticator_for(None)` → `Err` listing both names.
- `unknown_provider_errors`: `authenticator_for(Some("saml"))` → `Err` containing `saml` and the available names.

**Acceptance:**
- `cargo test -p camel-dsl --lib provider_registry` passes (4 new tests).
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.

- [x] 3.1

### Task 3.2: `security_policy.provider` field + load-time validation

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified — add `provider: Option<String>` to `SecurityPolicyAst` authenticating variants; `deny_unknown_fields` there means the key is rejected at parse until added)
- `crates/camel-dsl/src/yaml.rs` (modified — parse `provider` on the authenticating variants; reject `provider` on `ref`/`wasm`/`permission` variants, mirroring the existing `credential_sources` variant rule)
- `crates/camel-dsl/src/model.rs` (modified — carry `provider: Option<String>` on `DeclarativeSecurityPolicy` Roles/Scopes variants)
- `crates/camel-dsl/src/compile.rs` (modified — resolve effective authenticator via `authenticator_for`)

**Steps:**
1. Add `#[serde(default, skip_serializing_if = "Option::is_none")] pub provider: Option<String>` to the authenticating `SecurityPolicyAst` variants (same place `credential_sources` lives in route_ast.rs ~line 173).
2. In `yaml.rs`, parse and forward `provider`; where `credential_sources` is rejected on non-authenticating variants, apply the identical rejection to `provider`.
3. Carry `provider` onto the `DeclarativeSecurityPolicy` Roles/Scopes variants in `model.rs`.
4. In `compile_security_policy` (compile.rs), resolve the effective authenticator via `ctx.authenticator_for(policy.provider.as_deref())`; on `Err(e)` fail route load with the error text; on `Ok(None)` fall through to the existing `require_authenticator` failure path (message from 1.4).

**Tests:** (command `cargo test -p camel-dsl --lib provider_validation`)
- `ambiguous_provider_fails_at_load`: context with two named providers; route with `security_policy: {roles: ["admin"]}` (no `provider`) → compile error naming both providers and the missing `provider` key.
- `unknown_provider_fails_at_load`: route with `provider: "saml"` → compile error containing `saml` and available names.
- `named_provider_compiles`: two providers; route with `provider: "native"` → compiles; policy wired to the native authenticator.
- `provider_on_non_authenticating_variant_rejected`: `security_policy: {ref: "…", provider: "native"}` → load error naming the policy block.

**Acceptance:**
- `cargo test -p camel-dsl --lib provider_validation` passes (4 new tests).
- `cargo test -p camel-api --lib` exits 0.
- `cargo clippy -p camel-dsl -- -D warnings` exits 0 (camel-api untouched).

- [x] 3.2

### Task 3.3: CLI registers every configured provider (XOR removed)

**Files:**
- `crates/camel-cli/src/lib.rs` (modified)

**Steps:**
1. Rework `resolve_authenticator` into `resolve_authenticators(security: &SecurityConfig) -> Result<Vec<(String, Arc<dyn TokenAuthenticator>)>, CamelError>`: builds and collects `("keycloak", _)`, `("oidc", _)`, `("native", _)` for each configured block — no count check, no XOR error. Errors from one provider abort the whole resolution (no partial registration).
2. Split registration out as `fn register_providers(ctx: SecurityCompileContext, providers: Vec<(String, Arc<dyn TokenAuthenticator>)>) -> SecurityCompileContext` — keeps `SecurityCompileContext::new(first, None)` for the first provider then chains `.with_named_authenticator(&name, auth)` for every entry (borrow: `with_named_authenticator` takes `name: &str` per Task 3.1); when exactly one provider exists, the legacy single field is the constructor argument (pre-3.2 consumers keep working). Apply `register_providers` in BOTH cfg-gated copies of `build_security_compile_context_from_config` (wasm and non-wasm paths, lib.rs ~295 and ~335).
3. Empty vec (no provider configured) behaves exactly as today's `None` case (anonymous routes allowed; protected routes fail with the 1.4 message).
4. Update the existing CLI test `multiple_auth_providers_returns_config_error` (lib.rs tests module ~421): it asserts the XOR error being removed — replace it with `native_only_resolves_single_provider` asserting `resolve_authenticators` over a native-only config returns a vec of len 1 with the `native` name.

**Tests:** (command `cargo test -p camel-cli --lib multi_provider`)
- `native_only_registers_named` — setup: config with `[security.native]` (one credential); action: `build_security_compile_context_from_config`; assert: `authenticator_for(Some("native"))` is `Ok(Some(_))`.
- `oidc_error_propagates` — setup: config with a broken `[security.oidc]` (unreachable jwks_uri) alongside a valid `[security.native]`; action: `resolve_authenticators`; assert: the oidc `Err` is returned (no partial registration).
- `sole_provider_back_compat` — setup: native-only config; action: build the context; assert: the legacy single-authenticator accessor is `Some`.
- `register_providers_registers_both` — setup: two in-memory authenticators (native store + `oidc_validator` with the in-memory JWKS provider from 1.3); action: `register_providers` then `authenticator_for` for each name; assert: both resolve, `authenticator_for(None)` errors naming both.
- `mixed_providers_route_selection_e2e` — setup: `SecurityCompileContext` built via `register_providers` with the two providers above; two routes `provider: native` and `provider: oidc`; action: native token against route A, JWT against route B, native token against route B; assert: 200 / 200 / 401. Request-level, closing the "Mixed providers with explicit selection" scenario.

**Acceptance:**
- `cargo test -p camel-cli --lib multi_provider` passes (5 new tests, 1 updated).
- `cargo test -p camel-cli --lib native_auth` (from 2.2) still passes — single-provider behavior unchanged.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 3.3

## Phase 4: gRPC credential sources

### Task 4.1: gRPC honors declared credential_sources; uncarryable sources rejected at load

**Files:**
- `crates/components/camel-component-grpc/src/server.rs` (modified)
- `crates/components/camel-component-grpc/src/consumer.rs` (modified — thread `Vec<CredentialSource>` from the consumer's `SecurityContext` into the server's per-route registration)

**Steps:**
1. Thread the sources: the consumer's `security_ctx.credential_sources` (consumer.rs ~266; `SecurityContext` per camel-component-api consumer.rs:318, wired by camel-core route_controller_trait.rs:163) is passed at registration into the per-route server entry tuple `(sender, mode, authenticator_opt)` at server.rs ~486 — add the `Vec<CredentialSource>` beside `authenticator_opt`, and extend `extract_principal(authenticator, metadata)` (server.rs ~430-433) to `extract_principal(authenticator, metadata, sources: &[CredentialSource])`.
2. Add `fn metadata_to_header_map(metadata: &tonic::MetadataMap) -> http::HeaderMap` in `server.rs`: map ASCII metadata keys to header names (lowercase), binary keys (`-bin` suffix) skipped.
3. Replace the hardcoded `token.strip_prefix("Bearer ")` extraction at server.rs:~438: build `http::Uri::from_static("/")` (synthetic — query params are load-rejected in step 4, so no query information is lost) and call `camel_auth::extract_token_multi(&header_map, &uri, sources)` — the shared multi-source extraction helper (signature `(headers, uri, sources)` per credential_source.rs:84, currently exercised by camel-ws); the `authorization` metadata key satisfies the default `[AuthorizationHeader]`.
4. Load-time validation (where the gRPC consumer validates its route config): `credential_sources` entries of form `query_param` or `cookie` → route load fails with `ConfigError` `grpc routes cannot carry {source_kind} credential sources; supported: authorization_header, header`. Default (no sources) unchanged.

**Tests:** (command `cargo test -p camel-component-grpc --lib credential_sources`)
- `grpc_credential_sources_custom_header_authenticates`: route with `credential_sources: [{header: {name: "x-api-key"}}]`, native store holding the key, request carrying `x-api-key` metadata (no authorization) → authenticates (mock/service-test harness per existing grpc consumer tests).
- `grpc_credential_sources_default_bearer_unchanged`: no declared sources, `authorization: Bearer <token>` metadata → authenticates as before.
- `grpc_credential_sources_uncarryable_rejected_at_load`: route declaring `{query_param: {param: "ticket"}}` and one declaring `{cookie: {name: "session"}}` → each fails load with the error naming the source kind and `grpc`.

**Acceptance:**
- `cargo test -p camel-component-grpc --lib credential_sources` passes (3 new tests).
- `cargo clippy -p camel-component-grpc -- -D warnings` exits 0.
- Canonical HTTP/DSL credential-source behavior untouched — regression ownership for the seven canonical scenarios in the MODIFIED requirement: `cargo test -p camel-dsl --lib` (parse/load-time rules: default header-only, malformed/empty/invalid-name rejection, non-authenticating-variant rejection) and `cargo test -p camel-test --test http_test` (request-level extraction incl. custom-header API key) both exit 0.
- WS parity suite stays green: `cargo test -p camel-component-ws --lib` exits 0.
