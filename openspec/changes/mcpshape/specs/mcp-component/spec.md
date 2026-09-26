# mcp-component delta — mcpshape

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
