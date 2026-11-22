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

### Requirement: `${env:}` at typed route-document positions outside `routes:` coerce after resolution

camel-dsl MUST widen the loader-layer typed probe (env-int probe) from
provenance paths under the top-level `routes:` key to the whole route
document: after a failed first typed parse of an interpolated document,
candidate leaves — provenance paths resolving to a whole-scalar substituted
value that is a clean integer (lexical form `-?(0|[1-9][0-9]*)`, i64 then
u64) or a clean boolean (exactly `true`/`false`) — are coerced in cloned
trees, subsets tried smallest-first in document order (cap eight combined
candidates), with the real typed document parse INCLUDING compilation as the
only oracle; the first parsing subset wins and otherwise the first-pass error
stands. The candidate set MUST exclude the two stay-out subtrees — paths
whose first segment is `templates` or `templated_routes` — so their
numeric-looking leaves never consume cap budget: `templated_routes.parameters`
is string-typed by contract (a coerced number can only fail a subset, never
fix one), and template bodies (`templates[].routes[]`) are untyped at
document parse (typing happens at materialization, which the document-parse
probe never observes). Every probed subtree (`routes:`, `rest:`, `mcp:`)
shares one cap: nine combined candidates from probed subtrees keep the
first-pass error, eight still search. The probe oracle spans deserialization
and compilation, so the minimal repair set S_min is the set of candidate
positions whose coercion is REQUIRED for full oracle success: typed
deserialization failures, plus polymorphic positions where the string
deserializes but compilation rejects it and the coerced value passes (e.g. a
`request_schema` leaf carrying `${env:SCHEMA:-true}` — the string
deserializes into the schema field but compilation expects valid JSON Schema,
which boolean `true` satisfies). The smallest-first search returns the FIRST
minimum-cardinality parsing subset in document order: when the repair set is
unique (the typed-failure case) every parsing subset must contain all of
S_min, so the search returns exactly S_min and never a gratuitous polymorphic
superset; when multiple independent sufficient subsets exist, the
document-order-first minimum wins deterministically. Coercion never touches
non-candidates; unsigned bounds, narrowing, and negatives are enforced by the
real parse, never by heuristics. This makes `rest:` block TYPED fields
(`RouteDslRest.port: u16`, and the HTTP error-mapping code maps) reachable
from `${env:}`; string fields such as `host` are not repair targets —
interpolation already loads them and they are unchanged. Pass-1 behavior is
unchanged: every document that parses today still parses with the same
values. Repair outcomes are classified: (a) routes:-only probe repairs are
preserved — candidate set and document order are identical under the widened
filter — EXCEPT when widened candidates from other subtrees push the count
over the cap (e.g. the eight-throttle fixture plus a numeric-looking
`$schema` root token — a string-typed position the value filter admits but
whose coercion can never repair): those keep the first-pass error, the same
outcome the cap produces today for over-cap documents; this boundary is
pinned, not hidden; (b) collision-guarded documents are EXCLUDED from the
preservation promise: a document whose provenance includes a stale path — a
substituted entry overwritten by a later mapping key — loses that path to
the insert-time collision guard, and a document that probe-loads today ONLY
through the stale path keeps the first-pass error (reproduced case pinned
below); the guard is a correctness fix — a surviving literal must never
coerce on a token's provenance — and the promise covers only documents whose
provenance is collision-free; (c) widened-scope repairs (`rest:`/`mcp:`
candidates) turn prior hard failures into loads — the purpose of this
requirement.
Typed template
numerics flow through the existing typed-parameter machinery
(`TemplateParamType::Number` / `Boolean` whole-node substitution at
materialization); a template-body DIRECT `${env:}` numeric (no typed
parameter) keeps today's documented materialization limitation. Provenance
gates are unchanged: only whole-scalar substituted placeholders are
candidates; embedded tokens (`p-${env:X}`) keep string typing, and a literal
quoted numeric with no placeholder token stays rejected. Interpolation
provenance MUST be collision-guarded: YAML mapping keys can collide, and the
YAML walk (whose maps preserve insertion order) detects insert-time
collisions — an entry overwritten by a later key has its path (and all
descendant paths) dropped from the provenance set regardless of value, so
neither an overwritten token nor a surviving same-value literal is ever a
candidate; a substituted key that overwrites an earlier literal keeps
provenance. The JSON repair path additionally rejects duplicate-key
documents outright (default `serde_json` maps are key-sorted and cannot
preserve authored overwrite order, so a duplicate-key document is never
repaired and keeps today's first-pass error), detects substitution-produced
key collisions by lookup, and records JSON provenance in the walk's
deterministic key-sorted iteration order (not authored source order — the
crate-wide `preserve_order` feature is deliberately rejected); the probe's
minimum-cardinality guarantee is order-independent and ties resolve
deterministically in that order.

#### Scenario: rest port from a placeholder default coerces

- **GIVEN** a route document whose `rest:` block carries
  `port: ${env:RC_REST_PORT:-8080}` with the variable unset
- **WHEN** the document loads through the loader seam
  (`parse_routes_with_env` / discovery)
- **THEN** the REST block carries `port == 8080` and `host` keeps its
  authored value

#### Scenario: rest port from a resolved environment value coerces

- **GIVEN** the same document with `RC_REST_PORT=9090` in the lookup
- **WHEN** the document loads
- **THEN** the REST block carries `port == 9090`

#### Scenario: routes-only document is byte-identical in behavior

- **GIVEN** a `routes:`-only document with an integer placeholder under
  `routes:` (the existing probe fixture shape), and the same document
  accompanied by numeric-looking `${env:}` strings under
  `templated_routes.parameters`
- **WHEN** each document loads
- **THEN** the route placeholder is fixed exactly as before the widening —
  the parameter-shaped strings are excluded from candidates, consume no cap
  budget, and the parameter values remain the strings — same candidates,
  same smallest-first search, same result as the pre-change probe

#### Scenario: polymorphic compile-required position coerces

- **GIVEN** a `rest:` operation whose `request_schema` leaf carries
  `${env:SCHEMA:-true}` (deserializes as a string, rejected by compilation)
  alongside a `port` placeholder that fails typed deserialization
- **WHEN** the document loads
- **THEN** both positions coerce — the block carries `port == <typed>` and
  the schema carries the boolean `true` — S_min spans deserialization and
  compilation requirements, with no search loop beyond the one bounded
  subset search

#### Scenario: template numeric through the typed-parameter machinery loads

- **GIVEN** a template declaring a `number` parameter used at a numeric body
  position, and a `templated_routes` instantiation supplying that parameter
  as `${env:RC_TPL_DELAY:-500}`
- **WHEN** the document loads through discovery and the route materializes
- **THEN** the materialized route carries the typed value `500` — the
  existing whole-node typed substitution is fed by the interpolated
  parameter string

#### Scenario: template-body direct token keeps the materialization limitation

- **GIVEN** a document whose template body carries a direct `${env:}` token
  at a numeric position (no typed parameter), alongside a `rest:` port
  placeholder that the widened probe repairs at document parse
- **WHEN** the document loads and the templated route materializes
- **THEN** the document parse succeeds (the widened probe repaired the
  `rest:` position) and materialization of the direct-token template body
  fails with today's typed error — the stay-out is pinned end-to-end

#### Scenario: embedded token at a typed position keeps the first-pass error

- **GIVEN** a `rest:` block carrying `port: p-${env:RC_EMB:-8080}` (token
  embedded in larger text)
- **WHEN** the document loads
- **THEN** the first-pass type error stands — embedded tokens are not
  whole-scalar candidates and are never probed

#### Scenario: non-numeric, leading-zero, and overflowing values keep the first-pass error

- **GIVEN** a `rest:` block whose `port` placeholder resolves (via default or
  lookup) to `notanumber`, `007`, or a value beyond u64
- **WHEN** the document loads
- **THEN** the first-pass error stands in all three cases — non-clean values
  are never candidates, and out-of-range clean integers fail the real parse

#### Scenario: probe cap is shared across probed subtrees

- **GIVEN** a document whose candidates total nine across `routes:` and
  `rest:` combined (both probed subtrees), and a second document with exactly
  eight
- **WHEN** each document loads
- **THEN** the nine-candidate document keeps the first-pass error and the
  eight-candidate document loads — one budget, counted from probed subtrees
  only

#### Scenario: cap-compatibility boundary on a string-typed position

- **GIVEN** the existing eight-throttle probe fixture plus a numeric-looking
  root `$schema` token (a string-typed position whose coercion can never
  repair anything)
- **WHEN** the document loads
- **THEN** the widened candidate count is nine and the first-pass error
  stands — the accepted cap boundary, the same outcome the cap produces
  today for over-cap documents, pinned explicitly

#### Scenario: unresolved no-default placeholder names the variable

- **GIVEN** a `rest:` block carrying `port: ${env:RC_MISSING}` with no
  default and no lookup value
- **WHEN** the document loads
- **THEN** loading fails naming `RC_MISSING` before any probe runs

#### Scenario: same-value duplicate-key collision never lends provenance

- **GIVEN** a mapping where a substituted `${env:}` leaf's value `"404"` is
  overwritten by a duplicate literal key whose surviving value is also
  `"404"`
- **WHEN** the document loads after a failed first pass elsewhere
- **THEN** the surviving literal is never a probe candidate — insert-time
  collision detection drops the stale path regardless of value equality

#### Scenario: below-cap stale-provenance repair loss is pinned

- **GIVEN** a routes:-only throttle document whose mapping carries the
  substituted key `'${env:KEY:-max_requests}'` (substituting onto
  `max_requests`) with value `'${env:VALUE:-404}'`, followed by a literal
  `max_requests: '404'` — a document that probe-loads today with one
  candidate only through the stale path
- **WHEN** the document loads under this change
- **THEN** the collision guard drops the stale provenance and the first-pass
  error stands BELOW the cap — the probe-repair through the stale path is
  intentionally lost; a collision-guarded document is excluded from the
  preservation promise, pinned as the accepted correctness boundary

### Requirement: JSON route documents carry provenance and probe

camel-dsl MUST give JSON route documents the same typed-repair seam as YAML
while preserving today's first-pass behavior byte-for-byte: the FIRST pass
keeps the legacy whole-text splice and the existing parse (escape decoding,
values, error text, and warnings unchanged for every document that loads
today); ONLY after that first pass fails does the repair pass run, and ONLY
on the legacy semantic baseline — (1) the splice-substituted text must still
parse as JSON AND carry no duplicate object keys (a substitution that broke
syntax, or a duplicate key anywhere, means no repair and today's first-pass
error stands: default `serde_json` maps are key-sorted and cannot preserve
authored overwrite order, so duplicate-key documents are never repaired),
(2) the splice-parsed tree and the tree-walk-parsed tree must be equal as
values (escape sanitization or any other drift means no repair), and (3) the
coercion subset must be nonempty (a repair pass with zero candidates
re-parses nothing). The repair pass then runs the SAME bounded
smallest-first probe (one subset-search loop, cap eight over the same
candidate rules and stay-out exclusions, clean integer/boolean lexical
gates) with a `serde_json`-native coerced copy: the probe re-serializes as
JSON and the JSON document parse is the oracle. JSON provenance is recorded
in the walk's deterministic key-sorted iteration order — not authored source
order (the crate-wide `preserve_order` feature is deliberately rejected) —
and the walk detects substitution-produced key collisions by lookup, dropping
the collided path and descendants. Speculative probe attempts MUST be quiet
(no `error!`/`warn!` — the warnings-suppressed twin of the threshold/security
parse, mirroring the YAML quiet twin; the compiler is not logging-free, so
quietness is pinned by counted fixtures), and final failure returns the
ORIGINAL first-pass error with no replay. When the repair wins, the winning
repaired text is what the document's downstream consumers parse —
discovery's JSON template extractors deserialize the whole
`RouteDslRoutes` and MUST consume the repaired text, not the original
splice. Discovery's JSON arm MUST use this two-pass shape; a direct string
seam sibling of `parse_routes_with_env` MUST exist for JSON documents with
the same `RoutesEnvError` surface (unresolved variable vs parse error, by
type). `load_json_from_file` keeps its documented no-interpolation contract.

#### Scenario: JSON rest port from a placeholder default coerces

- **GIVEN** a JSON route document whose `rest` block carries
  `"port": "${env:RC_J_PORT:-8080}"` with the variable unset (first pass
  fails on the typed string, repair pass probes)
- **WHEN** the document loads through the JSON seam / discovery
- **THEN** the REST block carries `port == 8080`

#### Scenario: JSON rest port from a resolved environment value coerces

- **GIVEN** the same JSON document with `RC_J_PORT=9090` in the lookup
- **WHEN** the document loads
- **THEN** the REST block carries `port == 9090`

#### Scenario: JSON document numeric under routes coerces

- **GIVEN** a JSON route document with a numeric-typed step position under
  `routes` carrying `${env:RC_J_MAX:-2}` (e.g. `throttle.max_requests`)
- **WHEN** the document loads
- **THEN** the step carries the typed value `2`

#### Scenario: JSON escape handling stays byte-identical to the splice

- **GIVEN** a JSON document whose placeholder default contains an escape
  sequence (e.g. `"${env:RC_J_ESC:-a\nb}"` at a string position)
- **WHEN** the document loads
- **THEN** the first (splice) pass succeeds and the value is byte-identical
  to today's splice result — the tree-walk repair pass never runs, so escape
  decoding and error positions are unchanged

#### Scenario: JSON embedded token and non-numeric values keep the first-pass error

- **GIVEN** a JSON document with an embedded token at a typed position
  (`"port": "p-${env:RC_J_E:-8080}"`) and a document whose placeholder
  resolves to `notanumber`
- **WHEN** the documents load
- **THEN** both keep the first-pass error with today's error text — no
  candidate, no coercion, and the repair pass's speculative attempts emitted
  no warnings or errors

#### Scenario: JSON repair restricted to the legacy semantic baseline

- **GIVEN** three JSON documents: (a) an unescaped quote inside a placeholder
  default at a typed position (the splice substitution breaks JSON syntax),
  (b) a repairable `port` placeholder alongside an `a\nb` escape-bearing
  default (splice and tree walks parse to DIFFERENT values), and (c) a
  repairable `port` placeholder with an otherwise clean document
- **WHEN** each document loads
- **THEN** (a) keeps today's splice syntax error with no repair attempt, (b)
  keeps the first-pass error (baseline inequality — no partial repair that
  would silently change the escape-bearing default), and (c) loads with
  `port` coerced — repair changes only provenance-backed leaf types on the
  legacy baseline

#### Scenario: JSON speculative attempts are quiet with counted warnings

- **GIVEN** the named `json_repair_quiet_idempotent_warning` fixture: a
  `set_body` candidate compile-valid as both string and coerced value so the
  retry reaches the compile stage and passes through the idempotent_consumer
  warning path, followed by a trailing `unmarshal: no-such-format` step whose
  LATE compile failure keeps the document failing (zero-throttle is NOT used:
  it fails earlier at AST lowering, before the warning site, making the
  fixture vacuous)
- **WHEN** the document loads and the result is compared against a direct
  splice-parse of the same document, for BOTH the string and the numeric
  `set_body` candidate values
- **THEN** the legacy pass emits exactly the baseline count (one warning),
  every repair attempt adds exactly zero new `error!`/`warn!` events, final
  failure adds nothing and returns the original first-pass error — asserted
  by counts, not by absence-of-error heuristics

#### Scenario: JSON unresolved no-default placeholder names the variable

- **GIVEN** a JSON document carrying `"host": "${env:RC_J_MISSING}"` with no
  default and no lookup value
- **WHEN** the document loads
- **THEN** loading fails naming `RC_J_MISSING`, typed as the unresolved
  error (distinguishable from a parse error by type)

#### Scenario: JSON duplicate-key document is never repaired

- **GIVEN** a JSON document carrying a duplicate object key anywhere, plus a
  repairable `port` placeholder
- **WHEN** the document loads
- **THEN** the first-pass error stands with no repair attempt — duplicate-key
  documents cannot preserve authored overwrite order under key-sorted maps,
  so they are excluded from repair entirely

#### Scenario: JSON post-substitution key collision never lends provenance

- **GIVEN** a JSON object where a substituted key `${env:RC_K:-shared}`
  collides with an existing sibling key `shared`
- **WHEN** the document loads after a failed first pass elsewhere
- **THEN** the collided path and its descendants are never probe candidates —
  lookup-based collision detection drops them regardless of value equality

#### Scenario: JSON provenance order is deterministic under sorted keys

- **GIVEN** a document with two candidates whose key-sorted iteration order
  differs from their authored source order
- **WHEN** the document loads
- **THEN** the winning minimum-cardinality subset resolves deterministically
  in key-sorted order — order selects among equal-size winning subsets only,
  and the probe's minimum-cardinality guarantee is unchanged

### Requirement: `camel openapi generate` shares the route-document probe

`extract_rest_blocks_from_file_with_env` MUST run both arms (YAML and JSON)
through the same typed-repair seam instead of hard-failing on typed
placeholder positions: the YAML arm interpolates with provenance and probes
with a quiet extract oracle, replaying the logging extract exactly once on
final failure; the JSON arm keeps the legacy splice first pass and adds the
quiet-probe repair pass. A `rest:` block whose only defect is a string
placeholder at a typed position loads with the typed value coerced. The
probe runs only after a failed first parse; an unresolved no-default
placeholder keeps the named-variable error (fired before any probe); a
genuinely broken document keeps today's error text and its single
error-level log. The CLI regression pins that expect the old hard failure
MUST be flipped to coercion expectations asserting actual server URLs and
statuses; the CLI production lookup stays default-only (`&|_| None`).

#### Scenario: openapi generate accepts a rest port placeholder

- **GIVEN** a route file whose `rest:` block carries
  `port: ${env:PORT:-8080}` and an otherwise-valid operation
- **WHEN** `camel openapi generate` extracts the REST blocks through
  `extract_rest_blocks_from_file_with_env`
- **THEN** extraction succeeds and the block carries `port == 8080` — the
  generated document reflects the coerced server URL

#### Scenario: openapi generate keeps the named-variable and broken-document errors

- **GIVEN** a file with `host: ${env:OPENAPI_UNSET}` (no default, unset) and
  a second file with a genuinely broken `rest:` document whose placeholder
  resolves to `notanumber`
- **WHEN** `camel openapi generate` extracts each file
- **THEN** the first fails naming `OPENAPI_UNSET`; the second fails with the
  first-pass typed error and exactly one error-level log — the old
  hard-failure pin is replaced by these contracts

