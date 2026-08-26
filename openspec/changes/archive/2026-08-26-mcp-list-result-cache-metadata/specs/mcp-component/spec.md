## ADDED Requirements

### Requirement: List-result cache metadata

The MCP server SHALL include SEP-2549 cache metadata — `ttlMs` and
`cacheScope` — in every `tools/list` and `resources/list` result, because
protocol revision `2026-07-28` marks both fields required and strict clients
reject result payloads that omit them. Until a caching design exists, list
results SHALL be non-cacheable: `ttlMs: 0` and `cacheScope: "private"`
(the catalog is dynamic under readiness gating, and list results must not be
shared across authorization contexts).

#### Scenario: tools/list carries cache metadata

- **GIVEN** a started MCP server with at least one ready tool route
- **WHEN** a host issues `tools/list` over Streamable-HTTP
- **THEN** the JSON-RPC result object contains `"ttlMs": 0` and
  `"cacheScope": "private"` alongside the `tools` array

#### Scenario: resources/list carries cache metadata

- **GIVEN** a started MCP server with at least one ready resource route
- **WHEN** a host issues `resources/list` over Streamable-HTTP
- **THEN** the JSON-RPC result object contains `"ttlMs": 0` and
  `"cacheScope": "private"` alongside the `resources` array

#### Scenario: empty list still carries cache metadata

- **GIVEN** a started MCP server with no ready tool routes
- **WHEN** a host issues `tools/list`
- **THEN** the result object still contains `"ttlMs": 0` and
  `"cacheScope": "private"` (field presence does not depend on catalog
  cardinality)

## MODIFIED Requirements

### Requirement: Baseline protocol version `2026-07-28`

The MCP component SHALL baseline exclusively on protocol revision
`2026-07-28`. The server SHALL advertise `2026-07-28` as its only supported
protocol version and SHALL NOT implement any pre-`2026-07-28` compatibility or
fallback path. The client SHALL negotiate using the discover lifecycle with
`2026-07-28` as its only preferred version.

#### Scenario: server advertises only 2026-07-28 via discover

- **GIVEN** a started MCP server
- **WHEN** a host issues `server/discover`
- **THEN** the response lists exactly `["2026-07-28"]` as supported protocol
  versions and includes the server identity and capabilities

#### Scenario: pre-2026-07-28 request is rejected

- **GIVEN** a started MCP server
- **WHEN** a client sends a request whose `_meta` protocol version is
  `2025-11-25` (or any version other than `2026-07-28`)
- **THEN** the server responds with JSON-RPC error `-32022`
  (`UnsupportedProtocolVersionError`) whose `data.supported` lists
  `["2026-07-28"]`
- **AND** the server emits a `warn!` naming the peer and the rejected version

#### Scenario: legacy initialize attempt does not open a session

- **GIVEN** a started MCP server
- **WHEN** a legacy client attempts the `initialize` handshake and then sends a
  follow-up request carrying a pre-`2026-07-28` `_meta` version
- **THEN** no `Mcp-Session-Id` is issued and the follow-up request is rejected
  with `-32022`

#### Scenario: legacy initialize is rejected fail-closed

- **GIVEN** a started MCP server
- **WHEN** a client sends `initialize` offering protocol version
  `2025-11-25` (or any version other than `2026-07-28`)
- **THEN** the server responds with JSON-RPC error `-32022` whose
  `data.supported` lists `["2026-07-28"]`
- **AND** no success response carrying a server-default protocol version is
  returned (no fallback path)
