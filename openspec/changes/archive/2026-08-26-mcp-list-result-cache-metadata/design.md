# Design: mcp-list-result-cache-metadata

## Approach

Two seams inside the rmcp `ServerHandler` impl for the shared listener, both
in `src/adapter/server.rs`:

1. **List-result cache metadata (SEP-2549).** rmcp 3.1.4's paginated result
   macro makes `ttl_ms` / `cache_scope` optional and skips them when `None`;
   `with_all_items` defaults both to `None`, so they never reach the wire.
   The fix calls `.with_ttl_ms(0).with_cache_scope(CacheScope::Private)` on
   the `ListToolsResult` (`list_tools`) and `ListResourcesResult`
   (`list_resources`) values. `ttlMs: 0` means "do not reuse" — correct here
   because the catalog is dynamic (readiness gating starts/stops routes), so
   any client-side caching could serve stale tool sets. `private` is the
   conservative cache scope (result must not be shared across authorization
   contexts), matching the per-route kernel auth model (ADR-0061). A named
   constant documents the SEP-2549 semantics. `prompts/list` is unaffected
   (already declined with a method error — no result body). The camel server
   speaks only `2026-07-28` (`SUPPORTED_VERSIONS`), so the fields are always
   required on the wire; no version-conditional emission is needed.

2. **Legacy initialize fail-closed.** rmcp 3.1.4's default `initialize`
   handling accepts a legacy `2025-11-25` offer, warns "falling back to
   server default", and answers success with `2026-07-28` — a fallback path
   the `mcp-component` spec forbids. The adapter overrides the
   `ServerHandler::initialize` method (signature:
   `initialize(request: InitializeRequestParams, context)` in rmcp 3.1.4's
   `handler/server.rs`): compare `request.protocol_version` against
   `SUPPORTED_VERSIONS`; on mismatch return
   `ErrorData::unsupported_protocol_version(...)` — the SDK's own
   constructor, which already emits the exact `-32022` shape with
   `data.requested` and `data.supported` (no new helper). On match, preserve
   the default handler's behavior: `context.peer.set_peer_info(request)`
   then return the server info. The override emits NO `warn!` of its own —
   the existing `warn_protocol_rejections` middleware already logs every
   `-32022` rejection exactly once, and explicit logging would duplicate it.
   All rmcp imports stay inside `src/adapter/`.

## Affected crates

- `camel-component-mcp`: `src/adapter/server.rs` — `list_tools`,
  `list_resources`, new `initialize` override, one named constant for the
  non-cacheable metadata. Tests: wire-level HTTP JSON-RPC assertions in
  `tests/` (raw request/response like the existing auth tests — assert
  `"ttlMs": 0`, `"cacheScope": "private"` on list responses; `-32022` on a
  legacy initialize).

## Architecture boundaries

- **Adapter confinement (ADR-0020):** every rmcp type touched
  (`CacheScope`, `ErrorData`, initialize params) stays inside
  `src/adapter/`; no MCP-shaped type crosses the crate boundary
  (ADR-0060).
- **Trust boundary (ADR-0032 / ADR-0061):** no auth or dispatch changes;
  `cacheScope: private` aligns the wire metadata with the existing
  per-authorization-context security model rather than weakening it.
- **Data/control plane:** server list responses are control-plane surface
  for peers; the fix only makes the emitted JSON schema-complete.

## Phases

Single-phase: one coherent slice (wire-shape compliance + one fail-closed
tightening), one crate, no ordering dependencies worth milestone grouping.
