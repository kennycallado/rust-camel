## ADDED Requirements

### Requirement: Route-declared credential sources

The route `security_policy` block SHALL accept an optional
`credential_sources` list. Each entry SHALL name one extraction source:
`authorization_header`, a query parameter, a cookie, or a custom header.
When the key is absent, the effective list SHALL be
`[authorization_header]` only (ADR-0033 fail-closed default). Unknown or
malformed source forms SHALL be rejected at route load time, not at request
time.

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
