## MODIFIED Requirements

### Requirement: Tool and resource route registration

Each `mcp:<server>/tool/<name>` consumer route SHALL register its tool
name and an `mpsc::Sender<McpToolInvocation>` into the per-listener
`McpToolRegistry` at `Consumer::start()`, then mark itself ready. Each
`mcp:<server>/resource/<name>` consumer route SHALL register its declared MCP
resource URI (operator config) into the per-listener `McpResourceRegistry`,
mapping that URI to the route. A consumer that stops SHALL unregister its
entry so that subsequent calls return a clean MCP error, not a panic on a dead
channel.

Registration SHALL be scoped to a live owner. Each registration SHALL carry
an owner-liveness token minted by the registering consumer. `register()`
SHALL reject a name or URI whose entry belongs to a live owner. If the
existing entry's owner is dead, `register()` SHALL replace the entry, and
the replacement SHALL take over the name without operator action or process
restart. Unregistration at `stop()` SHALL remove the entry only when the
stopping consumer still owns it. Resolution and listing SHALL ignore and
reclaim entries whose owner is dead.

#### Scenario: tool call routes to the registered handler

- **GIVEN** a started `mcp:crm/tool/lookup` consumer with a registered handler
- **WHEN** the dispatch loop receives `tools/call {name: "lookup", args}`
- **THEN** the invocation is sent to the route's handler and the route's result
  is returned

#### Scenario: stopped tool is unregistered

- **GIVEN** a started `mcp:crm/tool/lookup` consumer that then stops
- **WHEN** the dispatch loop receives `tools/call {name: "lookup"}`
- **THEN** the call returns a clean MCP method-error and no dead channel is
  awaited

#### Scenario: aborted consumer releases its name

- **GIVEN** a started `mcp:crm/tool/lookup` consumer whose bridge task is
  aborted and whose consumer is dropped without `stop()`
- **WHEN** a new `mcp:crm/tool/lookup` consumer starts on the same bind in
  the same process
- **THEN** registration succeeds and the new consumer owns the entry

#### Scenario: dropped consumer releases its name

- **GIVEN** a started `mcp:crm/tool/lookup` consumer that is dropped without
  `stop()`
- **WHEN** a new `mcp:crm/tool/lookup` consumer starts on the same bind in
  the same process
- **THEN** registration succeeds and the new consumer owns the entry

#### Scenario: live duplicate still rejected

- **GIVEN** two consumers with a started tool registration of the same name
  on the same bind
- **WHEN** the second consumer starts while the first is still live
- **THEN** the second start fails with the "already registered" error

#### Scenario: late stop does not remove the replacement

- **GIVEN** consumer A died without `stop()` and consumer B took over the
  name
- **WHEN** a late `stop()` attributed to consumer A runs
- **THEN** consumer B's entry stays registered and live

### Requirement: Resource URI addressing

A declared resource SHALL carry an MCP resource URI (operator config). The
`McpResourceRegistry` SHALL map that URI to the resource route.
`resources/list` SHALL advertise the registered URIs. `resources/read {uri}`
SHALL resolve the URI through the registry, send an `McpResourceRead` into the
bound route, and return the route's body. A `resources/read` for an unknown
URI or a stopped resource route SHALL return a clean MCP error and SHALL NOT
create an Exchange.

Resource registration SHALL follow the same owner-liveness scoping as tool
registration. A resource URI whose owner is dead SHALL be replaceable by a
new registration. A late `stop()` SHALL NOT remove a successor's resource
entry. `resources/list` SHALL NOT advertise a URI whose owner is dead.

#### Scenario: resources list advertises URIs

- **GIVEN** a started server with a resource route declaring URI
  `crm://customers`
- **WHEN** a host issues `resources/list`
- **THEN** the response includes `crm://customers`

#### Scenario: resource read by URI dispatches to the route

- **GIVEN** a started resource route for URI `crm://customers`
- **WHEN** a host issues `resources/read {uri: "crm://customers"}`
- **THEN** the read is routed to the bound route and the route's body is
  returned

#### Scenario: unknown resource URI rejected

- **GIVEN** a started server with no resource registered at `crm://unknown`
- **WHEN** a host issues `resources/read {uri: "crm://unknown"}`
- **THEN** the call returns a clean MCP error and no Exchange is created

#### Scenario: stopped resource route rejected

- **GIVEN** a resource route for URI `crm://customers` that has stopped and
  unregistered
- **WHEN** a host issues `resources/read {uri: "crm://customers"}`
- **THEN** the call returns a clean MCP error and no Exchange is created

#### Scenario: dead-owner resource URI is replaced on restart

- **GIVEN** a resource consumer for URI `crm://customers` that was dropped
  without `stop()`
- **WHEN** a new resource consumer for the same URI starts in the same
  process
- **THEN** registration succeeds, and a `resources/read` dispatches to the
  new route

### Requirement: Readiness gating

The MCP server SHALL advertise only ready routes in `tools/list` and
`resources/list`, filtering by the ready flag at list/dispatch time (stateless:
there is no `initialize` handshake to gate on). A tool whose consumer has not
yet marked ready SHALL NOT appear in the catalog. A `tools/call` for a
not-ready tool SHALL be rejected. An entry whose owner is dead SHALL NOT
appear in the catalog even if its ready flag was set before the owner died.

#### Scenario: not-ready tool is hidden from listing

- **GIVEN** a server with one consumer still starting
- **WHEN** a host issues `tools/list`
- **THEN** the not-ready tool is absent from the response

#### Scenario: dead-owner tool is hidden from listing

- **GIVEN** a tool that was marked ready and whose consumer then died
  without `stop()`
- **WHEN** a host issues `tools/list`
- **THEN** the tool is absent from the response

### Requirement: MCP server fail-closed authentication

The MCP server endpoint SHALL refuse to bind unless a `security_policy` is
configured for the named server, because MCP inputs are adversary-controlled
Exchange data that crosses the ADR-0032 trust boundary into the data plane.
The bind SHALL prefer a loopback address and SHALL emit a `warn!` log when the
configured bind is `0.0.0.0` or a non-loopback interface.

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

#### Scenario: concurrent restart loser cannot strip the winner's plan

- **GIVEN** consumer A died without `stop()` leaving its entry and plan
  behind, and consumers B and C start concurrently on the same route
  identity
- **WHEN** C wins the entry registration and B fails the duplicate guard
  and runs its failure cleanup
- **THEN** the route's dispatch enforces C's security plan and does not
  fall back to unauthenticated pass-through

#### Scenario: bind refused without security policy

- **GIVEN** a `mcp:` server declaration with no `security_policy`
- **WHEN** the server consumer starts
- **THEN** the consumer fails to start with a `CamelError`

#### Scenario: non-loopback bind warns

- **GIVEN** a `mcp:` server declaration with a `security_policy` and bind
  `0.0.0.0:9100`
- **WHEN** the server consumer starts
- **THEN** the consumer starts successfully and emits a `warn!` about the
  non-loopback bind

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

### Requirement: Catalog cardinality cap

The system SHALL bound the number of tools and resources per named server by a
configurable `max_tools` and `max_resources` (default 128 each), declared via
the per-item config channel (ADR-0038, `#[serde(deny_unknown_fields)]`,
hardened-but-raisable, no global disable switch). On breach, the system SHALL
reject the (N+1)th route at `Consumer::start()` with a clean `CamelError` and
SHALL NOT silently truncate the catalog. Cap enforcement SHALL prune
dead-owner entries first, so a dead entry under any name releases its slot.
An entry whose owner is dead SHALL release its cap slot, both on prune and on
replace. A replacement that takes over a dead owner's name SHALL NOT consume
an additional slot.

#### Scenario: 129th tool rejected

- **GIVEN** a server with `max_tools` left at default 128 and 128 tools
  registered
- **WHEN** the 129th tool consumer starts
- **THEN** the consumer fails to start with a `CamelError`

#### Scenario: raised cap allows more tools

- **GIVEN** a server declaring `max_tools: 200`
- **WHEN** 150 tool consumers start
- **THEN** all 150 start successfully

#### Scenario: takeover at full cap does not consume a slot

- **GIVEN** a server with `max_tools` 128 and 128 tools registered, one of
  which belongs to a dead owner
- **WHEN** a new consumer starts and takes over the dead owner's name
- **THEN** the start succeeds and the registry still holds 128 entries

#### Scenario: dead entry under another name releases its slot

- **GIVEN** a server with `max_tools` 128 and 128 tools registered, one of
  which belongs to a dead owner under a different name
- **WHEN** a new consumer starts and registers a new tool name
- **THEN** the start succeeds and the registry still holds 128 entries
