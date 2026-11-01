# validator-component Specification

## Purpose

Define the normative contract for the `validator:` component's schema validation producers: the accepted schema types and URI options, route-build failure behavior, and per-exchange validation semantics for the protobuf schema type.
## Requirements
### Requirement: Protobuf schema type contract

The `validator:` component SHALL accept `schemaType=protobuf` (alias `type=protobuf`; both keys SHALL agree and unknown values SHALL be rejected) over a `.proto` path or a precompiled descriptor set (`.binpb`, `.pb`, `.desc`, `.protoset`). The `.proto` extension SHALL infer the protobuf schema type. The `message` query parameter (a fully-qualified message name) SHALL be REQUIRED; a missing or empty `message` SHALL fail endpoint creation with a config error naming `message`. The descriptor SHALL be compiled once at endpoint creation through the hermetic proto compiler (ADR-0084) with empty include paths and the existing schema-path traversal rejection. A missing proto file or a compile error SHALL fail route creation.

#### Scenario: protobuf URI parses with required message

- **GIVEN** `validator:schemas/person.proto?schemaType=protobuf&message=test.v1.Person` (the `type=protobuf` alias behaves identically)
- **WHEN** the endpoint is created
- **THEN** the descriptor compiles once and validation runs against `test.v1.Person`

#### Scenario: missing message fails at route build

- **GIVEN** a protobuf URI without `message` or with an empty value
- **WHEN** the endpoint is created
- **THEN** creation fails with a config error naming `message`

#### Scenario: compile error fails at route build

- **GIVEN** a proto file that fails to compile or a missing file
- **WHEN** the endpoint is created
- **THEN** creation fails with an error naming the schema path

### Requirement: Protobuf validation semantics

The protobuf validator SHALL accept `Body::Json` as the primary form plus `Body::Text` and `Body::Bytes` parsed as UTF-8 JSON. It SHALL reject unknown fields, wrong scalar, enum or oneof shapes, malformed JSON, and non-UTF-8 bytes with a validation error (`CamelError::ValidationError`), mapped by the shared transport mapping to HTTP 400. It SHALL reject unsupported body variants with an error naming the accepted variants. It SHALL NOT encode or decode protobuf and SHALL NOT mutate the body, which SHALL pass through byte-identical with no JSON normalization. JSON `null` and missing proto3 fields SHALL follow proto3 default semantics. The `maxPayloadBytes` option SHALL keep the unchanged payload-too-large path, and `headerName`, `failOnNullBody`, and `failOnNullHeader` SHALL apply unchanged.

#### Scenario: valid JSON body passes with the body unchanged

- **GIVEN** a valid person JSON body
- **WHEN** it is validated
- **THEN** the exchange passes and the body is byte-identical afterward

#### Scenario: unknown field rejected

- **GIVEN** a JSON body with a field absent from the descriptor
- **WHEN** it is validated
- **THEN** validation fails with a validation error (`CamelError::ValidationError`) that the shared transport mapping turns into HTTP 400

#### Scenario: wrong scalar type rejected

- **GIVEN** a JSON body with a wrong scalar type for a declared field
- **WHEN** it is validated
- **THEN** validation fails

#### Scenario: malformed JSON rejected

- **GIVEN** a `Body::Text` body that is not valid JSON
- **WHEN** it is validated
- **THEN** validation fails

#### Scenario: non-UTF-8 bytes rejected

- **GIVEN** a `Body::Bytes` body that is not valid UTF-8
- **WHEN** it is validated
- **THEN** validation fails

#### Scenario: null and missing fields follow proto3 defaults

- **GIVEN** an empty JSON object or a body with `null` fields
- **WHEN** it is validated
- **THEN** validation passes

#### Scenario: payload bound applies

- **GIVEN** `maxPayloadBytes` configured and an oversized body
- **WHEN** it is validated
- **THEN** the exchange is rejected with a payload-too-large error

#### Scenario: header validation path applies

- **GIVEN** `headerName` configured and a valid JSON header value present
- **WHEN** the exchange is validated
- **THEN** the header value is validated instead of the body

### Requirement: Protobuf descriptor hot reload

A validator endpoint with a protobuf schema SHALL register a reload target in
the `TemplateReloadRegistry` for its route when a route id is available (the
`validator-bridge` fallback id registers nothing), holding the compiled
descriptor behind an atomic swap cell and exposing the committed schema file
plus its resolved local imports as watch sources. On reload, the endpoint
SHALL recompile the `.proto` in-process within a fixed 5000 ms deadline
matching camel-template's `reload_timeout_ms` — with the synchronous
compilation isolated from the async runtime — and swap the descriptor
together with the recomputed source list atomically. A failed or timed-out
recompile SHALL keep the previously committed descriptor and leave the
endpoint serving, and no commit SHALL happen after the deadline passes.

#### Scenario: descriptor recompiles on reload

- **GIVEN** a running validator endpoint with a compiled protobuf descriptor
- **WHEN** the `.proto` source changes and the route's reload runs
- **THEN** the descriptor is recompiled and swapped, and the next validation
  uses the new schema

#### Scenario: import-only edit recompiles the descriptor

- **GIVEN** a running validator endpoint whose schema imports another local
  `.proto` file
- **WHEN** only the imported file changes and the route's reload runs
- **THEN** the recompiled descriptor reflects the import change after the
  swap

#### Scenario: reload that adds a local import reports the new source

- **GIVEN** a running validator endpoint with a committed source list
- **WHEN** a reload commits a schema that imports a new local `.proto` file
- **THEN** the new file appears in the target's watch sources afterwards

#### Scenario: failed recompile keeps prior descriptor

- **GIVEN** a running validator endpoint with a compiled protobuf descriptor
- **WHEN** the `.proto` source becomes invalid and the route's reload runs
- **THEN** the reload fails, the prior descriptor keeps validating, and the
  endpoint stays in service

#### Scenario: stalled build times out without commit

- **GIVEN** a validator reload whose compilation does not finish within the
  configured deadline
- **WHEN** the deadline passes
- **THEN** the reload is abandoned with an error and no descriptor swap
  happens afterwards

