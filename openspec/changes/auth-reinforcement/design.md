# Design: auth-reinforcement

## Context

r_glm review verdict: reinforce wiring, not redesign. Trait split
(`TokenAuthenticator` authn / `SecurityPolicy` authz, core types in camel-api) is
correct and preserved. ADRs: ADR-0051 (credential redaction), ADR-0059
(`credential_sources` + `extract_token_multi`), ADR-0033 (config stickiness
post-1.0 — the window justifying Phase 2 deletions). Phase 1 absorbs the amended
T1-T6 breakdown (e_opus) from
`docs/audits/rc-xb19-security-placeholder-resolution-analysis.md`. Composition
constraint with rc-w5bf: Phase 1 fail-closed semantics must survive a future
`${env:NAME:-default}` unification (keep `{{env:VAR:default}}` single-colon
canonical in Camel.toml; do not unify syntax in this change).

## Phase 1 — Config honesty & fail-closed

- **T1**: `resolve_placeholders` (camel-config config.rs) walks ALL
  `[security.*]` string leaves — credential leaves (`native.bearer_token`,
  `native.api_key`, `keycloak.client_secret`, `oidc.client_secret`),
  non-credential leaves (samples: `subject`, `issuer`, `server_url`, `realm`,
  `client_id`, `jwks_uri`, audiences; the walk is structural over every string
  leaf under `[security.*]`, not an enumerated allowlist) — and
  `[datasources.*]` (`db_url` plus recursion into `extra`, incl. SurrealDB
  `password`; these datasource leaves are treated as credential-class).
- **T3**: UNIFORM fail-closed semantics for every leaf the walk covers
  (`[security.*]` and `[datasources.*]` alike): unset env var without a
  default → `ConfigError` naming field and variable; default segment starting
  with `-` (the `{{env:X:-default}}` double-dash trap) → `ConfigError`;
  surviving `{{`/`${` literal → hard `ConfigError`, not a warning. Valid
  single-colon defaults (`{{env:X:fallback}}`, `X` unset) resolve to
  `fallback` normally. Optional-field tolerance is a non-security-subtree
  concern and stays outside this walk (rc-w5bf territory).
- **T4**: authenticator boundary guard — `NativeCredentialStore::try_new` and the
  keycloak/oidc paths reject any secret still containing `{{` or `${` markers.
- **T5**: docs — schema.md drops "Prefer a placeholder" recipes in favor of
  fail-closed notes; README syntax-boundary note; remove Keycloak-specific
  `jwks_uri` default under generic OIDC.
- **T6**: regression tests (executable, in tasks.md): rc-xb19 E2E repro
  (placeholder string NOT valid credential), unset-var fail-closed,
  dash-default rejection on a credential leaf AND on a non-security resolved
  field (proves resolver-wide guard), single-colon default accepted,
  successful non-credential security resolution (e.g. `keycloak.realm`),
  datasource `db_url` + SurrealDB `extra.password` resolution, marker-guard
  construction rejection.
- **OIDC wiring**: `resolve_authenticator` assembles `OidcSecurityConfig` +
  `RemoteJwksProvider` + `LocalJwtValidator` + `JsonPointerClaimsMapper` (~20
  lines, pieces exist). `jwks_uri` becomes required for oidc; JWKS fetch at
  startup, fail-closed `ConfigError` when unreachable. No silent `None`.
- **Error texts**: camel-dsl compile.rs names providers
  (`[security.keycloak|oidc|native]`); CLI stops silently fabricating Principal
  claims/audience. yaml-dsl `credential_sources` docs (rc-fxfl).

Phase exit: every documented security.*/datasources.* field resolves or errors
loudly; no silent OIDC.

## Phase 2 — Multi-credential native + dead-surface removal

- New `[[security.native.credentials]]` array — `{subject, secret_env | secret,
  roles, scopes}` — in `NativeAuthConfig`. `native_authenticator` folds all
  entries + scalar `bearer_token`/`api_key` (single-entry sugar, unchanged
  semantics) into one `NativeCredentialStore::try_new(vec![...])`. Env-based
  secrets (`NativeCredentialSecret::Env`) are wired HERE: the array shape and the
  env shape land together, keeping Phase 1 purely about existing scalar fields.
- DELETE (maintainer decision 2026-08-17): config fields `token_issuer`, `clients`
  (+ their `NativeIssuerConfig`/`NativeM2mClientConfig` structs); camel-auth
  `native_issuer.rs`, `native_client_store.rs`, `native_jwks.rs`,
  `ApiKeyAuthenticator`; camel-http `src/auth.rs` wrapper; schema.md rows
  rewritten to match reality. Scalar `api_key` REMAINS (wired, not deleted).
  `deny_unknown_fields` fails stale configs loudly — accepted pre-1.0. Future
  issuance demand: design fresh against Phase 3 architecture.
- E2E test: two principals with distinct roles both authenticate; unknown token
  rejected 401.

Phase exit: N-credential native auth end-to-end; dead surface gone.

## Phase 3 — Named providers

`SecurityCompileContext` holds authenticators keyed by provider name
(`HashMap<String, Arc<dyn TokenAuthenticator>>`) plus a deprecated single-provider
accessor. `security_policy` gains optional `provider`. Deterministic selection:
omitted `provider` selects the sole configured authenticator; when more than one
provider is configured, routes MUST name one — route load fails otherwise, naming
the available providers. Unknown provider name fails route load. Single-provider
configs behave exactly as Phase 2 (back-compat, no XOR error).
`resolve_authenticator` registers every configured provider instead of XOR-picking.

Phase exit: keycloak-humans + native-m2m configs work; single-provider unchanged.

## Phase 4 — gRPC credential sources

`camel-component-grpc/src/server.rs` (~438): replace `strip_prefix("Bearer ")`
with shared extraction over gRPC metadata honoring `sec_ctx.credential_sources`:
`authorization_header` maps to the `authorization` metadata key; `{header: {name}}`
maps to the custom metadata key of the same name. Transport-unsupported sources —
`query_param`, `cookie` (no HTTP semantics on gRPC metadata) — are rejected at
route load with an error naming the source (fail-closed at load, consistent with
the canonical requirement). Default with no declared sources remains
`authorization` Bearer. Closes rc-9f15; rc-fzgm stays a separate change.

## Architecture boundaries

- camel-api: `provider` handling lives in camel-dsl (route AST + declarative policy); camel-api changes are limited to the Task 2.3 doc-comment correction (stale `ApiKeyAuthenticator` reference) — no type changes.
- camel-auth: deletions plus the construction-time marker guard (T4 in
  `NativeCredentialStore::try_new`); credential lookup behavior unchanged.
- camel-dsl: `SecurityCompileContext` internal shape change; compile-time
  `require_authenticator` semantics preserved.
- Components: gRPC only. HTTP/WS/MCP already correct per review.

## Testing strategy

Per-phase executable tests in tasks.md. Regression sentinels must stay green:
ADR-0051 redaction, constant-time lookup, JWKS private-IP rejection. E2E for
multi-credential, OIDC-only, mixed providers, gRPC sources. Local gates:
`cargo test --workspace --lib` + hexagonal boundary test; full Docker suite
deferred to CI.
