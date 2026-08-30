# camel-component-mcp

First-class MCP (Model Context Protocol) Component, scheme `mcp:` (charter:
ADR-0060). Owns both MCP roles, disambiguated by consumer-vs-producer
creation:

- **Consumer (server)**: exposes tools and resources through one shared
  Streamable-HTTP listener per bind.
- **Producer (client)**: dispatches tool calls and resource reads to remote
  MCP servers.

Tool dispatch is route-owned: the producer issues exactly one `tools/call` /
`resources/read` per Exchange and never auto-loops (no LLM call, no second
call). It is a dispatch target, never a decider (ADR-0060).

## Endpoints

Producer (client role):

- `mcp:call?server=<remote>&tool=<name>` — one `tools/call` with the Exchange
  body as arguments.
- `mcp:read?server=<remote>&uri=<uri>` — one `resources/read`; the body is
  replaced with the resource bytes.

Consumer (server role):

- `mcp:<server>/tool/<name>?schema=<url-encoded JSON Schema>` — serves
  `tools/call` for `<name>`; arguments are validated against the schema.
- `mcp:<server>/resource/<name>?uri=<mcp-uri>` — serves `resources/read` for
  `<uri>`; `resources/list` projects the registered URIs.

A `mcp:` DSL block lowers each tool/resource to a step-less consumer route.
The `rest:` → `http:` analogue is partial: the block is a catalog declaration
(names, schemas, resource URIs), not a behavior spec — its lowered routes
carry no processing steps, so a bare `tools/call` echoes its arguments. Real
tool and resource behavior is expressed by explicit routes whose `from:` is an
`mcp:` URI and that attach their own steps; steps passthrough inside the block
is a tracked limitation (bd rc-23y2).

## Config keys

Global TOML key: `mcp` (`McpBundle`).

- `mcp.servers.<name>` (`McpServerConfig`): `bind`, `tls`, `security_policy`,
  `max_tools`, `max_resources`, `allowed_hosts`.
- `mcp.remotes.<name>` (`McpRemoteConfig`): `url`, `transport`.

Server config merges by ownership (ADR-0061 Rule 9): when a route
originates from a DSL `mcp:` block, the block's `bind`/`tls`/`max_tools`/
`max_resources` ARE the runtime listener values; TOML and the DSL block
declaring the same key with DIFFERENT values is a hard startup error naming
both sources (never silent TOML-wins). Keys declared on one side only flow
from that side; catalog caps default to 128 only when neither side declares
them. The DSL block's `security_policy` is a real `RouteDslSecurityPolicy`
that propagates to every lowered tool and resource route. The DSL server
`name` MUST match a `mcp.servers.<name>` key or consumer start fails
cleanly (`McpError::Endpoint` naming the server).

`transport` accepts exactly `streamable-http`. The enum deserializer rejects
any other string at config load; `deny_unknown_fields` applies to both config
structs (ADR-0033). Server policy follows the unified transport auth kernel
(ADR-0061). The per-bind exposure gate runs at consumer start, fail-closed:
the bind is an IP literal, caps are nonzero, and a non-loopback bind serving
any `Public` route requires a `[binds."<addr>"] allow_public_exposure = true`
acknowledgement or start fails naming the bind (`camel-auth` bind gate,
ADR-0061 Rule 4); an acknowledged exposure warns permanently. Enforcement is
kernel-owned: the adapter extracts credentials per the route plan's
credential sources from the normalized request headers, authenticates via
`kernel_authenticate`, and installs the typed carrier on the Exchange
before the route pipeline; denial surfaces as the tool `isError`/resource
error body. Remote URLs are validated at connect time, not at startup.
Catalog caps default to 128 and ride the ADR-0038 per-item channel; a breach
rejects the (N+1)th route with a clean `CamelError` (no silent truncation).

## Registries (server role)

- `McpServerRegistry` — process-global map of bind → shared listener
  (mirror of camel-http's `ServerRegistry`). A later consumer whose config
  conflicts with the live listener (`allowed_hosts`, TLS, caps) is rejected
  fail-closed; duplicate registration is rejected atomically.
- `McpToolRegistry` — per-listener `name → route` map backing `tools/call`.
  Each entry carries the registering consumer's owner-liveness token
  (ADR-0068): a dead owner's entry is replaced on re-registration and
  pruned lazily. The unregister used by `stop()` and failure cleanup is
  owner-conditional (`unregister_owned`, `Weak::ptr_eq`).
- `McpResourceRegistry` — per-listener `uri → route` map backing
  `resources/read`. Entries follow the same owner-liveness discipline as
  `McpToolRegistry` (ADR-0068).

## Adapter confinement

All rmcp imports are confined to `src/adapter/` (ADR-0020). Project-owned
traits carry Camel-shaped types across the boundary: `McpClient` (client
role) and the boundary types `McpToolInvocation`, `McpToolResult`,
`McpResourceRead`, `McpResource`. `McpServerMap` is the client-role twin of
the LLM `ProviderMap` (ADR-0020). No MCP-shaped type leaves the crate
(ADR-0060).

## Protocol baseline

`2026-07-28`, stateless: no `initialize` handshake, no protocol sessions, no
`Mcp-Session-Id`, per-request `_meta`. The client connects with the Discover
lifecycle; an incompatible remote fails fast at producer start (ADR-0060).
The server overrides `supported_protocol_versions()` to the single baseline;
rmcp's inline guard rejects other peers with JSON-RPC `-32022`.
The adapter answers a legacy `initialize` offer with `-32022` and fails closed
without falling back to the server default. The `tools/list` and
`resources/list` results carry SEP-2549 cache metadata (`ttlMs: 0`,
`cacheScope: "private"`) and are not cacheable.

Exclusions: Prompts (duplicates `camel-template`, ADR-0047), stdio transport,
legacy SSE transport (excluded at the Cargo level), resource subscriptions.

MCP server inputs are adversary-controlled exchange data crossing the
ADR-0032 trust boundary. The producer propagates the remote's `is_error: bool`
faithfully; the route author decides how to handle a remote-reported failure
(ADR-0060).
