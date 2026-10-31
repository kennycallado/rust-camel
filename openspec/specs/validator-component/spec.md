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
