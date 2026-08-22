# transport-convergence delta

## MODIFIED Requirements

### Requirement: WebSocket convergence

The `ws:` transport SHALL consume the compiled security plan through
the shared kernel as its ONLY authentication path: a route without a
compiled plan is Public pass-through (no extraction, no local policy
evaluation). The pre-kernel handshake arm (`extract_token_multi` +
`authenticate_bearer` + local `policy.evaluate`) and `LegacyPrincipal`
SHALL NOT exist. Query-parameter redaction (ADR-0051) is preserved.

#### Scenario: ws handshake authenticates via the kernel

- **GIVEN** a `ws:` route with an `Authenticated` plan and a client
  presenting a valid token at upgrade
- **WHEN** the handshake completes
- **THEN** the connection proceeds with the kernel-minted principal;
  an invalid token closes the handshake with the ws denial idiom

#### Scenario: ws route without a plan is public pass-through

- **GIVEN** a `ws:` route compiled without a security plan, a provider
  registry whose sole provider counts authentication calls, and a client
  presenting a valid credential at upgrade
- **WHEN** the handshake completes and a message is sent
- **THEN** the connection is established, the route body runs, and the
  provider's authentication call counter is zero (no extraction attempted)

### Requirement: gRPC interceptor security lifecycle

The gRPC interceptor SHALL consume the compiled security plan through
the shared kernel as its ONLY authentication path: construction
captures plan + provider registry before the request loop; a route
without a compiled plan is Public pass-through. The legacy
authentication arm (`GrpcPrincipal { provider_id: "legacy" }`,
`extract_principal`, and the `SecurityContext.authenticator` slot in
the dispatch entry) SHALL NOT exist.

#### Scenario: grpc kernel path is exclusive

- **GIVEN** a gRPC route with an `Authenticated` plan
- **WHEN** a request arrives without valid credentials
- **THEN** the interceptor denies with `Status::unauthenticated` and
  the route body never runs

#### Scenario: plan available at interceptor construction

- **GIVEN** a `grpc:` server route with an `Authenticated` plan
- **WHEN** the gRPC service is constructed
- **THEN** the interceptor holds the compiled plan and the first
  inbound request is authenticated via the core path before dispatch

### Requirement: Per-route dispatch enforcement

The core SHALL enforce each route's compiled access mode at dispatch
time. A late-registered route SHALL be revalidated atomically before
it becomes reachable: its plan SHALL classify AND the per-bind
exposure gate SHALL re-run for the target bind — a late `Public`
route on an already-bound non-loopback listener without
`allow_public_exposure` SHALL be rejected before serving any
request.
#### Scenario: grpc route without a plan is public pass-through

- **GIVEN** a gRPC route compiled without a security plan and a provider
  registry whose sole provider counts authentication calls
- **WHEN** a request arrives with a valid credential
- **THEN** the request dispatches and the provider's authentication
  call counter is zero (no extraction attempted at the transport)

#### Scenario: late route classified before reachable

- **GIVEN** an application with a running listener and a route
  registered after startup
- **WHEN** the route registers
- **THEN** its plan is compiled and gate-checked atomically; requests
  are refused until classification completes

#### Scenario: late public route on non-loopback bind rejected

- **GIVEN** a running non-loopback listener without
  `allow_public_exposure` and a late-registered `Public` route
  targeting it
- **WHEN** the route registers
- **THEN** registration is rejected with an error naming the bind
  and the route; the route never becomes reachable

## ADDED Requirements

### Requirement: Late registration gate integration coverage

The late-registration per-bind exposure gate SHALL be covered by an
integration test against a genuinely bound listener, not a constructed
listener state.

#### Scenario: late public route on a real non-loopback socket refused

- **GIVEN** a running HTTP listener bound to a non-loopback address
  without an exposure acknowledgement and a late `Public` route
  registered onto it
- **WHEN** registration completes
- **THEN** the registration is refused with an error naming the bind
  and the route, and nothing is served on the listener for that route

#### Scenario: late public route on a real loopback socket accepted

- **GIVEN** a running HTTP listener bound to a loopback address and a
  late `Public` route registered onto it
- **WHEN** registration completes
- **THEN** the route is reachable and serves requests
