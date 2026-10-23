## ADDED Requirements

### Requirement: Typed probe coerces boolean positions in route documents

On typed-parse failure of an interpolated route document, the loader's typed
probe MUST treat a provenance leaf under the `routes` array whose substituted
value is exactly the text `true` or `false` (the clean boolean lexical gate —
no other spelling: no case folding, no surrounding whitespace, no `1`/`0`, no
`yes`/`no`) as a boolean candidate and coerce it to a YAML boolean. Boolean
candidates share the integer candidates' search unchanged: one bounded cap (a
document whose combined candidates exceed eight keeps the pass-1 error), the
same smallest-first document order, the same QUIET-parser oracle; the first
subset that parses wins, and if none does, the loader MUST return the pass-1
error. Coercion stays provenance-gated: a literal quoted `"true"` at a
boolean-typed route position is never a candidate and keeps failing, and a
string or polymorphic position whose placeholder resolves to `true` parses on
the first pass and keeps its string value. The logging-replay contract is
unchanged: speculative attempts stay quiet and the final failure replays
exactly once.

#### Scenario: boolean-typed route field with default loads via the typed probe

- **GIVEN** a route file with route-level `auto_startup: ${env:RC_AUTO:-false}`
- **WHEN** `load_from_file` parses the file
- **THEN** loading succeeds with `auto_startup == false` — the provenance leaf
  coerces and the typed parse accepts it

#### Scenario: boolean-typed route field honors an explicit lookup value

- **GIVEN** a route file with route-level `auto_startup: ${env:RC_AUTO:-false}`
- **WHEN** `load_from_file_with_env` parses the file with an injected lookup
  returning `Some("true")` for `RC_AUTO`
- **THEN** loading succeeds with `auto_startup == true`

#### Scenario: literal quoted boolean at a boolean position stays rejected

- **GIVEN** a route file with route-level `auto_startup: "true"` (no
  placeholder token)
- **WHEN** `load_from_file` parses the file
- **THEN** loading fails with the pass-1 error — no token, so the leaf is
  never a candidate

#### Scenario: string position resolving to true stays a string

- **GIVEN** a route file containing a `set_header` step with value
  `${env:RC_STR:-true}` and no boolean-typed placeholder
- **WHEN** `load_from_file` parses the file
- **THEN** loading succeeds and the header value equals the string `"true"`
  (mirror case; pass 1 parses, so the probe never runs)

#### Scenario: out-of-set placeholder value at a boolean position keeps today's error

- **GIVEN** a route file with `auto_startup: ${env:RC_B:-yes}` (likewise for
  defaults `TRUE`, `1`, and `true ` with trailing space)
- **WHEN** `load_from_file` parses the file
- **THEN** loading fails with the pass-1 error — none of these is a clean
  boolean; the variable is named only by the existing unresolved-variable
  error path, which fires before any probe

#### Scenario: mixed candidates beyond the probe cap keep today's error

- **GIVEN** a route document with eight boolean candidates and one integer
  candidate (nine combined, over the cap)
- **WHEN** `load_from_file` parses the file
- **THEN** loading fails with the pass-1 error — boolean candidates count
  against the same cap as integers
