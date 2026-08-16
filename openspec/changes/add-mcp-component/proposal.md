# Proposal: add-mcp-component

## Why

rust-camel is an integration bus, but its routes and components cannot be
driven by an LLM host today. The Model Context Protocol (MCP) standardizes how
an LLM host connects to external tools and context over JSON-RPC. A first-class
MCP component lets any host (Claude Desktop, Cursor, a custom agent) call
camel routes as MCP tools and read camel data as MCP resources. This turns the
bus into a capability surface that AI systems can drive directly.

Conversely, camel routes that enrich an LLM conversation (via
`camel-component-llm`) currently cannot dispatch the model's `ChatEvent::ToolCall`
intent to a remote MCP server. The new component also fills this outbound role:
a route can call an external MCP server and feed the result back into the next
LLM turn as a `ChatRole::Tool` message.

Architectural fit is settled by an oracle memo
(`docs/research/mcp-architectural-fit-verdict.md`, three rounds): MCP is a
capability-bearing external system with a lifecycle, so it is a Component, not
a provider bolted onto `camel-component-llm` (that would violate the
`LlmProvider` adapter boundary, ADR-0020) and not a transport shim inside
`camel-http` (that would conflate JSON-RPC semantics with HTTP server
mechanics).

## What Changes

- **New crate `crates/components/camel-component-mcp`**, scheme `mcp:`, one
  component owning BOTH roles (mirrors `camel-http` server-Consumer /
  client-Producer split):
  - **Consumer (server, primary):** exposes camel routes as MCP tools and
    resources on a shared Streamable-HTTP listener. Declared via a `mcp:` DSL
    block that lowers each tool to an `mcp:<server>/tool/<name>` consumer route
    carrying an input JSON Schema, and each resource to an
    `mcp:<server>/resource/<name>` consumer route (direct MCP analogue of the
    `rest:`→`http:` lowering).
  - **Producer (client, secondary):** `mcp:call?server=<name>&tool=<name>`
    (tools/call) and `mcp:read?server=<name>&uri=<uri>` (resources/read).
- **rmcp (Rust MCP SDK)** confined behind project-owned seams in `src/adapter/`,
  with a boundary test (ADR-0020 pattern): the `McpClient` trait (client role)
  and server-side registry/mpsc seams (`McpServerRegistry`, `McpToolRegistry`,
  `McpResourceRegistry`, `McpToolInvocation`, `McpResourceRead`). No rmcp import
  leaks outside the adapter.
- **Shared-listener registry** (`McpServerRegistry` + per-listener
  `McpToolRegistry`/`McpResourceRegistry`) — copies the proven
  `ServerRegistry`/`HttpRouteRegistry` pattern from `camel-http`.
- **New ADR-0060** (charter: MCP as first-class Server+Host component;
  route-owned tool dispatch; DSL-lowered catalog; SDK confined; Streamable
  HTTP only).
- **Protocol baseline `2026-07-28` only.** The server advertises `2026-07-28`
  as its sole supported version via the mandatory `server/discover` RPC; the
  client negotiates with the discover lifecycle. Stateless per-request `_meta`
  version carriage — no `initialize` handshake, no protocol sessions, no
  `Mcp-Session-Id`. rmcp pinned to `^3` (the first line implementing
  `2026-07-28` inline negotiation + discover), confined to `src/adapter/`.

**Explicitly excluded from v1:** pre-`2026-07-28` protocol versions (the
server rejects them with `UnsupportedProtocolVersionError`; the client fails
endpoint start — no legacy fallback path); the legacy HTTP+SSE transport
(deprecated since `2025-03-26`, never re-added by rmcp); protocol-level
sessions and the `Mcp-Session-Id` header; MCP **Prompts** primitive (overlaps
`camel-template`/MiniJinja, ADR-0047 — deferred to avoid duplication); stdio
transport (camel is not a process supervisor); `subscriptions/listen` (the
`2026-07-28` replacement for `resources/subscribe`) — needs a change-feed the
DSL does not model, deferred; any auto-loop that bypasses the route to
dispatch tool calls.

## Acceptance criteria

- A `mcp:` DSL block declares a server with tools (each with an input JSON
  Schema) and resources (each with an MCP resource URI); the block lowers to
  consumer routes that a real MCP host can `server/discover` against, list,
  and call/read.
- A `mcp:call` producer dispatches a `tools/call` to a configured MCP server and
  returns the result; a `mcp:read` producer reads a resource by URI.
- An MCP server endpoint refuses to bind without a `security_policy`
  (fail-closed), and warns on a non-loopback bind.
- Tool/resource catalogs are bounded (`max_tools`/`max_resources`, default 128);
  the (N+1)th route is rejected at `Consumer::start()` with a clean
  `CamelError`.
- rmcp imports appear only in `src/adapter/` (boundary test passes).
- `ComponentMetadata` declares both `supports_consumer` and `supports_producer`.

## Risk budget

- **Acceptable:** rmcp 3.x tracking (`^3`) — the `2026-07-28` inline-
  negotiation + discover APIs are new; churn is absorbed by the
  `src/adapter/` boundary (ADR-0020). Streamable-HTTP transport maturity
  (single transport, confined).
- **Out of bounds:** pre-`2026-07-28` interoperability — no legacy
  `initialize`/session fallback, no auto-downgrade; non-conformant peers are
  warned and rejected (`-32022` server-side, fail-fast start client-side).
  spec-completeness as a goal (ADR-0046 anti-pattern — implement the subset
  that serves integration routing). Any tool-execution auto-loop that
  bypasses route ownership. Adding cross-crate MCP types to `camel-api`
  (ADR-0045 module-discipline ceiling).
