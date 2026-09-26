# Design: mcpsteps

## Context

`lower_all_mcp_to_routes` (`crates/camel-dsl/src/mcp.rs`) lowers every
`mcp:` block tool/resource to a consumer route via `consumer_route()`,
which hardcodes `steps: Vec::new()`. The rest surface (`rest.rs`
`lower_operation`) already solves optional pipeline declaration: a
`RouteDslRestOperation` carries `to: Option<String>` XOR
`steps: Vec<RouteDslStep>`, validates the XOR at lowering, and lowers the
`to` shorthand to a single `RouteDslStep::To(ToStep { to, parameters })`.

## Goals / Non-Goals

- Goals: opt-in `steps`/`to` on tool/resource declarations; reuse the rest
  lowering shape; byte-identical v1 default; schema + docs aligned.
- Non-Goals: consumer-side changes (`camel-component-mcp` untouched — the
  pipeline is standard route machinery); auto-wrapping steps with
  unmarshal/marshal (unlike rest, the MCP consumer submits typed input and
  v1 defines no data-format steps); any new step validation mechanism.

## Decisions

- **D1 — field shape.** `to: Option<String>` and
  `steps: Vec<RouteDslStep>` on BOTH `RouteDslMcpTool` and
  `RouteDslMcpResource`, each `#[serde(default)]`. `deny_unknown_fields`
  stays, so unknown keys keep failing and both fields stay optional. The
  schemars derive emits them in `route-schema.json` automatically (non-
  verbose doc comments, mirroring the 264/271/280 style).

- **D2 — XOR guard at lowering, absence allowed.** Declaring BOTH `to` and
  `steps` on one declaration fails `lower_all_mcp_to_routes` with
  `CamelError::RouteError` naming the kind and the declaration — wording
  mirrors rest: `mcp tool '{name}' cannot have both 'to' and 'steps'`.
  UNLIKE rest (which requires `to` or `steps`), NEITHER is required: the
  v1 identity default remains a first-class outcome. On the YAML path the
  error surfaces through `DeserializeStageError::Shape` (`yaml.rs:153`)
  exactly like the 280 name/schema rejections — the parse entry point
  wraps it with document context; on the JSON path (`json.rs:44`) it
  returns directly.

- **D3 — reuse, do not fork, the step machinery.** Effective steps =
  `to` shorthand → `vec![RouteDslStep::To(ToStep { to, parameters:
  BTreeMap::new() })]`; else explicit `steps` cloned verbatim; else empty.
  This is the identical construction `rest.rs` performs (lines 363-370) —
  no mcp-owned step translation, no wrapping, no reordering. Invalid step
  SHAPES need no new check: `RouteDslStep` is an untagged enum whose every
  variant struct is `deny_unknown_fields`, so a malformed step fails at
  document load with the parser's location-carrying serde error (the
  load/lowering ownership split is the 280 pattern: shape errors are
  load-owned, cross-field rules are lowering-owned).

- **D4 — `consumer_route` signature.** Gains a `steps: Vec<RouteDslStep>`
  parameter; all call sites pass the effective steps. No other
  `RouteDslRoute` field changes (`parameters`/`auto_startup`/... stay as
  v1).

- **D5 — byte-identical default.** Without `to`/`steps`, the lowered route
  equals v1 exactly (same id, same from-URI, empty steps). Existing
  camel-dsl mcp tests must pass UNCHANGED — they are the regression
  witness for the identity pipeline.

- **D6 — schema regen.** `cargo xtask schema` regenerates
  `schemas/dsl/route-schema.json` (+ TS types) and the
  `crates/camel-lint/schema/route-schema.json` copy; `cargo xtask schema
  --check` must be green. Both new fields appear as optional — no pattern
  constraints needed beyond what `ToStep`/`RouteDslStep` already carry
  (the 264/271 charset patterns govern mcp NAMES, which are unchanged).

- **D7 — docs.** `crates/camel-dsl/README.md` MCP DSL section: replace the
  "step-less catalog declaration" sentence with the opt-in description +
  a passthrough example. `docs/src/components/mcp.md`: a subsection under
  Server (Consumer) documenting `to`/`steps` on tool/resource
  declarations and the identity default.

## Risks / Trade-offs

- Schema noise: the `steps` array reuses the existing `RouteDslStep`
  schema (shared with routes/rest) — no schema duplication.
- Behavior surprise: an identity tool (no steps) still echoes arguments;
  the docs entry states this explicitly.
- No runtime risk: consumer code path unchanged.

## Phases

Single-phase change (2 tasks): DSL + lowering + tests, then schema regen +
docs. No cross-crate sequencing — `camel-lint` only receives the regen'd
schema copy.
