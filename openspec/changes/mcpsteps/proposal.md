# Proposal: mcpsteps

## Why

`RouteDslMcpTool`/`RouteDslMcpResource` (`crates/camel-dsl/src/mcp.rs`) carry
no `steps`/`to`, so `lower_all_mcp_to_routes` emits step-less consumer routes
and a `tools/call` returns its own arguments (the identity pipeline). v1
documented the block as a catalog declaration only; behavior had to live in
separate explicit routes consuming `from mcp:<server>/...`. Operators get two
artifacts for one behavior (bd rc-23y2, discovered in the add-mcp-component
Phase-3 inter-phase review).

## What Changes

- Add OPT-IN `to: Option<String>` and `steps: Vec<RouteDslStep>` to
  `RouteDslMcpTool` and `RouteDslMcpResource` (`#[serde(default)]`,
  `deny_unknown_fields` preserved).
- Lowering reuses the REST machinery shape (`rest.rs` `lower_operation`):
  `to` shorthand lowers to one `RouteDslStep::To(ToStep { to, .. })`;
  explicit `steps` clone verbatim onto the consumer route. Declaring BOTH
  fails at lowering with a named error mirroring rest's wording. No fork of
  the step machinery — the same `RouteDslStep` AST, the same shorthand.
- Without `to`/`steps`, lowering is byte-identical to v1 (identity pipeline,
  empty steps) — every existing camel-dsl test stays green (their struct
  literals gain only the two no-op fields).
- `route-schema.json` regenerates with the two optional fields (schemars
  derive; both copies: `schemas/dsl/` + `crates/camel-lint/schema/`).
- Docs: the v1 catalog-only note in `crates/camel-dsl/README.md` is updated
  and `docs/src/components/mcp.md` gains the passthrough entry.
- Consumer side (`camel-component-mcp`) is UNTOUCHED: the pipeline is
  standard route machinery; the component reads routes as today.

Affected crates: `camel-dsl` (code + tests), `camel-lint` (schema copy
regen only). bd: rc-23y2.

## Acceptance criteria

- A tool/resource declaration with `steps` (or `to`) lowers to a consumer
  route carrying that pipeline; `tools/call` executes the steps instead of
  returning its own arguments.
- A declaration without `to`/`steps` lowers byte-identically to v1
  (identity pipeline).
- `to` AND `steps` together on one declaration fails at lowering with an
  error naming the tool/resource; invalid step shapes fail at document
  load via the shared `RouteDslStep` serde (span-carrying parse errors).
- `cargo test -p camel-dsl --lib` green at the 842+ baseline (new tests
  added on top); `cargo xtask schema --check` green after regen.

## Risk budget

Low: additive optional fields with a fail-closed XOR guard; no consumer or
runtime change; no new validation mechanism (reuses `RouteDslStep` serde and
rest's shorthand lowering). Out of bounds: touching `camel-component-mcp`,
auto-wrapping steps with data-format processors, per-step DSL extensions
beyond what `RouteDslStep` already offers.
