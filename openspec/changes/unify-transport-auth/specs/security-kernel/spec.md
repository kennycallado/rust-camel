# Spec Delta: security-kernel

## ADDED Requirements

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
  `grpc:`, or `mcp:`)
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

At listener construction the system SHALL gate every network bind:
if the bind address is non-loopback and the bind's route set contains
any `Public` route, startup SHALL fail unless the operator has set
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

### Requirement: Transport credential capability validation

At plan compilation the system SHALL validate that the route's
`credential_sources` are carryable by the route's transport,
rejecting the route with an explicit error before listener
construction. Permitted sources by transport: `http:` —
AuthorizationHeader, Header, Cookie, QueryParam; `ws:` —
AuthorizationHeader, Header, Cookie; `grpc:` — AuthorizationHeader,
Header; `mcp:` — AuthorizationHeader, Header, Cookie. QueryParam is
forbidden on `ws:` and `mcp:`.

#### Scenario: uncarryable source rejected at compile time

- **GIVEN** an `mcp:` route declaring a `QueryParam` credential
  source
- **WHEN** plans compile
- **THEN** route staging fails with an error naming the source and
  the transport

### Requirement: Security config test fixture

The test workspace SHALL provide a `SecurityConfigFixture` builder
that constructs security configuration with deterministic secrets so
tests start under fail-closed `{{env:}}` placeholder enforcement. No
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
