# ADR-0060: MCP as a First-Class Component

**Date:** 2026-08-16
**Status:** Accepted
**Amends:** none
**Cross-refs:** ADR-0020 (adapter confinement), ADR-0032 (exchange-data trust
boundary), ADR-0033 (fail-closed security defaults), ADR-0038 (per-item DoS
caps), ADR-0052 (diagnostic endpoint posture)
**Origin:** OpenSpec change `add-mcp-component` (bd rc-elcd)

## Context

MCP (Model Context Protocol) tools are the natural extension surface for the
LLM component (`camel-component-llm`). Without a transport, a route can only
request a tool call. It cannot dispatch one to an external tool server, nor
expose its own tools to an MCP host.

Two folds were rejected. Folding MCP into `camel-component-llm` double-binds
the chat/embed-shaped `LlmProvider` to a tool-transport protocol. Folding it
into `camel-http` conflates JSON-RPC semantics with HTTP server mechanics.
MCP is its own bounded concern.

MCP is therefore a first-class Component (scheme `mcp:`), on the same footing
as `camel-http` or `camel-kafka`.

## Decision

### Rule 1: First-class Server + Host component

`camel-component-mcp` owns both MCP roles. Creation path disambiguates the
role, mirroring the split `camel-http` uses for HTTP server vs client:

- **Consumer (server)**: exposes tools and resources over Streamable HTTP.
- **Producer (client)**: dispatches tool calls and resource reads to remote
  MCP servers.

### Rule 2: Route-owned tool dispatch

The component never auto-loops. The LLM component emits `ChatEvent::ToolCall`
intent. The route decides to route to `mcp:call`. The route reshapes the
result into `ChatRole::Tool` for the next turn.

The MCP producer is a dispatch *target*, never a *decider*. It issues exactly
one JSON-RPC request per Exchange.

### Rule 3: DSL-lowered catalog (declaration surface, TOML-owned runtime)

A `mcp:` DSL block declares a named server, its tools, and its resources. The
block lowers each tool to an `mcp:<server>/tool/<name>` consumer route and
each resource to an `mcp:<server>/resource/<name>` consumer route.

This is not a full `rest:` → `http:` analogue. `rest:` lowering emits routes
with processing steps (unmarshal, marshal, status injection); the `mcp:` block
is a step-less *catalog declaration* — it registers names, schemas, and
resource URIs on the server. Its lowered routes carry no processing steps
(`deny_unknown_fields` forbids steps inside the block), so a `tools/call` on a
bare lowered route echoes its arguments through the identity pipeline. Tool
and resource behavior is expressed by explicit routes whose `from:` is an
`mcp:` URI and that attach their own steps. Steps passthrough inside the
`mcp:` block is a tracked limitation (bd rc-23y2), not a design goal.

Runtime server config is TOML-owned. `mcp.servers.<name>` (`McpServerConfig`)
is the sole source of runtime server config: `bind`, `tls`,
`security_policy`, `max_tools`, `max_resources`, `allowed_hosts`. The DSL
block's server fields (`bind`, `tls`, `security_policy`, `max_tools`,
`max_resources`) are a declaration/validation surface only — they document
declared intent and mirror the TOML keys (declaration parity), but their
values do NOT flow to the runtime. Divergence between a DSL `bind` and the
TOML `bind` for the same name is not reconciled: TOML wins.

The DSL server `name` is the coupling key: it MUST match a
`mcp.servers.<name>` key or consumer START fails cleanly (`McpError::Endpoint`
naming the missing server). A DSL block whose server is absent from TOML
config lowers routes that fail at consumer start, never silently.

### Rule 4: rmcp confined to `src/adapter/`

All rmcp imports live in `src/adapter/` (ADR-0020). Project-owned traits
(`McpClient`, the server handler) carry Camel-shaped types across the
boundary. The JSON-RPC envelope dies at the trait boundary. A boundary test
scans for rmcp references outside the allowlist.

`McpServerMap` (`HashMap<String, Arc<dyn McpClient>>`) is the client-role
twin of the LLM `ProviderMap`. It is a per-component map, not a global
registry (ADR-0020).

### Rule 5: Streamable HTTP only

The only transport is Streamable HTTP. The `transport` config key accepts
exactly the string `streamable-http`; anything else fails at deserialization
(see Implementation decisions). The deprecated HTTP+SSE transport is excluded
at the Cargo level: the rmcp `transport-sse-*` features are never enabled.

### Rule 6: Protocol baseline `2026-07-28`, stateless

The component speaks exactly MCP `2026-07-28`, the first stateless revision.
No `initialize` handshake. No protocol sessions. No `Mcp-Session-Id`.
Per-request protocol version and client capabilities travel in `_meta`.

The client connects with `ClientLifecycleMode::Discover`. A remote that does
not speak `2026-07-28` fails fast at producer start as a `CamelError`. The
server overrides `supported_protocol_versions()` to the single baseline;
rmcp's inline guard rejects other peers with JSON-RPC `-32022`.

### Rule 7: Exclusions

- **Prompts**: deferred. Duplicates `camel-template` / MiniJinja (ADR-0047).
- **stdio transport**: rejected. Camel is not a process supervisor.
- **Protocol sessions**: rejected. The baseline is stateless.
- **Legacy transports (SSE)**: rejected at the Cargo level.
- **Resource subscriptions**: rejected. v1 serves read-on-demand only.

### Rule 8: Security posture

MCP server inputs are adversary-controlled Exchange data. They cross the
ADR-0032 trust boundary and drive the data plane. The server endpoint is
therefore NOT a diagnostic endpoint: ADR-0052 exempts only non-data-plane
inbound. It does not inherit the ADR-0052 unauthenticated convention.

Instead (ADR-0033 Require-Explicit-Choice) the server uses two gates. The
bind gate is presence-only: a `mcp.servers.<name>.security_policy` key must
be set, or the consumer refuses to start (fail-closed). The enforcement gate
is route-level: the adapter copies the inbound HTTP request headers onto the
Exchange, and each request runs through the route's `SecurityPolicy`
(camel-api, ADR-0033) before any route step. A `mcp:` block's server
`security_policy` propagates to every lowered tool and resource route. The
TOML `security_policy` stays a presence gate only. Loopback bind is
preferred. A non-loopback bind emits a `warn!` at startup (ADR-0052 rule 3).

Catalog cardinality is capped. `max_tools` / `max_resources` default to 128
each and ride the ADR-0038 per-item config channel: hardened but raisable, no
global disable switch. Breach rejects the (N+1)th route at consumer start
with a clean `CamelError`. Silent truncation is forbidden (ADR-0038).

### Rule 9: No cross-crate types

MCP types stay in the component crate. No MCP-shaped type crosses into
`camel-api` or `camel-component-api`. The client Exchange contract is
`Body` + headers (`CamelMcpToolCall`). The crate boundary is this ADR's own
rule, following the ADR-0020 confinement precedent.

## Implementation decisions (carried from implementation review)

The following decisions surfaced during implementation. They are recorded
here as carried reviewer obligations — explicit decisions, not omissions.

### Remote config fails fast at deserialization

Remote entries (`remotes` map) use `#[serde(deny_unknown_fields)]`. The
`transport` field is an enum whose deserializer accepts only
`streamable-http`. A typo or a legacy transport string fails at config load.
Unknown keys fail too.

URL strings are NOT validated at startup. They are validated at first use —
connect time — matching the `LlmBundle` precedent. Server entries are the
opposite: `validate_server_policy` runs at consumer start, fail-closed
(security policy present, bind is an IP literal, caps nonzero).

### Producer propagates `is_error` faithfully

The producer receives `McpToolResult` with a structured `is_error: bool`. It
carries the flag and the content into the Exchange body and the
`CamelMcpResult` header. It does NOT act on the remote's error flag.

This is an explicit decision, not an omission. The route author decides how
to handle a remote-reported failure. The component must not invent policy.

### Same-process both-roles start order

The producer connects at route `start()` and the discover connect is not
retried. In a same-process deployment that serves both roles, the server
consumer must start before any producer route that targets it. Operators
control this with route definition order or `startup_order`. A producer that
starts first fails fast and stays not-ready; it does not silently recover.

### Shared-listener bind conflicts fail closed

One shared listener serves each bind. A later consumer whose config conflicts
with the live listener — different `allowed_hosts`, TLS shape, or catalog
caps — is rejected with `McpError::Endpoint`. Duplicate tool/resource
registration is rejected atomically under the registry lock. Two concurrent
same-name starts cannot silently overwrite the first registration.

### Non-object input schemas advertise as `{}`

A non-object input schema cannot be expressed in rmcp's catalog shape. It
degrades to `{}` at the catalog, while call-time validation accepts anything
for such a schema. This is a known drift between advertisement and
enforcement. Hardening is tracked in bd rc-ap58.

## Consequences

- MCP tool dispatch composes in routes without an LLM auto-loop.
- rmcp churn is confined to `src/adapter/` (ADR-0020).
- The DSL `mcp:` block gives operators a declarative server catalog.
- Security posture is fail-closed: no bind without policy, caps enforced.
- The protocol baseline is single-version. Legacy peers are rejected, not
  negotiated.
- Prompts and subscriptions stay out of v1.

## Options considered

### Fold MCP into `camel-component-llm`

Rejected. Double-binds the chat/embed-shaped `LlmProvider` to a
tool-transport protocol. Re-imports tool execution that ADR-0020 isolates.

### Fold MCP into `camel-http`

Rejected. Conflates JSON-RPC semantics with HTTP server mechanics. Reuse its
reqwest client and `ServerRegistry` pattern as dependencies; do not host MCP
semantics there.

### Generalized tool-dispatch SPI in `camel-component-api`

Rejected for v1. Follows the `LlmProvider` precedent: provider traits stay
local while there is a single consumer. Promote to SPI only if a second
consumer appears.

## Self-grill record

**Questions generated:**

1. [glossary] Does "MCP Server Consumer" collide with the Components
   "Consumer" term?
2. [sharpen] Does the security posture contradict ADR-0052 (unauthenticated
   by convention)?
3. [cross-ref] Does route-owned dispatch need an ADR at all, or is it plain
   component behavior?
4. [scenario] What happens when a later consumer on the same bind differs in
   TLS or caps?

**Answers:**

1. [glossary] No. "Consumer" names the runtime role (inbound adapter).
   "MCP Server Consumer" names the MCP-specific instance of that role. The
   terms differ at the identifier level; no glossary collision.
2. [sharpen] No. ADR-0052 governs diagnostic endpoints — non-data-plane
   metadata. An MCP server carries tool invocations from a host; that is data
   plane, crossing the ADR-0032 boundary. It must authenticate. The
   loopback-preference and the non-loopback `warn!` transfer (ADR-0052 rule
   3); the unauthenticated-by-convention rule does not.
3. [cross-ref] No prior ADR governs tool dispatch semantics. The LLM
   component never executes tools; nothing in ADR-0020 or the EIP corpus
   decides who calls `mcp:call`. Recording it prevents a future auto-loop
   "convenience" from shipping as default behavior.
4. [scenario] The second consumer is rejected at `get_or_spawn` with
   `McpError::Endpoint` naming the conflict. Fail-closed, never silent.

**Outcome:** approve as new ADR (0060). MCP as first-class Server+Host
component; route-owned tool dispatch; DSL-lowered catalog; rmcp confined to
`src/adapter/` (ADR-0020); Streamable HTTP only; protocol baseline
`2026-07-28` stateless. Exclusions: Prompts, stdio, sessions, legacy
transports, subscriptions. Implementation decisions recorded as carried
obligations; the schema-advertisement drift is tracked in bd rc-ap58.
**Self-grill mode:** manual (4 principles L6: glossary consistency, conflict
with existing ADRs, redundancy with implicit ADRs, numbering — 0060 next free
after 0058).
