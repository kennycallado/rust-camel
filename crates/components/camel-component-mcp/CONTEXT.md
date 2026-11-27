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
- `tools/list` / `resources/list` list only what the caller may see: Public
  entries to every caller, protected entries only after kernel authentication
  (and authorization for request-independent policies).

A `mcp:` DSL block lowers each tool/resource to a consumer route. The
`rest:` → `http:` analogue now holds: the block is a catalog declaration
(names, schemas, resource URIs) plus an optional behavior spec — without
`to`/`steps` the lowered route keeps the identity pipeline (a bare
`tools/call` echoes its arguments); with `to` or `steps` that pipeline
lowers onto the consumer route, and declaring both fails at lowering
(bd rc-23y2). Real tool and resource behavior is equally expressed by
explicit routes whose `from:` is an `mcp:` URI and that attach their own
steps.

## Config keys

Global TOML key: `mcp` (`McpBundle`).

- `mcp.servers.<name>` (`McpServerConfig`): `bind`, `tls`, `security_policy`,
  `max_tools`, `max_resources`, `allowed_hosts`, `skills_root`.
- `mcp.remotes.<name>` (`McpRemoteConfig`): `url`, `transport`,
  `request_timeout_secs`, `connect_timeout_secs`, `allow_internal`,
  `allow_cleartext`.

`request_timeout_secs` (whole seconds, minimum 1, default 120) bounds every
post-connect request to that remote — one `tools/call` or one
`resources/read`. Expiry cancels the in-flight request best-effort
(`notifications/cancelled`) and fails the dispatch with an error naming the
operation and the bound (`tool call '<name>' exceeded the 120s request
deadline`).

`connect_timeout_secs` (whole seconds, minimum 1, default 15) bounds the
connect phase — the TCP connect plus the `server/discover` handshake — after
DNS pinning. On expiry connect fails naming the bound (`connect to remote
'<server>' did not complete the discover handshake within 15s`). DNS
resolution keeps its own fixed 5 s limit. Reclaiming the handshake's worker
and socket in the wedge shape is an upstream rmcp limitation (the bootstrap
POST is un-cancellable; route start is bounded regardless — bd rc-s4xod).

Cleartext `http://` remotes to public targets are rejected (IP literals
at config load, hostnames at connect) unless
`allow_cleartext = true` (ADR-0081).

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

## Skills (server role)

`skills_root` is an opt-in `McpServerConfig` path. Absent, the listener loads
no catalog and the extension stays off. Present, `McpServerRegistry::spawn`
loads `McpSkillCatalog` before binding. A bad root, an invalid `SKILL.md`, an
escaping symlink, or a limit breach fails the start with a typed `McpError`
and binds nothing. The catalog is immutable for the listener lifetime and
holds every regular file of each skill with its `skill://` URI, a `sha256:`
digest of the served bytes, and the byte size. `skills/list` pages this
manifest 50 skills at a time; `resources/read` serves the stored bytes and
rejects any `..` segment. `resources/list` stays route-only, and a resource
route that declares a `skill://` URI on a listener with a root fails consumer
start: the catalog owns that namespace. A second named server on the same bind
must declare the same literal root: `get_or_spawn` compares `Option<String>`
values, so `/srv/skills/` differs from `/srv/skills`.

Catalog visibility follows the bind's `protected_seen` latch. Skills are public
until the bind has committed a non-Public route plan; the latch survives
listener respawns. After that, at least one live non-Public plan must admit the
caller through the same kernel and `catalog_entry_visible` tail as `tools/list`;
a stopped protected route does not reopen the catalog. The latch keys on
registered route plans and governs only this plan-less catalog. It is not the
ADR-0061 Rule 8 bind-presence gate. Both methods and skill-file reads emit
`ttlMs: 0` and `cacheScope: "private"`.

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
