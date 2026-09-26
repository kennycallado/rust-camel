# mcp-component Delta — mcpsteps

## MODIFIED Requirements

### Requirement: MCP DSL block and lowering

The system SHALL provide a declarative `mcp:` DSL block that declares a named
server (bind, TLS, `security_policy`, `max_tools`, `max_resources`) and its
tools and resources, each tool carrying an input JSON Schema. The block SHALL
lower each tool to an `mcp:<server>/tool/<name>` consumer route and each
resource to an `mcp:<server>/resource/<name>` consumer route, injecting the
schema as a validation step — structurally analogous to the `rest:`→`http:`
lowering. The listener bind, TLS, and `security_policy` are properties of the
named server, not per-tool URI options. A tool whose `input_schema` is not a
JSON object SHALL be rejected at lowering with an error naming the tool and
the actual JSON kind of the offending value — the same predicate the
consumer applies to the decoded `schema` endpoint parameter, applied at
parse time instead of consumer start (bd rc-ap58). Load behavior is
unchanged: a non-object `input_schema` deserializes into the AST verbatim;
the rejection is lowering-owned.

A tool or resource declaration MAY additionally carry an opt-in pipeline:
`to` (a single endpoint URI shorthand) or `steps` (the standard
`RouteDslStep` list the route and rest surfaces use). Declaring BOTH on one
declaration SHALL be rejected at lowering with an error naming the
declaration. With neither, the lowered route keeps the identity pipeline
(no steps) — v1 behavior unchanged. The shorthand and the step list lower
through the same construction the rest surface uses; no step translation,
wrapping, or reordering is applied (bd rc-23y2).

#### Scenario: DSL block lowers to consumer routes

- **GIVEN** a `mcp:` block declaring server `crm` with tool `lookup`
- **WHEN** the DSL is parsed and lowered
- **THEN** an `mcp:crm/tool/lookup` consumer route exists carrying the declared
  input schema

#### Scenario: schema lives in operator config not on the wire

- **GIVEN** a lowered `mcp:crm/tool/lookup` route
- **WHEN** an Exchange is processed
- **THEN** the input schema is sourced from the DSL config, not from any
  Exchange header or body

#### Scenario: Non-object tool input schema is rejected at lowering

- **GIVEN** a `mcp:` block whose tool `lookup` declares a non-object
  `input_schema` (a JSON string, number, array, boolean, or null)
- **WHEN** the DSL is parsed and lowered
- **THEN** lowering fails with an error naming the tool `lookup` and the
  JSON kind of the offending value, and no consumer route is produced for
  the block

#### Scenario: tool with steps lowers to a processing pipeline

- **GIVEN** a `mcp:` block whose tool `lookup` declares a `steps` list with
  one `to: log:audit` step
- **WHEN** the DSL is parsed and lowered
- **THEN** the `mcp:crm/tool/lookup` consumer route carries exactly that
  step list, and a `tools/call` executes the steps instead of returning
  the call's own arguments

#### Scenario: resource with to shorthand lowers to a processing pipeline

- **GIVEN** a `mcp:` block whose resource `customers` declares
  `to: direct:mirror`
- **WHEN** the DSL is parsed and lowered
- **THEN** the `mcp:crm/resource/customers` consumer route carries exactly
  one `To` step targeting `direct:mirror`

#### Scenario: declaration without steps or to keeps the identity pipeline

- **GIVEN** a `mcp:` block whose tool `lookup` and resource `customers`
  declare neither `steps` nor `to`
- **WHEN** the DSL is parsed and lowered
- **THEN** both lowered consumer routes carry no steps, byte-identical to
  the v1 catalog-only lowering

#### Scenario: steps and to declared together are rejected at lowering

- **GIVEN** a `mcp:` block whose tool `lookup` (or resource `customers`)
  declares both `to` and `steps`
- **WHEN** the DSL is parsed and lowered
- **THEN** lowering fails with an error naming the tool or resource, and
  no consumer route is produced for the declaration
