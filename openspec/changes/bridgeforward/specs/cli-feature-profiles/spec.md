## MODIFIED Requirements

### Requirement: Slim profile excludes the controllable bridge set

camel-cli SHALL provide a `slim-benchmarks` feature naming the
`--no-default-features` baseline: an http-only deployment that links the
non-optional core plumbing (http, direct, seda, log, mock, timer, file,
controlbus, container, validator, cron, and the DSL/config/core stack)
and SHALL NOT link the camel-cli-controllable optional set — kafka,
grpc, wasm, llm, mcp, mqtt, surrealdb, exec, the lsp stack (camel-lsp,
tower-lsp), the language runtimes gated by the `lang-*` features (js,
rhai, jsonpath, xpath), and seven of the eight bridge chains
(camel-component-jms, camel-component-sql, camel-component-opensearch,
camel-component-ws, camel-component-cxf, camel-xj, camel-xslt) — when
unselected. The historical `slim-http` name SHALL live exactly one
release as a forwarding alias (`slim-http = ["slim-benchmarks"]`, drop
at 0.50). `camel-component-redis` remains linked in slim through the
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
  surrealdb,exec,jms,sql,opensearch,ws,cxf}, camel-lsp, tower-lsp,
  camel-xj, camel-xslt, or the excluded language-runtime crates (js,
  rhai, jsonpath, xpath — NOT minijinja, see the requirement text)
  appears in the graph (camel-component-redis may appear via
  camel-config → camel-redis-repo; ariadne may appear: `camel lint`
  keeps it non-optional)

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
`llm`, `mcp`, `mqtt`, `surrealdb`, `exec`, `lsp`, `lang-js`,
`lang-rhai`, `lang-jsonpath`, `lang-xpath`, `lang-minijinja`, and the
eight bridge features `jms`, `sql`, `redis`, `opensearch`, `ws`, `cxf`,
`xj`, `xslt`), composing additively with `slim-benchmarks` (and, for
one release, the `slim-http` alias). Each camel-cli bridge feature
SHALL carry both halves of the capability: the camel-cli own-dependency
activation (`dep:<bridge-crate>`) and the camel-bundles gate forward
(`camel-bundles/<bridge>`), keeping the lint catalog and the
`camel_bundles::boot()` registration cascade in lockstep. For kafka,
camel-cli SHALL expose exactly two features: `kafka` (capability:
component activation plus registration in the lint registry and the
boot cascade, with librdkafka built from source) and `dynamic-linking`
(capability plus system-librdkafka linking, implying `kafka`). The
historical `cmake-build` and `kafka-static` names SHALL NOT exist as
camel-cli features. `redis-tls` SHALL imply `redis` (TLS is a
capability refinement, not a standalone surface).

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

### Requirement: camel-bundles bridge optionality

camel-bundles SHALL expose the eight bridge crates (camel-component-jms,
camel-component-sql, camel-component-redis, camel-component-opensearch,
camel-component-ws, camel-component-cxf, camel-xj, camel-xslt) as
optional dependencies activated by eight per-bridge feature keys
(`jms`, `sql`, `redis`, `opensearch`, `ws`, `cxf`, `xj`, `xslt`, each
`["dep:<bridge-crate>"]`), all members of camel-bundles' `default` set.
The mission-115 transitional carrier — the eight `dep:` entries on
`http-static` — SHALL be removed; `http-static` reverts to its pre-115
meaning (`[]`, gating only the `HttpStaticBundle` registration, which
predates the carrier). camel-cli, the sole boot consumer, SHALL forward
every camel-bundles feature key (existing forwards plus the eight
same-named bridge features), keeping lint-gate-forwarding Rules 1 and 2
green. Default-feature consumers keep the exact pre-change dependency
closure and boot registration.

#### Scenario: default closure byte-identical

- **GIVEN** camel-cli built with default features on a tree where the
  bridges are split into per-bridge features per this requirement
- **WHEN** `cargo tree -p camel-cli -e features,no-dev --prefix none --locked`
  is normalized and compared against the committed golden fixture
  (tests/fixtures/default-deptree.txt)
- **THEN** the comparison passes WITHOUT regenerating the fixture

#### Scenario: slim drops the bridges

- **GIVEN** camel-bundles resolved with `--no-default-features`
- **WHEN** its no-dev dependency closure is listed
- **THEN** none of the eight bridge crates appear via camel-bundles
  (camel-component-redis excepted while the unconditional
  camel-config → camel-redis-repo path remains — documented out-of-zone
  deferral), and enabling the per-bridge features restores exactly the
  enabled subset

#### Scenario: per-bridge activation composes

- **GIVEN** camel-bundles resolved with `--no-default-features
  --features sql`
- **WHEN** its no-dev dependency closure is listed
- **THEN** camel-component-sql is active and the other six bridges
  stay absent (camel-component-redis excepted per the unconditional
  camel-config → camel-redis-repo path — it remains in camel-bundles'
  own tree under any feature set)

#### Scenario: http-static keeps its true gate

- **GIVEN** camel-bundles with the carrier removed
- **WHEN** the `http-static` feature is enabled
- **THEN** `HttpStaticBundle` registers (the `http-static:` scheme of
  ADR-0009) and NO bridge dependency activates through it

#### Scenario: gate-forwarding lint stays green

- **GIVEN** camel-bundles' `[features]` table carrying the eight new
  per-bridge keys and camel-cli's same-named forwards
- **WHEN** `cargo xtask lint-gate-forwarding` runs
- **THEN** it exits zero (no boot-consumer forwarding violations; every
  gate forwarded, no shadow without forward)

### Requirement: gated boot registration and pool handles

camel-bundles' `boot()` SHALL cfg-gate each bridge registration site on
its own per-bridge feature (`jms`, `sql`, `redis`, `opensearch`, `ws`,
`cxf`, `xj`, `xslt`), and the `BootHandle` jms/cxf pool fields and
their shutdown drain steps SHALL be cfg-gated identically
(`jms_pool` on `jms`, `cxf_pool` on `cxf`). The always-on `BootHandle`
surface (`datasource_catalog()`, `shutdown()`,
`shutdown_with_deadline()`) SHALL compile and behave identically in
every feature combination, and no external consumer of camel-bundles
SHALL need source changes for any feature combination camel-cli can
express. camel-cli's lint-catalog registrations
(`register_builtin_components_for_lint`) SHALL mirror the same
per-bridge gates.

#### Scenario: feature-off boot succeeds

- **GIVEN** camel-bundles compiled with `--no-default-features`
- **WHEN** `boot()` runs against a fixture that references no bridge
- **THEN** boot completes, core components register, and shutdown drains
  without the gated pools

#### Scenario: default boot registers all bridges

- **GIVEN** camel-bundles compiled with default features
- **WHEN** `boot()` runs against the bundles-present fixture
- **THEN** the eight bridge components register exactly as before the
  change (camel-xj registration accompanied by camel-xslt, matching the
  pre-change transitive dependency)

#### Scenario: per-bridge subset boots coherently

- **GIVEN** camel-bundles compiled with `--no-default-features
  --features sql,jms`
- **WHEN** `boot()` runs against a fixture referencing only sql and jms
  endpoints
- **THEN** both register, the six unselected bridges stay absent from
  the registry, and shutdown drains the jms pool and the datasource
  catalog without the cxf pool
