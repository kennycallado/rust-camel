# route-lint Delta — env-int-placeholder-parity (rev 2: wave-E canon)

## ADDED Requirements

### Requirement: R-SCHEMA validates an interpolated copy with tree-walk typing

R-SCHEMA MUST interpolate `${env:NAME:-default}` placeholders (default-only lookup,
MUST NOT read the process environment) into a validation copy before schema
validation, and MUST replicate the tree-walk typing semantics of the boot path: a
value leaf whose authored scalar is exactly one substituted `${env:X:-d}` token is
validated as the STRING `"d"`. Consequently: string-typed positions validate cleanly
(one Info-severity diagnostic per substituted default, span-anchored on the authored
placeholder); integer- or boolean-typed positions produce a schema type Error anchored
on the placeholder value node. A no-default token (`${env:X}`) at any value position
MUST produce an Error (the boot path hard-fails on it; lint must not stay silent);
`$${env:...}` escapes are exempt. Comment tokens MUST produce no diagnostics
(comments are not part of the parsed instance). Per-token semantics apply — a
whole-document error fallback that re-literals every token is forbidden. The
interpolator is a SYNC-annotated mirror of the whole-text splice arm of
`camel-dsl::env_interpolation` (crate purity forbids the dependency).

#### Scenario: string-position default yields no error and one Info note

- **Given** a route document with a string-typed field `title: ${env:MY_TITLE:-hello}`
- **When** R-SCHEMA analyzes the document
- **Then** there are zero Error-severity R-SCHEMA diagnostics and exactly one
  Info-severity diagnostic reporting the substituted default, span-anchored on the
  placeholder value node

#### Scenario: integer-position default is flagged as a type error

- **Given** a route document with `throttle.max_requests: ${env:MY_LIMIT:-2}`
- **When** R-SCHEMA analyzes the document
- **Then** an Error-severity R-SCHEMA diagnostic anchored on the placeholder reports
  the type violation (the validated leaf is the string `"2"`, matching boot-parity
  rejection)

#### Scenario: no-default token is flagged at any value position

- **Given** a route document with `title: ${env:NO_DEFAULT_xyz}` (a string-typed
  position)
- **When** R-SCHEMA analyzes the document
- **Then** an Error-severity diagnostic flags the unresolved token (the same document
  hard-fails at boot); `$${env:X}` escaped forms produce no diagnostic

#### Scenario: mixed document keeps per-token semantics

- **Given** a route document containing both `title: ${env:WITH_DEF:-hello}` and
  `throttle.max_requests: ${env:ALSO_WITH_DEF:-2}`
- **When** R-SCHEMA analyzes the document
- **Then** the string-position field validates cleanly with exactly one Info note,
  AND the integer-position field produces a type Error — each token judged at its own
  position

#### Scenario: commented tokens produce no diagnostics

- **Given** a route document whose YAML comments contain `${env:X}` and
  `${env:Y:-d}` tokens
- **When** R-SCHEMA analyzes the document
- **Then** neither comment token produces any R-SCHEMA diagnostic

#### Scenario: engine never reads the process environment

- **Given** ambient `MY_TITLE=ambient` set in the environment and a document with
  `title: ${env:MY_TITLE:-hello}`
- **When** R-SCHEMA analyzes the document
- **Then** the validated value derives from the literal default `"hello"`, not the
  ambient value (diagnostics are a pure function of document text; enforced by the
  lookup-only mirror, no `std::env` read)

#### Scenario: LSP inherits the interpolated validation

- **Given** an LSP session opens a document with a string-position
  placeholder-with-default field
- **When** diagnostics are published
- **Then** no ERROR diagnostic is published for the placeholder field and the Info
  note appears (LSP shares the lint engine; completions/hover still see the literal
  placeholder via the raw CST entrypoint)
