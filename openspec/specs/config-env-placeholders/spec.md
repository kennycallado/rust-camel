# config-env-placeholders Specification

## Purpose
`${env:...}` placeholder resolution in TOML configuration. Numeric and other
typed leaves coerce after resolution, using the existing strict-versus-plain
leaf rules, and a missing placeholder without a default fails with the
variable name.
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

### Requirement: Component-block TOML leaves coerce at the bundle seam

camel-config MUST make the resolution provenance of the `components.*` subtree
reachable beyond `CamelConfig` deserialization: the provenance recorded for the
whole resolved tree (the same set the root-field probe consumes, in the
resolver's traversal order — table map iteration, then array index order, not
source-text order) is split by component block (`components.<key>` parents
stripped, structural key/index paths preserved) and carried on
`ComponentsConfig` behind the public cross-crate accessor `provenance_for`,
without changing the type or values of `ComponentsConfig.raw` — resolution
happens before deserialization, and coercion MUST NOT rewrite `raw` (its
values stay the resolved strings). The coercion guarantee applies to
loader-produced blocks: a caller that edits `ComponentsConfig.raw` after load
invalidates the recorded provenance and MUST rebuild the config
(default-construct `ComponentsConfig`, assign the edited map) so no stale path
coerces a token-free value. camel-bundles MUST run the typed probe at the
bundle seam: `bundle_from_config` first deserializes the block through the
bundle's own `from_toml`; only after that fails does it coerce candidate
leaves — provenance-backed leaves of THAT block whose resolved value is a
clean integer (lexical form `-?(0|[1-9][0-9]*)`, parsing as i64) or a clean
boolean (exactly `true`/`false`) — in copies of the block, subsets tried
ascending size in traversal order, each copy retried through the same
`from_toml`. The first subset that deserializes wins; if none does, the
first-pass `CamelError` is returned verbatim. The per-block probe is bounded:
a block whose candidates exceed the probe cap (more than eight) keeps the
first-pass error; the cap applies after a successful root load (the root-field
probe keeps its existing whole-tree candidate count and cap unchanged). The
search is scoped to the block being deserialized: provenance of other blocks
or of non-component subtrees MUST NOT enter it. Coercion stays
provenance-gated and failure-gated: a literal quoted numeric with no
placeholder token is never a candidate and stays rejected; a string field
whose placeholder resolves to a numeric-looking value deserializes on the
first pass and keeps its string; an unrelated bundle error (one no candidate
subset can fix) surfaces verbatim. The bundle's own `try_into` remains the
type oracle — no per-type schemas, no second placeholder resolution, and no
`ComponentBundle::from_toml` signature or implementation changes. Ownership is
unchanged: camel-config owns resolution and provenance recording; each bundle
owns its config key and parsing; `provenance_for` only hands the block's
metadata across that boundary. Bundles keep their parsed configs private, so
tests that assert exact field values observe them through test-local
`ComponentBundle` adapters over the real config types (an adapter with the
real `config_key` whose `from_toml` runs the real typed `try_into` and
exposes the parsed config); error scenarios compare the exact error variant
and payload against a direct `from_toml` of the same raw block. The cap
scenarios cover the boundary: eight candidates in one block still search,
nine in one block keep the first-pass error, and nine split across two blocks
(after a successful root load) both parse — the per-block cap is per-block,
not whole-tree.

#### Scenario: integer leaf set from the environment value coerces

- **GIVEN** a loaded config carrying
  `[components.redis] port = "${env:REDIS_PORT}"` with `REDIS_PORT=7100` in
  the lookup
- **WHEN** `bundle_from_config` parses the `redis` block through the real
  redis target type (`RedisConfig`)
- **THEN** the parsed config carries `port == 7100`

#### Scenario: integer leaf coerces to the placeholder default

- **GIVEN** a loaded config carrying
  `[components.redis] port = "${env:REDIS_PORT:-6379}"` with `REDIS_PORT`
  unset
- **WHEN** `bundle_from_config` parses the `redis` block through the real
  redis target type (`RedisConfig`)
- **THEN** the parsed config carries `port == 6379`

#### Scenario: Option-typed integer leaf coerces

- **GIVEN** a block whose typed target carries an `Option<u16>` leaf, with
  `fault_status = "${env:CXF_FAULT:-503}"` and the variable unset (exercised
  through the same helper with a test-local typed target — the real CXF field
  is nested)
- **WHEN** the block is parsed after its first pass fails
- **THEN** the leaf deserializes as `Some(503)`

#### Scenario: boolean leaf coerces

- **GIVEN** a loaded config carrying
  `[components.ws] binary_payload = "${env:WS_BIN:-true}"` with `WS_BIN`
  unset (`binary_payload` is `Option<bool>` on the ws target)
- **WHEN** `bundle_from_config` parses the `ws` block through the real ws
  target type
- **THEN** the parsed config carries `binary_payload == Some(true)`

#### Scenario: numeric-looking string leaf stays a string

- **GIVEN** a loaded config carrying
  `[components.redis] host = "${env:REDIS_HOST:-1234}"` with the variable
  unset (`host` is a `String` field; the default resolves numeric-looking)
- **WHEN** `bundle_from_config` parses the `redis` block
- **THEN** the first pass deserializes and `host` keeps the resolved string
  `"1234"` — no coercion runs because no block failed

#### Scenario: literal quoted numeric stays rejected

- **GIVEN** a loaded config carrying
  `[components.ws] max_connections = "8080"` (a quoted numeric with no
  placeholder token; `max_connections` is `Option<u32>`)
- **WHEN** `bundle_from_config` parses the `ws` block
- **THEN** parsing fails with the first-pass `invalid type: string` error —
  no token, no provenance, no candidate

#### Scenario: token-free override unchanged

- **GIVEN** the `CAMEL_*` environment-override allowlist (which addresses
  allowlisted root fields only and never supplies component leaves) and the
  existing token-free-override root scenarios
- **WHEN** the configuration loads and any bundle seam parses
- **THEN** override semantics are unchanged by this requirement: a component
  leaf never gains provenance from a token-free override, and root-field
  override behavior keeps today's contract

#### Scenario: unrelated bundle error surfaces verbatim

- **GIVEN** a loaded config carrying a `redis` block with a placeholder
  `port` candidate AND an untokened wrong-typed leaf
  `connection_timeout_secs = "not-a-number"` (`u64` field)
- **WHEN** `bundle_from_config` parses the block, comparing against a direct
  `from_toml` of the same raw block
- **THEN** every candidate subset still fails, and the returned error is the
  first-pass `CamelError` verbatim (same variant and payload as the direct
  call)

#### Scenario: two blocks each with one candidate both coerce

- **GIVEN** a loaded config carrying `[components.redis]` and
  `[components.http]` blocks whose integer leaves are placeholders resolving
  to clean integers
- **WHEN** `bundle_from_config` parses each block with its own bundle
- **THEN** each bundle's leaf is typed from its own block's provenance —
  neither block sees the other's candidates

#### Scenario: eight candidates in one block still search

- **GIVEN** a loaded config carrying a component block with exactly eight
  provenance candidates (clean integer/boolean leaves on that block's typed
  numeric/boolean fields)
- **WHEN** the bundle seam parses the block and the full coercion subset
  deserializes
- **THEN** the leaves are typed — the search runs at the cap boundary

#### Scenario: nine candidates in one block keep the first-pass error

- **GIVEN** a loaded config carrying a component block with nine provenance
  candidates
- **WHEN** the bundle seam parses the block
- **THEN** the cap is exceeded before any subset search and the first-pass
  `CamelError` is returned verbatim

#### Scenario: nine candidates split across two blocks both parse

- **GIVEN** a loaded config carrying two component blocks, five candidates in
  the first and four in the second (nine whole-tree component candidates,
  root load successful)
- **WHEN** `bundle_from_config` parses each block
- **THEN** both blocks coerce — the per-block cap is per-block, not
  whole-tree

#### Scenario: root probe still counts component candidates

- **GIVEN** a config with a root token
  `timeout_ms = "${env:T:-1500}"` plus eight clean component-block
  placeholders (nine whole-tree candidates)
- **WHEN** the configuration loads
- **THEN** the root-field probe exceeds its whole-tree cap and the load fails
  with the root first-pass error — bundle parsing is never reached (existing
  root-arm behavior, pinned so the route-document change cannot regress it)

