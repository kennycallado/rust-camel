# Proposal: mcp-request-deadline

## Why

A remote MCP server that accepts the connection and then never answers parks
an Exchange forever. `camel-component-mcp` bounds TCP connect and DNS
resolution at 5 s (`hardened_client_builder`, `src/adapter/dns_pin.rs:199-205`),
and rmcp 3.9.0 bounds its cancellation/reply POSTs and session recovery at 5 s
defaults — but every post-connect request (`tools/call`, `resources/read`)
awaits its response unbounded. Verified against rmcp 3.9.0 (Cargo.lock): the
generated client requests run with `PeerRequestOptions::no_options()`, whose
`timeout` is `None`, and no client-level or transport-level default exists.

This is audit finding rc-kpf3m (row 17 of rc-s8itn): "connect 5s; no total, no
read timeout". The docs (`docs/src/components/mcp.md`) state no timeout, and
the DNS error names the operation but not its 5 s bound (audit verdict:
PARTIAL honesty).

## What Changes

- `McpRemoteConfig` gains `request_timeout_secs` (u64, default **120**,
  zero and non-numeric values such as `""` rejected at parse time, each
  message naming the option). The component's sibling options (`timeout_secs`
  in exec and llm) fix the `*_secs` unit convention.
- `RmcpClient` stores the deadline at connect and applies it at each of the
  two post-connect request sites (`call_tool`, `read_resource` — the adapter
  performs no prompt reads). rmcp offers no compatible transport-level knob:
  its per-request `PeerRequestOptions` would bypass the component's existing
  call paths (MRTR round-driving, resource-read cache). The deadline is
  therefore applied at the component boundary with `tokio::time::timeout`.
- Timeout errors name the operation, the tool/resource, and the bound that
  fired (for example: `tool call 'search' exceeded the 120s request
  deadline`).
- The DNS-resolution error names its 5 s bound. The TCP connect error names
  the 5 s connect bound too: `map_connect_error` inspects rmcp's
  `ClientInitializeError::TransportError` (whose `DynamicTransportError.error`
  is public) by downcasting to rmcp's `StreamableHttpError<reqwest::Error>`
  wrapper (the real boxed type — reqwest errors enter it as the
  `Client` arm) and checking `is_connect() && is_timeout()` on the wrapped
  cause — no fork needed. DNS (5 s) and TCP connect (5 s) are documented as
  separate phase limits, not as one overall startup deadline.
- `docs/src/components/mcp.md` documents the option, its default, and both
  bounds (connect/DNS 5 s, request 120 s default).
- Spec delta on `openspec/specs/mcp-component/`.

Behaviour change, deliberate: requests are bounded by default where today
they never time out.

Out of scope (residual, filed separately as rc-9kpye): the connect-time
`server/discover` POST is unbounded inside rmcp's discover lifecycle —
bounding it is a connect-time change, which this change excludes.

## Acceptance criteria

- A stalled remote trips the deadline with an error naming the operation, the
  tool or resource, and the configured bound.
- A fast call is unaffected by the deadline wrapper.
- With the option absent, the default (120 s) bounds the call.
- `request_timeout_secs: 0` and `request_timeout_secs: ""` fail config parse
  naming the option.
- `docs/src/components/mcp.md` states the option, the default, and what it
  bounds.
- `cargo test -p camel-component-mcp` green.

## Risk budget

- Default 120 s is generous by intent: remote tool calls legitimately run
  minutes (LLM-backed agents, batch reads). A stalled server now costs 2 min
  instead of forever.
- No wire or protocol change; no new dependency; the 5 s connect/DNS bound
  and DNS pinning stay untouched.
- Existing tests construct `McpRemoteConfig` literally; they gain the new
  field (mechanical).

Bd: rc-kpf3m
