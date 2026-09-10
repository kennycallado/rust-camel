# route-lint delta — env-int-placeholder-typing

## MODIFIED Requirements

### Requirement: R-SCHEMA validates an interpolated copy with tree-walk typing

R-SCHEMA MUST interpolate `${env:NAME:-default}` placeholders (default-only lookup,
MUST NOT read the process environment) into a validation copy before schema
validation, and MUST replicate the typed-load semantics of the boot path
(the typing mirror). A value leaf whose authored scalar is exactly one
substituted `${env:X:-d}` token is validated as the STRING `"d"`, with one
carve-out mirroring the boot path's numeric-knob repair: when the leaf sits
at an integer-typed schema position and `d` is a clean integer (lexical
form `-?(0|[1-9][0-9]*)`, parsing as i64 or u64 — the schema itself
enforces the target type's bounds), the leaf is validated as the NUMBER
(the boot loader coerces the same leaf, so flagging it would be a false
positive). Consequently:
string-typed and polymorphic positions validate cleanly (one Info-severity
diagnostic per substituted default, span-anchored on the authored
placeholder; a polymorphic position such as `set_header` value carries the
default as a JSON string, mirroring the loader's minimal-subset result);
integer-typed positions with clean-integer defaults produce no diagnostic;
integer-typed positions with non-integer defaults and boolean-typed
positions produce a schema type Error anchored on the placeholder value
node (the boot path rejects those). A no-default token (`${env:X}`) at any
value position MUST produce an Error (the boot path hard-fails on it; lint
must not stay silent); `$${env:...}` escapes are exempt. Comment tokens
MUST produce no diagnostics (comments are not part of the parsed
instance). Per-token semantics apply — a whole-document error fallback
that re-literals every token is forbidden. The interpolator is a
SYNC-annotated mirror of the whole-text splice arm of
`camel-dsl::env_interpolation` (crate purity forbids the dependency).

#### Scenario: string-position default yields no error and one Info note

- **Given** a route document with a `set_header` step value `${env:MY_TITLE:-hello}`
- **When** R-SCHEMA analyzes the document
- **Then** there are zero Error-severity R-SCHEMA diagnostics and exactly one
  Info-severity diagnostic reporting the substituted default, span-anchored on the
  placeholder value node

#### Scenario: integer-position clean-integer default yields no diagnostic

- **Given** a route document with `throttle.max_requests: ${env:MY_LIMIT:-2}`
- **When** R-SCHEMA analyzes the document
- **Then** no R-SCHEMA diagnostic is emitted for the field — the default `2`
  is a clean integer at an integer-typed position, so the validation copy
  carries the number `2` exactly as the boot loader coerces it

#### Scenario: integer-position non-integer default is flagged as a type error

- **Given** a route document with `throttle.max_requests: ${env:MY_LIMIT:-notanumber}`
- **When** R-SCHEMA analyzes the document
- **Then** an Error-severity R-SCHEMA diagnostic anchored on the placeholder reports
  the type violation (the carve-out does not apply; the validated leaf is the
  string `"notanumber"`, and the same document fails at boot)

#### Scenario: integer-position default is flagged as a type error

Supersession note: before the R-SCHEMA integer-position carve-out, every
integer-position env default was flagged. Clean-integer defaults now yield no
diagnostic — see the preceding scenario. This scenario now covers the
remaining flagged surface:

- **Given** a route document with `throttle.max_requests: ${env:MY_LIMIT:-2x}`
- **When** R-SCHEMA analyzes the document
- **Then** an Error-severity R-SCHEMA diagnostic anchored on the placeholder
  reports the type violation (`2x` is not a clean integer, the carve-out does
  not apply, and the same document fails at boot — boot parity)

#### Scenario: no-default token is flagged at any value position

- **Given** a route document with a `set_header` step value `${env:NO_DEFAULT_xyz}`
  (a polymorphic JSON position — the no-default Error applies at every value
  position regardless of typing)
- **When** R-SCHEMA analyzes the document
- **Then** an Error-severity diagnostic flags the unresolved token (the same document
  hard-fails at boot); `$${env:X}` escaped forms produce no diagnostic

#### Scenario: mixed document keeps per-token semantics

- **Given** a route document containing both a `set_header` step value
  `${env:WITH_DEF:-hello}` and
  `throttle.max_requests: ${env:ALSO_WITH_DEF:-2}`
- **When** R-SCHEMA analyzes the document
- **Then** the non-integer-position field validates cleanly with exactly one Info
  note (the header value stays the string default),
  AND the integer-position field produces no diagnostic — each token judged at its own
  position

#### Scenario: commented tokens produce no diagnostics

- **Given** a route document whose YAML comments contain `${env:X}` and
  `${env:Y:-d}` tokens
- **When** R-SCHEMA analyzes the document
- **Then** neither comment token produces any R-SCHEMA diagnostic

#### Scenario: engine never reads the process environment

- **Given** ambient `MY_TITLE=ambient` set in the environment and a document with
  a `set_header` step value `${env:MY_TITLE:-hello}`
- **When** R-SCHEMA analyzes the document
- **Then** the validated value derives from the literal default `"hello"`, not the
  ambient value (diagnostics are a pure function of document text; enforced by the
  lookup-only mirror, no `std::env` read)

#### Scenario: LSP inherits the interpolated validation

- **Given** an LSP session opens a document with a placeholder-with-default
  field at a non-integer position
- **When** diagnostics are published
- **Then** no ERROR diagnostic is published for the placeholder field and the Info
  note appears (LSP shares the lint engine; completions/hover still see the literal
  placeholder via the raw CST entrypoint)
