# mcp-component Specification

## Purpose
TBD - created by archiving change add-mcp-component. Update Purpose after archive.
## Requirements
### Requirement: MCP component scheme and dual roles

The system SHALL register a `camel-component-mcp` crate exposing the `mcp:`
URI scheme. A single `Component` impl SHALL own both the Consumer (MCP server,
inbound) and Producer (MCP client, outbound) roles, disambiguated by whether
an endpoint creates a consumer or a producer — mirroring the `http:` scheme.

#### Scenario: metadata declares both roles

- **GIVEN** the `mcp:` component is registered
- **WHEN** the registry harvests `ComponentMetadata`
- **THEN** `supports_consumer` and `supports_producer` are both `true` and
  `supports_streaming` is `true`

#### Scenario: client producer endpoint resolves a named server

- **GIVEN** a configured MCP server map entry named `crm`
- **WHEN** a producer endpoint is built from `mcp:call?server=crm&tool=lookup`
- **THEN** the endpoint resolves the `crm` entry and construction succeeds

### Requirement: Baseline protocol version `2026-07-28`

The MCP component SHALL baseline exclusively on protocol revision
`2026-07-28`. The server SHALL advertise `2026-07-28` as its only supported
protocol version and SHALL NOT implement any pre-`2026-07-28` compatibility or
fallback path. The client SHALL negotiate using the discover lifecycle with
`2026-07-28` as its only preferred version.

#### Scenario: server advertises only 2026-07-28 via discover

- **GIVEN** a started MCP server
- **WHEN** a host issues `server/discover`
- **THEN** the response lists exactly `["2026-07-28"]` as supported protocol
  versions and includes the server identity and capabilities

#### Scenario: pre-2026-07-28 request is rejected

- **GIVEN** a started MCP server
- **WHEN** a client sends a request whose `_meta` protocol version is
  `2025-11-25` (or any version other than `2026-07-28`)
- **THEN** the server responds with JSON-RPC error `-32022`
  (`UnsupportedProtocolVersionError`) whose `data.supported` lists
  `["2026-07-28"]`
- **AND** the server emits a `warn!` naming the peer and the rejected version

#### Scenario: legacy initialize attempt does not open a session

- **GIVEN** a started MCP server
- **WHEN** a legacy client attempts the `initialize` handshake and then sends a
  follow-up request carrying a pre-`2026-07-28` `_meta` version
- **THEN** no `Mcp-Session-Id` is issued and the follow-up request is rejected
  with `-32022`

#### Scenario: legacy initialize is rejected fail-closed

- **GIVEN** a started MCP server
- **WHEN** a client sends `initialize` offering protocol version
  `2025-11-25` (or any version other than `2026-07-28`)
- **THEN** the server responds with JSON-RPC error `-32022` whose
  `data.supported` lists `["2026-07-28"]`
- **AND** no success response carrying a server-default protocol version is
  returned (no fallback path)

### Requirement: Client producer tool call

The system SHALL provide an `mcp:call?server=<name>&tool=<name>` producer that
issues a JSON-RPC `tools/call` to the resolved MCP server once per Exchange,
carrying the `2026-07-28` protocol version and client capabilities in request
`_meta` (no `initialize` handshake, no session), using the Exchange body as
the arguments object, and SHALL return the tool result as the Exchange output
body.

#### Scenario: tool call returns result

- **GIVEN** a started `mcp:call` producer whose server answers `tools/call`
- **WHEN** an Exchange with a JSON arguments body is sent
- **THEN** the producer issues `tools/call`, receives the result, and sets it
  as the output body

#### Scenario: tool call carries per-request protocol metadata

- **GIVEN** a started `mcp:call` producer
- **WHEN** three Exchanges are sent in sequence
- **THEN** each of the three `tools/call` requests independently carries
  `io.modelcontextprotocol/protocolVersion = 2026-07-28` in `_meta`
- **AND** no `initialize` request and no `Mcp-Session-Id` is ever sent

### Requirement: Client producer resource read

The system SHALL provide an `mcp:read?server=<name>&uri=<resource-uri>`
producer, distinct from `mcp:call`, that issues a JSON-RPC `resources/read`
addressed by URI with no arguments body. The Exchange contract for read SHALL
NOT field-sniff tool-vs-resource on a single operation.

#### Scenario: resource read returns content

- **GIVEN** a started `mcp:read` producer whose server answers
  `resources/read`
- **WHEN** an Exchange is sent with `server=docs` and `uri=file:///a`
- **THEN** the producer issues `resources/read`, receives the content, and sets
  it as the output body

### Requirement: Producer fail-fast on incompatible remote

When a producer resolves and starts a client for a named server whose
`server/discover` does not report `2026-07-28` (or which does not implement
`server/discover`), endpoint start SHALL fail with a `CamelError` naming the
server and the detected or absent version. The producer SHALL NOT fall back to
a legacy lifecycle and SHALL NOT defer the failure to per-Exchange time.

#### Scenario: remote without 2026-07-28 fails endpoint start

- **GIVEN** an `mcp:call` producer configured for server `legacy` whose
  discover reports only `2025-11-25`
- **WHEN** the producer endpoint starts
- **THEN** start fails with a `CamelError` naming `legacy` and `2025-11-25`
- **AND** a `warn!` is emitted before the error is returned

#### Scenario: remote without discover fails endpoint start

- **GIVEN** an `mcp:call` producer configured for a server that returns
  `METHOD_NOT_FOUND` for `server/discover`
- **WHEN** the producer endpoint starts
- **THEN** start fails with a `CamelError` indicating the remote is not a
  `2026-07-28` server, with no legacy fallback attempted

### Requirement: rmcp SDK confinement boundary

The system SHALL confine every `rmcp` import to files under `src/adapter/`
within `camel-component-mcp`. No type originating from `rmcp` SHALL appear in
the component's public surface; only project-owned, Camel-shaped types
(`McpToolInvocation`, `McpToolResult`, `McpResource`) SHALL cross the
boundary. A boundary test SHALL fail if any `rmcp::` path appears outside
`src/adapter/`.

#### Scenario: boundary test passes

- **GIVEN** the built `camel-component-mcp` crate
- **WHEN** the boundary test scans source files for `rmcp::` usage outside
  `src/adapter/`
- **THEN** the scan finds zero violations and the test passes

### Requirement: MCP server shared listener registry

The system SHALL spawn exactly one Streamable-HTTP listener plus one JSON-RPC
dispatch loop per bind address, shared by every tool and resource consumer
route on that server. The first consumer to start on a bind SHALL spawn the
listener; subsequent consumers on the same bind SHALL reuse the existing
handle. A consumer that declares an incompatible configuration (bind conflict,
TLS mode, body caps) for an already-spawned bind SHALL fail to start with a
`CamelError`.

#### Scenario: first consumer spawns the listener

- **GIVEN** no listener exists for bind `127.0.0.1:9100`
- **WHEN** the first `mcp:crm/tool/lookup` consumer starts
- **THEN** a Streamable-HTTP listener is spawned for that bind

#### Scenario: second consumer reuses the listener

- **GIVEN** a listener already exists for bind `127.0.0.1:9100`
- **WHEN** a second `mcp:crm/tool/search` consumer starts on the same bind
- **THEN** no new listener is spawned and the consumer registers into the
  existing per-listener tool registry

#### Scenario: conflicting bind rejected

- **GIVEN** a listener exists for `127.0.0.1:9100` with TLS enabled
- **WHEN** a consumer starts on the same bind declaring TLS disabled
- **THEN** the consumer fails to start with a `CamelError`

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

### Requirement: Tool argument JSON Schema validation

The system SHALL validate `tools/call` arguments against the tool's declared
input JSON Schema (operator config, ADR-0032 trusted) before the invocation
reaches the route. An args payload that fails schema validation SHALL be
rejected with a clean MCP error and SHALL NOT produce an Exchange.

#### Scenario: valid args pass validation

- **GIVEN** a tool route with an input schema requiring `{id: string}`
- **WHEN** a host calls the tool with `{"id": "42"}`
- **THEN** validation passes and the invocation reaches the route

#### Scenario: invalid args rejected before dispatch

- **GIVEN** a tool route with an input schema requiring `{id: string}`
- **WHEN** a host calls the tool with `{"id": 7}`
- **THEN** the call is rejected with a schema-validation error and no Exchange
  is created

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

### Requirement: MCP DSL block and lowering

The system SHALL provide a declarative `mcp:` DSL block that declares a named
server (bind, TLS, `security_policy`, `max_tools`, `max_resources`) and its
tools and resources, each tool carrying an input JSON Schema. The block SHALL
lower each tool to an `mcp:<server>/tool/<name>` consumer route and each
resource to an `mcp:<server>/resource/<name>` consumer route, injecting the
schema as a validation step — structurally analogous to the `rest:`→`http:`
lowering. The listener bind, TLS, and `security_policy` are properties of the
named server, not per-tool URI options.

#### Scenario: DSL block lowers to consumer routes

- **GIVEN** a `mcp:` block declaring server `crm` with tool `lookup`
- **WHEN** the DSL is parsed and lowered
- **THEN** an `mcp:crm/tool/lookup` consumer route exists carrying the declared
  input schema

#### Scenario: schema lives in operator config not on the wire

- **GIVEN** a lowered `mcp:crm/tool/lookup` route
- **WHEN** an Exchange is processed
- **THEN** the input schema is sourced from the DSL config, not from any
  Exchange header or body

### Requirement: Route-owned tool dispatch, no auto-loop

The system SHALL NOT provide any automatic loop that dispatches a tool call and
returns its result to an LLM turn without an explicit route step. The MCP
producer SHALL be a dispatch target only; the route SHALL own the decision to
dispatch and the reshaping of the result into an LLM tool message.

#### Scenario: producer does not loop back to LLM

- **GIVEN** a `mcp:call` producer and an LLM route
- **WHEN** the producer returns a tool result
- **THEN** the result is returned to the route as the Exchange output and no
  automatic second LLM call is made by the component

### Requirement: v1 protocol surface

The system SHALL support exactly one transport — Streamable HTTP — and exactly
two MCP primitives — Tools (full) and Resources (read-on-demand). The system
SHALL reject or decline every other MCP transport and primitive: the stdio
transport, the legacy HTTP+SSE transport, the Prompts primitive,
`resources/subscribe`, and the `2026-07-28` `subscriptions/listen`
change-feed stream.

#### Scenario: stdio transport unsupported

- **GIVEN** an `mcp:` endpoint configuration
- **WHEN** a configuration declares a stdio transport
- **THEN** the configuration is rejected with a `CamelError` at construction

#### Scenario: legacy HTTP+SSE transport unsupported

- **GIVEN** an `mcp:` endpoint configuration
- **WHEN** a configuration declares the legacy HTTP+SSE transport
- **THEN** the configuration is rejected with a `CamelError` at construction

#### Scenario: prompts unsupported

- **GIVEN** a started MCP server
- **WHEN** a host issues `prompts/list`
- **THEN** the server responds that the prompts capability is unavailable

#### Scenario: resource subscriptions unsupported

- **GIVEN** a started MCP server
- **WHEN** a host issues `resources/subscribe`
- **THEN** the server responds that the operation is unsupported

### Requirement: Standard Streamable-HTTP request headers

On the Streamable-HTTP transport the server SHALL accept and the client SHALL
emit the `2026-07-28` standard request headers `Mcp-Method` and `Mcp-Name` on
JSON-RPC POST, and SHALL NOT depend on `Mcp-Session-Id` for request routing.

#### Scenario: session header is not required

- **GIVEN** a started MCP server
- **WHEN** a host issues `tools/call` with no `Mcp-Session-Id` header
- **THEN** the request is dispatched normally on the strength of its `_meta`
  protocol version alone

### Requirement: Component-local MCP types

All MCP-specific request, result, and invocation types
(`McpToolInvocation`, `McpToolResult`, `McpResourceRead`, `McpResource`) SHALL
remain inside `camel-component-mcp`. No MCP type SHALL be added to `camel-api`
or any other crate. Cross-crate Exchange contracts SHALL use the existing
`Body` and header types only (e.g. a `CamelMcpToolCall` header), honoring the
ADR-0045 module-discipline ceiling.

#### Scenario: camel-api gains no MCP type

- **GIVEN** the built workspace
- **WHEN** `camel-api`'s public items are inspected
- **THEN** no type whose name begins with `Mcp` is present

### Requirement: List-result cache metadata

The MCP server SHALL include SEP-2549 cache metadata — `ttlMs` and
`cacheScope` — in every `tools/list` and `resources/list` result, because
protocol revision `2026-07-28` marks both fields required and strict clients
reject result payloads that omit them. Until a caching design exists, list
results SHALL be non-cacheable: `ttlMs: 0` and `cacheScope: "private"`
(the catalog is dynamic under readiness gating, and list results must not be
shared across authorization contexts).

#### Scenario: tools/list carries cache metadata

- **GIVEN** a started MCP server with at least one ready tool route
- **WHEN** a host issues `tools/list` over Streamable-HTTP
- **THEN** the JSON-RPC result object contains `"ttlMs": 0` and
  `"cacheScope": "private"` alongside the `tools` array

#### Scenario: resources/list carries cache metadata

- **GIVEN** a started MCP server with at least one ready resource route
- **WHEN** a host issues `resources/list` over Streamable-HTTP
- **THEN** the JSON-RPC result object contains `"ttlMs": 0` and
  `"cacheScope": "private"` alongside the `resources` array

#### Scenario: empty list still carries cache metadata

- **GIVEN** a started MCP server with no ready tool routes
- **WHEN** a host issues `tools/list`
- **THEN** the result object still contains `"ttlMs": 0` and
  `"cacheScope": "private"` (field presence does not depend on catalog
  cardinality)

