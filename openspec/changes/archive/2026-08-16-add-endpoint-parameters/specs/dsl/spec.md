## ADDED Requirements

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
