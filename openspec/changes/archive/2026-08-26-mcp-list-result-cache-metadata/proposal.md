# Proposal: mcp-list-result-cache-metadata

## Why

bd rc-kyv0: a `@modelcontextprotocol/client@2` (SDK v2, protocol `2026-07-28`)
peer performs discover + handshake against the camel `mcp:` server, then
`tools/list` fails client-side schema validation. The `2026-07-28` final
revision (SEP-2549) makes `ttlMs: number` and `cacheScope: 'public' | 'private'`
REQUIRED on list results; rmcp 3.1.4 models both as `Option` with
`skip_serializing_if = "Option::is_none"`, and the adapter builds results via
`with_all_items` which leaves both `None` — the fields are absent on the wire
and the TS SDK's zod schema rejects the whole result. `tools/call` validates
fine (non-paginated result type); `server/discover` is fine (non-optional
defaults in rmcp).

Secondary defect in the same ticket: a legacy `initialize` offering
`2025-11-25` makes rmcp's default handler warn "falling back to server
default" and answer a SUCCESS response with `2026-07-28` — a fallback path the
`mcp-component` spec explicitly forbids ("SHALL NOT implement any
pre-2026-07-28 compatibility or fallback path").

## What Changes

- `camel-component-mcp` server adapter (`src/adapter/server.rs`) only:
  - `tools/list` and `resources/list` results carry `ttlMs: 0` and
    `cacheScope: "private"` (SEP-2549: `0` = do not reuse; the catalog is
    dynamic under readiness gating, so non-cacheable is the correct default).
  - Legacy `initialize` with a pre-`2026-07-28` version offer is rejected
    fail-closed with `-32022` naming `["2026-07-28"]` (same shape as rmcp's
    inline guard), instead of the rmcp fallback-to-default success.
- Excluded: Prompts surface (already declined), client/producer role, rmcp
  version bump (3.1.4 stays), any new protocol feature, auth changes.

## Acceptance criteria

- Wire-level `tools/list` and `resources/list` JSON responses contain
  `"ttlMs": 0` and `"cacheScope": "private"` alongside the items array.
- A raw `initialize` request offering `2025-11-25` receives JSON-RPC error
  `-32022`; no success response with a server-default version is returned.
- Existing `mcp-component` scenarios (discover advertisement, `-32022` on
  pre-2026 requests, no `Mcp-Session-Id`) still pass.
- `cargo fmt --check`, `cargo clippy -p camel-component-mcp -- -D warnings`
  clean; crate test suite green.

## Risk budget

Low. Single crate, additive wire fields (strictly more compliant), one
behavior tightened from silent-fallback to fail-closed rejection — matching
what the spec already demands. Out of bounds: protocol renegotiation, caching
semantics beyond the non-cacheable default, stdio/SSE transports.
