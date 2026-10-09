# cli-feature-profiles Specification (delta)

## MODIFIED Requirements

### Requirement: Slim profile excludes the controllable bridge set

camel-cli SHALL provide a `slim-benchmarks` feature naming the
`--no-default-features` baseline: an http-only deployment that links the
non-optional core plumbing (http, direct, seda, log, mock, timer, file,
controlbus, container, validator, cron, and the DSL/config/core stack)
and SHALL NOT link the camel-cli-controllable optional set — kafka,
grpc, wasm, llm, mcp, mqtt, rabbitmq, surrealdb, exec, the lsp stack
(camel-lsp, tower-lsp), the language runtimes gated by the `lang-*`
features (js, rhai, jsonpath, xpath), and seven of the eight bridge
chains (camel-component-jms, camel-component-sql,
camel-component-opensearch, camel-component-ws, camel-component-cxf,
camel-xj, camel-xslt) — when unselected. The historical `slim-http`
name SHALL live exactly one release as a forwarding alias
(`slim-http = ["slim-benchmarks"]`, drop at 0.50).
`camel-component-redis` remains linked in slim through the
unconditional camel-config → camel-redis-repo path (documented
out-of-zone deferral; the camel-cli-owned redis edge is optional as of
this change). `camel-language-minijinja` remains linked in every
profile: the workspace consumes camel-template with default features;
the in-workspace flip requires the named-feature shape whose deptree
rendering forces golden-fixture regeneration, so it stays deferred to a
follow-up change (the engine itself is optional at the source since
mission 115).

#### Scenario: slim closure excludes the controllable set

- **GIVEN** a build configured `--no-default-features --features slim-benchmarks`
- **WHEN** `cargo tree -p camel-cli --no-default-features -e no-dev`
  resolves
- **THEN** none of camel-component-{kafka,grpc,wasm,llm,mcp,mqtt,
  rabbitmq,surrealdb,exec,jms,sql,opensearch,ws,cxf}, camel-lsp,
  tower-lsp, camel-xj, camel-xslt, or the excluded language-runtime
  crates (js, rhai, jsonpath, xpath — NOT minijinja, see the
  requirement text) appears in the graph (camel-component-redis may
  appear via camel-config → camel-redis-repo; ariadne may appear:
  `camel lint` keeps it non-optional)

#### Scenario: slim build compiles and boots an http route

- **GIVEN** the slim-benchmarks release binary
- **WHEN** it boots the cold-start fixture (an http consumer route that
  must bind, plus the one-shot timer→log marker route)
- **THEN** it reaches the route-ready marker, proving the profile is
  self-coherent (no dangling forward such as `otel` without the http
  bridge, and the LSP-free binary still parses and boots routes)

#### Scenario: slim lint degrades to unverified-scheme notes

- **GIVEN** a slim build (bridge features unselected)
- **WHEN** `camel lint` runs on a route whose endpoint scheme is a
  gated bridge scheme (for example `jms:queue`)
- **THEN** the scheme surfaces as an `unverified-scheme` note — the
  same accepted graceful-degradation class as wasm/exec — and no bridge
  code is linked into the binary

### Requirement: Bridge features are individually selectable

Each controllable optional surface SHALL be re-selectable in slim and
full builds through its camel-cli feature (`kafka`, `grpc`, `wasm`,
`llm`, `mcp`, `mqtt`, `rabbitmq`, `surrealdb`, `exec`, `lsp`,
`lang-js`, `lang-rhai`, `lang-jsonpath`, `lang-xpath`,
`lang-minijinja`, and the eight bridge features `jms`, `sql`, `redis`,
`opensearch`, `ws`, `cxf`, `xj`, `xslt`), composing additively with
`slim-benchmarks` (and, for one release, the `slim-http` alias). Each
camel-cli bridge feature SHALL carry both halves of the capability: the
camel-cli own-dependency activation (`dep:<bridge-crate>`) and the
camel-bundles gate forward (`camel-bundles/<bridge>`), keeping the lint
catalog and the `camel_bundles::boot()` registration cascade in
lockstep. The `rabbitmq` capability feature SHALL likewise carry both
halves: the camel-cli own-dependency activation
(`dep:camel-component-rabbitmq`) and the camel-bundles gate forward
(`camel-bundles/rabbitmq`). For kafka,
camel-cli SHALL expose exactly two features: `kafka` (capability:
component activation plus registration in the lint registry and the
boot cascade, with librdkafka built from source) and
`dynamic-linking` (capability plus system-librdkafka linking,
implying `kafka`). The historical `cmake-build` and `kafka-static`
names SHALL NOT exist as camel-cli features. `redis-tls` SHALL imply
`redis` (TLS is a capability refinement, not a standalone surface).

#### Scenario: slim plus one bridge resolves that bridge only

- **GIVEN** a build configured `--no-default-features --features slim-benchmarks,grpc`
- **WHEN** the dependency graph resolves
- **THEN** the grpc stack (camel-component-grpc, tonic) is present and
  the remaining controllable set stays excluded

#### Scenario: slim plus one legacy bridge resolves that bridge only

- **GIVEN** a build configured `--no-default-features --features slim-benchmarks,sql`
- **WHEN** the dependency graph resolves
- **THEN** camel-component-sql (with its datasource stack) is present
  and the remaining controllable set stays excluded — the other six
  bridges absent, camel-component-redis retained through the
  unconditional camel-config → camel-redis-repo path

#### Scenario: dynamic-linking implies kafka capability

- **GIVEN** the camel-cli feature table and a build configured
  `--no-default-features --features dynamic-linking`
- **WHEN** the feature list is resolved and the dependency graph
  renders
- **THEN** the `dynamic-linking` feature list includes `kafka`, and the
  closure contains camel-component-kafka while the remaining
  controllable set stays excluded (feature-forwarding edges do not
  render in cargo tree, so the implication is asserted at the
  feature-table level)

#### Scenario: removed kafka feature names fail to resolve

- **GIVEN** a build request using `--features cmake-build` or
  `--features kafka-static`
- **WHEN** cargo resolves the feature graph
- **THEN** resolution fails with an error naming the unknown feature

## ADDED Requirements

### Requirement: rabbitmq capability ships in the regular flavor

camel-cli SHALL expose a `rabbitmq` capability feature activating the
native AMQP 0-9-1 component: `rabbitmq = ["dep:camel-component-rabbitmq",
"camel-bundles/rabbitmq"]`. The `flavor-regular` marker SHALL include
`rabbitmq` (like `jms`), so the default build registers the
`rabbitmq:` scheme, and the default-closure golden fixture SHALL be
regenerated to carry camel-component-rabbitmq and its dependency tree.
`flavor-full` inherits rabbitmq through `flavor-regular`.

#### Scenario: default build registers the rabbitmq scheme

- **GIVEN** a camel-cli build with default features (flavor-regular)
- **WHEN** the boot cascade registers bundles
- **THEN** the `rabbitmq:` scheme resolves in the component registry

#### Scenario: default closure golden carries the new tree

- **GIVEN** the regenerated `default-deptree.txt` golden
- **WHEN** the feature-profile test compares the resolved default
  closure against it
- **THEN** the comparison passes and the golden contains
  camel-component-rabbitmq package lines

#### Scenario: regular closure contract requires rabbitmq

- **GIVEN** the `REGULAR_REQUIRED_PREFIXES` list in
  `crates/camel-cli/tests/feature_profiles.rs`
- **WHEN** the flavor-regular closure test runs
- **THEN** the list contains `camel-component-rabbitmq v` and the
  closure test fails if the crate is absent from the regular build

#### Scenario: slim plus rabbitmq resolves rabbitmq only

- **GIVEN** a build configured `--no-default-features --features slim-benchmarks,rabbitmq`
- **WHEN** the dependency graph resolves
- **THEN** camel-component-rabbitmq (with lapin and the rustls ring
  stack) is present and the remaining controllable set stays excluded
