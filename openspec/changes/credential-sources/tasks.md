# Tasks: credential-sources

## Phase 1: Contract + extraction refactor (no behavior change)

### camel-api / camel-auth

#### Task 1.1: Move `CredentialSource` to camel-api and thread it through the policy contract

**Files:**
- `crates/camel-api/src/security_policy.rs` (modified)
- `crates/services/camel-auth/src/credential_source.rs` (modified)
- `crates/services/camel-auth/src/lib.rs` (modified)
- `crates/services/camel-auth/src/built_in.rs` (modified)
- `crates/camel-dsl/src/compile.rs` (modified — `RolePolicy::new` / `ScopePolicy::new` call sites at lines ~77 and ~94)
- `examples/security-keycloak/src/main.rs` (modified — `RolePolicy::new` call sites at lines ~150 and ~207)

**Steps:**
1. Move the `CredentialSource` enum (variants `AuthorizationHeader`, `QueryParam { param }`, `Cookie { name }`, plus `variant_name()` and the redacting `Debug` impl) from `crates/services/camel-auth/src/credential_source.rs` to `crates/camel-api/src/security_policy.rs`, exported as `camel_api::security_policy::CredentialSource`. Keep `#[derive(Clone, PartialEq, Eq)]` and the `// allow-secret` Debug discipline.
2. In `crates/services/camel-auth/src/credential_source.rs`, replace the enum definition with `pub use camel_api::security_policy::CredentialSource;` so every existing `camel_auth::CredentialSource` path (e.g. `crates/components/camel-ws/src/lib.rs`) keeps compiling unchanged.
3. In `crates/camel-api/src/security_policy.rs`, add to `SecurityPolicyConfig` (line ~80) a `pub credential_sources: Vec<CredentialSource>` field. `SecurityPolicyConfig` has a required `policy: Arc<dyn SecurityPolicy>` field, so `Default` is not derivable: every existing constructor (`new`, and any other that builds the struct) initializes `credential_sources` to `vec![CredentialSource::AuthorizationHeader]` (fail-closed, ADR-0033). Add a consuming builder `pub fn with_credential_sources(mut self, sources: Vec<CredentialSource>) -> Self`.
4. In `crates/services/camel-auth/src/built_in.rs`, extend `RolePolicy` and `ScopePolicy` with a `credential_sources: Vec<CredentialSource>` field; their `new()` constructors gain a `credential_sources: Vec<CredentialSource>` parameter (last position); add `pub fn credential_sources(&self) -> &[CredentialSource]` accessors on both. Update every `RolePolicy::new` / `ScopePolicy::new` call site (grep to enumerate; known: `crates/camel-dsl/src/compile.rs:77,94`, tests in `native_auth.rs` and `built_in.rs`, `examples/security-keycloak/src/main.rs:150,207`) passing `vec![CredentialSource::AuthorizationHeader]` (behavior unchanged this phase).
5. Run `cargo clippy -p camel-api -p camel-auth -- -D warnings` and `cargo check -p security-keycloak`; fix all findings.

**Tests:** (executable spec)
- `credential_source_reexport_path_stable`: existing `camel_auth::CredentialSource` path resolves → `cargo test -p camel-auth --lib` compiles and passes → expected: pass before and after (pure move).
- `security_policy_config_new_is_header_only`: construct `SecurityPolicyConfig::new(arc_test_policy)` → assert `config.credential_sources == vec![CredentialSource::AuthorizationHeader]` → `cargo test -p camel-api --lib` → expected: pass after implementation.
- `role_policy_constructor_accepts_sources`: build `RolePolicy::new(vec!["r".into()], true, false, arc_authenticator, vec![CredentialSource::Cookie { name: "s".into() }])` → `policy.credential_sources()` returns the cookie list → `cargo test -p camel-auth --lib` → expected: pass after implementation.

**Acceptance:**
- `cargo clippy -p camel-api -p camel-auth -- -D warnings` exits 0; `cargo check -p security-keycloak` exits 0.
- `cargo test -p camel-api --lib` and `cargo test -p camel-auth --lib` pass.
- `rg -n 'enum CredentialSource' crates/` returns exactly one definition (camel-api).
- `cargo test --workspace --lib` passes (no behavior change anywhere).

- [x] 1.1

#### Task 1.2: Route `authenticate()` through multi-source extraction over the Exchange

**Files:**
- `crates/services/camel-auth/src/credential_source.rs` (modified)
- `crates/services/camel-auth/src/built_in.rs` (modified)
- `crates/camel-api/src/security_policy.rs` (modified — new public constant)
- `docs/adr/0059-auth-extraction-path-divergence.md` (new — draft skeleton)

**Steps:**
1. Add `pub const CAMEL_HTTP_QUERY_HEADER: &str = "CamelHttpQuery";` to `crates/camel-api/src/security_policy.rs` — camel-http stores the raw query string as an input header under this exact name (`crates/components/camel-http/src/lib.rs:1435-1436`), and camel-auth cannot depend on camel-http, so the contract constant lives in camel-api.
2. Add `pub fn extract_token_from_exchange(exchange: &Exchange, sources: &[CredentialSource]) -> Option<ExtractedToken>` in `credential_source.rs`, where `ExtractedToken` is the existing return item type of `extract_token_multi` (`credential_source.rs:41`, the struct with `.token` / `.source` fields). Implementation: build an `http::HeaderMap` view from `exchange.input` headers (each camel-api header appended as `http::header::HeaderName` + value; skip any name or value rejected by `http::HeaderName::try_from` / `http::HeaderValue::from_str` — this covers non-ASCII, control characters, and non-string `serde_json` values, since `HeaderValue::from_str` rejects everything outside visible-ASCII-plus-space — the source is treated as absent, never fatal, ADR-0032); build the query input by reading `exchange.input.header_ic(CAMEL_HTTP_QUERY_HEADER)` (a raw query string, `serde_json::Value::String`) and parsing it into pairs; a missing or malformed query header is treated as no query pairs. Cookie parsing stays inside the existing `extract_token_multi` cookie path.
3. Change the private `authenticate()` in `built_in.rs` (line ~25) to take `sources: &[CredentialSource]` and obtain the token via `extract_token_from_exchange(exchange, sources)` instead of the hardcoded `header_ic("authorization").strip_prefix("Bearer ")`. Preserve exactly: on `Some(token)` → `authenticator.authenticate_bearer` → store `PRINCIPAL_KEY` → return; on `None` → `trust_upstream_principal` branch unchanged (preloaded-principal fallback only when the flag is true; `Unauthenticated` otherwise).
4. Update the two callers (`RolePolicy::evaluate`, `ScopePolicy::evaluate`) to pass `&self.credential_sources`.
5. Create `docs/adr/0059-auth-extraction-path-divergence.md` as a draft skeleton (Status: draft; Context and Decision sections only — full write-up is Task 3.2): Context records the two auth paths (policy-owned extraction on the HTTP layer via `SecurityPolicyLayer`; component-owned on WS/gRPC via pre-authentication + preloaded principal); Decision records extraction standardized behind `extract_token_multi` with route-declared sources, and WS `roles`/`scopes` routes requiring explicit `trust_upstream_principal: true`.

**Tests:**
- `authenticate_default_equals_bearer_prefix_strip`: exchange with `Authorization: Bearer TOK`, policy sources `[AuthorizationHeader]` → `evaluate` grants exactly as the pre-change hardcoded path did (same principal) → `cargo test -p camel-auth --lib` → expected: pass after implementation, and the pre-change equivalent assertion (valid/invalid token grant/deny) still passes.
- `authenticate_header_source_reads_authorization`: sources `[AuthorizationHeader]`, header present → authenticates; header absent, no other source → `Err(Unauthenticated)` → `cargo test -p camel-auth --lib` → expected: pass.
- `cookie_parse_malformed_is_absent_not_error`: exchange input carries `Cookie: garbage-no-equals`, sources `[Cookie { name: "session" }]` → result is `Err(Unauthenticated)` (absent source), never a parse panic or `CamelError` other than `Unauthenticated` at the extraction layer → `cargo test -p camel-auth --lib` → expected: pass.
- `query_source_reads_camel_http_query_header`: exchange input carries `CamelHttpQuery: "token=TOK"` header, sources `[QueryParam { param: "token" }]`, store holds `TOK` → `evaluate` grants → `cargo test -p camel-auth --lib` → expected: pass (proves the header-based query path, mirroring how camel-http really stores it).
- `trust_false_preloaded_principal_unauthenticated`: exchange with `PRINCIPAL_KEY` property set and no credentials, policy `trust_upstream_principal=false` → `evaluate` returns `Err(Unauthenticated)` → `cargo test -p camel-auth --lib` → expected: pass.
- `trust_true_preloaded_principal_fallback`: same exchange, `trust_upstream_principal=true` → `evaluate` grants using the preloaded principal → `cargo test -p camel-auth --lib` → expected: pass.
- `prefix_credential_unauthenticated`: native store holds `SENTINEL_FULL_9kq2`; request presents `SENTINEL_FULL` in each source type (authorization header, cookie via exchange headers, query via the `CamelHttpQuery` header) → every case `Err(Unauthenticated)` through the shared store lookup → `cargo test -p camel-auth --lib` → expected: pass.

**Acceptance:**
- `cargo test -p camel-auth --lib` passes with all seven new tests.
- `cargo test --workspace --lib` passes (default Bearer behavior byte-identical).
- `cargo clippy -p camel-auth -p camel-api -- -D warnings` exits 0; `cargo fmt --check` clean.
- No new `unwrap()` (verifiable: `cargo xtask lint-unwrap` exits 0).
- `docs/adr/0059-auth-extraction-path-divergence.md` exists with Status/Context/Decision.

- [x] 1.2

## Phase 2: DSL + schema + HTTP redaction

### camel-dsl

#### Task 2.1: `credential_sources` DSL field, model threading, load-time validation, schema regen

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/model.rs` (modified — `DeclarativeSecurityPolicy` at line ~62)
- `crates/camel-dsl/src/yaml.rs` (modified — RouteDsl→Declarative conversion at lines ~344-465; this is production code)
- `crates/camel-dsl/src/compile.rs` (modified)
- `schemas/dsl/route-schema.json` (modified — regenerated)
- `crates/camel-lint/schema/route-schema.json` (modified — regenerated copy)

**Steps:**
1. In `route_ast.rs`, add `pub enum CredentialSourceDsl` with variants `AuthorizationHeader`, `QueryParam { param: String }`, `Cookie { name: String }`, serde-tagged to accept exactly the YAML forms `authorization_header` (string), `query_param: {param: token}`, `cookie: {name: session}` (field names as shown; values are the user-declared names). Derive the same `#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]` + `Deserialize` + `Debug` + `Clone` pattern used by neighbors.
2. Add `#[serde(default)] pub credential_sources: Option<Vec<CredentialSourceDsl>>` to `RouteDslSecurityPolicy` (after `permission`, keeping `deny_unknown_fields`).
3. In `model.rs`, add `credential_sources: Option<Vec<CredentialSourceDsl>>` to the `Roles` and `Scopes` variants of `DeclarativeSecurityPolicy`. `Ref`, `Wasm`, and `Permission` variants gain no field.
4. In `yaml.rs` conversion (lines ~344-465): copy the field onto `Roles`/`Scopes` when present, and add validation producing the file's existing conversion-error type, all errors naming `credential_sources`: (a) empty list; (b) `param`/`name` empty after trim; (c) `credential_sources` present while the block has neither `roles` nor `scopes` — this one check covers `ref`-only, `wasm`-only, and `permission`-only blocks, because the conversion sees the whole `RouteDslSecurityPolicy`. (Unknown source forms are rejected by serde before conversion — add the test anyway.)
5. In `compile.rs`, map `CredentialSourceDsl` → `camel_api::security_policy::CredentialSource` and uphold the two-carrier invariant: the SAME mapped list goes into the `RolePolicy::new` / `ScopePolicy::new` calls (lines ~77 and ~94) AND into `SecurityPolicyConfig` via `.with_credential_sources(list.clone())`. When the key is absent, both carriers receive `vec![CredentialSource::AuthorizationHeader]`.
6. Run `cargo xtask schema`; confirm `cargo xtask schema --check` exits 0.

**Tests:**
- `load_rejects_empty_source_list`: route YAML with `credential_sources: []` under `security_policy: {roles: [r]}` → conversion error message contains `credential_sources` → `cargo test -p camel-dsl` → expected: fail before validation exists, pass after.
- `load_rejects_empty_cookie_name`: `{cookie: {name: ""}}` → error names the entry → `cargo test -p camel-dsl` → expected: pass after.
- `load_rejects_empty_query_param`: `{query_param: {param: "  "}}` → error → `cargo test -p camel-dsl` → expected: pass after.
- `load_rejects_malformed_source_form`: `{cookie: {}}` (missing `name`) → parse error names the field → `cargo test -p camel-dsl` → expected: pass after.
- `load_rejects_unknown_source_form`: `{weird_source: {}}` → serde unknown-variant error naming the field → `cargo test -p camel-dsl` → expected: pass after.
- `load_rejects_wasm_only_with_sources`: block with only `wasm` + `credential_sources` → conversion error names the policy block → `cargo test -p camel-dsl` → expected: pass after.
- `load_rejects_ref_only_with_sources`: block with only `ref` + `credential_sources` → conversion error → `cargo test -p camel-dsl` → expected: pass after.
- `load_rejects_permission_only_with_sources`: block with only `permission` + `credential_sources` → conversion error → `cargo test -p camel-dsl` → expected: pass after.
- `compile_maps_sources_to_config`: route with `roles` + `credential_sources: [{cookie: {name: session}}]` → compiled `SecurityPolicyConfig.credential_sources == vec![Cookie { name: "session" }]` → `cargo test -p camel-dsl` → expected: pass after. (The policy-side carrier of the same list is proven behaviorally by Task 2.2's `cookie_source_authenticates_img_request` — the config assertion alone does not prove extraction works.)
- `absent_key_compiles_to_header_default`: route without the key → config default `[AuthorizationHeader]` → `cargo test -p camel-dsl` → expected: pass after.

**Acceptance:**
- `cargo test -p camel-dsl` passes with all ten new tests.
- `cargo xtask schema --check` exits 0.
- `cargo clippy -p camel-dsl --all-features -- -D warnings` exits 0.

- [x] 2.1

### camel-test

#### Task 2.2: HTTP integration — cookie-authenticated route, 401 mapping, precedence

**Files:**
- `crates/camel-test/tests/http_test.rs` (modified)

**Steps:**
1. Build the test harness — `http_test.rs` has NO existing security_policy route, so do not look for one to copy: use `crates/camel-test/tests/security_policy_test.rs` as the reference for how `SecurityPolicyConfig` is attached to a route programmatically, and `bean_integration_test.rs` as the reference for route loading. Construct the new fixture: a `from: http://` route (bind URI following the existing fixtures in the same file) with `security_policy: {roles: [tiles-reader], credential_sources: [{cookie: {name: session}}]}`, with the native store seeded with credential value `SENTINEL_CRED_1` (principal has role `tiles-reader`), seeded so that discovery's `SecurityCompileContext` → `require_authenticator` path (`crates/camel-dsl/src/compile.rs:48`) resolves `StaticTokenAuthenticator` from a native store containing one credential whose principal has role `tiles-reader` (seed via the `[security.native]` config section the CLI uses — see `crates/camel-cli/src/lib.rs:160-183` for the shape).
2. Write the tests below against that route using the HTTP client harness the file already uses.
3. Verify behavior comes from Phases 1–2.1 wiring: no camel-http production changes belong in this task; if a test cannot pass without a camel-http change, STOP the task and report back a message titled `wiring-gap` that names the failing test, the expected vs observed behavior, and the specific camel-http code path that would need the change — do not patch the component yourself.

**Tests:**
- `cookie_source_authenticates_img_request`: native store holds credential value `SENTINEL_CRED_1`; client GETs the route path with `Cookie: session=SENTINEL_CRED_1` and NO `Authorization` header → response is 200 (the `<img>` browser case) → `cargo test -p camel-test --test http_test --features integration-tests cookie_source_authenticates_img_request` → expected: pass after Phases 1–2.1.
- `cookie_miss_maps_401_not_500`: same route; client sends no cookie and no header → response status is 401, not 500 → `cargo test -p camel-test --test http_test --features integration-tests cookie_miss_maps_401_not_500` → expected: pass.
- `absent_key_rejects_cookie_token`: identical route but WITHOUT `credential_sources`; client sends the cookie → 401 (default stays header-only) → `cargo test -p camel-test --test http_test --features integration-tests absent_key_rejects_cookie_token` → expected: pass.
- `multi_source_first_match_wins`: route with `[{authorization_header}, {cookie: {name: session}}]`; store holds only `SENTINEL_CRED_1`; client sends `Authorization: Bearer SENTINEL_CRED_1` AND `Cookie: session=SENTINEL_WRONG_1` → 200 (header wins) → `cargo test -p camel-test --test http_test --features integration-tests multi_source_first_match_wins` → expected: pass.
- `fallback_to_second_source`: same route; client sends only `Cookie: session=SENTINEL_CRED_1` → 200 → `cargo test -p camel-test --test http_test --features integration-tests fallback_to_second_source` → expected: pass.
- `default_absent_key_bearer_identical`: route with `roles` and no `credential_sources`; client sends valid Bearer → 200; invalid Bearer → 401 (pre-change behavior) → `cargo test -p camel-test --test http_test --features integration-tests default_absent_key_bearer_identical` → expected: pass.
- `all_sources_miss_maps_401`: multi-source route; client sends neither header nor cookie → 401 → `cargo test -p camel-test --test http_test --features integration-tests all_sources_miss_maps_401` → expected: pass.

**Acceptance:**
- `cargo test -p camel-test --test http_test --features integration-tests` passes with all seven new tests.
- `cargo xtask lint-log-levels` exits 0.

- [x] 2.2

### camel-http

#### Task 2.3: Keep credential values out of the HTTP diagnostic output

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified — the auth-failure error path around `pipeline_error_to_reply` at line ~2315)

**Steps:**
1. Audit every tracing record and reply-body construction on the path a failed authentication takes: the `Unauthenticated` error → `pipeline_error_to_reply` mapping and any `warn`/`error`/`debug` log between the security layer's rejection and the reply. camel-http has NO request access log today — do not add one; the contract is redact-by-construction over what exists.
2. Verify the `Unauthenticated` error Display strings on that path contain no extracted credential value (they are generic today: `"invalid credential"`, `"no Bearer token and trust_upstream_principal is false"` — keep them that way when extraction error messages are added). Where any record renders the request URI, apply `camel_auth::redact_query_params` with the route's declared query-source parameter names (extend beyond the hardcoded `access_token`/`token` list WS uses).
3. The 401/403 reply body carries a generic reason only; never an extracted value.
4. Add tests with sentinel values; use `// allow-secret` comments where the linter requires (test fixtures only). Capture tracing output with a test subscriber spanning the failed-auth request.

**Tests:**
- `error_context_redacts_query_sentinel`: route with `{query_param: {param: token}}`; request `?token=SENTINEL_QRY_42` with an invalid value → response is 401, response body lacks `SENTINEL_QRY_42`, and every tracing record captured during request handling lacks `SENTINEL_QRY_42` → `cargo test -p camel-component-http error_context_redacts_query_sentinel` → expected: pass after.
- `error_context_redacts_cookie_sentinel`: route with cookie source `session`; request `Cookie: session=SENTINEL_CKY_7` (invalid) → 401, body and all captured records lack `SENTINEL_CKY_7` → `cargo test -p camel-component-http error_context_redacts_cookie_sentinel` → expected: pass after.
- `error_reply_no_credential_value`: invalid cookie token `SENTINEL_BAD_1` → response body AND captured error logs lack `SENTINEL_BAD_1`; status 401 → `cargo test -p camel-component-http error_reply_no_credential_value` → expected: pass after.

**Acceptance:**
- `cargo test -p camel-component-http` passes with the three new tests.
- `cargo xtask lint-secrets` exits 0.
- `cargo xtask lint-log-levels` exits 0.

- [x] 2.3

## Phase 3: WS activation + docs

### camel-core / camel-ws

#### Task 3.1: Activate `with_credential_sources` on the consumer SecurityContext

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified — lines ~162 and ~507)
- `crates/components/camel-ws/tests/credential_sources_test.rs` (new)

**Steps:**
1. At both `SecurityContext::from_arc` construction sites (lines ~162 and ~507), replace with the builder form that calls `.with_credential_sources(sp_config.credential_sources.clone())` so the consumer `SecurityContext` carries the route-declared list instead of the hardcoded default.
2. Create the new WS test file following the harness pattern of `crates/components/camel-ws/tests/accept_loop_metric_regression.rs` for spinning a WS endpoint with `path_policies`. Each test route uses `security_policy` with `roles`.
3. Write the three tests below. They assert the code-truth semantics documented in design.md: the grant flows through the preloaded-principal branch, so a WS `roles` route needs explicit `trust_upstream_principal: true`; the mechanism never sets the flag itself.

**Tests:**
- `ws_cookie_source_authenticates_with_explicit_trust`: WS route with `roles`, `trust_upstream_principal: true`, `credential_sources: [{cookie: {name: session}}]`, native store holding credential value `SENTINEL_CRED_1`; client upgrade request carries `Cookie: session=SENTINEL_CRED_1` and no `Authorization` header → upgrade succeeds (101) and the connection is authorized → `cargo test -p camel-component-ws --test credential_sources_test ws_cookie_source_authenticates_with_explicit_trust` → expected: pass after.
- `ws_no_flag_rejects_even_with_valid_token`: same route but WITHOUT `trust_upstream_principal`; client sends `Authorization: Bearer SENTINEL_CRED_1` → connection rejected unauthenticated (401/403 per current WS mapping) → `cargo test -p camel-component-ws --test credential_sources_test ws_no_flag_rejects_even_with_valid_token` → expected: pass (fail-closed preserved).
- `ws_no_credential_rejected_before_eval`: route with declared `credential_sources` and `trust_upstream_principal: true`; client presents no credential in any declared source → connection rejected with 401 before policy evaluation runs (the WS no-source rejection path), and the mechanism has not set the trust flag implicitly → `cargo test -p camel-component-ws --test credential_sources_test ws_no_credential_rejected_before_eval` → expected: pass.
- `ws_default_header_only_unchanged`: WS route with `roles` + `trust_upstream_principal: true` and NO `credential_sources`; valid Bearer header → 101; valid cookie only → rejected (default source list unchanged by the activation) → `cargo test -p camel-component-ws --test credential_sources_test ws_default_header_only_unchanged` → expected: pass.

**Acceptance:**
- `cargo test -p camel-component-ws` passes (new file + existing `accept_loop_metric_regression`).
- `cargo test --workspace --lib` passes.
- `rg -n 'from_arc' crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` shows no remaining default-only construction for policy-backed consumers.

- [x] 3.1

### docs

#### Task 3.2: Divergence ADR + CONTEXT updates + CSRF guidance

**Files:**
- `docs/adr/0059-auth-extraction-path-divergence.md` (modified — full write-up of the Task 1.2 draft skeleton)
- `crates/services/camel-auth/CONTEXT.md` (modified)
- `crates/components/camel-http/CONTEXT.md` (modified)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Write ADR-0059 in the house ADR format (read `docs/adr/0058-outcome-aware-segment-composition.md` for structure): Context (two auth paths: policy-owned extraction on the HTTP layer, component-owned on WS/gRPC), Decision (extraction standardized behind `extract_token_multi` with route-declared sources; the divergence stays — the layer calls extraction inside `authenticate()`, components call it before `policy.evaluate`; WS `roles`/`scopes` routes require explicit `trust_upstream_principal: true` and the spoof caveat does not apply there because the component gates evaluation on successful authentication), Consequences (first-match-wins precedence; gRPC fast-follow noted; `ref`/`wasm`/`permission` excluded for lack of capability metadata).
2. Add a "Credential sources" section to `crates/services/camel-auth/CONTEXT.md`: the enum, the constant-time store path shared by every source, and the trust-flag semantics.
3. Add to `crates/components/camel-http/CONTEXT.md`: the `credential_sources` route key, the redaction behavior, and the browser cookie guidance — SameSite=Lax+ and HttpOnly cookies are the operator's responsibility; cookie auth on state-changing verbs requires CSRF defense; GET-only tile services are the primary target.
4. Add a `CredentialSource` entry to the Key Terms / glossary section of `CONTEXT-MAP.md` citing ADR-0059 and ADR-0051.

**Tests:**
- Documentation task — no unit tests; the acceptance commands are the machine checks.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- `openspec validate credential-sources --type change` exits 0 (spec untouched by docs work).
- All four files exist and cite ADR-0032/0033/0051/0059 where the semantics depend on them.

- [x] 3.2

## Phase 4: API-key custom header source

### camel-auth

#### Task 4.1: `CredentialSource::Header { name }` variant and extraction

**Files:**
- `crates/camel-api/src/security_policy.rs` (modified — `Header` variant, `variant_name()`, redacting `Debug`)
- `crates/services/camel-auth/src/credential_source.rs` (modified — extraction match in `extract_token_multi`)

These are the only exhaustive `match` sites on `CredentialSource` (verified: `variant_name` and `Debug` at `credential_source.rs:18-32` move to camel-api in Task 1.1; the extraction match at `credential_source.rs:104-120` stays here; `camel-ws/src/lib.rs` uses non-exhaustive `matches!` on a single variant and `camel-component-api/src/consumer.rs` only constructs values — neither breaks). `cargo check --workspace --all-features` is the acceptance gate that catches any site this list missed.

**Steps:**
1. Add `Header { name: String }` to `camel_api::security_policy::CredentialSource`; extend `variant_name()` (`"Header"`) and the redacting `Debug` impl (name is not a secret; render `Header { name: .. }`).
2. Extend `extract_token_multi` to read the named header (case-insensitive lookup consistent with `header_ic`) as a source, in declared order with the other variants.
3. Update both exhaustive match sites listed under Files atomically.
4. Document on the variant: the extracted value flows into the same constant-time `NativeCredentialStore::lookup`; `ApiKeyAuthenticator` is superseded for YAML use (its programmatic API stays).

**Tests:**
- `header_source_authenticates_api_key`: store holds key `SENTINEL_KEY_1`; exchange input has `X-API-Key: SENTINEL_KEY_1`; policy sources `[Header { name: "x-api-key" }]` → `evaluate` grants the stored principal → `cargo test -p camel-auth header_source_authenticates_api_key` → expected: pass after.
- `header_source_miss_maps_401`: no such header, no other source → `Err(Unauthenticated)` → `cargo test -p camel-auth header_source_miss_maps_401` → expected: pass after.
- `header_lookup_case_insensitive`: header set as `X-API-KEY`, source declares `x-api-key` → authenticates → `cargo test -p camel-auth header_lookup_case_insensitive` → expected: pass after.

**Acceptance:**
- `cargo test -p camel-auth` passes with the three new tests.
- `cargo check --workspace --all-features` exits 0 (all exhaustive match sites updated atomically).
- `cargo test --workspace --lib` passes.
- `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings` exits 0.

- [x] 4.1

### camel-dsl / camel-test

#### Task 4.2: DSL `header` form, header-token validation, schema, integration + redaction test

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/yaml.rs` (modified — header-token validation at the conversion site)
- `crates/camel-dsl/src/compile.rs` (modified)
- `crates/camel-test/tests/http_test.rs` (modified)
- `schemas/dsl/route-schema.json` (modified — regenerated)
- `crates/camel-lint/schema/route-schema.json` (modified — regenerated copy)

**Steps:**
1. Add `Header { name: String }` variant to `CredentialSourceDsl` accepting YAML `{header: {name: X-API-Key}}`; map to the camel-api variant in `compile.rs`.
2. Add `fn valid_header_token(name: &str) -> bool` in `yaml.rs` implementing the RFC 9110 token charset (`tchar`: alphanumerics and `!#$%&'*+-.^_`|~`, at least one char, no whitespace/colons). Invoke it in the RouteDsl→Declarative conversion alongside the Task 2.1 validations (same error type, naming the entry): reject `credential_sources` entries whose header name fails it.
3. Run `cargo xtask schema`; confirm `--check` exits 0.
4. Add the integration + redaction tests below to `crates/camel-test/tests/http_test.rs` (route with `credential_sources: [{header: {name: X-API-Key}}]` + native store).

**Tests:**
- `load_rejects_invalid_header_token_name`: `{header: {name: "Bad Header:"}}` → compile error names the header source → `cargo test -p camel-dsl load_rejects_invalid_header_token_name` → expected: pass after.
- `header_source_authenticates_api_key_http`: client sends `X-API-Key: SENTINEL_CRED_1` and no `Authorization` → 200 → `cargo test -p camel-test --test http_test --features integration-tests header_source_authenticates_api_key_http` → expected: pass after.
- `header_source_miss_maps_401_http`: no header → 401 → `cargo test -p camel-test --test http_test --features integration-tests header_source_miss_maps_401_http` → expected: pass after.
- `error_context_redacts_custom_header_sentinel`: client sends `X-API-Key: SENTINEL_HDR_9` on a failing auth attempt; every tracing record captured during request handling and the 401 reply body lack `SENTINEL_HDR_9` → `cargo test -p camel-test --test http_test --features integration-tests error_context_redacts_custom_header_sentinel` → expected: pass after (extends Task 2.3's audit to the declared custom header).

**Acceptance:**
- `cargo test -p camel-dsl` passes; `cargo test -p camel-test --test http_test` passes with the new tests.
- `cargo xtask schema --check` exits 0; `cargo xtask lint-secrets` exits 0.

- [x] 4.2
