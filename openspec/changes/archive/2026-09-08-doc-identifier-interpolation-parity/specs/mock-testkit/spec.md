## ADDED Requirements

### Requirement: Doc-side identifiers interpolate default-only for name-match parity with route sources

`camel test` document parsing SHALL resolve `${env:NAME:-default}`
placeholders in identifier fields of unit-tier `*.test.yaml` documents
using the same default-only scanner and grammar as route sources: no
ambient process-environment lookup ever runs, defaults resolve verbatim,
and the escape forms `$${env:...}` and `$$` yield literal text exactly as
in route files. Interpolation SHALL run immediately after deserialization
and BEFORE all other document validation, so every subsequent guard
(scheme checks, blank-name guards, built-in `memory` guard) evaluates the
RESOLVED value.

The identifier fields SHALL be exactly: (1) `repositories` map keys in
every registry map (`cache`, `idempotent`, `claimCheck`); (2) `beans` map
keys; (3) `intercepts` map keys (source URIs) and their action target
values (`skipTo`, `divertCopyTo`); (4) `mock:` references — `expects` map
keys and `sequence` entries; (5) `inputs[].to` values.

Assertion data SHALL NOT interpolate: input `body` and `headers` values,
`expectReply` blocks, expectation matcher contents, bean `methods` and
`config` values, repository stub targets, `settle`, and the route source
PATH fields (`routeFiles`, `routeFilesFromRoot`) all stay literal — the
identifier pass never touches them. Inline `routes` content is likewise
untouched by the identifier pass; it interpolates only through the
route-source loading layer, exactly like route-file content. Path and
glob fields (`routeFiles`, `routeFilesFromRoot`) SHALL never
interpolate — declarations like
`routeFiles: ["${env:ROUTE_DIR:-routes}/demo.yaml"]` reach route
resolution as the literal text, and the run fails to open that literal
path when it does not exist. The scenario (integration-tier) vocabulary
is out of scope for this requirement.

A `${env:NAME}` placeholder WITHOUT a default, in any identifier field,
SHALL fail document validation with exit code 2 and a message naming the
environment variable and the field position (mirroring the route-side
wording), never a resolved value. Two keys of one map that resolve to the
same string SHALL fail document validation naming the map and the
resolved value (no silent stub shadowing). The intercept key
verbatim-matching clause of the declarative test document parsing
requirement applies to the RESOLVED key: interpolation happens first, then
the resolved source URI matches verbatim (no trimming, query parameters
significant).

#### Scenario: repository stub key default matches the interpolated route reference (rc-4hexo)

- **GIVEN** a route file whose cache step declares
  `repository: "${env:CACHE_REPO_NAME:-persistent}"` and a test document
  declaring `repositories: { cache: { "${env:CACHE_REPO_NAME:-persistent}": memory } }`,
  with no `CACHE_REPO_NAME` variable resolved anywhere
- **WHEN** `camel test` runs the document
- **THEN** the route-side reference and the doc-side stub key both resolve
  to `persistent`, the stub registers, and the run passes — the
  "repository 'persistent' is not registered" error is gone

#### Scenario: bean key default matches

- **GIVEN** a route invoking a bean named via
  `${env:BEAN_NAME:-audit}` and a test document declaring
  `beans: { "${env:BEAN_NAME:-audit}": { kind: echo } }`
- **WHEN** `camel test` runs the document
- **THEN** both sides resolve to `audit` and the stub bean handles the
  invocation

#### Scenario: intercept source and target defaults match

- **GIVEN** a route sending to `direct:${env:TARGET:-archive}` and a test
  document declaring an intercept keyed
  `"direct:${env:TARGET:-archive}"` with
  `skipTo: "mock:${env:SINK:-skipped}"`
- **WHEN** `camel test` runs the document
- **THEN** the intercept applies to the resolved source URI
  `direct:archive` and the copy lands on `mock:skipped`

#### Scenario: mock references in expects and sequence resolve

- **GIVEN** a test document whose `expects` key and `sequence` entries are
  `mock:${env:EP:-result}` and whose single input declares
  `to: "direct:${env:IN:-start}"`
- **WHEN** `camel test` parses the document
- **THEN** the expectation registers for bare endpoint `result`, the
  sequence entry normalizes to `result`, and the input targets
  `direct:start`

#### Scenario: no-default identifier fails naming the variable at exit 2

- **GIVEN** a test document declaring
  `repositories: { cache: { "${env:NO_SUCH_VAR}": memory } }` with the
  variable unresolved
- **WHEN** `camel test` parses the document
- **THEN** parsing fails with exit code 2 and a message naming
  `NO_SUCH_VAR` and the field position, mirroring the route-side wording —
  the message names no resolved value

#### Scenario: assertion data stays literal (anti-widening witness)

- **GIVEN** a test document whose input body, input headers,
  `expects` matcher body, and bean `config` value each contain text like
  `${env:PAYLOAD:-leaked}`
- **WHEN** `camel test` parses the document
- **THEN** the parsed fields carry the placeholder text verbatim — no
  value position interpolates, and the run compares and delivers the
  literal text

#### Scenario: escaped placeholder key stays literal

- **GIVEN** a test document declaring
  `repositories: { cache: { "$${env:CACHE_REPO_NAME:-persistent}": memory } }`
  and a route file whose repository reference is the same escaped form
- **WHEN** `camel test` parses the document and loads the route
- **THEN** both sides keep the literal name
  `${env:CACHE_REPO_NAME:-persistent}` (one `$` stripped by the shared
  escape grammar) and the stub registers under that literal name

#### Scenario: route file path never interpolates

- **GIVEN** a test document declaring
  `routeFiles: ["${env:ROUTE_DIR:-routes}/demo.yaml"]` where no such
  literal path exists
- **WHEN** `camel test` runs the document
- **THEN** route resolution receives the literal entry text unchanged and
  the run fails to open it — the path is never substituted

#### Scenario: empty default resolving to a blank name is a document error

- **GIVEN** a test document declaring
  `repositories: { cache: { "${env:NAME:-}": memory } }`
- **WHEN** `camel test` parses the document
- **THEN** the resolved key is empty and parsing fails with exit code 2
  stating repository names must be non-blank — the blank-name guard sees
  the RESOLVED value

#### Scenario: default resolving to the built-in memory name is rejected

- **GIVEN** a test document declaring
  `repositories: { cache: { "${env:NAME:-memory}": memory } }`
- **WHEN** `camel test` parses the document
- **THEN** parsing fails with exit code 2 stating `memory` is a built-in
  repository name and cannot be stubbed — the built-in guard sees the
  RESOLVED value

#### Scenario: two keys resolving to the same name collide as a document error

- **GIVEN** a test document declaring
  `repositories: { cache: { "${env:A:-x}": memory, "x": memory } }`
- **WHEN** `camel test` parses the document
- **THEN** parsing fails with exit code 2 naming the map
  (`repositories.cache`) and the resolved value `x` — no silent stub
  shadowing
