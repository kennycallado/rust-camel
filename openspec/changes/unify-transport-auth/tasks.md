# Tasks: unify-transport-auth

## Phase 1: Security kernel + per-bind gate

### Task 1.1: Kernel types + SecurityPolicy signature in camel-api

**Files:**
- `crates/camel-api/src/security_policy.rs` (modified)
- `crates/camel-api/src/lib.rs` (modified — re-exports)

**Steps:**
1. In `security_policy.rs` add `pub enum TransportId { Http, Ws, Grpc, Mcp }` (Clone, Copy, Debug, PartialEq, Eq, Hash).
2. Add `pub enum AccessMode { Public, Authenticated, Authorized(Arc<dyn SecurityPolicy>) }` with manual `Clone` (mirroring `SecurityPolicyConfig::clone`) and `Debug` rendering the policy as `<SecurityPolicy>`.
3. Add `pub struct AudienceBinding { pub issuers: Vec<String>, pub audiences: Vec<String> }` (Clone, Debug, PartialEq) — PLURAL audiences: every configured accepted audience is kept, none silently discarded.
4. Add `pub struct RouteSecurityPlan { pub access_mode: AccessMode, pub provider_ref: Option<String>, pub transport: TransportId, pub credential_sources: Vec<CredentialSource>, pub audience_binding: Option<AudienceBinding> }` with manual `Clone`/`Debug` (policy redacted).
5. Add `pub trait AuthPrincipal: Send + Sync { fn principal(&self) -> &Principal; fn provider_id(&self) -> &str; }` — the READ interface policies and the layer see. The concrete `AuthenticatedPrincipal` type lives in camel-auth (Task 1.4) so its construction is crate-private; camel-api never names the concrete type (avoids the camel-api↔camel-auth cycle).
6. Add `pub struct AuthContext<'a> { pub principal: &'a dyn AuthPrincipal, pub transport: TransportId }`.
7. Change the `SecurityPolicy` trait method to `async fn evaluate(&self, exchange: &mut Exchange, auth: &AuthContext<'_>) -> Result<AuthorizationDecision, CamelError>`. Keep `AuthorizationDecision` unchanged; document that `Granted { principal }` is the policy's advisory asserted principal (stored to properties for observability) — the authentication identity is the `AuthContext` principal, always. A trait method signature change cannot have a default adaptation; Task 1.2 updates all implementors.

**Tests:**
- `access_mode_debug_redacts_policy`: setup `AccessMode::Authorized(Arc::new(grant_policy))` → action `format!("{:?}", mode)` → assert contains `<SecurityPolicy>`, no policy internals; command `cargo test -p camel-api security_policy`; expected: pass.
- `transport_id_derives_all_four`: assert `TransportId` has exactly Http/Ws/Grpc/Mcp variants via exhaustive match (compile-level); command `cargo test -p camel-api security_policy`; expected: pass.
- `route_security_plan_clone_debug`: setup plan with `AccessMode::Authenticated`, `provider_ref Some("idp-a")`, `TransportId::Http`, one `CredentialSource::AuthorizationHeader`, `audience_binding None` → action clone + Debug → assert cloned fields equal, Debug contains `provider_ref: Some("idp-a")` and NOT policy internals; command `cargo test -p camel-api security_policy`; expected: pass.

**Acceptance:**
- `cargo test -p camel-api --lib` passes.
- `cargo clippy -p camel-api -- -D warnings` exits 0.

- [x] 1.1

### Task 1.2: Update SecurityPolicy implementors for AuthContext

**Files:**
- `crates/services/camel-auth/src/built_in.rs` (modified — RolePolicy + authenticate-only policies + trust_upstream_principal removal)
- `crates/camel-dsl/src/route_ast.rs` (modified — `trust_upstream_principal` field removed at line ~187)
- `crates/camel-dsl/src/yaml.rs` (modified — flag validation/branch removal)
- `crates/camel-dsl/src/model.rs` (modified — flag plumbing removal)
- `crates/camel-dsl/src/compile.rs` (modified — flag compile branch removal)
- `crates/camel-dsl/src/mcp.rs` (modified — flag doc/reference removal)
- `crates/services/camel-auth/src/permission_policy.rs` (modified)
- `crates/services/camel-auth/src/registry.rs` (modified — `impl SecurityPolicy` present in this file)
- `crates/components/camel-component-api/src/consumer.rs` (modified — `impl SecurityPolicy` present)
- `crates/components/camel-component-wasm/src/security_policy.rs` (modified — `impl SecurityPolicy for WasmSecurityPolicy`)
- `crates/camel-api/src/security_policy.rs` (modified — test stubs at ~line 254)
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified — StubPolicy ~line 1335)
- `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs` (modified — CaptureSecCtxPolicy ~line 2980)
- `crates/camel-processor/src/security_policy_layer.rs` (modified — test stubs)
- `crates/camel-dsl/tests/security_policy_integration.rs` (modified — TestPolicy ~line 35)
- `crates/camel-test/tests/security_policy_test.rs` (modified — 4 impls)
- `crates/camel-test/tests/ws_security_test.rs` (modified — 3 impls)

**Steps:**
1. Add `auth: &AuthContext<'_>` parameter to every `SecurityPolicy::evaluate` implementation listed above; policies read claims/roles via the `AuthPrincipal` trait (`auth.principal.principal()`, `auth.principal.provider_id()`).
2. RolePolicy: read roles from `auth.principal.principal()` (the trait accessor) instead of Exchange properties. PRE-1.0 BREAKING: DELETE `trust_upstream_principal` entirely in this task — remove the field from `RouteDslSecurityPolicy` (`crates/camel-dsl/src/route_ast.rs:187`, add to Files) and the property-evidence branch from RolePolicy/built_in (`built_in.rs:25-62` region): `deny_unknown_fields` makes stale configs fail at load naming the field; property-only evidence fails closed from this commit (spec scenario 'exchange-property principal evidence never authorizes').
3. Wasm policy: pass `auth` through to the WASM guest as an additional serialized field in the existing payload (serialize `{ principal: <Principal as today's serialization>, transport: "http" }`); guest contract unchanged otherwise.
4. Test stubs (GrantPolicy/DenyPolicy/FailPolicy/CountingPolicy/StubPolicy/CaptureSecCtxPolicy/TestPolicy + the camel-test impls): accept `&_` and behave as before; CountingPolicy increments on call as before.

**Tests:**
- `role_policy_reads_typed_principal_roles`: this task lands BEFORE Task 1.4's concrete type; define the test stub in camel-auth's own test module with a throwaway `impl AuthPrincipal for TestPrincipal` (the trait is open — implementing it for a test stub is legal and grants NO minting power; only the concrete type matters for security) → assert RolePolicy grants when roles match via `auth.principal.principal()`; command `cargo test -p camel-auth built_in`; expected: pass.
- `trust_upstream_flag_fails_at_load`: YAML route with `trust_upstream_principal: true` → load error names the field (deny_unknown_fields); command `cargo test -p camel-dsl yaml`; expected: pass.
- `property_only_evidence_denies`: Exchange with `camel.auth.principal` property + valid-format principal data, NO token, NO carrier → RolePolicy denies; command `cargo test -p camel-auth built_in`; expected: pass.
- Existing stub tests in api/core/processor updated and green: command `cargo test -p camel-api -p camel-core -p camel-processor --lib`; expected: pass.

**Acceptance:**
- `cargo test -p camel-api -p camel-auth -p camel-core -p camel-processor --lib` passes AND integration-test implementor targets compile: `cargo test -p camel-dsl --test security_policy_integration` and `cargo test -p camel-test --test security_policy_test --test ws_security_test` pass.
- `cargo clippy -p camel-api -p camel-auth -p camel-core -p camel-processor -p camel-component-wasm -- -D warnings` exits 0.

- [x] 1.2

### Task 1.3: ProviderRegistry type with audience reservation

**Files:**
- `crates/services/camel-auth/src/registry.rs` (modified)
- `crates/services/camel-auth/src/lib.rs` (modified — re-export)
- `crates/camel-dsl/src/model.rs` (modified — conversion impl)

**Steps:**
1. In `registry.rs` (which today has `NamedRegistry<T>`, `SecurityPolicyRegistry`, `PermissionEvaluatorRegistry`) add `pub struct ProviderRegistry { inner: NamedRegistry<ProviderEntry> }` and `pub struct ProviderEntry { pub authenticator: Arc<dyn TokenAuthenticator>, pub audience_binding: Option<camel_api::security_policy::AudienceBinding> }`.
2. `ProviderRegistry::register(name, entry)`, `resolve(&self, name: &str) -> Option<Arc<ProviderEntry>>` (Arc-return: `NamedRegistry` is DashMap-backed — a `&ProviderEntry` cannot escape the map guard; entries are stored as `Arc<ProviderEntry>` so resolve clones the Arc out), `len()`, `is_empty()`, `names() -> Vec<String>`. Callers hold the `Arc<ProviderEntry>`.
3. The conversion CANNOT live in camel-auth (camel-dsl depends on camel-auth; reverse would cycle): put it in `crates/camel-dsl/src/model.rs` as `impl SecurityCompileContext { pub fn provider_registry(&self) -> camel_auth::ProviderRegistry }` mapping each named authenticator to an entry with `audience_binding` from the context's `provider_bindings` map (added in Task 1.6; until then `None`).
4. Reserve the Phase-3 cache slot NOW: `ProviderRegistry` gains `pub(crate) authn_cache: Option<Arc<crate::authn_cache::AuthnCache>>` initialized `None` (the `authn_cache` module itself is built in Task 3.2 — until then type the field as `Option<Arc<dyn std::any::Any + Send + Sync>>` OR defer the field to Task 3.2; choose the latter: NO field in Phase 1, Task 3.2 adds it — keeps Phase 1 minimal).
5. Re-export `ProviderRegistry`, `ProviderEntry` from camel-auth `lib.rs`.

**Tests:**
- `provider_registry_registers_and_resolves`: setup register `idp-a` with static authenticator → assert `resolve("idp-a").is_some()`, `resolve("ghost").is_none()`; command `cargo test -p camel-auth registry`; expected: pass.
- `compile_context_converts_to_registry`: setup `SecurityCompileContext` with one provider → action `provider_registry()` → assert same name resolves, `audience_binding` is None (Phase 1 default); command `cargo test -p camel-dsl model`; expected: pass.
- `sole_and_multiple_provider_counts`: setup 1 then 2 providers → assert `len()` 1 and 2 (backs Task 1.8 sole/multiple rules); command `cargo test -p camel-auth registry`; expected: pass.

**Acceptance:**
- `cargo test -p camel-auth registry` + `cargo test -p camel-dsl model` pass.
- `cargo clippy -p camel-auth -p camel-dsl -- -D warnings` exits 0.

- [x] 1.3

### Task 1.4: AuthenticatedPrincipal in camel-auth + kernel_authenticate + dispatch guard

**Files:**
- `crates/services/camel-auth/src/kernel.rs` (new)
- `crates/services/camel-auth/src/lib.rs` (modified — `pub mod kernel;` + re-exports)
- `crates/services/camel-auth/tests/seal_compile_fail.rs` (new — trybuild)
- `crates/services/camel-auth/Cargo.toml` (modified — dev-dependency `trybuild`)

**Steps:**
1. In `kernel.rs`: `pub struct AuthenticatedPrincipal { principal: Principal, provider_id: String }` — pub TYPE (nameable so `get_extension::<AuthenticatedPrincipal>` downcast works from other crates), ALL fields private, `#[derive(Clone)]`, `impl camel_api::security_policy::AuthPrincipal for AuthenticatedPrincipal` (the trait accessors), and NO public constructor AT ALL: construction is an inherent private fn in this module. This is the spec's 'construction is not expressible' — no `__mint`, no doc-hidden escape, no feature-gated test constructor (Cargo feature unification makes those unsound). Same-crate construction is the only sound seal.
2. NO test-only constructor exists. Every test that needs a principal mints through the REAL path: build a `ProviderRegistry` with a `StaticTokenAuthenticator` fixture provider + plan + token → `kernel_authenticate` returns a genuine principal.
3. `pub async fn kernel_authenticate(plan: &RouteSecurityPlan, providers: &ProviderRegistry, credentials: &ExtractedToken) -> Result<AuthenticatedPrincipal, CamelError>` — resolves provider from `plan.provider_ref` via `providers.resolve` (`Unauthenticated` naming the provider when unresolved), calls `authenticate_bearer` (Task 3.1 switches to `authenticate(AuthnRequest)`), constructs via the private fn. Canonical arity `(plan, providers, credentials)` for every Phase-2 task.
4. Public bypass guard (a Public plan has NO provider_ref): `pub fn enforce_dispatch(plan: &RouteSecurityPlan, exchange: &Exchange) -> Result<(), CamelError>` — `AccessMode::Public` → `Ok(())` (no extraction, no carrier, pass-through; transports ALSO short-circuit extraction for Public plans before calling anything); non-Public → `Ok` only if the carrier is present AND `carrier.provider_id() == plan.provider_ref` (ROUTE-BOUND: a principal minted for provider A must NOT satisfy provider B's route — cross-provider replay denied), else `Err(Unauthenticated)`. Transports call `enforce_dispatch` after their authn attempt; Task 2.9's core dispatch check uses it too.
5. Carrier: `pub const KERNEL_PRINCIPAL_KEY: &str = "camel.auth.principal.typed"` + `pub fn install_carrier(exchange: &mut Exchange, principal: &AuthenticatedPrincipal)` doing `exchange.set_extension(KERNEL_PRINCIPAL_KEY, Arc::new(principal.clone()))` (matches `set_extension(key, Arc<dyn Any + Send + Sync>)`, exchange.rs:174) + `pub fn read_carrier(exchange: &Exchange) -> Option<AuthenticatedPrincipal>` via `get_extension::<AuthenticatedPrincipal>` + clone. Transports call `install_carrier` after `kernel_authenticate`; the layer (Task 1.7) uses `read_carrier`. Unforgeable: storing a value under the key requires an `AuthenticatedPrincipal`, which no external code can construct; wrong-type values fail the downcast.

**Tests:**
- `kernel_authenticate_mints_with_provider`: registry with static provider `idp-a` (token `t-a`), plan `Authenticated` provider_ref `idp-a`, credentials `t-a` → `provider_id() == "idp-a"`; command `cargo test -p camel-auth kernel`; expected: pass.
- `kernel_authenticate_denies_wrong_token`: same, credentials `wrong` → `Err(Unauthenticated)`; command same; expected: pass.
- `principal_construction_unexpressible_outside_crate`: trybuild compile-fail — external crate attempts `AuthenticatedPrincipal { .. }` (E0451 private fields); with zero public constructors there is no other path to test; place in `crates/services/camel-auth/tests/seal_compile_fail.rs`; command `cargo test -p camel-auth --test seal_compile_fail`; expected: pass.
- `enforce_dispatch_public_passes_without_carrier`: Public plan + empty Exchange → `Ok(())`; command `cargo test -p camel-auth kernel`; expected: pass.
- `enforce_dispatch_nonpublic_requires_carrier`: Authenticated plan (provider_ref `idp-a`) + empty Exchange → `Err(Unauthenticated)`; with `install_carrier` of an `idp-a` principal applied first → `Ok(())`; command same; expected: pass.
- `enforce_dispatch_rejects_cross_provider_carrier`: Authenticated plan provider_ref `idp-b` + carrier holding an `idp-a` principal → `Err(Unauthenticated)` (route-bound); command same; expected: pass.

**Acceptance:**
- `cargo test -p camel-auth kernel` + seal compile-fail pass.
- `cargo clippy -p camel-auth -- -D warnings` exits 0.

- [x] 1.4

### Task 1.5: SecurityContext carries plan + provider registry

**Files:**
- `crates/components/camel-component-api/src/consumer.rs` (modified — `SecurityContext` line ~318)
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified — context construction sites)

**Steps:**
1. Add `pub plan: Option<camel_api::security_policy::RouteSecurityPlan>` AND `pub providers: Option<std::sync::Arc<camel_auth::ProviderRegistry>>` to `SecurityContext` (camel-component-api already depends on camel-api AND camel-auth — the existing `authenticator: Arc<dyn TokenAuthenticator>` field proves the camel-auth dep; verify in Cargo.toml).
2. Add `pub fn with_plan(mut self, plan: RouteSecurityPlan) -> Self` and `pub fn with_providers(mut self, providers: Arc<ProviderRegistry>) -> Self`; existing fields/constructors unchanged (Phase 2 consumers migrate incrementally).
3. REGISTRY INJECTION PATH: the `set_security_context` call sites in `route_controller_trait.rs` (~157-164, ~503-510) that build the context from `SecurityCompileContext` also attach `provider_registry()` — this is how `kernel_authenticate`'s `providers` argument reaches every transport (grpc 2.1, mcp 2.6, ws 2.8, http 2.9 read it from their SecurityContext).

**Tests:**
- `security_context_holds_plan_and_providers`: setup a Public plan + a ProviderRegistry with one provider → action `SecurityContext::new(policy, auth).with_plan(plan).with_providers(reg)` → assert `ctx.plan` present with matching access mode AND `ctx.providers` resolves the provider; command `cargo test -p camel-component-api consumer`; expected: pass.

**Acceptance:**
- `cargo test -p camel-component-api --lib` passes; `cargo clippy -p camel-component-api -- -D warnings` exits 0.

- [x] 1.5

### Task 1.6: AudienceBinding from EXISTING config schema + CLI wiring

**Files:**
- `crates/camel-cli/src/security.rs` (modified — `resolve_authenticators` ~line 239)
- `crates/camel-dsl/src/model.rs` (modified — `SecurityCompileContext`)

**Steps:**
1. NO new config fields: the schema already carries audience/issuer — `OidcSecurityConfig { issuer: String (line ~743), audience: Vec<String> (line ~747) }` and `KeycloakValidationConfig { audience: Vec<String> (line ~907) }`. In camel-cli `resolve_authenticators`: after building each named provider's authenticator, construct `AudienceBinding` FROM the existing fields — oidc provider: `issuers: vec![oidc.issuer.clone()]`, `audiences: oidc.audience.clone()` (ALL configured audiences kept); keycloak provider: `KeycloakSecurityConfig` has NO issuer field — derive it: `issuers: vec![format!("{}/realms/{}", server_url, realm)]` from the existing `server_url`/`realm` fields (same shape keycloak JWKS URLs use); `audiences: validation.audience.clone()`; native provider: `AudienceBinding { issuers: vec![], audiences: vec![] }` (static tokens have no issuer/audience semantics — reservation only).
2. Extend `SecurityCompileContext` with `pub provider_bindings: HashMap<String, AudienceBinding>` populated alongside the provider map (`register_providers` and `SecurityCompileContext::new` updated; the existing `providers: HashMap<String, Arc<dyn TokenAuthenticator>>` map is unchanged).
3. `SecurityCompileContext::provider_registry()` (Task 1.3) populates each `ProviderEntry.audience_binding` from `provider_bindings` (`None` when absent).

**Tests:**
- `oidc_binding_maps_issuer_and_audience`: setup `[security.oidc]` with `issuer = "https://a"`, `audience = ["api", "api-2"]`, wiremock JWKS (follow existing camel-cli security test precedent) → action `resolve_authenticators` → assert binding `issuers == ["https://a"]`, `audiences == ["api", "api-2"]` (plural, nothing discarded); command `cargo test -p camel-cli security`; expected: pass.
- `keycloak_binding_maps_validation_audience`: setup keycloak section with `server_url = "https://kc"`, `realm = "demo"`, `validation.audience = ["api"]` → assert `audiences == ["api"]` AND `issuers == ["https://kc/realms/demo"]`; command `cargo test -p camel-cli security`; expected: pass.
- `native_binding_is_reservation_only`: setup native credentials → assert `issuers` and `audiences` empty (reservation only); command `cargo test -p camel-cli security`; expected: pass.
- `provider_registry_carries_audience_from_context`: setup context with `provider_bindings` → assert `provider_registry().resolve(name).audience_binding` equals configured; command `cargo test -p camel-dsl model`; expected: pass.

**Acceptance:**
- `cargo test -p camel-cli security -p camel-dsl model` passes.
- `cargo xtask schema --check` exits 0 (schema untouched — no new fields).

- [x] 1.6

### Task 1.7: Dual-read SecurityPolicyLayer (carrier or legacy bearer)

**Files:**
- `crates/camel-processor/src/security_policy_layer.rs` (modified)
- `crates/camel-processor/Cargo.toml` (modified — `camel-auth = { workspace = true }` in `[dependencies]` if absent)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_ext.rs` (modified — layer construction sites ~lines 218/251)

**Steps:**
1. In `SecurityPolicyService::call`, dual-read (Phase-1 additive; the legacy branch is deleted in Task 2.9): first read the typed carrier via `camel_auth::kernel::read_carrier(exchange)` (returns a cloned `AuthenticatedPrincipal`; the clone ends the extension borrow before `evaluate(&mut exchange, ..)` — avoids E0502); if present, build `AuthContext { principal: &cloned as &dyn AuthPrincipal, transport }` (transport passed into `SecurityPolicyLayer::new` as a new parameter) and call `evaluate(exchange, &auth)`.
2. Layer constructor becomes `SecurityPolicyLayer::new(policy, authenticator: Arc<dyn TokenAuthenticator>, credential_sources: Vec<CredentialSource>, transport: TransportId)` — the legacy branch needs the route's authenticator (today extraction+authn live inside policies, `built_in.rs:32-51`; the layer itself holds only `policy`, constructed at `route_compiler_ext.rs:218/251`). Thread the three new arguments from `managed.compiled`'s security fields (`security_authenticator`, credential sources, scheme-derived transport) at BOTH construction sites in `route_compiler_ext.rs`.
3. If NO carrier: fall back to the CURRENT legacy decision path using the threaded authenticator (Bearer extraction from Exchange headers per `credential_sources` + authenticate + property-based principal). Post-1.2 the trait needs an `AuthContext`: the legacy branch wraps its obtained `Principal` in a PRIVATE adapter `struct LegacyPrincipal(Principal)` (in the layer module, not exported) implementing `AuthPrincipal` with `provider_id() == "legacy"` — Bearer-evidence ONLY (the property-evidence path was deleted in Task 1.2; the adapter wraps the Principal the threaded authenticator returned for a real token). No `AuthenticatedPrincipal` minting involved, same Principal data the policy saw before.
4. `Granted { principal }` outcome still calls `store_principal_properties` (advisory) on both branches.
5. No transition caveats remain: property evidence died in Task 1.2 (fail-closed from the first Phase-1 commit — spec-consistent at every intermediate commit); the dual-read's legacy branch handles real Bearer tokens only and is deleted at Task 2.9 strict mode.

**Tests:**
- `layer_denies_without_typed_principal_or_token`: setup grant-everything policy + Exchange with NO carrier and NO token/property → action `call` → assert `Err(Unauthenticated)` and policy-evaluated counter == 0; command `cargo test -p camel-processor security_policy_layer`; expected: pass.
- `layer_grants_with_typed_principal`: setup same + carrier installed via `install_carrier` with a principal minted through the REAL path (camel-auth dev-dep: `ProviderRegistry` + StaticTokenAuthenticator fixture + `kernel_authenticate`) and NO token in the Exchange (proves carrier path independent of legacy) → assert Ok, counter == 1, and AuthContext carried the principal (CountingPolicy-style stub records it); command `cargo test -p camel-processor security_policy_layer`; expected: pass.
- `layer_legacy_bearer_still_grants`: setup NO carrier + valid Bearer token in Exchange headers + route authenticator (existing behavior regression guard) → assert Ok (dual-read keeps Phase 1 green); command `cargo test -p camel-processor security_policy_layer`; expected: pass.
- `spoofed_extension_value_does_not_authorize`: setup carrier key set to a WRONG type value (`Arc::new("x".to_string())` under `KERNEL_PRINCIPAL_KEY`), no token → downcast fails, legacy finds nothing → assert deny; command `cargo test -p camel-processor security_policy_layer`; expected: pass.
- `spoofed_legacy_property_without_token_denies`: setup raw `camel.auth.principal` property, no carrier, no token, flag-false policy → assert deny; command `cargo test -p camel-processor security_policy_layer`; expected: pass.

**Acceptance:**
- `cargo test -p camel-processor security_policy_layer` passes (5 tests).
- Existing auth suites stay green: `cargo test -p camel-processor --lib` and `cargo test -p camel-test --test security_policy_test --test auth_multi_credential_test` exit 0.

- [x] 1.7

### Task 1.8: Plan compilation in camel-core

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_compiler_ext.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified — plan threading into SecurityContext)
- `crates/camel-dsl/src/route_ast.rs` (modified — `RouteDslSecurityPolicy.audiences` field)
- `crates/camel-dsl/src/yaml.rs` (modified — audiences validation ~line 444)

**Steps:**
1. In `route_compiler_ext.rs` add `pub fn compile_route_security_plan(definition: &RouteDefinition, providers: &ProviderRegistry) -> Result<Option<RouteSecurityPlan>, CamelError>` — `Ok(None)` means "not a server consumer, skip plan attachment". Transport derived from `definition.from_uri` scheme prefix: `http`→Http, `ws`→Ws, `grpc`→Grpc, `mcp`→Mcp; non-consumer or unknown scheme → `Ok(None)` (the every-server-route invariant applies only to consumer-backed routes the controller starts).
2. Classification per blessed rules: roles/scopes policy with named `provider` → resolve in registry (missing → `RouteError` naming route + provider, NEVER downgrade); roles/scopes without named provider → same resolution rules as authorization-only forms (zero providers → error naming route; exactly one → that provider; multiple → error requiring selection); `ref`/`wasm`/`permission` forms → identical sole/multi resolution; authenticate-only form → `Authenticated`. No policy and no provider → `Public`.
3. Credential-source capability per `TransportId`: Http allows all four sources; Ws/Mcp allow AuthorizationHeader/Header/Cookie; Grpc allows AuthorizationHeader/Header. Violation → `RouteError` naming source + transport.
4. `audience_binding` populated with ROUTE-LEVEL PRECEDENCE: `RouteDslSecurityPolicy` gains `#[serde(default)] pub audiences: Option<Vec<String>>` (route override; load-time validation mirrors the existing `provider` field rule — only valid with the roles/scopes forms, enforced in `yaml.rs` next to the provider validation ~line 444). Plan compilation: route `audiences` present → plan binding uses them (issuers still from provider); absent → copy the resolved `ProviderEntry.audience_binding` verbatim. This is the route-level audience source the audience-distinguished scenarios require.
5. In `route_controller.rs` staging: call `compile_route_security_plan` before consumer start for every consumer route; attach the plan to the compiled/managed route struct; failure aborts staging. In `route_controller_trait.rs` (the `set_security_context` sites ~lines 157-164 and 503-510), thread the plan into `SecurityContext` via `with_plan` (Task 1.5).

**Tests:**
- `compilation_public_default_no_declaration`: setup definition without policy/provider + one registered provider → assert `Public`, `provider_ref None`; command `cargo test -p camel-core compile_route_security`; expected: pass.
- `compilation_roles_named_provider_resolves_with_audience`: setup roles policy `provider: "idp-a"` registered with AudienceBinding → assert `Authorized`, `provider_ref Some("idp-a")`, `audience_binding` copied from the provider; command `cargo test -p camel-core compile_route_security`; expected: pass.
- `compilation_route_audiences_override_provider`: setup same provider (provider audiences `["api"]`) + route declaring `audiences: ["api-2"]` → assert plan binding `audiences == ["api-2"]` while `issuers` still from provider; second route without override keeps `["api"]`; command `cargo test -p camel-core compile_route_security`; expected: pass.
- `dsl_route_audiences_validated_like_provider`: setup `audiences` on a `ref`-form policy → load error mirroring the provider rule; command `cargo test -p camel-dsl yaml`; expected: pass.
- `compilation_named_provider_missing_fails`: setup `provider: "ghost"` → assert Err contains route id + `ghost`, no Public downgrade; command `cargo test -p camel-core compile_route_security`; expected: pass.
- `compilation_wasm_sole_provider_resolves`: setup wasm form + exactly one provider → assert `Authorized` + `provider_ref Some(sole)`; command `cargo test -p camel-core compile_route_security`; expected: pass.
- `compilation_authonly_zero_providers_fails`: setup wasm form + empty registry → assert Err naming route; command `cargo test -p camel-core compile_route_security`; expected: pass.
- `compilation_ref_multiple_unnamed_fails`: setup ref form + two providers → assert Err requiring selection; command `cargo test -p camel-core compile_route_security`; expected: pass.
- `compilation_rejects_queryparam_on_mcp_and_ws`: setup mcp route + QueryParam source → Err naming source+transport; same for ws; http accepts QueryParam; command `cargo test -p camel-core compile_route_security`; expected: pass.
- `staging_attaches_plan_before_consumer_start_all_schemes`: setup definitions for `http:`, `ws:`, `grpc:`, `mcp:` consumer routes with test consumers recording call order → action stage → assert each compiled route holds a plan with correct `TransportId` and consumer start observed AFTER compilation (order recorded); command `cargo test -p camel-core compile_route_security`; expected: pass.

**Acceptance:**
- `cargo test -p camel-core compile_route_security` passes (10 tests).
- `cargo clippy -p camel-core -p camel-dsl -- -D warnings` exits 0.

- [x] 1.8

### Task 1.9: Per-bind exposure gate (camel-config owned channel)

**Files:**
- `crates/camel-config/src/config.rs` (modified — new per-bind section on `CamelConfig`)
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified)
- `crates/camel-cli/src/commands/run.rs` (modified — BindExposureAcks construction near the `build_security_compile_context_from_config` call ~line 246)

**Steps:**
1. Channel decision (uniform, TOML, honors the blessed spec wording "set ... in TOML"): ONLY `McpServerConfig` among component configs is TOML-deserialized (`HttpServerConfig` is URI-parsed, camel-ws/grpc have no config struct), so the ack gets ONE camel-config-owned section: add to `CamelConfig` `#[serde(default)] pub binds: HashMap<String, BindExposureConfig>` keyed by bind address string (`"0.0.0.0:8080"`) with `pub struct BindExposureConfig { #[serde(default)] pub allow_public_exposure: bool }` (`deny_unknown_fields`). No component config changes.
2. In camel-core `route_controller_trait.rs` add `pub struct BindExposureAcks(HashMap<String, bool>)` with `pub fn new(map: HashMap<String, bool>) -> Self` and `pub fn acknowledged(&self, bind: &str) -> bool` — plain map, NO camel-config dependency (camel-core stays camel-config-free). Built in camel-cli (`crates/camel-cli/src/commands/run.rs`, adjacent to the `build_security_compile_context_from_config` call ~line 246) from `CamelConfig.binds` as `binds.iter().map(|(k, v)| (k.clone(), v.allow_public_exposure)).collect()`, then passed into route staging.
3. Add `pub fn enforce_bind_exposure_gate(bind_addr: SocketAddr, plans: &[(&str, &RouteSecurityPlan)], acked: bool) -> Result<(), CamelError>` — each pair is `(route_id, plan)` so the error can NAME the offending Public routes: loopback (`127.0.0.0/8`, `::1`) → Ok; non-loopback + any `Public` + `!acked` → `Err(RouteError)` naming bind + public route ids; acked + publics present → `tracing::warn!` naming bind + count, then Ok. Non-Public plans that failed classification never reach here (Task 1.8 aborts staging first).
4. Call the gate where `set_security_context` runs (~lines 157-164, 503-510), passing all plans sharing that bind address and `acks.acknowledged(bind_string)`.

**Tests:**
- `gate_refuses_nonloopback_public_without_ack`: plans `[("r1", Public)]`, `0.0.0.0:8080`, ack false → Err naming bind AND route `r1`; command `cargo test -p camel-core bind_exposure_gate`; expected: pass.
- `gate_acknowledged_warns_and_passes`: same + ack true, warn captured via local tracing subscriber → Ok + one warn containing bind and `1`; command `cargo test -p camel-core bind_exposure_gate`; expected: pass.
- `gate_loopback_public_needs_no_ack`: `[Public]`, `127.0.0.1:0` and `::1` → Ok, zero warns; command `cargo test -p camel-core bind_exposure_gate`; expected: pass.
- `gate_ack_does_not_excuse_failed_sibling`: staging integration — one good Public + one unresolvable-provider route on one bind → whole staging fails (Task 1.8 error surfaces, gate never reached); command `cargo test -p camel-core bind_exposure_gate`; expected: pass.
- `config_parses_binds_section`: TOML `[binds."127.0.0.1:0"] allow_public_exposure = true` → parsed; absent section → empty map, ack false everywhere; command `cargo test -p camel-config config`; expected: pass.

**Acceptance:**
- `cargo test -p camel-core bind_exposure_gate` + `cargo test -p camel-config config` pass.
- `cargo clippy -p camel-core -p camel-config -- -D warnings` exits 0.

- [x] 1.9

### Task 1.10: SecurityConfigFixture in camel-test

**Files:**
- `crates/camel-test/src/security_fixture.rs` (new)
- `crates/camel-test/src/lib.rs` (modified — module + re-export)
- `crates/camel-test/Cargo.toml` (modified — `camel-config` and `camel-auth` moved to/added under `[dependencies]`: the fixture is PUBLIC LIBRARY code, dev-deps do not compile into normal `cargo build -p camel-test`)

**Steps:**
1. `pub struct SecurityConfigFixture` with `pub fn single_static_provider(name: &str) -> Self` building a `camel_config::config::SecurityConfig` whose `native` section carries one credential entry: token `test-token-<name>`, role `test-role`, plaintext secret (NO `{{env:}}` placeholders — deterministic).
2. `pub fn to_config(self) -> camel_config::config::SecurityConfig` (concrete type; feeds `resolve_authenticators` in E2E tests, Task 1.11).
3. `pub fn providers(self) -> ProviderRegistry` convenience: builds a `ProviderRegistry` with the fixture's static authenticator registered under `name` (uses camel-auth `StaticTokenAuthenticator`).

**Tests:**
- `fixture_serializes_without_env_placeholders`: setup fixture `idp-test` → action serialize to TOML → assert contains `test-token-idp-test`, zero `{{env:` substrings; command `cargo test -p camel-test security_fixture`; expected: pass.
- `fixture_registry_resolves`: action `providers()` → assert `resolve("idp-test").is_some()`; command `cargo test -p camel-test security_fixture`; expected: pass.

**Acceptance:**
- `cargo test -p camel-test security_fixture` passes; `cargo clippy -p camel-test -- -D warnings` exits 0.

- [x] 1.10

### Task 1.11: Cross-plane denial E2E test

**Files:**
- `crates/camel-test/tests/kernel_fail_closed_test.rs` (new)

**Steps:**
1. E2E: `CamelTestContext` + one `http:` server route declaring a roles policy resolved to the fixture provider (Task 1.10), body → `mock:result`.
2. Raw request WITHOUT `Authorization` → assert 401 + mock inbox empty.
3. Same route WITH valid token → assert 2xx + inbox 1 (rides the Task 1.7 legacy fallback in Phase 1; the kernel mint path takes over in Task 2.9 — both must hold green at every phase).

**Tests:**
- `secured_route_denies_missing_credentials_e2e`: as step 2; command `cargo test -p camel-test --test kernel_fail_closed_test`; expected: pass.
- `secured_route_grants_valid_token_e2e`: as step 3; command `cargo test -p camel-test --test kernel_fail_closed_test`; expected: pass.

**Acceptance:**
- `cargo test -p camel-test --features integration-tests --test kernel_fail_closed_test` passes (DEViation recorded: feature-gated like auth_multi_credential_test — real sockets/servers; CI job added mirroring the auth_multi job).
- Kernel suite: `cargo test -p camel-api -p camel-auth -p camel-processor -p camel-core -p camel-component-api -p camel-dsl -p camel-test --lib` exits 0.

- [x] 1.11

### Task 1.12: ADR-0061

**Files:**
- `docs/adr/0061-unified-transport-auth.md` (new)
- `CONTEXT-MAP.md` (modified — ADR index)

**Steps:**
1. Record: ratified contract from 73076dbd (fail-closed placeholders, named providers, OIDC prefetch, N credentials); kernel types (AccessMode, AuthPrincipal trait + AuthenticatedPrincipal concrete, RouteSecurityPlan, AudienceBinding reserved, TransportId, AuthContext); provider = authn selector / access mode = enforcement predicate; Public-by-default + per-bind `allow_public_exposure` via `[binds."<addr>"]` (expert-ruled additive layering; ADR-0052 rule-3 permanent warn); ADR-0060 Rule 3 amendment + Rule 8 supersession; policy-form sole/multi/zero provider resolution; `trust_upstream_principal` REMOVAL (pre-1.0 breaking: property evidence has no authorization path from the first Phase-1 commit; stale configs fail at load); per-provider independent validation + provider-local authn cache (decision now, enforcement Phase 3); HONEST seal statement: `AuthenticatedPrincipal` construction is same-crate-only (camel-auth `kernel.rs`) with zero public or feature-gated constructors — not even test-only ones (tests mint through the real `kernel_authenticate` path); the seal guards against accidental construction and property spoofing everywhere, and against hostile IN-CRATE code only at review level (in-process trust boundary documented).
2. Add `CONTEXT-MAP.md` ADR table entry.

**Tests:**
- `adr_indexed`: `grep -c "0061-unified-transport-auth" CONTEXT-MAP.md` → 1.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- ADR format matches ADR-0060 (Status/Context/Decision/Consequences).

- [x] 1.12

## Phase 2: Transport convergence

### Task 2.1: gRPC interceptor lifecycle

**Files:**
- `crates/components/camel-component-grpc/src/server.rs` (modified)
- `crates/components/camel-component-grpc/src/consumer.rs` (modified)
- `crates/components/camel-component-grpc/tests/server_auth_test.rs` (new — no auth test file exists today; tests/ has only integration.rs)

**Steps:**
1. `consumer.rs` `set_security_context` (line ~641): extract plan from context (Task 1.5), pass into the server builder as a constructor argument so the interceptor is CREATED with the plan captured (not a post-hoc setter — the lifecycle fix).
2. `server.rs` interceptor: `AccessMode::Public` → skip extraction entirely (pass-through; kernel_authenticate is never called for Public); otherwise extract credentials per `plan.credential_sources` (Authorization metadata + named headers for grpc) → `camel_auth::kernel_authenticate(plan, providers, credentials)` → minted principal or `tonic::Status::unauthenticated` denial. No local JWT parsing. CRITICAL: the interceptor's request→Exchange construction MUST call `camel_auth::kernel::install_carrier(&mut exchange, &principal)` BEFORE the pipeline runs — the gRPC server/consumer boundary creates a fresh Exchange per request; without the typed carrier on THAT Exchange, Task 2.9's dispatch enforcement would deny valid gRPC requests.
3. Denial mapping unchanged in idiom (`tonic::Status`).

**Tests:**
- `grpc_plan_present_at_interceptor_construction`: setup consumer with plan-bearing SecurityContext → start server → assert interceptor struct captured plan (unit assert on builder field); command `cargo test -p camel-component-grpc server`; expected: pass.
- `grpc_denies_without_credentials_under_kernel`: setup Authenticated plan + fixture provider; tonic client without metadata → assert `Status` unauthenticated + route body counter 0; command `cargo test -p camel-component-grpc --test server_auth_test`; expected: pass.
- `grpc_named_header_credential_authenticates`: setup plan with `Header { name: "x-api-key" }` + token in that metadata → assert call succeeds and principal reaches route; command `cargo test -p camel-component-grpc --test server_auth_test`; expected: pass.

**Acceptance:**
- gRPC suite green; `cargo clippy -p camel-component-grpc --all-targets -- -D warnings` exits 0.

- [x] 2.1

### Task 2.2: Per-route dispatch enforcement + late registration

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_controller.rs` (modified — `add_route` late-registration path)
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified only if a shared helper is needed)

**Steps:**
1. This task is scoped to LATE REGISTRATION only (the pre-pipeline carrier-requirement dispatch denial is deliberately deferred to Task 2.9 — enforcing it here would deny every not-yet-migrated transport's legacy-bearer grants between 2.2 and 2.9, the exact window the round-2 ruling banned).
2. Late registration: compile plan (Task 1.8) + run `enforce_bind_exposure_gate` for the target bind atomically (single lock scope around registration); failure rejects registration, route never reachable.

**Tests:**
- `late_public_route_nonloopback_rejected`: setup running non-loopback listener without ack + late Public route → assert Err naming bind+route, nothing served; command `cargo test -p camel-core late_registration_gate`; expected: pass.
- `late_route_loopback_public_accepted`: setup loopback listener + late Public route → assert Ok + dispatch works; command `cargo test -p camel-core late_registration_gate`; expected: pass.

**Acceptance:**
- `cargo test -p camel-core late_registration_gate` passes; `cargo clippy -p camel-core -- -D warnings` exits 0.

- [x] 2.2

### Task 2.3: Typed McpTlsConfig

**Files:**
- `crates/components/camel-component-mcp/src/config.rs` (modified — `tls: Option<serde_json::Value>` at line ~64)
- `crates/components/camel-component-mcp/src/adapter/server.rs` (modified — TLS load)
- `crates/components/camel-component-mcp/tests/server_config_test.rs` (modified)
- `crates/camel-dsl/src/mcp.rs` (modified — `RouteDslMcpServer.tls` line ~55 same shape)

**Steps:**
1. `pub struct McpTlsConfig { pub cert_path: String, pub key_path: String }` serde `deny_unknown_fields`, both paths trimmed-non-empty validated at deserialize (custom deserialize fn; error names the field on empty/missing).
2. `McpServerConfig.tls: Option<McpTlsConfig>`. (Amended 2026-08-20: the crate has no TLS load path yet — consumer TLS wiring is Task 2.4 step 1, where the typed struct is consumed at listener construction. No rustls/PEM helpers exist to update in this task.)
3. DSL AST mirrors the same field shape (no cross-crate type, ADR-0060 Rule 9).

**Tests:**
- `tls_typed_config_parses`: TOML `{ cert_path = "a.pem", key_path = "b.pem" }` → typed fields; command `cargo test -p camel-component-mcp config`; expected: pass.
- `tls_empty_cert_path_rejected_at_load`: `cert_path = ""` → deserialize error names `cert_path`; command `cargo test -p camel-component-mcp config`; expected: pass.
- `tls_unknown_field_rejected`: extra `min_version` key → deny_unknown_fields error; command `cargo test -p camel-component-mcp config`; expected: pass.

**Acceptance:**
- `cargo test -p camel-component-mcp config` passes; `cargo clippy -p camel-component-mcp -p camel-dsl -- -D warnings` exits 0.

- [x] 2.3

### Task 2.4: MCP listener ownership DSL→runtime + hard conflicts

**Files:**
- `crates/components/camel-component-mcp/src/consumer.rs` (modified)
- `crates/components/camel-component-mcp/src/registry.rs` (modified)
- `crates/components/camel-component-mcp/tests/server_consumer_test.rs` (modified)
- `crates/camel-dsl/src/mcp.rs` (modified — module docs: fields flow to runtime; step 4)

**Steps:**
1. Consumer start: when the route originates from a DSL `mcp:` block (thread `RouteDslMcpServer` values through lowering into an endpoint parameter `mcp.declared.*`), DSL `bind`/`tls`/`max_tools`/`max_resources` ARE the runtime values for listener construction/lookup.
2. Both DSL and TOML declaring the same key with DIFFERENT values → startup error naming both sources (spec scenario 'TOML/DSL conflict fails startup'); equal values → proceed; TOML-only keys (`allowed_hosts`) still apply.
3. TOML-only servers unchanged.
4. Update `crates/camel-dsl/src/mcp.rs` module docs: fields flow to runtime now (remove 'declaration parity' language).

**Tests:**
- `dsl_tls_reaches_listener`: DSL tls paths (rcgen tempdir PEMs) → start → TLS client handshake succeeds; command `cargo test -p camel-component-mcp --test server_consumer_test`; expected: pass.
- `dsl_toml_bind_conflict_fails_startup`: DSL `127.0.0.1:9100` + TOML `127.0.0.1:9200` → Err naming `dsl` and `toml`; command same; expected: pass.
- `dsl_toml_equal_values_proceed`: same bind both sides → starts; command same; expected: pass.
- `toml_only_server_unchanged`: no DSL block → starts as before; caps default 128 regression assert; command same; expected: pass.

**Acceptance:**
- `cargo test -p camel-component-mcp` whole crate green; clippy exits 0.

- [x] 2.4

### Task 2.5: Repeated MCP header normalization

**Files:**
- `crates/components/camel-component-mcp/src/headers.rs` (modified)
- `crates/components/camel-component-mcp/src/adapter/server.rs` (modified — `request_headers` ~line 314)

**Steps:**
1. `pub fn normalize_repeated(headers: &HeaderMap) -> HeaderMap` in `headers.rs`: `Cookie` joins `"; "`; `Authorization`/`Proxy-Authorization` keep FIRST value + `tracing::warn!` naming header; every other repeated header joins `", "` (first-seen order).
2. Call it in `adapter/server.rs` `request_headers` before building `McpRequestHeaders`.

**Tests:**
- `multiple_cookies_join_semicolon`: two Cookie headers → single `a=1; b=2`; command `cargo test -p camel-component-mcp headers`; expected: pass.
- `repeated_authorization_first_value_warns`: two Authorization → first kept + one warn naming `Authorization`; command same; expected: pass.
- `arbitrary_header_joins_comma`: two `X-Trace-Id` → `t1, t2`; command same; expected: pass.

**Acceptance:**
- `cargo test -p camel-component-mcp headers` passes; clippy exits 0.

- [x] 2.5

### Task 2.6: MCP kernel migration (dispatch-registry injection)

**Files:**
- `crates/components/camel-component-mcp/src/consumer.rs` (modified)
- `crates/components/camel-component-mcp/src/adapter/server.rs` (modified)
- `crates/components/camel-component-mcp/src/registry.rs` (modified)

**Steps:**
1. Consumer start: when the compiled route carries a `RouteSecurityPlan` (Task 1.8 attaches it), register the plan with the per-bind dispatch registry entry so the rmcp adapter's request path can reach it (`McpServerRegistry` entry gains `plans: HashMap<String /*route id*/, RouteSecurityPlan>` plus the `ProviderRegistry` snapshot).
2. `adapter/server.rs` request path (the `request_headers` seam): for each tool/resource invocation: `AccessMode::Public` plan → skip extraction (pass-through); otherwise extract credentials per the plan's `credential_sources` from the normalized headers (Task 2.5) → `camel_auth::kernel_authenticate(plan, providers, credentials)` → set the typed carrier on the Exchange before the route pipeline runs. Denial maps to the MCP error idiom: tools get an `isError` result carrying the denial, resources get the error body — the route body never sees a denied Exchange (same error path `mcp_server_auth_test.rs` asserts today).
3. No rmcp types leak past `src/adapter/` (ADR-0060 Rule 4 unchanged).
4. AMENDMENT (from Task 1.9 review): MCP binds are invisible to the route-level gate — `mcp:` from-URIs carry no authority and the listener binds via `McpServerConfig.bind` in `McpServerRegistry`. Wire the per-bind exposure gate into the MCP registry's existing `validate_server_policy` start validation: for each server entry, collect the plans registered in step 1 for that bind and call `camel_core::route_controller::enforce_bind_exposure_gate(bind, is_loopback, plans, acked)`; acks thread into the registry alongside the provider snapshot (CLI already builds `BindExposureAcks`). The existing warn-only behavior is REPLACED by the gate semantics (refuse-without-ack on non-loopback, permanent warn when acked).

**Tests:**
- `mcp_kernel_denies_without_credentials`: setup Authenticated-plan tool route + fixture provider, JSON-RPC call without credentials → assert isError result carrying the denial, route body counter 0; command `cargo test -p camel-test --test mcp_server_auth_test`; expected: pass.
- `mcp_kernel_grants_authorization_header`: setup same + `Authorization: Bearer test-token-<name>` → assert tool result ok, route body ran, typed principal present on the Exchange (assert via route body reading the carrier); command `cargo test -p camel-test --test mcp_server_auth_test`; expected: pass.
- `mcp_rmcp_confinement_holds_after_migration`: existing `rmcp_boundary_test.rs` stays green (no new rmcp leakage); command `cargo test -p camel-component-mcp --test rmcp_boundary_test`; expected: pass.

**Acceptance:**
- The three tests above pass; `cargo test -p camel-component-mcp` green.
- `mcp_bind_gate_refuses_public_without_ack`: non-loopback bind + Public plan + no ack → registry start fails naming bind; acked → warn + start proceeds (mirror the camel-core gate tests).
- `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exits 0.

- [x] 2.6

### Task 2.7: MCP credential_sources coverage

**Files:**
- `crates/camel-test/tests/mcp_server_auth_test.rs` (modified — relocated file, bd rc-03zo)

**Steps:**
1. Add per-source cases: Cookie, named Header (AuthorizationHeader already covered — keep). Each: `CamelTestContext` + fixture provider, raw JSON-RPC with credential placed correctly, assert body executed (mock inbox) or denied (isError + empty inbox).
2. Compile-time rejection case for QueryParam.

**Tests:**
- `mcp_cookie_credential_authenticates`: Cookie-source route + `session=test-token-idp` cookie → tool ok + inbox 1; command `cargo test -p camel-test --test mcp_server_auth_test`; expected: pass.
- `mcp_named_header_credential_authenticates`: `x-api-key` source → same; command same; expected: pass.
- `mcp_queryparam_rejected_at_compile`: mcp route with QueryParam → staging error names source+transport before listener; command same; expected: pass.

**Acceptance:**
- `cargo test -p camel-test --test mcp_server_auth_test` passes.

- [x] 2.7

### Task 2.8: WS convergence to the kernel

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified — `set_security_context` ~1198, extraction ~419, `authenticate_bearer` ~445)

**Steps:**
1. `set_security_context`: extract plan; replace inline `extract_token_multi` + `authenticate_bearer` with: `AccessMode::Public` → skip extraction entirely (pass-through); otherwise extract per plan sources at the handshake → `kernel_authenticate(plan, providers, credentials)` → denial closes handshake (idiom unchanged).
2. PER-MESSAGE CARRIER: ws creates a FRESH Exchange per inbound message (`crates/components/camel-ws/src/lib.rs:709,753` — locate the two message-Exchange construction sites). The minted `AuthenticatedPrincipal` is stored in the connection state at handshake and `install_carrier` is called on EVERY message Exchange at both construction sites — without this, Task 2.9's dispatch enforcement denies valid authenticated traffic.
3. Keep ADR-0051 upgrade-debug query redaction helper (defense-in-depth; ws forbids QueryParam at compile since Task 1.8).

**Tests:**
- `ws_kernel_handshake_grants`: Authenticated plan + valid token upgrade → connection established + typed principal PRESERVED onto the SECOND message Exchange (send two messages; assert carrier present on both); command `cargo test -p camel-component-ws`; expected: pass.
- `ws_kernel_handshake_denies`: invalid token → close with denial; command same; expected: pass.
- `ws_public_route_passes_without_extraction`: Public-plan ws route + no credentials → connection established, route body runs (no extraction attempted, no denial); command `cargo test -p camel-component-ws`; expected: pass.
- Existing ws auth tests stay green.

**Acceptance:**
- `cargo test -p camel-component-ws` passes; `cargo clippy -p camel-component-ws -- -D warnings` exits 0.

- [x] 2.8

### Task 2.9: HTTP migration last + delete duplicate generations

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)
- `crates/components/camel-component-mcp/src/config.rs` (modified — remove `MissingSecurityPolicy` start-gate)
- `crates/components/camel-component-mcp/tests/server_config_test.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified — dispatch check from Task 2.2 deferral)
- `crates/camel-processor/src/security_policy_layer.rs` (modified — legacy branch deletion, step 3)

**Steps:**
1. camel-http server consumer implements `set_security_context`: capture plan; at request boundary: `AccessMode::Public` → pass-through (no extraction); otherwise extract per plan sources → `kernel_authenticate(plan, providers, credentials)` → `install_carrier` → deny 401 idiom on failure; before pipeline.
2. Now that every transport mints (2.1 grpc, 2.6 mcp, 2.8 ws, this task http), enforce the pre-pipeline dispatch check deferred from Task 2.2 in `route_controller_trait.rs`: non-Public plan → require the typed carrier on the Exchange; absent → deny with `CamelError::Unauthenticated` (transport maps to its idiom).
3. Delete residual raw-header extraction from the layer's call path in camel-processor (Task 1.7 made the layer dual-read; now DELETE the legacy fallback branch (carrier-only strict mode) and any dead extraction helpers no caller uses).
4. Remove MCP's component-local bind-gate (`McpError::MissingSecurityPolicy` consumer-start check, config.rs ~line 115) — superseded by kernel gate; update its tests: non-loopback TOML-less public server without ack now errors from the KERNEL gate naming `allow_public_exposure`.

**Tests:**
- `http_kernel_denies_without_credentials`: Authenticated http route, no token → 401 + inbox empty; command `cargo test -p camel-test --test kernel_fail_closed_test` (extend); expected: pass.
- `http_kernel_grants_with_token`: 2xx + inbox 1; command same; expected: pass.
- `dispatch_denies_nonpublic_without_carrier`: non-Public plan route + Exchange without carrier → pre-pipeline dispatch check denies before the pipeline runs; command `cargo test -p camel-core dispatch_enforcement`; expected: pass.
- `mcp_old_bind_gate_removed`: non-loopback public server, no ack → error names `allow_public_exposure` (kernel), NOT `MissingSecurityPolicy`; command `cargo test -p camel-component-mcp --test server_config_test`; expected: pass.

**Acceptance:**
- `cargo test -p camel-component-http -p camel-component-ws -p camel-component-grpc -p camel-component-mcp -p camel-test` exits 0.
- `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings` exits 0.

> Note (inter-phase review): every `kernel_fail_closed_test` command in 2.9/2.10 requires
> `--features integration-tests` (the file is `#![cfg(feature = "integration-tests")]`);
> without the flag the binary compiles empty and passes vacuously.

- [x] 2.9

### Task 2.10: REST block security_policy declaration

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified — `RouteDslRest.security_policy` field)
- `crates/camel-dsl/src/rest.rs` (modified — lowering copies policy; replaces hardcoded `security_policy: None` at line ~357)
- `crates/camel-dsl/src/yaml.rs` (modified — rest-block policy validation, mirrors the mcp/path rules)
- `docs/src/yaml-dsl/route-structure.md` (modified — rest: block security_policy key documented, anchoring the runnable example)
- `docs/src/components/http.md` (modified — pointer: rest:-declared endpoints are secured via the block policy)
- `examples/rest-crud/` (modified — the anchored example source: add a documented `security_policy` block variant under `examples/rest-crud/` config/docs, following that example's existing structure)

**Steps:**
1. `RouteDslRest` gains `#[serde(default)] pub security_policy: Option<RouteDslSecurityPolicy>` (same surface/validation as `RouteDslRoute.security_policy` — load-time validation mirrors the existing route-level rules in `yaml.rs`).
2. `lower_all_rest_to_routes` copies `rest.security_policy` into every lowered route's `security_policy` (replacing the hardcoded `None` at `rest.rs:~357`; mirror the `mcp.rs:135-160` copy pattern).
3. Plan compilation (Task 1.8) needs no change — it already reads `RouteDefinition.security_policy`; rest-lowered routes flow through it like any `http:` route.

**Tests:**
- `rest_block_policy_copies_to_lowered_routes`: setup `rest:` block with roles policy `provider: "idp-a"` → action lower + compile → assert every lowered route's plan is `Authorized` with `provider_ref Some("idp-a")`; command `cargo test -p camel-dsl rest`; expected: pass.
- `bare_rest_block_lowers_public`: setup `rest:` block without policy → lowered routes have `security_policy: None` (Public under the gate); command `cargo test -p camel-dsl rest`; expected: pass.
- `rest_e2e_secured_endpoint_denies_and_grants`: setup rest block with policy + fixture provider → no token → 401 + inbox empty; valid token → 2xx + inbox 1; command `cargo test -p camel-test --test kernel_fail_closed_test` (extend); expected: pass.

**Acceptance:**
- `cargo test -p camel-dsl rest` passes; `cargo test -p camel-test --test kernel_fail_closed_test` passes.
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.
- `cargo xtask lint-context-citations` exits 0.
- `cargo test -p camel-dsl --test documentation_examples` passes (existing doc-example gate; extend it if the rest security variant needs coverage).
- mdbook gate: `nix shell nixpkgs#mdbook -c mdbook build docs` exits 0 (the route-structure.md anchored example renders and references the examples/rest-crud source).

- [x] 2.10

## Phase 3: Audience enforcement

### Task 3.1: Audience/issuer in authn requests

**Files:**
- `crates/services/camel-auth/src/kernel.rs` (modified — Task 1.4 home)
- `crates/services/camel-auth/src/token_authenticator.rs` (modified)
- `crates/services/camel-auth/src/jwt.rs` (modified — per-request audience/issuer validation)

**Steps:**
1. `pub struct AuthnRequest<'a> { pub token: &'a str, pub audiences: &'a [String], pub accepted_issuers: &'a [String], pub transport: TransportId }` in `token_authenticator.rs`; new trait method `async fn authenticate(&self, req: AuthnRequest<'_>) -> Result<Principal, CamelError>` with default impl delegating to `authenticate_bearer(req.token)` (backward compatible).
2. Trait surgery (avoids blanket-impl overlap): `jwt.rs`'s blanket `impl<T: JwtValidator> TokenAuthenticator for T` stays, but the `JwtValidator` trait GAINS a method `async fn validate_signature(&self, token: &str) -> Result<claims, Error>` (signature-only: constructor keyset verification, NO issuer/audience check); `LocalJwtValidator` implements it by refactoring its `validate` into `validate_signature` + fixed-claims-check (validate keeps current behavior by composing both). The blanket `authenticate` default then composes: `validate_signature` + request issuer/audience sets when non-empty (REPLACEMENT checks), else delegate to `authenticate_bearer` (constructor-fixed behavior). No specialized overlapping impl exists. `IntrospectionAuthenticator` overrides `authenticate` directly enforcing the request sets against introspected claims (its own impl, no blanket involved).
3. `kernel_authenticate` builds `AuthnRequest` from the plan's `audience_binding` + `TransportId` + resolved provider's binding and calls `authenticate`. No provider sees another's outcome (stateless trait).

**Tests:**
- `issuer_not_accepted_rejects`: provider accepting `https://a` + token iss `https://b` → Unauthenticated; command `cargo test -p camel-auth token_authenticator`; expected: pass.
- `audience_mismatch_rejects`: request audiences `["api-a", "api-c"]` + token aud `api-b` → Unauthenticated (request-scoped check active even when constructor default differs); command same; expected: pass.
- `request_audience_overrides_constructor_default`: constructor-fixed audience `["api"]` + request audiences `["api-2"]` + token aud `api-2` → OK (fixed check bypassed on the authenticate path); empty request audiences → constructor behavior via delegation; command same; expected: pass.
- `default_delegation_backcompat`: authenticator implementing only `authenticate_bearer` → new `authenticate` delegates (same result); command same; expected: pass.
- `empty_issuers_accepts_any`: empty accepted_issuers → issuer check skipped; command same; expected: pass.

**Acceptance:**
- `cargo test -p camel-auth --lib` passes; clippy exits 0.

- [x] 3.1

### Task 3.2: Authn result cache (provider-local keys)

**Files:**
- `crates/services/camel-auth/src/authn_cache.rs` (new)
- `crates/services/camel-auth/src/lib.rs` (modified — module + re-export)
- `crates/services/camel-auth/src/kernel.rs` (modified — kernel_authenticate consults/inserts the cache)
- `crates/services/camel-auth/src/registry.rs` (modified — `ProviderRegistry.authn_cache` slot + accessor)
- `crates/camel-dsl/src/model.rs` (modified — registry construction accepts the cache)
- `crates/camel-cli/src/security.rs` (modified — constructs AuthnCache and attaches at wiring)

**Steps:**
1. NEW `pub struct AuthnCache` caching AUTHENTICATION results, backed by `std::sync::RwLock<HashMap<AuthnCacheKey, CacheEntry>>` where `CacheEntry { principal: AuthenticatedPrincipal, expires_at: Instant }` — matching permission_cache.rs's actual backend (RwLock<HashMap>, NOT moka; no new dependency). Ownership/injection: `ProviderRegistry` (Task 1.3) gains `authn_cache: Option<Arc<AuthnCache>>` + `pub fn authn_cache(&self) -> Option<&Arc<AuthnCache>>` accessor (Task 1.3 step 4 deferred this field here — this task adds it); Exact API + RETENTION: `SecurityCompileContext` gains `pub provider_reg: Option<Arc<camel_auth::ProviderRegistry>>` — camel-cli's wiring constructs the registry WITH the cache (`ProviderRegistry::with_authn_cache(cache)`) and STORES it on the context (the context travels to core; nothing is dropped). Core's `set_security_context` sites (Task 1.5 step 3) attach `ctx.provider_reg.clone().unwrap_or_else(|| Arc::new(ctx.provider_registry()))` — the cached registry when present. Affected call sites: camel-cli `security.rs` (construct + store), camel-dsl `model.rs` (field + attach), kernel.rs (reads `providers.authn_cache()`). Phase 1-2 behavior unchanged. Keyed by `pub struct AuthnCacheKey { provider: String, audiences: Vec<String>, issuers: Vec<String>, transport: TransportId, token_hash: String }` (token hashed; Debug redacts to `[hash]` following `ExtractedToken` precedent). It does NOT touch `CachingPermissionEvaluator` — that caches authorization `PermissionRequest` decisions, a different plane; the authn cache is additive.
2. `kernel_authenticate` consults `providers.authn_cache()` (when `Some`) before calling the provider and inserts `AuthenticatedPrincipal` results after (hit returns the cached minted principal; denial results are NOT cached).
3. `AuthnCache::new(opts)` with TTL following permission_cache's option shape, EXCEPT entry lifetime is `min(configured TTL, token exp - now)`: a cached authentication MUST NOT outlive its token — entries whose JWT `exp` passes are evicted/effective-expired (negative or zero remaining → not stored). The cached value records the token's exp at insert; hits re-check `now < exp`.

**Tests:**
- `cache_separates_providers`: two providers identical binding, same token → two distinct entries (cache len / hit counters); command `cargo test -p camel-auth authn_cache`; expected: pass.
- `cache_separates_transports`: same provider+token, Http and Ws → two entries; command same; expected: pass.
- `cache_key_debug_redacts_token`: Debug contains no token substring; command same; expected: pass.
- `denials_not_cached`: wrong token twice → provider called twice (counter), cache stays empty; command same; expected: pass.
- `expired_token_not_served_from_cache`: token with `exp` 1s ahead cached; sleep past exp (tokio time); present same token → `Unauthenticated` (hit re-checks exp — NOT served from cache) and the provider IS called again (2 total calls: initial + post-expiry re-auth attempt which then fails on exp); command same; expected: pass.

**Acceptance:**
- `cargo test -p camel-auth authn_cache` passes; `cargo test -p camel-auth permission_cache` UNCHANGED and green; clippy exits 0.

- [x] 3.2

### Task 3.3: Cross-transport substitution E2E tests

**Files:**
- `crates/camel-test/tests/audience_substitution_test.rs` (new)
- `docs/adr/0061-unified-transport-auth.md` (modified — Phase 3 enforcement-live note)

**Steps:**
1. Two-provider fixture sharing the SAME issuer URL (`https://shared`) and audience `["api"]` but trusting DIFFERENT signing keys (provider A's JWKS/keyset knows only key-A; provider B knows only key-B) — this isolates the per-provider key axis (disjoint-issuer fixtures would let issuer rejection mask missing key isolation). JWTs minted per key via rcgen/JWT helpers precedent in camel-test.
2. Cross-provider: token signed with key-A (trusted only by A) → route of B (same transport) → denied BY B's OWN signature verification; cache miss for B asserted.
3. Issuer isolation: token SIGNED BY B's trusted key but CARRYING issuer claim `https://attacker` (issuer is a payload claim attacker controls; signature proves B's key, issuer proves nothing) on a B route whose accepted_issuers excludes it → B denies (issuer-set check, not key check).
4. Same-audience cross-transport: same token+provider on http AND ws routes → both succeed; two cache entries.
5. Audience-distinguished: token aud `api-1` on a route declaring route-level `audiences: ["api-2"]` (same issuer/provider as another route accepting `api-1`) → denied, no cache reuse.

**Tests:**
- `cross_provider_substitution_rejected_e2e`: case 2; command `cargo test -p camel-test --test audience_substitution_test`; expected: pass.
- `issuer_isolation_e2e`: case 3; command same; expected: pass.
- `same_audience_cross_transport_isolated_cache_e2e`: case 4; command same; expected: pass.
- `audience_distinguished_no_cache_reuse_e2e`: case 5; command same; expected: pass.

**Acceptance:**
- `cargo test -p camel-test --test audience_substitution_test` passes.
- Phase 3 exit: `cargo test -p camel-auth -p camel-test` green; ADR-0061 Phase 3 note updated (enforcement live).

- [x] 3.3
