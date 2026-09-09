# mock-testkit delta — env-driven-knobs

## RENAMED Requirements

- FROM: `### Requirement: LEAN tier resolves env placeholders in route sources via default-only lookup with boot parity`
- TO: `### Requirement: LEAN tier resolves env placeholders in route sources via the document env layer with boot parity`

- FROM: `### Requirement: Doc-side identifiers interpolate default-only for name-match parity with route sources`
- TO: `### Requirement: Doc-side identifiers interpolate through the document env layer for name-match parity with route sources`

## ADDED Requirements

### Requirement: Unit-tier document env layer

A unit-tier `*.test.yaml` document MAY declare an optional `env:` map of fixture
values. The map MUST contain string keys and string values; a non-string value
(YAML number, boolean, or null) MUST fail document validation with an error naming
the `env` field. When the map is present, every unit-tier interpolation seam —
route-file loading (`routeFiles`, `routeFilesFromRoot`), inline `routes:`
interpolation, and doc-side identifier interpolation — MUST consult the map BEFORE
any inline `${env:NAME:-default}` default. The layer MUST be strictly
document-sourced: no ambient process-environment read, no passthrough allowlist, no
harness provisioning. Values in the `env:` map MUST NOT themselves be interpolated
(no recursive resolution); a value containing `${env:...}` text substitutes that
text verbatim. The `env:` map MUST NOT participate in tier derivation: a document
declaring `env:` and no `scenario:` vocabulary stays unit tier and runs LEAN.
Scenario-tier documents keep their own layered environment (ADR-0069 §4) and are
out of scope.

#### Scenario: non-string env value is a document error

- **GIVEN** a `.test.yaml` declaring `env: { CB_MS: 500 }` (YAML integer value)
- **WHEN** `camel test` parses the document
- **THEN** parsing fails with a document error naming the `env` field — fixture
  values are strings, and numeric knobs are not served by this layer (rc-v1sw
  track)

#### Scenario: env values are never themselves interpolated

- **GIVEN** a `.test.yaml` declaring `env: { A: "${env:B:-x}literal" }` and a route
  field `${env:A:-d}`
- **WHEN** the document's routes load
- **THEN** the field carries the verbatim text `${env:B:-x}literal` — the fixture
  value is data, not a template

#### Scenario: absent env map keeps default-only behavior

- **GIVEN** a `.test.yaml` without an `env:` map and a route with
  `title: ${env:LEAN_T:-hello}`
- **WHEN** the document's routes load
- **THEN** the field is `"hello"` — documents predating the layer behave exactly
  as before

#### Scenario: ambient variable without doc entry or default stays unresolved

- **GIVEN** a variable set in the ambient process environment, absent from the
  document `env:` map, referenced as `${env:AMBIENT_ONLY}` in a route value
  position
- **WHEN** the document's routes load
- **THEN** loading fails naming `AMBIENT_ONLY` — the ambient environment is never
  a resolution source in the unit tier

#### Scenario: env map does not change tier derivation

- **GIVEN** a `.test.yaml` declaring `env: { NAME: value }`, a route source, and
  expectations — no `scenario:` vocabulary
- **WHEN** `camel test` derives the document's tier
- **THEN** the document stays unit tier and runs through the LEAN runner

## MODIFIED Requirements

### Requirement: LEAN tier resolves env placeholders in route sources via the document env layer with boot parity

The unit-tier runner (`camel test --unit`) MUST load route sources — `routeFiles`,
`routeFilesFromRoot`, and inline `routes:` — after interpolating `${env:NAME:-default}`
placeholders with a lookup that consults the document `env:` map first, then the
inline default, and that MUST NOT read the ambient environment (ADR-0069 §13.1
global-state flake class), using the SAME interpolation strategy and typing
semantics as the route loader (`camel run` boot parity): string-typed fields
interpolate; integer-typed fields carrying a placeholder fail to load — including
when the document `env:` map supplies the value (substituted leaves keep string
typing; numeric knobs are the rc-v1sw track). Unresolved no-default placeholders
in value positions — not in the document `env:` map either — MUST fail the document
with an error naming the variable, mirroring the boot path's `DiscoveryError::Env`
wording.

#### Scenario: file route with string-field placeholder loads under LEAN

- **Given** a `.test.yaml` whose `routeFiles` reference a route with a string-typed
  field `title: ${env:LEAN_T:-hello}`
- **When** the document's routes load
- **Then** the route loads with `title == "hello"` and no document error occurs

#### Scenario: document env value steers a route-file string field before the default

- **Given** a `.test.yaml` declaring `env: { LEAN_T: docval }` whose `routeFiles`
  reference a route with `title: ${env:LEAN_T:-hello}`
- **When** the document's routes load
- **Then** the route loads with `title == "docval"` — the document layer wins over
  the inline default

#### Scenario: inline routes with string-field placeholder load under LEAN

- **Given** a `.test.yaml` with inline `routes:` containing a string-typed field with
  `${env:P:-one}` and no `env:` map
- **When** the document's routes load
- **Then** the route loads with the field `"one"` and no document error occurs

#### Scenario: document env value steers an inline-routes string field

- **Given** a `.test.yaml` declaring `env: { P: two }` with inline `routes:`
  containing a string-typed field `${env:P:-one}`
- **When** the document's routes load
- **Then** the route loads with the value `"two"` and no document error occurs

#### Scenario: no-default placeholder resolves from the document env map

- **Given** a `.test.yaml` declaring `env: { LEAN_DEF: supplied }` whose route file
  contains `title: ${env:LEAN_DEF}` in a value position
- **When** the document's routes load
- **Then** the route loads with `title == "supplied"` — no unresolved-variable
  error

#### Scenario: file route with integer-field placeholder fails with doc_error (boot parity)

- **Given** a `.test.yaml` whose `routeFiles` reference a route with
  `circuit_breaker.open_duration_ms: ${env:CB_MS:-750}`
- **When** the document's routes load
- **Then** loading fails with a document error — the substituted leaf keeps string
  typing exactly as `camel run` would reject the same file; LEAN never accepts a
  route the boot path rejects

#### Scenario: integer-field placeholder fails even when the document env supplies the value

- **Given** a `.test.yaml` declaring `env: { CB_MS: "500" }` whose `routeFiles`
  reference a route with `circuit_breaker.open_duration_ms: ${env:CB_MS:-750}`
- **When** the document's routes load
- **Then** loading fails with a document error — the substituted leaf keeps string
  typing regardless of the value's source; the unit tier never resolves an
  int-typed position the boot path rejects (numeric knobs: rc-v1sw track)

#### Scenario: unresolved no-default placeholder fails the document naming the variable

- **Given** a route file referenced by a `.test.yaml` containing
  `title: ${env:LEAN_UNDEF}` in a value position, with no `LEAN_UNDEF` entry in
  the document `env:` map
- **When** the document's routes load
- **Then** the result is an error string containing `LEAN_UNDEF` (boot-parity
  wording), not a serde type error

#### Scenario: LEAN ignores ambient environment

- **Given** ambient `LEAN_T=ambient` set in the process environment and a route
  with `title: ${env:LEAN_T:-hello}`, with no `env:` map in the document
- **When** the document's routes load with ambient values present
- **Then** the field is still `"hello"` (document env absent, default applies;
  ambient never consulted; hermetic determinism)

### Requirement: Doc-side identifiers interpolate through the document env layer for name-match parity with route sources

`camel test` document parsing SHALL resolve `${env:NAME:-default}`
placeholders in identifier fields of unit-tier `*.test.yaml` documents using the
same scanner and grammar as route sources, with a lookup that consults the
document `env:` map first, then the inline default: no ambient process-environment
lookup ever runs, defaults resolve verbatim, and the escape forms `$${env:...}`
and `$$` yield literal text exactly as in route files. Interpolation SHALL run
immediately after deserialization and BEFORE all other document validation, so
every subsequent guard (scheme checks, blank-name guards, built-in `memory`
guard) evaluates the RESOLVED value.

The identifier fields SHALL be exactly: (1) `repositories` map keys in every
registry map (`cache`, `idempotent`, `claimCheck`); (2) `beans` map keys;
(3) `intercepts` map keys (source URIs) and their action target values (`skipTo`,
`divertCopyTo`); (4) `mock:` references — `expects` map keys and `sequence`
entries; (5) `inputs[].to` values.

Assertion data SHALL NOT interpolate: input `body` and `headers` values,
`expectReply` blocks, expectation matcher contents, bean `methods` and `config`
values, repository stub targets, `settle`, and the route source PATH fields
(`routeFiles`, `routeFilesFromRoot`) all stay literal — the identifier pass never
touches them. Inline `routes` content is likewise untouched by the identifier
pass; it interpolates only through the route-source loading layer, exactly like
route-file content. Path and glob fields (`routeFiles`, `routeFilesFromRoot`)
SHALL never interpolate — declarations like
`routeFiles: ["${env:ROUTE_DIR:-routes}/demo.yaml"]` reach route resolution as
the literal text, and the run fails to open that literal path when it does not
exist. The scenario (integration-tier) vocabulary is out of scope for this
requirement.

A `${env:NAME}` placeholder WITHOUT a default, in any identifier field, not
present in the document `env:` map, SHALL fail document validation with exit code
2 and a message naming the environment variable and the field position (mirroring
the route-side wording), never a resolved value. Two keys of one map that resolve
to the same string SHALL fail document validation naming the map and the resolved
value (no silent stub shadowing). The intercept key verbatim-matching clause of
the declarative test document parsing requirement applies to the RESOLVED key:
interpolation happens first, then the resolved source URI matches verbatim (no
trimming, query parameters significant).

#### Scenario: repository stub key default matches the interpolated route reference (rc-4hexo)

- **GIVEN** a route file whose cache step declares
  `repository: "${env:CACHE_REPO_NAME:-persistent}"` and a test document
  declaring `repositories: { cache: { "${env:CACHE_REPO_NAME:-persistent}": memory } }`,
  with no `CACHE_REPO_NAME` variable resolved anywhere
- **WHEN** `camel test` runs the document
- **THEN** the route-side reference and the doc-side stub key both resolve to
  `persistent`, the stub registers, and the run passes — the
  "repository 'persistent' is not registered" error is gone

#### Scenario: document env steers both the route reference and the stub key (rc-l7m7t)

- **GIVEN** a route file whose cache step declares
  `repository: "${env:CACHE_REPO_NAME:-persistent}"`, a test document declaring
  `env: { CACHE_REPO_NAME: faststub }` and
  `repositories: { cache: { "${env:CACHE_REPO_NAME:-persistent}": memory } }`
- **WHEN** `camel test` runs the document
- **THEN** the route-side reference and the doc-side stub key both resolve to
  `faststub`, the stub registers under that name, and the run passes — steering
  through one shared layer preserves name-match parity by construction

#### Scenario: bean key default matches

- **GIVEN** a route invoking a bean named via `${env:BEAN_NAME:-audit}` and a
  test document declaring `beans: { "${env:BEAN_NAME:-audit}": { kind: echo } }`
- **WHEN** `camel test` runs the document
- **THEN** both sides resolve to `audit` and the stub bean handles the invocation

#### Scenario: intercept source and target defaults match

- **GIVEN** a route sending to `direct:${env:TARGET:-archive}` and a test
  document declaring an intercept keyed `"direct:${env:TARGET:-archive}"` with
  `skipTo: "mock:${env:SINK:-skipped}"`
- **WHEN** `camel test` runs the document
- **THEN** the intercept applies to the resolved source URI `direct:archive` and
  the copy lands on `mock:skipped`

#### Scenario: mock references in expects and sequence resolve

- **GIVEN** a test document whose `expects` key and `sequence` entries are
  `mock:${env:EP:-result}` and whose single input declares
  `to: "direct:${env:IN:-start}"`
- **WHEN** `camel test` parses the document
- **THEN** the expectation registers for bare endpoint `result`, the sequence
  entry normalizes to `result`, and the input targets `direct:start`

#### Scenario: no-default identifier fails naming the variable at exit 2

- **GIVEN** a test document declaring
  `repositories: { cache: { "${env:NO_SUCH_VAR}": memory } }` with the variable
  unresolved and absent from the document `env:` map
- **WHEN** `camel test` parses the document
- **THEN** parsing fails with exit code 2 and a message naming `NO_SUCH_VAR` and
  the field position, mirroring the route-side wording — the message names no
  resolved value

#### Scenario: no-default identifier resolves from the document env map

- **GIVEN** a test document declaring `env: { BEAN_NAME: audit }` and
  `beans: { "${env:BEAN_NAME}": { kind: echo } }`
- **WHEN** `camel test` parses the document
- **THEN** the bean key resolves to `audit` and the stub registers — no
  unresolved-variable error

#### Scenario: assertion data stays literal (anti-widening witness)

- **GIVEN** a test document whose input body, input headers, `expects` matcher
  body, and bean `config` value each contain text like `${env:PAYLOAD:-leaked}`
- **WHEN** `camel test` parses the document
- **THEN** the parsed fields carry the placeholder text verbatim — no value
  position interpolates, and the run compares and delivers the literal text

#### Scenario: escaped placeholder key stays literal

- **GIVEN** a test document declaring
  `repositories: { cache: { "$${env:CACHE_REPO_NAME:-persistent}": memory } }`
  and a route file whose repository reference is the same escaped form
- **WHEN** `camel test` parses the document and loads the route
- **THEN** both sides keep the literal name
  `${env:CACHE_REPO_NAME:-persistent}` (one `$` stripped by the shared escape
  grammar) and the stub registers under that literal name

#### Scenario: route file path never interpolates

- **GIVEN** a test document declaring
  `routeFiles: ["${env:ROUTE_DIR:-routes}/demo.yaml"]` where no such literal path
  exists
- **WHEN** `camel test` runs the document
- **THEN** route resolution receives the literal entry text unchanged and the run
  fails to open it — the path is never substituted

#### Scenario: empty default resolving to a blank name is a document error

- **GIVEN** a test document declaring
  `repositories: { cache: { "${env:NAME:-}": memory } }`
- **WHEN** `camel test` parses the document
- **THEN** the resolved key is empty and parsing fails with exit code 2 stating
  repository names must be non-blank — the blank-name guard sees the RESOLVED
  value

#### Scenario: default resolving to the built-in memory name is rejected

- **GIVEN** a test document declaring
  `repositories: { cache: { "${env:NAME:-memory}": memory } }`
- **WHEN** `camel test` parses the document
- **THEN** parsing fails with exit code 2 stating `memory` is a built-in
  repository name and cannot be stubbed — the built-in guard sees the RESOLVED
  value

#### Scenario: two keys resolving to the same name collide as a document error

- **GIVEN** a test document declaring
  `repositories: { cache: { "${env:A:-x}": memory, "x": memory } }`
- **WHEN** `camel test` parses the document
- **THEN** parsing fails with exit code 2 naming the map (`repositories.cache`)
  and the resolved value `x` — no silent stub shadowing
