# dsl Delta — env-int-placeholder-parity (rev 2: wave-E canon)

## ADDED Requirements

### Requirement: Route file loading interpolates env placeholders with tree-walk-first semantics

`camel_dsl::load_from_file` MUST resolve `${env:NAME:-default}` placeholders using the
same strategy as discovery's YAML arm (`interpolate_for_parse`): parse-tree
interpolation first (`interpolate_env_tree` — comments are never interpolated, and an
interpolated leaf keeps STRING typing per the wave-E canon), falling back to legacy
whole-text interpolation (`interpolate_env_with`) when the document does not survive
the YAML round-trip. The default-only lookup MUST NOT read the process environment. An
injectable variant (`load_from_file_with_env`) MUST accept an explicit lookup for
callers that need one. An unset variable without default referenced from a value
position MUST fail with an error naming the variable (wording mirrors
`DiscoveryError::Env`).

#### Scenario: string-typed field with default interpolates

- **Given** a route file containing a string-typed field `title: ${env:RC_T:-hello}`
- **When** `load_from_file` parses the file
- **Then** loading succeeds and the field equals `"hello"`

#### Scenario: integer-typed field with default fails with type mismatch (boot parity)

- **Given** a route file containing `throttle.max_requests: ${env:MY_LIMIT:-2}`
- **When** `load_from_file` parses the file
- **Then** loading fails — the substituted leaf keeps string typing, so the route does
  not deserialize, matching discovery/`camel run` behavior exactly

#### Scenario: unset variable without default errors naming the variable

- **Given** a route file containing `title: ${env:UNSET_NO_DEF}` in a value position
  with no ambient value
- **When** `load_from_file` parses the file
- **Then** loading fails with an error naming `UNSET_NO_DEF` (mirroring
  `DiscoveryError::Env` wording), not a serde "did not match" error

#### Scenario: ambient environment is ignored by the default path

- **Given** a route file containing `title: ${env:AMBIENT_VAR:-hello}` and ambient
  `AMBIENT_VAR=goodbye`
- **When** `load_from_file` (no explicit lookup) parses the file
- **Then** the field resolves to `"hello"` — the default-only path never consults
  ambient values

#### Scenario: explicit lookup is the sole injection point for ambient values

- **Given** a route file containing `title: ${env:AMBIENT_VAR:-hello}`
- **When** `load_from_file_with_env` parses the file with an injected lookup returning
  `Some("goodbye")`
- **Then** the field resolves to `"goodbye"` — ambient values enter only through the
  explicit lookup (no process-env mutation in tests)

#### Scenario: commented placeholder is harmless with or without default

- **Given** a route file whose YAML comment contains `${env:RC_C}` (no default) and
  whose body carries only literal values
- **When** `load_from_file` parses the file
- **Then** loading succeeds — the tree walk never interpolates comments

#### Scenario: round-trip-fragile document fails with the parse error, not the env error

- **Given** a route file using a YAML construct the tree walk cannot round-trip
  (e.g. a tagged node)
- **When** `load_from_file` parses the file
- **Then** the failure is the document's own parse/deserialization error under either
  strategy — the interpolation-layer fallback is wave-E machinery tested at that layer;
  no placeholder-env wording appears for an otherwise-resolvable document
