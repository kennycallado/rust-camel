# Proposal: mcpblank — reject blank MCP bind/name at load (bd rc-sghtz)

## Why

`camel lint` (R-SCHEMA, ROUTE_SCHEMA) emits 0 diagnostics for an `mcp:` block
whose `server.name` or `server.bind` is blank (`""` or whitespace-only), but the
runtime rejects both later:

- blank `bind` — consumer-config parse (`crates/components/camel-component-mcp/src/config.rs`,
  `mcp.declared.bind` `trim().is_empty()` check) fails at consumer start;
- blank `name` — DSL lowering (`camel-dsl/src/mcp.rs`, `validate_mcp_name`
  charset `[A-Za-z0-9._-]+`) fails at route build.

This is the same schema/runtime contract-drift class as rc-n3t73 (blank MCP TLS
paths, fixed 4c8a0563): the loader should reject what the runtime will
(fail-early principle). Lint currently approves documents that boot rejects.

## What Changes

Mirror the rc-n3t73 pattern exactly (no new validation mechanism):

1. `crates/camel-dsl/src/mcp.rs` — add a blank-reject deserializer for
   `RouteDslMcpServer.bind`, `RouteDslMcpServer.name`, and (sweep, same blank
   class) `RouteDslMcpTool.name` / `RouteDslMcpResource.name`, plus the
   `schemars(regex(pattern = r"\S"))` schema mirror on each field.
2. Regenerate schema copies (`cargo xtask schema`): `schemas/dsl/route-schema.json`
   + `crates/camel-lint/schema/route-schema.json`.
3. Update the stale rschema.rs comment ("the schema's only pattern keywords are
   the two MCP TLS path fields") — the pattern-keyword set grows.
4. R-SCHEMA tests (blank bind/name, empty + whitespace classes) + lint-corpus
   negative-witness fixture + baseline entry.
5. Spec delta: `route-lint` — new requirement sibling of "R-SCHEMA rejects
   blank MCP TLS path values with runtime parity".

## Acceptance Criteria

- Blank (`""`, whitespace-only) `server.bind` / `server.name` / tool+resource
  `name` values fail document LOAD with a serde error naming the field
  (`"<field> must not be empty"`), and emit R-SCHEMA Error(s) anchored on the
  blank value in lint.
- Valid non-blank values load and lower exactly as before; runtime code paths
  (`camel-component-mcp` config, lowering) are untouched — runtime behavior
  unchanged.
- `cargo xtask schema --check` passes (copies synced).

## Impact / Risk Budget

Crates: camel-dsl (source), camel-lint (schema copy + comment + tests),
camel-cli (corpus fixture/baseline). Spec: route-lint delta. Risk: LOW —
additive deserializer attributes; the raw-vs-trimmed return-value question is
settled in design.md (return raw, unlike the TLS sibling's trimmed return,
because bind/name runtime consumers use the raw string).
