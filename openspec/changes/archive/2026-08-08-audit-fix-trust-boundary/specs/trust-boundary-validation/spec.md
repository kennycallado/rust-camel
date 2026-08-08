# Spec: trust-boundary-validation

## ADDED Requirements

### Requirement: SurrealDB query op must gate untrusted query text behind explicit opt-in

The `query` operation MUST NOT resolve query text from the `CamelSurrealDbQuery`
header or exchange body unless the operator has explicitly set
`allow_dynamic_query=true` in the endpoint configuration. When
`allow_dynamic_query=false` (the default) and no config query is set, the producer
MUST return a runtime `MissingParam` error at invocation time.

#### Scenario: SurrealDB query rejects header query text by default

- **Given** a SurrealDB endpoint configured with `operation=query` and no
  `allow_dynamic_query` parameter (defaults to false)
- **When** an exchange arrives with the `CamelSurrealDbQuery` header set to
  arbitrary SurrealQL text
- **Then** the producer uses the config query (not the header)
- **And** if no config query is set, the producer returns a MissingParam error

#### Scenario: SurrealDB query accepts header query text with explicit opt-in

- **Given** a SurrealDB endpoint configured with `operation=query` and
  `allow_dynamic_query=true`
- **When** an exchange arrives with the `CamelSurrealDbQuery` header set to
  SurrealQL text
- **Then** the producer uses the header query text

#### Scenario: SurrealDB query rejects body query text by default

- **Given** a SurrealDB endpoint configured with `operation=query` and no
  `allow_dynamic_query` parameter (defaults to false)
- **When** an exchange arrives with a `Body::Text` containing SurrealQL text
- **Then** the producer uses the config query (not the body)
- **And** if no config query is set, the producer returns a MissingParam error

#### Scenario: SurrealDB query accepts body query text with explicit opt-in (Body::Text)

- **Given** a SurrealDB endpoint configured with `operation=query` and
  `allow_dynamic_query=true`
- **When** an exchange arrives with a `Body::Text(String)` body containing
  SurrealQL text
- **Then** the producer uses the body query text

#### Scenario: SurrealDB query accepts body query text with explicit opt-in (Body::Json String)

- **Given** a SurrealDB endpoint configured with `operation=query` and
  `allow_dynamic_query=true`
- **When** an exchange arrives with a `Body::Json(Value::String(s))` body
  containing SurrealQL text
- **Then** the producer uses the body query text

#### Scenario: SurrealDB query with no source and gate off returns runtime error

- **Given** a SurrealDB endpoint configured with `operation=query`,
  `allow_dynamic_query=false`, and no config query
- **When** an exchange arrives with no query header and no text body
- **Then** the producer returns a MissingParam error at runtime

#### Scenario: SurrealDB allow_dynamic_query defaults to false

- **Given** a SurrealDB endpoint configured from URI without
  `allow_dynamic_query` parameter
- **When** the config is parsed
- **Then** `allow_dynamic_query` is `false`

#### Scenario: SurrealDB allow_dynamic_query rejects invalid truthy values

- **Given** a SurrealDB endpoint configured from URI with
  `allow_dynamic_query=maybe`
- **When** the config is parsed
- **Then** the parser returns an InvalidUri error

### Requirement: OpenSearch doc_id from exchange header must be validated

The `CamelOpenSearch.Id` header is untrusted exchange data (ADR-0032). Before
passing it to any `_doc/{id}` request builder, the producer MUST validate it
against path-injection characters. Validation MUST apply to all five operations
that read the header: INDEX (explicit-ID path), GET, DELETE, UPDATE, and EXISTS.

#### Scenario: OpenSearch rejects doc_id with path separator on all operations

- **Given** an OpenSearch producer configured for each of INDEX, GET, DELETE,
  UPDATE, and EXISTS
- **When** the `CamelOpenSearch.Id` header value contains a forward slash (`/`)
- **Then** the producer returns a Permanent error before sending the request

#### Scenario: OpenSearch rejects doc_id with query separator

- **Given** an OpenSearch producer configured for any operation that reads
  `CamelOpenSearch.Id`
- **When** the header value contains a question mark (`?`)
- **Then** the producer returns a Permanent error before sending the request

#### Scenario: OpenSearch rejects doc_id with fragment separator

- **Given** an OpenSearch producer configured for any operation that reads
  `CamelOpenSearch.Id`
- **When** the header value contains a hash (`#`)
- **Then** the producer returns a Permanent error before sending the request

#### Scenario: OpenSearch rejects doc_id with percent-encoding

- **Given** an OpenSearch producer configured for any operation that reads
  `CamelOpenSearch.Id`
- **When** the header value contains a percent sign (`%`)
- **Then** the producer returns a Permanent error before sending the request

#### Scenario: OpenSearch rejects doc_id with null bytes

- **Given** an OpenSearch producer configured for any operation that reads
  `CamelOpenSearch.Id`
- **When** the header value contains a null byte (`\0`)
- **Then** the producer returns a Permanent error before sending the request

#### Scenario: OpenSearch rejects doc_id with backslash

- **Given** an OpenSearch producer configured for any operation that reads
  `CamelOpenSearch.Id`
- **When** the header value contains a backslash (`\`)
- **Then** the producer returns a Permanent error before sending the request

#### Scenario: OpenSearch rejects doc_id with exact dot segment

- **Given** an OpenSearch producer configured for any operation that reads
  `CamelOpenSearch.Id`
- **When** the header value is exactly `.` or `..`
- **Then** the producer returns a Permanent error before sending the request

#### Scenario: OpenSearch rejects doc_id with control characters

- **Given** an OpenSearch producer configured for any operation that reads
  `CamelOpenSearch.Id`
- **When** the header value contains a control character (U+0000–U+001F or U+007F)
- **Then** the producer returns a Permanent error before sending the request

#### Scenario: OpenSearch rejects empty doc_id

- **Given** an OpenSearch producer configured for any operation that reads
  `CamelOpenSearch.Id`
- **When** the header value is an empty string
- **Then** the producer returns a Permanent error before sending the request

#### Scenario: OpenSearch rejects oversized doc_id

- **Given** an OpenSearch producer configured for any operation that reads
  `CamelOpenSearch.Id`
- **When** the header value exceeds 512 bytes in length
- **Then** the producer returns a Permanent error before sending the request

#### Scenario: OpenSearch accepts valid doc_id

- **Given** an OpenSearch producer configured for any operation that reads
  `CamelOpenSearch.Id`
- **When** the header value contains only alphanumeric characters, hyphens,
  underscores, dots (not exact `.` or `..`), and colons
- **Then** the producer passes the doc_id to the request builder without error

### Requirement: SQL query op already gates untrusted query text

The SQL component MUST gate query text from the `CamelSql.Query` header and
exchange body behind the `allow_dynamic_query=true` opt-in (URI param name:
`allowDynamicQuery`). This requirement is already satisfied by existing code;
this change adds verification tests.

#### Scenario: SQL allow_dynamic_query defaults to false

- **Given** a SQL endpoint configured from URI without `allowDynamicQuery`
- **When** the config is parsed
- **Then** `allow_dynamic_query` is `false`

#### Scenario: SQL rejects header query text by default

- **Given** a SQL endpoint configured with `allowDynamicQuery=false`
- **When** an exchange arrives with `CamelSql.Query` header set to SQL text
- **Then** the producer uses the config query (not the header)
