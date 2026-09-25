# route-lint delta — mcpblank

## ADDED Requirements

### Requirement: R-SCHEMA rejects blank MCP server bind/name values with runtime parity

The route schema SHALL reject blank (empty or whitespace-only) values for
`mcp[].server.bind`, `mcp[].server.name`, `mcp[].tools[].name`, and
`mcp[].resources[].name` — mirroring the runtime, which rejects a blank `bind`
at consumer-config parse (`mcp.declared.bind` must not be empty) and a blank
`name` at DSL lowering (`validate_mcp_name`). Each blank value emits exactly one
R-SCHEMA Error anchored on the offending value with the pattern-violation
message. Values that are non-blank after trimming SHALL NOT be rejected by this
requirement (deeper validation — `SocketAddr` literal for `bind`,
`[A-Za-z0-9._-]+` charset for names — stays runtime-owned and unchanged).

#### Scenario: Empty server bind is rejected and anchored on the value

- **Given** an `mcp:` block whose `server.bind` is the empty string `""` and
  `server.name` is a valid name
- **When** the document is linted
- **Then** exactly one R-SCHEMA Error is emitted, anchored on the blank `bind`
  value with a pattern-violation message, and no Error is anchored on `name`

#### Scenario: Whitespace-only server name is rejected

- **Given** an `mcp:` block whose `server.name` is whitespace-only (e.g. `"   "`)
  and `server.bind` is a valid address
- **When** the document is linted
- **Then** exactly one R-SCHEMA Error is emitted, anchored on the blank `name`
  value with a pattern-violation message, and no Error is anchored on `bind`

#### Scenario: Blank tool or resource name is rejected

- **Given** an `mcp:` block whose `server` declaration is valid but one
  `tools[].name` or `resources[].name` is empty or whitespace-only
- **When** the document is linted
- **Then** an R-SCHEMA Error is emitted, anchored on that blank `name` value
  with a pattern-violation message

#### Scenario: Non-blank values load and lower unchanged

- **Given** an `mcp:` block with non-blank `server.name`, `server.bind`, tool
  and resource names (including values the runtime itself rejects later, such
  as a non-literal `bind` host or a charset-violating name)
- **When** the document is loaded and linted
- **Then** the schema emits no pattern-violation Error for these fields, and
  the runtime rejection behavior (consumer-config parse, lowering charset
  check) is byte-identical to before this requirement
