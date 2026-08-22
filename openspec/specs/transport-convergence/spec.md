# transport-convergence Specification

## Purpose
TBD - created by archiving change unify-transport-auth. Update Purpose after archive.
## Requirements
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

### Requirement: MCP listener ownership

Amending ADR-0060 Rule 3 (TOML-owned runtime): the `mcp:` DSL block
SHALL own its listener configuration the way `rest:` does — `bind`,
TLS, and catalog caps declared in the DSL SHALL flow to the runtime.
TOML `mcp.servers.<name>` SHALL remain the source for keys with no
DSL counterpart (e.g. `allowed_hosts`). Where both declare the same
key with different values, startup SHALL fail with a hard error
naming both sources. No DSL field SHALL be silently ignored. The
superseding ADR-0060 Rule 8 bind gate (presence-only
`security_policy`) is itself superseded by the kernel per-bind
exposure gate with uniform semantics for all transports; the
route-level enforcement gate is preserved unchanged.

#### Scenario: DSL TLS reaches the listener

- **GIVEN** an `mcp:` block declaring TLS with cert and key paths
- **WHEN** the server starts
- **THEN** the listener presents TLS from those paths

#### Scenario: TOML/DSL conflict fails startup

- **GIVEN** an `mcp:` block declaring `bind 127.0.0.1:9100` while
  TOML declares `bind 127.0.0.1:9200` for the same server name
- **WHEN** the application starts
- **THEN** startup fails with an error naming both sources

### Requirement: Typed MCP TLS configuration

`McpServerConfig` SHALL carry TLS as a typed `McpTlsConfig` with
`cert_path` and `key_path` (PEM files), not an untyped JSON value.
Invalid or empty paths SHALL be rejected at configuration load.

#### Scenario: malformed TLS paths rejected at load

- **GIVEN** TOML configuration with an empty `cert_path`
- **WHEN** configuration loads
- **THEN** loading fails with an error naming the field

### Requirement: Repeated MCP header normalization

The MCP server SHALL normalize repeated HTTP headers by a closed,
deterministic rule. `Cookie` joins with `; ` into one value.
`Authorization` and `Proxy-Authorization` SHALL NOT be joined: the
first value is used and a `warn!` names the header. Every other
repeated header joins with `, ` into one value — combinability is not
decided per header at runtime; the join is uniform so behavior is
auditable and testable.

#### Scenario: multiple cookie headers join deterministically

- **GIVEN** an inbound MCP request with two `Cookie` headers
- **WHEN** request headers are normalized
- **THEN** credential extraction sees one `Cookie` header with both
  pairs joined by `; `

#### Scenario: repeated authorization header does not join

- **GIVEN** an inbound MCP request with two `Authorization` headers
- **WHEN** request headers are normalized
- **THEN** the first value is used, a `warn!` names the header, and
  the request proceeds with one credential

#### Scenario: arbitrary repeated header joins with comma

- **GIVEN** an inbound MCP request with two `X-Trace-Id` headers
- **WHEN** request headers are normalized
- **THEN** one header remains with both values joined by `, `

### Requirement: MCP credential source coverage

MCP server routes SHALL be covered by tests exercising each permitted
credential source (`AuthorizationHeader`, `Header`, `Cookie`) against
real authenticated requests.

#### Scenario: cookie-sourced credential authenticates an mcp route

- **GIVEN** an `Authorized` mcp route declaring a Cookie credential
  source and a static provider
- **WHEN** a request carries the session cookie
- **THEN** the route body executes with the authenticated principal

#### Scenario: named-header credential authenticates an mcp route

- **GIVEN** an `Authorized` mcp route declaring a named `Header`
  credential source and a static provider
- **WHEN** a request carries the token in that header
- **THEN** the route body executes with the authenticated principal

### Requirement: REST block security declaration

The `rest:` DSL block SHALL accept an optional `security_policy`
with the same surface and load-time validation as `RouteDslRoute`,
and lowering SHALL copy it to every lowered `http:` route (mirroring
`mcp:` block lowering). A `rest:` endpoint declaring security
classifies through plan compilation like any `http:` route.

#### Scenario: rest block policy reaches lowered routes

- **GIVEN** a `rest:` block declaring a roles policy with provider
  `idp-a`
- **WHEN** the block lowers and plans compile
- **THEN** every lowered `http:` route is `Authorized` with
  `provider_ref "idp-a"`

#### Scenario: bare rest block stays public under the gate

- **GIVEN** a `rest:` block without `security_policy`
- **WHEN** plans compile on a non-loopback bind
- **THEN** the lowered routes are `Public` and the per-bind exposure
  gate governs them

### Requirement: HTTP security context migration

`camel-http` SHALL consume the compiled security plan through the
same injection path as other transports, deleting its
generation-local authentication wiring. Migration SHALL be last among
transports.

#### Scenario: http route denies without principal under the kernel

- **GIVEN** an `Authenticated` http route and a request without valid
  credentials after migration
- **WHEN** the kernel enforces the plan
- **THEN** the request is denied before the route body executes

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

