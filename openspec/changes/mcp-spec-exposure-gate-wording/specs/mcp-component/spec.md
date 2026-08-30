## MODIFIED Requirements

### Requirement: MCP server fail-closed authentication

The MCP server endpoint SHALL enforce the ADR-0061 per-bind exposure
gate at consumer start. A loopback bind SHALL start without a
`security_policy`. A non-loopback bind SHALL expose its routes only
when a route declares a `security_policy` or the operator acknowledges
public exposure for that bind address (`allow_public_exposure`,
fail-closed when empty). The gate SHALL refuse a non-loopback route
that has neither. The bind SHALL prefer a loopback address. A bind
that exposes routes publicly on a non-loopback interface after an
explicit acknowledgement SHALL emit a `warn!` log, because MCP inputs
are adversary-controlled Exchange data that crosses the ADR-0032 trust
boundary into the data plane.

Enforcement is route-level: the adapter SHALL carry the inbound HTTP request
headers onto the Exchange, and each request SHALL pass through the route's
`SecurityPolicy` before any route step. A denied request SHALL produce a tool
error result or a resource error body, and the route body SHALL NOT run.

Route security plans SHALL be scoped to a live owner. Each plan SHALL
retain the registering consumer's owner token. Registering a plan for a
`route_id` whose incumbent plan belongs to a live owner SHALL NOT remove
or overwrite the incumbent plan. A plan whose owner is dead SHALL be
replaced by a new registration and SHALL be ignored by plan lookup and
the exposure gate. A `stop()` SHALL remove the route's security plan
only when the stopping consumer still owns it. A late `stop()` of a dead
owner SHALL NOT remove a live replacement's plan, because a missing plan
downgrades dispatch to unauthenticated pass-through. A consumer whose
`Consumer::start()` fails after plan registration SHALL NOT remove or
overwrite a plan owned by another live consumer. A consumer that wins
registration of a tool name or resource URI SHALL re-assert its route
security plan after winning; the re-assertion SHALL overwrite a plan
whose owner is another consumer, even a live one, because entry
ownership proves the right to the route identity.

#### Scenario: loopback policy-less bind starts

- **GIVEN** a `mcp:` server declaration with no `security_policy` and a
  loopback bind
- **WHEN** the server consumer starts
- **THEN** the consumer starts successfully; `validate_server_policy`
  returns no refusal for the loopback policy-less config

#### Scenario: non-loopback policy-less bind refused without ack

- **GIVEN** a `mcp:` server declaration with no `security_policy`, bind
  `0.0.0.0:9100`, and an empty `allow_public_exposure` acknowledgement map
- **WHEN** the server consumer starts
- **THEN** the kernel per-bind exposure gate fails the start with a
  `CamelError` and no route is exposed

#### Scenario: acknowledged public non-loopback exposure warns

- **GIVEN** a `mcp:` server declaration with bind `0.0.0.0:9100` and a
  route exposed publicly on that bind under an `allow_public_exposure`
  acknowledgement
- **WHEN** the server consumer starts
- **THEN** the consumer starts successfully and emits a `warn!` about the
  public non-loopback exposure

#### Scenario: request without credentials denied

- **GIVEN** a tool route with a `security_policy` requiring the role
  `mcp-client`
- **WHEN** a host calls the tool without an `Authorization` header
- **THEN** the call returns a tool error result and no Exchange reaches the
  route body

#### Scenario: request with valid credentials granted

- **GIVEN** a tool route with a `security_policy` requiring the role
  `mcp-client`
- **WHEN** a host calls the tool with a valid `Authorization: Bearer <token>`
  whose principal holds `mcp-client`
- **THEN** the call succeeds, the route body runs, and the principal is
  attached to the Exchange

#### Scenario: late stop keeps the replacement's security plan

- **GIVEN** consumer A died without `stop()` and consumer B re-registered
  the same route identity with its own security plan
- **WHEN** a late `stop()` attributed to consumer A runs
- **THEN** the route's dispatch keeps enforcing consumer B's plan and does
  not fall back to unauthenticated pass-through

#### Scenario: failed duplicate start keeps the incumbent's security plan

- **GIVEN** consumer A is live with a security plan and a registered tool
  name
- **WHEN** consumer B starts on the same route identity and fails the
  duplicate-name guard
- **THEN** the route's dispatch keeps enforcing consumer A's plan and no
  failure-path cleanup removed or overwrote it

#### Scenario: concurrent restart loser cannot strip the winner's plan

- **GIVEN** consumer A died without `stop()` leaving its entry and plan
  behind, and consumers B and C start concurrently on the same route
  identity
- **WHEN** C wins the entry registration and B fails the duplicate guard
  and runs its failure cleanup
- **THEN** the route's dispatch enforces C's security plan and does not
  fall back to unauthenticated pass-through
