## ADDED Requirements

### Requirement: OpenAPI generation interpolates env placeholders with tree-walk-first semantics

`camel openapi generate` MUST resolve `${env:NAME:-default}` placeholders in
`rest:` blocks using the same strategy as the route loader
(`load_from_file_with_env`): tree-walk-first interpolation
(`interpolate_yaml_source` — comments are never interpolated, and an
interpolated leaf keeps STRING typing per the wave-E canon) for YAML inputs,
falling back to legacy whole-text interpolation when the document does not
survive the YAML round-trip; JSON inputs use the legacy whole-text splice
(mirroring discovery's `interpolate_for_parse` non-YAML arm). The default
path MUST use the default-only lookup (never the process environment). An
unset variable without default referenced from a value position MUST fail
with an error naming the variable (wording mirrors the loader's). Route file
reads MUST honor the shared route-file size cap.

#### Scenario: string-typed field with default resolves to a concrete value

- **Given** a route file whose `rest:` block carries `host: ${env:HOST:-0.0.0.0}`
- **When** `camel openapi generate` runs on the file
- **Then** generation succeeds and the emitted `servers` entry is
  `http://0.0.0.0:<port>` — no literal `${env:...}` token appears anywhere in
  the document

#### Scenario: integer-typed field with placeholder fails with type mismatch

- **Given** a route file whose `rest:` block carries `port: ${env:PORT:-8080}`
- **When** `camel openapi generate` runs on the file
- **Then** generation fails — the substituted leaf keeps string typing, so
  `port` does not deserialize into `u16`, matching discovery/`camel run`/
  loader behavior exactly (boot parity)

#### Scenario: unset variable without default errors naming the variable

- **Given** a route file whose `rest:` block carries `host: ${env:REST_HOST}`
  with no default and no ambient value
- **When** `camel openapi generate` runs on the file
- **Then** generation fails with an error naming `REST_HOST` (mirroring the
  loader's environment-variable wording), not a serde "did not match" error

#### Scenario: free-Value position resolves through the same interpolation

- **Given** a route file whose operation `request_schema` carries
  `type: ${env:T:-string}` inside a property
- **When** `camel openapi generate` runs on the file
- **Then** generation succeeds and the emitted schema property reads
  `"type": "string"` — interpolation runs before the schema becomes a JSON
  value

#### Scenario: ambient process environment is never consulted

- **Given** a route file carrying `host: ${env:AMBIENT_HOST:-0.0.0.0}` and a
  process environment that defines `AMBIENT_HOST=evil`
- **When** `camel openapi generate` runs on the file
- **Then** the emitted server URL uses `0.0.0.0` — the default-only path
  never reads ambient values

#### Scenario: escape sequences stay literal

- **Given** a route file whose `rest:` block carries a string field set to
  `$${env:KEEP:-x}` or containing `$$`
- **When** `camel openapi generate` runs on the file
- **Then** the emitted document contains the literal text `${env:KEEP:-x}`
  (one `$` stripped) or a single `$` respectively — escapes never resolve

#### Scenario: placeholders in comments never fail resolution

- **Given** a YAML route file containing `${env:MISSING}` inside a YAML
  comment and `rest:` blocks with only literal or defaulted values
- **When** `camel openapi generate` runs on the file
- **Then** generation succeeds — the tree walk never interpolates comments

#### Scenario: JSON input follows splice parity

- **Given** the same rest-block inputs expressed as a `.json` route file
  (string-typed field with default; integer-typed field with placeholder;
  no-default token)
- **When** `camel openapi generate` runs on the file
- **Then** outcomes match the YAML arm: the string default resolves to a
  concrete value, the integer position fails deserialization, and the
  no-default token fails naming the variable

#### Scenario: oversized route file is rejected

- **Given** a route file larger than the shared route-file size cap
- **When** `camel openapi generate` runs on the file
- **Then** generation fails with the same size-cap error the route loader
  produces, instead of reading an unbounded document
