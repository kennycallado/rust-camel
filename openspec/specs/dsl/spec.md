# dsl Specification

## Purpose
TBD - created by archiving change remove-header-dsl-verb. Update Purpose after archive.
## Requirements
### Requirement: remove_header declarative step

The DSL SHALL provide a `remove_header` declarative step that removes a single
named header from the exchange input message headers.

#### Scenario: remove an existing input header

- **GIVEN** a route containing `- remove_header: { key: CamelHttpPath }` and
  an exchange whose input message has header `CamelHttpPath` set
- **WHEN** the step executes
- **THEN** the header `CamelHttpPath` is absent from `exchange.input.headers`
  and all other input headers are preserved

#### Scenario: remove a non-existent key is a no-op

- **GIVEN** a route containing `- remove_header: { key: X-Not-Present }` and
  an exchange whose input message does NOT have header `X-Not-Present`
- **WHEN** the step executes
- **THEN** the exchange passes through unchanged and the step completes
  successfully (no error)

#### Scenario: input-only removal preserves output header

- **GIVEN** a route containing `- remove_header: { key: X-Shared }` and an
  InOut exchange where BOTH the input message and the output message carry
  header `X-Shared`
- **WHEN** the step executes
- **THEN** header `X-Shared` is removed from `exchange.input.headers` but is
  PRESERVED on `exchange.output.headers` (removal is input-only, matching
  SetHeader semantics)

#### Scenario: empty or whitespace-only key is rejected at compile time

- **GIVEN** a route containing `- remove_header: { key: "   " }` (whitespace
  only, treated as empty by `trim().is_empty()`)
- **WHEN** the route is compiled
- **THEN** compilation fails with an error message mentioning `remove_header`
  and the empty-key constraint (same guard as `set_header`)

### Requirement: remove_header in the route JSON schema

The route JSON schema (`route-schema.json`) SHALL include `remove_header` as a
valid step so that `camel-lint` accepts routes that use it.

#### Scenario: lint accepts a route with remove_header

- **GIVEN** the canonical route JSON schema and a route YAML containing a
  `remove_header` step with a non-empty `key`
- **WHEN** the route is validated against the schema
- **THEN** validation passes (the step is recognized as a permitted step kind)

### Requirement: CircuitBreaker route-level fallback sub-pipeline

The DSL SHALL accept an optional `fallback:` sub-pipeline (a list of steps) on the
route-level `circuit_breaker` configuration. When the circuit is open, the runtime SHALL
execute the fallback sub-pipeline instead of failing with `CircuitOpen`. Absent or empty
`fallback` SHALL produce identical behavior to today (no fallback processor configured).
The fallback sub-pipeline SHALL be part of the canonical route contract: it serializes
as a list of canonical steps and survives the canonical roundtrip.

#### Scenario: fallback declared in YAML parses and compiles

- **GIVEN** a YAML route with `circuit_breaker: { failure_threshold: 1, open_duration_ms: 60000, fallback: [ cache_peek_stale: { repository: persistent, key: "tile-xyz" } ] }`
- **WHEN** the route is parsed and compiled
- **THEN** parsing succeeds, the compiled route's circuit breaker runs the fallback
  sub-pipeline when the circuit is open, and the route schema
  (`route-schema.json`, both copies) validates the shape

#### Scenario: absent or empty fallback leaves behavior unchanged

- **GIVEN** a YAML route with `circuit_breaker: { failure_threshold: 1, open_duration_ms: 60000 }` (no `fallback` key)
- **WHEN** the route is compiled
- **THEN** `CircuitBreakerConfig.fallback` is `None` and route behavior is
  indistinguishable from a route compiled before this requirement existed

#### Scenario: open circuit executes the fallback sub-pipeline

- **GIVEN** a route whose circuit breaker has a `fallback: [ cache_peek_stale: { repository: persistent, key: "tile-xyz" } ]` and whose circuit is open
- **WHEN** an exchange arrives
- **THEN** the fallback sub-pipeline runs and the exchange body is produced by it
  (for the peek: the post-expiry cached value), instead of `CircuitOpen` propagating

#### Scenario: stopped fallback yields a clean outcome

- **GIVEN** a route whose circuit is open and whose fallback sub-pipeline stops (e.g.
  `cache_peek_stale` MISS with the default `on_miss: stop` policy)
- **WHEN** the fallback sub-pipeline signals stop
- **THEN** the circuit breaker fallback path surfaces `Ok(exchange)` with the Exchange
  state intact — no `CircuitOpen` and no error escapes, because the composed fallback
  pipeline already translates Stop to `Ok` at its `into_tower_result` boundary
  (ADR-0024/0025 single-translation-site)

#### Scenario: canonical roundtrip preserves fallback steps

- **GIVEN** a canonical route spec with `circuit_breaker.fallback` containing canonical steps
- **WHEN** the spec is serialized and deserialized back
- **THEN** the fallback step list is preserved, and routes serialized before this
  requirement (no `fallback` key) deserialize with an empty fallback

#### Scenario: unknown fields remain rejected

- **GIVEN** a YAML route with `circuit_breaker: { failure_threshold: 1, unknown_key: 1 }`
- **WHEN** the route is parsed
- **THEN** parsing fails with an unknown-field error (`deny_unknown_fields` intact)

#### Scenario: invalid nested fallback steps are rejected

- **GIVEN** a canonical route spec whose `circuit_breaker.fallback` contains an invalid
  step (e.g. an empty body where one is required)
- **WHEN** the canonical contract is validated
- **THEN** validation fails naming the fallback step (the same validations that apply to
  top-level steps recurse into `circuit_breaker.fallback`)

#### Scenario: builder reverse path fails closed on opaque fallback

- **GIVEN** a programmatic `CircuitBreakerConfig` with a compiled fallback processor
  (`BoxProcessor`)
- **WHEN** camel-builder attempts to reconstruct a canonical route spec from it
- **THEN** the reconstruction fails with an explicit error naming the opaque fallback
  (no silent loss, ADR-0016); DSL and canonical authored routes are unaffected

### Requirement: parameters map on endpoint surfaces

The DSL SHALL accept an optional `parameters` map (deserialized as `BTreeMap<String,String>`, defaulting to empty) alongside the URI on every endpoint-bearing surface — `from`, `to`, `wire_tap`, `enrich`, and `poll_enrich` — in the authoring AST:

- AST (`crates/camel-dsl/src/route_ast.rs`): `ToStep`, `WireTapStep`, `EnrichStep`, and `PollEnrichStep` (the latter two via the shared `EnrichBody` shorthand and `EnrichConfig` full form) gain a `parameters` field alongside their raw `uri`; the route-level `from` surface gains the same. The AST holds the RAW pair (uri string + parameters map), unmerged.
- Model (`crates/camel-dsl/src/model.rs`): `DeclarativeRoute.from`, `ToStepDef`, `WireTapStepDef`, and `EnrichStepDef` (used by both `enrich` and `poll_enrich` steps) receive the canonical merged URI — the AST→model lowering in `yaml.rs` (shared by the YAML and JSON authoring paths) collapses the raw pair via `EndpointUri::to_canonical_string()` into the existing `uri: String`, and `compile.rs` consumes canonical model URIs only.

Values SHALL be strings: YAML/JSON scalar coercion (e.g. `password: 0123` parsed as int, `enabled: on` parsed as bool) SHALL be a deserialization error naming the offending key, not a silent coercion. The endpoint AST structs (`ToStep`, `WireTapStep`, `EnrichStep`, `PollEnrichStep`, `EnrichConfig`) SHALL be `#[non_exhaustive]`.

Concrete shapes (YAML; JSON is the mechanical translation):

```yaml
# from
- from: timer:tick
  parameters:
    period: "1000"
# to
- to: kafka:orders
  parameters:
    brokers: my-host:9092
# wire_tap
- wire_tap: log:audit
  parameters:
    showBody: "true"
# enrich (shorthand uri form and full form)
- enrich: db:query
  parameters:
    dataSource: customers
- enrich:
    uri: db:query
    parameters:
      dataSource: customers
    strategy: ...
# poll_enrich (shares the EnrichBody shapes)
- poll_enrich: file:inbox
  parameters:
    delay: "500"
```

#### Scenario: to step with parameters compiles to the canonical merged URI

- **GIVEN** a route with `to: kafka:orders` and `parameters: {brokers: my-host:9092}`
- **WHEN** the route compiles
- **THEN** the resulting step URI is `kafka:orders?brokers=my-host:9092`

#### Scenario: from with parameters merges identically

- **GIVEN** a route with `from: timer:tick` and `parameters: {period: "1000"}`
- **WHEN** the route compiles
- **THEN** the resulting `from_uri` equals `timer:tick?period=1000`

#### Scenario: Equivalent query-string and parameters forms produce identical output

- **GIVEN** one route using `to: log:out?showBody=true` and another using `to: log:out` with `parameters: {showBody: "true"}`
- **WHEN** both compile
- **THEN** the resulting step URIs are byte-identical

#### Scenario: Non-string parameter values are rejected

- **GIVEN** `parameters: {retries: 3}` (YAML integer)
- **WHEN** the route deserializes
- **THEN** deserialization fails with an error naming the offending key `retries`

#### Scenario: Duplicate key between query string and parameters is a compile error

- **GIVEN** `to: kafka:orders?brokers=a` with `parameters: {brokers: b}`
- **WHEN** the route compiles
- **THEN** compilation fails with an error naming `brokers` and both sources; no output is produced

#### Scenario: wire_tap with parameters compiles to the canonical merged URI

- **GIVEN** a route with `wire_tap: log:audit` and `parameters: {showBody: "true"}`
- **WHEN** the route compiles
- **THEN** the resulting wire-tap step URI is `log:audit?showBody=true`

#### Scenario: enrich with parameters compiles to the canonical merged URI, in both forms

- **GIVEN** one route using shorthand `enrich: db:query` + `parameters: {dataSource: customers}` and another using the full form `enrich: {uri: db:query, parameters: {dataSource: customers}}`
- **WHEN** both compile
- **THEN** each resulting enrich step URI is `db:query?dataSource=customers`, and the two outputs are byte-identical

#### Scenario: poll_enrich with parameters compiles to the canonical merged URI

- **GIVEN** a route with `poll_enrich: file:inbox` and `parameters: {delay: "500"}`
- **WHEN** the route compiles
- **THEN** the resulting poll_enrich step URI is `file:inbox?delay=500`

#### Scenario: Duplicate key between full-form and step-level parameters fails closed

- **GIVEN** an `enrich` full form `{uri: db:query, parameters: {dataSource: customers}}` with step-level `parameters: {dataSource: other}` declaring the same key in both maps (the same both-set shape on `poll_enrich`), and the same full form with disjoint step-level `parameters: {timeout: "5000"}`
- **WHEN** the route compiles
- **THEN** the overlapping key makes compilation fail with the duplicate-key error naming `dataSource` (no output), and the disjoint case produces the step URI `db:query?dataSource=customers&timeout=5000` containing parameters from both maps

### Requirement: RouteBuilder parameters parity

The `camel-builder` RouteBuilder surface SHALL offer an optional `.parameters(BTreeMap<String,String>)` that records a pending parameter map attaching to the most recent endpoint slot: the `from` endpoint when called before any step, and the `to`/`wire_tap`/`enrich`/`poll_enrich` step when called immediately after one. The method SHALL follow the builder's consuming-`self` signature style; misuse SHALL NOT panic — a pending map with no endpoint slot to attach to, or a duplicate-key conflict against the slot's URI query, SHALL surface as an error at `build()` through the established builder error policy.

#### Scenario: RouteBuilder parameters on to merge identically to the DSL

- **GIVEN** `RouteBuilder::from("timer:tick")` followed by `.to("log:out")` and `.parameters({showBody: "true"}.into())`
- **WHEN** `build()` succeeds
- **THEN** the resulting step URI equals `log:out?showBody=true`

#### Scenario: RouteBuilder parameters on from merge identically

- **GIVEN** `RouteBuilder::from("timer:tick")` followed immediately by `.parameters({period: "1000"}.into())` and then any step
- **WHEN** `build()` succeeds
- **THEN** the resulting `from_uri` equals `timer:tick?period=1000`

#### Scenario: RouteBuilder parameters on wire_tap, enrich, and poll_enrich

- **GIVEN** builders with `.wire_tap("log:audit")` + `.parameters({showBody: "true"})`, `.enrich("db:query")` + `.parameters({dataSource: customers})`, and `.poll_enrich("file:inbox", 1000)` + `.parameters({delay: "500"})`
- **WHEN** `build()` succeeds
- **THEN** the step URIs equal `log:audit?showBody=true`, `db:query?dataSource=customers`, and `file:inbox?delay=500` respectively

#### Scenario: RouteBuilder parameters with no pending endpoint fails at build

- **GIVEN** `.parameters(...)` called when the only prior call is another `.parameters(...)` or a non-endpoint step such as `.log(...)`
- **WHEN** `build()` is called
- **THEN** `build()` returns an error through the builder error policy (no panic)

#### Scenario: RouteBuilder duplicate key fails closed at build

- **GIVEN** a `.to("kafka:orders?brokers=a")` step with `.parameters` containing `brokers`
- **WHEN** `build()` is called
- **THEN** `build()` returns the same duplicate-key error class the declarative DSL produces

### Requirement: Reserved test suffix in route discovery

Route discovery SHALL treat file names ending in `.test.yaml` or `.test.yml`
as camel test documents, never as route documents. When a wildcard pattern
(default glob, `Camel.toml` `routes` entry, or `--routes` value) matches a
test-suffixed file, discovery SHALL skip the file with no error. When an
explicit pattern with no glob metacharacters names a test-suffixed file,
discovery SHALL fail with a `ReservedTestSuffix` error whose message names
the file and states the `camel test` command as the correct action. The
suffix predicate SHALL live in `camel-dsl` and SHALL be the single source of
truth consumed by the CLI (run, watch) and lint.

#### Scenario: wildcard glob skips colocated test document

- **GIVEN** a directory containing `routes/demo.yaml` and `routes/demo.test.yaml`
- **WHEN** discovery runs with pattern `routes/*.yaml`
- **THEN** `demo.yaml` loads as a route and `demo.test.yaml` is skipped with no error

#### Scenario: explicit Camel.toml routes entry skips test document

- **GIVEN** `Camel.toml` with `routes = ["routes/*.yaml"]` and a colocated `routes/demo.test.yaml`
- **WHEN** `camel run` starts
- **THEN** discovery skips the test document and startup succeeds

#### Scenario: explicit no-wildcard naming errors

- **GIVEN** an invocation `camel run --routes routes/demo.test.yaml`
- **WHEN** discovery runs
- **THEN** discovery fails with a `ReservedTestSuffix` error naming `demo.test.yaml` and instructing the user to run `camel test` instead

#### Scenario: test-json names stay governed by JSON gating

- **GIVEN** a file named `routes/x.test.json` matched by `routes/*.json`
- **WHEN** discovery runs
- **THEN** the file is not treated as test-suffixed (test documents are YAML-only) and the existing JSON pattern gating applies unchanged

### Requirement: Non-printable character rejection parity

The DSL front-ends SHALL keep format-correct behavior for route documents
whose raw text contains characters outside the YAML printable set: the JSON
front-end SHALL accept such a document when it is JSON-valid, and the YAML
front-end SHALL reject it with a format-annotated error.

#### Scenario: raw DEL byte in a JSON-valid document

- Given a routes document that serde_json accepts and whose raw text contains
  U+007F
- When the document is parsed through the JSON front-end
- Then parsing succeeds
- When the document is parsed through the YAML front-end
- Then parsing fails with an error prefixed `YAML DSL error:`

#### Scenario: escaped DEL stays strict-parity

- Given a routes document that carries DEL only in escaped form (the six ASCII
  bytes `\u007f`) and contains no raw non-printable character
- When the document is parsed through both front-ends
- Then both front-ends succeed and produce equal route steps

### Requirement: dsl_parity oracle carve-out for the non-printable class

The dsl_parity fuzz oracle SHALL treat a YAML rejection of a JSON-valid
document as expected behavior when the raw document contains characters
outside the YAML printable set, and SHALL panic on every other YAML rejection.

#### Scenario: minimized assurance input does not panic

- Given the minimized document from assurance run 33984285881 containing a raw
  U+007F inside a JSON string
- When the dsl_parity harness consumes it
- Then the harness asserts the YAML front-end rejection and returns without
  panic

#### Scenario: printable rejection still panics

- Given a document that serde_json accepts, whose raw text is fully printable,
  and that the YAML front-end rejects
- When the dsl_parity harness consumes it
- Then the harness panics with `parity divergence: yaml rejects json-valid
  document`

### Requirement: Env-lookup-injected route discovery

Route discovery SHALL resolve `${env:}` placeholders through the injected environment lookup, applying interpolation to parsed YAML string keys and string leaves only, and only where the scalar text contains a placeholder or escape token. Interpolated leaves SHALL resolve as string scalars (numeric-looking results keep string typing — the camel-config leaf-interpolation precedent). For documents the tree walk processes, comment content SHALL never be interpolated and SHALL never fail resolution. The escape grammar (`$${env:X}`, `$$`) SHALL keep raw-splice semantics inside each string. If the raw text cannot be parsed as YAML by the same parser discovery hands off to, or the parsed tree contains tagged nodes, discovery SHALL fall back to whole-text raw interpolation with legacy semantics (including legacy comment sensitivity for those documents).

#### Scenario: numeric-looking interpolation result stays a string

- **GIVEN** a route value `port: ${env:PORT}` with `PORT` resolving to `8080`
- **WHEN** discovery interpolates the document
- **THEN** the leaf parses back as the string `"8080"`, not a number

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

#### Scenario: string-typed field with default interpolates

- **Given** a route file containing a string-typed field `title: ${env:RC_T:-hello}`
- **When** `load_from_file` parses the file
- **Then** loading succeeds and the field equals `"hello"`

#### Scenario: integer-typed field with default fails with type mismatch (boot parity)

- **Given** a route file containing `throttle.max_requests: ${env:MY_LIMIT:-2}`
- **When** `load_from_file` parses the file
- **Then** loading fails — the substituted leaf keeps string typing, so the route does
  not deserialize, matching discovery/`camel run` behavior exactly

#### Scenario: unset variable without default errors naming the variable

- **Given** a route file containing `title: ${env:UNSET_NO_DEF}` in a value position
  with no ambient value
- **When** `load_from_file` parses the file
- **Then** loading fails with an error naming `UNSET_NO_DEF` (mirroring
  `DiscoveryError::Env` wording), not a serde "did not match" error

#### Scenario: ambient environment is ignored by the default path

- **Given** a route file containing `title: ${env:AMBIENT_VAR:-hello}` and ambient
  `AMBIENT_VAR=goodbye`
- **When** `load_from_file` (no explicit lookup) parses the file
- **Then** the field resolves to `"hello"` — the default-only path never consults
  ambient values

#### Scenario: explicit lookup is the sole injection point for ambient values

- **Given** a route file containing `title: ${env:AMBIENT_VAR:-hello}`
- **When** `load_from_file_with_env` parses the file with an injected lookup returning
  `Some("goodbye")`
- **Then** the field resolves to `"goodbye"` — ambient values enter only through the
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
  no placeholder-env wording appears for an otherwise-resolvable document

### Requirement: on_exceptions wildcard clause

A declarative `error_handler.on_exceptions` clause with `kind: "*"` SHALL
match every `CamelError` variant. The wildcard SHALL combine with
`message_contains` by conjunction (both must hold). Clause evaluation SHALL
keep first-match-wins order, so a wildcard clause SHALL only receive errors
that no earlier clause matched. All clause features (`handled`, `continued`,
`retry.handled_by`, `steps`) SHALL behave with the wildcard exactly as with a
specific kind.

#### Scenario: wildcard matches every error kind

- **Given** a route with `error_handler.on_exceptions: [{kind: "*", handled:
  true, retry: {handled_by: "direct:shaper"}}]`
- **When** a route step fails with `ValidationError("schema mismatch")`, and
  a second request's step fails with `ProcessorError("boom")`
- **Then** in both cases the `direct:shaper` handler route runs and its
  output exchange is the final result (error cleared, pipeline
  `Completed`)

#### Scenario: wildcard with handled true owns the full HTTP response

- **Given** an HTTP consumer route with the wildcard clause above and a
  handler route that sets body to `"shaped"`, sets the string-valued header
  `X-Custom: "yes"`, and sets `CamelHttpResponseCode: 422`
- **When** the route's validator step fails with `ValidationError`
- **Then** the HTTP response has status 422, body `"shaped"`, and the
  `X-Custom` header with value `"yes"`

#### Scenario: specific clause takes precedence over wildcard

- **Given** a route whose first clause is `kind: "Io", continued: true` and
  second clause is `kind: "*", handled: true, retry: {handled_by:
  "direct:shaper"}}`, where the route continues (after the handled error)
  to a step that writes `"recovered"` into the body
- **When** a step fails with `Io("disk")`
- **Then** the route continues to the next step and the final body is
  `"recovered"`, and the `direct:shaper` endpoint receives no exchange
- **When** a step fails with `ValidationError("schema mismatch")`
- **Then** the wildcard clause runs `direct:shaper` and its output is the
  final result

#### Scenario: wildcard narrowed by message_contains

- **Given** a route whose first clause is `kind: "*", message_contains:
  "timeout", handled: true, retry: {handled_by: "direct:timeout-shaper"}}`
  and second clause is `kind: "*", handled: true, retry: {handled_by:
  "direct:generic-shaper"}}`
- **When** a step fails with `Io("connection timeout")`
- **Then** the first clause matches and `direct:timeout-shaper` output is
  the final result, and `direct:generic-shaper` receives no exchange
- **When** a step fails with `Io("disk full")`
- **Then** the first clause does not match, `direct:generic-shaper` runs
  and its output is the final result

#### Scenario: wildcard compiles from the JSON route format

- **Given** a JSON route definition with `error_handler.on_exceptions` containing
  `{"kind": "*", "handled": true, "retry": {"handled_by": "direct:shaper"}}`
- **When** the JSON route compiles
- **Then** compilation succeeds and the resulting policy matches a
  `ValidationError` and a `ProcessorError`

#### Scenario: unknown kind still rejected

- **Given** an `on_exceptions` clause with `kind: "NoSuchKind"`
- **When** the route compiles
- **Then** compilation fails with the existing unknown-kind error, and
  `"*"` is the only newly accepted kind value

