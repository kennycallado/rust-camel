# Design: add-mcp-component

## Approach

A new first-class Component (`camel-component-mcp`, scheme `mcp:`) owns both
MCP roles, disambiguated by consumer-vs-producer creation — the same split
`camel-http` uses for HTTP server/client. The rmcp Rust SDK is confined behind
project-owned traits in `src/adapter/` with a boundary test (ADR-0020 pattern).

**Server role (Consumer, primary value).** A `mcp:` DSL block declares a named
server (bind, TLS, `security_policy`) plus its tools (each with an input JSON
Schema) and resources (each with an MCP resource URI), and the block lowers
each tool to an `mcp:<server>/tool/<name>` consumer route and each resource to
an `mcp:<server>/resource/<name>` consumer route — the direct MCP analogue of
the `rest:`→`http:` lowering (`camel-dsl/src/rest.rs:248-259`). One shared
Streamable-HTTP listener per bind serves all tool/resource routes on that
server, via a per-`(bind)` `McpServerRegistry` that spawns the listener +
JSON-RPC dispatch loop once and hands the same handle to subsequent consumers
(copy of `ServerRegistry::global().get_or_spawn`, `camel-http/lib.rs:665-676,
1249-1281`). Each tool consumer registers its `name → mpsc::Sender` into a
per-listener `McpToolRegistry` (sibling of `HttpRouteRegistry`) at
`Consumer::start()` and marks ready; the dispatch loop resolves
`tools/call {name, args}` to the registered handler, validates args against the
schema, sends an `McpToolInvocation` into the route, awaits the result, and
returns it. Each resource consumer registers its MCP URI into a
`McpResourceRegistry` (a URI→route map); `resources/read {uri}` resolves
through that map, sends an `McpResourceRead` into the route, and returns the
body, while `resources/list` projects the registered URIs.

**Client role (Producer, secondary).** `mcp:call?server=<name>&tool=<name>`
and `mcp:read?server=<name>&uri=<...>`. `server=<name>` resolves through an
`McpServerMap` (`HashMap<String, Arc<dyn McpClient>>`), the client-role twin
of LLM's `ProviderMap` (ADR-0020). The client connects via
`serve_with_lifecycle(ClientLifecycleMode::Discover { preferred_versions:
vec![ProtocolVersion::V_2026_07_28] })` — never `serve()` (the legacy
`initialize` lifecycle). Discover runs at connect; each Exchange issues one
`tools/call` or `resources/read` carrying `2026-07-28` in request `_meta`.
If discover reports no `2026-07-28` support (or returns `METHOD_NOT_FOUND`),
connect maps the `ClientInitializeError` to a `CamelError` and the endpoint
fails to start (no `legacy_startup`, no per-Exchange degradation).

**Tool dispatch stays route-owned.** The component never auto-loops: the LLM
component emits `ChatEvent::ToolCall` intent, the route decides to route to
`mcp:call`, and the route reshapes the result into `ChatRole::Tool` for the
next turn. The MCP producer is a dispatch *target*, never a *decider*.

**Security posture.** MCP server inputs are adversary-controlled Exchange data
that crosses the ADR-0032 trust boundary and drives the data plane → the server
endpoint is NOT a diagnostic endpoint (ADR-0052 exempts only non-data-plane
inbound). Therefore: refuse to bind without a `security_policy`
(ADR-0033 Require-Explicit-Choice), prefer loopback bind, `warn!` on
`0.0.0.0` (ADR-0052 rule 3).

**Catalog DoS cap.** `max_tools`/`max_resources`, default 128 each, via the
ADR-0038 per-item config channel (`#[serde(deny_unknown_fields)]`,
hardened-but-raisable, no global disable). Breach rejects the (N+1)th route at
`Consumer::start()` with a clean `CamelError` — never silent truncation
(ADR-0032 "silent behaviour forbidden").

**Primitives scope.** v1 = Tools (full: list + call, input/output JSON Schema)
+ Resources (read-on-demand: list + read; no subscriptions). Prompts DEFERRED
— duplicates `camel-template`/MiniJinja (ADR-0047).

**Protocol baseline (`2026-07-28`).** The component speaks exactly MCP
`2026-07-28`, the first stateless revision: no
`initialize`/`notifications/initialized` handshake, no protocol sessions, no
`Mcp-Session-Id`, mandatory `server/discover`, and per-request protocol
version + client capabilities carried in `_meta`
(`io.modelcontextprotocol/protocolVersion`,
`io.modelcontextprotocol/clientCapabilities`). The rejection of
pre-`2026-07-28` peers is delegated to rmcp: the server overrides
`ServerHandler::supported_protocol_versions() ->
Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])`, after which rmcp's inline
guard returns JSON-RPC `-32022 UnsupportedProtocolVersionError` (with the
supported-versions list in `data`) for any non-matching request; the client
uses the `Discover` lifecycle so rmcp never auto-downgrades
(`negotiate_protocol_version` echoes of older versions only run on the
`Initialize` path we never enable). `on_initialized` exists on the
`ServerHandler` trait but fires only on the legacy `serve()` lifecycle; we
leave it a no-op — it is not evidence of a live handshake. Enforcement is
single-channel: JSON-RPC `-32022` (no additional HTTP 400, to avoid
double-signalling). Rejected peers are logged at `warn!` (expected
foreign/legacy traffic); `error!` is reserved for the supervisor.

## Affected crates

- `crates/components/camel-component-mcp` (NEW): Component, Endpoint, Consumer
  (server), Producer (client), the `McpClient` trait (the only provider trait —
  the server role is served by registries + mpsc senders, not handler traits),
  rmcp adapters, `McpServerMap`,
  `McpServerRegistry`, `McpToolRegistry`, `McpResourceRegistry`, config,
  headers, error, bundle, CONTEXT.md.
- `crates/camel-dsl`: new `mcp:` DSL block AST + lowering to `mcp:` consumer
  routes (mirrors `rest.rs`). Registered in the DSL parser.
- `crates/components/CONTEXT.md` and `CONTEXT-MAP.md`: new context entry +
  domain terms.
- `docs/adr/0060-mcp-first-class-component.md` (NEW): charter ADR.
- Workspace `Cargo.toml`: add the new member crate and the `rmcp = "3"`
  dependency on the component (features `client`, `server`,
  `transport-streamable-http-server`, `transport-streamable-http-client`; NO
  `transport-sse-*` feature — the deprecated HTTP+SSE transport is excluded at
  the Cargo level, not just in prose). 3.x is the first line implementing the
  `2026-07-28` inline-negotiation + discover APIs; track `^3` and let the
  adapter absorb churn (ADR-0020).

## Architecture boundaries

- **Data/control plane (ADR-0001):** MCP server inbound is data plane (drives
  routes); the shared-listener registry + dispatch loop are the consumer
  machinery, not control plane. Catalog caps and auth live at endpoint
  construction/start, not in the runtime bus.
- **Components (ADR-0041):** `metadata()` declares
  `supports_consumer: true`, `supports_producer: true`, `supports_streaming:
  true`; `uri_options` document `server`, `tool`/`uri`, `bind`,
  `security_policy`, `transport`. No new `ComponentCapabilities` variant —
  `supports_consumer` already means "can originate a route."
- **No cross-crate types (ADR-0045):** MCP types stay in the component crate.
  The client Exchange contract is `Body` + headers (`CamelMcpToolCall`); the
  server contract is the internal `McpToolInvocation`→Exchange mapping.
- **rmcp boundary (ADR-0020):** all rmcp imports confined to `src/adapter/`;
  the JSON-RPC envelope dies at the trait boundary; only Camel-shaped types
  (`McpToolInvocation`, `McpToolResult`, `McpResource`) cross.

## Phases

### Phase 1: Crate skeleton + rmcp client boundary + Producer (client) role

- **Goal:** Stand up the crate, the rmcp confinement discipline, and the
  outbound client role.
- **Dependencies:** rmcp `^3` (discover lifecycle + `2026-07-28` negotiation);
  `camel-component-api` SPI; `camel-api`.
- **Externally-visible types/interfaces:** `McpComponent` (scheme `mcp:`),
  `McpClient` trait, `McpServerMap`, producer endpoints (`mcp:call`,
  `mcp:read`), `CamelMcpToolCall`/`CamelMcpResult` headers.
- **Deliverable:** Compiling crate with a working `mcp:call`/`mcp:read`
  producer against an in-process mock MCP server; passing rmcp boundary test.
  The client connects with `serve_with_lifecycle(ClientLifecycleMode::Discover
  { preferred_versions: vec![ProtocolVersion::V_2026_07_28] })`; discover runs
  at `Producer::start()` and a non-`2026-07-28` remote is mapped to a
  `CamelError` (fail-fast).
- **Exit-criteria:** `cargo build -p camel-component-mcp`; producer test calls
  a mock `2026-07-28` server and asserts the result Exchange; boundary test
  asserts no `rmcp::` import outside `src/adapter/`; **negative test** using an
  in-process rmcp server whose `supported_protocol_versions() =
  [V_2025_11_25]` asserts producer start fails with a `CamelError` (copy the
  `test_server_discover_http.rs` server shape).

### Phase 2: Server Consumer — shared listener + registries + dispatch

- **Goal:** Inbound server role: one shared Streamable-HTTP listener per bind,
  per-listener tool/resource registries, JSON-RPC dispatch, consumer routes.
- **Dependencies:** Phase 1; `camel-http`'s `ServerRegistry` pattern as the
  reference; rmcp `^3` server API.
- **Externally-visible types/interfaces:** `McpServerRegistry`,
  `McpToolRegistry`, `McpResourceRegistry`, consumer endpoints (`mcp:<server>/tool/<name>`,
  `mcp:<server>/resource/<name>`).
- **Deliverable:** Consumer routes that an MCP host can `server/discover`
  against, then list/call/read via per-request `_meta` (no session); the
  adapter's `ServerHandler` overrides `supported_protocol_versions() ->
  Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])` and provides a
  `DiscoverResult` (bind-derived identity `camel-mcp@<bind>` — the shared
  listener hosts many named servers — + capabilities + `[2026-07-28]`);
  pre-version
  rejection is delegated to rmcp's inline guard (`-32022`); the component adds
  one `warn!` per rejection via the handler seam; fail-closed auth; catalog
  caps; no reading or requiring of `Mcp-Session-Id`.
- **Exit-criteria:** Integration test where a real MCP client (rmcp)
  `server/discover`s, lists tools, calls one backed by a route, and reads a
  resource; a `server/discover` assertion that `supported == ["2026-07-28"]`;
  a rejection test that sends a `2025-11-25` `_meta` request and asserts
  JSON-RPC `-32022` plus a `warn!`; auth test refuses bind without
  `security_policy`; cap test rejects the 129th tool.

### Phase 3: DSL `mcp:` block lowering + metadata + bundle integration

- **Goal:** Declarative authoring surface and registration into the runtime.
- **Dependencies:** Phase 2; `camel-dsl` `rest.rs` lowering as the reference.
- **Externally-visible types/interfaces:** `RouteDslMcp`/`RouteDslMcpServer`/
  `RouteDslMcpTool`/`RouteDslMcpResource` AST types, the lowering pass,
  `ComponentMetadata` for `mcp:`, the crate bundle.
- **Deliverable:** A YAML/JSON `mcp:` block declares a server + tools/resources
  and lowers to running consumer routes; crate registered in the component
  bundle and discoverable by `camel-catalog`.
- **Exit-criteria:** DSL acceptance test runs a full `mcp:` block end-to-end
  (declare → lower → start → host calls tool → result); `openspec validate`
  passes; CONTEXT-MAP.md updated. (The DSL is transport- and version-agnostic;
  no `session`/`initialize` config keys exist to lower.)

## Alternatives considered

- **Fold MCP into `camel-component-llm`:** rejected — double-binds the
  chat/embed-shaped `LlmProvider` to a tool-transport protocol, and re-imports
  tool execution that ADR-0020 deliberately isolates (the LLM component never
  executes tools).
- **Fold MCP into `camel-http`:** rejected — conflates JSON-RPC semantics with
  HTTP server mechanics and camel-http's own trust/error taxonomy. Reuse its
  reqwest client and `ServerRegistry` pattern as dependencies; do not host MCP
  semantics there.
- **Generalized tool-dispatch SPI in `camel-component-api`:** rejected for v1
  — follows the `LlmProvider` precedent (provider traits stay local while there
  is a single consumer). Promote to SPI only if a second consumer appears.
- **stdio transport:** rejected — camel is not a process supervisor; lifecycle
  questions (restart, hot-reload of a subprocess) are out of scope.
- **MCP-native Prompts primitive:** deferred — duplicates `camel-template` /
  MiniJinja (ADR-0047). Revisit only if an MCP-Prompts-only need appears that
  the template component cannot serve.
