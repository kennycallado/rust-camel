# MCP

The MCP component connects routes to Model Context Protocol servers. It has two roles. The Consumer role exposes tools and resources — plus an opt-in Skills catalog — on one shared Streamable-HTTP listener per bind. The Producer role sends `mcp:call` and `mcp:read` requests to remote MCP servers. The protocol baseline is `2026-07-28`, stateless: no `initialize` handshake, no sessions.

The `mcp:` DSL block declares a server catalog. It lowers each tool to an `mcp:<server>/tool/<name>` consumer route and each resource to an `mcp:<server>/resource/<name>` consumer route:

```yaml
mcp:
  - server:
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

One Streamable-HTTP listener serves each bind address. Every tool and resource route on that server shares it. The first consumer on a bind starts the listener. A later consumer with a conflicting config (`tls`, `allowed_hosts`, `skills_root`, catalog caps) is rejected. Registration of a name or URI held by a live owner is rejected; a dead owner's entry is replaced on restart (ADR-0068).

Security follows the unified transport auth kernel (ADR-0061): a server
without a `security_policy` starts **Public by default**, and the per-bind
exposure gate applies — a non-loopback bind serving any Public route
requires a `[binds."<addr>"] allow_public_exposure = true` acknowledgement
or start fails naming the bind (an acknowledged exposure warns permanently).
The DSL `mcp:` block carries one `security_policy` for the whole server and
propagates it to every lowered tool and resource route. The block has no per-tool
policy: bind, TLS, and policy are server properties, not per-tool URI options.
A tool that needs its own policy needs an explicit consumer route declared with
`from: mcp:<server>/tool/<name>` and its own `security_policy`. The kernel
authenticates each request per the route plan's credential sources (normalized
RFC 9110 headers) and installs the typed principal before the pipeline.
`tools/list` and `resources/list` use the same kernel gate: a Public entry is
listed to every caller, a protected entry only when the caller authenticates
(and, for request-independent policies, authorization grants); omission never
weakens call-time enforcement — a listed or omitted entry faces the same
kernel and pipeline gate at call time. Catalog caps (`max_tools`,
`max_resources`, default 128 each) reject surplus
registrations at start. The mcp-component spec owns the server-property rule
(`openspec/specs/mcp-component/spec.md` in the repository, outside the rendered
book).

Remote hosts that announce a protocol version other than `2026-07-28` get a `-32022` rejection and one `warn!` record per event. The server reads no `Mcp-Session-Id` header.

### Tool and resource behavior (steps/to passthrough)

A tool or resource declared without `to` or `steps` keeps the identity pipeline: a `tools/call` returns its own arguments (v1 behavior). An opt-in `to` or `steps` — not both — lowers onto the consumer route through the same step machinery routes and rest use. Declaring both fails lowering with a named error.

A business failure projects to a tool error result: `isError: true` and
a payload of exactly `{"error": <code>, "kind": <kind>}`. A resource read
returns error content with the same payload. The `message` field of a
`Business` error is log-only, and `data` is redacted diagnostics that are
never logged in clear. No projection sends either field to the client.
[ADR-0097](../adr/0097-business-error-envelope.md) owns the envelope contract.

## Skills

`skills_root` is an optional TOML key on `[mcp.servers.<name>]`. Without the key, the component loads no catalog, advertises no extension, and keeps the current behavior. With a directory, the listener loads one skill catalog at spawn, before binding. A missing root, a root that is not a directory, or an invalid skill stops the start with a typed error that names the cause. The listener then binds nothing and serves no partial catalog.

A skill is a direct child directory of the root that contains `SKILL.md`. `server/discover` then advertises Resources plus the `io.modelcontextprotocol/skills` extension with `directoryRead: false`. The server implements `skills/list` and `skills/get` through rmcp's SEP-2640 typed surface. Each skill entry carries a complete manifest: every regular file with its `skill://` URI, a `sha256:` digest of the served bytes, and the raw byte size. `skills/list` pages 50 skills at a time and never splits one manifest. A malformed or out-of-range cursor answers `-32602`.

A skill file is read-only and served through `resources/read`, which returns the stored bytes with a MIME type from the file extension. `resources/list` does not list skill files: the manifest is the listing channel. While a root is configured, the catalog owns the `skill://` namespace. A resource route that declares a `skill://` URI on that listener fails consumer start, so one URI has exactly one owner. A request URI with a `..` segment, raw or percent-encoded, answers `-32602` and never reaches the filesystem. A symlink whose canonical target lies outside the root fails the load with `SkillPathEscape`.

`get_or_spawn` compares `skills_root` with the live listener config as literal `Option<String>` equality. Two named servers on one bind must declare the same literal value. `/srv/skills` and `/srv/skills/` differ, and the later start is rejected with a named conflict.

The catalog follows the bind's `protected_seen` latch. Skills are public only until the bind has committed a non-Public route plan; the latch survives listener respawns. After that, at least one live non-Public plan must admit the caller. Admission uses the same kernel authentication and authorization tail that `tools/list` applies. A stopped protected route does not make the catalog public again, and no live protected plan keeps it hidden. A hidden caller sees an empty catalog. `skills/list` returns an empty page. `skills/get` and `resources/read` answer `-32602` with the same message as an unknown skill. The latch keys on registered route plans and governs only the plan-less Skills catalog. It is not the ADR-0061 Rule 8 bind-presence gate.

`skills/list`, `skills/get`, and skill-file reads emit `ttlMs: 0` and `cacheScope: "private"`. The manifest is fixed for one listener lifetime, but visibility depends on the caller and on the bind's plan history, so a reply is never shared across authorization contexts.

## Client (Producer)

`mcp:call?server=crm-prod&tool=lookup` sends the Exchange body as tool arguments. The reply content goes to the Exchange body. The `CamelMcpResult` header carries `{"is_error": <bool>, "content": <content>}`. The producer does not act on the flag. The route author decides what to do with a failed call.

Every post-connect request (`tools/call`, `resources/read`) runs under a request deadline. `request_timeout_secs` (whole seconds, minimum 1, default 120) bounds the call. A stalled server fails with an error naming the operation, the tool or resource, and the bound that fired (`tool call 'slow' exceeded the 120s request deadline`). A tool that legitimately runs longer sets a larger value. On deadline expiry the in-flight request is cancelled with a best-effort `notifications/cancelled` to the remote.

Startup is bounded before the first request. DNS resolution runs once to pin the remote's addresses and is limited to 5 s, failing with `timed out resolving remote host '<host>' after 5s`. The discover lifecycle that follows is limited by `connect_timeout_secs` (whole seconds, minimum 1, default 15), which covers the TCP connect inside rmcp's transport plus the `server/discover` POST and fails with `connect to remote '<server>' did not complete the discover handshake within 15s`. TCP connect keeps its own 5 s budget, inner to the connect deadline: it surfaces first only when the configured bound leaves more than 5 s remaining when the connect starts (true for the default 15 s), while a tighter bound fires the connect error instead. With defaults the worst case is about 20 s of bounded startup — DNS plus connect — where the connect phase was previously unbounded.

One limitation remains. On a wedged remote, dropping the discover future at the deadline does not reclaim the in-flight handshake's worker and socket: rmcp 3.9.0 awaits the bootstrap discover POST directly before its worker enters the cancellation-select loop, so those resources are released only when the remote answers or TCP gives up, not at the deadline (upstream limitation, bd rc-s4xod). Route start itself is bounded either way.

## Configuration

Server runtime config lives in `Camel.toml` under `[mcp.servers.<name>]` (bind, tls, security_policy, max_tools, max_resources, allowed_hosts, skills_root). Remotes live under `[mcp.remotes.<name>]` (url, transport, request_timeout_secs, connect_timeout_secs). Startup phases are individually bounded — DNS resolution ≤ 5 s, TCP connect ≤ 5 s, discover handshake ≤ `connect_timeout_secs` (default 15 s) — so route start cannot hang. A DSL `mcp:` block names a server; that name must match a TOML key or the consumer start fails. When the block declares `bind`/`tls`/`max_tools`/`max_resources`, those values ARE the runtime listener values; TOML and the DSL declaring the same key with different values fails startup naming both sources (ADR-0061 Rule 9 — never silent TOML-wins). TOML-only keys (`allowed_hosts`, `skills_root`) still apply.

**Reference**: [MCP crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-mcp/CONTEXT.md), [ADR-0060](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0060-mcp-first-class-component.md). Example source: [`examples/mcp-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/mcp-example).
