# route-lint delta — mcpchars

## MODIFIED Requirements

### Requirement: R-SCHEMA rejects blank MCP server bind/name values with runtime parity

The route schema SHALL reject blank (empty or whitespace-only) values for
`mcp[].server.bind`, `mcp[].server.name`, `mcp[].tools[].name`, and
`mcp[].resources[].name` — mirroring the runtime, which rejects a blank `bind`
at consumer-config parse (`mcp.declared.bind` must not be empty) and a blank
`name` at DSL lowering (`validate_mcp_name`). Each blank value emits exactly
one R-SCHEMA Error anchored on the offending value with the pattern-violation
message. This requirement covers the blank class only; non-blank shape
violations moved from runtime-only enforcement to load/schema enforcement by
the sibling requirements "R-SCHEMA rejects MCP names outside the runtime
charset with parity" and "R-SCHEMA rejects malformed MCP bind values with
runtime parity" (bd rc-vh9dt / rc-38iiz).

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
  and resource names
- **When** the document is loaded and linted
- **Then** no blank-class Error is emitted for these fields. Names keep their
  verbatim load and their lowering-owned charset rejection (byte-identical
  error). A `bind` outside the IP-literal grammar is rejected at load and by
  the schema per the sibling requirement "R-SCHEMA rejects malformed MCP bind
  values with runtime parity"; a name outside the charset is flagged by the
  schema per the sibling requirement "R-SCHEMA rejects MCP names outside the
  runtime charset with parity".

## ADDED Requirements

### Requirement: R-SCHEMA rejects MCP names outside the runtime charset with parity

The route schema SHALL reject values for `mcp[].server.name`,
`mcp[].tools[].name`, and `mcp[].resources[].name` outside the charset the
runtime enforces — `[A-Za-z0-9._-]+`, anchored, mirroring `validate_mcp_name`
(bd rc-ap58). Each violation emits exactly one R-SCHEMA Error anchored on the
offending value with the pattern-violation message. Load behavior for names
is unchanged: the charset check stays at lowering (its error names the
offending key and the `?`/`/` URI hazards).

#### Scenario: Charset-violating server name is rejected and anchored

- **Given** an `mcp:` block whose `server.name` contains a character outside
  `[A-Za-z0-9._-]` (e.g. `café` or `a/b`) and whose `server.bind` is a valid
  IP-literal
- **When** the document is linted
- **Then** exactly one R-SCHEMA Error is emitted, anchored on the offending
  `name` value with a pattern-violation message, and no Error is anchored on
  `bind`

#### Scenario: Charset-violating tool or resource name is rejected

- **Given** an `mcp:` block whose `server` declaration is valid but one
  `tools[].name` or `resources[].name` contains a character outside
  `[A-Za-z0-9._-]`
- **When** the document is linted
- **Then** an R-SCHEMA Error is emitted, anchored on that `name` value with a
  pattern-violation message

#### Scenario: Charset-valid names with separators stay clean

- **Given** an `mcp:` block whose server, tool, and resource names use the
  separator characters the charset allows (e.g. `crm-api_v2.prod`)
- **When** the document is linted
- **Then** no R-SCHEMA diagnostic is emitted for any name value

#### Scenario: Charset class stays lowering-owned at load

- **Given** an `mcp:` block whose `server.name` violates the charset (e.g.
  `café`)
- **When** the document is parsed into the DSL AST
- **Then** parsing succeeds with the name verbatim, and lowering
  (`lower_all_mcp_to_routes`) rejects it with the pre-existing
  `validate_mcp_name` error naming the offending key

#### Scenario: Name-charset corpus negative fixture is baselined as failing

- **Given** the corpus fixture
  `crates/camel-cli/tests/fixtures/lint-corpus/mcp-name-charset.yaml` carrying
  charset-violating server and tool names
- **When** the `corpus_zero_false_positives` gate runs
- **Then** the fixture emits `("R-SCHEMA", "error")` exactly as recorded in
  the baseline with a justification, and the gate passes

### Requirement: R-SCHEMA rejects malformed MCP bind values with runtime parity

The route schema SHALL reject `mcp[].server.bind` values outside the
IP-literal `SocketAddr` grammar the consumer enforces at start
(`camel-component-mcp` config/registry: `bind '<value>' is not an IP:port
literal (hostnames are not allowed)`). The schema pattern is the anchored
shape `^((\d{1,3}\.){3}\d{1,3}|\[[0-9A-Fa-f:.]+\]):\d{1,5}$` — dotted-quad
IPv4 or bracketed IPv6, colon, 1–5 digit port. Load mirrors the runtime
exactly: `deserialize_mcp_bind` runs the runtime's own predicate
(`std::net::SocketAddr` parse of the verbatim value) and rejects with the
runtime's message text. Every bind the runtime accepts matches the pattern
(no false positives); values inside the shape but numerically invalid
(e.g. `999.1.1.1:80`) pass lint and die at load.

#### Scenario: Hostname bind is rejected and anchored

- **Given** an `mcp:` block whose `server.bind` is a hostname form such as
  `localhost:9100` and whose `server.name` is valid
- **When** the document is linted
- **Then** exactly one R-SCHEMA Error is emitted, anchored on the offending
  `bind` value with a pattern-violation message, and no Error is anchored on
  `name`

#### Scenario: Port-less bind is rejected and anchored

- **Given** an `mcp:` block whose `server.bind` lacks the port (e.g.
  `127.0.0.1`)
- **When** the document is linted
- **Then** an R-SCHEMA Error is emitted, anchored on the `bind` value with a
  pattern-violation message

#### Scenario: Valid IPv4 and IPv6 literals stay clean

- **Given** an `mcp:` block whose `server.bind` is `127.0.0.1:9100` or a
  bracketed IPv6 literal such as `[::1]:9100`
- **When** the document is linted
- **Then** no R-SCHEMA diagnostic is emitted for the `bind` value

#### Scenario: Malformed bind is rejected at load with the runtime message

- **Given** an `mcp:` document whose `server.bind` is `localhost:9100` or a
  padded value such as `" 127.0.0.1:9100 "`
- **When** the document is parsed into the DSL AST
- **Then** parsing fails with an error containing
  `not an IP:port literal` — the same verdict the consumer start gives
  today, moved to load

#### Scenario: Bind-grammar corpus negative fixture is baselined as failing

- **Given** the corpus fixture
  `crates/camel-cli/tests/fixtures/lint-corpus/mcp-bind-grammar.yaml` carrying
  a hostname `server.bind`
- **When** the `corpus_zero_false_positives` gate runs
- **Then** the fixture emits `("R-SCHEMA", "error")` exactly as recorded in
  the baseline with a justification, and the gate passes
