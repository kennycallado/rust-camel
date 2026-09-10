# config-env-placeholders Specification

## Purpose
TBD - created by archiving change env-int-placeholder-typing. Update Purpose after archive.
## Requirements
### Requirement: Placeholder-bearing numeric TOML leaves coerce after resolution

camel-config MUST resolve `${env:NAME:-default}` placeholders in string leaves of
the TOML tree (the existing `resolve_tree_walk` semantics: strict leaves under
`security`, `datasources`, `idempotent_repo`, `cache_repo`; plain leaves
elsewhere; unresolved no-default placeholders fail naming the variable). The
resolver MUST record structural key/index paths (not dotted strings — keys
containing `.` must resolve unambiguously; dot-joined rendering is
diagnostic display only) of leaves that carried a `${env:` token (the
provenance set). When deserialization of the resolved tree fails,
camel-config MUST run a typed probe: candidate leaves — provenance leaves
whose resolved value is a clean integer (lexical form `-?(0|[1-9][0-9]*)`,
parsing as i64) — are coerced to TOML integers in copies of the tree,
subsets tried smallest-first in document order, and each copy re-deserialized.
The first subset that deserializes wins; if none does, the first-pass error
is returned. The probe is bounded: configurations whose candidates exceed
the probe cap (more than eight) keep the first-pass error. Environment overrides merge into the tree BEFORE placeholder resolution,
so a token-bearing override value (e.g.
`CAMEL_CACHE_REPO_MAX_ENTRIES="${env:N:-8}"`) follows the same
provenance-and-probe semantics as a file-authored leaf; a token-free
override value keeps today's typed contract unchanged. Coercion MUST be
provenance-gated: a literal quoted numeric with no placeholder token (e.g.
`timeout_ms = "1000"`) is never a candidate and stays rejected; a
string-typed field whose placeholder resolves to a numeric-looking value
(e.g. `log_level = "${env:CFG_LL:-debug}"`, or a string field resolving to
`"8080"`) deserializes on the first pass and keeps its string value.
Unresolved variables surface before any probe and keep the existing error
shape.

#### Scenario: quoted placeholder in an integer field resolves and coerces

- **GIVEN** a `Camel.toml` root field `timeout_ms = "${env:CFG_TIMEOUT_MS:-8000}"`
  with `CFG_TIMEOUT_MS` unset
- **WHEN** the configuration loads
- **THEN** the effective config carries `timeout_ms == 8000`

#### Scenario: resolved environment value coerces in an integer field

- **GIVEN** a `Camel.toml` root field `timeout_ms = "${env:CFG_TIMEOUT_MS:-8000}"`
  with `CFG_TIMEOUT_MS=9000` in the lookup
- **WHEN** the configuration loads
- **THEN** the effective config carries `timeout_ms == 9000`

#### Scenario: literal quoted numeric stays rejected

- **GIVEN** a `Camel.toml` root field `timeout_ms = "1000"` (no placeholder token)
- **WHEN** the configuration loads
- **THEN** deserialization fails with an error naming the field — the strict
  typed-field contract is unchanged for literals

#### Scenario: string field with numeric-looking placeholder value stays a string

- **GIVEN** a config string field carrying a placeholder whose resolved value
  is `8080` (e.g. `log_level = "${env:CFG_LL:-8080}"`)
- **WHEN** the configuration loads
- **THEN** the field equals the string `"8080"` — the first pass deserializes
  string fields, so the probe never runs (mirror case)

#### Scenario: token-free override keeps the typed contract

- **GIVEN** a loaded config with an integer-typed `cache_repo` field and the
  environment variable `CAMEL_CACHE_REPO_MAX_ENTRIES=notanumber`
- **WHEN** env overrides merge and the config loads
- **THEN** the non-numeric override fails typed parsing — it carries no
  `${env:` token, so it is never a probe candidate

#### Scenario: token-bearing override coerces like a file-authored leaf

- **GIVEN** a loaded config with an integer-typed `cache_repo` field and the
  environment variable `CAMEL_CACHE_REPO_MAX_ENTRIES=${env:CFG_N:-8}` with
  `CFG_N` unset
- **WHEN** env overrides merge and the config loads
- **THEN** the effective config carries the value `8` — the override merged
  before resolution, its token-bearing leaf entered the provenance set, and
  the probe coerces it exactly as a file-authored leaf

#### Scenario: non-integer placeholder default in an integer field keeps the first-pass error

- **GIVEN** a `Camel.toml` integer field `timeout_ms = "${env:CFG_TIMEOUT_MS:-notanumber}"`
- **WHEN** the configuration loads
- **THEN** deserialization fails with the first-pass error naming the field —
  coercion applies only to clean integers

#### Scenario: leading-zero placeholder value in an integer field keeps the first-pass error

- **GIVEN** a `Camel.toml` integer field `timeout_ms = "${env:CFG_TIMEOUT_MS:-007}"`
  with `CFG_TIMEOUT_MS` unset
- **WHEN** the configuration loads
- **THEN** deserialization fails with the first-pass error naming the field —
  leading-zero values are not clean integers (octal ambiguity)

#### Scenario: overflowing placeholder value in an integer field keeps the first-pass error

- **GIVEN** a `Camel.toml` integer field
  `timeout_ms = "${env:CFG_TIMEOUT_MS:-9223372036854775808}"` (i64::MAX + 1)
- **WHEN** the configuration loads
- **THEN** deserialization fails with the first-pass error naming the field —
  the value parses as neither i64 nor a narrower target

#### Scenario: beyond the probe cap keeps the first-pass error

- **GIVEN** a `Camel.toml` whose integer-typed positions carry more
  placeholder candidates than the probe cap (more than eight)
- **WHEN** the configuration loads
- **THEN** deserialization fails with the first-pass error — the probe is
  bounded by design

#### Scenario: unresolved no-default placeholder keeps the variable-name error

- **GIVEN** a `Camel.toml` field carrying `${env:CFG_MISSING}` with no default and
  no value in the lookup
- **WHEN** the configuration loads
- **THEN** loading fails with the existing unresolved-variable error naming
  `CFG_MISSING`, before any probe

