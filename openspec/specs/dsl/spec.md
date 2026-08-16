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

