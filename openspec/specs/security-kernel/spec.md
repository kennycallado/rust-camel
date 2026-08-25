# security-kernel Specification

## Purpose
TBD - created by archiving change unify-transport-auth. Update Purpose after archive.
## Requirements
### Requirement: Route security plan compilation

The system SHALL compile a `RouteSecurityPlan` for every server route
at staging time, before any listener binds. The plan SHALL carry an
`AccessMode` (`Public`, `Authenticated`, or `Authorized`), a
`provider_ref` (mandatory for `Authenticated`/`Authorized`, absent
for `Public`), the route's `credential_sources`, and an
`audience_binding` slot reserved for future enforcement.

A route that declares a security policy or provider SHALL be
classified `Authenticated` or `Authorized`; it SHALL NOT be
downgraded to `Public` by omission of wiring. A route that declares
neither is `Public` by default, subject to the per-bind exposure
gate.

Authorization-only policy forms (`ref`, `wasm`, `permission`) accept
no `provider` field and classify as `Authorized`: compilation fails
when no provider is configured; when exactly one provider exists it
is resolved and stored in the plan's `provider_ref`; compilation
fails when multiple providers exist and none is named for the route.

#### Scenario: every server route has a compiled plan

- **GIVEN** a route document with a server consumer (`http:`, `ws:`,
  `grpc:`, `mcp:`, or `wasm:` with an http-listener capability)
- **WHEN** the application stages routes
- **THEN** each server route carries a compiled `RouteSecurityPlan`
  and listener construction begins only after compilation succeeds

#### Scenario: sole provider satisfies an authorization-only form

- **GIVEN** a route using the `wasm` policy form and an application
  with exactly one named provider
- **WHEN** the plan compiles
- **THEN** the plan is `Authorized` with that provider resolved into
  the plan's `provider_ref`

#### Scenario: authorization-only form with zero providers fails

- **GIVEN** a route using the `wasm` policy form and an application
  with no configured provider
- **WHEN** the plan compiles
- **THEN** compilation fails naming the route and the missing
  provider configuration

#### Scenario: multiple providers and unnamed form fails compilation

- **GIVEN** a route using the `ref` policy form and an application
  with two named providers
- **WHEN** the plan compiles
- **THEN** compilation fails naming the route and requiring a
  provider selection

#### Scenario: declared security is never downgraded

- **GIVEN** a route declaring a security policy whose provider cannot
  be resolved
- **WHEN** the plan is compiled
- **THEN** compilation fails with an error naming the route and the
  unresolved provider; the route is not silently classified `Public`

### Requirement: Fail-closed principal enforcement

The authorization layer SHALL fail closed when a non-`Public` route
receives an Exchange without an authenticated principal. Additive
role grants SHALL NOT create authentication; explicit denial SHALL
override grants.

#### Scenario: missing wiring yields deny, not bypass

- **GIVEN** a transport that accepted a request without
  authenticating because its security wiring was not injected
- **WHEN** the Exchange reaches the authorization layer of an
  `Authenticated` route
- **THEN** the request is denied and the route body never executes

#### Scenario: additive grants never create authentication

- **GIVEN** an `Authorized` route whose policy grants a role to any
  principal and an Exchange with no authenticated principal
- **WHEN** authorization runs
- **THEN** the request is denied

### Requirement: Authenticated principal unforgeability

The system SHALL provide an `AuthenticatedPrincipal` constructible
only by the authentication path from a resolved provider. Exchange
properties SHALL remain advisory metadata; authorization decisions
SHALL be made against the typed principal, and a forged
authentication marker in Exchange properties SHALL NOT authorize a
request.

#### Scenario: policies receive the typed principal

- **GIVEN** an authenticated request on an `Authorized` route
- **WHEN** the policy evaluates
- **THEN** it receives the `AuthenticatedPrincipal` minted by the
  route's resolved provider

#### Scenario: spoofed exchange property does not authorize

- **GIVEN** an `Authenticated` route and a request whose Exchange
  carries properties mimicking an authenticated principal (set by
  the route body or an upstream processor) without provider minting
- **WHEN** authorization runs
- **THEN** the request is denied because no typed principal exists

#### Scenario: exchange-property principal evidence never authorizes

- **GIVEN** a secured route and a request whose only principal
  evidence is a raw `camel.auth.principal` Exchange property (the
  `trust_upstream_principal` flag is REMOVED — property evidence has
  no authorization path; stale configs fail at load)
- **WHEN** authorization runs
- **THEN** the request is denied

#### Scenario: stale trust_upstream_principal configuration fails loading

- **GIVEN** a route configuration declaring `trust_upstream_principal`
- **WHEN** the configuration loads
- **THEN** loading fails with an error naming the removed field

#### Scenario: principal construction outside the authn path fails

- **GIVEN** code attempting to construct an `AuthenticatedPrincipal`
  without a resolved provider's mint path
- **WHEN** it compiles
- **THEN** construction is not expressible (private constructor or
  provider-token sealed)

### Requirement: Per-bind exposure gate

At listener construction the system SHALL gate every network bind,
including the WASM source's host-granted http listener: if the bind
address is non-loopback and the bind's route set contains any `Public`
route, startup SHALL fail unless the operator has set
`allow_public_exposure = true` for that bind in TOML. The
acknowledgment SHALL be per-bind and SHALL NOT satisfy the gate for
misconfigured sibling routes: any route on the bind that declares
security but fails classification still blocks startup. An
acknowledged bind SHALL emit a `warn!` at startup naming the bind and
its `Public` route count; acknowledgment changes control flow, not
the audit signal. Loopback binds SHALL permit `Public` routes without
acknowledgment.

#### Scenario: non-loopback public bind refused without acknowledgment

- **GIVEN** a listener binding a non-loopback address whose route set
  contains a `Public` route and no `allow_public_exposure` setting
- **WHEN** listener construction runs the gate
- **THEN** startup fails with an error naming the bind and the Public
  routes

#### Scenario: acknowledged public bind warns at startup

- **GIVEN** a non-loopback bind with `allow_public_exposure = true`
  and two `Public` routes
- **WHEN** the application starts
- **THEN** startup succeeds and a `warn!` record names the bind and
  the count of exposed Public routes

#### Scenario: acknowledgment does not excuse misconfigured siblings

- **GIVEN** a non-loopback bind with `allow_public_exposure = true`,
  one `Public` route, and one route declaring a policy with an
  unresolvable provider
- **WHEN** the per-bind gate runs
- **THEN** startup fails because the declaring route never classified

#### Scenario: loopback public bind needs no acknowledgment

- **GIVEN** a loopback bind with a `Public` route
- **WHEN** the application starts
- **THEN** startup succeeds with no acknowledgment required

#### Scenario: wasm source bind runs the same gate

- **GIVEN** a `wasm:` source route whose resolved listener bind is a
  non-loopback address and whose plan is `Public`, with no
  `allow_public_exposure` setting for that bind
- **WHEN** the source consumer starts
- **THEN** startup fails with an error naming the bind and the route,
  and no TCP listener is bound

### Requirement: Transport credential capability validation

At plan compilation the system SHALL validate that the route's
`credential_sources` are carryable by the route's transport,
rejecting the route with an explicit error before listener
construction. Permitted sources by transport: `http:` —
AuthorizationHeader, Header, Cookie, QueryParam; `ws:` —
AuthorizationHeader, Header, Cookie; `grpc:` — AuthorizationHeader,
Header; `mcp:` — AuthorizationHeader, Header, Cookie; `wasm:` source
routes with an http-listener capability — AuthorizationHeader,
Header, Cookie, QueryParam. QueryParam is forbidden on `ws:` and
`mcp:`.

#### Scenario: uncarryable source rejected at compile time

- **GIVEN** an `mcp:` route declaring a `QueryParam` credential
  source
- **WHEN** plans compile
- **THEN** route staging fails with an error naming the source and
  the transport

### Requirement: Security config test fixture

The test workspace SHALL provide a `SecurityConfigFixture` builder
that constructs security configuration with deterministic secrets so
tests start under fail-closed `${env:}` placeholder enforcement. No
environment-optional escape hatch SHALL exist.

#### Scenario: tests start without real environment secrets

- **GIVEN** a test building security config via `SecurityConfigFixture`
- **WHEN** the application starts
- **THEN** placeholder resolution succeeds with deterministic test
  values and no environment variable is consulted

### Requirement: Audience reservation in kernel types

Provider registry entries and `RouteSecurityPlan` SHALL reserve an
`AudienceBinding` field (accepted issuer, audience) in Phase 1. The
field SHALL be populated from configuration where present and SHALL
participate in authentication cache keys no later than Phase 3
enforcement.

#### Scenario: audience present in configuration reaches the plan

- **GIVEN** a named provider entry configured with issuer and
  audience
- **WHEN** a route referencing that provider compiles its plan
- **THEN** the plan's `audience_binding` carries the configured
  values

### Requirement: WASM source boundary authentication

The WASM source host listener SHALL authenticate inbound requests at
the host edge before forwarding them to the guest: for non-`Public`
plans the host SHALL extract credentials from the request's declared
`credential_sources`, authenticate via the auth kernel, and render
authentication denial as HTTP 401 (the host edge performs
authentication only; authorization remains owned by the pipeline's
strict dispatch and policy layers downstream) without forwarding the
request to the guest and without reading its body. For authenticated
requests the host SHALL thread the minted `AuthenticatedPrincipal` to
Exchange assembly and install the typed carrier on the native Exchange
before it enters the route pipeline. The guest SHALL NOT observe
whether authentication ran, and a denied request SHALL NOT be enqueued
to the guest request channel nor have its body consumed. Credential
extraction SHALL honor each permitted `CredentialSource` for `wasm:`
routes (AuthorizationHeader, Header, Cookie, QueryParam).

#### Scenario: unauthenticated request denied before the guest

- **GIVEN** an `Authenticated` `wasm:` source route and an inbound
  request with no credential
- **WHEN** the request reaches the host listener
- **THEN** the host responds 401, the guest is not woken, and no
  channel item is sent

#### Scenario: each permitted credential source authenticates

- **GIVEN** an `Authenticated` `wasm:` source route whose plan
  permits a credential source (AuthorizationHeader, Header, Cookie,
  or QueryParam, one variant per case)
- **WHEN** an inbound request carries a valid credential in that
  source
- **THEN** the kernel authenticates and the pipeline Exchange carries
  the typed carrier

#### Scenario: valid credential installs the typed carrier

- **GIVEN** an `Authenticated` `wasm:` source route and an inbound
  request carrying a valid credential for the route's provider
- **WHEN** the guest submits the exchange
- **THEN** the native Exchange entering the pipeline carries the
  kernel-minted carrier and `read_carrier` succeeds downstream

#### Scenario: provider substitution fails

- **GIVEN** an `Authenticated` `wasm:` source route bound to provider
  B and an inbound request carrying a valid credential minted by
  provider A
- **WHEN** the host authenticates
- **THEN** the request is denied and the guest never sees it

#### Scenario: missing kernel wiring fails closed

- **GIVEN** a `wasm:` source route classified non-`Public` whose
  security wiring was never injected
- **WHEN** an inbound request arrives
- **THEN** the request is denied (no kernel means no pass-through
  for non-`Public` plans)

### Requirement: Typed auth-provider unavailability propagation

The system SHALL propagate auth-provider unavailability (JWKS, introspection, or token
endpoint unreachable/failing) as a typed `CamelError::AuthProviderUnavailable` variant
across the kernel boundary, and the WebSocket and gRPC transports SHALL select the denial
status by matching that variant — never by inspecting the error message text.

#### Scenario: Typed mapping at the kernel boundary

- **GIVEN** an `AuthError::ProviderUnavailable(detail)` raised inside camel-auth
- **WHEN** it is converted into `CamelError` via the `From` impl
- **THEN** the result is `CamelError::AuthProviderUnavailable` carrying the detail, not a
  `ProcessorError` with an embedded magic string

#### Scenario: WebSocket denial maps to 503 by variant

- **GIVEN** WebSocket handshake authentication fails with `CamelError::AuthProviderUnavailable`
- **WHEN** the upgrade is rejected by `ws_upgrade_auth_error`
- **THEN** the HTTP response status is 503 Service Unavailable, selected by variant match
  with no string inspection of the error message

#### Scenario: gRPC denial maps to UNAVAILABLE by variant

- **GIVEN** gRPC per-request authentication fails with `CamelError::AuthProviderUnavailable`
- **WHEN** the error is mapped by `auth_error_to_status`
- **THEN** the `tonic::Status` code is `unavailable`, selected by variant match with no
  string inspection of the error message

#### Scenario: Wording independence

- **GIVEN** an `AuthProviderUnavailable` whose detail text is arbitrary (including text
  that does not contain any fixed marker substring)
- **WHEN** the error reaches the WebSocket or gRPC transport's denial mapping
- **THEN** the status is still 503 / UNAVAILABLE — a wording change alone can never
  degrade the denial to 500 / INTERNAL

#### Scenario: Error-handler catch compatibility

- **GIVEN** a `CamelError::AuthProviderUnavailable` flowing through route error handling
- **WHEN** `doTry` catch-by-variant matching consults `CamelError::variant_name()` and
  the error is classified via `CamelError::classify()`
- **THEN** `variant_name()` reports `"ProcessorError"` (same aliasing as
  `ProcessorErrorWithSource`) and `classify()` reports `"processor"`, so existing
  ProcessorError catch handlers keep matching exactly as before the change

#### Scenario: Other processor errors keep the internal status

- **GIVEN** authentication fails with a generic `CamelError::ProcessorError` unrelated to
  provider availability
- **WHEN** the error reaches the WebSocket or gRPC transport's denial mapping
- **THEN** the status is 500 Internal Server Error / INTERNAL as before

