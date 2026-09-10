# dsl delta — env-int-placeholder-typing

## MODIFIED Requirements

### Requirement: Env-lookup-injected route discovery

Route discovery SHALL resolve `${env:}` placeholders through the injected environment lookup, applying interpolation to parsed YAML string keys and string leaves only, and only where the scalar text contains a placeholder or escape token. Interpolated leaves SHALL resolve as string scalars (numeric-looking results keep string typing — the camel-config leaf-interpolation precedent), and discovery SHALL track which leaves were whole-scalar substituted tokens (provenance) so the loader-layer typed probe can coerce integer positions after a failed typed parse. For documents the tree walk processes, comment content SHALL never be interpolated and SHALL never fail resolution. The escape grammar (`$${env:X}`, `$$`) SHALL keep raw-splice semantics inside each string. If the raw text cannot be parsed as YAML by the same parser discovery hands off to, or the parsed tree contains tagged nodes, discovery SHALL fall back to whole-text raw interpolation with legacy semantics (including legacy comment sensitivity for those documents; the typed probe does not apply on the fallback path).

#### Scenario: numeric-looking interpolation result stays a string

- **GIVEN** a route value `port: ${env:PORT}` with `PORT` resolving to `8080`
- **WHEN** discovery interpolates the document
- **THEN** the leaf parses back as the string `"8080"`, not a number

#### Scenario: integer-typed step field with default loads through the typed probe

- **GIVEN** a route step `throttle: {max_requests: ${env:DISC_LIMIT:-2}}`
- **WHEN** discovery interpolates the document with `DISC_LIMIT` unset and
  loads the routes
- **THEN** the route compiles with `max_requests == 2` — pass-1 typing keeps
  the leaf a string, the typed parse fails, and the probe coerces the
  provenance leaf to the integer `2`

#### Scenario: integer-typed step field honors the injected lookup

- **GIVEN** a route step `throttle: {max_requests: ${env:DISC_LIMIT:-2}}`
  and an injected lookup returning `Some("5")` for `DISC_LIMIT`
- **WHEN** discovery runs through the env-injected entry
- **THEN** the route compiles with `max_requests == 5`

#### Scenario: placeholders in comments do not fail resolution

- **GIVEN** a route file containing `${env:MISSING}` inside a YAML comment and no default
- **WHEN** discovery interpolates with a lookup that does not define `MISSING`
- **THEN** the document loads and the comment content is ignored

#### Scenario: quoted hash survives interpolation

- **GIVEN** a route value containing a quoted `#` character
- **WHEN** discovery interpolates the document
- **THEN** the value keeps its `#` (tree-walk interpolation, not comment stripping)

#### Scenario: block scalar content interpolates as a value

- **GIVEN** a literal block scalar whose content contains `${env:X}`
- **WHEN** discovery interpolates with `X` defined
- **THEN** the placeholder resolves inside the block content, matching raw-splice behavior
#### Scenario: injected lookup resolves placeholders

- **GIVEN** a route file containing `from: direct:${env:TIER_ONLY}` and an
  injected lookup that maps `TIER_ONLY` to `start`
- **WHEN** discovery runs through the env-injected entry
- **THEN** the route compiles with the `direct:start` endpoint

#### Scenario: process environment is not consulted

- **GIVEN** a route file containing `${env:PROC_ONLY}`, a process
  environment that defines `PROC_ONLY`, and an injected lookup that
  returns `None` for `PROC_ONLY`
- **WHEN** discovery runs through the env-injected entry
- **THEN** discovery fails with the environment error naming `PROC_ONLY`
  and the file path, without reading the process environment

#### Scenario: templates materialize through the injected entry

- **GIVEN** a route file declaring one template and two templated routes,
  and an injected lookup resolving every placeholder the file uses
- **WHEN** discovery runs through the env-injected entry with a threshold
  and a security compile context
- **THEN** both templated routes materialize and compile with the given
  threshold and security context, identical to the process-environment
  entry over the same file after equivalent interpolation

#### Scenario: JSON route files never re-infer integer placeholders

- **GIVEN** a JSON route file containing an integer-typed field whose value is
  the string `"${env:J_LIMIT:-2}"`
- **WHEN** discovery loads it
- **THEN** loading fails with the type error — JSON has no plain-scalar
  re-inference and this failed before 0.42.0 as well (non-goal, not a
  regression)

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

On typed-parse failure of the interpolated document, the loader MUST run a
typed probe over the interpolated tree: candidate leaves — those under the
`routes` array whose authored scalar was exactly one whole-scalar
`${env:...}` token (tracked as provenance by the tree walk) and whose
substituted value is a clean integer (lexical form `-?(0|[1-9][0-9]*)`,
parsing as i64 or u64) — are coerced to
numbers in copies of the tree, subsets tried smallest-first in document
order, and the copy re-parsed with the same typed parser. The first —
smallest — subset that parses wins: every parsing subset must coerce all
integer-typed positions, so the smallest parsing subset is unique and
contains exactly those positions (polymorphic supersets parse too, but the
size-first search never returns them); if none does, the loader MUST
return the pass-1 error.
The probe MUST NOT run when pass 1 parses, on the legacy-fallback path, or
for documents whose candidates exceed the probe cap (they keep the pass-1
error). Candidates are collected ONLY under the `routes` array — REST
blocks, route templates, and every other subtree keep today's semantics.
Positions carrying substituted values keep today's semantics even in mixed
documents: strict string-typed positions reject an integer outright, and
polymorphic positions (`set_header` value wraps a JSON value) stay
string-valued because the size-first search returns the unique minimal
parsing subset.
Unsigned bounds and narrowing are enforced by the real typed parse, not by
the coercion.

#### Scenario: string-typed field with default interpolates

- **Given** a route file containing a `set_header` step with value
  `${env:RC_T:-hello}`
- **When** `load_from_file` parses the file
- **Then** loading succeeds and the header value equals `"hello"`

#### Scenario: integer-typed field with default loads via the typed probe

- **Given** a route file containing a route step
  `throttle: {max_requests: ${env:MY_LIMIT:-2}}`
- **When** `load_from_file` parses the file
- **Then** loading succeeds with `max_requests == 2` — the provenance leaf
  coerces and the typed parse accepts it

#### Scenario: integer-typed field with default fails with type mismatch (boot parity)

Supersession note: before the typed probe, every env-substituted default at
an integer position failed this way. Defaults that parse as a whole integer
now load — see the preceding scenario. The failure surface that remains:

- **Given** a route file containing a route step
  `throttle: {max_requests: ${env:MY_LIMIT:-abc}}`
- **When** `load_from_file` parses the file
- **Then** loading fails with today's type-mismatch error, matching
  discovery and `camel run` behavior exactly (boot parity)

#### Scenario: integer-typed field honors an explicit lookup value

- **Given** a route file containing a route step
  `throttle: {max_requests: ${env:MY_LIMIT:-2}}`
- **When** `load_from_file_with_env` parses the file with an injected lookup
  returning `Some("5")` for `MY_LIMIT`
- **Then** loading succeeds with `max_requests == 5`

#### Scenario: mixed document keeps string semantics and coerces the integer

- **Given** a route file containing, in the same route, a
  `set_header` step with value `${env:RC_H:-123}` (a polymorphic JSON
  position) and a `throttle` step with `max_requests: ${env:RC_L:-2}`
- **When** `load_from_file` parses the file
- **Then** loading succeeds, the header value equals the string `"123"`,
  and `max_requests == 2` — the returned minimal subset contains exactly
  the strict integer position, so the polymorphic leaf stays a string

#### Scenario: mixed document in the opposite order behaves identically

- **Given** the same mixed document with the `throttle` step placed before
  the `set_header` step
- **When** `load_from_file` parses the file
- **Then** loading succeeds with `max_requests == 2` and the header value
  equals the string `"123"` — the result is independent of document order

#### Scenario: beyond the probe cap keeps today's error

- **Given** a route file whose integer-typed positions carry more
  placeholder candidates than the probe cap (more than eight)
- **When** `load_from_file` parses the file
- **Then** loading fails with today's pass-1 error — the probe is bounded
  by design

#### Scenario: string-typed field with numeric-looking default stays a string

- **Given** a route file containing a `set_header` step with value
  `${env:RC_NUM:-123}` and no integer-typed placeholder
- **When** `load_from_file` parses the file
- **Then** loading succeeds and the header value equals the string `"123"`
  (mirror case; pass 1 parses, so the probe never runs)

#### Scenario: embedded-token leaf at an integer position stays rejected

- **Given** a route file containing a route step
  `throttle: {max_requests: p-${env:RC_N:-2}}`
- **When** `load_from_file` parses the file
- **Then** loading fails with today's type error — only whole-scalar
  placeholder leaves are probe candidates

#### Scenario: non-integer default at an integer position keeps today's error

- **Given** a route file containing a route step
  `throttle: {max_requests: ${env:RC_J:-notanumber}}`
- **When** `load_from_file` parses the file
- **Then** loading fails with today's type error — the value is not a clean
  integer, so no candidate exists

#### Scenario: leading-zero default at an integer position keeps today's error

- **Given** a route file containing a route step
  `throttle: {max_requests: ${env:RC_Z:-007}}`
- **When** `load_from_file` parses the file
- **Then** loading fails with today's type error — leading-zero values are
  not clean integers (YAML 1.1 octal ambiguity)

#### Scenario: negative default at an unsigned position keeps today's error

- **Given** a route file containing a route step
  `throttle: {max_requests: ${env:RC_NEG:--2}}`
- **When** `load_from_file` parses the file
- **Then** loading fails with today's type error — the coerced `-2` fails
  the `usize` typed parse, so no subset succeeds

#### Scenario: overflowing default at an integer position keeps today's error

- **Given** a route file containing a route step
  `throttle: {max_requests: ${env:RC_BIG:-99999999999999999999999}}`
- **When** `load_from_file` parses the file
- **Then** loading fails with today's type error — the value parses as
  neither i64 nor u64

#### Scenario: unset variable without default errors naming the variable

- **Given** a route file containing a `set_header` step with value
  `${env:UNSET_NO_DEF}` and no ambient value
- **When** `load_from_file` parses the file
- **Then** loading fails with an error naming `UNSET_NO_DEF` (mirroring
  `DiscoveryError::Env` wording), not a serde "did not match" error

#### Scenario: ambient environment is ignored by the default path

- **Given** a route file containing a `set_header` step with value
  `${env:AMBIENT_VAR:-hello}` and ambient `AMBIENT_VAR=goodbye`
- **When** `load_from_file` (no explicit lookup) parses the file
- **Then** the header value resolves to `"hello"` — the default-only path
  never consults ambient values

#### Scenario: explicit lookup is the sole injection point for ambient values

- **Given** a route file containing a `set_header` step with value
  `${env:AMBIENT_VAR:-hello}`
- **When** `load_from_file_with_env` parses the file with an injected lookup returning
  `Some("goodbye")`
- **Then** the header value resolves to `"goodbye"` — ambient values enter only through the
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
  no placeholder-env wording appears for an otherwise-resolvable document, and the
  typed probe does not run on the fallback path
