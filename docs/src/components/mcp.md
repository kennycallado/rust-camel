# MCP

The MCP component connects routes to Model Context Protocol servers. It has two roles. The Consumer role exposes tools and resources on one shared Streamable-HTTP listener per bind. The Producer role sends `mcp:call` and `mcp:read` requests to remote MCP servers. The protocol baseline is `2026-07-28`, stateless: no `initialize` handshake, no sessions.

The `mcp:` DSL block declares a server catalog. It lowers each tool to an `mcp:<server>/tool/<name>` consumer route and each resource to an `mcp:<server>/resource/<name>` consumer route:

```yaml
mcp:
  server:
    name: crm
    bind: 127.0.0.1:9100
    security_policy: { roles: [mcp-client] }
  tools:
    - name: lookup
      input_schema: { type: object, properties: { id: { type: string } }, required: [id] }
  resources:
    - name: customers
      uri: crm://customers
```

## URI

```text
mcp:<server>/tool/<name>?schema=<schema>
mcp:<server>/resource/<name>?uri=<uri>
mcp:call?server=<remote>&tool=<name>
mcp:read?server=<remote>&uri=<uri>
```

Consumer URIs come from DSL lowering. The `schema` and `uri` values travel on the query string, never in Exchange headers or bodies. Producer URIs dispatch exactly one JSON-RPC request per Exchange. The producer never auto-loops an LLM call. The route owns every dispatch decision.

## Server (Consumer)

One Streamable-HTTP listener serves each bind address. Every tool and resource route on that server shares it. The first consumer on a bind starts the listener. A later consumer with a conflicting config (`tls`, `allowed_hosts`, catalog caps) is rejected. Registration of a duplicate tool name or resource URI is also rejected.

Security follows the unified transport auth kernel (ADR-0061): a server
without a `security_policy` starts **Public by default**, and the per-bind
exposure gate applies — a non-loopback bind serving any Public route
requires a `[binds."<addr>"] allow_public_exposure = true` acknowledgement
or start fails naming the bind (an acknowledged exposure warns permanently).
The DSL block's `security_policy` propagates to every lowered tool and
resource route; the kernel authenticates each request per the route plan's
credential sources (normalized RFC 9110 headers) and installs the typed
principal before the pipeline. Catalog caps (`max_tools`, `max_resources`,
default 128 each) reject surplus registrations at start.

Remote hosts that announce a protocol version other than `2026-07-28` get a `-32022` rejection and one `warn!` record per event. The server reads no `Mcp-Session-Id` header.

## Client (Producer)

`mcp:call?server=crm-prod&tool=lookup` sends the Exchange body as tool arguments. The reply content goes to the Exchange body. The `CamelMcpResult` header carries `{"is_error": <bool>, "content": <content>}`. The producer does not act on the flag. The route author decides what to do with a failed call.

## Configuration

Server runtime config lives in `Camel.toml` under `[mcp.servers.<name>]` (bind, tls, security_policy, max_tools, max_resources, allowed_hosts). Remotes live under `[mcp.remotes.<name>]` (url, transport). A DSL `mcp:` block names a server; that name must match a TOML key or the consumer start fails. When the block declares `bind`/`tls`/`max_tools`/`max_resources`, those values ARE the runtime listener values; TOML and the DSL declaring the same key with different values fails startup naming both sources (ADR-0061 Rule 9 — never silent TOML-wins). TOML-only keys (`allowed_hosts`) still apply.

**Reference**: [MCP crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-mcp/CONTEXT.md), [ADR-0060](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0060-mcp-first-class-component.md). Example source: [`examples/mcp-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/mcp-example).
