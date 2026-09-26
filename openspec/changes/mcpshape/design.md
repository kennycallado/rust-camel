# Design: mcpshape

## Context

bd rc-ap58 asks for parse-time shape validation of the `mcp:` DSL
block. Recon on current main (f4c3b4f6):

- **Names: DONE.** `validate_mcp_name` (mcp.rs) runs at lowering for
  server, tool, and resource names against the closed charset
  `[A-Za-z0-9._-]+` (a superset rejection of `/ ? # %` — percent
  included, since it is not in the charset). Landed by mcpchars
  (spec'd in `route-lint` "R-SCHEMA rejects MCP names outside the
  runtime charset with parity", whose "Charset class stays
  lowering-owned at load" scenario pins the lowering rejection).
  This mission adds only the missing DSL unit-test witnesses for the
  `/`, `#`, `%` classes and the server/resource kinds (test-only; the
  behavior and its spec are already in place).
- **input_schema: GAP.** `RouteDslMcpTool.input_schema` is an untyped
  `serde_json::Value`; lowering percent-encodes
  `input_schema.to_string()` into the `schema` query param with no
  shape check. The first rejection happens at consumer start
  (`camel-component-mcp/src/endpoint.rs`, `!input_schema.is_object()` →
  `McpError::Endpoint("... 'schema' parameter that is not a JSON
  object")`).

## Decisions

### D1 — Validation lives at LOWERING, not at deserialization

The name-charset precedent (mcpchars D1, pinned by a route-lint spec
scenario) keeps the AST permissive at load and rejects at lowering.
input_schema follows the same split:

- `serde_json::Value` must keep accepting any JSON (it is the field's
  declared type; a serde `deserialize_with` hook rejecting non-objects
  would change load behavior the mcpchars spec pins as
  "lowering-owned").
- Struct-literal construction (Rust API users, tests) bypasses serde
  entirely — a deserialize-only check would miss those; lowering
  catches every path (`expand_mcp_into` funnels YAML, JSON, and direct
  calls through `lower_all_mcp_to_routes`).

### D2 — Mirror the consumer predicate exactly

The runtime check is `input_schema.is_object()`. The lowering check is
the identical predicate — no stricter grammar (e.g. no "must contain
`type: object`" requirement, no jsonschema meta-validation): the
consumer accepts any JSON object as the schema param, so the DSL must
not reject what the runtime would accept. Zero drift by construction;
any future tightening happens on both sides or neither.

### D3 — Error shape: named, value-identifying, `CamelError::RouteError`

Consistent with `validate_mcp_name` (the existing lowering error
style): `CamelError::RouteError` with a message that names the tool and
the actual JSON kind of the offending value, e.g.
`mcp tool 'lookup' input_schema is invalid: must be a JSON object, got
a JSON string`. Lowering has no source spans; naming the offending key
and value IS the established location mechanism for this pass (same as
`validate_mcp_name`). A small named helper (`validate_tool_input_schema`)
with a doc comment citing rc-ap58 and the endpoint.rs parity, mirroring
`validate_mcp_name`'s documentation pattern.

### D4 — No lint/schema-side changes in this mission

The generated route schema (`route-schema.json`) and the R-SCHEMA lint
are untouched: `input_schema` stays schema-side `any` (a JSON Schema
`true`), so `schema --check` must pass byte-identical (no regen).
Aligning R-SCHEMA to also flag non-object `input_schema` at lint time
is a real follow-up surface, but it drags corpus fixtures + baseline
entries + a route-lint delta spec — out of this mission's order (the
order scopes lowering rejection only). Filed as a deferral with
follow-up context in the park report.

### D5 — Defense-in-depth preserved

The `endpoint.rs` consumer-start object check stays exactly as is.
Early rejection is additive; nothing is removed late-failure-side.

## Phase-exit criteria (single phase)

All rejection and happy-path tests green in camel-dsl; `schema
--check` passes without regen; the mcp-component delta validates; docs
table carries the object-ness note.
