# MCP Example

One process plays both MCP roles over a real loopback HTTP hop:

1. **Server (Consumer) role** — exposes the `greet` tool on the shared
   Streamable-HTTP listener for server `demo` (bind `127.0.0.1:9100`).
2. **Client (Producer) role** — a timer route calls the same tool through
   `mcp:call?server=demo&tool=greet`, as it would call any remote MCP server.

The tool's input JSON Schema travels on the `from` URI's `schema` query param
(percent-encoded). Tool arguments travel as the Exchange body; the reply
content returns as the body and `CamelMcpResult` carries the `is_error` flag.

## Run

```sh
cargo run -p mcp-example
```

Expected output: one log line per timer tick with the tool reply body
(`Hello from the camel MCP server!`) and headers, including `CamelMcpResult`
with `is_error: false`.

External MCP hosts can also reach the server directly at
`http://127.0.0.1:9100/mcp` (Streamable HTTP, protocol baseline `2026-07-28`,
stateless Discover lifecycle: no sessions or `Mcp-Session-Id`; a legacy
`initialize` offer is answered fail-closed with `-32022` unless it offers the
baseline version).

## Files

- `src/main.rs` — route definitions and MCP bundle registration.
