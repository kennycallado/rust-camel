## ADDED Requirements

### Requirement: dsl_rest fuzz harness

The `dsl_rest` fuzz target SHALL feed arbitrary bytes as UTF-8 text to
both route-document front-ends —
`camel_dsl::json::parse_json_with_threshold_and_security` and
`camel_dsl::yaml::parse_yaml_with_threshold_and_security`, each with
`camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD` and
`SecurityCompileContext::default()` — and SHALL hold the invariant that
parsing (including `rest:` block lowering, cross-block duplicate and
ambiguity validation, and path-template parsing) never panics. Invalid
UTF-8 input SHALL be skipped before either parse call. Results SHALL be
discarded.

#### Scenario: valid rest document lowers without panic

- **GIVEN** a seed document containing one `rest:` block with listener,
  path, and operation entries
- **WHEN** the harness parses it through both front-ends
- **THEN** each parse call returns either `Ok` or `Err` and the fuzz
  target reports no crash

#### Scenario: malformed or hostile rest content is rejected, not a panic

- **GIVEN** truncated JSON, ambiguous path templates
  (`/users/{id}` vs `/users/{name}` on one listener), duplicate
  `(host, port, verb, path)` tuples, or invalid media declarations
- **WHEN** the harness parses the bytes
- **THEN** the front-ends return `Err` (or `Ok` with rejection recorded)
  and never panic

#### Scenario: invalid UTF-8 input is skipped

- **GIVEN** byte sequences that are not valid UTF-8
- **WHEN** the harness runs
- **THEN** the harness returns early without calling either parser and
  reports no crash

### Requirement: dsl_mcp fuzz harness

The `dsl_mcp` fuzz target SHALL feed arbitrary bytes as UTF-8 text to
both route-document front-ends (same calls and arguments as
`dsl_rest`), covering `mcp:` block lowering — name validation, tool
input-schema validation, resource-URI and schema percent-encoding into
consumer route URIs, and listener parameter lowering — and SHALL hold
the invariant that parsing never panics. Invalid UTF-8 input SHALL be
skipped; results SHALL be discarded.

#### Scenario: valid mcp document lowers without panic

- **GIVEN** a seed document with an `mcp:` block declaring a server, a
  tool with `input_schema`, and a resource with a URI
- **WHEN** the harness parses it through both front-ends
- **THEN** neither parse call panics and the fuzz target reports no
  crash

#### Scenario: hostile mcp content is rejected, not a panic

- **GIVEN** tool or server names failing the name pattern, invalid
  `bind` literals, invalid tool `input_schema` values, or blank TLS
  cert/key paths
- **WHEN** the harness parses the bytes
- **THEN** the front-ends return `Err` and never panic

#### Scenario: invalid UTF-8 input is skipped

- **GIVEN** byte sequences that are not valid UTF-8
- **WHEN** the harness runs
- **THEN** the harness returns early without calling either parser

### Requirement: dsl_openapi fuzz harness

The `dsl_openapi` fuzz target SHALL feed arbitrary bytes as UTF-8 text
into the camel-cli `run_generate` validated-generation stage: extract
the `rest` AST without lowering (YAML via
`camel_dsl::yaml::extract_rest_blocks`, JSON via
`serde_json::from_str::<RouteDslRoutes>` then `.rest`), validate the
blocks with `lower_all_rest_to_routes` and
`check_duplicate_route_ids`, and for blocks that validate SHALL call
`camel_dsl::openapi::generate_openapi` with fixed title and version
strings, discarding the result. The invariant SHALL be that neither
extraction, nor validation, nor document generation panics. Invalid
UTF-8 input SHALL be skipped.

#### Scenario: valid rest document generates an OpenAPI document

- **GIVEN** a seed document whose `rest:` blocks pass lowering
  validation, including an operation with an explicit `operationId`,
  path parameters, and an operation with no response schemas (the
  weak-stub warning path)
- **WHEN** the harness extracts, validates, and generates
- **THEN** `generate_openapi` returns a result that is discarded and no
  panic occurs; the seed-contract test for this seed asserts the
  weak-stub warning is present in `warnings`

#### Scenario: duplicate path-verb across different listeners takes the warning path

- **GIVEN** a seed document where two operations on DIFFERENT listeners
  claim the same `(path, verb)` with distinct `operationId`s (passes
  per-listener §6.3 validation)
- **WHEN** the harness validates and generates
- **THEN** the duplicate-operation warning is recorded internally, no
  panic occurs, and the seed-contract test for this seed asserts the
  warning is present

#### Scenario: invalid UTF-8 input is skipped

- **GIVEN** byte sequences that are not valid UTF-8
- **WHEN** the harness runs
- **THEN** the harness returns early without calling the extractor

### Requirement: Non-DSL-channel seed corpora

Each of `dsl_rest`, `dsl_mcp`, and `dsl_openapi` SHALL have a committed,
minimized seed corpus under `fuzz/seeds/<target>/` containing at least
one document whose channel blocks parse `Ok` through both front-ends
plus adversarial shapes exercising the channel's rejection and warning
paths. Seed sets SHALL be pinned exactly (file names, no extras) and
exercised through their harness by committed tests; every valid seed
SHALL parse `Ok` through both front-ends and every malformed seed SHALL
be rejected by at least one documented path without panicking.

#### Scenario: seed directory shape is pinned

- **GIVEN** the committed `fuzz/seeds/dsl_rest/` directory
- **WHEN** the seed-contract test enumerates it
- **THEN** the file set matches the pinned name list exactly

#### Scenario: all seeds pass through the harness without panic

- **GIVEN** any committed seed of any of the three targets
- **WHEN** its bytes are fed to that target's harness
- **THEN** the harness returns without panicking

#### Scenario: valid seeds parse through both front-ends

- **GIVEN** a `valid_*` seed of any of the three targets
- **WHEN** parsed through the JSON and YAML front-ends
- **THEN** both parses return `Ok`
