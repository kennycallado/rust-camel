## MODIFIED Requirements

### Requirement: R-SCHEMA validates an interpolated copy with tree-walk typing

R-SCHEMA MUST interpolate `${env:NAME:-default}` placeholders (default-only lookup,
MUST NOT read the process environment) into a validation copy before schema
validation, and MUST replicate the typed-load semantics of the boot path
(the typing mirror). A value leaf whose authored scalar is exactly one
substituted `${env:X:-d}` token is validated as the STRING `"d"`, with two
carve-outs mirroring the boot path's typed coercions. First, the integer
carve-out (numeric-knob repair): when the leaf sits at an integer-typed
schema position and `d` is a clean integer (lexical form
`-?(0|[1-9][0-9]*)`, parsing as i64 or u64 — the schema itself enforces the
target type's bounds), the leaf is validated as the NUMBER. Second, the
boolean carve-out: when the leaf sits at a boolean-typed schema position and
`d` is exactly the lowercase text `true` or `false` (the boot rule shared by
`camel-config`'s `clean_bool` and `camel-dsl`'s `clean_boolean`; any other
spelling — `TRUE`, `True`, `yes`, `on`, `1`, `0`, whitespace-padded forms —
is rejected by the boot loader and stays flagged), the leaf is validated as
the BOOLEAN. Float defaults are excluded everywhere: boot coerces only
clean integers and exact-lowercase booleans, so a float default (`2.0`,
`0.5`) at any numeric position keeps the STRING typing and produces the
type Error — no float carve-out exists. Both carve-outs are structured as
whole-scalar candidate lists collected during the typing-mirror walk and
consumed by `analyze`'s carve-out pass (Condition A: the STRING copy flags
the leaf's error chain; Condition B: a probe with the leaf replaced by the
typed copy clears that chain; the leaf is carved — and its Info note
suppressed — only when both hold). A leaf's error chain is flagged by an
error whose span covers the leaf — anchored at the leaf itself or collapsed
at an ancestor anyOf/oneOf node — and the same span coverage applies when
the probe must show the chain cleared. Both conditions MUST retain this
ancestor coverage: branch failures under anyOf surface at the nearest
non-matching schema node (step schemas use anyOf, not oneOf), so leaf-only
matching over-carves candidates that share a branch node and hides real
string errors. Consequently:
string-typed and polymorphic positions validate cleanly (one Info-severity
diagnostic per substituted default, span-anchored on the authored
placeholder; a polymorphic position such as `set_header` value carries the
default as a JSON string, mirroring the loader's minimal-subset result);
integer-typed positions with clean-integer defaults produce no diagnostic;
NUMBER-typed (non-integer, e.g. double) positions with clean-integer
defaults produce no diagnostic (the typed copy wins whenever the STRING
copy's chain clears); boolean-typed positions with exact-lowercase
`true`/`false` defaults produce no diagnostic; integer- and number-typed
positions with non-integer defaults and boolean-typed positions with any
non-boolean default produce a schema type Error whose span covers the
placeholder value node, possibly anchored at an ancestor anyOf node (the
boot path rejects those; genuinely leaf-anchored positions keep leaf
anchoring). The substituted-default Info note
MUST NOT be emitted when an Error-severity diagnostic's span covers the
same leaf: the note (which reports the default as validated) is truthful
only when the leaf's chain is error-free, and no leaf may show a validating
note alongside a concurrent type Error. Because an anyOf failure may be
collapsed at an ancestor node, the covering Error span can also cover
SIBLING leaves that are individually valid; their Info notes are suppressed
with the offending leaf's. This collateral suppression is bounded to Info
output — it MUST NOT remove, downgrade, or relocate any Error, and notes
whose leaf span no Error covers are unaffected.
A no-default token (`${env:X}`) at any value position MUST produce an Error
(the boot path hard-fails on it; lint must not stay silent); `$${env:...}`
escapes are exempt. Comment tokens MUST produce no diagnostics (comments are
not part of the parsed instance). Per-token semantics apply — a
whole-document error fallback that re-literals every token is forbidden. A
placeholder inside a larger scalar (not a whole-scalar token) never gets a
carve-out. The interpolator is a SYNC-annotated mirror of the whole-text
splice arm of `camel-dsl::env_interpolation`, and the boolean predicate is a
SYNC-annotated third mirror of `camel-config::clean_bool` /
`camel-dsl::clean_boolean` (crate purity forbids the dependencies; the
mirror is pinned by a lexical-gate test enumerating the accepted and
rejected forms).

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
  not apply, and the same document fails at boot — boot parity), and no
  substituted-default Info note accompanies the Error

#### Scenario: boolean-position true default yields no diagnostic

- **Given** a route document with `auto_startup: ${env:AUTO_STARTUP:-true}`
  (`auto_startup` is a strict boolean field in ROUTE_SCHEMA)
- **When** R-SCHEMA analyzes the document
- **Then** no Error-severity diagnostic is emitted for the field — the
  default `true` is accepted by the boot boolean rule at a boolean-typed
  position, so the validation copy carries the boolean `true` exactly as the
  boot loader coerces it (the substituted-default Info note is suppressed
  for the carved leaf, like carved integer leaves)

#### Scenario: boolean-position false default yields no diagnostic

- **Given** a route document with `auto_startup: ${env:AUTO_STARTUP:-false}`
- **When** R-SCHEMA analyzes the document
- **Then** no Error-severity diagnostic is emitted for the field — the
  validation copy carries the boolean `false` exactly as the boot loader
  coerces it

#### Scenario: boolean-position non-boolean defaults are flagged as type errors

- **Given** route documents with `auto_startup: ${env:AUTO_STARTUP:-yes}`,
  `auto_startup: ${env:AUTO_STARTUP:-TRUE}`, and
  `auto_startup: ${env:AUTO_STARTUP:-1}`
- **When** R-SCHEMA analyzes each document
- **Then** each produces an Error-severity R-SCHEMA diagnostic anchored on
  the placeholder value node (`yes`, `TRUE`, and `1` are rejected by the
  boot boolean rule; the validated leaf stays the string default — boot
  parity), and none of them carries a substituted-default Info note

#### Scenario: boolean-position no-default token keeps the unresolved Error

- **Given** a route document with `auto_startup: ${env:AUTO_STARTUP}`
- **When** R-SCHEMA analyzes the document
- **Then** the unresolved-token Error is produced exactly as before this
  change (the boolean carve-out never applies to no-default tokens)

#### Scenario: embedded placeholder never gets a carve-out

- **Given** a route document whose `auto_startup` value is a larger scalar
  containing a token (not a whole-scalar token), e.g.
  `auto_startup: "${env:AUTO_STARTUP:-true}x"` (token embedded in a quoted
  string with trailing text)
- **When** R-SCHEMA analyzes the document
- **Then** the boolean carve-out does not apply — the leaf is not a
  whole-scalar token, keeps STRING-copy semantics, and is flagged exactly as
  before this change (boot parity: the loader keeps embedded-token leaves
  strings and rejects them at a boolean position)

#### Scenario: boolean default at a string position keeps string semantics

- **Given** a route document with a `set_header` step value
  `${env:MY_FLAG:-true}` (a polymorphic/string JSON position)
- **When** R-SCHEMA analyzes the document
- **Then** there are zero Error-severity diagnostics and exactly one
  Info-severity note for the substituted default — the boolean carve-out
  does not apply outside boolean-typed positions (the leaf stays the JSON
  string, mirroring the loader's minimal-subset result)

#### Scenario: mixed integer and boolean candidates keep per-leaf semantics

- **Given** a route document containing both
  `throttle.max_requests: ${env:MY_LIMIT:-2}` (clean integer at an
  integer-typed position) and
  `auto_startup: ${env:AUTO_STARTUP:-true}` (clean boolean at a
  boolean-typed position)
- **When** R-SCHEMA analyzes the document
- **Then** neither field produces a diagnostic — each candidate is judged at
  its own leaf; and a document where an integer default sits at the boolean
  position (`auto_startup: ${env:AUTO_STARTUP:-1}`) still produces the type
  Error (the default `1` records an integer candidate, not a boolean one;
  the NUMBER probe fails at the boolean-typed position, so the leaf is not
  carved — boot parity)

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
  field at a string-typed or polymorphic position (not an integer- or
  boolean-typed position — typed positions follow their own carve-out rules
  above)
- **When** diagnostics are published
- **Then** no ERROR diagnostic is published for the placeholder field and the Info
  note appears (LSP shares the lint engine; completions/hover still see the literal
  placeholder via the raw CST entrypoint)

#### Scenario: delay_ms full-config default yields no diagnostic (rc-jq775 shape)

- **Given** route documents that parse successfully, written in block
  mapping form: a bare single route whose steps contain
  `delay:` with `delay_ms: ${env:OPS_X:-20000}` (the full-config form under
  `DelayBody`'s `anyOf[integer, DelayFullConfig]`), the same document with
  the token quoted (`delay_ms: "${env:OPS_X:-20000}"`), and both again
  wrapped in a `{routes: [...]}` envelope
- **When** R-SCHEMA analyzes each document
- **Then** each document parses (no R-SYN diagnostic) and no R-SCHEMA
  diagnostic is emitted for `delay_ms` in any of the four variants — the
  STRING copy's anyOf failure collapses at an ancestor node, and the
  ancestor-anchored Condition A coverage lets the NUMBER probe carve the
  leaf exactly as the boot loader coerces it

#### Scenario: delay_ms shorthand whole-body default yields no diagnostic

- **Given** a route document that parses successfully whose steps contain
  the shorthand form `delay: ${env:OPS_X:-20000}` (the whole delay body is
  one token in block mapping form, validated against
  `anyOf[integer, DelayFullConfig]` with branch 0 being the integer itself)
- **When** R-SCHEMA analyzes the document
- **Then** the document parses and no R-SCHEMA diagnostic is emitted for
  the delay body — the typed copy validates as the NUMBER against the
  integer branch exactly as boot coerces it

#### Scenario: delay_ms and dynamic_header under one anyOf carve per-leaf

- **Given** a route document that parses successfully with a delay step
  whose body is a block mapping carrying
  `delay_ms: ${env:OPS_X:-20000}` (clean-integer default at the
  integer-typed position) and `dynamic_header: ${env:HDR:-3000}`
  (clean-integer default at the `string|null` position) — two whole-scalar
  candidates under the same `DelayBody` anyOf
- **When** R-SCHEMA analyzes the document
- **Then** the document parses, `delay_ms` produces no diagnostic (carved
  to the NUMBER so the final instance is error-free), and `dynamic_header`
  produces exactly one Info note (its STRING default is valid at the
  `string|null` position — the leaf is not carved, no Error covers it in
  the carved final instance, and the ancestor-collapsed STRING-copy error
  must not cause the header leaf to be carved into a NUMBER that would
  type-error the string position)

#### Scenario: NUMBER-typed position with clean-integer default yields no diagnostic

- **Given** a route document that parses successfully with
  `error_handler:` carrying `retry:` with `max_attempts: 4` and
  `multiplier: ${env:M:-2}` (`multiplier` is a NUMBER-typed (double)
  position; `max_attempts` is required by `RouteDslRedeliveryPolicy`)
- **When** R-SCHEMA analyzes the document
- **Then** the document parses and no R-SCHEMA diagnostic is emitted for
  `multiplier` — the clean integer `2` is coerced to the NUMBER `2` by the
  boot probe (the f64 field parses the integer), so the typed copy wins and
  the leaf stays silent

#### Scenario: float default at a NUMBER position keeps the boot-parity Error

- **Given** route documents that parse successfully with
  `error_handler:` carrying `retry:` with `max_attempts: 4` and
  `multiplier: ${env:M:-2.0}`, and the same shape with
  `jitter_factor: ${env:J:-0.5}` (float defaults at NUMBER-typed positions)
- **When** R-SCHEMA analyzes each document
- **Then** each produces exactly one Error-severity R-SCHEMA diagnostic
  whose span covers the placeholder (the collapsed anyOf error anchors at
  the ancestor `error_handler` node; descendant anchoring is not required,
  and relocating Errors to the leaf is out of scope — boot's clean-integer
  probe rejects float defaults, so the same documents fail at boot and the
  Error is boot-correct) and no substituted-default Info note (the note is
  suppressed when an Error's span covers the leaf; it would contradict the
  concurrent Error)

#### Scenario: collateral Info suppression under a shared ancestor stays bounded to Info

- **Given** a route document that parses successfully with a delay step
  whose body carries `delay_ms: ${env:OPS_X:-2.0}` (float default at the
  integer-typed position — boot rejects it) and
  `dynamic_header: ${env:HDR:-x-custom}` (valid string default at the
  `string|null` position) under the same `DelayBody` anyOf, plus a disjoint
  `set_header` step value `${env:MY_TITLE:-hello}` (string position)
- **When** R-SCHEMA analyzes the document
- **Then** the float `delay_ms` still produces the boot-parity Error (one
  Error whose span covers the placeholder); the `dynamic_header` Info note
  is suppressed (its leaf span is covered by the same ancestor-anchored
  Error span — an informational omission only, never an Error omission);
  and the disjoint `set_header` Info note survives (its leaf span is not
  covered by any Error)

#### Scenario: empty default at an integer position keeps the Error without an Info note

- **Given** a route document that parses successfully with a steps entry
  `delay:` carrying `delay_ms: "${env:OPS_X:-}"` (empty default at the
  integer-typed position)
- **When** R-SCHEMA analyzes the document
- **Then** the document parses and an Error-severity diagnostic whose span
  covers the placeholder is emitted for the type violation (the empty
  string is not a clean integer; the same document fails at boot) and no
  substituted-default Info note accompanies it
