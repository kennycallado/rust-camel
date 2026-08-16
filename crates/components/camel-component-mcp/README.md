# camel-component-mcp

MCP (Model Context Protocol) component for [rust-camel](https://crates.io/crates/camel-api).

## Features

- **Server (Consumer) role** — expose tools and resources on one shared Streamable-HTTP listener per bind address
- **Client (Producer) role** — `mcp:call` / `mcp:read` dispatch one JSON-RPC request per Exchange to a remote MCP server
- **Route-owned dispatch** — the component never auto-loops; every tool decision belongs to a route
- **DSL catalog** — the `mcp:` block lowers each tool to an `mcp:<server>/tool/<name>` consumer route and each resource to an `mcp:<server>/resource/<name>` route
- **Fail-closed server config** — mandatory `security_policy`, catalog caps (`max_tools` / `max_resources`, default 128), duplicate-name and bind-conflict rejection at start
- **Protocol baseline `2026-07-28`** — stateless: no `initialize` handshake, no sessions; other versions get a `-32022` rejection with one `warn!` per event
- **Lossless resource encoding** — UTF-8 resources read as text; binary resources round-trip as base64 blobs with exact bytes
- **Structured error flag** — `CamelMcpResult` header carries `{"is_error": <bool>, "content": <content>}`; the producer never acts on the flag

## URI

```text
# Consumer (server role) — from DSL lowering
mcp:<server>/tool/<name>?schema=<input JSON Schema>
mcp:<server>/resource/<name>?uri=<resource URI>

# Producer (client role)
mcp:call?server=<remote>&tool=<name>
mcp:read?server=<remote>&uri=<uri>
```

The `schema` and `uri` values travel on the query string (percent-encoded), never in Exchange headers or bodies.

## Configuration

Server runtime config lives in `Camel.toml`:

```toml
[mcp.servers.crm]
bind = "127.0.0.1:9100"
security_policy = { require = "auth" }
max_tools = 128

[mcp.remotes.crm-prod]
url = "https://crm.example.com/mcp"
transport = "streamable-http"
```

A DSL `mcp:` block names a server; that name must match a `mcp.servers.<name>` key or the consumer start fails.

## Example

See [`examples/mcp-example`](../../examples/mcp-example) — one process plays both roles over a real loopback HTTP hop.

## Reference

- [Crate CONTEXT](./CONTEXT.md) — purpose, endpoints, registries, adapter confinement
- [ADR-0060](../../docs/adr/0060-mcp-first-class-component.md) — charter: first-class component decisions
- rmcp is confined to `src/adapter/` (ADR-0020 pattern)
