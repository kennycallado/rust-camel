# security Specification

## Purpose
TBD - created by archiving change audit-fix-principal-debug-redact. Update Purpose after archive.
## Requirements
### Requirement: Principal Debug redaction of untrusted claims

The system SHALL NOT render the raw `claims` payload of `Principal` through the
`Debug` formatting path. The `claims` field SHALL render as the literal
`[REDACTED]` under any `{:?}` formatting. The retained fields (`subject`,
`issuer`, `audience`, `scopes`, `roles`) are an intentional operator-visible
allowlist of identity descriptors; only the free-form untrusted `claims` blob is
suppressed, because claims carry provider-mapped identity data (ADR-0032) that
may include personally identifiable information.

Note on achievable contract: a claim value may coincidentally equal a retained
descriptor (for example a `sub` claim equal to `subject`). The contract therefore
targets the claims payload itself: the `claims` field renders as `[REDACTED]`,
and any value that appears ONLY in `claims` (not in a retained descriptor) is
absent from the Debug output. It does not require the absence of strings that
also appear in the retained allowlist.

#### Scenario: Debug output redacts claims payload (compact formatting)

- **GIVEN** a `Principal` whose `claims` is a JSON object containing a sentinel
  key `piid` with value `SENTINEL_CLAIM_VALUE_9kq2` (a value that appears in no
  retained descriptor), and whose retained fields are populated
- **WHEN** the principal is formatted with `format!("{principal:?}")` (compact
  Debug)
- **THEN** the formatted string contains `claims: "[REDACTED]"`, does NOT
  contain `SENTINEL_CLAIM_VALUE_9kq2`, and DOES contain the retained descriptor
  values (`subject`, `issuer`, `audience`, `scopes`, `roles`)

#### Scenario: Debug output redacts claims payload (pretty formatting)

- **GIVEN** the same `Principal` as above
- **WHEN** the principal is formatted with `format!("{principal:#?}")` (pretty
  Debug)
- **THEN** the formatted string contains `[REDACTED]` for the `claims` field and
  does NOT contain `SENTINEL_CLAIM_VALUE_9kq2`

#### Scenario: serialization is unaffected

- **GIVEN** a `Principal` with populated `claims`
- **WHEN** the principal is serialized with serde (`Serialize`)
- **THEN** the serialized output contains the full `claims` value, because
  claims are the principal's legitimate data payload crossing the auth boundary

### Requirement: Route-declared credential sources

The route `security_policy` block SHALL accept an optional
`credential_sources` list. Each entry SHALL name one extraction source:
`authorization_header`, a query parameter, a cookie, or a custom header.
When the key is absent, the effective list SHALL be
`[authorization_header]` only (ADR-0033 fail-closed default). Unknown or
malformed source forms SHALL be rejected at route load time, not at request
time. Every transport consumer (HTTP, WS, MCP, gRPC) SHALL extract tokens
according to this declaration via the shared extraction helper. Sources the
transport cannot carry SHALL be rejected at route load time with an error
naming the source (gRPC: `query_param` and `cookie` are not carryable;
`authorization_header` maps to the `authorization` metadata key and
`{header: {name}}` to the same-named metadata key).

#### Scenario: default is header-only when key absent

- **GIVEN** a `from: http://` route with `security_policy: {roles: [...]}`
  and no `credential_sources` key
- **WHEN** the route is loaded and a request arrives with a valid Bearer
  token in the `Authorization` header
- **THEN** authentication succeeds exactly as before the change

#### Scenario: absent key rejects cookie-transported token

- **GIVEN** the same route without `credential_sources`
- **WHEN** a request arrives with no `Authorization` header and a valid token
  in a cookie
- **THEN** authentication fails with `Unauthenticated` (HTTP 401)

#### Scenario: malformed source form rejected at load

- **GIVEN** a route declaring `credential_sources: [{cookie: {}}]` (missing
  `name`)
- **WHEN** the route is loaded
- **THEN** loading fails with a parse error naming the field

#### Scenario: empty list and empty names rejected at load

- **GIVEN** routes declaring `credential_sources: []`, or
  `[{query_param: {param: ""}}]`, or `[{cookie: {name: ""}}]`
- **WHEN** each route is loaded
- **THEN** loading fails with an error naming the invalid entry

#### Scenario: invalid custom-header name rejected at load

- **GIVEN** a route declaring `credential_sources: [{header: {name:
  "Bad Header:"}}]` (not a valid HTTP header token)
- **WHEN** the route is loaded
- **THEN** loading fails with an error naming the header source

#### Scenario: sources on non-authenticating policy variant rejected at load

- **GIVEN** a route whose `security_policy` block declares only `ref`, only
  `wasm`, or only `permission` — no `roles` and no `scopes` — with a
  `credential_sources` key present
- **WHEN** the route is loaded
- **THEN** loading fails with an error naming the policy block, because
  `credential_sources` is supported only on the authenticating variants
  (`roles`, `scopes`)

#### Scenario: custom header source authenticates API key

- **GIVEN** a route declaring `credential_sources:
  [{header: {name: X-API-Key}}]` with the native credential store holding the
  key
- **WHEN** a request arrives with `X-API-Key: <valid key>` and no
  `Authorization` header
- **THEN** authentication succeeds and maps the stored principal

#### Scenario: gRPC honors declared custom-header source

- **GIVEN** a `from: grpc://` route declaring
  `credential_sources: [{header: {name: X-API-Key}}]` with the native
  credential store holding the key
- **WHEN** a gRPC request carries the token in the `x-api-key` metadata key
- **THEN** the request authenticates against the configured provider,
  matching HTTP behavior for the same declaration

#### Scenario: gRPC default remains authorization bearer

- **GIVEN** a `from: grpc://` route with no declared `credential_sources`
- **WHEN** a request presents `authorization: Bearer <token>` metadata
- **THEN** the request authenticates (existing default preserved)

#### Scenario: uncarryable source on gRPC rejected at load

- **GIVEN** a `from: grpc://` route declaring
  `credential_sources: [{query_param: {param: ticket}}]` or
  `[{cookie: {name: session}}]`
- **WHEN** the route is loaded
- **THEN** loading fails with an error naming the source and the transport,
  because gRPC metadata cannot carry query parameters or cookies

### Requirement: Multi-source extraction precedence

When multiple credential sources are declared, extraction SHALL try them in
declared order and use the first source that yields a value. A miss on every
declared source SHALL produce `Unauthenticated`. A source miss SHALL NOT
trigger the `trust_upstream_principal` path; that path stays gated on the
explicit flag.

#### Scenario: first-match-wins order

- **GIVEN** a route declaring `[{authorization_header}, {cookie:
  {name: session}}]`
- **WHEN** a request carries both a valid Bearer header and a session cookie
  holding a different (invalid) token
- **THEN** the `Authorization` header value is used and authentication
  succeeds

#### Scenario: fallback to second source

- **GIVEN** the same route declaration
- **WHEN** a request carries no `Authorization` header and a valid token in
  the `session` cookie
- **THEN** the cookie value is used and authentication succeeds

#### Scenario: all sources miss maps to 401

- **GIVEN** a route declaring `[{authorization_header}, {cookie:
  {name: session}}]`
- **WHEN** a request carries neither an `Authorization` header nor a
  `session` cookie, and `trust_upstream_principal` is not set
- **THEN** the response is HTTP 401, not 500

#### Scenario: source miss with preloaded principal and trust disabled

- **GIVEN** a route whose declared sources all miss, where the exchange
  carries a preloaded principal (component-owned extraction path), and
  `trust_upstream_principal: false` or absent
- **WHEN** the security policy evaluates
- **THEN** authentication fails with `Unauthenticated` (HTTP 401); the
  preloaded principal is not consulted

#### Scenario: explicit trust permits the fallback

- **GIVEN** the same route with `trust_upstream_principal: true` set
  explicitly
- **WHEN** the declared sources all miss and the exchange carries a
  preloaded principal
- **THEN** the policy evaluates against the preloaded principal

#### Scenario: WS activation does not silently enable trust

- **GIVEN** a `from: ws://` route with declared `credential_sources` and no
  `trust_upstream_principal` key
- **WHEN** a connection presents no credential in any declared source
- **THEN** the connection is rejected with 401 before policy evaluation;
  the activation mechanism itself never sets the trust flag

### Requirement: Credential redaction for non-header sources

Diagnostic output on the HTTP consumer path SHALL NOT render credential
values extracted from query parameters, cookies, or custom headers,
following ADR-0051. The contract covers every diagnostic boundary the
request-handling path emits — error-context logs and error replies — and any
access or diagnostic log added later inherits the same obligation.

#### Scenario: query-parameter token absent from diagnostic output

- **GIVEN** a route declaring `credential_sources: [{query_param:
  {param: token}}]`
- **WHEN** a request arrives with `?token=SENTINEL_QRY_42` and
  authentication fails
- **THEN** no tracing record emitted while handling the request contains
  `SENTINEL_QRY_42`, and the 401 reply body does not contain it

#### Scenario: cookie value absent from diagnostic output

- **GIVEN** a route declaring `credential_sources: [{cookie: {name:
  session}}]`
- **WHEN** a request arrives with `Cookie: session=SENTINEL_CKY_7` and
  authentication fails
- **THEN** no tracing record emitted while handling the request contains
  `SENTINEL_CKY_7`, and the 401 reply body does not contain it

#### Scenario: custom-header token absent from diagnostic output

- **GIVEN** a route declaring `credential_sources: [{header: {name:
  X-API-Key}}]`
- **WHEN** a request arrives with `X-API-Key: SENTINEL_HDR_9` and
  authentication fails
- **THEN** no tracing record emitted while handling the request contains
  `SENTINEL_HDR_9`, and the 401 reply body does not contain it

#### Scenario: credential value absent from the 401 error path

- **GIVEN** a route declaring any non-header source
- **WHEN** a request presents an invalid credential value in that source and
  authentication fails
- **THEN** neither the error reply body nor the error-context log contains
  the presented credential value

### Requirement: Constant-time comparison for every source

Token values extracted from any declared credential source SHALL be verified
through the shared constant-time credential store comparison. No source
SHALL introduce a comparison path with an early exit on length or content
mismatch.

#### Scenario: prefix credential does not authenticate

- **GIVEN** the native store holds credential `SENTINEL_FULL_9kq2` and a
  request presents `SENTINEL_FULL` (a strict prefix) in any declared source
- **WHEN** authentication runs
- **THEN** it fails with `Unauthenticated`, through the same store lookup
  used by every other source

### Requirement: WS parity for declared sources

The WS consumer SHALL resolve the same route-declared `credential_sources`
when authenticating inbound connections. When no sources are declared, WS
behavior SHALL remain unchanged (header-only default).

#### Scenario: WS authenticates cookie source with explicit trust

- **GIVEN** a `from: ws://` route with `security_policy` (`roles`),
  `trust_upstream_principal: true` declared on the route, and
  `credential_sources: [{cookie: {name: session}}]`
- **WHEN** a client connects presenting the `session` cookie with a valid
  token and no `Authorization` header
- **THEN** the connection authenticates and the policy evaluates against the
  principal the component verified from the cookie

#### Scenario: WS without the explicit flag rejects even valid tokens

- **GIVEN** a `from: ws://` route with `security_policy` (`roles`) and no
  `trust_upstream_principal` key
- **WHEN** a client connects with a valid Bearer token
- **THEN** the connection is rejected as unauthenticated (fail-closed;
  resolving declared or default sources never implies the flag)

#### Scenario: WS default unchanged

- **GIVEN** a `from: ws://` route with `security_policy` and no
  `credential_sources`
- **WHEN** a client connects as before the change
- **THEN** authentication behavior is identical to the pre-change default

### Requirement: Native multi-credential authentication

The runtime SHALL accept multiple native credentials declared as
`[[security.native.credentials]]` entries (each with `subject`, an env-based or
plaintext secret, `roles`, `scopes`) and authenticate each declared principal
through the existing `NativeCredentialStore`. Scalar `bearer_token` and
`api_key` fields SHALL keep working as single-entry equivalents.

#### Scenario: Two principals both authenticate

- **GIVEN** a Camel.toml with two `[[security.native.credentials]]` entries (subject `ops` with role `admin`, subject `svc` with role `service`) and a route requiring role `admin`
- **WHEN** a request presents the `ops` credential and another presents the `svc` credential
- **THEN** `ops` authenticates and passes the role check, `svc` authenticates but fails the role check with 403, and an unknown token value is rejected with 401

#### Scenario: api_key-only config starts

- **GIVEN** a Camel.toml with `[security.native]` declaring a credential consumed only via a non-bearer `credential_sources` entry (e.g. `{header: {name: X-API-Key}}`)
- **WHEN** the CLI starts
- **THEN** startup succeeds without requiring `bearer_token`, and the route enforces against the declared credential store

#### Scenario: Legacy scalar bearer_token still works

- **GIVEN** a Camel.toml with `[security.native]` using only scalar `bearer_token` + `subject` + `roles`
- **WHEN** the CLI starts and a request presents that token
- **THEN** behavior is identical to v0.29.0 single-credential enforcement

### Requirement: Native secrets from environment variables

Credentials declared via `[[security.native.credentials]]` with `secret_env`
SHALL be constructed using the store's env-var secret variant, failing closed
with a `ConfigError` when the variable is unset or empty.

#### Scenario: Env-based credential resolves at startup

- **GIVEN** `[[security.native.credentials]]` with `secret_env = "AUTH_SVC_TOKEN"` and `AUTH_SVC_TOKEN` set
- **WHEN** the CLI starts
- **THEN** the credential authenticates via the environment value, and no plaintext token appears in the Camel.toml

#### Scenario: Missing env var fails closed

- **GIVEN** `secret_env = "AUTH_SVC_TOKEN"` with `AUTH_SVC_TOKEN` unset
- **WHEN** the CLI starts
- **THEN** startup fails with a `ConfigError` naming the variable

### Requirement: Security credential placeholder resolution

ALL `[security.*]` string leaves SHALL be resolved by the config placeholder
resolver — credential leaves (`native.bearer_token`, `native.api_key`,
`keycloak.client_secret`, `oidc.client_secret`) and non-credential leaves
(samples: `subject`, `issuer`, `keycloak.realm`, `oidc.jwks_uri`; the walk is
structural over every string leaf under `[security.*]`, not an enumerated
allowlist) — plus `[datasources.*]` connection leaves (`db_url`, SurrealDB
`password` under `extra`), which are treated as credential-class. EVERY leaf
covered by this walk SHALL share uniform fail-closed semantics: an unset env
var without a default yields `ConfigError`; a placeholder whose default
segment starts with `-` (the `{{env:X:-default}}` double-dash trap) yields
`ConfigError`; a surviving `{{` or `${` literal yields `ConfigError`. A valid
single-colon default (`{{env:X:fallback}}`) SHALL resolve to `fallback` when
`X` is unset. A literal marker SHALL never be accepted as a credential value,
at the resolver or at the authenticator boundary.

#### Scenario: Placeholder resolves to real secret

- **GIVEN** `[security.native]` with `bearer_token = "{{env:AUTH_TOKEN}}"` and `AUTH_TOKEN` set
- **WHEN** the CLI starts
- **THEN** the resolved environment value is the accepted credential, and the placeholder string itself is NOT a valid credential

#### Scenario: Unset env var on a covered leaf fails closed

- **GIVEN** `bearer_token = "{{env:AUTH_TOKEN}}"` (credential leaf) or `db_url = "{{env:DB_URL}}"` (datasource credential-class leaf), env var unset
- **WHEN** the CLI starts
- **THEN** startup fails with a `ConfigError` naming the field and the variable

#### Scenario: Single-colon default resolves normally

- **GIVEN** `bearer_token = "{{env:AUTH_TOKEN:fallback-secret}}"` with `AUTH_TOKEN` unset
- **WHEN** the CLI starts
- **THEN** the credential resolves to `fallback-secret` with no error or warning about the default syntax

#### Scenario: Dash-prefixed default fails closed on any covered leaf

- **GIVEN** any leaf the walk covers — security credential, non-credential security, or datasource — carrying `{{env:X:-changeme}}` with `X` unset
- **WHEN** the CLI starts
- **THEN** startup fails with `ConfigError` — the manufactured `-`-prefixed value is never used

#### Scenario: Non-credential security leaf resolves

- **GIVEN** `[security.keycloak]` with `realm = "{{env:KC_REALM:main}}"` and `KC_REALM` unset
- **WHEN** the CLI starts
- **THEN** the realm resolves to `main` with no error

#### Scenario: Datasource leaves resolve

- **GIVEN** `[datasources.main]` with `db_url = "{{env:DB_URL}}"` and a SurrealDB `extra.password = "{{env:SURREAL_PASS}}"`, both env vars set
- **WHEN** the CLI starts
- **THEN** both leaves resolve to the environment values

#### Scenario: Authenticator boundary guard rejects marker secrets

- **GIVEN** any code path that constructs a native credential store or keycloak/oidc authenticator from a secret still containing `{{` or `${`
- **WHEN** the store or authenticator is constructed
- **THEN** construction fails with a `ConfigError` naming the unresolved marker

### Requirement: OIDC-only configuration registers an authenticator or fails

An `[security.oidc]`-only configuration with a valid `issuer` and reachable
`jwks_uri` SHALL register a functioning JWT authenticator (issuer validation via
`LocalJwtValidator`). Malformed configuration (missing `jwks_uri`) or an
unreachable JWKS endpoint at startup SHALL fail with an explicit `ConfigError`.
Silent resolution to no authenticator is not a permitted outcome.

#### Scenario: Valid OIDC-only config protects routes

- **GIVEN** a Camel.toml with only `[security.oidc]` (issuer, jwks_uri, audience) and a route declaring `security_policy`
- **WHEN** the CLI starts and a request presents a valid JWT for that issuer
- **THEN** the request authenticates and reaches the route

#### Scenario: Missing jwks_uri fails closed

- **GIVEN** `[security.oidc]` without `jwks_uri`
- **WHEN** the CLI starts
- **THEN** startup fails with a `ConfigError` naming `security.oidc.jwks_uri` (no implicit Keycloak-style default)

#### Scenario: Unreachable JWKS at startup fails closed

- **GIVEN** `[security.oidc]` with a `jwks_uri` that does not respond at startup
- **WHEN** the CLI starts
- **THEN** startup fails with a `ConfigError` describing the unreachable JWKS endpoint

### Requirement: Named authentication providers

`SecurityCompileContext` SHALL hold authenticators keyed by provider name
(`keycloak`, `oidc`, `native`). Route `security_policy` MAY declare a `provider`
name. When exactly one provider is configured, an omitted `provider` SHALL select
it (back-compatible). When more than one provider is configured, a route SHALL
name one; route load SHALL fail otherwise, naming the available providers. An
unknown `provider` name SHALL fail route load.

#### Scenario: Mixed providers with explicit selection

- **GIVEN** a Camel.toml with both `[security.keycloak]` (human JWTs) and `[[security.native.credentials]]` (m2m), and two routes declaring `provider: keycloak` and `provider: native` respectively
- **WHEN** requests present a valid Keycloak JWT and a valid native credential
- **THEN** each route authenticates against its named provider

#### Scenario: Sole provider works without a provider key

- **GIVEN** a single-provider Camel.toml and routes without `provider` in `security_policy`
- **WHEN** the CLI starts
- **THEN** routes authenticate against the sole provider, exactly as pre-change single-provider configs

#### Scenario: Ambiguous provider fails at load

- **GIVEN** a multi-provider Camel.toml and a route whose `security_policy` omits `provider`
- **WHEN** the route is loaded
- **THEN** loading fails with an error naming the configured providers and the missing `provider` key

#### Scenario: Unknown provider fails at load

- **GIVEN** a route declaring `provider: saml` when no such provider is configured
- **WHEN** the route is loaded
- **THEN** loading fails with an error naming the unknown provider and the available ones

