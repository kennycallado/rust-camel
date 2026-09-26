# Proposal: mcpshape

## Why

`lower_all_mcp_to_routes` (`crates/camel-dsl/src/mcp.rs`) accepts any
`input_schema: serde_json::Value`. A tool declaring a non-object schema
(string, array, number, boolean, null) lowers cleanly into the
`mcp:<server>/tool/<name>?schema=...` URI and fails only at consumer
start, when `camel-component-mcp` (`endpoint.rs`) decodes the schema
param and rejects `!input_schema.is_object()` with
`McpError::Endpoint`. Fail-loud, but late: the operator learns the
malformed declaration at route start, not at document parse.

The name half of bd rc-ap58 (reject `/ ? # %` and friends in
server/tool/resource names at lowering) already landed with the mcpchars
charset (`validate_mcp_name`, closed charset `[A-Za-z0-9._-]+`, spec'd in
`route-lint` R-SCHEMA). The remaining gap is the input_schema shape.

## What Changes

- Add an object-ness validation for `tools[].input_schema` at MCP
  LOWERING (parse time): a non-object schema fails
  `lower_all_mcp_to_routes` with a named, actionable error (tool name +
  actual JSON kind), mirroring the existing `validate_mcp_name` error
  style.
- The check mirrors the consumer's own predicate
  (`serde_json::Value::is_object`, `endpoint.rs`) exactly — zero drift
  by construction. The consumer-side check stays (defense-in-depth);
  early rejection is additive.
- Load behavior is unchanged (design parity with the name charset):
  a non-object schema deserializes fine into `serde_json::Value`; the
  rejection is lowering-owned.
- Extend camel-dsl unit tests: both rejection classes (non-object
  schema kinds; remaining name-charset classes `/`, `#`, `%` and
  server/resource kinds) plus the happy path.
- One-line docs note in `docs/src/yaml-dsl/step-verbs.md` (mcp block
  table): `input_schema` must be a JSON object.

## Impact

- Affected specs: `mcp-component` (Requirement "MCP DSL block and
  lowering" — MODIFIED, adds the object-ness scenario; existing
  scenarios carried).
- Affected code: `crates/camel-dsl/src/mcp.rs` only (validation +
  tests). No schema attributes touched — the generated route schema and
  R-SCHEMA lint are unchanged (lint-side alignment recorded as a
  deferral, see design.md).
- bd: rc-ap58 (closes at landing).
