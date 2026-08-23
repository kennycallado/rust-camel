## MODIFIED Requirements

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

## ADDED Requirements

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
