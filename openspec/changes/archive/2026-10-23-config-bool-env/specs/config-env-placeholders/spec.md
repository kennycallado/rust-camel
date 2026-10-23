## ADDED Requirements

### Requirement: Placeholder-bearing boolean TOML leaves coerce after resolution

camel-config MUST extend the typed probe to boolean positions: a provenance
leaf whose resolved value is exactly the text `true` or `false` (the clean
boolean lexical gate — no other spelling: no case folding, no surrounding
whitespace, no `1`/`0`, no `yes`/`no`) is a boolean candidate and is coerced
to a TOML boolean in the probe's cloned trees. Boolean candidates share the
integer candidates' search unchanged: one bounded cap (a configuration whose
combined candidates exceed eight keeps the first-pass error), the same
smallest-first subset order, the same strict-deserialize oracle; the first
subset that deserializes wins, and if none does, the first-pass error is
returned. Coercion stays provenance-gated: a literal quoted `true` or `false`
with no placeholder token is never a candidate and stays rejected at a boolean
field, and a string-typed field whose placeholder resolves to `true`
deserializes on the first pass and keeps its string value. Documents that
deserialize today are unaffected — the probe runs only after the strict first
pass fails.

#### Scenario: boolean field with a default coerces

- **GIVEN** a `Camel.toml` root field `watch = "${env:CFG_WATCH:-true}"` with
  `CFG_WATCH` unset
- **WHEN** the configuration loads
- **THEN** the effective config carries `watch == true`

#### Scenario: resolved environment value coerces in a boolean field

- **GIVEN** a `Camel.toml` root field `watch = "${env:CFG_WATCH:-true}"` with
  `CFG_WATCH=false` in the lookup
- **WHEN** the configuration loads
- **THEN** the effective config carries `watch == false`

#### Scenario: literal quoted boolean stays rejected

- **GIVEN** a `Camel.toml` root field `watch = "true"` (no placeholder token)
- **WHEN** the configuration loads
- **THEN** deserialization fails with the first-pass error naming the field —
  the strict typed-field contract is unchanged for literals

#### Scenario: string field with boolean-looking placeholder stays a string

- **GIVEN** a config string field carrying a placeholder whose resolved value
  is `true` (e.g. `log_level = "${env:CFG_LL:-true}"`)
- **WHEN** the configuration loads
- **THEN** the field equals the string `"true"` — the first pass deserializes
  string fields, so the probe never runs (mirror case)

#### Scenario: out-of-set placeholder value in a boolean field keeps the first-pass error

- **GIVEN** a `Camel.toml` boolean field `watch` carrying a placeholder whose
  resolved value is `yes`, `TRUE`, `1`, or `true ` (trailing space)
- **WHEN** the configuration loads
- **THEN** deserialization fails with the first-pass error naming the field —
  none of these is a clean boolean; the variable is named only by the existing
  unresolved-variable error path, which fires before any probe

#### Scenario: mixed candidates within the probe cap coerce

- **GIVEN** a `Camel.toml` whose typed positions carry placeholder candidates
  totalling exactly eight across both kinds (booleans and integers)
- **WHEN** the configuration loads
- **THEN** loading succeeds — boolean and integer candidates share one
  smallest-first search under the same cap

#### Scenario: mixed candidates beyond the probe cap keep the first-pass error

- **GIVEN** a `Camel.toml` with eight boolean candidates and one integer
  candidate (nine combined, over the cap)
- **WHEN** the configuration loads
- **THEN** deserialization fails with the first-pass error — boolean
  candidates count against the same cap as integers
